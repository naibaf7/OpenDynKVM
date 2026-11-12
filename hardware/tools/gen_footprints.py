#!/usr/bin/env python3
"""
KVM project footprints -- generator (KiCad 10 pcbnew API).

Footprints for parts KiCad's library lacks are converted from the EasyEDA footprint JLCPCB
assembles with (so JLC's placement needs no correction), plus the MX hot-swap key footprint
and the OpenDynKVM logo as silkscreen (from ../branding/, drawn by branding/make_logo.py).
EasyEDA data is cached in tools/easyeda_cache/ (fetched like tools/jlc_align.py does).

Run from hardware/ (network only for uncached parts):   python3 tools/gen_footprints.py
"""
import json
import math
import os
import re
import subprocess
import sys
from functools import reduce

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIB = os.path.join(HERE, 'CustomFootprints.pretty')
CACHE = os.path.join(HERE, 'tools', 'easyeda_cache')
UNIT = 0.254   # EasyEDA unit = 10 mil
HDRS = {'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36',
        'Accept': 'application/json, text/javascript, */*; q=0.01', 'Referer': 'https://easyeda.com/'}

# KiCad footprint name -> (LCSC part, description, fab body (w, h) centred on the pad pattern or None)
FROM_EASYEDA = {
    'TI_RUX0012A_VQFN-HR-12_2x2.5mm_P0.5mm': ('C485916', 'TI RUX0012A VQFN-HR-12 2.0x2.5 mm (TPS2121), JLC land pattern', (2.0, 2.5)),
    # The pads are the package's back row, overhanging its body at -Y; the lens is the long face at +Y, away from the
    # pads (datasheet §4, §6).  Every board up to 0.16.0 read it the other way round (errata E3).
    'LED_SK6812SIDE-A_4.0x1.6mm': ('C5378721', 'SK6812SIDE-A side-emitting RGB LED 4.0x2.0x1.6 mm; light exits towards +Y (away from the pads)', None),
}


def easyeda(lcsc):
    path = os.path.join(CACHE, f'{lcsc}.json')
    if not os.path.exists(path):
        os.makedirs(CACHE, exist_ok=True)
        url = f'https://easyeda.com/api/products/{lcsc}/components?version=6.4.19.5'
        cmd = ['curl', '-s', '-m', '30', '-o', path, url] + sum((['-H', f'{k}: {v}'] for k, v in HDRS.items()), [])
        subprocess.run(cmd, check=True)
    return json.load(open(path))['result']['packageDetail']['dataStr']


def V(x, y):
    return pcbnew.VECTOR2I_MM(x, y)


def add_line(fp, layer, a, b, w):
    s = pcbnew.PCB_SHAPE(fp)
    s.SetShape(pcbnew.SHAPE_T_SEGMENT)
    s.SetStart(V(*a))
    s.SetEnd(V(*b))
    s.SetLayer(layer)
    s.SetWidth(pcbnew.FromMM(w))
    fp.Add(s)


def add_rect(fp, layer, x0, y0, x1, y1, w):
    for a, b in [((x0, y0), (x1, y0)), ((x1, y0), (x1, y1)), ((x1, y1), (x0, y1)), ((x0, y1), (x0, y0))]:
        add_line(fp, layer, a, b, w)


def finish(fp, name, desc, extent, body=None):
    """Reference/value text, fab body, courtyard around pads + body."""
    x0, y0, x1, y1 = extent
    if body:
        bx0, by0, bx1, by1 = body
        add_rect(fp, pcbnew.F_Fab, bx0, by0, bx1, by1, 0.1)
        x0, y0, x1, y1 = min(x0, bx0), min(y0, by0), max(x1, bx1), max(y1, by1)
    c = 0.25
    cx0, cy0, cx1, cy1 = [round(v / 0.05) * 0.05 for v in (x0 - c, y0 - c, x1 + c, y1 + c)]
    add_rect(fp, pcbnew.F_CrtYd, cx0, cy0, cx1, cy1, 0.05)
    fp.SetFPID(pcbnew.LIB_ID('CustomFootprints', name))
    fp.SetLibDescription(desc)
    fp.Reference().SetPosition(V(0, cy0 - 0.8))
    fp.Reference().SetLayer(pcbnew.F_SilkS)
    fp.Value().SetPosition(V(0, cy1 + 0.8))
    fp.Value().SetLayer(pcbnew.F_Fab)


