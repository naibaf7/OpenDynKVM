#!/usr/bin/env python3
"""
Derive JLCPCB placement corrections for every LCSC part on the board.

JLC places parts using the part's EasyEDA footprint (its own origin and 0-degree orientation),
which often differs from the KiCad footprint.  For each LCSC number this script fetches the
EasyEDA footprint, matches its pads to the KiCad footprint's pads (by pin number, else by
geometry; 2-pin polarized parts by the EasyEDA symbol's K/A pin names) and solves

        p_kicad = R(theta) * p_easyeda + t          (footprint-local, y down, CCW rotation)

The result goes to tools/jlc_corrections.json, which tools/make_fab.py applies to the CPL:
        JLC rotation = KiCad rotation + theta,   JLC position = KiCad position + R(phi) * t

Run from hardware/ (needs network):   python3 tools/jlc_align.py  (adapted from OpenRower)
"""
import json
import math
import os
import sys
import subprocess

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
OUT = os.path.join(HERE, 'tools', 'jlc_corrections.json')
UNIT = 0.254     # EasyEDA unit = 10 mil
HDRS = {'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36',
        'Accept': 'application/json, text/javascript, */*; q=0.01', 'Referer': 'https://easyeda.com/'}

# KiCad pad name -> datasheet / EasyEDA pad number, where the KiCad footprint names pads differently
PAD_ALIAS = {
    # footprints whose pad names differ from the EasyEDA ones: None = match by geometry
    'SW_Push_1P1T_XKB_TS-1187A': None,          # pads 1,1,2,2 vs 1..4: geometry (0/180 equivalent)
    'USB_A_Molex_67643_Horizontal': None,       # SH vs numbered shell pins
    'USB_C_Receptacle_HRO_TYPE-C-31-M-12_Edge': None,
}
POLAR = set()   # no 2-pin polarized parts on this board (LEDs are 4-pin, matched by pin number)
# Parts whose pads are point-symmetric about the land pattern's origin: matching pads cannot tell the part from itself
# turned 180 degrees, so the body decides.  {footprint: (extra rotation, why)}; checked below that the turn moves no pad.
BODY_ROT = {
    # Kailh / HanElectricity MX hot-swap socket: its concave side must wrap round the switch's 4 mm centre post (Kailh
    # drawing: the sweep runs ~2.8 mm from the switch centre, 0.8 mm clear of the hole).  The pad match gave theta 0,
    # and JLC's preview (2026-10-28) then had the straight side 0.86 mm from the switch centre, over the centre hole -
    # the switch could not seat ("the hole for the switch should be unobscured", splitkb's build guide).
    'SW_MX_Hotswap_Kailh_1u': (180, 'socket turned so its curved side wraps the switch centre post'),
}


def rot(theta, x, y):
    a = math.radians(theta)
    return (x * math.cos(a) + y * math.sin(a), -x * math.sin(a) + y * math.cos(a))


CACHE = os.path.join(HERE, 'tools', 'easyeda_cache')


def easyeda(lcsc):
    path = os.path.join(CACHE, f'{lcsc}.json')
    if not os.path.exists(path):        # EasyEDA rejects urllib; curl with browser headers works
        os.makedirs(CACHE, exist_ok=True)
        url = f'https://easyeda.com/api/products/{lcsc}/components?version=6.4.19.5'
        cmd = ['curl', '-s', '-m', '30', '-o', path, url] + sum((['-H', f'{k}: {v}'] for k, v in HDRS.items()), [])
        subprocess.run(cmd, check=True)
    d = json.load(open(path))['result']
    pk = d['packageDetail']['dataStr']
    ox, oy = float(pk['head']['x']), float(pk['head']['y'])
    pads = []
    for s in pk['shape']:
        if s.startswith('PAD~'):
            f = s.split('~')
            pads.append((f[8], (float(f[2]) - ox) * UNIT, (float(f[3]) - oy) * UNIT))
    pin_names = {}
    for s in d['dataStr']['shape']:          # schematic symbol pins: number -> name
        if s.startswith('P~'):
            parts = s.split('^^')
            head = parts[0].split('~')
            num = head[3] if len(head) > 3 else ''
            name = ''
            for p in parts[1:]:
                q = p.split('~')
                if len(q) > 4 and q[0] in ('0', '1') and q[4] and not q[4].replace('.', '').isdigit():
                    name = q[4]
                    break
            pin_names[num] = name
    return d['packageDetail']['title'], pads, pin_names


