# OpenDynKVM: design (rev 5)

Implements [specs.md](specs.md). The KiCad project is generated from scripts in [hardware/](hardware/) (§14).

**Rev 5 changes** (layout):
- **Switch input: back to the MX key with an RGB LED**, replacing the touch key (AT42QT1010 and electrode removed).
  - It sits in the **centre of the board** (160, 140) for stability, with no other part within 2.25 mm of its 18 mm
    keycap.
  - It is **hand-fitted after JLC assembly**: a Kailh hot-swap socket on the bottom side, and an SK6812MINI-E
    reverse-mount LED on the bottom that shines up through a board cut-out into the switch's LED window. What to order
    is listed in §11.
- **Ports spread out and equally spaced:**
  - rear USB-C at a 45 mm pitch (x = 115 / 160 / 205);
  - front USB-A at a 35 mm pitch (x = 125 / 160 / 195).
- **New floor plan** around the key, built from identical local macros (§11):
  - one input macro per PC: USB-C, ESD, hub, AT32F405;
  - one output macro per front port.

  AT32 #1 sits on **port 4** of Hub-1 (the east end of its row, right of the chip) and AT32 #2 on **port 1** of
  Hub-2 (the west end, left of the chip). Only port 1 can be strapped non-removable, so only Hub-2 reports a
  compound device (report.md F5).
- **LED chain**, left to right along the front: PC1, Keyboard, Mouse, Other, PC2, then the key LED **last**, so a
  missing or damaged hand-fitted part cannot darken the others (analysis.md finding 8).
- **Hub-0 control lines** move to **GPIO2 / GPIO3** (RP2354B pins 79/80, the corner facing Hub-0), from GPIO32 / GPIO37
  (pins 40/46 on the far side). Both pairs are Digital IO (FT) in the IOVDD domain with a pull-down reset state and
  identical pad registers (RP2350 datasheet Table 1674, Tables 887/888 vs 917/922), and the bootrom does not use them,
  so the behaviour is unchanged. The lines simply no longer wind around the package.
- **Other-port control lines** move from GPIO28–31 (the middle of the RP2354B's south side) to **GPIO32/33/34/37**
  (pins 40/42/43/46, the south-east corner facing the Other port): EN, FAULT, SEL0, SEL1. Same pad type and reset
  state (Table 1674); the lines no longer cross the crystal and SWD lines that leave the south side eastwards. The SWD
  pads sit east of the crystal, so those lines end before the Other-port lines turn south.
- **Room for via fan-outs around the dense chips** (§14, router):
  - The RP2354B and its parts move 1 mm south, away from Hub-0. Its crystal moves east, clear of the east fan-out
    and 2.25 mm off the keycap, with its load caps and series resistor tight around it. The SWD pads move just beyond
    the crystal.
  - Hub passives move out:
    - the NR1/CFG0/CFG1 pull-down row 1.3 mm north;
    - the CRFILT cap 0.9 mm east;
    - PLLFILT/RBIAS 0.5 mm west;
    - the hub crystals 0.3 mm west.
  - Auto-placed parts keep 2.5 mm from the hubs and AT32s (was 1.3 / 1.5 mm).
  - The AT32 crystals move 1.3 mm further from their pin rows.
  - The service switch (U601) moves 0.5 mm east.
  - The decoupling caps end up about 1 mm further from their pins, which is negligible at these frequencies. The
    RP2354B's DVDD (+1V1) caps stay just outside its fan-out, 6–10 mm from their pins (the core supply comes from
    the external LDO; the Raspberry Pi design guide accepts such compromises for escape room). Closer would need
    bottom-side caps.
  - PC1 / PC2's upstream pair (ESD → hub) now runs west just below the ESD and then south, clear of the hub's
    north-west corner. That corner takes the hub's NR0 strap resistor, beside its pin.
- **Passives pinned next to their pins** (auto-placement had put them across a bus or a lane):
  - each crystal's load caps beside its pads (hubs and AT32s; they were 6–17 mm away);
  - the RP2354B's RUN cap and pull-up under the SWD pads' RUN row;
  - each AT32's NRST cap and pull-up off its SPI side, and AT32 #2's ROLE strap where AT32 #1's lands;
  - Hub-0's unused-port-4 resistors at the hub's south-east corner.

**Rev 6 changes** (crowding and routing rework):
- **Auxiliary placement is checked, not assumed:** `gen_pcb.py`'s search now orders candidate spots by their *distance
  from the anchor pin* (it used to try the far side of the chip at a shorter search depth than the near side), only
  lets a part turn towards a second connection that is genuinely local (every pad on GND used to point at the board
  centre), and picks the rotation that brings the part's pad on the anchor's net *closest to that pin* — it used to
  rotate parts on the two adjacent sides by the same amount, which left many resistors connected to their chip by the
  far pad. Parts that must sit on a specific pin's side (the AT32 BOOT0 pull-downs, AT32 #1's ROLE strap, Hub-1's reset
  RC, the VBUS → VDET divider) are placed before that chip's decoupling caps, which would otherwise fill the ring and
  push them across the package. `tools/check_aux.py` reports any that still sit on the far side of their pin.
