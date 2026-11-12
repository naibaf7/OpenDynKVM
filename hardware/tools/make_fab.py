#!/usr/bin/env python3
"""
OpenDynKVM -- JLCPCB fabrication outputs (adapted from the OpenRower tool).

Writes bom/jlcpcb_bom.csv and bom/jlcpcb_cpl.csv from kvm.kicad_pcb (LCSC numbers
come from the symbols' LCSC field).  Once the board is routed it also writes gerbers + drill
files to fab/ and zips them.  Coordinates use the board's place/drill origin (bottom-left).

Two assembly variants (0.16.0): every part, and every part but those with the field Fit = BT (the Bluetooth module and
its parts, hardware/tools/gen_schematic.py block 16), written as jlcpcb_bom.csv / jlcpcb_cpl.csv and
jlcpcb_bom_nobt.csv / jlcpcb_cpl_nobt.csv.  The board works either way: the firmware finds the module or its absence.

JLC assembles exactly the parts with an LCSC number, on both sides (since 2026-10-27: the per-pin decoupling
caps under the chips make it double-sided assembly anyway, so the MX key's hot-swap socket and the reverse-mount key
LED go to JLC too; only the switch itself is plugged in afterwards).  A part with only an MPN would go to
bom/hand_assembly.csv.  Parts with neither (mounting holes, Tag-Connect pads) are not parts.

JLC places parts with their EasyEDA footprints, whose origin and 0-degree orientation differ
from KiCad's for many packages.  The per-part corrections in tools/jlc_corrections.json (made by
tools/jlc_align.py from the actual EasyEDA footprints) are applied to the CPL:
    rotation = KiCad rotation + theta,   position = KiCad position + R(rotation) * t

Run from hardware/:   uv run tools/make_fab.py
"""
import csv
import json
import math
import os
import shutil
import subprocess

import pcbnew

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PCB = os.path.join(HERE, 'kvm.kicad_pcb')
BOMDIR = os.path.join(HERE, 'bom')
FABDIR = os.path.join(HERE, 'fab')


def natural(r):
    head = r.rstrip('0123456789')
    return (head, int(r[len(head):] or 0))


CORR = os.path.join(HERE, 'tools', 'jlc_corrections.json')


def on_bottom(fp):
    """The part sits on the bottom: a footprint flipped to B.Cu, or a top footprint whose soldered pads are all on
    B.Cu (the MX key's hot-swap socket, mounted under the switch)."""
    if fp.IsFlipped():
        return True
    pads = [p for p in fp.Pads() if p.GetAttribute() != pcbnew.PAD_ATTRIB_NPTH]
    return bool(pads) and not any(p.IsOnLayer(pcbnew.F_Cu) for p in pads)


def jlc_place(fp, lcsc, corr):
    """KiCad footprint position/orientation -> JLC (EasyEDA footprint) position/orientation.

    Top:     rotation = phi + theta,          position = KiCad position + R(phi) * t
    Bottom:  rotation = a + theta,            position = KiCad position + mirror_x(R(a) * t)
             with a = 180 - phi for a flipped footprint (JLC looks at the bottom from below, where the part shows its
             own top view; KiCad's left/right flip adds 180 - the convention of kicad-jlcpcb-tools' fabrication.py),
             a = -phi for the socket under a top footprint (the same part flipped would read phi + 180).
    theta and t are measured in the part's own top view (tools/jlc_align.py kicad_pads), so they mean the same on
    both sides.  The bottom rows are derived, not yet confirmed by a JLC order: check D1501 / SW1501 (the polarized
    bottom parts) in JLC's placement preview."""
    if lcsc not in corr:
        raise SystemExit(f'{fp.GetReference()} ({lcsc}) has no JLC correction: run tools/jlc_align.py')
    c = corr[lcsc]
    phi = fp.GetOrientationDegrees() % 360
    bottom = on_bottom(fp)
    ang = phi if not bottom else ((180 - phi) if fp.IsFlipped() else -phi) % 360
    a = math.radians(ang)
    tx, ty = c['dx'], c['dy']
    ox, oy = tx * math.cos(a) + ty * math.sin(a), -tx * math.sin(a) + ty * math.cos(a)
    if bottom:
        ox = -ox
    pos = fp.GetPosition()
    return pcbnew.ToMM(pos.x) + ox, pcbnew.ToMM(pos.y) + oy, (ang + c['theta']) % 360


