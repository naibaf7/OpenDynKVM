//! Bluetooth hosts as targets (software.md §4.13, design.md §13): the state the engine keeps of the Bluetooth module
//! (boards 0.16.0 on, when fitted) and what it sends it.
//!
//! The module holds up to four bonded hosts. The engine decides which target gets input: a PC (through its AT32F405,
//! as before) or one Bluetooth host. A Bluetooth host is a lesser target: it gets the KVM's own keyboard, mouse and media
//! reports ([`kvm_proto::link::bt::report`]), never a mirrored device's descriptor, and the front ports in passthrough
//! stay with the PC that was active before (the *home* PC, `Engine::active`).
//!
//! The module can also be an ESP32 board on a front port (a *dongle*, for a KVM without the module on board): an
//! ESP32-C3 with the same firmware, reached over the chip's built-in USB serial port instead of the board's UART, or
//! an original ESP32 (dual mode: Bluetooth Classic too, `firmware/esp32`) behind its board's CP210x bridge
//! ([`dongle_endpoints`]). The link and its messages are the same; the module says its chip in its `Hello`.
//!
//! The module is also the central of up to four Bluetooth devices (keyboards, mice), in device slots of their own
//! ([`Devices`]): their reports come to the engine with their report map, and it treats them as it does a normalized
//! front-port device.

use heapless::Vec;
use kvm_hid::translate::ConsumerState;
use kvm_hid::{KeyState, Layout, Translator};
use kvm_proto::crc::crc32;
use kvm_proto::ctrl::{
    BT_DEVICES, BT_HOSTS, BtDeviceInfo, BtDeviceName, BtFound, BtHostInfo, BtPolicy, DevicePairing, bt_device_name,
};
use kvm_proto::link::bt::{DeviceState, EventKind, HostState, MAP_MAX, report};

use crate::usbdesc::{DeviceDesc, class, parse_config};

/// What the engine asks of the Bluetooth module (the firmware sends it as a [`kvm_proto::link::BtMsg`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BtOut {
    Input {
        host: u8,
        id: u8,
        data: Vec<u8, { report::INPUT_MAX }>,
    },
    /// The host that is the active target (0: a PC is), and the hosts devices are pinned to (bit n−1 for host n).
    Select {
        host: u8,
        pinned: u8,
    },
    Pair {
        on: bool,
        seconds: u8,
    },
    Forget(u8),
    Passkey {
        host: u8,
        value: Option<u32>,
    },
    Policy {
        host: u8,
        policy: BtPolicy,
    },
    RequestState,
    Scan {
        on: bool,
        seconds: u8,
    },
    PairDevice([u8; 7]),
    ForgetDevice(u8),
    /// Forget every host and device, and restart the module (`Request::BtEraseAll`).
    EraseAll,
    RequestMap(u8),
    /// An output report for device slot `dev` (a keyboard's LEDs), without its ID byte.
    DeviceOutput {
        dev: u8,
        id: u8,
        data: Vec<u8, 8>,
    },
    /// A control-protocol report for host `host`'s session over the control service (software.md §8.1): the answer to
    /// its request, or an event.
    Control {
        host: u8,
        data: [u8; kvm_proto::ctrl::REPORT_LEN],
    },
}

/// Espressif's USB serial port in silicon (USB-Serial-JTAG: ESP32-C3 and later chips), which a dongle shows: a CDC-ACM
/// port whose descriptors the chip fixes, the same in its ROM bootloader as in any firmware.
pub const DONGLE_VID: u16 = 0x303A;
pub const DONGLE_PID: u16 = 0x1001;
/// Silicon Labs' CP210x USB-to-UART bridge, an ESP32 board's (an ESP32 has no USB of its own).
pub const CP210X_VID: u16 = 0x10C4;
pub const CP210X_PID: u16 = 0xEA60;

/// Whether a device with these USB IDs may be an ESP32 board the module can be (an ESP32-C3's own USB serial port,
/// or a CP210x bridge): what a KVM without a module offers to adopt (`Request::AdoptModule`).
pub fn dongle_ids(vid: u16, pid: u16) -> bool {
    matches!((vid, pid), (DONGLE_VID, DONGLE_PID) | (CP210X_VID, CP210X_PID))
}

/// The USB serial port a dongle shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum DongleKind {
    /// The chip's own (ESP32-C3): CDC-ACM; DTR/RTS reach its reset logic, and it leaves USB when it restarts.
    UsbJtag,
    /// A CP210x bridge (an ESP32 board): vendor requests set its speed and its DTR/RTS, which drive the board's EN
    /// and GPIO0 through the usual two transistors; it stays on USB while the chip restarts.
    Cp210x,
}

/// Where a dongle's serial port is: the interface its line requests go to (CDC communication, or the bridge's), and
/// the data endpoints (numbers without the direction bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DongleEps {
    pub kind: DongleKind,
    pub comm: u8,
    pub ep_in: u8,
    pub ep_out: u8,
    pub mps: u16,
}

