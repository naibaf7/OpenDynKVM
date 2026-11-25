//! Control protocol between `odkvmctl` and the RP2354B (software.md §8).
//!
//! Every message is one 64-byte HID report on the KVM's vendor interface:
//! `type u8 | request id u8 | payload[62]`. A response has the request's type | 0x80 and the same id; errors are type
//! 0xFF; events (unsolicited, sent to both PCs, `HotkeyLearned` only to the PC that asked) use types 0xC0.. and id 0.

use heapless::{String, Vec};

use crate::wire::{Reader, WireError, Writer, wire_enum};

pub const REPORT_LEN: usize = 64;
pub const PAYLOAD_LEN: usize = REPORT_LEN - 2;
/// The protocol's version. A host and the firmware talk only when theirs are the same: otherwise the host offers the
/// firmware it came with, and nothing else (`GetInfo` and the `Fw*` requests are all it uses then). Raised with every
/// change of a message, a config key or a value.
pub const PROTOCOL_VERSION: u8 = 15;

/// Vendor usage page and usage of the control interface; hosts discover it by these, not by VID/PID.
pub const USAGE_PAGE: u16 = 0xFF4B;
pub const USAGE: u16 = 0x0001;
/// `iInterface` string of the control interface (matched by the Linux udev rule).
pub const INTERFACE_STRING: &str = "OpenDynKVM Control";
/// `iInterface` string of the Vial interface (matched by the Linux udev rule, which lets the logged-in user's browser
/// open it for vial.rocks; without it the hidraw node is root's, and Vial waits at "connecting" for good).
pub const VIAL_INTERFACE_STRING: &str = "OpenDynKVM Vial";

pub const FW_CHUNK: usize = 56;
pub const STRING_MAX: usize = 58;

wire_enum! {
    pub enum Pc { Pc1 = 1, Pc2 = 2 }
}

impl Pc {
    pub const fn other(self) -> Pc {
        match self {
            Pc::Pc1 => Pc::Pc2,
            Pc::Pc2 => Pc::Pc1,
        }
    }

    pub const fn index(self) -> usize {
        self as usize - 1
    }

    pub const fn from_index(i: usize) -> Pc {
        if i == 0 { Pc::Pc1 } else { Pc::Pc2 }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Pc::Pc1 => "pc1",
            Pc::Pc2 => "pc2",
        }
    }
}

wire_enum! {
    pub enum Port { Keyboard = 0, Mouse = 1, Other = 2 }
}

impl Port {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(i: usize) -> Option<Port> {
        match i {
            0 => Some(Port::Keyboard),
            1 => Some(Port::Mouse),
            2 => Some(Port::Other),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Port::Keyboard => "keyboard",
            Port::Mouse => "mouse",
            Port::Other => "other",
        }
    }

    pub fn from_name(s: &str) -> Option<Port> {
        match s {
            "keyboard" | "kb" => Some(Port::Keyboard),
            "mouse" | "ms" => Some(Port::Mouse),
            "other" | "ot" => Some(Port::Other),
            _ => None,
        }
    }
}

/// Bluetooth device slots (keyboards, mice and other HID devices bonded to the module, software.md §4.13).
pub const BT_DEVICES: usize = 4;
/// The devices the KVM routes: the three front ports, then the Bluetooth device slots ([`Device`]).
pub const DEVICES: usize = 3 + BT_DEVICES;

wire_enum! {
    /// A device the KVM takes input from and routes (software.md §4.3): a front port's, or a Bluetooth device slot's.
    /// Each has a mode and a routing in every profile.
    pub enum Device {
        Keyboard = 0,
        Mouse = 1,
        Other = 2,
        Wireless1 = 3,
        Wireless2 = 4,
        Wireless3 = 5,
        Wireless4 = 6,
    }
}

impl Device {
    pub const fn index(self) -> usize {
        self as usize
    }

    pub const fn from_index(i: usize) -> Option<Device> {
        match i {
            0 => Some(Device::Keyboard),
            1 => Some(Device::Mouse),
            2 => Some(Device::Other),
            3 => Some(Device::Wireless1),
            4 => Some(Device::Wireless2),
            5 => Some(Device::Wireless3),
            6 => Some(Device::Wireless4),
            _ => None,
        }
    }

    pub const fn port(self) -> Option<Port> {
        Port::from_index(self as usize)
    }

    pub const fn of_port(p: Port) -> Device {
        match p {
            Port::Keyboard => Device::Keyboard,
            Port::Mouse => Device::Mouse,
            Port::Other => Device::Other,
        }
    }

    /// The Bluetooth device slot (1–[`BT_DEVICES`]), if it is one.
    pub const fn wireless(self) -> Option<u8> {
        match self {
            Device::Keyboard | Device::Mouse | Device::Other => None,
            _ => Some(self as u8 - 2),
        }
    }

    /// Bluetooth device slot `slot` (1–[`BT_DEVICES`]).
    pub const fn wireless_slot(slot: u8) -> Option<Device> {
        if slot >= 1 && slot as usize <= BT_DEVICES { Device::from_index(slot as usize + 2) } else { None }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Device::Keyboard => "keyboard",
            Device::Mouse => "mouse",
            Device::Other => "other",
            Device::Wireless1 => "wireless1",
            Device::Wireless2 => "wireless2",
            Device::Wireless3 => "wireless3",
            Device::Wireless4 => "wireless4",
        }
    }

    pub fn from_name(s: &str) -> Option<Device> {
        Device::ALL.iter().copied().find(|d| d.name() == s).or_else(|| Port::from_name(s).map(Device::of_port))
    }

    /// The modes it can be set to: all of them on a front port; over Bluetooth only the KVM's own reports
    /// (`Auto` is `Normalized` there) or none.
    pub fn modes(self) -> &'static [PortMode] {
        match self.port() {
            Some(_) => PortMode::ALL,
            None => &[PortMode::Auto, PortMode::Normalized, PortMode::Off],
        }
    }

    pub fn takes(self, m: PortMode) -> bool {
        self.modes().contains(&m)
    }
}

impl From<Port> for Device {
    fn from(p: Port) -> Device {
        Device::of_port(p)
    }
}

wire_enum! {
    /// Mode setting of a device (design.md §4); where its input goes is its [`Routing`].
    pub enum PortMode {
        Auto = 0,
        Normalized = 1,
        Mirrored = 2,
        MirroredIdentity = 3,
        /// The device on the PC's own USB, through the port's router.
        Passthrough = 4,
        /// Its input goes nowhere (a front port's device stays powered and hosted).
        Off = 5,
    }
}

impl PortMode {
    pub const fn name(self) -> &'static str {
        match self {
            PortMode::Auto => "auto",
            PortMode::Normalized => "normalized",
            PortMode::Mirrored => "mirrored",
            PortMode::MirroredIdentity => "identity",
            PortMode::Passthrough => "passthrough",
            PortMode::Off => "off",
        }
    }

    pub fn from_name(s: &str) -> Option<PortMode> {
        PortMode::ALL.iter().copied().find(|m| m.name() == s)
    }

    pub const fn is_passthrough(self) -> bool {
        matches!(self, PortMode::Passthrough)
    }
}

/// Where a device's input goes (software.md §4.3): to the active target, or always to one host whichever is active.
/// A device in passthrough can only be pinned to a PC (a Bluetooth host pin makes it follow).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Routing {
    #[default]
    Follow,
    Pin(Host),
}

impl Routing {
    /// Follow, then a pin to each host in [`Host::index`] order.
    pub const ALL: [Routing; 1 + HOSTS] = [
        Routing::Follow,
        Routing::Pin(Host::Pc1),
        Routing::Pin(Host::Pc2),
        Routing::Pin(Host::Bt1),
        Routing::Pin(Host::Bt2),
        Routing::Pin(Host::Bt3),
        Routing::Pin(Host::Bt4),
    ];

    /// 0: follow; 1 + the host's index: pinned to it.
    pub const fn to_u8(self) -> u8 {
        match self {
            Routing::Follow => 0,
            Routing::Pin(h) => 1 + h.index() as u8,
        }
    }

    pub const fn from_u8(v: u8) -> Option<Routing> {
        match v {
            0 => Some(Routing::Follow),
            _ => match Host::from_index(v as usize - 1) {
                Some(h) => Some(Routing::Pin(h)),
                None => None,
            },
        }
    }

    /// `follow`, or the host's name (`pc1`, `bt3`, …).
    pub const fn name(self) -> &'static str {
        match self {
            Routing::Follow => "follow",
            Routing::Pin(h) => h.name(),
        }
    }

    pub fn from_name(s: &str) -> Option<Routing> {
        Routing::ALL.iter().copied().find(|r| r.name() == s)
    }

    pub const fn pinned(self) -> Option<Host> {
        match self {
            Routing::Follow => None,
            Routing::Pin(h) => Some(h),
        }
    }
}

wire_enum! {
    pub enum PortState {
        Empty = 0,
        Probing = 1,
        /// Enumerated by the KVM: Normalized, Mirrored or Mirrored + identity.
        Hosted = 2,
        Passthrough = 3,
        /// VBUS switched off (fault back-off or power budget).
        Unpowered = 4,
        Fault = 5,
        /// Enumerated but nothing usable (e.g. a hub on a front port).
        Unsupported = 6,
        /// The KVM's Bluetooth module: an ESP32-C3 board with the module's firmware on this port, for a KVM without the
        /// module on board (software.md §4.13). `Status.bt_module` says how it is doing.
        Module = 7,
    }
}

wire_enum! {
    /// Why Auto picked the effective mode.
    pub enum AutoReason {
        /// The port is not in Auto.
        NotAuto = 0,
        /// All inputs fit the normalized keyboard/mouse.
        FitsNormalized = 1,
        /// Vendor-page inputs, more than 16 buttons, absolute axes, unknown usage pages, or a descriptor larger than
        /// the KVM's parser holds.
        ExtraInputs = 2,
        /// HID device on the Other port.
        OtherPortHid = 3,
        /// At least one non-HID interface.
        NonHid = 4,
        XInput = 5,
        /// Not enough endpoints left for another mirror.
        EndpointBudget = 6,
        /// Another port already uses identity.
        IdentityTaken = 7,
    }
}

wire_enum! {
    pub enum LinkState {
        /// The PC's VBUS is off.
        Unpowered = 0,
        /// VBUS present, not enumerated (or soft-disconnected).
        Detached = 1,
        Enumerating = 2,
        Connected = 3,
        Suspended = 4,
        /// The AT32F405 does not answer on the SPI link.
        LinkDown = 5,
    }
}

wire_enum! {
    pub enum PowerSource { PwrPort = 0, PcBus = 1 }
}

wire_enum! {
    /// Current advertised by a USB-C source through CC.
    pub enum Advertised { None = 0, Default = 1, A1_5 = 2, A3_0 = 3 }
}

impl Advertised {
    pub const fn milliamps(self) -> u16 {
        match self {
            Advertised::None => 0,
            Advertised::Default => 500,
            Advertised::A1_5 => 1500,
            Advertised::A3_0 => 3000,
        }
    }

    /// Classify the voltage across a 5.1 kΩ Rd (USB Type-C spec, table 4-36, with margins).
    pub const fn from_cc_mv(mv: u16) -> Advertised {
        match mv {
            0..200 => Advertised::None,
            200..660 => Advertised::Default,
            660..1230 => Advertised::A1_5,
            _ => Advertised::A3_0,
        }
    }
}

wire_enum! {
    pub enum SwitchSource {
        Key = 0,
        Hotkey = 1,
        Software = 2,
        Boot = 3,
        /// Automatic: the active PC was switched off, or only the other one is running (config `AUTO_SELECT`).
        Power = 4,
        /// Screen hop: the pointer was pushed across a screen edge onto the other PC's screen.
        Edge = 5,
    }
}

wire_enum! {
    pub enum Speed { Unknown = 0, Low = 1, Full = 2, High = 3 }
}

wire_enum! {
    pub enum Target {
        Pc1 = 1,
        Pc2 = 2,
        /// The other PC; from a Bluetooth host, back to the PC that was active before it.
        Toggle = 3,
        /// The next target in the order PC1, PC2, Bluetooth hosts 1–4, skipping hosts that aren't ready.
        Next = 4,
        /// Bluetooth host slot 1–4 (boards with the Bluetooth module).
        Bt1 = 0x11,
        Bt2 = 0x12,
        Bt3 = 0x13,
        Bt4 = 0x14,
    }
}

impl Target {
    /// The target for Bluetooth host slot `host` (1–[`BT_HOSTS`]).
    pub const fn bt(host: u8) -> Option<Target> {
        match host {
            1 => Some(Target::Bt1),
            2 => Some(Target::Bt2),
            3 => Some(Target::Bt3),
            4 => Some(Target::Bt4),
            _ => None,
        }
    }

    /// The Bluetooth host slot this target names, if it names one.
    pub const fn bt_host(self) -> Option<u8> {
        match self {
            Target::Bt1 | Target::Bt2 | Target::Bt3 | Target::Bt4 => Some(self as u8 - 0x10),
            _ => None,
        }
    }

    pub fn from_name(s: &str) -> Option<Target> {
        match s {
            "pc1" | "1" => Some(Target::Pc1),
            "pc2" | "2" => Some(Target::Pc2),
            "toggle" | "other" => Some(Target::Toggle),
            "next" => Some(Target::Next),
            "bt1" => Some(Target::Bt1),
            "bt2" => Some(Target::Bt2),
            "bt3" => Some(Target::Bt3),
            "bt4" => Some(Target::Bt4),
            _ => None,
        }
    }
}

/// Bluetooth host slots (`Target::Bt1`–`Bt4`, `kvm_proto::link::bt::MAX_HOSTS`).
pub const BT_HOSTS: usize = 4;

wire_enum! {
    /// A computer the KVM serves: a PC on its USB port, or a Bluetooth host slot. Its values are
    /// [`Target`]'s.
    pub enum Host { Pc1 = 1, Pc2 = 2, Bt1 = 0x11, Bt2 = 0x12, Bt3 = 0x13, Bt4 = 0x14 }
}

/// The hosts there are ([`Host::index`]): PC1, PC2 and the Bluetooth host slots.
pub const HOSTS: usize = 2 + BT_HOSTS;

impl Host {
    pub const fn pc(pc: Pc) -> Host {
        match pc {
            Pc::Pc1 => Host::Pc1,
            Pc::Pc2 => Host::Pc2,
        }
    }

    /// Bluetooth host slot `slot` (1–[`BT_HOSTS`]).
    pub const fn bt(slot: u8) -> Option<Host> {
        match slot {
            1 => Some(Host::Bt1),
            2 => Some(Host::Bt2),
            3 => Some(Host::Bt3),
            4 => Some(Host::Bt4),
            _ => None,
        }
    }

    /// The PC, if it is one.
    pub const fn as_pc(self) -> Option<Pc> {
        match self {
            Host::Pc1 => Some(Pc::Pc1),
            Host::Pc2 => Some(Pc::Pc2),
            _ => None,
        }
    }

    /// The Bluetooth host slot (1–4), if it is one.
    pub const fn bt_slot(self) -> Option<u8> {
        match self {
            Host::Pc1 | Host::Pc2 => None,
            _ => Some(self as u8 - 0x10),
        }
    }

    /// 0–5: PC1, PC2, Bluetooth host slots 1–4.
    pub const fn index(self) -> usize {
        match self.bt_slot() {
            None => self as usize - 1,
            Some(n) => 1 + n as usize,
        }
    }

    pub const fn from_index(i: usize) -> Option<Host> {
        match i {
            0 => Some(Host::Pc1),
            1 => Some(Host::Pc2),
            _ if i < HOSTS => Host::bt(i as u8 - 1),
            _ => None,
        }
    }

    pub const fn target(self) -> Target {
        match self {
            Host::Pc1 => Target::Pc1,
            Host::Pc2 => Target::Pc2,
            Host::Bt1 => Target::Bt1,
            Host::Bt2 => Target::Bt2,
            Host::Bt3 => Target::Bt3,
            Host::Bt4 => Target::Bt4,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Host::Pc1 => "pc1",
            Host::Pc2 => "pc2",
            Host::Bt1 => "bt1",
            Host::Bt2 => "bt2",
            Host::Bt3 => "bt3",
            Host::Bt4 => "bt4",
        }
    }

    pub fn from_name(s: &str) -> Option<Host> {
        Host::ALL.iter().copied().find(|h| h.name() == s)
    }
}

impl From<Pc> for Host {
    fn from(pc: Pc) -> Host {
        Host::pc(pc)
    }
}

wire_enum! {
    pub enum StringKind { Manufacturer = 0, Product = 1, Serial = 2 }
}

wire_enum! {
    pub enum RebootTarget {
        Normal = 0,
        /// RP2354B ROM bootloader (UF2 drive on the PWR/SERVICE port).
        Bootloader = 1,
        /// Hold AT32 #1 / #2 in its ROM bootloader (USB DFU towards its PC).
        At32Dfu1 = 2,
        At32Dfu2 = 3,
        /// Forget the failed attempts at programming the AT32F405s, then restart, so that both get up to 5 more
        /// (software.md §4.9).
        At32Retry = 4,
    }
}

wire_enum! {
    /// What an update writes: the RP2354B's firmware (into the A/B partition it doesn't run from; it waits for the
    /// key), or the Bluetooth module's ([`crate::modimage`]), which the RP2354B passes on to the module as it arrives.
    /// That needs no key: the RP2354B takes only the module image its own firmware was built with (`caps::BT_UPDATE`).
    pub enum FwTarget { Rp2354 = 0, Module = 1 }
}

wire_enum! {
    /// The virtual gamepad the PCs get, for what the keymaps send it (hid_mapping.md §6.1): none (a phantom controller
    /// confuses games), a HID gamepad (a report ID on the NKRO interface: 32 buttons, a hat, 8 axes), or an XInput
    /// controller (an interface of its own, one mirror fewer).
    pub enum GamepadOut { Off = 0, Hid = 1, XInput = 2 }
}

// `wire_enum!` takes no derives beyond its own.
#[allow(clippy::derivable_impls)]
impl Default for GamepadOut {
    fn default() -> Self {
        GamepadOut::Off
    }
}

/// Keymaps (each profile uses one or none), layers per keymap, and the controls a keymap maps (`kvm_map::pos`):
/// keyboard usages 0–231, mouse buttons, gamepad buttons, the hat, axis directions.
pub const KEYMAPS: usize = 4;
pub const KEYMAP_LAYERS: usize = 8;
/// Keycodes per `GetKeycodes` / `SetKeycodes`.
pub const KEYCODES_PER: usize = 28;
/// Axis rules (6 bytes each, `kvm_map::axes::Rule`) per `Response::AxisRules`.
pub const RULES_PER: usize = 8;
pub const RULE_LEN: usize = 6;

/// Bytes per entry of the keymaps' shared entries (`Request::GetMapEntry`): Vial's layout, 10 at most.
pub const MAP_ENTRY_MAX: usize = 10;
/// Macro buffer bytes per `Request::SetMacros` and `Response::Macros`.
pub const MACRO_PIECE: usize = 56;
/// Keyboard layouts a host can be set to (`kvm_hid::layouts::Layout::ALL`).
pub const LAYOUTS: u8 = 13;
/// Field overrides a device can have (`Request::SetOverride`).
pub const MAX_OVERRIDES: usize = 8;
/// Raw report bytes per `Event::Report`.
pub const REPORT_PIECE: usize = 56;

/// A field of a device's reports made a control (hid_mapping.md §9): bits its descriptor doesn't declare, or
/// declares as something else, read as a gamepad button or axis. Kept with the device's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct FieldOverride {
    /// The interface (a Bluetooth device's is 0) and report ID (0: the interface has none) whose reports carry it.
    pub iface: u8,
    pub report_id: u8,
    /// Its first bit, counted from the report's first byte after the report ID, and how many (1–16).
    pub bit: u16,
    pub bits: u8,
    /// [`field`] flags.
    pub flags: u8,
    /// What it becomes: gamepad button 1–32 as 0–31, or [`field::AXIS`] + gamepad axis 0–9 (`kvm_hid::gamepad::axis`).
    pub target: u8,
}

pub mod field {
    /// The value is signed (two's complement); otherwise it runs from 0.
    pub const SIGNED: u8 = 1 << 0;
    /// Read backwards: a button pressed while the bit is 0, an axis turned round.
    pub const INVERT: u8 = 1 << 1;
    pub const AXIS: u8 = 0x40;
}

/// What a host is offered and what it took (`Response::Outputs`): bit per output.
pub mod outputs {
    pub const KEYBOARD: u8 = 1 << 0;
    pub const MOUSE: u8 = 1 << 1;
    pub const MEDIA: u8 = 1 << 2;
    /// The absolute pointer (screen hop).
    pub const POINTER: u8 = 1 << 3;
    pub const GAMEPAD: u8 = 1 << 4;
    /// The virtual gamepad as an XInput controller.
    pub const XINPUT: u8 = 1 << 5;
    pub const VIAL: u8 = 1 << 6;
    /// Mirrored devices.
    pub const MIRRORS: u8 = 1 << 7;
}

/// What a device has, for the host software's picture of it (`Response::Controls`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Controls {
    pub keyboard: bool,
    /// Mouse buttons 1–16, gamepad buttons 1–32 (bit n = button n + 1).
    pub mouse_buttons: u16,
    pub pad_buttons: u32,
    /// Mouse X/Y, wheel, pan.
    pub mouse_axes: u8,
    /// Gamepad axes X, Y, Z, Rx, Ry, Rz, slider, dial, brake, accelerator.
    pub pad_axes: u16,
    pub hat: bool,
    /// Identifies the device for its calibration (and the host software's names for its controls): VID/PID and
    /// interface, or the Bluetooth address, hashed; 0 = none.
    pub identity: u32,
}

/// One gamepad axis's calibration, in normalized units (−32767…32767; a trigger 0…32767).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct AxisCal {
    pub min: i16,
    pub centre: i16,
    pub max: i16,
    /// Around the centre, in 256ths of the half range, read as the centre.
    pub deadzone: u8,
}

impl AxisCal {
    pub const NONE: AxisCal = AxisCal { min: -32767, centre: 0, max: 32767, deadzone: 0 };
}

wire_enum! {
    pub enum FwState { Idle = 0, Receiving = 1, AwaitingKey = 2, Writing = 3, Done = 4, Failed = 5 }
}

wire_enum! {
    pub enum FaultKind {
        OverCurrent = 0,
        EnumerationFailed = 1,
        LinkDown = 2,
        PowerBudget = 3,
        HubError = 4,
        /// A wireless host changes its notification settings so often that the module stopped writing them to flash
        /// for this run (its wear guard): they are kept in RAM.
        BtSettingsHeld = 5,
    }
}

wire_enum! {
    pub enum ErrorCode {
        Unknown = 1,
        Malformed = 2,
        /// Software switching or configuration is disabled at the device.
        Locked = 3,
        Busy = 4,
        Unsupported = 5,
        InvalidValue = 6,
        /// Press the key on the KVM, then retry.
        NeedsKeyPress = 7,
        NoDevice = 8,
        /// Only the active PC may ask for this (learning a hotkey).
        NotActive = 9,
        /// A Bluetooth host that may not control the KVM asked (software.md §8.1, "Over Bluetooth"): it asks for that
        /// with `AskControl`, confirmed by a tap on the KVM's key.
        NotAllowed = 10,
    }
}

