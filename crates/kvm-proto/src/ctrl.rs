//! Control protocol between `odkvmctl` and the RP2354B (software.md §8).
//!
//! Every message is one 64-byte HID report on the KVM's vendor interface:
//! `type u8 | request id u8 | payload[62]`. A response has the request's type | 0x80 and the same id; errors are type
//! 0xFF; events (unsolicited, sent to both PCs, `HotkeyLearned` only to the PC that asked) use types 0xC0.. and id 0.

use heapless::{String, Vec};

use crate::wire::{Reader, WireError, Writer, wire_enum};

pub const REPORT_LEN: usize = 64;
pub const PAYLOAD_LEN: usize = REPORT_LEN - 2;
/// 6: Bluetooth hosts (boards 0.16.0 with the module): `caps::BLUETOOTH`, `Target::Next`/`Bt1`–`Bt4`, the Bluetooth
/// fields at the end of `Status`, the `Bt*` requests and events, `HotkeyNext`, `BtName`, `BtPolicy`.
/// 7: screen hop (`caps::SCREEN_HOP`): the absolute pointer, the hop settings, the screen arrangement and each
/// profile's hop switch (config keys 0x00A0–0x00B7), `SwitchSource::Edge`, the hop fields at the end of `Status`,
/// `PointerAt`.
/// 8: screen hop: each PC's units (`HopUnits`, config keys 0x00AC–0x00AD: whether its desktop counts logical
/// pixels), and a screen's flags in its slot's PC byte: off for hop (`Screen::off`, bit 7) and a fixed place on its
/// PC (`Screen::fixed`, bit 6).
pub const PROTOCOL_VERSION: u8 = 8;

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

wire_enum! {
    /// Mode setting of a front port (design.md §4).
    pub enum PortMode {
        Auto = 0,
        Normalized = 1,
        Mirrored = 2,
        MirroredIdentity = 3,
        /// Passthrough that follows the active PC.
        Passthrough = 4,
        PassthroughPc1 = 5,
        PassthroughPc2 = 6,
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
            PortMode::PassthroughPc1 => "passthrough-pc1",
            PortMode::PassthroughPc2 => "passthrough-pc2",
        }
    }

    pub fn from_name(s: &str) -> Option<PortMode> {
        PortMode::ALL.iter().copied().find(|m| m.name() == s).or(match s {
            "mirrored-identity" => Some(PortMode::MirroredIdentity),
            "pinned-pc1" => Some(PortMode::PassthroughPc1),
            "pinned-pc2" => Some(PortMode::PassthroughPc2),
            _ => None,
        })
    }

    pub const fn is_passthrough(self) -> bool {
        matches!(self, PortMode::Passthrough | PortMode::PassthroughPc1 | PortMode::PassthroughPc2)
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
    }
}

wire_enum! {
    /// Why Auto picked the effective mode.
    pub enum AutoReason {
        /// The port is not in Auto.
        NotAuto = 0,
        /// All inputs fit the normalized keyboard/mouse.
        FitsNormalized = 1,
        /// Vendor-page inputs, more than 16 buttons, absolute axes or unknown usage pages.
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
        /// Screen hop: the pointer was pushed across a screen edge onto the other PC's screen (protocol 7).
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
        /// The next target in the order PC1, PC2, Bluetooth hosts 1–4, skipping hosts that aren't ready (protocol 6).
        Next = 4,
        /// Bluetooth host slot 1–4 (protocol 6, boards with the Bluetooth module).
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
        /// The same for the Bluetooth module (software.md §4.13), protocol 6.
        BtRetry = 5,
    }
}

wire_enum! {
    pub enum FwTarget { Rp2354 = 0 }
}

wire_enum! {
    pub enum FwState { Idle = 0, Receiving = 1, AwaitingKey = 2, Writing = 3, Done = 4, Failed = 5 }
}

wire_enum! {
    pub enum FaultKind { OverCurrent = 0, EnumerationFailed = 1, LinkDown = 2, PowerBudget = 3, HubError = 4 }
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
        /// Cycle the Other port's mode: Auto → Passthrough → pinned PC1 → pinned PC2.
        OtherPortMode = 3,
        /// The next target: PC1, PC2, then each Bluetooth host that is ready (protocol 6).
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

/// Profiles the KVM stores: each a name and the three port modes (design.md §4.1).
pub const MAX_PROFILES: usize = 4;
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

/// Screen slots of the screen-hop arrangement (`ConfigValue::Screen`).
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
    /// What a PC's desktop coordinates count ([`Screen::desk`], `ConfigValue::HopUnits`, protocol 8).
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

/// Physical pixels: what the KVM assumed before protocol 8.
impl Default for Units {
    fn default() -> Self {
        Units::Physical
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
        /// The active PC has no screen in the arrangement.
        NoScreens = 4,
        /// The active PC uses the boot protocol (its firmware setup, before the OS runs).
        BootProtocol = 5,
        /// A Bluetooth host has the input.
        Wireless = 6,
    }
}

/// One screen of the screen-hop arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Screen {
    /// The PC it belongs to; None: the slot is free.
    pub pc: Option<Pc>,
    /// Resolution, in the PC's pixels.
    pub w: u16,
    pub h: u16,
    /// The PC's display scaling, in percent: in the arrangement the screen is w×100/scale by h×100/scale.
    pub scale: u16,
    /// Its top-left corner in the PC's own display arrangement, in the PC's pixels.
    pub desk: [i16; 2],
    /// Its top-left corner in the arrangement, in logical pixels.
    pub at: [i16; 2],
    /// Screen hop leaves it alone (protocol 8): its PC has it, but another PC shows on it (a monitor cabled to both).
    /// It stands where that PC's screen does, and still counts for its PC's desktop: the PC spreads the absolute
    /// pointer across it too.
    pub off: bool,
    /// Its place on its PC (`desk`) is the PC's own, detected or typed in, not derived from the arrangement (protocol
    /// 8; the KVM only stores it).
    pub fixed: bool,
}

