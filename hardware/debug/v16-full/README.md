# Live-view snapshot: 0.16.0 routed from scratch (2026-11-09)

A static copy of the live view of the full run that made the 0.16.0 board (the Bluetooth module U1601 added):

```sh
python3 tools/gen_pcb.py --force
python3 -u tools/route.py --live
```

`route.log` (local, not in git) is that run's log: 870 s, 0 unrouted nets, 3076 segments, 1010 vias.

**What 0.16.0 adds.** The ESP32-C3-MINI-1 on the bottom side, east edge (U1601, design.md §16), its two UART lines to
the RP2354B's GPIO28/29 (pins 36/37, `ESP_RXD` / `ESP_TXD`), its strap and decoupling parts, and an antenna keep-out on
every copper layer.  The logos moved off the module's corner.

## Checks on the result

- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 parity issues.
- `kicad-cli sch erc`: 0 violations; `check_netlist.py`: OK (1030 pins, 294 nets), USB polarity 0 crossed.
- `check_copper.py`: PASS (the accepted `H0_D4` pinch only, as on 0.15.0).
- `check_spacing.py`: no spacing problems.  `check_aux.py`: passes.  `check_usb_si.py`: 0 hard failures.
- `dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5`: **84 / 74 / 49 mV** (0.15.0: 96 / 87 / 66 mV;
  0.12.0: 91 / 82 / 59 mV).  +5 V runs 174 mm on F.Cu, 95.5 mm on B.Cu, 13.6 mm on In2.Cu.
- Antenna keep-out (214.1, 134.4) - (220.0, 166.4) on F.Cu, In1.Cu, In2.Cu and B.Cu: no track, no via, no zone fill.
- `xtal_report.py`: `RP_XIN` 13.70 mm routed (2 vias, 0.15.0: 15.49 mm), `RP_XO_R` 21.91 mm on F.Cu (0.15.0: 21.06 mm).

## Why the router changed

The module's UART sits on the RP2354B's south row between the SWD pins (33-35) and OT_EN (40), the pins 0.15.0 left
free and used as room for the crystal input and the SWD lines.  Seeding from 0.15.0 (`--seed`, with `--reroute` of the
SWD, RUN, OT_EN and crystal lines) never fitted the two new lines in; full runs left one to three of `ESP_RXD`,
`RP_XIN`, `RP_SWCLK`, `RP_SWDIO`, `RP_RUN`, `MS_SEL1`, `MS_FLT` unrouted.  What it took (`tools/route.py`):

| Change | Why |
|---|---|
| UART pins get planned via sites like any pin (`FANOUT_SURFACE` empty) and route last, after power | as fixed lanes they split the south row's plan (MS_SEL1 / SWCLK lost their sites); routed before power they took the In2.Cu crossing of the OTHER port's pairs that +5 V needs (+5 V unrouted in three runs) |
| `XTAL_LANES`: `RP_XIN` laid as fixed copper before the dense fan-outs are planned - down to B.Cu right under pins 29/30, above R502, along y 141.85 under the other pins' lanes, up at (152.75, 141.85), F.Cu to Y501 | the crystal input must cross the fan-out of pins 33-40; routed later it found no way through, or a way that shut in MS_FLT (every via spot west of R502) or RP_SWCLK (every spot east of it); a reservation was not enough, the line took a shorter way and shut MS_FLT in again |
| `FANOUT_SKIP = {'U501.29'}` | its planned +3V3 via sat on the crystal's way; left alone, the plane fan-out ties it inwards as on 0.15.0 |
| `XTAL_PIN_ZONE` 2.5 -> 3.5 mm | the crystal guard vetoed `RP_SWDIO`'s planned via site, 3.3 mm below the pads, 0.53 mm from `RP_XO_R` |
| SWD lines (RUN, SWDIO, SWCLK) right after the crystal, before the RP2354B's other nets | OT_EN, routed first, looped round Hub-2's mouse pair on In2.Cu and walled RP_RUN in |
| rule areas: copper extent, not centres; board-level ones too; `write_board` keeps them | two vias sat 0.15 mm outside the logo's no-via area with their pads inside it (DRC); the antenna keep-out moved to the board (a zone inside the module's footprint made DRC report `lib_footprint_mismatch`) and the pour step had deleted it |

Each experiment ran as a full route, up to three side by side in copies of `hardware/` on ports 8766-8768, with
`tools/build/live/` of each kept until it was understood.  Runs of the same code were mostly identical item for item; this
one (16 planner threads) and its predecessor (12, sharing the machine) both routed fully and differ by a few items.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/v16-full
# then: http://localhost:8765/
```
