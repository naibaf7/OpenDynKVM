# OpenDynKVM: firmware and host software (rev 2)

Implements the behaviour in [design.md](design.md) on the rev-5 board in [hardware/](hardware/). Everything here is Rust.
C/C++ is the fallback for two specific parts, each with a decision gate (§11).

## 1. Decisions

| Component | Runs on | Language / main crates | Notes |
|---|---|---|---|
| [`firmware/rp2354`](firmware/rp2354) | RP2354B | Rust, `embassy-rp` 0.10, `embassy-usb-host` 0.1 | Own driver for the RP2350's **native** USB host controller (§4.2). |
| [`firmware/at32f405`](firmware/at32f405) | 2 × AT32F405 | Rust, `at32f4xx-pac` 0.4, `embassy-executor`, `embassy-usb-synopsys-otg` 0.4 | The OTGHS core is a dwc2; the AT32-specific parts are clocks, PHY power-up and the SPI slave. |
| [`firmware/esp32c3`](firmware/esp32c3) | ESP32-C3-MINI-1 (boards 0.16.0 on, when fitted) | Rust, `esp-hal` 1.2, `esp-rtos` 0.4, `esp-radio` 1.0 (beta), `trouble-host` 0.8 | Bluetooth LE keyboard and mouse towards up to four hosts (§4.13). |
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
  kvm-vial/                Vial protocol, keymap and keymap engine (§4.11); parts adapted from RMK
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

- **The engine** (`kvm_core::Engine`, §4.4–4.5) is a plain struct behind a thread-mode mutex. It is about 125 KB, so
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
Empty ─attach─► Probing ─descriptors read─► decide mode ─┬─► Hosted (Normalized | Mirrored | Mirrored+identity)
  ▲                                                       └─► Passthrough(PC1 | PC2 | follow) ── router flips away
  └──────────── detach / over-current / re-detect ◄───────────────────────────────────┘
