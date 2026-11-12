#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11,<3.15"
# dependencies = ["build123d==0.13.0"]
# ///
"""3D models for the parts KiCad's 3D library has none for, and the MX key's switch and keycap.

    uv run tools/fetch_3d.py            (from hardware/; network only for models not in 3dmodels/ yet)

KiCad's 3D library (Fedora: kicad-packages3d; part of KiCad's installer on Windows and macOS) covers most of the
board.  It has no file for the RP2354B's QFN-80, the TS3USB3031's WQFN-12, the HRO USB-C receptacle or the TS-1187A
button (their KiCad footprints name a model that KiCad 10.0.8 does not ship), nor anything for our own footprints
(CustomFootprints.pretty).  For every LCSC part whose library footprint names no model that exists, this takes the
part's EasyEDA model - the one JLC's placement preview shows - and puts it on the KiCad footprint:

- EasyEDA puts a model's bounding box, turned by c_rotation (counter-clockwise), centred on c_origin, its lowest
  point on the board plus z;
- tools/jlc_corrections.json (tools/jlc_align.py) maps EasyEDA's footprint onto KiCad's: p = R(theta) p_e + t;
- a part soldered on the bottom under a top footprint (the MX key's hot-swap socket) is that, seen from below.

Checked against every part that has both models (2026-11-12, 31 LCSC parts): the boxes agree within 0.2 mm, rotated
and bottom-side parts included, except three whose EasyEDA c_origin is itself off (C19702, C45783, C7519).  All
three keep KiCad's model.  The USB-A receptacle, whose EasyEDA model is far off-centre, is what rules out "the model's
origin on c_origin".

The MX switch and the keycap are drawn here: a plain Cherry MX outline, and a keycap to the case's numbers
(case/gen_case.py KEY: 18.1 mm wide, its top 21.0 mm over the board, its skirt at 8.5 mm when pressed, so 12.5 mm
at rest after the switch's 4 mm travel).

Writes the STEP files to 3dmodels/ (a local cache, not in git: EasyEDA's terms for redistributing them are unclear)
and their placement to tools/models_3d.json, which tools/gen_pcb.py puts on the footprints
(`uv run tools/gen_pcb.py --models` on a routed board).  tools/render_3d.py then renders the board.
"""
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path

from build123d import (Align, Box, Color, Compound, Cylinder, Plane, Pos, RectangleRounded, Sphere, export_step,
                       import_step, loft)

TOOLS = Path(__file__).resolve().parent
HW = TOOLS.parent
sys.path.insert(0, str(TOOLS))
from sexpr import find, first, parse, prop  # noqa: E402

PCB = HW / 'kvm.kicad_pcb'
CORR = TOOLS / 'jlc_corrections.json'
CACHE = TOOLS / 'easyeda_cache'
MODELS = HW / '3dmodels'
OUT = TOOLS / 'models_3d.json'
KICAD3D = Path(os.environ.get('KICAD10_3DMODEL_DIR', '/usr/share/kicad/3dmodels'))
KICADFP = Path(os.environ.get('KICAD10_FOOTPRINT_DIR', '/usr/share/kicad/footprints'))
FPLIBS = {'CustomFootprints': HW / 'CustomFootprints.pretty'}
PRJ = '${KIPRJMOD}/3dmodels'
UNIT = 0.254                    # EasyEDA unit = 10 mil
HDRS = {'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 '
                      'Safari/537.36',
        'Accept': 'application/json, text/javascript, */*; q=0.01', 'Referer': 'https://easyeda.com/'}
STEP_URL = 'https://modules.easyeda.com/qAxj6KHrDKw4blvCG8QJPs7Y/{}'

BOTTOM = (Align.CENTER, Align.CENTER, Align.MIN)
KEY_FP = 'SW_MX_Hotswap_Kailh_1u'
TRAVEL = 4.0                    # MX switch travel
CAP_W, CAP_TOP, CAP_LOW = 18.1, 21.0, 8.5      # case/gen_case.py KEY: width, top and pressed skirt over the board
CAP_COLOUR = (0.91, 0.90, 0.87)                # off-white
HOUSING, TOP_HOUSING, STEM = (0.16, 0.17, 0.18), (0.24, 0.25, 0.27), (0.85, 0.85, 0.85)
METAL = (0.80, 0.68, 0.38)


