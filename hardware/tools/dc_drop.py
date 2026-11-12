#!/usr/bin/env python3
"""
DC drop of one net's routed copper under a load case: how much voltage a supply loses between its source and its
loads, and which segments carry the current.  Answers questions like "is 0.5 oz inner copper enough?" (jlcpcb.md).

Model: every track segment is a resistor (copper 1.72e-8 ohm m; the thickness of its layer from the board's stack-up,
or --outer / --inner), a via is its plated barrel (VIA_PLATING), pads are ideal conductors.  Copper joins where a
track end overlaps another track, a via pad or a pad - the same overlap rule as KiCad's connectivity.  Zones are not
modelled, so a net with a zone is refused (GND and +3V3 are planes; +5V and the VBUS nets are tracks only).

Load case: the SOURCE pads sit at 0 V and every SINK draws its current at the same time.  Printed:
  - the drop at each sink, and the two-terminal resistance source -> sink (that sink alone);
  - the net's copper per layer and its share of the I^2 R loss;
  - the most heavily loaded segments, with an IPC-2221 temperature rise from its external chart (k = 0.048).  For
    an inner-layer segment the internal chart (k = 0.024) is printed too: it assumes a trace with no copper around
    it and wildly overstates one that runs inside a plane layer (IPC-2152), so it is an upper bound, not an estimate.

Run from hardware/:   uv run tools/dc_drop.py NET SOURCE SINK[=AMPS] ... [--outer UM] [--inner UM] [--top N]
  SOURCE  comma list of REF.PIN, tied together (e.g. U402.1,U402.8: the TPS2121's OUT pins)
  SINK    REF (all its pads on NET, tied) or a comma list of REF.PIN; AMPS defaults to 1
Example (jlcpcb.md, copper weight; three front ports at 0.5 A and the 3.3 V buck at 0.5 A):
  uv run tools/dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5
"""

import collections
import math
import os
import sys

import numpy as np
import pcbnew
import scipy.sparse as sp
import scipy.sparse.csgraph as csgraph
import scipy.sparse.linalg as spl

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import board_stackup  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
RHO = 1.72e-8                  # copper, ohm m at 20 C
VIA_PLATING = 0.018            # mm, JLC's minimum barrel plating
R_TIE = 1e-7                   # ohm: pads, and points that are the same copper
IPC_K = {'outer': 0.048, 'inner': 0.024}


def args():
    a = sys.argv[1:]
    opts = {'--outer': None, '--inner': None, '--top': '8'}
    for o in list(opts):
        if o in a:
            i = a.index(o)
            opts[o] = a[i + 1]
            del a[i:i + 2]
    if len(a) < 3:
        sys.exit(__doc__)
    sinks = []
    for s in a[2:]:
        name, _, amps = s.partition('=')
        sinks.append((name, float(amps) if amps else 1.0))
    return a[0], a[1].split(','), sinks, opts