```

- **Probing** reads the device, configuration, string and HID report descriptors, and the Microsoft OS descriptors of
  an XInput pad. The result is a `DeviceSummary` (`kvm-core::policy`).
- **Auto policy** is design.md §4 verbatim, as a pure function with unit tests.
- **Profiles** (`kvm-core::config::Profile`, design.md §4): the mode settings in force are the active profile's, and
  the `PortMode` setting and `SetPortMode` change that profile. Any change of the settings in force (a port's mode, the
  active profile, its modes, a factory reset) decides all ports again in one step; the ports whose setting changed
  are reported with `PortChanged` even when their mode stays.
- **Passthrough:** the router (TS3USB3031) is flipped to Hub-1 or Hub-2 and the device re-enumerates on that PC.
  - On a PC switch, a port that follows the active PC is routed straight to the new PC (it re-enumerates there anyway).
  - The KVM can no longer see the device, so it doesn't notice an unplug. Swapping devices on a passthrough port
    therefore needs a re-probe: `odkvmctl port other redetect`, or a mode change.
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
3. B's stored keyboard **output report** (Num/Caps/Scroll LEDs) is sent to the physical keyboard.
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
- Bindings: toggle, select PC1, select PC2. Stored in the config (§8.3).
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
  mode in the active profile (Auto → Passthrough → pinned PC1 → pinned PC2 → Auto), or nothing. For 1.5 s after the
  active profile changes, the five front LEDs count its number in white from the left. While a firmware update waits
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
- Budget estimate = advertised current − 0.3 A for the electronics; it is shown in `odkvmctl` and turns the PC LEDs amber
  when below the sum of the attached devices' `bMaxPower`.

### 4.8 Config and keymap storage

The config, the keymap layers, the macros and the keymap settings are items in a **log-structured key-value store**
(`kvm-core::store`) on the **data partition** (68 KB = 17 sectors at 1976 KB, §4.9).

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
- **What is written, when:** the engine marks what changed (config, each of the 8 layers, macros, keymap settings) and
  emits `Save` 5 s after the last change, or at once on `odkvmctl config save`. The storage task then writes only the
  changed items (`Engine::take_save_item`). Switching PCs, Bluetooth hosts or screens never writes flash; changing
  profile does only with "power up in the profile last active". A change made while the storage task writes keeps its
  own autosave (`Engine::config_saved`).
- **Save pacing:** up to 30 saves go ahead as they come; after that at most one a minute, which is also how fast the
  allowance comes back (`SAVE_BURST`, `SAVE_PACE_MS`). Setting the KVM up never meets it. It holds a host script that
  changes a setting every few seconds, or saves in a loop, to 1 440 saves a day; a `SaveConfig` held back leaves
  `CONFIG_DIRTY` set until it is written.
- **Endurance** (W25Q16JV in the RP2354: 100 000 erase cycles per sector), measured by `endurance_with_the_firmware_items`
  with everything the firmware stores at its largest (8 layers, a full macro buffer and the records, 5.9 KB carried
  around the ring) and the config at its largest (576 bytes; 412 with the defaults): each sector is erased once per 92
  config saves, the 17 within one cycle of each other, so 9.3 million saves before the first sector reaches its rated
  cycles. Paced, that is at least 17 years of a script saving once a minute day and night; set up by hand, it doesn't
  wear out. The two alternating 4 KB slots used before erased a sector on every save: 200 000 saves.
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
| 8 KB | 984 KB | partition A |
| 992 KB | 984 KB | partition B, the A/B partner of A |
| 1976 KB | 68 KB | partition "data" (id 2): config and keymap store (§4.8). Not bootable, accepts no UF2 family |
| 2044 KB | 4 KB | free: the RP2350-E10 workaround block (picotool `--abs-block`, at 0x10FF_FF00) lands in the last sector |

- **The partition geometry is fixed once a board is flashed.** At boot `update.rs` checks the table against the
  constants in `flash.rs`. A firmware built with another geometry switches USB updates off on a board with the old
  table, and every board would need the pinhole + PWR/SERVICE + `kvm-first.uf2` procedure. What lives *inside* the
  data partition is the firmware's own and may change with any update (items carry versions).
- The image is 355 KB of the 984 KB (2026-10-29, with the 40 KB AT32F405 image and Vial).

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
- The same image without the TBYB flag (UF2 drag-and-drop, picotool, a debug probe) boots normally and needs no buy.
- Without the expected partition table (a board flashed by a debug probe), `Info.caps` leaves out `FW_UPDATE` and the
  engine refuses `FwBegin`.

**Service port** (`odkvmctl::bootsel`, `odkvmctl::firmware`): installing on a new board, or on one whose firmware
doesn't come up on the PC ports, from a computer on PWR/SERVICE. The firmware routes the RP2354B's USB to Hub-0
(`SVC_SEL` low), so that port shows the KVM only while the RP2354B is in the boot ROM's bootloader: blank, the pinhole
held while plugging in, or `odkvmctl reboot bootloader`. The ROM then shows its `RP2350` drive and its PICOBOOT
interface (2e8a:000f; RP2350 datasheet §5.5–5.6).
- **Which board:** PICOBOOT's GET_INFO `UF2_TARGET_PARTITION` (rp2350-arm-s) says where an image would go. The KVM's
  A (sectors 2–247) or B (248–493), from `kvm_proto::rpimage::layout`: a KVM, whose other partition holds the firmware
  in use (its IMAGE_DEF's `VERSION` item, read from the first 4 KB with READ). 0xFF (no partition table) and erased
  flash: a new board; no partition table but the KVM's `FwId` in the image at the start of flash: a board flashed by a
  debug probe, set up like a new one. Anything else (another table, other firmware) is not a KVM and isn't touched.
- **Installing:** the UF2 the board takes, copied onto the drive as dragging it would (`OPENDYN.UF2`, flushed in
  64 KB steps): a KVM gets the update UF2 (`kvm-rp2354.uf2`, or one made from the `.bin`), which the ROM writes into
  the partition not booted; a new board gets `kvm-first.uf2` (partition table and A). The ROM then does a flash update
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
| `BT_FW` | the Bluetooth module (when fitted, §4.13) doesn't run the embedded firmware this boot; tried again at the next power-up |
| `BT_GAVE_UP` | 5 attempts in a row at programming the Bluetooth module failed: no more until the image changes, the key is held at power-up, or `wireless-retry` |
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

### 4.11 Keymap and Vial (`kvm-vial`)

A QMK-style keymap for keyboards without Vial of their own, edited with the Vial app (design.md §4). It runs on the
RP2354B, which already parses the keyboard and owns the config storage; the AT32F405s only serve one more interface.

- **Matrix:** position = HID keyboard usage 0x00–0xE7, row = usage / 8, column = usage % 8 (29 × 8). One byte per
  row keeps Vial's matrix tester inside one 32-byte report. Usages outside the layout (keypad extras 0xA5–0xDF, and
  0xE8–0xFF) pass through unmapped.
- **Layout** (`build.rs`): full-size ANSI with ISO Enter and ISO left Shift as layout options, plus F13–F24, the
  international and language keys, Power, Mute and Volume. The JSON is xz-compressed as vial-qmk does it (616 bytes)
  and served in 32-byte blocks.
- **Engine** (`remap.rs`): runs on the merged keyboard state of Normalized ports, after the hotkeys (so no keymap can
  take the PC switch away) and before the output builders. 8 layers (MO, TG, TO, DF/PDF, LM, OSL, tri-layer), modded
  keys, mod-tap / layer-tap / TT / Space Cadet (tapping term, permissive hold, hold-on-other-key-press), one-shot
  modifiers, Grave Escape, media and system keys, mouse keys with QMK's acceleration, and 16 macros in Vial's format
  (text, taps, presses, releases, delays). Macro steps are at least 2 ms apart: the AT32F405 keeps 4 reports per
  endpoint. On a PC switch it lets go of everything held; toggled layers stay.
- **Protocol** (`protocol.rs`): VIA protocol 9 and Vial protocol 6 (keyboard id, definition, keymap get/set and
  buffers, layout options, macros, unlock, QMK settings 6, 7, 18, 22, 23, dynamic entries = none). Keycodes are stored
  exactly as written, so every keycode reads back unchanged. Command decoding and the keycode ranges are adapted from
  RMK (MIT, `crates/kvm-vial/LICENSE-RMK-MIT`); RMK's keymap engine is async and tied to its runtime, so it is not
  reused.
- **Interface:** usage page 0xFF60 / usage 0x61, 32-byte reports, IN endpoint only (EP7): the requests arrive as
  SET_REPORT on a `LOCAL_HID` interface, which the AT32F405 already forwards, so its firmware is unchanged. The serial
  number gets Vial's marker `vial:f64c2b3c` (also in identity mode, appended to the cloned serial).
- **Setting** `vial` (§8.3): `off` (default), `on` (it keeps EP7 IN, so mirrors get one IN endpoint less), `auto`
  (shown only while an IN endpoint is spare, so mirrors always come first). Changing it re-enumerates both PCs once, as a
  mirror attaching does anyway. `keymap` (default on) applies the keymap; a fresh keymap changes nothing.
- **Rules:** every write needs "configuration from software" (0x0041). Macros, the matrix tester and QK_BOOT also need
  Vial's unlock: both Shift keys held for 3 s, per PC. The matrix tester answers only the active PC. The bootloader
  jump is ignored.

### 4.12 Standby, wakeup and the active PC (`kvm-core::standby`)

What the KVM powers while no PC uses it, and which PC is active when PCs are switched on and off. Nothing of it is
saved: after a power loss the KVM finds the running PC again from the PCs' states, so no flash is written for it.

- **What a PC does** (`Presence`, from its AT32F405's `Status`, §5.4): *no 5 V* (unpowered); *no host* (5 V, but its
  USB never started: shut down with standby power, or early in its boot); *asleep* (bus suspended), with or without
  remote wakeup allowed; *up* (default, addressed or configured); *unknown* while its link is down.
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

### 4.13 Bluetooth hosts (boards 0.16.0 on, `bt.rs`, `espisp.rs`, `kvm-core::bt`, `firmware/esp32c3`)

Up to four extra hosts (PCs, tablets, phones) connect over Bluetooth LE to an ESP32-C3-MINI-1 on the board, which the
RP2354B treats as a third, lesser target: the hosts get the KVM's own keyboard, mouse and media reports, never a
mirrored device's descriptor, and the front ports in passthrough stay with the PC that was active before (the *home*
PC). Everything below is off on a board without the module, and on boards before 0.16.0: the control protocol then
reports no `BLUETOOTH` capability and the window shows the two PCs only (design.md §16).

**Wiring and presence** (hardware/tools/gen_schematic.py block 16). Two wires: UART0 TX (GPIO28) into the module's
RXD0, its TXD0 into UART0 RX (GPIO29), both on the module's ROM bootloader's own pins. The module's BOOT strap (IO9) is
on the same net as its RXD0, with a 10 kΩ pull-up, and its EN has only Espressif's 10 kΩ / 1 µF power-on delay.
- **Fitted or not:** `main` reads GPIO29 with the pad's pull-down for 20 ms before handing it to the UART: the
  module's TXD0 idles high, a bare pad (no module, or a board before 0.16.0) reads low. Without it the Bluetooth task
  stops at once and nothing is ever sent on GPIO28.
- **Why two wires:** the four free pins on the RP2354B's south side lie between its SWD pins and its crystal's way
  east, and in the first full routes of 0.16.0 the planned via fan-out of GPIO30/31 sat on Hub-0's Mouse pair while
  their lines cut the crystal's off (hardware/tools/route.py `FANOUT_SURFACE`).

**Link** (`kvm-proto::link::bt`). The AT32F405 links' 72-byte frames and go-back-N ARQ, over UART0 at 3 Mbaud
(0.24 ms a frame), the RP2354B as master: it sends a frame at least every 5 ms and the module answers each with one.
The RP2354B side runs on core 0 (`bt::run`); core 1 keeps to the AT32F405 links.

| Type | Direction | Message |
|---|---|---|
| 0x01 | both | `Hello` (the module's role is 3) |
| 0x60 | RP → BT | `Input`: host, report ID (1 keyboard, 2 mouse, 3 consumer), data |
| 0x61 | RP → BT | `Select`: the host that gets input, 0 = a PC |
| 0x62 | RP → BT | `Pair`: open or close pairing, seconds |
| 0x63 | RP → BT | `Forget`: a host's bond, 0xFF = all |
| 0x64 | RP → BT | `Passkey`: the six digits typed for a pairing host, or cancel |
| 0x65 | RP → BT | `Policy`: stay connected, or connect only while selected |
| 0x66 | RP → BT | `RequestState`: every slot's `Host` now |
| 0x67 | RP → BT | `EnterRom`: restart into the ROM bootloader |
| 0x70 | BT → RP | `Output`: a host's keyboard LEDs |
| 0x71 | BT → RP | `Host`: a slot's flags (bonded, connected, encrypted, subscribed), interval, latency, address, RSSI |
| 0x72 | BT → RP | `Event`: pairing open / closed, passkey wanted, paired, pairing failed, bond lost |

**Reports.** One HID service with a fixed report map (`kvm_proto::link::bt::report::MAP`, 161 bytes): keyboard (boot
layout, ID 1, with the LED output report), mouse (16 buttons, 16-bit X/Y, wheel, AC pan, ID 2), consumer (one 16-bit
usage, ID 3). Desktop and mobile systems take it without a driver: HID over GATT is built into them.

**Engine** (`kvm-core::bt`, `Engine::switch`):
- **Targets:** PC1, PC2 and the bonded hosts 1–4. `Target::Bt1`–`Bt4` select a host; `Toggle` from a host goes back
  to the home PC; `Next` (hotkey `hotkey-next`, or the key's `next-target` action) cycles PC1, PC2, then the hosts that
  are ready (connected, encrypted, subscribed).
- **Switching to a host:** as a PC switch: the PC left gets neutral reports, what is held stays hidden from the host
  until released, the keymap lets go. Then `Select`, and the keyboard's LEDs show the host's lock state. Leaving a host
  sends it every key and button released.
- **Reports:** with a host selected, the normalized path (keymap included) builds its reports instead of a PC's, and
  a mirrored device's input reaches it as the KVM's own keyboard and mouse. More than six keys give the boot layout's
  ErrorRollOver; system control (power, sleep) stays with the PCs.
- **Pairing:** `BtPair` opens it for 60 s (`BT_PAIR_SECONDS`). LE Secure Connections, IO capability KeyboardOnly: the
  host shows six digits, and while it waits the KVM's keyboard goes to nobody else: digits on the top row or keypad,
  Backspace, Enter sends them, Escape cancels. What is held when it ends stays hidden until released. `BtPasskey` does
  the same from software. No Just Works, so a host in range can't pair unnoticed.
- **Standby and auto-select:** a host in use, or pairing, keeps the KVM awake (its keyboard is on the front ports);
  auto-select never moves the input off a host because a PC was switched off. The LEDs: the key glows violet with a
  host selected (both PC LEDs as inactive), breathes violet while pairing is open, blinks violet while a passkey is
  typed.
- **Power budget:** `Status.budget_ma` takes `BT_CURRENT_MA` (30 mA) more off the source when the module is there.

**Module firmware** (`firmware/esp32c3`, Rust on `esp-hal` + `esp-rtos`; Bluetooth: `esp-radio`'s controller and the
`trouble-host` stack). Up to five connections (four slots plus one pairing). It advertises while pairing is open (then
discoverable) or while a host that should be connected isn't; an unknown host that connects outside pairing is
dropped. The selected host is asked for a 7.5–15 ms connection interval, the others for 30–50 ms with latency 4.
Transmit power is capped at +9 dBm (190 mA peaks, design.md §6.1). Bonds live in the last two flash sectors, which
the RP2354B never writes (A/B records with a CRC). A save erases one of them, so the module saves only when a slot
really changes (paired with new keys, forgotten here or by the host; forgetting several is one save): reconnecting
writes nothing, and 200 000 saves at the flash's rated 100 000 cycles last decades (`bonds.rs`, "Wear"). A forgotten
host's keys also leave the Bluetooth stack at once, so it can't come back in encrypted, without a passkey, while
pairing is open. IO3 carries a plain-text log (UART1, 115200) to test pad TP1601.

**Programming** (`espisp.rs`, like `at32isp.rs` for the AT32F405s). `cargo xtask dist` builds the module's firmware and
its flash image from offset 0 (bootloader, partition table, application, with espflash), and the RP2354B firmware
embeds it zlib-compressed (`build.rs`: 289 KB of the 685 KB image; its version from a `KVMESP32` marker, its MD5).
When the module's `Hello` carries another version, the RP2354B sends `EnterRom` and the module restarts into its ROM
bootloader; when it says nothing within 4 s (blank, or a firmware that doesn't come up and keeps resetting), the
RP2354B holds TX low for 3 s (a UART break) so that one of those resets starts the ROM. Then SYNC, 921 600 baud,
FLASH_DEFL_* in 1 KB blocks, SPI_FLASH_MD5 against the image's, and FLASH_BEGIN(0) + FLASH_END(reboot): with TX idle
the module boots the new firmware. A module found in its ROM after an unplanned reset (TX low at that moment) is
restarted the same way.
- **Limits:** as for the AT32F405s (§4.9, `kvm-core::at32prog`), with the module's own record in the store (item
  0x0003: CRC-32 of the embedded image's MD5, one count). Each attempt is counted in flash before it starts, and the
  count goes back to 0 when the module says `Hello` with the embedded version. Within one boot it tries again at once
  only if the transfer failed; a module that took its image but doesn't answer waits for the next power-up
  (`selftest::BT_FW`, Bluetooth off until then). After 5 failed attempts in a row it gives up (`BT_GAVE_UP`), until
  the image changes, the key is held at power-up, or `odkvmctl reboot wireless-retry` / the GUI's *Try again*.
- **State** (`bt::State`, `Status.bt_module`): not fitted, starting (waiting for its `Hello`), up, being programmed,
  failed this boot, gave up. `Info.bt_fw` is its firmware while it is up. Both are protocol 6; hosts hear of a
  change at once (`BtChanged(0)`).
- **Self-test and LEDs** (§4.10): `BT_FW` and `BT_GAVE_UP` show on the key in the self-test's LED code and keep it
  blinking red afterwards (`selftest::ON_KEY`). A module that is still starting or being programmed isn't a failure;
  the self-test's verdict waits for it until its 12 s deadline, and the bits follow the module after that.
- **Power:** the budget counts `BT_CURRENT_MA` whenever the module is fitted, also while it doesn't answer.

**Diagnostics** (§8.4): a *Bluetooth* group, only with the module fitted: its state (as `Status.bt_module`),
the version from its `Hello`, the UART exchanges that failed (`bt::LINK_ERRORS`) and the hosts connected.

**Unverified on hardware** (no module on the bench yet): the UART break as a way into the ROM of a blank module, the
ROM's FLASH_END reboot, multi-host HID over GATT with desktop, tablet and phone hosts, and the passkey flow. Host
side and engine are tested on the PC (`kvm-core` engine tests, the simulator with `OPENDYN_SIM_BT=1`).

### 4.14 Screen hop (`kvm-core::hop`)

The KVM moves the pointer itself, across an arrangement of both PCs' screens, and switches to the other PC when the
pointer is pushed across an edge onto one of its screens (design.md §4, "Screen hop"). It runs on the RP2354B, in the
engine, on the motion of the normalized pointing devices; the PCs get an absolute pointer.

- **Needs:** on in the active profile (`profileN-hop`); the absolute pointer (`abs-pointer`, below); the Mouse port's
  device running Normalized (an empty port is fine: any normalized pointing device moves the pointer); the active PC
  in report protocol and with a screen in the arrangement; no Bluetooth host active. `Status.hop` says which is
  missing, and the hosts offer the fix.
- **Absolute pointer:** with `abs-pointer` on, interface 1 carries report IDs: ID 1 is the relative mouse as before
  (the resolution-multiplier feature report with it), ID 2 an absolute pointer, a top-level collection of usage
  Pointer: 16 buttons, X/Y 0–32767, wheel and AC Pan in detents (`normalized::MOUSE_ABS_DESC`). A collection of its
  own and another usage than Mouse, so Linux makes it an input device of its own (an absolute pointer, as a virtual
  machine's tablet), and Windows a second mouse. Changing the setting re-enumerates both PCs once; a profile change
  never does, so a profile without hop (gaming) gets the relative mouse at once. In boot protocol everything is
  relative.
- **Arrangement:** 8 screen slots (`screen0`–`screen7`), each a PC's: resolution, display scaling (100–400 %), its
  top-left in that PC's own display arrangement (`desk`, in the PC's units, below), and its top-left in the
  arrangement (`at`, logical pixels: a screen at 150 % takes two thirds of its pixels each way, so screens of the same
  physical size line up). The pointer lives in the arrangement, in logical pixels with sub-pixel precision. Two flags
  per screen (protocol 8, in the slot's PC byte):
  - **Off for hop** (`off`): a screen of a monitor cabled to both PCs, on the PC it doesn't show. It lies where the
    other PC's screen is; the pointer never goes onto it, but it still counts for its PC's desktop (mapping, below).
    Measured on the first board's desk (2026-11-12): PC1 (KDE) had the shared 4K monitor left of its HD monitor, and
    with the HD monitor alone in the arrangement the cursor sat 2.33× too far along, on the 4K monitor's hidden area,
    before every hop.
  - **Fixed** (`fixed`): its `desk` is the PC's own (detected, or typed in); the app doesn't derive it from where the
    screen is dragged. The KVM only stores it.
- **Units** (`hop-units-pc1/-pc2`, protocol 8): what a PC's `desk` coordinates count. `physical` (Windows, X11): a
  screen covers its resolution there. `logical` (Wayland desktops: KDE Plasma, GNOME): it covers its arrangement
  size, its resolution over its scaling, and an absolute pointer is spread in those units.
- **Edges**, axis by axis: past a screen's edge, the next screen in that direction among those spanning the pointer's
  other coordinate (the nearest, however far: a gap is jumped; up to 8 pixels of overlap still count).
  - The same PC's: the pointer goes on, by what is left of the motion.
  - The other PC's: a wall until pushed `hop-resistance` pixels further (default 100; a pause of 400 ms starts the
    push over, a motion back from the edge too). Never within `hop-corner` pixels of either end of the current
    screen's edge (default 20, so corners stay easy to hit), never while a button is held (a drag stays on its PC),
    and only to a PC that is connected, awake and in report protocol. Then the PC left gets the pointer at its edge,
    released (a switch like any other, `SwitchSource::Edge`), and the other PC the pointer at the same place along
    its screen's edge.
  - Nothing there: a wall.
- **Mapping** (`hop-mapping-pc1/-pc2`): where a PC puts 0–32767. `desktop`: across the bounding box of all its
  screens' `desk` rectangles, those off for hop included (Linux: X11, GNOME, KDE). `main-screen`: on its main screen
  only, the one at `desk` 0,0 (Windows
  10 and later map an absolute HID mouse onto the primary monitor). Its other screens get relative motion instead: the
  KVM knows where it last put the cursor (an absolute report puts it exactly there) and sends the difference to where
  the pointer is, in the PC's pixels; back on the main screen, the next absolute report puts it exactly in place again.
  Windows accelerates that relative motion ("Enhance pointer precision"), so with it on the cursor drifts from the
  KVM's pointer on those screens until it rests (cursor sync, below) or returns to the main screen.
- **Two pointers, one button:** to a PC the relative and the absolute pointer are two devices, and Linux keeps a
  button down on the device it was pressed on. A button stays on the pointer it was pressed on and is released there
  (`PcOutput::held`); switching away releases both, the absolute one where it is.
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
- **Cursor sync** (`PointerAt`, protocol 7): a PC's host software may say where its own cursor is (its pixels).
  Taken over once the KVM hasn't moved the pointer for 150 ms: a touchpad or a program moved it, or Windows'
  acceleration on a screen reached relatively. `opendyn-kvm` sends it on Windows each time the cursor has rested for
  250 ms while hop is on in the active profile; on Linux the position isn't available to programs on Wayland (and
  isn't needed: the whole desktop is absolute).
- **Switching otherwise** (hotkey, key, software, auto-select): the pointer is found where it was left on that PC, or
  in the middle of its main screen.
- Costs: the engine grows by ~500 bytes (the pointer, the speed tracker), the config item by up to 194 bytes
  (`config::ITEM_MAX` 576). The arithmetic is single-precision float (the RP2354B's FPU); no `libm`.

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
| 2 | NKRO keyboard (ID 1), consumer control (ID 2), system control (ID 3) | EP3 IN | unless left out |
| 3 | KVM control (vendor page 0xFF4B, 64-byte in/out) | EP4 IN + EP4 OUT | always |
| 4 | Vial (raw HID, page 0xFF60, 32-byte reports; requests as SET_REPORT) | EP7 IN | `vial` on, or auto while an IN endpoint is spare (§4.11) |
| 4/5… | Mirrored interfaces of the front-port devices | EP5–EP7 IN (EP5–EP6 with Vial), then EP1–EP3 IN while 0–2 are left out; EP5–EP7, then EP1–EP3 OUT | Mirrored / identity |

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

### 8.2 Messages

Report layout: `type u8 | request id u8 | payload (62 bytes)`. The response type is the request type | 0x80; errors
are type 0xFF with the request type and an error code; events use request id 0.

| Request | Response |
|---|---|
| 0x01 `GetInfo` | protocol version (7), firmware versions (RP2354B, AT32F405 ×2), hardware version (u32, packed like the firmware's), serial, capability bits (`BLUETOOTH` when the module answers, `SCREEN_HOP` from protocol 7); then (protocol 6) the Bluetooth module's firmware version |
| 0x02 `GetStatus` | active PC, **which PC is asking**, link state of each PC, power source, advertised current, budget, lock bits, uptime, link errors, self-test bits 0–15 (§4.10); then (protocol 6) the Bluetooth host that gets input, the hosts ready and bonded (bit masks), seconds of pairing left, the host waiting for a passkey, the module's state, and self-test bits 16–31 (so older hosts read bits 0–15 where they were); then (protocol 7) screen hop's state (off, on, or what it needs) and the pointer's place in the arrangement (two i16) |
| 0x03 `GetPort` | state, mode setting and effective mode, Auto reason, VID:PID, speed, interface classes, report rate, faults |
| 0x04 `GetPortString` | manufacturer or product string of a port's device (UTF-8, ≤ 58 bytes) |
| 0x05 `SetActive` | PC1 / PC2 / toggle |
| 0x06 `SetPortMode` | Auto / Normalized / Mirrored / Mirrored + identity / Passthrough (follow / PC1 / PC2) |
| 0x07 `GetConfig`, 0x08 `SetConfig` | key + value (§8.3) |
| 0x09 `SaveConfig`, 0x0A `FactoryReset`, 0x0B `Reboot`, 0x0C `Identify`, 0x0D `Redetect` | — |
| 0x0E `LearnHotkey` | start (1) or stop (0) learning a hotkey (§4.5); start answers `NotActive` from the inactive PC, `Busy` while another PC's session runs |
| 0x0F `GetDiag` | page n of the diagnostics (§8.4): page, page count, up to 9 items of id, grade, value (protocol 5) |
| 0x10–0x13 `FwBegin` / `FwData` / `FwCommit` / `FwStatus` | §4.9 |
| 0x14 `GetBtHost` | a Bluetooth host slot (1–4): flags, interval, latency, RSSI, address (protocol 6, §4.13) |
| 0x15 `BtPair` / 0x16 `BtForget` / 0x17 `BtPasskey` | open or close pairing; forget a slot (0xFF = all); the passkey a pairing host shows |
| 0x18 `PointerAt` | where the asking PC's cursor is, in its own pixels (two i16), for screen hop's cursor sync (§4.14); `NotActive` from the service port (protocol 7) |

| Event | Body |
|---|---|
| 0xC0 `Switched` | active PC, source (key / hotkey / software / boot / power / screen edge) |
| 0xC1 `PortChanged` | port |
| 0xC2 `Fault` | kind, port |
| 0xC3 `ConfigChanged` | key |
| 0xC4 `HotkeyLearned` | learned / timed out / cancelled, the hotkey (kind, modifiers, key, swallow); to the PC that asked only |
| 0xC5 `BtChanged` | a Bluetooth host slot changed, or 0: pairing opened, closed or wants a passkey |
| 0xC6 `BtActive` | input goes to Bluetooth host 1–4, or 0: back on a PC (`Switched` follows) |

### 8.3 Config keys

| Key | Value |
|---|---|
| 0x0001–0x0003 | hotkey for toggle / PC1 / PC2: kind (none, double-tap, combo), modifiers, key usage, swallow |
| 0x0004 | double-tap window (ms) |
| 0x0005 / 0x0006 | what a tap / a long press on the KVM's key does: nothing, toggle PC (tap's default), next profile (long press's default), cycle the Other port's mode (§4.6) |
| 0x0007 | how long the key is held for a long press: 800–3000 ms (default 1000) |
| 0x0010 | LED brightness |
| 0x0011 / 0x0012 | PC1 / PC2 colour |
| 0x0020 | PC-side polling: 125 µs or 1 ms |
| 0x0021 | VBUS power cycle when a passthrough port switches |
| 0x0030–0x0032 | mode of the Keyboard / Mouse / Other port in the active profile |
| 0x0040 | software switching allowed; from software only **off**: turning it on answers `NeedsKeyPress` and waits 30 s for a tap on the key (§4.6) |
| 0x0041 | software configuration allowed (likewise; asking for it back works while it is off) |
| 0x0050 | Vial interface: off / on / auto (§4.11) |
| 0x0051 | apply the keymap to normalized keyboards |
| 0x0060 | auto-select: make the running PC active when the active one is switched off (default on, §4.12) |
| 0x0061 | standby-wake: in standby, keep keyboard and mouse powered so a key press wakes a sleeping PC (default on) |
| 0x0062 | standby-glow: in standby, the key LED glows instead of all LEDs going dark (default off) |
| 0x0070 | active profile, 0–3 and below the count |
| 0x0071 | profiles in use, 1–4 (default 2); fewer than the active one's number makes profile 0 active |
| 0x0072 | remember-profile: power up in the profile last active (default off: in profile 0, and a change of profile isn't saved) |
| 0x0080–0x0083 | port modes of profile 0–3 (Keyboard, Mouse, Other); defaults: 0 all Auto, 1 all Passthrough, 2 and 3 all Auto |
| 0x0088–0x008B | name of profile 0–3, UTF-8, at most 12 bytes; defaults: work, gaming, profile 2, profile 3 |
| 0x0008 | hotkey for the next target: PC1, PC2, the Bluetooth hosts that are ready |
| 0x0090–0x0093 | name of Bluetooth host slot 1–4 (empty: "BT n") |
| 0x0098–0x009B | policy of Bluetooth host slot 1–4: stay connected (default), or connect only while selected |
| 0x00A0 | absolute pointer on the KVM's mouse interface (default off; changing it re-enumerates both PCs, §4.14) |
| 0x00A1 | screen hop's acceleration: flat, precision (default), adaptive |
| 0x00A2 | pointer speed, −10–10 (default 0) |
| 0x00A3 | edge resistance: how far the pointer is pushed against an edge to the other PC before it hops, 0–2000 logical pixels (default 100) |
| 0x00A4 | corners: how far from each end of such an edge it never hops, 0–500 (default 20) |
| 0x00A6 / 0x00A7 | how PC1 / PC2 maps the absolute pointer: across all its screens (default) or onto its main screen only |
| 0x00A8–0x00AB | screen hop on in profile 0–3 (default off) |
| 0x00AC / 0x00AD | what PC1's / PC2's desktop coordinates count: physical pixels (default) or logical (protocol 8) |
| 0x00B0–0x00B7 | screen slot 0–7: PC (0 = free; bit 7 off for hop, bit 6 fixed place, protocol 8), width, height, scaling %, `desk` x, y, `at` x, y (15 bytes) |

The config item (§4.8) keeps 0x0030–0x0032 too, so firmware from before profiles still finds the active profile's
modes, and a config saved by it loads its modes into profile 0. Hosts skip keys an older firmware refuses.

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
| Power | PWR VBUS and CC, each PC's VBUS and CC, the 3.3 V at each AT32F405, the front-port budget, each port's power-switch faults | ADCs of both chips |
| PC-side chips | each AT32F405's die temperature and firmware version | AT32F405 `Status`, `Hello` |
| Bluetooth | only with the module fitted (§4.13): its state (starting, up, being programmed, failed this boot, gave up), its firmware version, the UART exchanges that failed, the hosts connected | `bt.rs`, the module's `Hello`, engine |

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
- **Bluetooth module:** orange while it is programmed, red when it doesn't run its firmware this boot or programming
  gave up (as the self-test's `BT_FW` / `BT_GAVE_UP`).

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
    `wireless forget <slot|all>`, `wireless name <slot> [<name>]`, `wireless policy <slot> stay|on-select` (a KVM with
    the Bluetooth module, §4.13). The user-facing name is neutral until the product is listed with the Bluetooth SIG
    (design.md §16); the earlier names `bt`, `bt1`…`bt4`, `bt-retry` and the settings `bt1-name`… still work;
  - `odkvmctl port <keyboard|mouse|other> [mode <…>|redetect]`;
  - `odkvmctl profile` (list), `odkvmctl profile next|<number>|<name>` (make it active),
    `odkvmctl profile <number|name> name <text>`, `odkvmctl profile <number|name> <keyboard|mouse|other> <mode>`;
    the count and the key's actions are settings: `config set profiles 3`, `config set key-long next-profile`;
  - `odkvmctl hotkey <toggle|pc1|pc2> [<spec>|learn]` (e.g. `double:scrolllock`, `ctrl+alt+1`, `f13`); `learn` has
    the KVM learn it from the keyboard (§4.5), warns of a plain key or one another hotkey has, and on a terminal asks
    before using it;
  - `odkvmctl config get [<name>]`, `config set <name> <value>`, `config save`, `config reset`. Turning
    `software-switching` or `software-config` off prints what stops working and, on a terminal, asks y/N; turning one
    on asks the KVM and waits for its key (`Client::allow_software`);
  - `odkvmctl diag`: the diagnostics, graded (§8.4);
  - `odkvmctl identify [<seconds>]`, `odkvmctl update [<file or folder>]` (default: the firmware shipped with the
    program; over a PC port to the KVM that answers there, otherwise onto a board in its bootloader on PWR/SERVICE,
    §4.9 "Service port"; `odkvmctl list` shows those boards too),
    `odkvmctl reboot [normal|bootloader|at32-dfu1|at32-dfu2|at32-retry|wireless-retry]` (`at32-retry`: up to 5 more
    programming attempts for the AT32F405s, §4.9; `wireless-retry`: the same for the Bluetooth module, §4.13).
    `odkvmctl status` names the Bluetooth module's firmware, or what it is doing instead, where one is fitted.
  - `odkvmctl display [list]`, `odkvmctl display input <id> <value>`: the monitors' inputs over DDC/CI (no KVM needed).
  - `odkvmctl screens [list]`: screen hop's state, the arrangement slot by slot, each PC's mapping and the pointer
    settings, with a warning when this computer's display settings no longer match the KVM's copy; `odkvmctl screens
    detect` brings this computer's screens up to date (as PC1 or PC2, whichever the KVM says this computer is: those
    already there keep their place and on/off, new ones go beside their neighbours, gone ones go) and sets its mapping
    and units (§4.14). The settings themselves are `config set abs-pointer`, `hop-accel`, `hop-speed`,
    `hop-resistance`, `hop-corner`, `hop-mapping-pc1/2`, `hop-units-pc1/2`, `profileN-hop`, and `screenN` as
    `pc1 2560x1440 150% at 1920,0 desk 0,0` (scaling and positions may be left out; `no-hop` for a screen off for hop,
    `fixed` for a place on its computer that dragging doesn't change) or `off`. `odkvmctl status` has a screen-hop
    line.
  - `--sim` runs any command against a built-in simulated device (`OPENDYN_SIM_HOP=1`: with screen hop set up).
- **GUI** (`opendyn-kvm`), laid out after Creative's Sound Blaster app: a sidebar with the logo, coloured page icons
  and, at its foot, the quick controls (profile, PC1 / PC2 in the KVM's LED colours, LED brightness, dark / light), and
  the pages as rounded cards. The accent colour is the active PC's. Pages:
  - *Overview*: the two PCs (active one highlighted, which one this computer is, link and VBUS, switch button), the
    front ports in one line each, power, recent events;
  - *Ports*: the profiles (active one, names, each one's port modes and screen-hop switch, how many are in use, whether
    the KVM powers up in the last one), then per port device, state, mode selector (the active profile's), report
    rate, faults, re-detect;
  - *Displays*: monitors that follow the KVM, per profile (below);
  - *Screen hop* (the feature's name is a translation term; for firmware with `caps::SCREEN_HOP`, §4.14): whether it
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
    PC's mapping and units. The pointer: acceleration, speed, edge resistance, corners. On Windows the worker reports this computer's
    cursor to the KVM each time it rests (cursor sync, §4.14);
  - *Keyboard*: hotkeys, typed or learned (**Learn**, then the hotkey on the KVM's keyboard; it goes into the field
    with any warning, and **Apply** sets it; offered from the active PC with a keyboard the KVM reads, on firmware with
    protocol 4), and the double-tap window; what a tap and a long press on the KVM's key do, and the long
    press's duration: a slider whose handle sits on a bar from 0 to 3 s that shows the tap window and where the long
    press begins; the Vial interface and keymap, shown when `Info.caps` has `VIAL`;
  - *Settings*: this computer (start at login, appearance: system / dark / light, language), LEDs, PC side, software control,
    save and factory reset. Software control stays usable while configuration is locked: turning a lock off goes
    through an "are you sure" dialog with the warning, turning it on shows a countdown until the key is pressed;
  - *Firmware*: versions (the Bluetooth module's too, or what it is doing instead), the firmware file, update,
    identify, a card for a PC-side chip or a Bluetooth module the KVM gave up programming (with *Try again*) or a
    module being programmed, and the diagnostics (§8.4) with a coloured dot per reading. The file starts as the
    firmware shipped with the app (`odkvmctl::firmware::bundled`: `firmware\` beside the programs, the packages'
    `/usr/share/opendyn-kvm/firmware`, `OPENDYN_FIRMWARE`, or a build tree's `dist/`); *Browse…* opens the desktop's
    file dialog (`rfd`: the XDG portal on Linux, no GTK; on a thread of its own). The page says the file's version and
    board, and whether it is newer, the same or older than the KVM's. It also works without a KVM on a PC port: a
    card per board in the boot ROM's bootloader (found every second, `worker::spawn_boot_scan`) says what it is and
    which UF2 it would get, with *Install* or *Set up* and *Restart without installing* (§4.9 "Service port"); the
    other pages point there while one waits. `OPENDYN_SIM_BOOT=kvm|new|other|unknown` shows one with the simulator;
  - *Wireless* (the Bluetooth feature, its user-facing name a translation term; only for a KVM with the module fitted,
    §4.13): pairing (with a countdown, and a passkey field while a
    host waits for one), and the four host slots: name, state, policy, forget. While the module is starting, being
    programmed or failed, the page says so instead (with *Try again* after 5 failed attempts).

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
  language and the monitor bindings, in `$XDG_CONFIG_HOME/opendyn-kvm/settings.json` (`~/.config` when unset or not
  absolute) or `%APPDATA%\OpenDynKVM\settings.json` (roaming: a binding applies only to the monitor with its id).
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
  - Linux (`ddc/linux.rs`): the connected connectors in `/sys/class/drm`, each with its EDID and its DDC bus (the
    DisplayPort AUX channel's `i2c-N` if there is one, else the connector's `ddc` link), `/dev/i2c-N` with
    `I2C_SLAVE` 0x37. Only address 0x37 is touched; never 0x50 (EDID comes from sysfs) or SMBus.
  - Windows (`ddc/windows.rs`): `EnumDisplayMonitors` → physical monitors (dxva2), `Get/SetVCPFeature` and the
    capability string; the id is the monitor's device interface path.
  - Bindings: per monitor, an input for PC1 and PC2 (from the capability string, the current input, or typed in hex:
    how it is wired), and per KVM profile what it does: follow the active PC, stay on PC1, stay on PC2, or nothing
    (`local::DisplayMode`). Files from before profiles have `enabled` instead, which becomes follow (or nothing) in
    every profile; it is still written, as profile 0's, for older versions.
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
  - **Window icon on Linux:** on Wayland the desktop takes it from the menu entry named after the app id
    (`opendyn-kvm.desktop`), as winit 0.30 doesn't send the window's own; X11 and Windows use the icon the window sets.
    `cargo xtask desktop [--remove]` installs the entry (for `target/debug/opendyn-kvm`) and the icons into
    `~/.local/share` for a development build.
  - Each package carries `THIRD-PARTY-LICENSES.txt`: the license texts of every crate compiled in (from
    `cargo metadata`, each distinct text once), and RMK's notice for `kvm-vial`.
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
(cd firmware/rp2354 && cargo build --release)          # flash: probe-rs (`cargo run`), or picotool over PWR/SERVICE
(cd firmware/at32f405 && cargo build --release)        # flash: probe-rs over SWD, or DFU (§4.9)
```