def kicad_pads(fp):
    """The part's pads in its *own* top view - the frame its EasyEDA footprint is drawn in - relative to the KiCad
    footprint's origin.  A part on the bottom is seen mirrored from the top, so its pads are mirrored back (x -> -x):
      - a footprint flipped to B.Cu: KiCad flips left/right and reports 180 degrees for a library footprint at 0
        (measured on D1501: its pads at 180 are the library's with x negated), so the copy is set to 180, not 0;
      - a top footprint whose soldered pads are all on B.Cu (the MX key's hot-swap socket): the socket is the part,
        mounted on the bottom under the switch.
    make_fab.py applies the result with the matching bottom-side transform (bottom_angle there)."""
    flipped = fp.IsFlipped()
    fp = pcbnew.FOOTPRINT(fp)               # copy, at the origin
    fp.SetOrientationDegrees(180 if flipped else 0)
    fp.SetPosition(pcbnew.VECTOR2I(0, 0))
    pads = [p for p in fp.Pads() if p.GetAttribute() != pcbnew.PAD_ATTRIB_NPTH]
    front = [p for p in pads if p.IsOnLayer(pcbnew.F_Cu)]
    mirror = flipped or not front
    use = [p for p in pads if p.IsOnLayer(pcbnew.B_Cu)] if mirror else front
    return [(p.GetNumber(), (-1 if mirror else 1) * pcbnew.ToMM(p.GetPosition().x), pcbnew.ToMM(p.GetPosition().y))
            for p in use]


def solve(kp, ep, by_number):
    best = None
    for theta in (0, 90, 180, 270):
        er = [(n, *rot(theta, x, y)) for n, x, y in ep]
        if by_number:
            # only pin numbers that occur exactly once on both sides (skips multi-square exposed
            # pads and corner pads that the two libraries number differently)
            kc_ = {}
            for k in kp:
                kc_[k[0]] = kc_.get(k[0], 0) + 1
            ec_ = {}
            for e in er:
                ec_[e[0]] = ec_.get(e[0], 0) + 1
            uniq = {n for n in kc_ if kc_[n] == 1 and ec_.get(n) == 1}
            pairs = [(k, e) for k in kp for e in er if k[0] == e[0] and k[0] in uniq]
            if len(pairs) < 2:
                return None
        else:   # geometry: align centroids, then nearest neighbours
            kc = (sum(p[1] for p in kp) / len(kp), sum(p[2] for p in kp) / len(kp))
            ec = (sum(p[1] for p in er) / len(er), sum(p[2] for p in er) / len(er))
            sh = (kc[0] - ec[0], kc[1] - ec[1])
            pairs = [(k, min(er, key=lambda e: math.hypot(e[1] + sh[0] - k[1], e[2] + sh[1] - k[2]))) for k in kp]
        mx = sum(k[1] - e[1] for k, e in pairs) / len(pairs)
        my = sum(k[2] - e[2] for k, e in pairs) / len(pairs)
        err = math.sqrt(sum((k[1] - e[1] - mx) ** 2 + (k[2] - e[2] - my) ** 2 for k, e in pairs) / len(pairs))
        # offset from the pad-pattern bounding boxes: robust to pads of different length
        tx = (min(k[1] for k in kp) + max(k[1] for k in kp)) / 2 - (min(e[1] for e in er) + max(e[1] for e in er)) / 2
        ty = (min(k[2] for k in kp) + max(k[2] for k in kp)) / 2 - (min(e[2] for e in er) + max(e[2] for e in er)) / 2
        if err < 0.02:     # pads coincide exactly: their mean offset is exact
            tx, ty = mx, my
        cand = (round(err, 4), theta, tx, ty, pairs)
        if best is None or cand[0] < best[0] - 1e-6:
            best = cand
    return best


