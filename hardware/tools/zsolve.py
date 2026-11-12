#!/usr/bin/env python3
"""
Impedance of the USB High-Speed pair on the board's own stack-up: a 2D quasi-static field solve (finite differences)
of an edge-coupled microstrip over its reference plane, with conformal solder mask.  Gives Zdiff = 2 Zodd and
Zcomm = Zeven / 2, against USB 2.0's 90 ohm +-15 % differential and 30 ohm +-30 % common mode.

The stack-up comes from the board file (gen_pcb.py STACKUP, written into kvm.kicad_pcb's setup): outer copper, the
prepreg under it and its Er, and the mask thickness.  The pair geometry is route.py's PAIR_W / PAIR_S.

  --corners  also the fabrication tolerances one at a time (etched width at a fixed pitch, prepreg thickness, Er,
             mask, plated copper, trapezoidal etch), their root-sum-square spread, the stacked worst cases, and the
             same pair on JLC's thick-prepreg JLC04161H-7628 build.  This is the evidence for jlcpcb.md section 4:
             the stack-up must be specified; JLC's paid impedance control only narrows an already in-spec spread.

Calibration: a single 0.15 mm line over 0.0994 mm (Er 4.1) solves to 53.4 ohm, close to the closed-form microstrip
value; grid dx 2.5 um, 40 cells across the prepreg.  Mask Er (MASK_ER) is an assumption, as are the tolerance
ranges (TOL), which are typical fab values, not JLC guarantees.

  --prepreg MM, --er X  evaluate another build without touching the board: the dielectric under the pair and its Er
             (e.g. JLC's 1 oz-inner 3313 build, 0.092 mm)

Run from hardware/:   uv run tools/zsolve.py [--corners] [--prepreg MM] [--er X]     (each solve takes a few seconds)
"""

import math
import os
import sys

import numpy as np
import scipy.sparse as sp
import scipy.sparse.linalg as spl
from scipy.ndimage import distance_transform_edt

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import board_stackup  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
C0 = 299792458.0
EPS0 = 8.854187817e-12
PAIR_W, PAIR_S = 0.15, 0.15      # route.py PAIR_W / PAIR_S
MASK_ER = 3.8                    # LPI solder mask, assumed
USB_ZDIFF, USB_ZCOMM = (76.5, 103.5), (21.0, 39.0)
TOL = {'width': 0.10, 'width_wide': 0.20, 'prepreg': 0.10, 'er': (3.9, 4.3), 'mask': (0.0, 0.020),
       'copper_plated': 0.045, 'trapezoid': 0.025}
ALT_7628 = ('JLC04161H-7628', 0.2104, 4.4)


