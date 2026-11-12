#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["build123d==0.13.0"]
# ///
"""The OpenDynKVM case: a tray, a flat lid and four spacers, from board.json (board_facts.py), as STEP and 3MF.

    cd hardware/case && uv run gen_case.py [--preview DIR]

uv builds the environment from the block above, pinned by gen_case.py.lock (`uv lock --script gen_case.py` after
changing it), with a Python it finds or fetches: OpenCascade has no build for Python 3.15 yet, so this script
keeps out of the repository's environment (../../pyproject.toml), which follows KiCad's Python.

- **Tray:** floor and walls. Under each of the board's four M3 holes sits a corner post with a hole for an M3 heat-set
  insert (or a self-tapping screw, FASTENER). The front wall has a notch per USB-A port (open at the top, the lid
  closes it) and a window per side LED; the rear wall has a window per USB-C port. The windows take a cable's plug
  body, not just the receptacle.
- **Lid:** a flat plate on the walls: a countersunk M3 hole over each post, a cutout for the keycap, a pinhole over
  the bootloader button, and the OpenDynKVM logo (../../branding/) raised on top, centred over the key.
- **Spacers:** four tubes between the board and the lid. One M3 countersunk screw per corner clamps lid, spacer,
  board and post together.

Outputs, next to this script:
- `case.step`: the assembled case as an assembly of named solids (tray, lid, logo, spacer 1-4) plus a simple
  reference model of the board (board outline and the envelopes of its taller parts, not printed). For editing by
  hand in FreeCAD or any other CAD program; it is exact geometry (B-rep), not a mesh.
- `tray.3mf`, `lid.3mf` (lid and logo as two parts of one object), `spacers.3mf` (all four): meshes for the
  slicer, in print orientation.
- `case.svg`: a line drawing of the assembled case, for the README.

Every dimension is a constant below. The positions come from the board, so after a board change run
`board_facts.py` (KiCad's Python) and then this again. The script checks the case against the board first
(clearances, ribs, screw lengths) and stops with the list of problems if one fails.
"""
import argparse
import gc
import json
import math
import sys
import warnings
from pathlib import Path

from OCP.BRep import BRep_Builder
from OCP.BRepBuilderAPI import BRepBuilderAPI_NurbsConvert
from OCP.TopoDS import TopoDS_Solid
from build123d import (Axis, Box, Color, Compound, Cone, Cylinder, ExportSVG, Face, Kind, LineType, Mesher, Plane,
                       Pos, RectangleRounded, Solid, chamfer, export_step, extrude, fillet, import_svg, offset)

HERE = Path(__file__).resolve().parent
FACTS = HERE / 'board.json'
LOGO_SVG = HERE.parent.parent / 'branding' / 'opendynkvm-logo-on-light.svg'

# ── Stack, bottom to top (mm; z = 0 is the underside of the floor) ──────────────────────────────────────────────────
FLOOR = 2.0             # floor thickness
UNDER = 5.0             # floor to board: the bottom side's tallest parts are 3.5 mm (UNDER_PARTS)
OVER = 9.0              # board to lid: the top side's tallest parts are the 6.3 x 7.7 mm electrolytics (8.0 max)
LID = 2.5               # lid thickness: room for a countersunk M3 head (1.7 mm) over the spacer
WALL = 2.0              # wall thickness
GAP_SIDE = 0.5          # board edge to wall, left and right
GAP_PORTS = 0.8         # board edge to wall, front and rear: the connectors overhang the edge by up to 0.55 mm
CORNER_R = 3.0          # outer corners, seen from above
INNER_R = 1.0           # inner corners
BOTTOM_CHAMFER = 0.6    # the tray's bottom edge (keeps the first layer's squish off the outside)
LID_FILLET = 1.0        # the lid's top edge

