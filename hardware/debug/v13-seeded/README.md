# Live-view snapshot: 0.13.0 routed from the 0.12.0 board (2026-11-05)

**Superseded by 0.14.0** ([v14-seeded](../v14-seeded/README.md)): 0.13.0 was designed and never ordered. It fixed
errata E1 by swapping U601's USB1 pins too, which left every pair pin of the switch carrying the other line's name.
`board.kicad_pcb` here is the routed 0.13.0 board, kept for reference on the machine that routed it (not in git, like
the other snapshot boards).

A static copy of `tools/build/live/` from the seeded run that made the 0.13.0 board:

```sh
git show hw-v0.12.0:hardware/kvm.kicad_pcb > /tmp/seed.kicad_pcb
python3 tools/gen_pcb.py --force
python3 -u tools/route.py --live --seed=/tmp/seed.kicad_pcb
```

`route.log` is that run's log.

**What changed against 0.12.0.** The fix for [errata E1](../../errata.md) (U601's USB1 pins swapped) and the badge.
Everything else is 0.12.0's copper, item for item: 3991 of its 4061 tracks and vias.

| Change | Copper |
|---|---|
| Hub-0 upstream pair `H0_UP_DP`/`H0_UP_DM` | 0.12.0's two lines kept, with a 1.6 mm twist spliced into the straight run at (152.96, 119.55): its vias sit 0.45 mm out at (153.25, 120.00) and (154.25, 119.10), and the pair grows from 21.95 to 22.55 mm with 0.00 mm skew |
| `HUB0_VBUS`, `PWR_ST` | cut within 1.5 mm of the twist to make room, then rejoined by the signal router |
| `GND` | the 35 stitching vias under the new badge's no-via area are gone |
| Zones | the same four, refilled |

Checks on the result:
- `kicad-cli pcb drc --schematic-parity`: 0 violations, 0 unconnected pads, 0 footprint errors.
- `check_copper.py`: PASS.
- `check_spacing.py`: no spacing problems.
- `check_aux.py`: passes.
- `check_netlist.py`: OK, 0 crossed.
- `check_usb_si.py`: 0 hard failures.

**A caveat on `check_usb_si.py`.** Its "mm split in N runs" column adds up consecutive segments in the order they are
stored in the file, so it changes when only the save order changes. For example, KB reads 5.3 mm on 0.12.0 and 2.8 mm
here, with identical copper. Compare the order-independent "% coupled" and "% split" columns instead; they are the same
on every link except H0_UP.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/v13-seeded
# then: http://localhost:8765/
```

Controls:
- mouse wheel: zoom; drag: pan; **Fit board** resets the view;
- the **net box**, or clicking copper, highlights a net's pads;
- the layer switches toggle F.Cu, B.Cu, In2.Cu and In1.Cu;
- the log panel shows the router's own lines.