- **Escape rings cover the via fan-out:** auto-placed parts now keep 4.8 mm from the RP2354B, 3.8 mm from the hubs,
  3.6 mm from the AT32s and 1.3 mm from the three port switches (less on their crystal sides). The old 2.5 mm let a
  decoupling cap land inside the staggered via fan-out, which left those chips' nets trapped.
  - **Fan-out for the parts that had none:** the three TS3USB3031 port switches (0.4 mm pitch, WQFN-12) now get a
    staggered via fan-out for their SEL0/SEL1 pins, and every pad of the LEDs, their caps, the USB-C / USB-A
    connectors, the VBUS switches and the power path gets a reserved escape run plus via site in step 1 (before the
    USB pairs are routed). Without it the HS pairs closed those pads off — that is what left the LED chain, the CC
    lines and `KB_EN`/`KB_FLT`/`MS_SEL1` unrouted.
  - The front macro's `KEEP_FREE` now covers the strip between its switch and the ESD, so no auto-placed part lands
    in the USB-A → ESD pair's escape again.
  - **The small nets get one too (rev 9):** every pad of a net with at most four pads on the auto-placed parts (R, C,
    D, small SOTs) gets a reserved run plus via site, pointing at the net's *nearest other pad* — the strap
    pull-downs, the sense dividers and the pull-ups each have exactly one corridor, and whichever net routed first
    used to take it (that is what left H0_CFG1, VSNS_PWR and PWR_ST's corridor unrouted). Nets with more pads are left
    out on purpose: their pads are the decoupling caps in the dense ICs' fan-out bands, where a reserved lane takes
    the via a chip pin needs (measured: five of the RP2354B's 43).
- **Row 2 moves left, not down.** The RP2354B and AT32 #1 each move 12 mm left (AT32 #1 to x = 114, i.e. directly
  below Hub-1; the RP2354B to x = 148), which frees the middle of the board for the PWR/SERVICE block and the power
  path, and gives AT32 #2's SPI and the RP2354B's east side room. Moving the row *down* was rejected: the Hub-2 lane
  band above the key had 0.2 mm to spare, so 1 mm down puts the RP2354B's south fan-out, crystal and RUN parts and
  AT32 #1's crystal into it.
  - AT32 #1 below Hub-1 puts its HS link right at the hub's port row, so Hub-0's three lanes and the RP2354B's Mouse
    lane were re-planned around it (see `H0_LANES`), and the Hub-1 lane turn moved west of AT32 #1's crystal.
  - Its link to Hub-1 is now a short twist at its own pins. The twist is **not** avoidable in copper: the AT32F405's
    `OTGHS1_D+` (pin 27) sits west of `OTGHS1_D−` (pin 26) when the part is rotated so its OTGHS faces the hub, while
    every USB2514B downstream row has D− west of D+ (both datasheets). The hub's Port-Swap register (FAh, PRTSP)
    exists for exactly this, but it is only writable over I²C/SMBus and the hubs are strap-configured.
- **The power path and service switch move 2 mm down**, so the rear USB-C pad rows and their VBUS / CC lines keep the
  strip along the rear edge.
- **Y701 moves up and left** (to the hub's north-west), and the PLLFILT / RBIAS parts sit at the hub's west edge,
  beside pins 34/35 (they are the only parts of the input macro whose pins are all on that side).
- **Routing** (see §14): an edge keep that leaves the board edge to the parts mounted there, power rails sized from
  their current and never necked down except at a pad, and the status LED chain routed before power.

**Rev 4 changes** (build):
- **Switch input:** a top-mounted **capacitive touch key** (Microchip AT42QT1010 + a 14 mm electrode under the lid)
  replaces the MX key. The board is an ordinary components-up board again.
- **Status LEDs:** six addressable RGB LEDs on one data line:
  - one top-emitting WS2812B-2020 at the touch key;
  - five side-emitting SK6812SIDE-A on the front edge: PC1, PC2, and one beside each USB-A port.
  - They show the active PC, each PC's connection state and each port's mode (§7).
- **Front-port switch paths** follow the pin geometry, with the MHL channel on the KVM hub so both PCs keep the
  datasheet-specified HS channels:
  - MHL → Hub-0, which is the power-up default (SEL1 pulled up, SEL0 pulled down), i.e. the KVM path;
  - USB1 → Hub-2;
  - USB2 → Hub-1.
  - **Rev 6 / 7:** the MHL channel is on Hub-0 and USB2 on Hub-1 (schematic, straps and firmware aligned), and the
  router was re-planned for those pin ends: Hub-0's keyboard pair routes before any of Hub-1's lanes (74.8 mm instead
  of 120.4 mm), the twists are symmetric (H1_AT 0.00 mm of skew), In1.Cu is reserved under the pairs and the SPI, and
  the SI checks use the spec's 5 ps skew budget.  Measured on rev 6/7: 16/16 pairs, `unrouted: 0`, ERC 0 violations,
  0 DRC violations (analysis.md 7).  The service switch sits top-left of Hub-0 so Hub-0's upstream link is short; that
  link still routes as two single nets and needs a fixed-copper breakout to become a pair.
- **Rev 5:** this swap is what the board does now (schematic, straps and firmware aligned on 2026-10-23); what the
    switch paths put on the USB1/USB2 channels is each PC's passthrough, whose high-speed eye the datasheet
    specifies. (The device↔hub link still negotiates HS on whichever channel is selected, so this is about
    where an HS *fallback* is harmless, not about avoiding HS signalling.) The router-side lanes are planned for
    these pin ends; the measurements below are from when the swap was first tried with the old lanes. — the second time with the six router-side lanes re-planned from the *pairs-only* board, which
    contains only the fixed copper and the pairs: `LED_D0`, `AT1_XI` and `KB_FLT` are signal nets, so they are
    not there during the pair stage and are not what blocks it. With Hub-0 routed first, H0_MS (skew 0.37 mm),
    H0_OT (0.50 mm) and H1_KB (0.11 mm, no vias) come out clean, but **H0_KB and H1_MS compress their pair
    spacing** (P-N gap 0.000 / −0.225 mm — the nets touch) and H2_KB fails mid-board. The two families have
    opposite needs: each wants *its* corner routed first, because the corners are only ≈1.4–1.6 mm wide where a
    pair plus its via pair needs ≈1.9 mm, while Hub-0's mid-board crossings were planned to weave *across* the HS
    lanes and therefore want to go last. Closing the ≈0.3–0.5 mm needs a layout change in those corners — move
    the SEL strap pairs (R1201/R1202, R1301/R1302) or the router's GND/+3V3 fan-out vias, or shift the router —
    or **two cascaded 1:2 switches** per port (PC1+PC2 on a HS-specified 1:2 such as the TS3USB221, Hub-0 behind
    a second one), which needs no MHL channel at all and costs +3 parts. Until then PC1's passthrough through
    the MHL channel stays a bring-up eye test (report.md F4, §12 risk 7).

**Rev 3 changes:**
- **Confirmed:**
  - AT32F405 on the PC side;
  - the Other port serves a **webcam or gamepad** (§4);
  - default hotkey **double-tap Scroll Lock**;
  - Rust for `odkvmctl`.
- **RP2354B core supply:** an external 1.1 V LDO (TI TLV73311) with the internal switcher disabled, as documented in
  the RP2350 datasheet. This **removes the hard-to-source inductor** and its orientation sensitivity (§6).
- **Key and LEDs:** the board mounts **components-down**, so the hot-swap socket and LEDs are assembled on the same
  side as everything else (§11).

**Rev 2 changes** (after the JLCPCB stock check, 2026-10-20):
- **PC-side MCU: STM32F723VE → Artery AT32F405CCT7.** JLC has only 10 STM32F723VET6 in stock, at $23 each. The
  AT32F405 has an integrated HS PHY, the same TinyUSB driver, and costs $2.09 (§3.3).
- **Connectors:**
  - USB-C → HRO TYPE-C-31-M-12, whose footprint and JLC offset are already proven in OpenRower;
  - USB-A → Molex 67643-0910, which has a stock KiCad footprint;
  - Kailh socket → the HanElectricity CPG151101S11-2 equivalent, because the genuine Kailh part is out of stock.
- **Clocks:** one crystal value, 12 MHz, serves all three MCUs.
- **BOM** now lists LCSC numbers and JLC stock (§5).
- **KiCad workflow** adopted from the OpenRower project (§14), plus a list of datasheets (§15).

## 1. Concept

Each PC is permanently connected to an **emulated** USB device that the KVM itself provides. The real keyboard, mouse and
"other" peripheral are enumerated by the KVM's own USB host, never by the PCs. Switching only changes which PC receives
the forwarded reports. Neither PC sees a disconnect, so switching takes under 1 ms and needs no re-enumeration.

What each PC sees, permanently, from the moment its USB port is powered:

```
PC ── 4-port USB 2.0 hub (on the KVM)
        ├─ port 1: "KVM HID" composite device (High-Speed)   ← always present (persistent emulation)
        │     ├─ keyboard / mouse interfaces (normalized or mirrored, see §4)
        │     ├─ "other" HID mirror (only if a HID device is on the Other port)
        │     └─ control interface (vendor HID) used by the host software (§8)
        └─ ports 2–4: empty, unless a front port is in passthrough mode
```

## 2. Block diagram

```
 REAR                                                                           FRONT
 USB-C PC1 ══► Hub-1 USB2514B (HS) ──DS1──► AT32F405 #1  (HS device) ◄─SPI1──────┐
                     DS2 DS3 DS4 ◄═══════════ passthrough ═════╗                 │
 USB-C PC2 ══► Hub-2 USB2514B (HS) ──DS1──► AT32F405 #2  (HS device) ◄─SPI1──────┤
                     DS2 DS3 DS4 ◄═══════════ passthrough ══╗  ║                 │
                                                            ║  ║   RP2354B (USB host, switching logic)
 USB-C PWR/SERVICE ═ D+/D− ══ TS3USB30E ════════════════════║══║═══ native USB
   (VBUS → power path)          ║ (normal position)          ║  ║
                                ╚═► Hub-0 USB2514B (runs at FS)
                                       DS1   DS2   DS3
                                        ║     ║     ║
                              ┌─────────╨─────╨─────╨──────────────────────────────────┐
                              │ one TS3USB3031 1:3 switch per front port:              │
                              │   A → Hub-0 (persistent emulation)     B → Hub-1 (PC1)   C → Hub-2 (PC2)  │
                              └────────┬──────────────────┬──────────────────┬─────────┘
                                 USB-A Keyboard     USB-A Mouse       USB-A Other
```

## 3. Key decisions and why

### 3.1 Three MCUs with split roles
- **RP2354B** handles USB host duty and all switching logic. It enumerates the peripherals, parses their HID reports,
  runs the hotkey engine, the button, the LEDs, the config storage and the switching state machine. It also programs
  the two AT32F405s (§10).
- **2 × AT32F405** are the PC-facing devices (Cortex-M4F, 216 MHz). Each one is a USB device toward one PC and is kept
  deliberately simple: it presents the descriptors the RP2354B tells it to and pushes whatever reports it sends.
- The RP2354B is the SPI master and links to each AT32F405 over its own SPI bus at ≥20 MHz with DMA. A 24-byte frame
  takes about 10 µs on the wire. The AT32F405's SPI slave receives at up to 36 Mbit/s but transmits at up to
  25 Mbit/s, so the link runs at 25 MHz (report.md F3e).
- The link uses the AT32F405's **SPI1 on PA4–PA7**. Those are exactly the pins its ROM bootloader listens on, on
  every package (datasheet v2.02, table 5). With BOOT0 and NRST also driven by the RP2354B, the same four wires both
  carry the link and reflash the AT32F405.

A single MCU cannot do this job. It would need 2 device controllers plus 3 host ports at the same time, and no
practical MCU offers that.

### 3.2 High-Speed on the PC side (the main latency lever)
An emulated HID device adds one extra wait: the time until the PC next polls it.

| PC-side link (bInterval = 1) | PC poll period | Added wait | Total added latency (incl. ~20 µs link + parse) |
|---|---|---|---|
| Full-Speed device | 1 ms | 0–1000 µs | **~520 µs avg, ~1.02 ms worst** |
| **High-Speed device (chosen)** | 125 µs | 0–125 µs | **~85 µs avg, ~145 µs worst** |

The KVM polls the mouse at the mouse's own 1000 Hz, which is the same as when the mouse is plugged straight into a PC.
That wait is therefore not added. The PC sees a 1000 Hz report rate either way. With High-Speed on the PC side, each
report reaches the PC within about 0.15 ms, so the KVM adds little measurable latency. This holds in every mode of the
persistent emulation, normalized or mirrored.

The PC-side MCU needs an **integrated** High-Speed PHY. That rules out the RP2040/RP2350, ESP32-S3 and most STM32s.

### 3.3 Choosing the PC-side MCU
All candidates have an integrated HS PHY and TinyUSB HS device support. Stock and prices are JLC's, 2026-10-20.

| Part | JLC stock | Price | Verdict |
|---|---|---|---|
| WCH CH32V305 | plenty | ~$1 | ✗ TinyUSB documents **interrupt IN endpoint starvation** on its USBHS controller: 3 interrupt IN endpoints at 125 µs gave ~80 % NAK and 4 ms gaps. That is exactly this design's workload |
| STM32F723VET6 (rev 1 choice) | **10** | **$23.00** | ✗ for JLC assembly. The LQFP-144 has 0 in stock; the BGA costs $19 |
| STM32U5A5 / H7S3 / F733, SAM E70/S70, RA6M3 | ≤26 | $8–29 | ✗ stock and price |
| NXP LPC55S69 (HTQFP-100) | 1541 | $6.13 | backup option |
| NXP i.MX RT1011 | 611 | $6.57 | ✗ needs external QSPI flash and more boot configuration |
| **Artery AT32F405CCT7** (LQFP-48) | **174** | **$2.09** | **chosen** |

Why the AT32F405:
- **USB core:** its OTGHS is a dwc2 core, the same IP as STM32 OTG, so it uses TinyUSB's mature `dwc2` driver (listed
  as device + host + HS for "F402_F405").
- **Endpoints:** it has 8 IN + 8 OUT endpoints including EP0, a dedicated 4 KB FIFO RAM, its own DMA, and dedicated
  D+/D−/R pins.
- **Clock:** its HS PHY needs a **12 MHz** crystal (and no other value), the same as the RP2354B.
- **Bootloader:** the ROM bootloader takes SPI1, USART2, I2C and **DFU over OTGHS** on every package, so even a
  blank chip can be flashed from a PC.
- **Downsides:**
  - it is a smaller vendor, though the English datasheet (v2.03), reference manual (v2.02) and getting-started guide
    are now in `datasheets/`;
  - TinyUSB's support for it is newer than for STM32, so it gets validated on an AT-START-F405 board first (§12).

**Endpoint budget:** the AT32F405 has 7 IN endpoints beyond EP0:
- boot keyboard;
- mouse;
- NKRO + consumer + system;
- control interface;
- **3 left for mirrored interfaces**, or 2 while the Vial interface is shown ("Keymap and Vial" in §4).

**When the keyboard and the mouse are both mirrored** and need more than that, the first three carry nothing: no port
runs Normalized. The KVM then leaves them out, and the mirrors get **6** (5 with Vial). The PCs see the devices' own
interfaces plus the control interface. A setup that fits without it keeps all four, so a device plugged in later is
normalized without re-enumerating the PCs. OUT endpoints are never short: only the control interface has one, so
mirrors get the other 6.

A mirrored device that doesn't fit runs Normalized if its keys and buttons fit that format, otherwise Passthrough.
All three ports are decided together: a port already mirrored keeps its endpoints against one that asks later, and
endpoints freed by a mode change or an unplug go to a port that was left out.

### 3.4 Host side: native USB host plus a hub, not PIO-USB
The RP2354B's native USB host feeds Hub-0 (FS upstream), and Hub-0 feeds the three front ports. TinyUSB's RP2040/RP2350
host driver supports hubs and low-speed devices behind a hub (PRE preamble), and has 15 hardware-polled interrupt
endpoints. Bit-banged PIO-USB would save the hub but has weaker device compatibility, and it would consume a whole CPU
core.

- FS bandwidth is ample: 3 devices with about 9 interrupt endpoints at 1 kHz use under half a frame.
- Consequence: an 8 kHz High-Speed gaming mouse falls back to Full-Speed, which means 1000 Hz. That is within spec.

### 3.5 Per-port 1:3 switch
Every front port has a TS3USB3031 (USB 2.0 HS 1:3 analog switch, 6.5 GHz, 1 pF). It connects the port either to Hub-0,
where the KVM emulates the device, or straight to Hub-1/Hub-2, where the real device reaches the active PC at full
480 Mbit/s. This switch is what makes the four port modes in §4 possible.

## 4. Port modes

Each front port (Keyboard, Mouse, Other) can be set independently, from the host software (§8) or by the Auto policy.
A set of modes for all three ports is a profile, and the key on the KVM switches between profiles (below).

| Mode | What the PCs see | Switching | The maker's configuration software | Use it for |
|---|---|---|---|---|
| **Normalized** | fixed KVM keyboard/mouse interfaces | seamless, under 1 ms | no | default for keyboard/mouse; most stable |
| **Mirrored** | the device's own HID interfaces, byte-identical, inside the KVM composite (KVM VID/PID) | seamless | only apps that find the device by HID usage page (few do) | extra buttons/keys/dials, unusual report formats, vendor HID reports |
| **Mirrored + identity** | as Mirrored, plus the device's VID/PID and name strings | seamless | usually works on the **active** PC (validate per device) | vendor software *and* seamless switching. **Max. one port at a time** |
| **Passthrough** | the real device, behind the PC's hub, 480 Mbit/s | re-enumerates on the other PC (~1 s), optional VBUS power-cycle | always | peripheral firmware updates, non-HID devices. It can follow the active PC (default) or be **pinned** to PC1 or PC2, e.g. a webcam that should stay with the work PC |

### Normalized
- Reports are translated into fixed interfaces:
  - boot keyboard;
  - NKRO keyboard + consumer + system control;
  - mouse with 16 buttons, 16-bit X/Y, high-resolution wheel and horizontal wheel.
- A descriptor-driven HID parser handles any input layout: boot, NKRO bitmaps, report IDs, 8/12/16-bit axes.
- Swapping the physical keyboard or mouse never makes a PC re-enumerate.

### Mirrored and Mirrored + identity
- **Input reports** are forwarded verbatim. The hotkey still works on a mirrored keyboard: the RP2354B parses the same
  reports and strips the hotkey usages before forwarding. On a switch, a "release all" report is built from the
  parsed layout.
- **Output and feature reports** flow between the device and the **active** PC only. That covers SET/GET_REPORT and
  interrupt OUT, i.e. LEDs, DPI and RGB commands, vendor command channels such as HID++.
- **The inactive PC** has its SET_REPORTs acknowledged and stored, and its GET_REPORTs answered from the last value
  seen. Its stored output state (lock LEDs, etc.) is applied when it becomes active.
  - Consequence: a vendor app on the inactive PC may show the device as idle or unresponsive until you switch to it,
    and it may re-apply its profile at that moment.
  - Settings stored on the device itself (onboard profiles, DPI steps) are unaffected.
- **Identity:** the composite takes the device's VID/PID and strings. The cloned interfaces keep their original
  interface numbers, and the KVM's own interfaces are appended after them, so vendor drivers bind to the interfaces
  they expect.
  - A USB device has exactly one VID/PID, so only one port per PC can use identity.
  - Changing it re-enumerates the composite once.
  - Fallback if a vendor driver rejects the extra interfaces: put the clone on the AT32F405's second controller
    (OTGFS1, already in the chip, crystal-less in device mode) as a separate Full-Speed device. That needs a hub with
    5 or more ports (USB2517), so it is not in rev 2.
- **Swapping the physical device** on a mirrored port changes the descriptors. That causes one re-enumeration of the
  composite (about 1 s) on both PCs.
- **Endpoint limit:** at most 3 mirrored IN interfaces in total, or 6 while the keyboard and mouse are both mirrored
  (§3.3).

### Auto policy (default)
- Any non-HID interface → **Passthrough**, except XInput pads (below). That covers flash drives, webcams and audio;
  such devices cannot be "connected" to two PCs at once.
- **Keyboard and Mouse ports:**
  - all input usages fit the normalized format → **Normalized**;
  - otherwise (vendor-page inputs, more than 16 buttons, absolute axes, unknown usage pages) → **Mirrored**.
  - A vendor *command* channel by itself (e.g. HID++) does not trigger Mirrored. The host software flags it as
    "vendor features available: use Mirrored + identity or Passthrough".
- **Other port, HID device** → **Mirrored**, as long as the endpoint budget allows; otherwise Passthrough.

### The Other port in practice: webcam or gamepad
- **Webcam** (UVC video + audio classes): **Passthrough** at full 480 Mbit/s, straight to the PC's hub.
  - The default is to follow the active PC. Each switch re-enumerates it on the new PC, which takes about 1 s; video
    apps typically need re-selecting the camera.
  - For a webcam that belongs to one PC, set the port to **Passthrough pinned to PC1/PC2**. It then never moves.
  - It draws up to about 0.5 A, so use the PWR port if the PCs' ports are weak (§6).
- **HID gamepad** (DirectInput / generic HID: most console-style and generic PC pads):
  **Mirrored**. It switches seamlessly like the keyboard and mouse. Rumble and LED output reports go to the active
  PC only.
- **XInput gamepad** (Xbox controllers and pads in "X-input" mode): these use a vendor-specific interface instead of
  HID, so plain Auto would pick Passthrough.
  - The firmware mirrors that interface as well: its descriptors plus the Microsoft OS descriptor that Windows uses
    to load its XInput driver. Xbox-style pads therefore also switch seamlessly.
  - It needs 1 IN + 1 OUT endpoint, which fits the budget.
  - It is validated with your pad on the prototype. If a pad refuses, it falls back to Passthrough.

### Profiles
- A profile is a name and the mode of each front port. The KVM stores 4, of which 1 to 4 are in use (default 2):
  - **work:** Auto on all three ports: emulated keyboard and mouse that switch instantly;
  - **gaming:** Passthrough on all three ports: the real devices, with their own polling rate and vendor software,
    each re-enumerating on a switch (about 1 s).
- A long press on the KVM's key makes the next profile active, after the last one the first (§7). `odkvmctl profile`
  and the GUI select, rename and edit profiles.
- Monitors switched over DDC/CI by the host software (software.md §9) can be routed per profile too: follow the
  active PC, stay on one PC, or be left alone. That is a setting of each PC, not of the KVM.
- Setting a port's mode changes it in the active profile. On a change of profile all three ports are decided again
  together, so the endpoint budget and identity are settled once for the new set.
- The profiles are saved. By default the KVM powers up in profile 0, and choosing a profile writes nothing to flash:
  it loses power only when both PCs are off (or it is unplugged), and starting over in the first profile is fine then.
  With "power up in the profile last active" on, it remembers the profile, at a flash write per change.
- With the keyboard in passthrough, the KVM doesn't see it, so the hotkey can't switch. The key on the KVM and the
  software still can.

### Keymap and Vial
- **Keyboards with Vial (or VIA) of their own** keep their keymap in the keyboard: set them up in Passthrough, and the
  remapped keys reach the KVM in Normalized mode as well. Their raw HID interface is a vendor command channel, so Auto
  keeps them Normalized.
- **Any other keyboard** can be remapped on the KVM itself, with the Vial app (software.md §4.11). The RP2354B runs a
  QMK-style keymap on the keyboard state it already parses: 8 layers, mod-tap and layer-tap, one-shot keys, media and
  mouse keys, 16 macros. One keymap serves both PCs; it applies to keyboards in Normalized mode.
  - The "matrix" is the HID keyboard page (one position per usage), so the KVM sees what the keyboard sends: its own
    Fn key stays invisible.
  - Vial talks to a raw HID interface of its own, which takes one IN endpoint. The setting is **Off** (default),
    **On** (it keeps its endpoint: mirrors get one less) or **Auto** (shown only while no mirrored device needs the
    endpoint).
    The keymap keeps working with the interface off.
  - Edits need "configuration from software"; macros and the matrix tester also need Vial's unlock (both Shift keys
    held for 3 s), and the matrix tester answers only the active PC.