# ── Fasteners: M3 countersunk screws (ISO 10642 / DIN 7991) from the top, one per corner ────────────────────────────
SCREW_D = 3.4           # clearance hole in the lid and the spacers
CSK_D = 6.8             # countersink diameter at the top of the lid (90°): head 6.0-6.72 mm
FASTENER = 'insert'     # 'insert': M3 heat-set insert in each post; 'tap': self-tapping M3 straight into the post
INSERT_D = 4.0          # hole for the insert (M3 x 5.7 x 4.6 mm inserts; check yours)
INSERT_L = 5.7          # insert length
TAP_D = 2.5             # pilot hole for 'tap'
POST_R = 4.0            # corner post radius at the hole; the post also reaches out to both walls
SPACER_OD = 6.4         # spacers between board and lid: the top side keeps 3.5 mm clear round each hole
SCREW_LENGTHS = (16, 18, 20)

# ── Openings ────────────────────────────────────────────────────────────────────────────────────────────────────────
USB_C_WIN = (13.0, 7.5, 2.0)    # rear windows (w, h, corner r): room for a USB-C plug's body
USB_C_Z = 1.6                   # opening centre above the board (HRO TYPE-C-31-M-12, 3.3 mm tall)
USB_A_W = 16.0                  # front notches, open at the top: a USB-A plug's body is up to 16 x 8 mm
USB_A_Z = 3.6                   # opening centre above the board (Molex 67643, 7.2 mm tall)
USB_A_HALF = 4.8                # notch bottom this far under the opening centre
USB_A_R = 1.5
LED_WIN = (5.0, 3.0, 0.6)       # front windows over the side LEDs (SK6812SIDE-A: 4.0 mm wide, light from the side)
LED_Z = -0.3                    # window bottom relative to the board's top
KEY_CUT = 19.6                  # keycap 18.1 mm (footprint's outline) + 0.75 mm round it
KEY_CUT_R = 1.5
PINHOLE_D = 2.0                 # over SW501, the bootloader button: a paper clip reaches it
LOGO_W = 36.0                   # logo width; centred over the key, between the key cutout and the rear edge
LOGO_RELIEF = 0.8               # logo height above the lid
LOGO_GROW = 0.06                # every edge of the logo moves out this far: at 36 mm wide the wordmark's thinnest
                                # strokes ("KVM", Montserrat Regular) are 0.36 mm, then 0.48 mm, for a 0.4 mm nozzle
MIN_RIB = 1.0                   # least material between two openings in a wall

# ── What the board carries, for the checks (envelope heights above / below the board, mm) ──────────────────────────
TOP_PARTS = {                   # footprint name prefix: height above the board (datasheet maximum, rounded up)
    'CP_Elec_6.3x7.7': 8.0, 'USB_A_Molex_67643': 7.2, 'USB_C_Receptacle_HRO_TYPE-C-31-M-12': 3.3,
    'SW_Push_1P1T_XKB_TS-1187A': 2.0, 'LED_SK6812SIDE-A': 2.0, 'LQFP-48': 1.6, 'L_Sunlord_SWPA3015S': 1.6,
    'SOT-23': 1.45, 'SOT-583': 0.6, 'QFN-': 1.0, 'TI_RUX0012A': 1.0, 'Texas_RMG0012A': 0.8,
    'Crystal_SMD_3225': 1.0, 'D_SOD-123F': 1.1, 'C_0402': 0.6, 'C_0603': 0.9, 'C_0805': 1.3, 'R_0402': 0.4,
    'Tag-Connect': 0.0, 'TestPoint': 0.0, 'MountingHole': 0.0, 'OSHW-Logo': 0.0, 'OpenDynKVM_Logo': 0.0,
    'ESP32-C3-MINI-1': 2.55,
}
UNDER_PARTS = {                 # on the bottom side
    'LED_SK6812MINI-E': 2.0, 'C_0402': 0.6, 'C_0603': 0.9, 'R_0402': 0.4,
    'TestPoint': 0.0,
}
LEADS = {'USB_A_Molex_67643': 2.5}            # through-hole leads under the board
KEY = {'switch': 11.6, 'socket': 3.5, 'cap': 18.1, 'cap_low': 8.5, 'cap_top': 21.0}  # MX switch, hot-swap socket


def load():
    f = json.loads(FACTS.read_text())
    x0, y0, x1, y1 = f['outline']
    f['cx'], f['cy'] = (x0 + x1) / 2, (y0 + y1) / 2
    f['w'], f['h'] = x1 - x0, y1 - y0
    return f


