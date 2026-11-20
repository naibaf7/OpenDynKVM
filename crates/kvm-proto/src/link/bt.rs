//! RP2354B ↔ Bluetooth LE coprocessor (ESP32-C3-MINI-1, boards 0.16.0 on) link messages (software.md §4.13).
//!
//! The same 72-byte [`Frame`](super::Frame) and go-back-N [`Arq`](super::Arq) as the AT32F405 links, carried over
//! UART0 at [`BAUD`] instead of SPI (two wires: the module's BOOT strap shares the RP2354B's TX line). The RP2354B is the master: it sends one frame and the module answers with one,
//! which is the SPI link's lockstep exchange, so the ARQ is unchanged. A frame whose CRC fails is dropped like a
//! corrupted SPI frame, and both sides resynchronise on the next [`MAGIC`](super::frame::MAGIC) whose frame checks.
//!
//! The coprocessor holds the Bluetooth hosts (PCs, tablets, phones) in up to [`MAX_HOSTS`] slots, numbered 1–4: a
//! bond each, and a connection while the host is in range. The RP2354B decides which one gets input
//! ([`BtMsg::Select`]) and sends it keyboard, mouse and media reports in the fixed Bluetooth report format below
//! ([`report`]); the coprocessor knows nothing of the KVM's ports, modes or keymap.
//!
//! At the same time it is the central of up to [`MAX_DEVICES`] Bluetooth devices (keyboards, mice: HID over GATT), in
//! device slots 1–4: a bond each, their report map, and a connection while they are on. Their input reports go to the
//! RP2354B as they come ([`BtMsg::DeviceInput`]), which parses them with the map ([`BtMsg::DeviceMap`]) as it does a
//! front-port device's.

use super::frame::MAX_MSG;
use crate::wire::{Reader, WireError, Writer, wire_enum};

/// UART0 speed (8N1). 72 bytes take 0.24 ms, far below a Bluetooth connection interval (≥ 7.5 ms).
pub const BAUD: u32 = 3_000_000;
/// Host slots on the coprocessor (one bond each).
pub const MAX_HOSTS: usize = 4;
/// `Hello.role` of the coprocessor.
pub const ROLE: u8 = 3;
/// Largest report body after `host` and `id`.
pub const REPORT_MAX: usize = MAX_MSG - 3;
/// Device slots on the coprocessor (one bond each).
pub const MAX_DEVICES: usize = 4;
/// Longest report map a device may have: a GATT attribute holds 512 bytes.
pub const MAP_MAX: usize = 512;
/// Report map bytes in one [`BtMsg::DeviceMap`].
pub const MAP_CHUNK: usize = MAX_MSG - 6;
/// Bytes of a module log line per [`BtMsg::Log`]; a longer line goes in several.
pub const LOG_MAX: usize = MAX_MSG - 2;
/// Longest name kept of a device (UTF-8 bytes).
pub const NAME_MAX: usize = crate::ctrl::BT_NAME_MAX;

pub mod ty {
    pub const HELLO: u8 = 0x01;
    pub const INPUT: u8 = 0x60;
    pub const SELECT: u8 = 0x61;
    pub const PAIR: u8 = 0x62;
    pub const FORGET: u8 = 0x63;
    pub const PASSKEY: u8 = 0x64;
    pub const POLICY: u8 = 0x65;
    pub const REQUEST_STATE: u8 = 0x66;
    pub const ENTER_ROM: u8 = 0x67;
    pub const SCAN: u8 = 0x68;
    pub const PAIR_DEVICE: u8 = 0x69;
    pub const FORGET_DEVICE: u8 = 0x6A;
    pub const REQUEST_MAP: u8 = 0x6B;
    pub const DEVICE_OUTPUT: u8 = 0x6C;
    pub const ERASE_ALL: u8 = 0x6D;
    pub const OUTPUT: u8 = 0x70;
    pub const HOST: u8 = 0x71;
    pub const EVENT: u8 = 0x72;
    pub const DEVICE_INPUT: u8 = 0x73;
    pub const DEVICE: u8 = 0x74;
    pub const DEVICE_MAP: u8 = 0x75;
    pub const FOUND: u8 = 0x76;
    pub const LOG: u8 = 0x77;
}

/// The reports a Bluetooth host gets: one HID service whose report map ([`report::MAP`]) every host accepts
/// (HID over GATT with a boot-style report map is built into desktop and mobile systems alike), so no host needs a driver and nothing depends on the
/// front-port devices. Mirroring a device's own descriptor over Bluetooth is not done (design.md §13).
pub mod report {
    /// Report IDs.
    pub const KEYBOARD: u8 = 1;
    pub const MOUSE: u8 = 2;
    pub const CONSUMER: u8 = 3;
    /// The absolute pointer of screen hop (protocol 9), for a host whose pointer is `BtPointer::Absolute`.
    pub const POINTER: u8 = 4;
    /// Keyboard: modifiers, reserved, 6 key usages (the boot layout; more than 6 keys → 6 × ErrorRollOver).
    pub const KEYBOARD_LEN: usize = 8;
    /// Mouse: buttons (16 bits), X, Y (16 bits, signed), wheel, pan (8 bits, signed).
    pub const MOUSE_LEN: usize = 8;
    /// Consumer: one 16-bit usage (0 = none).
    pub const CONSUMER_LEN: usize = 2;
    /// Absolute pointer: buttons (16 bits), X, Y (0–32767), wheel, pan (8 bits, signed).
    pub const POINTER_LEN: usize = 8;
    /// Keyboard output report (LEDs: Num, Caps, Scroll, Compose, Kana).
    pub const LEDS_LEN: usize = 1;