def solve(w, s, h, t, er, mask_t=0.0, mask_er=MASK_ER, pair=True, dx=0.0025, half_w=2.0, trap=0.0):
    """Zdiff / Zcomm (pair) or Z0 (single line) of traces w wide, s apart, t thick, on h of dielectric er over a
    plane; mask_t of conformal mask; trap = how much narrower the trace top is than its foot."""
    dy = h / 40
    top = h + 1.0
    nx = int(round(2 * half_w / dx)) + 1
    ny = int(round(top / dy)) + 1
    xs = (np.arange(nx) - (nx - 1) / 2) * dx
    ys = np.arange(ny) * dy
    X, Y = np.meshgrid(xs, ys)                       # [j, i]
    jt0, jt1 = 40, 40 + int(round(t / dy))           # trace bottom / top rows

    def trace_mask(xc):
        m = np.zeros_like(X, bool)
        for j in range(jt0, jt1 + 1):
            f = (j - jt0) / max(jt1 - jt0, 1)
            m[j] |= np.abs(xs - xc) <= w / 2 - f * trap / 2 + 1e-9
        return m

    cond = [trace_mask(c) for c in ([-(s + w) / 2, (s + w) / 2] if pair else [0.0])]

    def eps_map(er_sub):
        e = np.ones_like(X)
        e[Y <= h + 1e-12] = er_sub
        if mask_t > 0:
            solid = Y <= h + 1e-12
            for c in cond:
                solid = solid | c
            d = distance_transform_edt(~solid, sampling=(dy, dx))
            e[(~solid) & (d <= mask_t)] = mask_er if er_sub != 1.0 else 1.0
        return e

    def energy(pots, er_sub):
        e = eps_map(er_sub)
        fixed = np.zeros_like(X, bool)
        val = np.zeros_like(X)
        fixed[0, :] = fixed[-1, :] = fixed[:, 0] = fixed[:, -1] = True    # plane at y = 0, far walls at 0 V
        for c, v in zip(cond, pots):
            fixed |= c
            val[c] = v
        idx = np.arange(nx * ny).reshape(ny, nx)
        ex = 0.5 * (e[:, 1:] + e[:, :-1])
        ey = 0.5 * (e[1:, :] + e[:-1, :])
        rows, cols, data = [], [], []
        for a, b, g in ((idx[:, :-1].ravel(), idx[:, 1:].ravel(), (ex * dy / dx).ravel()),
                        (idx[:-1, :].ravel(), idx[1:, :].ravel(), (ey * dx / dy).ravel())):
            rows += [a, b, a, b]
            cols += [b, a, a, b]
            data += [-g, -g, g, g]
        A = sp.csr_matrix((np.concatenate(data), (np.concatenate(rows), np.concatenate(cols))), shape=(nx * ny,) * 2)
        fr = fixed.ravel()
        free = ~fr
        phi = val.ravel().copy()
        phi[free] = spl.spsolve(A[free][:, free].tocsc(), -A[free][:, fr] @ phi[fr])
        P = phi.reshape(ny, nx)
        W = 0.5 * (np.sum(ex * (P[:, 1:] - P[:, :-1]) ** 2) * dy / dx + np.sum(ey * (P[1:, :] - P[:-1, :]) ** 2) * dx / dy)
        return W * EPS0

    if not pair:
        c1, c10 = 2 * energy((1,), er), 2 * energy((1,), 1.0)
        return {'Z0': 1 / (C0 * math.sqrt(c1 * c10))}
    zo = 1 / (C0 * math.sqrt(energy((1, -1), er) * energy((1, -1), 1.0)))
    ze = 1 / (C0 * math.sqrt(energy((1, 1), er) * energy((1, 1), 1.0)))
    return {'Zdiff': 2 * zo, 'Zcomm': ze / 2}


def outer_stackup():
    st = board_stackup(PCB)
    if not st:
        sys.exit('the board has no stack-up (uv run tools/gen_pcb.py --stackup)')
    names = [n for n, *_ in st]
    cu = names.index('F.Cu')
    _, _, t, _ = st[cu]
    _, _, h, er = st[cu + 1]                          # the dielectric right under F.Cu: the pair's reference gap
    mask = next((th for n, _, th, _ in st if n == 'F.Mask'), 0.0) or 0.0
    return t, h, er, mask