# ── 3 x 3 rotations, exact for quarter turns ──────────────────────────────────────────────────────────────────────
def rot(axis, deg):
    c, s = round(math.cos(math.radians(deg)), 12), round(math.sin(math.radians(deg)), 12)
    return {'x': [[1, 0, 0], [0, c, -s], [0, s, c]],
            'y': [[c, 0, s], [0, 1, 0], [-s, 0, c]],
            'z': [[c, -s, 0], [s, c, 0], [0, 0, 1]]}[axis]


def mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]


def app(m, p):
    return tuple(sum(m[i][k] * p[k] for k in range(3)) for i in range(3))


def kicad_angles(m):
    """KiCad turns a model by Rz(-rz) Ry(-ry) Rx(-rx) (its 3D viewer and STEP export): the (rx, ry, rz) for m."""
    b = math.degrees(math.asin(max(-1.0, min(1.0, -m[2][0]))))
    if abs(m[2][0]) < 1 - 1e-9:
        a = math.degrees(math.atan2(m[2][1], m[2][2]))
        g = math.degrees(math.atan2(m[1][0], m[0][0]))
    else:
        a, g = math.degrees(math.atan2(-m[1][2], m[1][1])), 0.0
    back = mul(mul(rot('z', g), rot('y', b)), rot('x', a))
    if any(abs(back[i][j] - m[i][j]) > 1e-6 for i in range(3) for j in range(3)):
        sys.exit(f'cannot express {m} as KiCad model angles')
    return [round(-v % 360, 6) % 360 for v in (a, b, g)]


def clean(v, nd=4):
    v = round(v, nd)
    return 0.0 if v == 0 else v


# ── the board and the libraries ─────────────────────────────────────────────────────────────────────────────────
def board_parts():
    """{LCSC: (footprint id, mirrored)} for every LCSC part on the board; mirrored = a top footprint whose soldered
    pads are all on B.Cu (the part sits under the board, seen from below: tools/jlc_align.py kicad_pads)."""
    tree = parse(PCB.read_text())
    parts = {}
    for fp in find(tree, 'footprint'):
        p = prop(fp, 'LCSC')
        if not p or not p[2] or p[2] in parts:
            continue
        pads = [q for q in find(fp, 'pad') if q[2] != 'np_thru_hole']
        copper = [str(x) for q in pads for x in (first(q, 'layers') or [])[1:] if str(x).endswith('.Cu')]
        top = str(first(fp, 'layer')[1]) == 'F.Cu'
        parts[str(p[2])] = (str(fp[1]), bool(top and copper and not any(c in ('F.Cu', '*.Cu') for c in copper)))
    return parts, float(re.search(r'\(thickness ([\d.]+)\)', PCB.read_text()).group(1))


def library_models(fpid):
    """The model files a library footprint names, resolved; [] if it names none."""
    lib, name = fpid.split(':')
    path = FPLIBS.get(lib, KICADFP / f'{lib}.pretty') / f'{name}.kicad_mod'
    if not Path(path).exists():
        sys.exit(f'footprint {fpid}: {path} not found')
    files = re.findall(r'\(model "([^"]+)"', Path(path).read_text())
    return [Path(f.replace('${KICAD10_3DMODEL_DIR}', str(KICAD3D)).replace('${KIPRJMOD}', str(HW))) for f in files]


# ── EasyEDA ─────────────────────────────────────────────────────────────────────────────────────────────────────
def curl(url, path):
    headers = sum((['-H', f'{k}: {v}'] for k, v in HDRS.items()), [])
    cmd = ['curl', '-s', '-f', '-m', '120', '-o', str(path), url] + headers
    subprocess.run(cmd, check=True)


def easyeda(lcsc):
    """The part's EasyEDA footprint (tools/easyeda_cache/, as tools/gen_footprints.py and tools/jlc_align.py keep
    it): its title, origin and 3D model attributes."""
    path = CACHE / f'{lcsc}.json'
    if not path.exists():
        CACHE.mkdir(exist_ok=True)
        curl(f'https://easyeda.com/api/products/{lcsc}/components?version=6.4.19.5', path)
    pk = json.loads(path.read_text())['result']['packageDetail']
    ds = pk['dataStr']
    node = next((json.loads(s.split('~', 1)[1])['attrs'] for s in ds['shape'] if s.startswith('SVGNODE')), None)
    return pk['title'], (float(ds['head']['x']), float(ds['head']['y'])), node


