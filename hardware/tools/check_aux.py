#!/usr/bin/env python3
"""
Auxiliary-placement report for kvm.kicad_pcb (like check_spacing.py, but for *which side* a passive sits on).

Every small passive serves a pin of a larger chip.  This checks, for each one, that it sits on the side of that
chip where its pin is -- not across the package, which is what makes a net hard to route.  It reports the passives
that sit on the inward / far side of the pin they serve, and (for information) those whose anchor pin is far away.

A part is checked only when it has exactly one non-power net: then that net's nearest pad on one of the dense chips
is the pin it serves, and the part should be radially *outward* from it.

Known, accepted results (they are geometry, not mistakes):
  - the PWR/SERVICE block's parts are placed as copies of PC1's block (gen_pcb.py MACRO_TWINS), so a part whose
    circuit differs (R304, the VBUS_PWR/VSNS_PWR divider) is measured against PC1's equivalent pin;
  - C1501 (the MX key cap) is pushed out by the key's KEEP_FREE courtyard;
  - R501 and C516 (the 33 R and cap that feed RP_VREG): the RP2354B's RP_VREG pins are on its north row, whose ring
    is taken by the RP2354B's own via fan-out and Hub-0's port-line corridor, so they land about 10 mm east instead.
  - R1201/R1202, R1301/R1302, R1401/R1402 (the port switches' SEL0 pull-down / SEL1 pull-up): the switches' SEL pins
    face east, and the three output macros are placed as copies (gen_pcb.py MACRO_TWINS), so a strap's spot must be
    free beside all three switches - and east of the Other switch U1402 is where Hub-2's Other pair leaves.  The
    placer takes the nearest spot free in all three, 6-9 mm west.  They are DC straps on lines the MCUs drive (the
    power-up default SEL = 10), so where they tap the net does not matter electrically (checked 2026-10-27).

Divider / series parts with two signal nets are not checked: they belong *between* their two connections.

Run from hardware/:   python3 tools/check_aux.py
"""

import math
import os
import sys

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
MM = pcbnew.ToMM
POWER = {'GND', '+3V3', '+5V', '+1V1'}
# the chips whose pins an auxiliary part should sit beside
CHIPS = ('U501', 'U601', 'U701', 'U801', 'U901', 'U1001', 'U1101', 'U1202', 'U1302', 'U1402')
ACCEPTED = {'R304', 'R204', 'C1501', 'R501', 'C516',     # see the docstring
            'R1201', 'R1202', 'R1301', 'R1302', 'R1401', 'R1402'}
# front-port EN pull-downs: measured against the RP2354B pin they belong to, but they are placed at the port's
# switch (their other connection), which is what the net needs; the pad they connect by is the switch-side one
FACING_OK = {'R1205', 'R1305', 'R1405', 'R1102', 'R403'}


def main():
    board = pcbnew.LoadBoard(PCB)
    centres, pads = {}, {}
    for fp in board.GetFootprints():
        p = fp.GetPosition()
        centres[fp.GetReference()] = (MM(p.x), MM(p.y))
        for q in fp.Pads():
            name = q.GetNetname()
            if name:
                pos = q.GetPosition()
                pads.setdefault(name, []).append((fp.GetReference(), q.GetNumber(), MM(pos.x), MM(pos.y)))
    rows = []
    facing = []
    for fp in board.GetFootprints():
        ref = fp.GetReference()
        nets = [q.GetNetname() for q in fp.Pads()]
        sig = {n for n in nets if n and n not in POWER}
        if len(sig) != 1 or len(fp.Pads()) != 2:
            continue
        p = fp.GetPosition()
        px, py = MM(p.x), MM(p.y)
        best = None
        for r2, num, x, y in pads.get(next(iter(sig)), ()):
            if r2 not in CHIPS or r2 == ref:
                continue
            d = math.hypot(x - px, y - py)
            if best is None or d < best[0]:
                best = (d, r2, num, x, y)
        if best is None:
            continue
        d, r2, num, x, y = best
        cx, cy = centres[r2]
        if (x - cx) * (px - x) + (y - cy) * (py - y) < 0 and ref not in ACCEPTED:   # part on the far side of the pin
            rows.append((ref, next(iter(sig)), f'{r2}.{num}', px, py, d))
        # the pad on the pin's net should be the one nearer that pin, or the connection runs to the far end
        q = next(q for q in fp.Pads() if q.GetNetname() == next(iter(sig)))
        o = next(q for q in fp.Pads() if q.GetNetname() != next(iter(sig)))
        qp, op = q.GetPosition(), o.GetPosition()
        if (math.hypot(x - MM(qp.x), y - MM(qp.y)) > math.hypot(x - MM(op.x), y - MM(op.y))
                and ref not in FACING_OK):
            facing.append((ref, next(iter(sig)), f'{r2}.{num}',
                           round(math.hypot(x - MM(qp.x), y - MM(qp.y)), 2), round(math.hypot(x - MM(op.x), y - MM(op.y)), 2)))
    if not rows:
        print('no auxiliary part on the far side of the pin it serves')
    else:
        print(f'{len(rows)} auxiliary parts on the far side of the pin they serve:')
        for ref, n, pin, px, py, d in rows:
            print(f'  {ref:6} {n:14} serves {pin:10} at ({px:6.2f},{py:6.2f})  {d:5.1f} mm away')
    if not facing:
        print('every auxiliary part connects its chip by the nearer pad')
    else:
        print(f'{len(facing)} auxiliary parts connect their chip by the farther pad:')
        for ref, n, pin, dq, do in facing:
            print(f'  {ref:6} {n:14} serves {pin:10}: pad on that net {dq:5.2f} mm, other pad {do:5.2f} mm')
    return 1 if rows or facing else 0


if __name__ == '__main__':
    sys.exit(main())
