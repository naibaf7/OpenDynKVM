# OpenDynKVM rev 5 — final cross-check report

**Date:** 2026-10-23 · **Board:** `hardware/kvm.kicad_pcb` (rev 5, routed and refreshed 2026-10-23)
· **Firmware:** `firmware/rp2354`, `firmware/at32f405`, `crates/*` · **Datasheets:** `datasheets/` (local)

This is a verification pass over the finished design. The earlier schematic review
[`analysis.md`](analysis.md) (rev 5, 2026-10-21) is re-checked here; every one of its 15 findings is marked
**fixed**, **open** or **doc**, and new findings are at the end.

**2026-10-23 — fixes applied and the design rebuilt.** F1, F2 and F3 are fixed (documentation, plus the router's
dangling-end trim), and the whole flow was re-run: schematic, ERC, netlist check, placement, routing, DRC,
gerbers/BOM/CPL, `cargo test` (90 tests) and `cargo xtask dist`. The board is now **DRC-clean: 0 violations, 0
unconnected pads, no dangling tracks** (3031 segments, 1124 vias, all 0.30/0.60 mm). F4 was attempted and reverted:
moving the MHL channel to Hub-0 cannot be routed without re-planning the hand-planned HS lanes, so it stays a
bring-up test with the evidence recorded in §9.

## 1. What was checked, and how

| Area | Method | Result |
|---|---|---|
| Schematic ↔ generator | `kicad-cli sch export netlist` + `tools/check_netlist.py` | `OK (950 pins, 264 nets)` |
| Board ↔ schematic | `kicad-cli pcb drc --schematic-parity kvm.kicad_pcb` (KiCad 10.0.6) | `0 DRC violations`, `0 unconnected pads` |
| Schematic ERC | `kicad-cli sch erc` | `Found 0 violations` |
| Placement spacing | `tools/check_spacing.py` | `245 footprints, board 120.1 x 80.1 mm`, `no spacing problems` |
| Auxiliary placement | `tools/check_aux.py` | every part connects its chip by the nearer pad; 6 informational rows |
| Pad/net connectivity | `pcbnew` audit (`/tmp/audit_final.py`) | 264 nets, 964 pads, no orphan, no net without copper |
| Firmware ↔ hardware | pin-by-pin: `firmware/**` vs `gen_schematic.py` + the board nets | match (one stale doc table) |
| Chips vs datasheets | pin tables and requirements read from the local PDFs | conforming, see §5 |
| Polarity/orientation | pad-1 nets, LED DIN/DOUT, TVS orientation on the board | correct, see §6 |
| Power/voltage | measured widths per rail, DS supply ranges of every load | in spec, see §7 |
| Fab outputs | `hardware/bom/*.csv`, `hardware/fab/`, `kvm_gerbers.zip` | complete, see §8 |

## 2. Verdict

* **Connectivity: clean.** Every one of the 964 pads with a net is connected (`0 unconnected pads`), the board's
  netlist equals the generator's intent, no multi-pad net lacks copper, and the 95 single-pad nets are all
  deliberate `unconnected-(<pin>)` nets (unused GPIO, SBU, NC pins) — there is no accidental orphan pad. The DRC is
  now **0 violations** as well: the two dangling track ends the first pass found are trimmed (F2).
