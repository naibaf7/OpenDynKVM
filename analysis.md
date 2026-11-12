# OpenDynKVM: schematic review (rev 5)

Review of 2026-10-21 against [specs.md](specs.md), [design.md](design.md) (rev 5) and the datasheets in
[datasheets/](datasheets/).

## 1. Scope and method

**Reviewed:**
- [hardware/tools/gen_schematic.py](hardware/tools/gen_schematic.py): the netlist source.
- [hardware/tools/gen_symbols.py](hardware/tools/gen_symbols.py): the custom symbols.
- [hardware/kvm.kicad_sch](hardware/kvm.kicad_sch): generated 2026-10-21 10:53.

**Tool checks:**
- The netlist exported from `kvm.kicad_sch` equals the generator's pin map. `tools/check_netlist.py` reports OK
  (912 pins, 257 nets).
- ERC: 0 errors, 0 warnings.

**Pin-by-pin check:** every IC pin was matched to its symbol pin name, then read against the pin table and the
application section of the part's datasheet (sections cited below).

**Firmware cross-check:** the RP2354B and AT32F405 firmware pin maps (`firmware/*/src/`) were compared with the
schematic.

**Not covered:**
- PCB layout and routing. The board is being reworked for rev 5: DRC shows 499 unconnected items. Of its 201
  schematic-parity messages, 199 are only the `/` net-name prefix (`VBUS_PWR` vs `/VBUS_PWR`). The other 2 are the
  BOM-exclusion flag on the hand-fitted D1501 and SW1501. None is a real net difference.
- Signal-integrity simulation.
- Current JLC stock.

**Severity:**

| Level | Meaning |
|---|---|
| **High** | A function fails, or the board resets, in normal use |
| **Medium** | Works, but with thin margin, a spec risk, or a damage risk under foreseeable conditions |
| **Low** | Robustness or bring-up convenience |
| **Doc** | Documentation does not match the schematic |

## 2. Results at a glance

The connectivity is clean. Every pinout, select-line polarity, strap and divider checked against its datasheet is
correct (§5). The issues are in the power path, in protection, in one firmware/schematic mismatch, and in
documentation.

| # | Sev. | Area | Finding |
|---|---|---|---|
| 1 | **High** | Firmware ↔ schematic | The Mouse and Other front-port GPIOs differ. The firmware drives the Other port as if it were "Mouse", and never powers the real Mouse port |
| 2 | **High** | Power path | When the PWR adapter is unplugged, the TPS2121 only starts switching to the PC supply once VBUS_PWR has fallen to about 3.3 V, and then takes about 100 µs. +5 V and 3V3 most likely collapse, which resets the KVM |
| 3 | Medium | Power path | No overvoltage protection on +5 V: OV1/OV2 are grounded and there is no VBUS TVS. The LM66200 is rated 6 V absolute maximum *(fixed in rev 9)* |
| 4 | Medium | Power path | The LM66200 is rated 2.5 A absolute maximum, but the only current limit in its path is the TPS2121 at about 4.6 A *(fixed in rev 9: 47 kΩ, 2.4 A)* |
| 5 | Medium | Front ports | Each port has about 30–50 µF of effective VBUS bulk (100 µF, 6.3 V X5R, at 5 V bias); USB 2.0 asks for ≥ 120 µF *(fixed in rev 9: 220 µF, 10 V polymer)* |
| 6 | Medium | Power budget | The electronics probably draw 0.35–0.6 A from 5 V, not 0.3 A |
| 7 | Medium | Protection | The USB-C CC lines go straight into ADC pins, which are not 5 V tolerant in analog mode |
| 8 | Medium | UI, bring-up | The hand-fitted key LED is first in the LED chain. If it is missing or damaged, every status LED goes dark |
| 9 | Low | AT32 control | Until the RP2354B firmware runs, AT32 NRST sits at 1.4–2.6 V, which is an undefined logic level |
| 10 | Low | Front ports | The 100 kΩ pull-up on SEL0 does not guarantee the documented Hub-0 default |
| 11 | Low | AT32 flashing | The RP2354B cannot hold Hub-1/2 in reset while it flashes an AT32 over SPI |
| 12 | Low | Clocks | The hub crystals (load capacitance 12 pF) have 15 pF caps; about 18–20 pF is right |
| 13 | Low | Power path | The TPS2121 soft-start capacitor (10 nF) gives a 0.3–0.4 A inrush at plug-in |
| 14 | Low | Various | Six smaller points: LED data resistor, debug UART, hub descriptors, MHL path, SPI speed, RP2354B stepping |
| 15 | Doc | Docs | Stale schematic PDF; design.md differs from the schematic in several places |

## 3. Findings

### 1. Front-port GPIO map: firmware and schematic disagree (High)