def from_easyeda(name, lcsc, desc, body_wh):
    pk = easyeda(lcsc)
    ox, oy = float(pk['head']['x']), float(pk['head']['y'])
    fp = pcbnew.FOOTPRINT(None)
    fp.SetAttributes(pcbnew.FP_SMD)
    xs, ys, body = [], [], None
    for s in pk['shape']:
        f = s.split('~')
        if f[0] == 'PAD':
            shape, num = f[1], f[8]
            cx, cy = (float(f[2]) - ox) * UNIT, (float(f[3]) - oy) * UNIT
            w, h, rot = float(f[4]) * UNIT, float(f[5]) * UNIT, float(f[11] or 0)
            if f[6] != '1':
                sys.exit(f'{name}: only top-layer SMD pads supported (pad {num} layer {f[6]})')
            pad = pcbnew.PAD(fp)
            pad.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
            pad.SetLayerSet(pad.SMDMask())
            pad.SetNumber(num)
            if shape == 'POLYGON':
                v = [float(t) for t in f[10].split()]
                pts = [((v[k] - ox) * UNIT, (v[k + 1] - oy) * UNIT) for k in range(0, len(v), 2)]
                px = sum(p[0] for p in pts) / len(pts)
                py = sum(p[1] for p in pts) / len(pts)
                # anchor: small rect inside the polygon; outline as a custom primitive
                pad.SetShape(pcbnew.F_Cu, pcbnew.PAD_SHAPE_CUSTOM)
                pad.SetAnchorPadShape(pcbnew.F_Cu, pcbnew.PAD_SHAPE_RECT)
                pad.SetSize(pcbnew.F_Cu, V(0.3, 0.3))
                pad.SetPosition(V(px, py))
                chain = pcbnew.VECTOR_VECTOR2I()
                for x, y in pts:
                    chain.append(V(x - px, y - py))
                pad.AddPrimitivePoly(pcbnew.F_Cu, chain, 0, True)
                xs += [p[0] for p in pts]
                ys += [p[1] for p in pts]
            else:
                if rot in (90, 270):
                    w, h = h, w
                elif rot not in (0, 180):
                    sys.exit(f'{name}: pad {num} rotation {rot} not supported')
                pad.SetShape(pcbnew.F_Cu, pcbnew.PAD_SHAPE_RECT if shape == 'RECT' else pcbnew.PAD_SHAPE_OVAL)
                pad.SetSize(pcbnew.F_Cu, V(w, h))
                pad.SetPosition(V(cx, cy))
                xs += [cx - w / 2, cx + w / 2]
                ys += [cy - h / 2, cy + h / 2]
            fp.Add(pad)
        elif f[0] == 'TRACK' and f[2] == '3':          # top silkscreen polyline
            width = float(f[1]) * UNIT
            v = [float(t) for t in f[4].split()]
            pts = [((v[k] - ox) * UNIT, (v[k + 1] - oy) * UNIT) for k in range(0, len(v), 2)]
            for a, b in zip(pts, pts[1:]):
                add_line(fp, pcbnew.F_SilkS, a, b, max(width, 0.12))
        elif f[0] == 'SOLIDREGION' and f[1] == '99':    # component body outline
            v = [t for t in f[3].replace('M', ' ').replace('L', ' ').replace('Z', ' ').replace(',', ' ').split()]
            nums = [float(t) for t in v]
            bx = [(nums[k] - ox) * UNIT for k in range(0, len(nums), 2)]
            by = [(nums[k + 1] - oy) * UNIT for k in range(0, len(nums), 2)]
            body = (min(bx), min(by), max(bx), max(by))
    extent = (min(xs), min(ys), max(xs), max(ys))
    if body is None and body_wh:
        cx, cy = (extent[0] + extent[2]) / 2, (extent[1] + extent[3]) / 2
        body = (cx - body_wh[0] / 2, cy - body_wh[1] / 2, cx + body_wh[0] / 2, cy + body_wh[1] / 2)
    finish(fp, name, desc, extent, body)
    return fp