`cargo xtask dist` builds the AT32F405 firmware, then the RP2354B firmware with it embedded, and writes to `dist/`:

| File | Use |
|---|---|
| `kvm-first.uf2` | blank board: copy onto the `RP2350` drive (PWR/SERVICE port); partition table + firmware in partition A |
| `kvm-rp2354.uf2` | board with the partition table: the ROM writes it into the partition that is not running |
| `kvm-rp2354.bin` | `odkvmctl update` |
| `kvm-at32f405.bin` | AT32F405 flash image for DFU or SWD (normally the RP2354B programs the AT32F405s itself) |
| `kvm-esp32c3.bin` | the Bluetooth module's flash from offset 0, as the RP2354B writes it (on the bench: `espflash write-bin 0x0`) |
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
- **Vial:** tap dance, combos, key overrides, caps word and the mouse-key settings report zero entries, so the Vial app
  hides them (§4.11). The whole path is tested against vial-gui's request sequence, not yet against the app on
  hardware (M2).
- **Source hi-res wheels:** the normalized mouse offers a resolution multiplier, but source mice are read in detent
  mode for now.

Needed from outside:

- **Artery ISP protocol** (SPI framing of the ROM bootloader): §4.9 implements ST's AN4286 framing on that
  assumption; Artery's note would confirm it before M1 instead of during.
- **USB PID:** request one from pid.codes (open-source hardware) to replace the test PID `1209:0001`.
- **Measured power budget** (design.md §12 item 5) feeds the budget estimate in §4.7.
- **WebHID page** (design.md §8) reuses `kvm-proto::ctrl` compiled to WebAssembly; after M4.
