#!/usr/bin/env python3
"""
KVM project footprints -- generator (KiCad 10 pcbnew API).

Footprints for parts KiCad's library lacks are converted from the EasyEDA footprint JLCPCB
assembles with (so JLC's placement needs no correction), plus the MX hot-swap key footprint.
EasyEDA data is cached in tools/easyeda_cache/ (fetched like tools/jlc_align.py does).

Run from hardware/ (network only for uncached parts):   python3 tools/gen_footprints.py
"""
import json
import os
import subprocess
import sys

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
    'LED_SK6812SIDE-A_4.0x1.6mm': ('C5378721', 'SK6812SIDE-A side-emitting RGB LED 4.0x2.0x1.6 mm; light exits towards -Y (pad side)', None),
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


def main():
    os.makedirs(LIB, exist_ok=True)
    name, fp = mx_hotswap()
    pcbnew.FootprintSave(LIB, fp)
    print(f'{name}: MX holes + hot-swap socket pads')
    for name, (lcsc, desc, body) in FROM_EASYEDA.items():
        fp = from_easyeda(name, lcsc, desc, body)
        pcbnew.FootprintSave(LIB, fp)
        print(f'{name}: {len(fp.Pads())} pads (from EasyEDA {lcsc})')


if __name__ == '__main__':
    main()
