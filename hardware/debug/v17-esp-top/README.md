# Live-view snapshot: 0.17.0 with the Bluetooth module on the top (2026-11-12)

A static copy of the live view of the seeded run that moved the ESP32-C3-MINI-1 (U1601) and its eight passives from
the bottom side to the top:

```sh
git show HEAD:hardware/kvm.kicad_pcb > /tmp/seed.kicad_pcb        # 0.17.0 as routed from scratch (../v17-full/)
uv run tools/gen_pcb.py --force                                   # U1601 at 270 degrees on F.Cu, passives mirrored
uv run python -u tools/route.py --live --seed=/tmp/seed.kicad_pcb \
    --move=U1601,C1601,C1602,C1603,R1601,R1602,R1603,R1604,TP1601
```

`route.log` (local, not in git) is that run's log: 18 s, 0 unrouted nets, 3062 segments, 1000 vias.

**Why:** with the module on the bottom, it was the one large part there; now the bottom carries only the 32
decoupling caps, the key's hot-swap socket and its LED (34 parts in both assembly variants, from 42 with the module).

**What moved:** the module keeps its centre (208.5 / 156.0) and its antenna at the east edge, so its pad rows mirror:
the 3V3, EN, IO2 and IO3 pads face north, the UART pads (RXD0, TXD0) south, IO8 and IO9 sit at the south end of the
west side.  The passives follow, mirrored; the two pull-ups west of the module stand on end between it and the logo.

**How it routed:** `route.py --move` takes the named parts as new parts.  The module's own nets keep 0.17.0's copper
except where it touched the old or new pads, and the router joined what was left to the new pads.  The two UART lines
keep their long runs from the RP2354B: `ESP_RXD` its northern one, `ESP_TXD` its southern one along y 163.6, which
now ends next to its pad.  Two attempts that did not work: a full run from scratch (the short nets came in a
different order, and `VBUS_PWR` at the power USB-C was left unrouted; `ESP_TXD` only routed in the last resort), and
a seeded run that dropped the module's lines and routed them anew (both lines then wanted the northern corridor, and
`ESP_RXD` was left unrouted).

## Checks on the result

- Copper against 0.17.0 (../v17-full/), item by item: 4011 of 4065 items identical; 54 removed and 51 added, all on
  the module's nets (`ESP_RXD`, `ESP_TXD`, `ESP_EN`, `ESP_IO2`, `ESP_IO8`, `ESP_LOG`) and its `+3V3` / `GND` vias.
- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 parity issues, 0 footprint errors.
- `check_copper.py`: PASS (the accepted `H0_D4` pinch only).
- `check_spacing.py`: no spacing problems (U1601 is an edge part now that it is measured: its F.Fab body is 0.47 mm
  inside the east edge, as it was on the bottom).  `check_aux.py`: passes.
- `check_usb_si.py`: 0 hard failures; the pairs' copper is 0.17.0's.
- `dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5`: **84.5 / 73.9 / 54.0 mV**, as 0.17.0 (the
  +5 V copper is unchanged).
- Antenna keep-out (214.1, 134.4) - (220.0, 166.4): no track or via.
- The module's lines: `ESP_RXD` 72.4 mm, 5 vias (0.17.0: 66.1 mm, 4); `ESP_TXD` 77.6 mm, 4 vias (80.0 mm, 5).
  `ESP_RXD` runs 4.5 mm on F.Cu under the module's body, from IO9 (pad 23) to RXD0 (pad 30), 0.27 mm from the south
  pad row; `ESP_TXD` passes under the module on B.Cu.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/v17-esp-top
# then: http://localhost:8765/
```