def main():
    t, h, er, mask = outer_stackup()
    for opt in ('--prepreg', '--er'):
        if opt in sys.argv:
            val = float(sys.argv[sys.argv.index(opt) + 1])
            h, er = (val, er) if opt == '--prepreg' else (h, val)
    corners = '--corners' in sys.argv
    print(f'pair {PAIR_W:.3f} / {PAIR_S:.3f} mm on F.Cu over In1.Cu (B.Cu over In2.Cu is the mirror image): '
          f'dielectric {h} mm, Er {er}, copper {t * 1000:.1f} um, mask {mask * 1000:.1f} um (Er {MASK_ER}, assumed)')
    print(f'USB 2.0: Zdiff {USB_ZDIFF[0]}-{USB_ZDIFF[1]} ohm (90 +-15 %), Zcomm {USB_ZCOMM[0]:.0f}-{USB_ZCOMM[1]:.0f} '
          f'ohm (30 +-30 %)\n')
    pitch = PAIR_W + PAIR_S

    def pair(w=PAIR_W, h_=h, er_=er, mask_=mask, t_=t, trap=0.0):
        return solve(w, pitch - w, h_, t_, er_, mask_t=mask_, trap=trap)

    def ok(z):
        return 'in spec' if USB_ZDIFF[0] <= z['Zdiff'] <= USB_ZDIFF[1] and USB_ZCOMM[0] <= z['Zcomm'] <= USB_ZCOMM[1] \
            else 'OUT OF SPEC'

    nom = pair()
    print(f'{"nominal":44} Zdiff {nom["Zdiff"]:6.1f}   Zcomm {nom["Zcomm"]:5.1f}   {ok(nom)}', flush=True)
    if not corners:
        z0 = solve(PAIR_W, 0, h, t, er, pair=False)['Z0']
        print(f'{"single line, no mask (calibration)":44} Z0 {z0:6.1f}')
        return

    print('\none tolerance at a time (pitch fixed: a wider line has a narrower gap):')
    rows = [('width', f'etched width -/+{TOL["width"]:.0%}', [pair(w=PAIR_W * (1 - TOL['width'])), pair(w=PAIR_W * (1 + TOL['width']))]),
            ('width_wide', f'etched width -/+{TOL["width_wide"]:.0%}',
             [pair(w=PAIR_W * (1 - TOL['width_wide'])), pair(w=PAIR_W * (1 + TOL['width_wide']))]),
            ('prepreg', f'prepreg -/+{TOL["prepreg"]:.0%}', [pair(h_=h * (1 - TOL['prepreg'])), pair(h_=h * (1 + TOL['prepreg']))]),
            ('er', f'Er {TOL["er"][0]} / {TOL["er"][1]}', [pair(er_=TOL['er'][0]), pair(er_=TOL['er'][1])]),
            ('mask', f'mask {TOL["mask"][0] * 1000:.0f} / {TOL["mask"][1] * 1000:.0f} um', [pair(mask_=TOL['mask'][0]), pair(mask_=TOL['mask'][1])]),
            ('copper', f'plated copper {TOL["copper_plated"] * 1000:.0f} um', [pair(t_=TOL['copper_plated'])]),
            ('trap', f'trapezoid etch (top {TOL["trapezoid"] * 1000:.0f} um narrower)', [pair(trap=TOL['trapezoid'])])]
    delta = {}
    for key, label, zs in rows:
        d = [z['Zdiff'] - nom['Zdiff'] for z in zs]
        delta[key] = d
        print(f'  {label:42} Zdiff ' + ' / '.join(f'{z["Zdiff"]:5.1f}' for z in zs)
              + '   (' + ' / '.join(f'{x:+.1f}' for x in d) + ')', flush=True)
    for wkey in ('width', 'width_wide'):
        keys = [wkey, 'prepreg', 'er', 'mask', 'copper', 'trap']
        up = math.sqrt(sum(max([0.0] + delta[k]) ** 2 for k in keys))
        dn = math.sqrt(sum(min([0.0] + delta[k]) ** 2 for k in keys))
        print(f'root-sum-square spread, width -/+{TOL[wkey]:.0%}: {nom["Zdiff"] - dn:5.1f} - {nom["Zdiff"] + up:5.1f} ohm')

    print('\nstacked worst cases (every tolerance the same way at once):')
    for wkey in ('width', 'width_wide'):
        f = TOL[wkey]
        hi = pair(w=PAIR_W * (1 - f), h_=h * (1 + TOL['prepreg']), er_=TOL['er'][0], mask_=TOL['mask'][0])
        lo = pair(w=PAIR_W * (1 + f), h_=h * (1 - TOL['prepreg']), er_=TOL['er'][1], mask_=TOL['mask'][1],
                  t_=TOL['copper_plated'])
        for name, z in ((f'high: width -{f:.0%}, prepreg +, Er low, no mask', hi),
                        (f'low:  width +{f:.0%}, prepreg -, Er high, mask, 45 um', lo)):
            print(f'  {name:42} Zdiff {z["Zdiff"]:6.1f}   Zcomm {z["Zcomm"]:5.1f}   {ok(z)}', flush=True)

    name, h7, er7 = ALT_7628
    z = pair(h_=h7, er_=er7)
    print(f'\nthe same pair on {name} (prepreg {h7} mm, Er {er7}):  Zdiff {z["Zdiff"]:6.1f}   Zcomm {z["Zcomm"]:5.1f}   {ok(z)}')


if __name__ == '__main__':
    main()
