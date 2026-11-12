# Ordering OpenDynKVM at JLCPCB

What to select in JLCPCB's quote form for the 0.16.0 board, and why. Written for rev 12 (0.12.0) on 2026-10-27/28
against the quote form as it looked then (`datasheets/Online PCB Instant Quote - JLCPCB.pdf`); the board's measured
numbers were updated for 0.16.0 on 2026-11-09, and stock and prices change daily.  Every measured
number here comes from a tool in `tools/` - section 13 lists which - so after a board change re-run the tool rather
than trusting the text.

**Short version:** 4 layers, 1.6 mm, **stack-up JLC04161H-3313** (without the paid impedance control), standard
1 oz / 0.5 oz copper, ENIG, **epoxy filled & capped vias**, Standard PCBA on **both sides**, edge rails by JLCPCB,
**confirm parts placement**.  The quote's defaults for stack-up, via covering and placement confirmation are wrong for
this board.  The order is blocked until the TS3USB3031 is in stock (§8).  Two assembly variants: with the Bluetooth
module (`jlcpcb_bom.csv` / `jlcpcb_cpl.csv`) or without it (`*_nobt.csv`, the board works the same minus Bluetooth).  Standard copper is enough: since rev 12 the
power rails are sized for the 0.5 oz inner layer they partly run on (§5).

## 1. Files

Generate them from `hardware/` on the final board (the zip and `fab/` are not in git):

```sh
uv run tools/make_fab.py
```

| Upload | File | Content |
|---|---|---|
| PCB | `kvm_gerbers.zip` | 4 copper, 2 mask, 2 paste, 2 silkscreen, outline, plated + non-plated drill, job file (with the stack-up) |
| BOM | `bom/jlcpcb_bom.csv` | 41 lines, every part with an LCSC number (with the Bluetooth module) |
| CPL | `bom/jlcpcb_cpl.csv` | 251 parts: 209 top, 42 bottom, JLC rotation/offset corrections applied |
| BOM, no Bluetooth | `bom/jlcpcb_bom_nobt.csv` | 40 lines: the module U1601 and its seven passives (Fit = BT) left out |
| CPL, no Bluetooth | `bom/jlcpcb_cpl_nobt.csv` | 243 parts: 209 top, 34 bottom |

Upload one BOM/CPL pair.  Without the module the board works as 0.15.0 does: the firmware finds no module at boot and
offers no Bluetooth (design.md §16); the module's pads and its antenna keep-out stay bare.

## 2. What JLCPCB will see

From `uv run tools/fab_facts.py`:

| Item | Value |
|---|---|
| Outline | 120 x 80 mm; one internal cut-out, the key LED D1501's, 3.63 x 3.23 mm overall |
| Copper | L1 signals + GND pour, L2 GND plane, L3 +3V3 plane (short power-rail runs cross it, sized for its 0.5 oz, §5), L4 signals + GND pour |
| Tracks | 3076 segments, 0.15-1.39 mm wide |
| Rules | clearance >= 0.15 mm, copper to board edge >= 0.5 mm, except D1501's pads: 0.2 mm from their own cut-out (custom rule in `kvm.kicad_dru`) |
| Vias | 1010, all through, 0.6 mm pad / 0.3 mm drill, tented both sides in the gerbers; 36 of them sit in the exposed pads of U501, U701, U801 and U901 (§6) |
| Plated holes | 12 x 0.95 mm and 6 x 2.3 mm (USB-A); 6 slots 0.6 x 1.2 mm and 6 slots 0.6 x 1.7 mm (USB-C shells) |
| Non-plated holes | 24: 4 x 3.2 mm (M3), the MX switch and socket (4.0, 2 x 1.75, 2 x 3.05 mm), 6 x 0.65 mm (USB-C pegs), 9 x 0.99 mm (guide holes of the three Tag-Connect SWD pads) |
| Edges | the USB-C bodies reach 0.05 mm past the north edge, the USB-A bodies end 0.16 mm inside the south edge, the Bluetooth module's antenna end (bottom side) 0.45 mm inside the east edge; no other part within 0.5 mm |
| Through-hole parts | 3 x USB-C (4 shell legs each), 3 x USB-A (6 pins each): JLC solders these by hand, for a separate fee |

## 3. PCB options