wire_enum! {
    pub enum HotkeyKind { None = 0, DoubleTap = 1, Combo = 2 }
}

wire_enum! {
    /// How learning a hotkey ended (`Event::HotkeyLearned`).
    pub enum LearnEnd {
        Learned = 0,
        /// Nothing that makes a hotkey was pressed in time.
        TimedOut = 1,
        /// Stopped from software, or by a switch to the other PC.
        Cancelled = 2,
    }
}

wire_enum! {
    /// PC-side interrupt endpoint interval.
    pub enum Polling { Us125 = 0, Ms1 = 1 }
}

wire_enum! {
    /// Whether the PCs see the Vial interface (keymap editor, software.md §4.11). It takes one IN endpoint.
    pub enum VialMode {
        Off = 0,
        /// Always present: mirrored devices get one IN endpoint less.
        On = 1,
        /// Present while an IN endpoint is spare: mirrored devices come first.
        Auto = 2,
    }
}

impl VialMode {
    pub const fn name(self) -> &'static str {
        match self {
            VialMode::Off => "off",
            VialMode::On => "on",
            VialMode::Auto => "auto",
        }
    }

    pub fn from_name(s: &str) -> Option<VialMode> {
        match s {
            "off" => Some(VialMode::Off),
            "on" => Some(VialMode::On),
            "auto" => Some(VialMode::Auto),
            _ => None,
        }
    }
}

wire_enum! {
    /// What a tap or a long press on the KVM's key does (design.md §7).
    pub enum KeyAction {
        None = 0,
        /// Switch to the other PC.
        TogglePc = 1,
        /// Make the next profile in use active, after the last one the first.
        NextProfile = 2,
        /// Cycle the Other port's mode and routing: Auto → passthrough → passthrough pinned to PC1 → to PC2.
        OtherPortMode = 3,
        /// The next target: PC1, PC2, then each Bluetooth host that is ready.
        NextTarget = 4,
    }
}

impl KeyAction {
    pub const fn name(self) -> &'static str {
        match self {
            KeyAction::None => "none",
            KeyAction::TogglePc => "toggle-pc",
            KeyAction::NextProfile => "next-profile",
            KeyAction::OtherPortMode => "other-port",
            KeyAction::NextTarget => "next-target",
        }
    }

    pub fn from_name(s: &str) -> Option<KeyAction> {
        KeyAction::ALL.iter().copied().find(|a| a.name() == s)
    }
}

/// Profiles the KVM stores: each a name, and every device's mode and routing (design.md §4.1).
pub const MAX_PROFILES: usize = 8;
pub const PROFILE_NAME_MAX: usize = 12;

/// A profile's name: UTF-8, at most [`PROFILE_NAME_MAX`] bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ProfileName {
    len: u8,
    bytes: [u8; PROFILE_NAME_MAX],
}

impl ProfileName {
    /// `s`, cut to [`PROFILE_NAME_MAX`] bytes at a character boundary.
    pub const fn new(s: &str) -> Self {
        let b = s.as_bytes();
        let mut len = if b.len() < PROFILE_NAME_MAX { b.len() } else { PROFILE_NAME_MAX };
        while len > 0 && len < b.len() && b[len] & 0xC0 == 0x80 {
            len -= 1;
        }
        let mut bytes = [0u8; PROFILE_NAME_MAX];
        let mut k = 0;
        while k < len {
            bytes[k] = b[k];
            k += 1;
        }
        Self { len: len as u8, bytes }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
}

/// Screen-hop arrangements the KVM keeps: each a name and [`MAX_SCREENS`] screens; a profile uses one of
/// them or none (`ConfigValue::ProfileArrangement`).
pub const MAX_ARRANGEMENTS: usize = 4;
/// Screen slots of an arrangement (`ConfigValue::Screen`).
pub const MAX_SCREENS: usize = 8;
/// Screen sizes the KVM accepts, in the PC's pixels, and display scaling in percent.
pub const SCREEN_PX: core::ops::RangeInclusive<u16> = 160..=16384;
pub const SCREEN_SCALE: core::ops::RangeInclusive<u16> = 100..=400;
/// Screen hop: the pointer speed setting (0 = the default), the push an edge to another PC takes (logical pixels),
/// and the stretch at each end of such an edge where it never hops.
pub const HOP_SPEED: core::ops::RangeInclusive<i8> = -10..=10;
pub const HOP_RESISTANCE_MAX: u16 = 2000;
pub const HOP_CORNER_MAX: u16 = 500;

wire_enum! {
    /// Pointer acceleration while screen hop moves the pointer: the PC's own acceleration doesn't apply to an absolute
    /// pointer, so the KVM accelerates.
    pub enum Accel {
        /// Speed only, no acceleration.
        Flat = 0,
        /// The five-point curve of Windows' "Enhance pointer precision".
        Precision = 1,
        /// The adaptive profile of libinput, as on Linux desktops.
        Adaptive = 2,
    }
}

impl Accel {
    pub const fn name(self) -> &'static str {
        match self {
            Accel::Flat => "flat",
            Accel::Precision => "precision",
            Accel::Adaptive => "adaptive",
        }
    }

    pub fn from_name(s: &str) -> Option<Accel> {
        Accel::ALL.iter().copied().find(|a| a.name() == s)
    }
}

wire_enum! {
    /// Where a PC puts the absolute pointer's 0–32767.
    pub enum Mapping {
        /// Across all its screens, their bounding box (Linux desktops).
        Desktop = 0,
        /// On its main screen only, the one at 0,0 of its own arrangement (Windows 10 and later). The KVM reaches its
        /// other screens with relative motion.
        MainScreen = 1,
    }
}

impl Mapping {
    pub const fn name(self) -> &'static str {
        match self {
            Mapping::Desktop => "desktop",
            Mapping::MainScreen => "main-screen",
        }
    }

    pub fn from_name(s: &str) -> Option<Mapping> {
        Mapping::ALL.iter().copied().find(|m| m.name() == s)
    }
}

wire_enum! {
    /// What a PC's desktop coordinates count ([`Screen::desk`], `ConfigValue::HopUnits`).
    pub enum Units {
        /// Physical pixels: each screen covers its resolution (Windows, X11).
        Physical = 0,
        /// Logical pixels: each screen covers its size in the arrangement, its resolution over its scaling (Wayland
        /// desktops: KDE Plasma, GNOME).
        Logical = 1,
    }
}

impl Units {
    pub const fn name(self) -> &'static str {
        match self {
            Units::Physical => "physical",
            Units::Logical => "logical",
        }
    }

    pub fn from_name(s: &str) -> Option<Units> {
        Units::ALL.iter().copied().find(|u| u.name() == s)
    }
}

/// Physical pixels.
impl Default for Units {
    fn default() -> Self {
        Units::Physical
    }
}

wire_enum! {
    /// How screen hop moves a Bluetooth host's pointer.
    pub enum BtPointer {
        /// The absolute pointer (report 4 of the module's HID service): exactly where the KVM's pointer is, as on the
        /// PCs (most desktop systems).
        Absolute = 0,
        /// The relative mouse only, for a host that takes no absolute pointer (some tablets and phones): pushed into
        /// its top-left corner when the pointer comes onto it, then moved by the difference. The host accelerates that
        /// motion, so its cursor drifts from the KVM's pointer until it is pushed against an edge again.
        Relative = 1,
    }
}

wire_enum! {
    /// The 5 V the KVM runs from (the PWR port, or the higher of the PCs' through their ideal diodes), as measured
    /// (`Status::supply`).
    pub enum Supply {
        /// Not judged: no reading yet.
        Unknown = 0,
        /// At least 4.5 V: the source keeps up with the load, whatever the devices declare.
        Good = 1,
        /// Under 4.5 V for 2 s or more: the source (or its cable) can't quite keep up.
        Sags = 2,
        /// Under 4.4 V for half a second or more, below what USB devices are required to work at: they may drop out.
        Low = 3,
    }
}

wire_enum! {
    /// Which of a PC's screens its host software places the cursor on (`Request::PlaceCursor`): where it
    /// does, screen hop sends it the position (`Event::PlaceCursor`) instead of a report of the pointer interfaces.
    pub enum Placement {
        /// None: the PC's mapping alone (no host software, or it can't place now).
        Off = 0,
        /// The screens its mapping reaches with relative motion only (Windows' screens other than its main one); the
        /// absolute pointer keeps the others.
        Others = 1,
        /// All of them (macOS, whose mapping of an absolute pointer onto several displays isn't known).
        All = 2,
    }
}

impl BtPointer {
    pub const fn name(self) -> &'static str {
        match self {
            BtPointer::Absolute => "absolute",
            BtPointer::Relative => "relative",
        }
    }

    pub fn from_name(s: &str) -> Option<BtPointer> {
        BtPointer::ALL.iter().copied().find(|p| p.name() == s)
    }
}

wire_enum! {
    /// Whether screen hop moves the pointer now (`Status.hop`), and if not, why not.
    pub enum HopState {
        /// Not on in the active profile.
        Off = 0,
        Active = 1,
        /// The absolute pointer (`ConfigValue::AbsPointer`) is off.
        NoAbsolute = 2,
        /// The device on the Mouse port doesn't run Normalized.
        NotNormalized = 3,
        /// The active host (a PC or a Bluetooth host) has no screen in the active profile's arrangement.
        NoScreens = 4,
        /// The active PC uses the boot protocol (its firmware setup, before the OS runs).
        BootProtocol = 5,
    }
}

/// One screen of the screen-hop arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Screen {
    /// The host it belongs to, a PC or a Bluetooth host; None: the slot is free.
    pub host: Option<Host>,
    /// Resolution, in the PC's pixels.
    pub w: u16,
    pub h: u16,
    /// The PC's display scaling, in percent: in the arrangement the screen is w×100/scale by h×100/scale.
    pub scale: u16,
    /// Its top-left corner in the PC's own display arrangement, in the PC's pixels.
    pub desk: [i16; 2],
    /// Its top-left corner in the arrangement, in logical pixels.
    pub at: [i16; 2],
    /// Screen hop leaves it alone: its PC has it, but another PC shows on it (a monitor cabled to both).
    /// It stands where that PC's screen does, and still counts for its PC's desktop: the PC spreads the absolute
    /// pointer across it too.
    pub off: bool,
    /// Its place on its PC (`desk`) is the PC's own, detected or typed in, not derived from the arrangement (protocol
    /// 8; the KVM only stores it).
    pub fixed: bool,
}

impl Screen {
    pub const NONE: Screen =
        Screen { host: None, w: 0, h: 0, scale: 100, desk: [0, 0], at: [0, 0], off: false, fixed: false };

    pub const fn new(host: Host, w: u16, h: u16, at: [i16; 2]) -> Screen {
        Screen { host: Some(host), w, h, scale: 100, desk: [0, 0], at, off: false, fixed: false }
    }

    /// Its size in the arrangement (logical pixels).
    pub fn size(&self) -> [i32; 2] {
        let scale = self.scale.max(1) as i32;
        [self.w as i32 * 100 / scale, self.h as i32 * 100 / scale]
    }

    /// Its size on its PC's desktop, in the desktop's [`Units`]: its resolution in physical pixels, its size in the
    /// arrangement in logical ones.
    pub fn desk_size(&self, units: Units) -> [i32; 2] {
        match units {
            Units::Physical => [self.w as i32, self.h as i32],
            Units::Logical => self.size(),
        }
    }

    /// The desktop's units per logical pixel of the arrangement on this screen: its scaling in physical pixels, 1 in
    /// logical ones.
    pub fn desk_per_logical(&self, units: Units) -> f32 {
        match units {
            Units::Physical => self.scale.max(1) as f32 / 100.0,
            Units::Logical => 1.0,
        }
    }

    /// A screen of a host with a size and scaling the KVM accepts, or a free slot.
    pub fn is_valid(&self) -> bool {
        self.host.is_none()
            || (SCREEN_PX.contains(&self.w) && SCREEN_PX.contains(&self.h) && SCREEN_SCALE.contains(&self.scale))
    }
}

/// `PortInfo.classes` bits.
pub mod class {
    pub const KEYBOARD: u16 = 1 << 0;
    pub const MOUSE: u16 = 1 << 1;
    pub const CONSUMER: u16 = 1 << 2;
    pub const GAMEPAD: u16 = 1 << 3;
    /// HID inputs on a vendor-defined usage page.
    pub const VENDOR_HID: u16 = 1 << 4;
    pub const NON_HID: u16 = 1 << 5;
    pub const XINPUT: u16 = 1 << 6;
    /// A vendor command channel (feature/output reports on a vendor page, e.g. HID++).
    pub const VENDOR_CHANNEL: u16 = 1 << 7;
    pub const AUDIO: u16 = 1 << 8;
    pub const VIDEO: u16 = 1 << 9;
    pub const STORAGE: u16 = 1 << 10;
    pub const HUB: u16 = 1 << 11;
    pub const OTHER_HID: u16 = 1 << 12;
}

/// Bits of a screen slot's host byte above the host (the host, a [`Host`](super::Host), in bits 0–5).
pub mod screen_flags {
    /// [`Screen::off`](super::Screen::off).
    pub const OFF: u8 = 1 << 7;
    /// [`Screen::fixed`](super::Screen::fixed).
    pub const FIXED: u8 = 1 << 6;
}

/// `Status.flags` bits.
pub mod status_flags {
    pub const SW_SWITCH_ALLOWED: u8 = 1 << 0;
    pub const SW_CONFIG_ALLOWED: u8 = 1 << 1;
    pub const CONFIG_DIRTY: u8 = 1 << 2;
    pub const UPDATE_ARMED: u8 = 1 << 3;
    pub const SELFTEST_FAILED: u8 = 1 << 4;
    /// The 5 V the KVM runs from sags under the load, as measured ([`super::Status::supply`] `Sags` or `Low`).
    pub const SUPPLY_SAGS: u8 = 1 << 5;
    /// No PC uses the KVM: the LEDs are dark and front ports are off (software.md §4.12).
    pub const STANDBY: u8 = 1 << 6;
    /// In standby, the active PC sleeps and a key press wakes it; the keyboard and mouse ports stay powered.
    pub const STANDBY_WAKE: u8 = 1 << 7;
}

/// `Status.selftest` bits: what the boot-time self-test found wrong (design.md §10, step 1.5). The self-test passed
/// when no bit but possibly [`selftest::RUNNING`] is set.
pub mod selftest {
    /// Hub-0 did not enumerate on the RP2354B's USB host.
    pub const HUB0: u32 = 1 << 0;
    /// No link to AT32F405 #1 / #2.
    pub const LINK1: u32 = 1 << 1;
    pub const LINK2: u32 = 1 << 2;
    /// The PC supplies VBUS, but its hub (Hub-1 / Hub-2) never reset the AT32F405. Clears once it does.
    pub const HUB1: u32 = 1 << 3;
    pub const HUB2: u32 = 1 << 4;
    /// A front-port power switch reported a fault when first switched on, or its FAULT line is stuck low.
    pub const PORT_KB: u32 = 1 << 5;
    pub const PORT_MS: u32 = 1 << 6;
    pub const PORT_OT: u32 = 1 << 7;
    /// PWR-port VBUS or CC readings out of range, or not matching the power-path status.
    pub const POWER: u32 = 1 << 8;
    /// Reprogramming AT32F405 #1 / #2 with the embedded firmware failed (this boot; tried again at the next one).
    pub const AT32_FW1: u32 = 1 << 9;
    pub const AT32_FW2: u32 = 1 << 10;
    /// Programming AT32F405 #1 / #2 failed 5 times in a row: no more attempts until the embedded image changes, the
    /// KVM's key is held at power-up, or `RebootTarget::At32Retry`.
    pub const AT32_GAVE_UP1: u32 = 1 << 11;
    pub const AT32_GAVE_UP2: u32 = 1 << 12;
    /// AT32F405 #1 / #2 runs without its 12 MHz crystal (on its internal oscillator): no USB to that PC.
    pub const AT32_XTAL1: u32 = 1 << 13;
    pub const AT32_XTAL2: u32 = 1 << 14;
    /// Still running; the other bits are provisional.
    pub const RUNNING: u32 = 1 << 15;
    /// The Bluetooth module (boards 0.16.0 on, when fitted, or a dongle) doesn't run this firmware's version, or none:
    /// Bluetooth is off until the host software sends it (`FwTarget::Module`). Bits 16 and up travel after the
    /// Bluetooth fields of `Status`.
    pub const BT_FW: u32 = 1 << 16;
    pub const BT: u32 = BT_FW;
    pub const PORTS: [u32; 3] = [PORT_KB, PORT_MS, PORT_OT];
    /// Everything that concerns PC `k`'s side (its link, hub and AT32F405 firmware).
    pub const PCS: [u32; 2] =
        [LINK1 | HUB1 | AT32_FW1 | AT32_GAVE_UP1 | AT32_XTAL1, LINK2 | HUB2 | AT32_FW2 | AT32_GAVE_UP2 | AT32_XTAL2];
    /// Faults that blink the key red: a part of the KVM doesn't work, and unplugging a device won't fix it. A front
    /// port's power switch fault isn't among them: that port's LED shows it, and it clears with the device. Nor is a
    /// module that is still being programmed.
    pub const ON_KEY: u32 = HUB0 | POWER | PCS[0] | PCS[1] | BT;
}

/// Diagnostics (`Request::GetDiag`, software.md §8.4): the KVM's measurements, each graded by the firmware.
///
/// Every reading is one [`Item`]: an [`id`], a [`Grade`] and an `i32` value whose meaning [`meta`] gives. The firmware
/// grades, because only it knows the board's limits and the context (a PC that is off has no VBUS, and that is fine).
/// A host that doesn't know an id still shows the grade and the raw number.
pub mod diag {
    use crate::wire::wire_enum;

    wire_enum! {
        /// How a reading looks. Hosts show `Ok` green, `Warn` orange, `Bad` red and `Info` without a colour.
        pub enum Grade {
            /// Nothing to judge: a count, a setting, or a part that is off.
            Info = 0,
            Ok = 1,
            /// Works, but not as it should (a sagging supply, a full-speed PC link).
            Warn = 2,
            /// Broken, or out of the part's limits.
            Bad = 3,
        }
    }