* **Every IC is wired as its datasheet requires.** All pin maps checked (see §5) match, including the four
  regulators/switches that a "standard SOT-23-5" habit would get wrong: the TLV62569's DBV pinout is
  `1=EN 2=GND 3=SW 4=VIN 5=FB` (not the TLV73311's `1=IN 2=GND 3=EN 4=NC 5=OUT`), and both are wired correctly.
* **Polarity: correct.** All 4 polarized capacitors have pad 1 on the positive node; the TVS diodes are
  reverse-biased; the LED chain runs DIN→DOUT through all six LEDs with the hand-fitted key LED last.
* **Power: in spec** for every part, with the two notes in §7 (worst-case 4.4 V on the 5 V rail; the ILIM cap at
  2.4 A).
* **Firmware matches the physical board** (GPIO map, dividers, straps, LED order). One doc comment is stale.
* **Remaining work: one bring-up test.** PC1's passthrough through the TS3USB3031's MHL channel (F4) is the only
  item left; it is a datasheet-quality question, not a defect. The documentation fixes (F1, F3) and the two dangling
  track ends (F2) are done, and the board was rebuilt end to end and is DRC-clean.

---

## 3. Firmware ↔ hardware

Pin-by-pin comparison of `firmware/rp2354/src/{board,main}.rs` and `firmware/at32f405/src/{board,adc,link}.rs`
against `hardware/tools/gen_schematic.py:217-236` (block 5) and the AT32/hub blocks. All rows are verified from
both sides (`main.rs` pin **numbers**, not the doc tables).

| GPIO | Net (schematic) | Firmware | |
|---|---|---|---|
| 20 / 21 / 22 | KB_SEL0 / KB_SEL1 / KB_EN | `FrontPort` 0: `PIN_20/21/22` | ✓ |
| 24 / 25 / 26 | MS_SEL0 / MS_SEL1 / MS_EN | `FrontPort` 1: `PIN_24/25/26` | ✓ |
| 32 / 34 / 37 | OT_EN / OT_SEL0 / OT_SEL1 | `FrontPort` 2: `PIN_32/34/37` | ✓ |
| 23 / 27 / 33 | KB_FLT / MS_FLT / OT_FLT | `main.rs:217` fault inputs | ✓ |
| 2 / 3 | HUB0_RST / HUB0_VBUS | `PIN_2` out high, `PIN_3` out high | ✓ |
| 6 / 35 | HUB1_RST / HUB2_RST | `At32Lines.hub_rst` (open drain) | ✓ |
| 13 / 14, 39 / 43 | AT1/AT2 NRST, BOOT0 | `Flex` NRST, `Output` BOOT0 = low | ✓ |
| 12 / 38 | AT1_IRQ / AT2_IRQ | `Input` with `Pull::Down` | ✓ |
| 18/19/16/17, 46/47/44/45 | SPI0 / SPI1 SCK, MOSI, MISO, CS | `Spi::new(SPI0, PIN_18, PIN_19, PIN_16)`, `PIN_46/47/44` | ✓ |
| 15 / 11 | LED_DATA / KEY | PIO WS2812 / `Input` with `Pull::Up` | ✓ |
| 0 / 1 | SVC_SEL / PWR_ST | `Output` low then high / `Input` | ✓ |
| 40 / 41 / 42 | VSNS_PWR / RP_CC1 / RP_CC2 | ADC channels | ✓ |
| 36 | DBG_TX | UART1 TX | ✓ |
| AT32 PA4–PA7 | SPI1 slave (CS/SCK/MISO/MOSI) | MUX5 | ✓ |
| AT32 PB12 / PB1 / PC13 | IRQ / HUBn_RST / ROLE | push-pull / open drain / input strap | ✓ |
| AT32 PA0 / PA1 / PB0 | VBUS/2, CC1, CC2 (IN0, IN1, IN8) | `CH_VDET/CH_CC1/CH_CC2` | ✓ |

Constants and behaviour that must agree with the hardware, all verified:

| Item | Firmware | Hardware | |
|---|---|---|---|
| Board revision | `HW_REV = 5` (`main.rs:47`) | schematic title block **rev 5**, 2026-10-22 | ✓ |
| SPI link speed | `SPI_HZ = 25_000_000` (`link.rs:27`) | AT32F405 SPI slave: 36 MHz receive / **25 MHz transmit** (DS Table 41) | ✓ |
| 5 V sense | `mv(r) * 147 / 47` (`power.rs:56`) | `R 100k` VBUS_PWR→VSNS_PWR, `R 47k`→GND | ✓ |
| PC VBUS sense | `saturating_mul(2)` (`at32/adc.rs:61`) | two 100 kΩ (2:1) | ✓ |
| CC into the ADC | `power_plausible`: CC ≤ 2300 mV | 5.1 kΩ Rd + 10 kΩ series at every ADC pin | ✓ |
| Router select | `Kvm=01, PC1=10, PC2=00, off=11` (`board.rs`) | TS3USB3031 SEL1/SEL0 truth table; `R 10k` pull-up on SEL0 | ✓ |
| LED count/order | `kvm_core::leds::COUNT = 6`, key LED last | D1502…D1506 left→right (`x = 106.5, 113, 148, 183, 213.5`), D1501 = key | ✓ |
| RP core supply | `vreg_hiz()` clears the on-chip regulator | DVDD fed by the TLV73311, `VREG_LX` open (DS Figure 24) | ✓ |
| AT32 boot | BOOT0 low from the RP, 10 kΩ to GND | AT32 SPI1 bootloader on PA4–PA7 | ✓ |

**Finding F1 (Low, doc):** the module table in `firmware/rp2354/src/board.rs:8-12` is stale — it says the "Other"
port is GPIO **28–31** (it is 32/33/34/37) and that GPIO **32/37** are Hub-0 RESET_N/VBUS_DET (they are GPIO 2/3).
The *code* in `main.rs` is correct, so nothing behaves wrongly; only the table misleads.

## 4. Connectivity of the routed board

* `kicad-cli pcb drc --schematic-parity`: **0 DRC violations, 0 unconnected pads** — the two dangling track ends of
  the first pass are gone (F2). Its "footprint errors" section is 199 `net_conflict` plus 2
  `footprint_symbol_mismatch` warnings; both are pre-existing naming artifacts, not defects (F7): the same DRC on
  the HEAD board under the same project rules reports exactly the same 201.
* `tools/check_netlist.py`: the KiCad netlist of the schematic equals the generator's pin map, **950 pins in
  264 nets**.
* Board audit: 264 nets / 964 pads; **0** multi-pad nets without copper; **95** single-pad nets, every one of them
  an explicit `unconnected-(<ref>-<pin>)` (unused AT32 GPIO, hub `PRTPWR/OCS_N` — both have internal pulls — the
  USB-C `SBU1/SBU2` pins, `PC13` usage as input, `VSNS` NC, the LM66200's `ST`, the key LED's unused DOUT).
* **60 pads carry no net**, and all are intentional: 4 × M3 mounting holes, the Tag-Connect locating holes
  (3 footprints × 3), the USB-C connector pegs (3 × 2), the hot-swap socket's mechanical pads (5), and the
  **stencil-only** paste array (9 pads each) of every QFN exposed pad — verified to be on `F.Paste` only, with no
  copper layer and no drill.
* Fabric: 245 footprints, 3094 tracks + 1133 vias, 4 copper layers, 120.05 × 80.05 mm. (The router plans nets in
  parallel, so the exact counts move by ~1 % between identical runs.) Vias are all 0.30 mm drill / 0.60 mm pad, i.e.
  exactly JLC's standard 4-layer limit (0.15 mm annular ring). Zones: GND on F.Cu, B.Cu and In1.Cu (solid plane),
  +3V3 on the full In2.Cu plane; the router adds 419 GND stitching vias (its log).
* Straps and defaults, checked on the board: `CFG_SEL[1:0] = 00` (10 kΩ to GND) on all three hubs, `NON_REM1 = 0`,
  `NON_REM0 = 1` only on Hub-2 (port 1 = its AT32), `RBIAS` 12 kΩ, `CRFILT`/`PLLFILT` 100 nF, `RESET_N` 10 kΩ +
  100 nF, `TEST` (pin 11) to GND (the DS allows "no connect **or** connect to ground"), `VDDA33/VDD33` decoupled
  with 6 × 100 nF + 4.7 µF per hub.
* **F2 (fixed):** the two dangling track ends (0.07 mm on `AT1_MISO`, 3.05 mm on `AT1_MOSI`) were a segment the
  router over-drew past its junction and a short stub in the hand-placed fan-out copper. `tools/route.py`'s trim
  pass used to skip hand-placed copper and to keep a segment that collapsed to zero length; it now trims both and
  drops collapsed segments. The router log records `dangling ends trimmed: 2 ['AT1_MISO', 'AT1_MOSI']` and the DRC
  reports no dangling track at all (it did on the HEAD board).
* Router log confirmation (`hardware/tools/route.log`): `unrouted: 0 nets []`, `done: 3099 segments, 1131 vias,
  239s`. The last-resort pass (full-board window, no heuristic) was needed exactly once, for `KB_FLT`, and
  succeeded on the first try; the `+3V3` plane ends with 0 connections off its main region.

---
## 5. Chip-by-chip checks against the local datasheets

Pin maps were read from the PDFs in `datasheets/`, not recalled. The four "SOT-23-5" parts that are easy to get
wrong are all correct: TLV62569 DBV = `1 EN, 2 GND, 3 SW, 4 VIN, 5 FB` (tlv62569.pdf §5), TLV73311 DBV =
`1 IN, 2 GND, 3 EN, 4 NC, 5 OUT` (tlv733p.pdf §5), 74AHCT1G125 DBV = `1 OE, 2 A, 3 GND, 4 Y, 5 VCC`
(sn74ahct1g125.pdf Table 4-1), TPS2553 DBV = `1 IN, 2 GND, 3 EN, 4 FAULT, 5 ILIM, 6 OUT` (tps2553.pdf §6).

| Part (refs) | Datasheet requirement | How the design meets it | |
|---|---|---|---|
| **RP2354B** (U501) | External-DVDD configuration, DS §6.1.3 / Figure 24 | `VREG_VIN`+`VREG_AVDD` (61/64) from 3V3 via 33 Ω + 4.7 µF; `VREG_FB`+`VREG_PGND` (62/65) to GND; `VREG_LX` (63) open; firmware clears the regulator (`vreg_hiz`) | ✓ |
| | One 100 nF per supply pin, 1 µF at the regulator | 11 × 100 nF + 10 µF on the 11 IOVDD/ADC_AVDD/OTP/QSPI pins; 3 × 100 nF + 1 µF on DVDD | ✓ |
| | QSPI in-package flash, `QSPI_SS` for BOOTSEL | QSPI pins (70–74) open; 1 kΩ + `SW_Push` on `QSPI_SS` | ✓ |
| | 12 MHz crystal, 27 Ω USB series | ABM8-272-T3 + 1 kΩ + 15 pF (Raspberry Pi's values); 27 Ω on D+/D− | ✓ |
| **AT32F405** (U1001, U1101) | "OTGHS1 must be used in conjunction with 12 MHz HEXT" (DS v2.03) | a 12 MHz crystal per AT32 (Y1001/Y1101), 15 pF C0G | ✓ |
| | `OTGHS1_R` (pin 25): 12 kΩ ±1 % to digital ground (spec 11.88–12.12 kΩ) | `R 12k` = C25752 = `0402WGF1202TCE`, UniOhm 0402 **±1 %** | ✓ |
| | `OTGHS1_VBUS` may be left open with VBUSIG set | PB13 (24) unconnected; firmware sets the internal VBUS comparator | ✓ |
| | Decoupling, BOOT0, NRST (Figures 9, 19) | 3 × 100 nF + 4.7 µF + 1 µF; BOOT0 10 kΩ to GND; NRST 100 nF + 10 kΩ pull-up | ✓ |
| **USB2514B ×3** (U701/801/901) | Strap configuration `CFG_SEL = 00`, `RBIAS` 12 kΩ ±1 %, `CRFILT`/`PLLFILT` ≤ 0.1 µF | 10 kΩ to GND on both CFG pins; C25752 ±1 %; 100 nF each | ✓ |
| | 24 MHz crystal, load caps ≈ 2·CL−stray | X322524MOB4SI (12 pF load) + **18 pF C0G** (analysis finding 12) | ✓ |
| | `VBUS_DET` from a divider, or high when self-powered | Hub-1/2: 2 × 100 kΩ from the PC's VBUS; Hub-0: GPIO3 driven high | ✓ |
| | `TEST` (pin 11): no connect or ground; `VSS` = the ground slug only | pin 11 to GND, EP (37) to GND; internal pulls on `PRTPWR/OCS_N` left open (the TPS2553s do the limiting) | ✓ |
| | `NON_REM[1:0]`: 01 = port 1, 10 = ports 1–2, 11 = ports 1–3 | Hub-2 = 01 (AT32 on port 1); Hub-1/0 = 00 | ✓ |
| **TS3USB3031 ×3** (U1202/1302/1402) | VCC 3.3 V, SEL truth table (Table 6-1) | pin 12 = +3V3, pin 9 = GND; `SEL1/SEL0` = 01 Hub-0, 10 MHL, 00 USB1(+2), 11 off, matching the firmware | ✓ |
| **TS3USB30E** (U601) | `S = 0` selects D1, `OE` low to enable | `SVC_SEL` low → the service port when the RP is blank (10 kΩ pull-down); OE to GND | ✓ |
| **TPS2121** (U402) | Priority as configured: XCOMP mode (Table 9-3) | `CP2` = VBUS_PCS × 12/45 (33 k/12 k), `PR1` = VBUS_PWR × 10/32 (22 k/10 k) → PWR wins above ≈ 4.0–4.5 V, PC otherwise, 5 µs fast switchover | ✓ |
| | Overvoltage supervision | `OV1`/`OV2` dividers 47 k/10 k → cut-off at ≈ 6.0 V on each input (2.4 A class) | ✓ |
| | Current limit and soft start | `ILM` 47 kΩ → ≈ 2.4 A (inside the LM66200's 2.5 A); `SS` 100 nF → gentle inrush | ✓ |
| **LM66200** (U401) | Pinout; `ON` low to enable; `ST` optional | VIN1/VIN2 = PC1/PC2 VBUS, VOUT = VBUS_PCS, `ON` to GND (always on), `ST` open (the RP reads the TPS2121's `PWR_ST` instead) | ✓ |
| **TPS2553 ×3** (U1203/1303/1403) | EN active high; ILIM sets the limit; FAULT open drain | EN from GPIO with 100 kΩ pull-down (off until firmware enables); `ILIM` 22 kΩ → 1.09–1.26 A; `FAULT` 10 kΩ pull-up to 3V3 | ✓ |
| **TLV62569** (U403) | Pinout; FB divider sets VOUT; 2.2 µH + 2 × 22 µF for VOUT ≥ 1.8 V | `1 EN`=+5V, `4 VIN`=+5V, `3 SW`→L401→+3V3, `5 FB` = 100 k/22 k → 3.33 V | ✓ |
| **TLV73311** (U404) | Pinout; 1 µF on OUT; EN driven | `1 IN`/`3 EN` = +3V3, `4` NC open, `5 OUT` = +1V1 with 1 µF; 1.085–1.115 V vs the RP's 1.05–1.16 V | ✓ |
| **74AHCT1G125** (U1501) | 5 V VCC, TTL thresholds accept 3.3 V in | `2 A` = LED_DATA, `1 OE` = GND (enabled), `4 Y` → 33 Ω → LED chain | ✓ |
| **USBLC6-2SC6 ×6** | Flow-through I/O 1/6 and 3/4, VBUS 5, GND 2 | one per USB2 pair at each of the 6 connectors | ✓ |
| **PESD5V0S2BT ×3** | Common pin 3 to GND | CC1 → 1, CC2 → 2, GND → 3 on all three USB-C receptacles | ✓ |
| **SMF5.0A ×3** | Unidirectional TVS, reverse-biased | cathode (pad 1) to VBUS, anode to GND, at each connector (stand-off 5.0 V vs a 5.25 V rail: leakage only, breakdown 6.4 V) | ✓ |
| **SK6812 SIDE-A ×5, MINI-E ×1** | 5 V supply in the 3.7–5.5 V range; DIN/DOUT order | side LEDs: `1 DIN, 2 VDD, 3 DOUT, 4 GND`; reverse-mount key LED: `1 GND, 2 DIN, 3 VDD, 4 DOUT` (unused) | ✓ |
| **Crystals** | 12 pF load → 18 pF C0G; 10 pF load → 15 pF C0G | C1549 = `0402CG180J500NT` 18 pF ±5 % C0G; C1548 = 15 pF C0G | ✓ |
| **Passives** | ±1 % where the DS demands it (`RBIAS`, `OTGHS1_R`) | all 5.1k/10k/12k/22k/33k/47k/100k are UniOhm 0402 ±1 % (C25741/44/52/68/79/92, C25905) | ✓ |
| **Cap voltage** | must cover the rail | 100 nF 16 V X7R, 22 µF 0805 25 V, 10 µF 0603 10 V, port/hold-up 220 µF **10 V polymer** | ✓ |

---
## 6. Polarity, orientation and chain order

| Item | Check | Result |
|---|---|---|
| 4 × 220 µF polymer `CP_Elec_6.3x7.7` (C411, C1203, C1303, C1403) | the net on pad 1 — the pad KiCad's `CP_Elec` land pattern marks as the can's positive terminal — against pad 2 | C411 pad 1 = `+5V`, pad 2 = `GND`; C1203/1303/1403 pad 1 = `VBUS_KB/MS/OT`, pad 2 = `GND` — **all four correct** |
| 3 × SMF5.0A (D102/D202/D302) | `Device:D_Zener` pad 1 = cathode, reverse-biased | pad 1 = VBUS, pad 2 = GND on all three |
| 3 × PESD5V0S2BT (D101/D201/D301) | bidirectional pair, common pin to GND | `1` = CC1, `2` = CC2, `3` = GND |
| 5 × SK6812SIDE-A | `1 DIN, 2 VDD, 3 DOUT, 4 GND` | pad 1 = `LED_Dk` (in), pad 3 = `LED_Dk+1` (out), pad 2 = +5V, pad 4 = GND |
| key LED D1501 (reverse mount, B.Cu) | `1 GND, 2 DIN, 3 VDD, 4 DOUT` | pad 2 = `LED_D5` (= the chain's end), pad 4 unconnected — **key LED is last**, so a missing hand-fitted part cannot darken the others (analysis finding 8) |
| chain order | software.md documents PC1 → Keyboard → Mouse → Other → PC2, then the key LED | `kvm_core::leds::COUNT = 6`; on the board LED_D0…D4 are D1502…D1506 at x = 106.5 → 213.5 mm, i.e. left to right, then D1501 |
| 6 × crystal `Device:Crystal_GND24` | 1/3 = electrodes, 2/4 = case ground | 1/3 to XI/XO, 2/4 to GND everywhere |
| USB-C ×3 | D+/D− rows tied, SBU open, shield pads | A6+B6 = DP, A7+B7 = DM, A8/B8 open, `SH` + pegs to GND |
| L401 (inductor), SW1501 (MX switch in its socket), and every non-polarized R/C | no polarity to get wrong | n/a |

## 7. Power, voltage and current

Rails, measured from the board, and the ranges every load needs:

| Rail | Source | Measured track widths (trunk / branches) | Loads and their required range | |
|---|---|---|---|---|
| `+5V` | TPS2121 OUT (PWR port) or LM66200 (PC VBUS), 220 µF polymer + 2 × 22 µF + 10 µF hold-up | 0.60 mm × 145 mm, 0.30 mm × 137 mm | SK6812 3.7–5.5 V; TLV62569 VIN 2.5–5.5 V; TPS2553 2.5–5.5 V; SN74AHCT1G125 4.5–5.5 V | see note |
| `+3V3` | TLV62569 buck (2.2 µH, FB 100 k/22 k = 3.33 V) + full **In2.Cu plane** | 0.30 mm × 35 mm, 0.15 mm fan-out × 75 mm (plane carries the current) | USB2514B 3.0–3.6 V; AT32F405 2.6–3.6 V; RP2354B IOVDD 3.0–3.6 V; TS3USB3031 2.3–4.3 V | ✓ |
| `+1V1` | TLV73311 LDO (1.085–1.115 V) | 0.40 mm | RP2354B DVDD 1.05–1.16 V | ✓ |
| `VBUS_KB/MS/OT` | one TPS2553 per port, ILIM 22 kΩ → 1.09–1.26 A | 0.60 mm × 29/28/27 mm | downstream USB devices, 4.75–5.25 V | ✓ |
| `VBUS_PC1/PC2`, `VBUS_PWR` | the PC/PWR connector | 0.50 mm (PC1 / PC2 / PWR) | LM66200 (abs max 6 V) / TPS2121 IN (3.6–22 V) | ✓ |

Currents and limits: the only current limit in the whole 5 V path is the TPS2121's `ILM` (47 kΩ → ≈ 2.4 A), which
also caps the LM66200 upstream of it below its 2.5 A absolute maximum (analysis finding 4 fixed); each front port
has its own 1.09–1.26 A TPS2553. Copper checks with IPC-2221 (external, 1 oz): 0.60 mm ≈ 1.7 A at a 10 °C rise,
0.50 mm ≈ 1.45 A, 0.30 mm ≈ 1.0 A. So every rail is inside its width for the continuous design currents
(front port 1.26 A on 0.6 mm ≈ 8 °C; 3V3 ≈ 0.45 A in a plane; 1V1 ≈ 40 mA on 0.4 mm). The 0.5 mm PC/PWR runs only
see 2.4 A in a fault/burst case, where the estimate is a ≈ 20 °C rise.

Two honest notes:

* **N1 (Low).** In the worst case (a PC-only supply at the low end of USB 2.0, 4.75 V, and a loaded port) the 5 V
  rail can sit near 4.4 V, which is just below the SN74AHCT1G125's 4.5 V recommended minimum. The part still works
  (its VOH tracks VCC, so 4.35 V is far above the SK6812's 0.7·VDD input threshold), and the 220 µF hold-up plus the
  PWR port make it unlikely. This is the same item as analysis finding 6, now with the hold-up bulk fitted.
* **N2 (Low).** Three series elements (LM66200 + TPS2121 + TPS2553, ≈ 0.15–0.3 V at 1 A) sit between a PC and the
  front ports, so front VBUS is ≈ 4.7–4.9 V from a 5.0 V source. That is inherent to the protected path, and the
  reason the PWR/SERVICE port is preferred for heavy peripherals.

The measured 3V3 budget — hubs 2 × 70–95 mA at HS plus ≈ 66 mA for Hub-0, AT32s 2 × (≈ 35 mA at the firmware's
144 MHz plus ≈ 30 mA while the OTGHS transfers), RP2354B ≈ 40 mA — is ≈ 0.35–0.45 A, and the 5 V rail adds the LED
chain (6 × ≈ 20–40 mA at white) for ≈ 0.5 A total. That is above the "about 0.3 A" still written in design.md §6
(finding F3c), but well inside the 2 A buck and covered by the firmware's advertised-budget warning.

---
## 8. Auxiliary checks (placement tooling, fab, housekeeping)

* `tools/check_spacing.py`: **no spacing problems** — 245 footprints, 120.1 × 80.1 mm; the closest courtyard pairs
  are 0.11–0.20 mm (C411–L401, C1502–C1503, C517–Y501 …), all in the fine-pitch IC neighbourhoods.
* `tools/check_aux.py`: "every auxiliary part connects its chip by the nearer pad". It prints 6 informational rows
  (the front-port SEL pull-downs R1201/1202/1301/1302/1401/1402), which sit at their routers rather than at the
  RP2354B pin they also serve; the docstring documents the other accepted exceptions (R304/R204, C1501, R501/C516,
  plus the switch-side EN pull-downs). None affect the routing, which is complete.
* Fab outputs, produced after the routing commit (`hardware/fab/` timestamp 2026-10-23 08:15):
  * `bom/jlcpcb_bom.csv`: **39 rows, every one with an LCSC part number** (they come from the value tables in
    `gen_schematic.py:53-56` plus `jlc_corrections.json`).
  * `bom/jlcpcb_cpl.csv`: 236 placed parts with designator, mid X/Y, layer and rotation.
  * `bom/hand_assembly.csv`: exactly the two hand-fitted items — D1501 (SK6812MINI-E, bottom) and SW1501 (MX switch
    in a Kailh hot-swap socket, top).
  * `fab/`: 11 gerber/job files (F.Cu, In1.Cu, In2.Cu, B.Cu, both masks, both silkscreens, paste, edge cuts, job)
    plus **kvm-PTH.drl and kvm-NPTH.drl**; `kvm_gerbers.zip` is 766 kB.
* Housekeeping (no action required): `hardware/kvm.stale-route.kicad_pcb` is a gitignored router backup, and
  `tools/easyeda_cache/` still holds six entries no longer used by the BOM (C15008 the old 100 µF 6.3 V port cap,
  C74512 the rev-4 touch key, C52917434 the WS2812B-2020, C15195 10 nF, C337699 22 nF, C25900 4.7 k).

## 9. Findings

### Carried over from analysis.md (rev 5 review)

| # | Finding (analysis.md) | Now | Evidence |
|---|---|---|---|
| 1 | Front-port GPIO map (High) | **fixed** | schematic = 20–23 / 24–27 / **32,33,34,37**; `main.rs:140-153,217` matches number for number |
| 2 | PWR unplug resets the board (High) | **fixed** | TPS2121 in XCOMP mode (`CP2` = VBUS_PCS × 12/45, `PR1` = VBUS_PWR × 10/32) + C411 220 µF hold-up |
| 3 | No overvoltage protection (Medium) | **fixed** | `OV1`/`OV2` 47 k/10 k → cut-off at ≈ 6.0 V, plus an SMF5.0A at each of the three connectors |
| 4 | LM66200 vs the only current limit (Medium) | **fixed** | `ILM` 47 kΩ → ≈ 2.4 A, below the LM66200's 2.5 A absolute maximum |
| 5 | Front-port VBUS bulk (Medium) | **fixed** | 220 µF **10 V** polymer cap on every front port (was 100 µF 6.3 V) |
| 6 | Power budget above 0.3 A (Medium) | **partly** | firmware runs the AT32s at 144 MHz (saves ≈ 22 mA each); design.md §6 still says 0.3 A → F3c |
| 7 | CC into non-5 V tolerant ADCs (Medium) | **fixed** | 10 kΩ in series at all six ADC pins (RP_CC1/2 and AT1/2_CC1/2) |
| 8 | Hand-fitted key LED heads the chain (Medium) | **fixed** | D1501 is last (`LED_D5`), software.md §4.6 updated |
| 9 | AT32 NRST undefined at power-up (Low) | **fixed** | 10 kΩ pull-up on AT1_NRST / AT2_NRST with the 100 nF |
| 10 | Router SEL0 default (Low) | **fixed** | 10 kΩ pull-up (was 100 kΩ) |
| 11 | Hub reset not under RP control (Low) | **fixed** | `HUB1_RST`/`HUB2_RST` on GPIO 6 / 35, driven open-drain |
| 12 | Hub crystal load caps (Low) | **fixed** | 18 pF C0G (C1549) for the 24 MHz crystals, 15 pF kept for the 12 MHz |
| 13 | TPS2121 soft start (Low) | **fixed** | `SS` = 100 nF |
| 14 | Smaller points (Low) | **mostly** | 33 Ω LED series R ✓, debug UART on GPIO36 → Tag-Connect pin 6 ✓, SPI link = 25 MHz ✓ (design.md still says 36 Mbit/s → F3e); MHL path → F4; descriptors → F5; USBLC6 2.5 pF accepted |
| 15 | Documentation (Doc) | **mostly** | schematic PDF re-exported (**rev 5**, 2026-10-22, no rev-4 parts); title block rev 5; software.md chain updated. Remaining design.md rows → F3 |

### New or still open

| ID | Severity | Where | Finding | Suggested action |
|---|---|---|---|---|
| **F1** | **fixed** (doc) | `firmware/rp2354/src/board.rs` | the module table listed the Other port as GPIO 28–31 and Hub-0 RESET_N/VBUS_DET as 32/37 | done: it now lists 34/37/32/33 and 2/3, matching `main.rs` |
| **F2** | **fixed** (board) | `kvm.kicad_pcb` | 2 dangling track ends (`AT1_MISO` 0.07 mm, `AT1_MOSI` 3.05 mm), missed by the trim pass because it skipped hand-placed copper | done: the trim covers that copper and drops collapsed segments; the DRC now reports no dangling track |
| **F3** | **fixed** (doc) | `design.md` | (a) "both AT32s on port 4" — Hub-2's AT32 is on **port 1**; (b) the 12 MHz crystals are the Abracon **ABM8-272-T3 / C20625731**; (c) the 5 V budget is ≈ 0.5 A, not 0.3 A; (d) the shields go straight to GND; (e) 36 Mbit/s vs the 25 MHz link; (f) U23 and U1501 both listed the AHCT; (g) no L401 row; plus (h) the §11 LED chain order, (i) D1501 "first in the chain", (j) the TPS2121 "4.5 A, priority mode" | done: all ten rows updated |
| **F4** | **open — bring-up** | TS3USB3031 ×3 | PC1's passthrough uses the MHL channel (`SEL = 10`), which TI specifies only at 1.5–3.3 V signal levels. Every router channel still negotiates HS with the attached device, so this is about **where an HS fallback is harmless** — the KVM path's data is full speed anyway, so MHL belongs there. **Measured** with the six lanes re-planned from the pairs-only board and Hub-0 routed first: `H0_MS`, `H0_OT`, `H1_KB`, `H2_MS`, `H2_OT` (and both AT32 links) route cleanly, while `H0_KB` (lines 0.000 mm apart), `H1_MS` (−0.225 mm) and `H2_KB` (0.025 mm) fail the new spacing guard and `H1_OT` finds no via-pair site | Two hard numbers now pin the fix: a pair's **layer-change site needs PAIR_VIA_O + via/2 = 1.2 mm** of clear diameter, and **0.45 mm is a floor** for PAIR_VIA_O (at 0.30 the via *pad* reaches the opposite trace — measured `-0.000`), so the router corners must fit 1.2 mm. Mitigations, in order: (a) **stagger the two vias** in `snap_via_site` (route.py, which forces them square today) and plumb the stagger into the store — one via ~0.5–1.0 mm ahead of the other halves the clear width and costs 3–7 ps of skew (the design already accepts ~1 mm); (b) move Hub-0's P/N twist from terminal B (its 0.5 mm pin rows) to A — one token, `(0.9, 'A')` in `pair_links`; (c) the layout options below. Continues as a bring-up eye/`fio` test meanwhile — **update after the router work below:** the twist no longer has to sit at a terminal. `route_pair` now routes the lane first and puts the twist on its roomiest straight stretch (`best_twist_site`, scored with the router's own clearance measure, `TWIST_L = 1.65 mm`), falling back to the terminal swap when a link is too short; `search_clean` prefers a centre line without a U-turn (a U-turn leaves a pair's two lines badly skewed). A pairs-only probe with the channels swapped (`MUX_PINS[0] = ('7','8')`, Hub-1 on Hub-0's channel) shows both: `H1_KB` routes cleanly with its twist **mid-lane** (47.29/47.29 mm, skew 0.01, gap 0.150), and the live view now **marks** the roadblocks — `H1_MS_DP: no way through to waypoint 0 (143.0, 153.75)` and `H1_OT_DP: … (178.0, 153.75)`: the lanes belong to the channel and have to be re-planned for the swap (both marks are in the sidebar and drawn on the copper). One new finding: the mid-lane twist's join left `H2_AT` at a **0.141 mm** P-N gap (want 0.150), so the spacing guard failed it — the twist zone's entry and exit must meet the two offset halves exactly, which is the next token. `check_copper.py` also checks connectivity now (zone fills modelled from the board), the pair widths and the GND plane under every F.Cu pair, and passes the committed board with all of them. **Third probe, the twist obeys the rule now:** `twist_sites` hands `route_pair` every candidate best-first and it takes the first whose *assembled* geometry (zone + both offset halves + stubs) keeps PAIR_S, marking each site it rejects with the gap it came out at — so the 0.141 mm `H2_AT` case is answered by trying the next site instead of committing it. The pairs' *pre-routing* was examined too: they are the only pads whose escape is drawn after the plane/IC fan-outs, so those fan-out vias land in the pair's escape and cut off the first lane leg. Giving the pairs the same reserved escape as everyone else did not survive measurement: with one PAIR_FAT corridor reserved for both nets, `H0_OT` came out at `lines only −0.119 mm apart` on the otherwise proven rev-5 config, and with each line's own escape via site the link failed outright. The mechanism was not isolated, and the note that first explained it ("a pair's two nets see each other as foreign copper") is **wrong** — both lines share one netcode (`Board.pair_of`), so they are invisible to each other's fields; that is in fact what makes a reservation *as the pair* safe (see the fourth probe). The experiment was dropped on the evidence, not on a theory. **Third probe outcome: 13 of 16 pairs route**, `H2_AT` and `H1_AT` included, each with its twist *mid-lane* at gap 0.150 (the site that came out at 0.141 is marked as rejected), and the only failures left are the three lane plans — `H0_KB`, `H1_MS`, `H1_OT`, all "no way through to waypoint 0": re-planning the six router-side lanes for the new pin ends is now the whole of the remaining swap work (the frozen view with those three circles is `hardware/debug/swap-attempt-3/`). `check_copper.py` still passes the committed rev-5 board; **fourth probe — the swap now routes as far as it can:** the three links that cannot fit the pair's 0.45 mm fat corridor out of a mux hand their two lines to the ordinary router (the pair is de-registered and its nets route per line, with rip-up, exactly like every other net close to a chip), so nothing is left unrouted; `hardware/debug/swap-attempt-4/` marks each of those spots twice, red ("the pair could not pass here") and yellow ("handed over"). Also measured and then reverted: reserving the *bulge* a pair's layer change needs (1.8 × 2.5 mm, from the static lane plan, before the fan-outs) **cost the chips their own fan-out** — U801/U901 lost three vias each, U1001/U1101 one each, and a proven pair regressed to −0.260 mm — which is the same lesson as the corridor: close to a chip the pair must not reserve space it cannot use, and the bulge has to be part of the *lane plan* rather than bolted on. Re-planning those lanes is the remaining work — and the fallback's own routing names the blockers exactly: `no rippable blockers for H1_MS_DP; leaving it partly unrouted (in the way: AT1_SCK, H0_MS_DM, H0_MS_DP)`. The retired channels' lanes *cross* each other (Hub-0's Mouse pair now runs through the corridor Hub-1's Mouse lane needs), which is the geometric proof that the six lanes belong to the pin ends and must be re-planned together; the two hub links cannot simply be ripped for each other because both are pairs. **Fifth probe — the three rules that came out of reading that:** (i) pair layer-change vias are now the fab's small via (0.45/0.2), which pulls the bulge a twist or lane change needs in by 0.075 mm a side (clear radius 0.675 instead of 0.75) without a reservation; (ii) a chip's fan-out area is never reserved for a pair (`FANOUT_IC`/`QFN_FANOUT`), so the chip's own vias get the room; (iii) a lane leg may now drop a via pair, run on the other layer and come back — a leg was *planned* on one layer but that is not a requirement. Measured effect: `H1_MS` **routes** (1525 cells, gap 0.150), `H0_KB` gets past leg 0 with "only fits with a layer change en route" and then fails on pair spacing (`−0.146 mm at (108.25, 149.36)`, i.e. the two lines cross where that en-route layer change sits — the store's composition of the two offset halves around a mid-leg change is the next token), and `H1_OT` now reaches **leg 11 → (117.75, 125.95)** on the hub side instead of dying at leg 0. Pairs routed as pairs: **14 of 16**; the two failures hand their lines to the ordinary router. Frozen view: `hardware/debug/swap-attempt-5/` (and the router's own 8765) |
| **F5** | Info | Hub-1 vs Hub-2 | AT32 #1 sits on Hub-1 port 4, which `NON_REM[1:0]` cannot mark (the strap covers ports 1–3 only), so the two PCs see slightly different hub descriptors | none — documented in the generator |
| **F6** | Info | LM66200 | `ST` (pin 8) is left open; the DS allows it, and the RP2354B reads the TPS2121's `PWR_ST` for the source | none |
| **F7** | Info (tooling) | parity DRC | `--schematic-parity` reports 199 `net_conflict` ("pad net `X` doesn't match the schematic's `/X`") and 2 `footprint_symbol_mismatch` (SW1501, D1501: "exclude from BOM" differs between symbol and footprint) | none needed: the `/` is KiCad's root-sheet path, `check_netlist.py` strips it deliberately, and the HEAD board produces the same 201. Do not chase these; if a clean parity report is wanted, write `/NAME` net names from the generators |
| **N1** | Low | +5V rail | worst-case PC-only 4.75 V port minus the series path can reach ≈ 4.4 V, just under the AHCT's 4.5 V recommended minimum (it still works; VOH tracks VCC) | done: noted in design.md §6; the PWR port removes it |
| **N2** | Low | front VBUS | the protected path costs ≈ 0.15–0.3 V, so front VBUS is ≈ 4.7–4.9 V from 5.0 V | none |

## 10. Reproducing this check

```bash
cd hardware
kicad-cli sch export netlist --format kicadsexpr -o /tmp/kvm.net kvm.kicad_sch && python3 tools/check_netlist.py /tmp/kvm.net
kicad-cli pcb drc --format report --output /tmp/drc.rpt --severity-all --schematic-parity kvm.kicad_pcb
python3 tools/check_spacing.py && python3 tools/check_aux.py
# §4, §6, §7 (pad/net, polarity, widths) came from a one-off audit script, not kept
```

Full rebuild after a design change (2026-10-23 run: 245 footprints, router 200–290 s, `cargo test` 90 passed):

```bash
cd hardware
python3 tools/gen_schematic.py && kicad-cli sch erc kvm.kicad_sch && kicad-cli sch export pdf kvm.kicad_sch -o kvm_schematic.pdf
python3 tools/gen_pcb.py --force && python3 tools/check_spacing.py && python3 tools/check_aux.py
python3 tools/route.py                      # ~5 min; watch tools/route.log for "unrouted: 0" and "dangling ends trimmed"
kicad-cli pcb drc --schematic-parity kvm.kicad_pcb
python3 tools/make_fab.py                   # bom/, fab/, kvm_gerbers.zip
cd .. && PATH=$HOME/.cargo/bin:$PATH cargo test && PATH=$HOME/.cargo/bin:$PATH cargo xtask dist   # dist/
```

* **Pair spacing is now enforced, not negotiated** (`tools/route.py`, 2026-10-23). `route_pair` used to answer a
  corridor that was a fraction of a millimetre too narrow by squeezing a pair's two lines together — which is a
  short, and is what produced the touching/overlapping pairs in the MHL-swap attempts (`0.000`, `−0.225 mm`). It now
  fails that pair instead (`lines only X mm apart (want 0.150 mm)`), so the retry ladder can try another order or
  another waypoint. Nothing changes for rev 5: every pair there already reports exactly 0.150 mm.
  Also measured while doing this: a pair's layer-change site needs **`PAIR_VIA_O + via/2` = 1.2 mm of clear
  diameter**, and `PAIR_VIA_O` cannot go below 0.45 mm (at 0.30 the via *pad* reaches the opposite line).

* **Manual review (F4):** `hardware/debug/swap-attempt/` is a live-view snapshot of the closest MHL-swap attempt
  (fixed copper + pairs only, i.e. the state the pair router sees). Serve it with
  `python3 -m http.server 8765 --directory hardware/debug/swap-attempt` and open http://localhost:8765/ — wheel
  zooms, the net box highlights a net's pads, and the log panel shows the router's own failures.

If placement or routing is changed again, re-run the chain in `hardware/README.md`:
`gen_pcb.py --force` → `check_spacing.py` / `check_aux.py` → `route.py --live` → `make_fab.py`. After any
*schematic* change, also re-export the schematic PDF and update this report's §5 table.


## 11. The full-speed KVM path: what it implies, and where it does not suffice

Hub-0's upstream is the RP2354B, whose USB is a **full-speed (12 Mbit/s)** device, so everything the KVM *emulates*
— keyboard, mouse, and any mirrored device — runs at full speed. That is deliberate (design.md §7) and it is enough
for the job:

| Question | Answer | Source |
|---|---|---|
| Is 12 Mbit/s enough for HID? | Yes, by an order of magnitude: ~9 interrupt endpoints at 1 kHz use under half a full-speed frame. | design.md §7 |
| Does it cost latency? | Not beyond the design's own budget: the KVM polls a mouse at the mouse's own 1000 Hz, and the PC sees 1000 Hz either way. Measured model: ≈520 µs average, ≈1.02 ms worst for a full-speed device. | design.md §7 |
| High-rate gaming mice? | An 8 kHz High-Speed mouse falls back to 1000 Hz behind Hub-0 — within spec — and keeps its full rate in Passthrough (Hub-1/Hub-2). | design.md §7 |
| What does *not* work through the KVM? | Anything needing more than ~10 Mbit/s: webcams, storage, audio. Those are exactly what the per-port router and Passthrough mode are for: the device goes to Hub-1/Hub-2 at 480 Mbit/s, **around** the RP2354B. | design.md §3.5, §4 |
| Does the PC see a full-speed device? | No. The PC-facing device is the AT32F405, a High-Speed device on a HS hub, so the PC polls it on 125 µs microframes. Only the device→KVM hop is full speed. | design.md §3.1, §7 |
| Why does this matter for F4? | A device's link to its hub runs at **High Speed on all three router channels** whenever a HS device is attached — the KVM's full-speed limit is in the hub's *data* path (the TT), not the signal path. So the spec-marginal MHL channel should sit where an HS fallback costs nothing: the KVM path (whose data is full speed anyway), not PC1's 480 Mbit/s passthrough. | report.md §9 F4 |

In short: full speed suffices because the design *splits* the two jobs — emulate HID over the full-speed path the RP
can drive, and hand anything bandwidth-hungry straight to the PC's own hub.
