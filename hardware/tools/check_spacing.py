#!/usr/bin/env python3
"""
Placement spacing report for kvm.kicad_pcb (adapted from the OpenRower checker).

Checks, per footprint pair: courtyard gap and copper pad-to-pad gap; per footprint: body
distance to the board edges.  JLCPCB adds assembly rails only to boards smaller than 70x70 mm;
this board is 120x80 mm, so no rail rule applies (RAIL_EDGES is empty).  The rear connectors,
the front connectors / side LEDs and the Bluetooth module's antenna end sit at the edge on purpose
(EDGE_PARTS).  Bodies are read from F.Fab, so a part on the bottom side is not measured here.

Run from hardware/:   uv run tools/check_spacing.py
"""
import itertools
import math
import os

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')

MIN_COURTYARD_GAP = 0.1     # mm, between courtyards (IPC-7351B nominal courtyards already add 0.25 mm)
MIN_PAD_GAP = 0.25          # mm, copper of different footprints
MIN_RAIL_EDGE = 2.5         # mm, body to an edge that carries JLC rails
RAIL_EDGES = ()             # none: the board is larger than 70x70 mm
MIN_EDGE = 0.5              # mm, body to left/right edge (edge-mounted parts exempt)
EDGE_PARTS = {'J101', 'J201', 'J301', 'J1201', 'J1301', 'J1401', 'D1502', 'D1503', 'D1504', 'D1505', 'D1506',
              'U1601'}     # the module's PCB antenna at the board edge (Espressif), 0.47 mm inside it: no overhang
MM = pcbnew.ToMM


def bbox(rect):
    return (MM(rect.GetLeft()), MM(rect.GetTop()), MM(rect.GetRight()), MM(rect.GetBottom()))


def gap(a, b):
    dx = max(b[0] - a[2], a[0] - b[2], 0)
    dy = max(b[1] - a[3], a[1] - b[3], 0)
    if dx == 0 and dy == 0:
        return -min(a[2] - b[0], b[2] - a[0], a[3] - b[1], b[3] - a[1])  # overlap depth
    return (dx ** 2 + dy ** 2) ** 0.5


def main():
    board = pcbnew.LoadBoard(PCB)
    edge = bbox(board.GetBoardEdgesBoundingBox())
    # board-only footprints without pads are print (logos), not parts: no body to space
    fps = {fp.GetReference(): fp for fp in board.GetFootprints() if fp.Pads() or not fp.IsBoardOnly()}
    # courtyards and pad gaps are only compared within one side: since rev 9 the per-pin decoupling caps sit on B.Cu
    # *under* their chip on purpose, which is not a spacing problem (a footprint flipped to B.Cu has no F.CrtYd at all)
    def side_court(fp):
        order = (pcbnew.F_CrtYd, pcbnew.B_CrtYd) if fp.GetLayer() == pcbnew.F_Cu else (pcbnew.B_CrtYd, pcbnew.F_CrtYd)
        for lay in order:
            r = fp.GetCourtyard(lay).BBox()
            if r.GetWidth() > 0:
                return bbox(r)
        return bbox(fp.GetBoundingBox(False))

    court = {r: side_court(fp) for r, fp in fps.items()}
    side = {r: ('B' if fp.GetLayer() == pcbnew.B_Cu else 'F') for r, fp in fps.items()}
    body = {}
    for r, fp in fps.items():
        fab = [bbox(g.GetBoundingBox()) for g in fp.GraphicalItems() if g.GetLayer() == pcbnew.F_Fab
               and g.GetClass() != 'PCB_TEXT']
        if fab:
            body[r] = (min(b[0] for b in fab), min(b[1] for b in fab), max(b[2] for b in fab), max(b[3] for b in fab))
    pads = {r: [bbox(p.GetBoundingBox()) for p in fp.Pads() if p.IsOnCopperLayer()] for r, fp in fps.items()}

    # round courtyards (touch electrode, mounting holes): compare as circles, not boxes
    circle = {r: ((c[0] + c[2]) / 2, (c[1] + c[3]) / 2, (c[2] - c[0]) / 2) for r, c in court.items()
              if r.startswith(('TP', 'H')) and abs((c[2] - c[0]) - (c[3] - c[1])) < 0.01}

    def cgap(a, b):
        if a in circle and b in circle:
            (x1, y1, r1), (x2, y2, r2) = circle[a], circle[b]
            return math.hypot(x1 - x2, y1 - y2) - r1 - r2
        if b in circle:
            a, b = b, a
        if a in circle:
            x, y, r = circle[a]
            bb = court[b]
            nx, ny = min(max(x, bb[0]), bb[2]), min(max(y, bb[1]), bb[3])
            return math.hypot(nx - x, ny - y) - r
        return gap(court[a], court[b])

    problems = []
    close = []
    for a, b in itertools.combinations(sorted(fps), 2):
        if side[a] != side[b]:
            continue                     # a bottom-side cap under a top-side chip is deliberate (see above)
        g = cgap(a, b)
        if g < MIN_COURTYARD_GAP:
            problems.append(f'courtyard gap {a}-{b}: {g:.2f} mm')
        elif g < 0.5:
            close.append((g, a, b))
        pg = min((gap(pa, pb) for pa in pads[a] for pb in pads[b]), default=99)
        if pg < MIN_PAD_GAP:
            problems.append(f'pad gap {a}-{b}: {pg:.2f} mm')
    for r, bb in body.items():
        if r.startswith('H'):
            continue
        dist = {'top': bb[1] - edge[1], 'bottom': edge[3] - bb[3], 'left': bb[0] - edge[0], 'right': edge[2] - bb[2]}
        for side, d in dist.items():
            if side in RAIL_EDGES and d < MIN_RAIL_EDGE:
                problems.append(f'{r} body {d:.2f} mm from the {side} (rail) edge')
            elif r not in EDGE_PARTS and d < MIN_EDGE:
                problems.append(f'{r} body {d:.2f} mm from the {side} edge')

    print(f'{len(fps)} footprints, board {edge[2] - edge[0]:.1f} x {edge[3] - edge[1]:.1f} mm')
    print(f'closest courtyard pairs (< 0.5 mm): ' + ', '.join(f'{a}-{b} {g:.2f}' for g, a, b in sorted(close)[:15]))
    print('\n'.join(problems) if problems else 'no spacing problems')
    return len(problems)


if __name__ == '__main__':
    raise SystemExit(main())