class Case:
    """Dimensions derived from the board and the constants above, in case coordinates: X right, Y to the rear, Z up,
    origin under the board's centre."""

    def __init__(self, f):
        self.f = f
        self.t = f['thickness']
        self.z_floor = FLOOR
        self.z_board = FLOOR + UNDER                    # board underside
        self.z_top = self.z_board + self.t              # board top
        self.z_lid = self.z_top + OVER                  # lid underside = top of the walls
        self.z_lid_top = self.z_lid + LID
        self.in_x = f['w'] / 2 + GAP_SIDE
        self.in_y = f['h'] / 2 + GAP_PORTS
        self.out_x = self.in_x + WALL
        self.out_y = self.in_y + WALL

    def xy(self, x, y):
        """KiCad (x right, y down) to case coordinates."""
        return x - self.f['cx'], self.f['cy'] - y

    def centre(self, part):
        x0, y0, x1, y1 = part['courtyard']
        return self.xy((x0 + x1) / 2, (y0 + y1) / 2)

    def holes(self):
        return [self.xy(h['x'], h['y']) for h in self.f['holes']]

    def logo_box(self):
        kx, ky = self.xy(self.f['key']['x'], self.f['key']['y'])
        h = LOGO_W * self.logo_aspect
        y = (ky + KEY_CUT / 2 + self.out_y) / 2
        return kx - LOGO_W / 2, y - h / 2, kx + LOGO_W / 2, y + h / 2


# ── Checks ─────────────────────────────────────────────────────────────────────────────────────────────────────────
def height(table, footprint):
    for prefix, h in table.items():
        if footprint.startswith(prefix):
            return h
    return None


def box_distance(px, py, box):
    """Distance from a point to an axis-aligned box (x0, y0, x1, y1), 0 inside."""
    x0, y0, x1, y1 = box
    return math.hypot(max(x0 - px, 0, px - x1), max(y0 - py, 0, py - y1))