def mx_hotswap():
    """MX switch in a Kailh CPG151101S11 hot-swap socket, origin = switch centre (top view).
    Holes per the Cherry MX pattern: centre post 4.0, plastic pegs 1.75 at +-5.08 (5-pin switches),
    switch pins 3.05 at (2.54, -5.08) / (-3.81, -2.54) going into the socket.  The socket sits on the
    bottom: pads 2.55 x 2.5 outboard of each pin hole (Kailh drawing, section 8).  JLC reflows it with the
    rest of the bottom side (decided 2026-10-27: double-sided assembly, no BOM exclusion), so the pads get
    paste.  Pad 1 = switch pin at (2.54, -5.08)."""
    name = 'SW_MX_Hotswap_Kailh_1u'
    fp = pcbnew.FOOTPRINT(None)
    fp.SetAttributes(pcbnew.FP_SMD)

    def hole(x, y, d):
        p = pcbnew.PAD(fp)
        p.SetAttribute(pcbnew.PAD_ATTRIB_NPTH)
        p.SetShape(pcbnew.F_Cu, pcbnew.PAD_SHAPE_CIRCLE)
        p.SetSize(pcbnew.F_Cu, V(d, d))
        p.SetDrillSize(V(d, d))
        p.SetLayerSet(p.UnplatedHoleMask())
        p.SetPosition(V(x, y))
        fp.Add(p)
    hole(0, 0, 4.0)
    hole(-5.08, 0, 1.75)
    hole(5.08, 0, 1.75)
    hole(2.54, -5.08, 3.05)
    hole(-3.81, -2.54, 3.05)
    for num, (x, y) in (('1', (5.842, -5.08)), ('2', (-7.085, -2.54))):
        p = pcbnew.PAD(fp)
        p.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
        ls = pcbnew.LSET()
        ls.AddLayer(pcbnew.B_Cu)
        ls.AddLayer(pcbnew.B_Mask)
        ls.AddLayer(pcbnew.B_Paste)
        p.SetLayerSet(ls)
        p.SetShape(pcbnew.B_Cu, pcbnew.PAD_SHAPE_RECT)
        p.SetSize(pcbnew.B_Cu, V(2.55, 2.5))
        p.SetNumber(num)
        p.SetPosition(V(x, y))
        fp.Add(p)
    add_rect(fp, pcbnew.F_Fab, -7.0, -7.0, 7.0, 7.0, 0.1)            # switch body
    add_rect(fp, pcbnew.F_CrtYd, -7.25, -7.25, 7.25, 7.25, 0.05)
    add_rect(fp, pcbnew.Dwgs_User, -9.0, -9.0, 9.0, 9.0, 0.1)          # 1u keycap
    add_rect(fp, pcbnew.B_Fab, -8.36, -6.6, 7.12, -1.0, 0.1)           # socket body
    add_rect(fp, pcbnew.B_CrtYd, -8.6, -6.85, 7.4, -1.05, 0.05)
    fp.SetFPID(pcbnew.LIB_ID('CustomFootprints', name))
    fp.SetLibDescription('Cherry MX-compatible switch in a Kailh CPG151101S11 hot-swap socket (socket on the bottom side, '
                         'reflowed with it); 5-pin peg holes; LED goes at (0, +5.08) as a separate reverse-mount part')
    fp.Reference().SetPosition(V(0, -8.0))
    fp.Reference().SetLayer(pcbnew.F_Fab)
    fp.Value().SetPosition(V(0, 8.0))
    fp.Value().SetLayer(pcbnew.F_Fab)
    return name, fp


# The logo, mark over wordmark, LOGO_W mm wide.  At that width the wordmark's thinnest strokes ("KVM", Montserrat
# Regular) are 0.14 mm; LOGO_GAIN thickens every edge so they reach the 0.15 mm JLC prints as a silkscreen line.
LOGO_SVG = os.path.join(HERE, '..', 'branding', 'opendynkvm-logo-on-light.svg')
LOGO_W = 14.0
LOGO_GAIN = 0.01


