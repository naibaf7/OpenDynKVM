#!/usr/bin/env python3
"""
The OpenDynKVM logo as SVG: the mark, the wordmark, both stacked, and the application icons (branding/README.md).

The mark is a keycap seen from the side, with its stem, and two ribbons crossing in front of it: one PC, then the
other.  It is built here from centre lines and one stroke width, measured on the original drawing (in its pixels), and
written as filled outlines whose curves are fitted again as cubic Béziers: the files need no stroke, mask, clip or font
support from whatever shows them.  The wordmark is Montserrat (SIL OFL 1.1), "OpenDyn" Bold and "KVM" Regular, as
outlines.

Run from anywhere:   python3 branding/make_logo.py [--fonts DIR]
Needs shapely, fontTools and Montserrat-Bold / Montserrat-Regular (.otf or .ttf) in DIR or a usual font directory.
"""
import argparse
import math
from pathlib import Path

from fontTools.pens.boundsPen import BoundsPen
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from shapely import affinity
from shapely.geometry import LineString, MultiPolygon, Polygon
from shapely.geometry.polygon import orient
from shapely.ops import unary_union

OUT = Path(__file__).resolve().parent
FONT_DIRS = ['/usr/share/fonts', '/usr/local/share/fonts', '~/.local/share/fonts', '~/.fonts', '~/Library/Fonts',
             'C:/Windows/Fonts']

WHITE = '#ffffff'           # the ink on dark backgrounds
DARK = '#0f1316'            # the original's background: the ink on light backgrounds and the icons' tile

# Design units: pixels of the original drawing.  Origin where the ribbons cross, y down.
S = 9.0                     # stroke
GAP = 9.0                   # clear space where a line passes behind another
MITRE = dict(join_style='mitre', mitre_limit=8.0)

# Keycap, centre line: a rounded top, sides flaring out to the shoulders, a chamfer in to the bottom.
CAP_TOP, CAP_TOP_HALF, CAP_R = -118.75, 105.25, 13.0
SHOULDER = (166.0, 82.25)   # (half width, y)
CAP_FOOT = (150.0, 115.25)
# The stem under it, narrowing down, with a slot from each end (turned by 180 degrees, like the ribbons' crossing).
STEM_TOP, STEM_FOOT = 51.0, (45.0, 192.25)
SLOT_X, SLOT_LEN = 23.5, 55.0
# Ribbons: half width (centre line to edge), centre lines RIB_Y above and below the crossing, one cubic in between
# (ends at x = ±RIB_A, horizontal handles RIB_K long).  The ends are cut on a slant through x = ±RIB_END on the
# centre line, along RIB_CUT, the long edge on top.  B runs from the lower left to the upper right, over A.
RIB_H, RIB_Y = 14.75, 48.5
RIB_A, RIB_K = 106.0, 74.0
RIB_END, RIB_CUT = 164.15, (35.7, 29.5)

# The small mark (icons up to 32 px): the same outline, the ribbons solid, the keycap drawn bolder.
SMALL_S, SMALL_GAP = 22.0, 16.0

# Wordmark: cap height and the baseline below the crossing in the stacked logo.
CAP_HEIGHT, BASELINE = 51.5, 289.75
WORDS = [('Montserrat-Bold', 'OpenDyn'), ('Montserrat-Regular', 'KVM')]

TOL = 0.04                  # curve fitting, design units
LONG = 3.0                  # a segment this long is a straight piece of the outline, not a sample of a curve


# --- the mark -------------------------------------------------------------------------------------------------------

def cubic(p0, p1, p2, p3, n=240):
    return [tuple((1 - t) ** 3 * a + 3 * (1 - t) ** 2 * t * b + 3 * (1 - t) * t * t * c + t ** 3 * d
                  for a, b, c, d in zip(p0, p1, p2, p3)) for t in (i / n for i in range(n + 1))]