def check(c):
    f, problems, notes = c.f, [], []
    over, under = OVER, UNDER
    key = f['key']
    for p in f['parts']:
        fp, ref = p['footprint'], p['ref']
        if ref == key['ref']:
            continue
        table = TOP_PARTS if p['side'] == 'top' else UNDER_PARTS
        h = height(table, fp)
        if h is None:
            problems.append(f'{ref} ({fp}, {p["side"]}): no height known; add it to TOP_PARTS / UNDER_PARTS')
            continue
        room = over if p['side'] == 'top' else under
        if h > room - 0.5:
            problems.append(f'{ref}: {h} mm tall on the {p["side"]}, {room} mm of room')
        lead = height(LEADS, fp)
        if lead is not None and lead > under - 0.5:
            problems.append(f'{ref}: leads {lead} mm under the board, {under} mm of room')
    tallest = max((height(TOP_PARTS, p['footprint']) or 0, p['ref']) for p in f['parts']
                  if p['side'] == 'top' and p['ref'] != key['ref'])
    lowest = max((height(UNDER_PARTS, p['footprint']) or 0, p['ref']) for p in f['parts'] if p['side'] == 'bottom')
    notes.append(f'tallest part on top: {tallest[1]} {tallest[0]} mm of {over} mm under the lid; '
                 f'underneath: {max(lowest[0], KEY["socket"]):.2f} mm (hot-swap socket, {lowest[1]} {lowest[0]}) '
                 f'of {under} mm')
    if KEY['socket'] > under - 0.5:
        problems.append(f'the hot-swap socket needs {KEY["socket"]} mm under the board')

    # The key passes through the lid: its courtyard and the keycap fit the cutout.
    x0, y0, x1, y1 = key['courtyard']
    if max(x1 - x0, y1 - y0, KEY['cap']) > KEY_CUT - 1.0:
        problems.append(f'key cutout {KEY_CUT} mm: the keycap ({KEY["cap"]}) or the switch needs 0.5 mm round it')
    if KEY['cap_low'] > over:
        notes.append('the keycap stays above the lid when pressed')
    lid_up = c.z_lid_top - c.z_top
    notes.append(f'keycap top about {KEY["cap_top"] - lid_up:.1f} mm above the lid, {KEY["cap"]} mm cap in a '
                 f'{KEY_CUT} mm cutout')

    # Spacers on top keep clear of the parts round each hole; the posts underneath (round at the hole, square out to
    # both walls) of the bottom side's parts and of through-hole leads.
    for h in f['holes']:
        hx, hy = c.xy(h['x'], h['y'])
        sx, sy = math.copysign(1, hx), math.copysign(1, hy)
        wx, wy = sx * c.in_x, sy * c.in_y
        post = [(sorted((hx, wx)), sorted((hy - POST_R, hy + POST_R))),           # the post's shape, as tray()
                (sorted((hx - POST_R, hx + POST_R)), sorted((hy, wy))),           # builds it: arms to both walls,
                (sorted((hx, wx)), sorted((hy, wy)))]                             # the corner, a round end
        for p in f['parts']:
            if p['ref'] == h['ref']:
                continue
            top = p['side'] == 'top'
            body = (height(TOP_PARTS if top else UNDER_PARTS, p['footprint']) or 0) > 0
            d = box_distance(h['x'], h['y'], p['courtyard'])
            if top and body and d < SPACER_OD / 2 + 0.2:
                problems.append(f'{h["ref"]}: {p["ref"]} is {d:.2f} mm from the hole, the spacer needs '
                                f'{SPACER_OD / 2 + 0.2:.2f}')
            if (not top and body) or height(LEADS, p['footprint']):
                (ax, ay), (bx, by) = c.xy(p['courtyard'][0], p['courtyard'][1]), c.xy(p['courtyard'][2],
                                                                                    p['courtyard'][3])
                px, py = sorted((ax, bx)), sorted((ay, by))
                hit = d < POST_R + 0.2 or any(px[0] < x1 + 0.2 and px[1] > x0 - 0.2 and py[0] < y1 + 0.2
                                              and py[1] > y0 - 0.2 for (x0, x1), (y0, y1) in post)
                if hit:
                    problems.append(f'{h["ref"]}: the corner post runs into {p["ref"]} under the board')

    # Connectors reach the wall, and nothing else sticks out of the board.
    for p in f['parts']:
        x0, y0, x1, y1 = p['courtyard']
        ox0, oy0, ox1, oy1 = f['outline']
        over_x = max(ox0 - x0, x1 - ox1, 0)
        over_y = max(oy0 - y0, y1 - oy1, 0)
        if over_x > GAP_SIDE - 0.2 or over_y > GAP_PORTS - 0.2:
            problems.append(f'{p["ref"]} overhangs the board edge by {max(over_x, over_y):.2f} mm: it would hit '
                            f'the wall')

    # Openings in the front wall: ribs between the USB-A notches and the LED windows.
    spans = [(c.centre(p)[0] - USB_A_W / 2, c.centre(p)[0] + USB_A_W / 2, p['ref']) for p in f['usb_a']]
    spans += [(c.xy(p['x'], p['y'])[0] - LED_WIN[0] / 2, c.xy(p['x'], p['y'])[0] + LED_WIN[0] / 2, p['ref'])
              for p in f['leds']]
    spans.sort()
    for (a0, a1, ra), (b0, b1, rb) in zip(spans, spans[1:]):
        if b0 - a1 < MIN_RIB:
            problems.append(f'front wall: {b0 - a1:.2f} mm between the openings of {ra} and {rb}, '
                            f'want {MIN_RIB}')
    notes.append(f'front wall: narrowest rib {min(b0 - a1 for (a0, a1, _), (b0, b1, _) in zip(spans, spans[1:])):.2f}'
                 f' mm')
    spans = sorted((c.centre(p)[0] - USB_C_WIN[0] / 2, c.centre(p)[0] + USB_C_WIN[0] / 2) for p in f['usb_c'])
    for (a0, a1), (b0, b1) in zip(spans, spans[1:]):
        if b0 - a1 < MIN_RIB:
            problems.append('rear wall: USB-C windows overlap')
    if c.z_top + USB_C_Z + USB_C_WIN[1] / 2 > c.z_lid - MIN_RIB:
        problems.append('rear wall: no rib left over the USB-C windows')

    # The logo stays clear of the key cutout, the pinhole, the countersinks and the lid's edge.
    lx0, ly0, lx1, ly1 = c.logo_box()
    kx, ky = c.xy(key['x'], key['y'])
    bx, by = c.xy(f['button']['x'], f['button']['y'])
    if ly0 < ky + KEY_CUT / 2 + 2 or ly1 > c.out_y - 2 or lx0 < -c.out_x + 2 or lx1 > c.out_x - 2:
        problems.append('the logo does not fit between the key cutout and the rear edge: reduce LOGO_W')
    if box_distance(bx, by, (lx0, ly0, lx1, ly1)) < PINHOLE_D / 2 + 1.5:
        problems.append('the logo runs into the pinhole: reduce LOGO_W')
    for hx, hy in c.holes():
        if box_distance(hx, hy, (lx0, ly0, lx1, ly1)) < CSK_D / 2 + 1.5:
            problems.append('the logo runs into a countersink: reduce LOGO_W')

    # Screws: from the top of the lid through lid, spacer, board into the post.
    stack = LID + OVER + c.t
    post = UNDER + FLOOR
    for L in SCREW_LENGTHS:
        grip = L - stack
        if FASTENER == 'insert':
            ok = 2.5 <= grip <= post - 0.1
            notes.append(f'M3 x {L} countersunk: {grip:.1f} mm into the post ({INSERT_L} mm insert)'
                         f'{"" if ok else " - too short" if grip < 2.5 else " - longer than the post"}')
        else:
            ok = 4.0 <= grip <= post - 0.8
            notes.append(f'M3 x {L} countersunk: {grip:.1f} mm into the post{"" if ok else " - no good"}')
    return problems, notes