/// The serial port on `device`, if it is one a dongle has: the ESP32-C3's own (VID/PID, a CDC communication interface
/// and a CDC data interface with a bulk endpoint each way), or a CP210x bridge (VID/PID, its vendor interface's bulk
/// endpoints). Any firmware looks like this, and a CP210x is on many boards: whether it is the module is for the link
/// to find out (`Hello`), and only an ESP32 with a blank flash is ever programmed without being one.
pub fn dongle_endpoints(device: &DeviceDesc, config: &[u8]) -> Option<DongleEps> {
    let kind = match (device.vid, device.pid) {
        (DONGLE_VID, DONGLE_PID) => DongleKind::UsbJtag,
        (CP210X_VID, CP210X_PID) => DongleKind::Cp210x,
        _ => return None,
    };
    let cfg = parse_config(config)?;
    let (comm, data) = match kind {
        DongleKind::UsbJtag => (
            cfg.interfaces.iter().find(|i| i.class == class::CDC)?.number,
            cfg.interfaces.iter().find(|i| i.class == class::CDC_DATA)?,
        ),
        DongleKind::Cp210x => {
            let i = cfg.interfaces.iter().find(|i| i.class == 0xFF)?;
            (i.number, i)
        }
    };
    let ep_in = data.endpoints.iter().find(|e| e.is_bulk() && e.is_in())?;
    let ep_out = data.endpoints.iter().find(|e| e.is_bulk() && !e.is_in())?;
    Some(DongleEps {
        kind,
        comm,
        ep_in: ep_in.number(),
        ep_out: ep_out.number(),
        mps: ep_in.max_packet.min(ep_out.max_packet),
    })
}

/// A passkey being typed on the KVM's keyboard for a pairing host (KeyboardOnly pairing: the host shows six digits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PasskeyEntry {
    pub host: u8,
    pub value: u32,
    pub digits: u8,
    /// Keys held at the last report, so that only new presses count.
    held: KeyState,
}

/// What a key press does to a passkey being entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyStep {
    /// Still typing.
    Typing,
    /// Enter after six digits: send it.
    Done(u32),
    /// Escape: cancel the pairing.
    Cancel,
}

impl PasskeyEntry {
    pub fn new(host: u8, held: KeyState) -> Self {
        Self { host, value: 0, digits: 0, held }
    }

    /// Feeds the keys held now; digits on the top row or the keypad, Backspace, Enter (or the sixth digit's Enter on
    /// the keypad), Escape.
    pub fn feed(&mut self, keys: &KeyState) -> PasskeyStep {
        let mut step = PasskeyStep::Typing;
        for k in keys.keys() {
            if self.held.is_pressed(k) {
                continue;
            }
            match digit(k) {
                Some(d) if self.digits < 6 => {
                    self.value = self.value * 10 + d as u32;
                    self.digits += 1;
                }
                Some(_) => {}
                None => match k {
                    0x2A => {
                        // Backspace
                        if self.digits > 0 {
                            self.value /= 10;
                            self.digits -= 1;
                        }
                    }
                    0x28 | 0x58 if self.digits == 6 => step = PasskeyStep::Done(self.value),
                    0x29 => step = PasskeyStep::Cancel,
                    _ => {}
                },
            }
        }
        self.held = *keys;
        step
    }
}

/// The digit a keyboard usage types: 1–9, 0 on the top row (0x1E–0x27) and the keypad (0x59–0x62).
fn digit(usage: u8) -> Option<u8> {
    match usage {
        0x1E..=0x26 => Some(usage - 0x1E + 1),
        0x27 => Some(0),
        0x59..=0x61 => Some(usage - 0x59 + 1),
        0x62 => Some(0),
        _ => None,
    }
}

/// The engine's view of the Bluetooth module.
#[derive(Debug, Clone, Copy)]
pub struct BtState {
    /// The module answered (its `Hello`); without it there are no Bluetooth targets (a board without the module, a
    /// board before 0.16.0, or a module that isn't programmed yet).
    pub present: bool,
    pub fw: u32,
    pub hosts: [HostState; BT_HOSTS],
    /// The host slot that gets input, 1–4 (None: the home PC does).
    pub active: Option<u8>,
    /// When pairing closes (ms, engine time); None while closed.
    pub pairing_until: Option<u32>,
    pub passkey: Option<PasskeyEntry>,
    /// Each host's keyboard LEDs (its output report).
    pub leds: [u8; BT_HOSTS],
    /// Last reports sent to the active host, so that only changes go out.
    pub sent_keyboard: [u8; report::KEYBOARD_LEN],
    pub sent_consumer: u16,
    /// Bit n−1: the module keeps host n's notification settings in RAM only, for this run of it (its flash wear guard,
    /// [`EventKind::SettingsHeld`]). The module tells again after every `RequestState`.
    pub settings_held: u8,
    /// The last `Select` sent (active host, pinned hosts), so that only changes go; None: send it again.
    pub selected: Option<(u8, u8)>,
}

