#!/usr/bin/env python3
"""
OpenDynKVM -- PCB placement generator (KiCad 10, pcbnew API).  Adapted from the OpenRower generator.

Builds kvm.kicad_pcb from the schematic's netlist (exported with kicad-cli).  The main parts are
placed from PLACEMENT below; every other part (passives, ESD, crystals' load capacitors, ...) is
placed automatically next to the IC pin it serves, radially outward from that IC, at the first
spot whose courtyard is clear.  Footprints are linked to their schematic symbols, so "Update PCB
from Schematic" reports no changes afterwards.

Run from hardware/:   uv run tools/gen_pcb.py [--force | --stackup | --models]
Refuses to touch a board that already contains tracks/vias/zones (i.e. after routing) unless
--force is given -- regenerating throws the routing away.  --stackup only rewrites the physical
stack-up (STACKUP) and the title block of the existing board, routed or not; --models only the
footprints' 3D models (tools/models_3d.json, else the library footprint's own), routed or not.
"""

import json
import math
import os
import subprocess
import sys
import tempfile

import pcbnew

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import Q, dump, find, first, parse  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCH = os.path.join(HERE, 'kvm.kicad_sch')
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
FPLIBS = {'CustomFootprints': os.path.join(HERE, 'CustomFootprints.pretty')}
SYSFP = '/usr/share/kicad/footprints'
# 3D models for the parts KiCad's 3D library has none for (EasyEDA's, placed on our footprints) and the MX key's
# switch and keycap, by LCSC part: written by tools/fetch_3d.py, which also fetches the files into 3dmodels/
MODELS_3D = os.path.join(HERE, 'tools', 'models_3d.json')

# Board: 120 x 80 mm, origin (100, 100).  Rear edge (top, y = 100): USB-C PC1 / PWR-SERVICE / PC2.
# Front edge (bottom, y = 180): USB-A Keyboard / Mouse / Other with the status LEDs; MX key in the centre.
X0, Y0, W, H = 100.0, 100.0, 120.0, 80.0
X1, Y1 = X0 + W, Y0 + H
EDGE_MIN = 1.0          # auto-placed courtyards stay this far inside the board edge
GAP = 0.35              # minimum courtyard gap for auto-placed parts: room for one track between any two

USB_C_Y = Y0 + 3.65                 # receptacle face flush with the rear edge (OpenRower geometry)
USB_A_Y = Y1 - 13.2                 # pad row; receptacle face flush with the front edge
REAR_X = {'J101': 115.0, 'J301': 160.0, 'J201': 205.0}     # USB-C centres, 45 mm pitch
PORT_C = {'KB': 125.0, 'MS': 160.0, 'OT': 195.0}           # USB-A centres, 35 mm pitch
PORT_X = {p: c - 3.5 for p, c in PORT_C.items()}           # USB-A pin 1 (VBUS); D-/D+ at +2.5/+4.5
# Side LEDs at 0 degrees: the lens (the footprint's +Y side, away from its pads) faces the front edge, 0.5 mm from it.
# Up to 0.16.0 they sat at 180 degrees, pads 0.5 mm from the edge, and shone into the board (errata E3).  The
# courtyard keeps the same span (body 1.36 mm and courtyard 1.6 mm south of the origin, 0.85 mm north of it).
LED_Y = Y1 - 0.5 - 1.36

KEY = (160.0, 153.5)                # MX key, centred left-right, right behind the middle USB-A port
KEY_CLEAR = 11.25                   # no parts within the 18 mm keycap + 2.25 mm

# Floor plan, rows from the rear edge:
#  1. three identical input macros (IN_MACRO): USB-C, ESD, USB2514B (ports south, upstream + crystal west), under
#     PC1 (Hub-1), PWR/SERVICE (Hub-0) and PC2 (Hub-2)
#  2. AT32F405 #1 | RP2354B | AT32F405 #2, mirrored about the centre line: each AT32's SPI side faces the RP
#     (AT32 #2 is AT32 #1 turned 180 degrees, so its OTGHS faces south); the RP's crystal sits beside it
#  3. the MX key, centred, right behind the middle USB-A port
#  4. three identical output macros (OUT_MACRO): USB-A with its ESD, 1:3 router and VBUS switch to its left
#  The auto-placed passives of the first macro of each kind are copied into the others (MACRO_TWINS).  Then the
#  rest: power path and service switch between the input macros, LED buffer by the key.
IN_MACRO = {   # {p} = USB-C block, {h} = hub block: (dx from the USB-C centre, y, rotation)
    'J{p}01': (0.0, USB_C_Y, 180), 'U{p}01': (0.0, 113.0, 270),      # ESD: D-/D+ run straight through it.  rev 9 puts
    #                            all three at 113.0 (1.0 mm north of the old spot): the service ESD had to go there
    #                            (see PLACEMENT['U301'] below) and the other two follow, so the three blocks stay
    #                            aligned and their connector -> ESD links are planned the same way.
    'U{h}01': (2.0, 122.0, 90), 'Y{h}01': (-9.5, 121.6, 0),      # crystal up and left of the hub: its XI/XO and the
    # PLLFILT / RBIAS pins all sit on the hub's west side, so this leaves the hub's west edge free for those three
    'C{h}09': (-3.5, 124.2, 180), 'R{h}01': (-3.5, 125.4, 180),   # PLLFILT / RBIAS caps by their pins
    # NR1 / CFG0 / CFG1 pull-downs in a row off the top-right corner (pins 19-21 there are unused), signal pad
    # down towards the pins; CRFILT cap beside its east-side pin. All clear of the hub's via fan-out (tools/route.py)
    'R{h}04': (4.3, 115.8, 90), 'R{h}02': (5.5, 115.8, 90), 'R{h}03': (6.7, 115.8, 90),
    'C{h}08': (7.75, 122.0, 0),
    # NR0 strap resistor in the hub's north-west corner, which the upstream pair now leaves free (tools/route.py
    # UPSTREAM_LANES); its signal pad faces pin 28
    'R{h}05': (-2.85, 117.85, 180),
    # crystal load caps right at the crystal: XI beside pad 1 (bottom left), XO above pad 3 (top right)
    'C{h}10': (-12.85, 122.45, 180), 'C{h}11': (-8.4, 118.6, 90),
}
OUT_MACRO = {  # {b} = port block: (dx from the USB-A centre, y, rotation)
    'J{b}01': (-3.5, USB_A_Y, 0),                                    # USB-A (origin = pin 1 = VBUS, west end)
    'U{b}01': (-13.5, USB_A_Y - 7.6, 90),                            # ESD, D- west / D+ east like the connector
    'U{b}02': (-13.5, USB_A_Y - 13.05, 180),                         # 1:3 router: D-/D+ face the ESD
    'U{b}03': (-19.0, USB_A_Y - 7.35, 0),                            # VBUS switch
    'C{b}03': (-13.0, USB_A_Y + 2.7, 90),                            # 220 uF port bulk (polymer can), in front
}
MACRO_TWINS = {1: [(3, 45.0), (2, 90.0)], 8: [(7, 45.0), (9, 90.0)], 12: [(13, 35.0), (14, 70.0)]}
AT_X = 34.0                         # AT32 #2's offset from the centre line (it sits at 160 + AT_X)
AT1_X, AT1_Y = 114.0, 139.0         # AT32F405 #1: another 10 mm left, so the PC1 USB-C block (connector, ESD, hub
                                    # and their passives) and the PWR/SERVICE block above it get vertical room. It
                                    # stays on the old row: 1 mm down would push its crystal and load caps into the
                                    # Hub-2 keyboard lane winding past them (that band is boxed in by the MX key)
AT2_Y = 138.0                       # AT32 #2: 1 mm above AT32 #1, so its OTGHS link's D+/D- swap stays above the
                                    # Hub-2 lanes (moving it down crowds those lanes the same way)
RP_X, RP_Y = 148.0, 136.0           # RP2354B: another 10 mm left, freeing the middle of the board for the PWR/SERVICE
                                    # block and the power path. Its row stays: 1 mm down would put its south fan-out,
                                    # crystal and SWD/RUN parts into the Hub-2 band above the key, where the
                                    # baseline already had 0.2 mm to spare