# ── Solids ─────────────────────────────────────────────────────────────────────────────────────────────────────────
def wall_cut(sketch, x, y, z, depth):
    """A sketch in the XZ plane, centred at (x, z), cut through the wall at y."""
    return Pos(x, y, z) * extrude(Plane.XZ * sketch, amount=depth, both=True)


def tray(c):
    f = c.f
    body = extrude(RectangleRounded(2 * c.out_x, 2 * c.out_y, CORNER_R), c.z_lid)
    body = chamfer(body.edges().group_by(Axis.Z)[0], BOTTOM_CHAMFER)       # the bottom outline, nothing else yet
    body -= Pos(0, 0, c.z_floor) * extrude(RectangleRounded(2 * c.in_x, 2 * c.in_y, INNER_R), c.z_lid)

    # Corner posts: round at the hole, square out to both walls.
    for hx, hy in c.holes():
        sx, sy = math.copysign(1, hx), math.copysign(1, hy)
        h = c.z_board - c.z_floor
        wx, wy = sx * c.in_x, sy * c.in_y
        post = Pos(hx, hy, c.z_floor + h / 2) * Cylinder(POST_R, h)
        for x0, x1, y0, y1 in ((hx, wx, hy - POST_R, hy + POST_R), (hx - POST_R, hx + POST_R, hy, wy),
                               (hx, wx, hy, wy)):
            post += Pos((x0 + x1) / 2, (y0 + y1) / 2, c.z_floor + h / 2) * Box(abs(x1 - x0) + 0.01,
                                                                               abs(y1 - y0) + 0.01, h)
        body += post
        if FASTENER == 'insert':
            depth = INSERT_L + 0.3
            body -= Pos(hx, hy, c.z_board - depth / 2) * Cylinder(INSERT_D / 2, depth)
            body -= Pos(hx, hy, c.z_board / 2) * Cylinder(SCREW_D / 2 - 0.2, c.z_board + 0.2)
        else:
            depth = UNDER + FLOOR - 0.8
            body -= Pos(hx, hy, c.z_board - depth / 2) * Cylinder(TAP_D / 2, depth + 0.01)

    depth = WALL + 2 * max(GAP_PORTS, GAP_SIDE) + 2
    w, h, r = USB_C_WIN
    for p in f['usb_c']:
        x, _ = c.centre(p)
        body -= wall_cut(RectangleRounded(w, h, r), x, c.in_y + WALL / 2, c.z_top + USB_C_Z, depth)
    bottom = c.z_top + USB_A_Z - USB_A_HALF
    tall = c.z_lid - bottom + 2 * USB_A_R + 1            # past the top: only the bottom corners are rounded
    for p in f['usb_a']:
        x, _ = c.centre(p)
        body -= wall_cut(RectangleRounded(USB_A_W, tall, USB_A_R), x, -c.in_y - WALL / 2, bottom + tall / 2, depth)
    w, h, r = LED_WIN
    for p in f['leds']:
        x, _ = c.xy(p['x'], p['y'])
        body -= wall_cut(RectangleRounded(w, h, r), x, -c.in_y - WALL / 2, c.z_top + LED_Z + h / 2, depth)
    return body