impl BtState {
    pub const EMPTY: BtState = BtState {
        present: false,
        fw: 0,
        hosts: [HostState { host: 0, flags: 0, interval: 0, latency: 0, addr: [0; 7], rssi: 0, reports: 0 }; BT_HOSTS],
        active: None,
        pairing_until: None,
        passkey: None,
        leds: [0; BT_HOSTS],
        sent_keyboard: [0; report::KEYBOARD_LEN],
        sent_consumer: 0,
        settings_held: 0,
        selected: None,
    };

    pub fn host(&self, n: u8) -> Option<&HostState> {
        n.checked_sub(1).and_then(|i| self.hosts.get(i as usize))
    }

    /// Slot `n` can be selected: the module is there and holds a bond in it.
    pub fn selectable(&self, n: u8) -> bool {
        self.present && self.host(n).is_some_and(|h| h.bonded())
    }

    /// Bit n−1 for each slot that `f` picks.
    pub fn mask(&self, f: impl Fn(&HostState) -> bool) -> u8 {
        if !self.present {
            return 0;
        }
        self.hosts.iter().enumerate().filter(|(_, h)| f(h)).fold(0, |m, (i, _)| m | 1 << i)
    }

    pub fn info(&self, n: u8) -> Option<BtHostInfo> {
        let h = self.host(n)?;
        Some(BtHostInfo {
            host: n,
            flags: h.flags,
            interval: h.interval,
            latency: h.latency,
            rssi: h.rssi,
            addr: h.addr,
        })
    }

    /// Seconds pairing stays open (0 = closed).
    pub fn pairing_seconds(&self, now: u32) -> u8 {
        self.pairing_until.map_or(0, |t| (t.saturating_sub(now).div_ceil(1000)).min(255) as u8)
    }

    /// Records a module event (`held`: the keys held now, which a passkey being typed ignores until released).
    pub fn event(&mut self, kind: EventKind, host: u8, value: u32, now: u32, held: KeyState) {
        match kind {
            EventKind::PairingOpen => self.pairing_until = Some(now.wrapping_add(value.saturating_mul(1000))),
            EventKind::PairingClosed => {
                self.pairing_until = None;
                self.passkey = None;
            }
            EventKind::PasskeyRequest => self.passkey = Some(PasskeyEntry::new(host, held)),
            EventKind::Paired => {
                self.pairing_until = None;
                self.passkey = None;
            }
            EventKind::PairFailed => self.passkey = None,
            EventKind::BondLost => {}
            EventKind::SettingsHeld => {
                if (1..=BT_HOSTS as u8).contains(&host) {
                    self.settings_held |= 1 << (host - 1);
                }
            }
            // The devices' (`Devices::event`).
            EventKind::ScanOpen
            | EventKind::ScanClosed
            | EventKind::DeviceStage
            | EventKind::DevicePasskey
            | EventKind::DeviceBondLost => {}
        }
    }
}

/// Devices found while looking for devices to pair, at most.
pub const MAX_FOUND: usize = 8;

/// Bytes of the module's log the engine keeps (`ModuleLog`): some 160 lines (40 were too few to hold a pairing that
/// failed and the restart after it, 2026-11-25).
pub const MODULE_LOG_LEN: usize = 8192;

/// The wireless module's recent log, as its lines come over the link (`BtMsg::Log`), each with the engine time it came
/// at, and lines of the engine's own about the module: on a dongle the module's log pin goes nowhere. Kept across
/// module restarts; the oldest lines make room. Positions count bytes since the engine started.
pub struct ModuleLog {
    buf: heapless::Deque<u8, MODULE_LOG_LEN>,
    /// Bytes dropped from the front so far.
    first: u32,
}

impl ModuleLog {
    pub const EMPTY: ModuleLog = ModuleLog { buf: heapless::Deque::new(), first: 0 };

    /// Adds a line, `now` ms after the engine started.
    pub fn push(&mut self, now: u32, text: &str) {
        let mut line: heapless::String<96> = heapless::String::new();
        let _ = core::fmt::Write::write_fmt(&mut line, format_args!("[{:>5}.{:03}] ", now / 1000, now % 1000));
        for c in text.chars() {
            if line.len() + c.len_utf8() + 1 > line.capacity() {
                break;
            }
            let _ = line.push(if c.is_control() { ' ' } else { c });
        }
        let _ = line.push('\n');
        // Whole lines make room.
        while MODULE_LOG_LEN - self.buf.len() < line.len() {
            while let Some(b) = self.buf.pop_front() {
                self.first = self.first.wrapping_add(1);
                if b == b'\n' {
                    break;
                }
            }
        }
        for b in line.bytes() {
            let _ = self.buf.push_back(b);
        }
    }

