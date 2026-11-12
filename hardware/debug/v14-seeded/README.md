# Live-view snapshot: 0.14.0 routed from the 0.12.0 board (2026-11-06)

A static copy of `tools/build/live/` from the seeded run that made the 0.14.0 board:

```sh
git show hw-v0.12.0:hardware/kvm.kicad_pcb > /tmp/seed.kicad_pcb
python3 tools/gen_pcb.py --force
python3 -u tools/route.py --live --seed=/tmp/seed.kicad_pcb
```

`route.log` is that run's log.

**What changed against 0.12.0.** The fix for [errata E1](../../errata.md) and the badge. Every pair pin of the service
switch U601 now carries the line the datasheet names it for: D+ 11 = `RP_DP`, USB1+ 3 = `H0_UP_DP`, USB2+ 5 = `SVC_DP`.
Everything else is 0.12.0's copper, item for item: 3990 of its 4061 tracks and vias.

| Change | Copper |
|---|---|
| RP pair `RP_DP`/`RP_DM` (27R resistors to U601) | 0.12.0's two lines, with a 1.6 mm twist spliced in at (137.70, 115.46), in the leg up the switch's west side. Vias 0.45 mm out at (137.25, 115.75) and (138.15, 116.75). Skew 0.74 mm (0.12.0: 0.61 mm) |
| Service pair `SVC_DP`/`SVC_DM` (ESD U301 to U601) | 0.12.0's two lines, with a 1.6 mm twist spliced in at (156.13, 115.08), in the diagonal leg south-west of the ESD. Vias 0.65 mm out at (156.80, 114.85) and (156.60, 116.45); the crossing line's S is 0.85 mm high, which trims the skew to 0.64 mm (0.12.0: 0.88 mm) |
| Hub-0 upstream pair `H0_UP_DP`/`H0_UP_DM` | unchanged: 0.12.0's, without the twist 0.13.0 had |
| `SVC_SEL` | cut within 1.5 mm of the RP twist (one segment), then rejoined by the signal router |
| `GND` | the 35 stitching vias under the badge's no-via area are gone |
| Zones | the same four, refilled |

Checks on the result:
- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 footprint errors.
- `check_copper.py`: PASS.
- `check_spacing.py`: no spacing problems.
- `check_aux.py`: passes.
- `check_netlist.py`: OK, 0 crossed.
- `check_usb_si.py`: 0 hard failures. RP 19.53 / 20.27 mm, SVC 33.90 / 34.54 mm; every other link has 0.12.0's
  lengths and coupling.

**The skew, and why the S height.** Behind a twist the two lines trade their remaining lengths, so where the twist goes
sets the pair's skew. The service pair's straight runs nearer the switch had no room (every site there is blocked by
Hub-0's pair, `VBUS_PWR`, `VBUS_PC1` or `RP_CC2`), and a plain twist on the diagonal leg gave 1.30 to 1.44 mm, over the
1.2 mm budget. `seed_twist` now chooses the site for the least skew and can draw the crossing line's S taller or
flatter than the hop, which lengthens or shortens that line alone. `ROUTE_SEED_TWIST_DEBUG=1` prints, per straight run,
how many sites fit, their skew and what blocks the rest.

The `check_usb_si.py` caveat from [v13-seeded](../v13-seeded/README.md) applies: its "mm split in N runs" column
depends on the order segments are stored in, so KB, MS, OT and PC2 read differently from 0.12.0 with identical copper.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/v14-seeded
# then: http://localhost:8765/
```

Controls:
- mouse wheel: zoom; drag: pan; **Fit board** resets the view;
- the **net box**, or clicking copper, highlights a net's pads;
- the layer switches toggle F.Cu, B.Cu, In2.Cu and In1.Cu;
- the log panel shows the router's own lines.