### Screen hop
- **What:** with the screens arranged as they stand on the desk, the KVM moves the pointer itself across all of them,
  and switches to another computer (keyboard and all) when the pointer is pushed across an edge onto one of its
  screens. The computers are both PCs and, with the Bluetooth module, the paired hosts (a laptop, a tablet). One
  mouse, several computers, no software needed on any.
- **How:** the KVM's mouse interface gets an absolute pointer as well (a setting: changing it makes both PCs
  reconnect the KVM's devices once). The RP2354B keeps the pointer in a plane of logical pixels, maps it onto the
  active computer's screens and sends it the absolute position (software.md §4.14). A Bluetooth host gets an absolute
  pointer of its own in the module's HID service; a host whose system takes none (some tablets and phones) is set to
  relative motion, and the KVM parks its cursor in a corner when the pointer comes onto it and steers it from there:
  close, not exact, and back in line at every edge it is pushed against.
- **Needs** the Mouse port in Normalized mode. The KVM keeps four arrangements, and each profile uses one of them or
  none: a "work" profile with the desk's, a "sofa" profile with the laptop and the tablet, a "gaming" profile without
  screen hop, where games get the relative mouse they need. Switching profiles switches the arrangement at once: no
  re-enumeration, no flash write.
- **Feel:** an absolute pointer isn't accelerated by the PC, so the KVM accelerates: Windows' "Enhance pointer
  precision" curve (default), libinput's adaptive profile as on Linux desktops, or flat; a speed setting for each.
- **Edges:** where the same computer's screens meet, the pointer passes (a gap between screens is jumped); toward
  another computer an edge holds the pointer until it is pushed further (100 pixels by default), never near the
  corners (so menus and window controls there stay easy to hit), never while a button is held, and only to a computer
  that is ready (a PC on and awake, a Bluetooth host connected). Any other edge is a wall.
- **Windows** maps an absolute pointer onto its main screen only. The KVM reaches such a PC's other screens with
  relative motion from where it last put the cursor; the host software, where it runs, reports the cursor whenever it
  rests, so what Windows' acceleration or a touchpad did is caught up. Linux maps the absolute pointer across all its
  screens.
