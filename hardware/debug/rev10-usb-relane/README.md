# Live-view snapshot: Hub-1's re-planned USB lanes, full run (2026-10-27)

Static copy of `tools/build/live/` from the final `python3 tools/route.py --live` run (full run, 429 s, 2962
segments, 1076 vias, `unrouted: 0 nets []`) after Hub-1's Mouse and Other lanes were re-planned:

- **H1_MS** (Mouse router U1302 pins 5/6 -> Hub-1 pins 4/3) runs straight at the hub instead of round the west edge:
  F.Cu out of the router and west along y 145.95, one via pair at (120.3, 141.5) south of AT32 #1's SPI pins, B.Cu
  north under AT32 #1's east pin row and under H1_AT, and a via pair at (116.25, 128.85) up into the pins.
  51.13 mm per line (was 82.98 mm, and only after a retry), skew 0.00, 2 via pairs.
- **H1_OT** (Other router -> Hub-1 pins 7/6) stays north: B.Cu at y 124.3 under the hub row until x 123.5, then one
  steep V under H1_AT into a via pair at (117.75, 126.75) - nothing south of y 128.6 is used, which is the room the
  Mouse pair now takes.  95.13 mm, skew 0.00.

Look at Hub-1 (around x 110-125, y 120-145) with In2.Cu off.  The verification of this board (DRC 0 violations,
0 unconnected, 0 parity; check_copper / check_spacing / check_aux / check_usb_si all pass) is in
[analysis.md](../../../analysis.md) section 9.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/rev10-usb-relane
```

then http://localhost:8765/ (stop the live router's server first: `fuser -k -TERM 8765/tcp`).