| Option | Select | Why |
|---|---|---|
| Base material / layers | FR-4, 4 | |
| Dimensions | as detected (120 x 80) | shows 120 x 90 once PCBA adds the edge rails |
| PCB qty | 5 | the minimum |
| Product type | Industrial/Consumer electronics | |
| Different design / delivery format | 1 / Single PCB | JLC adds the rails for assembly (§7) |
| PCB thickness | **1.6 mm** | the stack-up, the USB-A legs and the hot-swap socket assume it |
| Colour / silkscreen | any / white | green was quoted; colour has no technical effect here |
| Material type | FR4 TG135, or TG155 | TG135 is fine; TG155 adds margin for two lead-free reflow passes (the quote shows any surcharge).  Any FR-4 keeps the pairs in spec (§4) |
| Surface finish | **ENIG**, 1 U" | flat pads for the 0.4 mm pitch QFN-80 (RP2354B) and WQFN-12 (TS3USB3031); HASL is uneven at that pitch.  2 U" buys nothing here |
| Outer / inner copper | **1 oz / 0.5 oz** (standard) | what JLC04161H-3313 is built with; the rails are sized for each layer's copper (§5) |
| Specify layer sequence | **Yes** | L1 `kvm-F_Cu.gtl`, L2 `kvm-In1_Cu.g1` (GND), L3 `kvm-In2_Cu.g2` (+3V3), L4 `kvm-B_Cu.gbl` |
| Specify stackup | **Yes: JLC04161H-3313** | the USB impedance depends on it (§4) |
| Impedance control (+$30) | No | the stack-up keeps the pairs in spec without it; it would only narrow the spread (§4) |
| Via covering | **Epoxy Filled & Capped** | §6; Tented is the cheaper fallback |
| Via plating method | Not specified | |
| Min via hole size / diameter | 0.3 mm / (0.4/0.45 mm) | the cheapest tier; every via is 0.3 mm drill / 0.6 mm |
| Board outline tolerance | +-0.2 mm (regular) | |
| Confirm production file | Yes (optional) | lets you see JLC's edited files before they build; worth it with a chosen stack-up and filled vias |
| Mark on PCB | Remove mark | no order number on the silkscreen; no extra cost shown in the quote |
| Electrical test | Flying probe fully test | |
| Gold fingers, castellated holes, press-fit, edge plating, blind slots, UL marking, backdrill, humidity card | No | |

**PCB remark** (paste as is; it names the stack-up without asking for the impedance-control service):

> Stack-up JLC04161H-3313.  USB differential pairs are 0.15 mm wide with a 0.15 mm gap on L1 (reference L2) and L4
> (reference L3), designed for 90 ohm on this stack-up.  The vias in the exposed pads of U501, U701, U801 and U901 are
> intentional (epoxy filled & capped).  D1501's pads are 0.2 mm from its internal cut-out by design.

## 4. Stack-up and impedance

JLC04161H-3313, 1.6 mm.  The board file carries it (KiCad Board Setup -> Physical Stackup, from `gen_pcb.py`
`STACKUP`, and the name in the title block), and so does `kvm-job.gbrjob` in the gerber zip.  JLC still takes the
stack-up from the quote form, so select it there as well.

| Layer | Thickness | |
|---|---|---|
| L1 copper | 0.035 mm | F.Cu, signals |
| 3313 prepreg | 0.0994 mm | Er 4.1 |
| L2 copper | 0.0152 mm | In1.Cu, GND |
| Core | 1.265 mm | |
| L3 copper | 0.0152 mm | In2.Cu, +3V3 |
| 3313 prepreg | 0.0994 mm | Er 4.1 |
| L4 copper | 0.035 mm | B.Cu, signals |

The USB High-Speed pairs are 0.15 mm wide with a 0.15 mm gap, on L1 over L2 and on L4 over L3.  USB 2.0 needs
76.5-103.5 ohm differential (90 +-15 %) and 21-39 ohm common mode (30 +-30 %).  From
`uv run tools/zsolve.py --corners`, a 2D field solve on the stack-up read from the board:

