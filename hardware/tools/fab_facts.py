#!/usr/bin/env python3
"""
The facts about kvm.kicad_pcb that a fab and assembly order depends on, measured from the board file - the evidence
for jlcpcb.md sections 2, 5 and 8:

  outline   board size, internal cut-outs (footprint Edge.Cuts)
  copper    the stack-up (gen_pcb.py STACKUP), zones per layer, track widths, the design rules that bound clearance,
            hole and edge distance (.kicad_pro, .kicad_dru)
  vias      size, count, tenting; vias inside a pad of their own net (via-in-pad: needs filled & capped vias, or
            accepts solder wicking), per part and whether that pad gets paste
  holes     plated holes, plated slots and non-plated holes by size, with the parts that own them
  edges     parts whose body (Fab layer, else courtyard) comes within EDGE_NEAR of the board outline - what the edge
            rails and V-cuts must clear
  assembly  parts per side (by the CPL), the bottom side's parts, and the through-hole parts JLC solders by hand

Run from hardware/:   uv run tools/fab_facts.py
"""

import collections
import csv
import json
import os
import re
import sys

import pcbnew

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import board_stackup  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
CPL = os.path.join(HERE, 'bom', 'jlcpcb_cpl.csv')
EDGE_NEAR = 0.5                  # mm
mm = pcbnew.ToMM


def bbox(item):
    r = item.GetBoundingBox()
    return mm(r.GetLeft()), mm(r.GetTop()), mm(r.GetRight()), mm(r.GetBottom())


def refs_text(refs, n=6):
    refs = sorted(set(refs), key=lambda r: (r.rstrip('0123456789'), int('0' + r.lstrip('ABCDEFGHIJKLMNOPQRSTUVWXYZ'))))
    return ', '.join(refs[:n]) + (f' ... ({len(refs)})' if len(refs) > n else '')


