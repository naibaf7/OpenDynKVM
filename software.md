# OpenDynKVM: firmware and host software (rev 2)

Implements the behaviour in [design.md](design.md) on the rev-5 board in [hardware/](hardware/). Everything here is Rust.
C/C++ is the fallback for two specific parts, each with a decision gate (§11).

## 1. Decisions

| Component | Runs on | Language / main crates | Notes |
|---|---|---|---|
| [`firmware/rp2354`](firmware/rp2354) | RP2354B | Rust, `embassy-rp` 0.10, `embassy-usb-host` 0.1 | Own driver for the RP2350's **native** USB host controller (§4.2). |
| [`firmware/at32f405`](firmware/at32f405) | 2 × AT32F405 | Rust, `at32f4xx-pac` 0.4, `embassy-executor`, `embassy-usb-synopsys-otg` 0.4 | The OTGHS core is a dwc2; the AT32-specific parts are clocks, PHY power-up and the SPI slave. |
| [`firmware/esp32c3`](firmware/esp32c3) | ESP32-C3-MINI-1 (boards 0.16.0 on, when fitted) | Rust, `esp-hal` 1.2, `esp-rtos` 0.4, `esp-radio` 1.0 (beta), `trouble-host` 0.8 | Bluetooth LE keyboard and mouse towards up to four hosts (§4.13). |
| [`firmware/esp32`](firmware/esp32) | an original ESP32 (an ESP32 board on a front port; a module on a board to come) | Rust (Xtensa toolchain), the ESP32-C3 firmware's modules, `kvm-classic`, a patched `esp-radio` | The same, dual mode: Bluetooth Classic gamepads too (§4.13, "The ESP32 module"). |
| [`software/odkvmctl`](software/odkvmctl) | Windows, Linux | Rust, `hidapi`, `clap`, `egui`/`eframe`, `tray-icon` (Windows), `ksni` (Linux) | One crate, two binaries: `odkvmctl` (CLI) and `opendyn-kvm` (window + tray icon). |
| [`crates/`](crates) | all of the above | Rust, `no_std` | Protocols, HID parsing, switching logic, USB device core. Tested on the PC with `cargo test`. |

**Changes against design.md §8–§9:**
- Firmware is Rust/embassy on both MCUs instead of C/pico-sdk/TinyUSB.
- The PC-side MCUs do not build descriptors. The RP2354B sends each AT32F405 a complete **descriptor set** (§7), and
  a generic device core serves it. All descriptor logic therefore lives in one image, and the AT32F405s rarely need
  reflashing.
- `odkvmctl` is split into a CLI and a GUI binary, because a Windows GUI program cannot also print to the console it was
  started from.

**Why Rust is practical on both MCUs (crates.io, October 2026):**
- **RP2354B:**
  - `embassy-rp` covers PIO, SPI with DMA, ADC, flash, multicore and the ROM API. It has no host-mode driver for the
    native USB controller.
  - `embassy-usb-host` provides enumeration, a hub class and HID helpers on top of two small traits
    (`UsbHostController`, `UsbPipe` from `embassy-usb-driver` 0.2), including the `PRE` prefix for low-speed devices
    behind a full-speed hub, which is exactly Hub-0's situation.
  - The missing piece is a register-level driver of about 1000 lines. TinyUSB's `hcd_rp2040.c` and
    `cotton-usb-host` (an RP2040 host driver in Rust) are the references.
- **AT32F405:**
  - `at32f4xx-pac` is generated from Artery's SVD and covers every peripheral used here.
  - `embassy-usb-synopsys-otg` is HAL-independent: the HAL only supplies the register base, FIFO size and PHY type.
    The AT32's OTGHS has an internal UTMI HS PHY, which the driver supports as `PhyType::InternalHighSpeed`.
  - Artery-specific: the PHY is powered up through `GCCFG.PWRDOWN` (bit 16, 1 = transceiver on) and VBUS sensing is
    switched off through `GCCFG.VBUSIG` (bit 21), RM v2.02 §21.6.3.12. Both are a few lines in our wrapper.

## 2. Repository layout

```
Cargo.toml                 host workspace: crates/* + software/odkvmctl + xtask (firmware is excluded)
crates/
  kvm-proto/               link frames + go-back-N ARQ, link messages, descriptor-set format, control protocol, CRCs
  kvm-hid/                 HID report-descriptor parser, field codec, normalized formats and their descriptors
  kvm-core/                hotkeys, switching, Auto policy, descriptor-set builder, config, control-request handler,
                           the flash store (§4.8)
  kvm-map/                Vial protocol, keymap and keymap engine (§4.11); parts adapted from RMK
  kvm-usbdev/              USB device core that serves a descriptor set (generic over embassy-usb-driver)
firmware/
  rp2354/                  RP2354B image (own Cargo workspace, target thumbv8m.main-none-eabihf)
  at32f405/                AT32F405 image (own Cargo workspace, target thumbv7em-none-eabihf)
  esp32c3/                 Bluetooth module image (own Cargo workspace, target riscv32imc-unknown-none-elf)
software/
  odkvmctl/                  CLI + GUI, device simulator, DDC/CI, udev rule, installers
xtask/                     `cargo xtask dist`: both firmwares, flash images, UF2 files, partition table (§10)
```

The firmware crates are separate workspaces so that `cargo test` at the root builds only host code. They take the
shared crates by path.

## 3. Data path and latency

```
mouse ─FS 1 kHz─► Hub-0 ─► RP2354B USB host IRQ ─► port task: parse, normalize ─► engine ─► link queue
                                                     (core 0)                               │ (core 1)
PC ◄─HS 125 µs poll─ Hub-n ◄─ AT32F405 IN FIFO ◄─ frame ISR ◄── SPI 25 MHz, 72-byte frame ◄─┘
```

| Step | Time |
|---|---|
| USB host buffer-done IRQ → report parsed and normalized | ~5 µs |
| Cross-core queue → SPI frame built (CRC included) | ~5 µs |
| 72-byte SPI frame at 25 MHz | 23 µs |
| AT32F405 frame ISR → report in the IN FIFO | ~3 µs |
| Wait for the PC's next poll (HS, bInterval 1) | 0–125 µs |
| **Added latency** | **~36 µs + 0–125 µs (≈ 100 µs average)** |

That is within design.md §3.2's ~85 µs average / ~145 µs worst-case budget for the transport, plus 13 µs for the
fixed-size frame. Variable-length frames (§6.1) would win back about 15 µs if measurements call for it.

## 4. RP2354B firmware

### 4.1 Tasks

| Core | Task (`firmware/rp2354/src/`) | Job |
|---|---|---|
| 0 | `host::hub_task` | Root port (Hub-0 only), hub events, hands each front port's device to its port task |
| 0 | `host::port_task` ×3 | Read the descriptors, let the engine pick the mode (§4.3), run the interrupt pipes, execute the engine's device requests |
| 0 | `link::dispatch` ×2, `link::uploader` ×2 | Link messages → engine; descriptor sets → AT32F405s |
| 0 | `ui::run` | The MX key (debounced: press / long press), the six LEDs through PIO (WS2812 timing), 100 Hz |
| 0 | `power::monitor`, `power::port_power` ×3 | ADC: PWR VBUS, CC1/CC2, `PWR_ST`; front-port faults, enables and power cycles |
| 0 | `storage::run`, `system::ticker`, `system::reboots` | Changed config and keymap items to flash (§4.8); engine timers and link error counts; reboots |
| 0 | `update::run` | The A/B update writer and the `explicit_buy` of a new image (§4.9) |
| 0 | `selftest::run` | The boot-time self-test (§4.10) |
| 0 | `uartlog::run` | Plain-text event log on `DBG_TX` (§4.10) |
| 1 | `link1`, `link2` | SPI0/SPI1 master with DMA, one per AT32F405; frame exchange and ARQ (§6). Each owns its AT32F405's NRST/BOOT0 and its PC hub's reset, and reprograms the chip when needed (§4.9) |

- **The engine** (`kvm_core::Engine`, §4.4–4.5) is a plain struct behind a thread-mode mutex. It is about 180 KB (measured on the PC, 2026-11-19), so
  its static is initialised at compile time (`Engine::empty()` is `const`) and `start()` fills it in at boot: built on
  the stack and moved, it took two copies, a 259 KB frame of the 286 KB of stack. Core-0 tasks call it
  and route the outputs it returns (`kvm.rs`); it never awaits, so each call is a few microseconds.
- **Cross-core queues** between the engine side and the links are `embassy_sync` channels on the RP2350's hardware
  spinlock. Core 1 runs nothing but the links, so a slow descriptor fetch on core 0 never delays a frame.
- **At boot**, right after `embassy_rp::init`: `POWMAN.VREG.HIZ = 1` (password-protected write; the core is fed by the
  external TLV73311, design.md §6). The low-power regulator mode is never used.

### 4.2 Native USB host driver (`firmware/rp2354/src/usbh.rs`)

Implements `embassy_usb_driver::host::{UsbHostController, UsbHostAllocator, UsbPipe}` for the RP2350 USB controller
in host mode (RP2350 datasheet §12.7).

- **Root port:** Hub-0 is the only device on it, always full-speed. Connect/disconnect come from `SIE_STATUS.SPEED` and
  the `HOST_CONN_DIS` interrupt; the driver drives the bus reset itself.
- **D+/D− polarity** ([hardware/errata.md](hardware/errata.md) E1): boards 0.8–0.12 cross the pair between the
  RP2354B and Hub-0, while the service port's path is straight.
  - Host mode starts with the PHY swapping the pair (`USB_MUXING.SWAP_DPDM`, `CROSSED_AT_START`). The boot ROM's
    bootloader on the service port starts with the bit cleared, and needs it cleared.
  - A board with the crossing fixed is recognised at run time, since the board has no ID. A low-speed attach on the root
    port can only be Hub-0 seen through a crossed pair, so the driver flips the swap once, restarts the controller and
    waits for Hub-0 again. The flip is logged on `DBG_TX`.