| Case | Differential | |
|---|---|---|
| **Nominal** | **89.5 ohm** (common mode 28.7) | in spec |
| Etched width -/+10 % (pitch fixed, so the gap moves the other way) | 95.3 / 84.1 | in spec |
| Etched width -/+20 % | 101.5 / 79.0 | in spec |
| Prepreg -/+10 % | 85.7 / 92.9 | in spec |
| Er 3.9 / 4.3 | 91.2 / 87.9 | in spec |
| Mask 0 / 20 um | 94.0 / 87.7 | in spec |
| Plated copper 45 um | 87.1 | in spec |
| Trapezoidal etch (top 25 um narrower) | 92.9 | in spec |
| All of them combined, root-sum-square, width +-10 % / +-20 % | 82.1-98.4 / 77.8-103.3 | in spec |
| Every tolerance the same way at once, width +-10 % / +-20 % | 105.6 / 75.8, 112.6 / 71.1 | out of spec |
| **The same pair on JLC04161H-7628** (0.2104 mm prepreg, Er 4.4) | **107.1** (common mode 44.1) | out of spec |

The tolerance ranges are typical fabrication values (`zsolve.py` `TOL`), not JLC guarantees.  What the table decides:

- **Specify the stack-up.**  On the thicker 7628 build, which is what a 4-layer order without a chosen stack-up can
  get, every board is out of spec in both modes.
- **Skip the paid impedance control (+$30).**  Every tolerance on its own and their realistic combination stay inside
  USB's window.  Only the case where every tolerance goes the same way at once leaves it.  The service adjusts the
  width for JLC's etching and measures test coupons, typically to +-10 % (about 81-99 ohm here).  Worth it for a
  production run, not for this prototype; the High-Speed passthrough test on the first boards is the real check.
- Before ordering, run JLC's impedance calculator (link next to the option): JLC04161H-3313, outer layer,
  differential, 0.15 / 0.15 mm.  Expect about 90 ohm.

## 5. Copper weight and the +5 V feed