def main():
    b = pcbnew.LoadBoard(PCB)
    edge = [d for d in b.GetDrawings() if d.GetLayer() == pcbnew.Edge_Cuts]
    xs = [v for d in edge for v in bbox(d)[0::2]]
    ys = [v for d in edge for v in bbox(d)[1::2]]
    X0, X1, Y0, Y1 = min(xs), max(xs), min(ys), max(ys)
    w0 = max(d.GetWidth() for d in edge) / 2 / 1e6 if edge else 0
    X0, Y0, X1, Y1 = X0 + w0, Y0 + w0, X1 - w0, Y1 - w0
    print(f'outline     {X1 - X0:.2f} x {Y1 - Y0:.2f} mm')
    for fp in b.GetFootprints():
        cut = [g for g in fp.GraphicalItems() if g.GetLayer() == pcbnew.Edge_Cuts]
        if cut:
            bx = [bbox(g) for g in cut]
            lw = max(g.GetWidth() for g in cut) / 1e6
            print(f'cut-out     {fp.GetReference()}: {max(r[2] for r in bx) - min(r[0] for r in bx) - lw:.2f} x '
                  f'{max(r[3] for r in bx) - min(r[1] for r in bx) - lw:.2f} mm')

    print('\nstack-up    ' + ', '.join(f'{n} {t * 1000:.1f} um' + (f' Er {er}' if er else '')
                                     for n, kind, t, er in board_stackup(PCB) if t and kind != 'Top Solder Mask'
                                     and kind != 'Bottom Solder Mask'))
    zones = collections.defaultdict(list)
    for z in b.Zones():
        for lay in z.GetLayerSet().CuStack():
            zones[b.GetLayerName(lay)].append(z.GetNetname())
    print('zones       ' + ', '.join(f'{lay}: {"/".join(sorted(set(n)))}' for lay, n in zones.items()))
    widths = collections.Counter()
    for t in b.GetTracks():
        if t.GetClass() == 'PCB_TRACK':
            widths[round(mm(t.GetWidth()), 3)] += 1
    print(f'tracks      {sum(widths.values())} segments, widths ' + ', '.join(f'{w:g} mm x{n}' for w, n in sorted(widths.items())))
    rules = json.load(open(PCB.replace('.kicad_pcb', '.kicad_pro')))['board']['design_settings']['rules']
    print('rules       ' + ', '.join(f'{k} {rules[k]:g}' for k in ('min_clearance', 'min_track_width', 'min_copper_edge_clearance',
                                                                 'min_hole_clearance', 'min_hole_to_hole', 'min_through_hole_diameter')
                                     if k in rules))
    dru = PCB.replace('.kicad_pcb', '.kicad_dru')
    if os.path.exists(dru):
        for line in open(dru):
            if line.strip().startswith('(rule'):
                print(f'            custom rule {line.strip()[6:].strip()}')

    vias = collections.Counter()
    via_pts = []
    for t in b.GetTracks():
        if t.GetClass() == 'PCB_VIA':
            vias[(round(mm(t.GetWidth(pcbnew.F_Cu)), 3), round(mm(t.GetDrill()), 3), t.GetViaType())] += 1
            via_pts.append(t)
    m = re.search(r'\(tenting\s*\(front (\w+)\)\s*\(back (\w+)\)', open(PCB).read())
    tent = f'tented front {m[1]}, back {m[2]}' if m else 'tenting not set'
    kinds = {pcbnew.VIATYPE_THROUGH: 'through'}
    print('\nvias        ' + ', '.join(f'{n} x {d:g}/{dr:g} mm {kinds.get(k, "blind/buried/micro")}'
                                     for (d, dr, k), n in vias.items()) + f'; {tent}')
    in_pad = collections.defaultdict(lambda: [0, ''])
    for fp in b.GetFootprints():
        for p in fp.Pads():
            if p.GetAttribute() != pcbnew.PAD_ATTRIB_SMD or not p.GetNetname():
                continue
            n = sum(1 for v in via_pts if v.GetNetname() == p.GetNetname() and p.HitTest(v.GetPosition()))
            if n:
                paste = (p.GetLayerSet().Contains(pcbnew.F_Paste) or p.GetLayerSet().Contains(pcbnew.B_Paste)
                         or any(q.GetAttribute() == pcbnew.PAD_ATTRIB_SMD and not q.GetNumber() and p.HitTest(q.GetPosition())
                                for q in fp.Pads()))
                in_pad[(fp.GetReference(), p.GetNumber())] = [n, 'paste on the pad' if paste else 'no paste']
    print(f'via-in-pad  {sum(v[0] for v in in_pad.values())} vias: ' +
          '; '.join(f'{r}.{pn} {n} ({paste})' for (r, pn), (n, paste) in sorted(in_pad.items())))

    holes = collections.defaultdict(list)
    for fp in b.GetFootprints():
        for p in fp.Pads():
            if p.GetAttribute() not in (pcbnew.PAD_ATTRIB_PTH, pcbnew.PAD_ATTRIB_NPTH):
                continue
            d = p.GetDrillSize()
            kind = 'NPTH' if p.GetAttribute() == pcbnew.PAD_ATTRIB_NPTH else 'PTH'
            size = f'{mm(min(d.x, d.y)):g} x {mm(max(d.x, d.y)):g} mm slot' if d.x != d.y else f'{mm(d.x):g} mm'
            holes[(kind, size)].append(fp.GetReference())
    print('\nholes       ' + '\n            '.join(f'{k:4} {s:18} x{len(r):<3} {refs_text(r)}' for (k, s), r in sorted(holes.items())))

    print(f'\nedges       parts within {EDGE_NEAR} mm of the outline (body on the Fab layer, else courtyard; - = past the edge):')
    for fp in sorted(b.GetFootprints(), key=lambda f: f.GetReference()):
        lay = (pcbnew.F_Fab, pcbnew.F_CrtYd) if not fp.IsFlipped() else (pcbnew.B_Fab, pcbnew.B_CrtYd)
        for L in lay:
            gs = [g for g in fp.GraphicalItems() if g.GetLayer() == L]
            if gs:
                break
        if not gs:
            continue
        bx = [bbox(g) for g in gs]
        x0, y0, x1, y1 = min(r[0] for r in bx), min(r[1] for r in bx), max(r[2] for r in bx), max(r[3] for r in bx)
        near = [(s, d) for s, d in (('W', x0 - X0), ('N', y0 - Y0), ('E', X1 - x1), ('S', Y1 - y1)) if d < EDGE_NEAR]
        if near:
            print(f'            {fp.GetReference():7} {fp.GetValue()[:20]:20} ' + ', '.join(f'{s} {d:+.2f} mm' for s, d in near)
                  + f'   ({b.GetLayerName(L)})')

    rows = list(csv.reader(open(CPL)))[1:]
    side = collections.Counter(r[3] for r in rows)
    print(f'\nassembly    CPL: {len(rows)} parts, ' + ', '.join(f'{n} {s.lower()}' for s, n in side.items()))
    bottom = {r[0] for r in rows if r[3] == 'Bottom'}
    groups = collections.defaultdict(list)
    for fp in b.GetFootprints():
        if fp.GetReference() in bottom:
            groups[f'{fp.GetValue()} ({fp.GetFPID().GetLibItemName()})'].append(fp.GetReference())
    for k, r in sorted(groups.items(), key=lambda kv: -len(kv[1])):
        print(f'            bottom: {len(r):3} x {k}: {refs_text(r, 8)}')
    placed = {r[0] for r in rows}
    tht = [fp for fp in b.GetFootprints() if fp.GetReference() in placed
           and any(p.GetAttribute() == pcbnew.PAD_ATTRIB_PTH for p in fp.Pads())]
    print(f'            through-hole (soldered by hand at JLC): ' +
          ', '.join(f'{fp.GetReference()} {fp.GetValue()} ({sum(p.GetAttribute() == pcbnew.PAD_ATTRIB_PTH for p in fp.Pads())} pins)'
                    for fp in sorted(tht, key=lambda f: f.GetReference())))


if __name__ == '__main__':
    main()