def rounded(points, radii, step=2.0):
    """A closed polygon with its corners rounded by the given radii (0: sharp)."""
    ring = []
    for i, (p, r) in enumerate(zip(points, radii)):
        if not r:
            ring.append(p)
            continue
        a, c = points[i - 1], points[(i + 1) % len(points)]
        u1, u2 = unit(a[0] - p[0], a[1] - p[1]), unit(c[0] - p[0], c[1] - p[1])
        half = math.acos(u1[0] * u2[0] + u1[1] * u2[1]) / 2
        bis = unit(u1[0] + u2[0], u1[1] + u2[1])
        o = (p[0] + bis[0] * r / math.sin(half), p[1] + bis[1] * r / math.sin(half))
        d = r / math.tan(half)
        a1 = math.atan2(p[1] + u1[1] * d - o[1], p[0] + u1[0] * d - o[0])
        a2 = math.atan2(p[1] + u2[1] * d - o[1], p[0] + u2[0] * d - o[0])
        sweep = (a2 - a1 + math.pi) % (2 * math.pi) - math.pi
        k = max(2, round(abs(math.degrees(sweep)) / step))
        ring += [(o[0] + r * math.cos(a1 + sweep * j / k), o[1] + r * math.sin(a1 + sweep * j / k))
                 for j in range(k + 1)]
    return Polygon(ring)


def unit(x, y):
    n = math.hypot(x, y)
    return (x / n, y / n)


def beside(p, d, toward):
    """The half plane beside the line through p along d, on the side x grows (toward = 1) or shrinks (-1)."""
    ends = [(p[0] - 1e3 * d[0], p[1] - 1e3 * d[1]), (p[0] + 1e3 * d[0], p[1] + 1e3 * d[1])]
    return Polygon(ends + [(x + 4e3 * toward, y) for x, y in reversed(ends)])


def keycap():
    t, (sx, sy), (fx, fy) = CAP_TOP_HALF, SHOULDER, CAP_FOOT
    return rounded([(-t, CAP_TOP), (t, CAP_TOP), (sx, sy), (fx, fy), (-fx, fy), (-sx, sy)], [CAP_R, CAP_R, 0, 0, 0, 0])


def ribbon():
    """Ribbon B, the area between its edges' centre lines."""
    y, a, k = RIB_Y, RIB_A, RIB_K
    centre = [(-400.0, y)] + cubic((-a, y), (-a + k, y), (a - k, -y), (a, -y)) + [(400.0, -y)]
    band = LineString(centre).buffer(RIB_H, cap_style='flat', join_style='round')
    dx, dy = RIB_CUT
    return band.intersection(beside((-RIB_END, y), (dx, dy), 1)).intersection(beside((RIB_END, -y), (-dx, dy), -1))


def outline(area, s):
    return area.buffer(s / 2, **MITRE).difference(area.buffer(-s / 2, **MITRE))


def line(points, s):
    return LineString(points).buffer(s / 2, cap_style='flat', **MITRE)


def mark(small=False):
    """The mark's ink: everything drawn with one stroke; small: solid ribbons and a bolder keycap, without the slots."""
    s, gap = (SMALL_S, SMALL_GAP) if small else (S, GAP)
    b = ribbon()
    a = affinity.scale(b, -1, 1, origin=(0, 0))
    if small:
        a, b = a.buffer(S / 2, **MITRE), b.buffer(S / 2, **MITRE)
        ribbons = [b] + list(parts(a.difference(b.buffer(gap, **MITRE))))
    else:
        ribbons = [outline(b, s)] + [outline(p, s) for p in parts(a.difference(b.buffer(s + gap, **MITRE)))]
    behind = unary_union([a, b]).buffer((0 if small else s / 2) + gap, **MITRE)
    (sx, sy), (fx, fy) = STEM_FOOT, CAP_FOOT
    stem = [(-STEM_TOP, fy), (-sx, sy), (sx, sy), (STEM_TOP, fy)]
    if small:
        stem = [Polygon(stem).buffer(s / 2, **MITRE)]
    else:
        stem = [line(stem, s), line([(-SLOT_X, fy), (-SLOT_X, fy + SLOT_LEN)], s),
                line([(SLOT_X, sy), (SLOT_X, sy - SLOT_LEN)], s)]
    return unary_union(ribbons + [outline(keycap(), s).difference(behind)] + stem)