def svg_rings(d, step=1.0):
    """The closed subpaths of SVG path data (absolute M, L, H, V, C, Q, Z) as polylines, curves cut into pieces of
    about `step` units."""
    rings, ring, pos = [], [], (0.0, 0.0)
    tokens = re.findall(r'[MLHVCQZ]|-?(?:\d+\.?\d*|\.\d+)(?:e-?\d+)?', d)
    i, cmd = 0, None

    def take(n):
        nonlocal i
        vals = [float(t) for t in tokens[i:i + n]]
        i += n
        return vals

    def curve(pts):
        length = sum(math.dist(a, b) for a, b in zip(pts, pts[1:]))
        n = max(2, math.ceil(length / step))
        k = len(pts) - 1
        for j in range(1, n + 1):
            t = j / n
            ring.append(tuple(sum(math.comb(k, m) * (1 - t) ** (k - m) * t ** m * p[c] for m, p in enumerate(pts))
                              for c in (0, 1)))

    while i < len(tokens):
        if tokens[i].isalpha():
            cmd = tokens[i]
            i += 1
        if cmd == 'M':
            if len(ring) > 2:
                rings.append(ring)
            pos = tuple(take(2))
            ring = [pos]
            cmd = 'L'                            # further pairs after M are lines
        elif cmd == 'L':
            pos = tuple(take(2))
            ring.append(pos)
        elif cmd == 'H':
            pos = (take(1)[0], pos[1])
            ring.append(pos)
        elif cmd == 'V':
            pos = (pos[0], take(1)[0])
            ring.append(pos)
        elif cmd in 'CQ':
            v = take(6 if cmd == 'C' else 4)
            pts = [pos] + [tuple(v[j:j + 2]) for j in range(0, len(v), 2)]
            curve(pts)
            pos = pts[-1]
        elif cmd == 'Z':
            if len(ring) > 2:
                rings.append(ring)
            ring = []
            cmd = None
        else:
            sys.exit(f'{LOGO_SVG}: unexpected path data at {tokens[i]!r}')
    if len(ring) > 2:
        rings.append(ring)
    return rings


def logo():
    """The OpenDynKVM logo as filled F.SilkS polygons, centred on the origin; board-only, not in the BOM or the CPL."""
    text = open(LOGO_SVG).read()
    w, h = (float(v) for v in re.search(r'viewBox="0 0 ([\d.]+) ([\d.]+)"', text).groups())
    d = ' '.join(re.findall(r' d="([^"]+)"', text))
    k = LOGO_W / w
    from shapely.geometry import Polygon          # only the logo needs shapely
    rings = [Polygon([((x - w / 2) * k, (y - h / 2) * k) for x, y in r]).buffer(0) for r in svg_rings(d)]
    ink = reduce(lambda a, b: a.symmetric_difference(b), rings)       # the even-odd area: holes stay holes
    ink = ink.buffer(LOGO_GAIN, join_style='mitre')
    name = f'OpenDynKVM_Logo_{LOGO_W:g}mm_SilkScreen'
    fp = pcbnew.FOOTPRINT(None)
    for poly in getattr(ink, 'geoms', [ink]):
        s = pcbnew.SHAPE_POLY_SET()
        o = s.NewOutline()
        for x, y in poly.exterior.coords[:-1]:
            s.Append(pcbnew.FromMM(x), pcbnew.FromMM(y), o)
        for hole in poly.interiors:
            hi = s.NewHole(o)
            for x, y in hole.coords[:-1]:
                s.Append(pcbnew.FromMM(x), pcbnew.FromMM(y), o, hi)
        s.Fracture()                             # holes bridged into the outline, as KiCad stores a filled polygon
        shape = pcbnew.PCB_SHAPE(fp)
        shape.SetShape(pcbnew.SHAPE_T_POLY)
        shape.SetPolyShape(s)
        shape.SetLayer(pcbnew.F_SilkS)
        shape.SetWidth(0)
        shape.SetFilled(True)
        fp.Add(shape)
    fp.SetFPID(pcbnew.LIB_ID('CustomFootprints', name))
    fp.SetLibDescription(f'OpenDynKVM logo (branding/opendynkvm-logo-on-light.svg), {LOGO_W:g} mm wide, silkscreen')
    fp.SetAttributes(pcbnew.FP_EXCLUDE_FROM_POS_FILES | pcbnew.FP_EXCLUDE_FROM_BOM | pcbnew.FP_BOARD_ONLY)
    fp.SetAllowMissingCourtyard(True)
    fp.Reference().SetText('LOGO**')
    fp.Reference().SetLayer(pcbnew.F_SilkS)
    fp.Reference().SetVisible(False)
    fp.Value().SetText(name)
    fp.Value().SetLayer(pcbnew.F_Fab)
    fp.Value().SetVisible(False)
    return name, fp, ink.bounds


