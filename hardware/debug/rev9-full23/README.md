# Live-view snapshot: rev-9 full route, run "full23" (2026-10-26)

Static copy of `tools/build/live/` + `tools/route.log` from

```sh
python3 tools/route.py --live            # 889 s, started 14:26:41, finished 14:41:36
```

on the regenerated rev-9 board (TS3USB3031 service switch re-placed, `PAIR_END_RUNS` keyed per part+pad so the
connector-side SVC link no longer inherits the switch-side 0.5 mm end run).

```sh
python3 -m http.server 8765 --directory hardware/debug/rev9-full23   # then http://localhost:8765/
```

Controls: wheel = zoom, drag = pan, **Fit board** resets, the net box (or clicking copper) highlights a net's pads,
the layer switches toggle F.Cu/B.Cu/In2.Cu/In1.Cu, and the log panel shows the router's own lines, `PAIR FAIL`
included.

## What this run produced

`done: 3026 segments, 1109 vias, 889s`; stitching vias 410; `In2.Cu: 135 track segments, +3V3 plane regions 43,
connections off the main region: 0`; dangling vias: 0 dropped, 42 connected by a stub to their own pad.

**Unrouted: 2 nets `['VBUS_PC2', 'VSNS_PWR']`** (retry round 1 had 8 nets, round 2 the same 6, these 2 survived both).

## What fails, with the coordinates to look at

| Check | Result |
|---|---|
| `check_usb_si.py` | 21 links, 2 FAIL: `H2_AT` 35.83 / 34.83 = **0.99 mm** (6.7 ps), `SVC` 30.79 / 31.78 = **0.99 mm** (6.6 ps); budget 0.75 mm. Every other link <= 0.50 mm, PC1/PC2 0.12 mm |
| `check_copper.py` | 0 track/via clearances, 0 stubs, 0 orphans; **3 nets in more than one piece**: `+3V3` 2, `VBUS_PC2` 2, `VSNS_PWR` 3; 1 bad pair `H0_D4` (gap 0.149 mm - the strapped-off stub pair by intent) |
| `kicad-cli pcb drc --schematic-parity` | **3 violations**, **4 unconnected items**, 200 `net_conflict` (all the `/`-prefix artifact) |
| `check_spacing.py` / `check_aux.py` | no spacing problems / every auxiliary part on the nearer pin's side |

| Failure | Where | Cause in the log |
|---|---|---|
| `VBUS_PC2` unrouted | `U201.5` at (205.00, 114.14) F.Cu | `no rippable blockers for VBUS_PC2; leaving it partly unrouted (in the way: PC2_DM, PC2_DP)` - the pair keeps it out and pairs are not ripped |
| `VSNS_PWR` unrouted (3 pieces) | `R303.2` (153.95, 116.51), `R304.1` (155.39, 136.60) | `walled in by the reservations of ['AT2_NRST']`, then `in the way: H0_UP_DM, H0_UP_DP` |
| `+3V3` in 2 pieces (C501.1 floating) | `C501.1` (159.05, 132.12) F.Cu; the net's via is (158.05, 130.85), 1.28 mm away | `+3V3 C501.1: no via`, then `blocked by a pair/plane net ['AT2_CS', 'GND']` |
| 3 clearance, 0.1346 mm < 0.15 mm | (118.40, 123.40), (164.90, 123.40), (208.40, 123.40) F.Cu | hub GND fan-out track to pad 12 (`unconnected-(Ux01-PRTPWR1/BC_EN1-Pad12)`) of U701/U801/U901 |

## What got better in this run

Rev 8's documented run left 5 nets unrouted - `H1_MS_DP/DM`, `H1_OT_DP/DM` and `SVC_DP`. All three links now route:
`H1_MS` 80.45 / 80.58 (0.13 mm), `H1_OT` 112.36 / 112.37 (0.01 mm), `SVC` 30.79 / 31.78. The three connector->ESD
links are 26 cells each (`PC1`/`PC2` 21.12 / 21.00, `SVC` link 1.18 / 1.19 with skew 0.00), so the roundabout on
`J301 -> U301` is gone. Neither of the two nets still unrouted is a USB pair.
