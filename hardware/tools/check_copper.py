#!/usr/bin/env python3
"""
Copper conflict report for kvm.kicad_pcb: shorts, stubs, connectivity, plane return paths, pair geometry.

Independent of the KiCad DRC on purpose - it re-derives the geometry from the board file, so a bug in the router
(or in the DRC setup) cannot hide.  Checks:

  clearance   copper of different nets on the same layer, closer than the board's minimum clearance
              (tracks and vias; the routing is where this goes wrong)
  stubs       track ends that touch nothing (a dead end, or a spur off a routed net)
  loops       copper that joins the same two points twice: a net's tracks and vias graphed with an edge wherever two of
              them touch; a connected piece with more edges than nodes - 1 has a cycle.  This is what a *doubled* route
              looks like (rev 8: the SVC pair was laid twice between the USB-C connector and U301, once as fixed copper
              and once by the pair router), and neither the DRC nor the connectivity check can see it - a second path
              is still one net
  orphans     nets with a single pad
  connected   a net's pads and copper must form one group: items join end to end, through vias and pads, and
              through the zone fills (the board's own filled polygons), which is how the DRC sees it too
  pairs       per differential pair: minimum gap between its two lines against the design's own spacing, the widths
              actually used (a squeezed or necked pair shows up here), length skew, vias, stubs
  planes      under every F.Cu high-speed pair: is the work over the In1.Cu GND fill (the stack-up's L2, the pair's
              reference), and how far from the fill's edge - a pair running along a plane split has no return path

The clearance, stub, orphan and pair checks are calibrated: on the committed rev-5 board they agree with the DRC (no
track or via clearance violation, no stub, pair numbers identical to the router's own summary).  The loops check is
calibrated too: on the committed rev-8 board (917 tracks, 1028 pads, 40 nets with copper) it names exactly the two nets
that board really had doubled - SVC_DP and SVC_DM, each reaching U301 by two paths - and nothing else.  The connected
and plane checks are new: their output is reported in full and only counted as a failure once it agrees with the DRC
and the design's own stack-up (see README).

Run from hardware/:   uv run tools/check_copper.py [board.kicad_pcb]   (exit 1 if anything is wrong; the optional
                      board path checks that file instead, e.g. a revision to compare against)
"""
import collections
import math
import os
import sys

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, 'kvm.kicad_pcb')   # a board path here is a check of
#                                                                                  that file instead (e.g. a rev to
#                                                                                  compare against, or a snapshot)
MM = pcbnew.ToMM
CELL = 1.5                       # spatial hash cell, mm
LAYERS = ('F.Cu', 'B.Cu', 'In1.Cu', 'In2.Cu')
PAIR_NAMES = ('H0_', 'H1_', 'H2_', 'PC1_', 'PC2_', 'KB_', 'MS_', 'OT_')
PAIR_W, PAIR_S = 0.15, 0.15      # the design's HS pair width and gap (design.md §11: ~88 ohm differential over L2)
PAIR_SKEW_MAX = 1.2              # mm of length mismatch that is still inside a high-speed pair's budget: 8 ps at
#                              6.7 ps/mm.  Was 0.75 mm (5 ps), the number usually quoted for USB 2.0 HS; rev 9 sets
#                              1.2 mm, which is 0.4 % of a 480 Mb/s bit and still ~6x tighter than the ~50 ps that HS
#                              app notes allow (see check_usb_si.py SKEW_MAX for why).  check_usb_si.py uses the same.
# a pair whose own two lines may sit closer than the pair gap, with the reason it is accepted (printed as such, and
# never counted as a violation): H0_D4 is Hub-0's fourth port and is disabled in this design - its D4 lines are strapped
# and carry nothing - so where the port-4 via row pinches the pair to 0.1346 mm (DRC, at (163.60, 125.75), 5.3 mil and
# comfortably fabricable) it is accepted instead of squeezed further.  Rev 9, 2026-10-26, by decision.
WAIVED_PAIRS = {frozenset(('H0_D4_DP', 'H0_D4_DM')): 'H0_D4 is a disabled port (D4 strapped)'}
PLANE_GAP_MAX = 4.0              # mm: longer than this without plane copper underneath is a return-path break
PLANE_STEP = 0.05                # mm: sampling step along a pair's F.Cu copper for the plane test
VIA_PLANE_CLR = 0.25             # mm: the planes' clearance round a via (zone 0.2 + margin)
GND_REF = 2                      # LAYERS index of the plane a F.Cu pair references: In1.Cu = L2 = GND (stack-up)
TOL = 0.01                       # mm: "touching" for the connectivity check