Standard 1 oz outer / 0.5 oz inner is what JLC04161H-3313 is built with, and since rev 12 the router sizes every
power rail for the copper of the layer it runs on (`route.py` `RAIL_LW`, from the board's stack-up): a rail on the
0.5 oz In2.Cu is 2.3 x its outer width, capped at 1.39 mm.  The +5 V feed from the TPS2121 (U402) to the three port
switches also has a drop rule (1 mV/mm at its 2.24 A): 1.1 mm on F.Cu / B.Cu.  Rev 11 ran it 106 mm on In2.Cu at the
0.6 mm meant for 1 oz outer copper.

From `uv run tools/dc_drop.py +5V U402.1,U402.8 U1203=... U1303=... U1403=... U403=...` (U1203 / U1303 / U1403 are
the Keyboard / Mouse / Other port switches, U403 the 3.3 V buck):

| Load case | Drop U402 -> port switches, 0.17.0 | 0.16.0 | 0.15.0 | rev 12 | rev 11 |
|---|---|---|---|---|---|
| USB-rated: 3 ports x 0.5 A + buck 0.5 A (2.0 A) | **85 / 74 / 54 mV** | 84 / 74 / 49 mV | 96 / 87 / 66 mV | 91 / 82 / 59 mV | 206 / 185 / 148 mV |
| typical: 0.1 / 0.1 / 0.5 A + buck 0.4 A (1.1 A) | 32 / 30 / 27 mV | 30 / 28 / 23 mV | 38 / 36 / 32 mV | 34 / 32 / 28 mV | 82 / 78 / 70 mV |

On 0.16.0 the +5 V track runs 174 mm on F.Cu, 95.5 mm on B.Cu and 13.6 mm on In2.Cu (12 % of the loss; 0.15.0:
35.5 mm on In2.Cu, 59 %; rev 12: 24 mm, 31 %; rev 11: 106 mm, 84 %).  Its most loaded inner segment, 2.6 mm at
(170.25, 151.75-154.35) on the way to the port switches, carries 1.5 A at 71 A/mm2: ~8 K by IPC-2221's external chart,
at most 38 K by its internal one, which ignores the planes either side (0.15.0: 2.0 A, 95 A/mm2, ~15 K; rev 11: 2.0 A,
219 A/mm2, ~61 K).  The hottest copper on rev 12, 0.15.0 and 0.16.0 alike is 0.5 mm outer track at 2.0 A, 114 A/mm2
(~21 K): the whole 52 mm `VBUS_PWR` feed from J301 and +5 V's 0.5 mm neck out of U402.

**A full run can pass every board check with a bad feed.**  The router has no drop budget, and three full runs of
0.15.0 from the same sources gave 495, 121 and 96 mV at the USB-rated load (the first ran +5 V 196 mm on B.Cu, the
second 96.5 mm on In2.Cu).  0.16.0's trial runs gave 212 mV once (+5 V 158 mm on B.Cu).  Run the drop after every full
run (§12).

**At the ports**, from a 5.00 V adapter at the USB-rated load, with typical on-resistance at 25 C:

| Step | Drop |
|---|---|
| VBUS_PWR copper, USB-C -> TPS2121 (`dc_drop.py VBUS_PWR J301 U402.7=2.0`) | 48 mV |
| TPS2121, 2.0 A x 56 mOhm (tps2121.pdf, RON) | 112 mV |
| **+5 V copper, TPS2121 -> port switch** | **49-84 mV** (0.15.0: 66-96 mV; rev 12: 59-91 mV; rev 11: 148-206 mV) |
| TPS2553, 0.5 A x 85 mOhm (tps2553.pdf, rDS(on), DBV) | 43 mV |
| VBUS copper, port switch -> USB-A (`dc_drop.py VBUS_KB U1203.6 J1201=0.5`) | 8-9 mV |
| **Port voltage** | **4.71-4.74 V** (0.15.0: 4.69-4.72 V; rev 12: 4.70-4.73 V; rev 11: 4.58-4.64 V; USB asks for >= 4.75 V, devices work down to about 4.4 V) |

At the full rated load the ports reach 4.75 V from a 5.05 V adapter; at the typical load they sit at 4.84-4.87 V from
5.00 V.  Measure the port voltage under load on the first boards.

**1 oz inner copper is not needed.**  JLC's 3313 build with it has 0.092 mm prepreg, 0.03 mm inner copper and a
1.23 mm core (the quote form, 2026-10-28); on rev 11's routing it would have cut the worst drop to 134 mV, and its
thinner prepreg lowers the USB pairs to 87.0 ohm nominal (79.4-95.3 ohm root-sum-square with +-10 % etch, 75.4-100.3
ohm with +-20 %; `zsolve.py --corners --prepreg 0.092`).

## 6. Vias in the exposed pads

The RP2354B (U501) and the three USB2514B hubs (U701, U801, U901) each have 9 vias (3 x 3, 0.3 mm) in their exposed
pad, and each via sits inside one of the footprint's 9 paste openings; there is no room between the openings
(`fab_facts.py`: "via-in-pad  36 vias ... paste on the pad").

| Via covering | Effect |
|---|---|
| **Epoxy Filled & Capped** (recommended) | the vias are filled and plated over, so the exposed pads are flat and keep their paste.  Applies to all 1014 vias; extra cost on 4 layers (the quote shows it) |
| Tented | the gerbers' default.  Paste in the open pad flows into the vias during reflow: less solder under the four chips, possible voids |
| Plugged / Untented | meant for vias outside pads; no advantage here |

The hubs' decoupling caps on the bottom side have their GND stubs to these vias, but no via sits in a bottom-side pad.

## 7. Assembly options (PCBA)

| Option | Select | Why |
|---|---|---|
| PCBA type | **Standard** | double-sided assembly (Economic has been one side only) |
| Assembly side | **Both sides** | 42 parts on the bottom, 34 without the Bluetooth module (§9) |
| PCBA qty | 5, or 2 | 2 is the minimum; assembling 2 and keeping 3 bare boards cuts the parts cost of a first prototype |
| Edge rails / fiducials | **Added by JLCPCB** | the board has no fiducials, and the connectors sit flush with both long edges (§2).  JLC adds two 5 mm rails (120 x 90 mm) |
| Confirm parts placement | **Yes** | a JLC engineer checks the placement before production; the bottom rows use a derived rotation convention (§9) |
| Stencil storage / fixture storage | No | |
| Parts selection | By customer (self-service) | the BOM carries LCSC numbers |
| Bake components, board cleaning, conformal coating, nitrogen reflow, special stencil | No | |
| Packaging | Antistatic bubble film | |
| Depanel boards & edge rail | your choice | without it you snap the rails off.  Take care next to the flush USB-C and USB-A bodies |
| Flying probe test / function test (PCBA) | No | the bare board is already flying-probe tested |
| Photo confirmation | optional | |
| Solder paste | High temp. | lead-free, both sides |
| Assembly remark | Yes, text below | |

**Assembly remark** (paste as is):

> Bottom side: D1501 (SK6812MINI-E, reverse mount: lens side down into the board cut-out, facing the top side; seen
> from the bottom, pin 1 (GND) is at the lower left) and SW1501 (MX hot-swap socket, rotation 180 in the CPL: its
> curved side wraps round the switch's 4 mm centre hole, which must stay unobstructed).  Please confirm both in the
> placement check.

## 8. Parts and stock

From `uv run tools/jlc_stock.py` on 2026-11-09, for 5 assembled boards with the Bluetooth module:

| Part | LCSC | Need | Stock | |
|---|---|---|---|---|
| **TS3USB3031** (U601, U1202, U1302, U1402) | C2652459 | 20 | **0** | **blocks the order** (82 on 2026-10-28) |
| AT32F405CCT7 | C47115471 | 10 | 114 | thin (below 20 x the need) |
| RP2354B | C39843328 | 5 | 632 | in stock again (0 on 2026-10-28) |
| USB-A receptacle | C2798029 | 15 | 3848 | |
| TLV73311 | C2865431 | 5 | 3924 | |
| ESP32-C3-MINI-1-H4X (U1601, Bluetooth variant only) | C41349510 | 5 | 4772 | |
| 12 MHz crystal | C20625731 | 15 | 9411 | |
| the other 34 lines | | | 7800+ | |

24 of the 41 lines are Extended parts (23 of 40 without the module); JLC charges a loading fee per Extended line
(shown in the quote).  Stock is not reserved until the order is paid, so re-run the tool right before ordering, or buy
the thin lines into your JLC parts inventory together with the blocking one.

Ways round the TS3USB3031:

- **JLC global sourcing / parts pre-order:** buy 20 into your JLC parts inventory, wait for them to arrive, then order
  the PCBA.  The usual path.
- **Consign:** buy the chips elsewhere and ship them to JLC.
- **Wait** for LCSC stock.
- **Fit them by hand:** order with the four marked "do not place".  A 0.4 mm pitch WQFN-12 (2.25 x 2 mm) with no exposed
  pad: hot air or a hotplate, and four per board.

The RP2354B blocked the order on 2026-10-28 (0 in stock).  If it runs out again: the same four ways (ask for stepping
A4 if you have the choice; A2 works on this board too, analysis.md finding 14), or fit U501 by hand, a 0.4 mm pitch
QFN-80 with an exposed pad (easier with filled vias, §6).  The RP2350B (C42415655) is not a drop-in: it has no
in-package flash and would need a QSPI flash chip and a re-layout around the MCU.

## 9. The bottom side

42 parts with the Bluetooth module, 34 without (`fab_facts.py`, "assembly"): 32 x 100 nF 0402 under their supply pins
(RP2354B C501-C506, C513, C515; AT32s C1001-C1004, C1101-C1104; hubs C702-C705, C801-C806, C901-C906), the key LED D1501
and the MX hot-swap socket SW1501; with the module also U1601 (ESP32-C3-MINI-1-H4X) and its passives C1601 (10 uF),
C1602 (100 nF), C1603 (1 uF) and R1601-R1604 (10 k).

`make_fab.py` writes the bottom rows with the convention of kicad-jlcpcb-tools: the rotation of a flipped footprint
is `180 - phi + theta`, and the part offset is mirrored in x.  In JLC's placement preview, check:

- **D1501** sits in its cut-out with its pin 1 on the footprint's pin 1, lens towards the top side.  The datasheet's
  side view has the leads mid-body (0.84 mm lens cup on one side, 0.94 mm on the other), and our pads match the pins
  only with the lens side in the cut-out.  JLC's bottom preview is seen from below, so it shows the LED's back: pin 1
  (GND, the chamfered pad) at the lower left, the datasheet's "bottom view".  Lens-down, GND would be at the lower
  right.  The lens circle in the preview is part of the land-pattern drawing, not the direction it faces.
- **SW1501**: the socket's curved side must wrap round the switch's 4 mm centre hole, the straight side away from it,
  and the centre hole stays unobstructed.  Its pads are point-symmetric, so matching pads cannot see a 180 degree
  error: JLC's first preview (2026-10-28) had it turned, the straight side 0.86 mm from the switch centre and over the
  hole.  `jlc_align.py` `BODY_ROT` now turns it (CPL rotation 180, same position: the turn is about the pads' centre
  of symmetry, so barrels and tabs stay where they were).  JLC's land pattern for the socket is 0.51 mm wider than
  ours, so its tabs may look 0.26 mm off our pads on each side while the part is centred.  That is expected: do not
  let it be "corrected" off-centre.
- **U1601** (Bluetooth variant): the module's antenna end (the end with no pads under it) at the east board edge,
  0.45 mm inside it, over the bare antenna keep-out; seen from the top, pin 1 (GND) is the east end of the pad row along
  the module's south side, next to the antenna.  JLC's footprint for C41349510 matched ours with no rotation or offset
  correction (`jlc_corrections.json`).
- A few of the 32 caps, to confirm the bottom transform in general.  They are not polarized, so any 180 degree error
  there is harmless.

## 10. Fitted after delivery

The MX switch (5-pin PCB-mount preferred, with an LED window or clear housing) and a translucent or shine-through 1u
keycap plug into SW1501 by hand; see design.md, "Key parts".  For bring-up, also have a Tag-Connect TC2030-IDC-NL
cable and an SWD probe (a Pico 2 running `debugprobe`), in case flashing the AT32s over SPI fails.

## 11. Price reference

The PCB-only quote of 2026-10-27, for 5 boards, was made with "Specify Stackup: No" and Plugged vias:

| Item | Price |
|---|---|
| Engineering fee | $25.00 |
| Surface finish (ENIG) | $17.80 |
| Board | $6.70 |
| **PCB total** | **$49.50** (3-4 day build; 2-3 days +$66.20) |
| Shipping, DHL Express | about $23.40 |

The stack-up, filled vias, assembly, through-hole soldering, parts and Extended-part fees are not in it yet.

## 12. Checklist

Before ordering:

1. Run the chain in `README.md` on the final board.  DRC with schematic parity, `check_copper.py`, `check_spacing.py`,
   `check_aux.py` and `check_usb_si.py` must be clean, and the +5 V drop (§5) near the table's.  Then run
   `make_fab.py`.
2. Check JLC's impedance calculator result for 0.15 / 0.15 mm on JLC04161H-3313 (§4).
3. Resolve the TS3USB3031 and re-run `tools/jlc_stock.py` (§8).
4. Choose the variant (§1: with or without the Bluetooth module), upload the gerbers and that BOM/CPL pair; set §3 and
   §7, paste the two remarks.
5. In the placement preview, check D1501, SW1501, U1601 (with the module) and a few bottom caps (§9).
6. Expect DFM questions about the vias in pads, D1501's 0.2 mm edge clearance and the flush connectors at the rails;
   the remarks answer them.

On the first boards (analysis.md §6 and §10): pull the PWR cable under load, measure the 5 V input current and the
port voltage under load (§5), and run the High-Speed passthrough test on every port, PC1 especially.

## 13. Where the numbers come from

Run from `hardware/`; each tool prints what it measured from the board file, the BOM or JLC's parts library.

| Section | Command | What it gives |
|---|---|---|
| §2, §6, §9 | `uv run tools/fab_facts.py` | outline and cut-outs, stack-up, zones, track widths, design rules, vias and vias in pads, holes by size and owner, parts at the edges, parts per side, through-hole parts |
| §4 | `uv run tools/zsolve.py --corners` (about 3 min) | the pair's differential and common-mode impedance on the board's stack-up, each fab tolerance, their spread, the stacked worst cases and the 7628 build |
| §5 | `uv run tools/dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1403=0.5 U403=0.5` (add `--inner 30` for 1 oz inner) | DC drop at each load, loss per layer, the most loaded segments with their current density and IPC-2221 rise |
| §5 | `uv run tools/dc_drop.py VBUS_PWR J301 U402.7=2.0`, `... VBUS_KB U1203.6 J1201=0.5` | the adapter feed and a port leg |
| §5 | `uv run tools/zsolve.py --corners --prepreg 0.092` | the pair on JLC's 1 oz-inner 3313 build |
| §8 | `uv run tools/jlc_stock.py [--boards N]` (network) | stock, part class and need per BOM line; exits 1 if a line blocks the order |
| §1, board checks | the chain in `README.md` | DRC, ERC, parity, copper, spacing, SI |

The datasheet values in §5 are typical at 25 C: TPS2121 RON 56 mOhm (`datasheets/tps2121.pdf`, ON-RESISTANCE) and
TPS2553 DBV rDS(on) 85 mOhm (`datasheets/tps2553.pdf`, POWER SWITCH).