| | Keyboard | Mouse | Other | GPIO 33–36 |
|---|---|---|---|---|
| Schematic ([gen_schematic.py:197-199](hardware/tools/gen_schematic.py#L197-L199)) | 20–23 | **24–27** | **28–31** | not connected |
| Firmware ([main.rs:104-116](firmware/rp2354/src/main.rs#L104-L116), [main.rs:181](firmware/rp2354/src/main.rs#L181), [board.rs:7-8](firmware/rp2354/src/board.rs#L7-L8)) | 20–23 | 28–31 | 33–36 | used for Other |

Each group is SEL0, SEL1, EN and FAULT, in that order.

With today's firmware on a rev-5 board:
- **"Mouse" commands** switch and power the **Other** port's TS3USB3031 and TPS2553.
- The Mouse fault input actually reads `OT_FLT`.
- **The real Mouse port is never powered**: `MS_EN` (GPIO26) stays at its 100 kΩ pull-down.
- **"Other" commands** drive pins that are not connected.

**Fix:** use the schematic's map in the firmware (Mouse = 24/25/26/27, Other = 28/29/30/31). The schematic comment
says the grouping follows the router positions on the board, so the schematic is the side to keep. The AT32F405
firmware pins (PA0/PA1/PB0 ADC, PB1 hub reset, PB12 IRQ, PC13 role, SPI1 on PA4–PA7) all match.

### 2. Unplugging the PWR adapter resets the board (High)

**How it is configured** ([gen_schematic.py:164-171](hardware/tools/gen_schematic.py#L164-L171)):
- PR1 = VBUS_PWR × 10/32.
- CP2, OV1 and OV2 are tied to GND.

**What the datasheet says** (tps2121.pdf):
- Table 9-3: with CP2 < VREF and PR1 ≥ VREF, OUT stays on IN1 whatever IN2 does ("VREF mode").
- It changes over only after PR1 falls below VREF. VREF falling is 0.99–1.09 V, which here means
  **VBUS_PWR ≈ 3.2–3.5 V**.
- The changeover then takes **t_SW = 100 µs** (§7.5, condition "CP2 or SEL < VREF").
- The 5 µs fast switchover (t_FSW) needs CP2 ≥ VREF.

**What happens when the adapter is pulled under load:**
1. **IN1 and +5 V sag together** down to about 3.3 V, because nothing else supplies the rail. That is already below
   the SK6812's 3.7 V minimum, and the buck can no longer hold 3.3 V.
2. **The rail then gets no supply for about 100 µs.** +5 V has about 30 µF effective: 2 × 22 µF 0805 and 10 µF 0603,
   after DC-bias derating.
   - At 0.5 A load it loses a further ~1.7 V in that time.
   - At 1.5 A it collapses completely.
3. **3V3 follows.** The hubs and the AT32 HS PHY need at least 3.0 V. Expect both PCs to see the KVM disconnect and
   re-enumerate, which is exactly what "power is optional" (specs.md) should avoid.

The figures are estimates. Confirm on the prototype by pulling the PWR cable under load.

**Fix, with the same part:** use **XCOMP mode** (TPS2121 §10.3).
- **CP2** gets a divider from VBUS_PCS, so that CP2 > VREF whenever a PC powers the board.
- **PR1** gets a divider that crosses CP2 at VBUS_PWR ≈ 4.4 V. Example:
  - CP2 = VBUS_PCS × 10k/(10k + 30.1k), which is 1.25 V at 5 V;
  - PR1 = VBUS_PWR × 10k/(10k + 25.5k), which is 1.41 V at 5 V and equals CP2 at 4.4 V.
- **Without a PC,** CP2 < VREF and the part falls back to VREF mode on PR1 (PWR alone above about 3.8 V), which is
  still correct.
- **Optionally,** add 100–220 µF of bulk on +5 V as hold-up.

Check the dividers against TI's design procedure before committing.

### 3. No overvoltage protection on +5 V (Medium)

**What sits on the 5 V paths:**

| Part | Where | Rating |
|---|---|---|
| LM66200 | on PC VBUS | 6 V absolute maximum (lm66200.pdf §6.1) |
| TLV62569 | on +5 V | VIN 6 V absolute maximum |
| TPS2553 | on +5 V | 7 V |
| SK6812 LEDs | on +5 V | 5.5 V maximum operating |

**What protects them:**
- **TPS2121 OV1/OV2 are tied to GND** ([gen_schematic.py:165](hardware/tools/gen_schematic.py#L165)), so its
  overvoltage cut-off is disabled.
- **The only VBUS clamp** is the USBLC6's VBUS pin. That is a reference zener (V_BR ≥ 6 V at 1 mA), not a surge
  part.

**Two realistic cases:**
- **Hot-plug ringing:** plugging a cable into a ceramic-only VBUS input can ring well above 6 V.
- **A faulty or high-voltage charger on PWR** passes straight through the TPS2121, which is rated 22 V, onto +5 V.

**Fix:**
- Put a divider on **OV1** (and OV2) that trips at about 5.8–6.0 V. For example, 47 kΩ / 10 kΩ trips at 6.0 V
  (1.06 V × 5.7). The channel then turns off, and the part changes over to the other input if that one is valid
  (§9.3.5).
- Add a **5 V-standoff TVS** (SMF5.0A class) at each USB-C VBUS, next to the connector. If board space is short, at
  least add a small electrolytic or polymer capacitor to damp hot-plug ringing.

**Fixed in rev 9:** exactly these two fixes are on the board — 47 kΩ / 10 kΩ on OV1 and on OV2
([gen_schematic.py:217](hardware/tools/gen_schematic.py#L217), 6.0 V cut-off on each input), and one SMF5.0A per USB-C
VBUS (VBUS_PC1, VBUS_PC2, VBUS_PWR) at the connector.

### 4. LM66200 current rating vs. the only current limit (Medium)

- **LM66200:** I_MAX = 2.5 A is an absolute-maximum rating, and the part has no current limit of its own.
- **TPS2121:** R_ILM = 22 kΩ gives 65.2 / 22^0.861 ≈ **4.6 A** (Eq. 2), and that is the only limit in the PC path.
- **Possible load:** the three TPS2553s with R_ILIM = 22 kΩ allow up to 1.26 A each (tps2553.pdf, Eq. 1, IOS max).
  With about 0.4 A of electronics that is about **4.2 A**.
- **Result:** on a PC Type-C 3 A port, or on a PC port that limits loosely, heavy peripherals can push the LM66200
  past 2.5 A. A short on a front port is not the risk, because the TPS2553 catches that.

**Options:**
- R_ILM = 44.2 kΩ, which gives 2.0–3.0 A (TPS2121 §7.5 table) and 2.5 A typical. This also caps the PWR path, to
  about 2 A of peripheral current with a 3 A charger.
- Or keep 4.5 A, and replace the LM66200 with a second TPS2121 in VCOMP (diode-OR) mode, which has its own adjustable
  current limit.

**Fixed in rev 9:** R_ILM is 47 kΩ ([gen_schematic.py:197](hardware/tools/gen_schematic.py#L197)):
65.2 / 47^0.861 ≈ **2.4 A**, inside the LM66200's 2.5 A absolute-maximum rating, and it caps the PWR path at the same
2.4 A. The LM66200 stays.

### 5. Front-port VBUS bulk capacitance (Medium)

- **Required:** USB 2.0 §7.2.4.1 asks for at least 120 µF of low-ESR capacitance per downstream port. The TPS2553's
  typical application also shows 120 µF.
- **Fitted:** one Samsung CL31A107MQHNNNE per port: 100 µF, **6.3 V**, X5R, 1206
  ([gen_schematic.py:340](hardware/tools/gen_schematic.py#L340)).
  - At 5 V DC bias a part like this keeps roughly a third to a half of its nominal value; check Samsung's DC-bias
    curve.
  - That leaves about 30–50 µF per port, and only 26 % voltage margin.
- **Effect:** hot-plugging a device on one port pulls the other ports' VBUS further down. A mouse can then brown out
  and re-enumerate.

**Fix:** use 150–220 µF per port, either as a low-ESR polymer or aluminium capacitor rated ≥ 10 V, or as 2–3 × 47 µF
10 V X5R in 1210.

**Fixed in rev 9:** each front port carries one MA10V220M6X8 — 220 µF, 10 V, polymer aluminium, 28 mΩ — so the
≥ 120 µF of USB 2.0 §7.2.4.1 is met at the rated voltage instead of at a third of a 6.3 V part's nominal value
([gen_schematic.py:416](hardware/tools/gen_schematic.py#L416)). +5 V itself got the same part as hold-up
([gen_schematic.py:221](hardware/tools/gen_schematic.py#L221)).

### 6. Power budget is higher than the 0.3 A in design.md §6 (Medium)

| Load on 3.3 V | Source | Current |
|---|---|---|
| Hub-1, Hub-2 at HS (AT32 port active) | usb2514b Table 6-4: 70 mA base, +25 mA per further active HS port | 2 × 70–95 mA |
| Hub-0 at FS, 3 devices | Table 6-5: 50 mA + 2 × 8 mA | ≈ 66 mA |
| AT32F405 × 2 at 216 MHz | DS Table 21: 57 mA with all peripherals on; Table 44: OTGHS +30 mA while transferring | 2 × ≈ 87 mA |
| RP2354B (core via LDO, I/O, USB) | estimate | 40–60 mA |
| **3.3 V total** | | **≈ 0.44–0.53 A** |

**Translated to the 5 V input:**
- **Electronics:** about 0.33–0.40 A, through a buck of about 88 % efficiency.
- **LEDs:** SK6812s draw up to 12 mA per colour, so 6 LEDs at full white draw about 0.22 A. Dimmed, they draw a few
  tens of mA.
- **Total:** **0.35–0.6 A** from 5 V.

**Consequences:**
- **A USB 2.0 PC port (500 mA) leaves almost nothing for peripherals.** Running from a PC alone needs a USB 3 or a
  Type-C 1.5 A / 3 A port.
- **The KVM breaks two USB current rules on a PC's VBUS.** It draws about 0.35 A before configuration, where 100 mA
  is allowed, and the same while the PC suspends the bus, where 2.5 mA is allowed.
  - This follows from the spec's "power from PC1".
  - Desktop ports generally tolerate it; some laptop ports limit or cut VBUS.
- **The front VBUS sags on PC-only power.** The PC delivers at least 4.75 V. After cable drop, and about 0.1–0.2 V
  across LM66200 + TPS2121 + TPS2553 at 1 A, the front VBUS can approach 4.4 V.

**Mitigations:**
- Run the AT32s at 144 MHz: 35 mA instead of 57 mA (DS Table 21), which still leaves enough for HS.
- Default the LEDs to dim.
- Measure on the prototype (design.md risk 5).

### 7. CC lines go straight into ADC pins that are not 5 V tolerant (Medium)

**The pins:**
- **RP2354B GPIO41/42** (ADC1/2) are "Digital IO / Analogue" pins, not fault-tolerant ones, so they are limited to
  IOVDD + 0.3 V (rp2350 datasheet Table 1673). `CC1_PWR` and `CC2_PWR` connect to them directly.
- **AT32 PA1/PB0** are FTa pins. These "cannot be 5 V-tolerant in analog mode" (DS v2.03 Table 9, note 2).

**The risk:** normally CC stays at or below 2.04 V, with the 5.1 kΩ Rd. But in the USB-C receptacle, CC (A5/B5)
sits right next to VBUS (A4/B4). Debris, a bent contact or a non-compliant cable puts 5 V on CC, and the PWR port
takes arbitrary chargers and cables.

**Fix:**
- Put a **10 kΩ series resistor** between each CC net and its ADC pin. Rd and the TVS stay at the connector.
- Optionally add 10 nF at the ADC pin.
- Check the AT32 sample time against DS Table 50, maximum source impedance.

### 8. The hand-fitted key LED heads the LED chain (Medium)

**The chain** ([gen_schematic.py:352-360](hardware/tools/gen_schematic.py#L352-L360)): U1501 → **D1501**
(SK6812MINI-E) → D1502 … D1506.

**Why that is fragile:** D1501 is hand-soldered after JLC assembly, on the bottom side, and it is heat-sensitive. If
it is not yet fitted, fitted badly or overheated, no data reaches D1502. All five front status LEDs then stay dark,
including the self-test colour code that bring-up relies on (design.md §10, step 1.5).

**Fix:**
- Put the key LED **last** in the chain (D1506 DOUT → D1501 DIN), or add a normally-open solder jumper from
  `LED_D0` to `LED_D1` to bypass it.
- Update the chain order in the firmware and software.md to match.

The reverse-mount footprint itself is correct. KiCad's pads are in the package's bottom-view order, and flipped to
B.Cu they give the datasheet's lens-side pad pattern: VDD and DOUT on one side, DIN and GND on the other.

### 9. AT32 NRST is undefined until the RP2354B firmware runs (Low)

**The two pulls:**
- **RP2350 side:** pads come out of reset with the pull-down on (PADS_BANK0 PDE reset value = 1; 36–113 kΩ at
  3.3 V). They stay that way during the boot ROM and in BOOTSEL/service mode.
- **AT32 side:** NRST has a permanent 30–50 kΩ pull-up. Low must be below 0.8 V and high above 2.0 V (DS Table 39).

**Result:**
- **NRST sits at 1.4–2.6 V,** with 100 nF on the pin. For some combinations of parts that is inside the undefined
  band.
- **The AT32s may stay in reset, run, or chatter.** A chattering AT32 would show its PC a USB device that keeps
  connecting and disconnecting while the RP2354B sits in BOOTSEL.
- **The firmware itself is fine.** Embassy's `Flex::new` clears the pull-down, and the firmware drives NRST
  open-drain.

**Fix:** add a **10 kΩ pull-up** on `AT1_NRST` and `AT2_NRST`. NRST is then at least 2.7 V in the worst case, and the
firmware keeps driving it low only.

### 10. The router SEL0 default is not guaranteed (Low)

- **The divider:** 100 kΩ from SEL0 to +3V3 ([gen_schematic.py:331](hardware/tools/gen_schematic.py#L331)) against
  the RP2350's reset pull-down gives **0.87–1.75 V**.
- **The threshold:** the TS3USB3031 needs at least 1.3 V for high and at most 0.6 V for low (ts3usb3031.pdf §5.5).
- **Result:** "Hub-0 by default" holds only once the firmware drives the pin.

Today this is harmless, because the TPS2553s keep front VBUS off until firmware enables them. Use 10 kΩ (≥ 2.58 V) if
the default matters; otherwise drop the claim from the comment and design.md.

### 11. AT32 flashing: the RP2354B cannot keep the PC away (Low, bring-up risk)

- **Two interfaces are live at once.** With BOOT0 = 1 the AT32 ROM listens on SPI1 **and** offers DFU on OTGHS1
  (DS §2.5). The 12 MHz crystal is fitted, so the DFU device appears on the hub, and a powered PC enumerates it.
- **Whether the ROM then stops listening on SPI is not documented here.** Artery's ISP protocol note is still missing
  (design.md §15). STM32 ROMs (AN2606) run an interface-detection phase and then commit to the interface that was
  selected.
- **The RP2354B cannot prevent this.** `HUBn_RST` is driven only by the AT32's PB1, which floats in bootloader mode.

**Fix:**
- Also wire `HUB1_RST` and `HUB2_RST` to a free RP2354B GPIO (2–10 and 33–36 are free). Drive it open-drain; it
  shares the existing 10 kΩ pull-up. The RP2354B can then hold the hub in reset while it flashes.
- In firmware, never clear **nBOOT1** (user system data bit 4, default 1). With nBOOT1 = 0, BOOT0 = 1 boots from
  SRAM, and the SPI recovery path is lost (RM v2.02, user system data at 0x1FFF F800, bit 4; AN0184 boot-mode
  note).

### 12. Hub crystal load capacitors (Low)

- **The crystal:** the X322524MOB4SI has a 12 pF load capacitance (LCSC C70590).
- **What the datasheet asks for:** usb2514b Figure 7-2 gives C = 2 × CL − C_stray ≈ 24 − (4–6) ≈ **18–20 pF**, and
  notes that "each … typically around 18 pF". The generator fits 15 pF
  ([gen_schematic.py:132-133](hardware/tools/gen_schematic.py#L132-L133), shared with the 12 MHz crystals).
- **The effect:** the 24 MHz runs roughly 30–40 ppm fast. That is inside the hub's 350 ppm budget, but margin is
  given away for free.
- **Fix:** 18 pF C0G for the 24 MHz crystals only.

The 15 pF caps are right for the ABM8-272-T3 12 MHz crystals (CL 10 pF, Raspberry Pi's values).

### 13. TPS2121 soft start (Low)

- **What 10 nF gives:** CSS = 10 nF scales TPS2121 Table 9-1 to about 7.8 V/ms, so +5 V reaches 5 V in about
  0.6 ms. Charging ~50 µF that fast draws about 0.3–0.4 A from the PC at plug-in, on top of the load.
- **Fix:** 47–100 nF brings the inrush down to about 40–90 mA, which is gentler on PC ports and closer to USB inrush
  practice.

### 14. Smaller points (Low)

- **LED data line:** add 33–100 Ω in series at U1501's output (pin 4). The trace runs from the board centre into the
  chain, and the series resistor damps ringing.
- **Debug UART:** GPIO0/1 (UART0) are used. Bring a UART TX on a free GPIO to a test pad or to the unused pin 6 of
  the RP2354B Tag-Connect footprint.
- **Hub descriptors differ between PC1 and PC2.**
  - AT32 #2 is on Hub-2 port 1, strapped non-removable, so Hub-2 reports itself as a compound device.
  - AT32 #1 is on Hub-1 port 4, which the NON_REM straps cannot mark: they cover ports 1–3 only.
  - The difference is harmless, but the two PCs see different hub descriptors.
- **TS3USB3031 MHL path (PC1 passthrough):**
  - It is specified only at 1.5–3.3 V signal levels. Figure 5-1 shows about 5 Ω typical at USB HS levels
    (0–0.4 V), with C_ON = 1.3 pF against 1 pF on the USB paths.
  - It is likely fine, but PC1's passthrough is not guaranteed by the datasheet, unlike PC2's.
  - Run the eye or `fio` test (design.md risk 7) on PC1 specifically.
- **USBLC6-2SC6:** it adds 2.5 pF typical (3.5 pF max) per line, twice on each HS passthrough path. This is
  acceptable. If the eye margin is short, use an ESD part of 0.5 pF or less.
- **AT32 SPI slave speed:** the limit is 36 MHz receive but 25 MHz transmit (DS Table 41), so the full-duplex link
  must stay at or below 25 MHz. design.md §3.1 quotes 36 Mbit/s.
- **RP2354B stepping:** erratum E9 (the weak pull-down latches at about 2.2 V) affects A2 and is fixed from A3. The
  schematic doesn't rely on RP2350 pull-downs to read any input: KEY uses a pull-up and the IRQ lines are driven
  push-pull. So even A2 stock would be fine, but confirm that JLC ships A4 as design.md assumes.

## 4. Documentation does not match the schematic (Doc)

- **[hardware/kvm_schematic.pdf](hardware/kvm_schematic.pdf) is stale.** It is dated 2026-10-20 17:07 and still
  shows rev 4: the AT42QT1010 touch key and a WS2812B-2020. Re-export it with `kicad-cli sch export pdf`.
- **12 MHz crystals:** design.md §5 lists the YXC X322512MSB4SI (C9002, Basic). The generator uses the Abracon
  ABM8-272-T3 (C20625731), whose 10 pF load suits the 15 pF caps. Update §5, and re-check stock for C20625731: the
  2026-10-20 stock check covered the YXC part.
- **AT32 hub port:** design.md (rev 5 notes, line 19, and the §5 note for U4–U6) says both AT32s are on port 4. The
  generator puts AT32 #2 on **Hub-2 port 1** ([gen_schematic.py:289](hardware/tools/gen_schematic.py#L289)), and the
  §1/§2 diagrams still show DS1/port 1 for both.
- **Shields:** design.md §11 (line 498) says each shield goes to GND through 1 MΩ ∥ 4.7 nF. The generator ties `SH`
  directly to GND on USB-C and USB-A ([:120](hardware/tools/gen_schematic.py#L120),
  [:322](hardware/tools/gen_schematic.py#L322)). Either choice works for a small desktop box, but the two should
  agree.
- **design.md §5 BOM table:**
  - U23 and U1501 both list the 74AHCT1G125, but only one is fitted.
  - The references U1–U24 predate the per-block numbering (U501, U1202, …).
  - The buck inductor is missing: L401, 2.2 µH Sunlord SWPA3015S2R2MT, C43389.
- **design.md §3.1:** the SPI speed is 36 MHz receive but 25 MHz transmit (finding 14).
- **design.md §6:** the 0.3 A budget (finding 6).
- **Schematic title block:** it still says rev 1, 2026-10-20.

## 5. Verified correct

| Block | Checked against | Result |
|---|---|---|
| AT32F405CCT7 symbol (LQFP-48) | DS v2.03 Figure 3, Table 9 | all 48 pins match: VDD 1/28/48, VDDA 9, VSSA 8, VSS 23/47, OTGHS1_R/D−/D+ 25/26/27, BOOT0 44, NRST 7, SWD 34/37 |
| AT32 OTGHS1 | DS §2.13.6, Table 44 | 12 kΩ ±1 % to GND (spec 11.88–12.12 kΩ); 12 MHz HEXT; PB13 (OTGHS1_VBUS) left open, and the firmware sets VBUSIG |
| AT32 boot | DS §2.5, Table 5; RM nBOOT1 | SPI1 bootloader on PA4–PA7 (CS/SCK/MISO/MOSI) for all packages; BOOT0 has 10 kΩ external and a permanent internal pull-down; NRST 100 nF as Figure 19 |
| AT32 ADC and strap | DS Table 9 | PA0 = ADC1_IN0, PA1 = IN1, PB0 = IN8; PC13 used only as an input strap (current-limited domain) |
| AT32 decoupling | DS Figure 9 | 100 nF per VDD/VSS pair, 4.7 µF bulk, VDDA 100 nF + 1 µF |
| RP2354B pins | rp2350 DS §1.2.1.2, function table | GPIO numbers ↔ QFN-80 pins match; SPI0 RX/CSn/SCK/TX = GPIO16/17/18/19; SPI1 = GPIO44/45/46/47; ADC0–2 = GPIO40–42 |
| RP2354B supplies | rp2350 DS §6.1.3, Figure 24, §14.9.5 | external DVDD: VREG_FB and VREG_PGND to GND, VREG_LX open, VREG_VIN + VREG_AVDD fed through 33 Ω / 4.7 µF, 100 nF per DVDD pin; QSPI_IOVDD at 3.3 V (required for RP2354); TLV73311 gives 1.085–1.115 V against the 1.05–1.16 V range |
| RP2354B flash, boot, clock, USB | RP2354 §14.3, hardware design guide | QSPI pins left open (in-package flash); BOOTSEL via 1 kΩ on QSPI_SS; 12 MHz ABM8 + 1 kΩ + 15 pF; 27 Ω USB series |
| USB2514B (×3) | DS00001692C Table 3-2, 3-3, 5-1, §7.1 | pinout; CFG_SEL = 00 (straps, self-powered); RBIAS 12 kΩ 1 %; CRFILT/PLLFILT 100 nF ("up to 0.1 µF"); VBUS_DET by 2 × 100 kΩ divider (exactly the datasheet's suggestion) or a host GPIO (Hub-0); port 4 disable on Hub-0 by 10 kΩ pull-ups on D+/D−; NON_REM straps 100 kΩ (47–100 kΩ allowed); TEST to GND; internal 1 MΩ feedback, so no external resistor |
| TS3USB3031 (×3) | ts3usb3031.pdf Table 4-1, Table 6-1, §5.3 | pinout; SEL1/SEL0 = **10 = MHL = Hub-0** (the KVM path, whose data is full speed anyway, so the channel TI specifies only at 1.5-3.3 V levels carries the harmless link), 01 = USB2 = Hub-1, 00 = USB1 = Hub-2, 11 = off (rev 5 had MHL = Hub-1, USB2 = Hub-0); VCC 3.3 V; the 0.8 ms buck soft start meets the 100–1000 µs/V VCC ramp |
| TS3USB30E | ts3usb30e.pdf Table 4-1, Table 7-1 | VSSOP-10 pinout; S = L → D1 (service port), OE to GND; TSSOP-10_3x3mm footprint is TI's DGS land pattern |
| TPS2121 | tps2121.pdf pin table, Table 9-3, Eq. 2 | pinout; PR1 priority above ≈ 3.4 V; ILM ≈ 4.6 A; ST high = IN1 (the firmware reads it that way) |
| LM66200 | lm66200.pdf Table 5-1, §7.4 | pinout; ON to GND (always on); 8 µs changeover without soft start, so losing one PC's VBUS is harmless |
| TPS2553 (×3) | tps2553.pdf pin table, Eq. 1 | pinout (TPS2553 = EN active high); 22 kΩ gives 1.09–1.26 A; EN 100 kΩ pull-down; FAULT 10 kΩ pull-up |
| TLV62569 | tlv62569.pdf pin table, Eq. 2, Table 4 | pinout; FB 100 k/22 k gives 3.33 V; 2.2 µH + 2 × 22 µF is a proven combination for VOUT ≥ 1.8 V |
| TLV73311, 74AHCT1G125 | pin tables | pinouts; EN/OE strapped correctly; AHCT accepts 3.3 V input at 5 V VCC |
| USBLC6-2SC6, PESD5V0S2BT | datasheet pinouts | flow-through I/O 1/6 and 3/4, VBUS 5, GND 2; bidirectional TVS, common pin to GND |
| USB-C (×3), USB-A (×3) | symbol vs. footprint pads | D+/D− rows tied; 5.1 kΩ Rd per CC pin (sink); SBU open; footprints have `SH` pads |
| SK6812MINI-E, SK6812SIDE-A | sk6812mini-e.pdf §4–6, sk6812side-a.pdf §5 | pin numbers match the symbols; 5 V supply in the 3.7–5.5 V range; reverse-mount footprint correct when flipped to B.Cu |
| Hub port ↔ router wiring | netlist | every front port reaches Hub-0, Hub-1 and Hub-2 on the intended TS3USB3031 path; each AT32 reaches its own hub; no single-pin nets |

## 6. Suggested order of work

1. **Firmware:** fix the Mouse/Other GPIO map (finding 1). It costs nothing and would otherwise make the first board
   look broken.
2. **Power path:** in one edit to block 4 of the generator, rework the TPS2121.
   - XCOMP dividers on PR1/CP2 (finding 2).
   - An OV1/OV2 divider (finding 3).
   - R_ILM (finding 4) and CSS (finding 13).
   - +5 V hold-up, and VBUS TVS diodes at the connectors.
3. **Front ports:** larger VBUS bulk (finding 5) and 10 kΩ on SEL0 (finding 10).
4. **Small additions:**
   - 10 kΩ series resistors on the CC ADC inputs (finding 7);
   - NRST pull-ups (finding 9);
   - an RP2354B GPIO on HUBn_RST (finding 11);
   - 18 pF hub crystal caps (finding 12);
   - the LED chain order or a bypass jumper (finding 8);
   - an LED data resistor and a UART pad (finding 14).
5. **Documentation:** re-export the PDF and align design.md (§4).
6. **On the prototype:**
   - pull the PWR cable under load;
   - measure the 5 V input current;
   - run the HS passthrough eye/`fio` test on the PC1 (MHL) path.

## 7. Rev 6 + rev 7 verification (MHL on Hub-0, routing and SI pass) - 2026-10-24

Rev 6 moves the MHL channel to Hub-0 (the KVM path, whose data is full speed anyway) and USB2 to Hub-1, so both PCs
keep the datasheet-specified USB1/USB2 channels: generator, straps (`SEL0` 100 k to GND, `SEL1` 10 k to +3V3, so
`SEL = 10` = MHL is the power-up default), firmware (`board.rs` `FrontPort::route`: Kvm = 1,0 = MHL = Hub-0; Pc1 =
0,1 = USB2 = Hub-1; Pc2 = 0,0 = USB1 = Hub-2) and the datasheet (TS3USB3031 Table 6-1) agree.  The mux pad ends on the
placed board are pins 3/4 (USB1, north), 5/6 (USB2, north), 7/8 (MHL, west), and the router's hand-planned lanes were
re-planned for those ends (tools/route.py).

Checks on the routed board (run of 2026-10-24, `unrouted: 0 nets`, 3000 segments / 1107 vias):

| Check | Result |
|---|---|
| `kicad-cli sch erc` | 0 violations |
| netlist vs generator (`check_netlist.py`) | OK, 950 pins / 264 nets |
| `kicad-cli pcb drc --schematic-parity` | **0 DRC violations**; 1 unconnected item; 2 footprint warnings; 199 `net_conflict` warnings |
| `check_spacing.py` / `check_aux.py` | no spacing problems / every auxiliary part on the nearer pin's side |
| `check_copper.py` | 0 clearance, 0 stubs, 0 orphans; 1 net in two pieces; 1 bad pair |
| `check_usb_si.py` | 18 links; all six rev-6 router lanes <= 3.4 ps skew, 100 %+ coupled |

**The four items, read together:**

- **1 unconnected item, +3V3 in two pieces and 1 bad pair are all the same defect**: a dangling *stitching via* at
  (158.95, 130.85) beside `C501.1` - the +3V3 via touches neither the pad nor a track, so the DRC sees an unconnected
  pad and `check_copper` sees +3V3 in two pieces.  Fix in `route.py`: `stitching()` should only place a via that lands
  on existing copper of its net (or `cleanup_dangling()` should drop a via with no adjacent copper).
- **199 `net_conflict` warnings** are a name-format artifact: `Pad net (VBUS_PC1) doesn't match net given by schematic
  (/VBUS_PC1)` - the generator writes bare net names, and the parity comparison reads them with a leading `/`.
  Systematic across the board, not electrical.
- **2 footprint warnings**: `Exclude from bill of materials` differs for `SW1501` (MX socket) and `D1501` (key LED) -
  the two parts `make_fab.py` sends to `bom/hand_assembly.csv` anyway.
- **`H0_KB` is the one plane gap worth watching**: 5.30 mm of its F.Cu copper have no In1.Cu plane under them (worst of
  all pairs; next `H2_KB` 1.64 mm, `RP` 1.39 mm).  Hub-0 is the full-speed KVM path, so this is a bring-up watch item
  rather than a blocker.

**Data sheets, re-checked for rev 6:** TS3USB3031 Table 6-1 (SEL truth table, above); USB2514B DS00001692C - CFG_SEL =
00 straps, self-powered, RBIAS 12 k 1 %, port 4 disabled by 10 k pull-ups on D+/D- (so `H0_D4` is a stub pair by
intent), VBUS_DET divider, NON_REM straps; RP2354B in-package flash / BOOTSEL / 27 ohm USB series; TPS2553 ILIM 22 k
= 1.09-1.26 A; TLV73311 = the 1.1 V core rail.  All match the generator.

**USB SI, link by link** (the three FAILs are pre-existing, not rev-6 regressions):

| link | skew | note |
|---|---|---|
| H1_KB / H1_MS / H1_OT (HS, PC1) | 0.0 / 0.0 / 0.1 ps | 100 %+ coupled, paired vias |
| H2_KB / H2_MS / H2_OT (HS, PC2) | 0.3 / 0.0 / 0.0 ps | the HS channels the passthroughs keep |
| PC1 / PC2 | 0.8 ps | connector -> ESD -> hub |
| H0_KB / H0_MS / H0_OT | 2.9 / 3.4 / 3.4 ps | full-speed path, inside the HS budget anyway |
| H0_UP | 0.6 ps, 103 % coupled, 9 % split, 0 vias | hub-0 upstream on the service port, a real pair now (18.73 / 18.83 mm) |
| **RP** / **SVC** | 41 / 49 ps | the RP2354B's own USB (full speed by design) and the service-port link |
| **H0_D4** | 15.8 ps | Hub-0 port 4, disabled by strap: a stub pair by intent |

`check_usb_si.py` budgets skew at 0.75 mm = 5 ps (tpd 6.7 ps/mm), the USB 2.0 high-speed guidance usually quoted - so a
pass here *is* a spec pass.  The six rev-6 lanes (0.0-3.4 ps) and `H0_UP` (0.6 ps) pass; `H2_AT` (6.7 ps) and the
strapped-off `H0_D4` (15.8 ps) do not.

**Firmware cross-check** (`firmware/rp2354/src/board.rs` against the generator's RP2354B pin map): GPIO 20/21/22/23 =
`KB_SEL0/SEL1/EN/FLT`, 24/25/26/27 = MS likewise, 34/37/32/33 = `OT_SEL0/SEL1/EN/FLT`, 2/3 = `HUB0_RST/HUB0_VBUS`,
6/35 = `HUB1_RST/HUB2_RST`, 0 = `SVC_SEL` (0 = service port, 1 = Hub-0), 1 = `PWR_ST`, 15 = `LED_DATA`, 11 = `KEY`,
40/41/42 = `VSNS_PWR/RP_CC1/RP_CC2`, 36 = `DBG_TX` - every one matches, and the SEL values the firmware writes match
the rev-6 truth table above.


### Rev 7 (routing and SI pass, 2026-10-24)

- **H0_KB routes before any Hub-1 lane**: 74.79 / 75.27 mm, skew 0.48 mm (was 120.40 / 120.84) - its long diagonal no
  longer has to weave around Hub-1's lanes.
- **The twist is symmetric**: P now takes the same lateral excursion as N's via hop, so both lines leave a twist with
  equal length.  H1_AT measured **0.00 mm** of skew (was 1.17 mm = 7.9 ps) - on a link short enough that the twist was
  most of its skew.
- **In1.Cu is reserved under the USB pairs and the AT SPI**: `legal_via()` refuses a via whose clearance hole would land
  under the *body* of such a run (allowed at its ends).  Before this, H0_KB had 5.30 mm of F.Cu copper with no plane
  under it (H2_KB 1.64 mm, RP 1.39 mm).
- **Dangling vias are cleaned**: `cleanup_dangling_vias()` connects a via to the nearest same-net pad with a stub where
  that is legal and drops it otherwise - the measured defect was a +3V3 via 1.28 mm from C501.1, which left +3V3 in two
  pieces and was the board's one unconnected item.
- **The SI budgets are the spec's**: `check_usb_si.py` and `check_copper.py` now use 0.75 mm (5 ps at 6.7 ps/mm) for
  intra-pair skew instead of 1.5 mm.  At that budget the six rev-6 router lanes still pass (0.0-3.4 ps), H1_AT passes
  (0.00 mm) and H0_UP (0.09 mm) do; the service-port link SVC, RP, H2_AT (0.99 mm) and the strapped-off H0_D4
  (2.36 mm) do not.
- **The service switch U601 moved top-left of Hub-0** (155.8, 117.0 from 171.0, 123.0), into the free space between the
  service connector's ESD and the hub: the service link and Hub-0's upstream pair are both short now.
- **H0_UP is a pair now, and why it was not is the sharpest lesson in this file.**  Its pads sit on opposite rows of the
  TS3USB30E, so `svc_breakout()` lays a fixed-copper breakout there (inward from each pad, east past the pad field, both
  legs equal) and offers the link one pair terminal at that exit.  With that terminal the leg search *still* failed at
  "leg 0" with the centre line provably clear - and the cause was not copper at all: `Board.__init__` implements the pair
  machinery by giving **both lines of a pair one net code**, and `Field.foreign()` answers with the *second*-nearest
  copper wherever the nearest carries one's own code.  `B.pair_of` does not do that (it only drives layer choice,
  rip-up and the pair phase), so a link has to be in `pair_nets` as well.  H0_UP was missing from that list: its fat
  centre line demanded CLR + PAIR_FAT/2 = 0.375 mm of clearance from a line that is only PAIR_O = 0.15 mm away at the
  fan-out, and no lane planning could answer that.  Measured after the fix: 18.73 / 18.83 mm, skew 0.09 mm (0.6 ps),
  103 % coupled, 9 % split, 0 vias - against 33.9 ps and 63 % split as two single nets.
- **That pair copper costs Hub-1's long lanes their answer.**  With H0_UP in the pair stage, `H1_MS_DP/DM`,
  `H1_OT_DP/DM` and `SVC_DP` end unrouted (5 nets; 6 unconnected items, 3 clearance and 3 dangling-via findings).  In
  the run before this change those same links routed as pairs on a later retry - `H1_MS` by an 83.18 mm detour - so the
  space H0_UP now uses is what they were using.  `H1_MS` (waypoint 5 at (103.5, 130.4)), `H1_OT` ((108.5, 150.8)) and
  `SVC_DP` need their lanes re-planned around the new copper; until that is done this board is *not* the one to keep.
- **The 199 parity warnings are all the `/`-prefix artifact**: verified 199 of 199 read `Pad net (X) doesn't match net
  given by schematic (/X)`, so the parity check found no real mismatch.  The board's bare net names are the
  generator/router contract, so the names stay and this stays documented.
- **The two BOM warnings are matched** (`FP_EXCLUDE_FROM_BOM` on the hand-assembly footprints).

## 8. Rev 9 verification (clean full run, ESD necks, budgets) - 2026-10-26

Full run of `hardware/tools/route.py --live` (506 s): **`unrouted: 0 nets []`**, 27/27 pairs routed, 3072 segments,
1117 vias.  `check_spacing.py` passes, `check_usb_si.py` passes, `check_copper.py` reports 0 clearance items, 0 stubs,
0 doubled copper, 0 bad pairs - and **one net in more than one piece (+3V3), the single open defect below**.
`check_aux.py` exits 1 on the six SEL-strap resistors (R1201/R1202/R1301/R1302/R1401/R1402 sit on the strap rail, not
beside the pin) on this board and on the committed one alike.

- **The three input ESDs' escape necks are the same as the output ports'**: 1.26 mm of straight run out of the pair's
  outer pad at U101, U201, U301, U1201, U1301 and U1401 (measured on the routed board; 2.28 mm of pad-to-pad track
  through each ESD's body on all six).  Two things were wrong: the pair's freedom window was a flat 2.0 mm, so any
  straight run over 40 cells was out of bounds and silently fell back to 0.2 mm, and the input ESDs' 4/6 pair - which
  straddles pin 5 (VBUS) rather than pin 2 (GND) - never got the 0.5 mm via-room the output ports get (`ESD_NECK`).
  Before: 0.76 mm below against 1.26 mm above.
- **The ESDs' middle pins escape north into the pad-row slot** (`PAD_ESCAPE_INWARD` for U101.5/U201.5 as well as
  U301.5), which is what lets VBUS_PWR out at U301.5 and leaves the column below each ESD free for that neck.
- **The SI budget is 1.2 mm = 8 ps** (`SKEW_MAX`; 0.4 % of a 480 Mb/s bit, ~6x tighter than the ~50 ps HS app notes
  allow).  At 8 ps every pair passes: PC1/PC2 0.12 mm, KB/MS/OT 0.00 mm, RP 0.50 mm, SVC 0.88 mm (5.9 ps), H2_AT
  0.99 mm (6.6 ps).  The 5 ps number of rev 7 was the spec's, not this board's.
- **H0_D4 is a documented exception, not a violation**: it is Hub-0's disabled port (D4 strapped) and its pair pinches
  to 0.1346 mm (DRC, at (163.60, 125.75), 5.3 mil - fabricable) where the port-4 via row meets its corridor.
  `check_copper.py` prints it under "accepted exceptions" and never counts it, so the live pairs keep 0.15 mm.

### Open defect: C501.1 (+3V3) is unconnected - the +3V3 net is in two pieces

- `C501` is the 100 nF decoupling cap at (159.05, 132.60); **pad 1 (+3V3) at (159.05, 132.12)**, F.Cu SMD, 0.56 x 0.62
  mm.  The only `+3V3` zone is on **In2.Cu** (`GND` owns F.Cu/B.Cu/In1.Cu), so an F.Cu SMD pad needs its own via.
  The nearest `+3V3` copper of any kind is the via at (158.40, 130.85), **1.43 mm** away, with nothing in between.
- **The plane itself is sound** - the break is not a cut plane: `in2_islands()`'s last pass reports `In2.Cu: 168 track
  segments, +3V3 plane regions 47, connections off the main region: 0` (the In2 track routing does cut the plane into 47
  regions, and `in2_repair()` re-routed the two early +3V3 vias at (153.25, 121.00) and (150.75, 123.85) that landed off
  the main one).  What is missing is the single pad-to-plane link at C501.1: a via is the only thing that can reach In2.Cu
  from an F.Cu pad, and it was never placed.
- Both the DRC and the router say so: `kicad-cli pcb drc` lists it as the board's one `[unconnected_items]` event
  (`Via [+3V3] on F.Cu - B.Cu` at (158.40, 130.85) / `Pad 1 [+3V3] of C501 on F.Cu` at (159.05, 132.12)), and the route
  log reads `+3V3 C501.1: no via` then `+3V3 C501.1: blocked by a pair/plane net ['AT2_CS', 'GND']` - `plane_fix()`
  refuses to rip because **GND** is among the blockers.
- Obstacles measured around the pad: `AT2_CS` on F.Cu at 0.62 mm, a GND track at 0.84 mm, C501.2's own GND pad at
  0.96 mm, a GND via at 1.05 mm.  Evaluated properly - clearance to copper *edges* on every layer, 0.45 mm of free ring
  for a 0.6 mm via - the pad sits in a **sealed pocket**: no legal 0.6 mm *or* 0.45 mm via site exists within 0.8 mm
  (the whole ring is inside the 0.45 mm clearance band), and no 0.15 mm track can escape either.  `AT2_CS` wraps the pad
  on three sides (it passes at y 131.50 north, then (158.05, 132.25)-(158.45, 131.85)-(158.70, 131.50) west and
  (160.05, 132.10)-(162.40, 132.10) east), the GND stitching via at (159.75, 132.90) plus its 0.4 mm stub close the
  south-east, and C501.2's GND pad closes the south.  The east channel between `AT2_CS`'s edge (y 132.175) and that GND
  via's edge (y 132.60) is narrower than a 0.15 mm track plus its two clearances (measured: a candidate track to the
  +3V3 via at (160.55, 133.25) leaves 0.10 mm to the GND via, needs 0.15).
- **Why it is a decoupling cap and not a dead-net cosmetic**: with pad 1 floating, C501 does not decouple anything and
  the board carries a real DRC error.  It must be closed before the board is released.
- **Rev 7 had this same pad and fixed it the cheap way**: then the nearest +3V3 via was 1.28 mm away and
  `cleanup_dangling_vias()` could join it with a stub.  Now the via is 1.43 mm away, the corridors are closed by
  `AT2_CS` and the GND stitching, so that rescue has nothing to work with.
- **Fixes that would work (none implemented - the board is kept as committed, decided 2026-10-26)**:
  1. relocate the GND stitching via at (159.75, 132.90) and its stub by about 0.6 mm - stitching vias are free to move,
     and that alone opens the east channel and a 0.45 mm via site;
  2. move C501 by about 0.5 mm (touches placement);
  3. let the plane rescue rip *signal* copper only, i.e. drop GND from the veto in `plane_fix()` when the GND item is a
     track/stub that the GND pour also covers - `AT2_CS` then takes a small detour and a full-size via fits;
  4. a via in the pad (needs filled/plated-over vias from the fab).
  The cheapest correct one is (1): it costs no part move and no rule relaxation, only a placement change in the GND via
  fan-out.

## 9. Rev 10: Hub-1 USB lanes, router fixes, every DRC item closed - 2026-10-27

Final full run of `hardware/tools/route.py --live` (429 s): **`unrouted: 0 nets []`**, 27/27 pairs routed in the first
pass, 2962 segments, 1076 vias.  `kicad-cli pcb drc --schematic-parity`: **0 violations, 0 unconnected items, 0
schematic-parity issues** (the committed rev-9 board, DRC'd with its own project file: 3 clearance errors, 1
unconnected item, 200 parity issues).  `check_copper.py` PASS, `check_spacing.py` no problems, `check_aux.py` exit 0,
`check_usb_si.py` 0 hard failures.  Live-view snapshot: `hardware/debug/rev10-usb-relane/`.

- **Hub-1's Mouse pair is direct.**  H1_MS (U1302 pins 5/6 -> Hub-1 pins 4/3) used to go round the west edge (82.98 mm,
  routed only on the retry pass after failing all six first-pass orders against H1_KB at the keyboard router).  It
  now runs F.Cu west along y 145.95, changes layer once south of AT32 #1's SPI pins and runs B.Cu under AT32 #1's east
  pin row and under H1_AT into a via pair below pins 4/3: **51.13 mm per line, skew 0.00, 2 via pairs**.  H1_OT keeps
  north (B.Cu at y 124.3 under the hub row, one V under H1_AT into a via pair at (117.75, 126.75)): 95.13 mm, skew 0.00.
- **`fan_keep` never recognised a net's own chips** (`route.py` Board: pad keys are `REF.PIN`, the test compared the
  bare `REF`).  Every net paid FAN_COST in its own chips' fan-out bands, which is why AT1/AT2's SPI buses (and later
  KB_SEL0/1, AT1_BOOT0) failed with "no way through at all" on an open board: AT2_CS found no path with both of its
  chips' bands on, and routed at once (718 cells) with either one lifted.  Both buses now route in their first order.
- **The fan-out's fallback stubs keep off later pins' planned ways out**, and are shortest paths (h_weight 1.0).  The
  bug above had made them near-optimal by accident; fixed, U801.23 / U901.23 (+3V3) lost their via sites to a
  neighbour's stub jogging at the pin tips.  Now U801/U901 give 10 signal + 4 plane pins a site each (pin 27 keeps a
  lane), U701.34 gets its site; AT1_XO (U1001.6, a crystal line that needs no via) is the one signal pin without one.
- **Plane pads in the SPI buses' way get their via before the buses** (step 3's "plane vias first" never covered them,
  the buses route inside the pair phase).  That closes **section 8's open defect: C501.1, now C511.1, has its via.**
- **Hub GND pin 11 clearance (0.1346 mm, all three hubs, also in rev 9)**: `cleanup_dangling_vias()` took the
  exposed-pad vias for dangling (it only looked at segments) and stubbed the corner one onto pin 11 with a 0.3 mm
  track.  A via standing in its own net's pad is connected now, and stubs are never wider than the pad they join.
  (Section 8 put this DRC figure on H0_D4 at (163.60, 125.75); the committed board's DRC, run with its own
  `kvm.kicad_dru`, has exactly three clearance items, and they are these tracks - H0_D4's gap is 0.150 mm.)
- **USB-C GND vias on the shield pads ("via_dangling", J301 and J101)**: plane vias are kept off and out of the
  thermal relief of a same-net through-hole pad, and `plane_fanout()` joins a GND pin to that pad instead.
- **Per-pin cap ties: 16 of 16** (C1003/C1103 at the AT32s' pin 48 found no site: C1001's via took the package
  corner; the site search now also slides along the 1.9 mm gap).
- **Parity**: the schematic's net labels are global labels, so its nets carry no `/` prefix and match the board.
- **`check_copper.py`'s plane-gap measurement was wrong**: it multiplied a run of misses by "first-to-last distance /
  sample count" over all of a pair's segments.  H0_KB read 4.02 mm; the real uncovered stretches are <= 0.40 mm, each
  at the pair's own via pair.  It now samples every 0.05 mm along each segment and leaves the pair's own via holes out.
- **Assembly: everything at JLC, both sides (decided 2026-10-27).**  D1501 (SK6812MINI-E, C5149201) and SW1501's hot-swap
  socket (HanElectricity CPG151101S11-2, C49352235, paste added) are in the BOM; `make_fab.py` writes Bottom rows
  (18: 16 per-pin caps, D1501, SW1501) and the B.Paste gerber.  The bottom transform is derived (KiCad flips left/right
  and reports 180 degrees; JLC looks from below, as kicad-jlcpcb-tools does) and **not yet confirmed by an order: check
  D1501 and SW1501 in JLC's placement preview**.  The socket's EasyEDA land pattern is 0.51 mm wider than ours (rms
  0.26 mm per tab on 2.55 mm pads).
- **check_aux's six SEL straps are accepted**: DC straps on MCU-driven lines; the three output macros are placed as
  copies and east of the Other switch is Hub-2's Other lane, so the shared nearest free spot is 6-9 mm west.

## 10. Order readiness (rev 10 board) - 2026-10-27

**Verdict: the board itself is ready to fabricate; a fully assembled JLC order is blocked by RP2354B stock, and
three settings/decisions remain (below).**

| Check (final board, all run 2026-10-27) | Result |
|---|---|
| `kicad-cli pcb drc --schematic-parity --severity-all --all-track-errors --refill-zones` | 0 violations, 0 unconnected, 0 parity (warnings included) |
| `kicad-cli sch erc --severity-all` / `check_netlist.py` | 0 / OK (962 pins, 267 nets) |
| `check_copper.py` / `check_spacing.py` / `check_aux.py` | PASS / no problems / no part on the far side |
| `check_usb_si.py` | 0 hard failures; skew <= 0.99 mm (6.7 ps, budget 1.2 mm); HS links 94-105 % coupled, 0 % split, vias paired |
| F.Cu HS pair copper over In1.Cu (GND) / B.Cu over In2.Cu (+3V3) | 100 % / 100 % (own via holes excluded) |
| Differential impedance, 0.15/0.15 mm on JLC04161H-3313 (2D field solve) | **89.5 ohm** (12.7 um mask), 87.7-94.0 over mask 0-20 um, trapezoid etch, Er 3.9-4.3; USB 2.0: 90 ohm +-15 % |
| Common-mode impedance | 28.1-29.5 ohm (USB 2.0: 30 ohm +-30 %) |
| Geometry vs JLC 4-layer | tracks >= 0.15 mm, clearance 0.15, vias 0.6/0.3 (ring 0.15), no microvias: standard capability |
| Gerbers / drill vs board | 11 layers incl. B.Paste; PTH 1118 hits = 1076 vias + 18 holes + 12 slots; NPTH 24 = 24 |
| BOM / CPL | 40 lines, 243 parts, BOM = CPL = every LCSC footprint; 225 top / 18 bottom |

Stack-up used for the solve: L1/L4 0.035 mm Cu, 3313 prepreg 0.0994 mm (Er 4.1), core 1.265 mm, L2/L3 0.0152 mm (JLC
template data via gsuberland/jlcpcb_autogenerated_stackups; Er from JLC's capabilities page).  Check: a single
0.15 mm line solves to 53.4 ohm, close to the closed-form microstrip value.

**Blocker - RP2354B stock.**  JLC lists C39843328 (RP2354B) at 0 (queried 2026-10-27; AT32F405 132, TS3USB3031 82
- enough, but thin).  Options: JLC parts pre-order / global sourcing, consigning the chips, or waiting.  The in-stock
RP2350B (C42415655, 3316) is not a drop-in: it has no in-package flash, so it would need a QSPI flash chip and a
re-layout round the MCU.

**Decisions before ordering:**
- **Hub decoupling (review question)** - *done in rev 11, section 11.*  Every USB2514B VDD33 pin's nearest 100 nF is 5.1-8.1 mm away and reaches it
  only through In2.Cu, on all three hubs; the RP2354B's and AT32s' supply pins have a cap 1.6-2.1 mm away, mostly tied
  by direct copper.  The hubs are in `PIN_CAP_CHIPS`, but no spot fits: under the QFN-36 there is 0.5 mm between the
  exposed-pad via keep-out and the pins, and just outside the pins are the USB pairs' corridors (rev 9: a cap there
  blocked H1_OT's layer change).  Estimated loop ~3-4 nH vs ~1-1.5 nH for the tied caps (the GND/+3V3 planes are 1.265
  mm apart).  Workable for a prototype; the weakest decoupling on the board, on the 480 Mb/s chips.
- **Exposed-pad vias sit inside the paste windows** (U501, U701, U801, U901: 3x3 x 0.3 mm vias, each inside one of the
  footprint's 9 paste apertures; no room between the apertures).  They are tented on the bottom.  Either order
  "epoxy filled & capped" vias (POFV; extra charge on 4 layers) or accept some wicking/voiding under the four QFNs.
- **Bottom-side placement**: D1501 and SW1501 rows follow a derived convention (`make_fab.py` `jlc_place`); check both
  in JLC's placement preview.  The socket's EasyEDA land pattern is 0.51 mm wider than our pads (0.26 mm per tab).

**Notes, not blockers:**
- Return path at HS layer changes (the reference flips L2 GND <-> L3 +3V3): nearest GND/+3V3 cap 5.6-9.8 mm from
  H1_MS / H1_OT / H2_KB / H2_MS via pairs.  A 100 nF stitching cap beside each would be the textbook fix.
- Hub-0's B.Cu sections have up to 7.3 mm without In2.Cu under them; Hub-0 is full speed (design.md), so harmless.
- 24 of 40 BOM lines are extended parts (JLC loading fee each).

**Order settings:** 4 layers, 1.6 mm, JLC04161H-3313 with impedance control (90 ohm differential, 0.15/0.15 mm, L1 ref
L2 and L4 ref L3), 1 oz outer / 0.5 oz inner, ENIG (0.4 mm pitch QFN-80 and WQFN-12), vias per the decision above;
PCBA both sides, `hardware/kvm_gerbers.zip`, `hardware/bom/jlcpcb_bom.csv`, `hardware/bom/jlcpcb_cpl.csv`.  The MX
switch and keycap are plugged in afterwards.

**On the prototype** (section 6, still open): pull the PWR cable under load, measure the 5 V input current, and run the
HS passthrough test on every port.

## 11. Rev 11: the hubs' VDD33 caps on the bottom side - 2026-10-27

Each hub's 100 nF VDD33 caps now sit on B.Cu radially under their own pin (`gen_pcb.py` `HUB_PIN_CAPS`, a pass at the
end of placement, so no other part moved - 16 caps moved, nothing else): the +3V3 pad under the pin's outer end, the
GND pad inwards facing an exposed-pad via.  `route.py` `pin_cap_links` ties them pin -> 0.45 mm F.Cu past the pin tip
-> via -> 0.67 mm B.Cu -> cap pad (the via is where the hubs' +3V3 fan-out via sat before), and the GND pad gets a
0.50-0.71 mm B.Cu stub to an exposed-pad GND via - a loop of about 2 mm of copper per pin instead of 5.1-8.1 mm away
through In2.Cu.

| Hub | VDD33 pins with a cap under them | Note |
|---|---|---|
| Hub-1 (U801), Hub-2 (U901), 480 Mb/s | 6 of 6 | |
| Hub-0 (U701), full speed | 4 of 6 | pins 5 and 36 keep their ring caps C701 / C706 |

Hub-0's two exceptions: Hub-1's Other pair crosses under Hub-0's south pin row on B.Cu (y 124.95) and steps north
just west of it.  With all six caps there the first run failed that pair ("leg 7 -> (158.0, 123.6) on B.Cu | blocked
by ['+3V3', 'H0_RBIAS']"): C701 lay on the crossing, C706's tie via on the step, and the changed west fan-out moved
H0_RBIAS's via into the only way round.  Re-planning that pair for a full-speed hub's two pins was not worth it.

Full run (442 s): **`unrouted: 0 nets []`**, 27/27 pairs, 2923 segments, 1049 vias, `pin-cap ties: 32 pin(s)`.
DRC (all severities, all track errors, zones refilled, schematic parity) **0 / 0 / 0**; ERC 0; `check_netlist.py` OK;
`check_copper.py` PASS; `check_spacing.py` and `check_aux.py` clean; `check_usb_si.py` 0 hard failures with every USB
link's length and skew identical to rev 10; HS pairs 100 % over In1.Cu (F.Cu) and In2.Cu (B.Cu); no via in an SMD pad
other than the exposed-pad arrays.  Order files regenerated: BOM 40 lines, CPL 243 parts (209 top, 34 bottom), B.Paste
70 flashes.  Snapshot: `hardware/debug/rev11-hub-caps/`.  Section 10's other items stand (RP2354B stock, exposed-pad
vias / POFV, bottom-side placement check in JLC's preview).

## 12. Rev 12: power rails sized for each layer's copper, crystal review - 2026-10-28

Two findings of the order review, both fixed in the generators: the +5 V feed ran on the 0.5 oz inner layer at a width
meant for 1 oz outer copper (jlcpcb.md section 5), and the RP2354B's crystal was routed long, through vias and next to
other signals (raised in review).  The measuring tools are in `hardware/tools/` (`dc_drop.py`,
`xtal_report.py`).

### The +5 V feed

`route.py` sized every rail with IPC-2221's external chart for 1 oz copper (`rail_width`: +5V 0.6 mm for 2.24 A at a
20 C rise) and then ran it on In2.Cu at that width: 106 mm of +5V on 15.2 um copper.  Rev 12:

- **Per-layer widths** from the board's own stack-up (`RAIL_LW`; JLC04161H-3313: 35 um outside, 15.2 um inside): the
  heating rule for each layer's copper - the external chart on In2.Cu too, which sits between the planes and runs no
  hotter (IPC-2152) - so a rail on In2.Cu is 2.3 x its outer width.
- **A drop rule for +5V** (`RAIL_DROP`, 1 mV/mm at 2.24 A): 1.1 mm on F.Cu / B.Cu.
- **A cap** (`RAIL_W_MAX` 1.39 mm).  The first run gave +5V 2.53 mm on In2.Cu and DRC found 3 shorts: the router's
  distance fields are exact only within `RMAX` = 0.9 mm of copper, and that track needed 1.415 mm of clearance from
  its centre line, so it was laid over a KEY via and two GND vias 0.9-1.4 mm away.  Nothing wider than the fields can
  check is laid now.
- **The port ESDs belong to their own nets** on every layer (U101, U201, U301: their middle VBUS pad escapes through a
  via between the pad rows, `PAD_ESCAPE_INWARD`).  With 1.15 mm VBUS rails on In2.Cu that via had no In2.Cu exit
  (1.03 mm needed from pad 2's GND via, 0.81 mm there), and in three runs out of three VBUS_PC1, then HUB0_RST took the
  B.Cu under U301 and VBUS_PWR stayed unrouted at U301.5.

From `dc_drop.py +5V U402.1,U402.8 ...` (U1203 / U1303 / U1403 Keyboard / Mouse / Other port switches, U403 the buck):

| | rev 11 | rev 12 |
|---|---|---|
| USB-rated load (3 x 0.5 A + buck 0.5 A): drop to the port switches | 206 / 185 / 148 mV | **91 / 82 / 59 mV** |
| typical load (0.1 / 0.1 / 0.5 A + buck 0.4 A) | 82 / 78 / 70 mV | 34 / 32 / 28 mV |
| +5V track on In2.Cu / its share of the loss (rated) | 106 mm / 83.5 % | 24 mm (1.39 mm wide) / 30.9 % |
| most loaded In2.Cu segment (rated) | 2.0 A, 219 A/mm2, ~61 K | 1.5 A, 71 A/mm2, ~8 K |
| port voltage at the rated load, 5.00 V adapter | 4.58-4.64 V | **4.70-4.73 V** |

The port voltage adds the adapter feed (VBUS_PWR, 47 mV at 2 A), the TPS2121 (2 A x 56 mOhm typ.), the TPS2553 (0.5 A x
85 mOhm typ.) and the port leg (7-9 mV).  At the full rated load it reaches USB's 4.75 V from a 5.05 V adapter; at the
typical load the ports sit at 4.83-4.87 V.  The hottest copper is the 0.45 mm long 0.5 mm neck at the TPS2121's OUT
pad (2.0 A, ~21 K by the external chart).  1 oz inner copper is no longer needed (on rev 11's routing it would have given 80-134 mV,
and it moves the pairs to 87 ohm; jlcpcb.md section 5).

### Crystals

**The circuits are right on all six** (datasheets and the crystals' JLC listings):

| Chip | Crystal | Load | Caps | Effective load |
|---|---|---|---|---|
| RP2354B | ABM8-272-T3, 12 MHz, ESR <= 50 ohm, C0 <= 3 pF | 10 pF | 2 x 15 pF + 1 kOhm on XOUT | Raspberry Pi's reference circuit, unchanged |
| AT32F405 x 2 | ABM8-272-T3 | 10 pF | 2 x 15 pF | 7.5 pF + 2-7 pF stray (AT32 datasheet) = 9.5-14.5 pF: tens of ppm at most; USB HS allows +-500 ppm |
| USB2514B x 3 | X322524MOB4SI, 24 MHz, ESR 50 ohm, +-10 ppm | 12 pF | 2 x 18 pF | Microchip's formula 2 x CL - (6 pF pin + board) = 16-17 pF: 18 pF pulls ~10 ppm against +-350 ppm |

The AT32s drive the ABM8 without a series resistor: even a full 3.3 V swing across 13 pF (CL + C0) at 12 MHz is about
65 uW against the crystal's 200 uW maximum drive level (ABM8-272-T3 datasheet), and the AT32's gain is controlled.

**Placement and routing** (`xtal_report.py`).  Raspberry Pi's own minimal design (`datasheets/rp2350-minimal-kicad`)
puts its crystal about 10 mm from the chip with an XIN track of 10.9 mm and R2 2.96 mm from XOUT, so Y501's 12.7-14.4 mm
was not the problem; its routing and the series resistor's place were:

| Crystal | rev 11 | rev 12 |
|---|---|---|
| Y501 (RP2354B) | XIN 32.6 mm, 3 vias via In2.Cu and B.Cu; XOUT side 22.5 mm, 3 vias; R502 at the crystal, 14 mm from pin 31; OT_SEL0 through its courtyard; 4.1 mm of foreign track (the SWD clock among it) within 0.5 mm away from the chip | XIN 15.5 mm, 2 vias (a B.Cu hop); R502 right below pin 31, its crystal side 22.0 mm on F.Cu without a via; courtyard clear; 0.1 mm alongside |
| Y1001, Y1101 (AT32s) | AT2_NRST through Y1101's courtyard; 1.7 mm alongside | courtyards clear; 0 mm alongside; lines 4.3-9.95 mm (AT2_XO 5.3 mm with 2 vias) |
| Y701 (Hub-0) | its upstream D+ through the courtyard, 4.7 mm alongside | courtyard clear; 1.3 mm alongside (that pair routes before any crystal line; 12 Mbit/s) |
| Y801, Y901 (Hub-1/2) | 7.5-9.9 mm on F.Cu, no vias | unchanged |

What changed (`route.py`): crystal lines (`XTAL_NETS`) stay off In2.Cu and a via costs them 5 mm of detour
(`XTAL_VIA_COST`); every other net, pairs included, keeps its copper out of a crystal's courtyard and 0.5 mm off a
crystal line (`XTAL_GUARD`, `Board.xtal_field`), except within 2.5 mm of the crystal's chip (`XTAL_PIN_ZONE`), where the
chip's other pins leave 0.4-0.5 mm from the crystal pins.  Without that zone, and with a 20 mm via cost, the first
crystal run left 7 nets unrouted (the hubs' PLLFILT, RP_SWCLK, MS_FLT, AT1_CC1, VBUS_PWR) and took AT2_XO on a 16.9 mm
detour.  `gen_pcb.py` moved R502 below pin 31 (pad 1, XOUT, north), as in Raspberry Pi's design.  What stays: Hub-1's
and Hub-2's crystal lines have their PC's High-Speed D+ 0.27 mm beside them for 2.2 mm at the hub, where the upstream
pins sit next to the crystal pins.

### Verification

Full run (874 s; rev 11 took 442 s): **`unrouted: 0 nets []`**, no `PAIR FAIL`, 3002 segments, 1059 vias.  DRC (all
severities, all track errors, zones refilled, schematic parity) **0 / 0 / 0**; `check_copper.py` PASS;
`check_spacing.py` and `check_aux.py` clean; `check_usb_si.py` 0 hard failures, every link's length and skew as in
rev 11 except H0_UP (21.95 mm, was 21.84: it now goes round Y701's courtyard).  F.Cu pairs over the In1.Cu GND plane: 17 at
100 %, KB / MS / OT at 95.8 % as in rev 11 (at the USB-A pins).  The schematic did not change (ERC not re-run).  Order
files regenerated: BOM unchanged (40 lines), CPL 243 parts with R502's row changed.  Snapshot:
`hardware/debug/rev12-rails-crystals/`.