- **Hub-0 control lines:** `HUB0_RST` (GPIO2, the hub's `RESET_N`) and `HUB0_VBUS` (GPIO3, its `VBUS_DET`), both
  push-pull outputs. Their pads come out of reset isolated with the pull-down on, so Hub-0 stays in reset and detached
  until `main` drives both high. A failed enumeration pulses `HUB0_RST` low for 10 ms, then retries (`host::hub_task`).
- **Control (and the rare bulk) transfers** use EPX, the controller's single software-scheduled endpoint, one packet per
  transaction: SETUP, each data packet, then the status packet (the TinyUSB `hcd_rp2040` approach, simpler than
  double-buffered multi-packet transactions). EPX is shared by every control pipe, so the driver serializes transfers
  with an async mutex. Hardware NAK retries are bounded with the pipe's timeout; an abandoned transaction is stopped
  with `SIE_CTRL.STOP_TRANS`.
- **The Bluetooth module as a board on a front port** (§4.13): its bulk endpoints are polled by interrupt slots, every
  millisecond (a device answers IN and OUT tokens the same whatever the host's schedule), so that EPX carries control
  transfers only. A slot read that gives up leaves the slot armed for the next; an OUT packet not taken in time is
  taken back (`RpPipe::out_within`). Bulk over EPX, one transaction at a time with `NAK_POLL.STOP_EPX_ON_NAK`, was
  tried first: on the first dongle (2026-11-14) control transfers failed after it (the DTR/RTS that restart the chip),
  and writes broke off in the middle of programming.
- **Known limitation:** `SIE_STATUS` error flags are global, so an error on a hardware-polled interrupt slot (e.g. a
  device just unplugged) can fail a concurrent EPX transfer. Enumeration retries absorb it; RP2350's per-endpoint
  `EP_RX_ERROR`/`EP_TX_ERROR` registers could separate them if M1 shows it matters.
- **Interrupt pipes** map one-to-one onto the 15 hardware-polled interrupt endpoint slots (`ADDR_ENDP1..15`,
  `INT_EP_CTRL`). The slot stays armed; the buffer-done IRQ wakes the pipe's future, which copies the report and re-arms.
  Budget: hub 1 + keyboard ~3 + mouse ~2 + other ~3 ≈ 9 of 15.
- **Low-speed devices behind Hub-0:** `SIE_CTRL.PREAMBLE_EN` for EPX and `ADDR_ENDPn.INTEP_PREAMBLE` for interrupt slots,
  selected when the pipe's `SplitInfo` says `SplitSpeed::Low`.
- **DPRAM layout (host mode):** setup packet at 0x000, interrupt-slot control at 0x008 + 8·(n−1), EPX buffer control
  at 0x080, interrupt-slot buffer control at 0x088 + 8·(n−1), EPX control at 0x100, EPX data at 0x180, interrupt-slot
  buffers of 64 bytes from 0x200. The slot poll interval (ms − 1) sits in bits 25:16 of the slot control word, which
  `rp-pac` doesn't model.
- RP2040 erratum E4 (host writes buffer status into the upper half in single-buffered mode) is handled the way TinyUSB
  does: if only the upper half shows a completed buffer, it is treated as the lower half.

### 4.3 Port manager

One task per front port. Hub-0 ports 1/2/3 are Keyboard/Mouse/Other (schematic block 7).

```
Empty ─attach─► Probing ─descriptors read─► decide mode ─┬─► Hosted (Normalized | Mirrored | Mirrored+identity | Off)
  ▲                                                       └─► Passthrough ── router flips to its PC
  └──────────── detach / over-current / re-detect ◄───────────────────────────────────┘
```

**Devices, modes and routings** (`ctrl::Device`, `ctrl::Routing`). The KVM routes seven devices: the three front
ports' and the four Bluetooth device slots' (§4.13). Each has, in every profile, a **mode** and a **routing**:
- **Mode:** Auto, Normalized, Mirrored, Mirrored + identity, Passthrough, or Off (its input goes nowhere; a front
  port's device stays powered and hosted). A Bluetooth device takes Auto (which is Normalized for it), Normalized and
  Off: it only ever reaches the KVM as reports.
- **Routing:** *follow* the active target (a PC or a Bluetooth host), or be *pinned* to one host whichever is active.
  A device in passthrough is pinned to a PC, or follows the active PC (the home PC while a Bluetooth host is active):
  a pin to a Bluetooth host makes it follow, as its USB can only reach a PC.

The devices that follow, and those pinned to the host that is active now, go the one way input has always gone: the
keymap, screen hop, the boot protocol. The devices pinned to any other host go there as they are, through that host's
KVM keyboard, mouse and media interfaces (a PC), its mirror endpoints (a device mirrored to a PC), or its Bluetooth
reports (a host), without the keymap or screen hop (`engine::Pinned`; a sleeping PC is woken by a key press as the
active one is). A switch, or a change of a routing, moves the devices whose host it concerns from the one way to the
other: what they hold is let go where it went, and stays hidden where it goes until released, as across a switch
(`Engine::join`). The hotkeys, learning one and a passkey being typed see every keyboard, wherever its input goes; a
keyboard's LEDs are its target's.

- **Probing** reads the device, configuration, string and HID report descriptors, and the Microsoft OS descriptors of
  an XInput pad. The result is a `DeviceSummary` (`kvm-core::policy`).
  - Hub-0 runs at full speed, so a high-speed device is probed at full speed; only a high-speed device answers
    GET_DESCRIPTOR(DEVICE_QUALIFIER) there. A port in passthrough reports the speed on the PC's hub instead: high for
    such a device, unless that hub's own link to the PC (the AT32F405's) is full speed. Until 0.1.11 it showed
    Hub-0's full speed (first board, 2026-11-12, a USB 2.0 flash drive on Other).
- **Auto policy** is design.md §4 verbatim, as a pure function with unit tests.
- **XInput pads** (`kvm_hid::xinput`, no report descriptor): a wired pad (interface 0xFF/0x5D/0x01) is mirrored by
  Auto, and normalized when set so. A wireless receiver (one interface 0xFF/0x5D/0x81 per pad, each with a headset
  interface 0x82) passes through by Auto, which gives the PC all four pads, the headsets and the receiver's own
  driver; set to Normalized, the KVM polls its first pad interface (`ConfigInfo::polled`: one pad, two interrupt
  pipes), asks the receiver whether a pad is linked when it starts hosting it (`INQUIRY`), lights the pad's player
  LED when it links, and decodes its input (the wired report from the packet's fifth byte); a pad that leaves lets
  go of everything. Both are decoded in the standard gamepad order, with the d-pad as the hat and the triggers as the
  brake and accelerator. Rumble from the PCs doesn't reach them.
- **Pad profiles** (`kvm-core::pads`, by VID/PID): the symmetric-stick pads of vendor 0x054C (the 4 and 5 series)
  number their face buttons left, bottom, right, top and put the right stick on Z/Rz and the triggers on Rx/Ry; the
  profile reorders them into the standard order before the field overrides and the calibration, and reads the 5
  series' touch pad from its vendor data (the first finger's contact as button 20, its position as the slider and the
  dial). Any other HID pad keeps its own numbering. The profile's style (generic, offset sticks, symmetric sticks)
  tells the host software how to draw the pad.
- **Profiles** (`kvm-core::config::Profile`, design.md §4): up to eight, each a name, every device's mode and routing,
  and a screen-hop arrangement. The settings in force are the active profile's, and the `Mode` and `Route` settings
  change that profile. Any change of the settings in force (a device's mode or routing, the active profile, its
  settings, a factory reset) decides all ports again in one step; the ports whose setting changed are reported with
  `PortChanged` even when their mode stays.
- **Passthrough:** the router (TS3USB3031) is flipped to Hub-1 or Hub-2 and the device re-enumerates on that PC.
  - On a PC switch, a port that follows the active PC is routed straight to the new PC (it re-enumerates there anyway).
    A port pinned to a PC stays; a change of its pin flips the router once (`PortSlot::routed`).
  - The KVM can no longer see the device, so it doesn't notice an unplug. Swapping devices on a passthrough port
    therefore needs a re-probe: `odkvmctl port other redetect`, or a mode change. When no device comes on Hub-0
    within 2 s of it (`REPROBE_MS`), the port is empty; until 0.1.11 it stayed probing (blinking yellow) for good.
  - A re-probe unplugs the device on its data lines (the router off for 250 ms) and plugs it into Hub-0, whose port
    reset restarts it; VBUS stays on. Until 0.1.6 it cycled VBUS for 300 ms, which is no power-off: the port's 220 µF
    has no discharge path but the device, so a receiver browned out without a reset and stayed stuck until it was
    replugged by hand (first board, 2026-11-12). The optional power cycle on a passthrough switch keeps VBUS off for 1 s.
- **Router select lines** (`SEL1`, `SEL0`): **MHL = Hub-0** `1,0` (power-up default, the KVM path), **USB2 = Hub-1/PC1**
  `0,1`, **USB1 = Hub-2/PC2** `0,0`, off = `1,1`. The strap resistors hold the default (SEL1 up, SEL0 down), so a
  blank board routes the device to the KVM; the MCU overrides them to send a port to a PC.
- **VBUS:** each port's TPS2553 is enabled once the RP2354B is up. A `FAULT` (active low) disables the port for 1 s, and
  the fault is reported (LED + control protocol).

### 4.4 Switching engine (`kvm-core::switch`)

On a switch from PC A to PC B:
1. A receives a **neutral report** on every interface: keys and buttons released, relative axes 0, absolute axes
   centred, hat switches null (`kvm-hid::neutral`).
2. Every key and button still held is **masked** on B until it is released.
3. Devices pinned to A or B change ways (§4.3); each keyboard gets its target's stored **output report**
   (Num/Caps/Scroll LEDs).
4. Passthrough ports that follow the active PC flip their router, with an optional VBUS power cycle.
5. LEDs update, and both PCs' control interfaces get an `EvSwitched` event.

### 4.5 Hotkeys (`kvm-core::hotkey`)

The engine sees the keyboard as a state (modifiers + 256-bit key set) after parsing, whatever the port mode, so
hotkeys also work on a mirrored keyboard.
- **Double-tap** of one key within `double_tap_ms` (default 300 ms; default key Scroll Lock). With `swallow` set, both
  taps are hidden from the PCs. A single tap is replayed to the active PC once the window has passed, so Scroll Lock still
  works, 300 ms late.
- **Combo:** exact modifier set + one key, e.g. Ctrl+Alt+1. The key's press and release are swallowed. The modifiers
  have already reached the PC; that is harmless and matches other KVMs.
- **The list** (`Config::hotkeys`, config keys 0x00F0–0x00FF, §8.3): up to 16 entries, each an action and its
  hotkey; an entry may have no hotkey yet (it does nothing), and two entries may do the same. The default is one entry,
  toggle PC on a double Scroll Lock. The actions (`HotkeyAction`; `odkvmctl` names in brackets):
  - toggle PC (`toggle-pc`): the other PC; from a Bluetooth host, back to the PC that was active before it;
  - next / previous host (`next-host`, `prev-host`): PC1, PC2, then each Bluetooth host that is ready, and round;
  - select a host (`select-pc1`, `select-pc2`, `select-host1`–`4`): a slot with no host bonded leaves the input where
    it is, as `SetActive` does;
  - next / previous profile (`next-profile`, `prev-profile`), among those in use; select profile 0–7
    (`select-profile0`–`7`), ignored while it isn't in use;
  - cycle the Other port's mode and routing (`other-port`), as the KVM's key can.

  Like the key, a hotkey works whatever the software locks say (`Origin::Device`).
- **Learning** (`hotkey::Learner`, request `LearnHotkey`): the KVM learns a hotkey from the keys pressed. It reads the
  same key state as the matcher, with the same timing, so it learns exactly what the matcher will recognise. A host
  couldn't: it sees the keys after the Vial keymap, and the OS keeps some from programs (Ctrl+Alt+Del, the Windows key,
  a modifier pressed alone). It recognises:
  - modifiers and one key, then everything released: that combo (`ctrl+alt+k`; each modifier on the side pressed);
  - one key alone, a modifier too, tapped twice: a double tap (the first tap released, and the second pressed, within
    `double_tap_ms` of the first press);
  - one key alone (not a modifier), pressed once: that key by itself (`f13`), once the window has passed, or at once
    if it was held longer.

  Modifiers alone, or two keys at once, teach nothing, and learning goes on.
  - **While learning** (up to 10 s, `engine::LEARN_MS`): the hotkeys are off, and the keys pressed are withheld from
    the PCs. Keys already held when it began pass as usual until released, such as the Enter that ran the command. Keys
    still held at the end stay hidden until released, as after a switch. The mouse keeps working.
  - **Who may ask:** only the active PC (`NotActive` otherwise, since the other PC would read what is typed on the
    active one), one session at a time (`Busy`), and not while configuration is locked. The same PC asking again
    starts over.
  - **The result:** `HotkeyLearned`, sent only to the PC that asked: learned, timed out, or cancelled (by
    `LearnHotkey(false)` or a switch). It changes no setting. The host shows it and sets it with `SetConfig` once the
    user agrees, so locks, checks, saving and `ConfigChanged` to both PCs stay on the one existing path.
  - **Passthrough:** the KVM never sees a keyboard in passthrough, so nothing is learned from it, just as no hotkey
    works on it.

### 4.6 User interface

- **Key** (`KEY`, GPIO11, MX switch to GND with 100 nF; internal pull-up, so active low): debounced for 10 ms. A press
  shorter than 600 ms (tap) and one held for `long-press-ms` (long press: 1 s by default, 0.8–3 s; it fires while the
  key is still down) do what the settings `key-tap` and `key-long` say (§8.3):
  toggle the PC (the tap's default), select the next profile in use (the long press's default), cycle the Other port's
  mode and routing in the active profile (Auto → passthrough following the active PC → pinned to PC1 → pinned to PC2
  → Auto, in one step: the router flips once), or nothing. For 1.5 s after the active profile changes, the five front
  LEDs count its number from the left: in white for profiles 0–4 (one LED for profile 0), in cyan for 5–7 (one for
  profile 5). While a firmware update waits
  for confirmation (§4.9), or software asks to be allowed again (below), the key LED pulses white and a tap confirms
  it, whatever the settings; that tap does nothing else. That press is the physical confirmation design.md §8 asks
  for. In standby (§4.12) the first press only wakes the KVM.
- **Software locks** (`software-switching`, `software-config`, §8.3): software can turn them off at any time (with
  configuration locked, nothing else either). `SetConfig` turning one back on is accepted even while configuration is
  locked, but only arms the key: the answer is `NeedsKeyPress`, and a tap within 30 s (`engine::UNLOCK_WAIT_MS`) turns
  it on, as a change made at the device. Turning it off again meanwhile withdraws the request. The key held at
  power-up turns both on (`Engine::allow_software`, called from `main` along with the AT32F405 retry).
- **LEDs** (`LED_DATA`, GPIO15, through the 74AHCT1G125): chain order PC1, Keyboard, Mouse, Other, PC2 (the
  side-emitting front LEDs, left to right), then the key LED (SK6812MINI-E) last, so a missing or damaged hand-fitted
  key LED leaves the others working (analysis.md finding 8; `kvm-core::leds::chain`). PIO program with WS2812/SK6812
  timing, 100 Hz refresh for breathing effects. The meaning of each colour is in design.md §7.
- **Brightness** defaults to 32/255: at full brightness the six LEDs draw up to 0.22 A of a PC port's budget
  (analysis.md finding 6). `odkvmctl config set led-brightness` changes it.

### 4.7 Power and faults

- `VSNS_PWR` (GPIO40, ADC0, divider 47k/147k), `RP_CC1`/`RP_CC2` (GPIO41/42, behind 10 kΩ series resistors because
  these pins are not 5 V tolerant): the PWR port's advertised current.
  The PC ports' CC and VBUS are measured by the AT32F405s and arrive in their status messages.
- `PWR_ST` (GPIO1): high = the PWR port is powering the board (TPS2121 IN1), low = PC bus (IN2).
- **Front ports** (TPS2553 + TS3USB3031 each): Keyboard GPIO20–23 and Mouse GPIO24–27, in the order SEL0, SEL1, EN,
  FAULT; Other SEL0 GPIO34, SEL1 GPIO37, EN GPIO32, FAULT GPIO33 (the RP2354B's south-east corner, facing that port).
  FAULT is active low with a 10 kΩ pull-up. A fault switches the port off for 1 s, then retries.
- **CC voltage across Rd (5.1 kΩ) → advertised current:** < 0.2 V nothing, 0.2–0.66 V default USB, 0.66–1.23 V 1.5 A,
  > 1.23 V 3.0 A.
- **Supply, measured** (`kvm-core::supply`, firmware 0.1.28): the board measures no current, and a PC
  port's limit can't be known without going over it; what shows whether a source keeps up is its 5 V under the real
  load. The KVM judges the source it runs from: the PWR port's VBUS (every 200 ms), or on PC power the higher of the
  PCs' (the ideal diodes feed the board from it), each AT32F405 sending its reading every 100 ms and the lowest of
  its 1 ms samples in between (`Status.vbus_min_mv`).
  - *Good* at 4.5 V and more, whatever the devices declare: many PC ports give far more than the 500 mA they
    advertise and hold their 5 V. *Sags* under 4.5 V for 2 s: the PC LEDs amber, and the window and `odkvmctl status`
    suggest the PWR port. *Low* under 4.4 V (the least USB devices must work at) for 0.5 s: in red, devices may drop
    out. Back to good after 5 s at 4.6 V or more.
  - The readings are good to about ±3 % (each AT32F405 takes its 3.3 V as the ADC reference), well clear of the 4.75 V
    and more of a healthy port.
  - Counted in the diagnostics, not judged: 100 ms windows whose minimum fell under 4.4 V (a device's inrush), and a
    PC's 5 V gone for less than 3 s (a port that cut out under the load, or the PC restarting: the two look the same
    from here). Many ports hold 5.0 V right up to an electronic current limit and then cut out: for those a dropout is
    the first sign there is.
  - The estimate stays as information: advertised current − 0.2 A for the electronics (`SELF_CURRENT_MA`: the first
    board took under 1 W from the PWR port in every state, front devices included, 2026-11-17), shown next to what the
    front devices declare (`bMaxPower`). Until 0.1.27 the LEDs turned amber and the window warned whenever the
    declared sum was more than that estimate, which on ports that coped well was most of the time.

### 4.8 Config and keymap storage

The config, the keymaps (layers and axis rules), the macros, the keymap settings and the gamepads' calibrations are
items in a **log-structured key-value store**
(`kvm-core::store`) on the **data partition** (244 KB = 61 sectors at 1800 KB, §4.9; 68 KB = 17 sectors before
0.1.51).

- **Why a log:** NOR flash programs bits only from 1 to 0, a 1 comes back only by erasing a whole 4 KB sector, and
  each sector is rated for 100 000 erases. Erasing and rewriting one sector per value would put every save's erase on
  that one sector.
- **Writes append** an item (`key | len | CRC-32 | data`) to the newest sector; nothing is rewritten in place, so a
  byte is programmed at most once between two erases. A sector is erased only when the ring needs a fresh one, after
  the items still current in the oldest sector have been copied forward. The sectors are taken in strict ring order,
  so each is erased once per trip of the ring, and values that never change travel with it (static levelling). There
  are no fixed-place flags, pointers or superblock: which sectors are in use, their order and each key's current value
  are worked out at mount from the sectors' contents (header CRCs, sequence numbers, item CRCs). Writing a value already
  stored costs nothing. The module documentation of `kvm-core::store` explains it in full.
- **Power loss:** an item's header is programmed before its data and a sector's header before its items, so a write cut
  short leaves an item whose CRC fails; reading stops there and the value before it stays in force. A cut garbage
  collection leaves two copies, and the newer wins. A host test cuts the power after every byte and erase of a run of
  writes and checks that no committed value is lost (`power_cuts_never_lose_a_committed_value`).
- **What is written, when:** the engine marks what changed (config, each screen-hop arrangement, each of the 32 layers,
  each keymap's axis rules, each known device's calibration and field overrides, macros, keymap settings) and emits
  `Save` 5 s after the last change, or at once on `odkvmctl config save`. The storage task then writes only the changed
  items (`Engine::take_save_item`). Switching PCs, Bluetooth hosts or screens never writes flash; changing profile does
  only with "power up in the profile last active". A change made while the storage task writes keeps its own autosave
  (`Engine::config_saved`).
- **Save pacing:** up to 30 saves go ahead as they come; after that at most one a minute, which is also how fast the
  allowance comes back (`SAVE_BURST`, `SAVE_PACE_MS`). Setting the KVM up never meets it. It holds a host script that
  changes a setting every few seconds, or saves in a loop, to 1 440 saves a day; a `SaveConfig` held back leaves
  `CONFIG_DIRTY` set until it is written.
- **Endurance** (W25Q16JV in the RP2354: 100 000 erase cycles per sector), measured by
  `endurance_with_the_firmware_items` with everything the firmware stores at its largest (the 4 keymaps' 32 layers and
  axis rules, 16 known devices' calibrations and field overrides, a full macro buffer, Vial's tap dances, combos, key
  overrides and alternate repeat keys, the records and the four arrangements with every screen slot used: 29.3 KB
  carried around the ring) and the config at its largest (800 bytes, `config::ITEM_MAX`: 752 with everything set, 720
  with the defaults, as all eight profiles' names, modes, routings, keymaps and fallback keymaps and the six hosts'
  keyboard layouts are written; the arrangements have items of their own): each sector is erased once per 266 config
  saves, the 61 within one cycle of each other, so 26.7 million saves before the first sector reaches its rated cycles
  (17 sectors before 0.1.51, with one keymap: 74 saves, 7.5 million). Paced, that is about 50 years of a script
  saving once a minute day and night; set up by hand, it doesn't wear out.
  The two alternating 4 KB slots used before erased a sector on every save: 200 000 saves.
- **Erase counts** are in each sector's header, for the boot log (`at most N erases per sector`). The spare sector has
  no header, so at mount it takes the highest count of the others (`erase_counts_survive_remounts`).
- **Stalls:** flash writes stall XIP for both cores, so core 1 parks in RAM (`embassy-rp` `multicore::pause_core1`)
  and link frames simply wait. A save is now a few page programs (0.4 ms typical, 3 ms worst case each); a sector
  erase (tens of ms, 45 ms typical for the W25Q…JV family) comes only once per about 8 saves.
- Booted from a partition, XIP 0x1000_0000 maps to that partition, so the store reads through the untranslated XIP
  alias 0x1C00_0000 (`flash.rs`).

### 4.9 Firmware update and AT32F405 programming

**Flash layout** (`firmware/rp2354/src/flash.rs`; `cargo xtask dist` writes the partition table):

| Offset | Size | Use |
|---|---|---|
| 0 | 8 KB | partition table (boot ROM block, encoded as `picotool partition create` does) |
| 8 KB | 896 KB | partition A |
| 904 KB | 896 KB | partition B, the A/B partner of A |
| 1800 KB | 244 KB | partition "data" (id 2): config and keymap store (§4.8). Not bootable, accepts no UF2 family |
| 2044 KB | 4 KB | free: the RP2350-E10 workaround block (picotool `--abs-block`, at 0x10FF_FF00) lands in the last sector |

- **The geometry changed once, with 0.1.51** (`kvm_proto::rpimage::layout::SINCE`): A and B went from 984 to
  896 KB and the data partition from 68 to 244 KB, as the Bluetooth module's firmware left the RP2354B image (the host
  software streams it to the module, §4.13). At boot `update.rs` checks the table against `flash.rs`; on any other
  table USB updates are off and the data partition is left alone too (`flash::layout_ours`: on the table from before
  0.1.51 it would lie in that table's partition B), so the config lives in RAM only. A board with the old table goes
  over once through the service port (below): the app and `odkvmctl update` recognise its table and give it
  `kvm-first.uf2`, and an update over a PC port never crosses 0.1.51 (`image::check_layout`). The old data partition's
  17 sectors are the last 17 of the new one, so its store is meant to mount there (not tried on a board yet). What
  lives *inside* the data partition is the firmware's own and may change with any update (items carry versions).
- The image is 597 KB of the 896 KB (2026-11-21, 0.1.51: with the AT32F405's 42 KB, without the module's; until 0.1.50
  it carried the module's too, compressed, and took 946 KB of 984 KB).

**RP2354B update** (`update.rs`; RP2350 datasheet §5.1.16–5.1.17):
1. `odkvmctl update kvm-rp2354.bin` (or the `.uf2`, or the app's Firmware page) checks that the file is an RP2350 Arm
   image built for this KVM's board, and sends it with `FwBegin` / `FwData` / `FwCommit`. The image says which board
   in its `FwId` (`kvm-proto::rpimage`: firmware version and hardware major version, DEVELOPMENT.md "Versions"); an image
   for another hardware major version, or without an `FwId`, is refused before anything is sent.
2. The firmware writes it into the partition that did not boot, all sectors but the first as they arrive, each read
   back. While a sector is written the engine answers `FwData` with Busy and `odkvmctl` retries.
3. After `FwCommit` the CRC-32 is checked, and the image's IMAGE_DEF gets the *try before you buy* (TBYB) flag. The
   first sector, which holds the IMAGE_DEF, stays in RAM, so the partition holds no bootable image yet.
4. The key LED pulses white. A press on the KVM's key (the physical confirmation of design.md §8) writes the first
   sector, and the firmware does a FLASH_UPDATE reboot into the new partition.
5. The boot ROM starts the new image under its 16.7 s watchdog. The image calls `explicit_buy` as soon as Hub-0 is
   enumerated and one link has answered (§4.10). The ROM then clears the TBYB flag and erases the old image's first
   sector. If the image hangs or fails that check, the watchdog reboots into the old image.
6. The host software then streams the release's module firmware for its chip (`kvm-module-<chip>.bin`) once the KVM is back and its module
   says it runs another version (§4.13), with no key: the new firmware takes only that image.
- The same image without the TBYB flag (UF2 drag-and-drop, picotool, a debug probe) boots normally and needs no buy.
- Without the expected partition table (a board flashed by a debug probe), `Info.caps` leaves out `FW_UPDATE` and the
  engine refuses `FwBegin`.

**Service port** (`odkvmctl::bootsel`, `odkvmctl::firmware`): installing on a new board, or on one whose firmware
doesn't come up on the PC ports, from a computer on PWR/SERVICE. The firmware routes the RP2354B's USB to Hub-0
(`SVC_SEL` low), so that port shows the KVM only while the RP2354B is in the boot ROM's bootloader: blank, the pinhole
held while plugging in, or `odkvmctl reboot bootloader`. The ROM then shows its `RP2350` drive and its PICOBOOT
interface (2e8a:000f; RP2350 datasheet §5.5–5.6).
- **Which board:** PICOBOOT's GET_INFO `UF2_TARGET_PARTITION` (rp2350-arm-s) says where an image would go. The KVM's
  A (sectors 2–225) or B (226–449), from `kvm_proto::rpimage::layout`: a KVM, whose other partition holds the firmware
  in use (its IMAGE_DEF's `VERSION` item, read from the first 4 KB with READ). The table from before 0.1.51 (A 2–247,
  B 248–493): a KVM that takes `kvm-first.uf2`, which writes the new table. 0xFF (no partition table) and erased
  flash: a new board; no partition table but the KVM's `FwId` in the image at the start of flash: a board flashed by a
  debug probe, set up like a new one. Anything else (another table, other firmware) is not a KVM and isn't touched.
- **Installing:** the UF2 the board takes, copied onto the drive as dragging it would (`OPENDYN.UF2`, flushed in
  64 KB steps): a KVM gets the update UF2 (`kvm-rp2354.uf2`, or one made from the `.bin`), which the ROM writes into
  the partition not booted; a new board, or one with the table from before 0.1.51, gets `kvm-first.uf2` (partition
  table and A, B's first sector erased, the data partition left as it is). The ROM then does a flash update
  boot: the new image is chosen whatever its version and the other partition's start is erased (§5.1.16), so this
  also installs an older version. PICOBOOT's `UF2_STATUS` counts the blocks the ROM has written (the progress bar, and
  its abort reasons as errors); the device leaving the bus means it restarted. Nothing is sent over PICOBOOT while the
  copy runs except `GET_INFO`: `INTERFACE_RESET` would abort it.
- **Access:** Linux needs the udev rule for 2e8a:000f (`TAG+="uaccess"`) and the drive mounted (desktops mount it;
  else `udisksctl mount`); the drive belongs to the device whose sysfs path holds its block device. Windows binds
  WinUSB to PICOBOOT from the ROM's own descriptors; a single RP2350 drive goes to a single device. Without PICOBOOT
  the state is unknown: the window offers both files with a warning, `odkvmctl update` stops and says which to copy.
- **Unverified on hardware:** all of it so far (2026-11-11: the bench board didn't show up on the service port); the
  logic is tested on the PC (`bootsel`, `firmware` tests).
- The IMAGE_DEF carries a VERSION item (major, minor << 8 | patch), visible to `picotool info`.
- **Versions** (DEVELOPMENT.md "Versions"): both chips run firmware `kvm_proto::FIRMWARE_VERSION`, and `Info.hw` reports
  `kvm_proto::HARDWARE_VERSION`, the board the build is for, as the board carries no ID of its own yet. `cargo
  xtask dist` checks the image's `FwId` and prints both.
- **Unverified on hardware:** whether the ROM takes FLASH_UPDATE's `p0` as an XIP address (used here, as the
  datasheet's `chain_image` example does) or as a storage offset. Check on the Pico 2 in M1.

**AT32F405 programming** (`link.rs`, `at32isp.rs`):
- The RP2354B image embeds the AT32F405 image (`build.rs`, from the AT32F405's release ELF), whose version sits behind
  a marker in the image (`FW_ID`). When an AT32F405's `Hello` reports another version, or it says nothing within 3 s
  of boot (blank or incompatible), its link loop reprograms it, as often as the limits below allow:
  1. hold the PC hub in reset (`HUBn_RST`, shared open drain with the AT32F405's PB1), so the PC never sees the ROM's
     own DFU device (analysis.md finding 11);
  2. BOOT0 high across an NRST pulse: the ROM bootloader starts;
  3. SPI at 1 MHz with a 10 µs gap after every byte (the ROM polls its SPI in software): synchronise, `Get` (version,
     commands, and whether replies start with a dummy byte), `Get ID`, mass erase of main flash, then `Write Memory` in
     256-byte chunks, each read back with `Read Memory`;
  4. BOOT0 low, NRST pulse, hub released. The link then checks whether the chip answers with the embedded version
     within 3 s; only that counts as success.
- **Limits** (`kvm-core::at32prog`), so that a chip that can't be programmed, or programs fine but never answers (a
  hard fault, a dead SPI line), isn't erased and rewritten at every power-up:
  - Failed attempts in a row are counted per chip in the store (item 0x0002: CRC-32 of the embedded image and one
    count per chip). Each attempt is counted *before* it starts, so a power loss during it (a weak PC port) counts
    too; the count goes back to 0 once the chip answers with the embedded version. That is a few writes per firmware
    version, nothing at ordinary boots.
  - Within one boot it tries again at once only if the transfer itself failed (the chip may be erased by then); a
    chip that took its image but doesn't answer waits for the next power-up. `AT32_FW1/2` in the self-test.
  - After **5** failed attempts in a row it gives up (`AT32_GAVE_UP1/2`, the key blinks red) and keeps the link going
    with whatever the chip runs.
  - The count starts again at 0 when the RP2354B firmware carries a different AT32F405 image (the record belongs to
    one image), when the KVM's key is held while it powers up (read before the links start; that press is neither a
    tap nor a long press; the Bluetooth module's count starts again too, §4.13), or with `odkvmctl reboot at32-retry`
    / the GUI's "Try again" (sent from the other PC if need be), which clears the count in flash and restarts the KVM.
- **No crystal:** an AT32F405 whose 12 MHz crystal doesn't start within 100 ms runs its PLL from the internal HICK at
  the same 144 MHz (§5.1), leaves USB off and says so in its `Hello` (`NO_CRYSTAL`). The RP2354B reports
  `AT32_XTAL1/2` and doesn't reprogram it: the firmware isn't the problem. Before, such a chip hung in its clock setup
  and looked blank.
- **Unverified protocol:** Artery's ISP note for the SPI bootloader is still missing (design.md §15). The sequence
  follows ST's SPI bootloader protocol (AN4286, timings from AN2606 Table 231), whose USART variant Artery's ROMs
  share. The read-back turns a protocol mismatch into a failed update, never into a wrong image.
- **What can't break:** the ROM bootloader is in ROM, and user system data (nBOOT1, which must stay 1 or BOOT0 = 1
  boots from SRAM and the SPI path is lost) is never written. If SPI programming fails, the AT32F405s can still be
  flashed over SWD (Tag-Connect) or the ROM's USB DFU: `odkvmctl reboot at32-dfu1` makes the RP2354B hold BOOT0 across a
  reset of AT32 #1, which then appears on PC1 for `dfu-util` (`dist/kvm-at32f405.bin`, at 0x0800_0000).

### 4.10 Self-test and debug log

The self-test (`selftest.rs`, design.md §10 step 1.5) runs at every boot and reports `ctrl::selftest` bits in `Status`:

| Bit | Check |
|---|---|
| `HUB0` | Hub-0 enumerates on the RP2354B's host port |
| `LINK1`, `LINK2` | the AT32F405 links come up |
| `AT32_FW1`, `AT32_FW2` | reprogramming an AT32F405 (§4.9) failed this boot; tried again at the next power-up |
| `AT32_GAVE_UP1`, `AT32_GAVE_UP2` | 5 attempts in a row failed: no more until the image changes, the key is held at power-up, or `at32-retry` |
| `AT32_XTAL1`, `AT32_XTAL2` | the AT32F405's 12 MHz crystal doesn't run: it works on its internal oscillator, without USB to its PC |
| `BT_FW` | the Bluetooth module (when fitted, or a dongle, §4.13) doesn't run this firmware's version and waits for the host software to send it; not shown on the key |
| `HUB1`, `HUB2` | a PC supplies VBUS, but its hub never resets the AT32F405 (judged 8 s after boot, cleared when it does) |
| `PORT_KB/MS/OT` | a TPS2553 FAULT is low while switched off, or within 100 ms of the first switch-on |
| `POWER` | 1 s after boot: PWR-port VBUS outside 4.4–5.6 V while `PWR_ST` says it powers the board (or above 4.8 V while it doesn't), or CC above 2.3 V |

- It finishes when everything has passed (about 8 s, the hub grace time) or after 12 s.
- **LED code:** while it runs, one white LED walks along the chain. For 5 s after it ends, each LED shows its part of
  the result, green for passed and blinking red for failed:
  - PC1 / PC2: that PC's link, hub and AT32F405 firmware;
  - Keyboard / Mouse / Other: the port's power switch;
  - key: Hub-0, the PWR-port readings and the Bluetooth module's firmware.
- **The red key:** after the self-test, while any fault in `selftest::ON_KEY` remains, the key LED blinks red,
  alternating with the active PC's colour, and in standby too (red and dark): Hub-0 missing, the PWR-port readings,
  for either PC its link, its hub, a failed or given-up programming and a missing crystal, and a Bluetooth module that
  doesn't run its firmware. These bits stay current
  after the self-test (a link that drops, a programming that ends after the 12 s, a hub that comes up). A front port's
  power-switch fault is left to that port's LED: it clears when the device is removed. A firmware update waiting
  for the key still pulses white.
- `odkvmctl status` and the GUI show failures in words.
- **Debug log:** `DBG_TX` (GPIO36, UART1 TX, 115200 8N1, on pin 6 of the RP2354B's Tag-Connect pads) carries a
  plain-text log of the events that matter at bring-up without a probe: boot, self-test, AT32F405 programming, updates.
  Everything else goes to defmt over RTT.

### 4.11 Mapping engine and Vial (`kvm-map`)

What every control of a normalized device does: a QMK-style keymap engine for keyboards, mice and gamepads alike,
rules for their axes, a virtual gamepad for the PCs, and the Vial app as a way into the keyboard part (design.md §4,
hid_mapping.md). It runs on the RP2354B, which already parses the devices and owns the config storage; the AT32F405s
only serve the interfaces. Since 0.1.51; before, the keymap was the keyboard's only.

- **Pipeline** (`kvm-core::engine`): each device's reports are decoded (`kvm-hid::translate`: keyboard, mouse,
  consumer and system controls, and a gamepad's buttons 1–32, its hat and its axes: X–Dial of the generic desktop
  page, brake and accelerator of the simulation page), a gamepad's axes go through its calibration, and the devices
  routed to the active host are merged (keys and buttons ORed, the hat last pressed, an axis the one moved last). The
  hotkeys come next, so no keymap can take the PC switch away. Then, if the active profile uses a keymap, the axis
  rules and after them the keymap engine over every control held, and the output builders; without one, the short
  path of before: keyboard and mouse as they are, a gamepad 1:1 onto the virtual gamepad.
- **Positions** (`kvm_map::pos`, 312): the KVM only learns which controls are down, so the "matrix" is the controls
  themselves: keyboard usages 0–231 (Vial's matrix, below), mouse buttons 1–16 at 232–247, gamepad buttons 1–32 at
  248–279, the hat's up, right, down and left at 280–283, and the two directions of each of the 14 axis inputs at
  284–311 (an axis whose rule makes it buttons).
- **Keymaps:** 4, of 8 layers each (`KEYMAPS`, `LAYERS`); each profile uses one or none (`profileN-keymap`, like its
  screen-hop arrangement; default none). Macros, tap dances, combos, key overrides and the QMK settings are shared by
  all four. A fresh keymap's layer 0 has every control do what it says, the other layers are transparent. Changing to
  a profile with another keymap lets go of everything held.
- **Keycodes** are QMK's, plus: mouse buttons 1–8 as `KC_BTN1`–`KC_BTN8` (0xD1–0xD8) and 9–16 as `KVM_BTN9` on
  (0x7E00–0x7E07), gamepad buttons as QMK's `JS_0`–`JS_31` (0x7400–0x741F), and the virtual gamepad's d-pad
  (0x7E08–0x7E0B: up, right, down, left). So a key can press a gamepad button and a gamepad button a key.
- **Axis rules** (`axes.rs`): per keymap and layer, one rule for each of 14 inputs (mouse X, Y, wheel, pan; gamepad X,
  Y, Z, Rx, Ry, Rz, slider, dial, brake, accelerator), 6 bytes: target (transparent, none, mouse X / Y / wheel / pan,
  virtual gamepad axis 0–7, buttons), scale (8.8 fixed point), invert and curve (linear, quadratic, cubic), deadzone
  and a parameter. The highest active layer whose rule isn't transparent decides; on layer 0 a transparent rule is the
  default (the mouse to the mouse, a gamepad axis to the same axis). Relative to relative is scaled with the
  remainder kept; absolute to absolute has deadzone, curve and scale; absolute to relative (stick → mouse) reads the
  deflection as a speed, moved every 8 ms (`TICK_MS`; the parameter is the counts per tick at full deflection);
  relative to absolute (mouse → stick) deflects the axis, which springs back at the parameter's rate (0: it stays);
  to buttons holds a direction past the threshold (absolute) or taps it for each report that moves it (relative).
  Several inputs on one gamepad axis: the last one moved wins.
- **Virtual gamepad** (setting `gamepad`: `off` (default; a controller nobody uses confuses games), `hid`, `xinput`;
  changing it re-enumerates both PCs): what the keymaps send to a gamepad, and the gamepads of the short path, reach
  the active PC as one gamepad. `hid`: report ID 4 on the NKRO interface (§7.1): 32 buttons, a hat and 8 axes of
  16 bits, so no endpoint leaves the mirrors. `xinput`: a vendor interface of its own on EP6 IN and OUT with the
  compatible ID `XUSB10`, its buttons in the standard gamepad's order (W3C: 1 A, 2 B, 3 X, 4 Y, 5 LB, 6 RB, 7 LT, 8 RT,
  9 Back, 10 Start, 11 LS, 12 RS, 13 Home), Z and Rz as the triggers; the PC's rumble and LED reports are dropped.
  Both came with milestone 3 (hid_mapping.md §14); one at a time, as two controllers would be two players.
- **Calibration** (`kvm-core::calibration`): kept with the device, not the profile: by its identity (FNV-1a of VID,
  PID and serial number on a front port, of the address of a Bluetooth device), 16 devices, one taking the place of
  the one set longest ago. Per axis: minimum, centre and maximum (the measured range onto the full one) and a deadzone
  around the centre, in 256ths; a trigger has no centre. Applied to the device's reports before they are merged.
- **The KVM's actions** (0x7E0C–0x7E1F, `KVM_ACTION`, milestone 4): the hotkey list's actions as keycodes, in its
  order (the other PC, next and previous host, PC1, PC2, Bluetooth hosts 1–4, next and previous profile, profiles 0–7,
  the Other port's mode), so a mouse or gamepad button switches the PC as a hotkey does. They fill QMK's keyboard
  range after mouse buttons 9–16 and the d-pad (0x7E00–0x7E0B), which the Vial definition names as Vial's custom
  keycodes (`build.rs`: BTN9…, DPAD_U…, PC_TOG, PC1, HOST1, PROF0, …), so Vial offers them too. The engine does an
  action once the steps before it reached the host (`Step::Kvm`).
- **Every Vial feature on every device:** mouse and gamepad controls are positions of the same engine, so layer-tap,
  mod-tap, tap dances, combos (by keycode: `pad1,pad2`), key overrides and macros work on them as on keys. A tap-hold
  keycode's tap may be any basic keycode, a mouse button or a media key too (`lt(1,mouse1)`). A macro holds and taps
  keys, media keys, mouse and gamepad buttons and the d-pad, and a mouse key in a macro moves the pointer or the wheel
  one step. Tested per device kind in `remap/tests.rs`.
- **Macro text per host** (milestone 6): every host has a keyboard layout (`layout-pc1`, `layout-pc2`,
  `layout-host1`–`4`: `kvm_hid::layouts`, 13 of them: US, UK, German, Swiss German, French, Spanish, Italian,
  Portuguese, Norwegian, Swedish/Finnish, Danish, Japanese, Brazilian), and a macro's text is typed through the layout
  of the host it goes to: a German PC gets `z` from the key where a US one has `y`, `@` with AltGr+Q, and a dead key
  such as `^` followed by Space. The same tables give the app's keyboard picture its legends.
- **Monitor and field overrides** (milestone 5, hid_mapping.md §9): `MonitorDevice` streams one device's raw reports
  to the host software (`Event::Report`, in pieces of 56 bytes: a report that changed, at most one per 16 ms per
  interface and report ID; guarded and renewed as a watch), and `GetDeviceMap` gives its descriptors, every HID
  interface's. A field override (`kvm-core::known`, up to 8 per device, kept with its calibration) reads bits the
  descriptor doesn't declare, or declares as something else, as a gamepad button or axis of the device: interface,
  report ID, first bit after the ID, 1–16 bits, signed or not, inverted or not. It applies to the device's reports
  before calibration and merging (a report ID of its own merges as a source of its own, so its fields add to the
  gamepad's), and `GetControls` reports the buttons and axes it gives. An axis rule with the `DELTA` flag reads an
  absolute input by how far it moved, so a touch pad's coordinates move the mouse; a jump larger than its parameter
  is a finger put down elsewhere and moves nothing.
- **The gamepad for Bluetooth hosts, and the fallback** (milestone 6): the module's report map has a gamepad (ID 5,
  §4.13), and a Bluetooth host that subscribed to it gets the virtual gamepad as the PCs do. A host that can't take
  one (a PC while `gamepad` is off, a Bluetooth host that didn't subscribe) runs the profile's fallback keymap
  instead of its own (`profileN-fallback`, none by default): the keymap that maps the sticks to the mouse and the
  buttons to keys. The engine picks the keymap again when the input's host changes or what it takes changes
  (`Engine::host_changed`). `GetOutputs` tells the host software what each host is offered and took: a PC everything
  its interfaces carry while its link is up, a Bluetooth host the reports it subscribed to (`HostState::reports`).
- **Watching controls** (`WatchControls`, §8.2): the host software asks for `Control` and `Axis` events from the
  normalized devices, as merged for the active host (an axis at most every 50 ms, and an absolute one on a change of
  1024 or more), to show what is held and to learn a control; "watch and swallow" keeps the controls from the hosts
  meanwhile, and a control still held when it ends stays away until it is let go. The events go to the PC that asked
  (either PC, or the service port); one watch at a time, a new one replaces it.

The keyboard part, and the Vial app's view of it:

- **Matrix:** position = HID keyboard usage 0x00–0xE7, row = usage / 8, column = usage % 8 (29 × 8). One byte per
  row keeps Vial's matrix tester inside one 32-byte report. Usages outside the layout (keypad extras 0xA5–0xDF, and
  0xE8–0xFF) pass through unmapped.
- **Layout** (`build.rs`): full-size ANSI with ISO Enter and ISO left Shift as layout options, plus F13–F24, the
  international and language keys, Power, Mute and Volume. The JSON, with the KVM's own keycodes as Vial's custom
  keycodes, is xz-compressed as vial-qmk does it (964 bytes) and served in 32-byte blocks.
- **Engine** (`remap.rs`): runs on the merged keyboard state of Normalized ports, after the hotkeys (so no keymap can
  take the PC switch away) and before the output builders. Events pass three stages, in QMK's order:
  - **combos** (32, up to 4 keys each, by keycode): a key of a combo waits until the combo is complete and no larger
    one is still possible, the combo term runs out, another key goes down or one of them comes up. The output is held
    until the last of its keys is up; keys that make no combo go on with the times they were pressed, so a waited-for
    tap-hold key keeps its tapping term. `CM_ON` / `CM_OFF` / `CM_TOGG`;
  - **tap-hold** (mod-tap, layer-tap, Space Cadet): tapping term, permissive hold, hold on other key press, chordal
    hold (opposite hands: the left half of the main block with F1–F5 is the left hand, Space and the extra keys are
    either), quick tap term, flow tap, retro tapping, the tap code and Caps Lock delays;
  - **actions:** 8 layers (MO, TG, TO, DF/PDF, LM, OSL, TT with tapping toggle, tri-layer, Layer Lock), modded keys,
    one-shot modifiers and layers with tap toggle, Grave Escape with its overrides, media and system keys, mouse keys
    with QMK's acceleration and Vial's settings for it, 16 macros in Vial's format (text, taps, presses, releases,
    delays), 32 tap dances (vial-qmk's tap / hold / double tap / tap-hold), caps word (idle timeout 5 s), the Repeat
    and Alternate Repeat keys (32 alternates of Vial's, then QMK's own), auto shift (its options, `AS_UP` / `AS_DN`
    / `AS_RPT`, which types the timeout) and the magic keycodes and settings (swaps, GUI off).

  The output is recomputed after each event and 32 **key overrides** are applied to it, as QMK does them (trigger
  as the last key pressed, required and negative modifiers, suppressed modifiers, layers, the 500 ms key repeat
  delay for a modifier pressed after the trigger, the trigger kept away after another key). Macro steps are at least
  2 ms apart: the AT32F405 keeps 4 reports per endpoint. On a PC switch it lets go of everything held, one-shot locks
  and caps word included; toggled and locked layers stay. 2 KB of RAM and 13 KB of code more than with layers and
  macros alone.
- **Protocol** (`protocol.rs`): VIA protocol 9 and Vial protocol 6 (keyboard id, definition, keymap get/set and
  buffers, layout options, macros, unlock, every QMK setting of vial-qmk but RGB and the pre-0.19 tapping flags, 16
  qsids per query reply; the dynamic entries: 32 tap dances, combos, key overrides and alternate repeat keys, with
  caps word and layer lock flagged). Keycodes are stored exactly as written, so every keycode reads back unchanged;
  QK_BOOT in a key or an entry is stored as KC_NO unless unlocked. Command decoding and the keycode ranges are adapted
  from RMK (MIT, `crates/kvm-map/LICENSE-RMK-MIT`); RMK's keymap engine is async and tied to its runtime, so it is
  not reused. The dynamic entries, the QMK settings and the engine follow vial-qmk and QMK's documentation.
- **Storage** (`keymap.rs`, `kvm_core::store`): keymap k's layer n at 0x0110 + 8k + n (version 2: the keycode of
  every position, 625 bytes; the keyboard-only layers of before 0.1.51 are not read), keymap k's axis rules at 0x0130 +
  k (673 bytes), the macros (0x0101), the settings (0x0100, 47 bytes), one item per kind of dynamic entry in Vial's own
  layout (0x0102–0x0105, 321 bytes each, 193 for the alternate repeat keys), and each device's calibration at 0x0200 +
  slot (`kvm-core::calibration`, 75 bytes).
- **Interface:** usage page 0xFF60 / usage 0x61, 32-byte reports, IN endpoint only (EP7): the requests arrive as
  SET_REPORT on a `LOCAL_HID` interface, which the AT32F405 already forwards, so its firmware is unchanged. The serial
  number gets Vial's marker `vial:f64c2b3c` (also in identity mode, appended to the cloned serial).
- **Setting** `vial` (§8.3): `off` (default), `on` (it keeps EP7 IN, so mirrors get one IN endpoint less), `auto`
  (shown only while an IN endpoint is spare, so mirrors always come first). Changing it re-enumerates both PCs once, as a
  mirror attaching does anyway. Vial edits the keyboard part (positions 0–231) of the active profile's keymap, or of
  keymap 0 while the profile uses none; the other positions and the axis rules only the host software sets
  (`odkvmctl keymap`, the Keymaps page, §9). The setting `keymap` of before 0.1.51 (apply the one keymap) is gone:
  whether a profile's devices go through a keymap is its `profileN-keymap`.
- **Rules:** every write needs "configuration from software" (0x0041). Macros, the matrix tester and QK_BOOT also need
  Vial's unlock: both Shift keys held for 3 s, per PC. The matrix tester answers only the active PC. The bootloader
  jump is ignored.

### 4.12 Standby, wakeup and the active PC (`kvm-core::standby`)

What the KVM powers while no PC uses it, and which PC is active when PCs are switched on and off. Nothing of it is
saved: after a power loss the KVM finds the running PC again from the PCs' states, so no flash is written for it.

- **What a PC does** (`Presence`, from its AT32F405's `Status`, §5.4): *no 5 V* (unpowered); *no host* (5 V, but its
  USB never started: shut down with standby power, or early in its boot); *asleep* (bus suspended), with or without
  remote wakeup allowed; *up* (default, addressed or configured); *unknown* while its link is down.
  - A suspend before the PC ever reset the AT32F405 (`Status.resets` 0) is *no host*, shown as "connecting"
    (`standby::host_usb_state`): the AT32F405's core takes an idle bus for a suspend from the moment it connects. Until
    0.1.19 such a PC showed as asleep, at full speed: on the first board (2026-11-15) PC2, whose Windows was stuck on a
    Bluetooth device it couldn't remove, never took up its hub again after the AT32F405s were reprogrammed. The
    self-test's `HUB1`/`HUB2` check still looks for `Powered` only, so it doesn't count such a PC, which may as well be
    shut down with standby power.
- **Auto-select** (`auto-select`, default on): the other PC becomes active (event `Switched`, source `Power`, so the
  monitors follow too, §9) when
  - the active PC gives no 5 V for 3 s (a reboot may drop it for less), or has not given any since the KVM started,
    which is the case when the KVM came up with the other PC; and the other PC gives 5 V;
  - or the active PC has no host, or sleeps without allowing a wakeup, for 30 s (shut down with standby power; a
    reboot's POST is shorter), and the other PC is up.
  It never moves away from a PC that a key press can wake. With both PCs on, the active one stays (PC1 after a
  power-up).
- **Standby** after 10 s without any PC up (and nothing on the LEDs: identify, an update, the self-test result):
  - *Wake standby* (`standby-wake`, default on) while the active PC sleeps and allows remote wakeup: the front ports
    with a keyboard or mouse, and the empty ones, stay powered (chosen when standby starts), the others switch off;
    Hub-0 runs; the LEDs are off.
  - *Deep standby* otherwise: all three front ports off, Hub-0 held in reset (`power::HUB0_HELD` keeps the hub task's
    recovery from releasing it), LEDs off.
  - `standby-glow` (default off): instead of all LEDs going dark, the key LED breathes slowly in the active PC's
    colour, between a quarter and the full set brightness.
  - It ends at once when a PC comes up (bus reset or resume) or the KVM's key is pressed; front-port devices then
    attach and are probed again (about 1 s).
- **Remote wakeup:** while the active PC sleeps, no input reaches it. A newly pressed key or mouse button (not
  motion), or a media key, sends `Command::RemoteWakeup` to its AT32F405, at most once a second, if the PC allowed
  wakeup; what is held then stays hidden from the PC until released, as after a switch. The hotkeys still work, so
  switching to the other PC needs no wakeup. This also wakes a PC that suspended only the KVM's device (USB selective
  suspend).
- **PC hubs:** each AT32F405 holds its PC's hub in reset after 1 s without that PC's 5 V and releases it as soon as the
  5 V returns (§5.1); in reset a USB2514B draws 0.75 mA instead of 40–45 mA (its datasheet, tables 6-2 and 6-7).
- **Not done yet:** the AT32F405s keep running at 144 MHz in standby (about 35 mA each) and the RP2354B at full
  speed. Lowering their clocks touches the SPI link, the OTGHS low-power entry (Artery's errata) and the time base,
  and is left until it can be measured on a board.
- `Output::Power(PowerPlan)` carries the plan to the firmware (`firmware/rp2354/src/power.rs`); the LEDs read it through
  `led_inputs`. Tested in `standby.rs` and the engine tests (auto-select, standby and the key, wakeup).

### 4.13 Bluetooth hosts and devices (boards 0.16.0 on, `bt.rs`, `espisp.rs`, `kvm-core::bt`, `firmware/esp32c3`, `firmware/esp32`)

Up to four extra hosts (PCs, tablets, phones) connect over Bluetooth LE to an ESP32-C3-MINI-1 on the board, which the
RP2354B treats as a third, lesser target: the hosts get the KVM's own keyboard, mouse and media reports (and screen
hop's absolute pointer, §4.14), never a
mirrored device's descriptor, and the front ports in passthrough stay with the PC that was active before (the *home*
PC). Everything below is off on a board without the module, and on boards before 0.16.0: the control protocol then
reports no `BLUETOOTH` capability and the window shows the two PCs only (design.md §16).

**Wiring and presence** (hardware/tools/gen_schematic.py block 16). Two wires, and a third from 0.18.0:
- **UART:** UART0 TX (GPIO28) into the module's RXD0, its TXD0 into UART0 RX (GPIO29), both on the module's ROM
  bootloader's own pins. The module's BOOT strap (IO9) is on the same net as its RXD0, with a 5.1 kΩ pull-up
  (10 kΩ until 0.18.0).
- **EN:** Espressif's 10 kΩ / 1 µF power-on delay. From 0.18.0 also the RP2354B's GPIO9 through 1 kΩ (R1605), open
  drain: driven low to reset the module (EN at 0.3 V), released otherwise. GPIO9 is unconnected on older boards,
  where pulling it does nothing.
- **While the RP2354B boots:** its pads come out of reset with a pull-down of 36–113 kΩ. Until `main` turns it off
  (its first lines release GPIO9 and drive GPIO28 high until the UART takes it), EN sits at 2.60–2.98 V and IO9 at
  2.93–3.17 V. Both are above the module's 0.75 × VDD (2.48 V), so it boots its firmware even while this chip is
  in its boot ROM, in BOOTSEL, or held in reset by a debugger. An update of the RP2354B leaves the module running.
- **Fitted or not:** `main` samples GPIO29 with the pad's pull-down for up to 0.5 s before handing it to the UART:
  the module's TXD0 idles high, a bare pad (no module, or a board before 0.16.0) stays low. One high sample is enough,
  as a module still in reset, or between the restarts of a ROM that finds no firmware, leaves the line low for a
  moment. Without it the Bluetooth task stops at once and nothing is ever sent on GPIO28.
- **Why these pins:** the four free pins on the RP2354B's south side lie between its SWD pins and its crystal's way
  east. In the first full routes of 0.16.0 the planned via fan-out of GPIO30/31 sat on Hub-0's Mouse pair while their
  lines cut the crystal's off (hardware/tools/route.py `FANOUT_SURFACE`). The EN line went to GPIO30 first and found
  no way out there either (hardware/debug/v18-esp-en/), so it is on GPIO9, on the west side.

**Link** (`kvm-proto::link::bt`). The AT32F405 links' 72-byte frames and go-back-N ARQ, over UART0 at 3 Mbaud
(0.24 ms a frame), the RP2354B as master: it sends a frame at least every 5 ms and the module answers each with one.
The RP2354B side runs on core 0 (`bt::run`); core 1 keeps to the AT32F405 links.

| Type | Direction | Message |
|---|---|---|
| 0x01 | both | `Hello` (the module's role is 3) |
| 0x60 | RP → BT | `Input`: host, report ID (1 keyboard, 2 mouse, 3 consumer, 4 absolute pointer), data |
| 0x61 | RP → BT | `Select`: the host that gets input as the active target (0 = a PC), and the hosts devices are pinned to (bit mask: they take input too, and stay connected) |
| 0x62 | RP → BT | `Pair`: open or close pairing, seconds |
| 0x63 | RP → BT | `Forget`: a host's bond, 0xFF = all |
| 0x64 | RP → BT | `Passkey`: the six digits typed for a pairing host, or cancel |
| 0x65 | RP → BT | `Policy`: stay connected, or connect only while selected |
| 0x66 | RP → BT | `RequestState`: every slot's `Host` now |
| 0x67 | RP → BT | `EnterRom`: restart into the ROM bootloader |
| 0x68 | RP → BT | `Scan`: look for devices to pair, seconds, or stop |
| 0x69 | RP → BT | `PairDevice`: pair the device at this address into a free device slot |
| 0x6A | RP → BT | `ForgetDevice`: a device's bond, 0xFF = all |
| 0x6B | RP → BT | `RequestMap`: send a device's report map |
| 0x6C | RP → BT | `DeviceOutput`: a device's output report (a keyboard's LEDs), report ID, data |
| 0x6D | RP → BT | `EraseAll`: forget every host and device, write empty records into both flash sectors of each, drop the stack's keys, and restart |
| 0x70 | BT → RP | `Output`: a host's keyboard LEDs |
| 0x71 | BT → RP | `Host`: a slot's flags (bonded, connected, encrypted, subscribed), interval, latency, address, RSSI |
| 0x72 | BT → RP | `Event`: pairing open / closed, passkey wanted, paired, pairing failed, bond lost, settings held in RAM; looking for devices open / closed, how far a device's pairing got, the passkey to type on it, a device's bond lost |
| 0x73 | BT → RP | `DeviceInput`: a device's input report as it came, its report ID (0: its map has none), data |
| 0x74 | BT → RP | `Device`: a device slot's flags (bonded, connected, encrypted, ready), RSSI, battery, appearance, address, its map's length and CRC-32, its name |
| 0x75 | BT → RP | `DeviceMap`: a piece of a device's report map (offset, total, ≤ 59 bytes) |
| 0x76 | BT → RP | `Found`: a device that may be paired, found while looking: address, RSSI, appearance, name (at most once a second each) |
| 0x77 | BT → RP | `Log`: a line of the module's log (≤ 63 bytes; a longer line in several), which the RP2354B keeps in RAM (2 KB, the latest whole lines, with the time it came; `GetModuleLog`) |
| 0x78–0x7B | BT → RP | `ControlRequest`: a control-protocol report host 1–4 wrote to the control service (the host in the type, so the 64 bytes fit; trailing zeros left out, `link::bt::control::trim`) |
| 0x50–0x53 | RP → BT | `ControlReply`: the answer to one of host 1–4's requests, or an event for it, to notify |

**Reports.** One HID service with a fixed report map (`kvm_proto::link::bt::report::MAP`, 305 bytes): keyboard (boot
layout, ID 1, with the LED output report), mouse (16 buttons, 16-bit X/Y, wheel, AC pan, ID 2), consumer (one 16-bit
usage, ID 3), screen hop's absolute pointer (16 buttons, X/Y 0–32767, wheel, AC pan, ID 4), and the virtual gamepad (32
buttons, a hat, X, Y, Z, Rx, Ry, Rz of 16 bits, ID 5, since 0.1.51). The pointer and the gamepad are top-level
collections of their own, as on the PCs' USB interface, so a host that takes no absolute pointer or no gamepad leaves it
alone; the engine sends the pointer nothing unless screen hop puts that host's pointer there (§4.14). Which input
reports a host subscribed to goes to the RP2354B with its state (`HostState::reports`, the module's notification bits):
the gamepad's tells whether the host took it (§4.11). A host bonded before 0.1.51 knows the old map and gets the gamepad
only after pairing again. Desktop and mobile systems take it without a driver: HID over GATT is built into them. Motion
is coalesced on its way (`link::bt::Motion`, on the RP2354B before the link and in the module before each notification):
a mouse reports up to 1000 times a second, the link to a module on a front port carries about 200 messages a second, and
a host takes notifications at its connection events (7.5–15 ms for the selected one). So one motion report waits at a
time, relative moves adding up and an absolute position replacing the one before; a report with other buttons sends the
waiting one first, so no click is lost. Queued one by one, as at first, the reports came late, and those that didn't fit
were dropped with their motion: on the first dongle (2026-11-14) the pointer crawled.

**Engine** (`kvm-core::bt`, `Engine::switch`):
- **Targets:** PC1, PC2 and the bonded hosts 1–4. `Target::Bt1`–`Bt4` select a host; `Toggle` from a host goes back
  to the home PC; `Next` (a hotkey's `next-host`, or the key's `next-target` action) cycles PC1, PC2, then the hosts
  that are ready (connected, encrypted, subscribed); `prev-host` goes the other way, `select-host1`–`4` to one host.
- **Switching to a host:** as a PC switch: the PC left gets neutral reports, what is held stays hidden from the host
  until released, the keymap lets go. Then `Select`, and the keyboard's LEDs show the host's lock state. Leaving a host
  sends it every key and button released.
- **Reports:** with a host selected, the normalized path (keymap included) builds its reports instead of a PC's, and
  a mirrored device's input reaches it as the KVM's own keyboard and mouse. More than six keys give the boot layout's
  ErrorRollOver; system control (power, sleep) stays with the PCs.
- **Pairing:** `BtPair` opens it for 60 s (`BT_PAIR_SECONDS`). LE Secure Connections, IO capability KeyboardOnly: the
  host shows six digits, and while it waits the KVM's keyboard goes to nobody else: digits on the top row or keypad,
  Backspace, Enter sends them, Escape cancels. `Status.bt_passkey_digits` / `bt_passkey_typed` carry
  what is typed so far, and each digit is a `BtChanged(0)`, so the window shows them as they come (the KVM has the
  keyboard, so nothing else does). What is held when it ends stays hidden until released. `BtPasskey` does
  the same from software. No Just Works, so a host in range can't pair unnoticed.
- **Standby and auto-select:** a host in use, or pairing, keeps the KVM awake (its keyboard is on the front ports);
  auto-select never moves the input off a host because a PC was switched off. The LEDs: the key glows violet with a
  host selected (both PC LEDs as inactive), breathes violet while pairing is open, blinks violet while a passkey is
  typed.
- **Power budget:** `Status.budget_ma` takes `BT_CURRENT_MA` (30 mA) more off the source when the module is there.

**Module firmware** (`firmware/esp32c3`, Rust on `esp-hal` + `esp-rtos`; Bluetooth: `esp-radio`'s controller and the
`trouble-host` stack). Up to five host connections (four slots plus one pairing), and four devices' (below). It
advertises while pairing is open (then
discoverable) or while a host that should be connected isn't; an unknown host that connects outside pairing is
dropped. Each connection has a task of its own and holds its slot from the moment it is let in until its task ends.
- **New hosts** pair only into a slot that no bond and no connection holds. Before 0.1.19, a second unknown device let
  in while a host paired got the same free slot and took it over. The first dongle (2026-11-14) showed it two ways: a
  connected phone shown as "not connected", and a pairing that broke off after its passkey.
- **A bonded host that forgot the KVM** pairs anew on its connection into its own slot, while pairing is open (outside
  it, it is let go and its old keys dropped). Every connection is bondable from the start for this: trouble-host answers
  the host's pairing request before the firmware hears of it, and before 0.1.19 that pairing took the passkey and
  gave no bond.
- **A bonded host that reconnects** before the module has seen its old connection end takes the slot over. The old
  task neither clears the slot's state when it ends nor acts on its connection's events.
- **Notification settings** (the CCCDs) are kept with each bond (`Slot::notify`, a byte that was padding) and put
  back when the host reconnects (`restore_notifications`); *subscribed* is read from them. A GATT server keeps them
  for a bonded client, which doesn't write them again, and trouble-host keeps them only in RAM, while the module runs.
  Until 0.1.21 a host back from sleep counted as not subscribed (the window showed it "connecting…" for good), and
  after a restart of the module a Windows host, which writes them once after pairing, got no input at all (first
  dongle, 2026-11-15). A bond from before 0.1.22 counts as having the input reports on. They are saved 2 s after the
  host's last change, as a host switches several on at once and each save erases a sector. Input goes out once the link
  itself is encrypted, not once the module saw its `Encrypted` event.
  - **Wear guard:** a real host saves them once, after pairing, and never on a reconnect (it writes the same values, or
    none). One that kept switching them would save every few seconds, so at most 16 such saves go to flash in a run
    of the module (`NOTIFY_SAVES`). After that the module holds them in RAM (`Bonds::hold`), they reach flash with the
    next save there is anyway (a pairing, a host forgotten), and it tells the RP2354B once per host
    (`EventKind::SettingsHeld`, again after every `RequestState`). The engine raises `FaultKind::BtSettingsHeld` on
    both PCs (the window's event list and `odkvmctl watch` show it) and the diagnostics an orange
    `BT_SETTINGS_HELD`, until the module restarts.
- **The stack's keys** are the slots' bonds and those of pairings under way, nothing else (`tidy_keys`, whenever a
  connection ends, a host bonds or one is forgotten). trouble-host stores a pairing's keys as soon as it encrypts,
  whether the module keeps the host or not, and never lets them go. Until 0.1.19 it had room for four, keys of
  pairings that broke off stayed, and a forgotten host's could too. Once the four were taken, every pairing failed
  just after its passkey: the host showed it connected for a moment and dropped off, and the KVM never showed it
  (first dongle, 2026-11-15). Room for nine now: the four slots and one pairing per connection.
- **Bond addresses** are kept as `Address::to_bytes` gives them, most significant byte first. Until 0.1.19 they were
  read back reversed, so a host that connects with its public address (a PC) was a stranger after every restart of
  the module.
- **Passkeys** go to the slot that asked for them. **When pairing closes** (it ran out, the RP2354B closed it, a host
  bonded), a connection without a bond that hasn't started pairing is let go. Until 0.1.19 such a connection stayed,
  holding its slot. One whose passkey was asked for finishes (0.1.20 cut it off): Windows takes long enough over
  adding a device that the 60 s may run out in the middle, and SMP's own 30 s ends a pairing that stalls.
- **A connection's event queue** holds 16 (`connection-event-queue-size-16`; trouble-host's default is 2). trouble-host
  drops an event that finds the queue full, `PairingComplete` too: right after encryption a PC changes PHY, data length
  and connection interval, and with 2 the bond's event was lost. The host had paired, the module never stored it, and
  its keys went with the connection, so the PC couldn't reconnect (first dongle, 2026-11-15: a phone paired, a Windows
  PC "connected" for a moment and then never again, and the KVM didn't show it). A new pairing also asks for its
  connection parameters only once it is complete, so the connection's task keeps taking events while the keys go
  across.
- **The controller's buffers are one pool.** trouble-host sends every connection's packets through one queue, in
  order, each once its connection has a credit for a controller buffer. Upstream 0.8 gives every connection the
  controller's whole buffer count, so with several hosts the module could hand the controller more packets than it
  holds; a connection whose credits then never came back held up the queue for every host. Each host's task waited in
  its next report, stopped taking its host's requests, and the stack logged `OutOfMemory` for each one that came: no
  input anywhere until the module was unplugged (three hosts, 2026-11-24). The firmware builds a copy of trouble-host
  0.8.0 (`firmware/esp32c3/trouble-host`, its changes marked "OpenDynKVM:") whose credits are one pool, freed by the
  controller's completions or a connection's end. **For any other way into that**, a report or reply that has waited
  10 s while the controller reported nothing sent restarts the module (`stall.rs`; longer than the 4 and 6 s
  supervision timeouts it asks hosts for, by which a host gone silent has freed its buffers): the hosts come back with
  their bonds, and the module's log says why.
- **A forgotten host** still lists the KVM as paired: Bluetooth LE has no way to make a host let go of a bond, so it is
  removed on the host too (the app's *Forget* says so). Until then it tries to reconnect with its old keys while the
  module advertises, and the stack turns it away (no key for it).

The selected host is asked for a 7.5–15 ms connection interval, the others for 30–50 ms with latency 4.
Transmit power is capped at +9 dBm (190 mA peaks, design.md §6.1). Bonds live in the last two flash sectors, which
the RP2354B never writes (A/B records with a CRC). A save erases one of them, so the module saves only when a slot
really changes (paired with new keys, forgotten here or by the host; forgetting several is one save): reconnecting
writes nothing, and 200 000 saves at the flash's rated 100 000 cycles last decades (`bonds.rs`, "Wear"). A forgotten
host's keys also leave the Bluetooth stack at once, so it can't come back in encrypted, without a passkey, while
pairing is open. The link task runs on an interrupt executor one level above the Bluetooth side (`esp_rtos`'s
`InterruptExecutor`), so that pairing's P-256 maths and a bond being written never keep it from answering: on the
first dongle (2026-11-14) they did, the RP2354B took the link for lost and restarted the board in mid-pairing. The
RP2354B takes the link for lost after 3 s without a good exchange. IO3 carries a plain-text log (UART1, 115200) to test pad TP1601, once the link has come over the
UART (on a dev board IO3 may be wired to anything); the first 1 KB from before is kept and printed then.

**Programming** (`espisp.rs`, like `at32isp.rs` for the AT32F405s, from the host software's stream: `modstream.rs`).
Since 0.1.51 the RP2354B keeps none of the module's firmware. `cargo xtask dist` builds it and its flash image from
offset 0 (bootloader, partition table, application, with espflash) and writes `kvm-module-esp32c3.bin`
(`kvm_proto::modimage`: a header with the version from the `KVMESP32` marker, the chip, the image's flash offset,
lengths, CRC-32 and MD5; then the image zlib-compressed in two parts, everything after its first 4 KB sector, then that
sector: 365 KB of 661 KB), and the same for the ESP32 (`kvm-module-esp32.bin`, below). The RP2354B firmware is built
with both files' SHA-256 (`build.rs`, `caps::BT_UPDATE`) and takes no other: the key that installed the firmware
vouches for its module, so the stream needs no key of its own, and other host software can't put another firmware on
the module. The host software sends the image for the module's chip (`Status.bt_chip`, which the module's `Hello` or
its ROM gave), and the RP2354B checks the header's chip against the module's ROM.
- **When:** a module whose `Hello` carries another version is sent `EnterRom` and restarts into its ROM bootloader (it
  sets FORCE_DOWNLOAD_BOOT, which every restart but a power-up keeps), and waits there, Bluetooth off
  (`BtModule::NeedsUpdate`, `selftest::BT_FW`). One that says nothing within 4 s (blank, or a firmware that doesn't
  come up) waits as well. A stream that comes while the module is up (`odkvmctl update --module`) sends it
  `EnterRom` first. The app and `odkvmctl update` stream the release's module image after the RP2354B's update,
  once the KVM is back; the Wireless and Firmware pages offer *Update the wireless module* while it waits.
- **Into the ROM:** a module that isn't listening there is caught: the RP2354B holds TX, the module's BOOT strap, low
  (a UART break) across a reset: from 0.18.0 one it makes on the module's EN (GPIO9 through 1 kΩ: 20 ms low under a
  300 ms break, as BOOT held at power-up), else, or if no ROM answers then, a 6 s break across one of the resets such
  a module makes by itself (a ROM that finds no firmware restarts about every 3 s, by its TG0 watchdog).
- **Writing:** SYNC, the chip checked (an ESP32-C3: `GET_SECURITY_INFO`'s chip ID, or the magic value at 0x4000_1000
  on older ROMs), 921 600 baud, the RTC watchdog off, all of the image's flash erased (FLASH_BEGIN: FLASH_DEFL_BEGIN
  erases only the sectors the inflated data goes to, so the image's blank gaps kept another firmware's data, and the
  first ESP32 board, which had run a sketch, read back other than written there: 2026-11-24), FLASH_DEFL_* in 1 KB
  blocks as the
  stream brings them, then the first sector, SPI_FLASH_MD5 against the header's, and a restart through the RTC
  watchdog with FORCE_DOWNLOAD_BOOT cleared: the ROM's own reboot (FLASH_END) keeps that flag, so a module sent
  `EnterRom` would come back in its ROM. The first sector is held in RAM until the whole file has arrived with the
  CRC-32 its `FwBegin` named and the SHA-256 the firmware was built with: a stream that stops (10 s), is cut short or
  is another image leaves it erased, so the module boots nothing of it and is found blank again (below).
- **Flow:** the engine passes `FwTarget::Module` operations to the module's task past the update writer (`kvm.rs`);
  with 40 of them waiting it answers `FwData` with Busy and the host retries (up to 30 s, which covers catching a
  silent module). `FwState` is `Receiving` (bytes so far, which are the bytes written) and then `Writing`, `Done` or
  `Failed`; `Status.bt_progress` is the percentage.
- A module found in its ROM after an unplanned reset (TX low at that moment) is restarted the same way; one hung in its
  firmware is restarted by EN (0.18.0 on), and in any case by its own watchdog (TIMG0, 5 s, fed by the main executor,
  where the Bluetooth side runs: esp-hal's start leaves every watchdog off).
- **No attempt counting:** the RP2354B never programs the module by itself any more, so there is no record in the
  store and nothing to give up on (until 0.1.50: 5 attempts in a row, item 0x0003, `BT_GAVE_UP`, `wireless-retry`).
- **State** (`bt::State`, `Status.bt_module`): not fitted, starting (waiting for its `Hello`), up, being programmed,
  needs an update. `Info.bt_fw` is its firmware while it is up. Hosts hear of a change at once (`BtChanged(0)`).
- **Self-test and LEDs** (§4.10): `BT_FW` while the module needs an update. It doesn't turn the key red: after an
  update it is the expected state until the host software sends the module's firmware. A module that is still
  starting or being programmed isn't a failure; the self-test's verdict waits for it until its 12 s deadline, and the
  bit follows the module after that.
- **Power:** the budget counts `BT_CURRENT_MA` whenever the module is fitted, also while it doesn't answer.

**On a front port** (a *dongle*, for a KVM without the module on its board: 0.12–0.15, or 0.16+ built without it).
Any ESP32-C3 board with the chip's own USB port (its USB-Serial-JTAG, 303A:1001, a CDC-ACM port in silicon: most
small C3 boards) runs the same module firmware, and the RP2354B reaches it on a front port instead of the UART
(`dongle.rs`, `bt::dongle_task`). Above the line everything is the same: the link, `Hello`, the engine, the hosts.
- **One firmware:** the module listens on UART0 and on its USB serial port, and answers, for good, on the one the
  first valid frame came over (`firmware/esp32c3/src/link.rs`). The image a dongle runs is byte for byte the one on a
  board, so testing with a dongle tests that firmware.
- **Which board is taken** (`Engine::port_attached`, `takes_dongle`): a device with the USB-Serial-JTAG's IDs and its
  CDC data interface (`kvm_core::bt::dongle_endpoints`), on a KVM without the module on its board, when no other
  dongle is in use, on a port not set to a passthrough mode. The port stays *probing* while the RP2354B finds out:
  - it says `Hello` as the module (any version) within 4 s: it is the module (`PortState::Module`);
  - it doesn't, and its ROM bootloader (it may be in it already, else DTR/RTS put it there) says it is an ESP32-C3
    whose first flash sector is erased: new, erased (`espflash erase-flash`), or the module after a programming that
    broke off. It is the module, and waits in its ROM for the host software's stream;
  - else it is someone's own ESP32 board, restarted into its own firmware if the RP2354B put it into its ROM, and then
    an ordinary device (`dongle_declined`: a non-HID device goes to the active PC in Auto). Its firmware is never
    touched. A port set to a passthrough mode leaves such a board alone from the start, and setting the dongle's port
    to one plugs it in anew as an ordinary device.
- **Programming:** as on the board (`espisp.rs`), with the ROM reached through the port's DTR/RTS (SET_CONTROL_LINE_STATE
  on its CDC interface, espflash 4.7's `UsbJtagSerialReset`): no EnterRom, no UART break, no speed change. The RTC
  watchdog goes off before the write (the ROM leaves it running in USB download mode), and the chip leaves the ROM by
  a restart through it, FORCE_DOWNLOAD_BOOT cleared first (espflash's `WatchdogReset` for this chip and port): a
  restart by RTS from the ROM brought the first dongle back in its ROM (2026-11-14), the new firmware written and
  verified but not started until the board was powered off. A dongle that needs the module's firmware waits in its
  ROM for the stream, as the module on the board does.
- **Into the ROM:** a dongle that runs another version is sent `EnterRom`, as the module on the board is; DTR/RTS
  is the way in when it doesn't answer, or doesn't go (DTR/RTS didn't get the first dongle into its ROM reliably while
  its firmware ran; from its ROM it did).
- **Restarts:** every restart takes the chip off USB, and it comes back as a new device: its own (an erased chip's ROM
  finds no firmware and restarts every 2.5 s or so, first board 2026-11-14) and the RP2354B's (into the ROM, out of
  it). So each session ends with a restart, and what it found out stays with the board in RAM (by its USB serial
  number, the chip's MAC, since power-up; `bt::Board`): the next session carries on. A new board gets 4 s in all to
  say `Hello`, counted from when it was first seen, so an erased chip that restarts sooner still gets to its ROM; a
  board restarted into its ROM on purpose goes straight there. A restart that Hub-0 doesn't see within 1.5 s is
  followed by a replug of the port (`REPLUG`: data lines off and on, VBUS stays). A board is only declined while it is
  attached and has answered (its ROM: not a blank ESP32-C3), or when 5 restarts into its ROM in a row missed; then it
  is restarted into its own firmware and declined again at once when it comes back (for 10 s). One of ours that won't
  go into its ROM can't be updated and is left alone until the KVM restarts.
- **Adopting a board** that runs other firmware (`Request::AdoptModule`: `odkvmctl wireless adopt <port>`, or the
  Devices page's *Make it the wireless module*, two presses): only on a KVM without a module. The port is probed again
  and the board taken whatever it runs (its port set back from passthrough to Auto, so that it stays taken across its
  restarts): restarted into its ROM, its chip read, and it waits there for the module's firmware, which
  `odkvmctl wireless adopt` streams at once (the window offers *Update the wireless module*).
- **The port:** `PortState::Module`: the LED glows violet, the window and `odkvmctl` say the port holds
  the wireless module and is attached to no PC, and how to get the port back (unplug the board, or a passthrough
  mode). Its current comes from the port, so the budget doesn't count `BT_CURRENT_MA` for it. Unplugged, Bluetooth goes
  as for a module that stops answering, and the next board may be the module.
- **Link:** the same 72-byte frames, a full USB packet and a short one each way, one packet per millisecond: about 4 ms
  an exchange, more while either side is busy. USB loses no frame, it only delays one, so the RP2354B waits 40 ms for a
  reply (`REPLY_TIMEOUT_USB`) and the module 30 ms for a frame's second packet (`link::rest_of_frame`; the UART keeps
  4 ms and 5 ms). Until 0.1.21 those were 10 ms and 5 ms: with the mouse going to a Bluetooth host about 15 exchanges
  a second ran over (measured on the first dongle, 2026-11-15; next to none while nothing moved), each one was resent,
  and the late reply broke the next exchange too, so the pointer stuttered.

**Bluetooth devices** (0.1.50, `devices.rs`, `kvm-core::bt::Devices`). At the same time as it is a keyboard and mouse
to its hosts, the module is the central of up to four Bluetooth LE HID devices (HID over GATT: keyboards, mice, other
input devices; the ESP32-C3 has no Bluetooth Classic, so devices that only have that can't pair), in device slots 1–4.
To the engine each is a device as a front port's is (§4.3): a mode (Auto, which is Normalized for it, Normalized or
Off) and a routing in every profile, its reports parsed with its report map by the same translator, merged with the
front ports' and sent wherever its routing says, keymap and hotkeys included.
- **Looking for devices** (`BtScan`, 60 s): an active scan; a connectable advertiser that is discoverable (LE Limited
  or General) and a HID device (the HID service's UUID 0x1812 or a HID appearance), not bonded already, is reported
  (`Found`, at most once a second each, its advertisement and scan response merged for the name). The engine keeps the
  eight strongest (`MAX_FOUND`). Host pairing closes while looking, and the other way round: the stack has one set of
  IO capabilities.
- **Pairing** (`BtPairDevice`): the module stops looking, connects (10 s), and pairs as the initiator with IO
  capability DisplayOnly: a keyboard (KeyboardOnly) gets six digits the window shows to be typed on it, then Enter; a
  mouse pairs without (Just Works). LE Secure Connections or legacy pairing, which many keyboards and mice still have
  only (`legacy-pairing`, let in only while a device pairs: hosts keep Secure Connections only). Then it reads the HID
  service: the report map (up to 512 bytes), each Report characteristic's Report Reference (ID and type), the Protocol
  Mode (set to Report), the battery level and the name (advertised, else GAP's Device Name). All of it is stored with
  the bond, written once, and the reports and the battery are switched on. A device without a HID service or input
  reports is not kept. `Status.bt_device_pairing` says how far it got (connecting, pairing, reading, paired, failed).
- **Reconnecting:** while a bonded device isn't connected, a passive scan watches a quarter of the time (40 ms every 160
  ms: about 20 mA at 3.3 V while a bonded device stays off) for its advertising, by its address or one its IRK resolves,
  and connects at once with the stored handles: a device that wakes up reports without being read anew. Its connection's
  parameters are what it asks for. A reconnection never becomes a pairing. The link is encrypted with the bond's key by
  `GattClient::new`, the stack's own way for a bonded peer, which stops when the key fails; the module doesn't ask for
  security itself (`request_security` is the user's way: when the key failed once, the stack sent the device a pairing
  request, a keyboard took it and changed its key, and every reconnection after ended in a MIC failure, until 0.1.49).
  The link counts as encrypted by the stack's state, as its `Encrypted` event doesn't always come, and its
  `PairingComplete`, which it also gives when the bond's key worked, isn't taken for anything. When the key doesn't work
  (`BondLost`) or the stack wants a passkey, the module lets the connection go without telling the device that a pairing
  failed; a link not encrypted 4 s after it connected, or not set up within 8 s more, goes too. Making the GATT client
  is tried up to 10 times, 100 ms apart, while the device stays connected (a keyboard waking while a mouse moved lost it
  again and again, 2026-11-20); its error and the stack's free packets go to the log. A reconnection that failed holds
  off the next for 0.5 s, doubling to 4 s while they keep failing. From 0.1.34 to 0.1.37 the module took that
  `PairingComplete` for a pairing begun anew and let every reconnection go the moment it was encrypted; before, it asked
  at once (and in 0.1.33 again after 2 s, which after a failed key made the stack pair anew) (2026-11-20, found with the
  module's log, `odkvmctl device log`). (0.1.35 to 0.1.39 also worked around a supposed address-type mismatch in the
  stack's bond lookup for hosts; that wasn't what failed, one host in two slots was (**Flash**), and 0.1.40 dropped it.)
- **Reports:** each input report's notification goes to the RP2354B (`DeviceInput`), which puts the report ID back in
  front when the map has IDs and parses it as a USB device's report. A report longer than a link message (62 bytes) is
  dropped. The engine asks for a device's map (`RequestMap`) once when it holds none with the CRC the device's state
  gives, so a restart of either side costs one map, not a pairing; a map not whole 2 s later (a piece lost) is asked for
  again (`MAP_RETRY_MS`), one that is whole but doesn't parse is not, and the device shows as not supported. The parser
  keeps what fits its capacities and leaves the rest out (`Layout::incomplete`; a long bitmap of individually listed
  media keys is split into fields and read exactly): until 0.1.32 such a map was rejected, and a keyboard paired and
  showed as not supported (2026-11-20). A USB device with an incomplete layout counts as not fitting the normalized
  interfaces (`misfit::INCOMPLETE`), so Auto mirrors it as before. Until 0.1.31 every state the module sent during the
  transfer asked again, and the module's single queue to the RP2354B lost a message per exchange while more than 8
  waited: a keyboard said connected and never typed (2026-11-19). The module now queues input reports and the rest
  separately, as the RP2354B does. While a device is connected the RP2354B polls the link every millisecond instead of
  every 5 ms, and again at once after a frame that carried a message (the module only answers). A keyboard's LEDs are
  its target's (`DeviceOutput`).
- **Flash:** the device slots live in the two sectors before the hosts' (A/B records with a CRC, `bonds::DeviceBonds`):
  a device's save never touches the hosts' bonds. Saves come only from pairing, forgetting and a lost bond. Both flash
  handles are made before either reads: a handle learns the flash's size from the chip's ID, which comes back wrong
  after a read; until 0.1.41 the devices' handle took the flash for 0 bytes long, so no device's bond was ever saved and
  a module restart lost them all (2026-11-20). One peer is never in two slots: a host or device that pairs anew clears
  any other slot that holds it (the same address, or an IRK of one resolving the other's, or the same IRK), with the
  same save, and a start that finds one peer in two slots keeps the first. A host in two slots put two entries for it in
  the controller's resolving list, and the controller asserted about 0.3 s after that host paired anew; firmware back to
  0.1.29 did the same until the slots were cleared (2026-11-20). *Erase all pairings* (`BtEraseAll`) clears whatever
  state is left: both records of both kinds, the stack's keys, and with the restart the controller's list.
- **Radio:** the controller takes 10 activities at most (connections, advertising, scanning): five host connections,
  four devices, advertising and a scan would be 11, which only happens with every slot of both kinds in use while a
  host pairs and a device is sought; the one that doesn't get its activity tries again later. The stack holds 17 keys
  (the eight slots' and one pairing's per connection; `tidy_keys` keeps the devices' too). A panic restarts the
  module; its place and message are kept in RTC RAM across the reset and sent with the next start's log lines, with
  the reset's reason. (The controller's own log lines can't be kept: esp-radio's `print-logs-from-driver` makes the
  controller log so much that the module stops advertising (0.1.39) or hangs until its watchdog resets it (0.1.43).)
  The Bluetooth stack's own warnings and errors go to the log (`log::Stack`, at most 10 a second; the rest of its
  lines are compiled out): why an encryption with a bond failed, for one.
- **The window and `odkvmctl`** show a device slot only while it holds a bond: a free slot is nothing to set. A
  device pinned to a Bluetooth host makes the module keep that host connected (`Select`'s mask) whatever its policy.

**Control over Bluetooth** (0.1.52, protocol 15; `kvm_proto::link::bt::control`, `ble.rs`, `Engine::bt_control`). A
computer paired as one of the hosts reaches the control protocol through the module, so a laptop or tablet that is
only a Bluetooth host can be configured without a cable (§8.1 "Over Bluetooth" for what it may ask).
- **Service:** a GATT service of its own, UUID `98141409-422d-48a6-aba2-21d691a393cc`: a characteristic the host writes
  64-byte requests to (`…1410…`, write and write without response) and one that notifies 64-byte answers and events
  (`…1411…`). Both need the bond's authenticated link (`permissions(authenticated)`: hosts pair with a passkey), and
  the module forwards a request only from a bonded slot that isn't pairing. The service's UUID is also in the scan
  response (30 of its 31 bytes with the name), for a host's software that looks for the KVM by it.
- **Session:** the reply characteristic's notifications on are a session (`host_flags::CONTROL`, in `HostState` to the
  RP2354B): the engine's events go there too, as to the PCs. They are never kept with the bond, nor counted by the
  wear guard; each connection starts without (`end_control_session`), and each session subscribes anew.
- **Size:** a report needs an ATT MTU of 67 (`control::MIN_MTU`): desktop and mobile systems negotiate 185 or more; a
  smaller one is logged.
- **Speed:** each request takes about two connection events. While requests come the host is asked for the selected
  host's 7.5–15 ms interval, back to its own 5 s after the last (`CONTROL_FAST`), and the RP2354B polls the link every
  millisecond for 0.5 s after each (`CONTROL_BUSY`). The host software keeps up to 6 requests under way
  (`control::WINDOW`, `Client::request_all`): a window's refresh is about 200 requests, which one at a time would take
  several seconds.
- **Queues:** the module holds 24 reports per host on their way to be notified (`CONTROL_OUT`): the 6 answers under
  way and the events between them, of which a switch alone brings several (the switch, the active host, each port,
  each host whose interval changes). One that doesn't fit is dropped. Until 0.1.53 there was room for 8, and a switch
  made a Windows host's window drop the KVM (2026-11-24).
- **A missed answer** is no lost KVM over this link, unlike over USB (`Client::lost`): a read whose answer doesn't
  come within the second is asked again once (`Client::request`, `request_all`; a change is not, it may have been
  made), and only 3 answers missed in a row (`MISSED_LOST`), or the link itself failing, make the window let go and
  connect again.
- **Connecting again** (Windows, macOS): a link that goes stops its notifications' thread and waits for it to let go
  of its GATT objects (`ble::sys::Link`'s `Drop`). Windows refuses a program's new link ("access denied") while it
  still holds the objects of an earlier one, and until 0.1.53 that thread held them until the next notification came:
  the window then couldn't reach the KVM again until it was restarted, or until another switch notified it.

**Diagnostics** (§8.4): a *Bluetooth* group, only with the module fitted: its state (as `Status.bt_module`),
the version from its `Hello`, the UART exchanges that failed (`bt::LINK_ERRORS`) and the hosts connected; while there
are any, the hosts whose notification settings the module holds in RAM (`BT_SETTINGS_HELD`, orange).

**On hardware** (2026-11-14, the first 0.12.0 board with an ESP32-C3 board on its Other port): the dongle is found,
programmed from blank and updated over USB (interrupt slots, DTR/RTS into the ROM, the watchdog restart out of it),
pairs with the passkey typed on the KVM's keyboard, takes input when switched to, and takes part in screen hop. Mouse
to host 5–10 ms at the host's 15 ms connection interval, against 1 ms over USB. **Unverified** (no module on a board
yet): the UART link at 3 Mbaud, the UART break as a way into the ROM of a blank module, the ROM's FLASH_END reboot;
and several hosts at once. Host side and engine are tested on the PC
(`kvm-core` engine tests, the simulator with `OPENDYN_SIM_BT=1`, or `OPENDYN_SIM_BT=dongle` for a board on the Other
port).

**The ESP32 module** (`firmware/esp32`, `kvm-classic`, 0.1.53). An original ESP32 is dual mode, so it is a Bluetooth
Classic host as well: for gamepads that pair only over Classic (the 5 series of the symmetric-stick pads). The module
can be either chip, and the RP2354B's side is the same for both: the link, its messages, the device slots.
- **Firmware:** `firmware/esp32` builds the ESP32-C3 firmware's modules (`#[path]`: the hosts, the LE devices, the
  bonds, the link, the log, the stall watchdog), their few chip-specific parts chosen by `target_arch`, plus its own
  `main.rs`, `radio.rs` and `classic.rs`. It needs the Xtensa toolchain (espup) and a copy of esp-radio
  (`firmware/esp32/esp-radio`, its changes marked "OpenDynKVM:"): upstream starts the ESP32's controller for LE only,
  the copy in dual mode (four Classic ACL links, no audio ones, whose memory esp-hal's linker script already keeps out
  of the firmware's), and it gives four ROM routines the controller library calls and esp-rom-sys's ROM script lacks
  (their addresses from ESP-IDF's). With Bluetooth on, the ESP32's main DRAM is 128 KB: a 72 KB heap (the
  controller's memory) and the LE stack's 23 KB of resources live in the DRAM after the ROM's data, 12 KB more heap
  and the other statics in the main DRAM, and the stack gets some 38 KB. Every device slot takes an LE device or a
  Classic pad, and is free for either once forgotten: four of either at once, an LE device's connection state on the
  heap while it lasts. Built without frame pointers: with them the Xtensa backend moved the stack pointer 4 bytes to
  pass a call's arguments, and 16-aligned values (the LE stack's keys) were copied to the wrong place.
- **The link, either place:** the ESP32 has no USB of its own, so on a front port it is a board whose USB-to-UART
  bridge (a CP210x: 10C4:EA60) is on its UART0, and on a board (to come) its UART0 is wired to the RP2354B. It listens
  at 3 Mbaud (the board's) and at 1 Mbaud (`BRIDGE_BAUD`: the CP2104 goes to 2 Mbaud) in turn, a quarter second each,
  and keeps the speed the first valid frame came at. Its `Hello` says its chip, and that it is a Classic host
  (`hello_flags`); a module whose `Hello` says no chip runs the ESP32-C3's firmware from before.
- **A bridge dongle** (`dongle.rs`, `DongleKind::Cp210x`): vendor requests set the bridge's UART on, 8N1, its speed
  (the link's, the ROM's 115 200, the transfer's 921 600) and DTR/RTS, which drive the board's EN and GPIO0 through
  the usual two transistors: esptool's classic reset into the ROM, RTS alone out of it. The bridge stays on USB while
  the ESP32 restarts, so a session goes on over the same line (`Outcome::Again`), where an ESP32-C3's ends and waits
  for the chip to come back on USB. As for an ESP32-C3 board, a CP210x board is only taken on a port not set to
  passthrough, and only if it says `Hello` as the module or its image's first sector is erased, or when adopted (above:
  the first HUZZAH32, 2026-11-24, ran a sketch and was declined until then).
- **The ROM** (`espisp.rs`): the chip comes from `GET_SECURITY_INFO` (the ESP32's ROM doesn't know it) or the magic
  value (0x00F0_1D83). The ESP32's ROM begins a flash region with four words (the ESP32-C3's five), its image starts at
  0x1000 (`Header::base`: espflash's merged image pads the sector before), its UART download mode leaves the RTC
  watchdog off, and it has no FORCE_DOWNLOAD_BOOT: `EnterRom` can't send it into its ROM (it logs that), the RP2354B
  does (DTR/RTS on a bridge, the break with EN on the board), and it leaves by a reset with its BOOT strap high
  (`Done::ResetIt`: RTS, or EN). **On a board** its GPIO0 must be on the RP2354B's TX line as the ESP32-C3's IO9 is,
  and its EN on GPIO9.
- **Classic** (`classic.rs`, with `kvm-classic`'s tested pieces): it shares the controller with trouble-host through
  `radio::Split`, which takes Classic's events, its links' ACL data and its share of Number of Completed Packets out of
  what the LE stack reads (`kvm_classic::hci::route_event`; a link's handle is Classic's from its Connection Complete to
  its Disconnection Complete), and sends its commands through the same controller. Once the LE stack has set the
  controller up it adds Classic's events to the event mask, turns on Secure Simple Pairing, extended inquiry results,
  role switches and page scan, and names itself (class: a computer). Looking for devices runs inquiries next to the
  LE scan; HID peripherals that answer go to the RP2354B as `Found` with a Classic address (type `ADDR_CLASSIC`).
  Pairing one: connect, authenticate (Secure Simple Pairing, Just Works: a pad has no input and no output), its link
  key into a free device slot (`bonds::Device`, the key where an LE device has its LTK), encryption, then L2CAP's HID
  control and interrupt channels (`kvm_classic::l2cap`). A bonded pad reconnects by paging the module (its home
  button): accepted, its key given, and it opens the channels itself (else the module does, after 2 s).
- **The pad's reports:** it sends a short report until its feature report 5 is read, then its full report 0x31 with a
  CRC-32; the module reads that feature report, lights its middle player light, and passes the full report on in the
  USB form of its report 1 (`kvm_classic::pad`: the USB report's body but its last, reserved, byte, which is what one
  link message holds), with that form's report map and the USB IDs (`DeviceState::vid`, `pid`). The engine applies the
  pad's profile to a Bluetooth device as to one on a port (`kvm_core::pads`): the buttons in the standard order, the
  right stick and the triggers moved, the touch pad.

**On hardware:** not yet (2026-11-24): the ESP32 firmware, the RP2354B's bridge dongle and both chips' ROM handling
are built and their logic tested on the PC (`kvm-classic`: HCI events, L2CAP signaling, the pad's reports and
descriptor through the KVM's HID parser; `kvm-core`: a bridge found as a dongle, the pad's reports through the engine).

### 4.14 Screen hop (`kvm-core::hop`)

The KVM moves the pointer itself, across an arrangement of its hosts' screens (both PCs' and the
Bluetooth hosts'), and switches to another host when the pointer is pushed across an edge onto one of its screens
(design.md §4, "Screen hop"). It runs on the RP2354B, in the engine, on the motion of the normalized pointing devices;
the hosts get an absolute pointer (a Bluetooth host set so: relative motion, below).

- **Needs:** an arrangement in the active profile (`profileN-arrangement`); the absolute pointer (`abs-pointer`,
  below); the Mouse port's device running Normalized (an empty port is fine: any normalized pointing device moves the
  pointer); the active host (a PC in report protocol, or a Bluetooth host) with a screen in the arrangement.
  `Status.hop` says which is missing, and the hosts offer the fix. Until 0.1.12 a Bluetooth host with the input
  turned it off (`HopState::Wireless`, never sent since).
- **Arrangements**: the KVM keeps four (`MAX_ARRANGEMENTS`), each a name (`arrangementN-name`, empty:
  "Arrangement n") and 8 screen slots, and each profile uses one of them or none (`profileN-arrangement`). Each is a
  store item of its own (0x0010–0x0013, §4.8, at most 160 bytes: its name and the slots in use): editing one writes
  only it, and switching profiles, which switches the arrangement in use, writes nothing (unless the profile itself is
  remembered). Another arrangement in use starts the pointer over in the middle of the active host's main screen.
- **Absolute pointer:** with `abs-pointer` on, interface 1 carries report IDs: ID 1 is the relative mouse as before
  (the resolution-multiplier feature report with it), ID 2 an absolute pointer, a top-level collection of usage
  Pointer: 16 buttons, X/Y 0–32767, wheel and AC Pan in detents (`normalized::MOUSE_ABS_DESC`). A collection of its
  own and another usage than Mouse, so Linux makes it an input device of its own (an absolute pointer, as a virtual
  machine's tablet), and Windows a second mouse. Changing the setting re-enumerates both PCs once; a profile change
  never does, so a profile without hop (gaming) gets the relative mouse at once. In boot protocol everything is
  relative.
- **Arrangement:** 8 screen slots (`arrangementN-screen0`–`7`), each a host's (PC1, PC2, or a Bluetooth host slot:
  its `Host` value, in the slot's host byte): resolution, display scaling (100–400 %), its top-left in that
  host's own display arrangement (`desk`, in its units, below), and its top-left in the arrangement (`at`, logical
  pixels: a screen at 150 % takes two thirds of its pixels each way, so screens of the same physical size line up).
  The pointer lives in the arrangement, in logical pixels with sub-pixel precision. Two flags per screen (in the
  slot's host byte):
  - **Off for hop** (`off`): a screen of a monitor cabled to both PCs, on the PC it doesn't show. It lies where the
    other PC's screen is; the pointer never goes onto it, but it still counts for its PC's desktop (mapping, below).
    Measured on the first board's desk (2026-11-12): PC1 (KDE) had the shared 4K monitor left of its HD monitor, and
    with the HD monitor alone in the arrangement the cursor sat 2.33× too far along, on the 4K monitor's hidden area,
    before every hop.
  - **Fixed** (`fixed`): its `desk` is the PC's own (detected, or typed in); the app doesn't derive it from where the
    screen is dragged. The KVM only stores it.
- **Units** (`hop-units-pc1/-pc2`; `hop-units-host1`–`4`): what a host's `desk` coordinates
  count. `physical` (Windows, X11): a
  screen covers its resolution there. `logical` (Wayland desktops: KDE Plasma, GNOME): it covers its arrangement
  size, its resolution over its scaling, and an absolute pointer is spread in those units.
- **Edges**, axis by axis: past a screen's edge, the next screen in that direction among those spanning the pointer's
  other coordinate (the nearest, however far: a gap is jumped; up to 8 pixels of overlap still count).
  - The same host's: the pointer goes on, by what is left of the motion.
  - Another host's: a wall until pushed `hop-resistance` pixels further (default 100; a pause of 400 ms starts the
    push over, a motion back from the edge too). Never within `hop-corner` pixels of either end of the current
    screen's edge (default 20, so corners stay easy to hit), never while a button is held (a drag stays on its host),
    and only to a host that is ready: a PC connected, awake and in report protocol, a Bluetooth host bonded,
    connected, encrypted and subscribed. Then the host left gets the pointer at its edge, released (a switch like any
    other, `SwitchSource::Edge`), and the other host the pointer at the same place along its screen's edge.
  - Nothing there: a wall.
- **Mapping** (`hop-mapping-pc1/-pc2`; `hop-mapping-host1`–`4`): where a host puts 0–32767. `desktop`:
  across the bounding box of all its screens' `desk` rectangles, those off for hop included (Linux: X11, GNOME, KDE).
  macOS (host software 0.1.27 on) detects as `desktop` in logical pixels (points); how it maps an absolute pointer
  onto several displays isn't measured yet (a community project reports it on the display the cursor is on,
  2026), so its host software places the cursor on all of them (below), and `desktop` alone is only sure for one display.
  `main-screen`: on its main screen only, the one at `desk` 0,0 (Windows 10 and later map an absolute HID mouse onto the
  primary monitor). Its other screens get relative motion instead: the KVM knows where it last put the cursor (an
  absolute report puts it exactly there) and sends the difference to where the pointer is, in the PC's pixels; back on
  the main screen, the next absolute report puts it exactly in place again. Windows accelerates that relative motion
  itself ("Enhance pointer precision"), and its curve counts its physical pixels, whatever the screen's scaling. So
  with `hop-accel` other than `flat`, once the cursor's place is known, the host gets counts that the same curve
  (`hop-accel`, `hop-speed`, at the mouse's speed now) takes as far as the KVM's took the pointer, in that screen's
  pixels (`hop::Raw`, `Accelerator::counts_for`): the mouse's own counts at 100 %, more at 200 % (two pixels per
  logical one), less than twice as many as the curve is steeper faster; less what the KVM held back at a wall
  (all of it where the host's own desktop ends at that wall too, `desktop_ends`: its cursor stops there by itself and
  meets the edge even where its curve took it less far, a fast motion), and with the fraction of a count kept for the
  next report. Measured on the first board (2026-11-16), a 2880×1800 screen
  at 200 %: until 0.1.24 the KVM's motion went, accelerated by the KVM and then by Windows again, so the cursor ran
  about twice as fast as the pointer and jumped to the main screen's edge when the pointer got there; 0.1.25 sent the
  mouse's counts alone, and the cursor got only half way across before the pointer met the wall. With `flat`
  (Enhance pointer precision off: Windows moves one of its pixels per count) the difference goes, in its pixels. What
  still differs (another slider position, another curve) drifts until the cursor rests (cursor sync, below) or
  returns to the main screen.
  - Tried and dropped (0.1.26, first board, 2026-11-16): an external pen instead (page Digitizer, usage Digitizer),
    which Windows maps onto its whole virtual desktop where it puts an absolute mouse on its main screen only. It
    hovered on every screen exactly once the host's `desk` places were Windows' own, but no click came through: not
    with the pen in range (Windows takes no mouse button then), nor with the pen taken out of range just before the
    mouse pressed. Its own tip and barrel would click only as a pen does (a hold turns into a right-click, a drag
    starts after a threshold, no middle or side buttons). Every screen absolute as a mouse needs a filter driver that
    sets `MOUSE_VIRTUAL_DESKTOP` (Windows' HID mouse driver never does), and that needs Microsoft's signature.
- **Two pointers, one button:** to a PC the relative and the absolute pointer are two devices, and Linux keeps a
  button down on the device it was pressed on. A button stays on the pointer it was pressed on and is released there
  (`PcOutput::held`); switching away releases both, the absolute one where it is.
  - With `main-screen` the buttons go on the relative pointer, and the absolute one only places the cursor (a
    Bluetooth host likewise: its mouse report carries them, its pointer report none). An absolute report always
    carries a position, and that pointer's reaches the main screen only: until 0.1.23 a window dragged from the main
    screen onto another was let go by the absolute pointer at its last position, and Windows dropped it back on the
    main screen (first board, 2026-11-16). Windows keeps one button state for all its mice, so a press on one and the
    motion on the other drag as one.
- **Acceleration** (`hop-accel`, `hop-speed` −10–10): an absolute pointer isn't accelerated by the PC, so the KVM is.
  The speed is measured as libinput measures it: back over the last 16 reports within 300 ms while the direction
  stays within neighbouring octants and the speed within 1 count/ms of the latest; the first report after a pause
  counts as slow.
  - `flat`: Windows' speed slider alone (position 10 + speed, ×1/32–×3.5).
  - `precision` (default): Windows' "Enhance pointer precision" curve (SmoothMouseXCurve/YCurve: 0, 0.43, 1.25, 3.86,
    40 → 0, 1.07, 4.14, 18.98, 443.75 inches per second, interpolated), with 400 counts and 96 pixels to the inch,
    times the slider: about 0.6 pixels per count when slow, 1 at moderate speed, 2.5 and more when fast.
  - `adaptive`: libinput's adaptive profile at 1000 counts per inch (its default): 10 v + 0.3 below 0.07 counts/ms,
    1:1 up to the threshold (0.4 − 0.25 s, at least 0.2), then a rise of 1.1 + 0.75 s to the cap 2 + 1.5 s, with s
    = speed / 10, averaged between the last speed and this one (Simpson's rule) as libinput does.
  - Mouse keys of the keymap accelerate themselves and go through unaccelerated.
- **Cursor sync** (`PointerAt`): a PC's host software may say where its own cursor is (its pixels).
  Taken over once the KVM hasn't moved the pointer for 150 ms: a touchpad or a program moved it, or Windows'
  acceleration on a screen reached relatively. `opendyn-kvm` sends it on Windows and macOS each time the cursor has
  rested for 250 ms while hop is on in the active profile; on Linux the position isn't available to programs on
  Wayland (and isn't needed: the whole desktop is absolute).
- **Placed by the PC** (`PlaceCursor`; optional): a PC's host software may put its cursor where the KVM's
  pointer is itself, on the screens it names (`Placement`): Windows those its mapping reaches only with relative
  motion (`others`: all but its main screen, which keeps the absolute pointer), macOS all of them (`all`: how macOS
  maps an absolute pointer onto several displays isn't known, below). It holds a lease of `PLACE_LEASE_MS` (1 s) and
  asks again every 300 ms while it may; meanwhile the KVM sends it the place on those screens, in its own pixels
  (`Event::PlaceCursor`, to that PC only, at most one a millisecond and then the latest: the control interface takes
  one report a millisecond, a mouse may send eight; `Engine::place_cursor`), and nothing on the pointer interfaces
  but a change of the buttons, the wheel or pan on the relative pointer (`PcOutput::pointer`). Without a lease (no
  host software, a work PC that may not run it, the system refusing) the PC's mapping
  alone applies, as above, and the hop works as before; a lease that runs out (the program crashed, the PC went to
  sleep) costs at most a second without the pointer on those screens, and the KVM then goes on relatively from where
  the cursor was put. A bus reset ends it. `opendyn-kvm` places on a thread of its own with a handle of its own on
  the control interface (`odkvmctl::placer`: the worker's snapshot once a second would hold placements up), on unless
  turned off on the Screen hop page (a setting of this computer), never with the simulator:
  - Windows: `SetCursorPos`, in physical pixels (the thread per-monitor DPI aware), while the input desktop is
    "Default": the sign-in, the lock screen and an administrator prompt ("Winlogon") get no lease. A placement
    refused (an administrator's window in front, which a program of the user may not reach) gives the lease back at
    once, and it is asked for again 2 s later.
  - macOS: a mouse-moved event (dragged, with the button that is down, so that a drag follows) at the place, with the
    modifiers held and the motion since the last, posted to the HID event tap from an event source that holds back no
    local events (by default macOS suppresses the real keyboard and mouse for 0.25 s after a synthetic event, and
    during a synthetic drag, which would swallow the KVM's own keys and buttons). macOS allows it once the app is
    under System Settings, Privacy & Security, Accessibility; until then the page says so, with *Allow…* for the
    system's prompt.
  - Not yet tried on a board or a Mac (2026-11-17): unit tests of the engine, the hop and the placer's protocol.
- **Switching otherwise** (hotkey, key, software, auto-select): the pointer is found where it was left on that PC, or
  in the middle of its main screen.
- **Bluetooth hosts** take the module's absolute pointer (report 4, §4.13), or with `hostN-pointer
  relative` the relative mouse only (`BtPointer`; for systems that take no absolute pointer, some tablets and phones).
  A relative host's cursor isn't known when the pointer comes onto it (a hop, or any switch to it): the KVM first
  pushes it into the top-left corner of the host's desktop (`hop::PARK`, 30 000 counts each way, further than any
  desktop before the host's own acceleration), then moves it to the pointer by the difference, and goes on with
  differences: with `hop-accel` other than `flat`, counts for the host's own curve (as for Windows' screens reached
  relatively, above; its units decide its pixels per logical one), else the difference in its pixels. Wherever the KVM holds the pointer at an edge (a wall, a push toward another host
  short of its resistance, a corner, a held button), the push goes on to the host as well, so its cursor meets that
  edge too: where the host's acceleration differs from the KVM's, the cursor drifts from the pointer only until the
  next edge. Leaving
  a Bluetooth host releases its buttons on its mouse and on its absolute pointer. A Bluetooth host has no host
  software: no cursor sync, and its screens are added by hand (`hostN 2388x1668 200% at …`), not detected.
- Costs (measured 2026-11-13): the pointer with its speed tracker is 504 bytes of the engine (a place and a cursor for
  each host), and the config in RAM 824 bytes, 632 of them the four arrangements. In flash each arrangement's item is
  at most 160 bytes (`config::ARRANGEMENT_ITEM_MAX`), the config item at most 344 of `config::ITEM_MAX` 576. The
  arithmetic is single-precision float (the RP2354B's FPU); no `libm`.

## 5. AT32F405 firmware

### 5.1 Clocks and pins

- HEXT 12 MHz → PLL (12 MHz ÷ 1 × 48 = 576 MHz VCO, ÷ 4) → **144 MHz** system clock, switched with auto
  step-by-step; APB2 at 144 MHz (SPI1), APB1 at 72 MHz; 4 flash wait states (RM v2.02 §5.8); ADC clock HCLK ÷ 6 =
  24 MHz.
- **Without the crystal** (HEXT not stable within 100 ms, counted at the 8 MHz reset clock): the PLL takes HICK / 6 =
  8 MHz instead (`PLLRCS` = 0, `HICKDIV` left at its reset value; RM §4.1.1) × 72 ÷ 4 = the same 144 MHz, factory
  calibrated to 1 %. SPI link, timers and status work as usual; the USB core isn't started (the HS PHY needs the
  crystal), its commands are drained, and `Hello` carries `NO_CRYSTAL` (§4.9).
- **Why 144 MHz:** about 35 mA instead of 57 mA per chip (DS v2.03 Table 21; analysis.md finding 6), with plenty of
  headroom for HS. The LDO stays at its 1.2 V default, which allows up to 192 MHz.
- The OTGHS PHY takes its 12 MHz straight from HEXT (`CRM_OTGHS.USBHS_PHY12_SEL = 0`, the reset value). Power-up order:
  OTGHS clock on → `GCCFG = PWRDOWN | VBUSIG` → wait 1 ms (RM §21.3.2) → core soft reset → device mode. `GCCFG` is
  written directly: its bits differ from ST's (ST bit 21 = VBUS detection enable, Artery bit 21 = VBUSIG), so the
  Synopsys driver's `config_v*` helpers are not used.
- `embassy-time` runs on a SysTick driver at 1 kHz; nothing here needs finer timing.

| LQFP-48 pin | Port | Net | Use |
|---|---|---|---|
| 14–17 | PA4–PA7 | `ATn_CS/SCK/MISO/MOSI` | SPI1 slave (hardware NSS). PA4's mux stays on SPI1, never OTGHS_SOF |
| 22 | PB12 | `ATn_IRQ` | output, high = "I have frames to send" |
| 19 | PB1 | `HUBn_RST` | open drain; pulses the PC hub's reset for recovery, and holds it in reset after 1 s without the PC's 5 V (§4.12). Shared with an RP2354B GPIO (6 / 35), which holds the hub in reset while it flashes this chip |
| 10 | PA0 | `VDET_PCn` | ADC, VBUS / 2 |
| 11 / 18 | PA1 / PB0 | `CC1/CC2_PCn` | ADC, advertised current (thresholds as §4.7) |
| 2 | PC13 | `ATn_ROLE` | strap: low = PC1, high = PC2 |
| 26 / 27 / 25 | — | OTGHS1 D−/D+/R | USB HS to the PC's hub |

### 5.2 USB device core (`crates/kvm-usbdev`)

- At boot the core allocates **EP1–EP7 IN and EP1–EP7 OUT** as interrupt endpoints with a 64-byte max packet size,
  once. The OTGHS has exactly 7 + 7 plus EP0, and every endpoint a PC can see on this device is an interrupt endpoint
  (HID and XInput). A new descriptor set therefore never re-creates the driver: the core soft-disconnects
  (`DCTL.SDIS` for 50 ms, through the firmware's `Host::soft_connect`; the driver's own `Bus::enable/disable` are no-ops),
  swaps the set and reconnects. FIFO use is about 300 of the 1024 words.
- Until the first descriptor set arrives, the core never polls the bus, so the controller stays uninitialised and off
  the PC's hub.
- **Serves the descriptor set verbatim:** device, configuration, string, BOS, HID report and Microsoft OS descriptors. It
  generates the device qualifier and the other-speed configuration itself, and patches `bInterval` / `wMaxPacketSize`
  when the hub link came up at full speed.
- **Handled locally** on interfaces flagged `LOCAL_HID`: `SET/GET_IDLE`, `SET/GET_PROTOCOL` (changes are reported to the
  RP2354B), `SET_REPORT` (acknowledged, cached for `GET_REPORT`, reported to the RP2354B), `GET_REPORT` (the cached value,
  or an idle report — nothing pressed — with its report ID).
- **Forwarded** to the RP2354B on interfaces flagged `FORWARD`: every class and vendor request. The data or status stage
  is NAKed until the reply arrives (timeout 500 ms → STALL). The RP2354B forwards it to the real device if this PC is
  active, or answers from its cache if not (design.md §4).
- Standard requests, remote wakeup and endpoint halt are handled locally.

### 5.3 SPI slave link

- SPI1 slave (PA4–PA7, MUX5), mode 0, 8-bit, hardware NSS on PA4. DMA1 channels 1 (RX, DMAMUX request 10) and 2
  (TX, request 11), armed for exactly one 72-byte frame.
- An EXINT on PA4's rising edge (NSS released) ends the frame: the ISR checks the CRC, runs the receive half of the ARQ,
  queues the message, builds the next outgoing frame and re-arms both DMA channels. This takes about 4 µs; the RP2354B
  keeps 15 µs between frames.
- **Flow control:** when the inbox is full, a received frame is handled as lost, so the ARQ doesn't acknowledge it and
  the sender repeats it later. The RP2354B side does the same.
- `IRQ` is high whenever the ARQ has something to send.

### 5.4 Status

Every 100 ms and on change: USB state (detached / default / addressed / configured / suspended), link speed, VBUS, CC1,
CC2, frame number, error counters, whether the PC allowed remote wakeup, and for the diagnostics (§8.4) the chip's VDDA
(the board's 3.3 V, against its 1.20 V reference VINTRV on ADC1_IN17) and die temperature (ADC1_IN16) → `Status`
message. Fields are only ever added at the end: firmware 0.1.0 sends neither the wake flag (the RP2354B then reads "not
allowed") nor the two measurements (left out of the diagnostics).

## 6. Link protocol (RP2354B ↔ AT32F405)

### 6.1 Frame

Fixed **72 bytes**, both directions on every transaction (SPI is full-duplex):

| Offset | Size | Field |
|---|---|---|
| 0 | 1 | `magic` = 0xA5 |
| 1 | 1 | `flags`: bit 0 `DATA` (carries a message with `seq`), bit 1 `SYN` (sender restarted), bit 2 `MORE` (sender has more queued) |
| 2 | 1 | `seq`: sequence number of this message; in a frame without `DATA`, the sender's next sequence number |
| 3 | 1 | `ack`: next sequence number the sender expects from its peer |
| 4 | 1 | `len`: message length, 0–65 |
| 5 | 65 | message, zero-padded |
| 70 | 2 | CRC-16/CCITT-FALSE of bytes 0–69, little-endian |

A 65-byte message holds a type byte plus a full 64-byte HID report.

### 6.2 Reliability: go-back-N, window 4

Errors on 2 cm of board trace should be rare, but a lost key-up means a stuck key, so every message is delivered
exactly once and in order.
- The receiver accepts a `DATA` frame only if `seq == expected`; anything else is dropped. Every frame (data or idle)
  carries `ack = expected`.
- A frame sent in transaction *t* is acknowledged by the peer's frame in transaction *t + 1*, because each side builds
  its next frame between transactions. If that ack does not cover it, the sender goes back to the oldest unacknowledged
  message.
- **Restart:** a side that boots sends `SYN` until its first message is acknowledged. A receiver seeing `SYN` (after
  frames without it) re-synchronises `expected` to the frame's `seq`. Messages in flight across a restart are lost; both
  sides re-send `Hello`.
- **Hello:** the AT32F405 says `Hello` when it starts and in answer to the RP2354B's; the RP2354B says it whenever its
  link starts (boot, after programming) and never answers. With both answering, the two `Hello`s bounced back and
  forth for good, each one triggering another descriptor-set upload, and every upload dropped the PC's connection for
  50 ms: the AT32F405 never stayed on the bus long enough for its hub to see it (first board, 2026-11-11). A
  descriptor set identical to the committed one is therefore also ignored, not re-enumerated.
- **Priorities:** reports are dequeued before control traffic, so a descriptor-set upload never delays the mouse.

The RP2354B starts a transaction when it has something queued or unacknowledged, when `IRQ` is high, or every 5 ms
otherwise (keepalive, status). Implemented in `kvm-proto::link::Arq`, which both firmwares use, with corruption and
restart tests.

### 6.3 Messages

The first byte is the type.

| Type | Direction | Message | Body |
|---|---|---|---|
| 0x01 | both | `Hello` | protocol version, firmware version (u32), role (AT32F405 → RP2354B), flags (bit 0 `NO_CRYSTAL`; a `Hello` without the byte reads as 0) |
| 0x11–0x17 | RP → AT | `InReport` | report for IN endpoint 1–7 (type − 0x10) |
| 0x11–0x17 | AT → RP | `OutReport` | data received on OUT endpoint 1–7 |
| 0x20 / 0x21 / 0x22 | RP → AT | `DescBegin` / `DescData` / `DescCommit` | total length + CRC-32 / offset + up to 62 bytes / — |
| 0x30 | AT → RP | `CtrlRequest` | tag, setup packet, OUT data stage (fragmented) |
| 0x31 | RP → AT | `CtrlReply` | tag, status (ok / stall), IN data stage (fragmented) |
| 0x32 | AT → RP | `SetReport` | interface, report type, report ID, data (on `LOCAL_HID` interfaces) |
| 0x33 | AT → RP | `ProtocolChanged` | interface, 0 = boot / 1 = report |
| 0x40 | RP → AT | `Command` | attach, detach, remote wakeup, hub-reset pulse, request status |
| 0x50 | AT → RP | `Status` | §5.4 |

## 7. Descriptor sets: what each PC sees

### 7.1 Interfaces and endpoints

| Interface | Kind | Endpoint | Present |
|---|---|---|---|
| 0 | Boot keyboard (6KRO, boot subclass) | EP1 IN, 8 bytes | unless left out for mirrors (below) |
| 1 | Mouse (boot subclass): 16 buttons, 16-bit X/Y, wheel + pan with resolution multiplier; with `abs-pointer`, report ID 1 and an absolute pointer as ID 2 (§4.14) | EP2 IN | unless left out |
| 2 | NKRO keyboard (ID 1), consumer control (ID 2), system control (ID 3); with `gamepad hid` the virtual gamepad (ID 4: 32 buttons, a hat, 8 axes; §4.11) | EP3 IN | unless left out |
| 3 | KVM control (vendor page 0xFF4B, 64-byte in/out) | EP4 IN + EP4 OUT | always |
| 4 | Vial (raw HID, page 0xFF60, 32-byte reports; requests as SET_REPORT) | EP7 IN | `vial` on, or auto while an IN endpoint is spare (§4.11) |
| 4/5… | The virtual gamepad as an XInput controller (class 0xFF/0x5D/0x01, compatible ID `XUSB10`, §4.11) | EP6 IN + EP6 OUT | `gamepad xinput` |
| 4/5… | Mirrored interfaces of the front-port devices | EP5–EP7 IN (less Vial's EP7 and XInput's EP6), then EP1–EP3 IN while 0–2 are left out; EP5–EP7 (less XInput's EP6), then EP1–EP3 OUT | Mirrored / identity |

- Keys go out through **either** interface 0 or interface 2, never both: interface 0 when the PC has selected the boot
  protocol (BIOS), interface 2 otherwise. The OS merges both into one keyboard, so nothing doubles.
- PC-side polling: `bInterval` 1 (125 µs) or 4 (1 ms), from the config.
- **Room for mirrors** (`kvm-core::policy`): interfaces 0–2 are left out, and the mirrors also get EP1–EP3 IN, while
  the Keyboard and Mouse ports are both mirrored, no port runs Normalized, and the mirrors need more IN endpoints than
  EP5–EP7 (less Vial's) have (`wide`). The control interface keeps EP4 and comes first, unless identity puts the
  cloned interfaces before it. All ports are decided together (`decide_all`): ports already mirrored are served
  first and keep their endpoints; a port that doesn't fit runs Normalized if its inputs allow, otherwise
  Passthrough. Every attach, unplug and mode or Vial change decides them again, so freed endpoints go to a port that
  was left out.
- **Identity:** the cloned interfaces keep their original numbers and come first; the KVM interfaces follow. The device
  descriptor takes the source's VID/PID, `bcdDevice` and strings.
- **XInput mirror:** the vendor interface (class 0xFF/0x5D/0x01) with its class-specific descriptor, plus the
  Microsoft OS 1.0 compatible-ID descriptor `XUSB10`.
- Device VID:PID `1209:0001` (the pid.codes test PID) until a pid.codes PID is allocated (§12).

### 7.2 Format of a descriptor set (`kvm-proto::descset`)

```
"KDS1" | total_len u16 | record_count u8 | flags u8
record: tag u8 | arg u8 | len u16 | data[len]
```

| Tag | `arg` | Data |
|---|---|---|
| 0x01 `DEVICE` | 0 | device descriptor (18 bytes) |
| 0x02 `CONFIG` | 0 | full configuration descriptor, high-speed values |
| 0x03 `STRING` | index | string descriptor (0 = language IDs, 0xEE = Microsoft OS string) |
| 0x04 `HID_REPORT` | interface | HID report descriptor |
| 0x05 `IFACE` | interface | flags: `LOCAL_HID`, `FORWARD`, `BOOT_KEYBOARD`, `BOOT_MOUSE` |
| 0x06 `BOS` | 0 | BOS descriptor |
| 0x07 `MSOS20` | vendor code | Microsoft OS 2.0 descriptor set |
| 0x08 `MSOS10_COMPAT` | vendor code | Microsoft OS 1.0 extended compat-ID descriptor |

Limit 4 KB. The AT32F405 keeps the active set and a staging buffer, and checks the CRC-32 before it swaps.

## 8. Control protocol (odkvmctl ↔ RP2354B)

### 8.1 Transport

- Interface 3 of the always-present composite, vendor usage page 0xFF4B, usage 1. 64-byte input and output reports, no
  report IDs. No driver on Windows; one udev rule on Linux.
- Present on **both** PCs, including the inactive one. The RP2354B holds the only state and pushes events to both.
- **Discovery** is by usage page + usage, not VID/PID, so the control interface is found in identity mode too. The
  Linux udev rule matches the interface string `OpenDynKVM Control` for the same reason.
- **One version:** host and firmware talk only when their `PROTOCOL_VERSION`s are the same. The host reads `GetInfo`
  first (`Client::info_checked`); with another version it reads nothing else, every page of the window says so and
  leads to the Firmware page, whose update works (`GetInfo` and the `Fw*` requests are all it needs), and `odkvmctl`
  refuses every command but `update`. Messages decode strictly: there is no support for older or newer firmware.
- **Over Bluetooth** (protocol 15, §4.13 "Control over Bluetooth"): the same reports through the module's control
  service, for a computer paired as one of its hosts (`odkvmctl::ble`: BlueZ over D-Bus in pure Rust on Linux,
  WinRT's GATT client on Windows and CoreBluetooth on macOS through bluest; Web Bluetooth in a browser, §9 "On the
  web"). The system already holds the link (the KVM is its keyboard and mouse), so the host software finds the KVM among
  the connected devices with the service, and prefers USB where it has both (`transport::open_first`).
  - **Allowed by the key:** a host may control the KVM only once a tap on the KVM's key allowed it
    (`ConfigValue::BtControl`, off by default). Until then it gets `GetInfo` (to see that it speaks this protocol) and
    `AskControl`, and `NotAllowed` for everything else. `AskControl` from the host itself, or `BtControl` set on from a
    PC, arms the key as allowing software control again does (§8 Safety, 30 s, the key LED pulsing white); a tap
    grants exactly that host, never the locks a PC waits for at the same time or another host (`Busy`). Off is at once.
    A slot whose bond goes (forgotten, paired with something else, every pairing erased) loses it: any state without
    a bond clears it, so one lost on the way doesn't stay for the next host to pair into the slot. A factory reset
    clears it too.
  - **Not over Bluetooth:** firmware updates (`Fw*` but `FwStatus`: the module that carries the stream is what an
    update replaces, and a module that needs one is off the air), restarts into a bootloader (only USB brings the
    board back), and what streams to a PC or needs its cursor (`WatchControls`, `MonitorDevice`, `LearnHotkey`,
    `PointerAt`, `PlaceCursor`: `NotActive`; stopping them is no error). The software locks apply as from a PC.
  - **Who asks:** `Status.asking` names the host, a PC or the Bluetooth slot (`Host`, one byte as before: 0x11–0x14
    for the slots).

### 8.2 Messages

Report layout: `type u8 | request id u8 | payload (62 bytes)`. The response type is the request type | 0x80; errors
are type 0xFF with the request type and an error code (10 `NotAllowed`: a Bluetooth host that may not control the KVM
yet); events use request id 0.

| Request | Response |
|---|---|
| 0x01 `GetInfo` | protocol version (15), firmware versions (RP2354B, AT32F405 ×2), hardware version (u32, packed like the firmware's), serial, capability bits (`BLUETOOTH` when the module answers); then the Bluetooth module's firmware version |
| 0x02 `GetStatus` | active PC, **which PC is asking**, link state of each PC, power source, advertised current, budget, lock bits, uptime, link errors, self-test bits 0–15 (§4.10); then the Bluetooth host that gets input, the hosts ready and bonded (bit masks), seconds of pairing left, the host waiting for a passkey, the module's state, and self-test bits 16–31; then screen hop's state (off, on, or what it needs) and the pointer's place in the arrangement (two i16); then the supply judged from its measured 5 V (unknown, good, sags, low), its voltage now and lowest in the last minute, and what the front devices declare (§4.7); then the Bluetooth devices bonded and ready (bit masks), seconds of looking for devices left, how many found, how far pairing a device got, and the passkey to type on it |
| 0x03 `GetPort` | state, mode setting and effective mode, where its input goes now (a host, or none), Auto reason, VID:PID, speed, interface classes, report rate, faults |
| 0x04 `GetPortString` | manufacturer or product string of a port's device (UTF-8, ≤ 58 bytes) |
| 0x05 `SetActive` | PC1 / PC2 / toggle |
| 0x07 `GetConfig`, 0x08 `SetConfig` | key + value (§8.3) |
| 0x09 `SaveConfig`, 0x0A `FactoryReset`, 0x0B `Reboot`, 0x0C `Identify`, 0x0D `Redetect` | — |
| 0x0E `LearnHotkey` | start (1) or stop (0) learning a hotkey (§4.5); start answers `NotActive` from the inactive PC, `Busy` while another PC's session runs |
| 0x0F `GetDiag` | page n of the diagnostics (§8.4): page, page count, up to 9 items of id, grade, value |
| 0x10–0x13 `FwBegin` / `FwData` / `FwCommit` / `FwStatus` | §4.9 |
| 0x14 `GetBtHost` | a Bluetooth host slot (1–4): flags, interval, latency, RSSI, address (§4.13) |
| 0x15 `BtPair` / 0x16 `BtForget` / 0x17 `BtPasskey` | open or close pairing; forget a slot (0xFF = all); the passkey a pairing host shows |
| 0x18 `PointerAt` | where the asking PC's cursor is, in its own pixels (two i16), for screen hop's cursor sync (§4.14); `NotActive` from the service port |
| 0x19 `PlaceCursor` | the asking PC's host software places its cursor (0 off, 1 the screens its mapping reaches only relatively, 2 all) for `PLACE_LEASE_MS` from now, screen hop's "Placed by the PC" (§4.14); `NotActive` from the service port |
| 0x1A `GetBtDevice` | a Bluetooth device slot (1–4): flags, RSSI, battery, appearance, address, name (§4.13) |
| 0x1B `BtScan` / 0x1C `GetBtFound` | look for devices to pair for 60 s, or stop (host pairing closes); entry n of the devices found (address, RSSI, appearance, name), none past the last |
| 0x1D `BtPairDevice` / 0x1E `BtForgetDevice` | pair the device found at an address into a free slot (`Busy` while one pairs); forget a slot (0xFF = all) |
| 0x1F `GetDeviceMap` | a device, the index of its HID interface, an offset: up to 48 bytes of that interface's report descriptor (a Bluetooth device's report map, index 0), the interface's number, how many HID interfaces it has, and the length (0: none) |
| 0x21 `BtEraseAll` | every pairing the module holds, hosts and devices, erased, and the module restarted (`EraseAll`); a configuration change |
| 0x20 `GetModuleLog` | position: up to 48 bytes of the module's log the KVM keeps, from there, and the positions of the first byte kept and past the last (counted since the RP2354B started) |
| 0x22 `GetKeycodes` / 0x23 `SetKeycodes` | keymap 0–3, layer 0–7, first position (u16): up to 28 keycodes from there (§4.11); `Set` is a configuration change |
| 0x24 `GetAxisRules` / 0x25 `SetAxisRule` | keymap, layer, first input: up to 8 axis rules of 6 bytes; `Set`: one input's rule |
| 0x26 `WatchControls` | 0 stop, 1 watch, 2 watch and keep the controls from the hosts: `Control` and `Axis` events to the asking PC (§4.11) |
| 0x27 `GetControls` | a device (`Device` order): whether it has keys, its mouse buttons (u16), gamepad buttons (u32), mouse axes, gamepad axes and hat, and its identity (u32; 0 none) |
| 0x2A `GetMapEntry` / 0x2B `SetMapEntry` | kind (0 tap dance, 1 combo, 2 key override, 3 alternate repeat key), index: the entry in Vial's layout (10 bytes, 6 for a repeat key), shared by all keymaps |
| 0x2C `GetMacros` / 0x2D `SetMacros` | offset: up to 56 bytes of the macro buffer (2 KB, Vial's format) and its length |
| 0x2E `GetMapSetting` / 0x2F `SetMapSetting` | a QMK setting by Vial's id (qsid) and its value (u32) |
| 0x30 `MonitorDevice` | a device, on or off: its raw reports as `Report` events to the asking PC; as `WatchControls`: from the active PC, a configuration change, for 10 s unless asked again |
| 0x31 `GetOverride` / 0x32 `SetOverride` | a device and field override 0–7: interface, report ID, first bit, bits, flags (signed, inverted), target (gamepad button 0–31, or 0x40 + axis), or none; kept with the device's identity |
| 0x33 `GetOutputs` | a host: what it is offered and what it took, as bits (keyboard, mouse, media, pointer, gamepad, XInput, Vial, mirrors) |
| 0x34 `AskControl` | from a Bluetooth host over the module: may it control the KVM (§8.1 "Over Bluetooth")? `Ok` if it may, `NeedsKeyPress` while the KVM waits for its key; from anyone else `Unsupported` |
| 0x28 `GetCalibration` / 0x29 `SetCalibration` | a device and its gamepad axis 0–9: minimum, centre, maximum (i16) and deadzone (256ths); kept with the device's identity, `Set` is a configuration change |

| Event | Body |
|---|---|
| 0xC0 `Switched` | active PC, source (key / hotkey / software / boot / power / screen edge) |
| 0xC1 `PortChanged` | port |
| 0xC2 `Fault` | kind, port |
| 0xC3 `ConfigChanged` | key |
| 0xC4 `HotkeyLearned` | learned / timed out / cancelled, the hotkey (kind, modifiers, key, swallow); to the PC that asked only |
| 0xC5 `BtChanged` | a Bluetooth host slot changed, or 0: pairing opened, closed or wants a passkey |
| 0xC6 `BtActive` | input goes to Bluetooth host 1–4, or 0: back on a PC (`Switched` follows) |
| 0xC7 `PlaceCursor` | where to put the cursor, in the PC's own pixels (two i16); only to a PC holding a `PlaceCursor` lease, at most one a millisecond |
| 0xC8 `BtDeviceChanged` | a Bluetooth device slot changed, or 0: looking for devices started, found one, stopped, or a pairing moved on |
| 0xC9 `Control` | a control's position (u16, `kvm_map::pos`) and whether it went down; only while watching |
| 0xCA `Axis` | an axis input (0–13) and its value (absolute) or the motion since the last event (relative); only while watching |
| 0xCB `Report` | a piece of the monitored device's raw report: device, interface, offset, total length, up to 56 bytes (the report ID first when the interface has them); only while monitoring |

### 8.3 Config keys

| Key | Value |
|---|---|
| 0x0004 | double-tap window (ms) |
| 0x0005 / 0x0006 | what a tap / a long press on the KVM's key does: nothing, toggle PC (tap's default), next profile (long press's default), cycle the Other port's mode and routing (§4.6) |
| 0x0007 | how long the key is held for a long press: 800–3000 ms (default 1000) |
| 0x0010 | LED brightness |
| 0x0011 / 0x0012 | PC1 / PC2 colour |
| 0x0020 | PC-side polling: 125 µs or 1 ms |
| 0x0021 | VBUS power cycle when a passthrough port switches |
| 0x0030–0x0036 | mode of each device in the active profile (`Device` order: the Keyboard, Mouse and Other ports, Bluetooth devices 1–4): auto, normalized, mirrored, mirrored + identity, passthrough, off (a Bluetooth device: auto, normalized, off) |
| 0x0038–0x003E | routing of each device in the active profile: 0 follow, 1 + a host's index pinned to it (PC1, PC2, Bluetooth hosts 1–4) |
| 0x0040 | software switching allowed; from software only **off**: turning it on answers `NeedsKeyPress` and waits 30 s for a tap on the key (§4.6) |
| 0x0041 | software configuration allowed (likewise; asking for it back works while it is off) |
| 0x0050 | Vial interface: off / on / auto (§4.11) |
| 0x0052 | the virtual gamepad: off (default), HID, XInput (§4.11; changing it re-enumerates both PCs) |
| 0x0060 | auto-select: make the running PC active when the active one is switched off (default on, §4.12) |
| 0x0061 | standby-wake: in standby, keep keyboard and mouse powered so a key press wakes a sleeping PC (default on) |
| 0x0062 | standby-glow: in standby, the key LED glows instead of all LEDs going dark (default off) |
| 0x0070 | active profile, 0–7 and below the count |
| 0x0071 | profiles in use, 1–8 (default 2); fewer than the active one's number makes profile 0 active |
| 0x0072 | remember-profile: power up in the profile last active (default off: in profile 0, and a change of profile isn't saved) |
| 0x0080–0x0087 | every device's mode in profile 0–7 (7 bytes, as 0x0030–0x0036); defaults: 0 all Auto; 1 the ports Passthrough, the Bluetooth devices Auto; 2–7 all Auto |
| 0x0088–0x008F | name of profile 0–7, UTF-8, at most 12 bytes; defaults: work, gaming, profile 2 … profile 7 |
| 0x0090–0x0093 | name of Bluetooth host slot 1–4 (empty: "BT n") |
| 0x0098–0x009B | policy of Bluetooth host slot 1–4: stay connected (default), or connect only while selected |
| 0x009C–0x009F | Bluetooth host slot 1–4 may control the KVM over the module (default off; on only with a tap on the key, §8.1) |
| 0x00A0 | absolute pointer on the KVM's mouse interface (default off; changing it re-enumerates both PCs, §4.14) |
| 0x00A1 | screen hop's acceleration: flat, precision (default), adaptive |
| 0x00A2 | pointer speed, −10–10 (default 0) |
| 0x00A3 | edge resistance: how far the pointer is pushed against an edge to the other PC before it hops, 0–2000 logical pixels (default 100) |
| 0x00A4 | corners: how far from each end of such an edge it never hops, 0–500 (default 20) |
| 0x00A6 / 0x00A7 | how PC1 / PC2 maps the absolute pointer: across all its screens (default) or onto its main screen only |
| 0x00AC / 0x00AD | what PC1's / PC2's desktop coordinates count: physical pixels (default) or logical |
| 0x00B0–0x00CF | arrangement a's screen slot s at 0x00B0 + 8a + s: host (0 = free, 1 PC1, 2 PC2, 0x11–0x14 Bluetooth host 1–4; bit 7 off for hop, bit 6 fixed place), width, height, scaling %, `desk` x, y, `at` x, y (15 bytes). Arrangement 0's slots are protocol 8's one arrangement. In the arrangement's own item (§4.8), not the config item |
| 0x00D0–0x00D3 | name of arrangement 0–3, UTF-8, at most 12 bytes (empty: "Arrangement n"); in the arrangement's item |
| 0x00D8–0x00DB | how Bluetooth host 1–4 maps the absolute pointer, as 0x00A6 |
| 0x00DC–0x00DF | what Bluetooth host 1–4's desktop coordinates count, as 0x00AC |
| 0x00E0–0x00E3 | how screen hop moves Bluetooth host 1–4's pointer: its absolute pointer (default) or relative motion only |
| 0x00E8–0x00ED | the keyboard layout of PC1, PC2, Bluetooth hosts 1–4 (`kvm_hid::layouts::Layout`, 0 US by default): macro text is typed through it (§4.11) |
| 0x00F0–0x00FF | entry 0–15 of the hotkey list (§4.5): action (kind, argument: a host for select-host, a profile for select-profile; kind 0 = unused entry), then the hotkey: kind (none, double-tap, combo), modifiers, key usage, swallow |
| 0x0100–0x0107 | every device's routing in profile 0–7 (7 bytes, as 0x0038–0x003E; default all follow) |
| 0x0108–0x010F | the arrangement profile 0–7 uses: 0 none (default, screen hop off), n + 1 arrangement n |
| 0x0110–0x0117 | the keymap profile 0–7 uses: 0 none (default, the short path), n + 1 keymap n (§4.11) |
| 0x0118–0x011F | profile 0–7's fallback keymap, for a host that can't take the gamepad: 0 none (default, the profile's own), n + 1 keymap n |

0x0030–0x003E are the active profile's modes and routings: the config item (§4.8) doesn't keep them, the profiles'
are there.

### 8.4 Diagnostics

`GetDiag` returns the KVM's measurements, each graded by the firmware: good (green), warning (orange), bad (red), or
no grade (a count, a setting, or a part that is off). The firmware grades because only it knows the limits and the
context, and a host that doesn't know an item still shows its grade and number. Ids, names and units are in
`kvm_proto::ctrl::diag`, the limits and their sources in `kvm_core::diag`.

| Group | Items | Where from |
|---|---|---|
| System | uptime, why the RP2354B last reset (power-on, brown-out, supply glitch, watchdog, software …), its die temperature, the self-test (§4.10), engine outputs dropped, whether firmware updates over USB are on (the partition it runs from), or which check of the partition table turned them off and what it read | POWMAN `CHIP_RESET` at boot, ADC temperature sensor, engine, `update.rs` at boot (boot ROM `get_partition_table_info`, `flash_runtime_to_storage_addr`, `get_sys_info`) |
| Clocks | crystal, system and USB PLLs, `clk_sys`, `clk_usb`, `clk_peri`, `clk_adc`, ring oscillator; each AT32F405's clock (crystal or internal) | the RP2354B's frequency counter against its crystal, every 2 s (1 ms each, 2 kHz resolution); `Hello` |
| USB | Hub-0 on the root port (speed, enumerated), the D+/D− swap of errata E1 and whether it was found at run time, Hub-0 status changes the hub class couldn't handle (cleared by `host.rs`), front-port enumerations or probes that failed (retried twice: replugged on the data lines, then with VBUS off for 1 s), each front port's device speed; per PC: USB speed, link state, bus resets, SPI link errors (counted once the AT32F405 has answered) | `usbh.rs`, `host.rs`, engine, AT32F405 `Status` |
| Power | PWR VBUS and CC, each PC's VBUS and CC, the 3.3 V at each AT32F405, the front-port budget (an estimate), each port's power-switch faults, the lowest 5 V in the last minute (graded as the supply, §4.7), 5 V dips under 4.4 V, each PC's 5 V dropouts | ADCs of both chips |
| PC-side chips | each AT32F405's die temperature and firmware version | AT32F405 `Status`, `Hello` |
| Bluetooth | only with the module fitted (§4.13): its state (starting, up, being programmed, failed this boot, gave up), its firmware version, the UART exchanges that failed, the hosts connected, and the hosts whose settings it holds in RAM (its wear guard, only while there are any) | `bt.rs`, the module's `Hello`, engine |

The main limits:
- **Temperatures:** the RP2354B warns from 70 °C, is bad from 85 °C. The AT32F405 warns from 85 °C, is bad from 105 °C,
  since its sensor is off by up to 20 °C from chip to chip.
- **Clocks:** the USB clocks must be within ±0.25 % of 48 MHz (USB full speed). The system clock must be within 1 % of
  its setting, and at least 10 % above `clk_usb` (RP2350-E12).
- **VBUS:** good from 4.75 to 5.50 V. Orange from 4.40 V (the USB 2.0 minimum at a device), and up to 5.70 V. No grade
  under 0.8 V, where nothing supplies the port.
- **CC:** red above 2.3 V, which means 5 V on CC. Orange below 0.2 V with VBUS there: a cable without Rp.
- **3.3 V:** good within ±5 %, against a reference that is itself ±3.3 %.
- **PC USB speed:** orange at full speed, since 125 µs polling needs high speed.
- **Hub-0:** red as low speed (errata E1), or when missing outside deep standby.
- **Bluetooth module:** orange while it is programmed, and while it needs an update (as the self-test's `BT_FW`).

The firmware measures every 2 s, and the engine builds the list when asked. Readings not taken yet (before the first
measurement, a PC whose chip doesn't answer) are left out, so a host merges the pages by id.
- **CLI:** `odkvmctl diag` prints the list by group, with ✓ / ! / ✗ marks, and exits with 1 if anything is bad.
- **GUI:** the *Firmware* page shows it with coloured dots and reads it again every 2 s while shown. Hovering over a
  name gives its limits, and *Copy* puts the CLI's text on the clipboard.

## 9. odkvmctl

- **CLI:**
  - `odkvmctl status` (the default), `odkvmctl list` (control interfaces this computer sees), `odkvmctl watch` (events);
  - `odkvmctl switch pc1|pc2|toggle|next|host1…host4`;
  - `odkvmctl wireless` (the four host slots), `wireless pair [stop]`, `wireless passkey <digits>`,
    `wireless forget <slot|all>`, `wireless name <slot> [<name>]`, `wireless policy <slot> stay|on-select`,
    `wireless erase` (every pairing the module holds, hosts and devices, and a restart of the module), `wireless
    allow <slot>` (from a PC: that host may control the KVM over the module, once the key is tapped), `wireless
    disallow <slot>`, and on a host that reaches the KVM over the module `wireless allow` alone (asks for itself;
    every other command answers `NotAllowed` until then, saying so) (a KVM with the Bluetooth module, §4.13). The
    host list marks the hosts that may control the KVM, and those whose software is connected now. `odkvmctl list`
    shows the KVMs reached over the module too. The user-facing name is neutral until the product is listed with the Bluetooth SIG
    (design.md §16);
  - `odkvmctl device` (the four device slots, and while looking the devices found, numbered), `device scan [stop]`
    (look for 60 s and list the devices as they show up; Ctrl-C leaves the KVM looking), `device pair <number|address>`
    (follows the pairing to its end, with the passkey to type on a keyboard that asks), `device forget <slot|all>`,
    `device mode <slot> auto|normalized|off`, `device route <slot> follow|pc1|pc2|host1…host4`, `device map <device>`
    (a device by name, `keyboard` … `wireless4`, or a Bluetooth device's slot: every HID interface's descriptor the
    KVM holds, as hex, and what the parser makes of it, run on the PC: to send along when a device shows as not
    supported), `device log` (the module's recent log as the KVM keeps it: connections, pairing, why a
    device was let go) (§4.13);
  - `odkvmctl port <keyboard|mouse|other> [mode <…>|route <…>|redetect]`;
  - `odkvmctl profile` (list: each device's mode, and its pin where it has one), `odkvmctl profile
    next|<number>|<name>` (make it active), `odkvmctl profile <number|name> name <text>`, `odkvmctl profile
    <number|name> <device> <mode|-> [<routing>]` (a device: `keyboard`, `mouse`, `other`, `wireless1`–`4`; `-` keeps
    its mode); the count and the key's actions are settings: `config set profiles 3`, `config set key-long
    next-profile`; every device's settings also as `mode-<device>`, `route-<device>`, `profileN-modes` and
    `profileN-routes` (seven comma-separated, in that order);
  - `odkvmctl hotkey [list]`, `hotkey add <action> <hotkey>`, `hotkey set <n> <hotkey> [--action <action>]`,
    `hotkey remove <n>`: the hotkey list (§4.5), entries numbered 1–16, actions by their names (`select-pc1`,
    `next-profile`, …), hotkeys as `double:scrolllock`, `ctrl+alt+1`, `f13` or `none`; `learn` instead has the KVM learn
    it from the keyboard, warns of a plain key or one another entry has, and on a terminal asks before using it. As
    settings: `hotkey1`–`hotkey16`, `<action> <hotkey>` or `none`;
  - `odkvmctl keymap [show [<keymap>]]` (what each keymap, 0–3, does differently from a fresh one, layer by layer,
    and the profiles using it), `keymap set <keymap> <layer> <control> <keycode>`, `keymap axis <keymap> <layer>
    <input> <target> [--scale <x>] [--invert] [--deadzone <%>] [--curve linear|quadratic|cubic] [--param <n>]`,
    `keymap learn [<seconds>]` (the controls of the normalized devices as they are used, by the names `set` takes;
    from the active PC), `keymap use <profile> <keymap|none>` (§4.11). Controls are key names (`a`, `f13`,
    `lctrl`), `mouse1`–`mouse16`, `pad1`–`pad32`, `hat-up` / `-right` / `-down` / `-left`, and axis directions
    (`x-`, `wheel+`); keycodes are a key or `ctrl+c`, `mouse1`, `pad5`, `dpad-up`, `mo(1)`, `tg(2)`, `lt(1,space)`,
    `mt(shift,a)`, `none`, `trans`, or a number (`odkvmctl::keycodes`). As settings: `profileN-keymap` (`none`, 0–3)
    and `gamepad` (`off`, `hid`, `xinput`);
  - `odkvmctl keymap macro [<n> [<step>…]]` (steps as `text:hello`, `tap:ctrl+c`, `down:shift`, `up:shift`,
    `delay:100`, or `none`), `keymap dance [<n> <tap> [<hold> [<double> [<tap-hold>]]] [--term <ms>]]`, `keymap combo
    [<n> <keycodes,…> <output>]`, `keymap override [<n> <trigger> <replacement> [--mods …] [--negative …]
    [--suppress …] [--layers 0,1] [--off]]`, `keymap repeat [<n> <keycode> <alternate> [--mods …] [--off]]`, `keymap
    setting [<name> [<value>]]` (the QMK settings by name: `tapping-term`, `permissive-hold`, …), `keymap fallback
    <profile> <keymap|none>`: without arguments each lists what is set (§4.11);
  - `odkvmctl monitor <device> [<seconds>]` (the fields its descriptors declare, then its reports as they change, the
    changed bytes marked), `odkvmctl field <device> [list|set <n> <interface> <report ID> <bit> <bits> <target>
    [--signed] [--invert]|remove <n>]` (targets `pad1`–`pad32` or an axis: `x` … `accelerator`), `odkvmctl outputs`
    (what each host is offered and took), `odkvmctl layout [<host> [<layout>]]` (each host's keyboard layout: `us`,
    `gb`, `de`, …);
  - `odkvmctl calibrate <device> [show]`, `calibrate <device> measure [<seconds>]` (move every stick and trigger to
    its limits, then let go: the range seen and where it rests become the calibration), `calibrate <device> set
    <axis> <min> <centre> <max> [<deadzone>]`, `calibrate <device> reset`; a device is `keyboard`, `mouse`, `other` or
    `wireless1`–`4`, the calibration is kept with the device that is there (§4.11);
  - `odkvmctl config get [<name>]`, `config set <name> <value>`, `config save`, `config reset`. Turning
    `software-switching` or `software-config` off prints what stops working and, on a terminal, asks y/N; turning one
    on asks the KVM and waits for its key (`Client::allow_software`);
  - `odkvmctl diag`: the diagnostics, graded (§8.4);
  - `odkvmctl identify [<seconds>]`, `odkvmctl update [<file or folder>] [--module]` (default: the firmware shipped
    with the program; over a PC port to the KVM that answers there, the RP2354B's and then, once the KVM is back and
    its module asks for it, the module's; otherwise onto a board in its bootloader on PWR/SERVICE, §4.9 "Service
    port"; `odkvmctl list` shows those boards too; `--module`: only the module's, whatever it runs, §4.13),
    `odkvmctl reboot [normal|bootloader|at32-dfu1|at32-dfu2|at32-retry]` (`at32-retry`: up to 5 more programming
    attempts for the AT32F405s, §4.9).
    `odkvmctl status` names the Bluetooth module's firmware, or what it is doing instead, where one is fitted.
  - `odkvmctl display [list]`, `odkvmctl display input <id> <value>`: the monitors' inputs over DDC/CI (no KVM needed).
  - `odkvmctl screens [--arrangement <n|name>] [list]`: screen hop's state, the arrangements (those with screens or a
    profile using them, `*` the one in use, or the one named) slot by slot with the profiles using each, each host's
    mapping, units and (a Bluetooth host's) pointer, and the pointer settings, with a warning naming the arrangements
    where this computer's display settings no longer match the KVM's copy; `odkvmctl screens [--arrangement …]
    detect` brings this computer's screens up to date in that arrangement (the active profile's by default; as PC1 or
    PC2, whichever the KVM says this computer is: those already there keep their place and on/off, new ones go beside
    their neighbours, gone ones go) and sets its mapping and units (§4.14). `odkvmctl profile <which> arrangement
    <n|name|none>` picks a profile's arrangement, and `odkvmctl profile` lists them. The settings themselves are
    `config set abs-pointer`, `hop-accel`, `hop-speed`, `hop-resistance`, `hop-corner`, `hop-mapping-pc1/2`,
    `hop-mapping-host1`–`4`, `hop-units-pc1/2`, `hop-units-host1`–`4`, `host1`–`4-pointer` (`absolute`, `relative`),
    `profileN-arrangement` (`none`, 0–3; N 0–7), `arrangementN-name`, and `arrangementN-screenS` as
    `pc1 2560x1440 150% at 1920,0 desk 0,0` (`pc1`, `pc2` or `host1`–`host4`; scaling and positions may be left out;
    `no-hop` for a screen off for hop, `fixed` for a place on its computer that dragging doesn't change) or `off`.
    `odkvmctl status` has a screen-hop line.
  - `--sim` runs any command against a built-in simulated device (`OPENDYN_SIM_HOP=1`: with screen hop set up, two
    arrangements and, with `OPENDYN_SIM_BT=1`, the tablet of host 1 in both; `OPENDYN_SIM_PAD=1`, `receiver` or
    `symmetric`: a generic gamepad, a wireless receiver with a pad linked, or a symmetric-stick pad with a touch pad on
    the Other port instead of the webcam, Normalized, with the virtual gamepad on).
- **GUI** (`opendyn-kvm`), laid out after Creative's Sound Blaster app: a sidebar with the logo, coloured page icons
  and, at its foot, the quick controls (profile, PC1 / PC2 in the KVM's LED colours, LED brightness, dark / light), and
  the pages as rounded cards. The accent colour is the active PC's. Pages:
  - *Overview*: the two PCs (active one highlighted, which one this computer is, link and VBUS, switch button), the
    front ports in one line each, power, recent events;
  - *Ports*: a tab per profile in use (the active one bold in the accent colour), and in the tab: its name, its
    screen-hop arrangement, *Make active*, and a block per device: the three front ports and each Bluetooth device
    that holds a bond (a free slot is nothing to set). Its row has the device (its state, its product or name), its
    mode, where its input goes (following the active host, or pinned to PC1, PC2 or a paired wireless host: in
    passthrough the PCs only, and the routing is off for a device that is off) and re-detect; under the row, across
    the card, what the port cards up to 0.1.29 said: a front port's IDs and speed, the mode it runs in and why, its
    classes, report rate and power, and over-current faults; a wireless device's battery and signal. Below, how many
    profiles are in use and whether the KVM powers up in the last one;
  - *Displays*: monitors that follow the KVM, per profile (below);
  - *Screen hop* (the feature's name is a translation term, §4.14): whether it
    runs and, if not, what it needs, with a button for the absolute pointer or the Mouse port's mode; the profiles it
    is on in; the absolute pointer. The arrangement on a canvas: the screens to scale in their PC's colour (its main
    screen marked), the pointer live on them; a click selects a screen, a drag moves it, its edges snapping to the
    others' (the canvas keeps its scale during a drag, the grabbed point stays under the pointer); a warning where
    screens overlap. Every edit of the arrangement (a drag, a screen added, removed or edited, *Detect*, a PC's
    mapping or units) goes to a draft on the page, and to the KVM only with **Apply**, all of it at once; **Discard**
    drops it. After Apply the draft stays until the KVM's copy has it all (at least 2 s, since the window's own copy
    takes a setting as soon as it is sent), so a refused request keeps the edits for another try. (Before, each edit
    was sent at once: a detection half taken left the arrangement neither old nor new, 2026-11-12.) Where a screen
    sits next to its PC's other screens is where it is on that PC: a drop, an added or removed screen, or a new
    resolution or scaling gives that PC's screens their place on their computer (`desk`) from the arrangement
    (`odkvmctl::screens::desks`, the inverse of detection), except places that are fixed (detected or typed in). A
    screen off for hop is drawn dashed, on top of the screen it lies on; clicking again picks the screen underneath.
    (Before, a screen added by hand kept "right of the PC's others" whichever side it was dragged to, and the KVM moved
    that PC's cursor the wrong way: 2026-11-12.) On Windows, the window warns when this
    computer's mapping is set to all its screens, and the page warns when this computer's display settings no longer
    match the KVM's copy (read every 3 s). Add a screen per PC, remove one, *Detect this computer's screens*
    (`odkvmctl::screens`: Windows' monitor API with each monitor's DPI; on KDE Plasma under Wayland
    `kscreen-doctor -j`, in logical pixels, since `xrandr` there sees XWayland's coordinates scaled by the largest
    scaling; otherwise `xrandr --listmonitors`; merged into the arrangement, each new screen next to the neighbour it
    touches on its PC, since Windows' pixels are physical and the scalings differ), and the selected screen's PC,
    resolution (a list, or typed), scaling, place on its own PC, place here, and whether screen hop goes onto it. Each
    PC's mapping and units. The pointer: acceleration, speed, edge resistance, corners. On Windows and macOS the worker
    reports this computer's cursor to the KVM each time it rests (cursor sync, §4.14), and the page has *Place the
    cursor from this app…* with what the placer does now (placing, or why not: no KVM, firmware of another version, the sign-in or
    an administrator prompt, Windows refusing, macOS not allowing it yet with *Allow…*; §4.14 "Placed by the PC");
  - *Keyboard*: the hotkey list, one row per entry: what it does (host and profile actions by their names; the
    wireless ones with the module, a profile only while in use), its hotkey typed or learned (**Learn**, then the
    hotkey on the KVM's keyboard; it goes into the field with any warning, and **Apply** sets it; offered from the
    active PC with a keyboard the KVM reads), and × to remove it; **Add hotkey** puts a new entry after the last one,
    doing something no entry does yet. Then the double-tap window; what a tap and a long press on the KVM's key do, and the long
    press's duration: a slider whose handle sits on a bar from 0 to 3 s that shows the tap window and where the long
    press begins; the Vial interface, shown when `Info.caps` has `VIAL`;
  - *Keymaps* (`keymaps.rs`, with a KVM): the four keymaps and their eight layers, what a layer does as a picture of
    a full-size keyboard (and a row of the keys it doesn't draw), of a mouse with its side buttons (buttons 6–16 in a
    row) or of a gamepad (the standard one's 17 buttons, or the gamepad on the page's device with its own, its extra
    buttons named as the user likes, kept on this computer in `odkvmctl::local`), each control's legend coloured
    where it does something other than it says. While the page shows, the controls held light up live (from the
    active PC, renewed every 4 s: `WatchControls`). A control is chosen by clicking it, or with *Learn* by pressing it
    on the device (meanwhile the controls reach no host); then its action from a list grouped by kind (mouse,
    gamepad, layers, media and system), typed as on the CLI, or *Press what it should do*: the next control pressed
    gives its own keycode. The axis rules of the layer in a table, each axis's live value beside it. The profiles
    using the keymap (checkboxes), the virtual gamepad (off, HID, XInput), the gamepad drawn in its device's style
    (offset sticks and lettered buttons for an XInput pad, sticks side by side with shapes, Share, Options, Mute and
    a touch pad for a symmetric one, buttons by number for any other; *Drawn as* changes it, kept on this computer by
    the device's identity), and for a gamepad its calibration: a
    wizard (move every stick and trigger to its limits, then *Done*; it waits until all is let go to take the
    centre) and *Reset*. The keyboard picture in the shape of this computer's keyboard (ANSI, ISO, JIS, ABNT2; the
    layout's own unless chosen, kept on this computer) with the legends of the host this computer is (its keyboard
    layout). The action list has the KVM's actions too. The page's other sections (`keymaps/parts.rs`): *Macros* (one
    step per line, as on the CLI), *Tap dances*, *Combos*, *Key overrides*, *Repeat keys* (rows of keycode fields that
    turn red while they don't read as a keycode and go to the KVM when left; the entries in use and one more), *Timing
    and options* (the QMK settings), *Monitor* (a device's reports as grids of bits, the declared ones tinted and the
    changing ones lighting up; bits selected with a click and a Shift-click become a field override, the device's
    overrides listed with ×) and *Hosts* (each host's keyboard layout and its outputs, what it took in colour; each
    profile's fallback keymap);
  - *Settings*: this computer (start at login, appearance: system / dark / light, language), LEDs, PC side, software control,
    save and factory reset. Software control stays usable while configuration is locked: turning a lock off goes
    through an "are you sure" dialog with the warning, turning it on shows a countdown until the key is pressed;
  - *Firmware*: versions (the Bluetooth module's too, or what it is doing instead), the firmware file, update (a bar
    for the RP2354B, then one for the module once the KVM is back; once the KVM answers with the new firmware and its
    module is through, the page starts afresh: bars and results go, the diagnostics and the file are read anew, and
    one line says what it runs), identify, a card for a PC-side chip the KVM gave up
    programming (with *Try again*) or a module being programmed or needing an update (with *Update the wireless
    module*), and the diagnostics (§8.4) with a coloured dot per reading. The file starts as the
    firmware shipped with the app (`odkvmctl::firmware::bundled`: `firmware\` beside the programs, the packages'
    `/usr/share/opendyn-kvm/firmware`, `OPENDYN_FIRMWARE`, or a build tree's `dist/`); *Browse…* opens the desktop's
    file dialog (`rfd`: the XDG portal on Linux, no GTK; on a thread of its own). The page says the file's version and
    board, and whether it is newer, the same or older than the KVM's. It also works without a KVM on a PC port: a
    card per board in the boot ROM's bootloader (found every second, `worker::spawn_boot_scan`) says what it is and
    which UF2 it would get, with *Install* or *Set up* and *Restart without installing* (§4.9 "Service port"); the
    other pages point there while one waits. `OPENDYN_SIM_BOOT=kvm|new|other|unknown` shows one with the simulator;
  - *Wireless* (the Bluetooth feature, its user-facing name a translation term; only for a KVM with the module fitted,
    §4.13): pairing a host (with a countdown, and a passkey field while a host waits for one); pairing a device (*Look
    for devices* with a countdown, the devices found with their kind, signal and *Pair*, how far the pairing got, and
    the six digits to type on a keyboard that asks for them); the four device slots (name, kind, state, battery,
    forget; their modes and routings are on *Ports*); the four host slots: name, state, policy, *May control the KVM*
    (on only with a tap on the KVM's key, which the card then asks for; "its software is connected" while it has a
    session, §8.1 "Over Bluetooth"), forget; and, last, *Erase all pairings* (two presses: every host and device the module holds, its keys, and a restart). While the
    module is starting, being programmed or needs an update, the page says so instead (with *Update the wireless
    module* when it needs one).
  - *Allow this computer* instead of every KVM page, on a computer that reaches the KVM over the module and may not
    control it yet (`NotAllowed`): *Ask the KVM*, then the 30 s to tap its key. The Firmware page says that updates go
    over USB while the KVM is reached over the module.

  Bluetooth hosts are lesser targets and the window stays the two-PC window: without the module nothing changes. With
  it:
  - the paired hosts show as small violet buttons under PC1 | PC2 in the sidebar and as one compact row under the two
    PC cards on *Overview*, only while any host is paired;
  - while a host has the input, the accent is violet. *Overview* says that the ports in passthrough stay with the
    PC that was active (the home PC);
  - while a pairing host waits for its passkey, every page shows a banner with the passkey field and *Cancel
    pairing*. The KVM's keyboard types nowhere else until then, which would otherwise look like a stuck keyboard;
  - *Recent* names the hosts. The KVM's events only say which slot changed, so the worker tells what changed between
    two snapshots: paired, forgotten, connected, disconnected, switched to, pairing open / closed;
  - *Firmware* lists the module's version (from the diagnostics);
  - the *Keyboard* page shows the next-target hotkey, and the key actions offer `next-target`.

  *Displays* and *Settings → This computer* work without a KVM. Closing the window leaves the tray icon. The tray menu
  has *Switch to PC1/PC2*, then *Switch to* each paired Bluetooth host, *Open* and *Quit*. Its icon is the logo on a
  tile in the active target's colour: the active PC's, or violet while a host has the input (grey without a KVM, dark
  ink on a light colour). The tooltip names the active PC or host. On Windows the menu is rebuilt when the paired
  hosts change, with fixed item ids, so the one event handler keeps working. Look: `look.rs`; pages: `pages.rs`,
  `displays.rs`, `screens.rs`; icons: `odkvmctl::icon`, which draws the files in [`branding/`](branding/) with
  `resvg`, the bolder small mark up to 48 px.

  *Legibility.* egui's own text (Ubuntu Light, mid grey) is faint in the light theme. The text is set in Adwaita
  Sans at regular weight (`software/odkvmctl/fonts/AdwaitaSans-UI.ttf`, SIL OFL 1.1, made by `make_ui_font.py`: one
  weight, tabular digits so live values don't shift the text after them, Latin plus every character of the
  translations; egui's fonts still draw the icons and other scripts). Light-theme text is near black (36, 40, 46),
  notes dark grey (84, 91, 102), check and radio boxes a darker grey than buttons. Text in a status or PC colour
  goes through `look::ink`: darkened (light theme) or lightened (dark theme) in steps of 5 % just until it has a
  contrast of 4.5:1 (WCAG AA) on its card or chip; the colour itself still fills the LEDs, chips and bars. Tests
  check the contrast rule and that the font has every character of every language.
- **Local settings** (`odkvmctl::local`): what belongs to this computer and user rather than to the KVM: the theme, the
  language, the monitor bindings and whether the window places the cursor for screen hop (`place_cursor`, on by
  default), in `$XDG_CONFIG_HOME/opendyn-kvm/settings.json` (`~/.config` when unset or not absolute),
  `%APPDATA%\OpenDynKVM\settings.json` (roaming: a binding applies only to the monitor with its id) or
  `~/Library/Application Support/OpenDynKVM/settings.json` (macOS).
  Only the window writes it, as a setting changes: to a temporary file, flushed to disk, then renamed over the old one
  (on Windows tried again for up to half a second while a scanner holds the file), so a power cut leaves one whole
  file. Unknown fields are ignored, missing ones and unknown theme or monitor-mode names take the defaults; a file that
  still doesn't parse gives the defaults and is first copied to `settings.json.unreadable`. Start at login is kept
  where the system looks for it (below); the tests use temporary directories and a Run value of their own, never the
  user's settings.
- **Languages** (`odkvmctl::i18n`, `software/odkvmctl/i18n/<language>/odkvmctl.ftl`): every text of `odkvmctl` and
  `opendyn-kvm` (pages, tray, events, errors, the CLI's output and help) is a Project Fluent message, through
  `i18n-embed` and the `fl!` macro, which checks each message id and its arguments against the English file at compile
  time. English (the fallback), French, German, Norwegian Bokmål and Simplified Chinese (`zh`, which every Chinese
  system language gets: zh-CN, zh-Hans, zh-SG, also zh-TW); a message missing from a translation shows in English. The language follows the system (`LANG` on Linux, the user's languages on Windows), or the one chosen in
  Settings → This computer, which the CLI uses too. Decimals follow the language (`i18n::decimal`). clap's own words in
  the CLI's help (Usage, Options, …) stay English.
  - **Branding** is Fluent *terms* at the top of each file: `-product` (OpenDynKVM), `-wireless` (the Bluetooth
    feature, "Wireless" / "Funk" / "sans fil" / "trådløs" until the Bluetooth SIG listing exists, design.md §16) and
    `-host-pairing-settings` (where a phone or PC pairs a keyboard: the host's own "Bluetooth settings"). Each language
    gives a term the forms its grammar needs (cases, a compound prefix); renaming means changing those few lines.
  - **Tests:** every language has every English message with the same arguments, and each formats without errors
    (`i18n` tests). `OPENDYN_LANG=de` (fr, nb, zh) renders the screenshot test's pages in that language.
  - **Fonts:** the window's text font (Adwaita Sans, `fonts/make_ui_font.py`) is Latin only. Chinese comes from Noto
    Sans CJK SC at regular weight, cut down to the characters the Chinese file uses (`fonts/make_cjk_font.py`, about
    600 characters in 200 KB), which follows it in the font list. A test checks that the two fonts together have every
    character of every language: after a new Chinese message, run `make_cjk_font.py` again.
  - **Adding a language:** copy `en/odkvmctl.ftl` to `<code>/odkvmctl.ftl`, translate, add the code and the language's
    own name to `i18n::LANGUAGES`. Fluent message ids must not repeat a term's name (`product` vs `-product`): the
    bundle keeps only one of them.
- **Monitors follow the KVM** (`odkvmctl::ddc`): DDC/CI, MCCS VCP 0x60 (input source).
  - Packets: `mccs.rs` (get/set VCP, capabilities string, checksums, EDID for the monitor's id
    `<maker>-<product>-<serial>`), tested against known packets and a real capability string.
  - On Linux and macOS the program builds the packets and speaks to the display itself: `ddc::Channel` writes a
    request and reads the reply after 50 ms, and on top of it reads a VCP code (three tries, a busy display's null
    message tried again), sets one and reads the capability string fragment by fragment. A test drives it with a
    scripted display.
  - Linux (`ddc/linux.rs`): the connected connectors in `/sys/class/drm`, each with its EDID and its DDC bus (the
    DisplayPort AUX channel's `i2c-N` if there is one, else the connector's `ddc` link), `/dev/i2c-N` with
    `I2C_SLAVE` 0x37. Only address 0x37 is touched; never 0x50 (EDID comes from sysfs) or SMBus.
  - Windows (`ddc/windows.rs`): `EnumDisplayMonitors` → physical monitors (dxva2), `Get/SetVCPFeature` and the
    capability string; the id is the monitor's device interface path.
  - macOS (`ddc/macos.rs`, 0.1.28 on): Quartz's online displays other than built-in panels, each one's registry entry
    from the `IODisplayLocation` in CoreDisplay's info dictionary (AirPlay and Sidecar displays have none and are
    left out); the id from the dictionary's EDID as on Linux (else from Quartz's vendor, model and serial numbers).
    No rights or driver needed. The two private functions' families (`CoreDisplay_DisplayCreateInfoDictionary`,
    `IOAVService…`) are looked up at run time, so a macOS without them loses DDC/CI, not the program.
    - Apple silicon: the AV service proxy (`DCPAVServiceProxy`, `Location` "External") that follows the display's
      framebuffer in the registry (m1ddc's way), `IOAVServiceWriteI2C` / `ReadI2C` at chip address 0x37 with the
      source address 0x51 as sub-address; 0xB7 behind the MCDP29xx converter of some models' HDMI port. Each request
      is written twice, 10 ms apart, as m1ddc and MonitorControl do (a single write is sometimes lost).
    - Intel Macs: up from the display's entry to its `IOFramebuffer`, then its I²C buses (`IOFBCopyI2CInterfaceForBus`,
      `IOI2CSendRequest`) in turn: the write and the reply in one request, the DDC/CI reply transaction where the bus
      lists it, else a plain read; the driver's wait for the reply 30 ms on AMD's framebuffers (ddcctl's finding),
      none on the others. `IOI2CRequest`'s 64-bit layout (124 bytes, packed to 4) is checked at compile time.
  - Bindings: per monitor, an input for PC1 and PC2 (from the capability string, the current input, or typed in hex:
    how it is wired), and per KVM profile what it does: follow the active PC, stay on PC1, stay on PC2, or nothing
    (`local::DisplayMode`).
  - Optionally, an input per paired Bluetooth host (§4.13), for a laptop or tablet wired to the monitor as well
    (`bt_inputs`). It is keyed by the host's identity address, not its slot, so a host paired into a slot another
    one was forgotten from doesn't inherit its input. While a host has the input, a monitor that follows the KVM shows
    that host's input if it has one, and stays on the home PC's otherwise; pinned monitors ignore hosts.
  - The worker passes the KVM's state (`local::KvmState`: active PC, the host with the input, profile) to a follower
    thread whenever it changes: from `Event::Switched` and `Event::BtActive` straight away, and from each snapshot,
    which a `ConfigChanged` event (a change of profile) refreshes at once. The follower reloads the local settings and sets each monitor whose input differs between the
    old state and the new one, at most one change behind (newer states replace queued ones); the first state it hears
    of is only noted. So a monitor pinned to a PC isn't sent its input again on every switch. Failures show as events.
  - One PC with the software is enough when the monitor answers DDC/CI on an input it isn't showing, since the KVM
    sends its events to both PCs. Monitors that answer only on their shown input need the software on both PCs.
  - The bindings are per computer, and the two PCs don't coordinate: a monitor bound on both is sent every change
    twice (a monitor still switching may not answer the second, a few blank on re-selecting the shown input), and
    bindings that differ give a mix of both. So the README and the Displays page recommend setting each monitor up
    on one PC, and on both (the same way) only for a monitor that answers on its shown input alone. A fix would keep
    the bindings on the KVM (an EDID-based monitor id on Windows too) and let only the PC the monitor shows send,
    with a status bit for whether the other PC's software is running; not built.
- **Start in the tray, start at login:** `opendyn-kvm --tray` starts with the tray icon only (on Linux no window until
  *Open*; on Windows the window exists, hidden, because its event loop runs the tray icon). Settings → This computer →
  "Start with the system, in the tray" (`odkvmctl::autostart`) writes the `OpenDynKVM` value under
  `HKCU\…\CurrentVersion\Run` on Windows (and clears a "disabled" left by Task Manager) or
  `~/.config/autostart/opendyn-kvm.desktop` on Linux, both with `--tray`. It is a setting of this computer and user,
  not of the KVM, and is shown even without a KVM. Without a tray (GNOME without the AppIndicator extension) the window
  opens instead.
- **One copy per user** (`single.rs`): a second start asks the running copy to open its window, then exits; a second
  `--tray` start just exits. Linux: a Unix socket in `$XDG_RUNTIME_DIR`, which also carries the new start's XDG
  activation token so the window may come to the front on Wayland. Windows: a named event in the session, after
  `AllowSetForegroundWindow`.
- **Tray backends:** `tray-icon` on Windows. On Linux `ksni` talks StatusNotifierItem over D-Bus in pure Rust, so
  neither GTK nor libappindicator is needed (KDE, and GNOME with the AppIndicator extension).
- **Simulator** (`odkvmctl::sim`): runs `kvm-core::control` (the same request handler as the firmware) against fake
  devices. The GUI is developed and demonstrated with it before hardware exists. It presses the KVM's key when the KVM
  waits for it; its keyboard types what the GUI sends while learning a hotkey: the keys pressed in the window
  (`Transport::simulated_keyboard`), since this computer's keyboard isn't on the simulated KVM.
  - **Trying it without a KVM** (0.1.28): while no KVM answers, the window's pages offer *Try with a simulated KVM*,
    for someone who has none yet; `opendyn-kvm --sim` starts with it. `Shared::simulate` says which the worker talks
    to, and it changes over at once (`worker::spawn_with`). The simulator is made up; this computer's side is real: the
    monitors bound on the Displays page switch along (DDC/CI, the follower as with a KVM, this computer as the PC1 the
    simulator says it is), *Detect* on the Screen hop page reads this computer's screens, and this computer's settings
    are its own file (so monitors set up while trying stay set up for the KVM). The cursor placer talks only to a
    real KVM and stays out of it.
  - Every page then says so at the top (not in a browser, which has nothing else), with *Leave the simulator*; while
    simulating, the worker looks for a KVM's control interface every 2 s (`transport::find`, without opening it), and
    once one is plugged in the banner says so and offers *Use it*. Leaving puts the monitors that follow back on this
    computer's input. Nothing of it is saved: the next start looks for the KVM again. The window's title says
    "simulated" while it lasts. A window test clicks through it against a scripted no-KVM and the simulator.
- **On the web** (0.1.52): `opendyn-kvm` also builds for `wasm32-unknown-unknown`, the window in a page's
  `<canvas id="opendyn-kvm">` (`web.rs`: eframe's web runner, WebGL through glow). It starts with the simulator and,
  in a Chromium-based browser, reaches a real KVM: its control interface over **WebHID** from a PC connected to it, or
  its module's control service over **Web Bluetooth** from a computer paired as one of its hosts (`odkvmctl::webio`; a
  small JS snippet keeps each link's reports until they are read and sends one write at a time, which Web Bluetooth
  needs). A banner offers *Connect over USB…* and *Connect over the wireless link…* (the browser's chooser, which needs
  the click), and *Back to the simulator*; a KVM the page was given before is taken again at the start, without
  asking. A KVM that goes away leaves the simulator in its place, with a line saying so. No files (so no firmware
  update: the Firmware page says to use the app), tray, single copy or DDC/CI.
  - **Async:** WebHID and Web Bluetooth answer only once the page's event loop has run, so a request can't wait in a
    loop as on a computer. The client (`Client`), the transports and the worker's rounds (`run`, `command`, `refresh`,
    `take_events`) are written once with `maybe-async`: blocking on a computer (its `is_sync`), async in a browser,
    where the worker is a task on the page's event loop (`worker::spawn_web`) and waits are the browser's timers
    (`time::pause`). No lock on the shared state is held across a request.
  - **Stack:** the simulator's engine (about 220 KB) is on the heap (`SimTransport::engine`), and the browser's
    simulator is made once, outside the task, and parked while a real KVM is in use: copies of it on the 1 MB stack
    overflowed it.
  - The local settings live in the page's storage, and the page's address stands in for the environment
    (`?OPENDYN_SIM_BT=1&OPENDYN_SIM_HOP=1`, `OPENDYN_LANG=de`; `OPENDYN_SIM_BT=wireless` plays a Bluetooth host that
    must ask first, here and in the app). The product page (repository opendynkvm-web) builds it from here; nothing
    here depends on that repository.
- **macOS** (11 and later, 0.1.27 on; built and checked from Linux with `cargo clippy --target aarch64-apple-darwin`
  and a stand-in C compiler for hidapi's C, not yet run on a Mac): the control interface through hidapi's IOHIDManager
  backend; the screens from Quartz's display list in points (`Units::Logical`), each display's scaling from its mode's
  pixels per point, mirrors left out; the cursor from a Quartz event; placing it as above (§4.14); the tray icon in
  the menu bar (`tray-icon`, on the window's event-loop thread, so the window starts hidden in the tray and hides on
  close, as on Windows); start at login as a launch agent (`~/Library/LaunchAgents/org.opendynkvm.opendyn-kvm.plist`,
  `autostart::BUNDLE_ID`); the boot ROM's drive under `/Volumes`; the firmware in the app's `Contents/Resources`;
  DDC/CI as above (0.1.28).
- **Linux:** `software/odkvmctl/udev/70-opendyn-kvm.rules` gives the logged-in user access to the control interface's
  hidraw node and to the graphics cards' I2C buses (PCI class 0x03, as ddcutil's rule) for DDC/CI; the packages install
  it, reload udev, and load `i2c-dev` (`modules-load.d`, and `modprobe` on install).
- **Installers** (`cargo xtask package`, files in `software/odkvmctl/packaging/`):
  - **Windows:** an NSIS installer for the current user (no administrator rights) to `%LOCALAPPDATA%\Programs\OpenDynKVM`:
    both programs, the icon, the licenses, a Start menu entry, an Apps entry with uninstaller, and the "Start with
    Windows" Run value (ticked by default, the same value the GUI's setting writes). It closes a running copy first.
    Not code-signed yet.
  - **Linux:** a `.deb` (written by xtask itself) and an `.rpm` (`rpmbuild`) with both programs, the menu entry, icons
    from 16 to 256 px and a scalable one, the udev rule (reloaded on install and removal),
    `modules-load.d/opendyn-kvm.conf` (`i2c-dev`) and the licenses. The binaries are built in a Debian 12 container
    (`build-env.sh`, podman or docker, the host's rustup toolchain mounted), so they need glibc 2.35 or later: Debian
    12, Ubuntu 22.04 and later. Built on Fedora 44 directly they would need glibc 2.43.
  - **macOS** (`cargo xtask package --macos`, on a Mac with Xcode's command line tools and the Rust targets
    `aarch64-apple-darwin`, `x86_64-apple-darwin`): `OpenDynKVM-<version>.dmg` with `OpenDynKVM.app`, both programs
    as universal binaries (`lipo`), the firmware and licenses in `Contents/Resources`, the icon as `AppIcon.icns`
    (PNGs of 16 to 1024 px), signed ad hoc (`codesign -s -`), not by an Apple developer ID nor notarized: a downloaded
    copy opens with *Open* from its context menu the first time, and macOS asks for the Accessibility permission again
    after each update (it sees a new app). `odkvmctl` lies in `Contents/MacOS`, for a link from `/usr/local/bin`.
  - **Window icon on Linux:** on Wayland the desktop takes it from the menu entry named after the app id
    (`opendyn-kvm.desktop`), as winit 0.30 doesn't send the window's own; X11 and Windows use the icon the window sets.
    `cargo xtask desktop [--remove]` installs the entry (for `target/debug/opendyn-kvm`) and the icons into
    `~/.local/share` for a development build.
  - Each package carries `THIRD-PARTY-LICENSES.txt`: the license texts of every crate compiled in (from
    `cargo metadata`, each distinct text once), and RMK's notice for `kvm-map`.
  - Each package carries the release's firmware: `cargo xtask package` runs `dist` into the same folder first, and
    puts `kvm-rp2354.bin`, `kvm-rp2354.uf2` and `kvm-first.uf2` into `/usr/share/opendyn-kvm/firmware` (Linux) or
    `firmware\` beside the programs (Windows). The window and `odkvmctl update` install it by default.
  - Tested: the `.deb` installed, ran and was removed in a Debian 12 container; the Windows installer installed and
    uninstalled silently under Wine, with the files, shortcut and registry values checked; the registry part of
    `odkvmctl::autostart` ran under Wine.

## 10. Build and test

**Toolchain:** rustup's stable Rust (1.99 at the time of writing) with the targets `thumbv8m.main-none-eabihf`,
`thumbv7em-none-eabihf`, `riscv32imc-unknown-none-elf` (the Bluetooth module) and, for Windows builds from Linux,
`x86_64-pc-windows-gnu` (MinGW-w64 linker). The module's flash image needs `espflash` (`cargo install espflash`).
The macOS app builds on a Mac (`aarch64-apple-darwin`, `x86_64-apple-darwin`; hidapi's C needs Apple's SDK); from Linux
the host software is checked for macOS with `cargo clippy -p odkvmctl --target aarch64-apple-darwin --bins`, with
`CC_aarch64_apple_darwin` set to a script that only writes the empty object its `-o` (or `-Fo`) names: nothing links.
The same for `x86_64-apple-darwin` (`CC_x86_64_apple_darwin`) checks the Intel Macs' DDC/CI path, which the Apple
silicon build leaves out.
- Each firmware directory pins its target in `.cargo/config.toml` and lists it in `rust-toolchain.toml`.
- A distribution `rustc` without these targets (Fedora's, for example) won't build the firmware: put `~/.cargo/bin`
  first in `PATH`, or call `~/.cargo/bin/cargo`.
- `rustfmt.toml` at the root sets 120 columns for all three workspaces.

```sh
cargo test                                             # shared crates + odkvmctl + xtask
cargo xtask dist                                       # all three firmwares → dist/ (below; needs espflash)
cargo xtask package                                    # installers for odkvmctl / opendyn-kvm → dist/ (§9)
cargo run -p odkvmctl --bin odkvmctl -- --sim status   # CLI against the simulator
cargo run -p odkvmctl --bin opendyn-kvm -- --sim       # GUI against the simulator
cargo build -p odkvmctl --release --target x86_64-pc-windows-gnu
cargo xtask package --macos                            # on a Mac: OpenDynKVM-<version>.dmg → dist/ (§9)
(cd firmware/rp2354 && cargo build --release)          # flash: probe-rs (`cargo run`), or picotool over PWR/SERVICE
(cd firmware/at32f405 && cargo build --release)        # flash: probe-rs over SWD, or DFU (§4.9)
```

`cargo xtask dist` builds the AT32F405 firmware and the Bluetooth module's, then the RP2354B firmware with the
AT32F405's embedded and the module image's SHA-256, and writes to `dist/`:

| File | Use |
|---|---|
| `kvm-first.uf2` | blank board, or one with the partition table from before 0.1.51: copy onto the `RP2350` drive (PWR/SERVICE port); partition table + firmware in partition A, B's start erased |
| `kvm-rp2354.uf2` | board with the partition table: the ROM writes it into the partition that is not running |
| `kvm-rp2354.bin` | `odkvmctl update` |
| `kvm-module-esp32c3.bin`, `kvm-module-esp32.bin` | the Bluetooth module's firmware for each chip as the host software streams it (`kvm_proto::modimage`); `odkvmctl update` sends the module's after `kvm-rp2354.bin` |
| `kvm-at32f405.bin` | AT32F405 flash image for DFU or SWD (normally the RP2354B programs the AT32F405s itself) |
| `kvm-esp32c3.bin`, `kvm-esp32.bin` | the Bluetooth module's flash from offset 0 (on the bench: `espflash write-bin 0x0`) |
| `pt.json` | the same partition table for `picotool partition create` |

The partition-table encoder is tested against a block made by `picotool partition create` (embassy-rp's test vector,
hash included).

**Test layers:**
1. **Unit tests** for everything in `crates/`: frame codec, ARQ under random corruption and restarts, HID parser
   against real descriptors (boot keyboard, NKRO keyboards, gaming mice with 16-bit axes, gamepads), normalization,
   hotkey timing, switching, Auto policy, descriptor-set builder, control handler.
2. **Simulator** for `odkvmctl`: every GUI page is drawn in both themes against it in a test. The ignored test
   `screenshots` renders them to images with a small software rasteriser
   (`OPENDYN_SHOTS=<dir> cargo test -p odkvmctl --bin opendyn-kvm -- --ignored screenshots`). With
   `OPENDYN_SIM_BT=1` the simulated KVM has the Bluetooth module, and the test adds a host with the input, the passkey
   banner and pairing; with `OPENDYN_SIM_BT=gave-up` the module is fitted but programming it gave up (*Try again*
   acts out programming it).
3. **Prototype (M1)** on a Pico 2 + AT-START-F405, wired as on the board, before the PCB is ordered (design.md §12).
4. **Board bring-up (M3)** with the built-in self-test (design.md §10).

## 11. Milestones, fallbacks

| | Scope | Exit criterion |
|---|---|---|
| **M0** ✓ | This plan; shared crates with tests; `odkvmctl` CLI + GUI on the simulator; both firmwares compiling with board init, link, USB device core and host driver | `cargo test` green, three builds green (2026-10-21: 80 tests; RP2354B 255 KB flash / 61 KB RAM, AT32F405 37 KB / 29 KB) |
| **M1** | Pico 2 + AT-START-F405: host driver enumerates a hub + keyboard + mouse; AT32F405 enumerates at HS with a descriptor set; reports flow end to end | latency measured ≤ 150 µs worst case added |
| **M2** | Mirrored + identity against the makers' configuration software; XInput pad | per-device result table in design.md §12 |
| **M3** | Rev-A board bring-up: power path, routers, LEDs, key, self-test, passthrough | self-test green, `fio` passthrough run |
| **M4** | Firmware update (A/B, AT32F405 SPI flashing), config storage, WebHID page | update from both PCs, rollback test. The firmware side is written (2026-10-21); verify the FLASH_UPDATE and ISP details marked unverified in §4.9 on the M1 prototype first |

**Fallback gates (C/C++):**
- **RP2354B host driver:** if our driver can't pass M1 within about two weeks of bench time, link TinyUSB's host stack
  (`hcd_rp2040.c` + `usbh` + hub) as a static C library behind the same port-task interface (via `cc` + `bindgen`).
  Only the USB host layer changes; everything in `crates/` stays.
- **AT32F405 USB:** if `embassy-usb-synopsys-otg` misbehaves on Artery's core (it was written against ST's and
  Espressif's dwc2 revisions), use TinyUSB's `dwc2` device driver the same way. `kvm-usbdev`'s request
  handling is transport-independent and survives.

## 12. Open items

Not built yet:
- **Vial:** every feature of vial-qmk is in (§4.11) but RGB, encoders and the pre-0.19 tapping flags. Macro text is
  typed through the host's keyboard layout (13 of them, ASCII only, as Vial's text is). The whole path is tested
  against vial-gui's request sequences, not yet against the app on hardware (M2); the custom keycodes' names are
  QMK's keyboard range as vial-gui reads it, not yet seen in the app.
- **Source hi-res wheels:** the normalized mouse offers a resolution multiplier, but source mice are read in detent
  mode for now.
- **Mapping on hardware** (hid_mapping.md, milestones 1–6): everything is tested in the engine and the simulator, not
  yet with real devices and games: the virtual gamepad (HID, XInput, over Bluetooth), calibration, a touch pad made a
  control with field overrides, which hosts subscribe to the Bluetooth gamepad.

Needed from outside:

- **Artery ISP protocol** (SPI framing of the ROM bootloader): §4.9 implements ST's AN4286 framing on that
  assumption; Artery's note would confirm it before M1 instead of during.
- **USB PID:** request one from pid.codes (open-source hardware) to replace the test PID `1209:0001`.
- **Measured power budget** (design.md §12 item 5): the whole board under 1 W from the PWR port (2026-11-17), so
  `SELF_CURRENT_MA` is 200 mA; a reading with the front ports empty would narrow it. The warning itself is measured
  (§4.7).
- **WebHID page** (design.md §8) reuses `kvm-proto::ctrl` compiled to WebAssembly; after M4.