def main():
    net, source, sinks, opts = args()
    board = pcbnew.LoadBoard(PCB)
    mm = pcbnew.ToMM
    thick = {name: t for name, kind, t, _ in board_stackup(PCB) if kind == 'copper'}
    if opts['--outer']:
        thick.update({'F.Cu': float(opts['--outer']) / 1000, 'B.Cu': float(opts['--outer']) / 1000})
    if opts['--inner']:
        thick.update({n: float(opts['--inner']) / 1000 for n in thick if n.startswith('In')})
    if not thick:
        sys.exit('the board has no stack-up (uv run tools/gen_pcb.py --stackup)')
    if any(z.GetNetname() == net for z in board.Zones()):
        sys.exit(f'{net} has a zone; zones are not modelled')

    segs, vias, pads = [], [], {}
    for t in board.GetTracks():
        if t.GetNetname() != net:
            continue
        if t.GetClass() == 'PCB_TRACK':
            s, e = t.GetStart(), t.GetEnd()
            segs.append((mm(s.x), mm(s.y), mm(e.x), mm(e.y), mm(t.GetWidth()), board.GetLayerName(t.GetLayer())))
        elif t.GetClass() == 'PCB_VIA':
            p = t.GetPosition()
            vias.append((mm(p.x), mm(p.y), mm(t.GetWidth(pcbnew.F_Cu)), mm(t.GetDrill())))
    for fp in board.GetFootprints():
        for p in fp.Pads():
            if p.GetNetname() == net:
                pads[f'{fp.GetReference()}.{p.GetNumber()}'] = (p, {board.GetLayerName(lay) for lay in p.GetLayerSet().CuStack()})

    nodes, half = {}, {}                        # (x, y, layer) -> index; the copper's half width at that point

    def node(x, y, layer, hw):
        key = (round(x, 4), round(y, 4), layer)
        if key not in nodes:
            nodes[key] = len(nodes)
        half[nodes[key]] = max(half.get(nodes[key], 0), hw)
        return nodes[key]

    edges = []                                  # (a, b, ohm, segment index or None)
    points = [(x0, y0, w, lay) for x0, y0, _, _, w, lay in segs] + [(x1, y1, w, lay) for _, _, x1, y1, w, lay in segs]
    points += [(x, y, d, lay) for x, y, d, _ in vias for lay in thick]
    for i, (x0, y0, x1, y1, w, lay) in enumerate(segs):
        dx, dy = x1 - x0, y1 - y0
        ln = math.hypot(dx, dy)
        on = [(0.0, node(x0, y0, lay, w / 2)), (1.0, node(x1, y1, lay, w / 2))]
        for px, py, pw, play in points:
            if play != lay:
                continue
            u = 0.0 if ln == 0 else min(max(((px - x0) * dx + (py - y0) * dy) / ln ** 2, 0.0), 1.0)
            if math.hypot(px - x0 - u * dx, py - y0 - u * dy) < (w + pw) / 2 - 1e-4:    # copper overlaps
                on.append((u, node(px, py, lay, pw / 2)))
        on = sorted(set(on))
        for (u1, a), (u2, b) in zip(on, on[1:]):
            if a != b:
                r = RHO * (u2 - u1) * ln * 1e-3 / (w * 1e-3 * thick[lay] * 1e-3)
                edges.append((a, b, max(r, R_TIE), i))
    for x, y, d, drill in vias:
        r = RHO * 1.6e-3 / (math.pi * drill * 1e-3 * VIA_PLATING * 1e-3)
        on = [nodes[k] for k in ((round(x, 4), round(y, 4), lay) for lay in thick) if k in nodes]
        edges += [(a, b, r / max(len(on) - 1, 1), None) for a, b in zip(on, on[1:])]
    padnode = {}
    for name, (p, lays) in pads.items():
        padnode[name] = len(nodes) + len(padnode)
    for key, i in nodes.items():
        for name, (p, lays) in pads.items():
            if key[2] in lays and p.HitTest(pcbnew.VECTOR2I_MM(key[0], key[1]), pcbnew.FromMM(half[i])):
                edges.append((padnode[name], i, R_TIE, None))
    n = len(nodes) + len(padnode)

    def group(spec):
        names = sorted(p for p in pads if p.split('.')[0] == spec) or spec.split(',')
        missing = [p for p in names if p not in padnode]
        if missing:
            sys.exit(f'not a pad of {net}: {", ".join(missing)}')
        return [padnode[p] for p in names]

    src = group(','.join(source)) if len(source) > 1 else group(source[0])
    loads = [(name, group(name), amps) for name, amps in sinks]
    ties = [(g[0], x) for g in [src] + [g for _, g, _ in loads] for x in g[1:]]
    rows = [a for a, b, _, _ in edges] + [a for a, b in ties]
    cols = [b for a, b, _, _ in edges] + [b for a, b in ties]
    cond = [1 / r for _, _, r, _ in edges] + [1 / R_TIE] * len(ties)
    adj = sp.coo_matrix((cond, (rows, cols)), shape=(n, n)).tocsr()
    adj = adj + adj.T
    _, comp = csgraph.connected_components(adj > 0)
    for name, g, _ in loads:
        if comp[g[0]] != comp[src[0]]:
            sys.exit(f'{name} is not connected to {",".join(source)} in the modelled copper')
    G = sp.diags(np.asarray(adj.sum(axis=1)).ravel()) - adj
    free = [i for i in range(n) if i != src[0] and comp[i] == comp[src[0]]]
    idx = {i: k for k, i in enumerate(free)}
    A = G[free][:, free].tocsc()

    def solve(currents):
        rhs = np.zeros(len(free))
        for g, amps in currents:
            rhs[idx[g[0]]] -= amps
        v = np.zeros(n)
        v[free] = spl.spsolve(A, rhs)
        return v

    v = solve([(g, amps) for _, g, amps in loads])
    total = sum(amps for _, _, amps in loads)
    lay_of = {i: lay for i, (*_, lay) in enumerate(segs)}
    print(f'{net}: {len(segs)} segments, {len(vias)} vias, {len(pads)} pads; copper '
          + ', '.join(f'{k} {v * 1000:.1f} um' for k, v in thick.items()))
    print(f'\nload case: {total:.2f} A from {",".join(source)}')
    for name, g, amps in loads:
        r2 = -solve([(g, 1.0)])[g[0]]
        print(f'  {name:16} {amps:5.2f} A   drop {-v[g[0]] * 1000:6.1f} mV   (alone: {r2 * 1000:6.1f} mOhm)')

    per_layer = collections.defaultdict(lambda: [0.0, 0.0])
    seg_i = collections.defaultdict(float)
    for a, b, r, si in edges:
        if si is None:
            continue
        i = abs(v[a] - v[b]) / r
        seg_i[si] = max(seg_i[si], i)
        per_layer[lay_of[si]][1] += i * i * r
    for x0, y0, x1, y1, w, lay in segs:
        per_layer[lay][0] += math.hypot(x1 - x0, y1 - y0)
    loss = sum(p for _, p in per_layer.values())
    print('\nper layer:')
    for lay in thick:
        if lay in per_layer:
            length, p = per_layer[lay]
            print(f'  {lay:7} {length:7.1f} mm of track   {p * 1000:6.1f} mW  ({100 * p / loss:4.1f} % of the loss)')

    print('\nmost loaded segments (IPC-2221 rise, external chart; inner layers also the internal-chart upper bound):')
    for si, i in sorted(seg_i.items(), key=lambda kv: -kv[1])[:int(opts['--top'])]:
        x0, y0, x1, y1, w, lay = segs[si]
        area = (w / 0.0254) * (thick[lay] / 0.0254)                     # mil^2
        rise = {c: (i / (k * area ** 0.725)) ** (1 / 0.44) for c, k in IPC_K.items()}
        print(f'  {lay:7} w {w:.2f} mm  ({x0:.2f},{y0:.2f})-({x1:.2f},{y1:.2f})  {i:5.2f} A  '
              f'{i / (w * thick[lay]):5.1f} A/mm2  dT ~{rise["outer"]:4.1f} K'
              + (f'  (internal chart <= {rise["inner"]:.0f} K)' if lay.startswith('In') else ''))


if __name__ == '__main__':
    main()