def seg_seg_dist(a, b, c, d):
    """Shortest distance between segments a-b and c-d."""
    def pt_seg(p, q, r):
        dx, dy = r[0] - q[0], r[1] - q[1]
        L2 = dx * dx + dy * dy
        t = 0.0 if L2 == 0 else max(0.0, min(1.0, ((p[0] - q[0]) * dx + (p[1] - q[1]) * dy) / L2))
        return math.hypot(p[0] - (q[0] + t * dx), p[1] - (q[1] + t * dy))
    if _cross(a, b, c, d):
        return 0.0
    return min(pt_seg(a, c, d), pt_seg(b, c, d), pt_seg(c, a, b), pt_seg(d, a, b))


def _cross(a, b, c, d):
    def side(p, q, r):
        v = (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])
        return (v > 0) - (v < 0)
    return side(a, b, c) * side(a, b, d) < 0 and side(c, d, a) * side(c, d, b) < 0


def bad_pair(gap, strs, wids, p=None, n=None):
    """Is anything wrong with one pair's own geometry: its two lines too close, a dead end, or a line whose width is
    not the design's pair width (a squeezed or necked pair shows up as either).  A pair in WAIVED_PAIRS is accepted as
    it is: the exception is printed, never silently dropped."""
    if p and n and frozenset((p, n)) in WAIVED_PAIRS:
        return False
    return gap < PAIR_S - 1e-6 or strs or any(abs(w - PAIR_W) > 0.01 for w in wids)