    /// The HID report map (descriptor) of the Bluetooth keyboard and mouse, 229 bytes. The absolute pointer is a
    /// top-level collection of its own, of usage Pointer rather than Mouse, as on the PCs' USB interface: a host makes
    /// it a device of its own, and one that takes no absolute pointer leaves it alone.
    #[rustfmt::skip]
    pub const MAP: &[u8] = &[
        // Keyboard, report 1
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, KEYBOARD,
        0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
        0x75, 0x08, 0x95, 0x01, 0x81, 0x01,
        0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, 0x75, 0x03, 0x95, 0x01, 0x91, 0x01,
        0x05, 0x07, 0x19, 0x00, 0x2A, 0xFF, 0x00, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x06, 0x81, 0x00,
        0xC0,
        // Mouse, report 2: 16 buttons, X/Y 16 bit, wheel, AC pan
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, MOUSE, 0x09, 0x01, 0xA1, 0x00,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x10, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x10, 0x81, 0x02,
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06,
        0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
        0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06,
        0xC0, 0xC0,
        // Consumer control, report 3
        0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, CONSUMER,
        0x15, 0x00, 0x26, 0xFF, 0x03, 0x19, 0x00, 0x2A, 0xFF, 0x03, 0x75, 0x10, 0x95, 0x01, 0x81, 0x00,
        0xC0,
        // Absolute pointer, report 4: 16 buttons, X/Y 0–32767, wheel, AC pan
        0x05, 0x01, 0x09, 0x01, 0xA1, 0x01, 0x85, POINTER, 0x09, 0x01, 0xA1, 0x00,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x10, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x10, 0x81, 0x02,
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02,
        0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
        0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06,
        0xC0, 0xC0,
    ];

    /// The input report IDs and their lengths, in the order of the report map.
    pub const INPUTS: [(u8, usize); 4] =
        [(KEYBOARD, KEYBOARD_LEN), (MOUSE, MOUSE_LEN), (CONSUMER, CONSUMER_LEN), (POINTER, POINTER_LEN)];

    /// Length of input report `id`, if it is one.
    pub fn input_len(id: u8) -> Option<usize> {
        INPUTS.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
    }
}

wire_enum! {
    /// What the coprocessor does with a bonded host while another target is active.
    pub enum Policy {
        /// Keep the link: switching to it takes effect at its next connection event (PCs).
        StayConnected = 0,
        /// Disconnect while not selected, reconnect when selected: many phones and tablets hide their on-screen
        /// keyboard while a hardware keyboard is connected (design.md §13).
        ConnectWhenSelected = 1,
    }
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Policy::StayConnected => "stay",
            Policy::ConnectWhenSelected => "on-select",
        }
    }

    pub fn from_name(s: &str) -> Option<Policy> {
        match s {
            "stay" | "stay-connected" => Some(Policy::StayConnected),
            "on-select" | "connect-when-selected" => Some(Policy::ConnectWhenSelected),
            _ => None,
        }
    }
}

wire_enum! {
    pub enum EventKind {
        /// Pairing is open (discoverable, bondable), `value` = seconds left.
        PairingOpen = 1,
        /// Pairing closed without a new bond (timed out or stopped).
        PairingClosed = 2,
        /// The host in `host` asks for the passkey it shows (KeyboardOnly): type it on the KVM's keyboard.
        PasskeyRequest = 3,
        /// The host in `host` is bonded now.
        Paired = 4,
        /// Pairing with a host failed, `value` = SMP reason code.
        PairFailed = 5,
        /// A bonded host lost its bond (it forgot the KVM); the slot is free again.
        BondLost = 6,
        /// The host in `host` changes its notification settings more often than the module writes them to flash in
        /// one run (its flash wear guard, `firmware/esp32c3/src/bonds.rs`): the module keeps them in RAM from now on,
        /// and they reach flash with the next save there is anyway (a pairing, a host forgotten). Once per host and
        /// run of the module.
        SettingsHeld = 7,
        /// Looking for devices to pair started, `value` = seconds.
        ScanOpen = 8,
        /// Looking for devices stopped (ran out, stopped, or a pairing began).
        ScanClosed = 9,
        /// Pairing a device moved on: `value` is the [`crate::ctrl::DevicePairing`] stage, `host` the device slot.
        DeviceStage = 10,
        /// The device being paired asks for a passkey to be typed on it: `value` (six digits), then Enter.
        DevicePasskey = 11,
        /// Device `host` lost its bond (it paired with something else): its slot is free again.
        DeviceBondLost = 12,
    }
}

