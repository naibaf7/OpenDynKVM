# Live-view snapshot: 0.18.0, the RP2354B's reset line to the Bluetooth module (2026-11-14)

A static copy of the live view of the seeded run that added `ESP_RST`: the RP2354B's GPIO9 through R1605 (1 kΩ, beside
the module's EN pad, `Fit = BT`) to the module's EN.

```sh
git show HEAD:hardware/kvm.kicad_pcb > /tmp/seed.kicad_pcb        # 0.17.0 (../v17-esp-top/)
uv run tools/gen_pcb.py --force
uv run python -u tools/route.py --live --seed=/tmp/seed.kicad_pcb
```

`route.log` (local, not in git) is that run's log: 6 s, 0 unrouted nets, 3092 segments, 1005 vias.

**Why:** on 0.16.0 and 0.17.0 the module's EN has only its RC, so the RP2354B can't restart a module that hangs, and
getting a blank or broken one into its ROM means holding its BOOT strap low (a UART break) until the module resets by
itself. With the line, the firmware resets it under a 300 ms break (software.md §4.13).

**Which pin:** GPIO30 (pin 38), beside the UART on pins 36/37, was tried first and did not route:

| Run | Re-routed with it | Unrouted |
|---|---|---|
| a | (none) | `ESP_RST`: no way out of pin 38 past the crystal's B.Cu run and `ESP_RXD`/`ESP_TXD` |
| b | `ESP_TXD`, `ESP_RXD` | `ESP_RST` (now `OT_EN`, `RP_RUN`, `RP_SWDIO` in the way) |
| c, f, i | + `OT_EN`, `RP_RUN`, `RP_SWDIO` (workers default, 10, 12) | `RP_RUN` (`RP_SWCLK`, `RP_XIN` in the way) |
| d, g, j | + `RP_SWCLK` (workers default, 10, 8) | `ESP_RST` (the `H1_OT` and `H2_MS` pairs in the way) |
| e, h | the same set, `RP_RUN` first | `ESP_RST`, `ESP_RXD`, `ESP_TXD`, `RP_SWDIO` |

The worker count changed nothing; the order changed which net lost.  Pins 33-40 under the crystal line have no room for
one more line, which is what stopped GPIO30/31's fan-out on 0.16.0 too.  GPIO9 (pin 7, on the west side, with free
pins 6 and 8 either side) routed at the first try with every other net's copper kept.

## Checks on the result

- Copper against 0.17.0, item by item: identical apart from `ESP_RST` and `ESP_EN` (R1605's pad).
- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 parity issues, 0 footprint errors.
- `check_copper.py`: PASS (the accepted `H0_D4` pinch only).  `check_spacing.py`: no spacing problems.
  `check_aux.py`: passes.
- `check_usb_si.py`: 0 hard failures; the pairs' copper is 0.17.0's.
- `dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5`: **84.5 / 73.9 / 54.0 / 15.4 mV**, as 0.17.0
  (the +5 V copper is unchanged).
- Antenna keep-out (214.1, 134.4) - (220.0, 166.4): no track or via.
- `ESP_RST`: 72.5 mm, 5 vias; 1.8 mm on F.Cu, 25.1 mm on B.Cu, 45.7 mm on In2.Cu (a slow open-drain line: the router
  keeps In2.Cu signals out from under B.Cu USB pairs).
- `case/gen_case.py`: the case still fits (its board check passed; tallest top part 8.0 of 9.0 mm).
- Afterwards R1604 (the IO9 strap's pull-up) went from 10 kΩ to 5.1 kΩ, a value only: a seeded run from this board
  (5 s, nothing re-routed) gave the same copper, item for item, and the same check results.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/v18-esp-en
# then: http://localhost:8765/
```