def step_file(lcsc, uuid):
    path = MODELS / f'{lcsc}.step'
    if not path.exists() or not path.read_bytes()[:13] == b'ISO-10303-21;':
        curl(STEP_URL.format(uuid), path)
        if not path.read_bytes()[:13] == b'ISO-10303-21;':
            path.unlink()
            sys.exit(f'{lcsc}: EasyEDA returned no STEP file for model {uuid}')
    return path


def corners(b):
    return [(x, y, z) for x in (b.min.X, b.max.X) for y in (b.min.Y, b.max.Y) for z in (b.min.Z, b.max.Z)]


def placement(lcsc, origin, node, box, corr, mirrored, thick):
    """KiCad offset and rotation for the EasyEDA model of `lcsc` on its KiCad footprint, and the model's box there.
    Coordinates are KiCad's 3D ones: x right, y up (the footprint's -y), z out of the footprint's side."""
    rx, ry, rz = (float(v) for v in node['c_rotation'].split(','))
    m = mul(mul(rot('z', rz), rot('y', ry)), rot('x', rx))     # every model here turns about z only
    pts = [app(m, p) for p in corners(box)]
    cx, cy = (float(v) for v in node['c_origin'].split(','))
    centre = ((cx - origin[0]) * UNIT, -(cy - origin[1]) * UNIT)
    shift = (centre[0] - (min(p[0] for p in pts) + max(p[0] for p in pts)) / 2,
             centre[1] - (min(p[1] for p in pts) + max(p[1] for p in pts)) / 2,
             float(node['z']) * UNIT - min(p[2] for p in pts))
    t = rot('z', corr['theta'])                                 # R(theta): counter-clockwise on screen and from above
    m = mul(t, m)
    off = [a + b for a, b in zip(app(t, shift), (corr['dx'], -corr['dy'], 0.0))]
    if mirrored:                                                # seen from below: x -> -x, under the board
        flip = rot('y', 180)
        m = mul(flip, m)
        off = [a + b for a, b in zip(app(flip, off), (0.0, 0.0, -thick))]
    pts = [tuple(a + b for a, b in zip(app(m, p), off)) for p in corners(box)]
    bounds = [clean(f(p[i] for p in pts), 2) for i in range(3) for f in (min, max)]
    return [clean(v) for v in off], kicad_angles(m), bounds


# ── the MX key: switch and keycap, drawn in the footprint's frame (switch centre, board top at z = 0) ─────────────
def named(shape, label, colour):
    shape.label = label
    shape.color = Color(*colour)
    return shape


def mx_switch():
    """A Cherry MX-style switch: bottom housing in the 14 mm plate cut-out, top housing to 11.6 mm, the stem's cross
    to 15.2 mm; under the board the centre post, the two plastic pegs and the two contact pins (the footprint's
    holes: pin 1 at (2.54, 5.08), pin 2 at (-3.81, 2.54), 3D y up).  The LED window faces -y, over D1501."""
    bottom = Box(14.0, 14.0, 5.0, align=BOTTOM)
    top = loft([Plane.XY.offset(5.0) * RectangleRounded(15.6, 15.6, 0.6),
                Plane.XY.offset(6.2) * RectangleRounded(15.6, 15.6, 0.6)])
    top += loft([Plane.XY.offset(6.2) * RectangleRounded(14.6, 14.6, 0.9),
                 Plane.XY.offset(11.6) * RectangleRounded(10.6, 11.0, 1.4)])
    top -= Pos(0, -4.6, 11.6) * Box(4.0, 1.6, 0.6)                             # the LED window
    stem = Pos(0, 0, 11.6) * Box(4.1, 1.31, 3.6, align=BOTTOM)
    stem += Pos(0, 0, 11.6) * Box(1.17, 4.1, 3.6, align=BOTTOM)
    stem += Pos(0, 0, 11.6) * Cylinder(2.7, 0.8, align=BOTTOM)
    post = Pos(0, 0, -3.0) * Cylinder(1.95, 3.0, align=BOTTOM)
    for x in (-5.08, 5.08):
        post += Pos(x, 0, -2.75) * Cylinder(0.85, 2.75, align=BOTTOM)
    pins = [named(Pos(x, y, -3.3) * Box(1.2, 0.4, 3.3, align=BOTTOM), f'pin {n}', METAL)
            for n, (x, y) in enumerate(((2.54, 5.08), (-3.81, 2.54)), 1)]
    return Compound(label='MX switch', children=[named(bottom + post, 'bottom housing', HOUSING),
                                                 named(top, 'top housing', TOP_HOUSING),
                                                 named(stem, 'stem', STEM), *pins])