PLACEMENT = {
    # ref: (x, y, rotation_deg[, 'B' = bottom side])
    # row 2: AT32 #1 (OTGHS north, SPI east) | RP2354B | AT32 #2 (OTGHS south, SPI west); crystals, SWD pads, RREF
    'U1001': (AT1_X, AT1_Y, 90), 'Y1001': (AT1_X - 0.5, AT1_Y + 8.8, 0),   # crystals clear of
    'J1001': (AT1_X - 7.65, AT1_Y, 90), 'R1002': (AT1_X + 4.45, AT1_Y - 6.2, 0),
    'U1101': (160 + AT_X, AT2_Y, 270), 'Y1101': (160 + AT_X + 0.5, AT2_Y - 8.8, 0),  # the via fan-out
    'R1101': (160 + AT_X + 8.3, AT2_Y - 8.6, 90),    # ROLE strap: where AT32 #1's lands, turned with the chip
    # NRST cap and pull-up beside each AT32's NRST pin, off its SPI side (on that side they sit in the SPI bus)
    'C1009': (AT1_X + 8.6, AT1_Y + 8.55, 0), 'R1003': (AT1_X + 8.6, AT1_Y + 10.2, 180),
    'C1109': (160 + AT_X - 8.6, AT2_Y - 8.55, 180), 'R1103': (160 + AT_X - 8.6, AT2_Y - 10.2, 0),
    # AT32 crystal load caps beside their crystal's pads: XI left of pad 1 (bottom left), XO right of pad 3 (top
    # right; both crystals sit unturned)
    'C1007': (AT1_X - 4.3, AT1_Y + 8.8 + 0.85, 180), 'C1008': (AT1_X - 0.5 + 3.35, AT1_Y + 8.8 - 0.85, 0),
    'C1107': (160 + AT_X + 0.5 - 3.4, AT2_Y - 8.8 + 0.85, 180), 'C1108': (160 + AT_X + 0.5 + 3.35, AT2_Y - 8.8 - 0.85, 0),
    'J1101': (160 + AT_X + 8.0, AT2_Y, 90), 'R1102': (160 + AT_X - 4.45, AT2_Y + 6.2, 0),
    'U501': (RP_X, RP_Y, 0), 'Y501': (RP_X + 13.5, RP_Y + 5.5, 0),   # crystal east of the RP's XIN/XOUT pins (the
    # middle of its south side: below them the south fan-out, east of x 148.75 the keycap's clearance), clear of the
    # east fan-out; load caps at its pads (XIN at pad 1, XO beside pad 3).  The XOUT series R sits right below pin 31,
    # pad 1 (XOUT) north, as in Raspberry Pi's minimal design (R2 3 mm from XOUT): the long line to the crystal is the
    # 1 kOhm side, and the chip's output sees no track (rev 12; rev 11 had R502 at the crystal, 14 mm from the pin)
    'C517': (RP_X + 12.4, RP_Y + 8.4, 270), 'R502': (RP_X + 0.25, RP_Y + 7.05, 270), 'C518': (RP_X + 16.8, RP_Y + 4.65, 180),
    # USB 27R series resistors at the north-east corner, outside both pin combs, on the way to the service switch
    'R503': (144.8, 124.4, 90), 'R504': (146.0, 124.4, 90),     # rev 8: between the RP2354B's USB pins (south) and the
    # service switch's south-facing common port (north), so both RP-side pairs are short and straight.  Side by side in x
    # with pad 1 south - the RP side, which check_aux wants nearer the RP2354B's pin - and pad 2 north, facing the port.
    # Hub-0's unused port 4: its D+ / D- resistors just east of the hub's south-east corner (below pins 9 / 8 sits
    # Hub-0's Other via pair, so both lines run east along the pin row: pin 9 to the nearer one, pin 8 below it)
    'R707': (166.4, 124.3, 90), 'R708': (167.65, 124.3, 90),
    # SWD pads (SWCLK/SWDIO/RUN leave the south side eastwards) just beyond the crystal, so those lines end before
    # the Other-port lines (south-east corner pins) turn south past them
    'J501': (RP_X + 22.0, RP_Y + 5.5, 0), 'U404': (RP_X - 12.5, RP_Y - 6.0, 0),
    # RUN cap and pull-up right under the SWD pads' RUN / GND / +3V3 row (south of the AT32 #2 SPI bus, which RUN
    # cannot cross; west of them Hub-0's Other pair comes up on F.Cu, south the Hub-2 band on B.Cu)
    'C519': (RP_X + 22.6, RP_Y + 8.3, 0), 'R505': (RP_X + 24.3, RP_Y + 8.75, 90),
    # top row between the input macros: BOOTSEL; service switch and power path right of PWR/SERVICE, 2 mm further
    # down so the rear USB-C pad rows and their VBUS / CC lines keep the strip along the rear edge
    'SW501': (138.0, 105.5, 0), 'U601': (140.0, 114.0, 90),    # the service switch (TS3USB3031, rev 8)
    # The service switch sits in the open strip west of Hub-0 and north of the RP2354B (moved there 2026-10-25:
    # 15 mm left, 5 mm up, and rotated).  At 90 its two channel pairs (USB1 3/4 = Hub-0's upstream, USB2
    # 5/6 = the service connector) face **east**, straight at the service ESD (160,115) and Hub-0's upstream pins
    # (159.06,121.0/121.5), so both channel pairs leave through open copper with no wrap - which is what keeps their skew
    # in the design's 0.00-0.99 mm range (at 270 they faced west and wrapped round the chip: 23.7 / 25.8 mm with 1.63 /
    # 1.13 mm of skew, measured).  Its common port (the RP2354B side) therefore faces **west**, and the RP2354B's own USB
    # pins are south-east of here, so the RP_DP pair has to come round the chip to reach that port; the 27R series
    # resistors sit between the two ends to keep both RP-side sections as short as that allows.
    # One of Hub-0's passives has to be hand-placed because U601 now sits where the hub macro puts it: R304 (the
    # VSNS_PWR divider leg for U501.49) was auto-placed into the switch's keep-free box, so it moved 23 mm out and
    # flipped its far pad onto the RP; pinned east of the RP2354B at pin 49's level, rot 0 so the VSNS_PWR pad faces the
    # chip.  (Hub-0's R705 needs the same treatment, but it is written by the IN_MACRO loop, so its override is below
    # that loop, not here.)
    # R304 moved 1.4 mm east (2026-10-26: almost to the pad of C514) so that the RP2354B's east
    # row gets its escape corridor back: at 155.90 its pads (y 136.28-136.92) sat across pin 48's row and forced
    # AT2_NRST's track down to y >= 137.145, where AT2_IRQ's own pad row (137.40) left it 0.255 mm for a 0.30 mm
    # spacing - see PAD_ESCAPE_PATH in route.py.  Its courtyard now runs 156.33-158.28, i.e. 0.26 mm from C514's.
    'R304': (157.3, 136.6, 0),
    # C514 turned 180 degrees (rot 90 -> 270) at the same position: its GND pad (pad 2, now the south one, 159.05,136.28)
    # faces R304's GND pad (157.81,136.60) across 1.3 mm of open copper, so the two can share one GND via instead of
    # each getting its own and blocking the corridor east of the RP2354B.
    'C514': (159.05, 135.8, 270),
    'U402': (175.5, 108.0, 0), 'U401': (175.5, 115.5, 0), 'U403': (188.0, 108.0, 0), 'L401': (188.0, 114.0, 0),
    'C411': (182.5, 119.5, 0),                                       # +5 V hold-up (polymer can)
    # +5 V bulk of the TPS2121 (2 x 22 uF): the mux ring is full (its per-pin 1 uF input caps and the 100 nF/10 uF
    # output pair sit within 3 mm), so the bulk pair goes on the free strip west of the power block - 3.0 x 1.6 mm
    # courtyards, the nearest free spots (measured, rev 9)
    'C407': (170.0, 105.0, 90), 'C408': (170.0, 111.0, 90),
    'C413': (133.8, 127.35, 180),                                    # TLV73311 input 1 uF: beside its +3V3 pins (the auto
    #                                                                  pass anchored it to the buck instead, 45 mm away)
    # the RP2354B's bulk caps keep their rev-8 spots: with the per-pin 100 nF caps gone to B.Cu, the auto pass moved the
    # 10 uF to (141.45,147.55) - straight into H0_MS's corridor, and that pair then squeezed itself ("lines only
    # 0.075 mm apart ... P via (139.60,147.35) / N track (139.15,147.65)")
    'C512': (137.45, 123.9, 90), 'C516': (163.05, 135.45, 270),
    # MX key (hot-swap socket on the bottom) with its RGB LED on the bottom, under the switch's LED window
    'SW1501': (KEY[0], KEY[1], 0), 'D1501': (KEY[0], KEY[1] + 5.08, 0, 'B'),
    'U1501': (134.0, 141.4, 0),                                      # LED data buffer west of the key's corner, west of
    'C1502': (131.1, 140.4, 90), 'C1503': (131.1, 142.45, 90),       # the RP2354B's fan-out; its and the key LED's caps
    # status LEDs on the front edge, left to right: PC1, KB, MS, OT (each left of its port), PC2
    'D1502': (106.5, LED_Y, 0), 'D1506': (213.5, LED_Y, 0),
    'H1501': (104.0, 104.0, 0), 'H1502': (216.0, 104.0, 0), 'H1503': (104.0, 170.0, 0), 'H1504': (216.0, 170.0, 0),
}
for j, pb, hb in (('J101', 1, 8), ('J301', 3, 7), ('J201', 2, 9)):
    for key, (dx, y, rot) in IN_MACRO.items():
        PLACEMENT[key.format(p=pb, h=hb)] = (REAR_X[j] + dx, y, rot)
for k, p in enumerate(['KB', 'MS', 'OT']):
    for key, (dx, y, rot) in OUT_MACRO.items():
        PLACEMENT[key.format(b=12 + k)] = (PORT_C[p] + dx, y, rot)
    PLACEMENT[f'D{1503 + k}'] = (PORT_C[p] - 12.0, LED_Y, 0)      # port status LED, left of the port
# rev 8: the service switch (U601, PLACEMENT above) sits exactly where the hub macro puts its NR0 strap resistor, so
# Hub-0's copy of R{h}05 is overridden here, after the macro has written it (an entry in the literal above would be
# overwritten by the loop).  Only Hub-0 is affected - it is the one hub with a switch beside it.  The new spot is south
# of the switch, still on pin 28's side and with the NR0 pad the nearer one (check_aux), and clear of the two pairs that
# leave the switch's north side and of the RP2354B's pair thread down to R503/R504.
PLACEMENT['R705'] = (153.0, 124.5, 180)
# rev 9: Hub-0's strap pull-downs are placed in *pin* order, each on its own pin's side.  The auto-placer wrote them
# west to east as R704, R702, R703 while the pins run west to east as 25 (CFG1), 24 (CFG0), 22 (NR1), so H0_CFG1's and
# H0_CFG0's tracks had to cross in the 2 mm strip north of the hub - and that crossing is where H0_CFG1 lost its way out
# (rev-9 board: four rip-up rounds, then unrouted; routing the straps before HUB0_RST instead walled in HUB0_RST,
# H0_CFG0 and AT1_NRST).  With the order reversed the three corridors nest: pin 25 -> R703 (164.3), pin 24 -> R702
# (165.5), pin 22 -> R704 (166.7).  All three are 10k pull-downs, so only the x-order changes; each stays vertical with
# its pad 1 toward the hub (check_aux).
for _ref, _x in (('R703', 164.3), ('R702', 165.5), ('R704', 166.7)):
    PLACEMENT[_ref] = (_x, 115.8, 90)