    /// Up to `n` bytes from position `at` (or from the first kept, if that is gone), and the positions of the first
    /// byte kept and past the last: (first, end, at, bytes).
    pub fn read(&self, at: u32, n: usize) -> (u32, u32, u32, Vec<u8, 64>) {
        let end = self.first.wrapping_add(self.buf.len() as u32);
        let at = at.clamp(self.first, end);
        let skip = (at - self.first) as usize;
        let data = self.buf.iter().skip(skip).take(n.min(64)).copied().collect();
        (self.first, end, at, data)
    }
}

/// A report map asked for and not whole after this long (ms) is asked for again: a piece of it was lost on the way.
pub const MAP_RETRY_MS: u32 = 2000;

/// Where the RP2354B is with a device's report map, which it reads the device's reports with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MapState {
    /// No bond, or the module has no map for it.
    #[default]
    None,
    /// Asked for (at this engine time), coming in pieces.
    Asked(u32),
    /// Whole and parsed: the device's reports are read.
    Held,
    /// Whole and as the module has it, but the KVM's parser can't read it: not asked for again.
    Unreadable,
}

/// A Bluetooth device slot as the engine knows it: what the module says of it, and its report map.
pub struct Wireless {
    pub state: DeviceState,
    pub name: BtDeviceName,
    /// The report map as it comes in, piece by piece, and then whole (`Held` or `Unreadable`): kept for
    /// `Request::GetBtDeviceMap`.
    map: Vec<u8, MAP_MAX>,
    /// Made of the whole map: the device's reports are parsed with it.
    pub translator: Option<Translator>,
    /// The CRC of the map held or on its way (0: none). Until 0.1.31 it was set only once the map was whole, so every
    /// state the module sent meanwhile threw the pieces away and asked again, and a piece lost on the way left the
    /// device without its map for good: connected, and none of its reports read (2026-11-19).
    map_crc: u32,
    pub map_state: MapState,
    pub reports: u32,
    pub rate_hz: u16,
}

impl Wireless {
    /// Its gamepad's profile, by the USB IDs the module says (a Classic pad's reports come in their USB form).
    pub fn profile(&self) -> crate::pads::Profile {
        crate::pads::profile(self.state.vid, self.state.pid, false)
    }

    pub const EMPTY: Wireless = Wireless {
        state: DeviceState {
            dev: 0,
            flags: 0,
            rssi: 0,
            battery: 0xFF,
            appearance: 0,
            addr: [0; 7],
            map_len: 0,
            map_crc: 0,
            vid: 0,
            pid: 0,
        },
        name: BtDeviceName::new(),
        map: Vec::new(),
        translator: None,
        map_crc: 0,
        map_state: MapState::None,
        reports: 0,
        rate_hz: 0,
    };

    /// It has a bond and the map its reports are read with: it is one of the KVM's devices.
    pub fn usable(&self) -> bool {
        self.state.bonded() && self.translator.is_some()
    }

    /// Its reports come now.
    pub fn live(&self) -> bool {
        self.usable() && self.state.ready()
    }
}

/// The Bluetooth devices: their slots, the devices found while looking for one to pair, and the pairing under way.
pub struct Devices {
    pub slots: [Wireless; BT_DEVICES],
    /// When looking for devices stops (ms, engine time); None while not looking.
    pub scan_until: Option<u32>,
    pub found: Vec<BtFound, MAX_FOUND>,
    pub pairing: DevicePairing,
    /// The passkey to type on the keyboard being paired.
    pub passkey: Option<u32>,
}

/// What a module message about a device slot means for the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlotChange {
    /// Its state or name changed (`Event::BtDeviceChanged`).
    pub changed: bool,
    /// It became one of the KVM's devices, or stopped being one: the ports are decided again.
    pub usable: bool,
    /// Its reports stopped: what it holds is let go.
    pub stopped: bool,
    /// Its reports started (connected again): it gets its keyboard LEDs.
    pub started: bool,
    /// Ask the module for its report map.
    pub want_map: bool,
}

impl Devices {
    pub const EMPTY: Devices = Devices {
        slots: [Wireless::EMPTY, Wireless::EMPTY, Wireless::EMPTY, Wireless::EMPTY],
        scan_until: None,
        found: Vec::new(),
        pairing: DevicePairing::Idle,
        passkey: None,
    };

    pub fn slot(&self, n: u8) -> Option<&Wireless> {
        n.checked_sub(1).and_then(|i| self.slots.get(i as usize))
    }

    /// Bit n−1 for each slot that `f` picks.
    pub fn mask(&self, f: impl Fn(&Wireless) -> bool) -> u8 {
        self.slots.iter().enumerate().filter(|(_, w)| f(w)).fold(0, |m, (i, _)| m | 1 << i)
    }

