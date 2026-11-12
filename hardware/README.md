# OpenDynKVM hardware

> **This branch holds hardware 0.12.0 as ordered** (2026-10-29): the KiCad files, the BOM/CPL that went to JLCPCB,
> and the scripts in `tools/` that generated them. Its faults are in [errata.md](errata.md); the current design is on
> `main`.

KiCad 10 project for the OpenDynKVM described in [../design.md](../design.md). Every KiCad file here is **generated**:
edit the generator scripts in `tools/`, not the schematic or board in eeschema / pcbnew.

| File | Made by |
|---|---|
| `KVM.kicad_sym` | `tools/gen_symbols.py` (custom symbols: AT32F405CCT7, TS3USB3031, TPS2121, LM66200, TPS2553, SK6812SIDE-A) |
| `CustomFootprints.pretty/` | `tools/gen_footprints.py` (TPS2121 RUX and SK6812SIDE-A from JLC's EasyEDA land patterns, MX switch in a Kailh hot-swap socket); the HRO USB-C footprint is copied from OpenRower |
| `kvm.kicad_sch` | `tools/gen_schematic.py`: the whole netlist, block by block |
| `kvm.kicad_pcb` | `tools/gen_pcb.py` (placement), then `tools/route.py` (routing, planes) |
| `bom/`, `fab/`, `kvm_gerbers.zip` | `tools/make_fab.py` |
| `kvm_schematic.pdf` | `kicad-cli sch export pdf` |

Checks that run on `kvm.kicad_pcb`: `tools/check_spacing.py` (courtyards, pad gaps, board edges),
`tools/check_aux.py` (every auxiliary passive sits on the side of the chip pin it serves),
`tools/check_copper.py` (copper conflicts, independent of the DRC: clearance between nets on tracks and vias,
dead-end stubs, copper that reaches the same point twice (a doubled route), and per differential pair the line-to-line
gap, skew, via count and stubs) and
`tools/check_usb_si.py` (the same pairs judged *as a channel*: per-lane length and skew in mm and ps, how much of
each link actually runs coupled versus as two single lines, split runs against the documented budgets, paired versus
unpaired vias, the layers used, and long parallel runs beside non-USB copper).

Order diagnostics - the evidence behind [jlcpcb.md](jlcpcb.md); each prints what it measured:
`tools/fab_facts.py` (outline, stack-up, rules, vias and vias in pads, holes, parts at the edges, parts per side),
`tools/zsolve.py [--corners]` (2D field solve of the USB pair on the board's stack-up, with fab tolerances and the
7628 build), `tools/dc_drop.py NET SOURCE SINK=AMPS ...` (DC drop and per-segment current of a power net's copper)
and `tools/jlc_stock.py [--boards N]` (JLC stock and part class for every BOM line; needs network).
`tools/xtal_report.py` reviews every crystal the same way: its chip and load caps, distance and routed length to the
chip's pins, vias and layers, foreign copper in its courtyard and foreign tracks running alongside its nets.

Budgets and accepted exceptions (rev 9, 2026-10-26): the intra-pair skew budget is **1.2 mm = 8 ps** at 6.7 ps/mm
(`check_usb_si.py` `SKEW_MAX`, shared by `check_copper.py` `PAIR_SKEW_MAX`).  That is 0.4 % of a 480 Mb/s bit and about
six times tighter than the ~50 ps USB 2.0 high-speed app notes allow; it was 0.75 mm (5 ps) before, which the two
board-spanning pairs cannot meet (SVC 0.88 mm, H2_AT 0.99 mm).  **H0_D4** is a *documented exception*: it is Hub-0's
disabled port (D4 is strapped), and where the port-4 via row pinches its pair to 0.1346 mm the pair is accepted, not
squeezed - `check_copper.py` prints it under "accepted exceptions" and never counts it, so the eleven live pairs keep the
strict 0.15 mm gap.

## Regenerating

Run everything from this folder:

```sh
python3 tools/gen_symbols.py
python3 tools/gen_footprints.py              # needs network only for parts not in tools/easyeda_cache/
python3 tools/gen_schematic.py
kicad-cli sch erc kvm.kicad_sch              # expect 0 violations
kicad-cli sch export netlist --format kicadsexpr -o /tmp/kvm.net kvm.kicad_sch
python3 tools/check_netlist.py /tmp/kvm.net  # the KiCad netlist must equal the generator's pin map
python3 tools/gen_pcb.py --force             # --force discards the routing
python3 tools/check_spacing.py               # courtyards, pad gaps, edges
python3 tools/check_aux.py                   # every auxiliary part is on the side of the pin it serves
python3 tools/check_copper.py                 # shorts, stubs, doubled copper, unconnected nets, pair geometry
                                              # under each HS pair (independent of the DRC; passes the committed board)
python3 tools/check_usb_si.py                 # USB as a channel: per-lane length/skew (mm and ps), coupled vs split
                                              # length, split runs vs the documented budgets, paired vias, aggressors
python3 tools/jlc_align.py                   # after adding a part: JLC rotation/offset corrections
python3 tools/route.py --live                # USB pairs, planes, power, signals (log: tools/route.log);
                                             # --live: watch all layers at http://localhost:8765/
                                             #   the Marks panel lists the router's problem areas (a lane with no
                                             #   way through, where a twist went, where a centre line U-turns);
                                             #   click one to jump to it.  The same spots are in the log.
                                             #   ROUTE_ONLY_PAIRS=H0_MS (comma list, P or N net name) routes that
                                             #   link alone - 25-45 s instead of 20 min, same lanes and vias -
                                             #   and ROUTE_DUMP_PAIR=H0_MS prints its centre runs and both
                                             #   offset lines as they are laid.  Neither changes a full run.
kicad-cli pcb drc --schematic-parity kvm.kicad_pcb
python3 tools/make_fab.py                    # JLC BOM + CPL (corrections applied), gerbers, drill
```

## Conventions

- **References** are numbered per schematic block (block 5 = RP2354B: U501, C501, R501, …). Adding a part only
  renumbers its own block.
- **Placement:** `gen_pcb.py` places the main parts from its `PLACEMENT` table. Every other part is placed next to
  the IC pin it serves (decoupling capacitors on their own power pin), outside the `KEEP_FREE` corridors that the USB
  pairs need.
- **Power rails and crystals (rev 12):** `route.py` sizes each rail for the copper of the layer it runs on, read from
  the board's stack-up (`RAIL_LW`: 2.3 × on the 0.5 oz In2.Cu, capped at `RAIL_W_MAX` = 1.39 mm, the widest track its
  distance fields can check), and +5V for drop too (`RAIL_DROP`).  Crystal lines (`XTAL_NETS`) stay off In2.Cu; every
  other net keeps out of crystal courtyards and `XTAL_GUARD` off crystal lines, except within `XTAL_PIN_ZONE` of the
  crystal's chip.  `tools/dc_drop.py` and `tools/xtal_report.py` measure the result.
- **Stack-up:** JLC04161H-3313, with L1 signals, L2 GND, L3 3V3 and L4 signals. USB HS pairs are 0.15 / 0.15 mm
  (≈ 88 Ω differential over L2; confirm with JLC's impedance calculator before ordering).  The board carries it
  (Board Setup → Physical Stackup, ENIG, impedance controlled; the name in the title block) from `gen_pcb.py`
  `STACKUP`; `python3 tools/gen_pcb.py --stackup` rewrites just that on a routed board.
- **JLC assembly:** both sides, exactly the parts with an LCSC number - on the bottom the per-pin decoupling caps
  under the chips, the MX key's hot-swap socket and the SK6812MINI-E key LED (decided 2026-10-27: no BOM exclusion).
  `make_fab.py` refuses to write a CPL unless every LCSC part has a correction in `tools/jlc_corrections.json`; the
  bottom rows use the mirrored transform documented in `make_fab.py` `jlc_place` - check the polarized bottom parts
  (D1501, SW1501) in JLC's placement preview.  Every quote-form setting: [jlcpcb.md](jlcpcb.md).
- **Hand assembly:** only the MX switch and keycap, plugged into the socket afterwards.  A part with an MPN but no
  LCSC number would go to `bom/hand_assembly.csv`.

`tools/sexpr.py`, `check_netlist.py`, `check_spacing.py`, `gen_pcb.py`, `gen_schematic.py`, `jlc_align.py`,
`make_fab.py` and `route.py` started as copies of the OpenRower project's tools and were adapted here. OpenRower itself
is not modified.