# The service ESD (U301, written by the IN_MACRO loop above) used to move 1.0 mm SOUTH of the macro spot, and C711
# above it moves 1.6 mm north.  Both are for the two pairs that meet on the ESD, which need the crossover zones their
# two ends force (see tools/route.py pair_links - the ESD is straight-through and both of its pairs change which side
# D+ is on):
#  - the connector's pair takes the zone *north* of the ESD's escape (its own A end, the connector's 0.5 mm-pitch
#    fan-out, has no room), so that 1.6 mm + approach has to clear the connector's courtyard (south edge 108.97).
#    rev 6 measured "no pair path ... (0,164,1200)", y 108.2 at the macro spot, and moved the ESD 1.0 mm south (zone
#    at y 110.4); in rev 9 the zone sits at 109.9 and the first height that fails is 2.0 mm north of here (y 112.5).
#  - the switch's pair takes its zone at the *switch* end instead, which leaves only ~0.4 mm of headroom under Hub-0's
#    north pad row (118.61) for the pair's own escape: 1.0 mm south puts that escape at y 117.5, so the pair's edge is
#    0.9 mm clear.
# C711 (Hub-0's XO load cap) moves north because the MHL pair now leaves the switch's west side and its escape at
# (153.4,119.0) is 1.3 mm from C711's courtyard - the pair has to turn south there, and with C711 in place the turn
# folded its own lines (measured "lines only -0.325 mm apart at (153.97,118.67)").
# rev 9: 1.5 mm of that 1.0 mm goes back north.  The strip between U301's south pad edge and Hub-0's north pad row is
# the only way out for Hub-0's north-row pins, its reset line, VBUS_PC1 and the PWR port's VBUS; at the 1.0 mm spot it
# is 2.495 mm (pad edge to pad edge, 1.53 mm courtyard to courtyard) and three nets plus the SVC pair's convergence
# have to share it, which left Hub-0's pin 25 without a fan-out via (its 1 mm funnel beside the pair was HUB0_RST's way
# out too) and VBUS_PWR unable to reach U301.5 through the pair's exit stubs at all.  1.5 mm north (y 113.0) makes the
# strip 4.495 mm (3.53 mm courtyard to courtyard, measured: south pad edge 114.44 mm, Hub-0's north pad edge 118.935).
# 1.5 mm is the measured maximum, not a preference: the connector's pair needs its crossover zone in the 1.6 mm plus
# approach *below the connector's courtyard* (south edge 108.97) - its A end has no room of its own - and that zone
# shrinks 1:1 with this move.  Pairs-only measurements at rev 9: y 113.0 (1.5 mm) routes both SVC links (J301->U301 38
# cells, skew 0.59 mm; U601->U301 380 cells, skew 0.61 mm), y 112.5 (2.0 mm) fails the connector's link ("lines only
# -0.099 mm apart" at its A end), y 112.0 (2.5 mm) fails it with "no pair path" at all.  Rotating the ESD 180 degrees
# (pads 1/3 swapped) does not help: the connector's A6/A7 and B6/B7 pads are one net each on 0.5 mm pitch, so the pair
# still has to join both and the zone is still needed ("lines only -0.150 mm apart").
# The trap at U301.5 is pad-level (pads 4 and 6 0.35 mm either side, the SVC pair's stubs converging south of it), so
# the wider strip does not remove it: PAD_ESCAPE_INWARD in route.py still gives VBUS_PWR its way out of the slot.
# U301 itself now takes the macro spot (0.0, 113.0, 270) like the other two input ESDs - no override needed.
PLACEMENT['C711'] = (151.6, 117.0, 90)
# rev 8: Hub-0's port-4 pull-ups (R707/R708) move from the hub macro's north-east corner to *south* of the hub's
# port-4 pins 9/8.  Those two pins are a pair like any other (the stub to the pull-ups is what tells the hub the port is
# not removable), and in the macro spot their pair had to round the hub's south fan-out corner - the only way round being
# Hub-0's Other pair's hand lane (H0_LANES['OT']), which the group's every order lost to.  South of the pins the same
# pair is a straight 2.5 mm run down the fan-out lane at x 163.75 (tools/route.py, FANOUT_IC['U701']).  Pad 1 (the D4
# line) faces north, toward the pins it serves (check_aux), and pad 2 is the +3V3 pull-up.
# R707 (the DP line) sits *east* of R708 and not the other way round: the hub's own DP pin (9) is east of its DM pin
# (8), so this keeps the P line on the same side at both ends of the link and the pair needs no P/N swap.  The other
# way round the router has to twist the pair, and measured on the first rev-8 attempt it could not: the swap zone
# landed on Hub-0's south pad row (PAIR FAIL H0_D4_DP/H0_D4_DM no pair path from (0,515,1275) to (0,538,1278)).
PLACEMENT['R707'] = (164.55, 130.0, 270)
PLACEMENT['R708'] = (163.3, 130.0, 270)
# The SVC_SEL pull-down follows the switch: it is on the net the RP2354B drives into U601.2, and with the switch at
# (140,114) the auto-placed spot left its served pad *further* from that pin than its GND pad (check_aux: "pad on that
# net 8.32 mm, other pad 7.32 mm").  It must not sit in the common port's escape ray either - pin 2 is on the switch's
# east side but the port faces south, and at (140.2,117.2) this resistor blocked the whole RP_DP pair ("no pair path
# from (0,454,908) to (0,344,800)").  West of the ray, pad 1 (SVC_SEL) north.
PLACEMENT['R510'] = (135.5, 120.5, 270)
# Two parts of the RP2354B's decoupling ring move clear of the pairs that have to pass them.  C510 (auto-placed,
# 143.25-145.16 x 121.44-122.45) sat across the corridor the RP_DP pair needs on its way from the 27R resistors
# (145.4,122.7) round the chip to the service switch's west-facing common port escape at (136.8,114.0), and C710 (Hub-0's
# XI load cap, from the macro) pinched the same corridor from the east.  C510 moves 1.2 mm south, C710 2 mm north; each
# stays beside what it decouples (C510 on the RP2354B's north side, C710 on Hub-0's XI/XO row, still near its crystal).
PLACEMENT['C510'] = (142.5, 123.6, 180)
PLACEMENT['C710'] = (147.15, 120.5, 180)

# ── Bluetooth LE module (0.16.0, block 16; fitted on the BT variant only) ─────────────────────────────────────────────
# ESP32-C3-MINI-1 on the top side near the south-east corner, its antenna end on the east edge (x 220.0, flush;
# Espressif's placement guide: antenna at the board edge, no copper under it on any layer).  The only board edge with
# room: the north edge has the three USB-C ports, the west edge and the band north of here carry the hub lanes, the
# south edge the USB-A ports and the LEDs.  Hub-2's lanes come down x 205-209 on F.Cu to y 140 and turn south-west to
# x 199 by y 146-151, north-west of the module.  On 2026-11-08/09 it moved up (from y 158.5 to 152.5), then, for no
# overhang past the board edge, a bit down and west (to 208.5 / 156.0).  0.16.0 put it on the bottom; on 2026-11-12 it
# moved to the top, its layout mirrored (same centre, antenna still east), so that the bottom carries only
# small parts: routed by seeding 0.17.0's board and moving these nine parts (tools/route.py --move, README.md).
# Its 22 ground pads join the F.Cu GND pour instead of taking a via each (route.py POUR_PADS).
# Rotated 270: antenna east, pads 1-11 along the north side (3V3 at x 210.9, EN 206.9, IO2 209.3, IO3 208.5), pads 12-24
# along the west side (IO8 at y ESP_AT + 3.2, IO9 + 4.0 ... from the north), pads 25-35 along the south side (RXD0 at
# x 208.5, TXD0 209.3), the ground row 36-48 along the antenna end.  Its passives sit north and west of it, mirrored
# from the bottom-side layout; the two pull-ups west stand on end in the 1.3 mm between the module and the logo.
# The antenna end is at x 219.5, 0.5 mm inside the board edge.
ESP_AT = (208.5, 156.0)
_ex, _ey = ESP_AT
PLACEMENT.update({
    'U1601': (_ex, _ey, 270),
    'C1602': (_ex + 2.55, _ey - 7.6, 180),        # 100 nF at the 3V3 pad
    'R1602': (_ex + 0.5, _ey - 7.6, 180),         # IO2 pull-up, at its pad
    'R1601': (_ex - 1.7, _ey - 7.6, 180),         # EN pull-up and EN delay cap, at the EN pad
    'C1601': (_ex + 1.9, _ey - 9.25, 180),        # 10 uF on the 3V3 pad's line
    'C1603': (_ex - 1.7, _ey - 9.1, 180),
    'TP1601': (_ex - 6.4, _ey - 8.6, 180),        # the module's log (IO3)
    'R1604': (_ex - 6.7, _ey + 4.9, 90),          # RXD0 / IO9 pull-up (boot from flash), beside IO9 (pad 23)
    'R1603': (_ex - 6.7, _ey + 2.6, 270),         # IO8 pull-up, west of its pad
    'R1605': (_ex - 4.0, _ey - 8.35, 90),         # 0.18.0: GPIO9 to EN, between the log pad and the EN RC
})
# Antenna keep-out, a board-level rule area on every copper layer (route.py honours it, and the zone filler
# leaves every plane out of it - GND on F.Cu, In1.Cu and B.Cu, +3V3 on In2.Cu): no track, via or zone fill from the
# antenna's start (5.6 mm beyond the pad centre, datasheet figure 10-1) to the board edge, and 15 mm beyond the module's
# sides along the edge (Espressif's clearance) - northwards, and southwards as far as H1504's courtyard (y 166.5; the
# hole is bare, a pad there is allowed).  Relative to ESP_AT: (x0, y0, x1, y1).
ANT_KEEPOUT = (5.6, -6.6 - 15.0, 220.0 - ESP_AT[0], 166.4 - ESP_AT[1])

# no auto-placed parts here: the key, the planned USB pair corridors (see tools/route.py) and their via sites
KEEP_FREE = [
    (KEY[0] - KEY_CLEAR, KEY[1] - KEY_CLEAR, KEY[0] + KEY_CLEAR, KEY[1] + KEY_CLEAR),
    (100.5, 126.0, 105.0, 154.3),  # Hub-1 lanes down the west edge ...
    (100.5, 150.2, 110.0, 154.3),  # ... into the band (layer changes) and the keyboard router
    (111.0, 149.9, 138.8, 151.3),  # Hub-2 keyboard lane: over to the keyboard router ...
    (137.4, 143.3, 138.8, 151.3),  # ... down from the band above the key
    (138.8, 143.3, 148.75, 146.4), # Hub-2 keyboard / mouse lanes above the key, west part
    (146.3, 143.3, 147.6, 152.2),  # Hub-2 mouse lane down to the mouse router
    (171.25, 143.3, 183.3, 151.3), # Hub-2 lanes: jog east of the key, band to the hub column
    (183.3, 145.0, 209.6, 151.3),  # Hub-2 lanes and AT32 #2 link along the band
    (204.6, 126.0, 209.6, 151.3),  # Hub-2 lanes straight down from the hub
    (AT1_X - 5.3, 126.4, AT1_X + 3.0, 127.6),                 # Hub-1 -> AT32 #1 ...
    (AT1_X + 1.5, 126.4, AT1_X + 3.0, AT1_Y - 3.1),           # ... down into its OTGHS pins
    (160 + AT_X - 2.8, AT2_Y + 3.9, 160 + AT_X - 1.2, 146.1), # Hub-2 -> AT32 #2, up into its OTGHS pins
    (142.5, 125.6, 155.0, 127.6),  # Hub-0's ports face the RP2354B: their lines fan out in this gap
    # the Bluetooth module and its south and west sides, and its antenna keep-out (from ESP_AT: a box at a fixed place
    # pushed Hub-2's C907 off its spot, and the hub macro's twins C707 / C807 with it - C807 then sat on CC2_PC1's
    # 0.15.0 via, and CC1/CC2_PC1 went unrouted in three runs)
    (ESP_AT[0] - 8.5, ESP_AT[1] - 7.0, 220.0, ESP_AT[1] + 10.2),
    (ESP_AT[0] + 5.4, ESP_AT[1] - 21.8, 220.0, 166.5),
    (184.15, 149.65, 199.55, 162.45),  # the OpenDynKVM logo (LOGOS)
    (124.95, 101.4, 132.25, 110.1),    # the OSHW logo and the version
    (138.3, 112.3, 141.7, 115.7),  # the service switch U601 (rev 8): Hub-0's decoupling ring must place clear of it
]
for _x in REAR_X.values():                    # per input macro
    KEEP_FREE += [(_x - 12.5, 126.0, _x + 3.3, 129.6),   # hub ports leaving the hub
                  (_x - 4.7, 115.0, _x + 1.5, 122.0),    # ESD -> hub upstream
                  (_x - 1.7, 108.4, _x + 1.7, 112.2)]    # connector -> ESD (D+/D- straight down)