- **Set up** in the host software's *Screen hop* page: drag the screens into place, or detect each PC's screens from
  the software running on it (a Bluetooth host's are added by hand); `odkvmctl screens`.
- Until firmware 0.1.12 there was one arrangement and screen hop was between PC1 and PC2 only.

## 5. Component selection (JLCPCB-checked 2026-10-20)

"Class" is JLC's library class. Extended parts cost a one-off fee of about $3 each per order. Stock is JLC's. Parts in
**bold stock** are thin; re-check them before ordering.

| Ref | Function | Part | Package | Qty | LCSC | Class / JLC stock | Unit $ | KiCad | Notes |
|---|---|---|---|---|---|---|---|---|---|
| U1 | Host MCU / logic | **RP2354B** (RP2350, A4, 2 MB flash in-package) | QFN-80 10×10 | 1 | C39843328 | Ext / 1889 | 1.52 | stock | 48 GPIO (about 40 used) |
| U2, U3 | PC-facing HS device | **AT32F405CCT7** (256 KB flash) | LQFP-48 7×7 | 2 | C47115471 | Ext / **174** | 2.09 | symbol custom, footprint stock | LQFP-48 pins: OTGHS1_R 25 (**12 kΩ ±1 %** to GND), D− 26, D+ 27, PF11/BOOT0 44, NRST 7, SPI1 PA4–PA7 14–17, HEXT PF0/PF1 5/6. OTGHS needs VDD 3.0–3.6 V and adds about 30 mA while transferring (DS v2.03, §4.3.18). Alternates: KBU7-4 QFN-32 C47090436 (396, 128 KB, no USART1); RCT7-7 LQFP-64 C19674180 (146) |
| U4–U6 | Hub-0 / Hub-1 / Hub-2 | **Microchip USB2514B-AEZC-TR** | VQFN-36 6×6 | 3 | C16251 | Ext / 13137 | 1.81 | stock (`USB2514B_Bi`) | strap-configured. Hub-1's AT32F405 on port 4, Hub-2's on port 1 (the only port `NON_REM[1:0]` can mark, so only Hub-2 reports a compound device) |
| U7–U9 | Front-port router | **TI TS3USB3031RMGR** 1:3 USB 2.0 HS switch | QFN-12 1.8×1.8, 0.4 mm | 3 | C2652459 | Ext / **82** | 1.03 | custom | enough for about 27 boards; select pulls make Hub-0 the default |
| U22 | Service USB switch | **TI TS3USB30EDGSR** 1:2 | VSSOP-10 | 1 | C131998 | Ext / 2159 | 0.76 | stock | pull resistor makes PWR/SERVICE the default |
| U10 | Input priority mux | **TI TPS2121RUXR** (2.4 A limit, XCOMP priority mode) | VQFN-HR-12 2×2.5 | 1 | C485916 | Ext / 27314 | 0.92 | custom | IN1 = PWR port, IN2 = PC bus |
| U11 | PC1/PC2 ideal-diode OR | **TI LM66200DRLR** dual ideal diode | SOT-583 | 1 | C3235556 | Ext / 15638 | 0.42 | custom | or 2× LM66100DCKR (C2869734, stock symbol) |
| U12–U14 | Front VBUS switches | **TI TPS2553DBVR** adjustable limit, EN + FAULT | SOT-23-6 | 3 | C55266 | Ext / 51844 | 0.54 | custom | about 1.0 A each. OpenRower's SY6280AAC (C55136, $0.09) has no FAULT output, which the host software needs |
| U15 | 3.3 V rail | **TI TLV62569DBVR** 2 A buck | SOT-23-5 | 1 | C141836 | Ext / 442749 | 0.07 | stock | |
| U16–U21 | ESD (D+/D−, VBUS) | **ST USBLC6-2SC6** | SOT-23-6 | 6 | C7519 | Ext / 43630 | 0.18 | stock | same part and JLC correction as OpenRower |
| L401 | 3.3 V buck inductor | **Sunlord SWPA3015S2R2MT** 2.2 µH shielded | 3.0×3.0×1.5 | 1 | C43389 | Ext / — | — | stock | output filter of the TLV62569 buck (§6) |
| D4–D6 | ESD on CC lines | **Nexperia PESD5V0S2BT,215** | SOT-23 | 3 | C49338 | Ext / 25103 | 0.09 | stock | |
| J1–J3 | Rear USB-C (PC1, PC2, PWR/SERVICE) | **HRO TYPE-C-31-M-12** | SMD 16-pin | 3 | C165948 | Ext / 345875 | 0.19 | OpenRower footprint | replaces GCT USB4105-GF-A (C3020560, 0 in stock); 5.1 kΩ Rd per CC pin |
| J4–J6 | Front USB-A | **Molex 67643-0910** | THT R/A | 3 | C2798029 | Ext / 4411 | 0.85 | stock (`USB_A_Molex_67643_Horizontal`) | alt: Amphenol 87520-0010BLF C599115 (18408, $0.68, needs a custom footprint) |
| SW1501 | Switch key | **MX-compatible switch** in a **HanElectricity CPG151101S11-2** hot-swap socket (Kailh CPG151101S11 equivalent) | MX 1u | 1 | C49352235 (socket) | **JLC, bottom side** (the switch is plugged in afterwards) | — | custom footprint | centred on the board; socket on the bottom. What to order: §11 |
| SW2 | Recovery (pinhole) | **XKB TS-1187A-B-A-B** | SMD 5.1×5.1 | 1 | C318884 | **Basic** / 274052 | 0.02 | OpenRower footprint | on RP2354B `QSPI_SS` |
| D1501 | Key LED | **SK6812MINI-E** reverse-mount RGB | 3.2×2.8 | 1 | C5149201 | **JLC, bottom side** | — | stock (`…_ReverseMount`) | bottom side, shines up through a cut-out into the switch's LED window; **last** in the LED chain, so a missing or damaged key LED cannot darken the five status LEDs |
| D1502–D1506 | Status LEDs | **SK6812SIDE-A** side-emitting RGB | 4.0×1.6 | 5 | C5378721 | Ext / 183498 | 0.11 | custom footprint from JLC's land pattern | front edge, left to right: PC1, Keyboard, Mouse, Other, PC2. Light leaves the long face opposite the pads, so they sit at 0° with the lens towards the edge (up to 0.16.0 they sat at 180° and shone into the board: hardware/errata.md E3) |
| U1501 | LED data buffer | **TI SN74AHCT1G125DBVR** | SOT-23-5 | 1 | C7484 | Ext / 49531 | 0.10 | stock | 3.3 V GPIO → 5 V LED data |
| U24 | RP2354B core supply (DVDD 1.1 V) | **TI TLV73311PDBVR** 1.1 V 300 mA LDO, ±1.4 % max | SOT-23-5 | 1 | C2865431 | Ext / 3592 | 0.14 | custom | replaces the switcher's inductor, which had only 4 in stock (§6). Stable with or without ceramic capacitors. The RP2350 needs DVDD 1.05–1.16 V; this gives 1.085–1.115 V |
| Y1–Y3 | 12 MHz crystals | **Abracon ABM8-272-T3** (Raspberry Pi's part) | 3225 | 3 | C20625731 | Ext / — | — | stock | RP2354B + both AT32F405s (the OTGHS PHY requires exactly 12 MHz); 10 pF load, so 15 pF C0G caps |
| Y4–Y6 | 24 MHz crystals | **YXC X322524MOB4SI** | 3225 | 3 | C70590 | Ext / 45855 | 0.09 | stock | hubs |
| R, C | passives | UNI-ROYAL 0402/0603, Samsung MLCC | | ~120 | as OpenRower | Basic | | stock | |
| J7–J9 | Debug (optional) | Tag-Connect TC2030-NL pads (SWD ×3) | — | — | — | not assembled | | footprint | firmware development only |

Rough cost is **about $26 of parts per board**, plus about $60 per order in JLC extended-part fees, before the PCB,
assembly and enclosure. The rev 1 STM32 BOM was about $41 per board at distributor prices, or about $70 at JLC's.

## 6. Power

```
USB-C PWR/SERVICE  VBUS ─────────────────────► TPS2121 IN1 (priority)
USB-C PC1  VBUS ─► LM66200 ch.A ─┐
USB-C PC2  VBUS ─► LM66200 ch.B ─┴───────────► TPS2121 IN2
                                               OUT = 5V_SYS
                                                 ├─► 3× TPS2553 ─► front USB-A VBUS
                                                 ├─► SK6812 LEDs
                                                 └─► TLV62569 ─► 3V3 (hubs, MCUs, switches)
```

- **Proposed deviation from the spec:** the spec takes power from PC1 only. This design also takes it from **PC2**,
  through an ideal diode. Without that, if PC1 is off or asleep with its USB unpowered, PC2's emulated keyboard and
  mouse disappear. The spec version is easy to build instead: leave channel B unpopulated.
- **Back-powering:** each hub's VBUS_DET pin watches its own PC's VBUS. A hub only pulls up D+ toward a PC that is
  actually powered.
- The KVM reads the CC voltage on each rear port with ADCs (the AT32F405s for the PC ports, the RP2354B for PWR),
  which reports the source's advertised current: Default, 1.5 A or 3 A.
- **Budget:** the electronics draw about 0.5 A from 5 V — roughly 0.35–0.45 A on 3V3 (three hubs, two AT32F405s at
  144 MHz, the RP2354B) plus the LED chain at up to ≈ 0.2 A at full white. Measure it on the prototype (§12 risk 5).
  The headroom left for peripherals depends on the source:

  | Source | Left for peripherals |
  |---|---|
  | PC USB 2.0 port (500 mA) | none — **not enough** |
  | PC USB 3 port (900 mA) | ~0.4 A — fine for a mouse and a modest keyboard |
  | PC Type-C 1.5 A | ~1.0 A |
  | PC Type-C 3 A, or the PWR port with a 3 A charger | ~2.5 A — everything |

  On a PC-only supply at the low end of USB 2.0 and with a loaded port, the rail can sit near 4.4 V, just below the
  SN74AHCT1G125's 4.5 V recommended minimum. It still works (its VOH tracks VCC), and the PWR port or a 5.0 V source
  removes the case (report.md N1).

  An RGB gaming keyboard alone can draw 300–500 mA, and a webcam another 200–500 mA. The host software and the LEDs
  warn when the advertised budget is too low.

### 6.1 Budget with the Bluetooth module (0.16.0)

The module (§16) runs from the same 3.3 V buck. ESP32-C3-MINI-1 datasheet v2.2: Bluetooth LE transmit at 0 / +9 / +20 dBm
draws 170 / 190 / 340 mA peak (table 6-5), receive 86 mA; the CPU at 160 MHz with the radio idle 16–28 mA (table 6-6).
The firmware caps transmit power at +9 dBm, so the peak is 190 mA, in bursts of a few hundred microseconds per
connection event.

| Rail | Before | With the module | Limit |
|---|---|---|---|
| 3.3 V, average | 0.35–0.45 A | + about 40 mA (four hosts connected, the selected one at 7.5–15 ms) | TLV62569: 2 A |
| 3.3 V, peak | — | + 190 mA | 2 A; 10 µF + 100 nF at the module's 3V3 pad |
| 5 V, average (90 % buck) | about 0.5 A + LEDs | + about 30 mA | the source's advertised current |

- The buck has more than 1 A of headroom at the peak, so the module needs no separate regulator or load switch.
- The front-port budget the firmware reports (`Status.budget_ma`) takes 30 mA more off the source when the module
  answers (`kvm_core::engine::BT_CURRENT_MA`). The source table above shifts by that much: a USB 3 PC port leaves
  about 0.37 A.
- The board built without the module (the `_nobt` BOM, §16) draws exactly what 0.15.0 did.

### RP2354B core supply: external 1.1 V LDO instead of the internal switcher
The RP2350's internal switching regulator needs a polarity-marked 3.3 µH inductor, laid out exactly as on the Pico 2
(datasheet §6.3.8). Raspberry Pi recommends the custom Abracon AOTA-B201610S3R3-101-T; **JLC has 4 in stock**.

Alternatives searched:
- **No published alternative inductor.** The datasheet would accept any part that is fully shielded, 3.3 µH ±20 %,
  ≤250 mΩ DCR, ≥1.5 A saturation current and **polarity-marked**.
  - Raspberry Pi say they found no such part in 0806 size, and the EEVblog thread on this question names no tested
    substitute.
  - Small inductors are normally unmarked, and their winding direction is random along a reel.
- **Documented by Raspberry Pi: an external DVDD supply** (datasheet §6.1.3 and §6.3.7, Figure 24). Feed the core
  from an external 1.1 V regulator and disable the on-chip one.
  - The inductor is then omitted entirely.
  - The **Bus Pirate 5XL/6** ships this way, with "a Texas Instruments 1.1 V regulator", chosen because it is
    cheaper, smaller and "less fiddly" than the switcher.

**Chosen: external DVDD**, using the TI **TLV73311PDBVR** (1.1 V, 300 mA, JLC C2865431, 3592 in stock).
- **Wiring** (Figure 24):
  - VREG_VIN and VREG_AVDD are tied together and fed from 3V3 through a 33 Ω / 4.7 µF RC filter. Both must stay
    powered, because they run the power-on reset and brown-out detector.
  - VREG_FB → GND, VREG_PGND → GND, VREG_LX unconnected.
  - LDO output → all DVDD pins, with 100 nF at each pin and 1 µF at the LDO.
- **Margins:**
  - LDO accuracy is 1.085–1.115 V against the RP2350's 1.05–1.16 V DVDD range.
  - That clears the power-on-reset threshold (0.99 V max) with margin.
  - Sequencing is fine: DVDD comes up just after 3V3, as with the internal regulator (§6.1.8).
- **Firmware rule:** at boot, set the regulator to high-impedance (`VREG.HIZ = 1`), which saves about 400 µA.
  **Never** use its low-power mode with an external DVDD: in that mode its output is hard-wired to DVDD (§6.3.1).
- **Cost:** about 0.1 W of LDO heat (≈ 40 mA × 2.2 V) and no core-voltage scaling for overclocking, which this design
  doesn't use.
- **Fallback:** the Abracon part through JLC global sourcing, reproducing the Pico 2 / minimal-design layout. Its
  geometry is in `datasheets/rp2350-minimal-kicad/`.

## 7. User interface on the device

- **Key:** a press toggles PC1/PC2. A long press (1 s by default, adjustable from 0.8 to 3 s) makes the next profile
  active (§4); for 1.5 s the front LEDs
  count its number from the left (one for profile 0, two for profile 1, …), then the port LEDs show its modes. Both are
  configurable: switch PCs, next profile, cycle the Other port's mode, or nothing. The LED under the keycap glows in
  the active PC's colour.
  - **Blinking red** (alternating with the active PC's colour, and in standby too): a part of the KVM doesn't work.
    This covers Hub-0, the PWR-port readings, for either PC its link, its hub, its chip's firmware or its chip's
    crystal, and the Bluetooth module's firmware (0.16.0, when fitted). The PC LEDs show which side, `odkvmctl status` says what. A front port's power-switch fault stays on that
    port's LED.
  - **Held while the KVM powers up:** the AT32F405s, and the Bluetooth module where fitted, get up to 5 more
    programming attempts. The RP2354B stops after 5 failed attempts in a row, so it doesn't reprogram a chip at every
    power-up (software.md §4.9, §4.13). Switching and
    configuration from software are allowed again too (§8). That press is neither a tap nor a long press.
  - **Pulsing white:** a firmware update, or software asking to be allowed again, waits for a tap (§8).
- **Status LEDs** on the front edge:
  - **PC1 and PC2:** connection state (off = unpowered, dim = connected, breathing = suspended). The active PC is
    bright.
  - **One per USB-A port:** that port's mode (Normalized, Mirrored, Mirrored + identity, Passthrough, Pinned), shown as
    a colour.
- **Hotkey:** the default is **double-tap Scroll Lock** to toggle PC1/PC2. Both taps are swallowed, so neither PC sees
  Scroll Lock change. The hotkey is fully configurable from `odkvmctl`, e.g. a direct "select PC1 / PC2" combination,
  typed or learned by pressing it on the KVM's keyboard (software.md §4.5).
- **Clean switching:** on every switch, the old PC receives a "release all" report (no stuck keys). Keys still held
  during the switch are suppressed on the new PC until they are released. The keyboard's lock LEDs (Caps, Num) are
  updated to the new PC's state.
- **PCs off or asleep** (software.md §4.12; each setting can be turned off from `odkvmctl` or the GUI):
  - When the active PC is switched off, the other one becomes active. Nothing is saved for it: after a power loss
    the KVM finds the running PC from the PCs' states.
  - **Standby:** after 10 s without any PC in use, all LEDs go dark (optionally the key glows slowly instead) and the
    front ports switch off.
  - While the active PC sleeps, its keyboard and mouse stay powered and a key press wakes it; that key isn't typed.
  - A PC coming up, or the KVM's key, ends standby; the first press on the key only wakes the KVM.

## 8. Host software

**Transport.** The control interface is a vendor-defined HID interface inside the always-present composite device.
- No driver is needed on Windows, macOS or Linux. Linux needs one udev rule for non-root `hidraw` access.
- It is available on **both** PCs at all times, including the inactive one.
- The RP2354B holds the only copy of the state. A change made from either PC is pushed as an event to the app on both
  PCs.

**Front ends.** Both use the same protocol:
- **`odkvmctl`**: a cross-platform tray app plus CLI (proposed: Rust + hidapi, one static binary per OS).
  - The tray icon shows the active PC, with notifications on switch and on faults.
  - The CLI allows scripting, e.g. `odkvmctl switch pc2`, `odkvmctl port mouse mode mirrored-identity`.
- **WebHID page** (Chromium-based browsers): zero-install configuration, for machines where you don't want to install anything.

**What it reports**

| Scope | Reported |
|---|---|
| Per front port | device name, VID:PID, speed, interface summary (HID / non-HID, vendor channels detected); current mode and, in Auto, *why* it was chosen; **measured report rate (Hz)**; enumeration errors; over-current faults. For passthrough ports, the identity read during auto-detection (the device is then attached to the PC, not the KVM) |
| Global | active PC; each PC's link state (connected / suspended / unpowered); power source, advertised current and estimated headroom; link health (CRC errors, dropped frames); firmware versions of all three MCUs; self-test result |

**What it controls**
- Per-port mode: Auto, Normalized, Mirrored, Mirrored + identity, Passthrough; profiles of these (§4).
- Screen hop: the arrangements of the screens, which one each profile uses, the pointer's acceleration and speed, and
  the edges (§4).
- What a tap and a long press on the KVM's key do.
- Hotkeys and button behaviour.
- LED colours and brightness.
- PC-side polling: 125 µs, or 1 ms for picky BIOSes.
- VBUS power-cycle when a passthrough port switches.
- Switching from software, and firmware updates.

**Safety.** Software switching and configuration can be turned off from either PC, after an "are you sure" that says
what stops working. Turning them back on needs the key on the KVM: a tap within 30 s of asking for it from software
(the key LED pulses white meanwhile), or the key held while the KVM powers up. So a program on one PC cannot undo a
lock set from the other. Firmware updates always require a physical key press on the KVM, so a program on one PC
cannot silently install firmware that snoops on the other PC.

## 9. Firmware outline

- **RP2354B (C, pico-sdk + TinyUSB host):**
  - Core 0: USB host, HID parsing, normalization and mirroring, hotkey engine, switching state machine.
  - Core 1: SPI link DMA, LEDs, button, power monitoring, config storage, and the AT32F405 programmer (the Artery ROM
    bootloader's SPI protocol over the link pins).
  - At boot, put the on-chip core regulator into high-impedance mode, because DVDD comes from the external LDO.
    Never use its low-power mode (§6).
- **AT32F405 (C, TinyUSB `dwc2` HS device + Artery's firmware library for clocks and GPIO):**
  - The descriptor set (normalized, mirrored or identity) is pushed by the RP2354B, and the AT32F405 re-attaches when
    it changes.
  - SPI slave with DMA.
  - Handles SET/GET_REPORT, boot protocol, suspend / remote wakeup, and the control interface.
  - A single image serves both AT32F405s; a strap pin tells it whether it is PC1 or PC2.
- **Link protocol:** framed, with sequence numbers and CRC.
  - RP2354B → AT32F405: reports, descriptor sets, control-interface replies.
  - AT32F405 → RP2354B (with an IRQ line): output/feature reports, control-interface requests, VBUS/CC status.

## 10. Flashing and bring-up without a programmer

A USB-C cable and a paper clip are the only tools needed.

1. **First flash (blank chips straight from the assembler):**
   1. With empty flash, the RP2354B's ROM boots into its USB bootloader. The TS3USB30E's pull resistor connects the
      RP2354B's USB to the **PWR/SERVICE** port's data lines, so you plug a computer into that port.
   2. An `RP2350` drive appears. Copy `kvm.uf2` onto it.
   3. That UF2 bundles the RP2354B firmware and the AT32F405 image. On boot, the RP2354B drives each AT32F405's BOOT0
      and NRST into its ROM bootloader, then programs it over the link's SPI1 pins (PA4–PA7). This takes a few
      seconds, and no SWD probe is involved.
      - **Second programmer-free path:** in bootloader mode the AT32F405's ROM also offers **USB DFU on its own HS
        port**. That needs the 12 MHz crystal, which the design already has. With BOOT0 held, a PC on PC1 or PC2 can
        flash that side's AT32F405 directly through the hub.
   4. The firmware switches its USB over to Hub-0 and starts normal operation. On boards 0.8–0.12 that link has D+
      and D− crossed in the service switch, and the firmware swaps them back in the RP2354B's PHY
      ([hardware/errata.md](hardware/errata.md) E1).
   5. The built-in **self-test** runs:
      - Hub-0 enumerates;
      - both AT32F405 links answer;
      - each port power switch toggles without a fault;
      - VBUS/CC readings are plausible;
      - Hub-1/2 are seen by their AT32F405 (via VBUS_DET);
      - the Bluetooth module, where fitted, runs the embedded firmware.

      The result is shown as an LED colour code and in the host software.
2. **Normal updates:** from `odkvmctl` or the WebHID page on either PC, through the control interface to the RP2354B.
   - It writes into its inactive A/B flash partition, asks for a key press to confirm, and reboots in
     *try-before-you-buy* mode. A bad image rolls back automatically.
   - It reprograms the AT32F405s, and the Bluetooth module where fitted, if their image changed.
   - Alternatively, reboot into service mode and drop a UF2 on the drive.
3. **Recovery:**
   - **RP2354B:** hold the pinhole button (on `QSPI_SS`) while powering via PWR/SERVICE, and the ROM bootloader comes
     up whatever is in flash.
   - **AT32F405s:** the RP2354B can always force them into their ROM bootloader.
   - **Bluetooth module:** its BOOT strap is on the RP2354B's TX line, so the RP2354B can always get it into its ROM
     bootloader (a UART break across one of its resets) and program it again.
   - All bootloaders live in ROM, so the device cannot be bricked by a bad flash.

During firmware *development*, a debug probe still helps for stepping through code (a $5 Pico 2 running Raspberry Pi's
`debugprobe` firmware, on the optional Tag-Connect pads), but it is never required.

## 11. PCB
- **Stack-up:** JLC04161H-3313, 4 layers:
  - L1: signals and every USB HS pair where possible;
  - L2: solid GND;
  - L3: solid 3V3;
  - L4: signals, and the HS pair hops where lanes cross.
- **HS pairs:** 0.15 / 0.15 mm, about 88 Ω differential over L2. To be confirmed with JLC's impedance calculator before
  ordering.
- **Power rails** (rev 12): each rail is sized for the copper of the layer it runs on: L3 is 0.5 oz, so a rail there
  is 2.3 × its outer width, capped at 1.39 mm. The +5 V feed to the front ports is also sized for drop (1 mV/mm): 1.1 mm
  on L1/L4, 59–91 mV at the USB-rated load (analysis.md §12).
- **Crystals** (rev 12): crystal lines stay on L1/L4 with as few vias as possible; other nets keep out of a crystal's
  footprint and 0.5 mm off its lines, except right at the chip. The RP2354B's 1 kΩ sits at its XOUT pin, as in Raspberry
  Pi's minimal design.
- **Board:** 120 × 80 mm. JLC assembles both sides (2026-10-27): the per-pin decoupling caps under the chips, the
  key's hot-swap socket and the key LED are on the bottom; only the switch itself is plugged in by hand.
  - **Rear edge:** USB-C PC1, PWR/SERVICE and PC2 at a 45 mm pitch, centred.
  - **Front edge:** the Keyboard, Mouse and Other USB-A ports at a 35 mm pitch, centred, each with its status LED
    to its left. The PC1 LED sits at the far left and the PC2 LED at the far right.
  - **Centre:** the MX key at (160, 153.5), right behind the Mouse port. Its 18.1 mm keycap outline (the footprint's
    User.Drawings) clears the Mouse USB-A body by 1.93 mm (Molex 67643, 7.16 mm tall; the keycap only moves straight
    down, so no height can collide) and the switch body by 3.9 mm. Within 2.25 mm of the keycap sit C517 (0402, 0.5 mm
    under its north edge, far below the skirt), C519 (1.0 mm), Y501 (1.65 mm), J1301 (1.93 mm) and J501 (2.10 mm: pull
    the keycap to attach the Tag-Connect cable), so a case rim or post round the key must leave them room.
- **Floor plan:** identical local macros first, then the rest (`gen_pcb.py`).
  - **Input macro, one per PC:** USB-C, ESD, then the hub (ports south, upstream and crystal west), then the AT32F405
    under the hub's east end (OTGHS north, SPI east).
    - PC2's macro is PC1's moved by the 90 mm connector pitch, passives included.
    - The AT32F405 cannot be mirrored, so AT32 #2's SPI side faces the right board edge. Those eight slow lines take
      the long way round to the RP2354B.
  - **Output macro, one per front port:** USB-A, ESD, 1:3 router, VBUS switch and their passives. The three copies
    sit 35 mm apart.
  - **The rest:**
    - The RP2354B sits in the open area left of the key, its AT32 #1 side facing AT32 #1. Its crystal and SWD pads sit
      east of its south-east corner, clear of its via fan-out.
    - Hub-0 sits right of the key. Its three strap pull-downs (R703 CFG1, R702 CFG0, R704 NR1) are placed in pin order,
      each on its own pin's side: the auto-placer had written them west to east as R704, R702, R703 while the pins run
      west to east as 25, 24, 22, so H0_CFG1's and H0_CFG0's tracks had to cross in the strip north of the hub — and
      that crossing is where H0_CFG1 lost its way out (four rip-up rounds, then unrouted).
    - PWR/SERVICE's ESD, the service switch and the power path sit under the middle connector. All three input ESDs
      (`U101`, `U201`, `U301`) sit at y 113.0, i.e. the macro spot 1.0 mm north of where rev 8 had the two PC ones:
      the service ESD had to go there (below) and the other two follow, so the three input blocks stay identical —
      their connector → ESD links are one and the same shape (20 cells, 0.93 mm per line, skew 0.00) instead of the
      service one needing its own fixed copper.
      The service ESD needs the height for the strip between its south pad edge and Hub-0's north pad row, the only
      way out for Hub-0's north pins, its reset line, VBUS_PC1 and the port's VBUS. At 2.495 mm three nets plus the
      SVC pair's convergence could not all pass (Hub-0's pin 25 got no fan-out via, VBUS_PWR never reached the ESD's
      VBUS pad); 4.495 mm does (pad edge 114.44 mm to Hub-0's 118.935 mm).
      1.5 mm north of its rev-8 spot is the measured maximum, not a preference: the connector's pair has no room at
      its own A end (0.5 mm-pitch pads), so its crossover zone has to sit directly below the connector's courtyard
      (south edge 108.97) and shrinks 1:1 with this move. Pairs-only runs: 1.5 mm routes the link J301→U301 (26
      cells, 1.18 / 1.19 mm, skew 0.00), 2.0 mm fails it ("lines only −0.099 mm apart"), 2.5 mm fails with "no pair
      path". That link used to come out with a roundabout: 0.5 mm of straight run at *both* ends (`PAIR_END_RUNS`)
      does not fit between the crossover zone and the connector's centre line, so the search ran south past the zone
      and came back north to enter it — 38 cells, 1.78 / 1.19 mm, skew 0.59. The end-run keys are now per part and
      pad (`('SVC_DP', 'B', 'U301.4')`, i.e. the switch-side link arriving on the ESD's south pads), and the
      connector's link keeps the default 0.2 mm run at its B end: 26 cells, skew 0.00, the same shape as the two PC
      links.
      The PC macros' own links were re-measured at the new height: J101/J201 → U101/U201 20 cells, 0.93 mm each,
      U101/U201 → U801/U901 172 cells, 10.23 / 9.73 mm, skew 0.50 mm.
  - **Auto-placed passives** stay out of an escape ring around each fine-pitch IC. They keep 0.35 mm apart, so a
    track fits between any two, and sit on the side of their pin that faces their other connection.
- **USB lanes** (`route.py` `LANES`): both hubs' lanes leave westwards and run down the west side of their input
  macro.
  - **Hub-1:** the Keyboard lane stays on F.Cu.
    - The Mouse and Other lanes dive under the Keyboard router and cross the board on B.Cu.
    - They surface just before their own routers.
  - **Hub-2:** lanes cross on F.Cu, in the band between the key and the routers. The Other lane drops straight into
    its router.
  - **Nesting:** the outermost lane always turns last, so no two lanes cross.
  - **Length:** the longest link, Hub-1 → Other router, is about 130 mm, within the usual USB 2.0 budget for a 90 Ω
    microstrip.
- **In the case:** the side LEDs shine through front-panel holes. Four M3 holes: two in the rear corners, and two at
  y = 170 near the front corners, which leaves the front corners for the PC LEDs. The bottom side carries only the
  key's socket and LED and the 0402 decoupling caps under the RP2354B, the two AT32s and the three hubs (32, at most
  0.5 mm tall), so the case can have a support post right next to the key.
- **Key parts** (2026-10-27: JLC assembles the socket and the LED with the rest of the bottom side; the switch and
  keycap are plugged in afterwards). What to order:

  | Item | Part | Note |
  |---|---|---|
  | Hot-swap socket | **HanElectricity CPG151101S11-2** (LCSC C49352235; the Kailh CPG151101S11, C2803348, had no stock) | bottom side, two pads, **JLC** |
  | Switch | any **MX-compatible switch, 5-pin (PCB-mount) preferred**, with an **LED window or clear housing** | 5-pin: the two plastic pegs locate it without a plate. 3-pin plate-mount switches fit too, but have no pegs to locate them |
  | Keycap | 1u MX-stem keycap, **translucent or shine-through** | |
  | Key LED | **SK6812MINI-E** (LCSC C5149201) | bottom side, body through the cut-out, lens up, **JLC** |

  Every MX switch has its LED window opposite its two contact pins, and the footprint puts the LED there, so any
  switch with a window lines up.
- High-Speed paths (front port → switch → PC hub, and hub ↔ AT32F405) are kept short and symmetric with no stubs. Both
  D+/D− pin pairs on the USB-C connectors are tied right at the pads, using OpenRower's breakout as the model.
- Each connector shield connects **directly to GND** (the `SH` pads of every USB-C and USB-A footprint).
- Parts are chosen for JLCPCB/LCSC PCBA. The 0.4 mm QFN switches and the QFN-80 MCU are not hand-solder friendly.

## 12. Risks and de-risking
1. **HS device timing and TinyUSB on AT32F405:** prototype first on an Artery AT-START-F405 board plus a Pico 2, linked
   over SPI. Measure the added latency with a USB analyser or an LED-to-photodiode rig before the PCB is ordered. If
   TinyUSB's AT32F405 support falls short, the LPC55S69 is the backup.
2. **Mirrored + identity versus vendor software:** test the makers' configuration software of several mice and keyboards against the composite with appended
   interfaces on the same prototype. If a driver rejects it, take the OTGFS1 separate-device fallback in §4. That
   changes the PC hubs to USB2517, so decide before the PCB.
3. **AT32F405 bootloader:** the datasheet (table 5) confirms SPI1 on PA4–PA7 for every package. The SPI framing still
   needs confirming from Artery's bootloader application note (§15).
   - In bootloader mode the ROM also listens on:
     - USART1 PA9/PA10 and USART2 PA2/PA3;
     - I2C1 PB6/PB7, I2C2 PB10/PB3 and I2C3 PA8/PB4;
     - CAN1 PB8/PB9;
     - OTGFS1 PA11/PA12;
     - OTGHS1.
   - The schematic keeps those pins unconnected, or connected only to signals that stay quiet during flashing.
4. **Thin JLC stock:** TS3USB3031 (82) and AT32F405CCT7 (174). Re-run the stock check before ordering, and use JLC
   pre-order or global sourcing where needed. The Abracon inductor (4 in stock) is no longer needed (§6).
5. **The power budget** is an estimate. Measure the 3V3 draw on the prototype.
6. **Mouse and keyboard compatibility** through Hub-0 at FS: test with the actual peripherals you will use.
7. **TS3USB3031 HS signal integrity:** it is a single analog switch with 1 pF and 4.5 Ω on its USB1/USB2 paths, well
   within USB 2.0 margins. Its **MHL path** is specified at 1.5–3.3 V signal levels (about 5 Ω and 1.3 pF at USB HS
   levels), and PC1's passthrough runs through it, so PC2's passthrough is datasheet-guaranteed while PC1's is not
   (report.md F4; the alternative — Hub-0, full speed, on the MHL path — needs the HS lanes re-planned). Verify both
   with an eye-pattern check or a long `fio` copy on a flash drive in passthrough, **PC1 first**. If PC1 turns out
   marginal, keep 480 Mbit/s devices on PC2, or re-plan the lanes for a revision.

## 13. Leaner alternative (not recommended)
Drop the three hubs and the three switches. Use 2× RP2354, each a Full-Speed device to its PC and each running a PIO-USB
host (the DeskHop approach).
- **Cost:** about $10 less.
- **Trade-offs:**
  - about 0.5 ms more average latency;
  - PIO-USB compatibility risk;
  - no passthrough, so non-HID devices on the Other port won't work at all, and vendor software can only be supported
    through Mirrored + identity.

## 14. KiCad workflow (adopted from OpenRower)

The sister project `../OpenRower/hardware` generates its KiCad files from scripts, checks them, and derives JLC's
placement corrections from data instead of guesswork. Its tools get **copied** into `hardware/tools/` here and adapted
there; OpenRower itself is never modified.

| Tool in `hardware/tools/` | Origin | Role |
|---|---|---|
| `sexpr.py`, `check_netlist.py` | copied from OpenRower | S-expression I/O; KiCad netlist vs. the generator's pin map |
| `gen_symbols.py` | new | `KVM.kicad_sym`: AT32F405CCT7, TS3USB3031, TPS2121, LM66200, TPS2553, SK6812SIDE-A |
| `gen_footprints.py` | new | footprints converted from JLC's own EasyEDA land patterns (TPS2121 RUX, SK6812SIDE-A), plus the MX switch in a Kailh hot-swap socket (socket pads on the bottom) |
| `gen_schematic.py` | adapted | netlist block by block. References are numbered per block (R501…), and symbols are laid out automatically on one A0 sheet |
| `gen_pcb.py` | adapted | creates the 4-layer board; places 57 main parts from a table and every passive automatically next to the pin it serves |
| `check_spacing.py` | adapted | courtyards (circles measured as circles), pad gaps, edges. No JLC rails at 120 × 80 mm |
| `jlc_align.py` + `jlc_corrections.json` | adapted | JLC rotation/offset per LCSC part, solved from EasyEDA pad data. All 41 parts are resolved, with residuals of 0.21 mm or less |
| `make_fab.py` | adapted | JLC BOM + CPL (corrections applied; only parts with an LCSC number), the hand-assembly list, 4-layer gerbers, drill |
| `route.py` | extended | 4-layer router, described below |

**Router (`route.py`).** It is OpenRower's exact-geometry grid A* (0.05 mm) with rip-up, extended for this board:
- **Layers and planes:** signals on F.Cu and B.Cu (equal cost); slow signals and power may also use In2.Cu, where the
  +3V3 pour flows around them. In2 stays solid under (and 0.6 mm beside) every high-speed B.Cu USB pair, its reference.
  Slow tracks may cross under Hub-0's full-speed pairs. In1 stays a solid GND plane. A check after routing re-routes In2 tracks that would cut a +3V3 connection off the plane.
- **Plane vias first:** every GND pad gets a via to In1 and every +3V3 pad one to In2 before any signal is routed;
  plane pins between USB pins (the hubs' VDD33) get theirs even before the pairs. A signal blocked only by such
  fan-out copper lifts it, and the pad gets a new via at the end. When signals wall a pad in, they are ripped up.
- **Fine-pitch ICs:** no F.Cu inside a QFN's pin ring and no vias just outside it; wide power tracks stay out of the
  RP2354B, hub and AT32 areas unless they have a pin there.
  - **Staggered via fan-out:** every used pin of the RP2354B, the three hubs, both AT32s and the service switch (shallow
    rows there, so its select pin's via sits just below it) gets a planned way out:
    straight out, one 45° jog, then a via in a near or a far row, alternating along the side. Vias are 0.6–0.8 mm
    apart along the side, so vias in one row are two via widths apart or more, and the rows spread into the corner
    space where the depth allows.
  - USB and SPI pins are planned first, then the north, south, east and west sides. USB pair pins keep their pair
    fan-out instead.
  - A signal pin's way out is a reservation: other nets keep off it, and the net itself may use it or not. It goes once
    the net is routed. Plane pins get theirs as copper.
  - Where a plan is blocked (a pair or a part in the way), the nearest legal site within 1.6 mm is used, if a short
    path reaches it.
  - A pin with no legal via site at all still keeps its **straight radial run** as a lane-only reservation, added
    after every other pin's site is placed so it cannot take one. Rev 9 measured 15 such pins (U501.48/50/51,
    U701.25/27, U801/U901.26/27, U1001.2/5/6/7/9/10/11, U1101.8) and they had nothing at all: the next net routed past
    their exit sealed them (VSNS_PWR by U501.48's fan-out via 0.7 mm from R304.1, H0_CFG1 by HUB0_RST's lane through
    the 1 mm funnel beside U701.25). The lane is what every pin has; the net leaves along it and turns where it ends.
  - A pad whose only way out is into its own part gets the same treatment, by hand. The PWR-port ESD's VBUS pad
    (U301.5) sits between the SVC pair's two exit stubs, 0.35 mm from its neighbours on both sides, and the one slot
    with room for a via is shared with pad 2's GND via. `PAD_ESCAPE_INWARD` reserves the slot's *east* half for
    VBUS_PWR (0.28 mm out from the pad centre, via 0.40 mm east and 0.64 mm north); `POWER_ORDER` then routes
    VBUS_PC1 first, because with VBUS_PWR first VBUS_PC1 lost the strip south of the ESD (5 of 9 pads unrouted) and
    the retries made it worse (`+1V1`, `AT2_NRST`, `H0_CFG0`, `H0_CFG1`, `VBUS_PC1` unrouted). The trap is
    pad-level, so the ESD's 1.5 mm on the strip does not remove it. Pad 2's GND via is **not** hand-placed: a
    hand-placed one is tuned to a fixed offset and lands between the two pad rows, and once the ESD moved north it
    sat on pads 4-6 while `plane_fanout` added its own as well (two GND vias on one pad, one of them illegal).
    `plane_fanout` picks it again, in the same slot west of VBUS_PWR's via.
  - In front of the RP2354B's pins, F.Cu moves are radial only (0.8 mm deep).
  - Each input hub's CFG1 pin (under the upstream pair's westward run) and AT32 #1's NRST pin (beside its crystal) get
    no via site and leave on F.Cu.
- **Windowed search:** each connection is searched inside a window around its ends, which is grown on failure. A failed
  search is retried from the pin's end: from the open side, the search can run out of steps before it finds a pin's
  way out of a dense fan-out.
- **Full-speed USB lines** (normal nets) route one line at a time. While a line routes, the straight exits of the IC
  pins still waiting stay clear: its partner's pins (so its via cannot land in front of them) and its own pins'
  neighbours (so it cannot wrap round in front of a select pin).
- **Power rails** are sized from the current they carry (IPC-2221 external layer, 1 oz copper, 20 °C rise:
  `rail_width()` in `tools/route.py`, which reproduces 0.6 mm for +5 V's 2.4 A and 0.5 / 0.4 / 0.3 mm for the
  lighter rails). A rail runs at that width the whole way and necks down only at a pad narrower than the rail —
  within 0.5 mm of its own pad, or through a dense IC's via fan-out, whose pin is otherwise unreachable at full
  width. A bottleneck anywhere else is cleared by rip-up, so a rail is never thinned just to squeeze past another
  net's via or pad. On +5 V the LEDs, their buffer and decoupling join last, on 0.3 mm branches, so no port switch
  hangs off a thin LED branch. Among the rails the order follows how local the link is: **VBUS_PWR first** — the
  PWR-port ESD's VBUS pad sits in a pocket closed by the SVC pair's two exit stubs and its own decoupling (D302, C301,
  R303), and the two long front-port through-rails (VBUS_PC1/PC2) take exactly the corridor it has to cross; routed
  after them it had no way out at all (rev 9).
- **The board edge stays clear.** No track and no via comes within `EDGE_KEEP` (1.6 mm) of the board edge, except
  near a pad of that net, so the parts mounted along the edge (the five side LEDs, the connectors) keep their escape.
  A track pays a heavy per-step cost there, a via is barred. The status LED chain is also routed *before* power: its
  data line runs the length of the front edge between the LED pads, where a wide +5 V rail used to take the space
  first and cut the chain off (the baseline board left two chain links unrouted because of it).
- **USB differential pairs:**
  - A 0.45 mm "fat" centre line is routed and then split into two coupled 0.15 mm lines, 45° turns only.
  - Via pairs handle layer changes.
  - Fan-out stubs reach pads of any pitch, from 0.4 mm QFN to 2.0 mm USB-A.
  - A swap macro handles the two AT32 ↔ hub links, whose pin order otherwise forces a D+/D− crossing.
  - The long hub → router links follow a **lane plan** (`LANES`): waypoints in the middle of each straight run, each
    with its layer and direction.
    - Each leg between waypoints stays on one layer. Layer changes happen only at waypoints, as via pairs.
    - Waypoints at both ends of every corner keep lanes from cutting across the inside of a corner, where an inner
      lane runs.
  - Each hub's three switch links are retried in every order until all three route.
  - Hub-0 runs at Full Speed: its links are routed as pairs after the HS lanes and the AT32 SPI, preferring B.Cu, with
    one lane (Keyboard, north of AT32 #1 on B.Cu) so they leave the AT32's F.Cu side free.
- **Priorities:** USB, SPI and crystal nets are only ever ripped up for each other, never for a lower-priority net.
  - The signal order puts the short local nets first, then every other net on the RP2354B in nesting order, the hub
    straps, the power rails, the control lines, and the rest by length. The hub straps were once ordered before the
    short-net rule, the strap whose pull-down sits furthest out first (rev 9: H0_CFG1's R703 is 5.7 mm out from pin 25,
    and H0_CFG0's route sealed the 1.2 mm funnel beside that pin). That turned out to be the wrong lever: routed before
    HUB0_RST, the straps walled in HUB0_RST, H0_CFG0 and AT1_NRST instead. The crossing they were meant to survive is
    a *placement* fault and is fixed there — Hub-0's three strap pull-downs sat in the opposite x-order to their pins
    (see the placement section) — so the rule is gone and the straps go in the plain short-net order.
  - The rip-up search for a blocked net only goes through copper that may be ripped. It never crosses a pair, a
    protected net or a net at its rip-up limit, so it finds the nets that can make room.
  - A net walled in by the fan-out reservations of nets still waiting has those nets routed first.
  - A routed pair drops its escape corridors before the next fan-out.
- **Speed and observation:** the A* inner loop is C (`tools/astar.c`, loaded with ctypes, identical results to the
  Python reference); threads plan the next nets and commit them in order, re-planning any plan whose search area changed,
  so results equal one-by-one routing. `route.py --live` serves an all-layer live view on http://localhost:8765/.
- **Fixed copper:**
  - the USB-C D+/D− row joins (D− behind the pads, D+ in front of them, leaving a clean D+-west pair);
  - straight runs through each flow-through ESD;
  - direct USB-A → ESD lines;
  - 3 × 3 via arrays in the QFN exposed pads.

**Part orientation rules.**
- **JLC corrections:** all JLC rotations and offsets come from `jlc_align.py`, never from memory. On this board:
  - SOT-23-5 and most SOT-23-6: +180°;
  - QFN-36, LQFP-48, SOT-583, TSSOP-10 and USBLC6-2SC6: +270°;
  - HRO USB-C: −1.50 mm along its axis;
  - USB-A: (+3.5, +1.36) mm, because its KiCad origin is pin 1.
- **Polarity:** the LEDs are 4-pin parts matched by pin number. There are no 2-pin polarized parts.
- **RP2354B core supply:** external LDO (§6), so there is no polarity-sensitive inductor to orient.
  - The crystal, the decoupling and the USB pins still follow Raspberry Pi's minimal design in
    `datasheets/rp2350-minimal-kicad/`.
  - The Abracon fallback would copy that design's regulator block, including the direction of the inductor's
    polarity dot.
- **Side LEDs:** the SK6812SIDE-A footprint comes from JLC's land pattern. Light leaves on the long face opposite the
  pads, so the LEDs sit at 0° with their lens 0.5 mm from the front edge (since 0.17.0; before, at 180°, they shone
  into the board: hardware/errata.md E3).
- **Key LED:** KiCad's SK6812MINI-E reverse-mount footprint, flipped to the bottom 5.08 mm south of the switch centre,
  under the switch's LED window. The router treats its cut-out as board edge.
- **Silkscreen:** references of 0402/0603 parts go on F.Fab, so the silkscreen stays readable (as OpenRower).
- **Origin:** the place/drill origin is the bottom-left corner, shared by the CPL and the gerbers.

## 15. Datasheets

**Downloaded** to [datasheets/](datasheets/):
- **MCUs:**
  - `rp2350-datasheet.pdf`, `hardware-design-with-rp2350.pdf`, `rp2350-product-brief.pdf`, `pico-2-datasheet.pdf`;
  - `rp2350-minimal-kicad/`: Raspberry Pi's RP2350A minimal design (KiCad schematic, PCB and BOM), the layout
    reference for the regulator, crystal and USB;
  - `DS_AT32F405_402_V2.03_EN.pdf`, `RM_AT32F402_405_EN_V2.02.pdf`, `AN0184_AT32F402_405_Get_started_guide_V2.0.3_EN.pdf`
    (all supplied by you). `at32f402-405-datasheet.pdf` is the older Chinese v2.02 and is superseded;
  - `stm32f722-723-datasheet-DS11853-rev3-2017-OUTDATED.pdf`. This 2017 revision predates the F723 in LQFP-100; it is
    kept only for reference now that the STM32 is dropped.
- **USB and switching:** `usb2514b-datasheet.pdf`, `ts3usb3031.pdf`, `ts3usb30e.pdf`.
- **Power:** `tps2121.pdf`, `tps2553.pdf`, `lm66200.pdf`, `lm66100.pdf`, `tlv62569.pdf`, `tlv733p.pdf` (RP2354B core
  LDO).
- **Protection and logic:** `usblc6-2.pdf`, `sn74ahct1g125.pdf`.
- **LED:** `sk6812mini-e.pdf`.
- **Connectors and socket:** `hro-type-c-31-m-12.pdf`, `gct-usb4105-drawing.pdf`, `cnctech-1001-011-01101-usb-a.pdf`,
  `kailh-cpg151101s11-hotswap-socket.pdf`, `hanelectricity-cpg151101s11-2-hotswap-socket.pdf`.
- **USB specs:** `usb-hid-1_11.pdf`, `usb-hid-usage-tables.pdf` (HUT 1.6).

`an2606-…pdf` (STM32 bootloader, supplied by you) is no longer needed with the STM32 dropped.

**Still missing.** These sites blocked automated download:
- **Artery bootloader (ISP) protocol application note:** the SPI framing used for the RP2354B → AT32F405 flashing
  path. It is on Artery's documentation pages: https://www.arterychip.com/en/support/index.jsp?index=1
- **AT-START-F405** evaluation-board manual and schematic (for the prototype): https://www.arterychip.com/en/product/AT32F405.jsp
- **Molex 67643-0910** drawing (the Molex site timed out): https://www.molex.com/en-us/products/part-detail/676430910
- **Nexperia PESD5V0S2BT** (HTTP 403): https://www.nexperia.com/product/PESD5V0S2BT
- **Abracon AOTA-B201610S3R3-101-T** datasheet: https://www.lcsc.com/product-detail/C42411119.html (the inductor's
  footprint already comes with Raspberry Pi's minimal design)

Not needed any more with the STM32 dropped: DS11853 (current revision), RM0431, AN4879, AN4286 and ES0360. st.com
refused all connections from here.

## 16. Bluetooth hosts (0.16.0)

Up to four extra hosts (PCs, tablets, phones) take the KVM's keyboard and mouse over Bluetooth LE, alongside the two
USB PCs. They are lesser targets: the KVM's own keyboard, mouse and media reports only (no mirroring, identity or
passthrough), and the window and the LEDs stay the two-PC KVM until a host is paired. Firmware and control protocol:
software.md §4.13.

- **Module:** ESP32-C3-MINI-1-H4X (LCSC C41349510), pre-certified, with its PCB antenna. Chosen over a bare nRF54L or
  ESP32-C3 for the certification it carries (the board's radio tests reuse the module's) and over an nRF54L15 module
  for price and LCSC stock. Bluetooth LE only: Classic-only devices (some keyboards, mice and game controllers) stay on the USB
  ports.
- **Placement:** on the top side near the south-east corner (centre 208.5 / 156.0), its antenna end at x 219.5,
  0.5 mm inside the east edge (nothing overhangs the board), south-east of Hub-2's lanes; its UART pads face south.
  0.16.0 put it on the bottom; on 2026-11-12 it moved to the top with its passives (same centre, the layout mirrored),
  so that the bottom side carries only the decoupling caps, the key's socket and its LED. A rule area keeps every
  copper layer, every plane included, out from the antenna's start to the edge and 15 mm along the edge each side
  (Espressif's clearance), as far as H1504. The module's ground pads join the F.Cu ground pour. The logos moved for
  it: the OpenDynKVM logo behind the OTHER port, the OSHW logo and the version at the rear between PC1 and BOOTSEL.
- **Interface:** UART0 on GPIO28/29, and from 0.18.0 GPIO9 to the module's EN through 1 kΩ (R1605, beside the EN
  pad). The module's BOOT strap shares the RP2354B's TX line, so the RP2354B gets the module into its ROM bootloader
  with a link command, or with a UART break across a reset. From 0.18.0 it makes that reset itself on EN. Before, it
  waits for one the module makes by itself. It then programs the module from its own image, as it does the AT32F405s.
  On 0.16.0 and 0.17.0 EN has only its RC. The line could not leave GPIO30, beside the UART, in a seeded route
  (hardware/debug/v18-esp-en/).
- **Without it on the board:** an ESP32-C3 board with the chip's own USB port, on a front port, can be the module
  instead (0.12–0.15 boards, the `_nobt` variant): the same firmware over its USB serial port, programmed and kept up
  to date by the RP2354B as the fitted module is. It takes that port; an ESP32 board with other firmware is never
  touched (software.md §4.13).
- **Assembly variants:** the module and its eight parts carry `Fit = BT`. `tools/make_fab.py` writes the BOM and CPL with
  them (`jlcpcb_*.csv`) and without them (`jlcpcb_*_nobt.csv`). The same firmware runs both, and boards 0.12–0.15: it
  looks for the module at boot and leaves Bluetooth out when there is none.
- **Selling it:** the radio is covered by the module's grants (FCC ID label "Contains FCC ID …", RED declaration with
  the band and the maximum power in the manual). Calling it Bluetooth needs a Bluetooth SIG listing ($12,000 per design
  for an Adopter, 2026), with or without advertising the word. Until then, build the `_nobt` variant; the firmware
  needs no change either way.