    impl Grade {
        /// The worse of two grades (`Info` counts as better than `Ok`).
        pub const fn worse(self, other: Grade) -> Grade {
            if other as u8 > self as u8 { other } else { self }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[cfg_attr(feature = "defmt", derive(defmt::Format))]
    pub struct Item {
        pub id: u8,
        pub grade: Grade,
        pub value: i32,
    }

    impl Item {
        pub const fn new(id: u8, grade: Grade, value: i32) -> Self {
            Self { id, grade, value }
        }
    }

    /// Items per `Response::Diag` (6 bytes each).
    pub const PER_PAGE: usize = 9;
    // A page is one report: type, request id, page, pages, count, and the items.
    const _: () = assert!(5 + 6 * PER_PAGE <= super::REPORT_LEN);
    /// Most items the firmware sends: 8 pages.
    pub const MAX_ITEMS: usize = 8 * PER_PAGE;

    wire_enum! {
        /// Why the RP2354B last started (POWMAN `CHIP_RESET`).
        pub enum ResetCause {
            /// No flag set: a reset from software (`odkvmctl reboot`, a configuration change that restarts).
            Software = 0,
            PowerOn = 1,
            /// The supply dipped below the brown-out level.
            BrownOut = 2,
            /// The power manager saw a supply glitch.
            Glitch = 3,
            /// The RUN pin was pulled low.
            RunPin = 4,
            Watchdog = 5,
            Debugger = 6,
        }
    }

    wire_enum! {
        /// What an AT32F405 runs from.
        pub enum ClockSource { Unknown = 0, Crystal = 1, Internal = 2 }
    }

    wire_enum! {
        /// Whether the RP2354B takes firmware updates over USB (`caps::FW_UPDATE`): what its check of the partition
        /// table found at boot. The item's low byte; the bits above it hold what the check read (`Unit::FwUpdates`).
        pub enum FwUpdates {
            /// No partition table (a board flashed by a debug probe): updates are not offered, as intended.
            NoTable = 0,
            /// On; the firmware runs from partition A or B, the update goes into the other one.
            PartitionA = 1,
            PartitionB = 2,
            /// Off: the boot ROM didn't return the partition table.
            ReadFailed = 3,
            /// Off: another partition table than the KVM's (not three partitions, or flags missing).
            OtherTable = 4,
            /// Off: three partitions, but not where the firmware's own layout has them, or B not linked to A.
            OtherLayout = 5,
            /// Off: neither the address translation nor the boot ROM says which partition runs.
            UnknownPartition = 6,
        }
    }

    wire_enum! {
        /// The Bluetooth module, as the RP2354B sees it (boards 0.16.0 on, when fitted).
        pub enum BtModule {
            /// Not fitted, or a board before 0.16.0, and no dongle: the Bluetooth items are left out.
            Absent = 0,
            /// The link is up and the module runs the RP2354B's version.
            Up = 1,
            /// Being programmed with the firmware the host software streams (`FwTarget::Module`).
            Programming = 2,
            /// Waiting for its `Hello`: just reset, or just programmed.
            Starting = 3,
            /// It runs another version than the RP2354B's, or none (blank, or a programming cut short): Bluetooth is
            /// off until the host software sends this version's (`FwTarget::Module`).
            NeedsUpdate = 4,
        }
    }

    /// The item ids. The high nibble is the [`Group`]; the arrays list one item per PC or per front port.
    pub mod id {
        pub const UPTIME: u8 = 0x01;
        pub const RESET_CAUSE: u8 = 0x02;
        pub const RP_TEMP: u8 = 0x03;
        pub const SELFTEST: u8 = 0x04;
        pub const DROPPED: u8 = 0x05;
        /// Whether the RP2354B takes firmware updates over USB, and from which partition it runs ([`FwUpdates`]).
        pub const FW_UPDATES: u8 = 0x06;

        pub const XOSC: u8 = 0x10;
        pub const PLL_SYS: u8 = 0x11;
        pub const PLL_USB: u8 = 0x12;
        pub const CLK_SYS: u8 = 0x13;
        pub const CLK_USB: u8 = 0x14;
        pub const CLK_PERI: u8 = 0x15;
        pub const CLK_ADC: u8 = 0x16;
        pub const ROSC: u8 = 0x17;
        pub const AT1_CLOCK: u8 = 0x18;
        pub const AT2_CLOCK: u8 = 0x19;

        pub const HUB0_SPEED: u8 = 0x20;
        pub const HUB0_UP: u8 = 0x21;
        pub const PHY_SWAP: u8 = 0x22;
        pub const KB_SPEED: u8 = 0x23;
        pub const MS_SPEED: u8 = 0x24;
        pub const OT_SPEED: u8 = 0x25;
        pub const PC1_SPEED: u8 = 0x26;
        pub const PC2_SPEED: u8 = 0x27;
        pub const PC1_LINK: u8 = 0x28;
        pub const PC2_LINK: u8 = 0x29;
        pub const PC1_RESETS: u8 = 0x2A;
        pub const PC2_RESETS: u8 = 0x2B;
        pub const LINK1_ERRORS: u8 = 0x2C;
        pub const LINK2_ERRORS: u8 = 0x2D;
        /// Hub-0 status changes the hub class couldn't handle (left set, they hid the ports after them); cleared by the
        /// firmware.
        pub const HUB0_ERRORS: u8 = 0x2E;
        /// Front-port devices whose enumeration or probe failed (each is retried with a VBUS cycle, a few times).
        pub const ENUM_FAILS: u8 = 0x2F;

        pub const PWR_VBUS: u8 = 0x30;
        pub const PWR_CC: u8 = 0x31;
        pub const PC1_VBUS: u8 = 0x32;
        pub const PC2_VBUS: u8 = 0x33;
        pub const PC1_CC: u8 = 0x34;
        pub const PC2_CC: u8 = 0x35;
        /// The 3.3 V rail as each AT32F405 measures it (against its 1.20 V reference).
        pub const AT1_3V3: u8 = 0x36;
        pub const AT2_3V3: u8 = 0x37;
        pub const BUDGET: u8 = 0x38;
        pub const KB_FAULTS: u8 = 0x39;
        pub const MS_FAULTS: u8 = 0x3A;
        pub const OT_FAULTS: u8 = 0x3B;
        /// The lowest 5 V the KVM ran from in the last minute, graded by `Status::supply`.
        pub const SUPPLY: u8 = 0x3C;
        /// Since start: the 100 ms windows whose millisecond minimum fell under 4.4 V (a device's inrush, a heavy
        /// load); a count, not a judgement.
        pub const SUPPLY_DIPS: u8 = 0x3D;
        /// Since start: how often each PC's 5 V went away for less than 3 s (a port that cut out, or a reboot).
        pub const PC1_DROPOUTS: u8 = 0x3E;
        pub const PC2_DROPOUTS: u8 = 0x3F;

        pub const AT1_TEMP: u8 = 0x40;
        pub const AT2_TEMP: u8 = 0x41;
        pub const AT1_FW: u8 = 0x42;
        pub const AT2_FW: u8 = 0x43;

        /// Bluetooth (only with the module fitted).
        pub const BT_MODULE: u8 = 0x50;
        pub const BT_FW: u8 = 0x51;
        pub const BT_LINK_ERRORS: u8 = 0x52;
        pub const BT_HOSTS: u8 = 0x53;
        /// Wireless hosts whose notification settings the module keeps in RAM only, for this run (its flash wear
        /// guard, `FaultKind::BtSettingsHeld`); only while there are any.
        pub const BT_SETTINGS_HELD: u8 = 0x54;

        /// The module as a board on a front port (software.md §4.13), once one was seen since start: for bring-up.
        pub const BOARD_SESSIONS: u8 = 0x58;
        /// The last steps it went through, a hexadecimal digit each, the newest last (`bt::step` in the firmware).
        pub const BOARD_STEPS: u8 = 0x59;
        /// The ROM bootloader's last error (`espisp::Error`, coded as `espisp::code`).
        pub const BOARD_ROM_ERROR: u8 = 0x5A;
        /// Bulk transactions with it: packets in and out, NAKs each way, and failed transfers.
        pub const BOARD_BULK_IN: u8 = 0x5B;
        pub const BOARD_BULK_OUT: u8 = 0x5C;
        pub const BOARD_NAKS: u8 = 0x5D;
        pub const BOARD_ERRORS: u8 = 0x5E;
        pub const BOARD_OUT_NAKS: u8 = 0x5F;
        /// The last failed transfer with it: `where << 8 | kind` (`usbh::error_code` in the firmware).
        pub const BOARD_LAST_ERROR: u8 = 0x60;

        pub const AT_CLOCK: [u8; 2] = [AT1_CLOCK, AT2_CLOCK];
        pub const PORT_SPEED: [u8; 3] = [KB_SPEED, MS_SPEED, OT_SPEED];
        pub const PC_SPEED: [u8; 2] = [PC1_SPEED, PC2_SPEED];
        pub const PC_LINK: [u8; 2] = [PC1_LINK, PC2_LINK];
        pub const PC_RESETS: [u8; 2] = [PC1_RESETS, PC2_RESETS];
        pub const LINK_ERRORS: [u8; 2] = [LINK1_ERRORS, LINK2_ERRORS];
        pub const PC_VBUS: [u8; 2] = [PC1_VBUS, PC2_VBUS];
        pub const PC_CC: [u8; 2] = [PC1_CC, PC2_CC];
        pub const AT_3V3: [u8; 2] = [AT1_3V3, AT2_3V3];
        pub const PORT_FAULTS: [u8; 3] = [KB_FAULTS, MS_FAULTS, OT_FAULTS];
        pub const PC_DROPOUTS: [u8; 2] = [PC1_DROPOUTS, PC2_DROPOUTS];
        pub const AT_TEMP: [u8; 2] = [AT1_TEMP, AT2_TEMP];
        pub const AT_FW: [u8; 2] = [AT1_FW, AT2_FW];
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Group {
        System,
        Clocks,
        Usb,
        Power,
        /// The two AT32F405s.
        PcChips,
        /// The Bluetooth module (boards 0.16.0 on, when fitted).
        Bluetooth,
    }

    impl Group {
        pub const ALL: [Group; 6] =
            [Group::System, Group::Clocks, Group::Usb, Group::Power, Group::PcChips, Group::Bluetooth];

        pub const fn of(id: u8) -> Option<Group> {
            match id >> 4 {
                0 => Some(Group::System),
                1 => Some(Group::Clocks),
                2 => Some(Group::Usb),
                3 => Some(Group::Power),
                4 => Some(Group::PcChips),
                5 => Some(Group::Bluetooth),
                _ => None,
            }
        }

        pub const fn name(self) -> &'static str {
            match self {
                Group::System => "System",
                Group::Clocks => "Clocks",
                Group::Usb => "USB",
                Group::Power => "Power",
                Group::PcChips => "PC-side chips",
                Group::Bluetooth => "Bluetooth",
            }
        }
    }

    /// What an item's value means.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Unit {
        Seconds,
        /// [`ResetCause`].
        ResetCause,
        /// Tenths of a degree Celsius.
        DeciCelsius,
        /// `ctrl::selftest` bits.
        SelfTest,
        Count,
        Hz,
        /// [`ClockSource`].
        ClockSource,
        /// `ctrl::Speed`; `Unknown` = nothing attached.
        Speed,
        /// 0 = no, 1 = yes.
        Bool,
        /// Bit 0: the RP2354B's PHY swaps D+/D−; bit 1: the firmware found that out at run time (errata E1).
        Swap,
        /// `ctrl::LinkState`.
        LinkState,
        MilliVolt,
        MilliAmp,
        /// `major << 16 | minor << 8 | patch`; 0 = unknown.
        Version,
        /// [`BtModule`].
        BtModule,
        /// Low byte [`FwUpdates`], the bits above what the check read (hexadecimal, for a bug report).
        FwUpdates,
        /// A code, in hexadecimal (for a bug report).
        Hex,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Meta {
        pub name: &'static str,
        pub unit: Unit,
    }

    const fn m(name: &'static str, unit: Unit) -> Option<Meta> {
        Some(Meta { name, unit })
    }

    /// Name and unit of item `id` (None: an id this version doesn't know).
    pub const fn meta(id: u8) -> Option<Meta> {
        use Unit::*;
        match id {
            id::UPTIME => m("Uptime", Seconds),
            id::RESET_CAUSE => m("Last reset", ResetCause),
            id::RP_TEMP => m("RP2354B temperature", DeciCelsius),
            id::SELFTEST => m("Self-test", SelfTest),
            id::DROPPED => m("Engine outputs dropped", Count),
            id::FW_UPDATES => m("Firmware updates over USB", FwUpdates),
            id::XOSC => m("Crystal (XOSC)", Hz),
            id::PLL_SYS => m("System PLL", Hz),
            id::PLL_USB => m("USB PLL", Hz),
            id::CLK_SYS => m("clk_sys", Hz),
            id::CLK_USB => m("clk_usb", Hz),
            id::CLK_PERI => m("clk_peri", Hz),
            id::CLK_ADC => m("clk_adc", Hz),
            id::ROSC => m("Ring oscillator", Hz),
            id::AT1_CLOCK => m("AT32F405 #1 clock", ClockSource),
            id::AT2_CLOCK => m("AT32F405 #2 clock", ClockSource),
            id::HUB0_SPEED => m("Hub-0 link", Speed),
            id::HUB0_UP => m("Hub-0 enumerated", Bool),
            id::PHY_SWAP => m("D+/D− swap (errata E1)", Swap),
            id::KB_SPEED => m("Keyboard port device", Speed),
            id::MS_SPEED => m("Mouse port device", Speed),
            id::OT_SPEED => m("Other port device", Speed),
            id::PC1_SPEED => m("PC1 USB speed", Speed),
            id::PC2_SPEED => m("PC2 USB speed", Speed),
            id::PC1_LINK => m("PC1 link", LinkState),
            id::PC2_LINK => m("PC2 link", LinkState),
            id::PC1_RESETS => m("PC1 bus resets", Count),
            id::PC2_RESETS => m("PC2 bus resets", Count),
            id::LINK1_ERRORS => m("AT32F405 #1 SPI errors", Count),
            id::LINK2_ERRORS => m("AT32F405 #2 SPI errors", Count),
            id::HUB0_ERRORS => m("Hub-0 changes not handled", Count),
            id::ENUM_FAILS => m("Front-port enumerations failed", Count),
            id::PWR_VBUS => m("PWR VBUS", MilliVolt),
            id::PWR_CC => m("PWR CC", MilliVolt),
            id::PC1_VBUS => m("PC1 VBUS", MilliVolt),
            id::PC2_VBUS => m("PC2 VBUS", MilliVolt),
            id::PC1_CC => m("PC1 CC", MilliVolt),
            id::PC2_CC => m("PC2 CC", MilliVolt),
            id::AT1_3V3 => m("3.3 V at AT32F405 #1", MilliVolt),
            id::AT2_3V3 => m("3.3 V at AT32F405 #2", MilliVolt),
            id::BUDGET => m("Front-port budget", MilliAmp),
            id::KB_FAULTS => m("Keyboard port faults", Count),
            id::MS_FAULTS => m("Mouse port faults", Count),
            id::OT_FAULTS => m("Other port faults", Count),
            id::SUPPLY => m("Lowest 5 V in the last minute", MilliVolt),
            id::SUPPLY_DIPS => m("5 V dips under 4.4 V", Count),
            id::PC1_DROPOUTS => m("PC1 5 V dropouts", Count),
            id::PC2_DROPOUTS => m("PC2 5 V dropouts", Count),
            id::AT1_TEMP => m("AT32F405 #1 temperature", DeciCelsius),
            id::AT2_TEMP => m("AT32F405 #2 temperature", DeciCelsius),
            id::AT1_FW => m("AT32F405 #1 firmware", Version),
            id::AT2_FW => m("AT32F405 #2 firmware", Version),
            id::BT_MODULE => m("ESP32-C3 module", BtModule),
            id::BT_FW => m("ESP32-C3 firmware", Version),
            id::BT_LINK_ERRORS => m("ESP32-C3 UART errors", Count),
            id::BT_HOSTS => m("Bluetooth hosts connected", Count),
            id::BT_SETTINGS_HELD => m("Hosts with settings held in RAM", Count),
            id::BOARD_SESSIONS => m("Module board: sessions", Count),
            id::BOARD_STEPS => m("Module board: last steps", Hex),
            id::BOARD_ROM_ERROR => m("Module board: last ROM error", Hex),
            id::BOARD_BULK_IN => m("Module board: packets in", Count),
            id::BOARD_BULK_OUT => m("Module board: packets out", Count),
            id::BOARD_NAKS => m("Module board: NAKs in", Count),
            id::BOARD_OUT_NAKS => m("Module board: NAKs out", Count),
            id::BOARD_ERRORS => m("Module board: transfer errors", Count),
            id::BOARD_LAST_ERROR => m("Module board: last transfer error", Hex),
            _ => None,
        }
    }
}

/// `Info.caps` bits.
pub mod caps {
    pub const MIRRORED: u16 = 1 << 0;
    pub const IDENTITY: u16 = 1 << 1;
    pub const PASSTHROUGH: u16 = 1 << 2;
    pub const FW_UPDATE: u16 = 1 << 3;
    /// Vial keymap editor and the keymap engine (software.md §4.11).
    pub const VIAL: u16 = 1 << 4;
    /// The Bluetooth module (boards 0.16.0 on) is fitted and answers: Bluetooth hosts are targets.
    pub const BLUETOOTH: u16 = 1 << 5;
    /// The firmware was built with the module's (it knows its SHA-256): `FwTarget::Module` is taken.
    pub const BT_UPDATE: u16 = 1 << 6;
    pub const SIMULATED: u16 = 1 << 15;
}

/// HID keyboard modifier bits (usage 0xE0 + bit).
pub mod modifier {
    pub const LCTRL: u8 = 1 << 0;
    pub const LSHIFT: u8 = 1 << 1;
    pub const LALT: u8 = 1 << 2;
    pub const LGUI: u8 = 1 << 3;
    pub const RCTRL: u8 = 1 << 4;
    pub const RSHIFT: u8 = 1 << 5;
    pub const RALT: u8 = 1 << 6;
    pub const RGUI: u8 = 1 << 7;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Hotkey {
    pub kind: HotkeyKind,
    /// Exact modifier set for `Combo` (modifier bits); unused for `DoubleTap`.
    pub modifiers: u8,
    /// Keyboard usage (page 0x07); for `DoubleTap` a modifier key (0xE0–0xE7) is allowed too.
    pub key: u8,
    /// Hide the hotkey's own key events from the PCs.
    pub swallow: bool,
}

impl Hotkey {
    pub const NONE: Hotkey = Hotkey { kind: HotkeyKind::None, modifiers: 0, key: 0, swallow: false };

    pub const fn double_tap(key: u8, swallow: bool) -> Self {
        Self { kind: HotkeyKind::DoubleTap, modifiers: 0, key, swallow }
    }

    pub const fn combo(modifiers: u8, key: u8) -> Self {
        Self { kind: HotkeyKind::Combo, modifiers, key, swallow: true }
    }
}

/// Entries in the hotkey list.
pub const MAX_HOTKEYS: usize = 16;

/// What a hotkey of the list does. On the wire: a kind byte and an argument byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum HotkeyAction {
    /// An unused entry.
    None,
    /// The other PC; from a Bluetooth host, back to the PC that was active before it.
    TogglePc,
    /// The next host: PC1, PC2, then each Bluetooth host that is ready, then PC1 again.
    NextHost,
    /// The same order backwards.
    PrevHost,
    SelectHost(Host),
    /// The next profile in use, after the last one the first.
    NextProfile,
    PrevProfile,
    /// Profile 0–7 (ignored while it isn't one of those in use).
    SelectProfile(u8),
    /// Cycle the Other port's mode and routing: Auto → passthrough → passthrough pinned to PC1 → to PC2.
    OtherPortMode,
}

impl HotkeyAction {
    /// Every action, in the order hosts list them.
    pub const ALL: [HotkeyAction; 21] = [
        HotkeyAction::TogglePc,
        HotkeyAction::NextHost,
        HotkeyAction::PrevHost,
        HotkeyAction::SelectHost(Host::Pc1),
        HotkeyAction::SelectHost(Host::Pc2),
        HotkeyAction::SelectHost(Host::Bt1),
        HotkeyAction::SelectHost(Host::Bt2),
        HotkeyAction::SelectHost(Host::Bt3),
        HotkeyAction::SelectHost(Host::Bt4),
        HotkeyAction::NextProfile,
        HotkeyAction::PrevProfile,
        HotkeyAction::SelectProfile(0),
        HotkeyAction::SelectProfile(1),
        HotkeyAction::SelectProfile(2),
        HotkeyAction::SelectProfile(3),
        HotkeyAction::SelectProfile(4),
        HotkeyAction::SelectProfile(5),
        HotkeyAction::SelectProfile(6),
        HotkeyAction::SelectProfile(7),
        HotkeyAction::OtherPortMode,
        HotkeyAction::None,
    ];

    /// Its name for `odkvmctl` (`toggle-pc`, `select-pc1`, `select-host3`, `select-profile2`, …).
    pub const fn name(self) -> &'static str {
        match self {
            HotkeyAction::None => "none",
            HotkeyAction::TogglePc => "toggle-pc",
            HotkeyAction::NextHost => "next-host",
            HotkeyAction::PrevHost => "prev-host",
            HotkeyAction::SelectHost(Host::Pc1) => "select-pc1",
            HotkeyAction::SelectHost(Host::Pc2) => "select-pc2",
            HotkeyAction::SelectHost(Host::Bt1) => "select-host1",
            HotkeyAction::SelectHost(Host::Bt2) => "select-host2",
            HotkeyAction::SelectHost(Host::Bt3) => "select-host3",
            HotkeyAction::SelectHost(Host::Bt4) => "select-host4",
            HotkeyAction::NextProfile => "next-profile",
            HotkeyAction::PrevProfile => "prev-profile",
            HotkeyAction::SelectProfile(0) => "select-profile0",
            HotkeyAction::SelectProfile(1) => "select-profile1",
            HotkeyAction::SelectProfile(2) => "select-profile2",
            HotkeyAction::SelectProfile(3) => "select-profile3",
            HotkeyAction::SelectProfile(4) => "select-profile4",
            HotkeyAction::SelectProfile(5) => "select-profile5",
            HotkeyAction::SelectProfile(6) => "select-profile6",
            HotkeyAction::SelectProfile(_) => "select-profile7",
            HotkeyAction::OtherPortMode => "other-port",
        }
    }

    /// The action named `name` ([`HotkeyAction::name`]).
    pub fn from_name(name: &str) -> Option<HotkeyAction> {
        Self::ALL.iter().copied().find(|a| a.name() == name)
    }

    pub fn encode(self, w: &mut Writer) -> Result<(), WireError> {
        let (kind, arg) = match self {
            HotkeyAction::None => (0, 0),
            HotkeyAction::TogglePc => (1, 0),
            HotkeyAction::NextHost => (2, 0),
            HotkeyAction::PrevHost => (3, 0),
            HotkeyAction::SelectHost(h) => (4, h as u8),
            HotkeyAction::NextProfile => (5, 0),
            HotkeyAction::PrevProfile => (6, 0),
            HotkeyAction::SelectProfile(n) => (7, n),
            HotkeyAction::OtherPortMode => (8, 0),
        };
        w.u8(kind)?;
        w.u8(arg)
    }

    pub fn decode(r: &mut Reader) -> Result<HotkeyAction, WireError> {
        let (kind, arg) = (r.u8()?, r.u8()?);
        Ok(match kind {
            0 => HotkeyAction::None,
            1 => HotkeyAction::TogglePc,
            2 => HotkeyAction::NextHost,
            3 => HotkeyAction::PrevHost,
            4 => HotkeyAction::SelectHost(arg.try_into()?),
            5 => HotkeyAction::NextProfile,
            6 => HotkeyAction::PrevProfile,
            7 if (arg as usize) < MAX_PROFILES => HotkeyAction::SelectProfile(arg),
            8 => HotkeyAction::OtherPortMode,
            _ => return Err(WireError::Invalid),
        })
    }
}

/// An entry of the hotkey list: what it does, and its hotkey (none yet: the entry is kept, and does nothing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct HotkeyBinding {
    pub action: HotkeyAction,
    pub hotkey: Hotkey,
}

impl HotkeyBinding {
    pub const NONE: HotkeyBinding = HotkeyBinding { action: HotkeyAction::None, hotkey: Hotkey::NONE };

    pub const fn new(action: HotkeyAction, hotkey: Hotkey) -> Self {
        Self { action, hotkey }
    }