def logo_faces():
    faces = [s for s in import_svg(str(LOGO_SVG)) if isinstance(s, Face)]
    if not faces:
        sys.exit(f'{LOGO_SVG}: no filled shapes')
    bb = Compound(faces).bounding_box()
    return faces, bb


def grow(face, amount):
    """The face with every edge moved out by amount (holes shrink).  OpenCascade's 2D offset fails on some outlines
    at some amounts (one of the logo's at 0.04 and 0.05 mm, not at 0.06), so nearby amounts are tried too."""
    for a in (amount, amount + 0.005, amount - 0.005, amount + 0.01, amount - 0.01):
        for kind in (Kind.ARC, Kind.INTERSECTION):
            try:
                grown = offset(face, a, kind=kind)
            except ValueError:
                continue
            if grown.area > face.area:
                # Offset curves as NURBS: OpenCascade meshes some offset curves into an invalid mesh, and other CAD
                # programs read NURBS best.
                return Face(BRepBuilderAPI_NurbsConvert(grown.wrapped, True).Shape())
    sys.exit(f'the logo: a shape of {face.area:.2f} mm² cannot be grown by {amount} mm; set LOGO_GROW = 0')


def lid(c, logo):
    plate = Pos(0, 0, c.z_lid) * extrude(RectangleRounded(2 * c.out_x, 2 * c.out_y, CORNER_R), LID)
    plate = fillet(plate.edges().group_by(Axis.Z)[-1], LID_FILLET)         # the top outline, before any hole
    sink = (CSK_D - SCREW_D) / 2                                   # 90°: as deep as it is wide on each side
    for hx, hy in c.holes():
        plate -= Pos(hx, hy, c.z_lid + LID / 2) * Cylinder(SCREW_D / 2, LID + 0.2)
        plate -= Pos(hx, hy, c.z_lid_top - sink / 2 + 0.05) * Cone(SCREW_D / 2, CSK_D / 2 + 0.05, sink + 0.1)
    kx, ky = c.xy(c.f['key']['x'], c.f['key']['y'])
    plate -= Pos(kx, ky, c.z_lid + LID / 2) * extrude(RectangleRounded(KEY_CUT, KEY_CUT, KEY_CUT_R), LID + 0.2,
                                                       both=True)
    bx, by = c.xy(c.f['button']['x'], c.f['button']['y'])
    plate -= Pos(bx, by, c.z_lid + LID / 2) * Cylinder(PINHOLE_D / 2, LID + 0.2)
    plate -= Pos(bx, by, c.z_lid_top - 0.25) * Cone(PINHOLE_D / 2, PINHOLE_D / 2 + 0.55, 0.6)

    faces, bb = logo
    x0, y0, x1, y1 = c.logo_box()
    # Scale first, then centre by the scaled box: how scale() treats a face's placement (import_svg puts the SVG's
    # y-flip there) differs between build123d versions.
    scaled = [fc.scale(LOGO_W / bb.size.X) for fc in faces]
    centre = Compound(scaled).bounding_box().center()
    placed = [Pos((x0 + x1) / 2 - centre.X, (y0 + y1) / 2 - centre.Y, c.z_lid_top - centre.Z) * fc for fc in scaled]
    if LOGO_GROW:
        placed = [grow(fc, LOGO_GROW) for fc in placed]
    ink = placed[0].fuse(*placed[1:])   # one outline: grown shapes that touch must not overlap as solids
    ink = list(ink) if isinstance(ink, list) else ink.faces()
    relief = extrude(Compound(ink), LOGO_RELIEF)
    got = relief.bounding_box()
    want = (x0 - LOGO_GROW, y0 - LOGO_GROW, c.z_lid_top, x1 + LOGO_GROW, y1 + LOGO_GROW, c.z_lid_top + LOGO_RELIEF)
    have = (got.min.X, got.min.Y, got.min.Z, got.max.X, got.max.Y, got.max.Z)
    if max(abs(a - b) for a, b in zip(have, want)) > 0.05:
        sys.exit(f'the logo landed at {[round(v, 2) for v in have]}, not {[round(v, 2) for v in want]}')
    return plate, relief