/// `HostState.flags` bits.
pub mod host_flags {
    /// A bond is stored in this slot.
    pub const BONDED: u8 = 1 << 0;
    /// Connected now.
    pub const CONNECTED: u8 = 1 << 1;
    /// The link is encrypted with the bond's key (only then are reports sent).
    pub const ENCRYPTED: u8 = 1 << 2;
    /// The host enabled notifications of the keyboard input report.
    pub const SUBSCRIBED: u8 = 1 << 3;
}

/// `DeviceState.flags` bits.
pub mod device_flags {
    /// A bond is stored in this slot.
    pub const BONDED: u8 = 1 << 0;
    /// Connected now.
    pub const CONNECTED: u8 = 1 << 1;
    /// The link is encrypted with the bond's key.
    pub const ENCRYPTED: u8 = 1 << 2;
    /// Its input reports are subscribed to: input comes.
    pub const READY: u8 = 1 << 3;
    /// Set by the RP2354B in `ctrl::BtDeviceInfo` only: it holds the device's report map, so its reports are read.
    pub const MAP_HELD: u8 = 1 << 6;
    /// Set by the RP2354B in `ctrl::BtDeviceInfo` only: the device's report map came whole, and the KVM's parser can't
    /// read it.
    pub const MAP_UNREADABLE: u8 = 1 << 7;
}

/// The state of one device slot (coprocessor → RP2354B, on change and on [`BtMsg::RequestState`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DeviceState {
    /// 1–[`MAX_DEVICES`].
    pub dev: u8,
    /// [`device_flags`].
    pub flags: u8,
    /// Received signal strength in dBm (0 = unknown).
    pub rssi: i8,
    /// Battery level in % (0xFF = it reports none).
    pub battery: u8,
    /// Its GAP appearance (0 = unknown).
    pub appearance: u16,
    /// Its identity address, type byte then 6 bytes little-endian.
    pub addr: [u8; 7],
    /// Its report map's length and CRC-32 (0, 0: no bond): the RP2354B asks for the map ([`BtMsg::RequestMap`]) when
    /// it doesn't hold this one.
    pub map_len: u16,
    pub map_crc: u32,
}

impl DeviceState {
    pub fn bonded(&self) -> bool {
        self.flags & device_flags::BONDED != 0
    }

    pub fn ready(&self) -> bool {
        self.flags & device_flags::READY != 0
    }
}

/// The state of one host slot (coprocessor → RP2354B, on change and on [`BtMsg::RequestState`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct HostState {
    /// 1–[`MAX_HOSTS`].
    pub host: u8,
    /// [`host_flags`].
    pub flags: u8,
    /// Connection interval in units of 1.25 ms (0 = not connected).
    pub interval: u16,
    /// Peripheral latency (connection events the module may skip).
    pub latency: u16,
    /// The host's identity address, type byte (0 public, 1 random) then 6 bytes, little-endian as on air.
    pub addr: [u8; 7],
    /// Received signal strength in dBm (0 = unknown).
    pub rssi: i8,
}

impl HostState {
    pub fn bonded(&self) -> bool {
        self.flags & host_flags::BONDED != 0
    }

    pub fn connected(&self) -> bool {
        self.flags & host_flags::CONNECTED != 0
    }

