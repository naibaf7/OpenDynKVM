# OpenDynKVM hardware

KiCad 10 project for the OpenDynKVM described in [../design.md](../design.md). Every KiCad file here is **generated**:
edit the generator scripts in `tools/`, not the schematic or board in eeschema / pcbnew.

| File | Made by |
|---|---|
| `KVM.kicad_sym` | `tools/gen_symbols.py` (custom symbols: AT32F405CCT7, TS3USB3031, TPS2121, LM66200, TPS2553, SK6812SIDE-A) |
| `CustomFootprints.pretty/` | `tools/gen_footprints.py` (TPS2121 RUX and SK6812SIDE-A from JLC's EasyEDA land patterns, MX switch in a Kailh hot-swap socket, the OpenDynKVM logo as silkscreen from `../branding/`); the HRO USB-C footprint is copied from OpenRower |
| `kvm.kicad_sch` | `tools/gen_schematic.py`: the whole netlist, block by block |
| `kvm.kicad_pcb` | `tools/gen_pcb.py` (placement), then `tools/route.py` (routing, planes) |
| `bom/`, `fab/`, `kvm_gerbers.zip` | `tools/make_fab.py` |
| `kvm_schematic.pdf` | `kicad-cli sch export pdf` |
| `tools/models_3d.json`, `3dmodels/` | `tools/fetch_3d.py`: 3D models for the parts KiCad's 3D library has none for (EasyEDA's, the ones JLC's preview shows) and the MX switch and keycap; the JSON says where each goes on its footprint (`gen_pcb.py` puts them there), the STEP files are a local cache, not in git |
| `render/` | `tools/render_3d.py`: the board raytraced by KiCad, top, bottom and at an angle from the front and the rear; local only, but for `kvm-front-readme.png`, the README's picture (256 colours, at most 256 KB) |
| `case/` | The 3D-printable case: `case/board_facts.py` (KiCad's Python) writes `case/board.json`, `case/gen_case.py` (its own environment: build123d) the STEP and 3MF files ([case/README.md](case/README.md)) |

Checks that run on `kvm.kicad_pcb`: `tools/check_spacing.py` (courtyards, pad gaps, board edges),
`tools/check_aux.py` (every auxiliary passive sits on the side of the chip pin it serves),
`tools/check_copper.py` (copper conflicts, independent of the DRC: clearance between nets on tracks and vias,
dead-end stubs, copper that reaches the same point twice (a doubled route), and per differential pair the line-to-line
gap, skew, via count and stubs) and
`tools/check_usb_si.py` (the same pairs judged *as a channel*: per-lane length and skew in mm and ps, how much of
each link actually runs coupled versus as two single lines, split runs against the documented budgets, paired versus
unpaired vias, the layers used, and long parallel runs beside non-USB copper).

Faults found after ordering, with their workarounds and the fix for the next board: [errata.md](errata.md).

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

Run everything from this folder, with uv ([DEVELOPMENT.md](../DEVELOPMENT.md), "Python scripts", sets it up once):

```sh
uv run tools/gen_symbols.py
uv run tools/gen_footprints.py               # needs network for parts not in tools/easyeda_cache/ (a local cache,
                                             # not in git: a fresh clone fetches them once)
uv run tools/gen_schematic.py
kicad-cli sch erc kvm.kicad_sch              # expect 0 violations
kicad-cli sch export netlist --format kicadsexpr -o /tmp/kvm.net kvm.kicad_sch
uv run tools/check_netlist.py /tmp/kvm.net   # the KiCad netlist must equal the generator's pin map, and every USB
                                             # pair keeps D+ on D+ end to end, through the switches (errata.md E1)
uv run tools/gen_pcb.py --force              # --force discards the routing
uv run tools/check_spacing.py                # courtyards, pad gaps, edges
uv run tools/check_aux.py                    # every auxiliary part is on the side of the pin it serves
uv run tools/check_copper.py                  # shorts, stubs, doubled copper, unconnected nets, pair geometry
                                              # under each HS pair (independent of the DRC; passes the committed board)
uv run tools/check_usb_si.py                  # USB as a channel: per-lane length/skew (mm and ps), coupled vs split
                                              # length, split runs vs the documented budgets, paired vias, aggressors
uv run tools/jlc_align.py                    # after adding a part: JLC rotation/offset corrections
uv run tools/route.py --live                 # USB pairs, planes, power, signals (log: tools/route.log);
                                             # --live: watch all layers at http://localhost:8765/
                                             #   the Marks panel lists the router's problem areas (a lane with no
                                             #   way through, where a twist went, where a centre line U-turns);
                                             #   click one to jump to it.  The same spots are in the log.
                                             #   ROUTE_ONLY_PAIRS=H0_MS_DP (comma list of P or N net names)
                                             #   routes that link alone - with --pairs-only about 1.5 min in all
                                             #   instead of 20, same lanes and vias - and ROUTE_DUMP_PAIR=H0_MS_DP
                                             #   (or all) prints its centre runs and both offset lines as they
                                             #   are laid.  Neither changes a full run.
kicad-cli pcb drc --schematic-parity kvm.kicad_pcb
uv run tools/dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5
                                             # the +5 V feed (jlcpcb.md §5): a full run can pass every check above
                                             # with a bad one (0.15.0's three runs: 495, 121 and 96 mV)
uv run tools/make_fab.py                     # JLC BOM + CPL (corrections applied), gerbers, drill; two assembly
                                             # variants: jlcpcb_*.csv with the Bluetooth module, jlcpcb_*_nobt.csv
                                             # without its parts (Fit = BT, design.md §16)
uv run case/board_facts.py                   # the case: what it needs from the board, into case/board.json
uv run case/gen_case.py                      # then the case itself; it checks the board's parts against it first
uv run tools/fetch_3d.py                     # after adding a part (and jlc_align.py): 3D models KiCad lacks, into
                                             # 3dmodels/ and tools/models_3d.json (network for new ones)
uv run tools/gen_pcb.py --models             # put them on the board; touches nothing else, routed or not
uv run tools/render_3d.py                    # render/kvm-{top,bottom,front,rear}.png (--quick: basic quality)
```

**3D models and renders.** Every part has a 3D model: KiCad's 3D library (on Fedora the separate package
`kicad-packages3d`) for most, and for the eight LCSC parts it has none for (KiCad 10.0.8 ships no file for the
RP2354B's QFN-80, the TS3USB3031's WQFN-12, the HRO USB-C receptacle or the TS-1187A button, and nothing for our own
footprints) the part's EasyEDA model, placed on the KiCad footprint by EasyEDA's own placement data and the JLC
correction (`tools/fetch_3d.py` documents how, and what it was checked against).  The MX key carries its hot-swap
socket, an MX switch and a keycap drawn to the case's numbers.  After a fresh clone, `uv run tools/fetch_3d.py` fills
`3dmodels/`; then KiCad's 3D viewer (Alt+3) shows the whole board, and `tools/render_3d.py` refuses to render while a
model is missing.

`uv run tools/gen_footprints.py` regenerates the custom footprints; the OpenDynKVM logo needs `shapely` (in the
environment; without it the logo footprint is left as it is).

**Revising an ordered board (seeded routing).** A full run re-routes everything, and the router is not deterministic,
so one changed pin moves copper all over the board. To change a board that was ordered and verified, seed the run
with that board instead:

```sh
git show hw-vX.Y.Z:hardware/kvm.kicad_pcb > /tmp/seed.kicad_pcb   # the ordered board, from its tag
uv run tools/gen_pcb.py --force                                     # the placement must be the seed's
uv run python -u tools/route.py --live --seed=/tmp/seed.kicad_pcb
```

- **What it keeps:** the seed's tracks and vias stay, as fixed copper, for every net whose pads are the same as in the
  seed.
- **What it routes:** only the changed nets and their pair mates.
- **A pair whose two lines only trade places at one part** (errata.md E1) keeps the seed's lines and gets a 1.6 mm
  twist spliced into one of their straight runs, never beyond another part on the line. The site is chosen for the
  least skew, and the crossing line's S can be drawn taller or flatter than the via hop to trim it (0.14.0: the service
  pair 0.64 mm, from a plain twist's 1.30 mm). If no site has room, simple signals within 1.5 mm are cut and re-routed.
  `ROUTE_SEED_TWIST_DEBUG=1` lists each run's sites, their skew and what blocks them.
- **What it drops:** seed copper that this board's rule areas forbid, such as a via under the badge, and seed copper
  that touches a pad of a part the seed does not have; a signal that loses copper that way is re-routed.  Plane nets
  keep their copper, and their new pads get a via of their own.
- **More nets:** `--reroute=NET,NET` re-routes nets whose pads did not change, in the order given and before the
  changed nets, to clear a new net's way out of a dense pin row.  A seeded net that is not listed cannot be ripped up.
- **Moved parts:** `--move=REF,REF` lets the parts named sit elsewhere than in the seed; each is taken as a new part.
  Its own signal nets keep their seed copper except where it touches the part's old or new pads, and the router joins
  what is left to the pads where they are now, then trims the dead ends; a plane net loses its tracks to the old pads
  and the +3V3 vias they led to.  0.17.0's Bluetooth module went from the bottom to the top this way (2026-11-12, 18 s,
  six nets re-joined; dropping the module's lines entirely and routing them anew left one of the two UART lines
  unrouted, as did a full run).