    pub fn info(&self, n: u8) -> Option<BtDeviceInfo> {
        use kvm_proto::link::bt::device_flags;
        let w = self.slot(n)?;
        let map = match w.map_state {
            MapState::Held => device_flags::MAP_HELD,
            MapState::Unreadable => device_flags::MAP_UNREADABLE,
            MapState::None | MapState::Asked(_) => 0,
        };
        Some(BtDeviceInfo {
            slot: n,
            flags: w.state.flags & !(device_flags::MAP_HELD | device_flags::MAP_UNREADABLE) | map,
            rssi: w.state.rssi,
            battery: w.state.battery,
            appearance: w.state.appearance,
            addr: w.state.addr,
            name: w.name.clone(),
        })
    }

    /// The whole report map the engine holds for slot `n` (empty while none is).
    pub fn map_of(&self, n: u8) -> &[u8] {
        match self.slot(n) {
            Some(w) if matches!(w.map_state, MapState::Held | MapState::Unreadable) => &w.map,
            _ => &[],
        }
    }

    /// Seconds left looking for devices (0: not looking).
    pub fn scan_seconds(&self, now: u32) -> u8 {
        self.scan_until.map_or(0, |t| (t.saturating_sub(now).div_ceil(1000)).min(255) as u8)
    }

    /// The module's word on slot `state.dev` (`now`: engine time, for when its map is asked for).
    pub fn update(&mut self, state: DeviceState, name: &str, now: u32) -> Option<SlotChange> {
        let w = self.slots.get_mut((state.dev as usize).checked_sub(1)?)?;
        let was = (w.state, w.usable(), w.live(), w.map_state);
        let name = bt_device_name(name);
        let renamed = w.name != name;
        w.state = state;
        w.name = name;
        let map = if state.bonded() && state.map_len > 0 { state.map_crc } else { 0 };
        let mut want_map = false;
        if map != w.map_crc {
            // Another device or another map, or none: the one held goes, and the next is asked for once.
            w.translator = None;
            w.map.clear();
            w.map_crc = map;
            w.map_state = if map == 0 { MapState::None } else { MapState::Asked(now) };
            want_map = map != 0;
        }
        Some(SlotChange {
            changed: renamed || was.0 != state || was.3 != w.map_state,
            usable: was.1 != w.usable(),
            stopped: was.2 && !w.live(),
            started: !was.2 && w.live(),
            want_map,
        })
    }

    /// The slots whose map was asked for `MAP_RETRY_MS` ago and isn't whole: asked for again (bit n−1 for slot n).
    pub fn maps_overdue(&mut self, now: u32) -> u8 {
        let mut again = 0;
        for (i, w) in self.slots.iter_mut().enumerate() {
            if let MapState::Asked(t) = w.map_state
                && now.wrapping_sub(t) >= MAP_RETRY_MS
            {
                w.map.clear();
                w.map_state = MapState::Asked(now);
                again |= 1 << i;
            }
        }
        again
    }

    /// A piece of slot `n`'s report map. Returns the map's new state when it is whole and checked against the CRC the
    /// module gave: held (the device's reports can be read from now on) or unreadable.
    pub fn map_piece(&mut self, n: u8, offset: u16, total: u16, data: &[u8]) -> Option<MapState> {
        let w = (n as usize).checked_sub(1).and_then(|i| self.slots.get_mut(i))?;
        if total != w.state.map_len || !matches!(w.map_state, MapState::Asked(_)) {
            return None;
        }
        if offset == 0 {
            w.map.clear();
        }
        if offset as usize != w.map.len() || w.map.extend_from_slice(data).is_err() {
            // A piece went missing: the rest is useless until the map is asked for again (`maps_overdue`).
            w.map.clear();
            return None;
        }
        if w.map.len() < total as usize {
            return None;
        }
        if crc32(&w.map) != w.map_crc {
            w.map.clear();
            return None;
        }
        let layout = Layout::parse(&w.map).ok();
        w.translator = layout.map(Translator::new);
        w.map_state = if w.translator.is_some() { MapState::Held } else { MapState::Unreadable };
        Some(w.map_state)
    }

    /// A device found while looking: added, or its signal and name brought up to date. A full list makes room by
    /// dropping the weakest signal. Returns whether the list changed other than in a signal strength.
    pub fn found(&mut self, addr: [u8; 7], rssi: i8, appearance: u16, name: &str) -> bool {
        let name = bt_device_name(name);
        if let Some(f) = self.found.iter_mut().find(|f| f.addr == addr) {
            f.rssi = rssi;
            let renamed = !name.is_empty() && f.name != name || f.appearance != appearance;
            f.appearance = appearance;
            if !name.is_empty() {
                f.name = name;
            }
            return renamed;
        }
        let f = BtFound { addr, rssi, appearance, name };
        if self.found.is_full() {
            let Some((i, weakest)) = self.found.iter().enumerate().min_by_key(|(_, f)| f.rssi) else { return false };
            if weakest.rssi >= rssi {
                return false;
            }
            self.found.swap_remove(i);
        }
        self.found.push(f).is_ok()
    }