impl Screen {
    pub const NONE: Screen =
        Screen { pc: None, w: 0, h: 0, scale: 100, desk: [0, 0], at: [0, 0], off: false, fixed: false };

    pub const fn new(pc: Pc, w: u16, h: u16, at: [i16; 2]) -> Screen {
        Screen { pc: Some(pc), w, h, scale: 100, desk: [0, 0], at, off: false, fixed: false }
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

    /// A screen of a PC with a size and scaling the KVM accepts, or a free slot.
    pub fn is_valid(&self) -> bool {
        self.pc.is_none()
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

/// Bits of a screen slot's PC byte above the PC (protocol 8).
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
    pub const BUDGET_LOW: u8 = 1 << 5;
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
    /// The Bluetooth module (boards 0.16.0 on, when fitted) doesn't run the embedded firmware: programming it failed
    /// this boot, or it doesn't answer; tried again at the next power-up. Bits 16 and up travel at the end of `Status`
    /// (protocol 6), so older hosts see bits 0–15 as before.
    pub const BT_FW: u32 = 1 << 16;
    /// Programming the Bluetooth module failed 5 times in a row: no more attempts until the embedded image changes, the
    /// KVM's key is held at power-up, or `RebootTarget::BtRetry`.
    pub const BT_GAVE_UP: u32 = 1 << 17;
    pub const BT: u32 = BT_FW | BT_GAVE_UP;
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
    /// Most items the firmware sends: 6 pages.
    pub const MAX_ITEMS: usize = 6 * PER_PAGE;

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
            /// Not fitted, or a board before 0.16.0: the Bluetooth items are left out.
            Absent = 0,
            /// The link is up and the module runs the firmware the RP2354B carries.
            Up = 1,
            /// Being programmed with the RP2354B's copy of its firmware.
            Programming = 2,
            /// Fitted, but it doesn't answer or couldn't be programmed: Bluetooth is off until the next power-up.
            Failed = 3,
            /// Programming failed 5 times in a row: not tried again until the image changes, the key is held at
            /// power-up, or `RebootTarget::BtRetry`.
            GaveUp = 4,
            /// Fitted, waiting for its `Hello`: just reset, or just programmed.
            Starting = 5,
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

        pub const AT1_TEMP: u8 = 0x40;
        pub const AT2_TEMP: u8 = 0x41;
        pub const AT1_FW: u8 = 0x42;
        pub const AT2_FW: u8 = 0x43;

        /// Bluetooth (only with the module fitted).
        pub const BT_MODULE: u8 = 0x50;
        pub const BT_FW: u8 = 0x51;
        pub const BT_LINK_ERRORS: u8 = 0x52;
        pub const BT_HOSTS: u8 = 0x53;

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
            id::AT1_TEMP => m("AT32F405 #1 temperature", DeciCelsius),
            id::AT2_TEMP => m("AT32F405 #2 temperature", DeciCelsius),
            id::AT1_FW => m("AT32F405 #1 firmware", Version),
            id::AT2_FW => m("AT32F405 #2 firmware", Version),
            id::BT_MODULE => m("ESP32-C3 module", BtModule),
            id::BT_FW => m("ESP32-C3 firmware", Version),
            id::BT_LINK_ERRORS => m("ESP32-C3 UART errors", Count),
            id::BT_HOSTS => m("Bluetooth hosts connected", Count),
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
    /// The Bluetooth module (boards 0.16.0 on) is fitted and answers: Bluetooth hosts are targets (protocol 6).
    pub const BLUETOOTH: u16 = 1 << 5;
    /// Screen hop and the absolute pointer (protocol 7).
    pub const SCREEN_HOP: u16 = 1 << 6;
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
    /// Firmware of the Bluetooth module (protocol 6; 0 = none, not answering, or older firmware).
    pub bt_fw: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Status {
    pub active: Pc,
    /// The PC this response is sent to (None in the simulator or over the service port).
    pub asking: Option<Pc>,
    pub links: [LinkState; 2],
    pub power: PowerSource,
    pub adv_pwr: Advertised,
    pub adv_pc: [Advertised; 2],
    /// VBUS of PWR, PC1, PC2 in mV.
    pub vbus_mv: [u16; 3],
    /// Estimated current left for the front ports.
    pub budget_ma: u16,
    pub flags: u8,
    pub uptime_s: u32,
    pub link_errors: [u16; 2],
    /// [`selftest`] bits; 16 and up are protocol 6.
    pub selftest: u32,
    /// Bluetooth (protocol 6; 0 from older firmware): the host slot that gets input (0 = a PC is active, `active`).
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
    /// Screen hop (protocol 7; Off from older firmware): whether it moves the pointer, and where the pointer is in the
    /// arrangement (logical pixels).
    pub hop: HopState,
    pub pointer: [i16; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PortInfo {
    pub port: Port,
    pub state: PortState,
    pub mode: PortMode,
    /// What actually runs: Normalized, Mirrored, MirroredIdentity or one of the Passthrough variants.
    pub effective: PortMode,
    pub reason: AutoReason,
    pub vid: u16,
    pub pid: u16,
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
    pub const HOTKEY_TOGGLE: u16 = 0x0001;
    pub const HOTKEY_PC1: u16 = 0x0002;
    pub const HOTKEY_PC2: u16 = 0x0003;
    pub const DOUBLE_TAP_MS: u16 = 0x0004;
    /// What a tap / a long press on the KVM's key does.
    pub const KEY_TAP: u16 = 0x0005;
    pub const KEY_LONG: u16 = 0x0006;
    /// How long the key is held for a long press (ms).
    pub const LONG_PRESS_MS: u16 = 0x0007;
    /// Hotkey for the next target: PC1, PC2, the Bluetooth hosts that are ready (protocol 6).
    pub const HOTKEY_NEXT: u16 = 0x0008;
    pub const LED_BRIGHTNESS: u16 = 0x0010;
    pub const COLOR_PC1: u16 = 0x0011;
    pub const COLOR_PC2: u16 = 0x0012;
    pub const PC_POLLING: u16 = 0x0020;
    pub const POWER_CYCLE_ON_SWITCH: u16 = 0x0021;
    pub const PORT_MODE_BASE: u16 = 0x0030;
    pub const SW_SWITCH_ALLOWED: u16 = 0x0040;
    pub const SW_CONFIG_ALLOWED: u16 = 0x0041;
    pub const VIAL: u16 = 0x0050;
    /// Apply the stored keymap to normalized keyboards.
    pub const KEYMAP: u16 = 0x0051;
    /// Make the running PC active when the active one is switched off (software.md §4.12).
    pub const AUTO_SELECT: u16 = 0x0060;
    /// In standby, keep the keyboard and mouse powered so that a key press wakes a sleeping PC.
    pub const STANDBY_WAKE: u16 = 0x0061;
    /// In standby, the key LED glows dimly instead of all LEDs going dark.
    pub const STANDBY_GLOW: u16 = 0x0062;
    /// The active profile; the port modes (`PORT_MODE_BASE`) are its modes.
    pub const PROFILE: u16 = 0x0070;
    /// Profiles in use (1–[`MAX_PROFILES`](super::MAX_PROFILES)): the key and `next` cycle through them.
    pub const PROFILE_COUNT: u16 = 0x0071;
    /// Power up in the profile last active (otherwise in profile 0, and choosing one writes nothing to flash).
    pub const REMEMBER_PROFILE: u16 = 0x0072;
    /// Port modes (Keyboard, Mouse, Other) and name of profile 0–3.
    pub const PROFILE_MODES_BASE: u16 = 0x0080;
    pub const PROFILE_NAME_BASE: u16 = 0x0088;
    /// Name of Bluetooth host slot 1–4 (UTF-8, at most 12 bytes; empty = "BT n"), protocol 6.
    pub const BT_NAME_BASE: u16 = 0x0090;
    /// What the module does with host slot 1–4 while another target is active ([`Policy`](super::BtPolicy)).
    pub const BT_POLICY_BASE: u16 = 0x0098;
    /// Screen hop (protocol 7): the absolute pointer on the KVM's mouse, the acceleration, speed, edge resistance and
    /// corner, each PC's mapping, each profile's switch, the screen slots.
    pub const ABS_POINTER: u16 = 0x00A0;
    pub const HOP_ACCEL: u16 = 0x00A1;
    pub const HOP_SPEED: u16 = 0x00A2;
    pub const HOP_RESISTANCE: u16 = 0x00A3;
    pub const HOP_CORNER: u16 = 0x00A4;
    pub const HOP_MAPPING_BASE: u16 = 0x00A6;
    pub const PROFILE_HOP_BASE: u16 = 0x00A8;
    /// Each PC's [`Units`](super::Units) for screen hop, protocol 8.
    pub const HOP_UNITS_BASE: u16 = 0x00AC;
    pub const SCREEN_BASE: u16 = 0x00B0;

    pub const ALL: &[u16] = &[
        HOTKEY_TOGGLE,
        HOTKEY_PC1,
        HOTKEY_PC2,
        DOUBLE_TAP_MS,
        KEY_TAP,
        KEY_LONG,
        LONG_PRESS_MS,
        LED_BRIGHTNESS,
        COLOR_PC1,
        COLOR_PC2,
        PC_POLLING,
        POWER_CYCLE_ON_SWITCH,
        PORT_MODE_BASE,
        PORT_MODE_BASE + 1,
        PORT_MODE_BASE + 2,
        SW_SWITCH_ALLOWED,
        SW_CONFIG_ALLOWED,
        VIAL,
        KEYMAP,
        AUTO_SELECT,
        STANDBY_WAKE,
        STANDBY_GLOW,
        // The count before the active profile, which must be one of those in use; the profiles' modes after the
        // port modes, which a config item also holds (for older firmware) and which go to the active profile.
        PROFILE_COUNT,
        PROFILE,
        REMEMBER_PROFILE,
        PROFILE_MODES_BASE,
        PROFILE_MODES_BASE + 1,
        PROFILE_MODES_BASE + 2,
        PROFILE_MODES_BASE + 3,
        PROFILE_NAME_BASE,
        PROFILE_NAME_BASE + 1,
        PROFILE_NAME_BASE + 2,
        PROFILE_NAME_BASE + 3,
        HOTKEY_NEXT,
        BT_NAME_BASE,
        BT_NAME_BASE + 1,
        BT_NAME_BASE + 2,
        BT_NAME_BASE + 3,
        BT_POLICY_BASE,
        BT_POLICY_BASE + 1,
        BT_POLICY_BASE + 2,
        BT_POLICY_BASE + 3,
        ABS_POINTER,
        HOP_ACCEL,
        HOP_SPEED,
        HOP_RESISTANCE,
        HOP_CORNER,
        HOP_MAPPING_BASE,
        HOP_MAPPING_BASE + 1,
        HOP_UNITS_BASE,
        HOP_UNITS_BASE + 1,
        PROFILE_HOP_BASE,
        PROFILE_HOP_BASE + 1,
        PROFILE_HOP_BASE + 2,
        PROFILE_HOP_BASE + 3,
        SCREEN_BASE,
        SCREEN_BASE + 1,
        SCREEN_BASE + 2,
        SCREEN_BASE + 3,
        SCREEN_BASE + 4,
        SCREEN_BASE + 5,
        SCREEN_BASE + 6,
        SCREEN_BASE + 7,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ConfigValue {
    HotkeyToggle(Hotkey),
    HotkeyPc1(Hotkey),
    HotkeyPc2(Hotkey),
    DoubleTapMs(u16),
    LedBrightness(u8),
    ColorPc1(Rgb),
    ColorPc2(Rgb),
    PcPolling(Polling),
    PowerCycleOnSwitch(bool),
    PortMode(Port, PortMode),
    SwSwitchAllowed(bool),
    SwConfigAllowed(bool),
    Vial(VialMode),
    Keymap(bool),
    AutoSelect(bool),
    StandbyWake(bool),
    StandbyGlow(bool),
    KeyTap(KeyAction),
    KeyLong(KeyAction),
    LongPressMs(u16),
    Profile(u8),
    ProfileCount(u8),
    /// Profile index, then the modes of the Keyboard, Mouse and Other port.
    ProfileModes(u8, [PortMode; 3]),
    ProfileName(u8, ProfileName),
    RememberProfile(bool),
    HotkeyNext(Hotkey),
    /// Bluetooth host slot 0–3 (slot n = index n−1) and its name (empty = "BT n").
    BtName(u8, ProfileName),
    /// Bluetooth host slot 0–3 and what the module does with it while another target is active.
    BtPolicy(u8, BtPolicy),
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
    /// How the PC maps the absolute pointer onto its screens.
    HopMapping(Pc, Mapping),
    /// What the PC's desktop coordinates count (protocol 8).
    HopUnits(Pc, Units),
    /// Profile index, and whether screen hop is on in it.
    ProfileHop(u8, bool),
    /// Screen slot 0–7.
    Screen(u8, Screen),
}

pub use crate::link::bt::Policy as BtPolicy;

impl ConfigValue {
    pub fn key(&self) -> u16 {
        match self {
            ConfigValue::HotkeyToggle(_) => key::HOTKEY_TOGGLE,
            ConfigValue::HotkeyPc1(_) => key::HOTKEY_PC1,
            ConfigValue::HotkeyPc2(_) => key::HOTKEY_PC2,
            ConfigValue::DoubleTapMs(_) => key::DOUBLE_TAP_MS,
            ConfigValue::LedBrightness(_) => key::LED_BRIGHTNESS,
            ConfigValue::ColorPc1(_) => key::COLOR_PC1,
            ConfigValue::ColorPc2(_) => key::COLOR_PC2,
            ConfigValue::PcPolling(_) => key::PC_POLLING,
            ConfigValue::PowerCycleOnSwitch(_) => key::POWER_CYCLE_ON_SWITCH,
            ConfigValue::PortMode(p, _) => key::PORT_MODE_BASE + *p as u16,
            ConfigValue::SwSwitchAllowed(_) => key::SW_SWITCH_ALLOWED,
            ConfigValue::SwConfigAllowed(_) => key::SW_CONFIG_ALLOWED,
            ConfigValue::Vial(_) => key::VIAL,
            ConfigValue::Keymap(_) => key::KEYMAP,
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
            ConfigValue::ProfileName(i, _) => key::PROFILE_NAME_BASE + *i as u16,
            ConfigValue::HotkeyNext(_) => key::HOTKEY_NEXT,
            ConfigValue::BtName(i, _) => key::BT_NAME_BASE + *i as u16,
            ConfigValue::BtPolicy(i, _) => key::BT_POLICY_BASE + *i as u16,
            ConfigValue::AbsPointer(_) => key::ABS_POINTER,
            ConfigValue::HopAccel(_) => key::HOP_ACCEL,
            ConfigValue::HopSpeed(_) => key::HOP_SPEED,
            ConfigValue::HopResistance(_) => key::HOP_RESISTANCE,
            ConfigValue::HopCorner(_) => key::HOP_CORNER,
            ConfigValue::HopMapping(pc, _) => key::HOP_MAPPING_BASE + pc.index() as u16,
            ConfigValue::HopUnits(pc, _) => key::HOP_UNITS_BASE + pc.index() as u16,
            ConfigValue::ProfileHop(i, _) => key::PROFILE_HOP_BASE + *i as u16,
            ConfigValue::Screen(i, _) => key::SCREEN_BASE + *i as u16,
        }
    }

    pub fn encode(&self, w: &mut Writer) -> Result<(), WireError> {
        w.u16(self.key())?;
        match *self {
            ConfigValue::HotkeyToggle(h)
            | ConfigValue::HotkeyPc1(h)
            | ConfigValue::HotkeyPc2(h)
            | ConfigValue::HotkeyNext(h) => {
                w.u8(h.kind as u8)?;
                w.u8(h.modifiers)?;
                w.u8(h.key)?;
                w.bool(h.swallow)
            }
            ConfigValue::DoubleTapMs(v) | ConfigValue::LongPressMs(v) => w.u16(v),
            ConfigValue::LedBrightness(v) => w.u8(v),
            ConfigValue::ColorPc1(c) | ConfigValue::ColorPc2(c) => w.bytes(&[c.r, c.g, c.b]),
            ConfigValue::PcPolling(p) => w.u8(p as u8),
            ConfigValue::PortMode(_, m) => w.u8(m as u8),
            ConfigValue::Vial(m) => w.u8(m as u8),
            ConfigValue::KeyTap(a) | ConfigValue::KeyLong(a) => w.u8(a as u8),
            ConfigValue::Profile(i) | ConfigValue::ProfileCount(i) => w.u8(i),
            ConfigValue::ProfileModes(_, m) => w.bytes(&m.map(|m| m as u8)),
            ConfigValue::ProfileName(_, n) | ConfigValue::BtName(_, n) => w.lv(n.as_str().as_bytes()),
            ConfigValue::BtPolicy(_, p) => w.u8(p as u8),
            ConfigValue::HopAccel(a) => w.u8(a as u8),
            ConfigValue::HopSpeed(v) => w.u8(v as u8),
            ConfigValue::HopResistance(v) | ConfigValue::HopCorner(v) => w.u16(v),
            ConfigValue::HopMapping(_, m) => w.u8(m as u8),
            ConfigValue::HopUnits(_, u) => w.u8(u as u8),
            ConfigValue::Screen(_, s) => {
                let flags = if s.off { screen_flags::OFF } else { 0 } | if s.fixed { screen_flags::FIXED } else { 0 };
                w.u8(s.pc.map_or(0, |p| p as u8 | flags))?;
                w.u16(s.w)?;
                w.u16(s.h)?;
                w.u16(s.scale)?;
                for v in s.desk.into_iter().chain(s.at) {
                    w.u16(v as u16)?;
                }
                Ok(())
            }
            ConfigValue::PowerCycleOnSwitch(b)
            | ConfigValue::SwSwitchAllowed(b)
            | ConfigValue::SwConfigAllowed(b)
            | ConfigValue::Keymap(b)
            | ConfigValue::AutoSelect(b)
            | ConfigValue::StandbyWake(b)
            | ConfigValue::StandbyGlow(b)
            | ConfigValue::RememberProfile(b)
            | ConfigValue::AbsPointer(b)
            | ConfigValue::ProfileHop(_, b) => w.bool(b),
        }
    }

    pub fn decode(r: &mut Reader) -> Result<Self, WireError> {
        let k = r.u16()?;
        let hotkey = |r: &mut Reader| -> Result<Hotkey, WireError> {
            Ok(Hotkey { kind: r.u8()?.try_into()?, modifiers: r.u8()?, key: r.u8()?, swallow: r.bool()? })
        };
        let rgb = |r: &mut Reader| -> Result<Rgb, WireError> { Ok(Rgb { r: r.u8()?, g: r.u8()?, b: r.u8()? }) };
        let profiles = |base: u16| base..base + MAX_PROFILES as u16;
        let bt_slots = |base: u16| base..base + BT_HOSTS as u16;
        Ok(match k {
            key::HOTKEY_TOGGLE => ConfigValue::HotkeyToggle(hotkey(r)?),
            key::HOTKEY_PC1 => ConfigValue::HotkeyPc1(hotkey(r)?),
            key::HOTKEY_PC2 => ConfigValue::HotkeyPc2(hotkey(r)?),
            key::DOUBLE_TAP_MS => ConfigValue::DoubleTapMs(r.u16()?),
            key::LED_BRIGHTNESS => ConfigValue::LedBrightness(r.u8()?),
            key::COLOR_PC1 => ConfigValue::ColorPc1(rgb(r)?),
            key::COLOR_PC2 => ConfigValue::ColorPc2(rgb(r)?),
            key::PC_POLLING => ConfigValue::PcPolling(r.u8()?.try_into()?),
            key::POWER_CYCLE_ON_SWITCH => ConfigValue::PowerCycleOnSwitch(r.bool()?),
            k if (key::PORT_MODE_BASE..key::PORT_MODE_BASE + 3).contains(&k) => ConfigValue::PortMode(
                Port::from_index((k - key::PORT_MODE_BASE) as usize).ok_or(WireError::Invalid)?,
                r.u8()?.try_into()?,
            ),
            key::SW_SWITCH_ALLOWED => ConfigValue::SwSwitchAllowed(r.bool()?),
            key::SW_CONFIG_ALLOWED => ConfigValue::SwConfigAllowed(r.bool()?),
            key::VIAL => ConfigValue::Vial(r.u8()?.try_into()?),
            key::KEYMAP => ConfigValue::Keymap(r.bool()?),
            key::AUTO_SELECT => ConfigValue::AutoSelect(r.bool()?),
            key::STANDBY_WAKE => ConfigValue::StandbyWake(r.bool()?),
            key::STANDBY_GLOW => ConfigValue::StandbyGlow(r.bool()?),
            key::KEY_TAP => ConfigValue::KeyTap(r.u8()?.try_into()?),
            key::KEY_LONG => ConfigValue::KeyLong(r.u8()?.try_into()?),
            key::LONG_PRESS_MS => ConfigValue::LongPressMs(r.u16()?),
            key::REMEMBER_PROFILE => ConfigValue::RememberProfile(r.bool()?),
            key::PROFILE => ConfigValue::Profile(r.u8()?),
            key::PROFILE_COUNT => ConfigValue::ProfileCount(r.u8()?),
            k if profiles(key::PROFILE_MODES_BASE).contains(&k) => ConfigValue::ProfileModes(
                (k - key::PROFILE_MODES_BASE) as u8,
                [r.u8()?.try_into()?, r.u8()?.try_into()?, r.u8()?.try_into()?],
            ),
            k if profiles(key::PROFILE_NAME_BASE).contains(&k) || bt_slots(key::BT_NAME_BASE).contains(&k) => {
                let b = r.lv()?;
                let s = core::str::from_utf8(b).map_err(|_| WireError::Invalid)?;
                if b.len() > PROFILE_NAME_MAX {
                    return Err(WireError::Invalid);
                }
                if k >= key::BT_NAME_BASE {
                    ConfigValue::BtName((k - key::BT_NAME_BASE) as u8, ProfileName::new(s))
                } else {
                    ConfigValue::ProfileName((k - key::PROFILE_NAME_BASE) as u8, ProfileName::new(s))
                }
            }
            key::HOTKEY_NEXT => ConfigValue::HotkeyNext(hotkey(r)?),
            k if bt_slots(key::BT_POLICY_BASE).contains(&k) => {
                ConfigValue::BtPolicy((k - key::BT_POLICY_BASE) as u8, r.u8()?.try_into()?)
            }
            key::ABS_POINTER => ConfigValue::AbsPointer(r.bool()?),
            key::HOP_ACCEL => ConfigValue::HopAccel(r.u8()?.try_into()?),
            key::HOP_SPEED => ConfigValue::HopSpeed(r.u8()? as i8),
            key::HOP_RESISTANCE => ConfigValue::HopResistance(r.u16()?),
            key::HOP_CORNER => ConfigValue::HopCorner(r.u16()?),
            k if (key::HOP_MAPPING_BASE..key::HOP_MAPPING_BASE + 2).contains(&k) => {
                ConfigValue::HopMapping(Pc::from_index((k - key::HOP_MAPPING_BASE) as usize), r.u8()?.try_into()?)
            }
            k if profiles(key::PROFILE_HOP_BASE).contains(&k) => {
                ConfigValue::ProfileHop((k - key::PROFILE_HOP_BASE) as u8, r.bool()?)
            }
            k if (key::HOP_UNITS_BASE..key::HOP_UNITS_BASE + 2).contains(&k) => {
                ConfigValue::HopUnits(Pc::from_index((k - key::HOP_UNITS_BASE) as usize), r.u8()?.try_into()?)
            }
            k if (key::SCREEN_BASE..key::SCREEN_BASE + MAX_SCREENS as u16).contains(&k) => {
                let b = r.u8()?;
                let pc = match b & !(screen_flags::OFF | screen_flags::FIXED) {
                    0 => None,
                    p => Some(p.try_into()?),
                };
                let (w, h, scale) = (r.u16()?, r.u16()?, r.u16()?);
                let mut v = [0i16; 4];
                for x in &mut v {
                    *x = r.u16()? as i16;
                }
                let (off, fixed) =
                    (pc.is_some() && b & screen_flags::OFF != 0, pc.is_some() && b & screen_flags::FIXED != 0);
                let s = Screen { pc, w, h, scale, desk: [v[0], v[1]], at: [v[2], v[3]], off, fixed };
                ConfigValue::Screen((k - key::SCREEN_BASE) as u8, s)
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
    SetPortMode(Port, PortMode),
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
    /// One page of the diagnostics ([`diag`]), from 0; `Response::Diag` says how many pages there are. Protocol 5.
    GetDiag(u8),
    /// The state of Bluetooth host slot 1–4 (protocol 6): `Response::BtHost`.
    GetBtHost(u8),
    /// Open (true) or close Bluetooth pairing; it closes by itself after `BT_PAIR_SECONDS`.
    BtPair(bool),
    /// Forget the bond in Bluetooth host slot 1–4 (0xFF = all).
    BtForget(u8),
    /// The passkey a pairing host shows, typed in software instead of on the KVM's keyboard.
    BtPasskey(u32),
    /// Where the asking PC's own cursor is, in its display arrangement's pixels (protocol 7). With screen hop, the KVM
    /// takes it over while it isn't moving the pointer itself: a touchpad or a program moved it.
    PointerAt([i16; 2]),
}

/// How long Bluetooth pairing stays open.
pub const BT_PAIR_SECONDS: u8 = 60;

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
    pub const SET_PORT_MODE: u8 = 0x06;
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
}

impl Request {
    pub fn type_byte(&self) -> u8 {
        match self {
            Request::GetInfo => req::GET_INFO,
            Request::GetStatus => req::GET_STATUS,
            Request::GetPort(_) => req::GET_PORT,
            Request::GetPortString(..) => req::GET_PORT_STRING,
            Request::SetActive(_) => req::SET_ACTIVE,
            Request::SetPortMode(..) => req::SET_PORT_MODE,
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
        }
    }

    /// Requests that change state and are refused while software configuration is locked.
    pub fn is_config_change(&self) -> bool {
        matches!(
            self,
            Request::SetPortMode(..)
                | Request::SetConfig(_)
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
            | Request::FwStatus => {}
            Request::GetPort(p) | Request::Redetect(p) => w.u8(*p as u8)?,
            Request::GetPortString(p, k) => {
                w.u8(*p as u8)?;
                w.u8(*k as u8)?;
            }
            Request::SetActive(t) => w.u8(*t as u8)?,
            Request::SetPortMode(p, m) => {
                w.u8(*p as u8)?;
                w.u8(*m as u8)?;
            }
            Request::GetConfig(k) => w.u16(*k)?,
            Request::SetConfig(v) => v.encode(&mut w)?,
            Request::Reboot(t) => w.u8(*t as u8)?,
            Request::Identify(s) | Request::GetDiag(s) | Request::GetBtHost(s) | Request::BtForget(s) => w.u8(*s)?,
            Request::BtPair(on) => w.bool(*on)?,
            Request::BtPasskey(v) => w.u32(*v)?,
            Request::PointerAt([x, y]) => {
                w.u16(*x as u16)?;
                w.u16(*y as u16)?;
            }
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
            req::SET_PORT_MODE => Request::SetPortMode(r.u8()?.try_into()?, r.u8()?.try_into()?),
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
    /// Input now goes to Bluetooth host slot 1–4, or 0: back to a PC (then `Switched` follows). Protocol 6.
    BtActive(u8),
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
                w.u8(s.asking.map_or(0, |p| p as u8))?;
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
                w.u8(s.bt_module as u8)?;
                w.u16((s.selftest >> 16) as u16)?;
                w.u8(s.hop as u8)?;
                w.u16(s.pointer[0] as u16)?;
                w.u16(s.pointer[1] as u16)?;
            }
            Response::Port(p) => {
                w.u8(p.port as u8)?;
                w.u8(p.state as u8)?;
                w.u8(p.mode as u8)?;
                w.u8(p.effective as u8)?;
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
            Event::BtChanged(n) | Event::BtActive(n) => {
                w.u8(if matches!(self, Event::BtChanged(_)) { ev::BT_CHANGED } else { ev::BT_ACTIVE })?;
                w.u8(0)?;
                w.u8(n)?;
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
                // Protocol 6; the report is zero-padded.
                bt_fw: if r.remaining() >= 4 { r.u32()? } else { 0 },
            }),
            req::GET_STATUS => {
                let mut st = Status {
                    active: r.u8()?.try_into()?,
                    asking: match r.u8()? {
                        0 => None,
                        p => Some(p.try_into()?),
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
                    // Protocol 6; the report is zero-padded, so older firmware reads as "no Bluetooth".
                    bt_active: if r.remaining() > 0 { r.u8()? } else { 0 },
                    bt_ready: if r.remaining() > 0 { r.u8()? } else { 0 },
                    bt_bonded: if r.remaining() > 0 { r.u8()? } else { 0 },
                    bt_pairing: if r.remaining() > 0 { r.u8()? } else { 0 },
                    bt_passkey: if r.remaining() > 0 { r.u8()? } else { 0 },
                    bt_module: if r.remaining() > 0 {
                        diag::BtModule::from_u8(r.u8()?).unwrap_or(diag::BtModule::Absent)
                    } else {
                        diag::BtModule::Absent
                    },
                    hop: HopState::Off,
                    pointer: [0, 0],
                };
                if r.remaining() >= 2 {
                    st.selftest |= (r.u16()? as u32) << 16;
                }
                // Protocol 7.
                if r.remaining() >= 5 {
                    st.hop = HopState::from_u8(r.u8()?).unwrap_or(HopState::Off);
                    st.pointer = [r.u16()? as i16, r.u16()? as i16];
                }
                Response::Status(st)
            }
            req::GET_PORT => Response::Port(PortInfo {
                port: r.u8()?.try_into()?,
                state: r.u8()?.try_into()?,
                mode: r.u8()?.try_into()?,
                effective: r.u8()?.try_into()?,
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
        let s = Screen { scale: 150, desk: [0, 0], ..Screen::new(Pc::Pc1, 3840, 2160, [-2560, 360]) };
        let enc = |s: Screen| {
            let mut b = [0u8; 32];
            let mut w = Writer::new(&mut b);
            ConfigValue::Screen(3, s).encode(&mut w).unwrap();
            w.finish().to_vec()
        };
        // A slot stored before protocol 8 has neither.
        assert_eq!(enc(s)[2], 1);
        assert_eq!(enc(Screen { off: true, ..s })[2], 0x81);
        assert_eq!(enc(Screen { fixed: true, ..s })[2], 0x41);
        let both = enc(Screen { off: true, fixed: true, ..s });
        assert_eq!(
            ConfigValue::decode(&mut Reader::new(&both)),
            Ok(ConfigValue::Screen(3, Screen { off: true, fixed: true, ..s }))
        );
    }

    #[test]
    fn requests_roundtrip() {
        let reqs = [
            Request::GetInfo,
            Request::GetStatus,
            Request::GetPort(Port::Other),
            Request::GetPortString(Port::Mouse, StringKind::Product),
            Request::SetActive(Target::Toggle),
            Request::SetPortMode(Port::Mouse, PortMode::MirroredIdentity),
            Request::GetConfig(key::COLOR_PC2),
            Request::SetConfig(ConfigValue::HotkeyToggle(Hotkey::double_tap(0x47, true))),
            Request::SetConfig(ConfigValue::PortMode(Port::Other, PortMode::PassthroughPc2)),
            Request::SetConfig(ConfigValue::ColorPc1(Rgb::new(1, 2, 3))),
            Request::SetConfig(ConfigValue::KeyLong(KeyAction::OtherPortMode)),
            Request::SetConfig(ConfigValue::Profile(1)),
            Request::SetConfig(ConfigValue::LongPressMs(1500)),
            Request::SetConfig(ConfigValue::RememberProfile(false)),
            Request::SetConfig(ConfigValue::ProfileModes(
                3,
                [PortMode::Auto, PortMode::Mirrored, PortMode::Passthrough],
            )),
            Request::SetConfig(ConfigValue::ProfileName(2, ProfileName::new("gaming"))),
            Request::SaveConfig,
            Request::FactoryReset,
            Request::Reboot(RebootTarget::At32Dfu2),
            Request::Identify(5),
            Request::Redetect(Port::Keyboard),
            Request::FwBegin { target: FwTarget::Rp2354, size: 123_456, crc32: 0xAABBCCDD },
            Request::FwData { offset: 56, data: Vec::from_slice(&[7u8; FW_CHUNK]).unwrap() },
            Request::FwCommit,
            Request::FwStatus,
            Request::LearnHotkey(true),
            Request::LearnHotkey(false),
            Request::GetDiag(3),
            Request::PointerAt([-1280, 1439]),
            Request::SetConfig(ConfigValue::AbsPointer(true)),
            Request::SetConfig(ConfigValue::HopAccel(Accel::Adaptive)),
            Request::SetConfig(ConfigValue::HopSpeed(-7)),
            Request::SetConfig(ConfigValue::HopResistance(250)),
            Request::SetConfig(ConfigValue::HopCorner(30)),
            Request::SetConfig(ConfigValue::HopMapping(Pc::Pc2, Mapping::MainScreen)),
            Request::SetConfig(ConfigValue::HopUnits(Pc::Pc1, Units::Logical)),
            Request::SetConfig(ConfigValue::HopUnits(Pc::Pc2, Units::Physical)),
            Request::SetConfig(ConfigValue::ProfileHop(3, true)),
            Request::SetConfig(ConfigValue::Screen(
                7,
                Screen { scale: 150, desk: [-2560, 0], ..Screen::new(Pc::Pc2, 3840, 2160, [-30000, 400]) },
            )),
            Request::SetConfig(ConfigValue::Screen(
                2,
                Screen { off: true, fixed: true, ..Screen::new(Pc::Pc1, 1920, 1080, [0, 0]) },
            )),
            Request::SetConfig(ConfigValue::Screen(0, Screen::NONE)),
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
            asking: Some(Pc::Pc1),
            links: [LinkState::Connected, LinkState::Suspended],
            power: PowerSource::PcBus,
            adv_pwr: Advertised::None,
            adv_pc: [Advertised::A3_0, Advertised::Default],
            vbus_mv: [0, 5100, 4950],
            budget_ma: 2700,
            flags: status_flags::SW_SWITCH_ALLOWED,
            uptime_s: 99,
            link_errors: [0, 2],
            selftest: selftest::PORT_MS | selftest::RUNNING | selftest::BT_GAVE_UP,
            bt_active: 2,
            bt_ready: 0b0011,
            bt_bonded: 0b0111,
            bt_pairing: 42,
            bt_passkey: 3,
            bt_module: diag::BtModule::GaveUp,
            hop: HopState::NotNormalized,
            pointer: [-1920, 1079],
        };
        let port = PortInfo {
            port: Port::Mouse,
            state: PortState::Hosted,
            mode: PortMode::Auto,
            effective: PortMode::Normalized,
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
            (req::SET_PORT_MODE, Response::Error(ErrorCode::Locked)),
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
            Event::ConfigChanged(key::LED_BRIGHTNESS),
            Event::HotkeyLearned(LearnEnd::Learned, Hotkey::combo(0x05, 0x0E)),
            Event::HotkeyLearned(LearnEnd::Learned, Hotkey::double_tap(0xE4, true)),
            Event::HotkeyLearned(LearnEnd::TimedOut, Hotkey::NONE),
        ] {
            assert_eq!(DeviceMsg::decode(&e.encode().unwrap()).unwrap(), DeviceMsg::Event(e));
        }
    }

    #[test]
    fn names_and_cc_classification() {
        for m in PortMode::ALL {
            assert_eq!(PortMode::from_name(m.name()), Some(*m));
        }
        assert_eq!(PortMode::from_name("mirrored-identity"), Some(PortMode::MirroredIdentity));
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
        ];
        for a in [id::AT_CLOCK, id::PC_SPEED, id::PC_LINK, id::PC_RESETS, id::LINK_ERRORS, id::PC_VBUS, id::PC_CC]
            .into_iter()
            .chain([id::AT_3V3, id::AT_TEMP, id::AT_FW])
        {
            ids.extend(a);
        }
        ids.extend(id::PORT_SPEED);
        ids.extend(id::PORT_FAULTS);
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