    /// Ready for input: connected, encrypted and subscribed to the keyboard report.
    pub fn ready(&self) -> bool {
        let need = host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED;
        self.flags & need == need
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BtMsg<'a> {
    /// Both ways, as [`super::LinkMsg::Hello`]: the coprocessor sends `role` [`ROLE`].
    Hello { proto: u8, fw_version: u32, role: u8, flags: u8 },
    /// RP → BT: input report `id` ([`report`]) for `host`. Only the selected host takes it.
    Input { host: u8, id: u8, data: &'a [u8] },
    /// RP → BT: the host that gets input as the active target (0 = none: a PC is), and the hosts devices are pinned to
    /// (bit n−1 for host n): those take input too, and stay connected whatever their policy.
    Select { host: u8, pinned: u8 },
    /// RP → BT: open (`on`) or close pairing; it closes by itself after `seconds`.
    Pair { on: bool, seconds: u8 },
    /// RP → BT: forget the bond in `host` (0xFF = all) and drop its connection.
    Forget { host: u8 },
    /// RP → BT: the passkey typed for `host`'s [`EventKind::PasskeyRequest`]; `None` cancels the pairing.
    Passkey { host: u8, value: Option<u32> },
    /// RP → BT: what to do with `host` while another target is active.
    Policy { host: u8, policy: Policy },
    /// RP → BT: send every slot's [`HostState`] now.
    RequestState,
    /// RP → BT: restart into the ROM bootloader (UART download), to be reprogrammed. The module's BOOT strap shares
    /// the RP2354B's TX line, so it can't be held low across a reset of a running module: it restarts into its ROM
    /// by itself (software.md §4.13).
    EnterRom,
    /// BT → RP: output report `id` from `host` (keyboard LEDs).
    Output { host: u8, id: u8, data: &'a [u8] },
    /// BT → RP: a slot changed.
    Host(HostState),
    /// BT → RP: pairing and bond events.
    Event { kind: EventKind, host: u8, value: u32 },
    /// RP → BT: look for devices to pair (`on`) for `seconds`, or stop. Host pairing closes meanwhile.
    Scan { on: bool, seconds: u8 },
    /// RP → BT: pair the device that advertises from `addr` ([`BtMsg::Found`]) into a free device slot.
    PairDevice { addr: [u8; 7] },
    /// RP → BT: forget device slot `dev` (0xFF = all) and drop its connection.
    ForgetDevice { dev: u8 },
    /// RP → BT: forget every host and device, write empty records into both flash sectors of each, drop the stack's
    /// keys, and restart (which empties the controller's resolving list too): for pairings that keep failing on
    /// state the module shouldn't hold (`ctrl::Request::BtEraseAll`).
    EraseAll,
    /// RP → BT: send device slot `dev`'s report map ([`BtMsg::DeviceMap`]).
    RequestMap { dev: u8 },
    /// RP → BT: output report `id` for device `dev` (a keyboard's LEDs), without the ID byte.
    DeviceOutput { dev: u8, id: u8, data: &'a [u8] },
    /// BT → RP: input report `id` of device `dev` as it came, without the ID byte (`id` 0: its map has no report IDs).
    DeviceInput { dev: u8, id: u8, data: &'a [u8] },
    /// BT → RP: a device slot changed; `name` is what the device calls itself.
    Device { state: DeviceState, name: &'a str },
    /// BT → RP: bytes from `offset` of device `dev`'s report map, `total` bytes long.
    DeviceMap { dev: u8, offset: u16, total: u16, data: &'a [u8] },
    /// BT → RP: a device that may be paired, found while looking (again at most once a second, with its signal now).
    Found { addr: [u8; 7], rssi: i8, appearance: u16, name: &'a str },
    /// BT → RP: a line of the module's log (at most [`LOG_MAX`] bytes), which the RP2354B keeps for
    /// `ctrl::Request::GetModuleLog`: on a dongle the module's own log pin goes nowhere.
    Log { text: &'a str },
}

const PASSKEY_CANCEL: u32 = u32::MAX;

impl<'a> BtMsg<'a> {
    pub fn encode(&self, out: &mut [u8; MAX_MSG]) -> Result<usize, WireError> {
        let mut w = Writer::new(out);
        match *self {
            BtMsg::Hello { proto, fw_version, role, flags } => {
                w.u8(ty::HELLO)?;
                w.u8(proto)?;
                w.u32(fw_version)?;
                w.u8(role)?;
                w.u8(flags)?;
            }
            BtMsg::Input { host, id, data } | BtMsg::Output { host, id, data } => {
                if data.len() > REPORT_MAX {
                    return Err(WireError::Invalid);
                }
                w.u8(if matches!(self, BtMsg::Input { .. }) { ty::INPUT } else { ty::OUTPUT })?;
                w.u8(host)?;
                w.u8(id)?;
                w.bytes(data)?;
            }
            BtMsg::Select { host, pinned } => {
                w.u8(ty::SELECT)?;
                w.u8(host)?;
                w.u8(pinned)?;
            }
            BtMsg::Pair { on, seconds } => {
                w.u8(ty::PAIR)?;
                w.bool(on)?;
                w.u8(seconds)?;
            }
            BtMsg::Forget { host } => {
                w.u8(ty::FORGET)?;
                w.u8(host)?;
            }
            BtMsg::Passkey { host, value } => {
                w.u8(ty::PASSKEY)?;
                w.u8(host)?;
                w.u32(value.unwrap_or(PASSKEY_CANCEL))?;
            }
            BtMsg::Policy { host, policy } => {
                w.u8(ty::POLICY)?;
                w.u8(host)?;
                w.u8(policy as u8)?;
            }
            BtMsg::RequestState => w.u8(ty::REQUEST_STATE)?,
            BtMsg::EraseAll => w.u8(ty::ERASE_ALL)?,
            BtMsg::EnterRom => w.u8(ty::ENTER_ROM)?,
            BtMsg::Host(h) => {
                w.u8(ty::HOST)?;
                w.u8(h.host)?;
                w.u8(h.flags)?;
                w.u16(h.interval)?;
                w.u16(h.latency)?;
                w.bytes(&h.addr)?;
                w.u8(h.rssi as u8)?;
            }
            BtMsg::Event { kind, host, value } => {
                w.u8(ty::EVENT)?;
                w.u8(kind as u8)?;
                w.u8(host)?;
                w.u32(value)?;
            }
            BtMsg::Scan { on, seconds } => {
                w.u8(ty::SCAN)?;
                w.bool(on)?;
                w.u8(seconds)?;
            }
            BtMsg::PairDevice { addr } => {
                w.u8(ty::PAIR_DEVICE)?;
                w.bytes(&addr)?;
            }
            BtMsg::ForgetDevice { dev } | BtMsg::RequestMap { dev } => {
                w.u8(if matches!(self, BtMsg::ForgetDevice { .. }) { ty::FORGET_DEVICE } else { ty::REQUEST_MAP })?;
                w.u8(dev)?;
            }
            BtMsg::DeviceOutput { dev, id, data } | BtMsg::DeviceInput { dev, id, data } => {
                if data.len() > REPORT_MAX {
                    return Err(WireError::Invalid);
                }
                w.u8(if matches!(self, BtMsg::DeviceInput { .. }) { ty::DEVICE_INPUT } else { ty::DEVICE_OUTPUT })?;
                w.u8(dev)?;
                w.u8(id)?;
                w.bytes(data)?;
            }
            BtMsg::Device { state: d, name } => {
                w.u8(ty::DEVICE)?;
                w.u8(d.dev)?;
                w.u8(d.flags)?;
                w.u8(d.rssi as u8)?;
                w.u8(d.battery)?;
                w.u16(d.appearance)?;
                w.bytes(&d.addr)?;
                w.u16(d.map_len)?;
                w.u32(d.map_crc)?;
                w.lv(&name.as_bytes()[..name.len().min(NAME_MAX)])?;
            }
            BtMsg::DeviceMap { dev, offset, total, data } => {
                if data.len() > MAP_CHUNK {
                    return Err(WireError::Invalid);
                }
                w.u8(ty::DEVICE_MAP)?;
                w.u8(dev)?;
                w.u16(offset)?;
                w.u16(total)?;
                w.bytes(data)?;
            }
            BtMsg::Found { addr, rssi, appearance, name } => {
                w.u8(ty::FOUND)?;
                w.bytes(&addr)?;
                w.u8(rssi as u8)?;
                w.u16(appearance)?;
                w.lv(&name.as_bytes()[..name.len().min(NAME_MAX)])?;
            }
            BtMsg::Log { text } => {
                w.u8(ty::LOG)?;
                w.lv(text.as_bytes())?;
            }
        }
        Ok(w.len())
    }

    pub fn decode(msg: &'a [u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(msg);
        Ok(match r.u8()? {
            ty::HELLO => BtMsg::Hello { proto: r.u8()?, fw_version: r.u32()?, role: r.u8()?, flags: r.u8()? },
            ty::INPUT => BtMsg::Input { host: r.u8()?, id: r.u8()?, data: r.rest() },
            ty::OUTPUT => BtMsg::Output { host: r.u8()?, id: r.u8()?, data: r.rest() },
            ty::SELECT => BtMsg::Select { host: r.u8()?, pinned: r.u8()? },
            ty::PAIR => BtMsg::Pair { on: r.bool()?, seconds: r.u8()? },
            ty::FORGET => BtMsg::Forget { host: r.u8()? },
            ty::PASSKEY => {
                let host = r.u8()?;
                let v = r.u32()?;
                BtMsg::Passkey { host, value: (v != PASSKEY_CANCEL).then_some(v) }
            }
            ty::POLICY => BtMsg::Policy { host: r.u8()?, policy: r.u8()?.try_into()? },
            ty::REQUEST_STATE => BtMsg::RequestState,
            ty::ERASE_ALL => BtMsg::EraseAll,
            ty::ENTER_ROM => BtMsg::EnterRom,
            ty::HOST => BtMsg::Host(HostState {
                host: r.u8()?,
                flags: r.u8()?,
                interval: r.u16()?,
                latency: r.u16()?,
                addr: r.array()?,
                rssi: r.u8()? as i8,
            }),
            ty::EVENT => BtMsg::Event { kind: r.u8()?.try_into()?, host: r.u8()?, value: r.u32()? },
            ty::SCAN => BtMsg::Scan { on: r.bool()?, seconds: r.u8()? },
            ty::PAIR_DEVICE => BtMsg::PairDevice { addr: r.array()? },
            ty::FORGET_DEVICE => BtMsg::ForgetDevice { dev: r.u8()? },
            ty::REQUEST_MAP => BtMsg::RequestMap { dev: r.u8()? },
            ty::DEVICE_OUTPUT => BtMsg::DeviceOutput { dev: r.u8()?, id: r.u8()?, data: r.rest() },
            ty::DEVICE_INPUT => BtMsg::DeviceInput { dev: r.u8()?, id: r.u8()?, data: r.rest() },
            ty::DEVICE => {
                let state = DeviceState {
                    dev: r.u8()?,
                    flags: r.u8()?,
                    rssi: r.u8()? as i8,
                    battery: r.u8()?,
                    appearance: r.u16()?,
                    addr: r.array()?,
                    map_len: r.u16()?,
                    map_crc: r.u32()?,
                };
                BtMsg::Device { state, name: name(&mut r)? }
            }
            ty::DEVICE_MAP => BtMsg::DeviceMap { dev: r.u8()?, offset: r.u16()?, total: r.u16()?, data: r.rest() },
            ty::LOG => BtMsg::Log { text: name(&mut r)? },
            ty::FOUND => {
                BtMsg::Found { addr: r.array()?, rssi: r.u8()? as i8, appearance: r.u16()?, name: name(&mut r)? }
            }
            _ => return Err(WireError::Invalid),
        })
    }

    /// Input reports go ahead of everything else, as on the AT32F405 links.
    pub fn prio(&self) -> super::Prio {
        match self {
            BtMsg::Input { .. } | BtMsg::DeviceInput { .. } => super::Prio::High,
            _ => super::Prio::Low,
        }
    }
}

fn name<'a>(r: &mut Reader<'a>) -> Result<&'a str, WireError> {
    core::str::from_utf8(r.lv()?).map_err(|_| WireError::Invalid)
}

/// What a GAP appearance (Bluetooth Assigned Numbers §2.6) says a device is.
pub mod appearance {
    pub const KEYBOARD: u16 = 0x03C1;
    pub const MOUSE: u16 = 0x03C2;
    pub const JOYSTICK: u16 = 0x03C3;
    pub const GAMEPAD: u16 = 0x03C4;
    pub const DIGITIZER: u16 = 0x03C5;
    pub const CARD_READER: u16 = 0x03C6;
    pub const DIGITAL_PEN: u16 = 0x03C7;
    pub const BARCODE: u16 = 0x03C8;
    pub const TOUCHPAD: u16 = 0x03C9;
    pub const PRESENTER: u16 = 0x03CA;

    /// The Human Interface Device category (0x03C0–0x03FF).
    pub const fn is_hid(a: u16) -> bool {
        a >> 6 == 0x03C0 >> 6
    }
}

/// The keyboard report for `keys` (modifier bits, then the pressed usages 0x04–0xA4 in any order).
pub fn keyboard_report(modifiers: u8, keys: impl Iterator<Item = u8>) -> [u8; report::KEYBOARD_LEN] {
    let mut r = [0u8; report::KEYBOARD_LEN];
    r[0] = modifiers;
    for (n, k) in keys.enumerate() {
        if n == 6 {
            // More than six: the boot keyboard's phantom state, every slot ErrorRollOver.
            r[2..].fill(0x01);
            return r;
        }
        r[2 + n] = k;
    }
    r
}

/// The mouse report: buttons, X/Y clamped to 16 bits, wheel and pan to 8.
pub fn mouse_report(buttons: u16, x: i32, y: i32, wheel: i32, pan: i32) -> [u8; report::MOUSE_LEN] {
    let x = x.clamp(-32767, 32767) as i16;
    let y = y.clamp(-32767, 32767) as i16;
    let b = buttons.to_le_bytes();
    let (xb, yb) = (x.to_le_bytes(), y.to_le_bytes());
    [b[0], b[1], xb[0], xb[1], yb[0], yb[1], wheel.clamp(-127, 127) as i8 as u8, pan.clamp(-127, 127) as i8 as u8]
}

/// The absolute pointer's report ([`report::POINTER`]): `x`, `y` 0–32767.
pub fn pointer_report(buttons: u16, x: u16, y: u16, wheel: i32, pan: i32) -> [u8; report::POINTER_LEN] {
    let b = buttons.to_le_bytes();
    let (xb, yb) = (x.min(0x7FFF).to_le_bytes(), y.min(0x7FFF).to_le_bytes());
    [b[0], b[1], xb[0], xb[1], yb[0], yb[1], wheel.clamp(-127, 127) as i8 as u8, pan.clamp(-127, 127) as i8 as u8]
}

/// Motion for a host not sent yet, coalesced: a mouse reports up to 1000 times a second, more than the link to a
/// module on a front port carries and far more than a host takes at its connection events, and a report that waits
/// in a queue is late while one that is dropped loses its motion (first dongle, 2026-11-14: the pointer crawled). So
/// one report waits at a time: relative moves (and wheels) add up, an absolute position replaces the one before. A
/// report with other buttons, or for another host, sends the waiting one first, so that no click is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Motion {
    waiting: Option<Waiting>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Waiting {
    host: u8,
    id: u8,
    buttons: u16,
    /// Relative: the sum of the moves; absolute: the position.
    x: i32,
    y: i32,
    wheel: i32,
    pan: i32,
}

/// A report for a host: its host slot, its report ID and its bytes.
pub type Out = (u8, u8, [u8; 8]);

impl Motion {
    pub const fn new() -> Self {
        Motion { waiting: None }
    }

    /// Whether report `id` is motion (the mouse, the absolute pointer).
    pub const fn is_motion(id: u8) -> bool {
        id == report::MOUSE || id == report::POINTER
    }

    /// Adds a motion report `data` (`id` [`report::MOUSE`] or [`report::POINTER`]) for `host`. Returns the report that
    /// has to go out before it, if one waited that can't take it in.
    pub fn add(&mut self, host: u8, id: u8, data: &[u8]) -> Option<Out> {
        let mut d = [0u8; 8];
        d[..data.len().min(8)].copy_from_slice(&data[..data.len().min(8)]);
        let buttons = u16::from_le_bytes([d[0], d[1]]);
        let (x, y) = if id == report::POINTER {
            (u16::from_le_bytes([d[2], d[3]]) as i32, u16::from_le_bytes([d[4], d[5]]) as i32)
        } else {
            (i16::from_le_bytes([d[2], d[3]]) as i32, i16::from_le_bytes([d[4], d[5]]) as i32)
        };
        let (wheel, pan) = (d[6] as i8 as i32, d[7] as i8 as i32);
        if let Some(w) = self.waiting.as_mut()
            && (w.host, w.id, w.buttons) == (host, id, buttons)
        {
            if id == report::POINTER {
                (w.x, w.y) = (x, y);
            } else {
                (w.x, w.y) = (w.x.saturating_add(x), w.y.saturating_add(y));
            }
            (w.wheel, w.pan) = (w.wheel.saturating_add(wheel), w.pan.saturating_add(pan));
            return None;
        }
        let before = self.take_all();
        self.waiting = Some(Waiting { host, id, buttons, x, y, wheel, pan });
        before
    }

    /// The waiting report to send now, if there is one. A relative move longer than one report carries comes out in
    /// steps: what doesn't fit keeps waiting.
    pub fn take(&mut self) -> Option<Out> {
        let w = self.waiting.as_mut()?;
        let out = if w.id == report::POINTER {
            let r = pointer_report(w.buttons, w.x.clamp(0, 0x7FFF) as u16, w.y.clamp(0, 0x7FFF) as u16, w.wheel, w.pan);
            let (wheel, pan) = (w.wheel.clamp(-127, 127), w.pan.clamp(-127, 127));
            (w.wheel, w.pan) = (w.wheel - wheel, w.pan - pan);
            (w.host, w.id, r)
        } else {
            let (x, y) = (w.x.clamp(-32767, 32767), w.y.clamp(-32767, 32767));
            let (wheel, pan) = (w.wheel.clamp(-127, 127), w.pan.clamp(-127, 127));
            let r = mouse_report(w.buttons, x, y, wheel, pan);
            (w.x, w.y, w.wheel, w.pan) = (w.x - x, w.y - y, w.wheel - wheel, w.pan - pan);
            (w.host, w.id, r)
        };
        // A position is sent once; a move keeps waiting while some of it is left.
        if w.id == report::POINTER && w.wheel == 0 && w.pan == 0
            || w.id == report::MOUSE && w.x == 0 && w.y == 0 && w.wheel == 0 && w.pan == 0
        {
            self.waiting = None;
        }
        Some(out)
    }

    /// Everything waiting, in one report (a move longer than one report carries is cut: it only happens with motion
    /// that has waited for a very long time).
    fn take_all(&mut self) -> Option<Out> {
        let out = self.take();
        self.waiting = None;
        out
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_adds_up_moves_keeps_positions_and_clicks() {
        let mut m = Motion::default();
        // Moves add up into one report.
        for _ in 0..10 {
            assert_eq!(m.add(1, report::MOUSE, &mouse_report(0, 3, -2, 1, 0)), None);
        }
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 30, -20, 10, 0))));
        assert_eq!(m.take(), None);
        // A press goes out after the move before it, and is not merged into it.
        m.add(1, report::MOUSE, &mouse_report(0, 5, 0, 0, 0));
        assert_eq!(
            m.add(1, report::MOUSE, &mouse_report(1, 0, 0, 0, 0)),
            Some((1, report::MOUSE, mouse_report(0, 5, 0, 0, 0)))
        );
        assert_eq!(m.add(1, report::MOUSE, &mouse_report(1, 2, 2, 0, 0)), None);
        assert_eq!(
            m.add(1, report::MOUSE, &mouse_report(0, 0, 0, 0, 0)),
            Some((1, report::MOUSE, mouse_report(1, 2, 2, 0, 0)))
        );
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 0, 0, 0, 0))));
        // Positions: the latest counts.
        m.add(2, report::POINTER, &pointer_report(0, 100, 100, 0, 0));
        m.add(2, report::POINTER, &pointer_report(0, 200, 300, 1, 0));
        assert_eq!(m.take(), Some((2, report::POINTER, pointer_report(0, 200, 300, 1, 0))));
        assert!(m.is_empty());
        // A move longer than a report carries comes out in steps.
        m.add(1, report::MOUSE, &mouse_report(0, 30000, 0, 0, 0));
        m.add(1, report::MOUSE, &mouse_report(0, 30000, 0, 0, 0));
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 32767, 0, 0, 0))));
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 27233, 0, 0, 0))));
        assert!(m.is_empty());
    }

    fn roundtrip(m: BtMsg) {
        let mut buf = [0u8; MAX_MSG];
        let n = m.encode(&mut buf).unwrap();
        assert_eq!(BtMsg::decode(&buf[..n]).unwrap(), m);
    }

    #[test]
    fn messages_roundtrip() {
        roundtrip(BtMsg::Hello { proto: 1, fw_version: 0x000100, role: ROLE, flags: 0 });
        roundtrip(BtMsg::Input { host: 2, id: report::KEYBOARD, data: &[2, 0, 4, 0, 0, 0, 0, 0] });
        roundtrip(BtMsg::Output { host: 1, id: report::KEYBOARD, data: &[0b10] });
        roundtrip(BtMsg::Select { host: 0, pinned: 0b0101 });
        roundtrip(BtMsg::Pair { on: true, seconds: 60 });
        roundtrip(BtMsg::Forget { host: 0xFF });
        roundtrip(BtMsg::Passkey { host: 3, value: Some(123456) });
        roundtrip(BtMsg::Passkey { host: 3, value: None });
        roundtrip(BtMsg::Policy { host: 4, policy: Policy::ConnectWhenSelected });
        roundtrip(BtMsg::RequestState);
        roundtrip(BtMsg::EnterRom);
        roundtrip(BtMsg::Host(HostState {
            host: 1,
            flags: host_flags::BONDED | host_flags::CONNECTED,
            interval: 12,
            latency: 0,
            addr: [1, 2, 3, 4, 5, 6, 7],
            rssi: -60,
        }));
        roundtrip(BtMsg::Event { kind: EventKind::PairFailed, host: 2, value: 5 });
        roundtrip(BtMsg::Event { kind: EventKind::SettingsHeld, host: 3, value: 0 });
        roundtrip(BtMsg::Event { kind: EventKind::DevicePasskey, host: 0, value: 123456 });
        roundtrip(BtMsg::Scan { on: true, seconds: 60 });
        roundtrip(BtMsg::PairDevice { addr: [1, 6, 5, 4, 3, 2, 0xC1] });
        roundtrip(BtMsg::ForgetDevice { dev: 0xFF });
        roundtrip(BtMsg::EraseAll);
        roundtrip(BtMsg::RequestMap { dev: 2 });
        roundtrip(BtMsg::DeviceOutput { dev: 1, id: 1, data: &[0b10] });
        roundtrip(BtMsg::DeviceInput { dev: 4, id: 0, data: &[0, 0, 4, 0, 0, 0, 0, 0] });
        let state = DeviceState {
            dev: 3,
            flags: device_flags::BONDED | device_flags::READY,
            rssi: -55,
            battery: 80,
            appearance: appearance::KEYBOARD,
            addr: [0, 1, 2, 3, 4, 5, 6],
            map_len: 200,
            map_crc: 0xDEAD_BEEF,
        };
        roundtrip(BtMsg::Device { state, name: "Desk keyboard" });
        roundtrip(BtMsg::DeviceMap { dev: 3, offset: 59, total: 200, data: &[0x05; MAP_CHUNK] });
        roundtrip(BtMsg::Found { addr: [1; 7], rssi: -70, appearance: appearance::MOUSE, name: "" });
        // The longest name fits, with everything else, in one message.
        let long = "n".repeat(NAME_MAX);
        roundtrip(BtMsg::Device { state, name: &long });
        roundtrip(BtMsg::Found { addr: [1; 7], rssi: -70, appearance: 0, name: &long });
        roundtrip(BtMsg::Log { text: "device 1: encrypted after 42 ms" });
        roundtrip(BtMsg::Log { text: &"l".repeat(LOG_MAX) });
        let mut buf = [0u8; MAX_MSG];
        assert!(BtMsg::Log { text: &"l".repeat(LOG_MAX + 1) }.encode(&mut buf).is_err());
        assert!(appearance::is_hid(appearance::KEYBOARD) && appearance::is_hid(0x03FF) && !appearance::is_hid(0x0400));
    }

    #[test]
    fn hello_is_the_spi_links_hello() {
        let mut buf = [0u8; MAX_MSG];
        let n = BtMsg::Hello { proto: 1, fw_version: 7, role: ROLE, flags: 0 }.encode(&mut buf).unwrap();
        assert!(matches!(
            super::super::LinkMsg::decode(&buf[..n]),
            Ok(super::super::LinkMsg::Hello { role: ROLE, .. })
        ));
    }

    #[test]
    fn map_is_well_formed() {
        // Every collection closed, and the report IDs in the map are the ones `INPUTS` lists.
        let mut depth = 0i32;
        let mut ids = heapless::Vec::<u8, 4>::new();
        let mut i = 0;
        while i < report::MAP.len() {
            let prefix = report::MAP[i];
            let size = match prefix & 3 {
                3 => 4,
                s => s as usize,
            };
            match prefix & 0xFC {
                0xA0 => depth += 1,
                0xC0 => depth -= 1,
                0x84 => ids.push(report::MAP[i + 1]).unwrap(),
                _ => {}
            }
            i += 1 + size;
        }
        assert_eq!(depth, 0);
        assert_eq!(i, report::MAP.len());
        assert_eq!(ids.as_slice(), &[report::KEYBOARD, report::MOUSE, report::CONSUMER, report::POINTER]);
        assert!(report::MAP.len() <= 512, "a GATT attribute holds at most 512 bytes");
        assert_eq!(report::MAP.len(), 229, "the length software.md §4.13 gives");
    }

    #[test]
    fn keyboard_rollover() {
        assert_eq!(keyboard_report(0x02, [4u8, 5].into_iter()), [0x02, 0, 4, 5, 0, 0, 0, 0]);
        assert_eq!(keyboard_report(0, 4u8..11), [0, 0, 1, 1, 1, 1, 1, 1]);
        assert_eq!(mouse_report(1, -40000, 3, 200, -1), [1, 0, 0x01, 0x80, 3, 0, 127, 0xFF]);
        assert_eq!(pointer_report(0x8001, 40000, 300, -2, 0), [1, 0x80, 0xFF, 0x7F, 0x2C, 1, 0xFE, 0]);
    }
}