def parts(geom):
    return geom.geoms if hasattr(geom, 'geoms') else [geom]


# --- outlines as SVG path data ----------------------------------------------------------------------------------------

def num(v):
    s = f'{v:.2f}'.rstrip('0').rstrip('.')
    return '0' if s == '-0' else s


def path_data(geom, k, tx, ty):
    """SVG path data for the polygons in geom, mapped by (x, y) -> (k x + tx, k y + ty).  Long segments stay lines; the
    runs of short ones between corners, samples of curves, become cubic Béziers within TOL."""
    def pt(p):
        return f'{num(k * p[0] + tx)} {num(k * p[1] + ty)}'

    out = []
    for poly in parts(geom):
        poly = orient(poly.simplify(0), 1.0)
        for ring in [poly.exterior] + list(poly.interiors):
            pts = list(ring.coords)[:-1]
            pieces = split(pts)
            out.append('M' + pt(pieces[0][0]))
            for piece in pieces:
                if len(piece) == 2 or straight(piece):
                    out.append('L' + pt(piece[-1]))
                    continue
                t0 = unit(piece[1][0] - piece[0][0], piece[1][1] - piece[0][1])
                t1 = unit(piece[-1][0] - piece[-2][0], piece[-1][1] - piece[-2][1])
                for c in fit(piece, t0, t1):
                    out.append('C' + ' '.join(pt(p) for p in c[1:]))
            out[-1] += 'Z'
    return ''.join(out)


def seg(p, q):
    return math.hypot(q[0] - p[0], q[1] - p[1])


def straight(pts):
    (x0, y0), (x1, y1) = pts[0], pts[-1]
    n = seg(pts[0], pts[-1])
    return all(abs((x1 - x0) * (y - y0) - (y1 - y0) * (x - x0)) / n < 0.01 for x, y in pts[1:-1])


def split(pts):
    """A closed ring as pieces that each start where the last ended: [p, q] for a long segment, else a run of points
    between corners (a turn above 25 degrees) and long segments."""
    n = len(pts)

    def turn(i):
        a, b, c = pts[i - 1], pts[i], pts[(i + 1) % n]
        u, v = unit(b[0] - a[0], b[1] - a[1]), unit(c[0] - b[0], c[1] - b[1])
        return math.degrees(math.acos(max(-1.0, min(1.0, u[0] * v[0] + u[1] * v[1]))))

    breaks = [i for i in range(n)
              if turn(i) > 25 or seg(pts[i - 1], pts[i]) >= LONG or seg(pts[i], pts[(i + 1) % n]) >= LONG]
    if not breaks:
        breaks = [0]
    pieces = []
    for j, i in enumerate(breaks):
        end = breaks[(j + 1) % len(breaks)]
        run = [pts[(i + m) % n] for m in range((end - i) % n or n)] + [pts[end]]
        pieces.append(run)
    return pieces


def bez(c, t):
    m = 1 - t
    return tuple(m ** 3 * a + 3 * m * m * t * b + 3 * m * t * t * d + t ** 3 * e for a, b, d, e in zip(*c))


