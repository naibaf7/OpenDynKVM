#!/usr/bin/env python3
"""
OpenDynKVM -- autorouter (4 layers, KiCad 10 pcbnew API).  Adapted from the OpenRower router.

Grid A* router with exact copper geometry: for every grid point it keeps the distance to the
nearest copper of the two nearest *different* nets, so a candidate track/via is checked against
the real pad/track shapes (not a raster).  Stack-up: F.Cu signals, In1.Cu GND plane, In2.Cu +3V3
plane, B.Cu signals.  Tracks run on F.Cu and B.Cu; slow signals and power may also use In2.Cu
(the +3V3 pour flows around them, never under a B.Cu USB pair).  In1.Cu stays a solid GND plane.
Vias go through all four layers.

Order of work:
  1. fixed copper: USB-C D+/D- row joins, straight runs through the flow-through ESD parts,
     via arrays in the QFN exposed pads
  2. USB differential pairs: a 'fat' centre line (track + gap + track) is routed with A*, then
     split into two coupled tracks (45-degree turns only, via pairs for layer changes, a swap
     macro where the pin order of the two ends requires it); the AT32 SPI, then the full-speed Hub-0 links
  3. a GND / +3V3 via next to every plane pad, before any signal can wall it in
  4. power nets as wide tracks, then every other signal (rip-up and reroute when blocked)
  5. plane pads that lost their via, stitching vias, zones on all four layers

The A* inner loop is C (tools/astar.c, compiled on demand, loaded with ctypes; ROUTE_PY_SEARCH=1 uses
the Python reference).  Step 4 plans the next nets on threads (ROUTE_WORKERS, default up to 16) and
commits them in order; a plan whose search region saw a change is planned again, so the result is
identical to routing one net after the other.

Run from hardware/ after tools/gen_pcb.py:   uv run tools/route.py [--live[=PORT]]
Writes kvm.kicad_pcb in place (routing is replaced, footprints untouched).  Log: tools/route.log.
--live serves tools/route_view.html on http://localhost:8765/ (all copper layers, redrawn as routing goes).

Debug hooks for one net without a full run (tools/check_copper.py still checks the result):
  --nets a,b,c           route only these nets on the placed board (seconds, not 21 minutes)
  ROUTE_NETS_PLANES=1    with --nets: place the GND/+3V3 fan-out vias first, as the full run does - without it a
                         pad whose only escape is a via looks blocked (the plane vias are missing)
  ROUTE_NETS_BEFORE=a,b  with --nets: route these nets before the --nets list, in the full run's order, so a net that
                         is routed late there is not favourably routed here
Neither hook changes a full run.
"""

import collections
import ctypes
import functools
import heapq
import math
import json
import os
import re
import subprocess
import sys
import time
import types

import numpy as np
import pcbnew
from scipy import ndimage
from scipy.sparse import coo_matrix
from scipy.sparse.csgraph import connected_components

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import board_stackup  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')


def load_kernel():
    """tools/astar.c, compiled on demand: the A* search in C (ctypes drops the GIL while it runs)."""
    src = os.path.join(HERE, 'tools', 'astar.c')
    lib = os.path.join(HERE, 'tools', 'build', 'astar.so')
    if os.environ.get('ROUTE_PY_SEARCH') or not os.path.exists(src):
        return None
    if not os.path.exists(lib) or os.path.getmtime(lib) < os.path.getmtime(src):
        os.makedirs(os.path.dirname(lib), exist_ok=True)
        subprocess.run(['cc', '-O2', '-shared', '-fPIC', '-o', lib, src], check=True)
    k = ctypes.CDLL(lib).astar
    P, I, D, L = ctypes.c_void_p, ctypes.c_int, ctypes.c_double, ctypes.c_int64
    k.argtypes = [I, I, I, P, P, P, P, P, P, P, P, L, I, I, I, I, I, D, D, I, P, D, L, P, L, P, P, P, D, P, D]
    k.restype = L
    return k


KERNEL = load_kernel()
try:
    from shapely.geometry import LineString as OffsetLine     # loop-free pair offsets (offset_polyline)
except ImportError:
    OffsetLine = None

G = 0.05                       # grid pitch, mm
X0, Y0, X1, Y1 = 100.0, 100.0, 220.0, 180.0
NX = int(round((X1 - X0) / G)) + 1
NY = int(round((Y1 - Y0) / G)) + 1
CLR = 0.15                     # copper-copper clearance
EDGE_CLR = 0.5                 # copper to board edge
# tracks and via sites keep this far from the board edge, except within EDGE_PAD_R of a pad of the same net. The
# strip along the edge stays free, so parts mounted there (the status LEDs, the USB connectors) can still reach
# their pins and escape: a +5 V rail pinned against the front edge used to block the LED data line completely.
EDGE_KEEP = 1.6
EDGE_PAD_R = 1.2
HOLE_CLR = 0.25                # copper to NPTH hole, hole to hole
THERMAL_GAP, ZONE_MIN_W = 0.3, 0.2   # the pours' thermal-relief gap round through-hole pads, and their minimum width
# Room for a GND through-hole pad's thermal spokes on the outer layers (Board.__init__): other nets keep CLR from the pad
# grown by this much, so the pours (0.2 mm clearance) can lay THERMAL_GAP and a ZONE_MIN_W ring all round it
THT_SPOKE_ROOM = THERMAL_GAP + ZONE_MIN_W + 0.05
VIA_D, VIA_DRILL = 0.6, 0.3
EPS = 0.005
W_SIG, W_GND = 0.15, 0.3
RMAX = 0.9                     # distance fields are exact up to this radius
PAIR_W, PAIR_S = 0.15, 0.15    # 90 ohm differential on JLC04161H-3313 (L1 over L2)
PAIR_O = (PAIR_W + PAIR_S) / 2           # offset of each line from the centre line
PAIR_FAT = 2 * PAIR_W + PAIR_S           # width of the fat centre line
# The fat line is what the pair's *search* occupies; the two offset lines are what gets emitted.  A fan-out band, a
# corridor and the inside of a corner can be wide enough for the centre line and too narrow for the offset lines,
# which is exactly how a pair comes out folded ("-0.048 mm apart at (110.56,144.03)").  ROUTE_PAIR_FAT lets the fat
# line be widened (it does not change the emitted copper, only how much room the search claims) so it takes the
# rounder path.  Off by default: it also narrows every corridor the pairs have to fit through.
PAIR_FAT = float(os.environ.get('ROUTE_PAIR_FAT', PAIR_FAT))
PAIR_VIA_O = 0.45                        # via offset from the centre line at a pair layer change
PAIR_VIA_R = PAIR_VIA_O + VIA_D / 2      # radius kept free around a pair via site
# copper thickness (mm) of the routing layers F.Cu, B.Cu, In2.Cu (LAYERS order), from the board's own stack-up
# (gen_pcb.py STACKUP): JLC04161H-3313 is 1 oz outside but 0.5 oz inside, so a rail on In2.Cu needs 2.3 x the width
_CU = {n: t for n, kind, t, _ in board_stackup(PCB) if kind == 'copper'}
if not _CU:
    sys.exit(f'{PCB} has no stack-up: uv run tools/gen_pcb.py --stackup')
CU = tuple(_CU[n] for n in ('F.Cu', 'B.Cu', 'In2.Cu'))
RHO_CU = 1.72e-5               # copper resistivity, ohm mm


def rail_width(i, t, rise=20.0, drop=None):
    """Track width (mm) for a current i (A) on copper t mm thick.  Heating: IPC-2221's external chart at a `rise` C
    rise, A = 0.048 dT^0.44 A_cu^0.725 (A_cu in mil^2), for In2.Cu too - between the two planes it runs no hotter
    than an outer track (IPC-2152), so the internal chart's k = 0.024 would only double its width for nothing.
    Drop: with `drop` (mV per mm at i), also wide enough to lose no more than that."""
    w = (i / 0.048 / rise ** 0.44) ** (1 / 0.725) / (t / 0.0254) * 0.0254
    if drop:
        w = max(w, 1000 * i * RHO_CU / (t * drop))
    return round(max(W_SIG, w), 2)
# each rail's width follows the current it carries between its source and its targets (rail_width, 20 C rise): as
# wide as the source can drive and the targets draw, and no wider - on each layer for that layer's copper
POWER_CURRENT = {'+5V': 2.24,                      # TPS2121 output: 3 x 0.5 A port switches + logic and LEDs (2.4 A limit)
                 'VBUS_KB': 2.24, 'VBUS_MS': 2.24, 'VBUS_OT': 2.24,  # TPS2553 -> USB-A VBUS (0.5 A load + inrush)
                 'VBUS_PC1': 1.96, 'VBUS_PC2': 1.96,                 # PC port -> LM66200: that PC's whole branch
                 'VBUS_PWR': 1.96, 'VBUS_PCS': 1.96,                 # adapter / PC-OR output into the TPS2121
                 'SW_3V3': 1.96,                                     # TLV62569 switch node
                 '+1V1': 1.67, 'RP_VREG': 1.36}                      # TLV73311 core supply / RP2354B POR, BOD
# the +5V feed runs from the TPS2121 in the north to the three port switches in the south, 60-110 mm, and its drop
# comes straight off the ports' VBUS (jlcpcb.md section 5, tools/dc_drop.py): sized to lose at most 1 mV per mm at its
# full current too, which the heating rule alone left at 3.4 mV/mm on In2.Cu (148-206 mV at the ports, rev 11)
RAIL_DROP = {'+5V': 1.0}
# no track wider than the distance fields can check: a search sees foreign copper only within RMAX of it, so a
# track whose clearance (CLR + half its width) reached past RMAX was laid over vias 0.9-1.4 mm off its centre line -
# measured, rev 12's first run: a 2.53 mm +5V track on In2.Cu over a KEY and two GND vias (DRC: 3 shorts)
RAIL_W_MAX = round(2 * (RMAX - CLR - EPS - G), 2)      # one grid step of margin: field windows end on the grid
RAIL_LW = {n: tuple(min(rail_width(i, t, drop=RAIL_DROP.get(n)), RAIL_W_MAX) for t in CU)
           for n, i in POWER_CURRENT.items()}
POWER = {n: lw[0] for n, lw in RAIL_LW.items()}     # a rail's width on F.Cu; RAIL_LW per layer (layer_width)
# a rail keeps its width the whole way. It necks down only at a pad narrower than the rail (within PAD_NECK_R of
# its own pad) or through a dense IC's own via fan-out, whose pin is otherwise unreachable at full width; never in
# between, so a wide stretch is never thinned just to squeeze past another net's via or pad
PAD_NECK_R = 0.5
NECK_WIDTHS = (0.5, 0.4, 0.3, 0.25, 0.2, W_SIG)   # widths a rail may step down to at a pad (the widest that fits)
# light loads on a rail (the status LEDs, their buffer and decoupling on +5 V): they join the rail after all its heavy
# loads (so no heavy load hangs off a thin branch), with branches LIGHT_W wide
LIGHT_LOADS = {'+5V': ('D150', 'C150', 'U1501')}
LIGHT_W = 0.3
PLANES = {'GND': 'In1.Cu', '+3V3': 'In2.Cu'}
LAYER_COST = (1.0, 1.1, 1.3)   # per-step cost factor F.Cu, B.Cu, In2.Cu (B.Cu nearly as cheap: a full signal layer)
VIA_COST = 30.0                # in grid steps
TURN_COST = {1: 3.0, 2: 8.0}   # 45 deg, 90 deg (in grid steps); sharper turns forbidden
PAIR_SKEW_MAX = 1.2            # mm: a pair's skew budget, as check_usb_si.py SKEW_MAX and check_copper.py PAIR_SKEW_MAX
KINK_SKEW_MAX = 0.45           # mm of pair skew a lane leg may take from its own corners before the leg is searched
#                              again with both layers: a turn of exterior angle phi costs the pair PAIR_O*tan(phi/2) a
#                              side (outside line longer, inside shorter), so a zig-zag of two 45s is near this and a
#                              sharp kink is far past it
PAIR_VIA_COST = 18.0           # what a via pair costs a pair's leg search (grid steps).  A layer change is about
#                              0.45 mm of skew whatever the path takes, so it must be *cheaper* than a sharp turn:
#                              measured, H0_OT_DM carried 4.72 mm of skew from one kink at (178, 141) while every
#                              straight part of that link matched its mate to 0.05 mm.  This is the trade the router
#                              gets wrong when a via pair is priced at 60 and a two-45 kink at 6.
DETOUR_MAX = 1.25              # a leg at most this much longer than the straight distance between its ends before the
#                              two-layer candidate is tried as well.  A detour is the other half of the same trade: a
#                              leg that goes far out of its way (measured: H0_OT's north leg ran east to x 188, almost
#                              to AT32 #2, before it turned down) should take a via pair and go straight instead.
HUG_COST = 0.6
EDGE_COST = 4.0                # extra cost per step on a cell in the edge strip: the search keeps off the board edge
#                               (the strip stays free so edge-mounted parts can still reach their pins and escape)
VIA_DEBUG = bool(os.environ.get('ROUTE_DEBUG_VIA'))    # report why a via-pair site has no room
STOP_IF_USB_FAILS = True       # Stop before the signal phase when any USB link could not be routed as a pair: a missing
#                              USB connection cannot be fixed by routing the rest of the board (minutes), and carrying
#                              on is what produced "complete" boards with clipped USB copper.  Fix the link instead.
DESPERATE = False              # set for the last-resort pass over the nets still unrouted: full-board windows, no
#                              search heuristic (h_weight 1.0) and many more expansions, so a roundabout route is
#                              found rather than given up on
FAN_COST = 3.0                 # extra cost per step in a chip's fan-out band for a net with no pin on that chip.
# This is what makes a route *respect* a chip's fan-out plus its reserved via space instead of crossing it: the band
# cells are not walls (a wall starves the search), they cost three steps' worth, so the router takes the band only
# where it must.  It used to be 0 - off - because the hand-planned HS lanes (LANES / H0_LANES) cross the AT32s' and
# hubs' bands by design and the lane search starved when those cells cost more; those searches now pass fan_cost=0
# (they follow a planned lane, so the band is not theirs to lose), while every general search pays it (rev 9,
# 2026-10-26: the board's remaining failures were all pins whose fan-out room another net had taken).
WINDOW = 12.0                  # routing window margin around a connection, mm (grown on failure)
PAIR_WINDOW = 3.0              # the same margin for a pair's lane *legs*.  A leg is a directed piece of a hand-planned
#                              lane, so its first search must keep close to the line its waypoints describe; the general
#                              12 mm let one wander 8 mm off in the middle of a link - measured, H0_OT's descent ran east
#                              to x 186.05 (exactly where AT32 #2's fan-out band starts) and then came back.  The grow
#                              ladder still widens this when the tight window has no answer.
H_WEIGHT = 1.5                 # weighted A*: long detours around congestion stay cheap to search (1.0 timed out)
WORKERS = min(16, os.cpu_count() or 1)   # threads planning the next nets in the queue (ROUTE_WORKERS overrides)
WORKERS = int(os.environ.get('ROUTE_WORKERS', WORKERS))
RIP_LIMIT = 5                  # times one net may be ripped up for others
LAYERS = (pcbnew.F_Cu, pcbnew.B_Cu, pcbnew.In2_Cu)   # routing layers 0, 1, 2; In1.Cu stays a solid GND plane
NL = len(LAYERS)
IN2_SIGNALS = True             # slow signals and power may also run on In2.Cu, the +3V3 plane flows around them
IN2_REF_KEEP = 0.6             # In2.Cu stays solid this far from a B.Cu pair line (its return path)
DENSE_ICS = {'U501': 2.5, 'U701': 1.5, 'U801': 1.5, 'U901': 1.5, 'U1001': 1.5, 'U1101': 1.5,   # courtyard margin, mm
             'U1202': 0.8, 'U1302': 0.8, 'U1402': 0.8, 'U601': 0.8}           # the four USB switches (0.4 mm pitch)
QFN_INSIDE = ('U501', 'U701', 'U801', 'U901')   # no F.Cu tracks between the pin ring and the exposed pad
# via-free band beyond the pin tips, mm (per side l/r/t/b where given); the RP2354B's north side faces Hub-0 closely
RP_ZONE = {'l': 0.8, 'r': 0.8, 't': 0.8, 'b': 0.8}   # radial-only escape, then the planned staggered fan-out
# staggered via fan-out (dense_fanout) for every used pin of the dense ICs: vias in a near and a far row beyond the pin
# tips, alternating along the side
FANOUT_STEP = 0.8             # via positions along a side: 0.8 mm steps, alternating near / far row, so vias in a row
#                               are 1.6 mm apart (two tracks pass between them)
# Floor of the step where a side has more pins than room: two neighbouring lanes can only *both* carry a via when they
# are CLR + W_SIG/2 + VIA_D/2 = 0.525 mm apart (a via must clear the next lane's copper).  Spread closer than that and
# the site search can only give a via to some of the pins - U501's east row (pins 44-49 at 0.4 mm pitch) is the case:
# at 0.45 mm it went without vias at 47/50/51, and at 0.525 the whole row got one.
VIA_LANE_SEP = CLR + W_SIG / 2 + VIA_D / 2
FANOUT_STEP_MIN = 0.45        # closer only where the depth runs out (FANOUT_IC sets the starting step per IC and side)
FANOUT_ROW_GAP = 0.7           # far row this much deeper than the near row
FANOUT_SIDES = 'tbrl'          # north, south, east, west (after the USB and SPI pins)
_sides = lambda l, r, t, b: {'l': l, 'r': r, 't': t, 'b': b}
# per IC and side: escape zone beyond the pin tips (radial path), deepest via row, and how far the vias may spread
# beyond the side's end pins into the corner space. RP2354B: Hub-0 north, the Hub-2 B.Cu band south
# beyond the side's end pins into the corner space, and the starting via step along the side. RP2354B: Hub-0 and its
# lanes' gaps north, the Hub-2 B.Cu band and Hub-0's mouse lane (south-west corner) south
FANOUT_IC = {'U501': dict(zone=RP_ZONE, max=_sides(4.6, 4.6, 3.0, 3.45), end=_sides(2.5, 2.5, 1.0, 1.0),
                          step=_sides(0.7, 0.7, 0.6, 0.6),
                          lanes={'t': [161.35, 153.6]})}    # 161.35: Hub-0's Mouse pair comes up through the north
# fan-out here.  153.6 (rev 8): the RP2354B's own pair leaves its 27R series resistors (R503/R504) 1 mm east of the row
# and has to climb past it before turning west to pins 67/66, whose own lanes it then descends; without the lane the
# fan-out's easternmost vias sit in that climb.
PAIR_LANE = 0.7                # fan-out vias keep this far (along the side) from a lane a pair runs through
# Pins that get no planned via, only a lane straight out of the pin, which the fan keeps clear (as for bus stubs); the
# signal phase routes them, on F.Cu as far as it can.  None since 0.16.0: the Bluetooth module's UART pins (U501.36/37)
# were tried here, and as fixed lanes they split the south row's plan in two and pushed pins 21-35's via sites west
# (MS_SEL1 and RP_SWCLK lost theirs).  They get planned sites like any pin now, held for them until they route, last.
FANOUT_SURFACE = set()
# Pins the dense fan-out leaves alone: their via comes from the plane fan-out (step 3), round whatever is there by then.
# U501.29 (+3V3) sits beside the crystal input (pin 30): planned, its via sat on the crystal's way down past R502, the
# crystal lifted it ("lifted 3 plane fan-out items for RP_XIN") and in one run the pin was left with no via at all (DRC:
# U501.29 unconnected).  Left alone, the plane fan-out ties it inwards along the pin row, as on 0.15.0.
FANOUT_SKIP = {'U501.29'}
# Crystal lines laid as fixed copper before the dense fan-outs are planned (reserve_xtal_lanes), so the plans go round
# them.  The RP2354B's crystal input has to cross the fan-out of pins 33-40 to reach Y501, and 0.15.0 threaded it
# through the room pins 36-39 left; 0.16.0 put the Bluetooth module's UART on pins 36/37, and in full runs the crystal,
# routed after the fan-out was planned, found no way through ("no way through at all"), or found one and shut a
# neighbour in - every via spot west of R502 boxed in MS_FLT (pin 28), every one east of it RP_SWCLK (pin 33).  This
# path drops to B.Cu right under pins 29/30, above R502 (0.24 mm off pin 29, 0.18 mm off R502), runs on B.Cu along
# y 141.85 between the pin row and the via rows (y 143.95 / 144.65), so every south pin's lane crosses over it, comes up
# at (152.75, 141.85) and goes on F.Cu to Y501: 13.5 mm, two vias (0.15.0: 15.5 mm, two vias).  Pins 29-32 need no
# outward escape (29 ties inwards, 30/31 are the crystal, 32 is +1V1), so it takes no other pin's way out.  If copper (a
# pair) is in its way it is not laid, and the line is routed like any other.
XTAL_LANES = {'RP_XIN': [
    ('seg', (147.80, 141.35, 147.70, 141.85, W_SIG, 0)),
    ('via', (147.70, 141.85)),
    ('seg', (147.70, 141.85, 152.75, 141.85, W_SIG, 1)),
    ('via', (152.75, 141.85)),
    ('seg', (152.75, 141.85, 159.30, 141.85, W_SIG, 0)), ('seg', (159.30, 141.85, 159.75, 142.30, W_SIG, 0))]}
_hub_fan = dict(zone=_sides(0.4, 0.4, 0.4, 0.4), max=_sides(3.6, 3.6, 3.6, 3.6), end=_sides(2.5, 2.5, 2.5, 2.5),
                step=_sides(*[FANOUT_STEP] * 4))
FANOUT_IC['U701'] = dict(_hub_fan, max=_sides(2.4, 3.6, 3.6, 3.6), lanes={'b': [163.75, 167.0], 'l': [118.0]})
# Hub-0's west side is shallower than the other three (2.4 mm): the service switch sits 3 mm off that row (rev 8,
# gen_pcb.py PLACEMENT), so the deepest via row must stay clear of its pads.
#  - 'b' 163.75 is the midpoint of the hub's port-4 pins 9/8, whose stub to R707/R708 is a pair like any other from rev 8
#    on: with the lane declared the south fan-out keeps PAIR_LANE (0.7 mm) off that corridor, so the pair can leave the
#    pins southwards.  167.0 clamps the fan-out's eastern end (its corner spread otherwise reaches x 167.0) so the pair
#    can climb back north to the resistors' escape: it goes south past the row, east, then north at x 167.0.
#  - 'l' 118.0 is the service pair's crossing (U601's USB1 pins to the service ESD, U301): that pair has to pass *through*
#    the west fan-out row, and the hub's other lanes cross theirs the same way (U801/U901's upstream lanes).  The row's
#    vias then stay at y >= 118.7 and the pair crosses at 118.0, north of the hub's own west pads.
for _h, _x in (('U801', 115.0), ('U901', 205.0)):  # Hub-1 / Hub-2: the upstream pair leaves its ESD straight south,
    # then runs west *through* the hub's north fan-out row to reach the hub's west-side pins.  Those three lanes
    # (the two stubs and the turn) stay free of fan-out vias, so the pair keeps its way and the CFG0/CFG1 pins beside
    # them still get theirs (they were the pins with no site)
    FANOUT_IC[_h] = dict(_hub_fan, lanes={'t': [_x - 2.15, _x - 1.45, _x - 0.75, _x - 0.05, _x + 0.65, _x + 0.95]})
# the three port switches (TS3USB3031, WQFN-12 at 0.4 mm pitch): their SEL0/SEL1 lines are routed after the HS pairs,
# so they need their own planned way out or the pairs close it off
FANOUT_IC.update({r: dict(zone=_sides(0.3, 0.3, 0.3, 0.3), max=_sides(2.4, 2.4, 2.4, 2.4), end=_sides(1.4, 1.4, 1.4, 1.4),
                          step=_sides(0.5, 0.5, 0.5, 0.5)) for r in ('U1202', 'U1302', 'U1402')})
FANOUT_IC.update({a: dict(zone=_sides(0.4, 0.4, 0.4, 0.4), max=_sides(3.5, 3.5, 3.5, 3.5), end=_sides(2.5, 2.5, 2.5, 2.5),
                          step=_sides(*[FANOUT_STEP] * 4)) for a in ('U1001', 'U1101')})          # AT32F405 LQFP-48
FANOUT_IC['U1001']['max']['b'] = 1.2    # AT32 #1's crystal side: its via row must stay clear of the Hub-0 keyboard
FANOUT_IC['U1101']['max']['t'] = 2.8    # lane's layer-change site north of the AT32 (tools/route.py H0_LANES)
# the service switch: the same WQFN-12 part as the three port switches from rev 8 on (every pair on adjacent pins), so it
# takes the same shallow fan-out - a via just beyond each pin, and the pair lanes leave straight from the pads
FANOUT_IC['U601'] = dict(zone=_sides(0.3, 0.3, 0.3, 0.3), max=_sides(2.4, 2.4, 2.4, 2.4), end=_sides(1.4, 1.4, 1.4, 1.4),
                         step=_sides(0.5, 0.5, 0.5, 0.5))
FANOUT_SPOT_R = 1.6            # a pin whose planned path is blocked: any via site this close to the planned one ...
FANOUT_VIA_GAP = 0.9           # ... at least this far from the IC's other fan-out vias (near / far row neighbours)
PIN_VIA_FREE = {'U501': RP_ZONE, 'U701': 0.3, 'U801': 0.3, 'U901': 0.3}
# escape zone in front of each side of a dense QFN, this deep beyond the pin tips: F.Cu moves there are radial only
# (straight out from that side; no bends, no sideways runs) and each unrouted pin's lane is reserved for its net
QFN_FANOUT = {'U501': RP_ZONE}
# AT32 #1's SPI leaves the RP in the opposite order to its own pins: the bus runs straight out past the fan-out comb
# and the cap ring first, so its four crossings happen where vias fit
FANOUT_LEN = {n: 2.65 for n in ('AT1_MISO', 'AT1_CS', 'AT1_SCK', 'AT1_MOSI')}
FANOUT_LEN['VSNS_PWR'] = 3.40  # RP pin 49 to R304.1 (divider leg), pad edge to just past R304.1's centre
# VSNS_PWR's only way out is that one straight run: the pin sits in the 0.4 mm east row with its neighbours 0.4 mm away
# and R304 3.4 mm east, so there is no fan site to compete for.  As a FANOUT_LEN net the run is fixed copper from the
# start (like the AT1 bus) and the via fan keeps 0.6 mm off it - held the other way round, rev 9's site search put the
# pin's reservation 4.8 mm east at (157.85, 136.95) and that lane ran 0.25 mm from U501.48's escape, which cost
# AT2_NRST its site; its net then failed at U501.48 ("no way through at all", both retry rounds).
# VBUS_PWR first: it is the one rail whose connection is a short local link in a closed pocket - the PWR-port ESD's
# VBUS pad (U301.5) sits between the SVC pair's two exit stubs, with HUB0_RST's B.Cu run north of it and VBUS_PC1's
# B.Cu/In2 run between it and its own decoupling (D302, C301, R303).  Routed third (after the front ports' through-
# rails) it had no way out at all: measured on rev 9, VBUS_PWR failed at U301.5 with "in the way: VBUS_PC1" in every
# pass, because VBUS_PC1's tail (x 154.65-170.2 at y 115.95) is exactly the corridor pad 5 has to cross.  Routed
# first, its 5.5 mm link takes that corridor and the two long through-rails go round it - the same rule the signal
# phase applies with "short local nets before the long ones that pass by".
POWER_ORDER = ['VBUS_PC1', 'VBUS_PWR', 'VBUS_PC2', 'VBUS_PCS', 'SW_3V3', 'RP_VREG', '+1V1',   # into the power path
               '+5V', 'VBUS_KB', 'VBUS_MS', 'VBUS_OT']                                       # then out of it
NOROUTE_BODY = {'J101': (0,), 'J201': (0,), 'J301': (0,), 'J1201': (0,), 'J1301': (0,), 'J1401': (0,),
                'SW501': (0,)}
EP_PADS = {('U501', '81'): 3, ('U701', '37'): 3, ('U801', '37'): 3, ('U901', '37'): 3}   # n x n via arrays
# Footprints whose GND pads join their own layer's GND pour instead of taking a via each (0.16.0): the Bluetooth module
# U1601 on B.Cu.  Its 22 ground pads (a row of 13 along the antenna end, four corners, the 3 x 3 centre grid) lie in
# the B.Cu GND zone, which the stitching vias round the module tie to In1.Cu; a via per pad would put 22 drills under
# the module, and the row along the antenna end would sit at the antenna keep-out's edge.  Its 3V3 pad takes a plane
# via like any other; the signal pads leave on B.Cu (the module needs no fan-out reservation: 0.8 mm pitch, and every
# failed 0.16.0 route of its lines failed at the RP2354B's end, never here).
POUR_PADS = {'U1601'}
# parts whose pads need a reserved way out (a straight run plus a via site) before the signals are routed, the same
# as a dense IC's pins: the edge-mounted parts would otherwise have no escape left, and the 0.5 mm-pitch power path
# is closed off by the 0.5-0.6 mm rails around it
PAD_ESCAPE = {
    'edge': ('J101', 'J201', 'J301', 'J1201', 'J1301', 'J1401',                    # USB-C and USB-A connectors
             'D1501', 'D1502', 'D1503', 'D1504', 'D1505', 'D1506',                 # key and status LEDs
             'C1501', 'C1502', 'C1503', 'C1504', 'C1505', 'C1506', 'C1507', 'C1508'),   # their decoupling
    'radial': ('U401', 'U402', 'U403', 'U404',                                    # power path (TPS2121 is 0.5 mm pitch)
               'U1201', 'U1301', 'U1401',                                         # port ESDs: their VBUS pad needs the
               #                                                                    via the port rail leaves by
               'U1203', 'U1303', 'U1403',                                         # port VBUS switches (EN, FLT, ILIM)
               'R1004', 'R1005', 'R1104', 'R1105'),                                # AT32 CC series resistors: their pads
    #                                                                               sit in the AT32 <-> RP2354B SPI
    #                                                                               corridor, where the SPI closes them in
}
# the hubs' strap / control pins on the north row: their fan-out room is taken by the upstream pair's westward run
# (a hand-planned HS lane that runs right over them).  Reserving their escapes before that pair is routed breaks the
# pair instead, so the table stays empty and those pins keep leaving on F.Cu (dense_fanout still gives them a plan);
# filling it in is only safe together with re-planning UPSTREAM_LANES.
PAD_ESCAPE_PINS = {}
# pads whose only way out is into their own part: a straight run to a via site inside the part's own pad outline.
# Each entry is (start offset from the pad centre, via offset).  U301.5 (the PWR port's VBUS pad) is in the middle of
# the ESD's bottom row: pads 4 and 6 are 0.35 mm either side (no track fits between them on any layer), south of it the
# SVC pair's two exit stubs converge to 0.3 mm, and east/west are those same pads.  North, in the 0.96 mm slot between
# the ESD's two pad rows, is the only via site - and the GND via of pad 2 wants the same spot (plane_fanout's natural
# "between the pair pins" place).  Measured on rev 9, VBUS_PWR failed at U301.5 in every pass ("no way through at all")
# without it - the trap is pad-level, not the width of the strip, so the 1.5 mm the ESD moved north does not remove it.
# With the escape but VBUS_PWR routed first, VBUS_PC1 lost the strip south of the ESD instead (5 of 9 pads unrouted).
# POWER_ORDER therefore gives VBUS_PC1 its corridor *first*, after which VBUS_PWR leaves the slot on B.Cu, north of
# the ESD, and GND's plane via keeps the west half of the slot (0.65 mm from VBUS_PWR's).
PAD_ESCAPE_INWARD = {('U301', '5'): (0.28, -0.04, 0.40, -0.64),
                     # rev 9, 2026-10-26: the two PC ports' ESDs escape their middle pad the same way - *north* into the
                     # slot between the pad rows - instead of south into the pocket, which is the corridor the pair
                     # needs for its straight run out of pads 4/6 (measured: with the escape south, the bottom run was
                     # 0.76 mm against 1.31 mm above the ESD; the pair's long run was rejected and fell back).
                     ('U101', '5'): (0.28, -0.04, 0.40, -0.64),
                     ('U201', '5'): (0.28, -0.04, 0.40, -0.64)}
# pads whose only way out is a via in a pocket that an *earlier* net can close before the pad's own net is routed: the
# same reservation, but the site is outside the pad (south of it), so a straight run plus the via is all this table
# needs.  Both entries were measured on the rev-9 full23 board, where the net failed at that pad in every pass and in
# both retry rounds.
#   U201.5 (VBUS_PC2): the PC2 ESD is the same footprint and rotation as U301, so pad 5 sits in the middle of the
#   south row with the pair's pass-through pads 4/6 (0.35 mm either side) and no room sideways; north, in the slot
#   between the pad rows, sits the GND via of pad 2 (204.80, 112.95) 1.2 mm away and the pair's through links at
#   x 204.05 / 205.95 0.55 mm either side (a 0.6 via needs 0.525), so the slot is not a site.  South, in the pocket
#   between the pair's two 45-degree exit stubs, is one: (205.00, 115.05), 0.601 mm from both stubs (0.226 clear).
#   The twin pad U101.5 gets exactly that via and routes ((115.00, 115.15), 0.601 mm too), but at U201 HUB2_RST's
#   B.Cu run (204.05, 113.95)-(205.40, 115.30) crosses the pocket 0.07 mm from the site - and HUB2_RST reaches
#   U501.44, so it is routed with the RP2354B's nets *before* power.  The soft search in find_blockers cannot take a
#   single step there, so it reported nothing rippable and the net was left unrouted ("in the way" then named only
#   the pair).  Held from the start, the site makes HUB2_RST plan round it (as HUB1_RST already does at U101).
#   R303.2 (VSNS_PWR): the VBUS_PWR -> VSNS_PWR leg of the divider that feeds U501.49.  Its south pad faces the
#   corridor the H0_UP pair closes: the pair's two 45-degree stubs (153.00, 116.412)-(157.44, 121.40) and
#   (153.00, 116.962)-(157.56, 121.10) pass 0.66 mm from the pad centre, and HUB0_RST's B.Cu diagonal
#   (153.95, 115.70)-(144.45, 125.20) sits 0.60 mm below it, so neither a via nor a B.Cu escape fits straight south.
#   The 45-degree step south-east to (154.65, 117.25) does fit: 0.663 mm from the pair's DM stub (0.30 clear) and
#   1.59 mm from HUB0_RST.  The pair is protected and can never be ripped, so the site has to be held before the pair
#   phase; from it the net leaves on In2.Cu - the strip east of the RP2354B is a wall of 0.15 mm fan-out
#   (RP_CC1 at y 135.40, RP_VREG's B.Cu/In2 run to 159.05, RP_XOUT, OT_SEL1), and the RP2354B's own fan-out already
#   keeps a lane out of U501.49 for it.
PAD_ESCAPE_TO = {('R303', '2'): (0.20, 0.20, 0.70, 0.74)}      # -> via (154.65, 117.25)
# The same reservation for a pad whose way out is a *bend*: the list is the polyline from the pad to the via site.
#   U501.48 (AT2_NRST): R304's two pads sit across the RP2354B's 0.4 mm east row - their bodies span y 136.28-136.92,
#   0.08 mm from pin 48's own centre line at 137.00 - so the net cannot leave straight east.  The staggered fan plans a
#   straight lane plus a 45-degree jog, and between R304's pad edges (136.92) and AT2_IRQ's reserved lane along y 137.40
#   (its via sits at (157.75, 137.90)) that leaves a slot 0.405 mm wide where a 0.15 mm track and two 0.15 mm clearances
#   need 0.45: 0.045 mm short.  So the site search reported "no path" for this pin in every rev-9 run - with 46/47 placed
#   first as well as with the planner's own order - and AT2_NRST was left unrouted ("no way through at all", both retry
#   rounds).  This is the path the rev-9 full24 board's router built for itself when the space was still free: straight
#   out at y 137.00 as far as x 154.95 (where AT2_IRQ's lane turns down), a steep step to y 137.20, under both R304 pads
#   to (156.95, 137.20) and down to a via clear of the GND stub at (157.70, 137.05).  Held from the start, it also keeps
#   the fan-out and the power nets off that slot.
#   Measured cost (rev 9, 2026-10-26): the bent path runs 0.20 mm from AT2_IRQ's lane at 137.40, so pin 47 cannot leave
#   its pad at all - giving AT2_NRST a way out is what leaves AT2_IRQ with none.  Spreading the six pins of that row
#   (44-49) further apart instead does not fit either: two 0.6 mm vias in neighbouring lanes need 0.525 mm of lane pitch
#   and the band between the divider's lane (136.60) and OT_SEL1's (137.80) holds only one; a 0.45 mm fan-out via or a
#   moved R304 would be needed, and both are design changes, not router settings (tools/route.log, fan41-43, net43).
# The same reservation for a pad whose way out is a *bend*: the list is the polyline from the pad to the via site.
#   Empty since R304 moved east (2026-10-26): it held a bent path under R304's pads for U501.48 (AT2_NRST), whose straight
#   lane at y 137.00 those pads blocked (0.08 mm).  With R304 1.4 mm further east its pads start at x 156.52, so pin 48's
#   lane runs straight out and takes its own via, and pin 47 (AT2_IRQ) keeps the lane at 137.90 - the corridor between
#   the divider leg (136.60) and pin 47's row (137.40) is no longer needed for a via.
PAD_ESCAPE_PATH = {
}

# the footprints whose pads get that escape, for rules that need to know whether a *single-ended* pin beside a pair
# really has a via coming out of the same side (fanout(): the room a USB pair needs for the pin between its lines)
PAD_ESCAPE_SET = {ref for refs in PAD_ESCAPE.values() for ref in refs}

MM = pcbnew.ToMM
XS = X0 + np.arange(NX) * G
YS = Y0 + np.arange(NY) * G


def gi(v, o):
    return int(round((v - o) / G))


# ── geometry: exact distance from grid points to shapes ───────────────────────
class Shape:
    """kind: 'rect' (cx,cy,hx,hy,ang,corner radius), 'circle' (cx,cy,r), 'seg' (x1,y1,x2,y2,r), 'poly' (pts)."""

    def __init__(self, kind, params, net, layers, smd=False):
        self.kind, self.p, self.net, self.layers, self.smd = kind, params, net, layers, smd

    def bbox(self):
        p = self.p
        if self.kind == 'rect':
            cx, cy, hx, hy, a = p[:5]
            r = math.hypot(hx, hy)
            return cx - r, cy - r, cx + r, cy + r
        if self.kind == 'circle':
            cx, cy, r = p
            return cx - r, cy - r, cx + r, cy + r
        if self.kind == 'poly':
            xs, ys = [q[0] for q in p], [q[1] for q in p]
            return min(xs), min(ys), max(xs), max(ys)
        x1, y1, x2, y2, r = p
        return min(x1, x2) - r, min(y1, y2) - r, max(x1, x2) + r, max(y1, y2) + r

    def window(self, pad=RMAX):
        x0, y0, x1, y1 = self.bbox()
        i0, i1 = max(gi(x0 - pad, X0), 0), min(gi(x1 + pad, X0) + 1, NX)
        j0, j1 = max(gi(y0 - pad, Y0), 0), min(gi(y1 + pad, Y0) + 1, NY)
        return j0, j1, i0, i1

    def dist(self, X, Y):
        p = self.p
        if self.kind == 'rect':
            cx, cy, hx, hy, a = p[:5]
            rr = p[5] if len(p) > 5 else 0.0          # rounded corners (roundrect pads)
            c, s = math.cos(a), math.sin(a)
            u = (X - cx) * c + (Y - cy) * s
            v = -(X - cx) * s + (Y - cy) * c
            return np.maximum(np.hypot(np.maximum(np.abs(u) - (hx - rr), 0), np.maximum(np.abs(v) - (hy - rr), 0)) - rr, 0)
        if self.kind == 'circle':
            cx, cy, r = p
            return np.maximum(np.hypot(X - cx, Y - cy) - r, 0)
        if self.kind == 'poly':
            d = np.full(X.shape, np.inf)
            inside = np.zeros(X.shape, bool)
            n = len(p)
            for k in range(n):
                (x1, y1), (x2, y2) = p[k], p[(k + 1) % n]
                dx, dy = x2 - x1, y2 - y1
                L2 = dx * dx + dy * dy
                t = np.clip(((X - x1) * dx + (Y - y1) * dy) / L2, 0, 1) if L2 else np.zeros_like(X)
                d = np.minimum(d, np.hypot(X - (x1 + t * dx), Y - (y1 + t * dy)))
                cond = ((y1 > Y) != (y2 > Y)) & (X < (x2 - x1) * (Y - y1) / (y2 - y1 + 1e-12) + x1)
                inside ^= cond
            return np.where(inside, 0.0, d)
        x1, y1, x2, y2, r = p
        dx, dy = x2 - x1, y2 - y1
        L2 = dx * dx + dy * dy
        t = np.zeros_like(X) if L2 == 0 else np.clip(((X - x1) * dx + (Y - y1) * dy) / L2, 0, 1)
        return np.maximum(np.hypot(X - (x1 + t * dx), Y - (y1 + t * dy)) - r, 0)


def pad_shapes(pad, net):
    pos = pad.GetPosition()
    cx, cy = MM(pos.x), MM(pos.y)
    sz = pad.GetSize(pcbnew.F_Cu)
    sx, sy = MM(sz.x), MM(sz.y)
    ang = math.radians(pad.GetOrientationDegrees())
    attr = pad.GetAttribute()
    if attr == pcbnew.PAD_ATTRIB_NPTH:
        r = MM(pad.GetDrillSize().x) / 2 + (HOLE_CLR - CLR)
        return [Shape('circle', (cx, cy, r), -1, tuple(range(NL)))]
    layers = tuple(i for i, L in enumerate(LAYERS) if pad.IsOnLayer(L))
    smd = attr == pcbnew.PAD_ATTRIB_SMD
    shp = pad.GetShape(pcbnew.F_Cu)
    if shp == pcbnew.PAD_SHAPE_CUSTOM:
        poly = pad.GetEffectivePolygon(pcbnew.F_Cu, pcbnew.ERROR_INSIDE)
        o = poly.Outline(0)
        pts = [(MM(o.CPoint(k).x), MM(o.CPoint(k).y)) for k in range(o.PointCount())]
        return [Shape('poly', pts, net, layers, smd)]
    if shp == pcbnew.PAD_SHAPE_CIRCLE:
        return [Shape('circle', (cx, cy, sx / 2), net, layers, smd)]
    if shp == pcbnew.PAD_SHAPE_OVAL:
        r = min(sx, sy) / 2
        a = (max(sx, sy) - min(sx, sy)) / 2
        ux, uy = (math.cos(ang), -math.sin(ang)) if sx > sy else (math.sin(ang), math.cos(ang))
        return [Shape('seg', (cx - a * ux, cy - a * uy, cx + a * ux, cy + a * uy, r), net, layers, smd)]
    if abs(ang % (math.pi / 2)) > 1e-3:
        hx, hy = sx / 2, sy / 2
    else:
        hx, hy = (sx / 2, sy / 2)
    rr = 0.0
    if shp == pcbnew.PAD_SHAPE_ROUNDRECT:     # KiCad connects a track only where it ends inside the rounded outline
        rr = min(MM(pad.GetRoundRectCornerRadius(pcbnew.F_Cu)), hx, hy)
    return [Shape('rect', (cx, cy, hx, hy, ang, rr), net, layers, smd)]


# ── distance fields: nearest + second-nearest-other-net distance per layer ───
class Field:
    def __init__(self):
        self.d1 = np.full((NL, NY, NX), np.inf, np.float32)
        self.n1 = np.full((NL, NY, NX), -9, np.int32)
        self.d2 = np.full((NL, NY, NX), np.inf, np.float32)

    def add(self, sh):
        j0, j1, i0, i1 = sh.window()
        if j0 >= j1 or i0 >= i1:
            return
        X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
        d = sh.dist(X, Y).astype(np.float32)
        for L in sh.layers:
            d1 = self.d1[L, j0:j1, i0:i1]
            n1 = self.n1[L, j0:j1, i0:i1]
            d2 = self.d2[L, j0:j1, i0:i1]
            same = n1 == sh.net
            np.minimum(d1, np.where(same, d, d1), out=d1)
            other_closer = (~same) & (d < d1)
            d2[...] = np.where(other_closer, d1, np.where((~same) & (d < d2), d, d2))
            d1[...] = np.where(other_closer, d, d1)
            n1[...] = np.where(other_closer, sh.net, n1)

    def foreign(self, net, sl=None):
        if sl is None:
            return np.where(self.n1 == net, self.d2, self.d1)
        n1, d1, d2 = self.n1[:, sl[0], sl[1]], self.d1[:, sl[0], sl[1]], self.d2[:, sl[0], sl[1]]
        return np.where(n1 == net, d2, d1)


class MinField:
    """Plain nearest distance (no net logic): SMD pads, drills."""

    def __init__(self):
        self.d = np.full((NY, NX), np.inf, np.float32)

    def add(self, sh):
        j0, j1, i0, i1 = sh.window()
        X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
        np.minimum(self.d[j0:j1, i0:i1], sh.dist(X, Y).astype(np.float32), out=self.d[j0:j1, i0:i1])


# ── board model ───────────────────────────────────────────────────────────────
class Board:
    def __init__(self, board, pair_nets):
        self.board = board
        self.netcode = {}
        self.pair_of = {}
        for p, n in pair_nets:            # both lines of a pair share one code: the fat centre line
            c = self.code(p)
            self.netcode[n] = c
            self.pair_of[p] = self.pair_of[n] = (p, n)
        self.pads = {}            # net -> list of (ref.pad, [Shape])
        self.padobj = {}
        self.static = Field()
        self.smd = MinField()
        self.drill_static = MinField()
        for fp in board.GetFootprints():
            for pad in fp.Pads():
                name = pad.GetNetname()
                net = self.code(name) if name else -2
                shapes = pad_shapes(pad, net)
                for sh in shapes:
                    self.static.add(sh)
                    if sh.smd:
                        self.smd.add(sh)
                if pad.GetAttribute() in (pcbnew.PAD_ATTRIB_PTH, pcbnew.PAD_ATTRIB_NPTH):
                    p = pad.GetPosition()
                    self.drill_static.add(Shape('circle', (MM(p.x), MM(p.y), MM(pad.GetDrillSize().x) / 2), -1, (0,)))
                key = f'{fp.GetReference()}.{pad.GetNumber()}'
                self.padobj.setdefault(key, []).append((pad, shapes))
                if name and not name.startswith('unconnected'):
                    self.pads.setdefault(name, []).append((key, shapes))
        # plane nets' through-hole pads: legal_via keeps plane vias out of their thermal relief
        self.tht_pads = {n: [s[0] for _, s in lst if len(s[0].layers) > 1] for n, lst in self.pads.items() if n in PLANES}
        # ... and every other net keeps out of the ring the outer GND pours need round them (THT_SPOKE_ROOM): KiCad wants
        # two of a pad's thermal spokes to reach the pour (the project's min_resolved_spokes), and a track that runs
        # closer than gap + fill + zone clearance cuts them off.  Measured on 0.15.0's first full run: +5V and VBUS_PWR
        # on B.Cu either side of J301's south-west shield pad left it one spoke, into a 3.4 mm2 island - DRC
        # starved_thermal, the run's only violation.  The grown pad goes into the static field as GND copper, so GND's
        # own copper ignores it and every other net's tracks and vias keep CLR from it.
        for sh in self.tht_pads.get('GND', []):
            g, p = THT_SPOKE_ROOM, sh.p
            q = {'circle': lambda: (p[0], p[1], p[2] + g), 'seg': lambda: (*p[:4], p[4] + g),
                 'rect': lambda: (p[0], p[1], p[2] + g, p[3] + g, p[4], (p[5] if len(p) > 5 else 0.0) + g)}.get(sh.kind)
            if q is not None:
                self.static.add(Shape(sh.kind, q(), sh.net, tuple(L for L in (0, 1) if L in sh.layers)))
        X, Y = np.meshgrid(XS, YS)
        self.edge = np.minimum.reduce([X - X0, X1 - X, Y - Y0, Y1 - Y]).astype(np.float32)
        outl = pcbnew.SHAPE_POLY_SET()
        board.GetBoardPolygonOutlines(outl, True)
        for k in range(outl.OutlineCount()):          # cut-outs inside the board (key LED window)
            for h in range(outl.HoleCount(k)):
                ho = outl.Hole(k, h)
                cut = Shape('poly', [(MM(ho.CPoint(q).x), MM(ho.CPoint(q).y)) for q in range(ho.PointCount())], -1, (0,))
                j0, j1, i0, i1 = cut.window(pad=2.0)
                np.minimum(self.edge[j0:j1, i0:i1], cut.dist(X[j0:j1, i0:i1], Y[j0:j1, i0:i1]).astype(np.float32),
                           out=self.edge[j0:j1, i0:i1])
        # rule areas (touch electrode, logos, the Bluetooth module's antenna): per-layer track keep-out, via keep-out.
        # The grids hold copper *centres*, and KiCad tests the copper itself, so the area grows by the via radius and
        # by a signal track's half width (0.16.0's first full run put two vias 0.15 mm outside the logo's no-via area,
        # their pads 0.15 mm inside it: DRC items_not_allowed).  Footprint rule areas and board-level ones alike.
        self.keep_trk = np.zeros((NL, NY, NX), bool)
        self.keep_via = np.zeros((NY, NX), bool)
        for z in [z for fp in board.GetFootprints() for z in fp.Zones()] + list(board.Zones()):
            if not z.GetIsRuleArea():
                continue
            o = z.Outline().Outline(0)
            pts = [(MM(o.CPoint(k).x), MM(o.CPoint(k).y)) for k in range(o.PointCount())]
            dist = Shape('poly', pts, -1, (0,)).dist(X, Y)
            if z.GetDoNotAllowVias():
                self.keep_via |= dist <= VIA_D / 2 + EPS
            if z.GetDoNotAllowTracks():
                for L, lyr in enumerate(LAYERS):
                    if z.IsOnLayer(lyr):
                        self.keep_trk[L] |= dist <= W_SIG / 2 + EPS
        self.noroute = np.zeros((NL, NY, NX), bool)
        # ... and the inside of a fine-pitch QFN (the band between its pin ring and its exposed pad) separately: no
        # F.Cu *track* may run there, but a via may - that is where a bottom-side decoupling cap's supply via belongs,
        # 0.5-1.0 mm from the cap's pad, so its loop stays inside the package (rev 9, 2026-10-26)
        self.qfn_inside = np.zeros((NY, NX), bool)
        inside_any_pad = np.zeros((NY, NX), bool)
        for shapes in [s for _, s in sum(self.pads.values(), [])]:
            for (L, j, i) in pad_cells(shapes):
                inside_any_pad[j, i] = True
        for fp in board.GetFootprints():
            ref = fp.GetReference()
            if ref not in NOROUTE_BODY:
                continue
            bb = fp.GetCourtyard(pcbnew.F_CrtYd).BBox()
            x0, x1 = MM(bb.GetLeft()) + 0.4, MM(bb.GetRight()) - 0.4
            y0, y1 = MM(bb.GetTop()) + 0.4, MM(bb.GetBottom()) - 0.4
            if fp.GetFPID().GetLibItemName().wx_str().startswith('USB_A_'):
                y0 = MM(fp.GetPosition().y) + 1.2
            elif fp.GetFPID().GetLibItemName().wx_str().startswith('USB_C_'):
                # body behind the pad row only: the pads must stay reachable from the board side
                row = [MM(p.GetBoundingBox().GetBottom()) for p in fp.Pads() if p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD]
                y1 = min(y1, max(row))
            box = (X >= x0) & (X <= x1) & (Y >= y0) & (Y <= y1) & ~inside_any_pad
            for L in NOROUTE_BODY[ref]:
                self.noroute[L] |= box
        self.escape_axis = np.zeros((NY, NX), np.uint8)   # 1 vertical-only, 2 horizontal-only F.Cu moves (escape zones)
        # fine-pitch QFNs: no F.Cu inside the pin ring (between the pins and the exposed pad), no vias just outside it,
        # and radial-only F.Cu moves in the escape zone in front of each side, so every pin keeps a straight way out
        for fp in board.GetFootprints():
            ref = fp.GetReference()
            if ref not in QFN_INSIDE and ref not in PIN_VIA_FREE:
                continue
            c = fp.GetPosition()
            cx, cy = MM(c.x), MM(c.y)
            pins = [p for p in fp.Pads() if p.GetNumber().isdigit() and (ref, p.GetNumber()) not in EP_PADS]  # no paste sub-pads
            bbs = [p.GetBoundingBox() for p in pins]
            sides = {'l': [], 'r': [], 't': [], 'b': []}
            for p, bb in zip(pins, bbs):
                dx, dy = MM(p.GetPosition().x) - cx, MM(p.GetPosition().y) - cy
                if abs(dx) > abs(dy):
                    sides['l' if dx < 0 else 'r'].append(MM(bb.GetRight()) if dx < 0 else MM(bb.GetLeft()))
                else:
                    sides['t' if dy < 0 else 'b'].append(MM(bb.GetBottom()) if dy < 0 else MM(bb.GetTop()))
            if ref in QFN_INSIDE and all(sides.values()):
                ix0, ix1, iy0, iy1 = max(sides['l']), min(sides['r']), max(sides['t']), min(sides['b'])
                interior = (X > ix0) & (X < ix1) & (Y > iy0) & (Y < iy1) & ~inside_any_pad
                self.noroute[0] |= interior
                self.qfn_inside |= interior          # tracks: no; vias: yes (see legal_via)
            if ref in QFN_FANOUT:
                for side, axis in (('l', 2), ('r', 2), ('t', 1), ('b', 1)):
                    z = QFN_FANOUT[ref][side]
                    sel = [bb for p, bb in zip(pins, bbs) if
                           (('l' if MM(p.GetPosition().x) < cx else 'r') if abs(MM(p.GetPosition().x) - cx) >
                            abs(MM(p.GetPosition().y) - cy) else ('t' if MM(p.GetPosition().y) < cy else 'b')) == side]
                    lo = [MM(bb.GetLeft()) for bb in sel] if axis == 1 else [MM(bb.GetTop()) for bb in sel]
                    hi = [MM(bb.GetRight()) for bb in sel] if axis == 1 else [MM(bb.GetBottom()) for bb in sel]
                    if side == 'l':
                        t = min(MM(bb.GetLeft()) for bb in sel)
                        m = (X >= t - z) & (X <= t) & (Y >= min(lo)) & (Y <= max(hi))
                    elif side == 'r':
                        t = max(MM(bb.GetRight()) for bb in sel)
                        m = (X >= t) & (X <= t + z) & (Y >= min(lo)) & (Y <= max(hi))
                    elif side == 't':
                        t = min(MM(bb.GetTop()) for bb in sel)
                        m = (Y >= t - z) & (Y <= t) & (X >= min(lo)) & (X <= max(hi))
                    else:
                        t = max(MM(bb.GetBottom()) for bb in sel)
                        m = (Y >= t) & (Y <= t + z) & (X >= min(lo)) & (X <= max(hi))
                    self.escape_axis[m] = axis
            if ref in PIN_VIA_FREE:
                m = PIN_VIA_FREE[ref]
                m = m if isinstance(m, dict) else {k: m for k in 'lrtb'}
                ox0, oy0 = min(MM(bb.GetLeft()) for bb in bbs) - m['l'], min(MM(bb.GetTop()) for bb in bbs) - m['t']
                ox1, oy1 = max(MM(bb.GetRight()) for bb in bbs) + m['r'], max(MM(bb.GetBottom()) for bb in bbs) + m['b']
                # the whole footprint stays via-free for *everything except* the bottom-side decoupling caps' own
                # plane vias: those are let in by legal_via(qfn_ok=True) on the qfn_inside cells (rev 9).  Keeping the
                # ring closed for pairs and single-net tracks is what protects the chip's fan-out - measured: with the
                # interior open, pairs dropped layer changes in there and 4 RP pins and 5 AT32 pins lost their sites.
                self.keep_via |= (X >= ox0) & (X <= ox1) & (Y >= oy0) & (Y <= oy1)
        self.fixed = []
        self.tracks = []
        self.reserved = []        # virtual copper: pair escape corridors, never written to the board
        self.escape = {}          # net -> its pins' escape reservations (dense ICs), active while the net is unrouted
        self.escape_on = set()
        self.changes = []         # bounding box of every copper item added or removed, in order
        # crystals: their courtyards, kept clear of other nets' copper by the crystal guard (xtal_field)
        self.xtal_court = []
        self.xtal_zone = np.zeros((NY, NX), bool)    # round the crystals' chips: no guard on crystal lines
        for fp in board.GetFootprints():
            if fp.GetReference().startswith('U') and any(re.match(XTAL_NETS, p.GetNetname()) for p in fp.Pads()):
                bb = fp.GetCourtyard(pcbnew.B_CrtYd if fp.IsFlipped() else pcbnew.F_CrtYd).BBox()
                self.xtal_zone |= ((X >= MM(bb.GetLeft()) - XTAL_PIN_ZONE) & (X <= MM(bb.GetRight()) + XTAL_PIN_ZONE)
                                   & (Y >= MM(bb.GetTop()) - XTAL_PIN_ZONE) & (Y <= MM(bb.GetBottom()) + XTAL_PIN_ZONE))
            if re.match(r'Y\d', fp.GetReference()):
                bb = fp.GetCourtyard(pcbnew.B_CrtYd if fp.IsFlipped() else pcbnew.F_CrtYd).BBox()
                self.xtal_court.append((MM(bb.GetLeft()), MM(bb.GetTop()), MM(bb.GetRight()), MM(bb.GetBottom()),
                                        1 if fp.IsFlipped() else 0))
        self.xtal_field()
        # wide power nets keep out of the fine-pitch ICs' escape areas unless they have a pin there
        areas = []
        for fp in board.GetFootprints():
            if fp.GetReference() in DENSE_ICS:
                m, bb = DENSE_ICS[fp.GetReference()], fp.GetCourtyard(pcbnew.F_CrtYd).BBox()
                areas.append((MM(bb.GetLeft()) - m, MM(bb.GetTop()) - m, MM(bb.GetRight()) + m, MM(bb.GetBottom()) + m))
        self.power_keep = {}             # net -> areas it may not enter
        for net, w in POWER.items():
            if w >= 0.4 and net in self.pads:
                ctr = [((s[0].bbox()[0] + s[0].bbox()[2]) / 2, (s[0].bbox()[1] + s[0].bbox()[3]) / 2) for _, s in self.pads[net]]
                self.power_keep[net] = [a for a in areas if not any(a[0] <= x <= a[2] and a[1] <= y <= a[3] for x, y in ctr)]
        # a pad that escapes into its own part (PAD_ESCAPE_INWARD: the port ESDs' VBUS pads, via in the slot between the
        # pad rows) has that one way out, and its rail leaves that via on B.Cu or In2.Cu right under the part - so the
        # part belongs to its own nets on every layer: no net without a pad on it crosses it (power_keep is every
        # net's keep-out list, rails' and signals').  Measured, rev 12, with the rails sized for In2.Cu's 0.5 oz (VBUS
        # 1.15 mm there, so the via's In2.Cu exit is too narrow): VBUS_PC1, then HUB0_RST took B.Cu under U301 and
        # VBUS_PWR could not leave U301.5 ("no way through at all"), three runs out of three
        for ref, pad in PAD_ESCAPE_INWARD:
            fp = board.FindFootprintByReference(ref)
            own = {p.GetNetname() for p in fp.Pads()}
            bb = fp.GetCourtyard(pcbnew.F_CrtYd).BBox()
            box = (MM(bb.GetLeft()), MM(bb.GetTop()), MM(bb.GetRight()), MM(bb.GetBottom()))
            for net in self.pads:
                if net not in own and net not in PLANES:
                    self.power_keep.setdefault(net, []).append(box)
        # each dense chip's fan-out band: the ring its staggered via fan-out lives in (the courtyard plus the deepest
        # via row).  A net that has no pin on that chip pays FAN_COST per step there and may not drop a via, so a
        # lane crossing a chip it has nothing to do with cannot take the room the chip's fan-out vias need.  The
        # chip's own area (inside the courtyard) stays open, so routing *under* it is still possible.
        self.fan_keep = {}               # net -> [(band box, courtyard box)]
        for fp in board.GetFootprints():
            ref = fp.GetReference()
            if ref not in FANOUT_IC:
                continue
            m = max(FANOUT_IC[ref]['max'].values()) + 0.35
            bb = fp.GetCourtyard(pcbnew.F_CrtYd).BBox()
            court = (MM(bb.GetLeft()), MM(bb.GetTop()), MM(bb.GetRight()), MM(bb.GetBottom()))
            band = (court[0] - m, court[1] - m, court[2] + m, court[3] + m)
            for net, lst in self.pads.items():
                # the pad keys are 'REF.PIN': compare the part.  Comparing the whole key never matched, so every net
                # paid FAN_COST in its *own* chips' bands too - measured on the rev-9 pair stage: AT2_CS
                # (U501.56 -> U1101.14) found no path at all with both its chips' bands on, and routed at once
                # (718 cells) with either one lifted, i.e. a net could not reach its own reserved fan-out vias.
                if ref not in {r.split('.')[0] for r, _ in lst}:
                    self.fan_keep.setdefault(net, []).append((band, court))
        self.rebuild()

    def code(self, name):
        if name not in self.netcode:
            self.netcode[name] = len(set(self.netcode.values())) + 1000
        return self.netcode[name]

    def rebuild(self):
        # built aside and swapped in whole: planner threads may be reading the old fields meanwhile
        dyn, drill = Field(), MinField()
        drill.d[...] = self.drill_static.d
        for name, kind, data in self.tracks + self.reserved + self.escape_items():
            self._add_dyn(name, kind, data, dyn, drill)
        self.dyn, self.drill = dyn, drill
        self.xtal_field()

    def xtal_field(self):
        """The crystal guard's distance, per routing layer: from a crystal line's copper, or from a crystal's
        courtyard plus XTAL_GUARD.  A guarded net's track centre needs XTAL_GUARD + half its width (so none of its
        copper comes within XTAL_GUARD of a crystal line or enters a courtyard), its via XTAL_GUARD + the via radius
        on every layer.  Exact out to XTAL_REACH, the most any of those tests asks.  Built aside and swapped in."""
        d = np.full((NL, NY, NX), np.inf, np.float32)
        for x0, y0, x1, y1, L in self.xtal_court:
            sh = Shape('rect', ((x0 + x1) / 2, (y0 + y1) / 2, (x1 - x0) / 2, (y1 - y0) / 2, 0.0), -1, (L,))
            self._xtal_add(sh, XTAL_GUARD, d)
        for name, kind, data in self.tracks:
            if re.match(XTAL_NETS, name):
                self._xtal_add(self._xtal_shape(kind, data), 0.0, d, zone=True)
        self.xtal_d = d

    @staticmethod
    def _xtal_shape(kind, data):
        if kind == 'via':
            return Shape('circle', (data[0], data[1], VIA_D / 2), -1, tuple(range(NL)))
        x1, y1, x2, y2, w, L = data
        return Shape('seg', (x1, y1, x2, y2, w / 2), -1, (int(L),))

    def _xtal_add(self, sh, off, d=None, zone=False):
        """zone: a crystal line, whose guard stops XTAL_PIN_ZONE short of its chip."""
        d = self.xtal_d if d is None else d
        j0, j1, i0, i1 = sh.window(pad=XTAL_REACH)
        if j0 < j1 and i0 < i1:
            X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
            v = sh.dist(X, Y).astype(np.float32) + off
            if zone:
                v[self.xtal_zone[j0:j1, i0:i1]] = np.inf
            for L in sh.layers:
                np.minimum(d[L, j0:j1, i0:i1], v, out=d[L, j0:j1, i0:i1])

    def escape_items(self):
        return [(n, k, d) for n in sorted(self.escape_on) for k, d in self.escape[n]]

    def release_escape(self, names):
        """A net is routed: its pins' escape reservations go (it no longer needs them, others may use the space)."""
        names = [n for n in names if n in self.escape_on]
        if names:
            for n in names:
                self.escape_on.discard(n)
                self.changes += [item_box(k, d) for k, d in self.escape[n]]
            self.rebuild()

    def restore_escape(self, names):
        names = [n for n in names if n in self.escape and n not in self.escape_on]
        if names:
            for n in names:
                self.escape_on.add(n)
                self.changes += [item_box(k, d) for k, d in self.escape[n]]
            self.rebuild()

    def reserve(self, name, data):
        self.reserved.append((name, 'seg', tuple(data)))
        self._add_dyn(name, 'seg', tuple(data))

    def clear_reserved(self):
        self.reserved = []
        self.rebuild()

    def unreserve(self, names):
        """Drop these nets' reservations (a pair routed already needs no escape corridor any more)."""
        names = set(names)
        if any(n in names for n, _, _ in self.reserved):
            self.changes += [item_box(k, d) for n, k, d in self.reserved if n in names]
            self.reserved = [r for r in self.reserved if r[0] not in names]
            self.rebuild()

    def _add_dyn(self, name, kind, data, dyn=None, drill=None):
        dyn, drill = dyn or self.dyn, drill or self.drill
        net = self.code(name)
        if kind == 'seg':
            x1, y1, x2, y2, w, L = data
            dyn.add(Shape('seg', (x1, y1, x2, y2, w / 2), net, (int(L),)))
        else:
            x, y = data
            dyn.add(Shape('circle', (x, y, VIA_D / 2), net, tuple(range(NL))))
            drill.add(Shape('circle', (x, y, VIA_DRILL / 2), -1, (0,)))

    def add_track(self, name, kind, data, fixed=False):
        item = (name, kind, tuple(data))
        self.tracks.append(item)
        if fixed:
            self.fixed.append(item)
        if getattr(self, 'attempt_log', None) is not None and not name.startswith('__'):
            self.attempt_log.append(item)      # what a pair attempt drew, for the live view's ghost of a failure
        self._add_dyn(name, kind, item[2])
        if re.match(XTAL_NETS, name):
            self._xtal_add(self._xtal_shape(kind, item[2]), 0.0, zone=True)
        self.changes.append(item_box(kind, item[2]))

    def rip(self, names):
        names = set(names)
        for n in list(names):
            if n in self.pair_of:
                names |= set(self.pair_of[n])
        gone = [t for t in self.tracks if t[0] in names and t not in self.fixed]
        self.tracks = [t for t in self.tracks if t[0] not in names or t in self.fixed]
        self.changes += [item_box(k, d) for _, k, d in gone]
        for n in names:                   # unrouted again: its pins' escapes are reserved again
            if n in self.escape and n not in self.escape_on:
                self.escape_on.add(n)
                self.changes += [item_box(k, d) for k, d in self.escape[n]]
        self.rebuild()

    def foreign(self, name, sl=None):
        net = self.code(name)
        return np.minimum(self.static.foreign(net, sl), self.dyn.foreign(net, sl))

    def lift_plane(self, hits, reach):
        """Remove the (non-fixed) plane fan-out copper within reach of the hit cells; the pads it served get a new
        via at the end (plane_open_pads). Returns the number of items removed."""
        gone = set()
        for k, (nm, kind, d) in enumerate(self.tracks):
            if nm not in PLANES or (nm, kind, d) in self.fixed:
                continue
            for pn, L, j, i in hits:
                if pn != nm:
                    continue
                x, y = X0 + i * G, Y0 + j * G
                if kind == 'via':
                    near = math.hypot(d[0] - x, d[1] - y) <= reach + VIA_D / 2
                else:
                    near = int(d[5]) == L and _on_seg(x, y, d, tol=reach + d[4] / 2)[0]
                if near:
                    gone.add(k)
                    break
        if gone:
            self.changes += [item_box(self.tracks[k][1], self.tracks[k][2]) for k in gone]
            self.tracks = [t for k, t in enumerate(self.tracks) if k not in gone]
            self.rebuild()
        return len(gone)

    def keep_in2_under_pairs(self):
        """No In2.Cu tracks under (and IN2_REF_KEEP beside) a B.Cu high-speed pair line: In2 is that line's reference.
        Hub-0's full-speed (12 Mbit/s) pairs may be crossed underneath."""
        for name, kind, d in self.tracks:
            if kind != 'seg' or name not in self.pair_of or int(d[5]) != 1 or name.startswith('H0_'):
                continue
            sh = Shape('seg', (d[0], d[1], d[2], d[3], IN2_REF_KEEP), -1, (2,))
            j0, j1, i0, i1 = sh.window(pad=0.0)
            X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
            self.keep_trk[2, j0:j1, i0:i1] |= sh.dist(X, Y) <= 0


def item_box(kind, d):
    """Area whose distance fields a track / via changes (its copper plus the exact-field radius)."""
    if kind == 'via':
        r = VIA_D / 2 + RMAX + G
        return d[0] - r, d[1] - r, d[0] + r, d[1] + r
    r = d[4] / 2 + RMAX + G
    return min(d[0], d[2]) - r, min(d[1], d[3]) - r, max(d[0], d[2]) + r, max(d[1], d[3]) + r


def pad_cells(shapes, layers=None):
    """Grid cells whose centre lies inside the pad copper."""
    cells = set()
    for sh in shapes:
        if sh.net < 0:
            continue
        j0, j1, i0, i1 = sh.window(pad=0.0)
        X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
        inside = sh.dist(X, Y) <= 1e-6
        for L in sh.layers:
            if layers is None or L in layers:
                for jj, ii in zip(*np.nonzero(inside)):
                    cells.add((L, j0 + jj, i0 + ii))
    return cells


# ── A* router (windowed) ──────────────────────────────────────────────────────
NO_IN2 = r'(.*_D[PM](_PIN)?)$'   # USB stays on the outer layers; SPI lines may cross on In2 (under In1 GND)
XTAL_NETS = r'.*_X(I|O|IN|OUT|O_R)$'    # crystal lines (RP_XO_R: between the RP2354B's 1 kOhm and its crystal)
# A crystal line stays on F.Cu / B.Cu and changes layer only if F.Cu has no way at all (XTAL_VIA_COST); every other
# net keeps its copper out of a crystal's courtyard and XTAL_GUARD (edge to edge) off a crystal line (Board.xtal_field).
# Rev 12, from tools/xtal_report.py on rev 11: the RP2354B's XIN ran 32.6 mm with 3 vias through In2.Cu and B.Cu,
# OT_SEL0 crossed its crystal and the SWD clock ran 0.27 mm beside it for 4 mm.
XTAL_GUARD = 0.5
# within this of a crystal's chip (beyond its courtyard) the guard round crystal lines is off: the chip's other pins sit
# 0.4-0.5 mm from its crystal pins and must escape past them (measured, rev 12's first crystal run: the hubs' PLLFILT,
# the RP2354B's SWCLK and MS_FLT, AT1_CC1 walled in), and the RP2354B's series R sits there too
XTAL_PIN_ZONE = 3.5
# 0.16.0: 2.5 -> 3.5, so the zone covers a dense fan-out's deepest via row (FANOUT_IC: the RP2354B's south side plans
# vias 3.3 mm below its pad tips, plus the via's radius).  With the Bluetooth module's UART on pins 36/37 the plan put
# RP_SWDIO's site there, 0.53 mm from RP_XO_R's way south past R502, and the guard vetoed it: RP_SWDIO unrouted.
# grid steps: a via costs a crystal line 5 mm of detour.  At 20 mm AT2_XO took a 16.9 mm F.Cu detour instead of a
# 5.3 mm line with two vias - more track (and pickup) than the vias' ~0.4 pF are worth
XTAL_VIA_COST = 100.0
XTAL_REACH = XTAL_GUARD + max(RAIL_W_MAX / 2, PAIR_VIA_R, VIA_D / 2) + G     # how far the guard's field must be exact


def layers_of(B, name):
    """Routing layers a net may use: pairs, planes, USB, SPI and crystal lines stay on F.Cu / B.Cu."""
    if not IN2_SIGNALS or name in PLANES or name in B.pair_of or re.match(NO_IN2, name) or re.match(XTAL_NETS, name):
        return (0, 1)
    return (0, 1, 2)


def xtal_guarded(name):
    """Nets the crystal guard applies to: all but the crystal lines themselves and the planes (their pads and
    fan-out sit at the crystal by design)."""
    return name not in PLANES and not re.match(XTAL_NETS, name)


DIRS = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)]
SQ2 = math.sqrt(2)


class NetRouter:
    """Free-space model of one net (or one fat pair) inside a window of the board."""

    def __init__(self, B, name, width, window, soft=False, via_r=None, layers=None, max_turn=2,
                 layer_cost=LAYER_COST, via_cost=VIA_COST, h_weight=H_WEIGHT, own_vias=(), avoid=(), neck=False,
                 fan_cost=FAN_COST):
        if layers is None:
            layers = layers_of(B, name)
        self.layers = tuple(layers)
        self.B, self.name, self.w = B, name, width
        self.hw = 1.0 if DESPERATE else h_weight       # no heuristic in the last-resort pass: optimal, slower
        x0, y0, x1, y1 = window
        self.i0, self.i1 = max(gi(x0, X0), 0), min(gi(x1, X0) + 1, NX)
        self.j0, self.j1 = max(gi(y0, Y0), 0), min(gi(y1, Y0) + 1, NY)
        self.nx, self.ny = self.i1 - self.i0, self.j1 - self.j0
        sl = (slice(self.j0, self.j1), slice(self.i0, self.i1))
        if re.match(XTAL_NETS, name):
            via_cost = max(via_cost, XTAL_VIA_COST)
        self.max_turn, self.layer_cost, self.via_cost = max_turn, layer_cost, via_cost
        self.fan_cost = fan_cost
        self.explored = None          # set by a successful kernel search: (x0, y0, x1, y1) of the expanded cells
        ax = B.escape_axis[self.j0:self.j1, self.i0:self.i1]
        self.axis = np.ascontiguousarray(ax) if ax.any() else None
        fd = B.foreign(name, sl)
        self.lw = [layer_width(name, width, L) for L in range(NL)]     # a rail is wider on 0.5 oz In2.Cu
        thr = [CLR + w / 2 + EPS for w in self.lw]
        edge = B.edge[sl]
        # the edge strip: no track and no via within EDGE_KEEP of the board edge, except near a pad of this net, so
        # edge-mounted parts keep their escape (own_pad = cells within a radius of one of this net's own pads)
        def own_pad(r):
            m = np.zeros((self.ny, self.nx), bool)
            for key, shapes in B.pads.get(name, []):
                for sh in shapes:
                    j0, j1, i0, i1 = sh.window(pad=r)
                    j0, j1, i0, i1 = max(j0, self.j0), min(j1, self.j1), max(i0, self.i0), min(i1, self.i1)
                    if j0 < j1 and i0 < i1:
                        X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
                        m[j0 - self.j0:j1 - self.j0, i0 - self.i0:i1 - self.i0] |= sh.dist(X, Y) <= r
            return m
        band = edge < EDGE_KEEP
        if band.any():
            band &= ~own_pad(EDGE_PAD_R)
        self.band = band                         # edge escape strip: a step here costs EDGE_COST, a via is barred
        edge_ok = [edge >= EDGE_CLR + w / 2 for w in self.lw]

        def keep_out(L):
            """Track keep-out (rule areas, In2.Cu under a B.Cu pair) for this net's track centre: a keep area holds
            the centre of a W_SIG track, so a wider track's centre stays the extra half-width further out."""
            r = (self.lw[L] - W_SIG) / 2
            m = int(math.ceil(r / G))
            if m <= 0:
                return B.keep_trk[L][sl]
            j0, j1 = max(self.j0 - m, 0), min(self.j1 + m, NY)
            i0, i1 = max(self.i0 - m, 0), min(self.i1 + m, NX)
            big = B.keep_trk[L][j0:j1, i0:i1]
            if not big.any():
                return B.keep_trk[L][sl]
            yy, xx = np.ogrid[-m:m + 1, -m:m + 1]
            big = ndimage.binary_dilation(big, structure=(xx * xx + yy * yy) * G * G <= r * r + EPS)
            return big[self.j0 - j0:self.j0 - j0 + self.ny, self.i0 - i0:self.i0 - i0 + self.nx]
        keep = [keep_out(L) for L in range(NL)]
        guard = B.xtal_d[:, sl[0], sl[1]] if xtal_guarded(name) else None      # crystal guard (Board.xtal_field)
        xok = [np.ones((self.ny, self.nx), bool) if guard is None else guard[L] >= XTAL_GUARD + self.lw[L] / 2
               for L in range(NL)]
        plane = name in PLANES
        free = []
        for L in range(NL):
            f = (fd[L] >= thr[L]) & edge_ok[L] & ~keep[L] & xok[L]
            if not plane:
                f &= ~B.noroute[L][sl]
            if L not in layers:
                f &= False
            free.append(f)
        self.fd, self.edge_d = fd, edge          # per-cell widths of a found path (neck-downs, cell_widths)
        self.near = None
        if neck:                                 # power rail: neck down only at its own (narrow) pads, or through a
            near = np.zeros((self.ny, self.nx), bool)   # dense IC's whole via fan-out (its pin is unreachable otherwise)
            for key, shapes in B.pads.get(name, []):
                ic = FANOUT_IC.get(key.split('.')[0])
                r = max(ic['max'].values()) + 0.5 if ic else PAD_NECK_R
                for sh in shapes:
                    j0, j1, i0, i1 = sh.window(pad=r)
                    j0, j1, i0, i1 = max(j0, self.j0), min(j1, self.j1), max(i0, self.i0), min(i1, self.i1)
                    if j0 < j1 and i0 < i1:
                        X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
                        near[j0 - self.j0:j1 - self.j0, i0 - self.i0:i1 - self.i0] |= sh.dist(X, Y) <= r
            self.near = near
            wn = min(NECK_WIDTHS)
            for L in layers:
                free[L] |= ((fd[L] >= CLR + wn / 2 + EPS) & (edge >= EDGE_CLR + wn / 2)
                            & ~B.keep_trk[L][sl] & ~B.noroute[L][sl] & near & xok[L])
        self.free = free
        self.hug = [(fd[L] < thr[L] + 0.1) for L in range(NL)]
        vr = via_r if via_r is not None else VIA_D / 2
        vthr = CLR + vr + EPS
        self.via_ok = (np.all(fd >= vthr, axis=0) & (edge >= EDGE_CLR + vr) & ~band & ~B.keep_via[sl]
                       & (True if guard is None else guard.min(axis=0) >= XTAL_GUARD + vr)
                       & ~B.noroute[0][sl] & ~B.noroute[1][sl]
                       & (B.drill.d[sl] >= HOLE_CLR + VIA_DRILL / 2 + (vr - VIA_D / 2))
                       & (B.smd.d[sl] >= vr + 0.05))
        for vx, vy in own_vias:          # planned, not yet committed vias of this net: same hole clearance
            sh = Shape('circle', (vx, vy, VIA_DRILL / 2), -1, (0,))
            j0, j1, i0, i1 = sh.window()
            j0, j1, i0, i1 = max(j0, self.j0), min(j1, self.j1), max(i0, self.i0), min(i1, self.i1)
            if j0 < j1 and i0 < i1:
                X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
                near = sh.dist(X, Y).astype(np.float32) < HOLE_CLR + VIA_DRILL / 2 + (vr - VIA_D / 2)
                self.via_ok[j0 - self.j0:j1 - self.j0, i0 - self.i0:i1 - self.i0] &= ~near
        for ax0, ay0, ax1, ay1 in avoid:  # F.Cu corridors kept clear for another net (mate_corridors)
            sh = Shape('seg', (ax0, ay0, ax1, ay1, W_SIG / 2), -1, (0,))
            j0, j1, i0, i1 = sh.window()
            j0, j1, i0, i1 = max(j0, self.j0), min(j1, self.j1), max(i0, self.i0), min(i1, self.i1)
            if j0 < j1 and i0 < i1:
                X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
                d = sh.dist(X, Y)
                sj, si = slice(j0 - self.j0, j1 - self.j0), slice(i0 - self.i0, i1 - self.i0)
                free[0][sj, si] &= d >= thr[0]
                self.via_ok[sj, si] &= d >= vthr
        if name in B.escape_on:          # its own reserved fan-out vias: that hole is its own (planned legal)
            for kind, d in B.escape[name]:
                j, i = gi(d[1], Y0) - self.j0, gi(d[0], X0) - self.i0
                if kind == 'via' and 0 <= j < self.ny and 0 <= i < self.nx:
                    self.via_ok[j, i] = bool(np.all(fd[:, j, i] >= vthr))
        if len(layers) < 2:
            self.via_ok &= False
        zone = np.zeros((self.ny, self.nx), bool)
        for ax0, ay0, ax1, ay1 in B.power_keep.get(name, ()):
            zj0, zj1 = max(gi(ay0, Y0) - self.j0, 0), min(gi(ay1, Y0) + 1 - self.j0, self.ny)
            zi0, zi1 = max(gi(ax0, X0) - self.i0, 0), min(gi(ax1, X0) + 1 - self.i0, self.nx)
            if zj0 < zj1 and zi0 < zi1:
                zone[zj0:zj1, zi0:zi1] = True
        if zone.any():
            for L in range(NL):
                free[L] &= ~zone
            self.via_ok &= ~zone
        # the fan-out bands of the chips this net has nothing to do with: a track pays FAN_COST per step there and
        # no via may be dropped, so the room the chip's own fan-out needs stays free
        fb = np.zeros((self.ny, self.nx), bool)
        for (bx0, by0, bx1, by1), (cx0, cy0, cx1, cy1) in B.fan_keep.get(name, ()):
            zj0, zj1 = max(gi(by0, Y0) - self.j0, 0), min(gi(by1, Y0) + 1 - self.j0, self.ny)
            zi0, zi1 = max(gi(bx0, X0) - self.i0, 0), min(gi(bx1, X0) + 1 - self.i0, self.nx)
            if zj0 < zj1 and zi0 < zi1:
                fb[zj0:zj1, zi0:zi1] = True
            pj0, pj1 = max(gi(cy0, Y0) - self.j0, 0), min(gi(cy1, Y0) + 1 - self.j0, self.ny)
            pi0, pi1 = max(gi(cx0, X0) - self.i0, 0), min(gi(cx1, X0) + 1 - self.i0, self.nx)
            if pj0 < pj1 and pi0 < pi1:
                fb[pj0:pj1, pi0:pi1] = False          # under the chip itself: still routable
        self.fband = fb
        self.soft = None
        if soft:
            sfd = np.minimum(B.static.foreign(B.code(name), sl), np.inf)
            self.soft = [(sfd[L] >= thr[L]) & edge_ok[L] & ~keep[L] & xok[L] & ~B.noroute[L][sl] & ~free[L]
                         & (L in layers) for L in range(NL)]
            if self.near is not None:
                wn = min(NECK_WIDTHS)
                for L in layers:
                    self.soft[L] |= ((sfd[L] >= CLR + wn / 2 + EPS) & (edge >= EDGE_CLR + wn / 2)
                                     & ~B.keep_trk[L][sl] & ~B.noroute[L][sl] & self.near & ~free[L] & xok[L])
            svia = (np.all(sfd >= vthr, axis=0) & (edge >= EDGE_CLR + vr) & ~band & ~B.keep_via[sl]
                    & (B.drill_static.d[sl] >= HOLE_CLR + VIA_DRILL / 2) & (B.smd.d[sl] >= vr + 0.05))
            if guard is not None:
                svia &= guard.min(axis=0) >= XTAL_GUARD + vr
            self.via_soft = svia & ~self.via_ok
            if zone.any():
                for L in range(NL):
                    self.soft[L] &= ~zone
                self.via_soft &= ~zone

    # flat Python lists for the pure-Python search and the per-cell checks elsewhere; built only when used
    @functools.cached_property
    def flat(self):
        return [self.free[L].ravel().tolist() for L in range(NL)]

    @functools.cached_property
    def hugf(self):
        return [self.hug[L].ravel().tolist() for L in range(NL)]

    @functools.cached_property
    def axisf(self):
        return self.axis.ravel().tolist() if self.axis is not None else None

    @functools.cached_property
    def bandf(self):
        return self.band.ravel().tolist()

    @functools.cached_property
    def fbandf(self):
        return self.fband.ravel().tolist()

    @functools.cached_property
    def viaf(self):
        return self.via_ok.ravel().tolist()

    @functools.cached_property
    def softf(self):
        return [self.soft[L].ravel().tolist() for L in range(NL)]

    @functools.cached_property
    def viasf(self):
        return self.via_soft.ravel().tolist()

    def reachable(self, sources, targets):
        """Cheap necessary condition for search(): some source and some target share a connected free region
        (4-connected per layer, joined across layers where a via fits)."""
        src, tgt = self.local(sources), self.local(targets)
        if not src or not tgt:
            return False
        lab = []
        n0 = 0
        for L in range(NL):
            m, n = ndimage.label(self.free[L])
            lab.append(np.where(m > 0, m + n0, 0))
            n0 += n
        # regions joined wherever a via fits: connected components of the region graph (one edge per via cell)
        ra, rb = [np.zeros(0, np.int64)], [np.zeros(0, np.int64)]
        for La, Lb in [(a, b) for a in self.layers for b in self.layers if a < b]:
            both = self.via_ok & (lab[La] > 0) & (lab[Lb] > 0)
            ra.append(lab[La][both].astype(np.int64))
            rb.append(lab[Lb][both].astype(np.int64))
        key = np.unique(np.concatenate(ra) * (n0 + 1) + np.concatenate(rb))     # one edge per region pair
        ra, rb = key // (n0 + 1), key % (n0 + 1)
        _, comp = connected_components(coo_matrix((np.ones(len(ra), np.int8), (ra, rb)), shape=(n0 + 1, n0 + 1)),
                                       directed=False)
        rs = {comp[lab[L][j, i]] for (L, j, i) in src if lab[L][j, i]}
        return any(lab[L][j, i] and comp[lab[L][j, i]] in rs for (L, j, i) in tgt)

    def local(self, cells):
        out = set()
        for (L, j, i) in cells:
            if self.j0 <= j < self.j1 and self.i0 <= i < self.i1:
                out.add((L, j - self.j0, i - self.i0))
        return out

    def search(self, sources, targets, tcenter, max_exp=4_000_000, start_dir=None, end_dir=None):
        """sources/targets: sets of global (L, j, i). Returns list of global (L, j, i) or None."""
        if KERNEL is None:
            return self.search_py(sources, targets, tcenter, max_exp, start_dir, end_dir)
        max_exp *= 8 if DESPERATE else 1           # the last-resort pass may search much longer
        src, tgt = self.local(sources), self.local(targets)
        if not src or not tgt:
            return None
        u8 = lambda a: np.ascontiguousarray(a, dtype=np.uint8)
        flat, hug, via = u8(np.stack(self.free)), u8(np.stack(self.hug)), u8(self.via_ok)
        eband = u8(self.band)
        fband = u8(self.fband)
        soft = u8(np.stack(self.soft)) if self.soft is not None else None
        vsoft = u8(self.via_soft) if self.soft is not None else None
        tmask = np.zeros((NL, self.ny, self.nx), np.uint8)
        t = np.array(sorted(tgt), np.int64).reshape(-1, 3)
        tmask[t[:, 0], t[:, 1], t[:, 2]] = 1
        sarr = np.ascontiguousarray(np.array(sorted(src), np.int32).reshape(-1, 3))
        lc = np.array(list(self.layer_cost) + [1.0] * (NL - len(self.layer_cost)), np.float64)
        tc = np.array([TURN_COST.get(k, 0.0) for k in range(5)], np.float64)
        tx = ty = 0
        if tcenter is not None:
            tx, ty = gi(tcenter[0], X0) - self.i0, gi(tcenter[1], Y0) - self.j0
        cap = 200_000
        box = np.zeros(4, np.int32)
        while True:
            out = np.zeros((cap, 3), np.int32)
            n = KERNEL(NL, self.ny, self.nx, flat.ctypes.data, soft.ctypes.data if soft is not None else None,
                       hug.ctypes.data, via.ctypes.data, vsoft.ctypes.data if vsoft is not None else None,
                       tmask.ctypes.data, lc.ctypes.data, sarr.ctypes.data, len(sarr),
                       DIRS.index(start_dir) if start_dir is not None else 8,
                       DIRS.index(end_dir) if end_dir is not None else -1,
                       int(tcenter is not None), tx, ty, float(self.hw), float(self.via_cost), int(self.max_turn),
                       tc.ctypes.data, HUG_COST, int(max_exp), out.ctypes.data, cap, box.ctypes.data,
                       self.axis.ctypes.data if self.axis is not None else None,
                       eband.ctypes.data, EDGE_COST, fband.ctypes.data, self.fan_cost)
            if n != -1:
                break
            cap *= 4
        if n == -2:
            raise MemoryError('astar kernel: allocation failed')
        if n == 0:
            return None
        # a found path depends only on the cells the search expanded and their neighbours (route_all's conflicts)
        i_lo, j_lo, i_hi, j_hi = (int(v) for v in box)
        self.explored = (X0 + (self.i0 + i_lo - 1) * G, Y0 + (self.j0 + j_lo - 1) * G,
                         X0 + (self.i0 + i_hi + 1) * G, Y0 + (self.j0 + j_hi + 1) * G)
        return [(int(L), int(j) + self.j0, int(i) + self.i0) for L, j, i in out[:n]]

    def search_py(self, sources, targets, tcenter, max_exp=4_000_000, start_dir=None, end_dir=None):
        """Pure-Python reference of the kernel in tools/astar.c (same states, costs and tie-breaking)."""
        nx, ny = self.nx, self.ny
        src, tgt = self.local(sources), self.local(targets)
        if not src or not tgt:
            return None
        tx = ty = None
        if tcenter is not None:
            tx, ty = gi(tcenter[0], X0) - self.i0, gi(tcenter[1], Y0) - self.j0
        soft = self.soft is not None
        heap, g = [], {}
        d0 = DIRS.index(start_dir) if start_dir is not None else 8
        for (L, j, i) in src:
            if self.flat[L][j * nx + i] or (soft and self.softf[L][j * nx + i]):
                k = (L, j, i, d0)
                g[k] = 0.0
                heapq.heappush(heap, (self._h(i, j, tx, ty), 0.0, L, j, i, d0))
        endd = DIRS.index(end_dir) if end_dir is not None else None
        prev = {}
        exp = 0
        while heap:
            f, gc, L, j, i, d = heapq.heappop(heap)
            if g.get((L, j, i, d), 1e18) < gc:
                continue
            if (L, j, i) in tgt and (endd is None or d == endd or d == 8):
                path = [(L, j, i)]
                k = (L, j, i, d)
                while k in prev:
                    k = prev[k]
                    path.append(k[:3])
                return [(L, j + self.j0, i + self.i0) for (L, j, i) in path[::-1]]
            exp += 1
            if exp > max_exp:
                return None
            lc = self.layer_cost[L]
            for nd, (di, dj) in enumerate(DIRS):
                if d != 8:
                    turn = min((nd - d) % 8, (d - nd) % 8)
                    if turn > self.max_turn:
                        continue
                    tcost = TURN_COST.get(turn, 0.0)
                else:
                    tcost = 0.0
                ni, nj = i + di, j + dj
                if not (0 <= ni < nx and 0 <= nj < ny):
                    continue
                idx = nj * nx + ni
                pen = 0.0
                if not self.flat[L][idx]:
                    if soft and self.softf[L][idx]:
                        pen = 5.0
                    else:
                        continue
                if di and dj:
                    if not (self.flat[L][j * nx + ni] or (soft and self.softf[L][j * nx + ni])) or \
                       not (self.flat[L][nj * nx + i] or (soft and self.softf[L][nj * nx + i])):
                        continue
                if L == 0 and self.axisf is not None:
                    a1, a2 = self.axisf[j * nx + i], self.axisf[idx]
                    if a1 or a2:
                        need = 2 if di else 1
                        if (di and dj) or (a1 and not a1 & need) or (a2 and not a2 & need):
                            continue
                step = (SQ2 if di and dj else 1.0) * lc
                if self.hugf[L][idx]:
                    step += HUG_COST
                if self.bandf[idx]:
                    step += EDGE_COST
                if self.fbandf[idx]:
                    step += self.fan_cost
                ng = gc + step + tcost + pen
                k = (L, nj, ni, nd)
                if ng < g.get(k, 1e18):
                    g[k] = ng
                    prev[k] = (L, j, i, d)
                    heapq.heappush(heap, (ng + self._h(ni, nj, tx, ty), ng, L, nj, ni, nd))
            idx = j * nx + i
            ok = self.viaf[idx] or (soft and self.viasf[idx])
            if ok:
                for OL in self.layers:
                    if OL == L or not (self.flat[OL][idx] or (soft and self.softf[OL][idx])):
                        continue
                    pen = 0.0 if self.viaf[idx] else 5.0
                    k = (OL, j, i, d)
                    ng = gc + self.via_cost + pen
                    if ng < g.get(k, 1e18):
                        g[k] = ng
                        prev[k] = (L, j, i, d)
                        heapq.heappush(heap, (ng + self._h(i, j, tx, ty), ng, OL, j, i, d))
        return None

    def _h(self, i, j, tx, ty):
        if tx is None:
            return 0.0
        dx, dy = abs(i - tx), abs(j - ty)
        return self.hw * ((dx + dy) + (SQ2 - 2) * min(dx, dy))


def cell_widths(R, path, width):
    """Per cell of a path, the widest of width and NECK_WIDTHS that fits there. A power rail necks down only where
    it may reach its own (narrow) pad -- at the pad, or through a dense IC's via fan-out; everywhere else it keeps
    its full width, so a rail is never thinned just to squeeze past another net's via or pad.  The full width is the
    rail's width on that cell's layer (R.lw: wider on In2.Cu); `width` is the one R was built for."""
    ws = [sorted({R.lw[L]} | {w for w in NECK_WIDTHS if w < R.lw[L]}, reverse=True) for L in range(NL)]
    out = []
    for L, j, i in path:
        jj, ii = j - R.j0, i - R.i0
        if R.near is not None and not R.near[jj, ii]:
            out.append(R.lw[L])
            continue
        fd, ed = R.fd[L][jj, ii], R.edge_d[jj, ii]
        out.append(next((w for w in ws[L] if fd >= CLR + w / 2 + EPS and ed >= EDGE_CLR + w / 2), ws[L][-1]))
    return out


def path_to_tracks(path, width):
    """Compress a cell path into straight segments and vias. width: one width, or one per cell (neck-downs): a
    segment then takes the narrowest width of its cells and splits where the width changes."""
    ws = list(width) if isinstance(width, (list, tuple)) else [width] * len(path)
    out = []
    pts = [(L, X0 + i * G, Y0 + j * G) for (L, j, i) in path]

    def emit(k0, k1):
        if pts[k0][1:] != pts[k1][1:]:
            out.append(('seg', (pts[k0][1], pts[k0][2], pts[k1][1], pts[k1][2], min(ws[k0:k1 + 1]), pts[k1][0])))
    start, prevdir = 0, None
    for k in range(len(pts) - 1):
        a, b = pts[k], pts[k + 1]
        if a[0] != b[0]:
            emit(start, k)
            out.append(('via', (a[1], a[2])))
            start, prevdir = k + 1, None
            continue
        d = (round((b[1] - a[1]) / G), round((b[2] - a[2]) / G))
        if prevdir is not None and d != prevdir:
            emit(start, k)
            start = k
        elif ws[k + 1] < ws[k] and k > start:         # narrowing: the narrow part starts at this cell
            emit(start, k)
            start = k
        elif ws[k + 1] > ws[k]:                        # widening: the narrow part ends at the next cell
            emit(start, k + 1)
            start = k + 1
        prevdir = d
    emit(start, len(pts) - 1)
    return out


def track_cells(tr):
    cells = set()
    for kind, data in tr:
        if kind == 'via':
            x, y = data
            for L in range(NL):
                cells.add((L, gi(y, Y0), gi(x, X0)))
        else:
            x1, y1, x2, y2, w, L = data
            n = max(abs(gi(x2, X0) - gi(x1, X0)), abs(gi(y2, Y0) - gi(y1, Y0)))
            for k in range(n + 1):
                t = k / n if n else 0
                cells.add((int(L), gi(y1 + t * (y2 - y1), Y0), gi(x1 + t * (x2 - x1), X0)))
    return cells


def window_of(points, margin):
    xs, ys = [p[0] for p in points], [p[1] for p in points]
    return (min(xs) - margin, min(ys) - margin, max(xs) + margin, max(ys) + margin)


# ── fixed copper ──────────────────────────────────────────────────────────────
def fp_xform(fp):
    """local (footprint, unrotated) -> board coordinates."""
    p = fp.GetPosition()
    cx, cy = MM(p.x), MM(p.y)
    a = math.radians(fp.GetOrientationDegrees())
    c, s = math.cos(a), math.sin(a)
    return lambda x, y: (round(cx + x * c + y * s, 4), round(cy - x * s + y * c, 4))


def usbc_breakout(B, fp, dp, dm):
    """HRO TYPE-C-31-M-12 USB 2.0 row (local coordinates): B7 -0.75 (D-), A6 -0.25 (D+), A7 0.25 (D-),
    B6 0.75 (D+), pads 0.3 x 1.45 at y = -4.04.  D- pads A7/B7 are joined behind the row (under the
    receptacle body, passing A6's end); D+ pads B6/A6 are joined in front of it (passing A7's end).
    The pair then leaves from A6 (D+) / B7 (D-).  Returns the pair terminal (pD+, pD-, out_dir)."""
    T = fp_xform(fp)
    yb = -(4.04 - 0.725 - 0.15 - W_SIG / 2 - 0.05)     # behind the row (towards the body, local +y side)
    yf = -(4.04 + 0.725 + 0.15 + W_SIG / 2 + 0.05)     # in front of the row (away from the body)
    # joins start 0.1 mm inside the pads, so the router sees them as connected (it tests track centre-line cells)
    pad_back, pad_front = -4.04 + 0.725 - 0.1, -4.04 - 0.725 + 0.1
    segs = []
    # D-: A7 (0.25) -> back -> B7 (-0.75)
    segs.append((dm, [(0.25, pad_back), (0.25, yb), (-0.75, yb), (-0.75, pad_back)]))
    # D+: B6 (0.75) -> front -> A6 (-0.25)
    segs.append((dp, [(0.75, pad_front), (0.75, yf), (-0.25, yf), (-0.25, pad_front)]))
    for net, pts in segs:
        bp = [T(*q) for q in pts]
        for a, b in zip(bp, bp[1:]):
            B.add_track(net, 'seg', (a[0], a[1], b[0], b[1], W_SIG, 0), fixed=True)
    # pair terminal: D+ from A6's front join point, D- from B7's front end
    pP = T(-0.25, yf)
    pN = T(-0.75, pad_front)
    B.add_track(dm, 'seg', (pN[0], pN[1], *T(-0.75, yf), W_SIG, 0), fixed=True)
    pN = T(-0.75, yf)
    o = T(0, -1)
    c = T(0, 0)
    out = (round(o[0] - c[0]), round(o[1] - c[1]))
    return pP, pN, out


def esd_through(B, fp, dm, dp):
    """USBLC6-2SC6 flow-through: I/O1 pins 1 -> 6, I/O2 pins 3 -> 4 (straight under the body)."""
    pads = {p.GetNumber(): p for p in fp.Pads()}
    for net, (a, b) in ((dm, ('1', '6')), (dp, ('3', '4'))):
        pa, pb = pads[a].GetPosition(), pads[b].GetPosition()
        B.add_track(net, 'seg', (MM(pa.x), MM(pa.y), MM(pb.x), MM(pb.y), W_SIG, 0), fixed=True)


def qfn_fanout(B, log):
    """Fine-pitch QFN escapes: a straight W_SIG path from every connected pin out past the pin field is reserved for
    that pin's net while the net is unrouted, so a net routed earlier cannot cross in front of it. The reservation is
    not copper: the net itself routes from its pad any way it likes, and the reservation goes once it is routed
    (Board.release_escape). Nets in FANOUT_LEN get a real fixed stub instead (the AT1 SPI bus's route)."""
    n = 0
    for ref in QFN_FANOUT:
        fp = B.board.FindFootprintByReference(ref)
        c = fp.GetPosition()
        cx, cy = MM(c.x), MM(c.y)
        for pad in fp.Pads():
            name = pad.GetNetname()
            if not name or name.startswith('unconnected') or (ref, pad.GetNumber()) in EP_PADS:
                continue
            p, bb = pad.GetPosition(), pad.GetBoundingBox()
            px, py = MM(p.x), MM(p.y)
            dx, dy = px - cx, py - cy
            side = ('l' if dx < 0 else 'r') if abs(dx) > abs(dy) else ('t' if dy < 0 else 'b')
            length = FANOUT_LEN.get(name, QFN_FANOUT[ref][side])
            if abs(dx) > abs(dy):
                tip = MM(bb.GetLeft()) - length if dx < 0 else MM(bb.GetRight()) + length
                end = (round(tip / G) * G, py)
            else:
                tip = MM(bb.GetTop()) - length if dy < 0 else MM(bb.GetBottom()) + length
                end = (px, round(tip / G) * G)
            if name in FANOUT_LEN:
                B.add_track(name, 'seg', (px, py, end[0], end[1], W_SIG, 0), fixed=True)
            else:
                B.escape.setdefault(name, []).append(('seg', (px, py, end[0], end[1], W_SIG, 0)))
                B.escape_on.add(name)
            n += 1
    B.rebuild()
    log.append(f'QFN escapes: {n} ({sum(len(v) for v in B.escape.values())} reserved, the rest fixed bus stubs)')


FAN_DEBUG = []


def seg_len(items):
    return sum(math.hypot(d[2] - d[0], d[3] - d[1]) for k, d in items if k == 'seg')


def fanout_path(q, lane):
    """One dense-IC pin's planned way out: radial through the escape zone, a 45-degree jog to its lane, on to the via at
    the planned depth.  (points, items) with the via last, both on grid."""
    side, tip = q['side'], q['tip']
    out = -1 if side in 'lt' else 1
    jog = round((lane - q['along']) / G) * G
    r1 = round((tip + out * q['start']) / G) * G
    r2 = r1 + out * abs(jog)
    r3 = tip + out * q['depth']
    pts = [(q['px'], q['py'])]
    if side in 'lr':
        pts += [(r1, q['along']), (r2, q['along'] + jog), (r3, q['along'] + jog)]
    else:
        pts += [(q['along'], r1), (q['along'] + jog, r2), (q['along'] + jog, r3)]
    pts = [(round(x / G) * G, round(y / G) * G) for x, y in pts]
    items = [('seg', (x1, y1, x2, y2, W_SIG, 0)) for (x1, y1), (x2, y2) in zip(pts, pts[1:])
             if math.hypot(x2 - x1, y2 - y1) > 1e-6] + [('via', pts[-1])]
    return pts, items


def dense_fanout(B, log, ref):
    """Staggered via fan-out for every used pin of a dense IC (USB and SPI pins first, then north, south, east, west).
    USB pair pins keep their pair fan-out (a lane the fan stays out of), plane pins with a via already are skipped.
    Per side, the pins between pins already routed (the SPI buses) form segments; a segment's via positions are its
    pins spread to FANOUT_STEP along the side (centred on them), alternately in a near and a far row, so vias in a
    row are two steps apart and each far-row track passes through the middle of a near-row gap. Rows sit just
    beyond the widest jog. Pins with the widest jog are fanned first. The path and via are reserved for the pin's
    net (plane pins get them as copper), and the net may use them or not."""
    cfg = FANOUT_IC[ref]
    fp = B.board.FindFootprintByReference(ref)
    c = fp.GetPosition()
    cx, cy = MM(c.x), MM(c.y)
    pins, fixed_at = [], {s: [] for s in 'lrtb'}
    row = {s: [] for s in 'lrtb'}              # along-side positions of each side's pads (used or not)
    for pad in fp.Pads():
        if pad.GetNumber().isdigit() and (ref, pad.GetNumber()) not in EP_PADS:
            px, py = MM(pad.GetPosition().x), MM(pad.GetPosition().y)
            if abs(px - cx) > abs(py - cy):
                row['l' if px < cx else 'r'].append(py)
            else:
                row['t' if py < cy else 'b'].append(px)
    open_plane = {key for pn in PLANES for key in plane_open_pads(B, pn)}
    for pad in fp.Pads():
        name = pad.GetNetname()
        if not name or name.startswith('unconnected') or not pad.GetNumber().isdigit() or (ref, pad.GetNumber()) in EP_PADS:
            continue
        key = f'{ref}.{pad.GetNumber()}'
        if key in getattr(B, 'tied_pins', ()):
            continue                                  # tied to its cap's via inwards: no outward stub, no lane
        if key in FANOUT_SKIP:
            continue
        if name in PLANES and key not in open_plane:
            continue                                  # has its via already
        shapes = [sh for k, sh in B.pads.get(name, []) if k == key]
        p, bb = pad.GetPosition(), pad.GetBoundingBox()
        px, py = MM(p.x), MM(p.y)
        if abs(px - cx) > abs(py - cy):
            side = 'l' if px < cx else 'r'
            tip, along = (MM(bb.GetLeft()) if side == 'l' else MM(bb.GetRight())), py
        else:
            side = 't' if py < cy else 'b'
            tip, along = (MM(bb.GetTop()) if side == 't' else MM(bb.GetBottom())), px
        if name in FANOUT_LEN or name in B.pair_of or not shapes or len(terminals(B, name)) < 2 and name not in PLANES \
                or key in FANOUT_SURFACE:
            fixed_at[side].append(along)              # bus stubs / pairs / routed already: a lane the fan keeps out of
            continue
        pins.append(dict(side=side, along=along, tip=tip, name=name, key=key, shapes=shapes[0], px=px, py=py))
    plan = []
    for side in 'lrtb':
        sp = sorted((q for q in pins if q['side'] == side), key=lambda q: q['along'])
        segs, cur = [], []
        for q in sp:                                  # split at fixed lanes
            if cur and any(cur[-1]['along'] < f < q['along'] for f in fixed_at[side] + cfg.get('lanes', {}).get(side, [])):
                segs.append(cur)
                cur = []
            cur.append(q)
        if cur:
            segs.append(cur)
        lanes = cfg.get('lanes', {}).get(side, [])
        zone = cfg['zone'][side]
        ends = [q['along'] for q in pins if q['side'] == side] + fixed_at[side]
        for seg in segs:
            a = [q['along'] for q in seg]
            lo = max([f + 0.6 for f in fixed_at[side] if f < a[0]] + [f + PAIR_LANE for f in lanes if f < a[0]]
                     + [min(ends) - cfg['end'][side]])
            hi = min([f - 0.6 for f in fixed_at[side] if f > a[-1]] + [f - PAIR_LANE for f in lanes if f > a[-1]]
                     + [max(ends) + cfg['end'][side]])

            def spread(step):
                v = list(a)
                for i in range(1, len(v)):
                    v[i] = max(v[i], v[i - 1] + step)
                shift = sum(a) / len(a) - sum(v) / len(v)
                shift = min(max(shift, lo - v[0]), hi - v[-1])
                return [round((x + shift) / G) * G for x in v]
            # every jog must be done before the near row: shrink the step (not below the pin pitch) until it is
            jog_room = cfg['max'][side] - FANOUT_ROW_GAP - zone - 0.4

            def starts(v):
                # parallel 45-degree jogs 0.4 mm apart would sit 0.28 mm apart: pins jogging the same way start one
                # after the other, the outermost first, 0.1 mm apart (then they stay 0.35 mm apart)
                st = [0.0] * len(v)
                for sgn in (1, -1):
                    movers = sorted((i for i in range(len(v)) if (v[i] - a[i]) * sgn > G / 2),
                                    key=lambda i: -abs(v[i] - a[i]))
                    for r, i in enumerate(movers):
                        st[i] = r * 2 * G           # two grid steps: still clear once the diagonals land on grid cells
                return st

            def reach(v):      # jogs start 0.2 mm past the zone, clear of the neighbours' reserved lane ends
                return 0.2 + max(s + abs(x - y) for s, x, y in zip(starts(v), v, a))
            step = cfg['step'][side]
            v = spread(step)
            while step > FANOUT_STEP_MIN and (reach(v) > jog_room + 0.06 or v[-1] - v[0] > hi - lo + 0.06):
                step -= 0.025
                v = spread(step)
            st = starts(v)
            near = min(zone + reach(v) + 0.4, cfg['max'][side] - FANOUT_ROW_GAP)
            for i, q in enumerate(seg):
                depth = near + (FANOUT_ROW_GAP if i % 2 else 0.0)
                q.update(v=v[i], row=i % 2, depth=round(depth / G) * G, jog=abs(v[i] - a[i]), zone=zone,
                         start=zone + 0.2 + st[i], pitch=max(2 * step - 0.01, 1.05))
                plan.append(q)
    # Each pin's planned way out, as keep-out lines for the site search: once the lanes are a full VIA_LANE_SEP apart,
    # every one of them can carry a via, so no site may be put *on* a neighbour's lane (that is how U501.47 lost its way
    # out: pin 48's fallback site landed 0.25 mm from 47's lane).  Lanes closer together than that are left to the
    # placement order - no arrangement can give both of them a via.
    lanes_geo = [(q['key'], q['side'], list(zip(fanout_path(q, q['v'])[0], fanout_path(q, q['v'])[0][1:])))
                 for q in plan]
    lane_at = {q['key']: q['v'] for q in plan}

    def on_neighbour_lane(x, y, key, side):
        mine = lane_at.get(key)
        for k2, s2, segs2 in lanes_geo:
            if k2 == key or s2 != side:
                continue
            if mine is not None and abs(lane_at.get(k2, mine) - mine) < VIA_LANE_SEP - 1e-6:
                continue
            for (ax, ay), (bx, by) in segs2:
                if seg_seg_dist((x, y), (x, y), (ax, ay), (bx, by)) < VIA_LANE_SEP - 1e-6:
                    return True
        return False

    first = [q for q in plan if re.match(r'(.*_D[PM](_PIN)?|AT[12]_(MISO|MOSI|SCK|CS))$', q['name'])]
    order = list(first)
    # Per side, the pins at the *ends* of a segment go first and the middle ones last (outside-in): a pin in the middle
    # can jog either way, one at an end only inwards, and the end pins are the ones an earlier-placed neighbour's lane
    # boxes in (rev 9: SVC_SEL and U501.76/77/U1001.8 were the segment's outermost, and they were the ones left
    # without a via).  Signals before plane pins: a signal pin without a via is trapped for good, a plane pin can still
    # be dropped onto its plane by plane_fanout().
    for side in FANOUT_SIDES:
        sp = sorted((q for q in plan if q['side'] == side and q not in first), key=lambda q: q['along'])
        out_in = []
        while sp:
            out_in += [sp.pop(0)] + ([sp.pop()] if sp else [])
        # outside-in, then the same order again for the plane pins of that side
        order += ([q for q in out_in if q['name'] not in PLANES] + [q for q in out_in if q['name'] in PLANES])
    placed = {(side, r): [] for side in 'lrtb' for r in (0, 1)}
    n_res = n_cu = 0
    missing, lanes_only = [], []
    for qi, q in enumerate(order):
        side, name, tip = q['side'], q['name'], q['tip']
        out = -1 if side in 'lt' else 1
        # The planned path: radial through the escape zone, a 45-degree jog to the via, on to the via.  If it is blocked,
        # the same plan with a *shorter* radial and/or its lane nudged by a grid step or two: a pin on a crowded 0.4 mm
        # row is boxed in by the end of its own radial and by the diagonal of the pin below it (U501.47's radial ran
        # 0.283 mm from the corner where U501.48's reserved path steps up to clear R304 - 0.017 mm short of the 0.30 mm
        # two tracks need - and ending it 0.15 mm earlier put its jog 0.204 mm from U501.46's jog, 0.021 mm short of the
        # router's own threshold).  Ending the radial early *and* landing the lane 0.05 mm higher clears both.  Only then
        # the site search.
        tx = tip + out * q['depth'] if side in 'lr' else q['v']
        ty = q['v'] if side in 'lr' else tip + out * q['depth']
        items, ok = None, False
        for strict in (True, False):                # a reservation only holds space: 0.204 mm of copper clearance there
            for back, dl in [(b, d) for b in (0.0, 0.05, 0.10, 0.15, 0.20, 0.30, 0.40, 0.50, 0.65, 0.80)
                             for d in (0.0, -0.05, 0.05, -0.10, 0.10, -0.15, 0.15)]:
                cand = dict(q, start=max(q['start'] - back, 0.0))
                cpts, citems = fanout_path(cand, q['v'] + dl)
                if not any(math.hypot(cpts[-1][0] - vx, cpts[-1][1] - vy) < q['pitch']
                           for vx, vy in placed[(side, q['row'])]) \
                        and not any(abs(cpts[-1][0 if side in 'lr' else 1] - f) < PAIR_LANE
                                    for f in cfg.get('lanes', {}).get(side, [])) \
                        and not on_neighbour_lane(cpts[-1][0], cpts[-1][1], q['key'], side) \
                        and items_legal(B, name, [(name, k, d) for k, d in citems], strict=strict):
                    pts, items, ok = cpts, citems, True
                    break
                if items is None:
                    pts, items = cpts, citems      # keep the planned one for the report and the site search
            if ok:
                break
        if ok:
            FAN_DEBUG.append((q['key'], side, (round(tx, 2), round(ty, 2)), 'ok'))
            placed[(side, q['row'])].append(pts[-1])
            if name in PLANES:
                for kind, data in items:
                    B.add_track(name, kind, data)
                n_cu += 1
            else:
                B.escape[name] = [it for it in B.escape.get(name, []) if it[1][:2] != (q['px'], q['py'])] + items
                B.escape_on.add(name)
                for kind, data in items:
                    B._add_dyn(name, kind, data)
                n_res += 1
            continue
        FAN_DEBUG.append((q['key'], side, 'planned path blocked', items_conflict(B, name, [(name, k, d) for k, d in items])))
        # any legal site near the planned one, outside the escape zone and clear of the IC's other fan-out vias
        spots = {}
        others = [v for vs in placed.values() for v in vs]
        # The stub keeps off the planned way out of the same-side pins still to come (as on_neighbour_lane does for the
        # sites): a later signal pin's whole path (radial, jog, lane), a later plane pin's radial only when this pin is
        # a signal - the plane pin needs no more than that straight exit to be dropped onto its plane afterwards, and
        # signals come first for that reason (see the order above).  The stub is also a *shortest* path (h_weight 1.0;
        # the detour check below compares it with the planned one).  Both used to hold by accident: fan_keep charged
        # FAN_COST in the pin's own chip's band, which scaled every step by 4 against the heuristic and made the search
        # near-optimal with a tie-break that ran straight out first.  Once fan_keep was fixed, U901.24's stub jogged at
        # the pin tips, 0.208 mm from U901.23's radial, and U801.23 / U901.23 (+3V3) lost their sites.
        later = {o['key']: o['name'] for o in order[qi + 1:] if o['side'] == side}
        keep_clear = [(ax, ay, bx, by) for k2, s2, segs2 in lanes_geo if k2 in later
                      for (ax, ay), (bx, by) in (segs2[:1] if name not in PLANES and later[k2] in PLANES else segs2)]
        R = NetRouter(B, name, W_SIG, (q['px'] - 10, q['py'] - 10, q['px'] + 10, q['py'] + 10), layers=(0,),
                      h_weight=1.0, avoid=keep_clear)
        path = None
        # (radius, shallowest depth a site may sit at).  The last pass exists because a deep lane can be shut by a part
        # across it while the pin's own row is still free close in: U501.48's lane at 137.20 runs under R304's pads, so
        # every site at the planned depth (x 155.65) is inside them and only a site just past the pad tip is legal.
        # Each pass is *searched*, not just collected: a set of deep sites that no track can reach used to stop the
        # shallower pass from ever being tried.
        for spot_r, min_depth in ((FANOUT_SPOT_R, q['zone'] + 0.6), (3.6, q['zone'] + 0.6), (3.6, 0.55),
                                  (4.8, 0.4), (6.0, 0.3), (7.5, 0.2)):
            n = int(spot_r / G)
            spots = {}
            for di in range(-n, n + 1):
                for dj in range(-n, n + 1):
                    if math.hypot(di, dj) * G > spot_r:
                        continue
                    x, y = round(tx / G) * G + di * G, round(ty / G) * G + dj * G
                    # in front of the side's pins: beyond the escape zone; in the corner space past the pin row's
                    # end, just clear of the pins
                    a = y if side in 'lr' else x
                    front = min(row[side]) - 0.3 <= a <= max(row[side]) + 0.3
                    if ((x - tip) * out if side in 'lr' else (y - tip) * out) < (min_depth if front else 0.3):
                        continue
                    if any(math.hypot(x - vx, y - vy) < FANOUT_VIA_GAP for vx, vy in others):
                        continue
                    if any(abs(a - f) < PAIR_LANE for f in cfg.get('lanes', {}).get(side, [])):
                        continue                     # a lane a pair runs through: keep the vias off it too
                    if on_neighbour_lane(x, y, q['key'], side):
                        continue                     # a via on a neighbouring pin's lane would trap that pin
                    if legal_via(B, x, y, name):
                        spots[(0, gi(y, Y0), gi(x, X0))] = (x, y)
            if spots:
                path = R.search(pad_cells(q['shapes'], layers=(0,)), set(spots), (tx, ty), max_exp=1_000_000)
                if path:
                    break
        if path is None:
            # GUARANTEE (rev 9, a hard rule): every pad that needs a fan-out gets a stub *and* a via site.  The rows above
            # can all be full - a part across the lane, a pair through the band - so the last word is a cell-by-cell scan
            # of the whole side's band (0.30 mm to 2.6 mm past the tips, 2 mm past the row's ends), nearest cell first,
            # with the neighbour-lane veto *dropped*: a pin with no via is dead for good, a neighbour's lane that ends up
            # 0.2 mm crowded is not.  Only the IC's other fan-out vias and the real copper are respected.
            band = []
            a_lo, a_hi = min(row[side]) - 2.0, max(row[side]) + 2.0
            for da in range(int((a_hi - a_lo) / G) + 1):
                a = round((a_lo + da * G) / G) * G
                for dp in range(int(2.3 / G) + 1):
                    dep = round((q['zone'] + 0.30 + dp * G) / G) * G
                    x, y = (round((tip + out * dep) / G) * G, a) if side in 'lr' else (a, round((tip + out * dep) / G) * G)
                    d = math.hypot(x - q['px'], y - q['py'])
                    band.append((d, x, y))
            band.sort()
            for d, x, y in band:
                if any(math.hypot(x - vx, y - vy) < FANOUT_VIA_GAP for vx, vy in others):
                    continue
                if any(abs((y if side in 'lr' else x) - f) < PAIR_LANE
                       for f in cfg.get('lanes', {}).get(side, [])):
                    continue                     # a hand-set pair lane: the one thing a reserved via may not sit on
                if not legal_via(B, x, y, name):
                    continue
                p2 = R.search(pad_cells(q['shapes'], layers=(0,)), {(0, gi(y, Y0), gi(x, X0))}, (x, y),
                              max_exp=200_000)
                if p2:
                    spots, path = {(0, gi(y, Y0), gi(x, X0)): (x, y)}, p2
                    break
        FAN_DEBUG.append((q['key'], side, (round(tx, 2), round(ty, 2)), 'no legal spot' if not spots else
                          ('no path' if not path else 'ok')))
        if not path:
            # No via site for this pin: keep its way out anyway, as a lane-only reservation (added after the whole loop,
            # so it cannot take a site from a pin that has one).  A pin with no reservation at all is the one the
            # signal phase cannot rescue - rev 9 measured 15 of them (U501.49/50/51, U701.25/27, U801/U901.26/27,
            # U1001.2/5/6/7/9/10/11, U1101.8), and the nets that needed those pins were walled in by the very next net
            # routed past their exit (VSNS_PWR by U501.48's fan-out via 0.7 mm from R304.1, H0_CFG1 by HUB0_RST's lane
            # through the 1 mm funnel beside U701.25).  The straight radial run is what every pin has; the net leaves
            # along it and turns where it ends.
            if name not in PLANES:
                lane_end = ((round((tip + out * q['zone']) / G) * G, q['py']) if side in 'lr'
                            else (q['px'], round((tip + out * q['zone']) / G) * G))
                lanes_only.append((q, [('seg', (q['px'], q['py'], lane_end[0], lane_end[1], W_SIG, 0))]))
            missing.append(q['key'])
            continue
        vx, vy = spots[path[-1]]
        items_fb = path_to_tracks(path, W_SIG)
        if seg_len(items_fb) > seg_len(items) + 1.5:     # no winding detours (a plane pin's via stays close)
            FAN_DEBUG.append((q['key'], side, (round(vx, 2), round(vy, 2)), 'no path'))
            missing.append(q['key'])
            continue
        placed[(side, q['row'])].append((vx, vy))
        items = items_fb + [('via', (vx, vy))]
        if name in PLANES:
            for kind, data in items:
                B.add_track(name, kind, data)
            n_cu += 1
        else:
            B.escape[name] = [it for it in B.escape.get(name, []) if it[1][:2] != (q['px'], q['py'])] + items
            B.escape_on.add(name)
            for kind, data in items:
                B._add_dyn(name, kind, data)
            n_res += 1
    for q, lane in lanes_only:                 # no via site: keep the pin's way out as a lane-only reservation
        name = q['name']
        if items_legal(B, name, [(name, k, d) for k, d in lane], strict=False):
            B.escape[name] = [it for it in B.escape.get(name, []) if it[1][:2] != (q['px'], q['py'])] + lane
            B.escape_on.add(name)
            for kind, data in lane:
                B._add_dyn(name, kind, data)
    B.rebuild()
    log.append(f'{ref} fan-out: {n_res} pins with a reserved via, {n_cu} plane pins with their via, no site for {missing}'
               + (f', {len(lanes_only)} of them keep a lane' if lanes_only else ''))
    if (VIA_DEBUG and ref in ('U801', 'U901')) or os.environ.get('ROUTE_FAN_DEBUG') in (ref, 'all'):
        for e in FAN_DEBUG:
            print('    fan', *e, flush=True)
        for q in plan:
            print(f"    plan {q['key']:<9s} side {q['side']} along {q['along']:7.2f} lane {q['v']:7.2f} "
                  f"depth {q['depth']:5.2f} row {q['row']} jog {q['jog']:5.2f} start {q['start']:5.2f}", flush=True)
        for s in 'lrtb':
            for r in (0, 1):
                if placed[(s, r)]:
                    print(f'    {ref} side {s} row {r} vias: {[(round(v[0], 2), round(v[1], 2)) for v in placed[(s, r)]]}',
                          flush=True)
    print(log[-1], flush=True)


def pad_escapes(B, log, length=1.5):
    """A way out for the pads of the parts below, the same way a dense IC's pins get one: a reserved straight run of
    `length` plus a via site, held for the pad's net while that net is unrouted, so a net routed earlier cannot
    close it off.  'edge' parts escape away from the nearest board edge (a status LED's pads sit 0.5 mm from the
    front edge, its only other option is the board edge), 'radial' parts away from their own centre.  The hubs'
    strap and control pins are reserved *now* because the upstream pair's westward run is routed over them later."""
    jobs = [(kind, ref, None) for kind, refs in PAD_ESCAPE.items() for ref in refs]
    jobs += [('radial', ref, pins) for ref, pins in PAD_ESCAPE_PINS.items()]
    n = 0
    for kind, ref, pins in jobs:
        fp = B.board.FindFootprintByReference(ref)
        if fp is None:
            continue
        c = fp.GetPosition()
        cx, cy = MM(c.x), MM(c.y)
        for pad in fp.Pads():
            if pins is not None and pad.GetNumber() not in pins:
                continue
            name = pad.GetNetname()
            if not name or name.startswith('unconnected') or name in PLANES or name in B.pair_of or mate_of(name):
                continue                     # planes get a via from plane_fanout, pairs have their own corridors
            if (ref, pad.GetNumber()) in EP_PADS:
                continue
            p = pad.GetPosition()
            px, py = MM(p.x), MM(p.y)
            if kind == 'edge':
                d = min((px - X0, (1.0, 0.0)), (X1 - px, (-1.0, 0.0)),
                        (py - Y0, (0.0, 1.0)), (Y1 - py, (0.0, -1.0)))[1]
            else:
                dx, dy = px - cx, py - cy
                d = (1.0 if dx > 0 else -1.0, 0.0) if abs(dx) >= abs(dy) else (0.0, 1.0 if dy > 0 else -1.0)
            ex = round((px + d[0] * length) / G) * G
            ey = round((py + d[1] * length) / G) * G
            items = [('seg', (px, py, ex, ey, W_SIG, 0)), ('via', (ex, ey))]
            B.escape[name] = [it for it in B.escape.get(name, []) if it[1][:2] != (px, py)] + items
            B.escape_on.add(name)
            for k, dd in items:
                B._add_dyn(name, k, dd)
            n += 1
    for (ref, num), (sx, sy, vx, vy) in list(PAD_ESCAPE_INWARD.items()) + list(PAD_ESCAPE_TO.items()):
        # a pad whose only way out is a via: into its own part (U301.5) or into a pocket an earlier net would close
        # (U201.5, R303.2 - see the tables)
        fp = B.board.FindFootprintByReference(ref)
        if fp is None:
            continue
        for pad in fp.Pads():
            if pad.GetNumber() != num:
                continue
            name = pad.GetNetname()
            p = pad.GetPosition()
            px, py = MM(p.x), MM(p.y)
            ex = round((px + vx) / G) * G
            ey = round((py + vy) / G) * G
            items = [('seg', (round((px + sx) / G) * G, round((py + sy) / G) * G, ex, ey, W_SIG, 0)), ('via', (ex, ey))]
            B.escape[name] = [it for it in B.escape.get(name, []) if it[1][:2] != (px, py)] + items
            B.escape_on.add(name)
            for k, dd in items:
                B._add_dyn(name, k, dd)
            n += 1
    for (ref, num), pts in PAD_ESCAPE_PATH.items():
        # a pad whose way out is a bend: the same reservation, as the polyline from the pad to the via (U501.48)
        fp = B.board.FindFootprintByReference(ref)
        if fp is None:
            continue
        for pad in fp.Pads():
            if pad.GetNumber() != num:
                continue
            name = pad.GetNetname()
            p = pad.GetPosition()
            px, py = round(MM(p.x) / G) * G, round(MM(p.y) / G) * G
            apath, with_via = pts, True
            if isinstance(pts, tuple):                      # (points, False): the corridor is wide enough for the
                apath, with_via = pts[0], bool(pts[1])       # track but not for a via - the net routes on from its end
            way = [(px, py)] + [(round(x / G) * G, round(y / G) * G) for x, y in apath]
            items = [('seg', (x1, y1, x2, y2, W_SIG, 0)) for (x1, y1), (x2, y2) in zip(way, way[1:])]
            if with_via:
                items.append(('via', way[-1]))
            B.escape[name] = [it for it in B.escape.get(name, []) if it[1][:2] != (px, py)] + items
            B.escape_on.add(name)
            for k, dd in items:
                B._add_dyn(name, k, dd)
            n += 1
    B.rebuild()
    log.append(f'pad escapes: {n} (connectors, LEDs and their caps, the power path)')


def pin_cap_links(B, log):
    """Tie every bottom-side decoupling cap to the pin it decouples, as *fixed* copper (rev 9, 2026-10-26).

    A cap under the chip is pointless if the pin and the cap only meet in the In2 plane: the pin's escape via goes
    wherever the fan-out has room, the cap's via goes wherever the plane fan-out finds a site, and the loop between them
    is the plane - the cap ends up decoupling "the rail", not that pin.  Here the pin's own line runs straight to a via
    in the bare-laminate gap between the pin's pad and the cap's pad (gen_pcb.py leaves 1.1 mm: 0.6 via + 2 x 0.15
    clearance + 0.2 margin), and that via drops straight onto the cap's pad on B.Cu:

        chip pin (F.Cu) --0.8 mm--> via --> 0.7 mm --> cap pad (B.Cu),   plane = the return path

    All three items are fixed copper added in step 1, so no later phase can rip them up, and plane_fanout() finds the
    via already in place instead of choosing an arbitrary +3V3 site.  The cap-to-pin pairing is written by gen_pcb.py
    to tools/build/pin_caps.json; without that file nothing happens (an older board still routes as before).
    """
    path = os.path.join(HERE, 'tools', 'build', 'pin_caps.json')
    if not os.path.exists(path):
        log.append('pin-cap ties: no pairing file (gen_pcb.py writes tools/build/pin_caps.json)')
        return 0
    try:
        pairs = json.load(open(path))
    except Exception as e:                                  # a half-written file must not stop a run
        log.append(f'pin-cap ties: unreadable pairing file ({e})')
        return 0
    n, missed, ties = 0, [], []
    B.tied_pins = getattr(B, 'tied_pins', set())
    for cap, info in sorted(pairs.items()):
        chip, pin, net = info.get('chip'), info.get('pin'), info.get('net')
        cfp, rfp = B.board.FindFootprintByReference(chip), B.board.FindFootprintByReference(cap)
        if cfp is None or rfp is None:
            missed.append(f'{cap}: footprint missing')
            continue
        pad = next((p for p in cfp.Pads() if p.GetNumber() == pin), None)
        cpad = next((p for p in rfp.Pads() if p.GetNetname() == net), None)
        if pad is None or cpad is None:
            missed.append(f'{cap}: pad missing')
            continue
        if rfp.GetLayer() != pcbnew.B_Cu:
            missed.append(f'{cap}: not on B.Cu')
            continue
        # the cap sits under the chip, so the tie is the short way round inside the package:
        #
        #     chip pin (F.Cu) --0.8 mm--> via (in the bare-laminate gap) --> 0.7 mm --> cap pad (B.Cu)
        #
        # with the In2 plane as the return path.  The pin, that via and its capacitor are then one copper island, so
        # the cap decouples *this pin* rather than the rail as a whole (rev 9, 2026-10-26).
        c = cfp.GetPosition()
        cx, cy = MM(c.x), MM(c.y)
        pp, qq = pad.GetPosition(), cpad.GetPosition()
        px, py, qx, qy = MM(pp.x), MM(pp.y), MM(qq.x), MM(qq.y)
        dx, dy = px - cx, py - cy
        n_ = ((1.0, 0.0) if dx > 0 else (-1.0, 0.0)) if abs(dx) >= abs(dy) else ((0.0, 1.0) if dy > 0 else (0.0, -1.0))
        pb, qb = pad.GetBoundingBox(), cpad.GetBoundingBox()
        pin_half = 0.5 * (MM(pb.GetWidth()) * abs(n_[0]) + MM(pb.GetHeight()) * abs(n_[1]))
        cap_half = 0.5 * (MM(qb.GetWidth()) * abs(n_[0]) + MM(qb.GetHeight()) * abs(n_[1]))
        if info.get('via'):
            # the hubs' VDD33 caps (gen_pcb.py HUB_PIN_CAPS): the cap lies radially under its pin on B.Cu and the via
            # sits just past the pin tip - pin -> F.Cu -> via -> B.Cu -> cap pad.  The spot is gen_pcb's; if it is not
            # legal here the via moves outwards along the pin's axis (it must stay in line with the pin and the cap)
            vx0, vy0 = info['via']
            site = next(((round((vx0 + n_[0] * k * G) / G) * G, round((vy0 + n_[1] * k * G) / G) * G) for k in range(5)
                         if legal_via(B, vx0 + n_[0] * k * G, vy0 + n_[1] * k * G, net)), None)
            if site is None:
                missed.append(f'{cap}: no via site past {chip}.{pin} at ({vx0:.2f}, {vy0:.2f})')
                continue
            vx, vy = site
            B.add_track(net, 'seg', (px, py, vx, vy, W_SIG, 0), fixed=True)
            B.add_track(net, 'via', (vx, vy), fixed=True)
            B.add_track(net, 'seg', (vx, vy, qx, qy, W_SIG, 1), fixed=True)
            ties.append((cap, chip, pin, round(math.hypot(vx - px, vy - py) + math.hypot(qx - vx, qy - vy), 2)))
            B.tied_pins.add(f'{chip}.{pin}')
            n += 1
            continue
        along = -((qx - px) * n_[0] + (qy - py) * n_[1])          # the cap is inward of the pin, hence the minus
        gap = along - pin_half - cap_half
        if gap < VIA_D + 2 * CLR:
            missed.append(f'{cap}: gap {gap:.2f} mm beside {chip}.{pin}')
            continue
        t_ = (-n_[1], n_[0])
        site = None
        # sideways first, then also *along* the gap where it is long enough: two pins at a package corner (AT32 #1's
        # pins 1 and 48) put their caps' vias at the same inside corner, and the second used to find no site at all
        # ("C1003: no via site in the 1.91 mm gap beside U1001.48" - C1001's via sat 0.32 mm away)
        slack = gap / 2 - (VIA_D / 2 + CLR)
        alongs = [a for a in (0.0, 0.15, -0.15, 0.3, -0.3, 0.45, -0.45) if abs(a) <= slack + 1e-6]
        for lat in (0.0, 0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4, 0.5, -0.5, 0.65, -0.65):
            for al in alongs:
                vx = round((px - n_[0] * (pin_half + gap / 2 + al) + t_[0] * lat) / G) * G
                vy = round((py - n_[1] * (pin_half + gap / 2 + al) + t_[1] * lat) / G) * G
                if legal_via(B, vx, vy, net, qfn_ok=True):
                    site = (vx, vy, lat)
                    break
            if site:
                break
        if site is None:
            missed.append(f'{cap}: no via site in the {gap:.2f} mm gap beside {chip}.{pin}')
            continue
        vx, vy, lat = site
        B.add_track(net, 'seg', (px, py, vx, vy, W_SIG, 0), fixed=True)
        B.add_track(net, 'via', (vx, vy), fixed=True)
        B.add_track(net, 'seg', (vx, vy, qx, qy, W_SIG, 1), fixed=True)
        ties.append((cap, chip, pin, round(gap, 2)))
        B.tied_pins.add(f'{chip}.{pin}')
        n += 1
    B.rebuild()
    log.append(f'pin-cap ties: {n} pin(s) wired straight to their cap ({len(pairs)} pairings)')
    for cap, chip, pin, dd in ties:
        log.append(f'  tie {cap} -> {chip}.{pin}: ' + (f'{dd:.2f} mm pin -> via -> cap, via past the pin'
                                                   if pairs[cap].get('via') else f'{dd:.2f} mm gap, via in it'))
    for m in missed:
        log.append(f'  pin-cap tie skipped: {m}')
    return n


def ep_vias(B, log):
    n_added = 0
    for (ref, num), n in EP_PADS.items():
        pad = B.padobj[f'{ref}.{num}'][0][0]
        p = pad.GetPosition()
        cx, cy = MM(p.x), MM(p.y)
        s = MM(pad.GetSize(pcbnew.F_Cu).x)
        pitch = (s - 0.9) / (n - 1)
        for a in range(n):
            for b in range(n):
                x = round((cx - (s - 0.9) / 2 + a * pitch) / G) * G
                y = round((cy - (s - 0.9) / 2 + b * pitch) / G) * G
                B.add_track('GND', 'via', (x, y), fixed=True)
                n_added += 1
    log.append(f'exposed-pad GND vias: {n_added}')


# ── differential pairs ────────────────────────────────────────────────────────
def unit(v):
    L = math.hypot(*v)
    if L < 1e-9:
        # A zero-length step has no direction.  route_pair() asks for the direction of the run it just laid to aim the
        # next end-run; a path whose last two points coincide (a reserved cell reached exactly where the stub ended,
        # which the fan-out's guarantee pass can now produce) must not crash the run - no turn is the right answer.
        return (0.0, 0.0)
    return (v[0] / L, v[1] / L)


def left_of(d):
    """Left-hand normal of travel direction d in board coordinates (y down, viewed from the top)."""
    return (d[1], -d[0])


def pad_terminal(B, ref, pnum, nnum):
    """Pair terminal at two pads of one footprint: (centre P, centre N, outward direction, pads)."""
    pp, _ = B.padobj[f'{ref}.{pnum}'][0]
    pn, _ = B.padobj[f'{ref}.{nnum}'][0]
    fp = pp.GetParentFootprint()
    cb = fp.GetCourtyard(pcbnew.F_CrtYd).BBox()
    ccx, ccy = MM(cb.GetCenter().x), MM(cb.GetCenter().y)
    a = pp.GetPosition()
    b = pn.GetPosition()
    mx, my = (MM(a.x) + MM(b.x)) / 2, (MM(a.y) + MM(b.y)) / 2
    vx, vy = mx - ccx, my - ccy
    out = (1 if vx > 0 else -1, 0) if abs(vx) >= abs(vy) else (0, 1 if vy > 0 else -1)
    return (MM(a.x), MM(a.y)), (MM(b.x), MM(b.y)), out, (pp, pn)


def padpair_terminal(B, refP, pnumP, refN, pnumN, out=None):
    """Pair terminal whose two pads sit on *two* footprints.  The RP2354B's USB pair is split by its 27R series resistors
    (R503/R504, which the RP2350 requires), so the section from those resistors to the switch has one line per part and no
    single footprint offers both.  Same geometry as pad_terminal, with the outward direction taken away from the two
    parts' centroid.

    ``out`` overrides that direction, and it is needed wherever the centroid does not decide it: R707/R708 and R503/R504
    are stacked *along* the pair axis (their pads' midpoint sits within 0.02 mm of the two courtyards' centroid), so the
    tie-break there hangs on that 0.02 mm - and it picks the side away from the other end of the link, which is the one
    direction the pair cannot leave in.  A link that knows where it is going passes the direction instead.
    """
    pp, _ = B.padobj[f'{refP}.{pnumP}'][0]
    pn, _ = B.padobj[f'{refN}.{pnumN}'][0]
    a, b = pp.GetPosition(), pn.GetPosition()
    if out is None:
        c = []
        for p in (pp, pn):
            cb = p.GetParentFootprint().GetCourtyard(pcbnew.F_CrtYd).BBox()
            c.append((MM(cb.GetCenter().x), MM(cb.GetCenter().y)))
        ccx, ccy = (c[0][0] + c[1][0]) / 2, (c[0][1] + c[1][1]) / 2
        mx, my = (MM(a.x) + MM(b.x)) / 2, (MM(a.y) + MM(b.y)) / 2
        vx, vy = mx - ccx, my - ccy
        out = (1 if vx > 0 else -1, 0) if abs(vx) >= abs(vy) else (0, 1 if vy > 0 else -1)
    return (MM(a.x), MM(a.y)), (MM(b.x), MM(b.y)), out, (pp, pn)


def pad_reach(pad, out):
    """Distance from the pad centre to its outer edge along out."""
    s = pad.GetSize(pcbnew.F_Cu)
    a = math.radians(pad.GetOrientationDegrees())
    sx, sy = MM(s.x), MM(s.y)
    hx = abs(sx / 2 * math.cos(a)) + abs(sy / 2 * math.sin(a))
    hy = abs(sx / 2 * math.sin(a)) + abs(sy / 2 * math.cos(a))
    return hx if out[0] else hy


def fanout(term):
    """Stubs from the two pads to the pair's start: returns (escape point, stubs P, stubs N, sideP)."""
    pP, pN, out, pads = term
    mid = ((pP[0] + pN[0]) / 2, (pP[1] + pN[1]) / 2)
    nl = left_of(out)
    sP = 1 if (pP[0] - mid[0]) * nl[0] + (pP[1] - mid[1]) * nl[1] > 0 else -1
    reach = max(pad_reach(pads[0], out), pad_reach(pads[1], out)) if pads else 0.0
    half = math.hypot(pP[0] - mid[0], pP[1] - mid[1])
    lat = max(half - PAIR_O, 0.0)
    straight = reach + 0.1
    if pads and pads[0].GetParentFootprint().GetValue() == 'USBLC6-2SC6':
        # Room for the via of the pin that sits *between* the two lines, so the pair's stubs can fan past it.  Whether
        # that pin gets a via there at all is a per-footprint question, not a net-class one:
        #   - a plane pin gets one (the `between` rule / plane_fanout), as on the ESDs' 3/1 pair, where pin 2 is GND;
        #   - a non-plane pin only gets one if its footprint is on the pad_escapes list *and* that escape comes out the
        #     same side the pair leaves.  The port ESDs U1201/U1301/U1401 are listed, and their pin 5 (VBUS_KB/...)
        #     escapes radially - which for that pad is the pair's own side - so its 1.5 mm stub and via land in the
        #     pair's corridor: without the room the stubs reach it only 0.46 mm away where the via plus clearance needs
        #     0.5 mm (measured: PAIR FAIL KB_DP/KB_DM "(111.50,156.30) blocked by ['KB_DM', 'VBUS_KB']", likewise MS
        #     and OT - three ports, six links).
        #   - U301 is not on that list (nothing leaves that way), so its pair on 4/6 must not take the room: its
        #     escape would reach 2.26 mm and run into Hub-0's north pad row, and SVC_DP cannot route at all
        #     ("no pair path from (0, 272, 842) to (0, 368, 1200)").  That is the rev-8 regression this rule fixes.
        fp = pads[0].GetParentFootprint()
        mx = (pads[0].GetPosition().x + pads[1].GetPosition().x) / 2
        my = (pads[0].GetPosition().y + pads[1].GetPosition().y) / 2
        between = min(fp.Pads(), key=lambda p: (p.GetPosition().x - mx) ** 2 + (p.GetPosition().y - my) ** 2)
        via_beside = between.GetNetname() in PLANES
        if not via_beside and fp.GetReference() in PAD_ESCAPE_SET:
            c = fp.GetPosition()
            dx = MM(between.GetPosition().x) - MM(c.x)
            dy = MM(between.GetPosition().y) - MM(c.y)
            d = ((1, 0) if dx > 0 else (-1, 0)) if abs(dx) >= abs(dy) else ((0, 1) if dy > 0 else (0, -1))
            via_beside = d == (int(out[0]), int(out[1]))
        if via_beside:
            straight += 0.5
    # rev 9, 2026-10-26: the ESDs' escape neck went down and 2-3 mm wider, so that the middle pin of each ESD can
    # get out below it.  U1201, U1301 and U1401 had a straight run of USB wires before the lines converge, U101,
    # U301 and U201 did not (their top straight run was longer than the bottom one); the neck was then balanced
    # so that U301, U201 and U101 match U1301, U1201 and U1101:
    # The 0.5 mm above is granted only where a via sits *between* the pair's two lines.  On the input ESDs' north pair
    # that is pin 2 (GND, a plane pin, always gets one) - hence reach+0.6 = 1.26 mm there - while their south pair
    # straddles pin 5 (the port's VBUS), whose escape goes north into the slot (PAD_ESCAPE_INWARD), so that side had no
    # room at all and converged after 0.76 mm, right under the ESD.  ESD_NECK gives those three south pairs the very
    # same 0.5 mm the output ports' ESDs get, so all six ESDs read alike: both lines leave the 4/6 pads 1.90 mm apart,
    # run 1.26 mm straight and only then turn in (measured before: 0.76 mm below U101/U301 against 1.26 mm above them).
    if pads and out == (0, 1):
        straight += ESD_NECK.get(pads[0].GetParentFootprint().GetReference(), 0.0)
    esc_d = straight + lat + 0.15
    esc_d = math.ceil(esc_d / G) * G
    esc = (round((mid[0] + out[0] * esc_d) / G) * G, round((mid[1] + out[1] * esc_d) / G) * G)
    stubs = {}
    for name, p, s in (('P', pP, sP), ('N', pN, -sP)):
        a = (p[0] + out[0] * straight, p[1] + out[1] * straight)
        e = (esc[0] + nl[0] * s * PAIR_O, esc[1] + nl[1] * s * PAIR_O)
        b = (e[0] - out[0] * 0.15, e[1] - out[1] * 0.15)
        # 45-degree jog from a to b: move along out by the lateral distance
        latd = (b[0] - a[0]) * nl[0] + (b[1] - a[1]) * nl[1]
        alongd = (b[0] - a[0]) * out[0] + (b[1] - a[1]) * out[1]
        k = (a[0] + out[0] * (alongd - abs(latd)), a[1] + out[1] * (alongd - abs(latd)))
        pts = [p, a, k, b, e]
        clean = [pts[0]]
        for q in pts[1:]:
            if math.hypot(q[0] - clean[-1][0], q[1] - clean[-1][1]) > 1e-4:
                clean.append(q)
        stubs[name] = clean
    return esc, stubs['P'], stubs['N'], sP


ESD_NECK = {'U101': 0.5, 'U201': 0.5, 'U301': 0.5}   # extra straight stub below the input ESDs' 4/6 pair, mm: the
#                                                      same 0.5 mm the output ports' ESDs get from the via-room rule
#                                                      (so all six ESDs run 1.26 mm straight before the pair turns in)
PAIR_END_RUN = 4         # straight centre-line cells where a pair meets its fan-out stubs (offset lines meet them square)
PAIR_VIA_RUN = 15        # straight cells before and after a lane via pair, so both vias sit square to the pair
# longer straight runs at some pair ends (P net, 'A' / 'B' end): Hub-0's Other pair leaves the hub 0.5 mm straight, so
# its run east under the hub's port-4 pins leaves room for both of their lines.  H0_MS is the other way round: its A end
# is the mouse switch's west pins, whose pair has to turn north 1.35 mm out, and with the default 4 cells the west run
# was so short that offset_polyline()'s mitre at the 45-degree entry collapsed both lines onto one point - the row then
# put the P via 0.15 mm from its own partner ("lines only -0.150 mm apart at (144.25,153.97)"); 0.5 mm of straight run
# keeps both lines apart until the turn, and H0_MS routes (P 48.91 / N 48.88 mm, skew 0.02 mm, 6 vias).
PAIR_END_RUNS = {('H0_OT_DP', 'B'): 10, ('H0_MS_DP', 'A'): 10,
                 # Rev 8, third configuration (switch at (140,114) rot 270): both channel pairs leave its west side and
                 # turn immediately to wrap round the chip, and SVC's own A end is the switch - with the default 4 cells
                 # the turn sits on the escape and the offset lines fold (measured "lines only 0.141 mm apart at
                 # (139.17,115.49)", and the same for H0_UP at (138.59,113.09)).  0.5 mm of straight run gives the turn
                 # its room.
                 # Both keys of SVC_DP are needed because the two SVC links share the net name, and they meet different
                 # ends of the ESD: the switch's link arrives on pads 4/6 (south) and needs the run there, while the
                 # connector's link arrives on pads 3/1 (north) and must *not* have it.  With 0.5 mm of straight run
                 # into its crossover zone the pair cannot also start 0.5 mm east of the connector's centre line, so it
                 # ran past the zone and came back - the roundabout the two PC links never had (rev 9, measured: 38
                 # cells, P 1.78 mm / N 1.19 mm, skew 0.59 mm against J101 -> U101's 20 cells, 0.93 / 0.93, skew 0.00).
                 # Its A end, the connector, takes the default run as well since 0.15.0: with 10 cells it failed in every
                 # run from 0.12.0 on, "lines only 0.050 mm apart at (160.25,109.36)" (hidden until 0.15.0 by a verdict
                 # keyed by net, see pair_failed); with 4 it routes exactly as the two PC links do, 20 cells, P 0.93 /
                 # N 0.93 mm, skew 0.00 mm (2026-11-07).  The rev-9 measurement that had asked for 10 ("lines only 0.142
                 # mm apart") predates the ESDs' move north.
                 ('SVC_DP', 'A'): 10, ('SVC_DP', 'B', 'U301.4'): 10, ('H0_UP_DP', 'A'): 10,
                 ('SVC_DP', 'A', 'J301'): PAIR_END_RUN}


def straight_cells(R, cell, d, k):
    """The k cells on from cell (L, j, i) along direction d, if the fat line is free on all of them (else None)."""
    L, j, i = cell
    di, dj = (d[0] > 0) - (d[0] < 0), (d[1] > 0) - (d[1] < 0)
    out = []
    for n in range(1, k + 1):
        jj, ii = j + n * dj, i + n * di
        if not (R.j0 <= jj < R.j1 and R.i0 <= ii < R.i1) or not R.free[L][jj - R.j0, ii - R.i0]:
            return None
        out.append((L, jj, ii))
    return out


def unloop(pts, close=None):
    """Drop a loop from a centre line: a stretch that leaves a point and comes back onto it.

    The leg search keeps the travel *direction* in its state, and at a waypoint that is close to the start cell it can
    answer with a tight loop - arrive from the east travelling west, then turn a full circle so that it reaches the
    waypoint heading south-west.  Measured (H0_MS's first waypoint, 0.5 mm from U1302's pad row): eleven centre
    points over 0.15 mm, (144.38,153.75) (144.33,153.80) (144.32,153.85) (144.35,153.88) (144.40,153.88)
    (144.43,153.85) (144.43,153.78).  The offset lines of such a centre fold onto each other, and the pair was then
    rejected with "lines only -0.150 mm apart at (142.83,152.32)".

    A loop is not copper: two points of one layer's centre line closer than the pair's own width mean the fat centre
    line overlaps itself, which the leg search's own clearance test forbids cell by cell - so it can only be the
    artefact above, and cutting it out leaves the cells that remain (all still free) and the 45-degree turn the
    waypoint asked for.  smooth_turns() then straightens the short joining segment this can leave behind.
    """
    close = close or PAIR_S
    out = list(pts)
    for _ in range(len(out)):                   # one loop at a time, until none is left
        cut = None
        for a in range(len(out) - 3):
            for b in range(a + 3, len(out)):    # >= 2 vertices in between: a loop, not a corner
                if math.hypot(out[a][0] - out[b][0], out[a][1] - out[b][1]) < close:
                    cut = (a, b)
                    break
            if cut:
                break
        if not cut:
            break
        a, b = cut
        out = out[:a + 1] + out[b:]
    return out


def smooth_turns(pts, min_len=0.15):
    """A short centre-line segment between two 45-degree turns the same way would fold the inner offset line back on
    itself (each turn cuts PAIR_O * tan 22.5 deg off it): its two corners move apart along their neighbouring
    segments until it is min_len long (it keeps its direction)."""
    pts = list(pts)
    for k in range(1, len(pts) - 2):
        a, b, c, d = pts[k - 1], pts[k], pts[k + 1], pts[k + 2]
        v1, v2, v3 = (b[0] - a[0], b[1] - a[1]), (c[0] - b[0], c[1] - b[1]), (d[0] - c[0], d[1] - c[1])
        s = math.hypot(*v2)
        if s >= min_len or s < 1e-9 or (v1[0] * v2[1] - v1[1] * v2[0]) * (v2[0] * v3[1] - v2[1] * v3[0]) <= 0:
            continue                              # long enough, or an S-jog (one line gains what the other loses)
        u1, u3 = unit(v1), unit(v3)
        g = math.hypot(u1[0] + u3[0], u1[1] + u3[1])
        if g < 1e-9:
            continue
        t = min((min_len - s) / g, 0.5 * math.hypot(*v1), 0.5 * math.hypot(*v3))
        pts[k] = (b[0] - u1[0] * t, b[1] - u1[1] * t)
        pts[k + 1] = (c[0] + u3[0] * t, c[1] + u3[1] * t)
    return pts


def seg_seg_dist(a, b, c, d):
    """Distance between segments ab and cd."""
    def pt_seg(p, q, r):
        dx, dy = r[0] - q[0], r[1] - q[1]
        L2 = dx * dx + dy * dy
        t = 0.0 if L2 == 0 else max(0.0, min(1.0, ((p[0] - q[0]) * dx + (p[1] - q[1]) * dy) / L2))
        return math.hypot(p[0] - q[0] - t * dx, p[1] - q[1] - t * dy)
    def cross(o, p, q):
        return (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0])
    if cross(a, b, c) * cross(a, b, d) < 0 and cross(c, d, a) * cross(c, d, b) < 0:
        return 0.0
    return min(pt_seg(a, c, d), pt_seg(b, c, d), pt_seg(c, a, b), pt_seg(d, a, b))


def items_clash(a, b, clr=CLR):
    """Two tracks / vias closer than clr (copper to copper); tracks on different layers never clash."""
    (ka, da), (kb, db) = a, b
    if ka != 'via' and kb != 'via' and da[5] != db[5]:
        return False
    ra = VIA_D / 2 if ka == 'via' else da[4] / 2
    rb = VIA_D / 2 if kb == 'via' else db[4] / 2
    pa = ((da[0], da[1]), (da[0], da[1])) if ka == 'via' else ((da[0], da[1]), (da[2], da[3]))
    pb = ((db[0], db[1]), (db[0], db[1])) if kb == 'via' else ((db[0], db[1]), (db[2], db[3]))
    return seg_seg_dist(pa[0], pa[1], pb[0], pb[1]) < ra + rb + clr - EPS


def seg_free(B, d, name, step=0.05, skip=0.3):
    """A track (x0, y0, x1, y1, w, layer) of net ``name`` clears all foreign copper and reservations by CLR (pads included,
    the grids hold them), sampled along its centre line; its first ``skip`` mm (the pin it leaves) excepted."""
    x0, y0, x1, y1, w, L = d
    n = max(1, int(math.hypot(x1 - x0, y1 - y0) / step))
    net = B.code(name)
    thr = CLR + w / 2 - EPS
    for k in range(n + 1):
        t = k / n
        x, y = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
        if math.hypot(x - x0, y - y0) < skip:
            continue
        i, j = gi(x, X0), gi(y, Y0)
        if not (0 <= i < NX and 0 <= j < NY):
            return False
        for fld in (B.static, B.dyn):
            fd = fld.d1[int(L), j, i] if fld.n1[int(L), j, i] != net else fld.d2[int(L), j, i]
            if fd < thr:
                return False
    return True


def relane(B, name, pin, via, avoid):
    """A pin's planned way out to ``via`` re-laid round ``avoid`` (items): straight out of the pin, a 45-degree jog
    over to the via's line, on to the via - the jog as close to the pin as clears, then a straight lane to a via at the
    same depth.  Returns the items, or None."""
    (px, py), (vx, vy) = pin, via
    radial_y = abs(vy - py) >= abs(vx - px)
    out = (vy - py) if radial_y else (vx - px)
    side = (vx - px) if radial_y else (vy - py)
    sgn, depth, jog = (1 if out > 0 else -1), abs(out), abs(side)

    def at(r, t):                         # r out of the pin, t along the side
        return (px + t, py + sgn * r) if radial_y else (px + sgn * r, py + t)
    cands = []
    for k in range(int((depth - jog - 0.3) / G) + 1):
        r = 0.3 + k * G
        p1, p2 = at(r, 0.0), at(r + jog, side)
        cands.append(([(px, py, *p1), (*p1, *p2), (*p2, vx, vy)], (vx, vy)))
    end = at(depth, 0.0)
    cands.append(([(px, py, *end)], end))
    for segs, v in cands:
        items = [('seg', (*sg, W_SIG, 0)) for sg in segs if math.hypot(sg[2] - sg[0], sg[3] - sg[1]) > EPS]
        items.append(('via', v))
        if any(items_clash(it, x) for it in items for x in avoid):
            continue
        if not legal_via(B, v[0], v[1], name):
            continue
        if all(seg_free(B, it[1], name, skip=0.3 if k == 0 else 0.0) for k, it in enumerate(items[:-1])):
            return items
    return None


def reserve_xtal_lanes(B, log):
    """XTAL_LANES: lay a crystal line's way out as fixed copper before the dense fan-outs are planned (see XTAL_LANES).
    A planned way out of another pin that it touches (none in a full run: the plans come after) is re-laid or dropped."""
    for name, items in XTAL_LANES.items():
        if name not in B.netcode or len(terminals(B, name)) <= 1:
            continue
        hit = sorted({n for n, k, d in B.tracks if n != name and any(items_clash(it, (k, d)) for it in items)})
        if hit:
            log.append(f'{name}: crystal lane not reserved, copper of {hit} in the way')
            print(log[-1], flush=True)
            continue
        # Another net's planned way out that touches the lane goes, pin by pin (a pin's items are its lane segments up
        # to and including its via site).  Such a pin then gets its lane re-laid to the same via site with the jog
        # further out (relane), if one clears the crystal's lane and everything else; otherwise it routes without one.
        # (0.16.0: MS_FLT, pin 28, planned with a 0.4 mm jog that ran past the crystal's first via; without any lane
        # MS_EN jogged into its column and MS_FLT was left with no way out, "no way through at all".)
        cut = []
        for n in list(B.escape):
            if n == name:
                continue
            chains, cur = [], []
            for it in B.escape[n]:
                cur.append(it)
                if it[0] == 'via':
                    chains.append(cur)
                    cur = []
            if cur:
                chains.append(cur)
            bad = [c for c in chains if any(items_clash(it, x) for it in c for x in items)]
            if bad:
                B.escape[n] = [it for c in chains if c not in bad for it in c]
                cut += [(n, c) for c in bad]
        for kind, data in items:                       # laid as fixed copper: the fan-out plans and every net see it
            B.add_track(name, kind, data, fixed=True)
        B.rebuild()
        straight, dropped = [], []
        for n, c in cut:
            segs, vias = [it for it in c if it[0] == 'seg'], [it for it in c if it[0] == 'via']
            alt = None
            if segs and vias:
                alt = relane(B, n, (segs[0][1][0], segs[0][1][1]), vias[-1][1], items)
            if alt:
                B.escape[n] = B.escape[n] + alt
                for kind, data in alt:
                    B._add_dyn(n, kind, data)
                straight.append(n)
            else:
                dropped.append(n)
        B.rebuild()
        log.append(f'{name}: crystal lane laid; planned ways out it crossed: re-laid {straight or "none"}, '
                   f'dropped {dropped or "none"}')
        print(log[-1], flush=True)


def pair_min_gap(items, P, N):
    """Smallest copper gap between the two lines of a pair (tracks and vias, per layer), and where it is.  The location
    matters: a pair that fails this check says only "lines only X mm apart", and on rev 8 that left it to be guessed
    whether the pinch was in the fan-out stubs, in a twist zone or in the routed stretch.  The third value names the
    two shapes that are that close, so it does not have to be reconstructed by hand either (finding H1_MS's 0.129 mm
    at (109.39,145.96) meant re-deriving the whole staggered via pair from the source)."""
    def shapes(nm):
        out = []
        for n, k, d in items:
            if n != nm:
                continue
            if k == 'seg':
                out.append((((d[0], d[1]), (d[2], d[3]), d[4] / 2, {int(d[5])}),
                            f'track ({d[0]:.2f},{d[1]:.2f})-({d[2]:.2f},{d[3]:.2f}) w{d[4]:.2f} L{int(d[5])}'))
            else:
                out.append((((d[0], d[1]), (d[0], d[1]), VIA_D / 2, set(range(NL))), f'via ({d[0]:.2f},{d[1]:.2f})'))
        return out
    best, where, detail = math.inf, None, ''
    for (a, b, ra, la), da in shapes(P):
        for (c, d, rc, lc), dc in shapes(N):
            if la & lc:
                g = seg_seg_dist(a, b, c, d) - ra - rc
                if g < best:
                    best, detail = g, f'P {da} / N {dc}'
                    where = ((a[0] + b[0] + c[0] + d[0]) / 4, (a[1] + b[1] + c[1] + d[1]) / 4,
                             'F.Cu' if 0 in la else ('B.Cu' if 1 in la else 'via'))
    return best, where, detail


def offset_polyline(pts, o):
    """Offset a polyline to its left by o (miter joins). With shapely the offset is loop-free: where turns lie
    closer than the offset, the inner line's folded-back piece is cut off instead of crossing its partner."""
    if OffsetLine is not None and len(pts) >= 2:
        g = OffsetLine(pts).offset_curve(-o, join_style='mitre', mitre_limit=5.0)
        if g.geom_type == 'LineString' and len(g.coords) >= 2:
            return [tuple(c) for c in g.coords]
    out = []
    n = len(pts)
    for k in range(n):
        if k == 0:
            d = unit((pts[1][0] - pts[0][0], pts[1][1] - pts[0][1]))
            nl = left_of(d)
            out.append((pts[0][0] + nl[0] * o, pts[0][1] + nl[1] * o))
        elif k == n - 1:
            d = unit((pts[k][0] - pts[k - 1][0], pts[k][1] - pts[k - 1][1]))
            nl = left_of(d)
            out.append((pts[k][0] + nl[0] * o, pts[k][1] + nl[1] * o))
        else:
            d1 = unit((pts[k][0] - pts[k - 1][0], pts[k][1] - pts[k - 1][1]))
            d2 = unit((pts[k + 1][0] - pts[k][0], pts[k + 1][1] - pts[k][1]))
            n1, n2 = left_of(d1), left_of(d2)
            dot = n1[0] * n2[0] + n1[1] * n2[1]
            f = o / (1 + dot)
            out.append((pts[k][0] + (n1[0] + n2[0]) * f, pts[k][1] + (n1[1] + n2[1]) * f))
    return out


def trim_pair_tails(lp, ln, need=None, most=3):
    """Drop the vertices at which the two offset lines have collapsed onto each other.

    offset_polyline() is called with clamp=False and a mitre limit of 5, and at a sharp tail it *cuts* the folded
    piece - the two lines then end at the same coordinate (measured, H0_MS: both offsets ended at (144.58,153.90)
    after its west run turned 45 degrees into the layer change, so the pair's own two lines overlapped by 0.15 mm
    and the via row then landed on the partner line).  Keeping the last vertices at which the two are still a pair
    (PAIR_O either side, i.e. PAIR_S + PAIR_W apart) removes the collapsed piece from the copper as well: the layer
    change's own connectors reach the vias from there, so the pair stays continuous.

    The two lines are cut *together*, one vertex each, and never by index into a common length: shapely bevels the
    mitre at a sharp turn and the two offset curves then have different vertex counts, so slicing both lists to
    `min(len(lp), len(ln))` cut the longer line's tail off entirely.  Measured on this run's pairs-only board:
    H1_OT's N line stopped 4.00 mm short of its fan-out (2 copper pieces, the pair visibly came apart) and H0_KB's
    P line lost its whole last run, which the layer change then bridged with a 26 mm straight connector
    (P 72.76 / N 75.27 mm, skew 2.51 mm).  A collapse is a *local* defect at one end, so `most` caps the trim as
    well: a real run is never shortened by more than a vertex or two.
    """
    need = need or PAIR_W            # centres this close is overlapping copper; the pair's own pitch is 0.30 mm
    lp, ln = list(lp), list(ln)
    for end in (1, 0):                # the tail, then the head
        for _ in range(most):
            if len(lp) < 3 or len(ln) < 3:
                break
            a, b = (lp[-1], ln[-1]) if end else (lp[0], ln[0])
            if math.hypot(a[0] - b[0], a[1] - b[1]) >= need:
                break
            if end:
                lp.pop()
                ln.pop()
            else:
                lp.pop(0)
                ln.pop(0)
    return lp, ln


def centre_runs(path):
    """Split a centre-line cell path into per-layer runs of corner points."""
    runs = []
    cur = None
    for (L, j, i) in path:
        p = (X0 + i * G, Y0 + j * G)
        if cur is None or cur[0] != L:
            if cur is not None:
                runs.append(cur)
            cur = [L, [p]]
        else:
            cur[1].append(p)
    runs.append(cur)
    out = []
    for L, pts in runs:
        simp = [pts[0]]
        for k in range(1, len(pts) - 1):
            a, b, c = simp[-1], pts[k], pts[k + 1]
            d1 = (round((b[0] - a[0]) / G), round((b[1] - a[1]) / G))
            d2 = (round((c[0] - b[0]) / G), round((c[1] - b[1]) / G))
            if d1[0] * d2[1] - d1[1] * d2[0] != 0 or d1[0] * d2[0] + d1[1] * d2[1] < 0:
                simp.append(b)
        if len(pts) > 1:
            simp.append(pts[-1])
        out.append((L, simp))
    return out


def direct_pair(B, P, N, termA, termB, log):
    """A short pair with nothing between its two ends: drawn, not searched ('direct' in pair_links).  Each end's fan-out
    converges the two lines from the pad pitch to the pair pitch, and between the two escapes they run straight and
    parallel, PAIR_O either side of the centre line, so the 0.30 mm pitch (0.15 gap) holds over the whole link.
    This exists for links whose escapes are too close for the pair search to *start* in: Hub-0's port-4 pair has 2.75 mm
    between its escapes, and the search's 0.45 mm-wide centre line cannot begin inside its own 0.8 mm-deep fan-out
    (measured: the router's field for that cell reads 0.075 mm, and every Hub-0 order failed with "no pair path").
    Pad-to-pad lines would not do: they leave the pair pitch at once, and on this link the DM line then ran down the
    middle of Hub-0's Other pair's corridor and took its via site away ("no via-pair site near waypoint 4 (162.75,
    127.6)").  The centre of the drawn link is the two escapes' midpoint, i.e. the corridor the fan-outs were planned for.
    """
    escA, stA_P, stA_N, sA = fanout(termA)
    escB, stB_P, stB_N, sB = fanout(termB)
    stubs = []
    for nm, pts in ((P, stA_P), (N, stA_N), (P, stB_P), (N, stB_N)):
        for a, b in zip(pts, pts[1:]):
            stubs.append((nm, (a[0], a[1], b[0], b[1], PAIR_W, 0)))
    if sA == sB:                                  # P would have to swap sides between the ends: that needs a swap zone,
        log.append(f'  PAIR FAIL {P}/{N} direct link needs a P/N swap (both ends keep P on the same side)')  # i.e. a
        return False                              # searched link, not a drawn one
    # the straight lines start and end on the fan-outs' own last points (not on the escape +/- PAIR_O, which the stubs
    # snap to the grid: the 0.05 mm they can be off is a third of the pair gap, and it read as "lines only 0.150 mm
    # apart" at (163.79, 126.40)).
    pA, pB = stA_P[-1], stB_P[-1]
    nA, nB = stA_N[-1], stB_N[-1]
    items = [(nm, 'seg', dd) for nm, dd in stubs] + \
            [(P, 'seg', (*pA, *pB, PAIR_W, 0)), (N, 'seg', (*nA, *nB, PAIR_W, 0))]
    gap, where, detail = pair_min_gap(items, P, N)
    if gap < PAIR_S - 0.005:          # 5 um: the drawn link's gap *is* 0.30 - 0.15 by construction, and the stubs' grid
        #                             snap moves it by less than a tenth of the 0.05 mm grid (the strict 1 um tolerance
        #                             rejected a link whose measured gap printed as exactly 0.150)
        log.append(f'  PAIR FAIL {P}/{N} direct link lines only {gap:.4f} mm apart (want {PAIR_S:.3f} mm) at '
                   f'({where[0]:.2f},{where[1]:.2f}) | {detail}')
        return False
    for nm, k, dd in items:
        B.add_track(nm, k, dd, fixed=True)
    B.rebuild()
    lp = math.hypot(pB[0] - pA[0], pB[1] - pA[1])
    ln = math.hypot(nB[0] - nA[0], nB[1] - nA[1])
    log.append(f'{P}/{N}: direct, P {lp:.2f} mm N {ln:.2f} mm (skew {abs(lp - ln):.2f} mm), P-N gap {gap:.3f} mm, '
               f'0 vias')
    return True


def swap_zone(P, N, start, d, sideP, gap, Lt=1.6, out=0.65, p_out=None, layer=0):
    """P/N swap: after a straight lead-in of length gap, P crosses over on F.Cu while N hops under it on
    B.Cu (two vias outside the pair).  P enters on side sideP (left normal of d) and leaves on -sideP.  p_out: the
    height of P's S, if not N's hop (``out``) - a taller or flatter S lengthens or shortens P alone (twist_sites trims
    skew with it).  layer: the one P crosses on (and both lines arrive on); N hops on the other."""
    nl = left_of(d)
    o = PAIR_O

    def at(base, along, side):
        return (base[0] + d[0] * along + nl[0] * side, base[1] + d[1] * along + nl[1] * side)

    def snap(q):
        return (round(q[0] / G) * G, round(q[1] / G) * G)
    s = sideP
    items = []
    if gap > 0:
        items += [(P, 'seg', (*at(start, 0, s * o), *at(start, gap, s * o), PAIR_W, layer)),
                  (N, 'seg', (*at(start, 0, -s * o), *at(start, gap, -s * o), PAIR_W, layer))]
    v1 = snap(at(start, gap + 0.3, -s * out))
    v2 = snap(at(start, gap + Lt - 0.3, s * out))
    # P takes the *same* lateral excursion as N's hop (out to 0.65 and back across), so the two lines leave the twist
    # with the same length.  N's two vias force it to detour sideways and a straight crossing gets none of that back -
    # that is the 0.6 mm (4 ps) a twist used to cost, and most of what H1_AT's 1.17 mm (7.9 ps) was, on a link too short
    # to dilute it.  A symmetric S on F.Cu costs no copper and makes the pair equal again.
    A = out if p_out is None else p_out
    hop = 1 - layer
    items += [(P, 'seg', (*at(start, gap, s * o), *at(start, gap + 0.3, s * A), PAIR_W, layer)),
              (P, 'seg', (*at(start, gap + 0.3, s * A), *at(start, gap + Lt - 0.3, -s * A), PAIR_W, layer)),
              (P, 'seg', (*at(start, gap + Lt - 0.3, -s * A), *at(start, gap + Lt, -s * o), PAIR_W, layer)),
              (N, 'seg', (*at(start, gap, -s * o), *v1, PAIR_W, layer)), (N, 'via', v1),
              (N, 'seg', (*v1, *v2, PAIR_W, hop)), (N, 'via', v2),
              (N, 'seg', (*v2, *at(start, gap + Lt, s * o), PAIR_W, layer))]
    return items, snap(at(start, gap + Lt, 0))


def block_report(B, name, pts):
    """Why a pair search had nowhere to start or stop: for each (x, y), the foreign nets owning the 3x3 cells around it.
    A pair that fails with "no pair path" says nothing about *what* is in the way, and twice on rev 8 that left the
    corridor's owner to be guessed at (the H0_D4 stub pair and the SVC pair both failed this way)."""
    B.foreign(name)                                     # build the foreign map first
    codes = {c: n for n, c in B.netcode.items()}
    out = []
    for x, y in pts:
        j, i = gi(y, Y0), gi(x, X0)
        owners = set()
        for dj in (-1, 0, 1):
            for di in (-1, 0, 1):
                for L in range(NL):
                    c = int(B.dyn.n1[L, j + dj, i + di])
                    if c:
                        owners.add(codes.get(c, f'#{c}'))
        out.append(f'({x:.2f},{y:.2f}) blocked by {sorted(owners) if owners else "nothing"}')
    return '; '.join(out)


def items_conflict(B, name, items):
    """Debugging: the first item that fails the strict check, where, and the nearest foreign copper there."""
    fd = B.foreign(name)
    codes = {c: n for n, c in B.netcode.items()}
    for idx, (nm, k, d) in enumerate(items):
        if k == 'via':
            if not legal_via(B, d[0], d[1], name):
                j, i = gi(d[1], Y0), gi(d[0], X0)
                return idx, 'via', d, [codes.get(int(B.dyn.n1[L, j, i]), '?') for L in range(NL)]
            continue
        x1, y1, x2, y2, w, L = d
        n = max(int(math.hypot(x2 - x1, y2 - y1) / G), 1)
        for t in range(n + 1):
            x, y = x1 + (x2 - x1) * t / n, y1 + (y2 - y1) * t / n
            i, j = gi(x, X0), gi(y, Y0)
            if fd[int(L), j, i] < CLR + w / 2 + EPS or B.keep_trk[int(L), j, i]:
                st = B.static.n1[int(L), j, i] if B.static.d1[int(L), j, i] < B.dyn.d1[int(L), j, i] else B.dyn.n1[int(L), j, i]
                return idx, 'seg', (round(x, 2), round(y, 2)), codes.get(int(st), '?'), round(float(fd[int(L), j, i]), 3)
    return None


def _hop_candidates(B, name, items, max_hop):
    """The pointless layer changes of one net: a Π the search left behind - down a short stub, across on the other
    layer, up a short stub - with nothing crossed under the crossbar.  Yields (vias, items to delete, bridge, saving),
    the bridge being the straight run on the layer the two joined runs are on.  Conservative: a plain through-junction
    at each via, both joined runs continuing collinearly past the junction, the bridge passing the router's own
    free-space test and (for a USB line) the pair spacing to the mate surviving."""
    near = lambda p, q: abs(p[0] - q[0]) < 1e-6 and abs(p[1] - q[1]) < 1e-6
    segs = [d for kind, d in ((t[1], t[2]) for t in items) if kind == 'seg']
    vias = [d for kind, d in ((t[1], t[2]) for t in items) if kind == 'via']
    out = []
    for a in range(len(vias)):
        for b in range(a + 1, len(vias)):
            va, vb = vias[a], vias[b]
            if math.hypot(va[0] - vb[0], va[1] - vb[1]) > max_hop:
                continue
            at = {va: [], vb: []}
            for d in segs:
                for v in (va, vb):
                    if near((d[0], d[1]), v):
                        at[v].append((d, (d[2], d[3])))
                    elif near((d[2], d[3]), v):
                        at[v].append((d, (d[0], d[1])))
            if len(at[va]) != 2 or len(at[vb]) != 2:
                continue
            for da, pa in at[va]:
                for db, pb in at[vb]:
                    if da[5] != db[5]:
                        continue
                    L, w = da[5], da[4]
                    hop = [x for x, _ in at[va] + at[vb] if x[5] != L]
                    if len(hop) != 2 or any(x[5] == L for x in hop):
                        continue
                    bridge = (pa[0], pa[1], pb[0], pb[1], w, L)
                    bl = math.hypot(pb[0] - pa[0], pb[1] - pa[1])
                    if bl < 1e-6:
                        continue
                    gone = [da, db] + hop
                    for d in segs:                     # the Π's crossbar sits exactly where the bridge goes
                        if d in gone or d[5] != L:
                            continue
                        u = ((d[0] - pa[0]) * (pb[0] - pa[0]) + (d[1] - pa[1]) * (pb[1] - pa[1])) / bl ** 2
                        v = ((d[2] - pa[0]) * (pb[0] - pa[0]) + (d[3] - pa[1]) * (pb[1] - pa[1])) / bl ** 2
                        if -1e-6 <= u <= 1 + 1e-6 and -1e-6 <= v <= 1 + 1e-6:
                            gone.append(d)
                    ok = True
                    for p in (pa, pb):                 # both joined runs must continue collinearly past the junction
                        cont = [d for d in segs if d not in gone and
                                (near((d[0], d[1]), p) or near((d[2], d[3]), p))]
                        if len(cont) != 1:
                            ok = False
                            break
                        d = cont[0]
                        ex, ey = d[2] - d[0], d[3] - d[1]
                        el = math.hypot(ex, ey) or 1.0
                        if abs(ex * (pb[0] - pa[0]) + ey * (pb[1] - pa[1])) / (el * bl) < math.cos(math.radians(25)):
                            ok = False
                            break
                    if not ok:
                        continue
                    saved = sum(math.hypot(d[2] - d[0], d[3] - d[1]) for d in gone) - bl
                    if saved < 0.2 or saved > 1.5:
                        continue
                    if not items_legal(B, name, [(name, 'seg', bridge)], strict=True):
                        continue
                    mate = B.pair_of.get(name)
                    if mate:                           # both lines share one net code: check the mate here
                        other = mate[0] if mate[1] == name else mate[1]
                        gap = pair_min_gap([(name, 'seg', bridge)] +
                                           [(other, k, d) for nm, k, d in B.tracks if nm == other], name, other)[0]
                        if gap < PAIR_S - 1e-6:
                            continue
                    out.append(((va, vb), [('seg', d) for d in gone] + [('via', va), ('via', vb)], bridge, saved))
    return out


def collapse_hops(B, log, max_hop=1.35):
    """Remove the layer changes that go nowhere (see _hop_candidates).

    Measured on the rev-9 board (tools/route.log, full24): H2_MS_DP and H2_MS_DM each carried two of these 0.75 mm hops
    (6 vias per line, where the rev-8 board had 2), plus KB_ILIM, H1_PLLFILT, H2_PLLFILT and HUB1_RST one each - 7 hop
    pairs, 16 vias and about 4 mm of round trip that served no purpose.  A pair's two lines are collapsed together and
    only with the same length saving, so the pair's skew cannot move."""
    if getattr(B, 'hops_done', False):
        return 0
    B.hops_done = True
    names = sorted({t[0] for t in B.tracks})
    seen, todo, n = set(), [], 0
    for name in names:
        if name in seen:
            continue
        mate = B.pair_of.get(name)
        group = [name] if not mate else sorted(set(mate))
        seen |= set(group)
        cands = {g: _hop_candidates(B, g, [t for t in B.tracks if t[0] == g and t not in B.fixed], max_hop)
                 for g in group}
        if len(group) == 2:                        # a pair: the same saving on both lines, or neither
            for c in cands[group[0]]:
                m = min(cands[group[1]], key=lambda o: abs(o[3] - c[3]), default=None) if cands[group[1]] else None
                if m and abs(m[3] - c[3]) <= 0.05:
                    todo += [(group[0], c), (group[1], m)]
        else:
            todo += [(name, c) for c in cands[name]]
    for name, cand in todo:
        gone, bridge = cand[1], cand[2]
        gone_set = {(name,) + tuple(g) for g in gone}
        B.tracks = [t for t in B.tracks if t not in gone_set]
        B.changes += [item_box(g[0], g[1]) for g in gone]
        B.rebuild()
        B.add_track(name, 'seg', bridge)
        n += 1
    log.append(f'layer hops collapsed: {n} (a net that dropped a layer and came straight back for nothing)')
    return n


def items_legal(B, name, items, strict=False, fd=None):
    """Clearance check of fixed copper (segments sampled every grid step, vias as circles). strict: the router's own
    free-space threshold (copper it places must pass the same test the search uses), else one grid step of slack.
    fd: B.foreign(name), when the caller checks many candidates against an unchanged board (it is a whole-board array:
    seed_twist's blocker search spent 45 s recomputing it, 2026-11-06)."""
    fd = B.foreign(name) if fd is None else fd
    slack = -EPS if strict else G
    guarded = xtal_guarded(name)
    for nm, k, d in items:
        if k == 'via':
            if not legal_via(B, d[0], d[1], name):
                return False
            continue
        x1, y1, x2, y2, w, L = d
        n = max(int(math.hypot(x2 - x1, y2 - y1) / G), 1)
        for t in range(n + 1):
            x, y = x1 + (x2 - x1) * t / n, y1 + (y2 - y1) * t / n
            i, j = gi(x, X0), gi(y, Y0)
            if not (0 <= i < NX and 0 <= j < NY) or fd[int(L), j, i] < CLR + w / 2 - slack or B.keep_trk[int(L), j, i]:
                return False
            if guarded and B.xtal_d[int(L), j, i] < XTAL_GUARD + w / 2 - slack:
                return False
    return True


def run_dir(pts, minlen=0.30):
    """Direction of a run for its via row: the chord over at least ``minlen`` mm, not the first cell's step.

    The first centre cell of a run can be a stub (the pair comes off a pad or out of smooth_turns with a one-cell
    jog), and taking the row's axis from it rotated the whole row: measured, H0_MS's run at y 153.90 had its vias
    put on a *horizontal* row by a one-cell vertical step, so the P via landed 0.15 mm from the N line's centre
    (needs 0.525) - "lines only -0.225 mm apart (want 0.150) at (144.80,153.82)".  A chord of 0.30 mm (the pair
    pitch) is long enough to ignore a stub and short enough to follow a real corner.
    """
    if len(pts) < 2:
        return None
    for q in pts[1:]:
        v = (q[0] - pts[0][0], q[1] - pts[0][1])
        if math.hypot(*v) >= minlen:
            return unit(v)
    v = (pts[1][0] - pts[0][0], pts[1][1] - pts[0][1])
    return unit(v) if math.hypot(*v) > 1e-9 else None


def recut_rows(runs, maxlen=0.9):
    """Slide each layer boundary off a corner onto the straight stretch that follows it.

    A layer change can land exactly where the pair turns - the leg search takes its via pair at a cell of its own
    path, and a lane waypoint that sits on a corner puts it there too.  A via row on a corner is square to neither
    of the two directions: its vias then sit on the pair's *own* lines, because the pair's width runs along the row
    (measured, H0_MS: "P via (144.85,153.75) / N track (144.90,153.90)-(144.61,153.90)" - 0.225 mm of overlap,
    while the row itself was square to the north leg).  Moving the boundary forward along the path until both sides
    are straight puts the row where it belongs: a few cells past the corner, on the new layer's run, so the old
    layer's copper stops at the corner and nothing runs past the vias.  Only the layer of the cells moves, not the
    copper: those cells were free on both layers (the two-layer search that produced them requires it).
    """
    out = [[L, list(pts)] for L, pts in runs]
    if VIA_DEBUG and len(out) > 1:
        print('    recut: runs ' + ' '.join(f'L{L}:{len(p)}' for L, p in out), flush=True)
    for i in range(1, len(out)):
        moved = 0.0
        while True:
            a, b = out[i - 1][1], out[i][1]
            if len(a) < 2 or len(b) < 2:
                break
            din = run_dir(list(reversed(a)), 0.30)          # the chord *back* from the boundary
            dout = run_dir(b, 0.30)
            if din is None or dout is None or abs(din[0] * dout[1] - din[1] * dout[0]) < 1e-3:
                break                       # already straight through the boundary
            step = math.hypot(b[1][0] - b[0][0], b[1][1] - b[0][1])
            if moved + step > maxlen:
                break
            if VIA_DEBUG:
                print(f'    recut row {i}: -{step:.2f} mm off the corner (din={tuple(round(q, 2) for q in din)} '
                      f'dout={tuple(round(q, 2) for q in dout)})', flush=True)
            moved += step
            a.append(b.pop(0))
    return [(L, pts) for L, pts in out]


def drop_layer_blips(runs, minlen=0.3):
    """Drop a short run that sits between two runs on the *same* other layer: that is not a layer change at all.

    lane_search's two-layer fallback may take a leg down early and then come back up to its own target cell, which
    belongs to the leg's *incoming* layer - so the pair changes layer three times within a cell or two.  centre_runs()
    splits on every layer change, hence two same-layer runs separated by a one-cell run of the other layer, and each
    run gets its own offset lines: the two lines of the runs either side then meet at a corner that was never on the
    path, and they cross.  Measured on H0_MS (run {H1_KB, H1_OT, H0_MS}, 2026-10-26): "lines only 0.106 mm apart
    (want 0.150 mm) at (140.26,144.33) on B.Cu", P line ending at (139.61,143.61) and N line starting at
    (139.32,143.69) - the two halves of one planned hairpin waypoint (139.5, 143.5), which the leg had already passed
    on the other layer.

    The blip's own cells are free on *both* layers (they come from the two-layer search), so continuing on the
    surrounding layer is legal, and it is what the lane asked for: one via row, at the waypoint.  Only runs shorter
    than minlen are merged - a real layer hop has to be wider than the obstacle it crosses, 0.5 mm at the least,
    while this one is a single repeated cell (length 0).
    """
    out = []
    k = 0
    while k < len(runs):
        L, pts = runs[k]
        ln = sum(math.hypot(b[0] - a[0], b[1] - a[1]) for a, b in zip(pts, pts[1:]))
        if out and k + 1 < len(runs) and ln < minlen and out[-1][0] == runs[k + 1][0] != L:
            merged = out[-1][1] + list(pts) + list(runs[k + 1][1])
            ded = [merged[0]]
            for p in merged[1:]:
                if math.hypot(p[0] - ded[-1][0], p[1] - ded[-1][1]) > 1e-6:
                    ded.append(p)
            out[-1] = (out[-1][0], ded)
            k += 2
            continue
        out.append((L, list(pts)))
        k += 1
    return out


def drop_flat_detours(B, P, runs, maxlen=1.6):
    """Merge a short run on one layer that sits between two runs on the *same* other layer, when the straight line
    joining its neighbours' ends is clear at the pair's full width on that layer: the search took a leg down and came
    straight back up, and nothing is crossed in between.

    drop_layer_blips() merges the one-cell version of this by length (its 0.3 mm limit exists because a real layer hop
    has to be wider than the obstacle it crosses).  These are longer and still pointless: measured on the rev-9 board
    (tools/route.log, full24/pairs36), H2_MS_DP and H2_MS_DM each dipped to In2.Cu for 1.2 mm and again for 0.75 mm
    between runs on F.Cu, with no copper of any other net within 1 mm of either detour - 4 vias per line to pass under
    nothing, where the rev-8 board had 2.  So test for the obstacle instead of guessing at a length: the merge is only
    taken when the fat centre line (PAIR_FAT, the pair's whole envelope) passes the router's own free-space test on the
    layer it would stay on."""
    out = []
    k = 0
    while k < len(runs):
        L, pts = runs[k]
        if out and k + 1 < len(runs) and out[-1][0] == runs[k + 1][0] != L:
            ln = sum(math.hypot(b[0] - a[0], b[1] - a[1]) for a, b in zip(pts, pts[1:]))
            a, b = out[-1][1][-1], runs[k + 1][1][0]
            br = math.hypot(b[0] - a[0], b[1] - a[1])
            if ln <= maxlen and br > 1e-6 and items_legal(
                    B, P, [(P, 'seg', (a[0], a[1], b[0], b[1], PAIR_FAT, out[-1][0]))], strict=True):
                # keep the rest of the following run: only its *first* point is replaced by the straight bridge
                out[-1] = (out[-1][0], out[-1][1] + [b] + list(runs[k + 1][1][1:]))
                k += 2
                continue
        out.append((L, list(pts)))
        k += 1
    return out


def pair_vias(centre, u, s_route, dt=0.0):
    """The two via positions of a pair's layer change: PAIR_VIA_O either side of the centre line, offset dt/2
    along the direction of travel.  dt = 0 is the square pair; a positive dt staggers the two lines in line,
    which fits a narrow band where a square pair does not (the pair then carries dt of extra skew: 0.6 mm is
    about 4 ps, and the design's own pairs already run up to 0.99 mm of skew).  The site search and the store
    both call this, so they cannot disagree about where the vias go."""
    nl = left_of(u)
    return [(round((centre[0] + nl[0] * s * PAIR_VIA_O - u[0] * s * dt / 2) / G) * G,
             round((centre[1] + nl[1] * s * PAIR_VIA_O - u[1] * s * dt / 2) / G) * G)
            for s in (s_route, -s_route)]


def snap_via_site(R, x, y, d, avail=None):
    """Nearest cell to (x, y) along the travel direction d where a via pair fits (or None): a clear disc of
    PAIR_VIA_R, else (tight spots) the fat centre line free on both layers and both vias legal, either square
    (across d) or staggered in line by a small dt (see pair_vias), which needs a narrower band.

    ``avail(cell)`` filters the candidates further; lane_search passes the straight-run test through it.  A via
    row is only usable where the pair runs *straight into and out of* it (its two lines meet their vias square),
    and this search used to ignore that: it returned the first cell with room for the vias, which could be a
    corner of the lane.  store() then found no straight run (it falls back to k = 0, the row on the corner) and,
    with the square pair illegal there, staggered the vias - and a stagger moves one via dt/2 *back* along the
    pair, i.e. straight into the inner corner of the other line (measured, H1_MS: "P via (109.25,145.75) / N
    track (109.71,146.00)-(109.36,146.35)", lines 0.129 mm apart at (109.39,145.96)).  Requiring the straight
    stretch makes the search slide along the lane to where the row really fits, which is what rev 5's own vias
    did (both of its H1_MS rows sit mid-run)."""
    def ok(j, i):
        return avail is None or avail((j, i))
    for k in range(0, 81):                 # first the clear-disc test the HS lanes were planned with
        #                                    (2.0 mm along the leg: at 0.625 and 1.0 mm a *square* pair was often not reached,
        #                                    and the staggered pair that answered instead pins its own lines - see
        #                                    the H1_MS note in LANES; 81 cells is 2.0 mm at the 0.025 grid)
        for sgn in ((1, -1) if k else (1,)):
            j, i = gi(y + sgn * k * G * d[1], Y0) - R.j0, gi(x + sgn * k * G * d[0], X0) - R.i0
            if 0 <= j < R.ny and 0 <= i < R.nx and R.via_ok[j, i] and R.free[0][j, i] and R.free[1][j, i] \
                    and ok(j + R.j0, i + R.i0):
                return j + R.j0, i + R.i0
    u = unit(d)
    for dt in (0.0, 0.6, 0.9, 1.2):
        for k in range(0, 81):             # then the two actual via positions (the same reach as above)
            for sgn in ((1, -1) if k else (1,)):
                cx, cy = x + sgn * k * G * u[0], y + sgn * k * G * u[1]
                j, i = gi(cy, Y0) - R.j0, gi(cx, X0) - R.i0
                if not (0 <= j < R.ny and 0 <= i < R.nx and R.free[0][j, i] and R.free[1][j, i]):
                    continue
                if not ok(j + R.j0, i + R.i0):
                    continue
                cx, cy = X0 + (i + R.i0) * G, Y0 + (j + R.j0) * G
                vias = pair_vias((cx, cy), u, 1, dt)
                if all(legal_via(R.B, vx, vy, R.name) for vx, vy in vias):
                    return j + R.j0, i + R.i0
                if VIA_DEBUG and dt == 0.0:
                    print(f'    via-site ({cx:.2f},{cy:.2f}) k={k}: vias '
                          f'{[(round(vx, 2), round(vy, 2), legal_via(R.B, vx, vy, R.name)) for vx, vy in vias]}',
                          flush=True)
    return None


def kink_skew(pts):
    """Skew a pair takes from the corners of a centre-line path ``pts`` (in mm).

    A turn of exterior angle phi makes the outside line longer by PAIR_O*tan(phi/2) and the inside line shorter by the
    same, so the pair's two lines come out 2*PAIR_O*tan(phi/2) apart in length at every corner.  A 45 degree turn
    costs 0.19 mm, a 90 degree turn 0.45 mm, and a sharp kink (a zig-zag of turns the same way) runs away - which is
    exactly what H0_OT_DM's 4.72 mm of skew was made of, while every straight part of that link matched to 0.05 mm.
    Used by lane_search to decide that a leg's corners cost the pair more than an extra via pair would.
    """
    tot = 0.0
    for a, b, c in zip(pts, pts[1:], pts[2:]):
        d1, d2 = (b[0] - a[0], b[1] - a[1]), (c[0] - b[0], c[1] - b[1])
        n1, n2 = math.hypot(*d1), math.hypot(*d2)
        if n1 < 1e-6 or n2 < 1e-6:
            continue
        cosphi = max(-1.0, min(1.0, (d1[0] * d2[0] + d1[1] * d2[1]) / (n1 * n2)))
        tot += 2.0 * PAIR_O * abs(math.tan(math.acos(cosphi) / 2.0))
    return tot


def lane_search(B, P, src, start_dir, wps, endB, end_dir, layers, lc):
    """Centre-line path from src through the waypoints (x, y, layer after, travel direction) to endB.
    Every leg stays on one layer; layer changes happen only at waypoints (as via pairs)."""
    path, L, sdir = [], 0, start_dir
    stops = list(wps) + [(endB[0], endB[1], 0, end_dir)]
    for n, (x, y, Lafter, d) in enumerate(stops):
        last = n == len(stops) - 1
        a = (XS[src[2]], YS[src[1]])
        leg = None
        # The ladder is the routing window's growth when a leg has no answer: 0 / 15 / 40 mm.  It was tried short
        # (0 / 2 / 4 mm) to force an early descent, and that is wrong as a *global* rule - measured, H0_KB's short leg
        # at its own via pair (105.0, 150.5) then had no room at all and the pair failed.  What actually stops the
        # eastward walk is the search itself: with h_weight 1.0 below the leg is the shortest path in the window, so a
        # longer way round is only taken when there is no shorter one - and when there is none, the two-layer candidate
        # below (via pair, short window) is what answers, which is the early descent we want.
        for grow in (0.0, 15.0, 40.0):
            win = window_of([a, (x, y)], PAIR_WINDOW + grow)
            # fan_cost=0: this search follows a hand-planned lane, and those lanes cross the AT32s' and hubs' fan-out
            # bands by design (LANES / H0_LANES) - charging them for it starves the lane search
            R = NetRouter(B, P, PAIR_FAT, win, via_r=PAIR_VIA_R, layers=(L,), max_turn=1,
                          layer_cost=lc, via_cost=60.0, h_weight=1.0, fan_cost=0.0)
            runs = [([], [])]
            if not last and Lafter != L:
                R2 = NetRouter(B, P, PAIR_FAT, win, via_r=PAIR_VIA_R, layers=(0, 1), max_turn=1, fan_cost=0.0)
                # A via row needs a straight stretch of the pair on both sides (its two lines meet their vias
                # square); until this test was passed to the site search, the search could return a corner and
                # store() fell back to k = 0 there.  Six cells (0.15 mm) is the middle rung of store()'s own
                # ladder (15 / 10 / 6 / 3 / 0), so a site found here always leaves it a straight run to use.
                _avail = (lambda cell, L=L, Lafter=Lafter, d=d:
                          straight_cells(R2, (L, cell[0], cell[1]), (-d[0], -d[1]), 6) is not None
                          and straight_cells(R2, (Lafter, cell[0], cell[1]), d, 6) is not None)
                # The row is square to d, the direction the pair continues in after the waypoint - the same axis
                # store() uses (drow), so the site search and the store cannot disagree about where the vias go.
                site = snap_via_site(R2, x, y, d, avail=_avail)
                if site is None:
                    return None, f'no via-pair site near waypoint {n} ({x}, {y})'
                tj, ti = site
                # the pair runs straight into and out of its via pair, so both vias sit square to it (the longest
                # straight run that is free and that the leg can reach)
                runs = []
                for k in (PAIR_VIA_RUN, 10, 6, 3, 0):
                    run_in = straight_cells(R2, (L, tj, ti), (-d[0], -d[1]), k)
                    run_out = straight_cells(R2, (Lafter, tj, ti), d, k)
                    if run_in is not None and run_out is not None:
                        runs.append((run_in, run_out))
            else:
                tj, ti = gi(y, Y0), gi(x, X0)
            for before, after in runs:
                tgt = before[-1] if before else (L, tj, ti)
                leg = R.search({src}, {tgt}, (x, y), start_dir=sdir, end_dir=d, max_exp=6_000_000)
                if leg:
                    break
            if leg:
                break
        # A pair pays for its corners in skew and for its detours in length, and a via pair costs about 0.45 mm of the
        # former whatever the path takes.  So the leg is searched a second time with *both* layers at PAIR_VIA_COST -
        # i.e. with a via pair allowed to take it down early - when the single-layer answer (a) has no answer at all in
        # these short windows, (b) spends more than KINK_SKEW_MAX mm of skew on its own corners (what made H0_OT_DM
        # 4.72 mm longer than its mate), or (c) runs more than DETOUR_MAX times its own straight distance.  This is the
        # piece that was missing: with only the skew and detour tests, a leg the short ladder could not route simply
        # failed, and the wide 15 / 40 mm rungs that used to cover for it are what walked H0_OT east to x 186.05.
        _plen = lambda q: sum(math.hypot(b[0] - a2[0], b[1] - a2[1]) for a2, b in zip(q, q[1:]))
        _straight = math.hypot(x - a[0], y - a[1]) or 1e-6
        if leg is None:
            _need2 = True
        else:
            _pts = [(XS[c[2]], YS[c[1]]) for c in leg]
            _need2 = kink_skew(_pts) > KINK_SKEW_MAX or _plen(_pts) > DETOUR_MAX * _straight
        if _need2:
            Rk = NetRouter(B, P, PAIR_FAT, window_of([a, (x, y)], PAIR_WINDOW + max(grow, 3.0)), via_r=PAIR_VIA_R,
                          fan_cost=0.0,
                           layers=tuple(layers), max_turn=1, layer_cost=lc, via_cost=PAIR_VIA_COST, h_weight=1.0)
            leg2 = Rk.search({src}, {tgt}, (x, y), start_dir=sdir, end_dir=d, max_exp=6_000_000)
            if leg2:
                _pts2 = [(XS[c[2]], YS[c[1]]) for c in leg2]
                if leg is None:
                    if VIA_DEBUG:
                        print(f'    lane leg {n}: no single-layer path -> two layers, '
                              f'{kink_skew(_pts2):.2f} mm skew / {_plen(_pts2):.2f} mm', flush=True)
                    leg = leg2
                else:
                    _score = lambda q: (0.0 if kink_skew(q) <= KINK_SKEW_MAX else kink_skew(q), _plen(q))
                    if _score(_pts2) < _score(_pts):
                        if VIA_DEBUG:
                            print(f'    lane leg {n}: {kink_skew(_pts):.2f} mm skew / {_plen(_pts):.2f} mm -> '
                                  f'{kink_skew(_pts2):.2f} mm / {_plen(_pts2):.2f} mm', flush=True)
                        leg = leg2
        if not leg:
            R = NetRouter(B, P, PAIR_FAT, window_of([a, (x, y)], WINDOW + 40.0), via_r=PAIR_VIA_R, layers=(L,),
                          fan_cost=0.0,
                          max_turn=1, layer_cost=lc, via_cost=60.0, h_weight=1.3)
            jj, ii = gi(y, Y0) - R.j0, gi(x, X0) - R.i0
            if VIA_DEBUG:
                inside = 0 <= jj < R.ny and 0 <= ii < R.nx
                print(f'    lane leg {n} FAIL to ({x},{y}) L={"FB"[L]}: cell_inside={inside} '
                      f'free={R.free[L][jj, ii] if inside else None} fd={R.fd[L][jj, ii] if inside else -1:.2f} '
                      f'edge={R.edge_d[jj, ii] if inside else -1:.2f} start={a} src={src} endd={d}', flush=True)
            return None, f'leg {n} -> ({x}, {y}) on {"FB"[L]}.Cu | ' + block_report(B, P, [(x, y), a])
        path += leg if not path else leg[1:]
        sdir = d
        if not last and Lafter != L:
            path += list(reversed(before[:-1])) + ([(L, tj, ti)] if before else []) + [(Lafter, tj, ti)] + after
            L = Lafter
        src = path[-1]
    return path, ''


def route_pair(B, P, N, termA, termB, log, layers=(0, 1), b_cost=1.6, twist_at='B', wps=(), ends=('', '')):
    """Route one differential link between two pair terminals. Returns True on success.
    wps: optional lane waypoints (x, y, layer after, travel direction) from A to B.
    ends: 'part.pad' keys of the A and B terminals, so PAIR_END_RUNS can tell the two ends of a net that two links
    reach from different parts (the two SVC links, one on either side of the same ESD).
    twist_at: where the crossover goes when the two ends disagree on which side P is: 'A' / 'B', a zone in front of that
    terminal; 'run', inside the run (twist_in_run), which needs no room at either end."""
    escA, stA_P, stA_N, sA = fanout(termA)
    escB, stB_P, stB_N, sB = fanout(termB)
    outA, outB = termA[2], termB[2]
    # handedness: going from A to B the P line must stay on the same side; arriving at B the travel
    # direction is -outB, so P must be on side sA relative to -outB.  B's own side is relative to outB.
    twist = (sA == sB)            # left of -outB == right of outB
    in_run = twist and twist_at == 'run'
    stubs = []
    for nm, pts in ((P, stA_P), (N, stA_N), (P, stB_P), (N, stB_N)):
        for a, b in zip(pts, pts[1:]):
            stubs.append((nm, (a[0], a[1], b[0], b[1], PAIR_W, 0)))
    for nm, d in stubs:
        B.add_track(nm, 'seg', d)
    endB, startA, s_route = escB, escA, sA
    twist_items = []
    if twist and not in_run:
        done = False
        if twist_at == 'A':
            # swap right after A's fan-out (A = the AT32, which has room around its pins)
            items, Z1 = swap_zone(P, N, escA, outA, sA, gap=1.0)
            if items_legal(B, P, items):
                twist_items, startA, s_route, done = items, Z1, -sA, True
        if not done:
            # swap in front of B: P/N cross, one line hops over the other on B.Cu
            Lt = 1.6
            T = (round((escB[0] + outB[0] * Lt) / G) * G, round((escB[1] + outB[1] * Lt) / G) * G)
            twist_items, _ = swap_zone(P, N, T, (-outB[0], -outB[1]), sA, gap=0.0)
            endB = T
        for nm, k, d in twist_items:
            B.add_track(nm, k, d)
    lc = b_cost if isinstance(b_cost, tuple) else (1.0, b_cost)
    path, why = None, ''
    # add_track() updates the dynamic fields item by item; one rebuild here makes the leg search see the fan-out and twist
    # copper as a single picture, consistent with B.tracks (cheap next to the search itself).
    B.rebuild()
    # the pair leaves and reaches its fan-outs straight for a few cells, so its offset lines meet the stubs square
    cA, cB = (0, gi(startA[1], Y0), gi(startA[0], X0)), (0, gi(endB[1], Y0), gi(endB[0], X0))
    kA = PAIR_END_RUNS.get((P, 'A', ends[0]), PAIR_END_RUNS.get((P, 'A'), PAIR_END_RUN))
    kB = PAIR_END_RUNS.get((P, 'B', ends[1]), PAIR_END_RUNS.get((P, 'B'), PAIR_END_RUN))
    # The freedom window has to cover the run asked for.  At a flat 2.0 mm it reached only 40 cells, so any longer
    # request - the 2.75 mm (55-cell) escape necks below the input ESDs, see PAIR_END_RUNS - was out of bounds,
    # straight_cells() answered None and the run silently fell back to PAIR_END_RUN (0.2 mm).  Measured on the board
    # before this fix: 0.76 mm of straight run below U101/U301 against 1.31 mm above them, i.e. the pair converged
    # right under the ESD's middle pad instead of running down beside it.
    RA = NetRouter(B, P, PAIR_FAT, window_of([startA], max(2.0, (kA + 4) * G + 0.4)), layers=(0,), fan_cost=0.0)
    RB = NetRouter(B, P, PAIR_FAT, window_of([endB], max(2.0, (kB + 4) * G + 0.4)), layers=(0,), fan_cost=0.0)
    if os.environ.get('ROUTE_END_RUN_DEBUG'):
        for tag, R_, c_, d_, k_ in (('A', RA, cA, outA, kA), ('B', RB, cB, outB, kB)):
            print(f'  end run {P} {tag}: asked {k_} cells ({k_ * G:.2f} mm), '
                  f'got {None if straight_cells(R_, c_, d_, k_) is None else k_}', flush=True)
    head = next((h for h in (straight_cells(RA, cA, outA, k) for k in (kA, PAIR_END_RUN, 2, 0)) if h is not None), [])
    tail = next((t for t in (straight_cells(RB, cB, outB, k) for k in (kB, PAIR_END_RUN, 2, 0)) if t is not None), [])
    s0 = head[-1] if head else cA
    e0 = tail[-1] if tail else cB
    e0xy = (X0 + e0[2] * G, Y0 + e0[1] * G)
    if wps:
        path, why = lane_search(B, P, s0, outA, wps, e0xy, (-outB[0], -outB[1]), layers, lc)
    else:
        win = window_of([startA, endB], WINDOW)
        for grow in (0.0, 15.0, 40.0):
            w = (win[0] - grow, win[1] - grow, win[2] + grow, win[3] + grow)
            R = NetRouter(B, P, PAIR_FAT, w, via_r=PAIR_VIA_R, layers=layers, max_turn=1, fan_cost=0.0,
                          layer_cost=lc, via_cost=60.0, h_weight=1.3)
            path = R.search({s0}, {e0}, e0xy, start_dir=outA, end_dir=(-outB[0], -outB[1]), max_exp=6_000_000)
            if path:
                break
        if not path:
            # A link with no planned lane that fails in the general search has to say so: this branch used to leave a bare
            # "PAIR FAIL P/N" with no reason in the log (the RP_DP link failed silently here on the first rev-8 run).
            why = f'no pair path from {s0} to {e0} within {WINDOW + 40.0:.0f} mm of the direct line'
            log.append('  ' + block_report(B, P, [(X0 + s0[2] * G, Y0 + s0[1] * G), (X0 + e0[2] * G, Y0 + e0[1] * G)]))
            # The block report only lists the copper *cells* around the two ends, and the ones that matter here are
            # foreign nets - the pair's own stubs and empty cells say nothing.  The straight attempt between the ends
            # says which cell the fat centre line cannot occupy and what it hits there (fd = clearance to the nearest
            # foreign copper of the net code it names), which is what distinguishes a tight corridor from a search that
            # never started: Hub-0's port-4 stub pair fails this way with nothing foreign anywhere near either end.
            log.append('  straight attempt: ' + str(items_conflict(B, P, [
                (P, 'seg', (X0 + s0[2] * G, Y0 + s0[1] * G, e0xy[0], e0xy[1], PAIR_FAT, 0))])))
    if path:
        path = [cA] + head[:-1] + path + list(reversed(tail[:-1])) + ([cB] if tail else [])
    if not path:
        log.append(f'  PAIR FAIL {P}/{N} {why}')
        for nm, d in stubs:
            B.tracks.remove((nm, 'seg', tuple(d)))
        for nm, k, d in twist_items:
            B.tracks.remove((nm, k, tuple(d)))
        B.rebuild()
        return False
    runs = recut_rows(drop_flat_detours(B, P, drop_layer_blips(centre_runs(path))))
    # offset lines; P is on side sA relative to the travel direction
    items = []
    prev_end = None
    dump = os.environ.get('ROUTE_DUMP_PAIR')          # debugging: the pair's centre runs and offset lines
    dbg = [] if dump in (P, N, 'all') else None
    for k, (L, pts) in enumerate(runs):
        if len(pts) < 2:
            pts = [pts[0], pts[0]]
        if math.hypot(pts[-1][0] - pts[0][0], pts[-1][1] - pts[0][1]) < 1e-6:
            continue
        pts = smooth_turns(unloop(pts))       # a loop in the centre line folds the pair's own lines onto each other
        lp = offset_polyline(pts, s_route * PAIR_O)
        ln = offset_polyline(pts, -s_route * PAIR_O)
        lp, ln = trim_pair_tails(lp, ln)      # a cut mitre at a sharp tail otherwise ends both lines on one point
        if dbg is not None:
            dbg.append((L, pts, lp, ln))
        if prev_end is not None:
            # layer change: two vias either side of the centre line.  Square is preferred; when only a narrow band
            # is free, pair_vias staggers the two lines in line instead (the site search used the same helper, so
            # the two agree on the geometry).
            (pL, pp_end, pn_end, din) = prev_end
            v = pts[0]
            # The row is square to the direction the pair *continues* in, not to the one it arrives in.  The lines
            # emitted from this row (lp / ln) are offset perpendicular to the new run, so a row placed across din
            # disagrees with them whenever the pair turns at the via: measured, H1_MS arrived at 45 degrees and left
            # due west, the vias went onto a diagonal row, and the N via came out 0.45 mm from the P line's centre
            # (needs 0.525) - "lines only 0.075 mm apart (want 0.150) at (114.21,145.62)".  Square to the new run,
            # the two lines' connectors fan out from the incoming offsets to the vias (mirrored about the axis), which
            # keeps each line on its own side.  A pair that arrives straight through the row has din == -dnew, so
            # nothing changes for the common case.
            # That rule is only right when the turn is *gentle*: at a square corner the arriving line runs straight
            # into the row's own ends, and a row square to the new run puts one via on top of the partner's line -
            # measured on H0_MS (arrives west along its pad row, turns north at the via): "P via (144.85,153.75) /
            # N track (144.90,153.90)-(144.61,153.90)", 0.225 mm, i.e. it failed on its *own* copper and not for want
            # of room.  So the axis is measured, not assumed: the square-to-the-new-run row is taken when the pair's
            # own gap survives it, and the square-to-the-arrival row (and the staggered rows on the new run) are the
            # fallbacks - the same pair_min_gap() that fails the link afterwards decides, so an axis that would pinch
            # is never emitted.
            drow = run_dir(pts) or din

            def row_items(vP, vN):
                return [(P, 'seg', (pp_end[0], pp_end[1], vP[0], vP[1], PAIR_W, pL)), (P, 'via', vP),
                        (N, 'seg', (pn_end[0], pn_end[1], vN[0], vN[1], PAIR_W, pL)), (N, 'via', vN),
                        (P, 'seg', (vP[0], vP[1], lp[0][0], lp[0][1], PAIR_W, L)),
                        (N, 'seg', (vN[0], vN[1], ln[0][0], ln[0][1], PAIR_W, L))]

            best = None
            for u, dt in ((drow, 0.0), (din, 0.0), (drow, 0.6), (drow, 0.9), (drow, 1.2)):
                vP, vN = pair_vias(v, u, s_route, dt)
                if not (legal_via(B, vP[0], vP[1], P) and legal_via(B, vN[0], vN[1], N)):
                    continue
                trial = row_items(vP, vN)
                gap, _gw, _gd = pair_min_gap(items + trial, P, N)
                if gap >= PAIR_S + 0.10:               # this row is clean: take it, no need to look further
                    best = (gap, u, dt, vP, vN, trial)
                    break
                if best is None or gap > best[0] + 1e-9:
                    best = (gap, u, dt, vP, vN, trial)
            if best is None:                            # no legal site at all: let the caller's check report the pinch
                vP, vN = pair_vias(v, drow, s_route, 0.0)
                best = (0.0, drow, 0.0, vP, vN, row_items(vP, vN))
            rgap, ru, rdt, vP, vN, trial = best
            if VIA_DEBUG:
                print(f'    row L{pL}->{L} at {v} din={tuple(round(q, 2) for q in din)} '
                      f'drow={tuple(round(q, 2) for q in drow)} -> u={tuple(round(q, 2) for q in ru)} dt={rdt} '
                      f'gap={rgap:.3f} P={tuple(round(q, 2) for q in pp_end)}->{vP} '
                      f'N={tuple(round(q, 2) for q in pn_end)}->{vN}', flush=True)
            items += trial
        for a, b in zip(lp, lp[1:]):
            items.append((P, 'seg', (a[0], a[1], b[0], b[1], PAIR_W, L)))
        for a, b in zip(ln, ln[1:]):
            items.append((N, 'seg', (a[0], a[1], b[0], b[1], PAIR_W, L)))
        din = unit((pts[-1][0] - pts[-2][0], pts[-1][1] - pts[-2][1]))
        prev_end = (L, lp[-1] if lp else None, ln[-1] if ln else None, din)
    run_note = ''
    if in_run:
        # routed as if no crossover were needed, so P's line ends on N's stub at B: the twist goes into the run now
        segs = lambda pts: [(a[0], a[1], b[0], b[1], PAIR_W, 0) for a, b in zip(pts, pts[1:])]
        twisted, run_note = twist_in_run(B, P, N, items, segs(stA_P) + segs(stA_N), segs(stB_P) + segs(stB_N), log)
        if twisted is None:
            for nm, d in stubs:
                B.tracks.remove((nm, 'seg', tuple(d)))
            B.rebuild()
            log.append(f'  PAIR FAIL {P}/{N} {run_note}')
            return False
        items = twisted
        run_note = ', ' + run_note
    for nm, k, d in items:
        if k == 'seg' and math.hypot(d[2] - d[0], d[3] - d[1]) < 1e-4:
            continue
        B.add_track(nm, k, d)
    lenP = sum(math.hypot(d[2] - d[0], d[3] - d[1]) for nm, k, d in items if nm == P and k == 'seg')
    lenN = sum(math.hypot(d[2] - d[0], d[3] - d[1]) for nm, k, d in items if nm == N and k == 'seg')
    nv = sum(1 for nm, k, d in items if k == 'via')
    if dbg is not None:                       # why the two lines do not look like a pair: their own runs
        for k, (L, pts, lp, ln) in enumerate(dbg):
            print(f'  DUMP {P}/{N} run {k} L{"FB"[L]} centre {len(pts)} pts {pts[0]} -> {pts[-1]}', flush=True)
            print('       C ' + ' '.join(f'({q[0]:.2f},{q[1]:.2f})' for q in pts), flush=True)
            print(f'       P {len(lp)} pts ' + ' '.join(f'({q[0]:.2f},{q[1]:.2f})' for q in lp), flush=True)
            print(f'       N {len(ln)} pts ' + ' '.join(f'({q[0]:.2f},{q[1]:.2f})' for q in ln), flush=True)
    gap, gwhere, gdetail = pair_min_gap([(P, 'seg', d) for nm, d in stubs if nm == P] + [(N, 'seg', d) for nm, d in stubs if nm == N]
                                        + twist_items + items, P, N)
    if gap < PAIR_S - 1e-6:
        # A pair's own two lines must keep the pair spacing (PAIR_S = the DRC clearance).  A corridor a fraction of
        # a millimetre too narrow used to be answered by squeezing the two lines together, which is a short; fail
        # instead, so the caller's retry ladder (other link orders, other waypoints) gets another chance.
        for nm, d in stubs:                       # stubs are (net, segment) without a kind field
            if (nm, 'seg', tuple(d)) in B.tracks:
                B.tracks.remove((nm, 'seg', tuple(d)))
        for nm, k, d in list(twist_items) + items:
            if (nm, k, tuple(d)) in B.tracks:
                B.tracks.remove((nm, k, tuple(d)))
        B.rebuild()
        log.append(f'  PAIR FAIL {P}/{N} lines only {gap:.3f} mm apart (want {PAIR_S:.3f} mm) at '
                   f'({gwhere[0]:.2f},{gwhere[1]:.2f}) on {gwhere[2]} | {gdetail}')
        return False
    log.append(f'{P}/{N}: {len(path)} cells, P {lenP:.2f} mm N {lenN:.2f} mm (skew {abs(lenP - lenN):.2f} mm), '
               f'{nv} vias{", swapped" if twist else ""}{run_note}, P-N gap {gap:.3f} mm')
    return True


# ── normal nets: routing with rip-up ──────────────────────────────────────────
def width_of(name):
    return POWER.get(name, W_SIG)


def layer_width(name, width, L):
    """A net's track width on routing layer L: a power rail at full width takes its width for that layer's copper
    (RAIL_LW); anything else (signals, a rail's light-load branches) is the same on every layer."""
    return RAIL_LW[name][L] if name in RAIL_LW and width == POWER[name] else width


def terminals(B, name):
    groups = []
    for ref, shapes in B.pads[name]:
        x0, y0, x1, y1 = shapes[0].bbox()
        c = pad_cells(shapes)
        groups.append([ref, c, set(c), ((x0 + x1) / 2, (y0 + y1) / 2)])
    # copper already on the net (fixed macros, pair stubs) merges the pads it touches; each connected piece of it
    # merges only its own pads, so fan-out stubs at both ends of a link still leave two terminals
    pieces = []
    for n, k, d in B.tracks:
        if n == name:
            cells = track_cells([(k, d)])
            for p in [p for p in pieces if p & cells]:
                cells |= p
                pieces.remove(p)
            pieces.append(cells)
    for cells in pieces:
        merged = None
        for g in list(groups):
            if g[2] & cells:
                if merged is None:
                    merged = g
                    merged[1] |= cells
                    merged[2] |= cells
                else:
                    merged[0] += '+' + g[0]
                    merged[1] |= g[1]
                    merged[2] |= g[2]
                    groups.remove(g)
    return [tuple(x) for x in groups]


MATE_ESCAPE = 1.0        # mm of straight F.Cu kept clear in front of an unrouted USB mate's IC pin


def mate_corridors(B, name):
    """While a full-speed USB line routes, the short straight way out of IC pins whose nets have no copper yet stays
    clear (no track, no via): its other line's pins (two adjacent pins otherwise take turns walling each other in) and
    its own pins' neighbours on the same IC (a USB line routed early must not wrap round in front of a select pin).
    ICs with a planned via fan-out (FANOUT_IC) reserve their pins' ways out already."""
    mate = mate_of(name)
    if not mate:
        return []
    routed = {t[0] for t in B.tracks}
    keys = []
    if mate in B.pads and mate not in B.pair_of and mate not in routed:
        keys += [(ref, shapes) for ref, shapes in B.pads[mate]]
    pad_net = {}
    for ref, _ in B.pads[name]:
        r, num = ref.split('.')
        if num.isdigit() and r not in FANOUT_IC:
            for nb in (int(num) - 1, int(num) + 1):
                pad_net[f'{r}.{nb}'] = None
    if pad_net:
        for n2, pads in B.pads.items():
            if n2 in (name, mate) or n2 in PLANES or n2 in B.pair_of or n2 in routed:
                continue
            keys += [(ref, shapes) for ref, shapes in pads if ref in pad_net]
    out = []
    for ref, shapes in keys:
        fp = B.board.FindFootprintByReference(ref.split('.')[0])
        if fp is None or len(fp.Pads()) < 6 or ref.split('.')[0] in FANOUT_IC:
            continue
        x0, y0, x1, y1 = shapes[0].bbox()
        px, py = (x0 + x1) / 2, (y0 + y1) / 2
        dx, dy = px - MM(fp.GetPosition().x), py - MM(fp.GetPosition().y)
        if abs(dx) > abs(dy):
            ex = x1 if dx > 0 else x0
            out.append((ex, py, ex + math.copysign(MATE_ESCAPE, dx), py))
        else:
            ey = y1 if dy > 0 else y0
            out.append((px, ey, px, ey + math.copysign(MATE_ESCAPE, dy)))
    return out


def route_net(B, name, width, log, plan=None):
    """Route one net. With plan (a dict) nothing is added to the board: the tracks and every window the searches
    looked at are collected in plan['tracks'] / plan['windows'] for a later commit (see route_all)."""
    p = plan if plan is not None else {}
    p.setdefault('tracks', [])
    p.setdefault('windows', [])
    own_vias = []
    avoid = mate_corridors(B, name)
    terms = terminals(B, name)
    if len(terms) < 2:
        return []
    pads = [(lbl, tgt) for lbl, tgt, _, _ in terms]
    allcells = [a for _, _, a, _ in terms]
    centers = [c for _, _, _, c in terms]
    light = LIGHT_LOADS.get(name, ())
    is_light = [bool(light) and all(part.startswith(light) for part in lbl.split('+')) for lbl, _ in pads]
    heavy = [k for k in range(len(pads)) if not is_light[k]] or list(range(len(pads)))
    cx = sum(centers[k][0] for k in heavy) / len(heavy)
    cy = sum(centers[k][1] for k in heavy) / len(heavy)
    root = min(heavy, key=lambda k: (centers[k][0] - cx) ** 2 + (centers[k][1] - cy) ** 2)
    tree = set(allcells[root])
    tree_pts = [centers[root]]
    remaining = [k for k in range(len(pads)) if k != root]
    failed = []
    while remaining:
        k = min(remaining, key=lambda k: (is_light[k], min((centers[k][0] - p[0]) ** 2 + (centers[k][1] - p[1]) ** 2
                                                            for p in tree_pts)))
        remaining.remove(k)
        ref, tgt = pads[k]
        if tgt & tree:
            tree |= allcells[k]
            tree_pts.append(centers[k])
            continue
        path = None
        near = min(tree_pts, key=lambda p: (p[0] - centers[k][0]) ** 2 + (p[1] - centers[k][1]) ** 2)
        rail = name in POWER
        wk = min(width, LIGHT_W) if is_light[k] else width
        widths = {wk} if rail else {width, min(width, 0.25), W_SIG}
        for w in sorted(widths, reverse=True):
            for grow in ((200.0,) if DESPERATE else (WINDOW, WINDOW * 3, 200.0)):
                win = window_of([near, centers[k]], grow)
                R = NetRouter(B, name, w, win, own_vias=own_vias, avoid=avoid, neck=rail)
                # the first (small) window is searched directly: in C a sealed-off region fails fast anyway
                if grow != WINDOW and not R.reachable(tree, tgt):
                    p['windows'].append(win)
                    continue
                path = R.search(tree, tgt, centers[k])
                if not path:      # from the open side the search can run out of expansions before it finds a pin's
                    path = R.search(tgt, tree, near)          # escape channel in a dense fan-out: from the pin
                p['windows'].append(R.explored if path and R.explored else win)
                if path:
                    break
            if path:
                break
        if not path:
            failed.append((ref, centers[k]))
            log.append(f'  FAIL {name} -> {ref}')
            continue
        tr = path_to_tracks(path, cell_widths(R, path, w) if rail else w)
        p['tracks'] += tr
        own_vias += [d for kind, d in tr if kind == 'via']
        tree |= allcells[k] | track_cells(tr)
        tree_pts.append(centers[k])
    if plan is None:
        for kind, data in p['tracks']:
            B.add_track(name, kind, data)
    return failed


def harden(R, B, nets, width):
    """Rip-through (soft) cells next to copper of these nets become blocked: that copper cannot be ripped up."""
    thr, vthr = [CLR + w / 2 + EPS for w in R.lw], CLR + VIA_D / 2 + EPS     # per layer: a rail is wider on In2.Cu
    for nm, kind, d in B.tracks + B.reserved + B.escape_items():
        if nm not in nets or nm == R.name:
            continue
        if kind == 'seg':
            sh, layers = Shape('seg', (d[0], d[1], d[2], d[3], d[4] / 2), -1, (int(d[5]),)), (int(d[5]),)
        else:
            sh, layers = Shape('circle', (d[0], d[1], VIA_D / 2), -1, tuple(range(NL))), tuple(range(NL))
        j0, j1, i0, i1 = sh.window()
        j0, j1, i0, i1 = max(j0, R.j0), min(j1, R.j1), max(i0, R.i0), min(i1, R.i1)
        if j0 >= j1 or i0 >= i1:
            continue
        X, Y = np.meshgrid(XS[i0:i1], YS[j0:j1])
        dd = sh.dist(X, Y)
        sj, si = slice(j0 - R.j0, j1 - R.j0), slice(i0 - R.i0, i1 - R.i0)
        for L in layers:
            R.soft[L][sj, si] &= dd >= thr[L]
        R.via_soft[sj, si] &= dd >= vthr


def find_blockers(B, name, width, plane_hits=None, hard=()):
    """Nets whose copper a soft (rip-through) route of this net crosses; plane-net cells go to plane_hits. The route
    does not cross copper of the nets in hard (pairs, protected or rip-limited nets: they are not ripped up anyway)."""
    plane_codes = {B.code(pn): pn for pn in PLANES}
    terms = terminals(B, name)
    blockers = set()
    tree = set(terms[0][2])
    hard = set(hard)
    for ref, cells, allc, centre in terms[1:]:
        win = window_of([terms[0][3], centre], WINDOW * 2)
        R = NetRouter(B, name, width, win, soft=True, neck=name in POWER)
        if hard:
            harden(R, B, hard, width)
        path = R.search(tree, cells, centre, max_exp=20_000_000) or R.search(cells, tree, terms[0][3], max_exp=20_000_000)
        if not path:
            continue
        own = B.code(name)
        hits = []
        for (L, j, i) in path:
            jj, ii = j - R.j0, i - R.i0
            if not R.flat[L][jj * R.nx + ii]:
                hits.append((L, j, i, B.dyn.n1[L, j, i]))
        for a, b in zip(path, path[1:]):          # a layer change where no via fits yet: what is too close to it
            if a[0] != b[0] and not R.viaf[(a[1] - R.j0) * R.nx + (a[2] - R.i0)]:
                for Lx in range(NL):
                    if B.dyn.d1[Lx, a[1], a[2]] < CLR + VIA_D / 2 + EPS:
                        hits.append((Lx, a[1], a[2], B.dyn.n1[Lx, a[1], a[2]]))
        for L, j, i, n in hits:
            if n == own:
                continue
            if n in plane_codes and plane_hits is not None:
                plane_hits.append((plane_codes[n], L, j, i))
            for nm, c in B.netcode.items():
                if c == n:
                    blockers.add(nm)
        tree |= set(path)
    return blockers


def mate_of(name):
    """The other line of a full-speed (normal-net) USB pair: X_DP <-> X_DM, X_DP_PIN <-> X_DM_PIN."""
    m = re.match(r'(.*_D)([PM])(_PIN)?$', name)
    return m and m.group(1) + ('M' if m.group(2) == 'P' else 'P') + (m.group(3) or '')


def escape_order(B, nets, ref):
    """Order a dense IC's nets so none walls in a pin routed after it: per side, nets turning the same way leave
    innermost first (on the south side, of the nets heading east the rightmost pin goes first and hugs the corner;
    the ones further along go round it). Straight-out nets first, nets with pins on several sides (power) last."""
    fp = B.board.FindFootprintByReference(ref)
    c = fp.GetPosition()
    cx, cy = MM(c.x), MM(c.y)

    def centre(shapes):
        x0, y0, x1, y1 = shapes[0].bbox()
        return (x0 + x1) / 2, (y0 + y1) / 2

    def key(n):
        mine = [centre(s) for r, s in B.pads[n] if r.split('.')[0] == ref]
        other = [centre(s) for r, s in B.pads[n] if r.split('.')[0] != ref]
        if len(mine) != 1 or not other:
            return (9, 0, 0.0)
        px, py = mine[0]
        ox, oy = sum(p[0] for p in other) / len(other), sum(p[1] for p in other) / len(other)
        if abs(px - cx) > abs(py - cy):              # west / east side: pins along y
            side, along, d = (0 if px < cx else 2), py, oy - py
        else:                                        # north / south side: pins along x
            side, along, d = (1 if py < cy else 3), px, ox - px
        heading = 0 if abs(d) < 2.0 else (1 if d > 0 else -1)
        return (side, abs(heading), -heading * along)
    return sorted(nets, key=key)


def route_span(B, n):
    xs = [s[0].bbox()[0] for _, s in B.pads[n]]
    ys = [s[0].bbox()[1] for _, s in B.pads[n]]
    return (max(xs) - min(xs)) + (max(ys) - min(ys))


def split_fallback(B, P, N, termA, termB, log, max_len=20.0, max_skew=1.5, min_gap=None):
    """Last resort for a differential link: route the two lanes as *single* nets, one after the other, and keep the
    result only if it is electrically sane - both lanes short, lengths matched, the two lanes not merged.

    Why this may be allowed at all: on this board both lanes are referenced to a solid plane, so at USB 2.0 high speed a
    lone line is 55-60 ohm instead of one 45 ohm leg of a 90 ohm pair, and the differential mode is hardly doing the
    work over a short run - the eye at 480 Mbit/s is set by the connectors, not by 10-20 mm of lane.  What is *not*
    allowed is a long or mismatched split, and the two lanes must not end up merged; so the length, the mismatch and the
    closest approach are measured and logged, and anything outside the budgets is refused rather than kept.
    It is the answer for a link whose own fan-out reservations leave the pair no legal corridor at all: without it the
    link loses its copper completely (a USB connection missing from the board).  On success the link is real copper, but
    marked 'split' in the log so it can be reviewed and, if wanted, fixed by moving the reservations instead."""
    if min_gap is None:
        min_gap = CLR + PAIR_W - 1e-6
    n0 = len(B.tracks)
    av = f'__split_{N}'                       # the first lane's copper, made foreign to the second
    B.release_escape([P, N])                  # the reservations are exactly what the pair could not use: drop them

    def _undo():
        del B.tracks[n0:]
        B.rebuild()
        B.restore_escape([P, N])

    def _one_order(first, second, hard_mate):
        """Route 'first' singly, then 'second' - with the first lane's copper as foreign copper when hard_mate, or with
        the two lanes free to interleave (they end up judged by the measured gap).  None if it does not come out."""
        del B.tracks[n0:]
        B.rebuild()
        if not route_net(B, first, PAIR_W, log):
            return None
        if hard_mate:
            for _nm2, _k2, _d2 in list(B.tracks[n0:]):
                B.add_track(av, _k2, _d2, fixed=True)
        good = route_net(B, second, PAIR_W, log)
        for it in [(nm, k, d) for nm, k, d in B.tracks if nm == av]:
            B.tracks.remove(it)
            if it in B.fixed:
                B.fixed.remove(it)
        B.netcode.pop(av, None)
        B.rebuild()
        return True if good else None

    # Three things are tried, hardest constraint first: each lane first and dodging the other; then each lane with the
    # two free to interleave - which is what a pair squeezed out of a blocked pad field needs.  Free interleaving is
    # only accepted if the measured closest approach passes, so "free" cannot turn into merged copper.
    best = None
    for first, second, hard in ((P, N, True), (N, P, True), (P, N, False), (N, P, False)):
        if not _one_order(first, second, hard):
            log.append(f'  SPLIT {P}/{N}: {first} then {second} '
                       f'({"dodging the mate" if hard else "lanes free"}) does not come out')
            continue

        def _pieces(nm):
            return [d for n2, k, d in B.tracks[n0:] if n2 == nm and k == 'seg']

        def _len(nm):
            return sum(math.hypot(d[2] - d[0], d[3] - d[1]) for d in _pieces(nm))

        def _sample(d):
            n = max(int(math.hypot(d[2] - d[0], d[3] - d[1]) / G), 1)
            return [(d[0] + (d[2] - d[0]) * t / n, d[1] + (d[3] - d[1]) * t / n) for t in range(n + 1)]

        lp, ln = _len(P), _len(N)
        dp, dn = _pieces(P), _pieces(N)
        gap = min((_on_seg(x, y, d2, tol=1e9)[1]
                   for d in dp for x, y in _sample(d) for d2 in dn), default=99.0)
        why = []
        if max(lp, ln) < 0.5:
            # Nothing was measured for one lane: route_net reported success but its copper is not in B.tracks from the
            # mark this function took.  A split that cannot be measured is not a split that may be trusted.
            why.append(f'no copper measured (P {lp:.2f}, N {ln:.2f} mm)')
        if max(lp, ln) > max_len:
            why.append(f'longest lane {max(lp, ln):.2f} mm > {max_len:.0f} mm')
        if abs(lp - ln) > max_skew:
            why.append(f'skew {abs(lp - ln):.2f} mm > {max_skew:.1f} mm')
        if gap < min_gap:
            why.append(f'lanes {gap:.3f} mm apart < {min_gap:.2f} mm')
        if why:
            log.append(f'  SPLIT {P}/{N}: single-net routing refused ({first} first'
                       f'{", dodging" if hard else ", free"}) - ' + '; '.join(why))
            continue
        log.append(f'  SPLIT {P}/{N}: routed as two single nets ({first} first'
                   f'{", dodging the mate" if hard else ", lanes interleaved"}) - P {lp:.2f} mm, '
                   f'N {ln:.2f} mm (skew {abs(lp - ln):.2f} mm), closest approach {gap:.3f} mm '
                   f'- inside {max_len:.0f} mm / {max_skew:.1f} mm, both plane-referenced')
        best = True
        break
    if not best:
        _undo()
        return False
    return True


def route_all(B, names, log, first=(), protect=()):
    """Route names (first ones first, the rest by span) with rip-up. Nets in protect (USB, SPI, crystals) are
    only ever ripped up for each other, never for a lower-priority net."""
    def span(n):
        xs = [s[0].bbox()[0] for _, s in B.pads[n]]
        ys = [s[0].bbox()[1] for _, s in B.pads[n]]
        return (max(xs) - min(xs)) + (max(ys) - min(ys))

    order = [n for n in first if n in names] + sorted((n for n in names if n not in first), key=span)
    queue = list(order)
    ripped_count = {}
    lifted = {}
    walled = {}

    def plan_net(name):
        p = {'version': len(B.changes), 'log': []}
        p['failed'] = route_net(B, name, width_of(name), p['log'], plan=p)
        return p

    def stale(p):
        """Copper changed (added or removed) inside a window the plan looked at since it started."""
        for cx0, cy0, cx1, cy1 in B.changes[p['version']:]:
            for wx0, wy0, wx1, wy1 in p['windows']:
                if cx0 <= wx1 and wx0 <= cx1 and cy0 <= wy1 and wy0 <= cy1:
                    return True
        return False
    # the next WORKERS nets are planned on threads (the C search runs without the GIL) against the board as it is;
    # commits stay in queue order, and a plan whose windows saw a change since it started is planned again, so the
    # result is the same as routing one net after the other
    from concurrent.futures import ThreadPoolExecutor
    pool = ThreadPoolExecutor(WORKERS) if WORKERS > 1 else None
    spec, n_used, n_redo = {}, 0, 0
    while queue:
        if pool is not None:
            for n in queue[:WORKERS]:
                if n not in spec:
                    spec[n] = pool.submit(plan_net, n)
        name = queue.pop(0)
        t = time.time()
        live(net=name)
        p = spec.pop(name).result() if name in spec else None
        if p is None or stale(p):
            n_redo += p is not None
            p = plan_net(name)
        else:
            n_used += 1
        log += p['log']
        for kind, data in p['tracks']:
            B.add_track(name, kind, data)
        failed = p['failed']
        if not failed:
            B.release_escape([name])
        log.append(f'{name:16} pads={len(B.pads[name]):2} {"OK" if not failed else "FAILED"} ({time.time() - t:.1f}s)')
        print(log[-1], flush=True)
        live_result(name, not failed)
        live()
        if failed:
            B.rip({name})
            plane_hits = []
            hard = {n for n in B.pair_of} | {n for n, c in ripped_count.items() if c >= RIP_LIMIT}
            if name not in protect:
                hard |= set(protect)
            found = find_blockers(B, name, width_of(name), plane_hits, hard=hard - {name})
            blockers = found - set(queue) - set(PLANES)
            blockers = {b for b in blockers if ripped_count.get(b, 0) < RIP_LIMIT and b not in B.pair_of}
            if name not in protect:
                blockers -= set(protect)
            mate = mate_of(name)
            if not blockers and mate in names and mate not in queue and ripped_count.get(mate, 0) < RIP_LIMIT:
                blockers = {mate}         # walled in by its own partner (it took the inner track): swap the order
            if not blockers and plane_hits and lifted.get(name, 0) < 2:
                # only plane fan-out copper (placed first) is in the way: lift it, those pads get a new via at the end
                lifted[name] = lifted.get(name, 0) + 1
                n_up = B.lift_plane(plane_hits, CLR + max(max(RAIL_LW.get(name, (width_of(name),))) / 2, VIA_D / 2) + G)
                log.append(f'  lifted {n_up} plane fan-out items for {name}')
                print(log[-1], flush=True)
                queue = [name] + queue
                continue
            # walled in by the fan-out reservations of nets still waiting: route those first (their real copper can be
            # ripped up, a reservation cannot), then this one
            waiting = {b for b in found & set(queue) if b in B.escape_on} - set(PLANES)
            if not blockers and waiting and walled.get(name, 0) < 2:
                walled[name] = walled.get(name, 0) + 1
                queue = [q for q in queue if q in waiting] + [name] + [q for q in queue if q not in waiting]
                log.append(f'  {name} walled in by the reservations of {sorted(waiting)}: routing those first')
                print(log[-1], flush=True)
                continue
            if not blockers:
                why = sorted(found - set(PLANES)) or sorted(find_blockers(B, name, width_of(name)) - set(PLANES))
                log.append(f'  no rippable blockers for {name}; leaving it partly unrouted'
                           + (f' (in the way: {", ".join(why)})' if why else ' (no way through at all)'))
                print(log[-1], flush=True)
                if LIVE is not None and name not in LIVE.unrouted:
                    LIVE.unrouted.append(name)
                route_net(B, name, width_of(name), log)
                continue
            log.append(f'  rip-up {sorted(blockers)} for {name}')
            print(log[-1], flush=True)
            for b in blockers:
                ripped_count[b] = ripped_count.get(b, 0) + 1
            B.rip(blockers)
            queue = [name] + sorted(blockers, key=lambda n: (first.index(n) if n in first else 99)) + queue
    if pool is not None:
        pool.shutdown(wait=True, cancel_futures=True)
        log.append(f'  parallel planning: {n_used} plans committed as planned, {n_redo} re-planned after a conflict')
        print(log[-1], flush=True)


# ── cleanup: trim dangling track ends ────────────────────────────────────────
def _on_seg(px, py, d, tol=0.01):
    x1, y1, x2, y2 = d[0], d[1], d[2], d[3]
    dx, dy = x2 - x1, y2 - y1
    L2 = dx * dx + dy * dy
    tt = 0 if L2 == 0 else max(0.0, min(1.0, ((px - x1) * dx + (py - y1) * dy) / L2))
    return math.hypot(px - (x1 + tt * dx), py - (y1 + tt * dy)) <= tol, tt


def _end_connected(B, idx, name, L, px, py, shapes, by_net):
    for sh in shapes:
        if L in sh.layers and float(sh.dist(np.array([px]), np.array([py]))[0]) <= 1e-6:
            return True
    for j in by_net.get(name, ()):
        n2, k2, d2 = B.tracks[j]
        if j == idx or k2 == 'gone':
            continue
        if k2 == 'via':
            if math.hypot(d2[0] - px, d2[1] - py) <= 0.01:
                return True
        elif int(d2[5]) == L and _on_seg(px, py, d2)[0]:
            return True
    return False


def cleanup_dangling_vias(B, log):
    """Drop a via that no copper of its own net touches - or connect it, when a same-net pad is within reach.

    ``cleanup_dangling`` only trims *segments*, so a via whose fan-out stub was never laid, or whose plane does not
    reach it, stays behind as isolated copper.  Measured before this: a +3V3 via beside C501.1 at (158.95, 130.85),
    1.28 mm from the pad, left the +3V3 net in two pieces and was the board's single unconnected item in the parity
    DRC.  A plane net's via is meant to be connected by the plane, so when a same-net pad is close the short stub from
    that pad is laid for it (if that stub is legal); a via that cannot be connected at all is removed.
    """
    plane_pads = {n: [s for _, sh in B.pads[n] for s in sh] for n in PLANES if n in B.pads}
    gone = stubbed = 0
    for it in [t for t in B.tracks if t[1] == 'via']:
        name, _kind, d = it
        x, y = d[0], d[1]
        # a via standing in a pad of its own net is connected by that pad: the QFNs' exposed-pad GND arrays.  They used
        # to count as dangling (only segments were looked at), and each got a W_GND "stub" to the first GND pad in
        # reach - eight zero-length dots on the exposed pad, and from the corner via a 0.3 mm track onto the 0.25 mm
        # pin 11, 0.1346 mm from the unconnected pin 12 (the DRC's clearance error on all three hubs)
        touched = any(sh.dist(np.array([x]), np.array([y]))[0] < VIA_D / 2
                      for _, shapes in B.pads.get(name, []) for sh in shapes[:1])
        for n2, k2, d2 in B.tracks:
            if touched:
                break
            if n2 != name or k2 != 'seg':
                continue
            if _on_seg(x, y, d2, tol=VIA_D / 2 + CLR)[0] or math.hypot(x - d2[0], y - d2[1]) <= VIA_D:
                touched = True
                break
        if not touched and name in plane_pads:
            reach = VIA_D / 2 + 1.6
            for sh in plane_pads[name]:
                bx0, by0, bx1, by1 = sh.bbox()
                if not (bx0 - reach <= x <= bx1 + reach and by0 - reach <= y <= by1 + reach):
                    continue
                px = min(max(x, bx0), bx1)
                py = min(max(y, by0), by1)
                w = W_GND if min(bx1 - bx0, by1 - by0) >= W_GND else W_SIG     # never wider than the pad it joins
                cand = (name, 'seg', (x, y, round(px, 4), round(py, 4), w, int(sh.layers[0])))
                if items_legal(B, name, [cand], strict=True):
                    B.add_track(name, 'seg', cand[2])
                    stubbed += 1
                    touched = True
                    break
        if not touched:
            B.tracks.remove(it)
            if it in B.fixed:
                B.fixed.remove(it)
            gone += 1
    if gone or stubbed:
        B.rebuild()
    log.append(f'dangling vias: {gone} dropped, {stubbed} connected by a stub to their own pad')


def cleanup_dangling(B, log):
    pad_sh = {n: [s for _, sh in B.pads[n] for s in sh] for n in B.pads}
    changed, trimmed = True, 0
    nets = set()
    while changed:
        changed = False
        by_net = {}
        for j, (n2, k2, d2) in enumerate(B.tracks):
            by_net.setdefault(n2, []).append(j)
        for idx, (name, kind, d) in enumerate(B.tracks):
            # hand-placed (fixed) copper is trimmed too: this runs after all routing, and one end of a segment
            # that touches nothing else on its own net is dead copper whatever placed it (it used to be skipped,
            # which left the two over-run stubs the DRC reported)
            if kind != 'seg':
                continue
            L = int(d[5])
            for end in (0, 1):
                px, py = (d[0], d[1]) if end == 0 else (d[2], d[3])
                if _end_connected(B, idx, name, L, px, py, pad_sh.get(name, []), by_net):
                    continue
                best = None
                for j in by_net.get(name, ()):
                    n2, k2, d2 = B.tracks[j]
                    if j == idx or k2 == 'gone':
                        continue
                    pts = [(d2[0], d2[1])] if k2 == 'via' else ([(d2[0], d2[1]), (d2[2], d2[3])] if int(d2[5]) == L else [])
                    for qx, qy in pts:
                        on, tt = _on_seg(qx, qy, d)
                        if on:
                            frac = tt if end == 0 else 1 - tt
                            if frac > 1e-6 and (best is None or frac < best[0]):
                                best = (frac, qx, qy)
                if best is None:
                    B.tracks[idx] = (name, 'gone', d)
                else:
                    nd = (best[1], best[2], d[2], d[3], d[4], d[5]) if end == 0 else (d[0], d[1], best[1], best[2], d[4], d[5])
                    if math.hypot(nd[2] - nd[0], nd[3] - nd[1]) < 0.005:
                        B.tracks[idx] = (name, 'gone', d)      # nothing left of it: drop the segment
                    else:
                        B.tracks[idx] = (name, 'seg', nd)
                        d = nd
                trimmed += 1
                nets.add(name)
                changed = True
                break
        B.tracks = [x for x in B.tracks if x[1] != 'gone']
    B.fixed = [f for f in B.fixed if f in B.tracks]
    B.rebuild()
    log.append(f'dangling ends trimmed: {trimmed} {sorted(nets)}')


# ── plane nets: a via next to every pad, stitching ────────────────────────────
def in_plane_shadow(B, x, y, name):
    """True when (x, y) lies under the *body* of a USB pair's or an AT32 SPI line's F.Cu / B.Cu copper.

    In1.Cu is the GND plane and the return path of every pair and of the SPI bus, so a via at such a spot punches its
    clearance hole through the reference that run is specified against.  A hole at a run's *end* (a fan-out, a
    terminal, a layer change) is fine - that is what the ends are for - but a row of them along the body is not, and
    that is what was measured before this: 5.30 mm of H0_KB's F.Cu copper had no In1.Cu under it (then H2_KB 1.64 mm,
    RP 1.39 mm).
    """
    own = B.pair_of.get(name)
    for n2, k2, d2 in B.tracks:
        if k2 != 'seg' or n2 == name or n2 == own:
            continue
        if n2 not in B.pair_of and not re.match(r'AT[12]_(MISO|MOSI|SCK|CS)$', n2):
            continue
        on, t = _on_seg(x, y, d2, tol=CLR + VIA_D / 2)
        if on and 0.15 < t < 0.85:
            return True
    return False


def legal_via(B, x, y, name, ignore_smd=False, static_only=False, qfn_ok=False):
    i, j = gi(x, X0), gi(y, Y0)
    if not (0 <= i < NX and 0 <= j < NY):
        return False
    net = B.code(name)
    vthr = CLR + VIA_D / 2 + EPS
    why = ''
    for L in range(NL):
        fd = B.static.d1[L, j, i] if B.static.n1[L, j, i] != net else B.static.d2[L, j, i]
        if not static_only:
            fd = min(fd, B.dyn.d1[L, j, i] if B.dyn.n1[L, j, i] != net else B.dyn.d2[L, j, i])
        if fd < vthr:
            why = f'L{L} copper {fd:.2f}'
            break
    if not why and (B.edge[j, i] < EDGE_CLR + VIA_D / 2
                    or (B.keep_via[j, i] and not (qfn_ok and B.qfn_inside[j, i]))):
        why = f'edge {B.edge[j, i]:.2f}' + (' keep_via' if B.keep_via[j, i] else '')
    if not why and not ignore_smd and ((B.noroute[0, j, i] or B.noroute[1, j, i])
                              and not (qfn_ok and B.qfn_inside[j, i])):
        why = 'noroute'
    if not why and (B.drill_static.d if static_only else B.drill.d)[j, i] < HOLE_CLR + VIA_DRILL / 2:
        why = 'drill'
    if not why and not ignore_smd and B.smd.d[j, i] < VIA_D / 2 + 0.05:
        why = f'smd {B.smd.d[j, i]:.2f}'
    if not why and xtal_guarded(name) and B.xtal_d[:, j, i].min() < XTAL_GUARD + VIA_D / 2:
        why = 'crystal guard'
    if not why and in_plane_shadow(B, x, y, name):
        why = 'under a pair/SPI run (its In1.Cu reference)'
    if not why and name in PLANES:
        # Not on or beside a same-net through-hole pad: on it the via adds nothing (the pad already reaches every layer),
        # and beside it the via sits in the pad's thermal relief, where the pours keep THERMAL_GAP and reach it only
        # if a ZONE_MIN_W strip squeezes past.  Measured on the rev-9 re-routes: the USB-C GND vias at (+-3.95, 3.75) /
        # (+-4.5, 3.85) from each connector sit on its shield pads, and the DRC called them "via_dangling" - connected
        # on one layer only - at J301 and J101.  plane_fanout() joins a pad to such a through-hole pad instead.
        for sh in B.tht_pads.get(name, ()):
            bx0, by0, bx1, by1 = sh.bbox()
            if bx0 - 1.0 <= x <= bx1 + 1.0 and by0 - 1.0 <= y <= by1 + 1.0:
                d = float(sh.dist(np.array([x]), np.array([y]))[0]) - VIA_D / 2
                if d < THERMAL_GAP + ZONE_MIN_W:
                    why = f'on / in the thermal relief of a same-net through-hole pad ({d:.2f} mm)'
                    break
    if why and VIA_DEBUG:
        print(f'      legal_via({x:.2f},{y:.2f}) no: {why}', flush=True)
    return not why


def plane_open_pads(B, name):
    """SMD pads of a plane net whose copper (pad + tracks) reaches no via."""
    items = [(k, d) for nm, k, d in B.tracks if nm == name]
    cells = [track_cells([it]) for it in items]
    parent = list(range(len(items)))

    def root(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a
    owner = {}
    for k, cs in enumerate(cells):
        for c in cs:
            if c in owner:
                ra, rb = root(owner[c]), root(k)
                if ra != rb:
                    parent[ra] = rb
            else:
                owner[c] = k
    has_via = {root(k) for k, (kind, _) in enumerate(items) if kind == 'via'}
    out = []
    for ref, shapes in B.pads[name]:
        if (ref.split('.')[0], ref.split('.')[1]) in EP_PADS or len(shapes[0].layers) > 1:
            continue
        if name == 'GND' and ref.split('.')[0] in POUR_PADS:
            continue
        roots = {root(owner[c]) for c in pad_cells(shapes) if c in owner}
        if not roots & has_via:
            out.append(ref)
    return out


SHARE_GND_DIST = 1.45          # two GND pads closer than this (centres) may share one via (see share_gnd_vias)


def share_gnd_vias(B, log, max_dist=SHARE_GND_DIST):
    """One via for two GND pads that sit close together, placed on the side away from their signal pads.

    Two 2-pad parts that meet GND pad to GND pad - R304/C514 east of the RP2354B are 1.29 mm apart - otherwise get a via
    each from plane_fanout, and the cheaper one lands wherever it likes: measured on rev 9 it went to (157.40,136.00),
    straight into the corridor VSNS_PWR needs to leave R304.1 northwards for R303.2, and VSNS_PWR then failed there
    ("no way through at all").  One shared via, scored to sit as far as the pair allows from the two parts' signal pads,
    leaves that corridor open.  The joined pads then count as connected, and plane_fanout gives no via to a pad whose
    copper already reaches one.
    """
    pads = []
    for ref, shapes in B.pads.get('GND', []):
        part = ref.split('.')[0]
        fp = B.board.FindFootprintByReference(part)
        if fp is None or len(list(fp.Pads())) != 2 or len(shapes[0].layers) > 1:
            continue                         # only 2-pad SMD parts: a GND pin of an IC has its own escape
        x0, y0, x1, y1 = shapes[0].bbox()
        pads.append((ref, part, (x0 + x1) / 2, (y0 + y1) / 2))
    vias = [(d[0], d[1]) for nm, k, d in B.tracks if k == 'via']
    used, n = set(), 0
    for i in range(len(pads)):
        for j in range(i + 1, len(pads)):
            a, b = pads[i], pads[j]
            if a[1] == b[1] or a[0] in used or b[0] in used:
                continue
            d = math.hypot(a[2] - b[2], a[3] - b[3])
            if d > max_dist:
                continue
            sig = []                          # the parts' signal pads: the shared via keeps away from them
            for part in (a[1], b[1]):
                for p in B.board.FindFootprintByReference(part).Pads():
                    if p.GetNetname() != 'GND':
                        q = p.GetPosition()
                        sig.append((MM(q.x), MM(q.y)))
            mx, my = (a[2] + b[2]) / 2, (a[3] + b[3]) / 2
            best, best_score = None, -1e9
            for dj in range(-8, 9):
                for di in range(-8, 9):
                    x, y = round((mx + di * G) / G) * G, round((my + dj * G) / G) * G
                    if math.hypot(x - a[2], y - a[3]) > 0.9 or math.hypot(x - b[2], y - b[3]) > 0.9:
                        continue              # both pads reach it with a short stub
                    if not legal_via(B, x, y, 'GND') or any(math.hypot(x - vx, y - vy) < FANOUT_VIA_GAP
                                                            for vx, vy in vias):
                        continue
                    stubs = [('seg', (a[2], a[3], x, y, W_SIG, 0)),
                             ('seg', (b[2], b[3], x, y, W_SIG, 0))]
                    if not items_legal(B, 'GND', [('GND', k, d) for k, d in stubs], strict=True):
                        continue              # a stub may not cross the parts' signal pads
                    score = min(math.hypot(x - sx, y - sy) for sx, sy in sig) \
                        - 2.0 * (math.hypot(x - a[2], y - a[3]) + math.hypot(x - b[2], y - b[3]))
                    if score > best_score:
                        best, best_score = (x, y, stubs), score
            if best is None:
                continue
            x, y, stubs = best
            for kind, data in stubs:
                B.add_track('GND', kind, data, fixed=True)
            B.add_track('GND', 'via', (x, y), fixed=True)
            vias.append((x, y))
            used |= {a[0], b[0]}
            n += 1
            log.append(f'shared GND via ({x:.2f},{y:.2f}) for {a[0]} + {b[0]} ({d:.2f} mm apart)')
    log.append(f'shared GND vias: {n}')
    return n


def plane_fanout(B, name, log, only=None):
    n_added = 0
    open_pads = []
    width = W_GND
    seen = set()
    for ref, shapes in B.pads[name]:
        if only is not None and ref not in only:
            continue
        if (ref.split('.')[0], ref.split('.')[1]) in EP_PADS:
            continue
        if name == 'GND' and ref.split('.')[0] in POUR_PADS:
            continue                                  # joins the pour on its own layer (POUR_PADS)
        if len(shapes[0].layers) > 1:                 # through-hole pad already reaches every layer
            continue
        L = shapes[0].layers[0]                       # SMD pad on F.Cu or B.Cu
        src = pad_cells(shapes, layers=(L,))
        x0, y0, x1, y1 = shapes[0].bbox()
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        # one physical pad with two numbers (the USB-C's A12/B1 GND) is fanned out once: the second pass used to lay
        # the same stub again on top of the first (doubled copper at J301, (156.05-156.45, 107.4))
        if (L, round(cx, 3), round(cy, 3)) in seen:
            continue
        seen.add((L, round(cx, 3), round(cy, 3)))
        existing = set()
        for nm, kind, data in B.tracks:
            if nm == name and kind == 'via' and math.hypot(data[0] - cx, data[1] - cy) <= 1.2:
                existing.add((L, gi(data[1], Y0), gi(data[0], X0)))
        # a same-net through-hole pad close by is as good as a via - it reaches every layer (the USB-C's GND pins
        # beside its shield pads); legal_via() keeps vias off and beside such pads, so this is the way to it
        for sh in B.tht_pads.get(name, ()):
            if float(sh.dist(np.array([cx]), np.array([cy]))[0]) <= 1.2:
                existing |= pad_cells([sh], layers=(L,))
        for reach in (2.5, 4.5):        # beyond 2.5 mm only where an escape zone keeps vias off a dense pin
            spots = {}
            n = int(reach / G)
            for dj in range(-n, n + 1, 2):
                for di in range(-n, n + 1, 2):
                    x, y = round((cx + di * G) / G) * G, round((cy + dj * G) / G) * G
                    if math.hypot(x - cx, y - cy) <= reach and legal_via(B, x, y, name, qfn_ok=(L == 1)):
                        spots[(L, gi(y, Y0), gi(x, X0))] = (x, y)
            if spots or existing:
                break
        targets = set(spots) | existing
        if not targets:
            open_pads.append((ref, shapes))
            continue
        path = None
        # never wider than the pad it leaves: the search's clearance test does not look at the pad's own cells, so a
        # 0.3 mm stub off a 0.25 mm QFN pin passed it - and each hub's GND pin 11 got one, 0.1 mm off the pin axis and
        # 0.1346 mm from the unconnected pin 12 (DRC clearance error, U701/U801/U901, rev 9 and earlier)
        for w in ((width, W_SIG) if min(x1 - x0, y1 - y0) >= width else (W_SIG,)):
            R = NetRouter(B, name, w, (cx - reach - 1.0, cy - reach - 1.0, cx + reach + 1.0, cy + reach + 1.0), layers=(L,))
            path = R.search(src, targets, None, max_exp=300000)
            if path:
                break
        if not path:
            open_pads.append((ref, shapes))
            continue
        for kind, data in path_to_tracks(path, w):
            if kind == 'seg' and math.hypot(data[2] - data[0], data[3] - data[1]) < 1e-6:
                continue                              # a pad already on its target: no zero-length "dot" track
            B.add_track(name, kind, data)
        end = path[-1]
        if end in spots and end not in existing and legal_via(B, *spots[end], name, qfn_ok=(L == 1)):
            B.add_track(name, 'via', spots[end])
            n_added += 1
    log.append(f'{name} fanout vias: {n_added}, pads without a via: {len(open_pads)}')
    for ref, _ in open_pads:
        log.append(f'  {name} {ref}: no via')
    return open_pads


def plane_fix(B, name, open_pads, log):
    """Plane pads walled in by signals: rip the signals in the way, add the via, reroute them."""
    still = []
    for ref, shapes in open_pads:
        L = shapes[0].layers[0]
        src = pad_cells(shapes, layers=(L,))
        x0, y0, x1, y1 = shapes[0].bbox()
        cx, cy = (x0 + x1) / 2, (y0 + y1) / 2
        spots = {}
        for dj in range(-40, 41, 2):
            for di in range(-40, 41, 2):
                x, y = round((cx + di * G) / G) * G, round((cy + dj * G) / G) * G
                if math.hypot(x - cx, y - cy) <= 2.0 and legal_via(B, x, y, name, static_only=True, qfn_ok=(L == 1)):
                    spots[(L, gi(y, Y0), gi(x, X0))] = (x, y)
        if not spots:
            log.append(f'  {name} {ref}: no via spot even after rip-up')
            still.append(ref)
            continue
        R = NetRouter(B, name, W_SIG, (cx - 3.0, cy - 3.0, cx + 3.0, cy + 3.0), soft=True, layers=(L,))
        path = R.search(src, set(spots), None, max_exp=400000)
        if not path:
            log.append(f'  {name} {ref}: unreachable')
            still.append(ref)
            continue
        vx, vy = spots[path[-1]]
        own = B.code(name)
        blockers = set()
        checks = [(L, j, i, CLR + W_SIG / 2) for (L, j, i) in path] + \
                 [(L, gi(vy, Y0), gi(vx, X0), CLR + VIA_D / 2) for L in range(NL)]
        for L, j, i, need in checks:
            n = B.dyn.n1[L, j, i]
            if n not in (own, -9) and B.dyn.d1[L, j, i] < need + EPS:
                blockers |= {nm for nm, c in B.netcode.items() if c == n}
        blockers.discard(name)
        if blockers & (set(B.pair_of) | set(PLANES)):
            log.append(f'  {name} {ref}: blocked by a pair/plane net {sorted(blockers)}')
            still.append(ref)
            continue
        B.rip(blockers)
        Rg = NetRouter(B, name, W_SIG, (cx - 3.0, cy - 3.0, cx + 3.0, cy + 3.0), layers=(L,))
        if not legal_via(B, vx, vy, name) or not all(Rg.flat[L][(j - Rg.j0) * Rg.nx + (i - Rg.i0)] for (L, j, i) in path):
            log.append(f'  {name} {ref}: spot still blocked after rip-up')
            still.append(ref)
        else:
            for kind, data in path_to_tracks(path, W_SIG):
                B.add_track(name, kind, data)
            B.add_track(name, 'via', (vx, vy))
            log.append(f'  {name} {ref}: ripped {sorted(blockers)} for its via')
        for nm in sorted(blockers):
            failed = route_net(B, nm, width_of(nm), log)
            if failed:
                log.append(f'  reroute {nm}: FAILED')
            else:
                B.release_escape([nm])
    return still


def in2_islands(B, log, clearance=0.2, min_w=0.2):
    """In2.Cu tracks must not cut the +3V3 plane into pieces: report +3V3 vias / pins that land outside the
    plane's main region (KiCad removes islands, which would leave them unconnected)."""
    fd = B.foreign('+3V3')[2]
    free = (fd >= clearance + min_w / 2) & (B.edge >= 0.5 + min_w / 2)
    lab, n = ndimage.label(free)
    if n == 0:
        return []
    sizes = np.bincount(lab.ravel())
    sizes[0] = 0
    main = int(np.argmax(sizes))
    pts = [(d[0], d[1], 'via') for nm, k, d in B.tracks if nm == '+3V3' and k == 'via']
    for ref, shapes in B.pads['+3V3']:
        if 2 in shapes[0].layers:
            x0, y0, x1, y1 = shapes[0].bbox()
            pts.append(((x0 + x1) / 2, (y0 + y1) / 2, ref))
    boxes = ndimage.find_objects(lab)
    cut = []
    for x, y, what in pts:
        k = lab[gi(y, Y0), gi(x, X0)]
        if k != main:
            sj, si = boxes[k - 1] if k else (slice(gi(y, Y0), gi(y, Y0) + 1), slice(gi(x, X0), gi(x, X0) + 1))
            cut.append((x, y, what, (X0 + si.start * G, Y0 + sj.start * G, X0 + si.stop * G, Y0 + sj.stop * G)))
    n_in2 = sum(1 for nm, k, d in B.tracks if k == 'seg' and int(d[5]) == 2)
    log.append(f'In2.Cu: {n_in2} track segments, +3V3 plane regions {n}, connections off the main region: {len(cut)}')
    for x, y, what, _ in cut:
        log.append(f'  +3V3 off the In2 plane: {what} at ({x:.2f}, {y:.2f})')
    return cut


def in2_repair(B, log, rounds=3):
    """+3V3 connections cut off the In2 plane: the In2 tracks around that island are routed again with In2 closed
    there (the nets keep F.Cu / B.Cu)."""
    for _ in range(rounds):
        cut = in2_islands(B, log)
        if not cut:
            return
        nets = set()
        for x, y, what, (bx0, by0, bx1, by1) in cut:
            bx0, by0, bx1, by1 = bx0 - 1.0, by0 - 1.0, bx1 + 1.0, by1 + 1.0
            B.keep_trk[2, max(gi(by0, Y0), 0):gi(by1, Y0) + 1, max(gi(bx0, X0), 0):gi(bx1, X0) + 1] = True
            for nm, k, d in B.tracks:
                if k == 'seg' and int(d[5]) == 2 and min(d[0], d[2]) <= bx1 and max(d[0], d[2]) >= bx0 \
                        and min(d[1], d[3]) <= by1 and max(d[1], d[3]) >= by0:
                    nets.add(nm)
        if not nets:
            return
        log.append(f'  In2 repair: re-routing {sorted(nets)} off the island(s)')
        print(log[-1], flush=True)
        B.rip(nets)
        route_all(B, sorted(nets), log, first=sorted(nets))      # with rip-up: a net must not end up unrouted


def stitching(B, log, pitch=3.0):
    n = 0
    fg = B.foreign('GND')
    y = Y0 + 1.5
    while y < Y1 - 1.0:
        x = X0 + 1.5
        while x < X1 - 1.0:
            gx, gy = round(x / G) * G, round(y / G) * G
            if legal_via(B, gx, gy, 'GND') and min(fg[L, gi(gy, Y0), gi(gx, X0)] for L in range(NL)) > 0.65:
                B.add_track('GND', 'via', (gx, gy))
                n += 1
            x += pitch
        y += pitch
    log.append(f'stitching vias: {n}')


# ── write back to KiCad ───────────────────────────────────────────────────────
def seed_copper(B, board, path, log):
    """--seed=BOARD: keep a routed board's copper (see main()).  Every track and via of the seed whose net still has the
    same pads goes in as fixed copper, unless a rule area of this board forbids it there (a via in a no-via keepout);
    the stage-1 copper of those nets is dropped for it.  Returns the nets left to route: every net with a pad whose net
    differs from the seed's, and their pair mates.  The placement must be the seed's - copper does not fit moved parts.
    Parts the seed does not have (0.16.0: the Bluetooth module and its passives) are added: seed copper that touches
    their pads goes, and so does its net's (it is re-routed).  A plane net keeps its copper; its new pads get their via
    in main().  ``--reroute=NET,NET`` re-routes more nets, to clear a new net's way out of a dense pin row."""
    seed = pcbnew.LoadBoard(path)
    moved = []
    for f in seed.GetFootprints():
        if not f.GetPadCount():
            continue          # a logo: no copper the routing depends on, and its no-via area is a rule area (below)
        g = board.FindFootprintByReference(f.GetReference())
        if (g is None or g.GetPosition() != f.GetPosition() or g.GetOrientationDegrees() != f.GetOrientationDegrees()
                or g.GetLayer() != f.GetLayer()):
            moved.append(f.GetReference())
    if moved:
        raise SystemExit(f'--seed: {len(moved)} footprint(s) placed differently from {path} ({", ".join(moved[:8])}): '
                         f'its copper does not fit this board - route it from scratch')

    def pad_nets(b):
        return {(f.GetReference(), p.GetNumber()): p.GetNetname() for f in b.GetFootprints() for p in f.Pads()}
    old, new = pad_nets(seed), pad_nets(board)
    changed = {n for k in set(old) | set(new) if old.get(k) != new.get(k) for n in (old.get(k), new.get(k)) if n}
    # A plane's copper stays (its vias are the seed's, and the planes are refilled anyway); only its new pads need a via
    B.seed_plane_pads = {pn: {f'{r}.{num}' for (r, num), nm in new.items() if nm == pn and old.get((r, num)) != pn}
                         for pn in PLANES}
    changed = {n for n in changed if n not in PLANES and not n.startswith('unconnected-')}
    extra = next((a.split('=', 1)[1] for a in sys.argv if a.startswith('--reroute=')), '')
    missing = [n for n in extra.split(',') if n and n not in B.netcode]
    if missing:
        raise SystemExit(f'--reroute: no such net(s) {missing}')
    reroute = set(changed) | {n for n in extra.split(',') if n}
    lidx = {lyr: k for k, lyr in enumerate(LAYERS)}
    # the new parts' pads: seed copper must keep CLR from them (and must not run through them)
    new_refs = {f.GetReference() for f in board.GetFootprints()} - {f.GetReference() for f in seed.GetFootprints()}
    new_pads = [(p, lyr) for f in board.GetFootprints() if f.GetReference() in new_refs for p in f.Pads()
                for lyr in LAYERS if p.IsOnLayer(lyr)]
    clr = pcbnew.FromMM(CLR)

    def collides(t):
        bb = t.GetBoundingBox()
        for p, lyr in new_pads:
            if t.GetClass() != 'PCB_VIA' and t.GetLayer() != lyr:
                continue
            pb = p.GetBoundingBox()
            if (bb.GetRight() + clr < pb.GetLeft() or pb.GetRight() + clr < bb.GetLeft()
                    or bb.GetBottom() + clr < pb.GetTop() or pb.GetBottom() + clr < bb.GetTop()):
                continue
            if p.GetEffectiveShape(lyr).Collide(t.GetEffectiveShape(lyr), clr):
                return True
        return False

    # The seed's copper met the seed's rules, so only a rule area this board has and the seed has not can forbid it
    # (the 0.13 badge's no-via area).  The router's own keep-out grids are not the test: they also cover places where
    # its fan-out may put a via all the same (inside a QFN), and the seed has exactly those vias.
    def rule_areas(b):
        zs = list(b.Zones()) + [z for f in b.GetFootprints() for z in f.Zones()]
        out = {}
        for z in zs:
            if z.GetIsRuleArea():
                o = z.Outline().Outline(0)
                out[tuple((o.CPoint(k).x, o.CPoint(k).y) for k in range(o.PointCount()))] = z
        return out
    seed_rules = rule_areas(seed)
    new_rules = [z for k, z in rule_areas(board).items() if k not in seed_rules]

    def forbidden(t):
        for z in new_rules:
            ol = z.Outline()
            if t.GetClass() == 'PCB_VIA':
                if z.GetDoNotAllowVias() and ol.Contains(t.GetPosition()):
                    return True
            elif z.GetDoNotAllowTracks() and z.IsOnLayer(t.GetLayer()):
                s, e = t.GetStart(), t.GetEnd()
                n = max(1, int(math.hypot(e.x - s.x, e.y - s.y) / pcbnew.FromMM(G)))
                if any(ol.Contains(pcbnew.VECTOR2I(int(s.x + (e.x - s.x) * k / n), int(s.y + (e.y - s.y) * k / n)))
                       for k in range(n + 1)):
                    return True
        return False
    # a signal whose seed copper this board forbids or a new part's pad touches is re-routed (a plane net only loses
    # that via or track: its pads reach the plane through their own vias)
    hit = {t.GetNetname() for t in seed.GetTracks()
           if t.GetNetname() not in PLANES and t.GetNetname() not in reroute and (forbidden(t) or collides(t))}
    if hit:
        log.append(f'seed: re-routing {sorted(hit)}: their copper touches a new part or a new rule area')
        print(log[-1], flush=True)
    reroute |= hit
    for n in list(reroute):
        reroute |= set(B.pair_of.get(n, ()))
    keep, dropped, rerouted = [], [], 0
    for t in seed.GetTracks():
        name = t.GetNetname()
        if name in reroute:
            rerouted += 1
            continue
        if t.GetClass() == 'PCB_VIA':
            item = (name, 'via', (MM(t.GetPosition().x), MM(t.GetPosition().y)))
        else:
            if t.GetLayer() not in lidx:
                raise SystemExit(f'--seed: {name} has copper on {seed.GetLayerName(t.GetLayer())}, not a routing layer')
            item = (name, 'seg', (MM(t.GetStart().x), MM(t.GetStart().y), MM(t.GetEnd().x), MM(t.GetEnd().y),
                                  MM(t.GetWidth()), lidx[t.GetLayer()]))
        (dropped if name not in B.netcode or forbidden(t) or collides(t) else keep).append(item)
    B.tracks = [t for t in B.tracks if t[0] in reroute or t[0].startswith('__')] + keep
    B.fixed = [t for t in B.fixed if t[0] in reroute or t[0].startswith('__')] + keep
    B.release_escape([n for n in list(B.escape_on) if n not in reroute])
    B.rebuild()
    # A pair whose two lines only changed places at one part keeps its seed copper and gets a twist instead.
    for Pn, Nn in sorted({B.pair_of[n] for n in changed if n in B.pair_of}):
        ends = {k for k in set(old) | set(new) if old.get(k) != new.get(k) and {old.get(k), new.get(k)} & {Pn, Nn}}
        if len({k[0] for k in ends}) == 1 and all({old.get(k), new.get(k)} == {Pn, Nn} for k in ends):
            ok, cut = seed_twist(B, seed, Pn, Nn, next(iter(ends))[0], log)
            if ok:
                reroute = (reroute - {Pn, Nn}) | cut
                rerouted -= sum(1 for t in seed.GetTracks() if t.GetNetname() in (Pn, Nn))
            print(log[-1], flush=True)
    log.append(f'seed {os.path.basename(path)}: {len(keep)} items kept as fixed copper, {rerouted} of the re-routed '
               f'nets dropped, {len(dropped)} dropped for this board\'s rules '
               f'({", ".join(sorted({d[0] for d in dropped})) or "none"}); re-routing {sorted(reroute) or "nothing"}')
    return reroute


def line_pieces(items):
    """Connected pieces of one line's copper, [(kind, d)]: items join where an end of one lies on the other (segments on
    different layers meet only through a via)."""
    def joined(i1, i2):
        if i1[0] == 'seg' and i2[0] == 'seg' and i1[1][5] != i2[1][5]:
            return False
        return any(_on_item(x, y, i2) for x, y in _ends(i1)) or any(_on_item(x, y, i1) for x, y in _ends(i2))
    groups = []
    for it in items:
        hit = [g for g in groups if any(joined(it, o) for o in g)]
        merged = [it] + [o for g in hit for o in g]
        groups = [g for g in groups if g not in hit] + [merged]
    return groups


def cut_run(seg, t0, t1, a0, d):
    """seg (a straight run along d) without its part between t0 and t1 (distances along d from a0)."""
    x1, y1, x2, y2, w, L = seg
    p = lambda t: (a0[0] + d[0] * t, a0[1] + d[1] * t)
    s1 = (x1 - a0[0]) * d[0] + (y1 - a0[1]) * d[1]
    s2 = (x2 - a0[0]) * d[0] + (y2 - a0[1]) * d[1]
    lo, hi = (s1, s2) if s1 <= s2 else (s2, s1)
    out = []
    if t0 - lo > 1e-6:
        out.append(('seg', (*p(lo), *p(t0), w, L)))
    if hi - t1 > 1e-6:
        out.append(('seg', (*p(t1), *p(hi), w, L)))
    return out


def zone_self_clear(zone, P, N):
    """A twist's crossing line keeps clear of the other line's two vias.  P and N share a net code (Board.__init__), so
    items_legal() does not check them against each other."""
    vias = [z[2] for z in zone if z[0] == N and z[1] == 'via']
    need = VIA_D / 2 + PAIR_W / 2 + CLR
    return all(seg_point_dist(vx, vy, *z[2][:4]) >= need - 1e-6
               for z in zone if z[0] == P and z[1] == 'seg' for vx, vy in vias)


def twist_sites(P, N, lines, at_end, at_other, Lt=1.6, margin=0.2, outs=(0.65, 0.55, 0.45), layers=(0,)):
    """Every place a twist can go in a pair whose two lines reach one end on each other's pads, and what each costs in
    skew.  Used on a seeded board (seed_twist) and on a link just routed as if it needed no crossover (twist_in_run).

    lines: {P: [(kind, d)], N: [...]}, the pair's copper.  at_end(piece) is True for a piece of one line that reaches the
    end where the lines arrive swapped; at_other(piece) for one that reaches anything else the line serves (its other
    end, another part on the way).  Between the twist and that end each line takes the other net, so a site whose stretch
    would also reach something else is refused: a line through the service ESD (U301) would hand the ESD's pads the
    wrong net, a short the DRC finds only afterwards.

    A site is a straight run where both lines are parallel at the pair's spacing on one of ``layers``, at least Lt + 2
    margin long.  swap_zone crosses P over on that layer while N hops on the other one.  Behind the twist the two lines
    trade their remaining lengths, corners included, so where it goes sets the pair's skew: each site's skew is worked
    out (each net's total track length, as check_usb_si.py measures it).  And every site is offered again with the
    crossing's S drawn taller or flatter than N's hop, which lengthens or shortens P alone - by -0.55 to +0.6 mm, from
    the pair's own offset to 0.2 mm beyond the hop in 0.05 mm steps (0.14.0: it brought the service pair's only site
    with room from 1.30 mm back inside the 1.2 mm budget, to 0.64 mm).

    Returns a namespace: sites; build(site), the pair's copper with that twist as [(net, kind, d)]; skew[id(site)], P
    minus N in mm; amp[id(site)], the S height; before, the skew without a twist; refused, sites whose stretch reached
    more than the end."""
    sites = []
    for ka, a in enumerate(lines[P]):
        if a[0] != 'seg' or a[1][5] not in layers:
            continue
        ax1, ay1, ax2, ay2 = a[1][:4]
        La = math.hypot(ax2 - ax1, ay2 - ay1)
        if La < Lt + 2 * margin:
            continue
        rest = [o for o in lines[P] if o is not a]
        # d points from the swapped end towards the unchanged one, along this run
        d = ((ax2 - ax1) / La, (ay2 - ay1) / La)
        a0 = (ax1, ay1)
        far = [g for g in line_pieces(rest) if any(_on_item(ax2, ay2, o) for o in g)]
        if far and at_end(far[0]):
            d, a0 = (-d[0], -d[1]), (ax2, ay2)
        nl = left_of(d)
        for kb, b in enumerate(lines[N]):
            if b[0] != 'seg' or b[1][5] != a[1][5]:
                continue
            bx1, by1, bx2, by2 = b[1][:4]
            Lb = math.hypot(bx2 - bx1, by2 - by1)
            if Lb < Lt or abs((bx2 - bx1) * d[1] - (by2 - by1) * d[0]) / Lb > 1e-3:
                continue                                       # not parallel
            off = (bx1 - a0[0]) * nl[0] + (by1 - a0[1]) * nl[1]
            if abs(abs(off) - 2 * PAIR_O) > 0.01:
                continue                                       # not this pair's spacing
            tb = sorted(((bx1 - a0[0]) * d[0] + (by1 - a0[1]) * d[1], (bx2 - a0[0]) * d[0] + (by2 - a0[1]) * d[1]))
            lo, hi = max(0.0, tb[0]) + margin, min(La, tb[1]) - margin - Lt
            side_b = 1 if off > 0 else -1
            for out_, t in [(o_, lo + k * 0.1) for o_ in outs for k in range(int((hi - lo) / 0.1 + 1e-9) + 1)]:
                centre = (a0[0] + d[0] * t + nl[0] * off / 2, a0[1] + d[1] * t + nl[1] * off / 2)
                # behind the twist each line is its own again, so P leaves on its own side (-side_b)
                zone, _ = swap_zone(P, N, centre, d, side_b, gap=0.0, Lt=Lt, out=out_, layer=a[1][5])
                sites.append((out_, t, centre, zone, a, ka, b, kb, a0, d, nl, off))

    def kept_pieces(site):
        out_, t, centre, zone, a, ka, b, kb, a0, d, nl, off = site
        res = []
        for net, line, cut in ((P, lines[P], a), (N, lines[N], b)):
            o = (net == N) * off
            base = (a0[0] + nl[0] * o, a0[1] + nl[1] * o)
            res.append((net, line_pieces([x for x in line if x is not cut] + cut_run(cut[1], t, t + Lt, base, d))))
        return res
    # Which stretch moves depends only on which two runs are cut, so it is worked out once per pair of runs.
    seen = {}

    def relabel_ok(site):
        key = (site[5], site[7], site[8], site[9])
        if key not in seen:
            seen[key] = not any(at_end(g) and at_other(g) for _, gs in kept_pieces(site) for g in gs)
        return seen[key]
    n0 = len(sites)
    sites = [site for site in sites if relabel_ok(site)]
    refused = n0 - len(sites)

    def build(site):
        out = []
        for net, gs in kept_pieces(site):
            other = N if net == P else P
            for g in gs:
                out += [(other if at_end(g) else net, k2, d2) for k2, d2 in g]
        return out + site[3]

    def seg_len(items, net):
        return sum(math.hypot(dd[2] - dd[0], dd[3] - dd[1]) for nm, k, dd in items if nm == net and k == 'seg')
    plain = [(n, k, dd) for n in (P, N) for k, dd in lines[n]]
    before = seg_len(plain, P) - seg_len(plain, N)
    skew, amp = {}, {}
    for site in sites:
        out = build(site)
        skew[id(site)] = seg_len(out, P) - seg_len(out, N)
        amp[id(site)] = site[0]

    def p_len(A):
        return 2 * math.hypot(0.3, A - PAIR_O) + math.hypot(Lt - 0.6, 2 * A)
    variants = []
    for site in sites:
        out_ = site[0]
        variants.append(site)
        for A in sorted({round(out_ + 0.05 * k, 3) for k in range(-12, 5)} - {round(out_, 3)}):
            if not PAIR_O - 1e-9 <= A <= out_ + 0.2 + 1e-9:
                continue
            zone, _ = swap_zone(P, N, site[2], site[9], 1 if site[11] > 0 else -1, gap=0.0, Lt=Lt, out=out_, p_out=A,
                                layer=site[4][1][5])
            v = (*site[:3], zone, *site[4:])
            skew[id(v)] = skew[id(site)] + p_len(A) - p_len(out_)
            amp[id(v)] = A
            variants.append(v)

    def rank(site):
        """Inside the skew budget first; then the least skew, to 0.1 mm (finer is not worth an odd shape); then the usual
        via distance, the S of the hop's own height, and the site nearest the run's start."""
        return (abs(skew[id(site)]) > PAIR_SKEW_MAX, round(abs(skew[id(site)]), 1), site[0] != outs[0],
                round(abs(amp[id(site)] - site[0]), 3), site[1])
    return types.SimpleNamespace(sites=variants, build=build, skew=skew, amp=amp, before=before, refused=refused,
                                 rank=rank)


def twist_legal(B, P, N, zone, fd=None):
    """A twist zone clears foreign copper, and its crossing line clears the other line's vias."""
    return (zone_self_clear(zone, P, N)
            and items_legal(B, P, [z for z in zone if z[0] == P], strict=True, fd=fd)
            and items_legal(B, N, [z for z in zone if z[0] == N], strict=True, fd=fd))


def twist_in_run(B, P, N, items, stubsA, stubsB, log, Lt=1.6):
    """route_pair(twist_at='run'): the link has been routed as if it needed no crossover, so its two lines reach the B
    end on each other's stubs.  Splice the twist into a straight stretch of the new lines that has room (twist_sites),
    as seed_twist does on a seeded board, and hand back the link's items with it - P's line again ending on P's stub.

    A twist at one end (swap_zone in front of a terminal) needs 1.6 mm of free copper right at the pins plus a lead-in,
    and neither the service pair nor the RP pair has that at either end (errata.md E1: since 0.14.0 both cross over in
    copper; measured in an unseeded run, 2026-11-06: "no pair path from (0, 272, 842) to (0, 364, 1200)" and "leg 3 ->
    (136.8, 114.0) blocked by ['RP_DM']", the zone in front of the switch sitting on the lane's own last waypoint).
    Inside the run the pair has a whole lane to choose from, and the pairs are routed before anything but the fixed
    copper, so a stretch with room for two vias 0.45-0.65 mm beside it is the rule, not the exception.
    Returns (items, note) or (None, why)."""
    lines = {n: [(k, d) for nm, k, d in items if nm == n] for n in (P, N)}
    endB = [('seg', d) for d in stubsB]
    endA = [('seg', d) for d in stubsA]

    def touches(g, stubs):
        return any(_on_item(x, y, s, tol=0.02) for k, d in g for x, y in _ends((k, d)) for s in stubs)
    ts = twist_sites(P, N, lines, lambda g: touches(g, endB), lambda g: touches(g, endA), Lt=Lt, layers=(0, 1))
    fd = B.foreign(P)
    best = next((s for s in sorted(ts.sites, key=ts.rank) if twist_legal(B, P, N, s[3], fd)), None)
    if best is None:
        return None, f'no straight stretch with room for a twist ({len(ts.sites)} sites, {ts.refused} refused)'
    out_, t, centre, zone, a = best[:5]
    if os.environ.get('ROUTE_TWIST_DEBUG') in (P, N, 'all'):    # debugging: the chosen site and the pair's new copper
        print(f'  TWIST {P}/{N}: {len(ts.sites)} sites, {ts.refused} refused; chosen out {out_} t {t:.2f} centre '
              f'{centre} along {best[9]} on L{a[1][5]}, S {ts.amp[id(best)]}', flush=True)
        for nm, k, d in ts.build(best):
            print(f'    {nm} {k} ' + ' '.join(f'{v:.3f}' for v in d), flush=True)
    note = (f'twist in the run at ({centre[0]:.2f}, {centre[1]:.2f}) on {"FB"[a[1][5]]}.Cu, vias {out_} mm out'
            + (f', crossing S {ts.amp[id(best)]:.2f} mm' if abs(ts.amp[id(best)] - out_) > 1e-6 else '')
            + f', skew {abs(ts.skew[id(best)]):.2f} mm (untwisted {abs(ts.before):.2f} mm)')
    return ts.build(best), note


def seed_twist(B, seed, P, N, ref, log, Lt=1.6, margin=0.2, outs=(0.65, 0.55, 0.45)):
    """--seed, a pair whose two lines changed places at one end (pads of ``ref``; errata.md E1: U601's pair pins): keep
    the seed's two lines and splice a twist (swap_zone: one line crosses on F.Cu, the other hops under it on B.Cu) into a
    straight stretch where the router's own clearance check passes (twist_sites: the least skew that fits).  Between that
    twist and ``ref`` each line carries the other net.  The pair then differs from the seed by the twist alone - a fresh
    lane would have to fit the corridor the seed's later nets left, which they packed at bare clearance round the old
    copper.  Returns (True if placed, the nets cut to make room)."""
    lines = {n: [] for n in (P, N)}
    for t in seed.GetTracks():
        n = t.GetNetname()
        if n in lines:
            if t.GetClass() == 'PCB_VIA':
                lines[n].append(('via', (MM(t.GetPosition().x), MM(t.GetPosition().y))))
            elif t.GetLayer() in LAYERS:
                lines[n].append(('seg', (MM(t.GetStart().x), MM(t.GetStart().y), MM(t.GetEnd().x), MM(t.GetEnd().y),
                                         MM(t.GetWidth()), LAYERS.index(t.GetLayer()))))
    fp = seed.FindFootprintByReference(ref)
    swapped_pads = [p for p in fp.Pads() if p.GetNetname() in lines]
    # the pair's pads on every other part: the stretch that changes net must not reach one of them
    other_pads = [p for f in seed.GetFootprints() if f.GetReference() != ref for p in f.Pads()
                  if p.GetNetname() in lines]

    def touches(items, pads):
        return any(p.HitTest(pcbnew.VECTOR2I_MM(x, y)) for k, d in items for (x, y) in
                   ([(d[0], d[1]), (d[2], d[3])] if k == 'seg' else [d]) for p in pads)
    ts = twist_sites(P, N, lines, lambda g: touches(g, swapped_pads), lambda g: touches(g, other_pads), Lt=Lt,
                     margin=margin, outs=outs, layers=(0,))
    if ts.refused:
        log.append(f'seed twist {P}/{N}: {ts.refused} sites refused, beyond another part of the line')
        print(log[-1], flush=True)
    sites, skews, amp, seed_skew = ts.sites, ts.skew, ts.amp, ts.before

    def legal(zone, fd=None):
        return twist_legal(B, P, N, zone, fd)

    def place(site, ripped):
        out_, t, centre, zone, a, ka, b, kb, a0, d, nl, off = site
        out = ts.build(site)
        # the seed's lines replace whatever stage 1 laid for this pair (they contain its fan-outs)
        B.tracks = [x for x in B.tracks if x[0] not in (P, N)] + out
        B.fixed = [x for x in B.fixed if x[0] not in (P, N)] + out
        B.release_escape([P, N])
        B.rebuild()
        log.append(f'seed twist {P}/{N}: spliced into the seed\'s run at ({centre[0]:.2f}, {centre[1]:.2f}), '
                   f'{Lt} mm along ({d[0]:.2f}, {d[1]:.2f}), vias {out_} mm out at '
                   f'{[tuple(round(v, 2) for v in z[2]) for z in zone if z[1] == "via"]}'
                   + (f', crossing S {amp[id(site)]:.2f} mm' if abs(amp[id(site)] - out_) > 1e-6 else '')
                   + f', skew '
                   f'{abs(skews[id(site)]):.2f} mm (seed {abs(seed_skew):.2f} mm, {len(sites)} sites)'
                   + (f'; cut {sorted(ripped)} within {RIP_R} mm, re-routed after' if ripped else ''))
        return True, set(ripped)

    def over(site):
        return abs(skews[id(site)]) > PAIR_SKEW_MAX
    rank = ts.rank
    fd0 = B.foreign(P)                                # P and N share a code, so this is both lines' field
    free = next((site for site in sorted(sites, key=rank) if legal(site[3], fd0)), None)
    if free is not None and not over(free):           # a site with room as it is, the best of those
        return place(free, ())
    # None: free a site by cutting the simple signals in its way (never a pair, a crystal line, a rail or a plane) near
    # it - the signal router re-joins each cut net afterwards, so the board changes by the twist and those few mm.
    codes = {c: n for n, c in B.netcode.items()}
    RIP_R = 1.5

    def blockers(zone, fd=None):
        nets = set()
        for nm, k, dd in zone:
            if items_legal(B, nm, [(nm, k, dd)], strict=True, fd=fd):
                continue
            pts = [(dd[0], dd[1], range(NL))] if k == 'via' else [
                (dd[0] + (dd[2] - dd[0]) * q / 8, dd[1] + (dd[3] - dd[1]) * q / 8, (int(dd[5]),)) for q in range(9)]
            for x, y, Ls in pts:
                j, i = gi(y, Y0), gi(x, X0)
                for L in Ls:
                    c = int(B.dyn.n1[L, j, i])
                    nets.add(codes.get(c, '#'))
        return nets

    def rippable(n):
        return (n in B.netcode and n not in B.pair_of and n not in PLANES and n not in POWER
                and not re.match(XTAL_NETS, n))
    if os.environ.get('ROUTE_SEED_TWIST_DEBUG'):      # debugging: per pair of runs, the sites, their skew, what blocks
        free_ids = {id(x) for x in sites if legal(x[3], fd0)}
        runs = {}
        for site in sites:
            runs.setdefault((site[5], site[7]), []).append(site)
        for (ka, kb), ss in runs.items():
            a = ss[0][4][1]
            fr = [x for x in ss if id(x) in free_ids]
            sk = sorted(abs(skews[id(x)]) for x in ss)
            bl = collections.Counter(n for x in ss if id(x) not in free_ids for n in blockers(x[3], fd0) - {P, N})
            print(f'  seed twist {P}: run ({a[0]:.2f},{a[1]:.2f})-({a[2]:.2f},{a[3]:.2f}): {len(ss)} sites, '
                  f'{len(fr)} fit, skew {sk[0]:.2f}-{sk[-1]:.2f} mm'
                  + (f' (fitting {min(abs(skews[id(x)]) for x in fr):.2f})' if fr else '')
                  + f', blocked by {dict(bl.most_common(6))}', flush=True)
    ranked = []
    for site in sites:
        if not zone_self_clear(site[3], P, N):
            continue
        bl = blockers(site[3], fd0) - {P, N}
        if bl and all(rippable(n) for n in bl):
            r = rank(site)
            ranked.append(((r[0], len(bl)) + r[1:], bl, site))
    for _, bl, site in sorted(ranked, key=lambda r: r[0]):
        if free is not None and over(site):
            break                                     # no cut buys a site within budget: take the best free one below
        cx, cy = site[2]
        near = [x for x in B.tracks if x[0] in bl and (
            math.hypot(x[2][0] - cx, x[2][1] - cy) <= RIP_R if x[1] == 'via' else
            seg_point_dist(cx, cy, *x[2][:4]) <= RIP_R)]
        B.tracks = [x for x in B.tracks if x not in near]
        B.fixed = [x for x in B.fixed if x not in near]
        B.rebuild()
        if legal(site[3]):
            return place(site, bl)
        B.tracks += near
        B.fixed += near
        B.rebuild()
    if free is not None:                              # nothing within the skew budget: the least skew that fits
        return place(free, ())
    log.append(f'seed twist {P}/{N}: no straight stretch with room for a twist - re-routing the pair')
    return False, set()


def seg_point_dist(x, y, x1, y1, x2, y2):
    L2 = (x2 - x1) ** 2 + (y2 - y1) ** 2
    u = 0.0 if L2 == 0 else max(0.0, min(1.0, ((x - x1) * (x2 - x1) + (y - y1) * (y2 - y1)) / L2))
    return math.hypot(x - x1 - u * (x2 - x1), y - y1 - u * (y2 - y1))


def _ends(item):
    k, d = item
    return [(d[0], d[1]), (d[2], d[3])] if k == 'seg' else [d]


def _on_item(x, y, item, tol=1e-3):
    k, d = item
    if k == 'via':
        return math.hypot(x - d[0], y - d[1]) <= tol
    x1, y1, x2, y2 = d[:4]
    L2 = (x2 - x1) ** 2 + (y2 - y1) ** 2
    u = 0.0 if L2 == 0 else max(0.0, min(1.0, ((x - x1) * (x2 - x1) + (y - y1) * (y2 - y1)) / L2))
    return math.hypot(x - x1 - u * (x2 - x1), y - y1 - u * (y2 - y1)) <= tol


def _nm(x, y):
    """mm -> board units, rounded: VECTOR2I_MM truncates, so 128.2 (128.19999...) became 128199999 nm and a seeded run
    (--seed) moved every such point of the seed by 1 nm."""
    return pcbnew.VECTOR2I(int(round(x * 1e6)), int(round(y * 1e6)))


def write_board(B, zones=True):
    n = collapse_hops(B, [])         # last: every net is routed, so nothing depends on the old copper any more
    if n:
        print(f'layer hops collapsed: {n} (a net that dropped a layer and came straight back for nothing)', flush=True)
    board = B.board
    for t in list(board.GetTracks()):
        board.Remove(t)
    for z in list(board.Zones()):
        if not z.GetIsRuleArea():                     # the old pours go; rule areas (the antenna keep-out) stay
            board.Remove(z)
    nets = {n: board.FindNet(n) for n in set(B.netcode)}
    for name, kind, data in B.tracks:
        data = tuple(float(v) for v in data)
        if kind == 'seg':
            x1, y1, x2, y2, w, L = data
            t = pcbnew.PCB_TRACK(board)
            t.SetStart(_nm(x1, y1))
            t.SetEnd(_nm(x2, y2))
            t.SetWidth(pcbnew.FromMM(w))
            t.SetLayer(LAYERS[int(L)])
        else:
            x, y = data
            t = pcbnew.PCB_VIA(board)
            t.SetPosition(_nm(x, y))
            t.SetViaType(pcbnew.VIATYPE_THROUGH)
            t.SetLayerPair(pcbnew.F_Cu, pcbnew.B_Cu)
            t.SetWidth(pcbnew.FromMM(VIA_D))
            t.SetDrill(pcbnew.FromMM(VIA_DRILL))
        t.SetNet(nets[name])
        board.Add(t)
    zone_layers = [(pcbnew.F_Cu, 'GND'), (pcbnew.In1_Cu, 'GND'), (pcbnew.In2_Cu, '+3V3'), (pcbnew.B_Cu, 'GND')]
    for L, netname in (zone_layers if zones else []):
        z = pcbnew.ZONE(board)
        z.SetLayer(L)
        z.SetNet(nets[netname])
        ol = z.Outline()
        if not hasattr(ol, 'NewOutline'):
            ol = pcbnew.SHAPE_POLY_SET()
            ol.thisown = 0
            z.SetOutline(ol)
        ol.NewOutline()
        for x, y in [(X0, Y0), (X1, Y0), (X1, Y1), (X0, Y1)]:
            ol.Append(pcbnew.FromMM(x), pcbnew.FromMM(y))
        z.SetLocalClearance(pcbnew.FromMM(0.2))
        z.SetMinThickness(pcbnew.FromMM(ZONE_MIN_W))
        z.SetThermalReliefGap(pcbnew.FromMM(THERMAL_GAP))
        z.SetThermalReliefSpokeWidth(pcbnew.FromMM(0.3))
        z.SetPadConnection(pcbnew.ZONE_CONNECTION_THT_THERMAL)   # SMD pads solid (a USB-C pad row fits no spokes)
        z.SetIslandRemovalMode(pcbnew.ISLAND_REMOVAL_MODE_ALWAYS)
        z.SetAssignedPriority(0)
        board.Add(z)
    filler = pcbnew.ZONE_FILLER(board)
    filler.Fill(board.Zones())
    pro = PCB.replace('.kicad_pcb', '.kicad_pro')
    keep = open(pro).read()
    out = PCB
    if os.path.getmtime(PCB) != PCB_MTIME:           # regenerated (gen_pcb.py) while we routed: do not clobber it
        out = PCB.replace('.kicad_pcb', '.stale-route.kicad_pcb')
        print(f'{PCB} changed during routing: result written to {out} instead', flush=True)
    pcbnew.SaveBoard(out, board)
    with open(pro, 'w') as f:
        f.write(keep)


# ── live view: JSON snapshots for tools/route_view.html, served on localhost ──
class Live:
    """uv run tools/route.py --live[=PORT]  ->  http://localhost:PORT/ redraws all copper layers as routing goes."""

    def __init__(self, B, board, log, port):
        import http.server
        import shutil
        import threading
        self.B, self.log, self.t0, self.t_last, self.seq = B, log, time.time(), 0.0, 0
        self.run = int(self.t0 * 1000)    # the page reloads the pads when a new run (maybe a new placement) starts
        self.stage, self.net, self.ok, self.failed, self.unrouted = '', '', 0, 0, []
        self.dir = os.path.join(HERE, 'tools', 'build', 'live')
        os.makedirs(self.dir, exist_ok=True)
        shutil.copy(os.path.join(HERE, 'tools', 'route_view.html'), os.path.join(self.dir, 'index.html'))
        names = sorted({pad.GetNetname() for lst in B.padobj.values() for pad, _ in lst if pad.GetNetname()})
        self.idx = {n: k for k, n in enumerate(names)}
        r = lambda v: round(float(v), 3)
        pads = []
        for lst in B.padobj.values():
            for pad, shapes in lst:
                for sh in shapes:
                    par = [[r(x), r(y)] for x, y in sh.p] if sh.kind == 'poly' else [r(v) for v in sh.p]
                    pads.append([sh.kind, par, [L for L in sh.layers if L < 2], self.idx.get(pad.GetNetname(), -1)])
        outl = pcbnew.SHAPE_POLY_SET()
        board.GetBoardPolygonOutlines(outl, True)
        rings = []
        for k in range(outl.OutlineCount()):
            for ch in [outl.Outline(k)] + [outl.Hole(k, h) for h in range(outl.HoleCount(k))]:
                rings.append([[r(MM(ch.CPoint(q).x)), r(MM(ch.CPoint(q).y))] for q in range(ch.PointCount())])
        xs = [p[0] for p in rings[0]]
        ys = [p[1] for p in rings[0]]
        self._write('static.json', {'run': self.run, 'nets': names, 'pads': pads, 'outline': rings,
                                    'extent': [min(xs), min(ys), max(xs), max(ys)]})
        live_dir = self.dir

        class Quiet(http.server.SimpleHTTPRequestHandler):
            def __init__(self, *a, **k):
                super().__init__(*a, directory=live_dir, **k)

            def end_headers(self):
                self.send_header('Cache-Control', 'no-store')
                super().end_headers()

            def log_message(self, *a):
                pass
        srv = http.server.ThreadingHTTPServer(('127.0.0.1', port), Quiet)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        print(f'live view: http://localhost:{port}/', flush=True)
        self.update('start', force=True)

    def _write(self, name, obj):
        import json
        tmp = os.path.join(self.dir, name + '.tmp')
        with open(tmp, 'w') as f:
            json.dump(obj, f, separators=(',', ':'))
        os.replace(tmp, os.path.join(self.dir, name))

    def update(self, stage=None, net=None, force=False):
        if stage is not None:
            self.stage = stage
        if net is not None:
            self.net = net
        now = time.time()
        if not force and now - self.t_last < 0.4:
            return
        self.t_last, self.seq = now, self.seq + 1
        r, ix = (lambda v: round(float(v), 3)), self.idx
        segs = [[r(d[0]), r(d[1]), r(d[2]), r(d[3]), r(d[4]), int(d[5]), ix.get(n, -1)]
                for n, k, d in self.B.tracks if k == 'seg']
        vias = [[r(d[0]), r(d[1]), ix.get(n, -1)] for n, k, d in self.B.tracks if k == 'via']
        if not hasattr(self, 'marks'):
            self.marks = []              # problem spots: no way through, a pair that could not keep its pitch, ...
        # the *reserved* fan-out rows and vias (virtual copper: never written to the board, so not in tracks) are shown
        # faintly, because they are what the chips' and connectors' escapes are made of while routing goes on
        rs = [(n, d) for n, k, d in self.B.reserved + self.B.escape_items() if k == 'seg' and d[4] <= W_SIG + 0.01]
        rv = [(n, d) for n, k, d in self.B.reserved + self.B.escape_items() if k == 'via']
        res = [[r(d[0]), r(d[1]), r(d[2]), r(d[3]), r(d[4]), int(d[5]), ix.get(n, -1)] for n, d in rs]
        resv = [[r(d[0]), r(d[1]), ix.get(n, -1)] for n, d in rv]
        # the last failed attempt of a link, kept as a drawing only (live_ghost): dashed in the view, never on the board,
        # gone as soon as that net is attempted again or routes.  Same payload shape as ``res`` so the layer switches,
        # the highlight/dim pass and click-to-pick all work on it.
        gh = getattr(self, 'ghosts', None) or {}
        gsegs, gvias, gwhy = [], [], []
        for _n, _g in gh.items():
            _i = ix.get(_n, -1)
            if _i < 0:
                continue
            for _k, _d in _g['items']:
                if _k == 'seg':
                    gsegs.append([r(_d[0]), r(_d[1]), r(_d[2]), r(_d[3]), r(_d[4]), int(_d[5]), _i])
                elif _k == 'via':
                    gvias.append([r(_d[0]), r(_d[1]), _i])
            gwhy.append([_i, _g.get('why') or ''])
        self._write('state.json', {'run': self.run, 'seq': self.seq, 'elapsed': now - self.t0, 'stage': self.stage,
                                   'net': self.net,
                                   'ok': self.ok, 'failed': self.failed, 'unrouted': sorted(self.unrouted),
                                   'segs': segs, 'vias': vias, 'res': res, 'resv': resv, 'marks': self.marks,
                                   'ghost': gsegs, 'ghostv': gvias, 'ghostwhy': gwhy,
                                   'log': [l.rstrip() for l in self.log[-60:]]})


LIVE = None


def live(stage=None, net=None, force=False):
    if LIVE is not None:
        LIVE.update(stage, net, force)


def live_ghost(names, items, why):
    """Keep the *last failed routing attempt* for these nets drawn on the live view.

    The pieces are taken off the board (they are the copper the failure path just dropped), so this is a picture only:
    it is not in ``B.tracks``, the router's field never sees it, it clears no clearance and holds nothing up - it is
    counted as discarded (the link is in ``unrouted`` and in the failed attempt count exactly as before).  It stays on
    screen until a *new* attempt for that net starts (``live_ghost_clear``) or that net routes, so the geometry that
    failed can be inspected lane by lane next to the reservations it was supposed to fit.
    """
    if LIVE is None:
        return
    gh = dict(getattr(LIVE, 'ghosts', None) or {})
    for n in names:
        gh.pop(n, None)
        mine = [(k, d) for n2, k, d in items if n2 == n]
        if mine:
            gh[n] = {'items': mine, 'why': why}
    LIVE.ghosts = gh


def live_ghost_clear(names):
    """A new route for these nets has come along: the discarded picture is dropped (it is only ever one attempt old)."""
    if LIVE is None:
        return
    gh = dict(getattr(LIVE, 'ghosts', None) or {})
    for n in names:
        gh.pop(n, None)
    LIVE.ghosts = gh


def live_mark(x, y, label, r=1.2, kind='fail'):
    """Put a mark on the live view: a problem spot (a lane with no way through, a pair that could not keep its pitch),
    so it can be inspected while the router runs.  kind 'fail' (red) or 'info' (yellow)."""
    if LIVE is not None:
        LIVE.marks.append({'x': round(float(x), 2), 'y': round(float(y), 2), 'r': round(float(r), 2),
                           'label': str(label)[:90], 'kind': kind})
        del LIVE.marks[:-60]
        LIVE.update(force=True)


def live_result(name, ok):
    if LIVE is not None:
        if ok:
            LIVE.ok += 1
            if name in LIVE.unrouted:
                LIVE.unrouted.remove(name)
        else:
            LIVE.failed += 1


# ── link list ─────────────────────────────────────────────────────────────────
PORTS = ['KB', 'MS', 'OT']
HUB_DS = {1: {'KB': ('2', '1'), 'MS': ('4', '3'), 'OT': ('7', '6'), 'AT': ('9', '8')},    # AT32 #1 east of Hub-1
          2: {'AT': ('2', '1'), 'KB': ('4', '3'), 'MS': ('7', '6'), 'OT': ('9', '8')}}    # AT32 #2 west of Hub-2
HUB_DS[0] = {'KB': ('2', '1'), 'MS': ('4', '3'), 'OT': ('7', '6')}
HUB_REF = {0: 'U701', 1: 'U801', 2: 'U901'}
# hub -> (P pin, N pin) on the TS3USB3031.  **Rev 6 (the MHL fix):** the MHL channel is on Hub-0 and USB2 on Hub-1,
# so the two pin groups swap ends: Hub-0's pairs leave the *west-facing* MHL pins 7/8 and Hub-1's the
# *north-facing* USB2 pins 5/6, while Hub-2 keeps USB1 (pins 3/4).  That is what the lanes below are planned for.
# What it buys: the channel TI specifies only at 1.5-3.3 V signal levels carries the KVM path, whose data is full
# speed anyway, and both PCs keep the datasheet-specified USB1/USB2 channels (design.md §7, report.md F4).
# Pad ends measured on the placed board (gen_pcb.py + gen_schematic.py): U1202 pins 7/8 at (110.75, 153.55/153.95)
# facing west, pins 5/6 at (110.9/111.3, 152.863) facing north, pins 3/4 at (111.7/112.1, 152.863); U1302 and
# U1402 the same shape 35 mm apart.  Both affected pairs therefore leave the same corner of a router, which puts
# their two lanes on opposite sides of each other and needs the layer change below (see the lane plan).
MUX_PINS = {0: ('7', '8'), 1: ('5', '6'), 2: ('3', '4')}
E, W, N, S = (1, 0), (-1, 0), (0, -1), (0, 1)
NE = (1, -1)                   # a waypoint inside a diagonal leg (DIRS has all eight)
# Lane plan (port router -> hub): waypoints (x, y, layer after it, travel direction), each in the middle of a
# straight run (or at both ends of a corner, so a lane cannot cut across the inside of the corner where the next
# lane runs).  The routers sit left of their USB-A ports: each has its MHL pins facing west, its USB1/USB2 pins
# north, and the corridors below are unchanged from rev 5 - only their router ends swapped (MUX_PINS).
#  - Hub-0's lanes (from the *west-facing* MHL pins) cross the board on B.Cu - under the keyboard router, under the
#    key's south edge - and come up into Hub-0 from the south-west (H0_LANES).  Because both affected pairs now
#    leave the same corner of a router, Hub-0's lane and Hub-1's lane cross just outside the pads: Hub-0's lane
#    changes to B.Cu at a via pair in the box west of its own pads (KEEP_FREE 100.5-110.0 x 150.2-154.3, the same
#    reserved area the layer changes used before) and runs north on B.Cu, so Hub-1's F.Cu escape into the west
#    corridor passes over it - the router resolves same-corner conflicts by layer, as it did when the two families
#    left different faces.
#  - Hub-1's lanes (from the *north-facing* USB2 pins) leave north and run west into the same corridors as before,
#    the keyboard lane on F.Cu and the mouse / other lanes under it on B.Cu, then down the west edge and east into
#    Hub-1's band (y 126.0-129.6, KEEP_FREE per rear connector).
#  - Hub-2 lanes run straight down from the hub, then west along a band above the key (between the RP2354B and
#    the switch's pin holes) and drop into their routers.
#  - Lanes nest (the outermost lane turns last) so none crosses another.
LANES = {
    # the keyboard lane leaves the north face and turns west 2.5 mm out (a pair needs room for its first turn,
    # measured: 0.66 mm gave "lines only -0.150 mm apart"); then down the west edge on F.Cu, as before
    (1, 'KB'): [(111.1, 150.4, 0, N), (108.9, 150.4, 0, W), (104.5, 150.4, 0, W), (102.5, 148.4, 0, N),
                (102.5, 129.2, 0, N), (104.5, 127.2, 0, E), (110.0, 127.2, 0, E)],
    # The mouse and Other lanes are planned against one measured limit: *both* Hub-1's (pins 5/6) and Hub-2's
    # (pins 3/4) pairs leave their router on the north row (y 152.863), and a pair terminal reserves its escape
    # corridor on **both** layers - to y 150.863 on B.Cu and to 148.863 on F.Cu (route.py, the reserve() pair at the
    # head of the pair phase).  So across the three routers' x-bands (110.5-113, 144.5-146.5, 179.5-181) any lane
    # needs y <= 150.21 on B.Cu or y <= 148.21 on F.Cu, whatever layer it runs on.  The rev-5 lanes ran through that
    # band on their own pad row (y 152.863) and at 150.8/151.3, which only ever fitted when they were routed before
    # the keyboard pair - measured in one run, in every other order the group tried:
    #   "leg 2 -> (112.5, 152.863) on B.Cu | blocked by ['#-9', 'H2_KB_DM']",
    #   "leg 4 -> (105.5, 152.2) on B.Cu   | blocked by ['#-9', 'H0_KB_DM']",
    #   "leg 5 -> (112.0, 151.3) on B.Cu   | blocked by ['#-9', 'H1_MS_DM', 'H2_KB_DM']".
    # Both lanes therefore drop to 149.6 / 148.6 - north of that band, 1.0 mm apart, each 1.0 mm below the keyboard
    # lane's 150.4 - and change to F.Cu at x 109.5 / 108.0, west of the keyboard router's corridor (x 110.45-111.75)
    # and east of the west-edge corridor those three nest in.  Running them on B.Cu all the way west was tried and
    # fails on the plan, not on room: Hub-0's keys' pair owns B.Cu there (its lane turns north at x 105.0 and runs out
    # diagonally), and two B.Cu lanes cannot cross without one of them re-planning.
    # The mouse lane descends 45 degrees (6 mm over 6 mm) to its own y and then runs one straight leg west on B.Cu,
    # entering its via pair at (108.5, 146.2) travelling west.  A *steep* descent, or a 45-degree arrival at a waypoint
    # marked W, folds the pair's inside line - measured "lines only -0.175 mm apart (want 0.150 mm) at (113.15,145.30)
    # on F.Cu" twice, with the second attempt also producing "-0.278 mm apart at (104.76,128.81)".  y 146.2 is 1.3 mm
    # north of Hub-0's keyboard lane's elbow (110.5, 147.5) and 1.0 mm south of the F.Cu keyboard corridors' boxes.
    # STILL OPEN: a *square* via pair does not fit at the west edge (H1_KB at x 102.5 and H1_OT at 104.5 on F.Cu), and a
    # staggered pair's two connecting diagonals run towards each other and pinch to 0.129 mm ("P via (109.25,145.75) /
    # N track (109.71,146.00)-(109.36,146.35)", measured 2026-10-25).  Moving the change east fixes it for this pair -
    # at x 118 it routes as "1487 cells, P 78.86 mm N 78.99 mm (skew 0.13 mm), 4 vias, gap 0.150" with the Other lane's
    # corridor dipped to 144.7 - but that dip then takes Hub-2's keyboard lane's via site at (138.0, 147.0) away
    # ("no via-pair site near waypoint 2"), so both lanes cannot have their way at once.  The real fix is in store()'s
    # staggered composition: each line must reach its via by its own perpendicular fan-out at that via's axial position,
    # instead of by the single diagonal that currently runs towards its partner.
    # rev 9, 2026-10-27: that west-edge detour (83 mm, and in all six first-pass orders in conflict with the
    # keyboard lane at the keyboard router) is gone.  The mouse pair now runs straight at the hub: F.Cu north-west out of
    # the router and west along y 145.95 (over Hub-0's Mouse B.Cu run and its via pair at (139.6, 147.3)), across Hub-0's
    # KB pair (B.Cu, y 142.75) on F.Cu, one via pair, then **B.Cu** north under AT32 #1's east pin row (clear of its
    # fan-out vias at (120.4, 138.35) / (121.1, 139.15) and of H1_AT's twist via at (116.65, 132.4)) and under H1_AT
    # itself, and a second via pair at (116.25, 128.85) heading straight up into pins 4/3.  Found by searching the fat
    # line on the routed pair stage with Hub-1's Other pair removed: 48.2 mm with two layer changes (a cheaper via cost
    # finds 41 mm with four).  The first via pair is asked for as far south as Hub-0's KB pair allows (the site search
    # puts it at (120.3, 141.5)), because AT32 #1's SPI pins 14-17 (y 139.75-141.25) have no fan-out via: they are bus
    # stubs that leave east on F.Cu to the RP's stubs at x 140 (FANOUT_LEN).  At (119.85, 141.0) the pair sat in front
    # of AT1_CS / AT1_SCK and the bus found no way out in any of its 24 orders; 0.5 mm further south it routes (order
    # SCK, MISO, CS, MOSI, 25.0-28.4 mm; AT1_CS leaves F.Cu once to get past it, 2 vias).
    (1, 'MS'): [(139.0, 145.95, 0, W), (124.5, 145.95, 0, W), (120.3, 141.95, 1, N), (120.3, 140.0, 1, N),
                (117.85, 135.0, 1, N), (116.25, 128.85, 0, N)],
    # The Other lane comes from the far end of the router row, so it drops as soon as it is clear of Hub-2's Other
    # corridor (x 179.25-180.55), and it crosses the key on **F.Cu**, at y 146.1-146.4 - the one corridor there is.
    # Measured on the placed board (pair centre needs 0.375 mm from every pad): across x 150..170 F.Cu is free only at
    # y 145.75-146.50 (C517 at x 159.7-160.9 below, the socket's 3.05 mm pad at x 160.6-164.4 from y 146.9 above), and
    # B.Cu only to 146.4 (SW1501's through-hole pins).  That is *one* pair-width per layer, so the two pairs that have
    # to get past the key cannot share a layer: with this lane on B.Cu at 148.2 the search slid it north to 146.15 and
    # H2_MS's lane at 146.65 then read "leg 2 -> (165.0, 146.65) on B.Cu | blocked by ['#-9', 'H1_OT_DM']", and its run
    # west also crossed Hub-0's Other pair's forced north leg at x 177.5 (that pair's MHL pads face west, so it turns
    # north immediately) - which is why Hub-0's Other pair failed in every Hub-0 order too.
    # On F.Cu both of those crossings are over/under instead of into, and the pair needs no via pair at all: the pads,
    # the key crossing and the west-edge corridor (x 102.5-104.5) are all F.Cu.  Two obstacles shape the exact heights:
    # C1501 (the key's debounce cap, its KEY pad at x 171.89-172.51, y 152.04-153.56) sits on the pad-row height at
    # 152.2, so the lane descends at 45 degrees from x 174 - before it - and C517 (x 159.7-160.9, up to y 145.5) plus
    # H2_MS's layer-change vias at (150.5, 146.2 / 147.1) set the crossing: 146.1-146.4 across the key, 145.45 at
    # x 150.  R505 at (172, 145.5) is what keeps the crossing from going lower than 146.1 east of x 168.
    #
    # rev 9, 2026-10-27: the key crossing is the one F.Cu corridor H1_MS / H0_MS / H2_MS also need, so this pair
    # now goes north, round the RP2354B *and* under the hub row, and leaves the key crossing to them.  A first try went
    # west through the 1.0-1.3 mm gap between the hubs' south pads and the RP's north via row (y 125.4 at x 141) and
    # died at a via pair there: Hub-0's KB pair owns B.Cu at y 126.4-126.7 (x 137.4-157.9), the RP's own USB pair
    # crosses the gap diagonally on F.Cu, and a +3V3 via at (158.3, 124.4) plugs the gap at Hub-0's west side.
    # The lane below was found by searching the pair's fat line on the routed pair stage with every later pair's lane
    # (H0_MS, H0_OT and Hub-2's three) kept clear and nothing south of y 125 allowed between x 122 and 176:
    #  - out of the router's Hub-1 pins straight north, east of Hub-2's Mouse via pair at (181.0, 147.3), then up the
    #    empty strip at x 176 on **F.Cu** (Hub-0's Other lane runs parallel on B.Cu at x 177.5-178, 1.5-2 mm east);
    #  - one via pair at (173.5, 124.95), clear of Hub-0's east caps (x <= 170.5), and from there **B.Cu** west, under
    #    Hub-0's south pad row (F.Cu pads; its bottom-side caps and exposed-pad vias are further north), stepping up to
    #    y 123.6 at x 158 to pass north of that +3V3 via, then at y 124.1-124.3 under the F.Cu parts between the hubs -
    #    north of Hub-0's KB wall, so no Hub-0 lane is touched;
    #  - at y 124.3 until x 123.5 (2 mm clear of the hub's east fan-out via at (120.6, 124.4)), then one steep V under
    #    Hub-1's AT pair (F.Cu): down at 45 degrees to y 128.3, 0.9 mm west, and back up into a via pair heading north
    #    at (117.75, 126.75), straight into the hub's pins 6/7 between its Mouse pins and its AT pins (H1_AT's lane keeps
    #    it room).  The first version ran west along y 128.6 from x 125.3 and walled the B.Cu south of the hub; the V
    #    keeps everything south of y 128.6 free for Hub-1's Mouse pair, which now comes up from the south on B.Cu.
    # (The jog east out of the router's column is over by y 148.4: Hub-2's Mouse pair changes layer at (181.0, 147.3)
    # with its run straight down x 181, and a later jog ran this pair over that via pair - Hub-2's Mouse then stayed on
    # B.Cu and walled Hub-0's Other lane at x 177.5.)
    (1, 'OT'): [(181.1, 150.0, 0, N), (182.3, 148.4, 0, N), (182.3, 147.2, 0, N), (176.05, 137.0, 0, N),
                (176.05, 127.5, 0, N), (173.5, 124.95, 1, W), (166.0, 124.95, 1, W), (158.0, 123.6, 1, W),
                (148.0, 124.1, 1, W), (130.0, 124.3, 1, W), (123.5, 124.3, 1, W), (118.6, 128.3, 1, W),
                (117.75, 126.75, 0, N)],
    # Hub-2 Keyboard used to cross the board at y 145.5 on B.Cu, straight through the band between the key and the USB
    # switches - which is exactly where the four single-ended USB lines (KB_DP/DM, MS_DP/DM), the three SEL pairs and
    # the reset lines have to get from AT32 #2 (x 174) west to the switches.  Those nets were left unrouted on rev 9's
    # first full runs, and the pair itself paid for the wall it made: H0_MS then had to take an early layer change onto
    # that same B.Cu (measured, "P track (142.98,153.60)-(142.80,153.83) / N via (142.75,153.60)", lines 0.193 mm
    # apart).  The lane now goes the long way south, where B.Cu is empty for 4 mm between the switch row's bottom pads
    # (y 160.4) and the USB-A pads (y 166.0): east 8.5 mm on F.Cu, down at x 120.5 (its own column between the keyboard
    # switch's escape corridor x 110.5-113 and the KB port at x 125), east along y 161.5 under all three ports, and
    # back up at x 186 - the 4 mm gap east of the Other switch (x 185.1) and west of the Other lane's F.Cu run at
    # y 150.6.  161.5 leaves 1.1 mm to the switch row's copper and 4.5 mm to the ports, and the two corners have a
    # straight run either side so the via rows sit where the pair is straight; the north leg keeps to B.Cu until
    # y 150.0, which is 0.6 mm below the Other lane's F.Cu, and changes back at (188, 147) as before - north of Hub-2's
    # mouse lane, whose B.Cu diagonal crosses x 186 at y 148.2.
    (2, 'KB'): [(111.9, 151.5, 0, N), (121.5, 150.6, 0, E), (120.5, 155.0, 1, S), (120.5, 161.5, 1, S),
                (184.0, 161.5, 1, E), (186.0, 159.5, 1, N), (186.0, 150.0, 1, N), (188.0, 147.0, 0, E),
                (195.0, 147.0, 0, E), (206.25, 135.0, 0, N)],
    # Hub-2's mouse lane changes back to F.Cu for x 174-181: Hub-0's Other pair has to turn north at x 177.5 (its MHL
    # pads face west, so that is its own escape), and this lane's east run crosses it there.  The layer change is what
    # makes that a crossing instead of a block, and 174/181.0 is where the two via pairs clear R505 (x 172, its pad to
    # 172.5) and Hub-0's Other lane's vias; F.Cu above the key at y 147 is free - R505 and C519 are the only parts there.
    (2, 'MS'): [(146.9, 150.0, 0, N), (150.5, 146.65, 1, E), (165.0, 146.65, 1, E), (174.0, 146.9, 0, E),
                (181.0, 147.3, 1, S), (189.5, 148.8, 0, E), (197.0, 148.8, 0, E), (207.75, 136.0, 0, N)],
    (2, 'OT'): [(181.9, 151.5, 0, N), (196.0, 150.6, 0, E), (208.75, 137.0, 0, N)],
}
# PC1 / PC2 ESD -> hub upstream (west side): west just below the ESD, then south well clear of the hub, so the hub's
# north-west corner (NR0 / +3V3 pins and the NR0 strap resistor) stays free.  Measured when the ESDs moved 1.0 mm north
# (rev 9): dropping the west run to 115.9, level with the new pad row, routes the same 172 cells but 0.17 mm longer
# (P 10.40 / N 9.90 vs 10.23 / 9.73), so the lane stays where it is.
UPSTREAM_LANES = {n: [(x - 2.0, 116.9, 0, W), (x - 4.2, 119.0, 0, S)] for n, x in ((1, 115.0), (2, 205.0))}
# AT32 #1 sits right below Hub-1 with its D+/D- pins mirrored against the hub's row, so its link is a short twist at the
# AT32's own pins; it climbs at x 119.8 and only turns into the hub's pins 8/9 at the last moment, so Hub-1's Other pair
# (pins 6/7, arriving from below on its own via pair at (117.75, 126.75)) has the 0.9 mm its vias need beside it.
AT_LANES = {1: [(119.8, 130.0, 0, N), (119.8, 127.6, 0, N)],
            2: [(198.0, 145.5, 0, E), (205.25, 135.0, 0, N)]}            # #2 wraps around its east side, up to
                                                                         # Hub-2's west end
PORT_C = {'KB': 125.0, 'MS': 160.0, 'OT': 195.0}                       # USB-A centres (tools/gen_pcb.py)
H0_LANES = {'KB': [(107.0, 153.75, 1, W), (105.0, 153.75, 1, W), (105.0, 150.5, 1, N), (110.5, 147.5, 1, NE),
                   (132.0, 132.0, 1, N), (157.9, 126.55, 0, E)],
            # 0.15.0: (110.5, 147.5) heads NE, the way the lane leaves it.  Marked N, it made the search turn E -> N ->
            # NE within 0.1 mm and the inner line folded back on itself: "lines only 0.137 mm apart (want 0.150 mm) at
            # (109.79,147.60) on B.Cu", the link's failure in every unseeded --pairs-only run (0.12.0's sources too).
            # rev 6: the MHL pads (7/8) are on this lane's own row, so it runs west 3.75 mm along that row - where the
            # pair sits exactly on its two pads - and drops to B.Cu at the via pair in the reserved box; that is what
            # lets the Hub-1 lane escape over it on F.Cu.  Then north on B.Cu (west of AT32 #1's crystal, whose GND pad
            # reaches x 111.70), north-east to the point where the old diagonal clears AT32 #1, and on into Hub-0's
            # keyboard pins.  (The north leg and the diagonal must not reverse direction: a dogleg back east made the
            # pair's two lines cross - "lines only -0.150 mm apart".)
            # rev 9, 2026-10-26: the old lane crossed *under* the RP2354B - (145.95,137.05) and (148.0,132.0)
            # are inside its 143-153 x 131-141 body - and that is now where the chip's per-pin decoupling caps sit on
            # B.Cu.  A lane west of the chip and east along y 128.6 did not route either: that band is the RP's north
            # via row ("leg 5 -> (152.5, 128.6) on B.Cu | blocked by ['RP_VREG']"), and its climb at x 139.6 would
            # have crossed Hub-0's KB pair, which owns B.Cu at y 126.4-126.7.  2026-10-27: with Hub-1's Other pair
            # moved north the key band is free, so this pair goes *east* of the chip instead (found by searching its
            # fat line on the routed pair stage): B.Cu from the via pair at (139.6, 147.3) east along y 145.45 (under
            # the RP's south via row, 0.2 mm clear of Hub-2's Mouse via pair at (150.5, 146.2)), north-east to x 156
            # - east of the RP's body and its bottom-side caps - and north-east again to x 161.25, straight up into a
            # via pair right under Hub-0's pins 4/3.
            'MS': [(143.0, 153.75, 0, W), (139.6, 150.2, 0, N), (139.6, 147.3, 1, N), (141.5, 145.45, 1, E),
                   (151.0, 145.45, 1, E), (156.0, 139.0, 1, N), (161.25, 131.0, 1, N), (161.25, 127.0, 0, N)],
            # rev 9: this lane climbed the x 139.5 slot on F.Cu and changed layer at the top of it, at the hairpin
            # (139.5, 143.5).  That works only while nothing else is in the slot - and two Hub-1 pairs are: Other
            # crosses it east-west on F.Cu at y 145.4 (its long run from x 110 to 170), Mouse crosses it on B.Cu
            # diagonally from (143, 152.2) to (137, 146.2), i.e. y 148.8 at x 139.6.  In the full run the F.Cu climb
            # was walled, the leg's two-layer fallback dropped to B.Cu at the escape instead, and the pair then
            # failed twice: "lines only 0.106 mm apart at (140.26,144.33)" (one planned hairpin split across two runs)
            # and, with that fixed, on its own vias at the escape ("P via (143.00,153.30) / N track
            # (142.89,153.86)-(139.30,150.26)").  The slot has room for exactly one pair on each layer, so this lane
            # now crosses *Mouse* on F.Cu (y 150.2 to 147.3) and descends in the 3.3 mm gap between Mouse's diagonal
            # and Other's F.Cu: the via row at y 147.3 is 1.8 mm south of Other's copper and 1.5 mm north of Mouse's,
            # and the climb above it is B.Cu, which passes under Other's run.  Everything else is as before: north on
            # B.Cu round the RP2354B's south-west corner, and up through its north fan-out lane at x 161.35.
            # rev 9: the first waypoint used to be (144.4, 153.75, (-1,-1)) - 0.5 mm west of the escape point and
            # already asking for a 45 degrees heading.  The leg search answered it with a six-cell loop (a full circle
            # of 0.15 mm) to arrive heading south-west, whose offset lines folded onto each other ("lines only
            # -0.150 mm apart at (142.83,152.32)"); it only ever looked routed because trim_pair_tails() then cut the
            # loop *and* the P line's last points off.  The lane now runs straight west first (W), and the search
            # turns south-west on its own where there is room.
            # ... F.Cu north-west past the mouse router first (clear of the Hub-2 lanes' corner), then B.Cu under
            # the RP2354B's south side and up through its north fan-out lane at x 161.35 into Hub-0's mouse pins.
            # This is the leg length that measured clean (P 48.71 / N 49.08 mm, skew 0.37 mm, 4 vias, gap 0.150 mm)
            # ... Other: west 3.25 mm along its pad row (long enough for the pair's first turn and for the via pair of
            # the layer change, whose site the Hub-1 Other lane keeps free by staying on F.Cu there), then north at
            # x 177.5 into the diagonal that reaches Hub-0's Other pins.  Tried and measured on 2026-10-24, and *not*
            # used: descending earlier, at x 172 (east of the key's courtyard) and crossing west above the key the way
            # Hub-0's mouse pair does - "PAIR FAIL leg 2 -> (172.0, 143.0) on B.Cu": that corridor is blocked, the
            # hot-swap socket's own bottom-side pads are on B.Cu there, so the Other router cannot reach the mouse
            # pair's corridor at 161.35.  The eastward excursion this plan has (to x 186.05, AT32 #2's fan-out edge,
            # and back) is *not* the search's preference: the tight PAIR_WINDOW has no answer for this leg, and it is
            # the 15 / 40 mm grow ladder below that finds the far-east path.  Capping that ladder for pair legs (and
            # letting the two-layer PAIR_VIA_COST candidate stand in) is the next thing to try here.
            # The last waypoint is Hub-0's *Other* pins' own centre (162.75 = the mid of pins 6/7 at 162.5/163.0), not
            # 164.6: a planned lane is a wall for every other pair, and at 164.6 this lane's closing run north sat
            # inside the port-4 pair's corridor (x 163.525-163.975, y 125.75-127.9), so Hub-0's port-4 stub pair had
            # "no pair path from (0, 515, 1275) to (0, 570, 1278) within 52 mm of the direct line" in every order the
            # Hub-0 group tried.  The pair's pins are 0.55 mm west of port 4's, so both reach the hub side by side.
            'OT': [(177.5, 153.75, 1, W), (177.5, 151.55, 1, N), (178.0, 141.0, 1, N), (170.0, 131.5, 1, N),
                   (162.75, 127.6, 0, N)]}




def pair_links(B):
    """(P, N, terminal A, terminal B, layers, B.Cu cost, waypoints) in routing order."""
    links = []
    for n, (j, esd, hub) in enumerate([('J101', 'U101', 'U801'), ('J201', 'U201', 'U901')], start=1):
        nP, nN = f'PC{n}_DP', f'PC{n}_DM'      # not P/N: N is the north direction tuple, used by the links below
        links.append((nP, nN, ('usbc', j), ('pads', esd, '3', '1'), (0,), 9.9, ()))
        links.append((nP, nN, ('pads', esd, '4', '6'), ('pads', hub, '31', '30'), (0,), 9.9, UPSTREAM_LANES[n]))
    # The service port's own pair: J301 -> ESD U301 -> the service switch's USB1 pins 3/4, the same shape as the two PC
    # links above.  It was missing from this list *and* from pair_nets until rev 7, so the router treated its two lines
    # as independent signals and the signal phase's rip-up was free to cut one of them - measured, SVC_DP was routed
    # (5 pads) and then trimmed, leaving it in two pieces.
    # The connector's own pair on the same ESD needs a crossover too (the connector's D+ pads are west of its D- pads, the
    # ESD's D+ pad 3 is west of pad 1 as well but its escape faces the other way, so the P line changes side going A->B),
    # and its A end - the connector's fan-out - has nowhere to put the zone: the USB-C's four service pads are 0.5 mm
    # apart and the pair's own stubs are already at the limit there (measured "lines only -0.090 mm apart at
    # (160.38,109.06)").  So this link takes the default B-end zone, which needs 1.6 mm of copper north of the ESD's
    # escape: rev 6 moved the ESD 1.0 mm south of the macro spot for it, and rev 9 has all three input ESDs 1.0 mm
    # north of the macro spot instead (gen_pcb.py IN_MACRO, y 113.0) - the zone still fits there (J301 -> U301 routes,
    # 38 cells, skew 0.59 mm; y 112.5, 0.5 mm further north, is where it first fails).
    # rev 9: its B end keeps the default end run (see PAIR_END_RUNS - 0.5 mm of straight run into the crossover zone
    # did not fit between the zone and the connector's centre line, and the search made a roundabout of it).
    links.append(('SVC_DP', 'SVC_DM', ('usbc', 'J301'), ('pads', 'U301', '3', '1'), (0,), 9.9, ()))
    # This pair crosses over since 0.14.0: the switch's D+ is pin 5 again, the eastern pin (gen_schematic.py block 6,
    # errata.md E1), while the ESD has D+ on its western pad.  Rev 8 to 0.13 had SVC_DP on pin 6 to avoid exactly that,
    # because neither of route_pair's swap zones fits here: the A end's via lands in the neighbouring USB1 terminal's
    # reserved corridor 0.8 mm away; the B end needs 1.6 mm of copper beyond the ESD's escape, which is Hub-0's north
    # pad row - measured as "no pair path ... (0,344,1108) to (0,380,1200)" and, when room was made, "lines only
    # -0.190 mm apart"; unseeded, 0.14.0 failed with "no pair path from (0, 272, 842) to (0, 364, 1200)", 2026-11-06).
    # 0.14.0 was therefore routed from 0.12.0 with the twist spliced into the seed's lines (seed_twist).  Since 0.15.0
    # the link takes twist_at='run': it is routed as if no crossover were needed, and the twist goes into a straight
    # stretch of its own new lines (twist_in_run).
    links.append(('SVC_DP', 'SVC_DM', ('pads', 'U601', '5', '6'), ('pads', 'U301', '4', '6'), (0,), (9.9, 'run'), ()))
    # The RP2354B's own USB (the switch's common port, pins 11/10), both sections as pairs: its pins to the 27R series
    # resistors R503/R504 - which the RP2350 requires, so they stay - and then those resistors to the switch.  The second
    # section's two lines sit on two different parts, which is what padpair_terminal() is for: before rev 8 this link
    # could not be expressed at all and RP_DP/RP_DM were left to the signal phase, the last USB singles on the board.
    # The resistors are hand-placed between the two ends (gen_pcb.py PLACEMENT): since the switch moved to (140,114) they
    # sit north of the RP2354B with their pad 1 south, so this section leaves *south* into the RP's USB pins while the
    # other leaves *north* towards the switch's south-facing common port.  Both escapes face away from each other, so
    # their 2 mm reserved corridors cannot overlap (with both on one side they did, and *both* sections failed with
    # "no pair path" in the first rev-8 runs), and with the DP pad west of the DM pad at both ends - as on the RP2354B's
    # own pins - neither section needs a P/N crossover.
    # RP_DP is declared *before* RP_USB, and that order is load-bearing: both sections start at the same resistor pair,
    # so whichever is routed second must thread the first one's reserved corridor.  Routed the other way round, RP_DP
    # could not leave its own fan-out - "(145.40,122.70) blocked by ['#-9', 'RP_DM']", i.e. RP_USB's reservation plus
    # this pair's own N stubs, the same symptom the H0_UP pair once had (see pair_nets below).
    # The last waypoint sits in the short east leg into the escape (x 137.95), not at the corner: at (137.7, 114.0),
    # on the north leg's own x, the pair could only be heading east there by jogging west first, and the jog folded the
    # inner line to 0.063 mm from the outer one (measured with the twist in the run, 2026-11-07: centre ... (137.70,
    # 114.20) (137.62,114.12) (137.62,114.08) (137.70,114.00) (138.40,114.00)).
    # RP_DP is hand-planned, because the direct line from the resistors to the port is blocked by the chip itself: at rot
    # 90 the common port faces **west**, so its escape at (138.4,114.0) wants the pair arriving *from the west*, while the
    # 27R resistors are east of the chip (145.4,122.7).  The pair therefore leaves the resistors northwards, crosses west
    # south of the chip, comes up its west flank and turns east into the escape - the three waypoints below mark the middle
    # of each straight leg.  Rev 8 to 0.13 had RP_DP on the port's pin 10 so that it needed no crossover, which the
    # router could not place here; since 0.14.0 RP_DP is on pin 11, the common D+ (gen_schematic.py block 6), and the
    # crossover is a twist inside the run (twist_at='run', twist_in_run - 0.14.0 spliced it into 0.12.0's copper
    # instead).  A zone in front of the switch cannot work here: it would sit on the lane's own last waypoint, measured
    # "leg 3 -> (136.8, 114.0) on F.Cu | (136.80,114.00) blocked by ['RP_DM']" (unseeded, 2026-11-06).
    links.append(('RP_DP', 'RP_DM', ('padpair', 'R503', '2', 'R504', '2', N),
                  ('pads', 'U601', '11', '10'), (0, 1), (2.0, 'run'),
                  [(145.4, 118.5, 0, W), (137.7, 118.5, 0, N), (137.95, 114.0, 0, E)]))
    links.append(('RP_USB_DP_PIN', 'RP_USB_DM_PIN', ('pads', 'U501', '67', '66'),
                  ('padpair', 'R503', '1', 'R504', '1', S), (0, 1), 2.0, ()))
    # Hub-0's port 4 is unused - its two lines are pulled to +3V3 by R707/R708 to tell the hub the port is not removable -
    # but the copper from the hub's pins to those resistors is a pair like any other, so it is routed as one (again with
    # padpair_terminal spanning the two resistors).  The enforcement check in main() refuses to leave it to the signal
    # phase, which is how it was found: as a single it had 2.36 mm of skew and the SI table flagged it.  Its escape
    # direction is given as S because R707/R708 end up side by side (their two pad-1s 1.25 mm apart along x), so the
    # pair leaves south and meets the hub's own south escape; W would put both lines on top of each other.
    # This pair is *drawn*, not searched ('direct' in b_cost - the link has nothing between its ends): DP is on the hub's
    # pin 9 at x 164.0 and on R707's pad 1 at 164.55, DM on pin 8 at 163.5 and R708's pad 1 at 163.30, so both lines run
    # straight and slightly apart at both ends - same side, no P/N swap.  What the pair phase cannot do here is *start*:
    # the two escapes are 2.75 mm apart, the fan-out itself is 0.8 mm deep, and the search's fat centre line (0.45 wide)
    # cannot begin inside its own converging stubs - measured on the rev-8 geometry as a straight attempt at (163.75,
    # 125.75) whose nearest foreign copper is 0.716 mm away (H0_OT's line) while the router's own field for the fat line
    # reads 0.075, so "no pair path from (0, 515, 1275) to (0, 570, 1278)" in every Hub-0 order.  Two straight tracks
    # between the pads are exact here, and they are the same 4.56 mm for both lines (skew 0.00).
    links.append(('H0_D4_DP', 'H0_D4_DM', ('pads', HUB_REF[0], '9', '8'),
                  ('padpair', 'R707', '1', 'R708', '1', N), (0, 1), 'direct', ()))
    for n, at in ((1, 'U1001'), (2, 'U1101')):
        links.append((f'H{n}_AT_DP', f'H{n}_AT_DM', ('pads', at, '27', '26'), ('pads', HUB_REF[n], *HUB_DS[n]['AT']),
                      (0, 1), (2.0, 'A'), AT_LANES[n]))
    for k, p in enumerate(PORTS):
        b = 12 + k
        # USB-A -> ESD left of it: north out of the connector, west, north into the ESD
        links.append((f'{p}_DP', f'{p}_DM', ('pads', f'J{b}01', '3', '2'), ('pads', f'U{b}01', '3', '1'), (0,), 9.9,
                      [(PORT_C[p] - 7.0, 163.8, 0, W)]))
        links.append((f'{p}_DP', f'{p}_DM', ('pads', f'U{b}01', '4', '6'), ('pads', f'U{b}02', '11', '10'), (0,), 9.9, ()))
    for hub in (1, 2):
        for k, p in enumerate(PORTS):
            b = 12 + k
            pp, pn = MUX_PINS[hub]
            links.append((f'H{hub}_{p}_DP', f'H{hub}_{p}_DM', ('pads', f'U{b}02', pp, pn),
                          ('pads', HUB_REF[hub], *HUB_DS[hub][p]), (0, 1), 1.0, LANES[(hub, p)]))
    # Hub-0 (full speed) is routed last, after the AT32 SPI: it prefers B.Cu and hops layers across the HS lanes;
    # KB is the long one and gets a B.Cu lane north of AT32 #1, so it leaves the AT32's F.Cu side free
    for k, p in enumerate(PORTS):
        links.append((f'H0_{p}_DP', f'H0_{p}_DM', ('pads', f'U{12 + k}02', *MUX_PINS[0]),
                      ('pads', HUB_REF[0], *HUB_DS[0][p]), (0, 1), 0.9, H0_LANES.get(p, ())))
    # Hub-0's upstream pair: the service switch's USB2 pins 5/6 (adjacent, rev 8) to Hub-0's west-side pins 31/30.  No
    # hand-built breakout and no hand-planned lane any more - with the pair on one side of the WQFN the router's own
    # search answers it (measured 0.6 ps of skew).  That is the point of rev 8: the old part's straddling D+/D- could not
    # be a pair terminal, so it needed fixed copper and a lane that was re-planned by hand four times.
    # Hub-0's upstream is on USB1 (pins 3/4): both lines are the ordinary HS channels now, MHL is left unused (on
    # purpose), and with D+ on the eastern pin as usual this pair's two ends already agree on which side D+
    # is (the hub's own DP pin 31 is north of its DM pin 30, which is the matching side), so no crossover either.
    # 0.13.0 had D+ on U601 pin 4 and a twist in this pair (errata.md E1); since 0.14.0 D+ is pin 3, USB1+, again, and
    # the pair runs straight, as on 0.12.0.
    links.append(('H0_UP_DP', 'H0_UP_DM', ('pads', 'U601', '3', '4'), ('pads', HUB_REF[0], '31', '30'), (0, 1), 0.9, ()))
    return links



PCB_MTIME = None


def main():
    global PCB_MTIME
    t0 = time.time()
    PCB_MTIME = os.path.getmtime(PCB)
    board = pcbnew.LoadBoard(PCB)
    # Every differential link belongs in this list, and the list is not bookkeeping: Board.__init__ gives *both* lines of
    # a pair one net code, and that shared code is what Field.foreign() uses to ignore the mate's copper (where the
    # nearest copper carries one's own code, it answers with the second-nearest field instead).  B.pair_of does not do
    # this - it only drives layer choice, rip-up and the pair phase.  H0_UP was missing here, so its fat centre line
    # demanded CLR + PAIR_FAT/2 = 0.375 mm of clearance from a line that is only PAIR_O = 0.15 mm away at the fan-out:
    # the leg search could not leave its own stubs and failed at "leg 0" however the lane was planned (2026-10-24).
    pair_nets = sorted({(f'PC{n}_DP', f'PC{n}_DM') for n in (1, 2)} | {(f'H{n}_AT_DP', f'H{n}_AT_DM') for n in (1, 2)} |
                       {(f'{p}_DP', f'{p}_DM') for p in PORTS} | {(f'H{h}_{p}_DP', f'H{h}_{p}_DM') for h in (0, 1, 2) for p in PORTS} |
                       {('H0_UP_DP', 'H0_UP_DM'), ('SVC_DP', 'SVC_DM'), ('RP_DP', 'RP_DM'),
                        ('RP_USB_DP_PIN', 'RP_USB_DM_PIN')})
    B = Board(board, pair_nets)
    log = []
    print(f'grid {NX}x{NY}, {len(B.pads)} nets, setup {time.time() - t0:.1f}s', flush=True)
    global LIVE
    for a in sys.argv:
        if a.startswith('--live'):
            LIVE = Live(B, board, log, int(a.split('=')[1]) if '=' in a else 8765)
    fpr = {f.GetReference(): f for f in board.GetFootprints()}
    cache = os.environ.get('ROUTE_PAIR_CACHE')      # debugging: reuse a routed pair stage
    # 1. fixed copper
    usbc_term = {}
    for n, j in ((1, 'J101'), (2, 'J201')):
        usbc_term[j] = usbc_breakout(B, fpr[j], f'PC{n}_DP', f'PC{n}_DM')
    usbc_term['J301'] = usbc_breakout(B, fpr['J301'], 'SVC_DP', 'SVC_DM')   # the SVC pair link starts here
    # No fixed copper from the connector to U301 any more.  rev 6 laid that run as two straight single tracks
    # (usbc_to_esd: down 0.6 mm, a 45-degree jog to the ESD's pin pitch, straight into pins 3/1) because the SVC pair
    # was not routed as a pair then; rev 7 added the pair link J301 -> U301 (pair_links below), and with both in place
    # every SVC net had two parallel runs between the connector's fan-out and the ESD's north pads - measured on the
    # rev-8 board: SVC_DP 33 segments / 38.57 mm and SVC_DM 34 / 39.03 mm for the same ~10 mm gap, the fixed pair at
    # x 159.05 / 160.95 and the routed pair at x 159.85 / 160.15 (a redundant loop, invisible to the DRC and to
    # check_copper's per-net connectivity because both paths are the same net).  The pair link is the one that stays:
    # it is what length-matches the two lines and what check_usb_si.py measures as a channel.
    for esd, dm, dp in [('U101', 'PC1_DM', 'PC1_DP'), ('U201', 'PC2_DM', 'PC2_DP'), ('U301', 'SVC_DM', 'SVC_DP'),
                        ('U1201', 'KB_DM', 'KB_DP'), ('U1301', 'MS_DM', 'MS_DP'), ('U1401', 'OT_DM', 'OT_DP')]:
        esd_through(B, fpr[esd], dm, dp)
    ep_vias(B, log)
    # the bottom-side decoupling caps share their pin's via: the tie is laid here, before the fan-outs, so
    # dense_fanout() finds those plane pins already connected and gives them no second via (rev 9, 2026-10-26)
    pin_cap_links(B, log)
    qfn_fanout(B, log)
    pad_escapes(B, log)
    share_gnd_vias(B, log)          # one via for GND pads that meet pad to pad, on their signal-free side
    print(log[-1], flush=True)
    # 2. differential pairs
    def term(t):
        if t[0] == 'usbc':
            pP, pN, out = usbc_term[t[1]]
            return pP, pN, out, None
        if t[0] == 'padpair':
            return padpair_terminal(B, t[1], t[2], t[3], t[4], t[5] if len(t) > 5 else None)
        return pad_terminal(B, t[1], t[2], t[3])

    # (P, A end, B end) -> (P, N) for links still unrouted after every retry: the USB phase's verdict.  Keyed by link,
    # not by net: the two service links share SVC_DP/SVC_DM (one either side of the ESD U301), and keyed by P the second
    # link's success erased the first one's failure - measured, J301 -> U301 failed in every run from 0.12.0 on while
    # the verdict read "2 of 2 link(s) routed" (ROUTE_ONLY_PAIRS=SVC_DP, 0.12.0's own sources, 2026-11-07).
    pair_failed = {}

    def end_key(t):
        """'part.pad' for a link terminal: the pad matters where two links of one net end on the same part."""
        if t[0] == 'usbc':
            return t[1]
        return f'{t[1]}.{t[2]}'

    def link_key(link):
        return (link[0], end_key(link[2]), end_key(link[3]))

    def run(link):
        P, N, ta, tb, layers, bcost, wps = link
        t = time.time()
        why, layer = None, ''
        if bcost == 'direct':
            ok = direct_pair(B, P, N, term(ta), term(tb), log)
        else:
            bc, tw = bcost if isinstance(bcost, tuple) else (bcost, 'B')
            before = collections.Counter(x for x in B.tracks if x[0] in (P, N))
            live_ghost_clear((P, N))     # a new attempt for these nets replaces the discarded picture of the last one
            B.attempt_log = []           # everything this attempt lays, even what it takes back before returning
            ok = route_pair(B, P, N, term(ta), term(tb), log, layers=layers, b_cost=bc, twist_at=tw, wps=wps,
                            ends=(end_key(ta), end_key(tb)))
            if not ok:
                why = log[-1]
                # Last resort, and only if the result is defensible: the two lanes as single nets (split_fallback
                # measures length, skew and closest approach and refuses anything outside the budgets).  Without it a
                # link that cannot fit a pair corridor loses its copper *entirely*, so the board is missing a USB
                # connection while the summary still reports "unrouted: 0" - the hole this router had.
                if split_fallback(B, P, N, term(ta), term(tb), log):
                    ok, layer = True, 'split'
            if not ok:
                # Nothing of a failed link may stay on the board: pieces added before the mark (a retained stub, a
                # restored reservation, first-mile copper) have been seen left behind as stray stubs and half routes.
                # But only what *this attempt* added: the nets' copper from before it is another link's (the two
                # service links share SVC_DP/SVC_DM) or fixed stage-1 copper (the USB-C breakout, the ESD's
                # flow-through), and taking it away cut the service connector off its ESD whenever the switch's link
                # failed.
                _stray = list((collections.Counter(x for x in B.tracks if x[0] in (P, N)) - before).elements())
                if _stray:
                    log.append(f'  {P}/{N}: {len(_stray)} stray piece(s) of this failed link removed')
                    for _s in _stray:
                        B.tracks.remove(_s)
                    B.rebuild()
                # Everything this attempt drew - its first mile, its runs, its vias, and whatever it laid and took back
                # while rejecting the corridor - stays visible as the discarded attempt (dashed, blocking nothing) until
                # these nets are attempted again or route.
                live_ghost((P, N), _stray + list(getattr(B, 'attempt_log', None) or []), (why or log[-1]).strip())
                B.attempt_log = None
        print(f'pair {P:12} {ta[1]:>6} -> {tb[1]:<6} {"OK" if ok else "FAILED"} ({time.time() - t:.1f}s)'
              f'{" [" + layer + "]" if (ok and layer) else ""}  {log[-1] if ok else (why or log[-1])}', flush=True)
        live_result(P, ok)             # USB in the same counters and list as the signals: every *attempt* counts
        if ok and LIVE is not None and N in LIVE.unrouted:
            LIVE.unrouted.remove(N)
        if not ok:
            # Both of a failed link's nets are unrouted, and the view lists them exactly as it lists an unrouted
            # signal - so a missing USB connection can never be invisible again.
            for _n in (P, N):
                if LIVE is not None and _n not in LIVE.unrouted:
                    LIVE.unrouted.append(_n)
        # The USB phase's verdict per link: a link whose *last* attempt failed is unrouted, whatever its earlier
        # retries did, and STOP_IF_USB_FAILS stops the run on it instead of routing the rest of the board around a hole.
        if ok:
            pair_failed.pop(link_key(link), None)
        else:
            pair_failed[link_key(link)] = (P, N)
        live('USB pairs', P)
        return ok

    links = pair_links(B)
    # Every pair terminal's pads must carry the link's nets.  pair_links() names its pads by number, so a netlist change
    # it does not follow (errata.md E1: U601's pair pins changed in 0.13.0 and in 0.14.0) would route one net's line
    # onto the other net's pad - a short that only the DRC would report, after a full run.
    for _P, _N, _ta, _tb, *_ in links:
        for _t in (_ta, _tb):
            _ends = {'pads': lambda t: ((t[1], t[2], _P), (t[1], t[3], _N)),
                     'padpair': lambda t: ((t[1], t[2], _P), (t[3], t[4], _N))}.get(_t[0], lambda t: ())(_t)
            for _ref, _num, _want in _ends:
                _got = B.padobj[f'{_ref}.{_num}'][0][0].GetNetname()
                if _got != _want:
                    raise SystemExit(f'pair_links(): {_P}/{_N} ends on {_ref}.{_num}, which is {_got}, not {_want} - '
                                     f'the netlist changed: update the link')
    for _P, _N, *_rest in links:
        B.pair_of[_P] = B.pair_of[_N] = (_P, _N)      # belt and braces: pair_nets already gave the two lines one net code
    # Enforcement: no USB line may be routed as a plain signal.  Every net whose name says differential pair has to be in
    # B.pair_of, i.e. offered to the pair phase above; a USB net that is not goes through the signal phase, where rip-up
    # is free to clip one line of a pair (measured: SVC_DP was routed as a signal and then trimmed, leaving it in two
    # pieces).  The one exception below has no pair terminal at all, and is reported rather than silently allowed.
    USB_SINGLE_OK = set()                  # nothing is exempt: every USB line on the board is a pair (rev 8).  The
    #                                        RP2354B's own USB is split by its 27R series resistors R503/R504 and both
    #                                        sections are pairs too - the second through padpair_terminal(), which spans
    #                                        those two footprints.
    usb_singles = sorted(n for n in B.pads if re.match(NO_IN2, n) and n not in B.pair_of and n not in USB_SINGLE_OK)
    if usb_singles:
        print(f'USB nets with no pair link (they would be routed as plain signals): {usb_singles}', flush=True)
    from_cache = bool(cache and os.path.exists(cache))
    if from_cache:
        import pickle
        B.tracks, B.fixed, B.escape, B.escape_on = pickle.load(open(cache, 'rb'))
        B.rebuild()
        links = []
        print(f'pair stage loaded from {cache}', flush=True)
    only_pairs = os.environ.get('ROUTE_ONLY_PAIRS')   # debugging: route only these links, so one pair's geometry
    if only_pairs:                                     # (its lanes, vias, offset lines) can be tested in seconds
        keep = set(only_pairs.split(','))
        links = [l for l in links if l[0] in keep or l[1] in keep]
        print(f'  ROUTE_ONLY_PAIRS: {len(links)} link(s) {[l[0] for l in links]}', flush=True)
    # --seed=BOARD: an engineering change on a board that was ordered and verified.  The seed's copper stays, as fixed
    # copper, for every net whose pads are the seed's; only the changed nets are routed (pairs here, any other net after
    # the pair stage), and the planes are refilled.  So the revision differs from the seed in the changed links alone,
    # where a full run would re-route the whole board (the router is not deterministic, and any change cascades).
    # 0.14.0 = 0.12.0's copper with a twist in the RP and service pairs (errata.md E1).  Needs the seed's placement.
    seed = next((a.split('=', 1)[1] for a in sys.argv if a.startswith('--seed=')), None)
    reroute = set()
    if seed:
        reroute = seed_copper(B, board, seed, log)
        extra = next((a.split('=', 1)[1] for a in sys.argv if a.startswith('--reroute=')), '')
        print(log[-1], flush=True)
        links = [l for l in links if l[0] in reroute or l[1] in reroute]
    # every pair terminal keeps its fan-out and a short straight run beyond the escape point free of
    # other pairs (an earlier pair must not run along the front of a pin row it does not end in); on B.Cu
    # only the fan-out itself, so planned lanes can pass under a router's pin row
    for P, N, ta, tb, layers, bcost, wps in links:
        if bcost == 'direct':
            continue
        for t in (ta, tb):
            T = term(t)
            esc, stP, stN, _ = fanout(T)
            out = T[2]
            mid = ((T[0][0] + T[1][0]) / 2, (T[0][1] + T[1][1]) / 2)
            far = (esc[0] + out[0] * 2.0, esc[1] + out[1] * 2.0)
            B.reserve(P, (mid[0], mid[1], far[0], far[1], PAIR_FAT, 0))
            B.reserve(P, (mid[0], mid[1], esc[0], esc[1], PAIR_FAT, 1))
    # plane pins between USB pins (the hubs' VDD33 between their D+/D- pairs) get their via now, while only the
    # pair escape corridors are taken; once the pairs are in, the gap is gone. These vias are fixed.
    # The service switch's common port is the exception: its pair sits on pins 10/11 with GND (9) and +3V3 (12) as its
    # immediate neighbours, 1.2 mm apart, so the pair's own fan-out has to pass between them - and with their vias
    # pre-placed there the gap is 0.75 mm where the pair's copper plus clearance needs 0.85 mm, 0.05 mm short in *every*
    # rotation (measured: RP_DP failed in all four orientations today, always at its own fan-out).  Their vias are left
    # to the ordinary plane fan-out, which runs after the pairs and can place them round the pair's copper instead.
    BETWEEN_SKIP = {('U601', '9'), ('U601', '12')}
    between = {pn: set() for pn in PLANES}
    plane_pad = {key: pn for pn in PLANES for key, _ in B.pads[pn]}
    for P, N, ta, tb, *_ in links:
        for t in (ta, tb):
            if t[0] == 'pads':
                for num in (t[2], t[3]):
                    for nb in (int(num) - 1, int(num) + 1):
                        if f'{t[1]}.{nb}' in plane_pad and (t[1], str(nb)) not in BETWEEN_SKIP \
                                and f'{t[1]}.{nb}' not in getattr(B, 'tied_pins', ()):    # tied: has its via
                            between[plane_pad[f'{t[1]}.{nb}']].add(f'{t[1]}.{nb}')
    for pn, keys in ({} if seed else between).items():      # seeded: the seed has these vias
        if keys:
            n0 = len(B.tracks)
            plane_fanout(B, pn, log, only=keys)
            B.fixed += B.tracks[n0:]
            print(next(l for l in reversed(log) if l.startswith(f'{pn} fanout vias')) + ' (between USB pins)', flush=True)
    # Order.  The service pair (U601 -> U301) is pulled to the very front (rev 8): its corridor has to cross Hub-0's
    # north fan-out row to reach the service ESD, it is the only link with no alternative way through, and the ESD's own
    # escape sits in that row - so it picks its way while the board is still empty.  H0_KB follows: its long diagonal
    # runs through the west and key-ward space Hub-1's lanes use, so it gets the board to itself.  H0_UP used to be at
    # the front too, from when it needed a hand-built breakout off the old TSSOP part; on the rev-8 switch it is a 12 mm
    # pad-to-pad pair, so it goes last of the singles.
    singles = ([l for l in links if l[0].startswith('SVC')] +
               [l for l in links if l[0].startswith('H0_KB')] + [
        l for l in links if not l[0].startswith(('SVC', 'H0_', 'H1_KB', 'H1_MS', 'H1_OT', 'H2_KB', 'H2_MS', 'H2_OT'))] + [
        l for l in links if l[0].startswith('H0_UP')])
    for link in singles:
        run(link)
    import itertools

    def route_usb_group(hub, group):
        """One hub's pairs: every order of its links until an order routes them all."""
        for perm in itertools.permutations(group):
            oks = [run(l) for l in perm]
            if all(oks):
                break
            log.append(f'  hub {hub}: order {[l[0] for l in perm]} failed, ripping its links')
            B.rip({l[0] for l in group})
        else:
            for l in group:
                run(l)

    # The singles are routed, so their leftover reservations can go - and *then* every dense IC's via rows are
    # reserved, before any hub pair is routed (rev 9, a hard rule: a pad that needs a fan-out MUST end up with a stub and
    # a via site; SVC_SEL/U501.77, AT1_BOOT0/U1001.44 and AT1_BOOT0's neighbours were left with a stub because a pair
    # had already taken their band).  A reservation only holds space: the pair still routes, it just weaves round the
    # rows, and route_usb_group() walks its orders until all its links fit.  The hubs' own fan-outs stay after their
    # pairs - the pair lanes there are hand-set and must not be blocked.
    B.clear_reserved()
    if not seed:
        reserve_xtal_lanes(B, log)                   # before the fan-out plans, so they go round it (XTAL_LANES)
    for ref in (() if seed else ('U501', 'U1001', 'U1101', 'U601', 'U1202', 'U1302', 'U1402', 'U701', 'U801', 'U901')):
        dense_fanout(B, log, ref)                    # seeded: the seed has these fan-outs

    for hub in (() if from_cache else (1, 2, 0)):
        group = [l for l in links if l[0].startswith(f'H{hub}_') and not l[0].startswith(f'H{hub}_AT')
                 and not l[0].startswith(f'H{hub}_UP')      # H0_UP is routed first, in the singles list above
                 and not (hub == 0 and l[0].startswith('H0_KB'))]   # so is H0_KB, before any Hub-1 lane
        if hub == 0:
            # Hub-0's pairs next, on their hand-planned lanes, with the SPI buses on the AT32s weaving around
            # the rows reserved above.  The order used to be pairs, then fan-outs, then SPI: that is how a pair
            # took the AT32 south band and left SVC_SEL/AT1_BOOT0 without a via site.  A pair's corridor is fixed
            # geometry but it has the whole board to weave in; a pin has one band.  Measured on the RP's east row
            # with the old order: 43 sites; with this one the guarantee pass gives every pin a site (probe: 42 + 7
            # vias, 'no site for []').
            route_usb_group(0, group)
            if seed:                                      # the buses and their plane vias are seeded nets
                B.unreserve({nm for nm, *_ in B.tracks if nm in B.pair_of})
                continue
            # The plane pads in the buses' way get their via *before* the buses: step 3 ("plane vias first") does that
            # for every other pad, but these two buses route here, inside the pair phase, so they used to come first.
            # AT2's bus then wrapped C511.1 (+3V3, the RP's cap at (159.05, 132.12)) on three sides with a via in the
            # fourth - the pad had no via site left in any later pass (rev 9's open defect, then named C501.1).
            for pn in PLANES:
                pad_at = {k: s[0].bbox() for k, s in B.pads[pn]}
                keys = set()
                for n in (1, 2):
                    bb = [s[0].bbox() for x in ('MISO', 'CS', 'SCK', 'MOSI') for _, s in B.pads[f'AT{n}_{x}']]
                    bx0, by0 = min(b[0] for b in bb) - 3.0, min(b[1] for b in bb) - 3.0
                    bx1, by1 = max(b[2] for b in bb) + 3.0, max(b[3] for b in bb) + 3.0
                    keys |= {k for k in plane_open_pads(B, pn)
                             if bx0 <= (pad_at[k][0] + pad_at[k][2]) / 2 <= bx1 and by0 <= (pad_at[k][1] + pad_at[k][3]) / 2 <= by1}
                if keys:
                    plane_fanout(B, pn, log, only=keys)
                    print(next(l for l in reversed(log) if l.startswith(f'{pn} fanout vias')) + ' (before the SPI buses)',
                          flush=True)
            # each AT32 SPI bus: every routing order of its four lines until all four fit (they cross each other)
            for n in (2, 1):
                bus = [f'AT{n}_{s}' for s in ('MISO', 'CS', 'SCK', 'MOSI')]
                for perm in itertools.permutations(bus):
                    if all(not route_net(B, x, W_SIG, log) for x in perm):
                        log.append(f'AT{n} SPI routed in the order {list(perm)}')
                        break
                    B.rip(bus)
                else:
                    log.append(f'AT{n} SPI: no order routes all four lines')
                    route_all(B, bus, log, first=bus)
                print(log[-1], flush=True)
        else:
            route_usb_group(hub, group)
        B.unreserve({nm for nm, *_ in B.tracks if nm in B.pair_of})   # routed pairs keep no corridors
    B.clear_reserved()
    # Second pass over anything the pair phase could not route.  By here every corridor reservation is released and all
    # the fixed copper is known, so a link that failed early has room that did not exist then - and a USB link is not
    # abandoned while the board still has space for it.  The retry runs before any signal, so the pair still gets the
    # first pick of that space (a signal would otherwise take it).
    for P, N, ta, tb, layers, bcost, wps in links:
        if bcost == 'direct' or len(terminals(B, P)) <= 1 or link_key((P, N, ta, tb)) not in pair_failed:
            # ... and only a link that is *not* routed.  This pass used to re-run every link: route_pair() plans
            # from the terminals again over its own copper, and when the re-plan failed (it has to find a *new* path
            # now that the pairs routed after it have taken the old one) run() removed every piece of that net - so a
            # link that was fine came out of this pass unrouted.  Measured on the rev-9 board (tools/route.log,
            # pairs36/37): H2_MS_DP/DM routed OK at 49.45/50.07 mm with 4 vias and was then wiped by its own retry,
            # 25 of 26 links reported, and that is also why full23 and full24 left *different* nets behind.
            continue
        log.append(f'  pair retry (reservations released): {P}/{N}')
        print(log[-1], flush=True)
        run((P, N, ta, tb, layers, bcost, wps))
        live('USB pairs', P)
    if seed:
        # the new parts' plane pads get their via first, as in a full run ("plane vias first")
        for pn, keys in B.seed_plane_pads.items():
            keys &= set(plane_open_pads(B, pn))
            if keys:
                plane_fanout(B, pn, log, only=keys)
                print(next(l for l in reversed(log) if l.startswith(f'{pn} fanout vias')) + ' (new pads)', flush=True)
        rest = sorted(n for n in reroute if n not in PLANES and n not in B.pair_of and len(terminals(B, n)) > 1)
        if rest:
            # crystals first, then the --reroute nets in the order given (they make room for the changed nets, and are
            # the ones with one way to go: 0.16.0's SWD lines to J501), then the nets that end on the RP2354B in its
            # nesting order, then the rest by span
            xtal = [n for n in rest if re.match(r'.*_X(I|O|IN|OUT|O_R)$', n)]
            given = [n for n in extra.split(',') if n in rest and n not in xtal]
            on_rp = [n for n in rest if n not in xtal + given and any(r.startswith('U501.') for r, _ in B.pads[n])]
            route_all(B, rest, log, first=xtal + given + (escape_order(B, on_rp, 'U501') if on_rp else []))
            # The cut nets' leftovers: the new run joins a piece wherever it likes, so the seed's old path beyond that
            # point is a dead end now.  Trim dangling tracks and drop vias that no longer hop between layers (KiCad:
            # via_dangling), in turns, until neither finds anything - each exposes more of the other.  Only the cut
            # nets: the seed's own via tails into the planes (laid after its run's cleanup) look dangling too.
            trimmed, dropped = 0, 0
            for _ in range(20):
                others = [x for x in B.tracks if x[0] not in rest]
                n0 = len(B.tracks)
                cleanup_dangling(B, log)
                lost = [x for x in others if x not in B.tracks]
                if lost:
                    B.tracks += lost
                    B.fixed += [x for x in lost if x not in B.fixed]
                    B.rebuild()
                cut_now = n0 - len(B.tracks)                # the cut nets' tracks this pass trimmed
                trimmed += cut_now
                gone = []
                for x in [x for x in B.tracks if x[1] == 'via' and x[0] in rest]:
                    on_pad = any(s_.dist(np.array([x[2][0]]), np.array([x[2][1]]))[0] <= 0
                                 for _, sh in B.pads[x[0]] for s_ in sh)
                    layers = {int(o[2][5]) for o in B.tracks if o[0] == x[0] and o[1] == 'seg'
                              and _on_item(*x[2], ('seg', o[2]), tol=0.05)}
                    if not on_pad and len(layers) <= 1:
                        gone.append(x)
                if not gone and not cut_now:
                    break
                B.tracks = [x for x in B.tracks if x not in gone]
                B.fixed = [x for x in B.fixed if x not in gone]
                B.rebuild()
                dropped += len(gone)
            log.append(f'  seed: the cut nets\' leftovers trimmed: {trimmed} track(s), {dropped} via(s)')
        left = sorted(n for n in B.pads if n not in PLANES and len(terminals(B, n)) > 1)
        if LIVE is not None:
            LIVE.unrouted = list(left)
        log.append(f'unrouted: {len(left)} nets {left}')
        print(log[-1], flush=True)
        if left:
            raise SystemExit(f'--seed: {len(left)} net(s) unrouted, board not written (the live view shows why)')
        live('writing board', '', force=True)
        write_board(B)
        log.append(f'done: {sum(t[1] == "seg" for t in B.tracks)} segments, {sum(t[1] == "via" for t in B.tracks)} vias, '
                   f'{time.time() - t0:.0f}s')
        live('done', '', force=True)
        print('\n'.join(l for l in log if l.startswith(('  ', 'seed', 'unrouted', 'done'))))
        with open(os.path.join(HERE, 'tools', 'route.log'), 'w') as f:
            f.write('\n'.join(log) + '\n')
        return
    if cache and not os.path.exists(cache):
        import pickle
        pickle.dump((B.tracks, B.fixed, B.escape, B.escape_on), open(cache, 'wb'))
    if '--nets' in sys.argv:                          # debugging: route only these nets, no planes
        only = sys.argv[sys.argv.index('--nets') + 1].split(',')
        if os.environ.get('ROUTE_NETS_PLANES'):       # debugging: with the plane fan-out already placed
            for pn in ('GND', '+3V3'):
                plane_fanout(B, pn, log, only=set(plane_open_pads(B, pn)))
            B.release_escape(list(PLANES))
        if os.environ.get('ROUTE_NETS_BEFORE'):       # debugging: plus these nets, in the full run's order
            pre = os.environ['ROUTE_NETS_BEFORE'].split(',')
            route_all(B, pre, log, first=[n for n in POWER if n in pre])
        route_all(B, only, log, first=[n for n in POWER if n in only])
        write_board(B, zones=False)
        print('\n'.join(l for l in log if l.startswith('  ')))
        return
    if '--pairs-only' in sys.argv:
        if os.environ.get('ROUTE_DEBUG_DUMP'):
            import pickle
            pickle.dump((B.tracks, B.escape_items(), FAN_DEBUG), open(os.environ['ROUTE_DEBUG_DUMP'], 'wb'))
        write_board(B, zones=False)
        # The verdict, in this mode too: --pairs-only used to return before the USB phase's summary, so a run that
        # failed a link looked like a run that had simply stopped - the per-link status had to be scraped back out of
        # the log.  Same numbers as the full run's STOP_IF_USB_FAILS report below.  The per-link "pair ..." lines are
        # printed straight to stdout (not collected in log), so the failures come from pair_failed, which run() fills.
        _bad = sorted(f'{p}/{n}' for p, n in pair_failed.values())
        _n = len([l for l in links if l[5] != 'direct']) if links else 0
        print(f'USB pair phase: {_n - len(_bad)} of {_n} link(s) routed, {len(_bad)} failed', flush=True)
        for _b in _bad:
            print(f'  failed: {_b}', flush=True)
        if usb_singles:
            print('  USB nets with no pair link: ' + ', '.join(usb_singles), flush=True)
        print('\n'.join(log))
        return
    if STOP_IF_USB_FAILS and (pair_failed or usb_singles):
        # The USB phase is the part that has to be right.  A link that could not be routed as a pair means the board is
        # missing a USB connection, and routing the rest of it (minutes) cannot change that - it only hides the hole in
        # a "complete" board whose USB copper was clipped by rip-up.  Stop here, name the links, fix their routing.
        print(f'USB phase FAILED: {len(pair_failed)} link(s) unrouted as pairs, {len(usb_singles)} USB net(s) with no '
              f'pair link - stopping before the signal phase (STOP_IF_USB_FAILS, tools/route.py):', flush=True)
        for _p, _n in sorted(pair_failed.values()):
            print(f'  link {_p}/{_n}', flush=True)
        for _n in usb_singles:
            print(f'  USB net {_n}: no pair link and not in USB_SINGLE_OK', flush=True)
        raise SystemExit(3)
    if IN2_SIGNALS:
        B.keep_in2_under_pairs()
    # 3. plane vias first: every GND / +3V3 pad gets its via before the signals can wall it in
    live('plane vias', '', force=True)
    open_pads = {pn: plane_fanout(B, pn, log, only=set(plane_open_pads(B, pn))) for pn in ('GND', '+3V3')}
    B.release_escape(list(PLANES))
    live('signals', force=True)
    print('\n'.join(l for l in log if l.startswith(('GND', '+3V3'))), flush=True)
    # 4. power and signals
    names = [n for n in B.pads if n not in PLANES and n not in B.pair_of and len(terminals(B, n)) > 1]
    # order: full-speed USB and SPI first, then crystals, then short local nets, then every net on the densest chip
    # (the RP2354B), then USB-C pad-row signals (so wide power tracks cannot seal a connector's pad row), power, the
    # other MCU control lines, then the rest by length
    groups = [r'(H0_UP_D[PM]|SVC_D[PM]|RP_D[PM]|RP_USB_D[PM]_PIN)$',
              r'AT[12]_(MISO|MOSI|SCK|CS)$',
              r'.*_X(I|O|IN|OUT|O_R)$',
              r'CC[12]_(PWR|PC[12])$']
    on_rp = lambda n: any(r.startswith('U501.') for r, _ in B.pads[n])
    first = []
    for g in groups[:3]:
        first += [n for n in names if re.match(g, n) and n not in first and not (g == groups[2] and on_rp(n))]
    # then short local nets (a strap, a filter cap: pads within 6 mm), so the long nets that pass by go round them
    # instead of walling them in
    def span(n):
        bb = [s[0].bbox() for _, s in B.pads[n]]
        return max(b[2] for b in bb) - min(b[0] for b in bb) + max(b[3] for b in bb) - min(b[1] for b in bb)
    first += sorted((n for n in names if n not in first and n not in POWER and span(n) < 6.0), key=span)
    # then the densest chip: every other net on the RP2354B (64 pins at 0.4 mm, its crystal included) in nesting order,
    # before power and the rest crowd it
    # (the Bluetooth module's two UART lines excepted: slow lines, they route last, from the via sites the fan-out plan
    # holds for them.  Routed with the RP2354B's nets they took the crystal's corridor, and routed before power they
    # took the In2.Cu crossing of the OTHER port's pairs that +5 V needs: +5 V unrouted in three runs of 0.16.0.)
    esp = [n for n in names if n.startswith('ESP_')]
    # the SWD connector's lines (and RUN) next: the board's programming interface, with one short way each to J501;
    # 0.16.0 with the Bluetooth module's UART on pins 36/37 left RP_RUN walled in when OT_EN went first
    swd = [n for n in names if n not in first and n in ('RP_SWCLK', 'RP_SWDIO', 'RP_RUN')]
    first += escape_order(B, swd, 'U501') if swd else []
    first += escape_order(B, [n for n in names if n not in first and on_rp(n) and n not in esp], 'U501')
    first += [n for n in names if re.match(groups[3], n) and n not in first]
    # USB, SPI (routed already), crystals - and the USB data lines themselves: the pairs route in the pair phase,
    # before any signal, and no signal may rip them up or run along the front of their escape (they are not in
    # ``names``, so they are never ripped; listing them makes it explicit and keeps ``hard`` true around their pads)
    protect = [n for n in B.pads if any(re.match(g, n) for g in groups[:3])] + sorted(B.pair_of)
    # the status LED chain along the front edge before power: its data line runs the length of the edge between the
    # LED pads, where a wide +5 V rail used to sit first and cut it off. Routing the chain first plus the edge keep
    # in NetRouter (a strong per-step cost) keeps the +5 V off that strip; the +5 V may still rip a link locally to
    # reach an LED's own +5 V pad, and the retry round re-routes the link afterwards.
    first += [n for n in names if re.match(r'LED_(BUF|DATA|D[0-5])$', n) and n not in first]
    first += [n for n in POWER_ORDER + list(POWER) if n in names and n not in first]
    first += [n for n in names if re.match(r'(AT[12]_(IRQ|NRST|BOOT0)|HUB\d_(RST|VBUS)|(KB|MS|OT)_(SEL[01]|EN|FLT)|'
                                           r'LED_DATA|KEY|RP_RUN)$', n) and n not in first]
    # the rest by span, as route_all() orders them, then the Bluetooth module's lines
    def span_all(n):
        xs = [s[0].bbox()[0] for _, s in B.pads[n]]
        ys = [s[0].bbox()[1] for _, s in B.pads[n]]
        return (max(xs) - min(xs)) + (max(ys) - min(ys))
    first += sorted((n for n in names if n not in first and n not in esp), key=span_all) + [n for n in esp if n not in first]
    route_all(B, names, log, first=first, protect=protect)
    for rnd in range(2):          # nets left partly unrouted: the board around them has changed since, try again
        left = [n for n in names if len(terminals(B, n)) > 1]
        if not left:
            break
        log.append(f'retry round {rnd + 1}: {len(left)} nets {left}')
        print(log[-1], flush=True)
        B.rip(left)
        route_all(B, left, log, first=[n for n in first if n in left], protect=protect)
    # 5. plane pads still without a via (or whose fan-out was lifted for a signal): new via, else rip the signals
    live('plane repair', '', force=True)
    for pn in ('GND', '+3V3'):
        lost = plane_open_pads(B, pn)
        if lost:
            log.append(f'{pn}: {len(lost)} pads lost their via')
            open_pads[pn] = plane_fanout(B, pn, log, only=set(lost))
        if open_pads[pn]:
            plane_fix(B, pn, open_pads[pn], log)
    if IN2_SIGNALS:
        live('In2 repair', '', force=True)
        in2_repair(B, log)
    left = [n for n in names if len(terminals(B, n)) > 1]
    if left:
        # last resort: try them again with a full-board window and no search heuristic, so a roundabout route is
        # found rather than given up on
        global DESPERATE
        DESPERATE = True
        log.append(f'last resort: {len(left)} nets, full-board window, no heuristic: {left}')
        print(log[-1], flush=True)
        B.rip(left)
        route_all(B, left, log, first=left, protect=protect)
        DESPERATE = False
    cleanup_dangling(B, log)
    cleanup_dangling_vias(B, log)
    left = [n for n in names if len(terminals(B, n)) > 1]
    # A pair that did not route is exactly as unrouted as a signal, but ``names`` deliberately holds no pair nets -
    # the signal stage must not route them as singles behind the pair machinery.  So the count is measured over *all*
    # nets: without this, a whole USB link could be missing from the board and the summary (and the live view's
    # Unrouted panel) would still say "0 unrouted" and hide it.
    left = sorted(set(left) | {n for n in B.pair_of if len(terminals(B, n)) > 1})
    if LIVE is not None:
        LIVE.unrouted = list(left)
    log.append(f'unrouted: {len(left)} nets {left}')
    print(log[-1], flush=True)
    stitching(B, log)
    if IN2_SIGNALS:
        in2_islands(B, log)
    live('writing board', '', force=True)
    write_board(B)
    segs = sum(1 for t in B.tracks if t[1] == 'seg')
    vias = sum(1 for t in B.tracks if t[1] == 'via')
    log.append(f'done: {segs} segments, {vias} vias, {time.time() - t0:.0f}s')
    live('done', '', force=True)
    print('\n'.join(l for l in log if l.startswith(('  ', 'GND', '+3V3', 'exposed', 'stitch', 'dangling', 'In2', 'unrouted',
                                                    'done'))))
    with open(os.path.join(HERE, 'tools', 'route.log'), 'w') as f:
        f.write('\n'.join(log) + '\n')


if __name__ == '__main__':
    main()
