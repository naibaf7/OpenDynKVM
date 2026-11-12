# Board errata

Faults found in a board after it was ordered. Each entry says which hardware versions it affects (DEVELOPMENT.md "Versions"),
what goes wrong, how firmware or software works around it, and what the next board has to change. An entry stays here
after it is fixed, marked with the version that fixed it, because boards with the fault stay in use and the workaround
has to stay in the firmware.

| ID | Fault | Affects | Workaround | Fixed in |
|---|---|---|---|---|
| [E1](#e1-dd-crossed-between-the-rp2354b-and-hub-0) | D+/D− crossed between the RP2354B and Hub-0 | 0.8.0 – 0.12.0 | RP2354B firmware swaps the pair in its USB PHY | 0.15.0 (designed, not yet ordered) |
| [E2](#e2-the-service-port-needs-svc_sel-held-high-by-hand) | The service port needs SVC_SEL held high by hand: R510 (100k) loses to the RP2350's pad pull-down | 0.8.0 – 0.16.0 | Firmware updates over a PC port; for the boot ROM, bridge R510 or fit 10k | 0.17.0 (designed, not yet ordered) |
| [E3](#e3-the-side-leds-shine-into-the-board) | The five side LEDs shine into the board, not out of the front edge | every board up to 0.16.0 | None (the light still shows, dimmer, around the parts) | 0.17.0 (designed, not yet ordered) |

## E1: D+/D− crossed between the RP2354B and Hub-0

**Affects:** hardware 0.8.0 to 0.12.0. The fault came in with rev 8 (2026-10-25), and 0.12.0 is the
board that was ordered. Found on 2026-11-05 by tracing the netlist against the TS3USB3031 datasheet; not yet seen on a
board.

### What is wrong

The service switch U601 (TS3USB3031) gives the RP2354B's USB either to Hub-0 (SEL0 low) or to the PWR/SERVICE port
(SEL0 high). Inside the part, the common D+ pin (11) switches to USB1+, USB2+ and MHL+, and the common D− pin (10) to the
minus pins (datasheet Table 6-1). `gen_schematic.py` block 6 wires it like this:

| U601 pin | Datasheet name | Net | Line |
|---|---|---|---|
| 11 | D+ (common) | `RP_DM` | RP2354B D− |
| 10 | D− (common) | `RP_DP` | RP2354B D+ |
| 3 | USB1+ | `H0_UP_DP` | Hub-0 upstream D+ |
| 4 | USB1− | `H0_UP_DM` | Hub-0 upstream D− |
| 5 | USB2+ | `SVC_DM` | PWR/SERVICE D− |
| 6 | USB2− | `SVC_DP` | PWR/SERVICE D+ |

Following the RP2354B's D+ through the part:

- **To the service port:** pin 10 → pin 6 = `SVC_DP`. Straight.
- **To Hub-0:** pin 10 → pin 4 = `H0_UP_DM`. **Crossed.**

The common pins and USB2 are both reversed, so they agree with each other. Hub-0's channel, USB1, is the only one wired
by the datasheet names, which makes it the odd one out.

**How it happened:** rev 8 swapped the pair on two sets of pins so the router would not need a crossover:
- the RP2354B's pair on the common pins, because the RP2354B's D+ pin is west of its D− pin;
- the service connector's pair on USB2.

The comments in block 6 give the reason as "the switch is a passive pass-through: D+ and D− are interchangeable on its
pins". That is true only if every channel is swapped together with the common pins, and USB1 was left as it was.

### Effect without the workaround

- **Hub-0 does not enumerate.** Its pull-up on D+ arrives on the RP2354B's D−, so the RP2354B sees a low-speed device
  that never answers. Nothing on the three front ports works in Normalized or Mirrored mode: no hotkeys, no persistent
  emulation, no keymap. The self-test reports Hub-0 as failed (key LED).
- **Unaffected:**
  - Passthrough: the front-port switches go straight to Hub-1 and Hub-2, and every one of their channels is correct.
  - The service port: the boot ROM's `RP2350` drive, first flashing, `odkvmctl reboot bootloader`.
  - The PC links, the AT32F405s, power and the LEDs.

### Workaround: RP2354B firmware

The RP2350 can swap D+ and D− inside its USB PHY: `USB_MUXING.SWAP_DPDM`, bit 31. The datasheet describes it as "Swap
the USB PHY DP and DM pins and all related controls and flip receive differential data. Can be used to switch USB DP/DM
on the PCB". Since only the Hub-0 path is crossed, only the firmware's host mode needs the swap.

- **Host mode** starts with the swap on (`CROSSED_AT_START` in [`firmware/rp2354/src/usbh.rs`](../firmware/rp2354/src/usbh.rs)).
- **The service port needs nothing.**
  - Its path is straight.
  - The boot ROM's USB bootloader starts with the bit cleared. The firmware gets there through a full ROM reboot
    (`system.rs`); in the bootloader only SVC_SEL's pull-up holds the switch on the service port, which boards up to
    0.16.0 need a hand with (E2).
  - **Do not set** the OTP flag `USB_BOOT_FLAGS.DP_DM_SWAP`, which makes the boot ROM swap too. It would cross the
    service port on every board.
- **One firmware image for both kinds of board.** The board carries no ID, and one image serves all 0.x boards
  (DEVELOPMENT.md "Versions"), so the firmware can't know whether it runs on a crossed board or a fixed one.
  - **How it detects a fixed board:** only Hub-0 is ever on the RP2354B's root port, and a hub is never low speed. So if
    the root port sees a low-speed attach, the pair is crossed for the current setting.
  - **What it does then:** the firmware flips the swap once, restarts the USB controller (nothing is enumerated yet),
    and waits for Hub-0 again.
  - **Cost on a fixed board:** about 0.2 s more at power-up, and one line on `DBG_TX`:
    `USB: Hub-0 seen as low speed, D+/D- swap now false (errata E1)`. An affected board logs nothing.
  - **Later:** once fixed boards are the common case, set `CROSSED_AT_START = false` to spare them the restart. Crossed
    boards keep working through the same flip, so the detection itself stays in the firmware for good.

**How sure this is.** The USB pins do bypass the GPIO function select (the IO mux), but the swap is not there: it sits
in the USB controller's own `USB_MUXING` register (offset 0x074), between the controller and its PHY. The RP2040 has no
such bit, which is why many sources say D+/D− can't be swapped in software.
- **Datasheet** (build of 2026-10-01): register table 1322, quoted above. The OTP flag `USB_BOOT_FLAGS.DP_DM_SWAP` exists
  "to support board layouts with mirrored USB routing (deliberate or accidental)". Appendix D lists no erratum on
  either.
- **Used on a board:** in device mode, someone fixed an RP2350B board with D+/D− swapped by setting this bit (Raspberry Pi
  forum, "Swapping USB data lines", February–March 2026).
  - The trap there was the SDK's USB init, which rewrites `USB_MUXING` and clears the bit.
  - Here the bit is set in the same write as `TO_PHY`, right after the controller's reset, and nothing else in the
    firmware writes the register.
- **Not confirmed anywhere: host mode.** Host mode also needs the controller's speed detection (which line Hub-0's
  pull-up is on) to follow the swap. "This is done at a low level so overrides all other controls" says it does. Step 1
  below shows whether it does.

**First board (0.12.0, 2026-11-11):** step 1 passed. With firmware 0.1.2, `odkvmctl diag` showed *Hub-0 link*
full-speed, *Hub-0 enumerated* yes, *D+/D− swap (errata E1)* on, and the self-test passed: the swap works in host
mode, speed detection included. No device was on the front ports yet. Steps 2 and 3 are still open (step 2 also
tests E2's reboot path). The checklist, as written before bring-up:

1. **Hub-0 enumerates.** The self-test's key LED shows green for Hub-0, and `DBG_TX` has no `errata E1` line.
   `odkvmctl diag` shows the same without a serial adapter: *Hub-0 link* full-speed, *D+/D− swap (errata E1)* on.
   - **If a 0.12 board logs `swap now false`** (diag: swap off, found at run time): the swap moves the data but not the speed detection. The auto-detection
     then turns the swap off on exactly the board that needs it.
   - **And then:** the controller would also run Hub-0 at low speed. `SIE_STATUS.SPEED` is read-only, and the USB
     chapter shows no way to force a host's speed, so that case probably needs a rework.
2. **The service port still works.** `odkvmctl reboot bootloader` brings up the `RP2350` drive on PWR/SERVICE.
3. **The detection works.** Build once with `CROSSED_AT_START = false`. The log should show
   `D+/D- swap now true`, and Hub-0 should still enumerate. This tests the fixed-board path on a crossed board.

### Fix in 0.15.0

**Netlist** (since 0.14.0). Every pair pin of U601 carries the line the datasheet names it for (since 2026-11-06), so
the part reads straight on its own, not only end to end. `hardware/tools/gen_schematic.py` block 6:

| U601 pin | Datasheet name | 0.12.0 | 0.14.0 and later |
|---|---|---|---|
| 11 | D+ (common) | `RP_DM` | `RP_DP` |
| 10 | D− (common) | `RP_DP` | `RP_DM` |
| 3 | USB1+ | `H0_UP_DP` | `H0_UP_DP` |
| 4 | USB1− | `H0_UP_DM` | `H0_UP_DM` |
| 5 | USB2+ | `SVC_DM` | `SVC_DP` |
| 6 | USB2− | `SVC_DP` | `SVC_DM` |

Hub-0's channel stays as 0.12.0 had it; the common pins and the service channel are swapped back. `check_netlist.py`
reports `0 crossed`, and its `KNOWN_CROSSED` list is empty.

The switch itself does not mind either way. The TS3USB3031 is passive and bidirectional (TI: it can switch I²C, UART
or LVDS as well), every figure in its datasheet applies to the + and − pins alike (R<sub>ON</sub> 4.5 Ω, matched to
0.1 Ω "between + and − paths"; C<sub>ON</sub>, bandwidth, leakage, the 5.5 V tolerance and power-off protection on all
I/O pins), and nothing in it senses which line is which. The +/− names only say which pins switch together, and that
is what E1 broke.

**Copper** (0.15.0, routed from scratch). Rev 8 swapped those pins to save two crossovers: the RP2354B's D+ pin is west
of its D− pin, and so is the service ESD U301's D+ pad, while the switch has D+ on the other side. Those two pairs
now cross over in copper. `route.py` routes each of the two links as if it needed no crossover, then splices a 1.6 mm
twist into a straight stretch of its own new lines (`twist_at='run'`, `twist_in_run`), choosing the stretch, and the
height of the crossing line's S, for the least skew:

- **RP pair** (`RP_DP`/`RP_DM`, from the 27R resistors to U601): twist at (137.70, 115.46), in its leg up the switch's
  west side, vias 0.65 mm out. 20.72 / 20.71 mm, skew 0.02 mm (0.12.0: 0.61 mm).
- **Service pair** (`SVC_DP`/`SVC_DM`, from U301 to U601): twist at (148.94, 113.60), in its run east of the switch,
  vias 0.65 mm out; that link alone has 0.02 mm of skew. The whole net, connector to ESD to switch: 33.71 / 34.07 mm,
  skew 0.36 mm (0.12.0: 0.88 mm).
- **Hub-0's pair:** straight, 21.19 mm per line, no vias.
- **Every other USB link** has 0.12.0's lengths, skew and vias exactly (`check_usb_si.py`).
- Both twisted links run at full speed (12 Mbit/s), where two vias in a pair cost nothing.

What a full run needed besides the twists (2026-11-07, all in `route.py`):

- **The service connector's link** (J301 → U301) had failed in every unseeded run since 0.12.0: "lines only 0.050 mm
  apart at (160.25, 109.36)", from a 0.5 mm end run at the connector that only the switch end needs. With the default
  run it routes exactly as the two PC links do (20 cells, skew 0.00 mm).
- **The pair verdict was kept per net**, so the switch link's success hid that failure ("2 of 2 link(s) routed"). It is
  kept per link now. And a failed attempt removes only the copper it laid itself: it used to remove every piece of its
  two nets, the other link's and the connector's fixed breakout included.
- **Two waypoints pointed the wrong way** and folded a line onto its partner at the turn: `H0_KB`'s (110.5, 147.5),
  marked north in a north-east leg ("lines only 0.137 mm apart"), and the RP lane's last one, on the corner instead of
  in the short leg into the switch ("0.063 mm").
- **Room for thermal spokes:** other nets now keep 0.7 mm from every GND through-hole pad on the outer layers (the 21
  connector shield and ground pins). The first full run put +5V and `VBUS_PWR` on B.Cu either side of J301's
  south-west shield pad, which left it one spoke into a 3.4 mm² island: DRC `starved_thermal`, its only violation.

**Checks** on the routed board (739 s, 0 unrouted nets):
- DRC with schematic parity: 0 violations, 0 unconnected pads, 0 footprint errors.
- `check_copper.py`: PASS, with no accepted `H0_D4` pinch this time. `check_spacing.py` and `check_aux.py`: pass.
  `check_netlist.py`: 0 crossed.
- `check_usb_si.py`: 0 hard failures.
- The +5 V feed (`dc_drop.py`, [jlcpcb.md](jlcpcb.md) §5): 96 / 87 / 66 mV to the port switches at the USB-rated load
  (0.12.0: 91 / 82 / 59 mV).
- The board is the third of three full runs from the same sources. Their USB copper is identical, item for item;
  signals and rails differ, as the router is not bit-deterministic. The second passed every check above as well, but
  its +5 V trunk ran 96.5 mm on the 0.5 oz In2.Cu, for 121 mV, and the first (before the thermal-spoke change) dropped
  495 mV. No board check sees that, so the drop is measured after every full run now (jlcpcb.md §12).
- The run's live view and log: [debug/v15-full/](debug/v15-full/).

**0.13.0 and 0.14.0, designed and never ordered.**
- 0.13.0 fixed the fault the other way round: it swapped U601's USB1 pins (3 = `H0_UP_DM`, 4 = `H0_UP_DP`) to match the
  reversed common pins, and spliced a twist into Hub-0's pair. The board was right end to end, but every pair pin of
  U601 carried the other line's name.
- 0.14.0 had today's netlist, routed from 0.12.0 (`route.py --seed`) with a twist spliced into each of the two pairs'
  old lines, because a full run could not place them yet.
- Snapshots: [debug/v13-seeded/](debug/v13-seeded/), [debug/v14-seeded/](debug/v14-seeded/).

**Firmware:** nothing to change. On a 0.15 board the host mode's first attach is seen as low speed, the swap flips once,
and the `DBG_TX` line `USB: Hub-0 seen as low speed, D+/D- swap now false (errata E1)` confirms it. Once 0.15 boards are
the common case, set `CROSSED_AT_START = false`.

**When 0.15.0 is ordered:**
1. Tag it `hw-v0.15.0`.
2. Set `kvm_proto::HARDWARE_VERSION` to 0.15.0.
3. At bring-up, check that Hub-0 enumerates and that the line above appears once.
4. Check the service port too, because its pair is new copper: `odkvmctl reboot bootloader` brings up the `RP2350`
   drive on PWR/SERVICE.

### Why the checks did not catch it, and what catches it now

- **The netlist check confirmed the mistake.** `check_netlist.py` compared the KiCad netlist with the generator's own
  pin map, so it agreed with whatever the generator said.
- **ERC and DRC can't see it.** They have no notion of a pair's polarity through a part.
- **New check:** `check_netlist.py` now also checks USB polarity from end to end, against the parts themselves.
  - Every net's USB pins (connectors, hubs, RP2354B, AT32F405, read from their pin names) must be all D+ or all D−.
  - Through every TS3USB3031, the common pair must meet each channel in use with the same polarity.
  - It fails on any crossing not listed in `KNOWN_CROSSED`. E1 was listed there until it was fixed, and the list is
    empty now.
  - It was tested on altered copies of the netlist. It caught a swapped front-port channel, a fixed U601 (it asks for
    the entry here to be closed), and D+ joined to D−.

## E2: The service port needs SVC_SEL held high by hand

**Affects:** hardware 0.8.0 to 0.16.0. The fault came in with rev 8 (2026-10-25); 0.12.0 is the board
that was ordered. Found on 2026-11-11 at the bring-up of the first 0.12.0 board: blank, it did not show the boot ROM's
`RP2350` drive on PWR/SERVICE. With SVC_SEL held at 3.3 V by a clamp across R510 it did, and the first firmware went on
that way.

### What is wrong

The service switch U601 (TS3USB3031) takes SEL0 from `SVC_SEL`, the RP2354B's GPIO0, and SEL1 low through a 100k:
SEL0 high gives the RP2354B's USB to the PWR/SERVICE connector (USB2), low to Hub-0 (USB1). While the RP2354B runs no
firmware (a blank chip, the boot ROM, the pinhole held at power-up) only R510, 100k to +3V3, pulls SEL0 high.

The RP2350 pulls the same pin down. Its GPIOs come out of reset with the pad pull-down on (datasheet §9.3), 36 to
113 kΩ at 3.3 V (§14.9.4, Table 1683), and the boot ROM leaves GPIO0 alone. SEL0 sits on the divider:

| Pad pull-down | SEL0 with R510 = 100k | SEL0 with 10k (0.17.0) |
|---|---|---|
| 36 kΩ (strongest) | 0.87 V | 2.58 V |
| 113 kΩ (weakest) | 1.75 V | 3.03 V |

The TS3USB3031 needs 1.3 V for a high and 0.6 V for a low (datasheet, digital control inputs; its own 6 MΩ pull-downs
on SEL0/SEL1 add nothing). With 100k most chips land below 1.3 V, in the undefined band at best, and the USB stays on
Hub-0. RP2350-E9 (pad leakage that holds an input near 2.2 V) does not come into it: the input buffer is off after reset.

**How it happened:** before rev 8 the service switch was a TS3USB30E, which picked the service port with SEL low, and
`SVC_SEL` had a 100k pull-down, which agreed with the RP2350's own. Rev 8's TS3USB3031 needs SEL0 high for the same
port; the resistor went to +3V3 and kept its value. The board's other pull-ups that have to win against the RP2350's
pads were already 10k (the AT32F405s' NRST, the front switches' SEL1, the Bluetooth module's straps). ERC and DRC have
no notion of pull strength, and nothing ran on a board before 2026-11-11.

### Effect

- **The boot ROM can't be reached on PWR/SERVICE**: on a blank board (first flashing), with the pinhole held at
  power-up, when a firmware does not come up, and after `odkvmctl reboot bootloader`.
- **Unaffected:**
  - Normal operation. The firmware drives `SVC_SEL` itself (low: Hub-0).
  - Firmware updates over a PC port: `odkvmctl update` and the window install over the KVM's own control interface,
    confirmed with a press of the key. A board that runs the firmware never needs the service port for an update.
- **`odkvmctl reboot bootloader` doesn't get round it.** `system.rs` drives `SVC_SEL` high before the boot ROM
  reboot, but nothing keeps it there once the ROM runs: in the bootloader only R510 holds SEL0. Tried on the first
  0.12.0 board on 2026-11-12: the RP2354B went into its bootloader (the KVM stopped answering on PC1, whose AT32F405
  stays on the bus), and no drive appeared on PWR/SERVICE. (The datasheet's pad isolation latches, §9.7, looked as if
  they might hold the level through a watchdog reboot; on the board they did not, or the ROM sets the pad up again.)
  The firmware can't hold the pin either: the only pin the boot ROM drives in the bootloader is its activity indicator
  (`reboot()` flag `GPIO_PIN_SPECIFIED`, or OTP), which toggles with drive activity and would flip the switch
  mid-write.

### Workarounds

1. **Update over a PC port** whenever the firmware runs (above).
2. **For the boot ROM** (blank board, recovery, after `odkvmctl reboot bootloader`): hold `SVC_SEL` high while the ROM
   runs. Bridging R510 while the board already sits in the bootloader works too: the switch hands the USB over and the
   drive appears. On 0.12.0, R510 is on the top
   side at (135.5, 120.5), west of U601; pad 1 is `SVC_SEL`, pad 2 +3V3.
   - **Short it** for one install, as on 2026-11-11 with a clamp. The contact has to hold for the whole write: each time
     the clamp slipped, the ROM left the bus mid-write (at 71 % and at 17 %) and the install had to start again
     (`odkvmctl update` tries again while the board is there). Take the short off once the install is done: the
     firmware drives `SVC_SEL` low as soon as it starts, against the short, and Hub-0 (Normalized and Mirrored ports)
     only gets the USB once SEL0 can go low.
   - **Rework it for good:** replace R510 with 10k 0402, or solder a 10k on top of it (100k ∥ 10k = 9.1k, SEL0
     ≥ 2.6 V). Nothing has to come off afterwards.
3. **Firmware:** nothing to change. It drives the pin itself, so the stronger pull-up only costs it 0.33 mA.

### Fix in 0.17.0

**R510 = 10k** (C25744, 0402WGF1002TCE, a Basic part the board already uses), `gen_schematic.py` block 5. Same
footprint and place; only the value changes.
- **Service port by default:** SEL0 2.58 to 3.03 V over the RP2350's whole pull-down range, at least 1.28 V above the
  switch's 1.3 V. That holds after any reset, whatever the pad was doing before, so `odkvmctl reboot bootloader`
  works as well: it relies on R510 alone.
- **Hub-0 when the firmware says so:** GPIO0 sinks 3.3 V / 10 kΩ = 0.33 mA. At its 4 mA drive setting the pad is
  specified to stay under 0.5 V at 4 mA (Table 1683), so at 0.33 mA SEL0 is a few tens of millivolts, far under the
  0.6 V the switch reads as low. Normalized and Mirrored ports keep working, with no firmware change.
- **Cost:** 1.1 mW while the firmware holds SEL0 low, standby included.
- **Not affected by RP2350-E9:** the firmware drives the pin; the pad's input leakage only matters for an undriven pin.

## E3: The side LEDs shine into the board

**Affects:** every board up to 0.16.0. The footprint and the placement date from the first design (2026-10-22);
0.12.0 is the board that was ordered. Found on 2026-11-11 on the first 0.12.0 board: the five status LEDs
on the front edge (D1502 to D1506) light up, but towards the inside of the board.

### What is wrong

The SK6812SIDE-A lies on its side: its four terminals are the back of the package, and the lens is the long face
opposite them (datasheet §4, the side view with the terminal at the bottom rear; §6, the land pattern with the pads
behind the body). JLC's land pattern, which `gen_footprints.py` converts, has it right: the pads overhang the body at
−Y, and the body with its lens reaches to +Y. But the footprint's description said "light exits towards −Y (pad
side)", design.md §16 repeated it ("Light leaves the pad side, so they are rotated 180°"), and `gen_pcb.py` placed the
LEDs at 180°, pads 0.5 mm from the front edge: the lens faced into the board, 2.5 mm in from the edge.

**Placement only, not the schematic.** The symbol and netlist match the datasheet (pin 1 DIN, 2 VDD = +5 V, 3 DOUT,
4 GND), and JLC mounted the parts as the footprint says, which is why they work. A part can't be turned on the board as
a rework: its pads are not symmetric, and turned round it would swap DIN with DOUT and +5 V with GND.

**Effect:** the LEDs work, but their light reaches the front only scattered around the parts, dimmer, and perhaps not
at all through an enclosure. The key LED (D1501, under the MX switch) is not affected. No workaround beyond the LED
brightness setting.

### Fix in 0.17.0

- **Placement:** `gen_pcb.py` places D1502 to D1506 at 0°, the lens 0.5 mm from the front edge (`LED_Y`). The
  courtyard spans the same strip as before (y 177.29 to 179.74), so nothing around them moved. The pads are now 1.3 to
  2.5 mm in from the edge, DIN on the west pad and DOUT east of the centre, so the chain still runs PC1 → Keyboard →
  Mouse → Other → PC2 from left to right, and each LED's 100 nF sits just north of its +5 V pad.
- **Footprint and docs:** the description now says the light exits towards +Y, away from the pads; design.md §16 is
  corrected.
- **Routing:** the pads moved, so 0.17.0 can't be seeded from 0.16.0 and is routed from scratch (2026-11-11: 1093 s,
  0 unrouted nets). DRC with schematic parity, `check_copper.py`, `check_spacing.py`, `check_aux.py` and
  `check_usb_si.py` pass; the USB pairs come out as on 0.16.0, and the +5 V feed drops 85 / 74 / 54 mV at the
  USB-rated load (0.16.0: 84 / 74 / 49 mV). Live view and numbers: [debug/v17-full/](debug/v17-full/).