for _c in PORT_C.values():                    # per output macro: router -> ESD fan-in, connector -> ESD pair
    KEEP_FREE += [(_c - 15.2, USB_A_Y - 10.0, _c - 10.6, USB_A_Y - 1.0),    # the ESD's escape and the USB-A -> ESD
                  # pair's approach.  It starts below the port switch's east ring (its SEL0/SEL1 pull-downs belong
                  # there) and covers only the ESD, whose own escape the pair needs
                  (_c - 14.6, USB_A_Y - 3.6, _c + 1.5, USB_A_Y - 2.0)]      # connector -> ESD pair
# escape rings: no auto-placed part within this distance of a fine-pitch IC's courtyard, so every pin can
# leave (or drop to B.Cu through a via) before it meets the decoupling capacitors.  It must cover the router's
# staggered via fan-out (tools/route.py FANOUT_IC: 3.2 mm beyond the pin tips for the hubs, 3.5-4.6 mm for the
# AT32s and the RP2354B), otherwise a capacitor lands in the fan-out and the chip's nets are left trapped
ESCAPE_RING = {'U501': 4.8, 'U701': 3.8, 'U801': 3.8, 'U901': 3.8, 'U1001': 4.3, 'U1101': 4.3,
               'U1202': 1.3, 'U1302': 1.3, 'U1402': 1.3, 'U601': 1.3}   # the four USB switches (rev 8)
# wider gaps between the parts placed around these ICs / connectors (room for the tracks between them); the
# input macros' first copy (J101, U101, U801) sets the gap for all three
SPREAD = {'U501': 0.9, 'U1001': 0.5, 'U1101': 0.5, 'J101': 0.5, 'U101': 0.5, 'U801': 0.5,
          'U601': 0.5, 'U401': 0.5, 'U402': 0.5, 'U403': 0.5}
HORIZONTAL_FIRST = {'U501'}           # its parts spread sideways (the RP2354B has little room above and below)
# ── per-pin decoupling caps on the bottom side (rev 9, 2026-10-26) ────────────────────────────────────────────────────
# A dense IC's per-pin decoupling cap goes on B.Cu, *under the body* next to the pin it serves, so its loop is the pin's
# via plus the planes instead of a long trace; where the underside at that pin is taken (the exposed pad's via cluster, a
# cap already there, the body edge) it goes just outside the pin instead.
# Measured before this: the RP2354B's 11 x 100 nF sat 6.10-9.90 mm from their pins, the AT32s' 6.1-11.6 mm, the hubs'
# 5.1-8.1 mm - against 1.17-3.19 mm in Raspberry Pi's own minimal RP2350 design (datasheets/rp2350-minimal-kicad), whose
# guide asks for "a 100 nF capacitor per power pin ... placed close to the power pins".
# The chips whose per-pin caps are re-placed on B.Cu outside their fan-out ring and tied to the pin's own via.  The
# hubs and the four USB switches find no spot in this pass: their surroundings are the USB pairs' own corridors, and a
# cap put outside them lands in one - measured, Hub-1's C806 at (117.00,126.65) sat 1.03 mm from H1_OT's waypoint
# (117.75, 125.95) and the pair could not change layer there.  The switches keep their ring caps; the hubs get theirs
# radially under each VDD33 pin in the HUB_PIN_CAPS pass at the end of placement (rev 11).
PIN_CAP_CHIPS = ('U501', 'U1001', 'U1101', 'U701', 'U801', 'U901', 'U601', 'U1202', 'U1302', 'U1402',
                 'U401', 'U402')                          # the two power muxes: their input caps go at the pin too
# half-size of the exposed-pad via cluster (route.py EP_PADS: 3x3 over size-0.9 mm) plus the cap's courtyard margin
EP_KEEPOUT = {'U501': 1.55, 'U701': 2.0, 'U801': 2.0, 'U901': 2.0}
PIN_CAP_UNDER = (1.5, 1.7, 1.9, 2.1, 2.3, 2.5, 2.7)      # fallback centre distance inside the pin, only used if the
#                                                          geometry below cannot be measured
VIA_GAP = 0.6 + 2 * 0.15 + 0.2   # mm of bare laminate needed between the chip's pad and the cap's pad: the cap's via
#                                  (0.6) plus the board clearance on both sides (2 x 0.15) plus 0.2 so the router really
#                                  puts the via in there instead of looking for a cheaper site elsewhere
PIN_CAP_OUT = (1.05, 1.25, 1.45, 1.7, 2.0, 2.4, 2.9)     # ... and outside it
# "Just outside the pin" is off: on the hubs it lands in the pairs' own exit corridors.  Measured: C806 (Hub-1's +3V3
# pin cap) went to (117.00, 126.65), 1.03 mm from the waypoint (117.75, 125.95) of H1_OT's lane, and that pair then failed
# with "(117.75,125.95) blocked by ['#-9', 'H1_OT_DM']" - the cap's pad is a via keepout on every layer.  A cap that does
# not fit under the body now falls back to the old ring placement, which is proven routable.
PIN_CAP_OUT_OK = False
# ... and the hubs, which neither of those fits, get theirs radially under each VDD33 pin in a pass of their own at the
# end of placement (see main(); rev 11).  HUB_TIE_OUT: the tie via's centre past the pin tip (copper clearance 0.15 +
# via radius 0.3 - the spot the hubs' +3V3 fan-out via took anyway); HUB_TIE_GAP: bare laminate between that via and
# the cap's +3V3 pad, which keeps the pad 0.4 mm from the via centre, past legal_via()'s 0.35 mm pad keep-out
HUB_PIN_CAPS = ('U701', 'U801', 'U901')
HUB_TIE_OUT, HUB_TIE_GAP = 0.45, 0.1
# ... except two pins of Hub-0, whose caps keep their ring spots: Hub-1's Other pair crosses under Hub-0's south pin
# row on B.Cu (y 124.95) and steps north just west of the hub (route.py LANES[(1, 'OT')]).  With them under the hub the
# first full run failed that pair ("leg 7 -> (158.0, 123.6) on B.Cu | blocked by ['+3V3', 'H0_RBIAS']": C701 lay on
# the crossing, C706's tie via on the step, and the west fan-out moved H0_RBIAS's via into the only way round).  Hub-0
# is the full-speed hub; Hub-1 and Hub-2, the 480 Mb/s ones, keep all six.
HUB_PIN_CAPS_SKIP = {('U701', '5'), ('U701', '36')}
# parts that must sit on the side of the pin they serve, so they are placed before the decoupling caps of that IC
# (which otherwise fill the ring and push them to the far side of the chip)
PIN_FIRST = {'R507', 'R508',                       # AT32 BOOT0 pull-downs
             'R1001',                              # AT32 #1's ROLE strap, same crowded west/south ring
             'R806', 'C812',                       # Hub-1 reset pull-up and cap (the twins follow their spots)
             'R104', 'R105'}                       # VBUS -> VDET divider legs, beside the hub's VBUS_DET pin
# Hub-1 lane layer changes (via pairs)
KEEP_FREE_CIRCLES = [(107.5, 152.2, 1.2), (108.5, 150.8, 1.2), (143.0, 153.75, 1.2), (178.0, 153.75, 1.2)]

# auto-placement: anchor overrides (part -> footprint ref whose pins it should sit next to)
ANCHOR_REF = {}
ANCHOR_REF.update({f'C{1504 + k}': f'D{1502 + k}' for k in range(5)})   # each side LED's decoupling
ANCHOR_REF.update({'C411': 'L401', 'C412': 'L401'})                    # 3.3 V buck output caps (refs shift when block 4's
#                                                                        schematic gains a part: these were C407/C408)
ANCHOR_REF.update({'R511': 'J301', 'R512': 'J301'})                    # CC series resistors: at the connector
ANCHOR_REF.update({'R507': 'U1001', 'R508': 'U1101'})                   # BOOT0 pull-downs: at their AT32

FAB_REF_PREFIX = ('',)              # every reference on the fab layer, not silk: the board is dense, JLC needs none

# project DRC rules (written next to the board): the reverse-mount key LED's land pattern puts its pads 0.25-0.35 mm
# from its own body cut-out, inside the board's 0.5 mm edge clearance but above JLC's 0.2 mm for routed edges
DRC_RULES = """(version 1)
(rule "Key LED pads at its own cut-out"
  (condition "A.Parent == 'D1501'")
  (constraint edge_clearance (min 0.2mm)))
"""

# Physical stack-up: JLC's JLC04161H-3313 (1.6 mm, 1 oz outer / 0.5 oz inner; layer data from JLC's template, prepreg
# Er from its capabilities page).  The USB pairs' 0.15 / 0.15 mm solve to 89.5 ohm on it (analysis.md section 10);
# order it by name (jlcpcb.md).  The mask thickness is the solve's nominal; the core's Er is KiCad's FR4 default -
# no impedance on this board depends on it.
STACKUP_NAME = 'JLC04161H-3313'
STACKUP = [   # (layer, type, thickness mm, dielectric Er)
    ('F.SilkS', 'Top Silk Screen', None, None),
    ('F.Paste', 'Top Solder Paste', None, None),
    ('F.Mask', 'Top Solder Mask', 0.0127, None),
    ('F.Cu', 'copper', 0.035, None),
    ('dielectric 1', 'prepreg', 0.0994, 4.1),    # 3313 x 1
    ('In1.Cu', 'copper', 0.0152, None),          # GND
    ('dielectric 2', 'core', 1.265, 4.5),
    ('In2.Cu', 'copper', 0.0152, None),          # +3V3
    ('dielectric 3', 'prepreg', 0.0994, 4.1),    # 3313 x 1
    ('B.Cu', 'copper', 0.035, None),
    ('B.Mask', 'Bottom Solder Mask', 0.0127, None),
    ('B.Paste', 'Bottom Solder Paste', None, None),
    ('B.SilkS', 'Bottom Silk Screen', None, None),
]

SILK_TEXT = [
    ('PC1', (123.0, 106.0), 0), ('PWR / SERVICE', (171.0, 101.2), 0), ('PC2', (197.0, 106.0), 0),
    ('KEYBOARD', (PORT_C['KB'], 163.2), 0), ('MOUSE', (PORT_C['MS'], 163.2), 0), ('OTHER', (PORT_C['OT'], 163.2), 0),
]

# The board's version, printed on it (DEVELOPMENT.md "Versions"): a new major version is a different board, a minor version a
# revision of this one.  It is the version of the design being worked on: once a board is ordered, its commit is
# tagged hw-vX.Y.Z and this moves on to the next one.  kvm_proto::HARDWARE_VERSION, what the firmware reports, is the
# newest board that was ordered (0.12.0 while 0.18.0 is designed).  0.13.0 and 0.14.0 were designed and never ordered:
# 0.14.0 replaced 0.13.0 (errata.md E1), and 0.15.0 is 0.14.0's netlist routed from scratch.  0.16.0 adds the Bluetooth
# module (block 16, design.md 16), fitted or not.  0.17.0 fixes what the first 0.12.0 board showed: SVC_SEL's pull-up
# (errata.md E2) and the side LEDs' direction (E3).  0.18.0 gives the RP2354B a reset line to the module's EN (GPIO9
# through R1605), routed by a seeded run from 0.17.0.
HW_VERSION = '0.18.0'

