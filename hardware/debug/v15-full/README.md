# Live-view snapshot: 0.15.0 routed from scratch (2026-11-07)

A static copy of the live view of the full run that made the 0.15.0 board:

```sh
python3 tools/gen_pcb.py --force
python3 -u tools/route.py --live
```

`route.log` is that run's log: 739 s, 0 unrouted nets.

**What 0.15.0 is.** 0.14.0's netlist, where every pair pin of the service switch U601 carries the line the datasheet
names it for ([errata E1](../../errata.md)), routed from scratch instead of from 0.12.0's copper. The RP and service
pairs cross over in copper: each link is routed as if it needed no crossover, and a 1.6 mm twist goes into a straight
stretch of its own new lines (`route.py` `twist_in_run`).

| Link | Copper | Length P / N, skew |
|---|---|---|
| RP pair `RP_DP`/`RP_DM` (27R resistors to U601) | twist at (137.70, 115.46) in its leg up the switch's west side, vias 0.65 mm out | 20.72 / 20.71 mm, 0.02 mm (0.12.0: 0.61 mm) |
| Service pair `SVC_DP`/`SVC_DM` (U301 to U601) | twist at (148.94, 113.60) east of the switch, vias 0.65 mm out | the whole net 33.71 / 34.07 mm, 0.36 mm (0.12.0: 0.88 mm) |
| Hub-0 upstream `H0_UP_DP`/`H0_UP_DM` | straight, no vias | 21.19 / 21.19 mm, 0.00 mm |
| every other USB link | as on 0.12.0 | 0.12.0's lengths, skew and vias exactly |

Checks on the result:
- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 footprint errors.
- `check_copper.py`: PASS, with no accepted `H0_D4` pinch.
- `check_spacing.py`: no spacing problems.
- `check_aux.py`: passes.
- `check_netlist.py`: OK, 0 crossed.
- `check_usb_si.py`: 0 hard failures.
- `dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5`: 96 / 87 / 66 mV (0.12.0: 91 / 82 / 59 mV).

**Three full runs, one board.** The same sources were routed three times. All three have identical USB copper, item
for item (1027 tracks and vias); signals and rails differ, as the router is not bit-deterministic.

| Run | Time | Board checks | +5 V drop, USB-rated load | |
|---|---|---|---|---|
| 1 | 1022 s | DRC: one `starved_thermal` at J301's south-west shield pad | 495 / 483 / 460 mV (+5 V 196 mm on B.Cu) | before `THT_SPOKE_ROOM` |
| 2 | 795 s | all pass | 121 / 109 / 89 mV (+5 V 96.5 mm on In2.Cu) | three accepted `H0_D4` pinches |
| **3** | **739 s** | **all pass** | **96 / 87 / 66 mV** | **the 0.15.0 board** |

Run 3 is the board because its +5 V feed matches 0.12.0's; the board checks cannot tell runs 2 and 3 apart. Run 3
pays for it at one crystal: `AT2_XI` is routed 10.46 mm instead of 4.34 mm (4.02 mm straight, no vias; about 0.6 pF
more on a 15 pF leg), still within the board's other crystal nets (`RP_XO_R` 22 mm, `H0_XI` 10.7 mm).

**How this snapshot was made.** Run 3's live state was not kept, so `state.json` was rebuilt from its board in the
format of `route.py` `LiveView.update`. The same rebuild, done on run 2's board, reproduced that run's real
`state.json` exactly (2968 segments, 1000 vias, the log). The reserved fan-out rows (virtual copper, drawn faintly
while routing) are not on a board and are left out. The ok / failed counters in the header are inferred from the log
the way run 2's came out (net routes plus 27, failed attempts). `static.json` and `index.html` are run 2's, which are
the same pads, outline and viewer.

On the machine that routed it, `run2/` has run 2's live view, log and board (not in git, like the other snapshot
boards).

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/v15-full
# then: http://localhost:8765/   (run 2: http://localhost:8765/run2/)
```

Controls:
- mouse wheel: zoom; drag: pan; **Fit board** resets the view;
- the **net box**, or clicking copper, highlights a net's pads;
- the layer switches toggle F.Cu, B.Cu, In2.Cu and In1.Cu;
- the log panel shows the router's own lines.
