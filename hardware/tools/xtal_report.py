#!/usr/bin/env python3
"""
Every crystal on the board, as its oscillator sees it - measured from the board file:

  circuit     the crystal's chip and pins, its two nets, the load caps and any series resistor (the RP2354B's 1 kOhm)
  distances   crystal pad -> chip pin, straight and as routed (track length of each crystal net, incl. the series
              resistor's chip side); vias and layers on those nets
  load caps   each cap's signal pad -> its crystal pad, and its GND pad -> the nearest GND via (the loop to the plane)
  intruders   foreign copper inside the crystal's courtyard (tracks or vias between or under its pads) and foreign
              tracks within NEAR mm of the crystal nets on the same layer - how long alongside, and how much of that
              is at the chip (within PIN_ZONE of its courtyard, where its neighbouring pins must leave anyway;
              route.py XTAL_PIN_ZONE)
  aggressors  the nearest copper of the switching node (SW_3V3) and of any USB D+/D- net, from the crystal centre

The oscillator requirements themselves are in the datasheets (design notes in gen_schematic.py `crystal`).

Run from hardware/:   python3 tools/xtal_report.py [--pcb FILE]
"""

import math
import os
import re
import sys

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = sys.argv[sys.argv.index('--pcb') + 1] if '--pcb' in sys.argv else os.path.join(HERE, 'kvm.kicad_pcb')
NEAR = 0.5                       # mm, edge to edge: foreign tracks this close to a crystal net on its layer
PIN_ZONE = 2.5                   # mm beyond the chip's courtyard: its other pins leave there, beside the crystal pins
NOISY = {'switch node': r'^SW_3V3$', 'USB pair': r'_D[PM](_PIN)?$'}
mm = pcbnew.ToMM


def seg_point(ax, ay, bx, by, px, py):
    dx, dy = bx - ax, by - ay
    t = 0.0 if dx == dy == 0 else max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - ax - t * dx, py - ay - t * dy)


def seg_seg(a, b):
    return min(seg_point(*a[:4], *b[:2]), seg_point(*a[:4], *b[2:4]), seg_point(*b[:4], *a[:2]), seg_point(*b[:4], *a[2:4]))