def keycap():
    """A 1u keycap with a spherical dish, hollow, on a stem tube with the MX cross: 18.1 mm at the skirt, 12.7 x 13.5
    on top, from 12.5 mm (at rest) to 21.0 mm over the board."""
    z0 = CAP_LOW + TRAVEL
    h = CAP_TOP - z0
    cap = loft([Plane.XY.offset(z0) * RectangleRounded(CAP_W, CAP_W, 1.6),
                Plane.XY.offset(CAP_TOP) * RectangleRounded(12.7, 13.5, 2.6)])
    cap -= loft([Plane.XY.offset(z0 - 0.01) * RectangleRounded(CAP_W - 2.4, CAP_W - 2.4, 0.8),
                 Plane.XY.offset(CAP_TOP - 1.6) * RectangleRounded(11.0, 11.8, 1.6)])
    dish = 0.6
    radius = 40.0
    cap -= Pos(0, 0, CAP_TOP - dish + radius) * Sphere(radius)
    tube = Pos(0, 0, z0 + 0.6) * Cylinder(2.75, h - 2.0, align=BOTTOM)
    tube -= Pos(0, 0, z0 + 0.5) * Box(4.2, 1.4, 3.9, align=BOTTOM)
    tube -= Pos(0, 0, z0 + 0.5) * Box(1.3, 4.2, 3.9, align=BOTTOM)
    return named(cap + tube, 'keycap 1u', CAP_COLOUR)


def main():
    if not (KICAD3D / 'Resistor_SMD.3dshapes' / 'R_0402_1005Metric.step').exists():
        sys.exit(f"KiCad's 3D models are not in {KICAD3D} (Fedora: dnf install kicad-packages3d), or set "
                 'KICAD10_3DMODEL_DIR: without them every part would look model-less here')
    corr = json.loads(CORR.read_text())
    parts, thick = board_parts()
    MODELS.mkdir(exist_ok=True)
    out = {}
    for lcsc, (fpid, mirrored) in sorted(parts.items()):
        if any(f.exists() for f in library_models(fpid)):
            continue                                            # KiCad's own model
        if lcsc not in corr:
            sys.exit(f'{lcsc} ({fpid}) has no JLC correction: run tools/jlc_align.py first')
        title, origin, node = easyeda(lcsc)
        if node is None:
            print(f'{lcsc:10} {fpid}: EasyEDA has no 3D model either - left without one')
            continue
        path = step_file(lcsc, node['uuid'])
        box = import_step(str(path)).bounding_box()
        off, angles, bounds = placement(lcsc, origin, node, box, corr[lcsc], mirrored, thick)
        out[lcsc] = {'footprint': fpid, 'easyeda': title, 'uuid': node['uuid'], 'bottom': bool(mirrored),
                     'box': bounds,
                     'models': [{'file': f'{PRJ}/{path.name}', 'offset': off, 'rotate': angles}]}
        print(f'{lcsc:10} {fpid:58} offset {off} rotate {angles}  box x {bounds[0]}..{bounds[1]} '
              f'y {bounds[2]}..{bounds[3]} z {bounds[4]}..{bounds[5]}')
    key = next((lcsc for lcsc, (fpid, _) in parts.items() if fpid.endswith(':' + KEY_FP)), None)
    if key is None:
        sys.exit(f'no {KEY_FP} on the board')
    for name, shape in (('MX_Switch', mx_switch()), ('Keycap_1u', keycap())):
        export_step(shape, str(MODELS / f'{name}.step'))
        out.setdefault(key, {'footprint': parts[key][0], 'models': []})['models'].append(
            {'file': f'{PRJ}/{name}.step', 'offset': [0.0, 0.0, 0.0], 'rotate': [0.0, 0.0, 0.0]})
        print(f'{name}: drawn')
    OUT.write_text(json.dumps(out, indent=1) + '\n')
    print(f'-> {OUT.relative_to(HW)} ({len(out)} parts), models in {MODELS.relative_to(HW)}/')


if __name__ == '__main__':
    main()