def main():
    board = pcbnew.LoadBoard(PCB)
    clearance = MM(board.GetDesignSettings().m_MinClearance)
    track_sh, via_sh, pad_sh = collections.defaultdict(list), collections.defaultdict(list), collections.defaultdict(list)
    ends = collections.defaultdict(list)                  # net -> [(x, y, layer, item-id)]
    items = []                                            # (net, layer, kind, geom, id)
    node_geom, node_layer = {}, {}                         # item id -> geometry, set of layers (connectivity)
    nets_with_copper = set()
    net_pads = collections.Counter()
    for t in board.GetTracks():
        net = t.GetNetname()
        if not net:
            continue
        nets_with_copper.add(net)
        if t.GetClass() == 'PCB_VIA':
            p = t.GetPosition()
            x, y, r = MM(p.x), MM(p.y), MM(t.GetWidth()) / 2
            iid = len(items)
            for L in range(4):
                via_sh[L].append((net, (x, y, r), iid))
            items.append((net, -1, 'via', (x, y, r), iid))
            node_geom[iid], node_layer[iid] = (x, y, r), set(range(4))
            ends[net].append((x, y, -1, iid))
        else:
            s, e = t.GetStart(), t.GetEnd()
            L = LAYERS.index(t.GetLayerName()) if t.GetLayerName() in LAYERS else 0
            g = (MM(s.x), MM(s.y), MM(e.x), MM(e.y), MM(t.GetWidth()) / 2)
            iid = len(items)
            track_sh[L].append((net, g, iid))
            items.append((net, L, 'seg', g, iid))
            node_geom[iid], node_layer[iid] = g, {L}
            for x, y in ((MM(s.x), MM(s.y)), (MM(e.x), MM(e.y))):
                ends[net].append((x, y, L, iid))
    for f in board.GetFootprints():
        for p in f.Pads():
            net = p.GetNetname()
            if not net:
                continue
            net_pads[net] += 1
            pos = p.GetPosition()
            x, y = MM(pos.x), MM(pos.y)
            bb = p.GetBoundingBox()                            # rotation-aware (an unrotated size box misses a
            box = (MM(bb.GetLeft()), MM(bb.GetTop()),          # rotated pad's ends, and a track meeting them then
                   MM(bb.GetRight()), MM(bb.GetBottom()))      # looks unconnected)
            ids = (pcbnew.F_Cu, pcbnew.B_Cu, pcbnew.In1_Cu, pcbnew.In2_Cu)
            pls = {L for L in range(4) if p.IsOnLayer(ids[L])}
            iid = len(items)
            for L in (0, 1):
                if L in pls:
                    pad_sh[L].append((net, box, f.GetReference()))
            items.append((net, -1, 'pad', box, iid))
            node_geom[iid], node_layer[iid] = box, pls or {0}
            ends[net].append((x, y, -1, iid))              # a pad always counts as a connection
    # ── clearance between different nets ─────────────────────────────────────
    bad_exact, bad_bbox = [], []
    waived = []                                           # accepted exceptions (WAIVED_PAIRS), printed, not counted
    def key(x, y):
        return (int(x // CELL), int(y // CELL))

    def span(g):
        if len(g) == 3:                                    # via
            x, y, r = g
            return x - r - clearance, y - r - clearance, x + r + clearance, y + r + clearance
        if len(g) == 4:                                    # pad bbox
            return g
        x1, y1, x2, y2, w = g                              # segment
        return min(x1, x2) - w - clearance, min(y1, y2) - w - clearance, \
               max(x1, x2) + w + clearance, max(y1, y2) + w + clearance

    def bbox_of(g):
        if len(g) == 4:
            return g
        if len(g) == 3:
            x, y, r = g
            return (x - r, y - r, x + r, y + r)
        x1, y1, x2, y2, w = g
        return (min(x1, x2) - w, min(y1, y2) - w, max(x1, x2) + w, max(y1, y2) + w)

    def dist(ga, gb):
        if len(ga) == 4 or len(gb) == 4:                    # a pad: bounding boxes (approximate; DRC is exact)
            a, b = bbox_of(ga), bbox_of(gb)
            dx = max(0.0, a[0] - b[2], b[0] - a[2])
            dy = max(0.0, a[1] - b[3], b[1] - a[3])
            return math.hypot(dx, dy)
        if len(ga) == 5 and len(gb) == 5:
            return seg_seg_dist(ga[:2], ga[2:4], gb[:2], gb[2:4]) - ga[4] - gb[4]
        if len(ga) == 3 and len(gb) == 3:
            return math.hypot(ga[0] - gb[0], ga[1] - gb[1]) - ga[2] - gb[2]
        if len(ga) == 3:                                    # via against a segment
            return seg_seg_dist((ga[0], ga[1]), (ga[0], ga[1]), gb[:2], gb[2:4]) - ga[2] - gb[4]
        return seg_seg_dist((gb[0], gb[1]), (gb[0], gb[1]), ga[:2], ga[2:4]) - gb[2] - ga[4]

    for L in range(4):
        buckets = collections.defaultdict(list)
        entries = [(n, g, i) for n, g, i in track_sh[L]] + \
                  [(n, g, None) for n, g, _ in via_sh[L]] + list(pad_sh[L])
        for e in entries:
            x0, y0, x1, y1 = span(e[1])
            for cx in range(key(x0, y0)[0], key(x1, y1)[0] + 1):
                for cy in range(key(x0, y0)[1], key(x1, y1)[1] + 1):
                    buckets[(cx, cy)].append(e)
        done = set()
        for (cx, cy), here in buckets.items():
            near = list(here)
            for dx, dy in ((1, 0), (0, 1), (1, 1), (-1, 1)):
                near += buckets.get((cx + dx, cy + dy), [])
            for i, e1 in enumerate(near):
                for e2 in near[i + 1:]:
                    n1, g1, r1 = e1
                    n2, g2, r2 = e2
                    if n1 == n2 or (r1 is not None and r1 == r2):
                        continue
                    k = (id(e1), id(e2)) if id(e1) < id(e2) else (id(e2), id(e1))
                    if k in done:
                        continue
                    done.add(k)
                    d = dist(g1, g2)
                    if d < clearance - 1e-6:
                        if frozenset((n1, n2)) in WAIVED_PAIRS:
                            waived.append((d, n1, n2, g1[:2], LAYERS[L]))
                            continue
                        pad_inv = len(g1) == 4 or len(g2) == 4
                        (bad_bbox if pad_inv else bad_exact).append((d, n1, n2, g1[:2], LAYERS[L]))

    # ── stubs: track ends that touch nothing of their own net ────────────────
    def _on_seg(g, x, y):
        x1, y1, x2, y2 = g[:4]
        dx, dy = x2 - x1, y2 - y1
        t = 0.0 if dx == dy == 0 else max(0.0, min(1.0, ((x - x1) * dx + (y - y1) * dy) / (dx * dx + dy * dy)))
        return math.hypot(x - (x1 + t * dx), y - (y1 + t * dy)) <= 0.02

    nvias = collections.defaultdict(list)
    nboxes = collections.defaultdict(list)
    nsegs = {L: collections.defaultdict(list) for L in range(4)}
    for L in range(4):
        for net, g, _ in via_sh[L]:
            nvias[net].append(g)
        for net, g, ref in pad_sh[L]:
            nboxes[net].append(g)
        for net, g, i in track_sh[L]:
            nsegs[L][net].append((i, g))
    stubs = []
    for net, pts in ends.items():
        for (x, y, L, ident) in pts:
            if items[ident][2] == 'pad':                      # a pad is a terminal, not a track end
                continue
            ok = any(math.hypot(g[0] - x, g[1] - y) <= g[2] + 0.01 for g in nvias[net])
            if not ok:
                ok = any(b[0] - 0.02 <= x <= b[2] + 0.02 and b[1] - 0.02 <= y <= b[3] + 0.02
                         for b in nboxes[net])
            if not ok and L in nsegs:
                ok = any(i != ident and _on_seg(g, x, y) for i, g in nsegs[L][net])
            if not ok:
                stubs.append((net, x, y, LAYERS[L] if L in nsegs else 'via'))

    # ── connectivity: a net's pads and copper must be one group ─────────────────────
    # Nodes are items (tracks, vias, pads, zone fills).  Two items of one net are joined when their copper touches:
    # end to end, end on a via, end on (or overlapping) a pad, or lying in a zone's filled polygon.  The fills are
    # the board's own, so a poured net (GND, +3V3) is connected through its planes exactly as the DRC sees it.  Pads
    # are their bounding boxes, as in the clearance check above (exact for rectangles).
    gnd_planes = []
    for z in board.Zones():
        net = z.GetNetname()
        if not net:
            continue
        for lid in z.GetLayerSet().Seq():
            name = board.GetLayerName(lid)
            if name not in LAYERS:
                continue
            try:
                fp = z.GetFilledPolysList(lid)
            except Exception:
                fp = None
            if fp is None or fp.OutlineCount() == 0:
                continue
            L = LAYERS.index(name)
            if net == 'GND' and L == GND_REF:
                gnd_planes.append(fp)
            item = (net, L, 'zone', (fp,), len(items))
            items.append(item)
            node_layer[item[4]] = {L}
    single = sorted(n for n in nets_with_copper if net_pads.get(n) == 1)
    parent = list(range(len(items)))

    def find(a):
        while parent[a] != a:
            parent[a] = parent[parent[a]]
            a = parent[a]
        return a

    def union(a, b):
        ra, rb = find(a), find(b)
        if ra != rb:
            parent[ra] = rb

    def V(x, y):
        return pcbnew.VECTOR2I(pcbnew.FromMM(x), pcbnew.FromMM(y))

    def in_fill(fp, x, y, margin=0.0):
        """Is (x, y) on the zone's filled copper?  With a margin > 0 every point that far away must be inside too, so
        a track creeping along the fill's edge does not count as being on the plane."""
        if margin:
            return all(fp.Contains(V(x + dx, y + dy)) for dx, dy in
                       ((0, 0), (-margin, 0), (margin, 0), (0, -margin), (0, margin)))
        return fp.Contains(V(x, y))

    def bb(i):
        g = node_geom.get(i)
        return (-1e9, -1e9, 1e9, 1e9) if g is None else bbox_of(g)      # a zone: everywhere, the fill test decides

    def samples(i):
        g = node_geom[i]
        if len(g) == 3:                                                # a via: its centre
            return [(g[0], g[1])]
        if items[i][2] == 'pad':                                       # a pad: a 5x5 grid over its box (a zone reaches
            x1, y1, x2, y2 = g                                         # a pad through narrow thermal spokes, so corners
            return [(x1 + (x2 - x1) * a / 4, y1 + (y2 - y1) * b / 4)   # and centre alone can miss it)
                    for a in range(5) for b in range(5)]
        x1, y1, x2, y2, w = g                                          # a track: every 0.2 mm
        k = max(int(math.hypot(x2 - x1, y2 - y1) / 0.2), 1)
        return [(x1 + (x2 - x1) * t / k, y1 + (y2 - y1) * t / k) for t in range(k + 1)]

    by_net = collections.defaultdict(list)
    for i in range(len(items)):
        by_net[items[i][0]].append(i)
    close = 0
    for net, ns in by_net.items():
        plain = [i for i in ns if items[i][2] != 'zone']
        zs = [i for i in ns if items[i][2] == 'zone']
        for a in range(len(plain)):
            ia, ga, ba = plain[a], node_geom[plain[a]], bb(plain[a])
            for b in range(a + 1, len(plain)):
                ib, gb_, bb_ = plain[b], node_geom[plain[b]], bb(plain[b])
                close += 1
                if not (node_layer[ia] & node_layer[ib]):
                    continue
                if ba[0] - TOL > bb_[2] or bb_[0] - TOL > ba[2] or ba[1] - TOL > bb_[3] or bb_[1] - TOL > ba[3]:
                    continue
                if dist(ga, gb_) <= TOL:
                    union(ia, ib)
        for iz in zs:
            L = next(iter(node_layer[iz]))
            for i in plain:
                if L not in node_layer[i]:
                    continue
                bx = bb(i)
                for x, y in samples(i):
                    if bx[0] <= x <= bx[2] and in_fill(items[iz][3][0], x, y):
                        union(i, iz)
                        break
    broken = []
    if os.environ.get('CHECK_WHY'):
        want = set(os.environ['CHECK_WHY'].split(','))
        for net, ns in sorted(by_net.items()):
            if net not in want:
                continue
            groups = collections.defaultdict(list)
            for i in ns:
                groups[find(i)].append(i)
            print(f'\nDEBUG {net}: {len(ns)} items in {len(groups)} groups, pads {net_pads.get(net, 0)}')
            for r, ii in sorted(groups.items(), key=lambda kv: -len(kv[1])):
                kinds = collections.Counter(items[i][2] for i in ii)
                g = next((node_geom[i] for i in ii if i in node_geom), 'zone')
                print(f'  root {r:5d}: {len(ii):4d} items {dict(kinds)} e.g. {g}')
            if os.environ['CHECK_WHY'].startswith('!'):
                for i in ns:
                    print(f'    {i:5d} {items[i][2]:<4} L={sorted(node_layer[i])} {node_geom.get(i, "zone")} '
                          f'root={find(i)}')
    for net, ns in sorted(by_net.items()):
        k = len({find(i) for i in ns})
        if k > 1 and net_pads.get(net, 0) > 1:
            broken.append((net, k))
    # ── loops: copper that joins the same two points twice ──────────────────
    # A graph whose nodes are track *endpoints* (a junction is one node, however many tracks meet there) and whose edges
    # are the track segments: a connected piece with E >= V has a cycle, i.e. the same connection laid twice.  Pads and
    # vias need no special case - a track ending on a via has that via's centre as its endpoint, so the node is shared.
    # A *plane* net may legitimately be meshed (GND stitching), so a loop there is information; on a signal net it means
    # the net reaches a point by two paths, which is what rev 8's SVC pair did between the USB-C connector and U301
    # (see the docstring).  Only track endpoints are used, so a track *passing through* another's end is not a node -
    # that can hide a loop, never invent one.
    PLANE_NETS = ('GND', '+3V3')
    loops = []
    for net, ns in sorted(by_net.items()):
        edges = {}
        for i in ns:
            if items[i][2] != 'seg':
                continue
            g = node_geom[i]
            a, b = (round(g[0], 3), round(g[1], 3)), (round(g[2], 3), round(g[3], 3))
            if a != b:
                edges[(min(a, b), max(a, b))] = i
        adj = collections.defaultdict(set)
        for a, b in edges:
            adj[a].add(b)
            adj[b].add(a)
        seen, comps = set(), 0
        for n in adj:
            if n in seen:
                continue
            comps += 1
            stack = [n]
            seen.add(n)
            while stack:
                for m in adj[stack.pop()]:
                    if m not in seen:
                        seen.add(m)
                        stack.append(m)
        cyc = len(edges) - len(adj) + comps
        if cyc > 0:
            a, b = next(iter(edges))
            loops.append((net, cyc, a, b, net in PLANE_NETS))
    # ── differential pairs ──────────────────────────────────────────────────
    pairs = []
    for p in sorted(n for n in nets_with_copper if n.endswith('_DP')):
        n = p[:-3] + '_DM'
        if n not in nets_with_copper:
            continue
        gp = [g for L in (0, 1) for nn, g, _ in track_sh[L] if nn == p]
        gn = [g for L in (0, 1) for nn, g, _ in track_sh[L] if nn == n]
        lp = sum(math.hypot(g[2] - g[0], g[3] - g[1]) for g in gp)
        ln = sum(math.hypot(g[2] - g[0], g[3] - g[1]) for g in gn)
        gap, where = math.inf, None
        for L in (0, 1):
            for nn1, g1, _ in track_sh[L]:
                if nn1 != p:
                    continue
                for nn2, g2, _ in track_sh[L]:
                    if nn2 != n:
                        continue
                    d = seg_seg_dist(g1[:2], g1[2:4], g2[:2], g2[2:4]) - g1[4] - g2[4]
                    if d < gap:
                        gap, where = d, ((g1[0] + g2[0]) / 2, (g1[1] + g2[1]) / 2, 'FB'[L])
        vias = sum(1 for L in (0, 1) for nn, _, _ in via_sh[L] if nn in (p, n)) // 2
        strs = sum(1 for s in stubs if s[0] in (p, n))
        wids = sorted({round(2 * g[4], 3) for g in gp + gn})
        # reference plane: sample the pair's F.Cu copper and ask the GND plane (In1.Cu, the stack-up's L2, which is
        # what the 0.15/0.15 geometry is for) whether it is underneath, and how far from the fill's edge
        # Each F.Cu segment is sampled every PLANE_STEP and the longest stretch with no plane under it is measured
        # along that segment, in mm.  This used to sample about once per mm, chain the samples of *all* the pair's
        # segments (P and N mixed) and multiply the longest run of misses by "first-to-last distance / count" - not
        # a step at all: on the rev-9 re-route H0_KB read a 4.02 mm gap where the real uncovered stretches are
        # <= 0.40 mm, each at the pair's own via pair.  The pair's own via holes are left out too: In1.Cu keeps its
        # clearance round every via, and a layer change has no plane under its barrel by definition.
        own_vias = [g for L in (0, 1) for nn, g, _ in via_sh[L] if nn in (p, n)]
        hole = lambda x, y: any(math.hypot(x - vx, y - vy) <= vr + VIA_PLANE_CLR for vx, vy, vr in own_vias)
        pts, longest = [], 0.0
        for g in (gg for nn, gg, _ in track_sh[0] if nn in (p, n)):
            k = max(int(math.hypot(g[2] - g[0], g[3] - g[1]) / PLANE_STEP), 1)
            seg_pts = [(g[0] + (g[2] - g[0]) * t / k, g[1] + (g[3] - g[1]) * t / k) for t in range(k + 1)]
            seg_pts = [q for q in seg_pts if not hole(*q)]
            pts += seg_pts
            start = None
            for x, y in seg_pts:
                if not any(in_fill(fp, x, y) for fp in gnd_planes):
                    start = start or (x, y)
                    longest = max(longest, math.dist(start, (x, y)))      # mm of pair copper with no plane under it
                else:
                    start = None
        cov = sum(1 for x, y in pts if any(in_fill(fp, x, y) for fp in gnd_planes))
        near = sum(1 for x, y in pts if any(in_fill(fp, x, y) for fp in gnd_planes)
                   and not any(in_fill(fp, x, y, 0.25) for fp in gnd_planes))
        at = next(((x, y) for x, y in pts if not any(in_fill(fp, x, y) for fp in gnd_planes)), None)
        pairs.append((p, n, lp, ln, gap, where, vias, strs, wids, len(pts), cov, near, at, longest))

    # ── report ──────────────────────────────────────────────────────────────
    print(f'board: {os.path.basename(PCB)}, minimum clearance {clearance:.2f} mm')
    print(f'\n== clearance below {clearance:.2f} mm: tracks/vias (exact) ==')
    if not bad_exact:
        print('  none')
    for d, na, nb, at, L in sorted(bad_exact)[:20]:
        print(f'  {na} <-> {nb}: {d:+.3f} mm at ({at[0]:.2f}, {at[1]:.2f}) {L}')
    print(f'\n== clearance below {clearance:.2f} mm: pad bounding boxes (approximate, DRC is exact) ==')
    if not bad_bbox:
        print('  none')
    for d, na, nb, at, L in sorted(bad_bbox)[:20]:
        print(f'  {na} <-> {nb}: {d:+.3f} mm at ({at[0]:.2f}, {at[1]:.2f}) {L}')
    print(f'\n== stubs (track ends touching nothing of their net): {len(stubs)} ==')
    for net, x, y, L in stubs[:20]:
        print(f'  {net} at ({x:.2f}, {y:.2f}) {L}')
    odd = [n for n in single if not n.startswith('unconnected')]
    print(f'\n== single-pad nets: {len(single)} ({len(odd)} not unconnected-*) ==')
    for n in odd[:20]:
        print(f'  {n}')
    print(f'\n== connectivity: nets whose pads and copper fall into more than one piece: {len(broken)} ==')
    if not broken:
        print('  none')
    for net, k in broken[:20]:
        print(f'  {net}: {k} pieces')
    sig_loops = [l for l in loops if not l[4]]
    print(f'\n== loops (a net reaching the same point by two paths): {len(sig_loops)} signal, '
          f'{len(loops) - len(sig_loops)} plane/information only ==')
    if not loops:
        print('  none')
    for net, k, p, q, plane in loops[:20]:
        print(f'  {net}: {k} redundant path(s), e.g. ({p[0]:.2f},{p[1]:.2f})-({q[0]:.2f},{q[1]:.2f})'
              f'{"  (plane net: meshed copper, information only)" if plane else ""}')
    print('\n== differential pairs ==')
    print(f'  {"pair":<14} {"P mm":>7} {"N mm":>7} {"skew":>6} {"gap mm":>7} {"widths":>11} {"vias":>5} {"stubs":>6}')
    for p, n, lp, ln, gap, where, vias, strs, wids, npt, cov, near, at, longest in pairs:
        flag = '  <-- ' if bad_pair(gap, strs, wids, p, n) else ''
        note = ('  OVERLAP (short)' if gap < 0 else
                ('  gap below 0.150' if gap < PAIR_S - 1e-6 else '')) + \
               ('  skew > budget' if abs(lp - ln) > PAIR_SKEW_MAX else '') + \
               (f'  waived: {WAIVED_PAIRS[frozenset((p, n))]}' if frozenset((p, n)) in WAIVED_PAIRS else '')
        print(f'  {p[:-3]:<14} {lp:7.2f} {ln:7.2f} {abs(lp - ln):6.2f} {gap:7.3f} '
              f'{",".join(f"{w:.2f}" for w in wids):>11} {vias:5} {strs:6}{flag}{note}')
    print('\n== F.Cu pair copper over the In1.Cu plane (L2, GND: the pair\'s reference in the stack-up) ==')
    print(f'  {"pair":<14} {"over fill":>10} {"near edge":>10} {"worst gap":>10}')
    for p, n, lp, ln, gap, where, vias, strs, wids, npt, cov, near, at, longest in pairs:
        flag = '  <-- ' if longest > PLANE_GAP_MAX else ''
        print(f'  {p[:-3]:<14} {cov / max(npt, 1) * 100:9.1f}% {near:10} {longest:9.2f}{flag}')

    bad = (len(bad_exact), len(stubs), len(broken), len(odd),
           sum(1 for q in pairs if bad_pair(q[4], q[7], q[8], q[0], q[1]) or q[13] > PLANE_GAP_MAX), len(sig_loops))
    if waived:
        print(f'\n== accepted exceptions: {len(waived)} clearance item(s), not counted ==')
        reasons = sorted({WAIVED_PAIRS[frozenset((a, b))] for _, a, b, _, _ in waived})
        for d, na, nb, at, L in sorted(waived)[:10]:
            print(f'  {na} <-> {nb}: {d:+.3f} mm at ({at[0]:.2f}, {at[1]:.2f}) {L}')
        for r in reasons:
            print(f'  reason: {r}')
    if any(bad):
        print(f'\nFAIL: {bad[0]} clearance, {bad[1]} stubs, {bad[2]} nets in more than one piece, {bad[3]} orphans, '
              f'{bad[4]} bad pairs (which includes a pair with no GND plane under its F.Cu copper), '
              f'{bad[5]} signal nets with doubled copper')
        print('      (pad-involved clearances are reported above but not counted: bounding boxes are approximate)')
        return 1
    print('\nPASS: no clearance, stub, orphan, doubled-copper or pair violation')
    return 0


if __name__ == '__main__':
    sys.exit(main())



