# Live-view snapshot: the MHL-swap routing attempt (2026-10-23)

Static copy of `tools/build/live/` from a `python3 tools/route.py --live --pairs-only` run of the **attempted
MHL-swap layout** described in [report.md](../../../report.md) finding **F4**: Hub-0 (the full-speed KVM path) on
the TS3USB3031's MHL channel, Hub-1 (PC1) on USB2, Hub-2 (PC2) on USB1, with the six router-side lanes re-planned
and Hub-0 routed first.

It is a *pairs-only* board on purpose: only the fixed copper and the differential pairs are in it, which is
exactly the state the pair router sees (`LED_D0`, `AT1_XI`, `KB_FLT` and the power nets are routed later and are
therefore not in the way at this stage).

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/swap-attempt
# then: http://localhost:8765/
```

Controls: mouse wheel = zoom, drag = pan, **Fit board** to reset, the **net box** (or clicking copper) highlights a
net's pads, the layer switches toggle F.Cu/B.Cu/In2.Cu/In1.Cu, and the log panel shows the router's own lines.

## What to look at

| Net | Coordinates | Measured | Meaning |
|---|---|---|---|
| `H0_KB_DP`/`H0_KB_DM` | **(110.05, 153.82) F.Cu** | centre gap 0.150 mm, **edge gap 0.000 — touching** | the pair cannot leave the router's west pads (0.4 mm pitch) and taper to via spacing before it runs out of room; it was squeezed right at the pads |
| `H1_MS_DP`/`H1_MS_DM` | **(142.85, 152.20) B.Cu** | centre gap 0.005 mm, **overlapping** | squeezed at its own layer change |
| `H1_OT_*`, `H2_KB_*` | – | **absent** | failed to route at all (`PAIR FAIL … leg 3 -> (160.0, 145.5)`) |
| `H0_MS_DP`/`H0_MS_DM`, `H0_OT_*`, `H1_KB_*`, `H2_MS_*`, `H2_OT_*` | – | gap 0.150 mm, skew 0.23–0.90 mm | these came out clean, so the geometry is close |

Fix ideas, in the order I would try them: give the Hub-1 keyboard lane more room by moving its westward band from
y≈151.75 to y≈151.0 (Hub-0's taper then has ~0.7 mm more); or move the router ~0.5 mm; or replace each router
with two cascaded 1:2 switches.