MINI1_SRC = ('/usr/share/kicad/footprints/RF_Module.pretty', 'ESP32-C6-MINI-1')


def esp32_c3_mini():
    """ESP32-C3-MINI-1 (0.16.0's Bluetooth LE coprocessor).  Its land pattern (datasheet v2.2 figure 11-1: 48 pads
    0.8 x 0.4 at 0.8 mm, a 3 x 3 grid of 1.45 mm ground pads, four 0.7 mm corner pads, 13.2 x 16.6 mm body) is the
    one KiCad ships for the ESP32-C6-MINI-1, the same module outline; the pads are taken from there.  Left out: that
    footprint's keep-out zone (43.2 x 20.4 mm, Espressif's 15 mm round a module whose antenna sits on the board) and
    its courtyard, which follows that zone.  The board places its own antenna keep-out (tools/gen_pcb.py ANT_KEEPOUT),
    cut to what is on the board, and the courtyard here is the module body: the antenna end at y = -11, the pad end
    at +5.6 (the C3 and C6 modules have the same outline, datasheet figure 10-1)."""
    src = pcbnew.FootprintLoad(*MINI1_SRC)
    fp = pcbnew.FOOTPRINT(None)
    fp.SetAttributes(pcbnew.FP_SMD)
    for p in src.Pads():
        q = p.Duplicate()
        q.SetParent(fp)
        fp.Add(q)
    add_rect(fp, pcbnew.F_Fab, -6.6, -11.0, 6.6, 5.6, 0.1)
    add_line(fp, pcbnew.F_Fab, (-6.6, -5.6), (6.6, -5.6), 0.1)          # where the antenna area begins
    add_rect(fp, pcbnew.F_SilkS, -6.75, -11.15, 6.75, 5.75, 0.12)
    add_line(fp, pcbnew.F_SilkS, (-7.2, -4.0), (-7.2, -3.2), 0.12)      # pin 1 mark
    name = 'ESP32-C3-MINI-1'
    finish(fp, name, 'Espressif ESP32-C3-MINI-1 (-H4X), 13.2 x 16.6 mm LGA-53, PCB antenna towards -Y; land pattern '
           'of datasheet v2.2 figure 11-1 (KiCad ESP32-C6-MINI-1 pads, same outline). The antenna keep-out on '
           'the board is placed with the footprint (tools/gen_pcb.py)', (-6.6, -11.0, 6.6, 5.6))
    return name, fp


def main():
    os.makedirs(LIB, exist_ok=True)
    name, fp = mx_hotswap()
    pcbnew.FootprintSave(LIB, fp)
    print(f'{name}: MX holes + hot-swap socket pads')
    name, fp = esp32_c3_mini()
    pcbnew.FootprintSave(LIB, fp)
    print(f'{name}: {len(fp.Pads())} pads (from KiCad {MINI1_SRC[1]})')
    try:
        name, fp, (x0, y0, x1, y1) = logo()
        pcbnew.FootprintSave(LIB, fp)
        print(f'{name}: {x1 - x0:.2f} x {y1 - y0:.2f} mm')
    except ImportError:
        print('logo: shapely is not installed, the logo footprint is left as it is')
    for name, (lcsc, desc, body) in FROM_EASYEDA.items():
        fp = from_easyeda(name, lcsc, desc, body)
        pcbnew.FootprintSave(LIB, fp)
        print(f'{name}: {len(fp.Pads())} pads (from EasyEDA {lcsc})')


if __name__ == '__main__':
    main()
