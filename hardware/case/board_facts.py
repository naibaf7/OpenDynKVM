#!/usr/bin/env python3
"""The board facts the case is built around, from ../kvm.kicad_pcb, written to board.json.

In the repository's environment (it needs pcbnew, KiCad's module: DEVELOPMENT.md, "Python scripts"), from anywhere in
the repository:

    uv run hardware/case/board_facts.py

gen_case.py then builds the case from board.json alone, in its own environment (build123d), which has no pcbnew.
board.json is in KiCad's frame: millimetres, x to the right, y down (the rear edge is the top of the board).

What is written:
- the board outline (bounding box) and thickness, and HW_VERSION from tools/gen_pcb.py;
- the mounting holes (H*), the USB-C (J101, J201, J301) and USB-A (J1201, J1301, J1401) connectors, the side LEDs
  (D1502 to D1506), the key (SW1501) and the bootloader button (SW501): position and courtyard;
- every footprint's courtyard, side and footprint name, for gen_case.py's clearance checks.
"""
import json
import re
import sys
from pathlib import Path

import pcbnew

HERE = Path(__file__).resolve().parent
BOARD = HERE.parent / 'kvm.kicad_pcb'
GEN_PCB = HERE.parent / 'tools' / 'gen_pcb.py'
OUT = HERE / 'board.json'


def mm(v):
    return round(pcbnew.ToMM(v), 4)


def courtyard(fp):
    """The footprint's courtyard on its own side as [x0, y0, x1, y1]; its pads' box when it has none."""
    layer = pcbnew.B_CrtYd if fp.IsFlipped() else pcbnew.F_CrtYd
    poly = fp.GetCourtyard(layer)
    box = poly.BBox() if poly.OutlineCount() else fp.GetBoundingBox(False)
    return [mm(box.GetLeft()), mm(box.GetTop()), mm(box.GetRight()), mm(box.GetBottom())]


def main():
    board = pcbnew.LoadBoard(str(BOARD))
    edge = board.GetBoardEdgesBoundingBox()
    version = re.search(r"^HW_VERSION = '([^']+)'", GEN_PCB.read_text(), re.M)
    parts, by_ref = [], {}
    for fp in sorted(board.GetFootprints(), key=lambda f: f.GetReference()):
        p = fp.GetPosition()
        item = {'ref': fp.GetReference(), 'footprint': str(fp.GetFPID().GetLibItemName()),
                'side': 'bottom' if fp.IsFlipped() else 'top', 'x': mm(p.x), 'y': mm(p.y),
                'rot': round(fp.GetOrientationDegrees(), 3), 'courtyard': courtyard(fp)}
        drills = [mm(pad.GetDrillSize().x) for pad in fp.Pads() if pad.GetDrillSize().x > 0]
        if fp.GetReference().startswith('H') and drills:
            item['drill'] = max(drills)
        parts.append(item)
        by_ref[item['ref']] = item

    def pick(refs):
        missing = [r for r in refs if r not in by_ref]
        if missing:
            sys.exit(f'{BOARD.name}: no {", ".join(missing)}')
        return [by_ref[r] for r in refs]

    facts = {
        'source': 'hardware/kvm.kicad_pcb',
        'hw_version': version.group(1) if version else None,
        'outline': [mm(edge.GetLeft()), mm(edge.GetTop()), mm(edge.GetRight()), mm(edge.GetBottom())],
        'thickness': mm(board.GetDesignSettings().GetBoardThickness()),
        'holes': [p for p in parts if p['ref'].startswith('H') and 'drill' in p],
        'usb_c': pick(['J101', 'J301', 'J201']),
        'usb_a': pick(['J1201', 'J1301', 'J1401']),
        'leds': pick(['D1502', 'D1503', 'D1504', 'D1505', 'D1506']),
        'key': pick(['SW1501'])[0],
        'button': pick(['SW501'])[0],
        'parts': parts,
    }
    if len(facts['holes']) != 4:
        sys.exit(f'{BOARD.name}: {len(facts["holes"])} mounting holes, the case has 4 posts')
    OUT.write_text(json.dumps(facts, indent=1) + '\n')
    print(f'{OUT.name}: hardware {facts["hw_version"]}, {len(parts)} footprints, '
          f'board {facts["outline"][2] - facts["outline"][0]:.2f} x {facts["outline"][3] - facts["outline"][1]:.2f} mm')


if __name__ == '__main__':
    main()