def main():
    board = pcbnew.LoadBoard(PCB)
    seen, out = {}, {}
    for fp in board.GetFootprints():
        lcsc = fp.GetFieldText('LCSC') if fp.HasField('LCSC') else ''
        if not lcsc or lcsc in seen:
            continue
        seen[lcsc] = fp
    for lcsc, fp in sorted(seen.items()):
        name = fp.GetFPID().GetLibItemName().wx_str()
        title, ep, pins = easyeda(lcsc)
        kp = kicad_pads(fp)
        alias = PAD_ALIAS.get(name, 1)
        if isinstance(alias, dict):
            kp = [(alias.get((round(x, 2), round(y, 2)), n), x, y) for n, x, y in kp]
        by_number = alias is not None and sum(any(e[0] == k[0] for e in ep) for k in kp) >= 2
        res = solve(kp, ep, by_number)
        if res is None:
            res = solve(kp, ep, False)
            by_number = False
        err, theta, tx, ty, pairs = res
        note = ''
        if name in POLAR:
            # EasyEDA pad whose symbol pin is the cathode must land on KiCad pad 1
            cath = [n for n, nm in pins.items() if nm.upper() in ('K', 'C', '-', 'CATHODE')]
            if cath:
                k1 = next(p for p in kp if p[0] == '1')
                kc = [e for n, *e in [(e[0], e[1], e[2]) for e in ep] if n == cath[0]]
                for th in (theta, (theta + 180) % 360):
                    ex, ey = rot(th, *[(x, y) for n, x, y in ep if n == cath[0]][0])
                    others = [rot(th, x, y) for n, x, y in ep]
                    cx = sum(o[0] for o in others) / len(others)
                    cy = sum(o[1] for o in others) / len(others)
                    kcx = sum(p[1] for p in kp) / len(kp)
                    kcy = sum(p[2] for p in kp) / len(kp)
                    if math.hypot(ex - cx + kcx - k1[1], ey - cy + kcy - k1[2]) < 0.3:
                        if th != theta:
                            theta = th
                            tx, ty = kcx - cx, kcy - cy
                        note = f'polarity via symbol pin {cath[0]}={pins[cath[0]]}'
                        break
                else:
                    note = 'POLARITY UNRESOLVED'
            else:
                note = 'no K/A pin names: check polarity in preview'
        if name in BODY_ROT:
            extra, why = BODY_ROT[name]
            # the turn is about the land pattern's origin, which lands at (tx, ty): every KiCad pad must have a pad
            # where the turn takes it, or the part would come off its pads
            moved = max(min(math.hypot(2 * tx - x - x2, 2 * ty - y - y2) for _, x2, y2 in kp) for _, x, y in kp)
            if extra % 360 == 180 and moved > 0.3:
                sys.exit(f'{name}: pads not point-symmetric about the origin ({moved:.2f} mm): BODY_ROT cannot apply')
            theta = (theta + extra) % 360
            note = why
        out[lcsc] = {'footprint': name, 'easyeda': title, 'theta': theta, 'dx': round(tx, 4), 'dy': round(ty, 4),
                     'rms_mm': err, 'matched_by': 'number' if by_number else 'geometry', 'note': note}
        print(f'{lcsc:10} {name:42} theta={theta:3}  t=({tx:+.3f},{ty:+.3f})  rms={err:.3f} {"num" if by_number else "geo"} {note}')
    json.dump(out, open(OUT, 'w'), indent=1)
    print('->', OUT)


if __name__ == '__main__':
    sys.exit(main())