    /// A module event about the devices; returns whether it was one.
    pub fn event(&mut self, kind: EventKind, value: u32, now: u32) -> bool {
        match kind {
            EventKind::ScanOpen => {
                self.scan_until = Some(now.wrapping_add(value.saturating_mul(1000)));
            }
            EventKind::ScanClosed => self.scan_until = None,
            EventKind::DeviceStage => {
                self.pairing = DevicePairing::from_u8(value as u8).unwrap_or(DevicePairing::Failed);
                if self.pairing != DevicePairing::Pairing {
                    self.passkey = None;
                }
            }
            EventKind::DevicePasskey => {
                self.pairing = DevicePairing::Pairing;
                self.passkey = Some(value);
            }
            EventKind::DeviceBondLost => {}
            _ => return false,
        }
        true
    }
}

/// The keyboard report of `keys` for a Bluetooth host (the boot layout with report ID [`report::KEYBOARD`]).
pub fn keyboard(keys: &KeyState) -> [u8; report::KEYBOARD_LEN] {
    keys.to_boot()
}

/// The virtual gamepad's report for a Bluetooth host ([`report::GAMEPAD`]): its 32 buttons, the hat of its d-pad, and
/// its first six axes (X, Y, Z, Rx, Ry, Rz).
pub fn gamepad(p: &kvm_hid::normalized::Pad) -> [u8; report::GAMEPAD_LEN] {
    let mut r = [0u8; report::GAMEPAD_LEN];
    r[..4].copy_from_slice(&p.buttons.to_le_bytes());
    r[4] = kvm_hid::GamepadInput::hat_of(p.dpad);
    for (i, a) in p.axes[..6].iter().enumerate() {
        r[5 + 2 * i..7 + 2 * i].copy_from_slice(&a.max(&-32767).to_le_bytes());
    }
    r
}

