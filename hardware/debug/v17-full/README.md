# Live-view snapshot: 0.17.0 routed from scratch (2026-11-11)

A static copy of the live view of the full run that made the 0.17.0 board:

```sh
python3 tools/gen_pcb.py --force
python3 -u tools/route.py --live
```

`route.log` (local, not in git) is that run's log: 1093 s, 0 unrouted nets, 3063 segments, 1002 vias.

**What 0.17.0 changes** (hardware/errata.md, from the first 0.12.0 board):
- **E2:** R510, `SVC_SEL`'s pull-up, is 10k instead of 100k, so the service port works while the RP2354B runs no
  firmware. Netlist only.
- **E3:** the five side LEDs D1502 to D1506 sit at 0° with their lens 0.5 mm from the front edge (at 180° they shone
  into the board). Their pads moved, which is why the board could not be seeded from 0.16.0.

## Checks on the result

- `kicad-cli sch erc`: 0 violations; `check_netlist.py`: OK (1030 pins, 294 nets), USB polarity 0 crossed.
- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 parity issues, 0 footprint errors.
- `check_copper.py`: PASS (the accepted `H0_D4` pinch only, as on 0.15.0 and 0.16.0).
- `check_spacing.py`: no spacing problems.  `check_aux.py`: passes.
- `check_usb_si.py`: 0 hard failures; every pair has 0.16.0's lengths and skew (RP 0.02 mm, SVC 0.36 mm, PC1/PC2
  0.12 mm, OT 0.00 mm).
- `dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5`: **85 / 74 / 54 mV** (0.16.0: 84 / 74 /
  49 mV); typical load (0.1 / 0.1 / 0.5 A + buck 0.4 A): 32 / 30 / 27 mV.  +5 V runs 123 mm on F.Cu, 153 mm on B.Cu,
  5.1 mm on In2.Cu.
- Antenna keep-out (214.1, 134.4) - (220.0, 166.4): no track or via.
- `xtal_report.py`: `RP_XIN` 13.45 mm routed (2 vias; 0.16.0: 13.70 mm), `RP_XO_R` 21.91 mm on F.Cu (0.16.0: the same).
- The status LED chain between the side LEDs (`LED_D1` to `LED_D4`) runs 0.95 to 3.45 mm in from the front edge, on
  F.Cu and on B.Cu under the LED bodies.