- **What it refuses:** to run if any other footprint with pads moved (logos may move), and to write the board if a net
  stays unrouted.
- **Where it stops:** 0.16.0 could not be seeded from 0.15.0.  The module's UART pins (RP2354B pins 36/37) sit
  between the SWD and crystal lines and Hub-0's mouse pair, which 0.15.0 had packed for two unused pins, and every
  `--reroute` set tried left two to four nets unrouted.  0.16.0 is routed from scratch.
- **Afterwards:** the same checks as after a full run.

## Conventions

- **References** are numbered per schematic block (block 5 = RP2354B: U501, C501, R501, …). Adding a part only
  renumbers its own block.
- **Placement:** `gen_pcb.py` places the main parts from its `PLACEMENT` table. Every other part is placed next to
  the IC pin it serves (decoupling capacitors on their own power pin), outside the `KEEP_FREE` corridors that the USB
  pairs need.
- **USB crossovers (0.15.0):** a link whose two ends have D+ on opposite sides takes `twist_at='run'` in
  `route.py` `pair_links`: it is routed as if no crossover were needed, and a 1.6 mm twist (one line crosses over, the
  other hops under it between two vias) goes into a straight stretch of its own new lines, chosen for the least skew;
  the crossing line's S can be drawn taller or flatter than the hop to trim it.  The RP pair and the service pair cross
  over this way (errata.md E1).  `ROUTE_TWIST_DEBUG=<P net>` prints the chosen site and the pair's copper.