/// The one consumer usage a Bluetooth host gets: the first held.
pub fn consumer(c: &ConsumerState) -> u16 {
    c.usages.iter().copied().find(|u| *u != 0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(us: &[u8]) -> KeyState {
        let mut k = KeyState::EMPTY;
        for u in us {
            k.press(*u);
        }
        k
    }

    #[test]
    fn the_gamepad_report_reads_back_through_the_report_map() {
        let t = kvm_hid::Translator::new(kvm_hid::Layout::parse(report::MAP).unwrap());
        let pad =
            kvm_hid::normalized::Pad { buttons: 1 << 16 | 1, dpad: 0b0010, axes: [-32767, 100, 0, 0, 0, 32767, 0, 0] };
        let mut raw = std::vec![report::GAMEPAD];
        raw.extend(gamepad(&pad));
        assert_eq!(report::input_len(report::GAMEPAD), Some(raw.len() - 1));
        let g = t.translate(&raw).gamepad.unwrap();
        assert_eq!((g.buttons, g.hat, g.axes[0], g.axes[1], g.axes[5]), (1 << 16 | 1, 2, -32767, 100, 32767));
    }

    #[test]
    fn dongle_found_by_ids_and_cdc_endpoints() {
        use crate::usbdesc::fixtures::{GAMING_MOUSE_CONFIG, USB_SERIAL_JTAG_CONFIG, USB_SERIAL_JTAG_DEVICE};
        let dev = DeviceDesc::parse(&USB_SERIAL_JTAG_DEVICE).unwrap();
        assert_eq!(
            dongle_endpoints(&dev, USB_SERIAL_JTAG_CONFIG),
            Some(DongleEps { kind: DongleKind::UsbJtag, comm: 0, ep_in: 1, ep_out: 1, mps: 64 })
        );
        // Other IDs, or the IDs without a CDC data interface: not a dongle.
        let other = DeviceDesc { pid: 0x1002, ..dev };
        assert_eq!(dongle_endpoints(&other, USB_SERIAL_JTAG_CONFIG), None);
        assert_eq!(dongle_endpoints(&dev, GAMING_MOUSE_CONFIG), None);
        // An ESP32 board's CP210x bridge: one vendor interface, a bulk endpoint each way.
        let bridge = DeviceDesc { vid: CP210X_VID, pid: CP210X_PID, ..dev };
        let config = [
            9, 2, 32, 0, 1, 1, 0, 0x80, 50, // configuration
            9, 4, 0, 0, 2, 0xFF, 0, 0, 2, // interface 0, vendor
            7, 5, 0x82, 2, 64, 0, 0, // bulk IN 2
            7, 5, 0x01, 2, 64, 0, 0, // bulk OUT 1
        ];
        assert_eq!(
            dongle_endpoints(&bridge, &config),
            Some(DongleEps { kind: DongleKind::Cp210x, comm: 0, ep_in: 2, ep_out: 1, mps: 64 })
        );
    }

    #[test]
    fn passkey_typed_on_top_row_and_keypad() {
        let mut e = PasskeyEntry::new(2, KeyState::EMPTY);
        // 1 2 3 on the top row, 4 5 6 on the keypad, each pressed and released.
        for u in [0x1E, 0x1F, 0x20, 0x5C, 0x5D, 0x5E] {
            assert_eq!(e.feed(&keys(&[u])), PasskeyStep::Typing);
            assert_eq!(e.feed(&KeyState::EMPTY), PasskeyStep::Typing);
        }
        assert_eq!(e.feed(&keys(&[0x28])), PasskeyStep::Done(123456));
    }

    #[test]
    fn passkey_backspace_escape_and_held_keys() {
        // Enter held from the command that started pairing doesn't count.
        let mut e = PasskeyEntry::new(1, keys(&[0x28]));
        assert_eq!(e.feed(&keys(&[0x28])), PasskeyStep::Typing);
        e.feed(&keys(&[0x27]));
        e.feed(&keys(&[0x27, 0x2A]));
        assert_eq!((e.value, e.digits), (0, 0));
        e.feed(&KeyState::EMPTY);
        // Leading zeros count as digits: 000042.
        for u in [0x27, 0x27, 0x27, 0x27, 0x21, 0x1F] {
            e.feed(&keys(&[u]));
            e.feed(&KeyState::EMPTY);
        }
        assert_eq!(e.feed(&keys(&[0x58])), PasskeyStep::Done(42));
        assert_eq!(e.feed(&keys(&[0x29])), PasskeyStep::Cancel);
    }

    #[test]
    fn a_device_map_comes_in_pieces_and_is_checked() {
        use kvm_proto::link::bt::device_flags;
        // A boot mouse: 3 buttons, X, Y.
        let map: &[u8] = &[
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00,
            0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30,
            0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0,
        ];
        let mut d = Devices::EMPTY;
        let state = DeviceState {
            dev: 2,
            flags: device_flags::BONDED | device_flags::CONNECTED | device_flags::ENCRYPTED | device_flags::READY,
            map_len: map.len() as u16,
            map_crc: crc32(map),
            ..Wireless::EMPTY.state
        };
        let len = map.len() as u16;
        let c = d.update(state, "Desk mouse", 0).unwrap();
        assert!(c.changed && c.want_map && !c.usable);
        assert_eq!(d.slots[1].map_state, MapState::Asked(0));
        // Out of order: dropped, and the rest with it.
        assert_eq!(d.map_piece(2, 20, len, &map[20..]), None);
        assert_eq!(d.map_piece(2, 0, len, &map[..20]), None);
        // Another state meanwhile (its battery): the map isn't asked for again, and the pieces so far stay.
        let state = DeviceState { battery: 80, ..state };
        let c = d.update(state, "Desk mouse", 5).unwrap();
        assert!(c.changed && !c.want_map);
        assert_eq!(d.map_piece(2, 20, len, &map[20..]), Some(MapState::Held));
        assert!(d.slots[1].live() && d.mask(Wireless::usable) == 0b10);
        assert_eq!(d.maps_overdue(MAP_RETRY_MS * 2), 0);
        // The same state again: nothing to ask for. Disconnected: its reports stop, it stays one of the devices.
        assert_eq!(d.update(state, "Desk mouse", 6), Some(SlotChange::default()));
        let off = DeviceState { flags: device_flags::BONDED, ..state };
        let c = d.update(off, "Desk mouse", 7).unwrap();
        assert!(c.stopped && !c.usable && d.slots[1].usable());
        assert!(d.update(state, "Desk mouse", 8).unwrap().started);
        d.update(off, "Desk mouse", 9);
        // Forgotten: the map goes with the bond.
        let c = d.update(DeviceState { flags: 0, map_len: 0, map_crc: 0, ..state }, "", 10).unwrap();
        assert!(c.usable && d.slots[1].translator.is_none() && d.slots[1].map_state == MapState::None);
        // A map whose CRC isn't the one given is not taken, and is asked for again later.
        let bad = DeviceState { map_crc: 1, ..state };
        assert!(d.update(bad, "x", 100).unwrap().want_map);
        assert_eq!(d.map_piece(2, 0, len, map), None);
        assert_eq!(d.maps_overdue(100 + MAP_RETRY_MS - 1), 0);
        assert_eq!(d.maps_overdue(100 + MAP_RETRY_MS), 0b10);
        assert!(d.update(state, "x", 200).unwrap().want_map);
    }

    #[test]
    fn a_device_map_with_a_lost_piece_is_asked_for_again_and_an_unparsable_one_is_not() {
        use kvm_proto::link::bt::device_flags;
        // Keyboard, report ID 1: modifiers and six keys, 39 bytes; in three pieces of 16.
        let map: &[u8] = &[
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
            0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x19, 0x00, 0x29, 0x65, 0x25, 0x65, 0x75, 0x08, 0x95, 0x06, 0x81, 0x00,
            0xC0,
        ];
        let len = map.len() as u16;
        let ready = device_flags::BONDED | device_flags::CONNECTED | device_flags::ENCRYPTED | device_flags::READY;
        let state = DeviceState { dev: 1, flags: ready, map_len: len, map_crc: crc32(map), ..Wireless::EMPTY.state };
        let mut d = Devices::EMPTY;
        assert!(d.update(state, "Keyboard", 1000).unwrap().want_map);
        // The middle piece lost on the link: the last doesn't fit, and nothing is asked for until the retry.
        assert_eq!(d.map_piece(1, 0, len, &map[..16]), None);
        assert_eq!(d.map_piece(1, 32, len, &map[32..]), None);
        assert!(!d.slots[0].live() && d.info(1).unwrap().flags & device_flags::MAP_HELD == 0);
        assert_eq!(d.maps_overdue(1000 + MAP_RETRY_MS), 0b1);
        for at in [0, 16, 32] {
            let piece = &map[at..(at + 16).min(map.len())];
            let whole = d.map_piece(1, at as u16, len, piece);
            assert_eq!(whole, (at == 32).then_some(MapState::Held));
        }
        assert!(d.slots[0].live() && d.info(1).unwrap().flags & device_flags::MAP_HELD != 0);
        // A map that can't be parsed (a collection closed that never opened): not asked for again, and said so.
        let junk: &[u8] = &[0xC0, 0xC0];
        let other = DeviceState { map_len: 2, map_crc: crc32(junk), ..state };
        let c = d.update(other, "Keyboard", 5000).unwrap();
        assert!(c.want_map && c.usable && c.stopped);
        assert_eq!(d.map_piece(1, 0, 2, junk), Some(MapState::Unreadable));
        assert_eq!(d.maps_overdue(5000 + 10 * MAP_RETRY_MS), 0);
        let flags = d.info(1).unwrap().flags;
        assert!(flags & device_flags::MAP_UNREADABLE != 0 && flags & device_flags::MAP_HELD == 0);
    }

    #[test]
    fn the_module_log_keeps_the_latest_whole_lines() {
        let mut log = ModuleLog::EMPTY;
        log.push(1234, "device 1: connected");
        let (first, end, at, data) = log.read(0, 64);
        assert_eq!((first, at), (0, 0));
        assert_eq!(core::str::from_utf8(&data).unwrap(), "[    1.234] device 1: connected\n");
        assert_eq!(end as usize, data.len());
        // About 20 bytes a line: twice what the log holds.
        let lines = (MODULE_LOG_LEN / 10) as u32;
        for i in 0..lines {
            let mut t: heapless::String<32> = heapless::String::new();
            let _ = core::fmt::Write::write_fmt(&mut t, format_args!("line {i}"));
            log.push(2000 + i, &t);
        }
        // Older lines made room, whole: reading from 0 starts at the first kept, at the start of a line.
        let (first, end, at, data) = log.read(0, 64);
        assert!(first > 0 && at == first && end - first <= MODULE_LOG_LEN as u32);
        assert!(data.starts_with(b"[    2."));
        let (_, _, _, tail) = log.read(end - 20, 64);
        let mut last: heapless::String<32> = heapless::String::new();
        let _ = core::fmt::Write::write_fmt(&mut last, format_args!("line {}\n", lines - 1));
        assert!(core::str::from_utf8(&tail).unwrap().ends_with(last.as_str()));
        // Control characters don't break lines; a long line is cut, not wrapped.
        log.push(5000, &"x\ny".repeat(100));
        let (_, end, _, last) = log.read(u32::MAX, 64);
        assert!(last.is_empty() && end > 0);
    }

    #[test]
    fn devices_found_keep_the_strongest_signals() {
        let mut d = Devices::EMPTY;
        for i in 0..MAX_FOUND as u8 {
            d.found([i; 7], -90 + i as i8, 0, "");
        }
        d.found([3; 7], -40, 0x03C1, "Keyboard");
        assert_eq!((d.found.len(), d.found[3].rssi, d.found[3].name.as_str()), (MAX_FOUND, -40, "Keyboard"));
        // Weaker than every one on the list: not taken; stronger: in place of the weakest.
        d.found([0xAA; 7], -95, 0, "");
        assert!(!d.found.iter().any(|f| f.addr == [0xAA; 7]));
        d.found([0xBB; 7], -30, 0, "");
        assert!(d.found.iter().any(|f| f.addr == [0xBB; 7]) && !d.found.iter().any(|f| f.addr == [0; 7]));
    }

    #[test]
    fn masks_need_the_module() {
        let mut s = BtState::EMPTY;
        s.hosts[1].flags = kvm_proto::link::bt::host_flags::BONDED;
        assert_eq!(s.mask(|h| h.bonded()), 0);
        s.present = true;
        assert_eq!(s.mask(|h| h.bonded()), 0b10);
        assert!(s.selectable(2) && !s.selectable(1) && !s.selectable(5) && !s.selectable(0));
    }
}