    /// An entry of the list (an unused one has no action).
    pub fn is_used(&self) -> bool {
        self.action != HotkeyAction::None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Info {
    pub proto: u8,
    pub rp_fw: u32,
    /// Firmware of AT32 #1 / #2 (0 = link down).
    pub at_fw: [u32; 2],
    /// The board's version ([`crate::HARDWARE_VERSION`]), packed like the firmware's.
    pub hw: u32,
    pub serial: [u8; 8],
    pub caps: u16,
    /// Firmware of the Bluetooth module (0 = none, or not answering).
    pub bt_fw: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Status {
    pub active: Pc,
    /// The host this response is sent to: a PC, or a Bluetooth host over the module's control service (None in the
    /// simulator or over the service port).
    pub asking: Option<Host>,
    pub links: [LinkState; 2],
    pub power: PowerSource,
    pub adv_pwr: Advertised,
    pub adv_pc: [Advertised; 2],
    /// VBUS of PWR, PC1, PC2 in mV.
    pub vbus_mv: [u16; 3],
    /// What the source advertises less the KVM's own draw: an estimate, for information (the warning
    /// comes from [`Status::supply`]).
    pub budget_ma: u16,
    pub flags: u8,
    pub uptime_s: u32,
    pub link_errors: [u16; 2],
    /// [`selftest`] bits.
    pub selftest: u32,
    /// Bluetooth: the host slot that gets input (0 = a PC is active, `active`).
    pub bt_active: u8,
    /// Host slots ready for input (bit n−1 for slot n): connected, encrypted, subscribed.
    pub bt_ready: u8,
    /// Host slots with a bond.
    pub bt_bonded: u8,
    /// Seconds left while pairing is open, 0 when closed.
    pub bt_pairing: u8,
    /// The host slot waiting for its passkey to be typed (0 = none).
    pub bt_passkey: u8,
    /// The Bluetooth module: absent, up, being programmed, failed (`diag::BtModule`).
    pub bt_module: diag::BtModule,
    /// Screen hop: whether it moves the pointer, and where the pointer is in the
    /// arrangement (logical pixels).
    pub hop: HopState,
    pub pointer: [i16; 2],
    /// The passkey typed so far on the KVM's keyboard for the host in `bt_passkey`: how many digits, and
    /// their value, so that the window can show them while they are typed.
    pub bt_passkey_digits: u8,
    pub bt_passkey_typed: u32,
    /// The 5 V the KVM runs from, judged from its measurements: the level,
    /// the voltage now, and the lowest in the last minute (the PC-side chips' millisecond minimum, the PWR port's
    /// readings), in mV.
    pub supply: Supply,
    pub supply_mv: u16,
    pub supply_lowest_mv: u16,
    /// What the front devices declare they may draw at most (their `bMaxPower`), in mA: information.
    pub declared_ma: u16,
    /// Bluetooth device slots with a bond, and those ready (connected, encrypted, reporting): bit n−1 for slot n.
    pub bt_devices_bonded: u8,
    pub bt_devices_ready: u8,
    /// Seconds left while the module looks for devices to pair (0: not looking), and how many it found.
    pub bt_scan: u8,
    pub bt_found: u8,
    /// Pairing a device: how far it got, and the passkey to type on a keyboard that asks for one.
    pub bt_device_pairing: DevicePairing,
    pub bt_device_passkey: Option<u32>,
    /// How far programming the module has got, in percent (while `bt_module` is `Programming`).
    pub bt_progress: u8,
    /// The module's chip, which picks its firmware image (`kvm-module-<chip>.bin`): from its `Hello`, or from its ROM
    /// while it runs no firmware of the KVM's (`Unknown` until either).
    pub bt_chip: crate::link::bt::Chip,
}

impl Status {
    /// The PC this response is sent to, if a PC asked.
    pub fn asking_pc(&self) -> Option<Pc> {
        self.asking.and_then(Host::as_pc)
    }

    /// The Bluetooth host slot (1–4) this response is sent to, if one asked over the module's control service.
    pub fn asking_bt(&self) -> Option<u8> {
        self.asking.and_then(Host::bt_slot)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PortInfo {
    pub port: Port,
    pub state: PortState,
    pub mode: PortMode,
    /// What actually runs: Normalized, Mirrored, MirroredIdentity, Passthrough or Off.
    pub effective: PortMode,
    /// Where its input goes now (its routing in the active profile, or the active target): None while it goes
    /// nowhere (Off, empty, unsupported).
    pub target: Option<Host>,
    pub reason: AutoReason,
    pub vid: u16,
    pub pid: u16,
    /// The device's speed on Hub-0 (full speed at most). In passthrough: on the PC's hub, where a high-speed device
    /// runs at high speed unless that hub's link to the PC is full speed (since firmware 0.1.12).
    pub speed: Speed,
    pub classes: u16,
    pub interfaces: u8,
    pub report_rate_hz: u16,
    pub max_power_ma: u16,
    pub faults: u16,
}

impl PortInfo {
    pub const fn empty(port: Port, mode: PortMode) -> Self {
        Self {
            port,
            state: PortState::Empty,
            mode,
            effective: mode,
            target: None,
            reason: AutoReason::NotAuto,
            vid: 0,
            pid: 0,
            speed: Speed::Unknown,
            classes: 0,
            interfaces: 0,
            report_rate_hz: 0,
            max_power_ma: 0,
            faults: 0,
        }
    }
}

/// Config keys (software.md §8.3).
pub mod key {
    pub const DOUBLE_TAP_MS: u16 = 0x0004;
    /// What a tap / a long press on the KVM's key does.
    pub const KEY_TAP: u16 = 0x0005;
    pub const KEY_LONG: u16 = 0x0006;
    /// How long the key is held for a long press (ms).
    pub const LONG_PRESS_MS: u16 = 0x0007;
    pub const LED_BRIGHTNESS: u16 = 0x0010;
    pub const COLOR_PC1: u16 = 0x0011;
    pub const COLOR_PC2: u16 = 0x0012;
    pub const PC_POLLING: u16 = 0x0020;
    pub const POWER_CYCLE_ON_SWITCH: u16 = 0x0021;
    /// The active profile's mode and routing of each device ([`Device`](super::Device) order).
    pub const MODE_BASE: u16 = 0x0030;
    pub const ROUTE_BASE: u16 = 0x0038;
    pub const SW_SWITCH_ALLOWED: u16 = 0x0040;
    pub const SW_CONFIG_ALLOWED: u16 = 0x0041;
    pub const VIAL: u16 = 0x0050;
    /// The virtual gamepad the PCs get ([`GamepadOut`](super::GamepadOut)).
    pub const GAMEPAD: u16 = 0x0052;
    /// Make the running PC active when the active one is switched off (software.md §4.12).
    pub const AUTO_SELECT: u16 = 0x0060;
    /// In standby, keep the keyboard and mouse powered so that a key press wakes a sleeping PC.
    pub const STANDBY_WAKE: u16 = 0x0061;
    /// In standby, the key LED glows dimly instead of all LEDs going dark.
    pub const STANDBY_GLOW: u16 = 0x0062;
    /// The active profile; the modes and routings (`MODE_BASE`, `ROUTE_BASE`) are its.
    pub const PROFILE: u16 = 0x0070;
    /// Profiles in use (1–[`MAX_PROFILES`](super::MAX_PROFILES)): the key and `next` cycle through them.
    pub const PROFILE_COUNT: u16 = 0x0071;
    /// Power up in the profile last active (otherwise in profile 0, and choosing one writes nothing to flash).
    pub const REMEMBER_PROFILE: u16 = 0x0072;
    /// Every device's mode, and the name, of profile 0–7.
    pub const PROFILE_MODES_BASE: u16 = 0x0080;
    pub const PROFILE_NAME_BASE: u16 = 0x0088;
    /// Name of Bluetooth host slot 1–4 (UTF-8, at most 12 bytes; empty = "BT n").
    pub const BT_NAME_BASE: u16 = 0x0090;
    /// What the module does with host slot 1–4 while another target is active ([`Policy`](super::BtPolicy)).
    pub const BT_POLICY_BASE: u16 = 0x0098;
    /// Host slot 1–4 may control the KVM over Bluetooth (bool, off by default). Turning it on takes a tap on the key;
    /// it goes off by itself when the slot's bond goes.
    pub const BT_CONTROL_BASE: u16 = 0x009C;
    /// Screen hop: the absolute pointer on the KVM's mouse, the acceleration, speed, edge resistance and
    /// corner, PC1's and PC2's mapping.
    pub const ABS_POINTER: u16 = 0x00A0;
    pub const HOP_ACCEL: u16 = 0x00A1;
    pub const HOP_SPEED: u16 = 0x00A2;
    pub const HOP_RESISTANCE: u16 = 0x00A3;
    pub const HOP_CORNER: u16 = 0x00A4;
    pub const HOP_MAPPING_BASE: u16 = 0x00A6;
    /// PC1's and PC2's [`Units`](super::Units) for screen hop.
    pub const HOP_UNITS_BASE: u16 = 0x00AC;
    /// Screen slot s of arrangement a at `SCREEN_BASE + a × MAX_SCREENS + s`.
    pub const SCREEN_BASE: u16 = 0x00B0;
    /// Name of arrangement 0–3.
    pub const ARRANGEMENT_NAME_BASE: u16 = 0x00D0;
    /// Bluetooth host slot 1–4's mapping and units.
    pub const HOP_MAPPING_BT_BASE: u16 = 0x00D8;
    pub const HOP_UNITS_BT_BASE: u16 = 0x00DC;
    /// Bluetooth host slot 1–4's pointer for screen hop ([`BtPointer`](super::BtPointer)).
    pub const BT_POINTER_BASE: u16 = 0x00E0;
    /// Entry 0–15 of the hotkey list ([`HotkeyBinding`](super::HotkeyBinding)).
    pub const HOTKEY_BASE: u16 = 0x00F0;
    /// Every device's routing in profile 0–7 ([`Routing`](super::Routing)).
    pub const PROFILE_ROUTES_BASE: u16 = 0x0100;
    /// Profile 0–7's arrangement (0: none, n + 1: arrangement n).
    pub const PROFILE_ARRANGEMENT_BASE: u16 = 0x0108;
    /// Profile 0–7's keymap (0: none, n + 1: keymap n; hid_mapping.md).
    pub const PROFILE_KEYMAP_BASE: u16 = 0x0110;
    /// Profile 0–7's keymap for a host that can't take the virtual gamepad (0: none, the profile's own; n + 1: keymap
    /// n): its gamepad mapped to the mouse and keys instead.
    pub const PROFILE_FALLBACK_BASE: u16 = 0x0118;
    /// The keyboard layout of each host ([`Host::index`] order: PC1, PC2, Bluetooth hosts 1–4), `kvm_hid::layouts`:
    /// macro text is typed through it.
    pub const LAYOUT_BASE: u16 = 0x00E8;

    pub const fn layout(h: Host) -> u16 {
        LAYOUT_BASE + h.index() as u16
    }

    use super::{DEVICES, Host, MAX_ARRANGEMENTS, MAX_HOTKEYS, MAX_PROFILES, MAX_SCREENS};

    /// Whether `k` is one of the `n` keys from `base`.
    pub const fn in_range(k: u16, base: u16, n: usize) -> Option<u8> {
        if k >= base && k < base + n as u16 { Some((k - base) as u8) } else { None }
    }

    /// The device whose mode (`MODE_BASE`) or routing (`ROUTE_BASE`) `k` is.
    pub const fn mode_of(k: u16) -> Option<u8> {
        in_range(k, MODE_BASE, DEVICES)
    }

    pub const fn route_of(k: u16) -> Option<u8> {
        in_range(k, ROUTE_BASE, DEVICES)
    }

    /// The profile `k` is a key of, from `base` (`PROFILE_MODES_BASE`, …).
    pub const fn profile_of(k: u16, base: u16) -> Option<u8> {
        in_range(k, base, MAX_PROFILES)
    }

    /// The key of entry `i` of the hotkey list.
    pub const fn hotkey(i: u8) -> u16 {
        HOTKEY_BASE + i as u16
    }

    /// The entry of the hotkey list a key is.
    pub const fn hotkey_of(k: u16) -> Option<u8> {
        if k >= HOTKEY_BASE && k < HOTKEY_BASE + MAX_HOTKEYS as u16 { Some((k - HOTKEY_BASE) as u8) } else { None }
    }

    pub const fn hop_mapping(h: Host) -> u16 {
        match h.bt_slot() {
            None => HOP_MAPPING_BASE + h.index() as u16,
            Some(n) => HOP_MAPPING_BT_BASE + n as u16 - 1,
        }
    }

    pub const fn hop_units(h: Host) -> u16 {
        match h.bt_slot() {
            None => HOP_UNITS_BASE + h.index() as u16,
            Some(n) => HOP_UNITS_BT_BASE + n as u16 - 1,
        }
    }

    pub const fn screen(arrangement: u8, slot: u8) -> u16 {
        SCREEN_BASE + (arrangement as usize * MAX_SCREENS + slot as usize) as u16
    }

    /// The arrangement a key belongs to (its screens and its name): these are stored in the arrangement's own item,
    /// not in the config item.
    pub const fn arrangement_of(k: u16) -> Option<u8> {
        if k >= SCREEN_BASE && k < SCREEN_BASE + (MAX_ARRANGEMENTS * MAX_SCREENS) as u16 {
            Some(((k - SCREEN_BASE) as usize / MAX_SCREENS) as u8)
        } else if k >= ARRANGEMENT_NAME_BASE && k < ARRANGEMENT_NAME_BASE + MAX_ARRANGEMENTS as u16 {
            Some((k - ARRANGEMENT_NAME_BASE) as u8)
        } else {
            None
        }
    }

    const BASE: &[u16] = &[
        DOUBLE_TAP_MS,
        KEY_TAP,
        KEY_LONG,
        LONG_PRESS_MS,
        LED_BRIGHTNESS,
        COLOR_PC1,
        COLOR_PC2,
        PC_POLLING,
        POWER_CYCLE_ON_SWITCH,
        SW_SWITCH_ALLOWED,
        SW_CONFIG_ALLOWED,
        VIAL,
        GAMEPAD,
        AUTO_SELECT,
        STANDBY_WAKE,
        STANDBY_GLOW,
        // The count before the active profile, which must be one of those in use.
        PROFILE_COUNT,
        PROFILE,
        REMEMBER_PROFILE,
        BT_NAME_BASE,
        BT_NAME_BASE + 1,
        BT_NAME_BASE + 2,
        BT_NAME_BASE + 3,
        BT_POLICY_BASE,
        BT_POLICY_BASE + 1,
        BT_POLICY_BASE + 2,
        BT_POLICY_BASE + 3,
        BT_CONTROL_BASE,
        BT_CONTROL_BASE + 1,
        BT_CONTROL_BASE + 2,
        BT_CONTROL_BASE + 3,
        ABS_POINTER,
        HOP_ACCEL,
        HOP_SPEED,
        HOP_RESISTANCE,
        HOP_CORNER,
        HOP_MAPPING_BASE,
        HOP_MAPPING_BASE + 1,
        HOP_MAPPING_BT_BASE,
        HOP_MAPPING_BT_BASE + 1,
        HOP_MAPPING_BT_BASE + 2,
        HOP_MAPPING_BT_BASE + 3,
        HOP_UNITS_BASE,
        HOP_UNITS_BASE + 1,
        HOP_UNITS_BT_BASE,
        HOP_UNITS_BT_BASE + 1,
        HOP_UNITS_BT_BASE + 2,
        HOP_UNITS_BT_BASE + 3,
        BT_POINTER_BASE,
        BT_POINTER_BASE + 1,
        BT_POINTER_BASE + 2,
        BT_POINTER_BASE + 3,
        LAYOUT_BASE,
        LAYOUT_BASE + 1,
        LAYOUT_BASE + 2,
        LAYOUT_BASE + 3,
        LAYOUT_BASE + 4,
        LAYOUT_BASE + 5,
    ];
    /// Each profile's keys, after the profile count and the active profile.
    const PROFILE_BASES: [u16; 6] = [
        PROFILE_NAME_BASE,
        PROFILE_MODES_BASE,
        PROFILE_ROUTES_BASE,
        PROFILE_ARRANGEMENT_BASE,
        PROFILE_KEYMAP_BASE,
        PROFILE_FALLBACK_BASE,
    ];
    const ALL_LEN: usize = MAX_HOTKEYS
        + BASE.len()
        + 2 * DEVICES
        + MAX_PROFILES * PROFILE_BASES.len()
        + MAX_ARRANGEMENTS * (1 + MAX_SCREENS);
    const ALL_KEYS: [u16; ALL_LEN] = {
        let mut out = [0u16; ALL_LEN];
        let mut i = 0;
        while i < MAX_HOTKEYS {
            out[i] = hotkey(i as u8);
            i += 1;
        }
        while i < MAX_HOTKEYS + BASE.len() {
            out[i] = BASE[i - MAX_HOTKEYS];
            i += 1;
        }
        // The active profile's modes and routings, then every profile's.
        let mut d = 0;
        while d < DEVICES {
            out[i] = MODE_BASE + d as u16;
            out[i + 1] = ROUTE_BASE + d as u16;
            i += 2;
            d += 1;
        }
        let mut p = 0;
        while p < MAX_PROFILES {
            let mut b = 0;
            while b < PROFILE_BASES.len() {
                out[i] = PROFILE_BASES[b] + p as u16;
                i += 1;
                b += 1;
            }
            p += 1;
        }
        let mut a = 0;
        while a < MAX_ARRANGEMENTS {
            out[i] = ARRANGEMENT_NAME_BASE + a as u16;
            i += 1;
            let mut slot = 0;
            while slot < MAX_SCREENS {
                out[i] = screen(a as u8, slot as u8);
                i += 1;
                slot += 1;
            }
            a += 1;
        }
        out
    };
    /// Every key, in the order a config is listed and stored; each arrangement's name before its screens.
    pub const ALL: &[u16] = &ALL_KEYS;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ConfigValue {
    /// Entry 0–15 of the hotkey list.
    Hotkey(u8, HotkeyBinding),
    DoubleTapMs(u16),
    LedBrightness(u8),
    ColorPc1(Rgb),
    ColorPc2(Rgb),
    PcPolling(Polling),
    PowerCycleOnSwitch(bool),
    /// The active profile's mode and routing of a device.
    Mode(Device, PortMode),
    Route(Device, Routing),
    SwSwitchAllowed(bool),
    SwConfigAllowed(bool),
    Vial(VialMode),
    Gamepad(GamepadOut),
    AutoSelect(bool),
    StandbyWake(bool),
    StandbyGlow(bool),
    KeyTap(KeyAction),
    KeyLong(KeyAction),
    LongPressMs(u16),
    Profile(u8),
    ProfileCount(u8),
    /// Profile index, then the modes of the Keyboard, Mouse and Other port.
    ProfileModes(u8, [PortMode; DEVICES]),
    ProfileRoutes(u8, [Routing; DEVICES]),
    ProfileName(u8, ProfileName),
    RememberProfile(bool),
    /// Bluetooth host slot 0–3 (slot n = index n−1) and its name (empty = "BT n").
    BtName(u8, ProfileName),
    /// Bluetooth host slot 0–3 and what the module does with it while another target is active.
    BtPolicy(u8, BtPolicy),
    /// Bluetooth host slot 0–3 may control the KVM over Bluetooth. On only by the key (`Request::AskControl`, or this
    /// set from a PC, confirmed with a tap); off at once.
    BtControl(u8, bool),
    /// The KVM's mouse also has an absolute pointer (screen hop needs it). Changing it reconnects the PCs' KVM devices.
    AbsPointer(bool),
    HopAccel(Accel),
    /// Pointer speed, −10–10 (0 = the acceleration's own default).
    HopSpeed(i8),
    /// How far the pointer must be pushed against an edge to another PC's screen before it hops (logical pixels).
    HopResistance(u16),
    /// The stretch at each end of such an edge where the pointer never hops (logical pixels), so a corner stays easy to
    /// hit.
    HopCorner(u16),
    /// How the host maps the absolute pointer onto its screens.
    HopMapping(Host, Mapping),
    /// What the host's desktop coordinates count.
    HopUnits(Host, Units),
    /// Profile index, and the arrangement screen hop uses in it (None: screen hop is off there).
    ProfileArrangement(u8, Option<u8>),
    /// Profile index, and the keymap its normalized devices go through (None: none, as without the mapping engine).
    ProfileKeymap(u8, Option<u8>),
    /// Profile index, and the keymap used instead while the input's host can't take the virtual gamepad (None: the
    /// profile's own).
    ProfileFallback(u8, Option<u8>),
    /// A host's keyboard layout (`kvm_hid::layouts::Layout` as its number, below [`LAYOUTS`]).
    HostLayout(Host, u8),
    /// Arrangement 0–3, screen slot 0–7.
    Screen(u8, u8, Screen),
    /// Arrangement 0–3 and its name.
    ArrangementName(u8, ProfileName),
    /// Bluetooth host slot 0–3 (slot n = index n−1) and how screen hop moves its pointer.
    BtPointer(u8, BtPointer),
}

pub use crate::link::bt::Policy as BtPolicy;

impl ConfigValue {
    pub fn key(&self) -> u16 {
        match self {
            ConfigValue::Hotkey(i, _) => key::hotkey(*i),
            ConfigValue::DoubleTapMs(_) => key::DOUBLE_TAP_MS,
            ConfigValue::LedBrightness(_) => key::LED_BRIGHTNESS,
            ConfigValue::ColorPc1(_) => key::COLOR_PC1,
            ConfigValue::ColorPc2(_) => key::COLOR_PC2,
            ConfigValue::PcPolling(_) => key::PC_POLLING,
            ConfigValue::PowerCycleOnSwitch(_) => key::POWER_CYCLE_ON_SWITCH,
            ConfigValue::Mode(d, _) => key::MODE_BASE + *d as u16,
            ConfigValue::Route(d, _) => key::ROUTE_BASE + *d as u16,
            ConfigValue::SwSwitchAllowed(_) => key::SW_SWITCH_ALLOWED,
            ConfigValue::SwConfigAllowed(_) => key::SW_CONFIG_ALLOWED,
            ConfigValue::Vial(_) => key::VIAL,
            ConfigValue::Gamepad(_) => key::GAMEPAD,
            ConfigValue::AutoSelect(_) => key::AUTO_SELECT,
            ConfigValue::StandbyWake(_) => key::STANDBY_WAKE,
            ConfigValue::StandbyGlow(_) => key::STANDBY_GLOW,
            ConfigValue::KeyTap(_) => key::KEY_TAP,
            ConfigValue::KeyLong(_) => key::KEY_LONG,
            ConfigValue::LongPressMs(_) => key::LONG_PRESS_MS,
            ConfigValue::RememberProfile(_) => key::REMEMBER_PROFILE,
            ConfigValue::Profile(_) => key::PROFILE,
            ConfigValue::ProfileCount(_) => key::PROFILE_COUNT,
            ConfigValue::ProfileModes(i, _) => key::PROFILE_MODES_BASE + *i as u16,
            ConfigValue::ProfileRoutes(i, _) => key::PROFILE_ROUTES_BASE + *i as u16,
            ConfigValue::ProfileName(i, _) => key::PROFILE_NAME_BASE + *i as u16,
            ConfigValue::BtName(i, _) => key::BT_NAME_BASE + *i as u16,
            ConfigValue::BtPolicy(i, _) => key::BT_POLICY_BASE + *i as u16,
            ConfigValue::BtControl(i, _) => key::BT_CONTROL_BASE + *i as u16,
            ConfigValue::AbsPointer(_) => key::ABS_POINTER,
            ConfigValue::HopAccel(_) => key::HOP_ACCEL,
            ConfigValue::HopSpeed(_) => key::HOP_SPEED,
            ConfigValue::HopResistance(_) => key::HOP_RESISTANCE,
            ConfigValue::HopCorner(_) => key::HOP_CORNER,
            ConfigValue::HopMapping(h, _) => key::hop_mapping(*h),
            ConfigValue::HopUnits(h, _) => key::hop_units(*h),
            ConfigValue::ProfileArrangement(i, _) => key::PROFILE_ARRANGEMENT_BASE + *i as u16,
            ConfigValue::ProfileKeymap(i, _) => key::PROFILE_KEYMAP_BASE + *i as u16,
            ConfigValue::ProfileFallback(i, _) => key::PROFILE_FALLBACK_BASE + *i as u16,
            ConfigValue::HostLayout(h, _) => key::layout(*h),
            ConfigValue::Screen(a, i, _) => key::screen(*a, *i),
            ConfigValue::ArrangementName(a, _) => key::ARRANGEMENT_NAME_BASE + *a as u16,
            ConfigValue::BtPointer(i, _) => key::BT_POINTER_BASE + *i as u16,
        }
    }

    pub fn encode(&self, w: &mut Writer) -> Result<(), WireError> {
        w.u16(self.key())?;
        match *self {
            ConfigValue::Hotkey(_, b) => {
                b.action.encode(w)?;
                let h = b.hotkey;
                w.u8(h.kind as u8)?;
                w.u8(h.modifiers)?;
                w.u8(h.key)?;
                w.bool(h.swallow)
            }
            ConfigValue::DoubleTapMs(v) | ConfigValue::LongPressMs(v) => w.u16(v),
            ConfigValue::LedBrightness(v) => w.u8(v),
            ConfigValue::ColorPc1(c) | ConfigValue::ColorPc2(c) => w.bytes(&[c.r, c.g, c.b]),
            ConfigValue::PcPolling(p) => w.u8(p as u8),
            ConfigValue::Mode(_, m) => w.u8(m as u8),
            ConfigValue::Route(_, r) => w.u8(r.to_u8()),
            ConfigValue::Vial(m) => w.u8(m as u8),
            ConfigValue::KeyTap(a) | ConfigValue::KeyLong(a) => w.u8(a as u8),
            ConfigValue::Profile(i) | ConfigValue::ProfileCount(i) => w.u8(i),
            ConfigValue::ProfileModes(_, m) => w.bytes(&m.map(|m| m as u8)),
            ConfigValue::ProfileRoutes(_, r) => w.bytes(&r.map(|r| r.to_u8())),
            ConfigValue::ProfileName(_, n) | ConfigValue::BtName(_, n) | ConfigValue::ArrangementName(_, n) => {
                w.lv(n.as_str().as_bytes())
            }
            ConfigValue::ProfileArrangement(_, a)
            | ConfigValue::ProfileKeymap(_, a)
            | ConfigValue::ProfileFallback(_, a) => w.u8(a.map_or(0, |a| a + 1)),
            ConfigValue::HostLayout(_, l) => w.u8(l),
            ConfigValue::Gamepad(g) => w.u8(g as u8),
            ConfigValue::BtPointer(_, p) => w.u8(p as u8),
            ConfigValue::BtPolicy(_, p) => w.u8(p as u8),
            ConfigValue::HopAccel(a) => w.u8(a as u8),
            ConfigValue::HopSpeed(v) => w.u8(v as u8),
            ConfigValue::HopResistance(v) | ConfigValue::HopCorner(v) => w.u16(v),
            ConfigValue::HopMapping(_, m) => w.u8(m as u8),
            ConfigValue::HopUnits(_, u) => w.u8(u as u8),
            ConfigValue::Screen(_, _, s) => {
                let flags = if s.off { screen_flags::OFF } else { 0 } | if s.fixed { screen_flags::FIXED } else { 0 };
                w.u8(s.host.map_or(0, |h| h as u8 | flags))?;
                w.u16(s.w)?;
                w.u16(s.h)?;
                w.u16(s.scale)?;
                for v in s.desk.into_iter().chain(s.at) {
                    w.u16(v as u16)?;
                }
                Ok(())
            }
            ConfigValue::PowerCycleOnSwitch(b)
            | ConfigValue::BtControl(_, b)
            | ConfigValue::SwSwitchAllowed(b)
            | ConfigValue::SwConfigAllowed(b)
            | ConfigValue::AutoSelect(b)
            | ConfigValue::StandbyWake(b)
            | ConfigValue::StandbyGlow(b)
            | ConfigValue::RememberProfile(b)
            | ConfigValue::AbsPointer(b) => w.bool(b),
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Self, WireError> {
        let k = r.u16()?;
        let hotkey = |r: &mut Reader| -> Result<Hotkey, WireError> {
            Ok(Hotkey { kind: r.u8()?.try_into()?, modifiers: r.u8()?, key: r.u8()?, swallow: r.bool()? })
        };
        let rgb = |r: &mut Reader| -> Result<Rgb, WireError> { Ok(Rgb { r: r.u8()?, g: r.u8()?, b: r.u8()? }) };
        let routing = |r: &mut Reader| Routing::from_u8(r.u8()?).ok_or(WireError::Invalid);
        let profiles = |base: u16| base..base + MAX_PROFILES as u16;
        let bt_slots = |base: u16| base..base + BT_HOSTS as u16;
        let arrangements = |base: u16| base..base + MAX_ARRANGEMENTS as u16;
        Ok(match k {
            k if let Some(i) = key::hotkey_of(k) => {
                let action = HotkeyAction::decode(r)?;
                ConfigValue::Hotkey(i, HotkeyBinding { action, hotkey: hotkey(r)? })
            }
            key::DOUBLE_TAP_MS => ConfigValue::DoubleTapMs(r.u16()?),
            key::LED_BRIGHTNESS => ConfigValue::LedBrightness(r.u8()?),
            key::COLOR_PC1 => ConfigValue::ColorPc1(rgb(r)?),
            key::COLOR_PC2 => ConfigValue::ColorPc2(rgb(r)?),
            key::PC_POLLING => ConfigValue::PcPolling(r.u8()?.try_into()?),
            key::POWER_CYCLE_ON_SWITCH => ConfigValue::PowerCycleOnSwitch(r.bool()?),
            k if let Some(d) = key::mode_of(k) => {
                let d = Device::from_index(d as usize).ok_or(WireError::Invalid)?;
                ConfigValue::Mode(d, r.u8()?.try_into()?)
            }
            k if let Some(d) = key::route_of(k) => {
                let d = Device::from_index(d as usize).ok_or(WireError::Invalid)?;
                ConfigValue::Route(d, routing(r)?)
            }
            key::SW_SWITCH_ALLOWED => ConfigValue::SwSwitchAllowed(r.bool()?),
            key::SW_CONFIG_ALLOWED => ConfigValue::SwConfigAllowed(r.bool()?),
            key::VIAL => ConfigValue::Vial(r.u8()?.try_into()?),
            key::GAMEPAD => ConfigValue::Gamepad(r.u8()?.try_into()?),
            key::AUTO_SELECT => ConfigValue::AutoSelect(r.bool()?),
            key::STANDBY_WAKE => ConfigValue::StandbyWake(r.bool()?),
            key::STANDBY_GLOW => ConfigValue::StandbyGlow(r.bool()?),
            key::KEY_TAP => ConfigValue::KeyTap(r.u8()?.try_into()?),
            key::KEY_LONG => ConfigValue::KeyLong(r.u8()?.try_into()?),
            key::LONG_PRESS_MS => ConfigValue::LongPressMs(r.u16()?),
            key::REMEMBER_PROFILE => ConfigValue::RememberProfile(r.bool()?),
            key::PROFILE => ConfigValue::Profile(r.u8()?),
            key::PROFILE_COUNT => ConfigValue::ProfileCount(r.u8()?),
            k if let Some(i) = key::profile_of(k, key::PROFILE_MODES_BASE) => {
                let mut m = [PortMode::Auto; DEVICES];
                for v in &mut m {
                    *v = r.u8()?.try_into()?;
                }
                ConfigValue::ProfileModes(i, m)
            }
            k if let Some(i) = key::profile_of(k, key::PROFILE_ROUTES_BASE) => {
                let mut m = [Routing::Follow; DEVICES];
                for v in &mut m {
                    *v = routing(r)?;
                }
                ConfigValue::ProfileRoutes(i, m)
            }
            k if profiles(key::PROFILE_NAME_BASE).contains(&k)
                || bt_slots(key::BT_NAME_BASE).contains(&k)
                || arrangements(key::ARRANGEMENT_NAME_BASE).contains(&k) =>
            {
                let b = r.lv()?;
                let s = core::str::from_utf8(b).map_err(|_| WireError::Invalid)?;
                if b.len() > PROFILE_NAME_MAX {
                    return Err(WireError::Invalid);
                }
                if k >= key::ARRANGEMENT_NAME_BASE {
                    ConfigValue::ArrangementName((k - key::ARRANGEMENT_NAME_BASE) as u8, ProfileName::new(s))
                } else if k >= key::BT_NAME_BASE {
                    ConfigValue::BtName((k - key::BT_NAME_BASE) as u8, ProfileName::new(s))
                } else {
                    ConfigValue::ProfileName((k - key::PROFILE_NAME_BASE) as u8, ProfileName::new(s))
                }
            }
            k if bt_slots(key::BT_POLICY_BASE).contains(&k) => {
                ConfigValue::BtPolicy((k - key::BT_POLICY_BASE) as u8, r.u8()?.try_into()?)
            }
            k if bt_slots(key::BT_CONTROL_BASE).contains(&k) => {
                ConfigValue::BtControl((k - key::BT_CONTROL_BASE) as u8, r.bool()?)
            }
            key::ABS_POINTER => ConfigValue::AbsPointer(r.bool()?),
            key::HOP_ACCEL => ConfigValue::HopAccel(r.u8()?.try_into()?),
            key::HOP_SPEED => ConfigValue::HopSpeed(r.u8()? as i8),
            key::HOP_RESISTANCE => ConfigValue::HopResistance(r.u16()?),
            key::HOP_CORNER => ConfigValue::HopCorner(r.u16()?),
            k if (key::HOP_MAPPING_BASE..key::HOP_MAPPING_BASE + 2).contains(&k) => ConfigValue::HopMapping(
                Host::from_index((k - key::HOP_MAPPING_BASE) as usize).ok_or(WireError::Invalid)?,
                r.u8()?.try_into()?,
            ),
            k if bt_slots(key::HOP_MAPPING_BT_BASE).contains(&k) => ConfigValue::HopMapping(
                Host::bt((k - key::HOP_MAPPING_BT_BASE) as u8 + 1).ok_or(WireError::Invalid)?,
                r.u8()?.try_into()?,
            ),
            k if profiles(key::PROFILE_ARRANGEMENT_BASE).contains(&k) => {
                let i = (k - key::PROFILE_ARRANGEMENT_BASE) as u8;
                ConfigValue::ProfileArrangement(i, r.u8()?.checked_sub(1))
            }
            k if profiles(key::PROFILE_KEYMAP_BASE).contains(&k) => {
                let i = (k - key::PROFILE_KEYMAP_BASE) as u8;
                ConfigValue::ProfileKeymap(i, r.u8()?.checked_sub(1).filter(|k| (*k as usize) < KEYMAPS))
            }
            k if profiles(key::PROFILE_FALLBACK_BASE).contains(&k) => {
                let i = (k - key::PROFILE_FALLBACK_BASE) as u8;
                ConfigValue::ProfileFallback(i, r.u8()?.checked_sub(1).filter(|k| (*k as usize) < KEYMAPS))
            }
            k if (key::LAYOUT_BASE..key::LAYOUT_BASE + HOSTS as u16).contains(&k) => {
                let h = Host::from_index((k - key::LAYOUT_BASE) as usize).ok_or(WireError::Invalid)?;
                let l = r.u8()?;
                if l >= LAYOUTS {
                    return Err(WireError::Invalid);
                }
                ConfigValue::HostLayout(h, l)
            }
            k if (key::HOP_UNITS_BASE..key::HOP_UNITS_BASE + 2).contains(&k) => ConfigValue::HopUnits(
                Host::from_index((k - key::HOP_UNITS_BASE) as usize).ok_or(WireError::Invalid)?,
                r.u8()?.try_into()?,
            ),
            k if bt_slots(key::HOP_UNITS_BT_BASE).contains(&k) => ConfigValue::HopUnits(
                Host::bt((k - key::HOP_UNITS_BT_BASE) as u8 + 1).ok_or(WireError::Invalid)?,
                r.u8()?.try_into()?,
            ),
            k if bt_slots(key::BT_POINTER_BASE).contains(&k) => {
                ConfigValue::BtPointer((k - key::BT_POINTER_BASE) as u8, r.u8()?.try_into()?)
            }
            k if (key::SCREEN_BASE..key::SCREEN_BASE + (MAX_ARRANGEMENTS * MAX_SCREENS) as u16).contains(&k) => {
                let b = r.u8()?;
                let host = match b & !(screen_flags::OFF | screen_flags::FIXED) {
                    0 => None,
                    h => Some(h.try_into()?),
                };
                let (w, h, scale) = (r.u16()?, r.u16()?, r.u16()?);
                let mut v = [0i16; 4];
                for x in &mut v {
                    *x = r.u16()? as i16;
                }
                let (off, fixed) =
                    (host.is_some() && b & screen_flags::OFF != 0, host.is_some() && b & screen_flags::FIXED != 0);
                let s = Screen { host, w, h, scale, desk: [v[0], v[1]], at: [v[2], v[3]], off, fixed };
                let n = (k - key::SCREEN_BASE) as usize;
                ConfigValue::Screen((n / MAX_SCREENS) as u8, (n % MAX_SCREENS) as u8, s)
            }
            _ => return Err(WireError::Invalid),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Request {
    GetInfo,
    GetStatus,
    GetPort(Port),
    GetPortString(Port, StringKind),
    SetActive(Target),
    GetConfig(u16),
    SetConfig(ConfigValue),
    SaveConfig,
    FactoryReset,
    Reboot(RebootTarget),
    /// Blink the LEDs for this many seconds.
    Identify(u8),
    Redetect(Port),
    FwBegin {
        target: FwTarget,
        size: u32,
        crc32: u32,
    },
    FwData {
        offset: u32,
        data: Vec<u8, FW_CHUNK>,
    },
    FwCommit,
    FwStatus,
    /// Learn a hotkey from the next keys pressed on the KVM's keyboards (true), or stop (false); software.md §4.5.
    /// The keys go to neither PC meanwhile, and the result comes as `Event::HotkeyLearned`. Changes no setting.
    LearnHotkey(bool),
    /// One page of the diagnostics ([`diag`]), from 0; `Response::Diag` says how many pages there are.
    GetDiag(u8),
    /// The state of Bluetooth host slot 1–4: `Response::BtHost`.
    GetBtHost(u8),
    /// Open (true) or close Bluetooth pairing; it closes by itself after `BT_PAIR_SECONDS`.
    BtPair(bool),
    /// Forget the bond in Bluetooth host slot 1–4 (0xFF = all).
    BtForget(u8),
    /// The passkey a pairing host shows, typed in software instead of on the KVM's keyboard.
    BtPasskey(u32),
    /// The state of Bluetooth device slot 1–4: `Response::BtDevice`.
    GetBtDevice(u8),
    /// Look for Bluetooth devices to pair (true), or stop; it stops by itself after `BT_PAIR_SECONDS`. Host
    /// pairing closes meanwhile: the module pairs one way at a time.
    BtScan(bool),
    /// Entry n of the devices found while looking (`Status.bt_found` of them): `Response::BtFound`.
    GetBtFound(u8),
    /// Pair the device found at this address (`BtFound::addr`) into a free device slot.
    BtPairDevice([u8; 7]),
    /// Forget the bond in Bluetooth device slot 1–4 (0xFF = all) and drop its connection.
    BtForgetDevice(u8),
    /// The report descriptor the KVM holds for a device's `index`-th HID interface (a Bluetooth device's report map,
    /// index 0), from byte `offset`: `Response::DeviceMap`, for the monitor and for diagnosing a device the KVM can't
    /// read (`odkvmctl device map`).
    GetDeviceMap {
        device: Device,
        index: u8,
        offset: u16,
    },
    /// The wireless module's recent log, which the KVM keeps, from byte `at` (counted since it started keeping it):
    /// `Response::ModuleLog` (`odkvmctl device log`).
    GetModuleLog(u32),
    /// Forget every host and device the wireless module has paired, clear every key it holds, and restart it: for
    /// pairings that keep failing on state the module shouldn't hold. The hosts keep the KVM in their own lists.
    BtEraseAll,
    /// Make the ESP32 board on this front port the wireless module, replacing its firmware: one the KVM left alone,
    /// because it runs other firmware (only a blank board, or one running the module's, is taken without asking). The
    /// port is probed again, and the board is taken whatever it runs (even with the port set to passthrough); it then
    /// waits for the module's firmware (`BtModule::NeedsUpdate`). `Busy` on a KVM with a module already.
    AdoptModule(Port),
    /// From a Bluetooth host over the module's control service: let this host control the KVM
    /// (`ConfigValue::BtControl`). `NeedsKeyPress` while the KVM waits for a tap on its key, `Ok` once it may; from
    /// anything but a Bluetooth host `Unsupported`.
    AskControl,
    /// Keycodes of keymap 0–3, layer 0–7, from position `first`: `Response::Keycodes`.
    GetKeycodes {
        keymap: u8,
        layer: u8,
        first: u16,
    },
    /// Sets keycodes of keymap 0–3, layer 0–7, from position `first`.
    SetKeycodes {
        keymap: u8,
        layer: u8,
        first: u16,
        codes: Vec<u16, KEYCODES_PER>,
    },
    /// Axis rules of keymap 0–3, layer 0–7, from input `first`: `Response::AxisRules`.
    GetAxisRules {
        keymap: u8,
        layer: u8,
        first: u8,
    },
    SetAxisRule {
        keymap: u8,
        layer: u8,
        input: u8,
        rule: [u8; RULE_LEN],
    },
    /// Stream the controls of the normalized devices to the asking PC (`Event::Control`, `Event::Axis`): 0 stop, 1
    /// watch, 2 watch and keep them from doing anything else meanwhile (learning a control in the editor). Only from
    /// the active PC, a configuration change, and for `WATCH_MS` (10 s): the host software asks again to go on.
    WatchControls(u8),
    /// What a device has: `Response::Controls`.
    GetControls(Device),
    /// The calibration of a device's gamepad axis 0–9: `Response::Calibration`.
    GetCalibration(Device, u8),
    SetCalibration(Device, u8, AxisCal),
    /// Entry `index` of one kind of the keymaps' shared entries (0 tap dance, 1 combo, 2 key override, 3 alternate
    /// repeat key: `kvm_map::keymap::Dynamic`), in Vial's layout: `Response::MapEntry`.
    GetMapEntry {
        kind: u8,
        index: u8,
    },
    SetMapEntry {
        kind: u8,
        index: u8,
        data: Vec<u8, MAP_ENTRY_MAX>,
    },
    /// Bytes `offset`.. of the macro buffer: `Response::Macros`.
    GetMacros(u16),
    SetMacros {
        offset: u16,
        data: Vec<u8, MACRO_PIECE>,
    },
    /// A QMK setting of the keymaps by Vial's id (qsid): `Response::MapSetting`.
    GetMapSetting(u16),
    SetMapSetting(u16, u32),
    /// Stream a device's raw input reports to the asking PC (`Event::Report`): changed ones, at most about 60 a second
    /// per interface and report ID. As `WatchControls`: from the active PC, a configuration change, for `WATCH_MS`.
    MonitorDevice(Device, bool),
    /// Field override 0–7 of a device: `Response::Override`.
    GetOverride(Device, u8),
    SetOverride(Device, u8, Option<FieldOverride>),
    /// What a host is offered and took: `Response::Outputs`.
    GetOutputs(Host),
    /// Where the asking PC's own cursor is, in its display arrangement's pixels. With screen hop, the KVM
    /// takes it over while it isn't moving the pointer itself: a touchpad or a program moved it.
    PointerAt([i16; 2]),
    /// The asking PC's host software places its cursor on these screens, for [`PLACE_LEASE_MS`] from
    /// now: it asks again before then while it can, and says `Off` when it can't (a screen it may not place on, as
    /// Windows' sign-in screen) or stops. Screen hop then sends it `Event::PlaceCursor` there.
    PlaceCursor(Placement),
}

/// How long a `PlaceCursor` lasts: a host software that stops without saying so (it crashed, the PC went to sleep)
/// costs at most this long without the pointer on its screens.
pub const PLACE_LEASE_MS: u32 = 1000;
/// How often host software renews its `PlaceCursor`.
pub const PLACE_RENEW_MS: u32 = 300;

/// How long Bluetooth pairing stays open.
pub const BT_PAIR_SECONDS: u8 = 60;

/// Longest name of a Bluetooth device the KVM keeps (UTF-8 bytes; a longer one is cut).
pub const BT_NAME_MAX: usize = 29;

/// A Bluetooth device's name, as it calls itself.
pub type BtDeviceName = String<BT_NAME_MAX>;

/// `name` cut to [`BT_NAME_MAX`] bytes on a character boundary.
pub fn bt_device_name(name: &str) -> BtDeviceName {
    let mut end = name.len().min(BT_NAME_MAX);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    String::try_from(&name[..end]).unwrap_or_default()
}

/// One Bluetooth device slot (`Response::BtDevice`); its mode and routing are config values like a front port's.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BtDeviceInfo {
    /// 1–[`BT_DEVICES`].
    pub slot: u8,
    /// `kvm_proto::link::bt::device_flags`.
    pub flags: u8,
    pub rssi: i8,
    /// Battery level in % (0xFF: the device reports none).
    pub battery: u8,
    /// Its GAP appearance (0x03C1 keyboard, 0x03C2 mouse, …; 0: unknown).
    pub appearance: u16,
    /// Identity address: type byte, then 6 bytes little-endian.
    pub addr: [u8; 7],
    pub name: BtDeviceName,
}

/// Bytes of a device's report map per `Response::BtDeviceMap` (header, slot, total, offset and length come first).
pub const MAP_PIECE: usize = 48;
const _: () = assert!(2 + 1 + 2 + 2 + 1 + MAP_PIECE <= REPORT_LEN);
// `Response::ModuleLog` carries as many: header, first, end, at and length come first.
const _: () = assert!(2 + 4 + 4 + 4 + 1 + MAP_PIECE <= REPORT_LEN);

/// A device found while looking for devices to pair (`Response::BtFound`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BtFound {
    /// The address it advertises from: type byte, then 6 bytes little-endian.
    pub addr: [u8; 7],
    pub rssi: i8,
    pub appearance: u16,
    pub name: BtDeviceName,
}

wire_enum! {
    /// Pairing a Bluetooth device (`Status.bt_device_pairing`).
    pub enum DevicePairing {
        Idle = 0,
        /// Connecting to the device picked.
        Connecting = 1,
        /// Pairing. A keyboard that asks for it gets `Status.bt_device_passkey`: type it on that keyboard, then Enter.
        Pairing = 2,
        /// Paired: reading what the device reports, and storing it.
        Reading = 3,
        /// Done: the device is in a slot of its own.
        Paired = 4,
        /// It didn't work (out of reach, refused, not a HID device, no slot free).
        Failed = 5,
    }
}

/// One Bluetooth host slot (`Response::BtHost`); its name and policy are config values (`BtName`, `BtPolicy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BtHostInfo {
    pub host: u8,
    /// `kvm_proto::link::bt::host_flags`.
    pub flags: u8,
    /// Connection interval in units of 1.25 ms (0 = not connected).
    pub interval: u16,
    pub latency: u16,
    pub rssi: i8,
    /// Identity address: type byte, then 6 bytes little-endian.
    pub addr: [u8; 7],
}

pub mod req {
    pub const GET_INFO: u8 = 0x01;
    pub const GET_STATUS: u8 = 0x02;
    pub const GET_PORT: u8 = 0x03;
    pub const GET_PORT_STRING: u8 = 0x04;
    pub const SET_ACTIVE: u8 = 0x05;
    pub const GET_CONFIG: u8 = 0x07;
    pub const SET_CONFIG: u8 = 0x08;
    pub const SAVE_CONFIG: u8 = 0x09;
    pub const FACTORY_RESET: u8 = 0x0A;
    pub const REBOOT: u8 = 0x0B;
    pub const IDENTIFY: u8 = 0x0C;
    pub const REDETECT: u8 = 0x0D;
    pub const LEARN_HOTKEY: u8 = 0x0E;
    pub const GET_DIAG: u8 = 0x0F;
    pub const FW_BEGIN: u8 = 0x10;
    pub const FW_DATA: u8 = 0x11;
    pub const FW_COMMIT: u8 = 0x12;
    pub const FW_STATUS: u8 = 0x13;
    pub const GET_BT_HOST: u8 = 0x14;
    pub const BT_PAIR: u8 = 0x15;
    pub const BT_FORGET: u8 = 0x16;
    pub const BT_PASSKEY: u8 = 0x17;
    pub const POINTER_AT: u8 = 0x18;
    pub const PLACE_CURSOR: u8 = 0x19;
    pub const GET_BT_DEVICE: u8 = 0x1A;
    pub const BT_SCAN: u8 = 0x1B;
    pub const GET_BT_FOUND: u8 = 0x1C;
    pub const BT_PAIR_DEVICE: u8 = 0x1D;
    pub const BT_FORGET_DEVICE: u8 = 0x1E;
    pub const GET_DEVICE_MAP: u8 = 0x1F;
    pub const GET_MODULE_LOG: u8 = 0x20;
    pub const BT_ERASE_ALL: u8 = 0x21;
    pub const GET_KEYCODES: u8 = 0x22;
    pub const SET_KEYCODES: u8 = 0x23;
    pub const GET_AXIS_RULES: u8 = 0x24;
    pub const SET_AXIS_RULE: u8 = 0x25;
    pub const WATCH_CONTROLS: u8 = 0x26;
    pub const GET_CONTROLS: u8 = 0x27;
    pub const GET_CALIBRATION: u8 = 0x28;
    pub const SET_CALIBRATION: u8 = 0x29;
    pub const GET_MAP_ENTRY: u8 = 0x2A;
    pub const SET_MAP_ENTRY: u8 = 0x2B;
    pub const GET_MACROS: u8 = 0x2C;
    pub const SET_MACROS: u8 = 0x2D;
    pub const GET_MAP_SETTING: u8 = 0x2E;
    pub const SET_MAP_SETTING: u8 = 0x2F;
    pub const MONITOR_DEVICE: u8 = 0x30;
    pub const GET_OVERRIDE: u8 = 0x31;
    pub const SET_OVERRIDE: u8 = 0x32;
    pub const GET_OUTPUTS: u8 = 0x33;
    pub const ASK_CONTROL: u8 = 0x34;
    pub const ADOPT_MODULE: u8 = 0x35;
    pub const ERROR: u8 = 0xFF;
}

pub mod ev {
    pub const SWITCHED: u8 = 0xC0;
    pub const PORT_CHANGED: u8 = 0xC1;
    pub const FAULT: u8 = 0xC2;
    pub const CONFIG_CHANGED: u8 = 0xC3;
    pub const HOTKEY_LEARNED: u8 = 0xC4;
    pub const BT_CHANGED: u8 = 0xC5;
    pub const BT_ACTIVE: u8 = 0xC6;
    pub const PLACE_CURSOR: u8 = 0xC7;
    pub const BT_DEVICE_CHANGED: u8 = 0xC8;
    pub const CONTROL: u8 = 0xC9;
    pub const AXIS: u8 = 0xCA;
    pub const REPORT: u8 = 0xCB;
}

impl Request {
    pub fn type_byte(&self) -> u8 {
        match self {
            Request::GetInfo => req::GET_INFO,
            Request::GetStatus => req::GET_STATUS,
            Request::GetPort(_) => req::GET_PORT,
            Request::GetPortString(..) => req::GET_PORT_STRING,
            Request::SetActive(_) => req::SET_ACTIVE,
            Request::GetConfig(_) => req::GET_CONFIG,
            Request::SetConfig(_) => req::SET_CONFIG,
            Request::SaveConfig => req::SAVE_CONFIG,
            Request::FactoryReset => req::FACTORY_RESET,
            Request::Reboot(_) => req::REBOOT,
            Request::Identify(_) => req::IDENTIFY,
            Request::Redetect(_) => req::REDETECT,
            Request::FwBegin { .. } => req::FW_BEGIN,
            Request::FwData { .. } => req::FW_DATA,
            Request::FwCommit => req::FW_COMMIT,
            Request::FwStatus => req::FW_STATUS,
            Request::LearnHotkey(_) => req::LEARN_HOTKEY,
            Request::GetDiag(_) => req::GET_DIAG,
            Request::GetBtHost(_) => req::GET_BT_HOST,
            Request::BtPair(_) => req::BT_PAIR,
            Request::BtForget(_) => req::BT_FORGET,
            Request::BtPasskey(_) => req::BT_PASSKEY,
            Request::PointerAt(_) => req::POINTER_AT,
            Request::PlaceCursor(_) => req::PLACE_CURSOR,
            Request::GetBtDevice(_) => req::GET_BT_DEVICE,
            Request::BtScan(_) => req::BT_SCAN,
            Request::GetBtFound(_) => req::GET_BT_FOUND,
            Request::GetDeviceMap { .. } => req::GET_DEVICE_MAP,
            Request::GetModuleLog(_) => req::GET_MODULE_LOG,
            Request::BtEraseAll => req::BT_ERASE_ALL,
            Request::AdoptModule(_) => req::ADOPT_MODULE,
            Request::AskControl => req::ASK_CONTROL,
            Request::GetKeycodes { .. } => req::GET_KEYCODES,
            Request::SetKeycodes { .. } => req::SET_KEYCODES,
            Request::GetAxisRules { .. } => req::GET_AXIS_RULES,
            Request::SetAxisRule { .. } => req::SET_AXIS_RULE,
            Request::WatchControls(_) => req::WATCH_CONTROLS,
            Request::GetControls(_) => req::GET_CONTROLS,
            Request::GetCalibration(..) => req::GET_CALIBRATION,
            Request::SetCalibration(..) => req::SET_CALIBRATION,
            Request::GetMapEntry { .. } => req::GET_MAP_ENTRY,
            Request::SetMapEntry { .. } => req::SET_MAP_ENTRY,
            Request::GetMacros(_) => req::GET_MACROS,
            Request::SetMacros { .. } => req::SET_MACROS,
            Request::GetMapSetting(_) => req::GET_MAP_SETTING,
            Request::SetMapSetting(..) => req::SET_MAP_SETTING,
            Request::MonitorDevice(..) => req::MONITOR_DEVICE,
            Request::GetOverride(..) => req::GET_OVERRIDE,
            Request::SetOverride(..) => req::SET_OVERRIDE,
            Request::GetOutputs(_) => req::GET_OUTPUTS,
            Request::BtPairDevice(_) => req::BT_PAIR_DEVICE,
            Request::BtForgetDevice(_) => req::BT_FORGET_DEVICE,
        }
    }

    /// Requests that change state and are refused while software configuration is locked.
    pub fn is_config_change(&self) -> bool {
        matches!(
            self,
            Request::SetConfig(_)
                | Request::SaveConfig
                | Request::FactoryReset
                | Request::Redetect(_)
                | Request::FwBegin { .. }
                | Request::FwData { .. }
                | Request::FwCommit
                | Request::Reboot(_)
                | Request::LearnHotkey(true)
                | Request::BtPair(true)
                | Request::BtForget(_)
                | Request::BtScan(true)
                | Request::BtPairDevice(_)
                | Request::BtForgetDevice(_)
                | Request::BtEraseAll
                | Request::AdoptModule(_)
                | Request::SetKeycodes { .. }
                | Request::SetAxisRule { .. }
                | Request::SetCalibration(..)
                | Request::WatchControls(1..=u8::MAX)
                | Request::SetMapEntry { .. }
                | Request::SetMacros { .. }
                | Request::SetMapSetting(..)
                | Request::MonitorDevice(_, true)
                | Request::SetOverride(..)
        )
    }

    pub fn encode(&self, id: u8) -> Result<[u8; REPORT_LEN], WireError> {
        let mut out = [0u8; REPORT_LEN];
        let mut w = Writer::new(&mut out);
        w.u8(self.type_byte())?;
        w.u8(id)?;
        match self {
            Request::GetInfo
            | Request::GetStatus
            | Request::SaveConfig
            | Request::FactoryReset
            | Request::FwCommit
            | Request::FwStatus
            | Request::BtEraseAll
            | Request::AskControl => {}
            Request::GetPort(p) | Request::Redetect(p) | Request::AdoptModule(p) => w.u8(*p as u8)?,
            Request::GetPortString(p, k) => {
                w.u8(*p as u8)?;
                w.u8(*k as u8)?;
            }
            Request::SetActive(t) => w.u8(*t as u8)?,
            Request::GetConfig(k) => w.u16(*k)?,
            Request::SetConfig(v) => v.encode(&mut w)?,
            Request::Reboot(t) => w.u8(*t as u8)?,
            Request::Identify(s)
            | Request::GetDiag(s)
            | Request::GetBtHost(s)
            | Request::BtForget(s)
            | Request::GetBtDevice(s)
            | Request::GetBtFound(s)
            | Request::BtForgetDevice(s) => w.u8(*s)?,
            Request::BtPair(on) | Request::BtScan(on) => w.bool(*on)?,
            Request::BtPairDevice(a) => w.bytes(a)?,
            Request::GetDeviceMap { device, index, offset } => {
                w.u8(*device as u8)?;
                w.u8(*index)?;
                w.u16(*offset)?;
            }
            Request::GetModuleLog(at) => w.u32(*at)?,
            Request::GetKeycodes { keymap, layer, first } => {
                w.u8(*keymap)?;
                w.u8(*layer)?;
                w.u16(*first)?;
            }
            Request::SetKeycodes { keymap, layer, first, codes } => {
                w.u8(*keymap)?;
                w.u8(*layer)?;
                w.u16(*first)?;
                w.u8(codes.len() as u8)?;
                for c in codes {
                    w.u16(*c)?;
                }
            }
            Request::GetAxisRules { keymap, layer, first } => {
                w.u8(*keymap)?;
                w.u8(*layer)?;
                w.u8(*first)?;
            }
            Request::SetAxisRule { keymap, layer, input, rule } => {
                w.u8(*keymap)?;
                w.u8(*layer)?;
                w.u8(*input)?;
                w.bytes(rule)?;
            }
            Request::WatchControls(m) => w.u8(*m)?,
            Request::GetControls(d) => w.u8(*d as u8)?,
            Request::GetCalibration(d, a) => {
                w.u8(*d as u8)?;
                w.u8(*a)?;
            }
            Request::SetCalibration(d, a, c) => {
                w.u8(*d as u8)?;
                w.u8(*a)?;
                axis_cal(&mut w, c)?;
            }
            Request::GetMapEntry { kind, index } => {
                w.u8(*kind)?;
                w.u8(*index)?;
            }
            Request::SetMapEntry { kind, index, data } => {
                w.u8(*kind)?;
                w.u8(*index)?;
                w.lv(data)?;
            }
            Request::GetMacros(offset) => w.u16(*offset)?,
            Request::SetMacros { offset, data } => {
                w.u16(*offset)?;
                w.lv(data)?;
            }
            Request::GetMapSetting(q) => w.u16(*q)?,
            Request::SetMapSetting(q, v) => {
                w.u16(*q)?;
                w.u32(*v)?;
            }
            Request::MonitorDevice(d, on) => {
                w.u8(*d as u8)?;
                w.bool(*on)?;
            }
            Request::GetOverride(d, n) => {
                w.u8(*d as u8)?;
                w.u8(*n)?;
            }
            Request::SetOverride(d, n, o) => {
                w.u8(*d as u8)?;
                w.u8(*n)?;
                field_override(&mut w, o)?;
            }
            Request::GetOutputs(h) => w.u8(*h as u8)?,
            Request::BtPasskey(v) => w.u32(*v)?,
            Request::PointerAt([x, y]) => {
                w.u16(*x as u16)?;
                w.u16(*y as u16)?;
            }
            Request::PlaceCursor(p) => w.u8(*p as u8)?,
            Request::LearnHotkey(start) => w.bool(*start)?,
            Request::FwBegin { target, size, crc32 } => {
                w.u8(*target as u8)?;
                w.u32(*size)?;
                w.u32(*crc32)?;
            }
            Request::FwData { offset, data } => {
                w.u32(*offset)?;
                w.lv(data)?;
            }
        }
        Ok(out)
    }

    /// Decodes a request report; returns the request id with it.
    pub fn decode(report: &[u8]) -> Result<(u8, Request), WireError> {
        let mut r = Reader::new(report);
        let t = r.u8()?;
        let id = r.u8()?;
        let req = match t {
            req::GET_INFO => Request::GetInfo,
            req::GET_STATUS => Request::GetStatus,
            req::GET_PORT => Request::GetPort(r.u8()?.try_into()?),
            req::GET_PORT_STRING => Request::GetPortString(r.u8()?.try_into()?, r.u8()?.try_into()?),
            req::SET_ACTIVE => Request::SetActive(r.u8()?.try_into()?),
            req::GET_CONFIG => Request::GetConfig(r.u16()?),
            req::SET_CONFIG => Request::SetConfig(ConfigValue::decode(&mut r)?),
            req::SAVE_CONFIG => Request::SaveConfig,
            req::FACTORY_RESET => Request::FactoryReset,
            req::REBOOT => Request::Reboot(r.u8()?.try_into()?),
            req::IDENTIFY => Request::Identify(r.u8()?),
            req::REDETECT => Request::Redetect(r.u8()?.try_into()?),
            req::FW_BEGIN => Request::FwBegin { target: r.u8()?.try_into()?, size: r.u32()?, crc32: r.u32()? },
            req::FW_DATA => {
                Request::FwData { offset: r.u32()?, data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Invalid)? }
            }
            req::FW_COMMIT => Request::FwCommit,
            req::FW_STATUS => Request::FwStatus,
            req::LEARN_HOTKEY => Request::LearnHotkey(r.bool()?),
            req::GET_DIAG => Request::GetDiag(r.u8()?),
            req::GET_BT_HOST => Request::GetBtHost(r.u8()?),
            req::BT_PAIR => Request::BtPair(r.bool()?),
            req::BT_FORGET => Request::BtForget(r.u8()?),
            req::BT_PASSKEY => Request::BtPasskey(r.u32()?),
            req::POINTER_AT => Request::PointerAt([r.u16()? as i16, r.u16()? as i16]),
            req::PLACE_CURSOR => Request::PlaceCursor(r.u8()?.try_into()?),
            req::GET_BT_DEVICE => Request::GetBtDevice(r.u8()?),
            req::BT_SCAN => Request::BtScan(r.bool()?),
            req::GET_BT_FOUND => Request::GetBtFound(r.u8()?),
            req::GET_DEVICE_MAP => Request::GetDeviceMap { device: device(&mut r)?, index: r.u8()?, offset: r.u16()? },
            req::GET_MODULE_LOG => Request::GetModuleLog(r.u32()?),
            req::BT_ERASE_ALL => Request::BtEraseAll,
            req::ADOPT_MODULE => Request::AdoptModule(r.u8()?.try_into()?),
            req::ASK_CONTROL => Request::AskControl,
            req::GET_KEYCODES => Request::GetKeycodes { keymap: r.u8()?, layer: r.u8()?, first: r.u16()? },
            req::SET_KEYCODES => {
                let (keymap, layer, first, n) = (r.u8()?, r.u8()?, r.u16()?, r.u8()? as usize);
                let mut codes = Vec::new();
                for _ in 0..n {
                    codes.push(r.u16()?).map_err(|_| WireError::Overflow)?;
                }
                Request::SetKeycodes { keymap, layer, first, codes }
            }
            req::GET_AXIS_RULES => Request::GetAxisRules { keymap: r.u8()?, layer: r.u8()?, first: r.u8()? },
            req::SET_AXIS_RULE => {
                Request::SetAxisRule { keymap: r.u8()?, layer: r.u8()?, input: r.u8()?, rule: r.array()? }
            }
            req::WATCH_CONTROLS => Request::WatchControls(r.u8()?),
            req::GET_CONTROLS => Request::GetControls(device(&mut r)?),
            req::GET_CALIBRATION => Request::GetCalibration(device(&mut r)?, r.u8()?),
            req::SET_CALIBRATION => Request::SetCalibration(device(&mut r)?, r.u8()?, read_axis_cal(&mut r)?),
            req::GET_MAP_ENTRY => Request::GetMapEntry { kind: r.u8()?, index: r.u8()? },
            req::SET_MAP_ENTRY => Request::SetMapEntry {
                kind: r.u8()?,
                index: r.u8()?,
                data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Overflow)?,
            },
            req::GET_MACROS => Request::GetMacros(r.u16()?),
            req::SET_MACROS => Request::SetMacros {
                offset: r.u16()?,
                data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Overflow)?,
            },
            req::GET_MAP_SETTING => Request::GetMapSetting(r.u16()?),
            req::SET_MAP_SETTING => Request::SetMapSetting(r.u16()?, r.u32()?),
            req::MONITOR_DEVICE => Request::MonitorDevice(device(&mut r)?, r.bool()?),
            req::GET_OVERRIDE => Request::GetOverride(device(&mut r)?, r.u8()?),
            req::SET_OVERRIDE => Request::SetOverride(device(&mut r)?, r.u8()?, read_field_override(&mut r)?),
            req::GET_OUTPUTS => Request::GetOutputs(r.u8()?.try_into()?),
            req::BT_PAIR_DEVICE => Request::BtPairDevice(r.array()?),
            req::BT_FORGET_DEVICE => Request::BtForgetDevice(r.u8()?),
            _ => return Err(WireError::Invalid),
        };
        Ok((id, req))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Response {
    Info(Info),
    Status(Status),
    Port(PortInfo),
    PortString(Port, StringKind, String<STRING_MAX>),
    Config(ConfigValue),
    Fw {
        state: FwState,
        progress: u32,
    },
    /// Page `page` of `pages` of the diagnostics.
    Diag {
        page: u8,
        pages: u8,
        items: Vec<diag::Item, { diag::PER_PAGE }>,
    },
    BtHost(BtHostInfo),
    BtDevice(BtDeviceInfo),
    /// Entry n of the devices found, None past the last.
    BtFound(Option<BtFound>),
    /// Bytes `offset`.. of the report descriptor of a device's `index`-th HID interface (number `iface`, of
    /// `interfaces`), `total` long (0: none: no device, no bond, or not read yet).
    DeviceMap {
        device: Device,
        index: u8,
        iface: u8,
        interfaces: u8,
        total: u16,
        offset: u16,
        data: Vec<u8, MAP_PIECE>,
    },
    /// Bytes `at`.. of the module's log, which the KVM holds from byte `first` to `end` (counted since it started
    /// keeping it; older lines made room for newer ones).
    ModuleLog {
        first: u32,
        end: u32,
        at: u32,
        data: Vec<u8, MAP_PIECE>,
    },
    Keycodes {
        keymap: u8,
        layer: u8,
        first: u16,
        codes: Vec<u16, KEYCODES_PER>,
    },
    AxisRules {
        keymap: u8,
        layer: u8,
        first: u8,
        rules: Vec<[u8; RULE_LEN], RULES_PER>,
    },
    Controls(Device, Controls),
    Calibration(Device, u8, AxisCal),
    MapEntry {
        kind: u8,
        index: u8,
        data: Vec<u8, MAP_ENTRY_MAX>,
    },
    /// Bytes `offset`.. of the macro buffer, `total` long.
    Macros {
        offset: u16,
        total: u16,
        data: Vec<u8, MACRO_PIECE>,
    },
    MapSetting(u16, u32),
    Override(Device, u8, Option<FieldOverride>),
    /// [`outputs`] bits: what the host is offered, and what it took (a Bluetooth host: the reports it subscribed to;
    /// a PC: everything offered while its link is up).
    Outputs {
        host: Host,
        offered: u8,
        taken: u8,
    },
    Ok,
    Error(ErrorCode),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event {
    Switched(Pc, SwitchSource),
    PortChanged(Port),
    Fault(FaultKind, Option<Port>),
    ConfigChanged(u16),
    /// The hotkey learned (`Hotkey::NONE` unless `Learned`), to the PC that asked for it.
    HotkeyLearned(LearnEnd, Hotkey),
    /// Bluetooth host slot 1–4 changed (connected, bonded, ...), or 0: pairing opened, closed, or wants a passkey.
    BtChanged(u8),
    /// Input now goes to Bluetooth host slot 1–4, or 0: back to a PC (then `Switched` follows).
    BtActive(u8),
    /// Bluetooth device slot 1–4 changed (paired, connected, its battery, …), or 0: looking for devices started,
    /// found one, stopped, or a pairing moved on.
    BtDeviceChanged(u8),
    /// Put the cursor here, in this PC's own pixels: only to a PC whose host software asked for it
    /// (`Request::PlaceCursor`), at most once a millisecond, the latest position.
    PlaceCursor([i16; 2]),
    /// A control of a normalized device went down or up (`Request::WatchControls`): its position (`kvm_map::pos`).
    Control(u16, bool),
    /// An axis input (`kvm_map::axes::input`) moved: its value (absolute), or the motion since the last (relative).
    Axis(u8, i16),
    /// Bytes `offset`.. of a raw input report of the monitored device's interface `iface`, `total` long, its report ID
    /// first when the interface has them (`Request::MonitorDevice`).
    Report {
        device: Device,
        iface: u8,
        offset: u8,
        total: u8,
        /// The first `len` bytes count.
        len: u8,
        data: [u8; REPORT_PIECE],
    },
}

/// Anything the device sends.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum DeviceMsg {
    Response { id: u8, request: u8, response: Response },
    Event(Event),
}

fn advertised(r: &mut Reader) -> Result<Advertised, WireError> {
    r.u8()?.try_into()
}

fn device(r: &mut Reader) -> Result<Device, WireError> {
    Device::from_index(r.u8()? as usize).ok_or(WireError::Invalid)
}

fn field_override(w: &mut Writer, o: &Option<FieldOverride>) -> Result<(), WireError> {
    w.bool(o.is_some())?;
    if let Some(o) = o {
        w.u8(o.iface)?;
        w.u8(o.report_id)?;
        w.u16(o.bit)?;
        w.u8(o.bits)?;
        w.u8(o.flags)?;
        w.u8(o.target)?;
    }
    Ok(())
}

fn read_field_override(r: &mut Reader) -> Result<Option<FieldOverride>, WireError> {
    if !r.bool()? {
        return Ok(None);
    }
    let o = FieldOverride {
        iface: r.u8()?,
        report_id: r.u8()?,
        bit: r.u16()?,
        bits: r.u8()?,
        flags: r.u8()?,
        target: r.u8()?,
    };
    let target_ok = (o.target as usize) < 32 || (field::AXIS..field::AXIS + 10).contains(&o.target);
    if !(1..=16).contains(&o.bits) || !target_ok {
        return Err(WireError::Invalid);
    }
    Ok(Some(o))
}

fn axis_cal(w: &mut Writer, c: &AxisCal) -> Result<(), WireError> {
    w.u16(c.min as u16)?;
    w.u16(c.centre as u16)?;
    w.u16(c.max as u16)?;
    w.u8(c.deadzone)
}

fn read_axis_cal(r: &mut Reader) -> Result<AxisCal, WireError> {
    Ok(AxisCal { min: r.u16()? as i16, centre: r.u16()? as i16, max: r.u16()? as i16, deadzone: r.u8()? })
}

fn device_name(r: &mut Reader) -> Result<BtDeviceName, WireError> {
    let s = core::str::from_utf8(r.lv()?).map_err(|_| WireError::Invalid)?;
    String::try_from(s).map_err(|_| WireError::Invalid)
}

impl Response {
    /// Encodes the response to a request of type `request` with id `id`.
    pub fn encode(&self, request: u8, id: u8) -> Result<[u8; REPORT_LEN], WireError> {
        let mut out = [0u8; REPORT_LEN];
        let mut w = Writer::new(&mut out);
        let t = match self {
            Response::Error(_) => req::ERROR,
            _ => request | 0x80,
        };
        w.u8(t)?;
        w.u8(id)?;
        match self {
            Response::Info(i) => {
                w.u8(i.proto)?;
                w.u32(i.rp_fw)?;
                w.u32(i.at_fw[0])?;
                w.u32(i.at_fw[1])?;
                w.u32(i.hw)?;
                w.bytes(&i.serial)?;
                w.u16(i.caps)?;
                w.u32(i.bt_fw)?;
            }
            Response::Status(s) => {
                w.u8(s.active as u8)?;
                w.u8(s.asking.map_or(0, |h| h as u8))?;
                w.u8(s.links[0] as u8)?;
                w.u8(s.links[1] as u8)?;
                w.u8(s.power as u8)?;
                w.u8(s.adv_pwr as u8)?;
                w.u8(s.adv_pc[0] as u8)?;
                w.u8(s.adv_pc[1] as u8)?;
                for v in s.vbus_mv {
                    w.u16(v)?;
                }
                w.u16(s.budget_ma)?;
                w.u8(s.flags)?;
                w.u32(s.uptime_s)?;
                w.u16(s.link_errors[0])?;
                w.u16(s.link_errors[1])?;
                w.u16(s.selftest as u16)?;
                w.u8(s.bt_active)?;
                w.u8(s.bt_ready)?;
                w.u8(s.bt_bonded)?;
                w.u8(s.bt_pairing)?;
                w.u8(s.bt_passkey)?;
                // The chip in the high nibble: the report has no byte left.
                w.u8(s.bt_module as u8 | (s.bt_chip as u8) << 4)?;
                w.u16((s.selftest >> 16) as u16)?;
                w.u8(s.hop as u8)?;
                w.u16(s.pointer[0] as u16)?;
                w.u16(s.pointer[1] as u16)?;
                w.u8(s.bt_passkey_digits)?;
                w.u32(s.bt_passkey_typed)?;
                w.u8(s.supply as u8)?;
                w.u16(s.supply_mv)?;
                w.u16(s.supply_lowest_mv)?;
                w.u16(s.declared_ma)?;
                w.u8(s.bt_devices_bonded)?;
                w.u8(s.bt_devices_ready)?;
                w.u8(s.bt_scan)?;
                w.u8(s.bt_found)?;
                w.u8(s.bt_device_pairing as u8)?;
                w.u32(s.bt_device_passkey.unwrap_or(u32::MAX))?;
                w.u8(s.bt_progress)?;
            }
            Response::Port(p) => {
                w.u8(p.port as u8)?;
                w.u8(p.state as u8)?;
                w.u8(p.mode as u8)?;
                w.u8(p.effective as u8)?;
                w.u8(p.target.map_or(0, |h| h as u8))?;
                w.u8(p.reason as u8)?;
                w.u16(p.vid)?;
                w.u16(p.pid)?;
                w.u8(p.speed as u8)?;
                w.u16(p.classes)?;
                w.u8(p.interfaces)?;
                w.u16(p.report_rate_hz)?;
                w.u16(p.max_power_ma)?;
                w.u16(p.faults)?;
            }
            Response::PortString(p, k, s) => {
                w.u8(*p as u8)?;
                w.u8(*k as u8)?;
                w.lv(s.as_bytes())?;
            }
            Response::Config(v) => v.encode(&mut w)?,
            Response::Fw { state, progress } => {
                w.u8(*state as u8)?;
                w.u32(*progress)?;
            }
            Response::Diag { page, pages, items } => {
                w.u8(*page)?;
                w.u8(*pages)?;
                w.u8(items.len() as u8)?;
                for i in items {
                    w.u8(i.id)?;
                    w.u8(i.grade as u8)?;
                    w.u32(i.value as u32)?;
                }
            }
            Response::BtHost(h) => {
                w.u8(h.host)?;
                w.u8(h.flags)?;
                w.u16(h.interval)?;
                w.u16(h.latency)?;
                w.u8(h.rssi as u8)?;
                w.bytes(&h.addr)?;
            }
            Response::BtDevice(d) => {
                w.u8(d.slot)?;
                w.u8(d.flags)?;
                w.u8(d.rssi as u8)?;
                w.u8(d.battery)?;
                w.u16(d.appearance)?;
                w.bytes(&d.addr)?;
                w.lv(d.name.as_bytes())?;
            }
            Response::DeviceMap { device, index, iface, interfaces, total, offset, data } => {
                w.u8(*device as u8)?;
                w.u8(*index)?;
                w.u8(*iface)?;
                w.u8(*interfaces)?;
                w.u16(*total)?;
                w.u16(*offset)?;
                w.lv(data)?;
            }
            Response::MapEntry { kind, index, data } => {
                w.u8(*kind)?;
                w.u8(*index)?;
                w.lv(data)?;
            }
            Response::Macros { offset, total, data } => {
                w.u16(*offset)?;
                w.u16(*total)?;
                w.lv(data)?;
            }
            Response::MapSetting(q, v) => {
                w.u16(*q)?;
                w.u32(*v)?;
            }
            Response::Override(d, n, o) => {
                w.u8(*d as u8)?;
                w.u8(*n)?;
                field_override(&mut w, o)?;
            }
            Response::Outputs { host, offered, taken } => {
                w.u8(*host as u8)?;
                w.u8(*offered)?;
                w.u8(*taken)?;
            }
            Response::ModuleLog { first, end, at, data } => {
                w.u32(*first)?;
                w.u32(*end)?;
                w.u32(*at)?;
                w.lv(data)?;
            }
            Response::Keycodes { keymap, layer, first, codes } => {
                w.u8(*keymap)?;
                w.u8(*layer)?;
                w.u16(*first)?;
                w.u8(codes.len() as u8)?;
                for c in codes {
                    w.u16(*c)?;
                }
            }
            Response::AxisRules { keymap, layer, first, rules } => {
                w.u8(*keymap)?;
                w.u8(*layer)?;
                w.u8(*first)?;
                w.u8(rules.len() as u8)?;
                for r in rules {
                    w.bytes(r)?;
                }
            }
            Response::Controls(d, c) => {
                w.u8(*d as u8)?;
                w.bool(c.keyboard)?;
                w.u16(c.mouse_buttons)?;
                w.u32(c.pad_buttons)?;
                w.u8(c.mouse_axes)?;
                w.u16(c.pad_axes)?;
                w.bool(c.hat)?;
                w.u32(c.identity)?;
            }
            Response::Calibration(d, a, c) => {
                w.u8(*d as u8)?;
                w.u8(*a)?;
                axis_cal(&mut w, c)?;
            }
            Response::BtFound(f) => {
                w.bool(f.is_some())?;
                if let Some(f) = f {
                    w.bytes(&f.addr)?;
                    w.u8(f.rssi as u8)?;
                    w.u16(f.appearance)?;
                    w.lv(f.name.as_bytes())?;
                }
            }
            Response::Ok => {}
            Response::Error(code) => {
                w.u8(request)?;
                w.u8(*code as u8)?;
            }
        }
        Ok(out)
    }
}

impl Event {
    pub fn encode(&self) -> Result<[u8; REPORT_LEN], WireError> {
        let mut out = [0u8; REPORT_LEN];
        let mut w = Writer::new(&mut out);
        match *self {
            Event::Switched(pc, src) => {
                w.u8(ev::SWITCHED)?;
                w.u8(0)?;
                w.u8(pc as u8)?;
                w.u8(src as u8)?;
            }
            Event::PortChanged(p) => {
                w.u8(ev::PORT_CHANGED)?;
                w.u8(0)?;
                w.u8(p as u8)?;
            }
            Event::Fault(kind, port) => {
                w.u8(ev::FAULT)?;
                w.u8(0)?;
                w.u8(kind as u8)?;
                w.u8(port.map_or(0xFF, |p| p as u8))?;
            }
            Event::ConfigChanged(k) => {
                w.u8(ev::CONFIG_CHANGED)?;
                w.u8(0)?;
                w.u16(k)?;
            }
            Event::HotkeyLearned(end, h) => {
                w.u8(ev::HOTKEY_LEARNED)?;
                w.u8(0)?;
                w.u8(end as u8)?;
                w.u8(h.kind as u8)?;
                w.u8(h.modifiers)?;
                w.u8(h.key)?;
                w.bool(h.swallow)?;
            }
            Event::BtChanged(n) | Event::BtActive(n) | Event::BtDeviceChanged(n) => {
                w.u8(match self {
                    Event::BtChanged(_) => ev::BT_CHANGED,
                    Event::BtActive(_) => ev::BT_ACTIVE,
                    _ => ev::BT_DEVICE_CHANGED,
                })?;
                w.u8(0)?;
                w.u8(n)?;
            }
            Event::PlaceCursor([x, y]) => {
                w.u8(ev::PLACE_CURSOR)?;
                w.u8(0)?;
                w.u16(x as u16)?;
                w.u16(y as u16)?;
            }
            Event::Control(p, down) => {
                w.u8(ev::CONTROL)?;
                w.u8(0)?;
                w.u16(p)?;
                w.bool(down)?;
            }
            Event::Axis(i, v) => {
                w.u8(ev::AXIS)?;
                w.u8(0)?;
                w.u8(i)?;
                w.u16(v as u16)?;
            }
            Event::Report { device, iface, offset, total, len, ref data } => {
                w.u8(ev::REPORT)?;
                w.u8(0)?;
                w.u8(device as u8)?;
                w.u8(iface)?;
                w.u8(offset)?;
                w.u8(total)?;
                w.lv(&data[..(len as usize).min(REPORT_PIECE)])?;
            }
        }
        Ok(out)
    }
}

impl DeviceMsg {
    pub fn decode(report: &[u8]) -> Result<DeviceMsg, WireError> {
        let mut r = Reader::new(report);
        let t = r.u8()?;
        let id = r.u8()?;
        match t {
            ev::SWITCHED => return Ok(DeviceMsg::Event(Event::Switched(r.u8()?.try_into()?, r.u8()?.try_into()?))),
            ev::PORT_CHANGED => return Ok(DeviceMsg::Event(Event::PortChanged(r.u8()?.try_into()?))),
            ev::FAULT => {
                let kind = r.u8()?.try_into()?;
                let port = match r.u8()? {
                    0xFF => None,
                    p => Some(p.try_into()?),
                };
                return Ok(DeviceMsg::Event(Event::Fault(kind, port)));
            }
            ev::CONFIG_CHANGED => return Ok(DeviceMsg::Event(Event::ConfigChanged(r.u16()?))),
            ev::BT_CHANGED => return Ok(DeviceMsg::Event(Event::BtChanged(r.u8()?))),
            ev::BT_ACTIVE => return Ok(DeviceMsg::Event(Event::BtActive(r.u8()?))),
            ev::BT_DEVICE_CHANGED => return Ok(DeviceMsg::Event(Event::BtDeviceChanged(r.u8()?))),
            ev::PLACE_CURSOR => {
                return Ok(DeviceMsg::Event(Event::PlaceCursor([r.u16()? as i16, r.u16()? as i16])));
            }
            ev::CONTROL => return Ok(DeviceMsg::Event(Event::Control(r.u16()?, r.bool()?))),
            ev::AXIS => return Ok(DeviceMsg::Event(Event::Axis(r.u8()?, r.u16()? as i16))),
            ev::REPORT => {
                let (device, iface, offset, total) = (device(&mut r)?, r.u8()?, r.u8()?, r.u8()?);
                let b = r.lv()?;
                let mut data = [0u8; REPORT_PIECE];
                data.get_mut(..b.len()).ok_or(WireError::Overflow)?.copy_from_slice(b);
                return Ok(DeviceMsg::Event(Event::Report { device, iface, offset, total, len: b.len() as u8, data }));
            }
            ev::HOTKEY_LEARNED => {
                let end = r.u8()?.try_into()?;
                let h = Hotkey { kind: r.u8()?.try_into()?, modifiers: r.u8()?, key: r.u8()?, swallow: r.bool()? };
                return Ok(DeviceMsg::Event(Event::HotkeyLearned(end, h)));
            }
            _ => {}
        }
        if t == req::ERROR {
            let request = r.u8()?;
            let code = r.u8()?.try_into()?;
            return Ok(DeviceMsg::Response { id, request, response: Response::Error(code) });
        }
        if t & 0x80 == 0 {
            return Err(WireError::Invalid);
        }
        let request = t & 0x7F;
        let response = match request {
            req::GET_INFO => Response::Info(Info {
                proto: r.u8()?,
                rp_fw: r.u32()?,
                at_fw: [r.u32()?, r.u32()?],
                hw: r.u32()?,
                serial: r.array()?,
                caps: r.u16()?,
                bt_fw: r.u32()?,
            }),
            req::GET_STATUS => {
                let module: u8;
                let mut st = Status {
                    active: r.u8()?.try_into()?,
                    asking: match r.u8()? {
                        0 => None,
                        h => Some(h.try_into()?),
                    },
                    links: [r.u8()?.try_into()?, r.u8()?.try_into()?],
                    power: r.u8()?.try_into()?,
                    adv_pwr: advertised(&mut r)?,
                    adv_pc: [advertised(&mut r)?, advertised(&mut r)?],
                    vbus_mv: [r.u16()?, r.u16()?, r.u16()?],
                    budget_ma: r.u16()?,
                    flags: r.u8()?,
                    uptime_s: r.u32()?,
                    link_errors: [r.u16()?, r.u16()?],
                    selftest: r.u16()? as u32,
                    bt_active: r.u8()?,
                    bt_ready: r.u8()?,
                    bt_bonded: r.u8()?,
                    bt_pairing: r.u8()?,
                    bt_passkey: r.u8()?,
                    bt_module: {
                        module = r.u8()?;
                        diag::BtModule::from_u8(module & 0x0F).ok_or(WireError::Invalid)?
                    },
                    hop: HopState::Off,
                    pointer: [0, 0],
                    bt_passkey_digits: 0,
                    bt_passkey_typed: 0,
                    supply: Supply::Unknown,
                    supply_mv: 0,
                    supply_lowest_mv: 0,
                    declared_ma: 0,
                    bt_devices_bonded: 0,
                    bt_devices_ready: 0,
                    bt_scan: 0,
                    bt_found: 0,
                    bt_device_pairing: DevicePairing::Idle,
                    bt_device_passkey: None,
                    bt_progress: 0,
                    bt_chip: crate::link::bt::Chip::from_u8(module >> 4),
                };
                // The self-test's bits 16–31, then screen hop, the passkey typed and the supply.
                st.selftest |= (r.u16()? as u32) << 16;
                st.hop = HopState::from_u8(r.u8()?).ok_or(WireError::Invalid)?;
                st.pointer = [r.u16()? as i16, r.u16()? as i16];
                st.bt_passkey_digits = r.u8()?;
                st.bt_passkey_typed = r.u32()?;
                st.supply = Supply::from_u8(r.u8()?).ok_or(WireError::Invalid)?;
                st.supply_mv = r.u16()?;
                st.supply_lowest_mv = r.u16()?;
                st.declared_ma = r.u16()?;
                st.bt_devices_bonded = r.u8()?;
                st.bt_devices_ready = r.u8()?;
                st.bt_scan = r.u8()?;
                st.bt_found = r.u8()?;
                st.bt_device_pairing = r.u8()?.try_into()?;
                st.bt_device_passkey = Some(r.u32()?).filter(|v| *v != u32::MAX);
                st.bt_progress = r.u8()?;
                Response::Status(st)
            }
            req::GET_PORT => Response::Port(PortInfo {
                port: r.u8()?.try_into()?,
                state: r.u8()?.try_into()?,
                mode: r.u8()?.try_into()?,
                effective: r.u8()?.try_into()?,
                target: match r.u8()? {
                    0 => None,
                    h => Some(h.try_into()?),
                },
                reason: r.u8()?.try_into()?,
                vid: r.u16()?,
                pid: r.u16()?,
                speed: r.u8()?.try_into()?,
                classes: r.u16()?,
                interfaces: r.u8()?,
                report_rate_hz: r.u16()?,
                max_power_ma: r.u16()?,
                faults: r.u16()?,
            }),
            req::GET_PORT_STRING => {
                let port = r.u8()?.try_into()?;
                let kind = r.u8()?.try_into()?;
                let s = core::str::from_utf8(r.lv()?).map_err(|_| WireError::Invalid)?;
                Response::PortString(port, kind, String::try_from(s).map_err(|_| WireError::Invalid)?)
            }
            req::GET_CONFIG | req::SET_CONFIG => Response::Config(ConfigValue::decode(&mut r)?),
            req::FW_BEGIN | req::FW_DATA | req::FW_COMMIT | req::FW_STATUS => {
                Response::Fw { state: r.u8()?.try_into()?, progress: r.u32()? }
            }
            req::GET_DIAG => {
                let (page, pages, n) = (r.u8()?, r.u8()?, r.u8()?);
                let mut items = Vec::new();
                for _ in 0..n {
                    // A grade this version doesn't know is shown without a colour.
                    let (id, grade, value) = (r.u8()?, r.u8()?, r.u32()? as i32);
                    let grade = diag::Grade::from_u8(grade).unwrap_or(diag::Grade::Info);
                    items.push(diag::Item { id, grade, value }).map_err(|_| WireError::Invalid)?;
                }
                Response::Diag { page, pages, items }
            }
            req::GET_BT_HOST => Response::BtHost(BtHostInfo {
                host: r.u8()?,
                flags: r.u8()?,
                interval: r.u16()?,
                latency: r.u16()?,
                rssi: r.u8()? as i8,
                addr: r.array()?,
            }),
            req::GET_BT_DEVICE => Response::BtDevice(BtDeviceInfo {
                slot: r.u8()?,
                flags: r.u8()?,
                rssi: r.u8()? as i8,
                battery: r.u8()?,
                appearance: r.u16()?,
                addr: r.array()?,
                name: device_name(&mut r)?,
            }),
            req::GET_DEVICE_MAP => Response::DeviceMap {
                device: device(&mut r)?,
                index: r.u8()?,
                iface: r.u8()?,
                interfaces: r.u8()?,
                total: r.u16()?,
                offset: r.u16()?,
                data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Overflow)?,
            },
            req::GET_MAP_ENTRY => Response::MapEntry {
                kind: r.u8()?,
                index: r.u8()?,
                data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Overflow)?,
            },
            req::GET_MACROS => Response::Macros {
                offset: r.u16()?,
                total: r.u16()?,
                data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Overflow)?,
            },
            req::GET_MAP_SETTING => Response::MapSetting(r.u16()?, r.u32()?),
            req::GET_OVERRIDE => Response::Override(device(&mut r)?, r.u8()?, read_field_override(&mut r)?),
            req::GET_OUTPUTS => Response::Outputs { host: r.u8()?.try_into()?, offered: r.u8()?, taken: r.u8()? },
            req::GET_MODULE_LOG => Response::ModuleLog {
                first: r.u32()?,
                end: r.u32()?,
                at: r.u32()?,
                data: Vec::from_slice(r.lv()?).map_err(|_| WireError::Overflow)?,
            },
            req::GET_KEYCODES => {
                let (keymap, layer, first, n) = (r.u8()?, r.u8()?, r.u16()?, r.u8()? as usize);
                let mut codes = Vec::new();
                for _ in 0..n {
                    codes.push(r.u16()?).map_err(|_| WireError::Overflow)?;
                }
                Response::Keycodes { keymap, layer, first, codes }
            }
            req::GET_AXIS_RULES => {
                let (keymap, layer, first, n) = (r.u8()?, r.u8()?, r.u8()?, r.u8()? as usize);
                let mut rules = Vec::new();
                for _ in 0..n {
                    rules.push(r.array()?).map_err(|_| WireError::Overflow)?;
                }
                Response::AxisRules { keymap, layer, first, rules }
            }
            req::GET_CONTROLS => Response::Controls(
                device(&mut r)?,
                Controls {
                    keyboard: r.bool()?,
                    mouse_buttons: r.u16()?,
                    pad_buttons: r.u32()?,
                    mouse_axes: r.u8()?,
                    pad_axes: r.u16()?,
                    hat: r.bool()?,
                    identity: r.u32()?,
                },
            ),
            req::GET_CALIBRATION => Response::Calibration(device(&mut r)?, r.u8()?, read_axis_cal(&mut r)?),
            req::GET_BT_FOUND => Response::BtFound(match r.bool()? {
                false => None,
                true => Some(BtFound {
                    addr: r.array()?,
                    rssi: r.u8()? as i8,
                    appearance: r.u16()?,
                    name: device_name(&mut r)?,
                }),
            }),
            _ => Response::Ok,
        };
        Ok(DeviceMsg::Response { id, request, response })
    }
}

/// Truncates `s` to at most `STRING_MAX` bytes on a character boundary.
pub fn short_string(s: &str) -> String<STRING_MAX> {
    let mut end = s.len().min(STRING_MAX);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    String::try_from(&s[..end]).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screens_flags_are_in_its_pc_byte() {
        let s = Screen { scale: 150, desk: [0, 0], ..Screen::new(Host::Pc1, 3840, 2160, [-2560, 360]) };
        let enc = |s: Screen| {
            let mut b = [0u8; 32];
            let mut w = Writer::new(&mut b);
            ConfigValue::Screen(0, 3, s).encode(&mut w).unwrap();
            w.finish().to_vec()
        };
        // Neither flag: the host alone, at arrangement 0's key for slot 3.
        let plain = enc(s);
        assert_eq!((u16::from_le_bytes([plain[0], plain[1]]), plain[2]), (0x00B3, 1));
        assert_eq!(enc(Screen { off: true, ..s })[2], 0x81);
        assert_eq!(enc(Screen { fixed: true, ..s })[2], 0x41);
        let both = enc(Screen { off: true, fixed: true, ..s });
        assert_eq!(
            ConfigValue::decode(&mut Reader::new(&both)),
            Ok(ConfigValue::Screen(0, 3, Screen { off: true, fixed: true, ..s }))
        );
        // A Bluetooth host's screen, with both flags.
        let bt = Screen { host: Some(Host::Bt3), off: true, fixed: true, ..s };
        assert_eq!(enc(bt)[2], 0x13 | 0x80 | 0x40);
        assert_eq!(ConfigValue::decode(&mut Reader::new(&enc(bt))), Ok(ConfigValue::Screen(0, 3, bt)));
    }

    #[test]
    fn arrangement_keys() {
        assert_eq!(key::screen(0, 0), 0x00B0);
        assert_eq!(key::screen(3, 7), 0x00CF);
        assert_eq!(key::arrangement_of(0x00B7), Some(0));
        assert_eq!(key::arrangement_of(0x00B8), Some(1));
        assert_eq!(key::arrangement_of(0x00D3), Some(3));
        assert_eq!((key::arrangement_of(0x00D4), key::arrangement_of(key::PROFILE_ARRANGEMENT_BASE)), (None, None));
        // Every key once.
        let mut keys = key::ALL.to_vec();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), key::ALL.len());
        // A profile's arrangement: 0 none, n + 1 arrangement n.
        let k = key::PROFILE_ARRANGEMENT_BASE + 2;
        for (stored, want) in [(0u8, None), (1, Some(0)), (4, Some(3))] {
            let raw = [k as u8, (k >> 8) as u8, stored];
            assert_eq!(ConfigValue::decode(&mut Reader::new(&raw)), Ok(ConfigValue::ProfileArrangement(2, want)));
        }
        assert_eq!(key::hop_mapping(Host::Pc2), 0x00A7);
        assert_eq!(key::hop_units(Host::Bt4), 0x00DF);
        for h in Host::ALL {
            assert_eq!(Host::from_index(h.index()), Some(*h));
            assert_eq!(Host::from_name(h.name()), Some(*h));
        }
    }

    #[test]
    fn hotkey_actions_roundtrip_by_wire_and_by_name() {
        for a in HotkeyAction::ALL {
            let mut buf = [0u8; 2];
            let mut w = Writer::new(&mut buf);
            a.encode(&mut w).unwrap();
            assert_eq!(HotkeyAction::decode(&mut Reader::new(&buf)), Ok(a), "{a:?}");
            assert_eq!(HotkeyAction::from_name(a.name()), Some(a));
        }
        assert_eq!(HotkeyAction::from_name("pc2"), None);
        // A profile past the last, or a host that isn't one, does not decode.
        assert_eq!(HotkeyAction::decode(&mut Reader::new(&[7, 8])), Err(WireError::Invalid));
        assert!(HotkeyAction::decode(&mut Reader::new(&[4, 3])).is_err());
        // The list's keys, first in `ALL`.
        assert_eq!(key::ALL[..MAX_HOTKEYS], core::array::from_fn::<u16, MAX_HOTKEYS, _>(|i| 0x00F0 + i as u16));
        assert_eq!((key::hotkey_of(0x00FF), key::hotkey_of(0x0100), key::hotkey_of(0x0001)), (Some(15), None, None));
    }

    #[test]
    fn requests_roundtrip() {
        let reqs = [
            Request::GetInfo,
            Request::GetStatus,
            Request::GetPort(Port::Other),
            Request::GetPortString(Port::Mouse, StringKind::Product),
            Request::SetActive(Target::Toggle),
            Request::GetConfig(key::COLOR_PC2),
            Request::SetConfig(ConfigValue::Hotkey(
                3,
                HotkeyBinding::new(HotkeyAction::SelectHost(Host::Bt2), Hotkey::double_tap(0x47, true)),
            )),
            Request::SetConfig(ConfigValue::Hotkey(15, HotkeyBinding::NONE)),
            Request::SetConfig(ConfigValue::Mode(Device::Other, PortMode::Passthrough)),
            Request::SetConfig(ConfigValue::Mode(Device::Wireless4, PortMode::Off)),
            Request::SetConfig(ConfigValue::Route(Device::Other, Routing::Pin(Host::Pc2))),
            Request::SetConfig(ConfigValue::Route(Device::Wireless1, Routing::Pin(Host::Bt3))),
            Request::SetConfig(ConfigValue::Route(Device::Keyboard, Routing::Follow)),
            Request::SetConfig(ConfigValue::ColorPc1(Rgb::new(1, 2, 3))),
            Request::SetConfig(ConfigValue::KeyLong(KeyAction::OtherPortMode)),
            Request::SetConfig(ConfigValue::Profile(1)),
            Request::SetConfig(ConfigValue::LongPressMs(1500)),
            Request::SetConfig(ConfigValue::RememberProfile(false)),
            Request::SetConfig(ConfigValue::ProfileModes(
                7,
                [
                    PortMode::Auto,
                    PortMode::Mirrored,
                    PortMode::Passthrough,
                    PortMode::Normalized,
                    PortMode::Off,
                    PortMode::Auto,
                    PortMode::Auto,
                ],
            )),
            Request::SetConfig(ConfigValue::ProfileRoutes(
                5,
                [
                    Routing::Follow,
                    Routing::Pin(Host::Pc1),
                    Routing::Pin(Host::Pc2),
                    Routing::Pin(Host::Bt1),
                    Routing::Follow,
                    Routing::Pin(Host::Bt4),
                    Routing::Follow,
                ],
            )),
            Request::SetConfig(ConfigValue::ProfileName(2, ProfileName::new("gaming"))),
            Request::SaveConfig,
            Request::FactoryReset,
            Request::Reboot(RebootTarget::At32Dfu2),
            Request::Identify(5),
            Request::Redetect(Port::Keyboard),
            Request::FwBegin { target: FwTarget::Rp2354, size: 123_456, crc32: 0xAABBCCDD },
            Request::FwBegin { target: FwTarget::Module, size: 370_000, crc32: 1 },
            Request::FwData { offset: 56, data: Vec::from_slice(&[7u8; FW_CHUNK]).unwrap() },
            Request::FwCommit,
            Request::FwStatus,
            Request::LearnHotkey(true),
            Request::LearnHotkey(false),
            Request::GetDiag(3),
            Request::PointerAt([-1280, 1439]),
            Request::PlaceCursor(Placement::Others),
            Request::PlaceCursor(Placement::Off),
            Request::SetConfig(ConfigValue::AbsPointer(true)),
            Request::SetConfig(ConfigValue::HopAccel(Accel::Adaptive)),
            Request::SetConfig(ConfigValue::HopSpeed(-7)),
            Request::SetConfig(ConfigValue::HopResistance(250)),
            Request::SetConfig(ConfigValue::HopCorner(30)),
            Request::SetConfig(ConfigValue::HopMapping(Host::Pc2, Mapping::MainScreen)),
            Request::SetConfig(ConfigValue::HopMapping(Host::Bt1, Mapping::MainScreen)),
            Request::SetConfig(ConfigValue::HopUnits(Host::Pc1, Units::Logical)),
            Request::SetConfig(ConfigValue::HopUnits(Host::Pc2, Units::Physical)),
            Request::SetConfig(ConfigValue::HopUnits(Host::Bt4, Units::Logical)),
            Request::SetConfig(ConfigValue::ProfileArrangement(3, Some(2))),
            Request::SetConfig(ConfigValue::ProfileArrangement(1, None)),
            Request::SetConfig(ConfigValue::Screen(
                3,
                7,
                Screen { scale: 150, desk: [-2560, 0], ..Screen::new(Host::Pc2, 3840, 2160, [-30000, 400]) },
            )),
            Request::SetConfig(ConfigValue::Screen(
                0,
                2,
                Screen { off: true, fixed: true, ..Screen::new(Host::Pc1, 1920, 1080, [0, 0]) },
            )),
            Request::SetConfig(ConfigValue::Screen(1, 4, Screen::new(Host::Bt2, 2388, 1668, [1920, 0]))),
            Request::SetConfig(ConfigValue::Screen(2, 0, Screen::NONE)),
            Request::SetConfig(ConfigValue::ArrangementName(3, ProfileName::new("couch"))),
            Request::SetConfig(ConfigValue::BtPointer(2, BtPointer::Relative)),
            Request::SetConfig(ConfigValue::Hotkey(
                0,
                HotkeyBinding::new(HotkeyAction::SelectProfile(7), Hotkey::NONE),
            )),
            Request::GetBtDevice(4),
            Request::BtScan(true),
            Request::BtScan(false),
            Request::GetBtFound(9),
            Request::GetDeviceMap { device: Device::Wireless2, index: 0, offset: 96 },
            Request::GetDeviceMap { device: Device::Other, index: 2, offset: 0 },
            Request::GetMapEntry { kind: 1, index: 31 },
            Request::SetMapEntry { kind: 3, index: 0, data: Vec::from_slice(&[1, 2, 3, 4, 5, 6]).unwrap() },
            Request::GetMacros(2000),
            Request::SetMacros { offset: 56, data: Vec::from_slice(&[b'x'; MACRO_PIECE]).unwrap() },
            Request::GetMapSetting(21),
            Request::SetMapSetting(7, 175),
            Request::MonitorDevice(Device::Other, true),
            Request::GetOverride(Device::Wireless4, 7),
            Request::SetOverride(
                Device::Other,
                1,
                Some(FieldOverride { iface: 3, report_id: 1, bit: 270, bits: 12, flags: field::SIGNED, target: 0x46 }),
            ),
            Request::SetOverride(Device::Keyboard, 0, None),
            Request::GetOutputs(Host::Bt3),
            Request::SetConfig(ConfigValue::ProfileFallback(5, Some(2))),
            Request::SetConfig(ConfigValue::HostLayout(Host::Bt2, 12)),
            Request::GetModuleLog(70_000),
            Request::BtEraseAll,
            Request::AdoptModule(Port::Other),
            Request::AskControl,
            Request::SetConfig(ConfigValue::BtControl(3, true)),
            Request::GetKeycodes { keymap: 3, layer: 7, first: 280 },
            Request::SetKeycodes {
                keymap: 1,
                layer: 2,
                first: 4,
                codes: Vec::from_slice(&[0x7400; KEYCODES_PER]).unwrap(),
            },
            Request::GetAxisRules { keymap: 0, layer: 1, first: 8 },
            Request::SetAxisRule { keymap: 2, layer: 0, input: 13, rule: [6, 1, 0, 1, 20, 16] },
            Request::WatchControls(2),
            Request::GetControls(Device::Wireless2),
            Request::GetCalibration(Device::Other, 9),
            Request::SetCalibration(Device::Mouse, 3, AxisCal { min: -30000, centre: 120, max: 31000, deadzone: 12 }),
            Request::SetConfig(ConfigValue::ProfileKeymap(7, Some(3))),
            Request::SetConfig(ConfigValue::ProfileKeymap(0, None)),
            Request::SetConfig(ConfigValue::Gamepad(GamepadOut::XInput)),
            Request::BtPairDevice([1, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5, 0xC6]),
            Request::BtForgetDevice(0xFF),
        ];
        for (i, req) in reqs.iter().enumerate() {
            let raw = req.encode(i as u8 + 1).unwrap();
            assert_eq!(Request::decode(&raw).unwrap(), (i as u8 + 1, req.clone()), "{req:?}");
        }
    }

    #[test]
    fn profile_names_are_cut_at_a_character_boundary() {
        assert_eq!(ProfileName::new("gaming").as_str(), "gaming");
        assert_eq!(ProfileName::new("a very long name").as_str(), "a very long ");
        // "ü" takes bytes 12 and 13: cutting at 12 would split it.
        assert_eq!(ProfileName::new("Arbeitsplatü").as_str(), "Arbeitsplat");
        let too_long = [0x88, 0x00, 13, b'x', b'x', b'x', b'x', b'x', b'x', b'x', b'x', b'x', b'x', b'x', b'x', b'x'];
        assert!(ConfigValue::decode(&mut Reader::new(&too_long)).is_err());
    }

    #[test]
    fn responses_and_events_roundtrip() {
        let status = Status {
            active: Pc::Pc2,
            asking: Some(Host::Bt2),
            links: [LinkState::Connected, LinkState::Suspended],
            power: PowerSource::PcBus,
            adv_pwr: Advertised::None,
            adv_pc: [Advertised::A3_0, Advertised::Default],
            vbus_mv: [0, 5100, 4950],
            budget_ma: 2700,
            flags: status_flags::SW_SWITCH_ALLOWED,
            uptime_s: 99,
            link_errors: [0, 2],
            selftest: selftest::PORT_MS | selftest::RUNNING | selftest::BT_FW,
            bt_active: 2,
            bt_ready: 0b0011,
            bt_bonded: 0b0111,
            bt_pairing: 42,
            bt_passkey: 3,
            bt_module: diag::BtModule::NeedsUpdate,
            hop: HopState::NotNormalized,
            pointer: [-1920, 1079],
            bt_passkey_digits: 4,
            bt_passkey_typed: 4242,
            supply: Supply::Sags,
            supply_mv: 4480,
            supply_lowest_mv: 4310,
            declared_ma: 700,
            bt_devices_bonded: 0b1001,
            bt_devices_ready: 0b0001,
            bt_scan: 17,
            bt_found: 3,
            bt_device_pairing: DevicePairing::Pairing,
            bt_device_passkey: Some(0),
            bt_progress: 37,
            bt_chip: crate::link::bt::Chip::Esp32,
        };
        // The status fits in its report.
        assert!(Response::Status(status).encode(req::GET_STATUS, 1).is_ok());
        let port = PortInfo {
            port: Port::Mouse,
            state: PortState::Hosted,
            mode: PortMode::Auto,
            effective: PortMode::Normalized,
            target: Some(Host::Bt2),
            reason: AutoReason::FitsNormalized,
            vid: 0x046D,
            pid: 0xC539,
            speed: Speed::Full,
            classes: class::MOUSE | class::VENDOR_CHANNEL,
            interfaces: 3,
            report_rate_hz: 1000,
            max_power_ma: 98,
            faults: 0,
        };
        let cases = [
            (
                req::GET_INFO,
                Response::Info(Info {
                    proto: 1,
                    rp_fw: 0x000100,
                    at_fw: [0x000100, 0],
                    hw: 0x000C00,
                    serial: *b"KVM00001",
                    caps: caps::MIRRORED,
                    bt_fw: 0x000100,
                }),
            ),
            (req::GET_STATUS, Response::Status(status)),
            (req::GET_PORT, Response::Port(port)),
            (
                req::GET_PORT_STRING,
                Response::PortString(Port::Mouse, StringKind::Product, short_string("Gaming mouse")),
            ),
            (req::GET_CONFIG, Response::Config(ConfigValue::DoubleTapMs(300))),
            (req::FW_STATUS, Response::Fw { state: FwState::AwaitingKey, progress: 1000 }),
            (req::SET_ACTIVE, Response::Ok),
            (req::SET_CONFIG, Response::Error(ErrorCode::Locked)),
            (
                req::GET_BT_DEVICE,
                Response::BtDevice(BtDeviceInfo {
                    slot: 2,
                    flags: 0b1111,
                    rssi: -61,
                    battery: 0xFF,
                    appearance: 0x03C2,
                    addr: [1, 2, 3, 4, 5, 6, 7],
                    name: bt_device_name(&"m".repeat(40)),
                }),
            ),
            (req::GET_BT_FOUND, Response::BtFound(None)),
            (
                req::GET_DEVICE_MAP,
                Response::DeviceMap {
                    device: Device::Wireless2,
                    index: 0,
                    iface: 0,
                    interfaces: 1,
                    total: 312,
                    offset: 288,
                    data: Vec::from_slice(&[0xC0; 24]).unwrap(),
                },
            ),
            (
                req::GET_DEVICE_MAP,
                Response::DeviceMap {
                    device: Device::Other,
                    index: 3,
                    iface: 5,
                    interfaces: 4,
                    total: 512,
                    offset: 0,
                    data: Vec::from_slice(&[0x05; MAP_PIECE]).unwrap(),
                },
            ),
            (
                req::GET_MAP_ENTRY,
                Response::MapEntry { kind: 0, index: 31, data: Vec::from_slice(&[0xAA; MAP_ENTRY_MAX]).unwrap() },
            ),
            (
                req::GET_MACROS,
                Response::Macros { offset: 1992, total: 2048, data: Vec::from_slice(&[7; MACRO_PIECE]).unwrap() },
            ),
            (req::GET_MAP_SETTING, Response::MapSetting(21, 0x07FF)),
            (
                req::GET_OVERRIDE,
                Response::Override(
                    Device::Wireless1,
                    7,
                    Some(FieldOverride { iface: 0, report_id: 0, bit: 7, bits: 1, flags: field::INVERT, target: 31 }),
                ),
            ),
            (req::GET_OVERRIDE, Response::Override(Device::Mouse, 0, None)),
            (
                req::GET_OUTPUTS,
                Response::Outputs { host: Host::Bt1, offered: 0x1F, taken: outputs::KEYBOARD | outputs::MOUSE },
            ),
            (
                req::GET_MODULE_LOG,
                Response::ModuleLog { first: 4096, end: 6144, at: 4096, data: Vec::from_slice(&[b'x'; 48]).unwrap() },
            ),
            (
                req::GET_KEYCODES,
                Response::Keycodes { keymap: 2, layer: 5, first: 252, codes: Vec::from_slice(&[0x7E08; 28]).unwrap() },
            ),
            (
                req::GET_AXIS_RULES,
                Response::AxisRules { keymap: 1, layer: 0, first: 8, rules: Vec::from_slice(&[[9u8; 6]; 6]).unwrap() },
            ),
            (
                req::GET_CONTROLS,
                Response::Controls(
                    Device::Other,
                    Controls {
                        keyboard: false,
                        mouse_buttons: 0,
                        pad_buttons: 0x3FFF,
                        mouse_axes: 0,
                        pad_axes: 0x33,
                        hat: true,
                        identity: 0xDEAD_BEEF,
                    },
                ),
            ),
            (req::GET_CALIBRATION, Response::Calibration(Device::Other, 2, AxisCal::NONE)),
            (
                req::GET_BT_FOUND,
                Response::BtFound(Some(BtFound {
                    addr: [0, 9, 8, 7, 6, 5, 4],
                    rssi: -80,
                    appearance: 0x03C1,
                    name: bt_device_name("Tastatur ü"),
                })),
            ),
            (req::LEARN_HOTKEY, Response::Ok),
            (req::LEARN_HOTKEY, Response::Error(ErrorCode::NotActive)),
            (
                req::GET_DIAG,
                Response::Diag {
                    page: 1,
                    pages: 5,
                    items: Vec::from_slice(&[diag::Item::new(diag::id::PLL_USB, diag::Grade::Ok, 48_000_000); 9])
                        .unwrap(),
                },
            ),
            (
                req::GET_DIAG,
                Response::Diag {
                    page: 0,
                    pages: 1,
                    items: Vec::from_slice(&[diag::Item::new(diag::id::RP_TEMP, diag::Grade::Warn, -123)]).unwrap(),
                },
            ),
        ];
        for (request, response) in cases {
            let raw = response.encode(request, 42).unwrap();
            assert_eq!(DeviceMsg::decode(&raw).unwrap(), DeviceMsg::Response { id: 42, request, response });
        }
        for e in [
            Event::Switched(Pc::Pc1, SwitchSource::Hotkey),
            Event::PortChanged(Port::Other),
            Event::Fault(FaultKind::OverCurrent, Some(Port::Keyboard)),
            Event::Fault(FaultKind::LinkDown, None),
            Event::Fault(FaultKind::BtSettingsHeld, None),
            Event::ConfigChanged(key::LED_BRIGHTNESS),
            Event::HotkeyLearned(LearnEnd::Learned, Hotkey::combo(0x05, 0x0E)),
            Event::HotkeyLearned(LearnEnd::Learned, Hotkey::double_tap(0xE4, true)),
            Event::HotkeyLearned(LearnEnd::TimedOut, Hotkey::NONE),
            Event::PlaceCursor([-2880, 1809]),
            Event::Control(283, true),
            Event::Axis(9, -32767),
            Event::Report {
                device: Device::Other,
                iface: 3,
                offset: 56,
                total: 64,
                len: 8,
                data: {
                    let mut d = [0u8; REPORT_PIECE];
                    d[..8].fill(0x80);
                    d
                },
            },
        ] {
            assert_eq!(DeviceMsg::decode(&e.encode().unwrap()).unwrap(), DeviceMsg::Event(e));
        }
    }

    #[test]
    fn names_and_cc_classification() {
        for m in PortMode::ALL {
            assert_eq!(PortMode::from_name(m.name()), Some(*m));
        }
        assert_eq!(PortMode::from_name("passthrough-pc1"), None);
        for d in Device::ALL {
            assert_eq!((Device::from_name(d.name()), Device::from_index(d.index())), (Some(*d), Some(*d)));
            assert_eq!(d.port().is_some(), d.wireless().is_none());
            assert!(d.takes(PortMode::Auto) && d.takes(PortMode::Off));
        }
        assert_eq!(Device::wireless_slot(1), Some(Device::Wireless1));
        assert_eq!((Device::wireless_slot(0), Device::wireless_slot(5)), (None, None));
        assert!(!Device::Wireless2.takes(PortMode::Passthrough) && Device::Other.takes(PortMode::Passthrough));
        for r in Routing::ALL {
            assert_eq!((Routing::from_u8(r.to_u8()), Routing::from_name(r.name())), (Some(r), Some(r)));
        }
        assert_eq!(Routing::from_u8(7), None);
        // Each profile's keys, the active profile's modes and routings: every key once, none an arrangement's.
        for p in 0..MAX_PROFILES as u8 {
            for base in [key::PROFILE_MODES_BASE, key::PROFILE_ROUTES_BASE, key::PROFILE_ARRANGEMENT_BASE] {
                assert_eq!(key::profile_of(base + p as u16, base), Some(p));
                assert_eq!(key::arrangement_of(base + p as u16), None);
            }
        }
        assert_eq!(key::profile_of(key::PROFILE_MODES_BASE + 8, key::PROFILE_MODES_BASE), None);
        assert_eq!((key::mode_of(0x0036), key::mode_of(0x0037), key::route_of(0x003E)), (Some(6), None, Some(6)));
        assert_eq!(Advertised::from_cc_mv(0), Advertised::None);
        assert_eq!(Advertised::from_cc_mv(420), Advertised::Default);
        assert_eq!(Advertised::from_cc_mv(940), Advertised::A1_5);
        assert_eq!(Advertised::from_cc_mv(1680), Advertised::A3_0);
        assert_eq!(short_string(&"ä".repeat(40)).len(), 58);
    }

    #[test]
    fn every_diag_id_has_a_name_and_a_group() {
        use diag::id;
        let mut ids = std::vec![
            id::UPTIME,
            id::RESET_CAUSE,
            id::RP_TEMP,
            id::SELFTEST,
            id::DROPPED,
            id::FW_UPDATES,
            id::XOSC,
            id::PLL_SYS,
            id::PLL_USB,
            id::CLK_SYS,
            id::CLK_USB,
            id::CLK_PERI,
            id::CLK_ADC,
            id::ROSC,
            id::HUB0_SPEED,
            id::HUB0_UP,
            id::PHY_SWAP,
            id::HUB0_ERRORS,
            id::ENUM_FAILS,
            id::PWR_VBUS,
            id::PWR_CC,
            id::BUDGET,
            id::SUPPLY,
            id::SUPPLY_DIPS,
        ];
        for a in [id::AT_CLOCK, id::PC_SPEED, id::PC_LINK, id::PC_RESETS, id::LINK_ERRORS, id::PC_VBUS, id::PC_CC]
            .into_iter()
            .chain([id::AT_3V3, id::AT_TEMP, id::AT_FW])
        {
            ids.extend(a);
        }
        ids.extend(id::PORT_SPEED);
        ids.extend(id::PORT_FAULTS);
        ids.extend(id::PC_DROPOUTS);
        ids.extend([id::BT_MODULE, id::BT_FW, id::BT_LINK_ERRORS, id::BT_HOSTS]);
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "two items share an id");
        assert!(n <= diag::MAX_ITEMS);
        for i in ids {
            assert!(diag::meta(i).is_some(), "id {i:#x} has no name");
            assert!(diag::Group::of(i).is_some(), "id {i:#x} has no group");
        }
        assert_eq!(diag::meta(0xEE), None);
    }
}