def main():
    b = pcbnew.LoadBoard(PCB)
    pads = {}                                    # net -> [(ref, pad, x, y)]
    for fp in b.GetFootprints():
        for p in fp.Pads():
            if p.GetNetname():
                pads.setdefault(p.GetNetname(), []).append((fp.GetReference(), p.GetNumber(), mm(p.GetPosition().x),
                                                            mm(p.GetPosition().y), p, fp))
    segs, vias = [], []
    for t in b.GetTracks():
        if t.GetClass() == 'PCB_TRACK':
            s, e = t.GetStart(), t.GetEnd()
            segs.append((mm(s.x), mm(s.y), mm(e.x), mm(e.y), mm(t.GetWidth()), b.GetLayerName(t.GetLayer()), t.GetNetname()))
        elif t.GetClass() == 'PCB_VIA':
            vias.append((mm(t.GetPosition().x), mm(t.GetPosition().y), mm(t.GetWidth(pcbnew.F_Cu)), t.GetNetname()))
    gnd_vias = [(x, y) for x, y, _, n in vias if n == 'GND']

    for y in sorted((fp for fp in b.GetFootprints() if re.match(r'Y\d', fp.GetReference())), key=lambda f: f.GetReference()):
        side = 'B' if y.IsFlipped() else 'F'
        layer = f'{side}.Cu'
        cx, cy = mm(y.GetPosition().x), mm(y.GetPosition().y)
        xnets = [p.GetNetname() for p in y.Pads() if p.GetNetname() and p.GetNetname() != 'GND']
        ypad = {p.GetNetname(): (mm(p.GetPosition().x), mm(p.GetPosition().y)) for p in y.Pads()}
        print(f'\n{y.GetReference()} {y.GetValue()} ({y.GetFieldText("MPN") or ""}) on {layer} at ({cx:.2f}, {cy:.2f})')
        chain = {}                               # crystal net -> nets up to the chip (through a series resistor)
        for n in xnets:
            chain[n] = [n]
            for ref, num, *_ in pads[n]:
                if ref.startswith('R'):
                    other = [p.GetNetname() for p in b.FindFootprintByReference(ref).Pads() if p.GetNetname() != n]
                    chain[n] += other
                    print(f'  series     {ref} {b.FindFootprintByReference(ref).GetValue()} between {n} and {other[0]}')
        for n in xnets:
            chip = [(ref, num, x, py) for nn in chain[n] for ref, num, x, py, *_ in pads[nn] if ref.startswith('U')]
            length = sum(math.hypot(s[2] - s[0], s[3] - s[1]) for s in segs if s[6] in chain[n])
            nv = sum(1 for v in vias if v[3] in chain[n])
            lays = sorted({s[5] for s in segs if s[6] in chain[n]})
            for ref, num, px, py in chip:
                yx, yy = ypad[n]
                print(f'  {n:12} {ref}.{num}: {math.hypot(px - yx, py - yy):5.2f} mm straight, {length:5.2f} mm routed, '
                      f'{nv} via(s), {"/".join(lays) or "pad to pad"}')
            for ref, num, px, py, p, fp in pads[n]:
                if ref.startswith('C'):
                    g = next((q for q in fp.Pads() if q.GetNetname() == 'GND'), None)
                    gx, gy = mm(g.GetPosition().x), mm(g.GetPosition().y)
                    dv = min((math.hypot(gx - vx, gy - vy) for vx, vy in gnd_vias), default=float('nan'))
                    yx, yy = ypad[n]
                    print(f'  cap        {ref} {fp.GetValue():5} on {"B" if fp.IsFlipped() else "F"}.Cu: '
                          f'{math.hypot(px - yx, py - yy):4.2f} mm to the crystal pad, GND pad {dv:4.2f} mm from a GND via')
        cy_box = y.GetCourtyard(pcbnew.B_CrtYd if y.IsFlipped() else pcbnew.F_CrtYd).BBox()
        x0, y0, x1, y1 = mm(cy_box.GetLeft()), mm(cy_box.GetTop()), mm(cy_box.GetRight()), mm(cy_box.GetBottom())
        xchain = {nn for c in chain.values() for nn in c}
        mine = xchain | {'GND'}

        def crosses(s):                          # any point of the segment (0.05 mm steps) inside the courtyard box
            n = max(1, int(math.hypot(s[2] - s[0], s[3] - s[1]) / 0.05))
            return any(x0 <= s[0] + k / n * (s[2] - s[0]) <= x1 and y0 <= s[1] + k / n * (s[3] - s[1]) <= y1
                       for k in range(n + 1))
        inside = sorted({s[6] for s in segs if s[6] not in mine and s[5] == layer and crosses(s)})
        vin = sorted({v[3] for v in vias if v[3] not in mine and x0 <= v[0] <= x1 and y0 <= v[1] <= y1})
        print(f'  courtyard  foreign tracks on {layer}: {", ".join(inside) or "none"}; foreign vias: {", ".join(vin) or "none"}')
        near, run, atchip = {}, {}, {}
        xsegs = [t for t in segs if t[6] in xchain]
        chips = []
        for nn in xchain:
            for ref, *_ , fp in pads[nn]:
                if ref.startswith('U'):
                    r = fp.GetCourtyard(pcbnew.B_CrtYd if fp.IsFlipped() else pcbnew.F_CrtYd).BBox()
                    chips.append((mm(r.GetLeft()) - PIN_ZONE, mm(r.GetTop()) - PIN_ZONE, mm(r.GetRight()) + PIN_ZONE,
                                  mm(r.GetBottom()) + PIN_ZONE))
        for s in segs:
            if s[6] in mine:
                continue
            close = [t for t in xsegs if t[5] == s[5] and seg_seg(s, t) - (s[4] + t[4]) / 2 < NEAR]
            if not close:
                continue
            near[s[6]] = min([near.get(s[6], 9)] + [seg_seg(s, t) - (s[4] + t[4]) / 2 for t in close])
            ln = math.hypot(s[2] - s[0], s[3] - s[1])        # how much of this track runs within NEAR of them
            n = max(1, int(ln / 0.05))
            pts = [(s[0] + k / n * (s[2] - s[0]), s[1] + k / n * (s[3] - s[1])) for k in range(n + 1)]
            hit = [q for q in pts if any(seg_point(*t[:4], *q) - (s[4] + t[4]) / 2 < NEAR for t in close)]
            run[s[6]] = run.get(s[6], 0) + ln * len(hit) / (n + 1)
            atchip[s[6]] = atchip.get(s[6], 0) + ln * sum(1 for qx, qy in hit if any(
                c[0] <= qx <= c[2] and c[1] <= qy <= c[3] for c in chips)) / (n + 1)
        print(f'  neighbours foreign tracks within {NEAR} mm of the crystal nets (closest gap, length alongside, of it '
              f'at the chip): ' + (', '.join(f'{n} {g:.2f} / {run[n]:.1f} mm ({atchip[n]:.1f})'
                                             for n, g in sorted(near.items(), key=lambda kv: kv[1])) or 'none'))
        away = sum(run[n] - atchip[n] for n in near)
        print(f'  away from the chip: {away:.1f} mm of foreign track runs within {NEAR} mm of the crystal nets')
        for what, pat in NOISY.items():
            d = [(math.hypot((s[0] + s[2]) / 2 - cx, (s[1] + s[3]) / 2 - cy), s[6], s[5]) for s in segs if re.search(pat, s[6])]
            d += [(seg_point(s[0], s[1], s[2], s[3], cx, cy), s[6], s[5]) for s in segs if re.search(pat, s[6])]
            if d:
                dist, n, lay = min(d)
                print(f'  {what:10} nearest {n} on {lay}: {dist:.1f} mm from the crystal centre')


if __name__ == '__main__':
    main()
