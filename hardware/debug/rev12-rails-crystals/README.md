# Live-view snapshot: power rails sized per layer, crystal rules (2026-10-28)

Static copy of `tools/build/live/` from the rev-12 `python3 tools/route.py --live` run (874 s, 3002 segments, 1059
vias, `unrouted: 0 nets []`, no `PAIR FAIL`).

What changed against rev 11 (`../rev11-hub-caps/`), all in the generators:

- **Power rails** are sized for the copper of the layer they run on (`route.py` `RAIL_LW`, from the board's stack-up):
  In2.Cu is 0.5 oz, so a rail there is 2.3 x its F.Cu / B.Cu width, capped at 1.39 mm (`RAIL_W_MAX`: the widest track
  the distance fields can check).  +5V also has a drop rule (`RAIL_DROP`, 1 mV/mm at 2.24 A): 1.1 mm on the outer
  layers.  Look at +5V from the TPS2121 (U402, x 175, y 108) down to the three port switches (y 160).
- **Crystals**: crystal lines stay off In2.Cu and a via costs them 5 mm of detour; every other net keeps out of the
  crystals' courtyards and 0.5 mm off crystal lines, except within 2.5 mm of the crystal's chip.  The RP2354B's 1 kOhm
  (R502) moved from the crystal to right below its XOUT pin.  Look at Y501 (x 161.5, y 141.5) and the RP2354B's south
  side (x 148, y 141).
- **Port ESDs** (U101, U201, U301) belong to their own nets on every layer: their middle VBUS pad's only escape is a via
  between the pad rows.

The verification and the numbers are in [analysis.md](../../../analysis.md) section 12.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/rev12-rails-crystals
```

then http://localhost:8765/ (stop the live router's server first: `fuser -k -TERM 8765/tcp`).