def main():
    board = pcbnew.LoadBoard(PCB)
    corr = json.load(open(CORR))
    origin = board.GetDesignSettings().GetAuxOrigin()
    rows, groups, hand = [], {}, []
    for fp in board.GetFootprints():
        attrs = fp.GetAttributes()
        ref = fp.GetReference()
        lcsc = fp.GetFieldText('LCSC') if fp.HasField('LCSC') else ''
        mpn = fp.GetFieldText('MPN') if fp.HasField('MPN') else ''
        side = 'Bottom' if on_bottom(fp) else 'Top'
        if not lcsc:
            if mpn:
                pos = fp.GetPosition()
                hand.append((ref, fp.GetValue(), mpn, side, pcbnew.ToMM(pos.x) - pcbnew.ToMM(origin.x),
                             pcbnew.ToMM(origin.y) - pcbnew.ToMM(pos.y)))
            elif not (attrs & pcbnew.FP_EXCLUDE_FROM_BOM) and fp.Pads() and \
                    any(p.GetAttribute() == pcbnew.PAD_ATTRIB_SMD for p in fp.Pads()) and not ref.startswith('J'):
                raise SystemExit(f'{ref} has neither an LCSC number nor an MPN')
            continue
        key = (fp.GetFPID().GetLibItemName().wx_str(), lcsc)
        fit = fp.GetFieldText('Fit') if fp.HasField('Fit') else ''
        groups.setdefault(key, []).append((ref, fp.GetValue(), fit))
        qx, qy, rot = jlc_place(fp, lcsc, corr)
        rows.append((ref, qx - pcbnew.ToMM(origin.x), pcbnew.ToMM(origin.y) - qy, side, rot, fit))

    os.makedirs(BOMDIR, exist_ok=True)
    for suffix, keep in (('', lambda fit: True), ('_nobt', lambda fit: fit != 'BT')):
        with open(os.path.join(BOMDIR, f'jlcpcb_bom{suffix}.csv'), 'w', newline='') as f:
            w = csv.writer(f)
            w.writerow(['Comment', 'Designator', 'Footprint', 'LCSC Part #'])
            for (fpname, lcsc), items in sorted(groups.items(), key=lambda kv: natural(min(r for r, _, _ in kv[1]))):
                items = [(r, v) for r, v, fit in items if keep(fit)]
                if not items:
                    continue
                refs = sorted((r for r, _ in items), key=natural)
                values = sorted({v for _, v in items})
                w.writerow(['/'.join(values), ','.join(refs), fpname, lcsc])
        with open(os.path.join(BOMDIR, f'jlcpcb_cpl{suffix}.csv'), 'w', newline='') as f:
            w = csv.writer(f)
            w.writerow(['Designator', 'Mid X', 'Mid Y', 'Layer', 'Rotation'])
            for ref, x, y, layer, rot, fit in sorted(rows, key=lambda r: natural(r[0])):
                if keep(fit):
                    w.writerow([ref, f'{x:.3f}mm', f'{y:.3f}mm', layer, f'{rot:.1f}'])
    with open(os.path.join(BOMDIR, 'hand_assembly.csv'), 'w', newline='') as f:
        w = csv.writer(f)
        w.writerow(['Designator', 'Value', 'MPN', 'Layer', 'X', 'Y'])
        for ref, val, mpn, side, x, y in sorted(hand, key=lambda r: natural(r[0])):
            w.writerow([ref, val, mpn, side, f'{x:.3f}mm', f'{y:.3f}mm'])
    bt = sum(1 for r in rows if r[5] == 'BT')
    print(f'BOM: {len(groups)} lines, CPL: {len(rows)} parts ({len(rows) - bt} without the Bluetooth module\'s {bt}), '
          f'hand assembly: {len(hand)} parts -> {BOMDIR}')

    if not len(board.GetTracks()):
        print('board not routed yet: skipping gerbers/drill')
        return
    shutil.rmtree(FABDIR, ignore_errors=True)
    os.makedirs(FABDIR)
    layers = 'F.Cu,In1.Cu,In2.Cu,B.Cu,F.Paste,B.Paste,F.Silkscreen,B.Silkscreen,F.Mask,B.Mask,Edge.Cuts'   # B.Paste: bottom stencil
    subprocess.run(['kicad-cli', 'pcb', 'export', 'gerbers', '--use-drill-file-origin', '-l', layers,
                    '-o', FABDIR + '/', PCB], check=True)
    subprocess.run(['kicad-cli', 'pcb', 'export', 'drill', '--drill-origin', 'plot', '--excellon-separate-th',
                    '-o', FABDIR + '/', PCB], check=True)
    shutil.make_archive(os.path.join(HERE, 'kvm_gerbers'), 'zip', FABDIR)
    print('gerbers + drill -> fab/, kvm_gerbers.zip')


if __name__ == '__main__':
    main()