# The logos (2026-11-08: reworked for 0.16.0, whose Bluetooth module took the badge's corner).  Before, one badge
# bottom right (2026-11-03): the OpenDynKVM logo over the open source hardware logo and the version.  The board has no
# other part-free 15 x 19 mm spot on top, so they are apart now: the OpenDynKVM logo behind the OTHER port, beside the
# module (x 184.5-199.2, y 150-162 is free of parts, and 0.15.0's copper had one signal via there, VBUS_OT's); the OSHW
# logo with the version at the rear, between the PC1 port and BOOTSEL (only ground stitching vias there in 0.15.0).
# West of the key was the first try: it took the vias of the lines around the keyboard and mouse switches.
# Each keeps vias out of its print with a rule area on every copper layer (route.py honours it).  Absolute positions;
# a keep-out is (x0, y0, x1, y1).
LOGOS = [('LOGO1', 'CustomFootprints:OpenDynKVM_Logo_14mm_SilkScreen', (191.85, 156.05), (184.15, 149.65, 199.55, 162.45)),
         ('LOGO2', 'Symbol:OSHW-Logo2_7.3x6mm_SilkScreen', (128.6, 104.5), (124.95, 101.4, 132.25, 110.1))]
VERSION_AT = (128.6, 109.3)


def export_netlist():
    tmp = tempfile.NamedTemporaryFile(suffix='.net', delete=False).name
    subprocess.run(['kicad-cli', 'sch', 'export', 'netlist', '--format', 'kicadsexpr', '-o', tmp, SCH],
                   check=True, capture_output=True)
    return parse(open(tmp).read())


def load_fp(fpid):
    lib, name = fpid.split(':')
    path = FPLIBS.get(lib, os.path.join(SYSFP, lib + '.pretty'))
    fp = pcbnew.FootprintLoad(path, name)
    if fp is None:
        sys.exit(f'footprint {fpid} not found')
    return fp


def models_3d():
    return json.load(open(MODELS_3D)) if os.path.exists(MODELS_3D) else {}


def set_models(fp, models, lib_fp=None):
    """The footprint's 3D models: tools/models_3d.json's for its LCSC part, else (with lib_fp) the library
    footprint's own; a footprint that is in neither keeps what it has."""
    lcsc = fp.GetFieldText('LCSC') if fp.HasField('LCSC') else ''
    if lcsc in models:
        want = []
        for m in models[lcsc]['models']:
            model = pcbnew.FP_3DMODEL()
            model.m_Filename = m['file']
            model.m_Offset = pcbnew.VECTOR3D(*m['offset'])
            model.m_Rotation = pcbnew.VECTOR3D(*m['rotate'])
            want.append(model)
    elif lib_fp is not None:
        want = list(lib_fp.Models())
    else:
        return
    fp.Models().clear()
    for model in want:
        fp.Add3DModel(model)


def write_models():
    """Put the 3D models (set_models) on the existing board, routed or not; nothing else on it changes."""
    models = models_3d()
    board = pcbnew.LoadBoard(PCB)
    for fp in board.GetFootprints():
        fpid = fp.GetFPID()
        lib = fpid.GetLibNickname().wx_str()
        lib_fp = load_fp(f'{lib}:{fpid.GetLibItemName().wx_str()}') if lib else None
        set_models(fp, models, lib_fp)
    pro = PCB.replace('.kicad_pcb', '.kicad_pro')
    keep = open(pro).read()          # SaveBoard rewrites the project file; keep ours
    pcbnew.SaveBoard(PCB, board)
    with open(pro, 'w') as f:
        f.write(keep)
    return sum(len(fp.Models()) for fp in board.GetFootprints())


CONTENT_TAGS = {'footprint', 'segment', 'arc', 'via', 'zone', 'net', 'group', 'generated', 'dimension',
                'image', 'table', 'target'}


def base_board():
    """Existing board with only its setup kept, or a new 4-layer board."""
    if os.path.exists(PCB):
        tree = parse(open(PCB).read())
        routed = any(isinstance(n, list) and n and n[0] in ('segment', 'via', 'zone', 'arc') for n in tree)
        if routed and '--force' not in sys.argv:
            sys.exit('board already has tracks/zones (routed?) -- refusing to regenerate; use --force')
        keep = [n for n in tree if not (isinstance(n, list) and n and
                                        (n[0] in CONTENT_TAGS or str(n[0]).startswith('gr_')))]
        tmp = tempfile.NamedTemporaryFile(suffix='.kicad_pcb', delete=False).name
        with open(tmp, 'w') as f:
            f.write(dump(keep) + '\n')
        return pcbnew.LoadBoard(tmp)
    board = pcbnew.BOARD()
    board.SetCopperLayerCount(4)
    board.SetLayerName(pcbnew.In1_Cu, 'In1.Cu')
    board.SetLayerName(pcbnew.In2_Cu, 'In2.Cu')
    ds = board.GetDesignSettings()
    ds.SetBoardThickness(pcbnew.FromMM(1.6))
    return board


def write_stackup():
    """Put STACKUP into the board's setup and STACKUP_NAME into its title block.  pcbnew's Python API cannot reach
    the stack-up, so the setup is edited as an S-expression and the board then loaded and saved by pcbnew, which
    keeps the file in KiCad's own format; nothing else on the board changes."""
    node = ['stackup']
    for name, kind, thick, er in STACKUP:
        layer = ['layer', Q(name), ['type', Q(kind)]]
        if thick is not None:
            layer.append(['thickness', thick])
        if er is not None:
            layer += [['material', Q('FR4')], ['epsilon_r', er], ['loss_tangent', 0.02]]
        node.append(layer)
    node += [['copper_finish', Q('ENIG')], ['dielectric_constraints', 'yes']]
    tree = parse(open(PCB).read())
    setup = first(tree, 'setup')
    setup[:] = [setup[0], node] + [n for n in setup[1:] if not (isinstance(n, list) and n and n[0] == 'stackup')]
    tmp = tempfile.NamedTemporaryFile(suffix='.kicad_pcb', delete=False).name
    with open(tmp, 'w') as f:
        f.write(dump(tree) + '\n')
    board = pcbnew.LoadBoard(tmp)
    tb = board.GetTitleBlock()
    tb.SetTitle('OpenDynKVM')
    tb.SetComment(0, f'Stack-up {STACKUP_NAME}, impedance controlled (USB pairs 90 ohm differential), ENIG')
    board.SetTitleBlock(tb)
    board.SetFileName(PCB)
    pro = PCB.replace('.kicad_pcb', '.kicad_pro')
    keep = open(pro).read()          # SaveBoard rewrites the project file; keep ours
    pcbnew.SaveBoard(PCB, board)
    with open(pro, 'w') as f:
        f.write(keep)


# ── geometry helpers for auto-placement ───────────────────────────────────────
def bbox(fp):
    # the courtyard lives on the part's own side (a footprint flipped to B.Cu has none on F.CrtYd), so ask for that one
    # first and fall back to the bare bounding box only if the footprint has no courtyard at all
    order = (pcbnew.F_CrtYd, pcbnew.B_CrtYd) if fp.GetLayer() == pcbnew.F_Cu else (pcbnew.B_CrtYd, pcbnew.F_CrtYd)
    r = None
    for lay in order:
        r = fp.GetCourtyard(lay).BBox()
        if r.GetWidth() > 0:
            break
    if r is None or r.GetWidth() == 0:
        r = fp.GetBoundingBox(False)
    return (pcbnew.ToMM(r.GetLeft()), pcbnew.ToMM(r.GetTop()), pcbnew.ToMM(r.GetRight()), pcbnew.ToMM(r.GetBottom()))


def overlaps(a, b, gap=GAP):
    return not (a[2] + gap <= b[0] or b[2] + gap <= a[0] or a[3] + gap <= b[1] or b[3] + gap <= a[1])


def box_ok(bb, placed, gap=GAP):
    if bb[0] < X0 + EDGE_MIN or bb[1] < Y0 + EDGE_MIN or bb[2] > X1 - EDGE_MIN or bb[3] > Y1 - EDGE_MIN:
        return False
    for k in KEEP_FREE:
        if overlaps(bb, k, 0):
            return False
    for cx, cy, r in KEEP_FREE_CIRCLES:
        nx, ny = min(max(cx, bb[0]), bb[2]), min(max(cy, bb[1]), bb[3])
        if math.hypot(nx - cx, ny - cy) < r:
            return False
    return not any(overlaps(bb, o, gap) for o in placed)


POWER_NETS = {'GND', '+3V3', '+5V', '+1V1'}