def named(shape, label, colour=None):
    """shape as a single solid, with its name and colour."""
    s = shape if isinstance(shape, Solid) else shape.solid()
    s.label = label
    if colour is not None:
        s.color = colour
    return s


def spacer():
    """One spacer, standing on z = 0."""
    return Pos(0, 0, OVER / 2) * (Cylinder(SPACER_OD / 2, OVER) - Cylinder(SCREW_D / 2, OVER + 0.2))


def board_reference(c):
    """The board and the envelopes of its parts taller than 1.5 mm (and the key), for looking at the fit."""
    f = c.f
    shapes = [Pos(0, 0, c.z_board + c.t / 2) * Box(f['w'], f['h'], c.t)]
    for p in f['parts']:
        x0, y0, x1, y1 = p['courtyard']
        (ax, ay), (bx, by) = c.xy(x0, y0), c.xy(x1, y1)
        cx, cy, w, d = (ax + bx) / 2, (ay + by) / 2, abs(bx - ax), abs(by - ay)
        if p['ref'] == f['key']['ref']:
            shapes.append(Pos(cx, cy, c.z_top + KEY['switch'] / 2) * Box(w, d, KEY['switch']))
            shapes.append(Pos(cx, cy, c.z_top + (KEY['cap_low'] + KEY['cap_top']) / 2 + 4)
                          * Box(KEY['cap'], KEY['cap'], KEY['cap_top'] - KEY['cap_low'] - 4))
            continue
        top = p['side'] == 'top'
        h = height(TOP_PARTS if top else UNDER_PARTS, p['footprint']) or 0
        if h < 1.5:
            continue
        if p['footprint'].startswith('CP_Elec'):
            body = Cylinder(3.15, h)
        else:
            body = Box(w, d, h)
        shapes.append(Pos(cx, cy, c.z_top + h / 2 if top else c.z_board - h / 2) * body)
    green = Color(0.1, 0.45, 0.2)
    ref = Compound(label='board (reference, not printed)',
                   children=[named(s, 'board' if i == 0 else 'part envelope', green) for i, s in enumerate(shapes)])
    return ref


def one_mesh(shape):
    """All closed shells of shape as one body, for meshing only: the slicer gets one mesh for the logo instead of one
    per letter (not a valid solid for CAD: a solid has one outer shell, so the STEP keeps the separate solids)."""
    builder, solid = BRep_Builder(), TopoDS_Solid()
    builder.MakeSolid(solid)
    for s in shape.solids():
        for shell in s.shells():
            builder.Add(solid, shell.wrapped)
    return Solid(solid)


def write_3mf(path, shapes, one_object=None):
    """The shapes as meshes in a 3MF file: one object each, or with one_object (a name) one object made of them
    as its parts.  The lid goes in as lid and logo, two parts: each meshes cleanly on its own, while OpenCascade
    meshes the fused lid with cracks along the logo's edges."""
    m = Mesher()
    with warnings.catch_warnings():
        warnings.simplefilter('error')                 # a mesh that is not manifold stops here
        for s in shapes:
            m.add_shape(s, linear_deflection=0.01, angular_deflection=0.2)
    if one_object:
        items = m.model.GetBuildItems()
        old = []
        while items.MoveNext():
            old.append(items.GetCurrent())
        for item in old:
            m.model.RemoveBuildItem(item)
        parts = m.model.AddComponentsObject()
        parts.SetName(one_object)
        for mesh in m.meshes:
            parts.AddComponent(mesh, m.wrapper.GetIdentityTransform())
        m.model.AddBuildItem(parts, m.wrapper.GetIdentityTransform())
    m.write(str(path))
    del m                                    # before interpreter shutdown, which lib3mf's destructor doesn't survive
    gc.collect()