def fit(pts, t0, t1):
    """Cubic Béziers through pts (after Schneider 1990): t0 the unit tangent leaving pts[0], t1 the one reaching
    pts[-1]."""
    if len(pts) == 2:
        d = seg(*pts) / 3
        return [(pts[0], (pts[0][0] + t0[0] * d, pts[0][1] + t0[1] * d), (pts[1][0] - t1[0] * d, pts[1][1] - t1[1] * d),
                 pts[1])]
    acc = [0.0]
    for p, q in zip(pts, pts[1:]):
        acc.append(acc[-1] + seg(p, q))
    u = [a / acc[-1] for a in acc]
    for _ in range(8):
        c = handles(pts, u, t0, t1)
        err = [seg(bez(c, t), p) for p, t in zip(pts, u)]
        worst = max(range(1, len(pts) - 1), key=err.__getitem__)
        if err[worst] < TOL:
            return [c]
        u = [newton(c, p, t) for p, t in zip(pts, u)]
    tc = unit(pts[worst + 1][0] - pts[worst - 1][0], pts[worst + 1][1] - pts[worst - 1][1])
    return fit(pts[:worst + 1], t0, tc) + fit(pts[worst:], tc, t1)


def handles(pts, u, t0, t1):
    """The cubic from pts[0] along t0 to pts[-1] along t1 whose handle lengths fit pts at u best (least squares)."""
    p0, p3 = pts[0], pts[-1]
    c00 = c01 = c11 = x0 = x1 = 0.0
    for p, t in zip(pts, u):
        m = 1 - t
        b0, b1, b2, b3 = m ** 3, 3 * m * m * t, 3 * m * t * t, t ** 3
        a0, a1 = (t0[0] * b1, t0[1] * b1), (-t1[0] * b2, -t1[1] * b2)
        r = (p[0] - p0[0] * (b0 + b1) - p3[0] * (b2 + b3), p[1] - p0[1] * (b0 + b1) - p3[1] * (b2 + b3))
        c00 += a0[0] * a0[0] + a0[1] * a0[1]
        c01 += a0[0] * a1[0] + a0[1] * a1[1]
        c11 += a1[0] * a1[0] + a1[1] * a1[1]
        x0 += a0[0] * r[0] + a0[1] * r[1]
        x1 += a1[0] * r[0] + a1[1] * r[1]
    det, chord = c00 * c11 - c01 * c01, seg(p0, p3)
    l0, l1 = ((x0 * c11 - x1 * c01) / det, (c00 * x1 - c01 * x0) / det) if abs(det) > 1e-12 else (0, 0)
    if l0 < 1e-3 * chord or l1 < 1e-3 * chord:
        l0 = l1 = chord / 3
    return (p0, (p0[0] + t0[0] * l0, p0[1] + t0[1] * l0), (p3[0] - t1[0] * l1, p3[1] - t1[1] * l1), p3)


def newton(c, p, t):
    """t moved towards the point of c nearest p."""
    d1 = [tuple(3 * (b - a) for a, b in zip(c[i], c[i + 1])) for i in range(3)]
    d2 = [tuple(2 * (b - a) for a, b in zip(d1[i], d1[i + 1])) for i in range(2)]
    m = 1 - t
    q = bez(c, t)
    q1 = tuple(m * m * a + 2 * m * t * b + t * t * e for a, b, e in zip(*d1))
    q2 = tuple(m * a + t * b for a, b in zip(*d2))
    r = (q[0] - p[0], q[1] - p[1])
    den = q1[0] ** 2 + q1[1] ** 2 + r[0] * q2[0] + r[1] * q2[1]
    return min(1.0, max(0.0, t - (r[0] * q1[0] + r[1] * q1[1]) / den)) if den else t


# --- the wordmark -----------------------------------------------------------------------------------------------------

def find_font(name, dirs):
    for d in dirs:
        for p in sorted(Path(d).expanduser().rglob(f'{name}.*')) if Path(d).expanduser().is_dir() else []:
            if p.suffix.lower() in ('.otf', '.ttf'):
                return p
    raise SystemExit(f'{name}.otf or .ttf not found: install Montserrat (SIL OFL 1.1) or pass --fonts DIR')