def main():
    if '--stackup' in sys.argv:
        write_stackup()
        print(f'wrote the {STACKUP_NAME} stack-up into {PCB}')
        return
    if '--models' in sys.argv:
        n = write_models()
        print(f'wrote {n} 3D models into {PCB}')
        return
    models = models_3d()
    board = base_board()
    net = export_netlist()
    pinnet = {}
    for n in find(first(net, 'nets'), 'net'):
        name = first(n, 'name')[1].lstrip('/')
        for nd in find(n, 'node'):
            pinnet[(first(nd, 'ref')[1], first(nd, 'pin')[1])] = name

    netinfo = {}

    def get_net(name):
        if name not in netinfo:
            ni = pcbnew.NETINFO_ITEM(board, name)
            board.Add(ni)
            netinfo[name] = ni
        return netinfo[name]

    comps = []
    for comp in find(first(net, 'components'), 'comp'):
        ref = first(comp, 'ref')[1]
        fpid = first(comp, 'footprint')[1]
        fp = load_fp(fpid)
        fp.SetFPID(pcbnew.LIB_ID(*fpid.split(':')))
        fp.SetReference(ref)
        ds = first(comp, 'datasheet')
        desc = first(comp, 'description')
        fp.SetField('Datasheet', ds[1] if ds else '')
        fp.SetField('Description', desc[1] if desc else '')
        fp.GetField('Datasheet').SetVisible(False)
        fp.GetField('Description').SetVisible(False)
        fp.SetValue(first(comp, 'value')[1])
        tstamps = first(comp, 'tstamps')
        fp.SetPath(pcbnew.KIID_PATH('/' + tstamps[1]))
        fp.SetSheetname('/')
        fp.SetSheetfile(os.path.basename(SCH))
        for f in find(comp, 'property'):
            key, val = first(f, 'name')[1], (first(f, 'value') or [None, ''])[1]
            if key in ('LCSC', 'MPN', 'Fit') and val:
                fp.SetField(key, val)
                fp.GetField(key).SetVisible(False)
            # the symbol's BOM / placement-file exclusion goes onto the footprint (today only the Tag-Connect pads and
            # the mounting holes are excluded): a library footprint that does not carry it already otherwise disagrees
            # with its symbol - rev 9's one footprint_symbol_mismatch in --schematic-parity was D1501, from KiCad's
            # LED_SMD library, while it was still a hand-fitted part
            if key == 'exclude_from_bom':
                fp.SetAttributes(fp.GetAttributes() | pcbnew.FP_EXCLUDE_FROM_BOM)
            if key == 'exclude_from_pos_files':
                fp.SetAttributes(fp.GetAttributes() | pcbnew.FP_EXCLUDE_FROM_POS_FILES)
        set_models(fp, models)
        if ref.startswith(FAB_REF_PREFIX):
            fp.Reference().SetLayer(pcbnew.F_Fab)
            fp.Reference().SetTextSize(pcbnew.VECTOR2I_MM(0.5, 0.5))
            fp.Reference().SetTextThickness(pcbnew.FromMM(0.08))
            fp.Reference().SetPosition(pcbnew.VECTOR2I(0, 0))
        for pad in fp.Pads():
            name = pinnet.get((ref, pad.GetNumber()))
            if name:
                pad.SetNet(get_net(name))
        comps.append((ref, fp))

    extra = set(PLACEMENT) - {r for r, _ in comps}
    if extra:
        sys.exit(f'PLACEMENT has refs not in the schematic: {sorted(extra)}')

    placed_boxes = []
    fixed = {}
    for ref, fp in comps:
        if ref in ESCAPE_RING:
            x, y = PLACEMENT[ref][:2]
            fp.SetPosition(pcbnew.VECTOR2I_MM(x, y))
            fp.SetOrientationDegrees(PLACEMENT[ref][2])
            r, (bx0, by0, bx1, by1) = ESCAPE_RING[ref], bbox(fp)
            KEEP_FREE.append((bx0 - r, by0 - r, bx1 + r, by1 + r))
        if ref in PLACEMENT:
            x, y, rot = PLACEMENT[ref][:3]
            fp.SetPosition(pcbnew.VECTOR2I_MM(x, y))
            fp.SetOrientationDegrees(rot)
            board.Add(fp)                          # Flip() needs the board (segfaults without)
            if PLACEMENT[ref][3:] == ('B',):
                fp.Flip(fp.GetPosition(), pcbnew.FLIP_DIRECTION_LEFT_RIGHT)
            placed_boxes.append(bbox(fp))
            fixed[ref] = fp

    # ── auto-placement ────────────────────────────────────────────────────────
    def pads_of(fp):
        return {p.GetNumber(): p for p in fp.Pads()}

    def block_of(ref):
        digits = ''.join(ch for ch in ref if ch.isdigit())
        return int(digits) // 100

    used_power_pins = {}

    def anchors_for(ref, fp):
        """Every pin the part could serve, best first: the same anchors anchor_for picked, but the caller can try
        them in turn, so a decoupling cap whose pin's side is full lands beside another pin of the same supply
        instead of 10 mm away."""
        nets = {p.GetNetname() for p in fp.Pads() if p.GetNetname()}
        sig = [n for n in nets if n not in POWER_NETS]
        blk = block_of(ref)
        cands = []
        for oref, ofp in fixed.items():
            if ANCHOR_REF.get(ref) and oref != ANCHOR_REF[ref]:
                continue
            for p in ofp.Pads():
                n = p.GetNetname()
                if not n:
                    continue
                if sig and n in sig:
                    cands.append((0 if block_of(oref) == blk else 1, 0, oref, p, n))
                elif not sig and n in nets and n != 'GND':
                    used = used_power_pins.get((oref, p.GetNumber()), 0)
                    cands.append((0 if block_of(oref) == blk else 1, used, oref, p, n))
        if not cands:
            return []
        cands.sort(key=lambda c: (c[0], c[1], c[2] != f'U{blk}01'))
        return [(c[2], c[3], c[4]) for c in cands]

    def anchor_for(ref, fp):
        a = anchors_for(ref, fp)
        return a[0] if a else None

    def place_near(fp, anchor, shifts=(), limit=None):
        oref, apad, anet = anchor
        ofp = fixed[oref]
        gap = SPREAD.get(oref, GAP)
        c = ofp.GetPosition()
        cx, cy = pcbnew.ToMM(c.x), pcbnew.ToMM(c.y)
        pp = apad.GetPosition()
        px, py = pcbnew.ToMM(pp.x), pcbnew.ToMM(pp.y)
        dx, dy = px - cx, py - cy
        if abs(dx) >= abs(dy):
            n = (1.0 if dx > 0 else -1.0, 0.0)
        else:
            n = (0.0, 1.0 if dy > 0 else -1.0)
        t = (-n[1], n[0])
        # rotate so the pad on the anchor's net points back at the anchor (radial orientation)
        mypads = [p for p in fp.Pads() if p.GetNetname() == anet] or list(fp.Pads())
        best = None
        for rot in (0, 90, 180, 270):
            fp.SetOrientationDegrees(rot)
            fp.SetPosition(pcbnew.VECTOR2I_MM(0, 0))
            q = mypads[0].GetPosition()
            qx, qy = pcbnew.ToMM(q.x), pcbnew.ToMM(q.y)
            score = qx * n[0] + qy * n[1]          # most negative = pointing back at the anchor
            if best is None or score < best[0]:
                best = (score, rot)
        fp.SetOrientationDegrees(best[1])
        obb = bbox(ofp)
        # a part that also connects somewhere else (series / pull resistor, divider, filter) goes on the side of
        # the anchor pin that faces that other connection, but only a *near* one: taking every pad on the net (GND
        # is everywhere) would point at the board centre and put the part on the far side of the chip
        FAR_R = 4.0
        far = [q for nn in {q.GetNetname() for q in fp.Pads()} - POWER_NETS - {''}
               for r2, q in fixed_pads.get(nn, ())
               if not (r2 == oref and q.GetNumber() == apad.GetNumber())
               and math.hypot(pcbnew.ToMM(q.GetPosition().x) - px, pcbnew.ToMM(q.GetPosition().y) - py) <= FAR_R]
        dirs = [n, t, (-t[0], -t[1])]
        if far:
            tx = sum(pcbnew.ToMM(q.GetPosition().x) for q in far) / len(far) - px
            ty = sum(pcbnew.ToMM(q.GetPosition().y) for q in far) / len(far) - py
            dirs.sort(key=lambda u: -(u[0] * tx + u[1] * ty))
        # then the other sides (at a board edge or in a crowd), inward last.  Nearest spot first: candidates are
        # ordered by their real distance from the anchor pin, ties broken by side, so a part never lands far out on
        # its preferred side while a closer spot is free on another, and never on the far side of the chip while a
        # closer spot on the pin's own side is free
        if oref in HORIZONTAL_FIRST:
            dirs.sort(key=lambda u: u[0] == 0)          # east / west before north / south (stable otherwise)
        cand = dirs + [(-n[0], -n[1])]
        edge = {(1.0, 0.0): obb[2] - px, (-1.0, 0.0): px - obb[0], (0.0, 1.0): obb[3] - py, (0.0, -1.0): py - obb[1]}
        offsets = [0.0] + [v for k in range(1, 14) for v in (0.55 * k, -0.55 * k)]
        spots = []
        for k in range(24):
            for order, dirn in enumerate(cand):
                d = max(edge[dirn], 0.0) + 0.9 + 0.5 * k
                for s in offsets:
                    spots.append((math.hypot(d, s), order, dirn, d, s))
        spots.sort(key=lambda q: (round(q[0], 3), q[1]))
        for _, _, dirn, d, s in spots:
            tt = (-dirn[1], dirn[0])
            x = px + dirn[0] * d + tt[0] * s
            y = py + dirn[1] * d + tt[1] * s
            fp.SetPosition(pcbnew.VECTOR2I_MM(round(x / 0.05) * 0.05, round(y / 0.05) * 0.05))
            # of the rotations that leave the part on the pin's side, take the one whose pad on the anchor's net ends
            # up nearest the anchor pin, so the connection never runs to the far end of the part
            rots = []
            for dr in (0, 180, 90, 270):
                fp.SetOrientationDegrees((best[1] + dr) % 360)
                q = mypads[0].GetPosition()
                rots.append((math.hypot(pcbnew.ToMM(q.x) - px, pcbnew.ToMM(q.y) - py), dr))
            for _, dr in sorted(rots):
                fp.SetOrientationDegrees((best[1] + dr) % 360)
                bb = bbox(fp)
                if box_ok(bb, placed_boxes, gap) and \
                        all(box_ok((bb[0] + dx, bb[1], bb[2] + dx, bb[3]), placed_boxes, gap) for dx in shifts):
                    if os.environ.get('GENPCB_DEBUG') == ref:
                        q = mypads[0].GetPosition()
                        print(f'    {ref} -> {oref}.{apad.GetNumber()} dirn={dirn} rot={fp.GetOrientationDegrees()} '
                              f'padsig=({pcbnew.ToMM(q.x):.2f},{pcbnew.ToMM(q.y):.2f}) '
                              f'd={math.hypot(pcbnew.ToMM(q.x) - px, pcbnew.ToMM(q.y) - py):.2f}', flush=True)
                    if limit is not None and math.hypot(x - px, y - py) > limit:
                        return None        # no spot right beside this pin: the caller tries another pin of the supply
                    return bb
        return None

    fixed_pads = {}                       # net -> [(ref, pad)] on the hand-placed parts
    for r2, f2 in fixed.items():
        for q in f2.Pads():
            if q.GetNetname():
                fixed_pads.setdefault(q.GetNetname(), []).append((r2, q))
    twin_of = {d: (src, dx) for src, lst in MACRO_TWINS.items() for d, dx in lst}
    byref = dict(comps)

    def source_of(ref):                   # the part in the first macro that this one is a copy of, if any
        if block_of(ref) not in twin_of:
            return None
        head = ref.rstrip('0123456789')
        src = f'{head}{twin_of[block_of(ref)][0]}{ref[len(head):][-2:]}'
        return src if src in byref else None
    auto = [(r, f) for r, f in comps if r not in PLACEMENT and source_of(r) is None]

    # ── per-pin decoupling caps onto B.Cu, beside the pin they serve (rev 9) ────────────────────────────────────────
    # One cap per supply pin of the dense ICs listed in PIN_CAP_CHIPS, flipped to the bottom and put as close to its pin
    # as the geometry allows: first *under* the body (centre distance PIN_CAP_UNDER inside the pin), then just outside it
    # (PIN_CAP_OUT).  The cap's pad on the supply net is turned towards the pin, so the connection is the pin's own
    # fan-out via plus the planes - a few hundred um of track at most - instead of the 5-12 mm F.Cu run the old ring
    # placement produced.  A cap that fits neither goes back to the normal auto-placement.  Every chip is handled on its
    # own (the macro copies too): their geometries are identical, so each gets the same arrangement.
    pin_caps = {}
    pin_pairs = {}                                 # cap -> the pin it decouples; tools/route.py ties them together
    placed_refs = set()
    for cref in PIN_CAP_CHIPS:
        cfp = fixed.get(cref)
        if cfp is None:
            continue
        c = cfp.GetPosition()
        cx, cy = pcbnew.ToMM(c.x), pcbnew.ToMM(c.y)
        body = bbox(cfp)
        kep = EP_KEEPOUT.get(cref, 0.0)
        # the two power muxes only get their *input* caps at the pin: their +5 V output caps are 22 uF bulk and belong in
        # the open (in the crowded power block they have nowhere to go under the part, and taking their spots parked
        # them off the board)
        vbus_only = cref in ('U401', 'U402')
        pads = [p for p in cfp.Pads()
                if p.GetNetname() and p.GetNetname() != 'GND'
                and (p.GetNetname().startswith('VBUS') if vbus_only else p.GetNetname() in POWER_NETS)]
        # scarce supplies first (+1V1 has 3 pins, +3V3 11 on the RP2354B): they have the fewest pins to sit at
        pads.sort(key=lambda p: (sum(1 for q in cfp.Pads() if q.GetNetname() == p.GetNetname()), p.GetNumber()))
        for pin in pads:
            net = pin.GetNetname()
            cand = [(r, f) for r, f in comps if r not in PLACEMENT and r not in placed_refs
                    and block_of(r) == block_of(cref)
                    and {q.GetNetname() for q in f.Pads()} == {net, 'GND'}]
            if not cand:
                continue
            # the per-pin 100 nF first: a bulk 1 uF/4.7 uF/10 uF belongs in the open, not squeezed under a body
            cand.sort(key=lambda rf: (0 if rf[1].GetValue().lower().replace('µ', 'u') in ('100nf', '100n', '0.1uf')
                                      else 1, rf[0]))
            ref, fp = cand[0]
            pp = pin.GetPosition()
            px, py = pcbnew.ToMM(pp.x), pcbnew.ToMM(pp.y)
            dx, dy = px - cx, py - cy
            n = ((1.0, 0.0) if dx > 0 else (-1.0, 0.0)) if abs(dx) >= abs(dy) else \
                ((0.0, 1.0) if dy > 0 else (0.0, -1.0))
            rail_pad = next(q for q in fp.Pads() if q.GetNetname() == net)
            board.Add(fp)                                     # KiCad only flips a footprint that is on a board
            fp.Flip(fp.GetPosition(), False)                  # bottom side; the pads keep their nets
            # the rotation whose supply pad points at the pin (place_near's trick, on the flipped part)
            best = None
            for rot in (0, 90, 180, 270):
                fp.SetOrientationDegrees(rot)
                fp.SetPosition(pcbnew.VECTOR2I_MM(0, 0))
                q = rail_pad.GetPosition()
                score = pcbnew.ToMM(q.x) * n[0] + pcbnew.ToMM(q.y) * n[1]
                if best is None or score > best[0]:
                    best = (score, rot)
            rot0 = best[1]
            fp.SetOrientationDegrees(rot0)
            fp.SetPosition(pcbnew.VECTOR2I_MM(0, 0))
            # the first distance that leaves VIA_GAP of bare laminate between the chip's pad and the cap's own pad, so the
            # cap's via has a home exactly between them (measured on rev 9: at a flat 1.05 mm the pads of U501 lost 0.8 mm
            # wide pads and the cap's 0.56 mm pads left only 0.34 mm of laminate - no room, so the via went to the fan-out
            # ring and its track ran back out)
            # UNDER the body, with VIA_GAP of bare laminate between the chip's pad and the cap's own pad: that gap is
            # where the cap's via goes, so the tie is pin -> 0.8 mm -> via -> 0.7 mm -> cap pad, all inside the package
            # outline (rev 9, 2026-10-26: measured gap 1.09 mm on the RP2354B and 1.11 mm on the AT32s, and the caps
            # fitted; the fan-out pins that lost their sites in those runs were unrelated to the caps - it was pairs
            # dropping vias inside the QFN interior, see route.py's PIN_VIA_FREE / legal_via)
            pb = pin.GetBoundingBox()
            pin_half = 0.5 * (pcbnew.ToMM(pb.GetWidth()) * abs(n[0]) + pcbnew.ToMM(pb.GetHeight()) * abs(n[1]))
            q = rail_pad.GetPosition()
            pad_off = abs(pcbnew.ToMM(q.x) * n[0] + pcbnew.ToMM(q.y) * n[1])
            rb = rail_pad.GetBoundingBox()
            cap_half = 0.5 * (pcbnew.ToMM(rb.GetWidth()) * abs(n[0]) + pcbnew.ToMM(rb.GetHeight()) * abs(n[1]))
            d0 = round((pin_half + cap_half + VIA_GAP + pad_off) / 0.05) * 0.05
            under_dists = tuple(round(d0 + 0.2 * k, 2) for k in range(len(PIN_CAP_UNDER)))
            spot, how = None, ''
            for d in under_dists:
                x, y = px - n[0] * d, py - n[1] * d
                fp.SetOrientationDegrees(rot0)
                fp.SetPosition(pcbnew.VECTOR2I_MM(round(x / 0.05) * 0.05, round(y / 0.05) * 0.05))
                bb = bbox(fp)
                if not (bb[0] >= body[0] and bb[1] >= body[1] and bb[2] <= body[2] and bb[3] <= body[3]):
                    continue                              # under the body
                if kep and not (bb[2] <= cx - kep or bb[0] >= cx + kep
                                or bb[3] <= cy - kep or bb[1] >= cy + kep):
                    continue                              # over the exposed pad's via cluster
                if bb[0] < X0 + EDGE_MIN or bb[1] < Y0 + EDGE_MIN or bb[2] > X1 - EDGE_MIN                         or bb[3] > Y1 - EDGE_MIN:
                    continue
                if any(overlaps(bb, bbox(f), GAP) for r2, f in comps
                       if f is not fp and f.GetLayer() == pcbnew.B_Cu):
                    continue
                spot, how = bb, 'under'
                break
            if spot is None:
                fp.Flip(fp.GetPosition(), False)             # put it back: the normal pass will place it
                board.Remove(fp)
                continue
            placed_refs.add(ref)
            pin_caps[ref] = spot
            pin_pairs[ref] = {'chip': cref, 'pin': pin.GetNumber(), 'net': net}
            if os.environ.get('GENPCB_PINCAPS'):
                q = rail_pad.GetPosition()
                print(f'  pin cap {ref} {fp.GetValue()} -> {cref}.{pin.GetNumber()} {net} '
                      f'({pcbnew.ToMM(fp.GetPosition().x):.2f},{pcbnew.ToMM(fp.GetPosition().y):.2f}) {how} '
                      f'pad {math.hypot(pcbnew.ToMM(q.x) - px, pcbnew.ToMM(q.y) - py):.2f} mm from the pin',
                      flush=True)
    pin_caps_bottom = list(pin_caps.values())      # B.Cu: kept out of placed_boxes (that list is the top side)
    if pin_pairs:                                  # the router reads this and wires each pin straight to its cap
        pair_path = os.path.join(HERE, 'tools', 'build', 'pin_caps.json')
        os.makedirs(os.path.dirname(pair_path), exist_ok=True)
        with open(pair_path, 'w') as fh:
            json.dump(pin_pairs, fh, indent=1, sort_keys=True)
    auto = [rf for rf in auto if rf[0] not in pin_caps]
    if pin_caps:
        print(f'per-pin decoupling caps on B.Cu: {len(pin_caps)}', flush=True)

    # the chips' pads per net: a supply with only a couple of pins on a chip (the RP2354B's +1V1 / RP_VREG, fed by
    # the external LDO) has no ring to spread along, so its caps must take the nearest spots before the +3V3 caps
    # (13 pins on the same chip) fill the whole ring and push them 9-11 mm out
    chip_pads = {}
    for ref2, fp2 in comps:
        if ref2 not in ESCAPE_RING:
            continue
        for q in fp2.Pads():
            if q.GetNetname():
                chip_pads[q.GetNetname()] = chip_pads.get(q.GetNetname(), 0) + 1

    def scarce(rf):
        nets = {p.GetNetname() for p in rf[1].Pads()} - {'GND', ''}
        return min((chip_pads.get(n, 99) for n in nets), default=99)

    twinned = lambda rf: block_of(rf[0]) in MACRO_TWINS
    supply = lambda rf: {p.GetNetname() for p in rf[1].Pads()} <= POWER_NETS
    # the first macro of each kind first (its spots must also be free in its copies), then the rest; decoupling
    # first in each group (it wants the closest spots), the constrained supplies first among those, then schematic order
    auto.sort(key=lambda rf: (0 if twinned(rf) else 1,
                              0 if rf[0] in PIN_FIRST else 1,
                              0 if supply(rf) else 1,
                              scarce(rf) if supply(rf) else 0, block_of(rf[0])))
    failed = []
    for ref, fp in auto:
        if ref.startswith('H'):
            continue
        shifts = [dx for _, dx in MACRO_TWINS.get(block_of(ref), [])]
        # a plain decoupling cap may try every pin of its supply until one has a spot right beside it (within its
        # chip's escape ring plus a little); other parts belong to the one pin their signal net names, so they go
        # as far as they must.  Without this the caps whose supply only has pins on the chip's crowded sides (the
        # RP2354B's +1V1 / RP_VREG, AT32 #1's +3V3) ended up 10-15 mm away
        supply_only = not ({p.GetNetname() for p in fp.Pads()} - POWER_NETS)
        cands = anchors_for(ref, fp)
        same_chip = [c for c in cands if c[0] == cands[0][0]] if cands else []
        bb, took = None, None
        for a in (same_chip if supply_only else cands[:1]):     # only pins of the same chip: never another chip's supply
            bb = place_near(fp, a, shifts, limit=ESCAPE_RING.get(a[0], 2.0) + 1.8 if supply_only else None)
            if bb:
                took = a
                break
        if bb is None and cands:
            bb = place_near(fp, cands[0], shifts)      # no pin had room nearby: take the intended one after all
            took = cands[0]
        if took is not None and supply_only:           # spread the next cap of this supply to another pin
            k = (took[0], took[1].GetNumber())
            used_power_pins[k] = used_power_pins.get(k, 0) + 1
        if bb is None:
            failed.append(ref)
            fp.SetPosition(pcbnew.VECTOR2I_MM(X1 + 10, Y0 + 5 + 2 * len(failed)))
        else:
            placed_boxes.append(bb)
            placed_boxes.extend((bb[0] + dx, bb[1], bb[2] + dx, bb[3]) for dx in shifts)
        board.Add(fp)
        # its copies in the other macros: same part number within the block, moved by the macro pitch
        for dst, dx in MACRO_TWINS.get(block_of(ref), []):
            head = ref.rstrip('0123456789')
            twin = byref[f'{head}{dst}{ref[len(head):][-2:]}']
            if twin.GetReference() in PLACEMENT:
                continue     # hand-placed twin keeps its own spot (rev 8: U601 sits where Hub-0's R705 would land)
            q = fp.GetPosition()
            twin.SetOrientationDegrees(fp.GetOrientationDegrees())
            twin.SetPosition(pcbnew.VECTOR2I_MM(pcbnew.ToMM(q.x) + dx, pcbnew.ToMM(q.y)))
            board.Add(twin)

    # ── the hubs' VDD33 caps on B.Cu, under their own pins (rev 11, 2026-10-27, user) ─────────────────────────────────
    # The per-pin pass above finds no spot under a hub (0.5 mm between the exposed-pad via keep-out and the pins) and
    # "just outside the pin" is the USB pairs' corridor, so the six 100 nF caps of each hub sat in the ring 5.1-8.1 mm
    # away, joined only through In2.Cu.  Here each one goes on B.Cu *radially under its pin*: the +3V3 pad under the pin's
    # outer end, the GND pad inwards, facing an exposed-pad GND via; the tie is pin -> HUB_TIE_OUT on F.Cu -> via just
    # past the pin tip (where the hubs' +3V3 fan-out via already sat) -> HUB_TIE_GAP on B.Cu -> cap.  This runs after the
    # whole placement, so no other part moves; the ring spots the caps leave stay empty.
    for cref in HUB_PIN_CAPS:
        cfp = fixed[cref]
        c = cfp.GetPosition()
        cx, cy = pcbnew.ToMM(c.x), pcbnew.ToMM(c.y)
        free_caps = [(r, f) for r, f in comps if block_of(r) == block_of(cref) and r.startswith('C')
                     and {q.GetNetname() for q in f.Pads()} == {'+3V3', 'GND'}
                     and f.GetValue().lower() in ('100nf', '100n', '0.1uf')]
        for pin in sorted((p for p in cfp.Pads() if p.GetNetname() == '+3V3'), key=lambda p: int(p.GetNumber())):
            px, py = pcbnew.ToMM(pin.GetPosition().x), pcbnew.ToMM(pin.GetPosition().y)
            dx, dy = px - cx, py - cy
            n = ((1.0, 0.0) if dx > 0 else (-1.0, 0.0)) if abs(dx) >= abs(dy) else ((0.0, 1.0) if dy > 0 else (0.0, -1.0))
            pb = pin.GetBoundingBox()
            pin_half = 0.5 * (pcbnew.ToMM(pb.GetWidth()) * abs(n[0]) + pcbnew.ToMM(pb.GetHeight()) * abs(n[1]))
            vx, vy = px + n[0] * (pin_half + HUB_TIE_OUT), py + n[1] * (pin_half + HUB_TIE_OUT)
            vx, vy = round(vx / 0.05) * 0.05, round(vy / 0.05) * 0.05
            if not free_caps:
                print(f'hub cap: none left for {cref}.{pin.GetNumber()}', flush=True)
                continue
            # the cap that sat nearest this pin, so the three hub macros keep one mapping (C701/C801/C901 ...)
            ref, fp = min(free_caps, key=lambda rf: math.hypot(pcbnew.ToMM(rf[1].GetPosition().x) - px,
                                                                  pcbnew.ToMM(rf[1].GetPosition().y) - py))
            free_caps.remove((ref, fp))
            if (cref, pin.GetNumber()) in HUB_PIN_CAPS_SKIP:
                continue                                  # its cap stays in the ring (same mapping as the other hubs)
            if fp.GetLayer() != pcbnew.B_Cu:
                fp.Flip(fp.GetPosition(), False)
            rail = next(q for q in fp.Pads() if q.GetNetname() == '+3V3')
            best = None
            for rot in (0, 90, 180, 270):                  # the +3V3 pad outwards, towards the pin's via
                fp.SetOrientationDegrees(rot)
                fp.SetPosition(pcbnew.VECTOR2I_MM(0, 0))
                q = rail.GetPosition()
                score = pcbnew.ToMM(q.x) * n[0] + pcbnew.ToMM(q.y) * n[1]
                if best is None or score > best[0]:
                    best = (score, rot)
            fp.SetOrientationDegrees(best[1])
            fp.SetPosition(pcbnew.VECTOR2I_MM(0, 0))
            q = rail.GetPosition()
            rb = rail.GetBoundingBox()
            rail_half = 0.5 * (pcbnew.ToMM(rb.GetWidth()) * abs(n[0]) + pcbnew.ToMM(rb.GetHeight()) * abs(n[1]))
            tx = vx - n[0] * (0.3 + HUB_TIE_GAP + rail_half)      # rail pad centre: HUB_TIE_GAP inside the via's edge
            ty = vy - n[1] * (0.3 + HUB_TIE_GAP + rail_half)
            fp.SetPosition(pcbnew.VECTOR2I_MM(round((tx - pcbnew.ToMM(q.x)) / 0.025) * 0.025,
                                              round((ty - pcbnew.ToMM(q.y)) / 0.025) * 0.025))
            bb = bbox(fp)
            clash = [r2 for r2, f2 in comps if f2 is not fp and f2.GetLayer() == pcbnew.B_Cu and overlaps(bb, bbox(f2), 0.1)]
            if clash:
                print(f'hub cap {ref} under {cref}.{pin.GetNumber()} overlaps {clash}', flush=True)
            pin_pairs[ref] = {'chip': cref, 'pin': pin.GetNumber(), 'net': '+3V3', 'via': [round(vx, 3), round(vy, 3)]}
    if pin_pairs:                                  # rewritten with the hub caps' outward ties
        with open(os.path.join(HERE, 'tools', 'build', 'pin_caps.json'), 'w') as fh:
            json.dump(pin_pairs, fh, indent=1, sort_keys=True)
    print(f'hub VDD33 caps on B.Cu: {sum(1 for v in pin_pairs.values() if "via" in v)}', flush=True)

    # board outline
    pts = [(X0, Y0), (X1, Y0), (X1, Y1), (X0, Y1)]
    for a, b in zip(pts, pts[1:] + pts[:1]):
        seg = pcbnew.PCB_SHAPE(board)
        seg.SetShape(pcbnew.SHAPE_T_SEGMENT)
        seg.SetStart(pcbnew.VECTOR2I_MM(*a))
        seg.SetEnd(pcbnew.VECTOR2I_MM(*b))
        seg.SetLayer(pcbnew.Edge_Cuts)
        seg.SetWidth(pcbnew.FromMM(0.05))
        board.Add(seg)

    for text, (x, y), rot in SILK_TEXT:
        t = pcbnew.PCB_TEXT(board)
        t.SetText(text)
        t.SetPosition(pcbnew.VECTOR2I_MM(x, y))
        t.SetLayer(pcbnew.F_SilkS)
        t.SetTextSize(pcbnew.VECTOR2I_MM(0.8, 0.8))
        t.SetTextThickness(pcbnew.FromMM(0.15))
        t.SetTextAngleDegrees(rot)
        board.Add(t)

    place_badge(board)
    place_antenna_keepout(board)

    # place/drill origin at the bottom-left corner: CPL and gerbers share it (tools/make_fab.py)
    board.GetDesignSettings().SetAuxOrigin(pcbnew.VECTOR2I_MM(X0, Y1))

    board.SetFileName(PCB)
    pro = PCB.replace('.kicad_pcb', '.kicad_pro')
    keep = open(pro).read()          # SaveBoard rewrites the project file; keep ours
    pcbnew.SaveBoard(PCB, board)
    with open(pro, 'w') as f:
        f.write(keep)
    with open(PCB.replace('.kicad_pcb', '.kicad_dru'), 'w') as f:
        f.write(DRC_RULES)
    write_stackup()
    print(f'wrote {PCB}:{len(comps)} footprints ({len(PLACEMENT)} placed by hand), {len(netinfo)} nets')
    if failed:
        print('NOT PLACED (parked right of the board):', ' '.join(failed))


