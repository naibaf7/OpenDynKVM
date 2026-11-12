#!/usr/bin/env python3
"""Render the board in 3D with KiCad (kicad-cli pcb render, raytraced) into render/: top, bottom and two views at an
angle (front: the USB-A ports and the key; rear: the USB-C ports), as PNG with a transparent background.

    uv run tools/render_3d.py [--quick] [VIEW ...]        (from hardware/; VIEW: top, bottom, front, rear)

--quick renders at KiCad's basic quality, for checking a view (about a second each instead of ten).

The full-size renders stay local (.gitignore).  The front view also gives README.md its picture,
render/kvm-front-readme.png, the one render in git: cropped to the board, 256 colours with transparency, and as wide
as fits in README_MAX (about 2200 px, twice the width README.md shows it at and more).  It needs Pillow (Fedora:
python3-pillow, which the repository's environment sees).

Every part has a model: KiCad's 3D library (Fedora: kicad-packages3d) for most, and for the parts it has none for,
plus the MX switch and keycap, the files tools/fetch_3d.py puts in 3dmodels/ (tools/models_3d.json, put on the board
by tools/gen_pcb.py).  This checks first that every model the board names exists, so that no part is silently left
out of a render.
"""
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
OUT = os.path.join(HERE, 'render')
SIZE = (3000, 2000)
README_PNG = os.path.join(OUT, 'kvm-front-readme.png')
README_MAX = 256 * 1024                 # bytes: the picture is in git, so every re-render adds this much to its history
README_WIDTHS = (2200, 2000, 1800, 1600, 1440)
VARS = {'KICAD10_3DMODEL_DIR': os.environ.get('KICAD10_3DMODEL_DIR', '/usr/share/kicad/3dmodels'), 'KIPRJMOD': HERE}

# kicad-cli: --zoom 1 frames the board with a wide margin; --pan is in the 3D view's units (about 9.4 mm on this
# board), and lifts the angled views to the middle of the picture
VIEWS = {
    'top': ['--side', 'top', '--zoom', '1.4'],
    'bottom': ['--side', 'bottom', '--zoom', '1.4'],
    'front': ['--perspective', '--rotate', '-50,0,-30', '--zoom', '1.15', '--pan', '0.2,1.6,0'],     # README picture
    'rear': ['--perspective', '--rotate', '-50,0,150', '--zoom', '1.15', '--pan', '0.2,1.6,0'],
}


def missing_models():
    """The model files the board names that do not exist, with the parts that name them."""
    text = open(PCB).read()
    missing = {}
    for block in re.split(r'\n\t\(footprint ', text)[1:]:
        ref = re.search(r'\(property "Reference" "([^"]*)"', block).group(1)
        for name in re.findall(r'\(model "([^"]+)"', block):
            path = re.sub(r'\$\{(\w+)\}', lambda m: VARS.get(m.group(1), m.group(0)), name)
            if '${' in path or not os.path.exists(path):
                missing.setdefault(name, []).append(ref)
    return missing


def readme_copy(src):
    """README.md's picture from the full-size front view: the board cropped out of its transparent margin, then the
    widest of README_WIDTHS whose 256-colour PNG stays within README_MAX."""
    try:
        from PIL import Image
    except ImportError:
        print('README picture not made: Pillow is missing (Fedora: python3-pillow); '
              f'{os.path.relpath(README_PNG, HERE)} is left as it is')
        return
    im = Image.open(src).convert('RGBA')
    x0, y0, x1, y1 = im.getchannel('A').point(lambda v: 255 if v > 8 else 0).getbbox()
    m = 30
    im = im.crop((max(0, x0 - m), max(0, y0 - m), min(im.width, x1 + m), min(im.height, y1 + m)))
    for w in README_WIDTHS:
        small = im.resize((w, round(im.height * w / im.width)), Image.LANCZOS)
        small = small.quantize(256, method=Image.Quantize.FASTOCTREE, dither=Image.Dither.FLOYDSTEINBERG)
        small.save(README_PNG, optimize=True)
        size = os.path.getsize(README_PNG)
        if size <= README_MAX:
            break
    print(f'README  -> {os.path.relpath(README_PNG, HERE)}: {small.width} x {small.height}, {size // 1024} KB')


def main():
    args = [a for a in sys.argv[1:] if not a.startswith('--')]
    quick = '--quick' in sys.argv
    views = args or list(VIEWS)
    unknown = [v for v in views if v not in VIEWS]
    if unknown:
        sys.exit(f'unknown view {", ".join(unknown)}: one of {", ".join(VIEWS)}')
    missing = missing_models()
    if missing:
        for name, refs in sorted(missing.items()):
            print(f'missing: {name} ({", ".join(refs[:6])}{" ..." if len(refs) > 6 else ""})')
        sys.exit("install KiCad's 3D models (Fedora: kicad-packages3d) for ${KICAD10_3DMODEL_DIR}, "
                 'run `uv run tools/fetch_3d.py` for ${KIPRJMOD}/3dmodels, then `uv run tools/gen_pcb.py --models` '
                 'if the board names models that tools/models_3d.json does not')
    os.makedirs(OUT, exist_ok=True)
    for view in views:
        out = os.path.join(OUT, f'kvm-{view}.png')
        cmd = ['kicad-cli', 'pcb', 'render', '--quality', 'basic' if quick else 'high',
               '-w', str(SIZE[0]), '-h', str(SIZE[1]), *VIEWS[view], '-o', out, PCB]
        r = subprocess.run(cmd, capture_output=True, text=True, env={**os.environ, 'KICAD10_3DMODEL_DIR':
                                                                     VARS['KICAD10_3DMODEL_DIR']})
        if r.returncode != 0 or not os.path.exists(out):
            sys.exit(f'{view}: kicad-cli failed\n{r.stdout[-2000:]}{r.stderr[-2000:]}')
        print(f'{view:7} -> {os.path.relpath(out, HERE)}')
        if view == 'front' and not quick:
            readme_copy(out)


if __name__ == '__main__':
    main()