def wordmark(dirs):
    """The wordmark's glyphs as (glyph, x, scale) in design units with the baseline at 0 and the ink centred on x = 0,
    and the ink's bounds."""
    glyphs, x = [], 0.0
    for name, text in WORDS:
        font = TTFont(find_font(name, dirs))
        gs, cmap, hmtx = font.getGlyphSet(), font.getBestCmap(), font['hmtx']
        scale = CAP_HEIGHT / font['OS/2'].sCapHeight
        for ch in text:
            g = cmap[ord(ch)]
            glyphs.append((gs[g], x, scale))
            x += hmtx[g][0] * scale
    boxes = []
    for g, x, scale in glyphs:
        bp = BoundsPen(None)
        g.draw(bp)
        x0, y0, x1, y1 = bp.bounds
        boxes.append((x + x0 * scale, -y1 * scale, x + x1 * scale, -y0 * scale))
    mid = (boxes[0][0] + boxes[-1][2]) / 2
    glyphs = [(g, x - mid, scale) for g, x, scale in glyphs]
    bounds = (boxes[0][0] - mid, min(b[1] for b in boxes), boxes[-1][2] - mid, max(b[3] for b in boxes))
    return glyphs, bounds


def text_data(glyphs, k, tx, ty):
    out = []
    for g, x, scale in glyphs:
        pen = SVGPathPen(None, ntos=num)
        g.draw(TransformPen(pen, (k * scale, 0, 0, -k * scale, k * x + tx, ty)))
        out.append(pen.getCommands())
    return ''.join(out)


# --- the files --------------------------------------------------------------------------------------------------------

def svg(w, h, body):
    size = f'width="{num(w)}" height="{num(h)}" viewBox="0 0 {num(w)} {num(h)}"'
    return (f'<svg xmlns="http://www.w3.org/2000/svg" {size}>'
            f'\n<!-- Generated by branding/make_logo.py: change that, not this file. -->\n<title>OpenDynKVM</title>\n'
            f'{body}</svg>\n')


def write(name, text):
    (OUT / name).write_text(text)
    print(f'{name}: {len(text)} bytes')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n\n')[0].strip())
    ap.add_argument('--fonts', action='append', default=[], help='a directory with Montserrat in it (searched first)')
    args = ap.parse_args()

    full, small = mark(), mark(small=True)
    x0, y0, x1, y1 = full.bounds
    glyphs, (wx0, wy0, wx1, wy1) = wordmark(args.fonts + FONT_DIRS)
    for ink, suffix in ((WHITE, 'on-dark'), (DARK, 'on-light')):
        write(f'opendynkvm-mark-{suffix}.svg',
              svg(x1 - x0, y1 - y0, f'<path fill="{ink}" d="{path_data(full, 1, -x0, -y0)}"/>\n'))
        write(f'opendynkvm-wordmark-{suffix}.svg',
              svg(wx1 - wx0, wy1 - wy0, f'<path fill="{ink}" d="{text_data(glyphs, 1, -wx0, -wy0)}"/>\n'))
        # Stacked: the wordmark centred under the mark.
        lx0, ly0, lx1, ly1 = min(x0, wx0), y0, max(x1, wx1), BASELINE + wy1
        write(f'opendynkvm-logo-{suffix}.svg',
              svg(lx1 - lx0, ly1 - ly0, f'<path fill="{ink}" d="{path_data(full, 1, -lx0, -ly0)}'
                                        f'{text_data(glyphs, 1, -lx0, BASELINE - ly0)}"/>\n'))
    # Application icons: the mark in white on a rounded tile, 512 units square.  The small one fills more of the tile.
    for name, geom, share in (('opendynkvm-icon.svg', full, 0.76), ('opendynkvm-icon-small.svg', small, 0.92)):
        bx0, by0, bx1, by1 = geom.bounds
        k = share * 512 / (bx1 - bx0)
        tx, ty = 256 - k * (bx0 + bx1) / 2, 256 - k * (by0 + by1) / 2
        write(name, svg(512, 512, f'<rect width="512" height="512" rx="112" fill="{DARK}"/>\n'
                                  f'<path fill="{WHITE}" d="{path_data(geom, k, tx, ty)}"/>\n'))


if __name__ == '__main__':
    main()