def place_antenna_keepout(board):
    """ANT_KEEPOUT as a board-level rule area: no tracks, vias or zone fills on any copper layer; pads and footprints
    are allowed, so the module itself does not violate it.  On the board, not in the module: a zone inside the
    footprint made the board's copy differ from the library's (DRC lib_footprint_mismatch); route.py reads both."""
    fp = board.FindFootprintByReference('U1601')
    if fp is None:
        return
    x0, y0, x1, y1 = ANT_KEEPOUT
    keep = pcbnew.ZONE(board)
    keep.SetIsRuleArea(True)
    keep.SetZoneName('antenna')
    keep.SetDoNotAllowVias(True)
    keep.SetDoNotAllowTracks(True)
    keep.SetDoNotAllowZoneFills(True)
    keep.SetDoNotAllowPads(False)
    keep.SetDoNotAllowFootprints(False)
    keep.SetLayerSet(pcbnew.LSET.AllCuMask())
    o = keep.Outline()
    o.NewOutline()
    ex, ey = ESP_AT
    for x, y in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)):
        o.Append(pcbnew.FromMM(ex + x), pcbnew.FromMM(ey + y))
    board.Add(keep)


def place_badge(board):
    """The logos and the version (LOGOS), board-only footprints that stay out of the BOM and the CPL."""
    for ref, fpid, (x, y), (x0, y0, x1, y1) in LOGOS:
        fp = load_fp(fpid)
        fp.SetReference(ref)
        fp.Reference().SetVisible(False)
        fp.SetBoardOnly(True)
        fp.SetPosition(pcbnew.VECTOR2I_MM(x, y))
        board.Add(fp)
        keep = pcbnew.ZONE(fp)
        keep.SetIsRuleArea(True)
        keep.SetDoNotAllowVias(True)
        for rule in (keep.SetDoNotAllowTracks, keep.SetDoNotAllowPads, keep.SetDoNotAllowZoneFills,
                     keep.SetDoNotAllowFootprints):
            rule(False)
        keep.SetLayerSet(pcbnew.LSET.AllCuMask())
        o = keep.Outline()
        o.NewOutline()
        for px, py in ((x0, y0), (x1, y0), (x1, y1), (x0, y1)):
            o.Append(pcbnew.FromMM(px), pcbnew.FromMM(py))
        fp.Add(keep)
    t = pcbnew.PCB_TEXT(board)
    t.SetText(f'V{HW_VERSION}')
    t.SetPosition(pcbnew.VECTOR2I_MM(*VERSION_AT))
    t.SetLayer(pcbnew.F_SilkS)
    t.SetTextSize(pcbnew.VECTOR2I_MM(1.0, 1.0))
    t.SetTextThickness(pcbnew.FromMM(0.15))
    board.Add(t)


if __name__ == '__main__':
    main()