- **Thermal spokes:** other nets keep 0.7 mm from every GND through-hole pad on the outer layers (`THT_SPOKE_ROOM`),
  so the pours reach the connectors' shield and ground pins with all their spokes (KiCad wants two).
- **Power rails and crystals (rev 12):** `route.py` sizes each rail for the copper of the layer it runs on, read from
  the board's stack-up (`RAIL_LW`: 2.3 × on the 0.5 oz In2.Cu, capped at `RAIL_W_MAX` = 1.39 mm, the widest track its
  distance fields can check), and +5V for drop too (`RAIL_DROP`).  Crystal lines (`XTAL_NETS`) stay off In2.Cu; every
  other net keeps out of crystal courtyards and `XTAL_GUARD` off crystal lines, except within `XTAL_PIN_ZONE` of the
  crystal's chip.  `tools/dc_drop.py` and `tools/xtal_report.py` measure the result.
- **Stack-up:** JLC04161H-3313, with L1 signals, L2 GND, L3 3V3 and L4 signals. USB HS pairs are 0.15 / 0.15 mm
  (≈ 88 Ω differential over L2; confirm with JLC's impedance calculator before ordering).  The board carries it
  (Board Setup → Physical Stackup, ENIG, impedance controlled; the name in the title block) from `gen_pcb.py`
  `STACKUP`; `uv run tools/gen_pcb.py --stackup` rewrites just that on a routed board.
- **JLC assembly:** both sides, exactly the parts with an LCSC number - on the bottom the per-pin decoupling caps
  under the chips, the MX key's hot-swap socket and the SK6812MINI-E key LED (decided 2026-10-27: no BOM exclusion).
  `make_fab.py` refuses to write a CPL unless every LCSC part has a correction in `tools/jlc_corrections.json`; the
  bottom rows use the mirrored transform documented in `make_fab.py` `jlc_place` - check the polarized bottom parts
  (D1501, SW1501) in JLC's placement preview.  Every quote-form setting: [jlcpcb.md](jlcpcb.md).
- **Version and badge:** `gen_pcb.py` `HW_VERSION` is the board's version (DEVELOPMENT.md "Versions"): bump it for every board
  that is ordered, and `kvm_proto::HARDWARE_VERSION` with it.  `place_badge` prints the OpenDynKVM logo (14 mm) behind
  the OTHER port, and KiCad's open source hardware logo with the version at the rear, between PC1 and BOOTSEL (0.16.0:
  the Bluetooth module took the badge's old corner, bottom right): board-only footprints LOGO1 / LOGO2, outside the BOM
  and CPL.  Each carries a rule area that keeps vias out, so no drill cuts the print; `route.py` honours it.  The logo's thinnest strokes ("KVM") are thickened to JLC's
  0.15 mm silkscreen minimum (`gen_footprints.py` `LOGO_GAIN`).  The badge is on the board since 0.13.0; rev 12 as
  ordered carries the version only in the top-left label, edited as text.
- **Hand assembly:** only the MX switch and keycap, plugged into the socket afterwards.  A part with an MPN but no
  LCSC number would go to `bom/hand_assembly.csv`.

`tools/sexpr.py`, `check_netlist.py`, `check_spacing.py`, `gen_pcb.py`, `gen_schematic.py`, `jlc_align.py`,
`make_fab.py` and `route.py` started as copies of the OpenRower project's tools and were adapted here. OpenRower itself
is not modified.

## License

The board is open hardware under the [CERN-OHL-S v2](../LICENSES/CERN-OHL-S-2.0.txt): the KiCad files, the generators
that hold the design (`gen_symbols.py`, `gen_footprints.py`, `gen_schematic.py`, `gen_pcb.py`, `jlc_corrections.json`),
the BOM, CPL and schematic PDF, and the design notes. The general-purpose tools in `tools/` (router, live viewer,
checks, fabrication, solvers) are [MIT](../LICENSES/MIT.txt). The HRO USB-C footprint and the open source hardware
logo come from the KiCad library: CC-BY-SA-4.0 with the KiCad libraries exception, which leaves the board's own license
untouched. File by file:
[../REUSE.toml](../REUSE.toml).

```
Copyright 2026 Fabian Tschopp.

This source describes Open Hardware and is licensed under the CERN-OHL-S v2.

You may redistribute and modify this source and make products using it under the terms of the CERN-OHL-S v2
(https://ohwr.org/cern_ohl_s_v2.txt).

This source is distributed WITHOUT ANY EXPRESS OR IMPLIED WARRANTY, INCLUDING OF MERCHANTABILITY, SATISFACTORY QUALITY
AND FITNESS FOR A PARTICULAR PURPOSE. Please see the CERN-OHL-S v2 for applicable conditions.

Source location: https://github.com/naibaf7/OpenDynKVM
```