def preview(directory, views):
    directory.mkdir(parents=True, exist_ok=True)
    for name, shape, eye, up in views:
        visible, _ = shape.project_to_viewport(eye, viewport_up=up, look_at=shape.bounding_box().center())
        size = max(*Compound(children=visible).bounding_box().size)
        svg = ExportSVG(scale=180 / size, line_weight=0.15)
        svg.add_layer('visible', line_type=LineType.CONTINUOUS)
        svg.add_shape(visible, layer='visible')
        path = directory / f'{name}.svg'
        svg.write(str(path))
        # A white ground, so the black lines also show on a dark page (GitHub's dark theme).
        text = path.read_text()
        at = text.index('>', text.index('<svg')) + 1
        path.write_text(text[:at] + '<rect x="-100%" y="-100%" width="300%" height="300%" fill="white"/>'
                        + text[at:])


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0])
    ap.add_argument('--preview', type=Path, help='also write line drawings (SVG) of the parts into this directory')
    args = ap.parse_args()

    c = Case(load())
    logo = logo_faces()
    c.logo_aspect = logo[1].size.Y / logo[1].size.X
    problems, notes = check(c)
    for n in notes:
        print(n)
    if problems:
        sys.exit('case does not fit the board:\n  ' + '\n  '.join(problems))

    # Names and colours go on the solids themselves: STEP and 3MF writers drop them from wrapping compounds.
    case_colour, logo_colour = Color(0.18, 0.19, 0.21), Color(0.92, 0.93, 0.95)
    t = named(tray(c).solid(), 'tray', case_colour)
    plate, relief = lid(c, logo)
    plate = named(plate.solid(), 'lid', case_colour)
    relief = Compound(label='logo', children=[named(s, 'logo', logo_colour) for s in relief.solids()])
    sp = [named(Pos(hx, hy, c.z_top) * spacer(), f'spacer {i}', Color(0.5, 0.5, 0.52))
          for i, (hx, hy) in enumerate(c.holes(), 1)]
    case = Compound(label=f'OpenDynKVM case (hardware {c.f["hw_version"]})',
                    children=[t, plate, relief, *sp, board_reference(c)])
    export_step(case, str(HERE / 'case.step'))

    # Print orientation: tray as it stands, lid flat side down with the logo up, spacers standing.
    write_3mf(HERE / 'tray.3mf', [t])
    lid_print = named(Pos(0, 0, -c.z_lid) * plate, 'lid', case_colour)
    logo_print = named(one_mesh(Pos(0, 0, -c.z_lid) * relief), 'logo', logo_colour)
    write_3mf(HERE / 'lid.3mf', [lid_print, logo_print], one_object='OpenDynKVM lid')
    write_3mf(HERE / 'spacers.3mf', [named(Pos((i - 1.5) * (SPACER_OD + 4), 0, 0) * spacer(), f'spacer {i + 1}')
                                     for i in range(4)])
    outer = (2 * c.out_x, 2 * c.out_y, c.z_lid_top)
    print(f'case {outer[0]:.1f} x {outer[1]:.1f} x {outer[2]:.1f} mm (+ {LOGO_RELIEF} mm logo); '
          f'lid {LID} mm on walls {c.z_lid:.1f} mm high; spacers {OVER} mm; '
          f'written: case.step, tray.3mf, lid.3mf, spacers.3mf, case.svg')
    iso = ('case', Compound(children=[t, plate, relief, *sp]), (-250, -300, 220), (0, 0, 1))
    preview(HERE, [iso])                                # case.svg, for the README
    if args.preview:
        lid_part = plate + relief
        preview(args.preview, [
            iso,
            ('lid-top', lid_part, (0, 0, 500), (0, 1, 0)),
            ('tray-top', t, (0, 0, 500), (0, 1, 0)),
            ('tray-front', t, (0, -500, c.z_lid / 2), (0, 0, 1)),
            ('tray-rear', t, (0, 500, c.z_lid / 2), (0, 0, 1)),
        ])
        print(f'previews in {args.preview}')


if __name__ == '__main__':
    main()
