//! The KVM's logic without I/O (software.md §4.3–§4.5): ports, switching, hotkeys, presentation, control requests.
//!
//! The RP2354B firmware feeds events in (device reports, link messages, the key, timers) and executes the
//! [`Output`]s that come back. `odkvmctl --sim` drives the very same engine with fake devices.

use heapless::{Deque, String, Vec};
use kvm_hid::neutral::neutral_report;
use kvm_hid::parser::ReportType;
use kvm_hid::translate::{ConsumerState, caps};
use kvm_hid::{KeyState, Layout, MouseInput, Translator, normalized};
use kvm_proto::crc::crc32;
use kvm_proto::ctrl::diag::{self as cdiag, BtModule, ClockSource, Grade, Item, id};
use kvm_proto::ctrl::{
    self, Advertised, AutoReason, BtPointer, ConfigValue, DEVICES, Device, DevicePairing, ErrorCode, Event, FaultKind,
    FwState, FwTarget, GamepadOut, HOSTS, HopState, Host, Hotkey, HotkeyAction, Info, KeyAction, LearnEnd, LinkState,
    MAX_ARRANGEMENTS, PORTS, Pc, Port, PortInfo, PortMode, PortState, PowerSource, RebootTarget, Request, Response,
    Routing, Speed, StringKind, SwitchSource, Target, VialMode, status_flags,
};
use kvm_proto::link::bt::{
    DeviceState, EventKind as BtEventKind, HostState as BtHostState, control as btcontrol, host_flags as bthost,
    report as btreport, reports as btreports,
};
use kvm_proto::link::msg::{Status as AtStatus, UsbSpeed, UsbState};

use crate::bt::{self, BtOut, BtState, Devices, PasskeyStep};
use crate::config::{self, Config, Origin};
use crate::diag::{self, BoardDiag};
use crate::hop;
use crate::hotkey::{HotkeyEngine, Learner};
use crate::leds::LedInputs;
use crate::merge::Merger;
use crate::output::{PcOutput, Report};
use crate::policy::{self, Budget, Candidate, DeviceSummary};
use crate::presentation::{self, IdentitySource, MirrorSource, Params, Presentation, Role};
use crate::standby::{PowerMode, PowerPlan, Presence, Tracker};
use crate::usbdesc::{DeviceDesc, parse_config};

pub const MAX_CONFIG_DESC: usize = 512;
pub const MAX_REPORT_DESC: usize = 1024;
pub const MAX_HID_IFACES: usize = 4;
pub const MAX_CTRL_DATA: usize = 256;
pub const OUTPUT_QUEUE: usize = 48;
/// Largest item [`Engine::take_save_item`] produces.
pub const SAVE_ITEM_MAX: usize = kvm_map::keymap::MACRO_BUFFER;
/// Electronics' own draw, subtracted from the advertised current for the budget (information only: the warning comes
/// from the measured 5 V, `supply`). design.md §6 estimated about 0.5 A before the board existed; the
/// first board took under 1 W from the PWR port in every state, its front devices included (2026-11-17): under 200 mA.
pub const SELF_CURRENT_MA: u16 = 200;
/// The Bluetooth module's average draw on 5 V when fitted (design.md §6.1: about 40 mA at 3.3 V with hosts connected,
/// through the 90 % buck); its +9 dBm transmit peaks (190 mA at 3.3 V) last a few hundred microseconds.
pub const BT_CURRENT_MA: u16 = 30;
/// Autosave waits this long after the last change, so that a run of changes (a slider dragged, Vial setting keys)
/// makes one save.
const AUTOSAVE_MS: u32 = 5000;
/// Flash wear cap (the `store` module documentation has the numbers): saves go ahead as they come up to `SAVE_BURST`
/// in a row, then at most one per `SAVE_PACE_MS`, which is also how fast the allowance comes back. Setting the KVM up
/// never meets it. A host script that changes, or changes and saves, the config every few seconds, day and night, is
/// held to one save a minute: 1 440 a day, about 50 years before a sector reaches its rated erases with the config at
/// its largest. Unpaced, a change every 6 s would take about five years, and `SaveConfig` in a loop days.
const SAVE_BURST: u8 = 30;
const SAVE_PACE_MS: u32 = 60_000;

pub type CtrlData = Vec<u8, MAX_CTRL_DATA>;

/// Everything read from a front-port device while probing it.
#[derive(Debug, Clone)]
pub struct Probed {
    pub device: DeviceDesc,
    /// The speed on Hub-0, which runs at full speed: a high-speed device is probed at full speed.
    pub speed: Option<Speed>,
    /// The device answered GET_DESCRIPTOR(DEVICE_QUALIFIER) at full speed, which only a high-speed device does
    /// (USB 2.0 §9.6.2): on a PC's hub, in passthrough, it runs at high speed.
    pub high_speed: bool,
    pub config: Vec<u8, MAX_CONFIG_DESC>,
    /// HID report descriptor per interface number.
    pub report_descs: Vec<(u8, Vec<u8, MAX_REPORT_DESC>), MAX_HID_IFACES>,
    pub manufacturer: String<64>,
    pub product: String<64>,
    pub serial: String<64>,
}

impl Probed {
    pub const EMPTY: Probed = Probed {
        device: DeviceDesc::EMPTY,
        speed: None,
        high_speed: false,
        config: Vec::new(),
        report_descs: Vec::new(),
        manufacturer: String::new(),
        product: String::new(),
        serial: String::new(),
    };
}

impl Default for Probed {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Route {
    /// Hub-0: the KVM hosts the device.
    Kvm,
    Pc(Pc),
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum KeyEvent {
    Tap,
    Long,
}

/// Who sent a control-protocol request (software.md §8.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Asker {
    /// The service port, or the simulator's own tests: no PC of its own.
    Service,
    /// A PC's control interface.
    Pc(Pc),
    /// Bluetooth host slot 1–4, over the module's control service.
    Bt(u8),
}

impl Asker {
    /// The PC that asked, if a PC did.
    pub const fn pc(self) -> Option<Pc> {
        match self {
            Asker::Pc(pc) => Some(pc),
            _ => None,
        }
    }

    /// The host that asked, as `Status.asking` names it.
    pub const fn host(self) -> Option<Host> {
        match self {
            Asker::Service => None,
            Asker::Pc(pc) => Some(Host::pc(pc)),
            Asker::Bt(n) => Host::bt(n),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FwOp {
    /// RP2354B images go to the update writer, the module's to the module (software.md §4.13).
    Begin {
        target: FwTarget,
        size: u32,
        crc32: u32,
    },
    Data {
        offset: u32,
        data: Vec<u8, { ctrl::FW_CHUNK }>,
    },
    Commit,
    /// The key on the KVM confirmed the committed image: install it.
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Output {
    /// Input report for IN endpoint `ep` (0x81–0x87) of `pc`'s emulated device.
    Report {
        pc: Pc,
        ep: u8,
        data: Report,
    },
    /// The descriptor set changed: upload [`Engine::descriptor_set`] to both AT32F405s.
    DescriptorSetChanged,
    /// SET_REPORT to a front-port device (keyboard LEDs after a switch).
    DeviceSetReport {
        port: Port,
        iface: u8,
        report_type: u8,
        report_id: u8,
        data: Report,
    },
    /// Data for a front-port device's interrupt OUT endpoint.
    DeviceOut {
        port: Port,
        iface: u8,
        data: Report,
    },
    /// A PC's control request for a mirrored interface, to run on the real device; answer with
    /// [`Engine::device_ctrl_reply`].
    DeviceControl {
        pc: Pc,
        tag: u8,
        port: Port,
        setup: [u8; 8],
        data: CtrlData,
    },
    /// Answer to `pc`'s forwarded control request `tag`.
    CtrlReply {
        pc: Pc,
        tag: u8,
        stall: bool,
        data: CtrlData,
    },
    Route {
        port: Port,
        route: Route,
        power_cycle: bool,
    },
    /// Send the port back to Hub-0 and probe it again.
    Reprobe(Port),
    /// Write what changed to flash: [`Engine::take_save_item`] until it returns `None`, then [`Engine::config_saved`].
    Save,
    Reboot(RebootTarget),
    Firmware(FwOp),
    /// Front-port power, Hub-0 and LEDs for standby (software.md §4.12); sent when the plan changes.
    Power(PowerPlan),
    /// Ask `pc`'s AT32F405 to signal remote wakeup (the PC sleeps and a key was pressed).
    RemoteWakeup(Pc),
    /// For the Bluetooth module (software.md §4.13): a report for the active host, or a command.
    Bt(BtOut),
}

struct Hosted {
    probed: Probed,
    translators: Vec<(u8, Translator), MAX_HID_IFACES>,
    /// The XInput interfaces (no report descriptor: `kvm_hid::xinput` decodes them), wired or a receiver's pads.
    xinput: Vec<(u8, kvm_hid::xinput::Kind), MAX_HID_IFACES>,
    /// How its gamepad becomes the standard one (`pads`).
    profile: crate::pads::Profile,
    summary: DeviceSummary,
    /// Length of the last raw report per interface (for neutral XInput reports).
    last_len: Vec<(u8, u8), MAX_HID_IFACES>,
}

impl Hosted {
    fn translator(&self, iface: u8) -> Option<&Translator> {
        self.translators.iter().find(|(n, _)| *n == iface).map(|(_, t)| t)
    }

    fn xinput(&self, iface: u8) -> Option<kvm_hid::xinput::Kind> {
        self.xinput.iter().find(|(n, _)| *n == iface).map(|(_, k)| *k)
    }

    /// The receiver pad interface the KVM polls (`ConfigInfo::polled`: the first).
    fn receiver_pad(&self) -> Option<u8> {
        self.xinput.iter().find(|(_, k)| *k == kvm_hid::xinput::Kind::Wireless).map(|(n, _)| *n)
    }
}

/// A front port's ESP32-C3 board as the Bluetooth module (software.md §4.13): being found out, or taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dongle {
    None,
    /// The firmware talks to it: the module's `Hello`, or a blank flash it may program.
    Probing(Port),
    /// It is the module: the port shows `PortState::Module`.
    Claimed(Port),
}

struct PortSlot {
    state: PortState,
    effective: PortMode,
    reason: AutoReason,
    /// The PC its router sends it to, while in passthrough.
    routed: Option<Pc>,
    dev: Option<Hosted>,
    /// Plugged into Hub-0 anew: empty at this time unless Hub-0 sees a device by then ([`REPROBE_MS`]).
    probe_until: Option<u32>,
    reports: u32,
    rate_hz: u16,
    faults: u16,
}

impl PortSlot {
    const fn new() -> Self {
        Self {
            state: PortState::Empty,
            effective: PortMode::Auto,
            reason: AutoReason::NotAuto,
            routed: None,
            dev: None,
            probe_until: None,
            reports: 0,
            rate_hz: 0,
            faults: 0,
        }
    }

    fn is_mirrored(&self) -> bool {
        self.state == PortState::Hosted && matches!(self.effective, PortMode::Mirrored | PortMode::MirroredIdentity)
    }

    fn is_normalized(&self) -> bool {
        self.state == PortState::Hosted && self.effective == PortMode::Normalized
    }
}

/// The devices pinned to a host while it isn't the active one (software.md §4.3): their input goes there as it comes,
/// without the keymap or screen hop, which are the active target's. (Pinned to the active host, a device's input goes
/// with the devices that follow it.)
struct Pinned {
    /// Normalized devices (and, for a Bluetooth host, mirrored ones: it only has the KVM's own reports), and devices
    /// mirrored to a PC (raw to its mirror endpoints; merged for the hotkeys).
    norm: Merger,
    mirr: Merger,
    /// What went out last, so that only changes go.
    keys: KeyState,
    consumer: ConsumerState,
    system: u8,
    buttons: u16,
    bt_keyboard: [u8; btreport::KEYBOARD_LEN],
    bt_consumer: u16,
    /// Held when a device's input came here, hidden until released.
    suppressed_keys: KeyState,
    suppressed_buttons: u16,
}

impl Pinned {
    const NEW: Pinned = Pinned {
        norm: Merger::new(),
        mirr: Merger::new(),
        keys: KeyState::EMPTY,
        consumer: ConsumerState { usages: [0; kvm_hid::normalized::CONSUMER_SLOTS] },
        system: 0,
        buttons: 0,
        bt_keyboard: [0; btreport::KEYBOARD_LEN],
        bt_consumer: 0,
        suppressed_keys: KeyState::EMPTY,
        suppressed_buttons: 0,
    };
}

struct PcSlot {
    link: LinkState,
    out: PcOutput,
    status: Option<AtStatus>,
    fw: u32,
}

impl PcSlot {
    const NEW: PcSlot = PcSlot { link: LinkState::LinkDown, out: PcOutput::DEFAULT, status: None, fw: 0 };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PowerInfo {
    pub source: PowerSource,
    pub adv_pwr: Advertised,
    pub vbus_pwr_mv: u16,
}

struct CacheEntry {
    port: Port,
    iface: u8,
    value: u16,
    data: CtrlData,
}

/// Watching the controls of the normalized devices for the host software (`Request::WatchControls`): learning one in
/// the mapping editor.
#[derive(Debug, Clone, Copy)]
struct Watch {
    /// Who asked (None: the service port or the simulator), and whether the controls are kept from the hosts meanwhile.
    pc: Option<Pc>,
    swallow: bool,
    /// The positions last told; the axes last told, motion summed since, and when each was last told.
    held: kvm_map::PosSet,
    axes: [i32; kvm_map::axes::INPUTS],
    moved: [i32; kvm_map::axes::RELATIVE],
    told_at: [u32; kvm_map::axes::INPUTS],
    /// When it ends unless asked again: a host program that went away doesn't keep the controls.
    until: u32,
}

/// How often an axis is told while watching, and how far an absolute one must move for it.
const WATCH_AXIS_MS: u32 = 50;
const WATCH_AXIS_STEP: i32 = 1024;
/// How long a watch lasts after the request that started or renewed it.
pub const WATCH_MS: u32 = 10_000;

/// Streaming one device's raw reports to the host software (`Request::MonitorDevice`, hid_mapping.md §9): a report
/// that changed, at most one per [`MONITOR_MS`] per interface and report ID (one held back is not sent later: the
/// next change brings it up to date). Ends as a watch does.
#[derive(Debug, Clone, Copy)]
struct Monitor {
    pc: Option<Pc>,
    device: Device,
    until: u32,
    /// Per interface and report ID: a hash of the report last told, and when.
    sent: [(u8, u8, u32, u32); MONITOR_SLOTS],
    used: u8,
}

const MONITOR_MS: u32 = 16;
const MONITOR_SLOTS: usize = 8;

pub struct Engine {
    config: Config,
    /// Unsaved changes (config or keymap) since then; saved [`AUTOSAVE_MS`] after the last one.
    dirty_since: Option<u32>,
    config_dirty: bool,
    /// Arrangements to write (bit n: arrangement n's item).
    arrangements_dirty: u8,
    /// Where screen hop last put the active Bluetooth host's absolute pointer: released there when it is left.
    bt_pointer_at: Option<[u16; 2]>,
    /// The buttons last sent in the active Bluetooth host's mouse report.
    bt_mouse_buttons: u16,
    /// Each PC's host software places its cursor on these screens until then (`Request::PlaceCursor`, a lease).
    placer: [Option<(ctrl::Placement, u32)>; 2],
    /// Each PC's last `Event::PlaceCursor`: when it went, and a later place waiting for the next millisecond.
    place_sent: [Option<u32>; 2],
    place_pending: [Option<[i16; 2]>; 2],
    /// Saves that may go ahead now, up to [`SAVE_BURST`]; one more each [`SAVE_PACE_MS`] after `save_paced`.
    save_allowance: u8,
    save_paced: u32,
    /// Keymap, keymap engine and Vial sessions (software.md §4.11).
    vial: kvm_map::Vial,
    keymap_changes: u32,
    hotkeys: HotkeyEngine,
    /// The devices whose input goes to the active target: normalized ones (forwarded through the KVM's interfaces)
    /// and mirrored ones (forwarded raw).
    norm: Merger,
    mirr: Merger,
    /// Each host's pinned devices while it isn't the active one ([`Host::index`]).
    pinned: [Pinned; HOSTS],
    /// Where each device's input is merged now: with the active target's (None), or a host's pinned devices.
    group: [Option<Host>; DEVICES],
    active: Pc,
    suppressed_keys: KeyState,
    suppressed_buttons: u16,
    forwarded_keys: KeyState,
    forwarded_consumer: ConsumerState,
    forwarded_system: u8,
    /// Mouse buttons the keymap holds.
    keymap_buttons: u16,
    /// The axis rules' state (hid_mapping.md), and the axis directions they hold down as keymap positions.
    axes: kvm_map::axes::Axes,
    axis_buttons: kvm_map::PosSet,
    /// The virtual gamepad: what the active target should have now, and what the active PC was sent.
    pad: normalized::Pad,
    pad_sent: normalized::Pad,
    /// Watching the controls for the host software (`Request::WatchControls`).
    watch: Option<Watch>,
    /// Streaming a device's raw reports to the host software (`Request::MonitorDevice`).
    monitor: Option<Monitor>,
    /// The keymap running now: the active profile's, or its fallback while the input's host can't take the virtual
    /// gamepad (`Config::keymap`).
    keymap_now: Option<u8>,
    /// Gamepad axis calibration and field overrides per device identity.
    cal: crate::known::KnownDevices,
    pcs: [PcSlot; 2],
    ports: [PortSlot; PORTS],
    presentation: Presentation,
    desc: Vec<u8, { kvm_proto::descset::MAX_LEN }>,
    out: Deque<Output, OUTPUT_QUEUE>,
    power: PowerInfo,
    info: Info,
    serial: String<16>,
    fw: (FwState, u32),
    /// The firmware's update writer is busy with flash, or the module's stream is full: FwBegin/FwData get Busy, and
    /// the host retries.
    fw_busy: bool,
    /// What the update under way writes.
    fw_target: FwTarget,
    identify_until: u32,
    /// `ctrl::selftest` bits, and when the self-test finished (its result shows on the LEDs for a while).
    selftest: u32,
    /// The Bluetooth module as the firmware sees it (`set_bt_module`): absent, up, being programmed, failed.
    bt_module: BtModule,
    /// The module is on the board (`set_bt_fitted`, from what the firmware read at boot): then no dongle is taken.
    bt_fitted: bool,
    /// How far programming the module has got (`set_bt_progress`, percent).
    bt_progress: u8,
    /// The module's chip (`set_bt_chip`): which firmware image is its.
    bt_chip: kvm_proto::link::bt::Chip,
    /// A front port's ESP32-C3 board that is, or may be, the module (software.md §4.13).
    dongle: Dongle,
    /// A front port whose ESP32 board is to become the module whatever it runs (`Request::AdoptModule`), until the
    /// board is the module or the port has another device.
    adopt: Option<Port>,
    selftest_done_at: Option<u32>,
    /// When the active profile last changed (its number shows on the LEDs for a while).
    profile_changed_at: Option<u32>,
    /// Software asked to turn switching / configuration from software back on: a tap on the key until this time
    /// confirms it.
    unlock: Option<Unlock>,
    /// Learning a hotkey (software.md §4.5).
    learning: Option<Learning>,
    link_errors: [u16; 2],
    /// The firmware's own measurements for the diagnostics; None until it sends them (and in tests).
    board: Option<BoardDiag>,
    now: u32,
    rate_epoch: u32,
    cache: Vec<CacheEntry, 8>,
    dropped_outputs: u32,
    /// Standby and auto-select (software.md §4.12).
    standby: Tracker,
    /// The 5 V the KVM runs from, as measured (software.md §4.7).
    supply: crate::supply::Tracker,
    /// The Bluetooth module and its hosts (software.md §4.13).
    bt: BtState,
    /// Its devices.
    wireless: Devices,
    /// Its recent log (`Request::GetModuleLog`).
    module_log: crate::bt::ModuleLog,
    /// Screen hop's pointer (software.md §4.14).
    hop: hop::Pointer,
}

/// How long the self-test result stays on the LEDs.
const SELFTEST_SHOW_MS: u32 = 5000;
/// How long the LEDs show the number of a profile just made active.
const PROFILE_SHOW_MS: u32 = 1500;
/// How long a request to allow software control again waits for the key.
pub const UNLOCK_WAIT_MS: u32 = 30_000;
/// How long learning a hotkey waits for the keys.
pub const LEARN_MS: u32 = 10_000;
/// How long a port plugged into Hub-0 anew (`Output::Reprobe`) waits for its device before it is empty: the replug
/// takes 250 ms and Hub-0's debounce 100 ms. A device unplugged in passthrough, where the KVM can't see it, never comes.
pub const REPROBE_MS: u32 = 2000;

/// A hotkey being learned: for which PC (None: the service port), until when.
struct Learning {
    pc: Option<Pc>,
    until: u32,
    learner: Learner,
}

/// Which software locks a tap on the key would lift, or the Bluetooth host it would let control the KVM, and until
/// when. A host's request and the locks' don't share a tap (`Busy`), nor do two hosts': a tap grants only what the one
/// asking is told it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unlock {
    switching: bool,
    config: bool,
    /// Bluetooth host slot 1–4 that asked to control the KVM (`Request::AskControl`, or `BtControl` set from a PC).
    host: Option<u8>,
    until: u32,
}

fn usb_to_link(s: UsbState) -> LinkState {
    match s {
        UsbState::Unpowered => LinkState::Unpowered,
        UsbState::Powered | UsbState::Default | UsbState::Addressed => LinkState::Enumerating,
        UsbState::Configured => LinkState::Connected,
        UsbState::Suspended => LinkState::Suspended,
        UsbState::Detached => LinkState::Detached,
    }
}

fn report(d: &[u8]) -> Report {
    Report::from_slice(&d[..d.len().min(64)]).unwrap_or_default()
}

impl Engine {
    /// An engine started with `config`, `info` and `serial` (see [`Engine::start`]).
    pub fn new(config: Config, info: Info, serial: &str) -> Self {
        let mut e = Self::empty();
        e.start(config, info, serial);
        e
    }

    /// An engine before [`Engine::start`]. `const`, so that the firmware's static holds it from the start: the engine
    /// is over 100 KB, and built on a stack and moved it would take two copies of that.
    pub const fn empty() -> Self {
        Self {
            config: Config::DEFAULT,
            dirty_since: None,
            config_dirty: false,
            arrangements_dirty: 0,
            bt_pointer_at: None,
            bt_mouse_buttons: 0,
            placer: [None; 2],
            place_sent: [None; 2],
            place_pending: [None; 2],
            save_allowance: SAVE_BURST,
            save_paced: 0,
            vial: kvm_map::Vial::new(),
            keymap_changes: 0,
            hotkeys: HotkeyEngine::EMPTY,
            norm: Merger::new(),
            mirr: Merger::new(),
            pinned: [Pinned::NEW, Pinned::NEW, Pinned::NEW, Pinned::NEW, Pinned::NEW, Pinned::NEW],
            group: [None; DEVICES],
            active: Pc::Pc1,
            suppressed_keys: KeyState::EMPTY,
            suppressed_buttons: 0,
            forwarded_keys: KeyState::EMPTY,
            forwarded_consumer: ConsumerState { usages: [0; kvm_hid::normalized::CONSUMER_SLOTS] },
            forwarded_system: 0,
            keymap_buttons: 0,
            axes: kvm_map::axes::Axes::new(),
            axis_buttons: kvm_map::PosSet::EMPTY,
            pad: normalized::Pad { buttons: 0, dpad: 0, axes: [0; 8] },
            pad_sent: normalized::Pad { buttons: 0, dpad: 0, axes: [0; 8] },
            watch: None,
            monitor: None,
            keymap_now: None,
            cal: crate::known::KnownDevices::new(),
            pcs: [PcSlot::NEW, PcSlot::NEW],
            ports: [const { PortSlot::new() }; PORTS],
            presentation: Presentation::EMPTY,
            desc: Vec::new(),
            out: Deque::new(),
            power: PowerInfo { source: PowerSource::PcBus, adv_pwr: Advertised::None, vbus_pwr_mv: 0 },
            info: Info { proto: 0, rp_fw: 0, at_fw: [0; 2], hw: 0, serial: [0; 8], caps: 0, bt_fw: 0 },
            serial: String::new(),
            fw: (FwState::Idle, 0),
            fw_busy: false,
            fw_target: FwTarget::Rp2354,
            identify_until: 0,
            selftest: ctrl::selftest::RUNNING,
            bt_module: BtModule::Absent,
            bt_fitted: false,
            bt_progress: 0,
            bt_chip: kvm_proto::link::bt::Chip::Unknown,
            dongle: Dongle::None,
            adopt: None,
            selftest_done_at: None,
            profile_changed_at: None,
            unlock: None,
            learning: None,
            link_errors: [0, 0],
            board: None,
            now: 0,
            rate_epoch: 0,
            cache: Vec::new(),
            dropped_outputs: 0,
            standby: Tracker::new(),
            supply: crate::supply::Tracker::new(),
            bt: BtState::EMPTY,
            wireless: Devices::EMPTY,
            module_log: crate::bt::ModuleLog::EMPTY,
            hop: hop::Pointer::NEW,
        }
    }

    /// Takes the saved config, the firmware's identity (`info` carries its version, hardware revision and capability
    /// bits) and the serial number, and builds the descriptor set.
    pub fn start(&mut self, config: Config, info: Info, serial: &str) {
        self.config = config;
        self.hotkeys.configure(&self.config);
        self.info = info;
        self.serial = String::try_from(&serial[..serial.len().min(16)]).unwrap_or_default();
        self.rebuild_presentation();
    }

    // ── accessors ────────────────────────────────────────────────────────────

    /// Next output to execute. Drain them after every call into the engine: the queue holds what one call produces
    /// ([`OUTPUT_QUEUE`]), and anything beyond that is dropped and counted in [`Engine::dropped_outputs`].
    pub fn poll_output(&mut self) -> Option<Output> {
        self.out.pop_front()
    }

    pub fn has_output(&self) -> bool {
        !self.out.is_empty()
    }

    pub fn descriptor_set(&self) -> &[u8] {
        &self.desc
    }

    pub fn descriptor_crc(&self) -> u32 {
        crc32(&self.desc)
    }

    pub fn presentation(&self) -> &Presentation {
        &self.presentation
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    /// The PC that gets input, or that got it before a Bluetooth host was selected (the *home* PC).
    pub fn active(&self) -> Pc {
        self.active
    }

    /// The Bluetooth host slot that gets input (1–4), if one does.
    pub fn bt_active(&self) -> Option<u8> {
        self.bt.active
    }

    pub fn bt(&self) -> &BtState {
        &self.bt
    }

    pub fn wireless(&self) -> &Devices {
        &self.wireless
    }

    pub fn port_info(&self, port: Port) -> PortInfo {
        let s = &self.ports[port.index()];
        let mut p = PortInfo::empty(port, self.config.mode(port.into()));
        p.state = s.state;
        p.effective = s.effective;
        p.reason = s.reason;
        p.target = match s.state {
            PortState::Passthrough => s.routed.map(Host::pc),
            PortState::Hosted if s.effective != PortMode::Off => Some(self.target_of(port.into())),
            _ => None,
        };
        p.report_rate_hz = s.rate_hz;
        p.faults = s.faults;
        if let Some(d) = &s.dev {
            p.vid = d.summary.vid;
            p.pid = d.summary.pid;
            p.speed = d.summary.speed.unwrap_or(Speed::Unknown);
            // In passthrough the device is on the PC's hub, not on Hub-0 where it was probed (at full speed at most).
            // A high-speed device runs at high speed there unless the hub's own link to the PC is full speed: the
            // AT32F405 on that hub shows it.
            if s.state == PortState::Passthrough && d.probed.high_speed {
                let pc = self.passthrough_pc(port);
                p.speed = match self.pcs[pc.index()].status.map(|st| st.speed) {
                    Some(UsbSpeed::Full) => Speed::Full,
                    _ => Speed::High,
                };
            }
            p.classes = d.summary.classes();
            p.interfaces = d.summary.interfaces.len() as u8;
            p.max_power_ma = d.summary.max_power_ma;
        }
        p
    }

    pub fn status(&self, asking: Option<Host>) -> ctrl::Status {
        let adv_pc =
            |i: usize| self.pcs[i].status.map_or(Advertised::None, |s| Advertised::from_cc_mv(s.cc1_mv.max(s.cc2_mv)));
        let adv = [adv_pc(0), adv_pc(1)];
        let available = match self.power.source {
            PowerSource::PwrPort => self.power.adv_pwr.milliamps(),
            PowerSource::PcBus => adv[0].milliamps().max(adv[1].milliamps()),
        };
        // A fitted module draws its current whether it answers or not (starting, being programmed, failed); a dongle
        // draws it from its port, which counts it as the port's device.
        let on_board = self.bt_module != BtModule::Absent && self.dongle == Dongle::None;
        let own = SELF_CURRENT_MA + if on_board { BT_CURRENT_MA } else { 0 };
        let budget = available.saturating_sub(own);
        let mut flags = 0;
        if self.config.sw_switch_allowed {
            flags |= status_flags::SW_SWITCH_ALLOWED;
        }
        if self.config.sw_config_allowed {
            flags |= status_flags::SW_CONFIG_ALLOWED;
        }
        if self.dirty_since.is_some() {
            flags |= status_flags::CONFIG_DIRTY;
        }
        if self.fw.0 == FwState::AwaitingKey {
            flags |= status_flags::UPDATE_ARMED;
        }
        if matches!(self.supply.level(), ctrl::Supply::Sags | ctrl::Supply::Low) {
            flags |= status_flags::SUPPLY_SAGS;
        }
        if self.selftest & !ctrl::selftest::RUNNING != 0 {
            flags |= status_flags::SELFTEST_FAILED;
        }
        ctrl::Status {
            active: self.active,
            asking,
            links: [self.pcs[0].link, self.pcs[1].link],
            power: self.power.source,
            adv_pwr: self.power.adv_pwr,
            adv_pc: adv,
            vbus_mv: [
                self.power.vbus_pwr_mv,
                self.pcs[0].status.map_or(0, |s| s.vbus_mv),
                self.pcs[1].status.map_or(0, |s| s.vbus_mv),
            ],
            budget_ma: budget,
            flags,
            uptime_s: self.now / 1000,
            link_errors: self.link_errors,
            selftest: self.selftest,
            bt_active: self.bt.active.unwrap_or(0),
            bt_ready: self.bt.mask(|h| h.ready()),
            bt_bonded: self.bt.mask(|h| h.bonded()),
            bt_pairing: self.bt.pairing_seconds(self.now),
            bt_passkey: self.bt.passkey.map_or(0, |p| p.host),
            bt_passkey_digits: self.bt.passkey.map_or(0, |p| p.digits),
            bt_passkey_typed: self.bt.passkey.map_or(0, |p| p.value),
            supply: self.supply.level(),
            supply_mv: self.supply.readings().0,
            supply_lowest_mv: self.supply.readings().1,
            declared_ma: self.declared_ma(),
            bt_module: self.bt_module,
            hop: self.hop_state(),
            pointer: self.hop.position_on(self.config.layout(), self.active_host()),
            bt_devices_bonded: if self.bt.present { self.wireless.mask(|w| w.state.bonded()) } else { 0 },
            bt_devices_ready: if self.bt.present { self.wireless.mask(|w| w.live()) } else { 0 },
            bt_scan: self.wireless.scan_seconds(self.now),
            bt_found: self.wireless.found.len() as u8,
            bt_device_pairing: self.wireless.pairing,
            bt_device_passkey: self.wireless.passkey,
            bt_progress: if self.bt_module == BtModule::Programming { self.bt_progress } else { 0 },
            bt_chip: self.bt_chip,
        }
    }

    /// Where `d`'s input goes now: the host it is pinned to, or the active one.
    pub fn target_of(&self, d: Device) -> Host {
        match self.config.route(d) {
            Routing::Pin(h) => h,
            Routing::Follow => self.active_host(),
        }
    }

    /// What `d` runs as now: a front port's effective mode while the KVM hosts its device, a Bluetooth device's
    /// setting (normalized, or off) while it is one of the KVM's devices; Off otherwise.
    fn running(&self, d: Device) -> PortMode {
        match (d.port(), d.wireless()) {
            (Some(p), _) => {
                let s = &self.ports[p.index()];
                if s.state == PortState::Hosted { s.effective } else { PortMode::Off }
            }
            (None, Some(n)) => {
                let usable = self.bt.present && self.wireless.slot(n).is_some_and(|w| w.usable());
                match self.config.mode(d) {
                    _ if !usable => PortMode::Off,
                    PortMode::Off => PortMode::Off,
                    _ => PortMode::Normalized,
                }
            }
            (None, None) => PortMode::Off,
        }
    }

    /// The translator of `d`'s interface `iface` (a Bluetooth device has one, interface 0).
    fn translator(&self, d: Device, iface: u8) -> Option<&Translator> {
        match (d.port(), d.wireless()) {
            (Some(p), _) => self.ports[p.index()].dev.as_ref()?.translator(iface),
            (None, Some(n)) => self.wireless.slot(n)?.translator.as_ref(),
            (None, None) => None,
        }
    }

    /// A Bluetooth device is normalized: the KVM's own interfaces are needed for it.
    fn wireless_normalized(&self) -> bool {
        Device::ALL.iter().any(|d| d.wireless().is_some() && self.running(*d) == PortMode::Normalized)
    }

    /// The host that has the input: a Bluetooth host, or the active PC.
    pub fn active_host(&self) -> Host {
        self.bt.active.and_then(Host::bt).unwrap_or(Host::pc(self.active))
    }

    /// Whether screen hop moves the pointer now, and if not, why not (`Status.hop`). A Bluetooth host takes part like
    /// a PC (protocol 9).
    pub fn hop_state(&self) -> HopState {
        let mouse = &self.ports[Port::Mouse.index()];
        let mouse_elsewhere =
            matches!(mouse.state, PortState::Hosted | PortState::Passthrough) && !mouse.is_normalized();
        let host = self.active_host();
        if !self.config.hop_on() {
            HopState::Off
        } else if !self.config.abs_pointer {
            HopState::NoAbsolute
        } else if mouse_elsewhere || !self.presentation.mouse_abs {
            HopState::NotNormalized
        } else if host.as_pc().is_some_and(|pc| self.pcs[pc.index()].out.boot_mouse) {
            HopState::BootProtocol
        } else if !hop::has_screens(self.config.layout(), host) {
            HopState::NoScreens
        } else {
            HopState::Active
        }
    }

    /// Where screen hop's pointer is (the slot of its screen, and the position in the arrangement).
    pub fn hop_pointer(&self) -> (Option<usize>, [i16; 2]) {
        (self.hop.screen(), self.hop.position())
    }

    /// What the front devices declare they may draw at most (`bMaxPower`): information, as most draw far less.
    fn declared_ma(&self) -> u16 {
        self.ports.iter().filter_map(|p| p.dev.as_ref()).map(|d| d.summary.max_power_ma).fold(0, u16::saturating_add)
    }

    pub fn led_inputs(&self) -> LedInputs {
        let pinned = |i: usize| match Device::from_index(i).map(|d| self.config.route(d)) {
            Some(Routing::Pin(h)) => h.as_pc(),
            _ => None,
        };
        let port = |i: usize| (self.ports[i].state, self.ports[i].effective, pinned(i));
        LedInputs {
            active: self.active,
            links: [self.pcs[0].link, self.pcs[1].link],
            aux1: self.has_port(Port::Aux1),
            ports: core::array::from_fn(port),
            color_pc1: self.config.color_pc1,
            color_pc2: self.config.color_pc2,
            brightness: self.config.led_brightness,
            identify: self.now < self.identify_until,
            awaiting_key: self.fw.0 == FwState::AwaitingKey || self.unlock.is_some(),
            supply_sags: matches!(self.supply.level(), ctrl::Supply::Sags | ctrl::Supply::Low),
            selftest: self.selftest_showing().then_some(self.selftest),
            standby: self.standby.plan().mode != PowerMode::Active,
            standby_glow: self.config.standby_glow,
            fault: self.selftest & ctrl::selftest::RUNNING == 0 && self.selftest & ctrl::selftest::ON_KEY != 0,
            profile: self
                .profile_changed_at
                .filter(|t| self.now.wrapping_sub(*t) < PROFILE_SHOW_MS)
                .map(|_| self.config.profile),
            bt_active: self.bt.active.is_some(),
            bt_pairing: self.bt.pairing_until.is_some() || self.wireless.scan_until.is_some(),
            bt_passkey: self.bt.passkey.is_some(),
        }
    }

    /// The board has front port `p`: every port but Aux 1, which only boards from 0.19.0 have (`caps::AUX1`).
    pub fn has_port(&self, p: Port) -> bool {
        p != Port::Aux1 || self.info.caps & ctrl::caps::AUX1 != 0
    }

    fn selftest_showing(&self) -> bool {
        self.selftest & ctrl::selftest::RUNNING != 0
            || self.selftest_done_at.is_some_and(|t| self.now.wrapping_sub(t) < SELFTEST_SHOW_MS)
    }

    /// The power plan in force (software.md §4.12).
    pub fn power_plan(&self) -> PowerPlan {
        self.standby.plan()
    }

    /// Outputs lost to a full queue since boot (should stay 0).
    pub fn dropped_outputs(&self) -> u32 {
        self.dropped_outputs
    }

    // ── plumbing ─────────────────────────────────────────────────────────────

    fn emit(&mut self, o: Output) {
        if self.out.push_back(o).is_err() {
            self.dropped_outputs += 1;
        }
    }

    fn send(&mut self, pc: Pc, ep: u8, data: Report) {
        if ep != 0 {
            self.emit(Output::Report { pc, ep, data });
        }
    }

    fn event(&mut self, e: Event) {
        for pc in [Pc::Pc1, Pc::Pc2] {
            self.event_to(pc, e);
        }
        // The Bluetooth hosts' sessions get what tells them something changed; the streams (a control, an axis, a
        // device's reports, the cursor's place) are for a PC that asked, and would fill the module's link.
        if matches!(e, Event::Control(..) | Event::Axis(..) | Event::Report { .. } | Event::PlaceCursor(_)) {
            return;
        }
        for n in 1..=ctrl::BT_HOSTS as u8 {
            if self.bt_session(n) {
                self.bt_reply(n, e.encode().ok());
            }
        }
    }

    /// Bluetooth host `n` has a control session open (its software subscribed to the control service's answers), and
    /// may control the KVM: it hears of changes as the PCs do.
    fn bt_session(&self, n: u8) -> bool {
        let Some(i) = (n as usize).checked_sub(1).filter(|i| *i < ctrl::BT_HOSTS) else { return false };
        let h = &self.bt.hosts[i];
        self.bt.present && self.config.bt_control[i] && h.connected() && h.flags & bthost::CONTROL != 0
    }

    /// A report for Bluetooth host `n`'s session: an answer or an event.
    fn bt_reply(&mut self, n: u8, report: Option<[u8; ctrl::REPORT_LEN]>) {
        if let Some(data) = report {
            self.emit(Output::Bt(BtOut::Control { host: n, data }));
        }
    }

    fn event_to(&mut self, pc: Pc, e: Event) {
        if let Ok(raw) = e.encode() {
            let ep = self.presentation.control.ep_in;
            self.send(pc, ep, report(&raw));
        }
    }

    fn config_changed(&mut self, key: u16) {
        if let Some(a) = ctrl::key::arrangement_of(key) {
            // An arrangement's screens and name: its own item.
            self.dirty_since = Some(self.now);
            self.arrangements_dirty |= 1 << a;
        } else if key != ctrl::key::PROFILE || self.config.remember_profile {
            // A profile that isn't remembered across power cycles needs no flash write.
            self.dirty_since = Some(self.now);
            self.config_dirty = true;
        }
        self.event(Event::ConfigChanged(key));
    }

    /// What identifies the device in slot `d` across ports and restarts (calibration, the host software's names for its
    /// controls): VID, PID and serial number on a front port, the address as a Bluetooth device. None: no device.
    fn identity(&self, d: Device) -> Option<u32> {
        match (d.port(), d.wireless()) {
            (Some(p), _) => {
                let dev = self.ports[p.index()].dev.as_ref()?;
                let desc = &dev.probed.device;
                Some(crate::known::identity(&[
                    &desc.vid.to_le_bytes(),
                    &desc.pid.to_le_bytes(),
                    dev.probed.serial.as_bytes(),
                ]))
            }
            (None, Some(n)) => {
                let w = self.wireless.slot(n)?;
                w.state.bonded().then(|| crate::known::identity(&[&w.state.addr]))
            }
            (None, None) => None,
        }
    }

    /// What the device in slot `d` has (`Request::GetControls`).
    fn controls(&self, d: Device) -> Option<ctrl::Controls> {
        let identity = self.identity(d)?;
        let mut c = ctrl::Controls { identity, ..ctrl::Controls::default() };
        let mut add = |t: &Translator| {
            let k = t.controls();
            c.keyboard |= k.keyboard;
            c.mouse_buttons |= k.mouse_buttons;
            c.pad_buttons |= k.pad_buttons;
            c.mouse_axes |= k.mouse_axes;
            c.pad_axes |= k.pad_axes;
            c.hat |= k.hat;
        };
        match (d.port(), d.wireless()) {
            (Some(p), _) => {
                let dev = self.ports[p.index()].dev.as_ref()?;
                dev.translators.iter().for_each(|(_, t)| add(t));
                // XInput pads, and the order the device's profile puts its buttons and axes in.
                if !dev.xinput.is_empty() {
                    c.pad_buttons |= kvm_hid::xinput::BUTTONS_PRESENT;
                    c.pad_axes |= kvm_hid::xinput::present();
                    c.hat = true;
                }
                (c.pad_buttons, c.pad_axes) = dev.profile.controls(c.pad_buttons, c.pad_axes);
            }
            (None, Some(n)) => add(self.wireless.slot(n)?.translator.as_ref()?),
            (None, None) => {}
        }
        let (buttons, axes) = self.cal.override_controls(identity);
        c.pad_buttons |= buttons;
        c.pad_axes |= axes;
        Some(c)
    }

    /// A device's raw report for the host software monitoring it ([`Monitor`]).
    fn monitor_report(&mut self, d: Device, iface: u8, data: &[u8]) {
        let Some(mut m) = self.monitor.filter(|m| m.device == d) else { return };
        let ids = self.translator(d, iface).is_some_and(|t| t.layout().uses_report_ids);
        let id = if ids { data.first().copied().unwrap_or(0) } else { 0 };
        let hash = crate::known::identity(&[data]);
        let used = m.used as usize;
        let i = match m.sent[..used].iter().position(|s| (s.0, s.1) == (iface, id)) {
            Some(i) => {
                let (_, _, h, at) = m.sent[i];
                if h == hash || self.now.wrapping_sub(at) < MONITOR_MS {
                    return;
                }
                i
            }
            None if used < MONITOR_SLOTS => {
                m.used += 1;
                used
            }
            None => return,
        };
        m.sent[i] = (iface, id, hash, self.now);
        self.monitor = Some(m);
        let total = data.len().min(u8::MAX as usize);
        for (k, chunk) in data[..total].chunks(ctrl::REPORT_PIECE).enumerate() {
            let mut piece = [0u8; ctrl::REPORT_PIECE];
            piece[..chunk.len()].copy_from_slice(chunk);
            let offset = (k * ctrl::REPORT_PIECE) as u8;
            let e = Event::Report { device: d, iface, offset, total: total as u8, len: chunk.len() as u8, data: piece };
            self.tell(m.pc, e);
        }
    }

    /// Ends watching the controls; those still held after a watch that kept them go nowhere until released, as after a
    /// switch.
    fn end_watch(&mut self) {
        if self.watch.take().is_some_and(|w| w.swallow) {
            self.suppressed_keys = self.norm.keys();
            self.suppressed_buttons = self.norm.buttons();
        }
    }

    /// The controls held down, and the axes, told to the host software watching them (`Request::WatchControls`).
    /// Returns whether it keeps them from the hosts.
    fn watch_controls(&mut self, keys: &KeyState, m: &crate::merge::Merged) -> bool {
        use kvm_map::axes::{INPUTS, RELATIVE};
        let Some(mut w) = self.watch else { return false };
        let mut pos = kvm_map::PosSet::from_keys(keys);
        pos.add_mask(kvm_map::pos::MOUSE, self.norm.buttons() as u32, kvm_map::pos::MOUSE_BUTTONS);
        let pad = self.norm.gamepad();
        pos.add_mask(kvm_map::pos::PAD, pad.buttons, kvm_map::pos::PAD_BUTTONS);
        pos.add_mask(kvm_map::pos::HAT, pad.dpad() as u32, 4);
        let mut told = 0;
        for p in 0..kvm_map::POSITIONS as u16 {
            let down = pos.has(p);
            // Up to 8 a report: a keyboard's NKRO report rarely changes more.
            if down != w.held.has(p) && told < 8 {
                w.held.set(p, down);
                told += 1;
                self.tell(w.pc, Event::Control(p, down));
            }
        }
        if let Some(mo) = m.mouse {
            for (i, d) in [mo.x, mo.y, mo.wheel, mo.pan].iter().enumerate() {
                w.moved[i] += d;
            }
        }
        for i in 0..INPUTS {
            if self.now.wrapping_sub(w.told_at[i]) < WATCH_AXIS_MS {
                continue;
            }
            let v = if i < RELATIVE { core::mem::take(&mut w.moved[i]) } else { pad.axes[i - RELATIVE] };
            let tell = if i < RELATIVE { v != 0 } else { (v - w.axes[i]).abs() >= WATCH_AXIS_STEP };
            if tell {
                w.axes[i] = v;
                w.told_at[i] = self.now;
                self.tell(w.pc, Event::Axis(i as u8, v.clamp(-32767, 32767) as i16));
            }
        }
        self.watch = Some(w);
        w.swallow
    }

    fn tell(&mut self, pc: Option<Pc>, e: Event) {
        match pc {
            Some(pc) => self.event_to(pc, e),
            None => self.event(e),
        }
    }

    /// What host `h` is offered and took ([`ctrl::outputs`]): a PC everything its interfaces carry while its link is up,
    /// a Bluetooth host the reports it subscribed to.
    fn outputs(&self, h: Host) -> (u8, u8) {
        use ctrl::outputs as o;
        if let Some(n) = h.bt_slot() {
            let Some(hs) = self.bt.hosts.get(n as usize - 1).copied() else { return (0, 0) };
            if !self.bt.present || !hs.bonded() {
                return (0, 0);
            }
            let offered = o::KEYBOARD | o::MOUSE | o::MEDIA | o::POINTER | o::GAMEPAD;
            let mut taken = 0;
            if hs.ready() {
                for (bit, r) in [
                    (o::KEYBOARD, btreports::KEYBOARD),
                    (o::MOUSE, btreports::MOUSE),
                    (o::MEDIA, btreports::CONSUMER),
                    (o::POINTER, btreports::POINTER),
                    (o::GAMEPAD, btreports::GAMEPAD),
                ] {
                    if hs.takes(r) {
                        taken |= bit;
                    }
                }
            }
            return (offered, taken);
        }
        let p = &self.presentation;
        let mut offered = 0;
        for (on, bit) in [
            (p.nkro.ep_in != 0, o::KEYBOARD | o::MEDIA),
            (p.mouse.ep_in != 0, o::MOUSE),
            (p.mouse_abs, o::POINTER),
            (p.gamepad == GamepadOut::Hid, o::GAMEPAD),
            (p.xinput.is_some(), o::XINPUT),
            (p.vial.is_some(), o::VIAL),
            (!p.mirrors.is_empty(), o::MIRRORS),
        ] {
            if on {
                offered |= bit;
            }
        }
        let pc = if h == Host::Pc2 { Pc::Pc2 } else { Pc::Pc1 };
        let up = self.pcs[pc.index()].link == LinkState::Connected;
        (offered, if up { offered } else { 0 })
    }

    /// Whether the input's host takes the virtual gamepad: a PC while it is on (`gamepad`), a Bluetooth host that
    /// subscribed to its report.
    fn takes_gamepad(&self) -> bool {
        match self.bt.active {
            Some(n) => self.bt.hosts.get(n as usize - 1).is_some_and(|h| h.takes(btreports::GAMEPAD)),
            None => self.presentation.gamepad != GamepadOut::Off,
        }
    }

    /// The input's host changed, or what it takes: the keymap that runs for it (a profile's fallback where it can't
    /// take the gamepad), and the keyboard layout macro text is typed through.
    fn host_changed(&mut self) {
        self.vial.remap.set_layout(self.config.keyboard_layout(self.active_host()));
        if self.config.keymap(self.takes_gamepad()) != self.keymap_now {
            self.keymap_switched();
        }
    }

    /// The active profile's keymap changed (another profile, or its keymap): whatever the old one held is let go, and
    /// the controls still held stay hidden until released, as after a switch; Vial now edits the new one.
    fn keymap_switched(&mut self) {
        let steps = self.vial.remap.release_all();
        self.keymap_now = self.config.keymap(self.takes_gamepad());
        self.vial.keymap.set_active(self.keymap_now.unwrap_or(0) as usize);
        let keys = self.norm.keys().without(&self.suppressed_keys);
        self.apply_steps(&steps, &keys);
        self.keymap_buttons = 0;
        let ax = self.axes.reset();
        self.axis_buttons = kvm_map::PosSet::EMPTY;
        self.apply_axes(&ax);
        self.pad.buttons = 0;
        self.pad.dpad = 0;
        self.send_pad();
        self.suppressed_keys = self.norm.keys();
        self.suppressed_buttons = self.norm.buttons();
        self.send_keyboard(KeyState::EMPTY);
        self.send_mouse(MouseInput::default());
        self.send_media(&kvm_map::remap::Out::default());
    }

    /// Re-arms the autosave if the keymap changed (Vial, or a persistent default-layer key).
    fn note_keymap_changes(&mut self) {
        let n = self.vial.keymap.changes();
        if n != self.keymap_changes {
            self.keymap_changes = n;
            self.dirty_since = Some(self.now);
        }
    }

    /// Whether the mirrors take EP1–EP3 IN in place of the KVM's own keyboard, mouse and media interfaces.
    fn wide(&self) -> bool {
        let modes =
            core::array::from_fn(|i| (self.ports[i].state == PortState::Hosted).then_some(self.ports[i].effective));
        let mirror_in = self
            .ports
            .iter()
            .filter(|p| p.is_mirrored())
            .filter_map(|p| p.dev.as_ref())
            .map(|d| d.summary.mirror_cost().0)
            .sum();
        policy::wide(modes, mirror_in, self.wireless_normalized(), self.config.vial == VialMode::On)
    }

    fn rebuild_presentation(&mut self) {
        let mut buf = [0u8; kvm_proto::descset::MAX_LEN];
        if let Some((n, pres)) = self.build_presentation(&mut buf) {
            let changed = self.desc.as_slice() != &buf[..n];
            self.desc.clear();
            let _ = self.desc.extend_from_slice(&buf[..n]);
            if pres.vial.is_none() {
                // Without the interface no app can be talking to it: start locked when it comes back.
                self.vial.lock();
            }
            self.presentation = pres;
            if changed {
                self.emit(Output::DescriptorSetChanged);
            }
            self.host_changed();
        }
    }

    fn build_presentation(&self, buf: &mut [u8]) -> Option<(usize, Presentation)> {
        let configs: [Option<_>; PORTS] = core::array::from_fn(|i| {
            let p = &self.ports[i];
            p.dev.as_ref().filter(|_| p.is_mirrored()).and_then(|d| parse_config(&d.probed.config).map(|c| (d, c)))
        });
        // Vial finds its interface by a magic string in the serial number.
        let mut kvm_serial: String<48> = String::new();
        let mut id_serial: String<96> = String::new();
        let mut mirrors: Vec<MirrorSource, { presentation::MAX_MIRRORS }> = Vec::new();
        let mut identity = None;
        for (i, entry) in configs.iter().enumerate() {
            let Some((d, cfg)) = entry else { continue };
            let port = Port::from_index(i).unwrap_or(Port::Aux2);
            for iface in cfg.interfaces.iter().filter(|f| f.in_ep().is_some()) {
                let rd = d.probed.report_descs.iter().find(|(n, _)| *n == iface.number).map(|(_, r)| r.as_slice());
                let usable = iface.is_xinput() || (iface.is_hid() && rd.is_some());
                if usable {
                    let _ = mirrors.push(MirrorSource { port, iface, report_desc: rd.unwrap_or(&[]) });
                }
            }
            if self.ports[i].effective == PortMode::MirroredIdentity {
                identity = Some(IdentitySource {
                    port,
                    device: d.probed.device,
                    manufacturer: &d.probed.manufacturer,
                    product: &d.probed.product,
                    serial: &d.probed.serial,
                });
            }
        }
        let wide = self.wide();
        let vial = match self.config.vial {
            VialMode::Off => false,
            VialMode::On => true,
            // Mirrors come first; Vial takes an IN endpoint only while one is spare.
            VialMode::Auto => mirrors.len() < Budget::room(wide, false).free_in as usize,
        };
        let with_magic = |out: &mut dyn core::fmt::Write, base: &str| {
            let _ = out.write_str(base);
            if vial {
                let _ = out.write_str(if base.is_empty() { "" } else { " " });
                let _ = out.write_str(kvm_map::SERIAL_MAGIC);
            }
        };
        with_magic(&mut kvm_serial, &self.serial);
        if let Some(id) = identity.as_mut() {
            with_magic(&mut id_serial, id.serial);
            id.serial = &id_serial;
        }
        let params = Params {
            polling: self.config.polling,
            serial: &kvm_serial,
            mirrors: &mirrors,
            identity,
            vial,
            normalized: !wide,
            mouse_abs: self.config.abs_pointer,
            gamepad: self.config.gamepad,
        };
        // If the mirrors don't fit, the PC still gets the KVM's own interfaces.
        presentation::build(&params, buf)
            .or_else(|_| presentation::build(&Params { mirrors: &[], identity: None, normalized: true, ..params }, buf))
            .ok()
    }

    // ── switching ────────────────────────────────────────────────────────────

    /// Switches the input to `target`; screen hop's pointer follows it.
    pub fn switch(&mut self, target: Target, source: SwitchSource) {
        let from = self.active_host();
        self.switch_input(target, source);
        let to = self.active_host();
        if to != from {
            // Screen hop's pointer is found where it was left on that host (a hop put it at the edge already). A host
            // that takes relative motion only gets its cursor parked again on the way in.
            let layout = *self.config.layout();
            self.hop.leave(&layout, from);
            if self.config.bt_pointer(to) == BtPointer::Relative {
                self.hop.forget_cursor(to);
            }
            if source != SwitchSource::Edge {
                self.hop.enter(&layout, to);
            }
        }
    }

    fn switch_input(&mut self, target: Target, source: SwitchSource) {
        let from_bt = self.bt.active;
        let pc = match target {
            Target::Pc1 => Pc::Pc1,
            Target::Pc2 => Pc::Pc2,
            // From a Bluetooth host, back to the home PC.
            Target::Toggle if from_bt.is_some() => self.active,
            Target::Toggle => self.active.other(),
            Target::Next => match self.next_target() {
                Some(n) => return self.switch_bt(n, source),
                None if from_bt.is_some() => Pc::Pc1,
                None => self.active.other(),
            },
            t => match t.bt_host() {
                Some(n) => return self.switch_bt(n, source),
                None => return,
            },
        };
        if from_bt.is_some() {
            self.leave_bt();
            if pc == self.active {
                // Back on the home PC: it was released when the host was selected, and keeps its passthrough ports.
                self.after_switch();
                self.event(Event::BtActive(0));
                self.event(Event::Switched(pc, source));
                return;
            }
        } else if pc == self.active {
            return;
        }
        self.end_learning(LearnEnd::Cancelled, Hotkey::NONE);
        self.end_watch();
        self.monitor = None;
        let old = self.active;
        if from_bt.is_none() {
            // 1. Release everything on the PC we leave.
            self.release_pc(old);
        }
        self.active = pc;
        // 3. Devices pinned to either PC change hands, the keyboards show their PC's lock LEDs.
        self.after_switch();
        // 4. Passthrough ports that follow the active PC move along.
        for &port in Port::ALL {
            let i = port.index();
            let to = self.passthrough_pc(port);
            if self.ports[i].state == PortState::Passthrough && self.ports[i].routed != Some(to) {
                self.ports[i].routed = Some(to);
                let power_cycle = self.config.power_cycle_on_switch;
                self.emit(Output::Route { port, route: Route::Pc(to), power_cycle });
            }
        }
        // 5. Tell both PCs.
        if from_bt.is_some() {
            self.event(Event::BtActive(0));
        }
        self.event(Event::Switched(pc, source));
    }

    /// Releases everything on `pc` (a switch away from it): neutral reports on every interface, and what is held now
    /// stays hidden from the next target until released.
    fn release_pc(&mut self, pc: Pc) {
        let releases = self.pcs[pc.index()].out.release_all(&self.presentation);
        for (ep, r) in releases {
            self.send(pc, ep, r);
        }
        self.release_mirrors(pc);
        self.hold_over();
    }

    /// Step 2 of a switch: keys and buttons still held stay hidden from the new target until released; the keymap lets
    /// go too (layers toggled on stay on), so the new target starts from nothing held.
    fn hold_over(&mut self) {
        self.suppressed_keys = self.norm.keys().union(&self.mirr.keys());
        self.suppressed_buttons = self.norm.buttons() | self.mirr.buttons();
        self.forwarded_keys = KeyState::EMPTY;
        self.forwarded_consumer = ConsumerState::default();
        self.forwarded_system = 0;
        let _ = self.vial.remap.release_all();
        self.keymap_buttons = 0;
        let _ = self.axes.reset();
        self.axis_buttons = kvm_map::PosSet::EMPTY;
        self.pad = normalized::Pad::default();
        self.pad_sent = normalized::Pad::default();
    }

    /// The target after the current one in the order PC1, PC2, Bluetooth hosts 1–4 that are ready: `Some(slot)` for a
    /// host, None for a PC (which one: see [`Engine::switch`]).
    fn next_target(&self) -> Option<u8> {
        let ready = |n: u8| self.bt.selectable(n) && self.bt.host(n).is_some_and(|h| h.ready());
        let after = match self.bt.active {
            Some(n) => n,
            // From PC1 the next is PC2; from PC2 the first ready host.
            None if self.active == Pc::Pc1 => return None,
            None => 0,
        };
        (after + 1..=ctrl::BT_HOSTS as u8).find(|n| ready(*n))
    }

    /// The target before the current one in the order of [`Engine::next_target`]: from PC1 the last host that is ready
    /// (PC2 if none is), from PC2 PC1, from a host the ready one before it or PC2.
    fn prev_target(&self) -> Target {
        let ready = |n: u8| self.bt.selectable(n) && self.bt.host(n).is_some_and(|h| h.ready());
        let before = match self.bt.active {
            Some(n) => n,
            None if self.active == Pc::Pc2 => return Target::Pc1,
            None => ctrl::BT_HOSTS as u8 + 1,
        };
        (1..before).rev().find(|n| ready(*n)).and_then(Target::bt).unwrap_or(Target::Pc2)
    }

    /// Selects Bluetooth host slot `n` (bonded; it gets input once ready).
    fn switch_bt(&mut self, n: u8, source: SwitchSource) {
        if !self.bt.selectable(n) || self.bt.active == Some(n) {
            return;
        }
        let _ = source;
        self.end_learning(LearnEnd::Cancelled, Hotkey::NONE);
        self.end_watch();
        self.monitor = None;
        match self.bt.active {
            Some(_) => self.leave_bt(),
            None => self.release_pc(self.active),
        }
        self.bt.active = Some(n);
        self.bt.sent_keyboard = [0; btreport::KEYBOARD_LEN];
        self.bt.sent_consumer = 0;
        self.bt_mouse_buttons = 0;
        self.after_switch();
        self.event(Event::BtActive(n));
    }

    /// After the active target changed: devices pinned to the host left, or to the one now active, change hands
    /// ([`Engine::regroup`]), the module hears which hosts take input, the keyboards show their targets' LEDs.
    fn after_switch(&mut self) {
        self.regroup();
        self.bt_select();
        self.apply_leds();
        // The new host gets the gamepad as it is.
        self.pad_sent = normalized::Pad::default();
        self.host_changed();
    }

    /// Tells the module which host is the active target and which ones devices are pinned to, if that changed.
    fn bt_select(&mut self) {
        let host = self.bt.active.unwrap_or(0);
        let mut pinned = 0u8;
        for d in Device::ALL {
            if let Routing::Pin(h) = self.config.route(*d)
                && let Some(n) = h.bt_slot()
                && self.running(*d) != PortMode::Off
            {
                pinned |= 1 << (n - 1);
            }
        }
        if self.bt.present && self.bt.selected != Some((host, pinned)) {
            self.bt.selected = Some((host, pinned));
            self.emit(Output::Bt(BtOut::Select { host, pinned }));
        }
    }

    /// Puts every device's input where its routing says ([`Engine::join`]): with the devices that follow the active
    /// target, or with the devices pinned to a host that isn't it.
    fn regroup(&mut self) {
        let active = self.active_host();
        for d in Device::ALL.iter().copied() {
            if self.running(d) == PortMode::Off {
                // Its input goes nowhere now: what it held is let go.
                self.release_dev(d);
                continue;
            }
            let to = Some(self.target_of(d)).filter(|h| *h != active);
            self.join(d, to);
        }
    }

    /// Moves `d`'s input to the devices that follow the active target (None) or to those pinned to `to`. What it holds
    /// is let go where it was, and stays hidden where it goes until released, as across a switch.
    fn join(&mut self, d: Device, to: Option<Host>) {
        let from = self.group[d.index()];
        if from == to {
            return;
        }
        self.group[d.index()] = to;
        let (keys, buttons) = match from {
            None => {
                let (nk, nb) = self.norm.held_by(d);
                let (mk, mb) = self.mirr.held_by(d);
                let m = self.norm.remove(d);
                self.mirr.remove(d);
                self.forward_merged(m, None);
                (nk.union(&mk), nb | mb)
            }
            Some(h) => {
                let p = &mut self.pinned[h.index()];
                let (nk, nb) = p.norm.held_by(d);
                let (mk, mb) = p.mirr.held_by(d);
                let m = p.norm.remove(d);
                p.mirr.remove(d);
                let hidden = self.hidden_keys();
                self.send_pinned(h, m.mouse, hidden);
                (nk.union(&mk), nb | mb)
            }
        };
        match to {
            None => {
                self.suppressed_keys = self.suppressed_keys.union(&keys);
                self.suppressed_buttons |= buttons;
            }
            Some(h) => {
                let p = &mut self.pinned[h.index()];
                p.suppressed_keys = p.suppressed_keys.union(&keys);
                p.suppressed_buttons |= buttons;
            }
        }
    }

    /// Releases everything on the active Bluetooth host and deselects it.
    fn leave_bt(&mut self) {
        let Some(n) = self.bt.active.take() else { return };
        self.bt_input(n, btreport::KEYBOARD, &[0; btreport::KEYBOARD_LEN]);
        self.bt_input(n, btreport::MOUSE, &[0; btreport::MOUSE_LEN]);
        self.bt_mouse_buttons = 0;
        if let Some([x, y]) = self.bt_pointer_at.take() {
            self.bt_input(n, btreport::POINTER, &kvm_proto::link::bt::pointer_report(0, x, y, 0, 0));
        }
        self.bt_input(n, btreport::CONSUMER, &[0; btreport::CONSUMER_LEN]);
        if self.bt.hosts.get(n as usize - 1).is_some_and(|h| h.takes(btreports::GAMEPAD)) {
            self.bt_input(n, btreport::GAMEPAD, &bt::gamepad(&normalized::Pad::default()));
        }
        self.hold_over();
        self.bt_select();
    }

    fn bt_input(&mut self, host: u8, id: u8, data: &[u8]) {
        let data = Vec::from_slice(&data[..data.len().min(btreport::INPUT_MAX)]).unwrap_or_default();
        self.emit(Output::Bt(BtOut::Input { host, id, data }));
    }

    fn release_mirrors(&mut self, pc: Pc) {
        let mut pending: Vec<(u8, Report), 24> = Vec::new();
        for m in &self.presentation.mirrors {
            let Some(d) = self.ports[m.port.index()].dev.as_ref() else { continue };
            match d.translator(m.src_iface) {
                Some(t) => {
                    let layout: &Layout = t.layout();
                    let off = layout.uses_report_ids as usize;
                    for id in layout.report_ids(ReportType::Input) {
                        let mut buf = [0u8; 64];
                        buf[0] = id;
                        if let Some(n) = neutral_report(layout, id, &mut buf[off..]) {
                            let _ = pending.push((m.f.ep_in, report(&buf[..off + n])));
                        }
                    }
                }
                None => {
                    // XInput: message type 0, length, everything else 0 (sticks are signed, centred at 0).
                    let len = d.last_len.iter().find(|(i, _)| *i == m.src_iface).map_or(20, |(_, l)| *l as usize);
                    let mut r = [0u8; 64];
                    r[1] = len as u8;
                    let _ = pending.push((m.f.ep_in, report(&r[..len.clamp(2, 64)])));
                }
            }
        }
        for (ep, r) in pending {
            self.send(pc, ep, r);
        }
    }

    /// A host's keyboard LEDs (its lock state, from its output reports).
    fn leds_of(&self, h: Host) -> u8 {
        match (h.as_pc(), h.bt_slot()) {
            (Some(pc), _) => self.pcs[pc.index()].out.leds,
            (None, Some(n)) => self.bt.leds[n as usize - 1],
            (None, None) => 0,
        }
    }

    /// Sends every keyboard the LEDs of the host its input goes to.
    fn apply_leds(&mut self) {
        self.apply_leds_of(None);
    }

    /// The same for the keyboards whose input goes to `only` (None: every keyboard).
    fn apply_leds_of(&mut self, only: Option<Host>) {
        for d in Device::ALL.iter().copied() {
            if self.running(d) == PortMode::Off {
                continue;
            }
            let target = self.target_of(d);
            if only.is_some_and(|h| h != target) {
                continue;
            }
            let leds = self.leds_of(target);
            let mut targets: Vec<(u8, u8), 4> = Vec::new();
            match (d.port(), d.wireless()) {
                (Some(p), _) => {
                    let Some(dev) = self.ports[p.index()].dev.as_ref() else { continue };
                    for (iface, t) in &dev.translators {
                        if t.verdict().caps & caps::KEYBOARD_LEDS != 0 {
                            let id = t.layout().report_ids(ReportType::Output).next().unwrap_or(0);
                            let _ = targets.push((*iface, id));
                        }
                    }
                }
                (None, Some(_)) => {
                    if let Some(t) = self.translator(d, 0).filter(|t| t.verdict().caps & caps::KEYBOARD_LEDS != 0) {
                        let _ = targets.push((0, t.layout().report_ids(ReportType::Output).next().unwrap_or(0)));
                    }
                }
                (None, None) => {}
            }
            for (iface, id) in targets {
                match (d.port(), d.wireless()) {
                    (Some(port), _) => {
                        let data = if id != 0 { report(&[id, leds]) } else { report(&[leds]) };
                        self.emit(Output::DeviceSetReport { port, iface, report_type: 2, report_id: id, data });
                    }
                    (None, Some(dev)) => {
                        let data = Vec::from_slice(&[leds]).unwrap_or_default();
                        self.emit(Output::Bt(BtOut::DeviceOutput { dev, id, data }));
                    }
                    (None, None) => {}
                }
            }
        }
    }

    // ── device side ──────────────────────────────────────────────────────────

    pub fn port_probing(&mut self, port: Port) {
        let s = &mut self.ports[port.index()];
        s.state = PortState::Probing;
        s.dev = None;
        s.probe_until = None;
        self.event(Event::PortChanged(port));
    }

    /// Plugs `port` into Hub-0 anew, to probe its device again (re-detect, back from passthrough).
    fn reprobe(&mut self, port: Port) {
        let s = &mut self.ports[port.index()];
        s.state = PortState::Probing;
        s.probe_until = Some(self.now.wrapping_add(REPROBE_MS));
        self.emit(Output::Reprobe(port));
    }

    pub fn port_attached(&mut self, port: Port, probed: &Probed) {
        let mut translators = Vec::new();
        for (iface, rd) in &probed.report_descs {
            if let Ok(l) = Layout::parse(rd) {
                let _ = translators.push((*iface, Translator::new(l)));
            }
        }
        let mut xinput = Vec::new();
        let summary = match parse_config(&probed.config) {
            Some(cfg) => {
                for i in &cfg.interfaces {
                    if let Some(k) = i.xinput() {
                        let _ = xinput.push((i.number, k));
                    }
                }
                DeviceSummary::new(&probed.device, &cfg, probed.speed.unwrap_or(Speed::Unknown), |n| {
                    translators.iter().find(|(i, _)| *i == n).map(|(_, t)| t.verdict())
                })
            }
            None => DeviceSummary::default(),
        };
        let profile = crate::pads::profile(probed.device.vid, probed.device.pid, !xinput.is_empty());
        let dongle = self.takes_dongle(port, probed);
        if !dongle && self.adopt == Some(port) {
            // Not an ESP32 board the module can be: nothing to adopt.
            self.adopt = None;
        }
        let s = &mut self.ports[port.index()];
        s.dev = Some(Hosted { probed: probed.clone(), translators, xinput, profile, summary, last_len: Vec::new() });
        s.probe_until = None;
        s.reports = 0;
        s.rate_hz = 0;
        if dongle {
            // The firmware talks to it first (`dongle_wanted`); the port stays probing until it says what it is.
            s.state = PortState::Probing;
            self.dongle = Dongle::Probing(port);
            self.event(Event::PortChanged(port));
            return;
        }
        self.settle(port);
    }

    /// A probed device becomes what its interfaces make it: Hosted (and every port is decided again) or Unsupported.
    fn settle(&mut self, port: Port) {
        let s = &mut self.ports[port.index()];
        if s.dev.as_ref().is_none_or(|d| d.summary.interfaces.is_empty()) {
            s.state = PortState::Unsupported;
            self.event(Event::PortChanged(port));
            return;
        }
        s.state = PortState::Hosted;
        let mut touched = [false; PORTS];
        touched[port.index()] = true;
        self.redecide(touched);
    }

    /// Whether the device just probed on `port` is tried as the Bluetooth module (software.md §4.13): an ESP32-C3's
    /// USB serial port ([`bt::dongle_endpoints`]), on a KVM without the module on board and without another dongle,
    /// on a port not set to passthrough (that keeps an ESP32 board of one's own reachable from a PC as it is).
    /// A board being adopted (`Request::AdoptModule`) is taken whatever its port is set to.
    fn takes_dongle(&self, port: Port, probed: &Probed) -> bool {
        !self.bt_fitted
            && self.dongle == Dongle::None
            && (self.adopt == Some(port) || !self.config.mode(port.into()).is_passthrough())
            && bt::dongle_endpoints(&probed.device, &probed.config).is_some()
    }

    /// The board on `port` is to be made the module whatever it runs (`Request::AdoptModule`): the firmware programs it
    /// rather than leave it alone.
    pub fn dongle_adopt(&self, port: Port) -> bool {
        self.adopt == Some(port)
    }

    /// The KVM has the module on its board (what the firmware read at boot): ESP32 boards on the front ports are then
    /// ordinary devices.
    pub fn set_bt_fitted(&mut self, fitted: bool) {
        self.bt_fitted = fitted;
    }

    /// The firmware is to find out whether the device on `port` is the Bluetooth module (`port_attached` took it).
    pub fn dongle_wanted(&self, port: Port) -> bool {
        self.dongle == Dongle::Probing(port)
    }

    /// The device on `port` is the module: it answered as the module, or it was blank and is being programmed.
    pub fn dongle_claimed(&mut self, port: Port) {
        if self.dongle == Dongle::Probing(port) {
            self.dongle = Dongle::Claimed(port);
            self.ports[port.index()].state = PortState::Module;
            self.event(Event::PortChanged(port));
        }
        if self.adopt == Some(port) {
            self.adopt = None;
        }
    }

    /// The device on `port` is not the module (another firmware, another chip): an ordinary device from now on.
    pub fn dongle_declined(&mut self, port: Port) {
        if self.dongle == Dongle::Probing(port) {
            self.dongle = Dongle::None;
            if self.adopt == Some(port) {
                // Not even adopted (its ROM never answered): it stays an ordinary device.
                self.adopt = None;
            }
            self.settle(port);
        }
    }

    /// The front port with the module on it, if there is one.
    pub fn dongle_port(&self) -> Option<Port> {
        match self.dongle {
            Dongle::Claimed(p) => Some(p),
            _ => None,
        }
    }

    /// The dongle's port was just set to passthrough: plug it in anew, so that it attaches as the ordinary device the
    /// setting asks for (`takes_dongle` leaves it alone then).
    fn release_dongle_for_passthrough(&mut self) {
        if let Dongle::Probing(p) | Dongle::Claimed(p) = self.dongle
            && self.config.mode(p.into()).is_passthrough()
        {
            self.reprobe(p);
        }
    }

    /// Decides every port's mode again (attach, unplug, a mode or Vial change). The ports compete for the mirror
    /// endpoints and identity, so a change on one can move another; those already mirrored keep their endpoints, and
    /// the one with identity keeps it. Ports `touched` are reported as changed even if their mode stays.
    fn redecide(&mut self, touched: [bool; PORTS]) {
        let rank = |p: &PortSlot| match (p.is_mirrored(), p.effective) {
            (true, PortMode::MirroredIdentity) => 0,
            (true, _) => 1,
            _ => 2,
        };
        let mut order: Vec<Port, PORTS> = Port::ALL.iter().copied().collect();
        order.sort_unstable_by_key(|p| (rank(&self.ports[p.index()]), p.index()));
        let settings = self.config.modes();
        let wireless = self.wireless_normalized();
        let decided = {
            let candidates: Vec<Candidate, PORTS> = order
                .iter()
                .filter_map(|&port| {
                    let s = &self.ports[port.index()];
                    let d = s.dev.as_ref().filter(|_| matches!(s.state, PortState::Hosted | PortState::Passthrough))?;
                    Some(Candidate { port, setting: settings[port.index()], dev: &d.summary })
                })
                .collect();
            policy::decide_all(&candidates, wireless, self.config.vial == VialMode::On)
        };
        let mut changed = [false; PORTS];
        for &port in Port::ALL {
            let i = port.index();
            let Some((eff, reason)) = decided[i] else { continue };
            let s = &self.ports[i];
            let was = (s.state, s.effective, s.reason, s.routed);
            self.ports[i].effective = eff;
            self.ports[i].reason = reason;
            if eff.is_passthrough() {
                let target = self.passthrough_pc(port);
                self.ports[i].state = PortState::Passthrough;
                self.release_port(port);
                if self.ports[i].routed != Some(target) {
                    self.ports[i].routed = Some(target);
                    self.emit(Output::Route { port, route: Route::Pc(target), power_cycle: false });
                }
            } else if was.0 == PortState::Passthrough {
                // Coming back from passthrough: the KVM has to see the device again first.
                self.ports[i].routed = None;
                self.reprobe(port);
            } else if eff == PortMode::Off {
                self.release_port(port);
            }
            let s = &self.ports[i];
            changed[i] = was != (s.state, eff, reason, s.routed);
        }
        // Always, against what the PCs have: a port coming back from passthrough is marked mirrored while it is still
        // probing, so on attach its mode looked unchanged, the set kept no mirror for it, and its reports went
        // nowhere (first board, 2026-11-12). The PCs only hear of a set whose bytes changed.
        self.rebuild_presentation();
        // Modes changed, so may where the devices' input goes.
        self.after_switch();
        for &port in Port::ALL {
            let i = port.index();
            if changed[i] || touched[i] {
                self.event(Event::PortChanged(port));
                // A wireless receiver the KVM now reads: asked whether a pad is linked (it says so, and the pad gets
                // its LED, `device_report`).
                let s = &self.ports[i];
                if s.state == PortState::Hosted
                    && s.effective == PortMode::Normalized
                    && let Some(iface) = s.dev.as_ref().and_then(Hosted::receiver_pad)
                {
                    let data = report(&kvm_hid::xinput::INQUIRY);
                    self.emit(Output::DeviceOut { port, iface, data });
                }
            }
        }
    }

    /// The PC a port in passthrough is routed to: the one it is pinned to, else the active one (the home PC while a
    /// Bluetooth host is active: a device in passthrough can't go to one).
    fn passthrough_pc(&self, port: Port) -> Pc {
        match self.config.route(port.into()) {
            Routing::Pin(Host::Pc1) => Pc::Pc1,
            Routing::Pin(Host::Pc2) => Pc::Pc2,
            _ => self.active,
        }
    }

    fn release_port(&mut self, port: Port) {
        self.release_dev(port.into());
    }

    /// Lets go of everything `d` holds, wherever its input went.
    fn release_dev(&mut self, d: Device) {
        if self.norm.has(d) || self.mirr.has(d) {
            let m = self.norm.remove(d);
            self.mirr.remove(d);
            self.forward_merged(m, None);
        }
        for h in Host::ALL.iter().copied() {
            let p = &mut self.pinned[h.index()];
            if p.norm.has(d) || p.mirr.has(d) {
                let m = p.norm.remove(d);
                p.mirr.remove(d);
                let hidden = self.hidden_keys();
                self.send_pinned(h, m.mouse, hidden);
            }
        }
        self.group[d.index()] = None;
    }

    pub fn port_detached(&mut self, port: Port) {
        let i = port.index();
        if matches!(self.dongle, Dongle::Probing(p) | Dongle::Claimed(p) if p == port) {
            let claimed = self.dongle == Dongle::Claimed(port);
            self.dongle = Dongle::None;
            if claimed {
                self.bt_link_down();
                self.set_bt_module(BtModule::Absent);
            }
            self.ports[i] = PortSlot { faults: self.ports[i].faults, ..PortSlot::new() };
            self.event(Event::PortChanged(port));
            return;
        }
        if self.ports[i].state == PortState::Passthrough {
            // We routed it away ourselves; Hub-0 just saw it leave.
            return;
        }
        let was_mirrored = self.ports[i].is_mirrored();
        if was_mirrored {
            let pc = self.active;
            self.release_mirrors(pc);
        }
        self.release_port(port);
        self.ports[i] = PortSlot { faults: self.ports[i].faults, ..PortSlot::new() };
        // Endpoints freed up, or Normalized lost a user: the other ports may now get the mode they asked for.
        self.redecide([false; PORTS]);
        if was_mirrored {
            self.rebuild_presentation();
        }
        self.event(Event::PortChanged(port));
    }

    pub fn port_fault(&mut self, port: Port) {
        let i = port.index();
        self.ports[i].faults = self.ports[i].faults.saturating_add(1);
        if self.ports[i].state != PortState::Passthrough {
            self.release_port(port);
        }
        self.ports[i].state = PortState::Fault;
        self.event(Event::Fault(FaultKind::OverCurrent, Some(port)));
    }

    /// A raw input report from interface `iface` of the device on `port`.
    pub fn device_report(&mut self, port: Port, iface: u8, data: &[u8], now: u32) {
        self.now = now;
        self.place_waiting();
        let i = port.index();
        if self.ports[i].state != PortState::Hosted {
            return;
        }
        self.ports[i].reports += 1;
        let Some(d) = self.ports[i].dev.as_mut() else { return };
        match d.last_len.iter_mut().find(|(n, _)| *n == iface) {
            Some(e) => e.1 = data.len() as u8,
            None => {
                let _ = d.last_len.push((iface, data.len() as u8));
            }
        }
        use kvm_hid::xinput::{self, Kind, Wireless};
        let pad = |g| kvm_hid::Translation { gamepad: g, ..Default::default() };
        let t = match d.xinput(iface) {
            Some(Kind::Wired) => pad(xinput::wired(data)),
            Some(Kind::Wireless) => match xinput::wireless(data) {
                Wireless::Input(g) => pad(Some(g)),
                // A pad that links gets its player LED; one that leaves lets go of everything.
                Wireless::Link(true) => {
                    let slot = d.xinput.iter().filter(|(_, k)| *k == Kind::Wireless).position(|(n, _)| *n == iface);
                    let data = report(&xinput::led(slot.unwrap_or(0) as u8));
                    self.emit(Output::DeviceOut { port, iface, data });
                    return;
                }
                Wireless::Link(false) => {
                    let rest = kvm_hid::GamepadInput { present: xinput::present(), ..Default::default() };
                    pad(Some(rest))
                }
                Wireless::Other => return,
            },
            None => d.translator(iface).map(|t| t.translate(data)).unwrap_or_default(),
        };
        self.input(port.into(), iface, &t, data, now);
    }

    /// An input report of Bluetooth device slot `dev`, as the module passes it on (without its report ID, `id`).
    pub fn bt_device_input(&mut self, dev: u8, id: u8, data: &[u8], now: u32) {
        self.now = now;
        self.place_waiting();
        let Some(d) = Device::wireless_slot(dev) else { return };
        let w = &mut self.wireless.slots[dev as usize - 1];
        let Some(tr) = w.translator.as_ref().filter(|_| w.state.ready()) else { return };
        // As a USB device sends it: its report ID first when the map has them.
        let mut raw = [0u8; 1 + kvm_proto::link::bt::REPORT_MAX];
        let at = tr.layout().uses_report_ids as usize;
        raw[0] = id;
        let n = data.len().min(raw.len() - at);
        raw[at..at + n].copy_from_slice(&data[..n]);
        let t = tr.translate(&raw[..at + n]);
        w.reports += 1;
        self.input(d, 0, &t, &raw[..at + n], now);
    }

    /// A device's report, translated: to the devices that follow the active target, or to the host it is pinned to.
    fn input(&mut self, d: Device, iface: u8, t: &kvm_hid::translate::Translation, data: &[u8], now: u32) {
        self.monitor_report(d, iface, data);
        let mode = self.running(d);
        if mode == PortMode::Off {
            return;
        }
        let normalized = mode == PortMode::Normalized;
        let target = self.target_of(d);
        if target != self.active_host() {
            return self.pinned_input(d, target, iface, t, data, normalized, now);
        }
        self.join(d, None);
        // The device's profile (its gamepad in the standard order), its field overrides, then a gamepad's axes through
        // its calibration, before they merge with anything.
        let mut t = *t;
        if let Some(dev) = d.port().and_then(|p| self.ports[p.index()].dev.as_ref()) {
            let ids = dev.translator(iface).is_some_and(|tr| tr.layout().uses_report_ids);
            dev.profile.apply(data, ids, &mut t.gamepad);
        } else if let Some(w) = d.wireless().and_then(|n| self.wireless.slots.get(n as usize - 1)) {
            let ids = w.translator.as_ref().is_some_and(|tr| tr.layout().uses_report_ids);
            w.profile().apply(data, ids, &mut t.gamepad);
        }
        if let Some(id) = self.identity(d) {
            let ids = self.translator(d, iface).is_some_and(|tr| tr.layout().uses_report_ids);
            self.cal.apply_overrides(id, iface, data, ids, &mut t.gamepad);
            if let Some(g) = t.gamepad.as_mut() {
                self.cal.apply(id, g);
            }
        }
        let t = &t;
        let merged = if normalized { self.norm.apply(d, iface, t) } else { self.mirr.apply(d, iface, t) };

        // A passkey for a pairing Bluetooth host is being typed: the keyboard goes to nobody else meanwhile.
        if self.bt.passkey.is_some() && merged.keyboard.is_some() {
            self.passkey_keys();
            return;
        }
        let swallowed = if merged.keyboard.is_some() { self.run_hotkeys(now) } else { self.hidden_keys() };
        // Held-over keys and buttons stop being hidden once released.
        self.suppressed_keys = self.suppressed_keys.intersect(&self.norm.keys().union(&self.mirr.keys()));
        self.suppressed_buttons &= self.norm.buttons() | self.mirr.buttons();

        // A Bluetooth host gets every hosted device as the KVM's own keyboard and mouse, mirrored ones too (it is never
        // offered a device's own descriptor), whatever the PCs do.
        if self.bt.active.is_some() {
            if normalized {
                self.forward_merged(merged, Some(swallowed));
            } else {
                self.forward_mirrored_bt(merged, swallowed);
            }
            return;
        }
        if let Presence::Asleep { wake } = self.standby.presence(self.active) {
            self.input_while_asleep(wake, now);
            return;
        }
        if normalized {
            self.forward_merged(merged, Some(swallowed));
        } else if let Some(m) = d.port().and_then(|p| self.presentation.mirror_of(p, iface)).copied() {
            let mask = swallowed.union(&self.suppressed_keys);
            let pc = self.active;
            self.send_mirrored(d, pc, m.f.ep_in, iface, data, &mask, self.suppressed_buttons);
        }
    }

    /// A mirrored device's raw report for `pc`'s mirror endpoint `ep`, without the keys and buttons in `mask` and
    /// `buttons` (a hotkey's, those held over).
    #[allow(clippy::too_many_arguments)]
    fn send_mirrored(&mut self, d: Device, pc: Pc, ep: u8, iface: u8, data: &[u8], mask: &KeyState, buttons: u16) {
        let mut raw = report(data);
        if let Some(t) = self.translator(d, iface) {
            t.strip(&mut raw, mask, buttons);
        }
        self.send(pc, ep, raw);
    }

    /// Every key held on every keyboard, wherever its input goes: the hotkeys, learning one and a passkey being typed
    /// take them all.
    fn all_keys(&self) -> KeyState {
        self.pinned
            .iter()
            .fold(self.norm.keys().union(&self.mirr.keys()), |k, p| k.union(&p.norm.keys()).union(&p.mirr.keys()))
    }

    /// A report of `d`, pinned to `h`, which isn't the active host: it goes there as it is, without the keymap or
    /// screen hop. A device mirrored to a PC goes to that PC's mirror of it; any other goes as the KVM's own keyboard
    /// and mouse.
    #[allow(clippy::too_many_arguments)]
    fn pinned_input(
        &mut self,
        d: Device,
        h: Host,
        iface: u8,
        t: &kvm_hid::translate::Translation,
        data: &[u8],
        normalized: bool,
        now: u32,
    ) {
        self.join(d, Some(h));
        let mirrored = !normalized && h.as_pc().is_some();
        let p = &mut self.pinned[h.index()];
        let merged = if mirrored { p.mirr.apply(d, iface, t) } else { p.norm.apply(d, iface, t) };
        if merged.keyboard.is_some() {
            if self.bt.passkey.is_some() {
                self.passkey_keys();
                return;
            }
            let _ = self.run_hotkeys(now);
            if self.group[d.index()] != Some(h) {
                // A hotkey made its host the active one: the report is gone with what it held.
                return;
            }
        }
        let hidden = self.hidden_keys();
        let p = &mut self.pinned[h.index()];
        p.suppressed_keys = p.suppressed_keys.intersect(&p.norm.keys().union(&p.mirr.keys()));
        p.suppressed_buttons &= p.norm.buttons() | p.mirr.buttons();
        if let Some(pc) = h.as_pc() {
            if let Presence::Asleep { wake } = self.standby.presence(pc) {
                self.pinned_asleep(h, pc, wake, now);
                return;
            }
            if mirrored {
                if let Some(m) = d.port().and_then(|port| self.presentation.mirror_of(port, iface)).copied() {
                    let p = &self.pinned[h.index()];
                    let (mask, buttons) = (hidden.union(&p.suppressed_keys), p.suppressed_buttons);
                    self.send_mirrored(d, pc, m.f.ep_in, iface, data, &mask, buttons);
                }
                return;
            }
        }
        self.send_pinned(h, merged.mouse, hidden);
    }

    /// The state of the devices pinned to `h` (not the active host) as the KVM's own reports: keyboard and media keys
    /// if they changed, and a mouse report with `mouse`'s motion. `hidden`: the hotkeys' keys.
    fn send_pinned(&mut self, h: Host, mouse: Option<MouseInput>, hidden: KeyState) {
        let p = &self.pinned[h.index()];
        let keys = p.norm.keys().without(&hidden).without(&p.suppressed_keys);
        let buttons = p.norm.buttons() & !p.suppressed_buttons;
        let (consumer, system) = (p.norm.consumer(), p.norm.system());
        let mouse = mouse.or((buttons != p.buttons).then(MouseInput::default));
        match (h.as_pc(), h.bt_slot()) {
            (Some(pc), _) => {
                let out = self.pcs[pc.index()].out;
                let p = &self.pinned[h.index()];
                let (send_keys, send_consumer, send_system) =
                    (keys != p.keys, consumer != p.consumer, system != p.system);
                let p = &mut self.pinned[h.index()];
                (p.keys, p.consumer, p.system, p.buttons) = (keys, consumer, system, buttons);
                if send_keys {
                    let (ep, r) = out.keyboard(&keys, &self.presentation);
                    self.send(pc, ep, r);
                }
                if send_consumer {
                    let (ep, r) = out.consumer(&consumer, &self.presentation);
                    self.send(pc, ep, r);
                }
                if send_system {
                    let (ep, r) = out.system(system, &self.presentation);
                    self.send(pc, ep, r);
                }
                if let Some(m) = mouse {
                    self.send_pointer_report(pc, hop::Report::Rel([m.x, m.y]), buttons, m.wheel, m.pan);
                }
            }
            (None, Some(n)) => {
                let kb = bt::keyboard(&keys);
                let usage = bt::consumer(&consumer);
                let p = &mut self.pinned[h.index()];
                let (send_kb, send_usage) = (kb != p.bt_keyboard, usage != p.bt_consumer);
                (p.bt_keyboard, p.bt_consumer, p.buttons) = (kb, usage, buttons);
                if send_kb {
                    self.bt_input(n, btreport::KEYBOARD, &kb);
                }
                if send_usage {
                    self.bt_input(n, btreport::CONSUMER, &usage.to_le_bytes());
                }
                if let Some(m) = mouse {
                    let r = kvm_proto::link::bt::mouse_report(buttons, m.x, m.y, m.wheel, m.pan);
                    self.bt_input(n, btreport::MOUSE, &r);
                }
            }
            (None, None) => {}
        }
    }

    /// Pinned input while `pc` sleeps: nothing reaches it; a newly pressed key or button wakes it if it allowed that,
    /// and what is held then stays hidden from it until released (as `input_while_asleep` does for the active PC).
    fn pinned_asleep(&mut self, h: Host, pc: Pc, wake: bool, now: u32) {
        let p = &self.pinned[h.index()];
        let keys = p.norm.keys().union(&p.mirr.keys());
        let buttons = p.norm.buttons() | p.mirr.buttons();
        let pressed = !keys.without(&p.suppressed_keys).is_empty() || buttons & !p.suppressed_buttons != 0;
        if pressed && wake && self.standby.may_wake(now, pc) {
            self.emit(Output::RemoteWakeup(pc));
        }
        let p = &mut self.pinned[h.index()];
        p.suppressed_keys = keys;
        p.suppressed_buttons = buttons;
    }

    /// A mirrored device's input for the active Bluetooth host: its keys with the normalized ones (no keymap: it applies
    /// to normalized keyboards only, as on the PCs), its buttons and motion, its media keys.
    fn forward_mirrored_bt(&mut self, m: crate::merge::Merged, swallowed: KeyState) {
        let Some(n) = self.bt.active else { return };
        let mask = swallowed.union(&self.suppressed_keys);
        let keys = self.mirr.keys().union(&self.forwarded_keys).without(&mask);
        let kb = bt::keyboard(&keys);
        if kb != self.bt.sent_keyboard {
            self.bt.sent_keyboard = kb;
            self.bt_input(n, btreport::KEYBOARD, &kb);
        }
        if let Some(mouse) = m.mouse {
            let buttons = (self.mirr.buttons() | self.norm.buttons()) & !self.suppressed_buttons;
            let r = kvm_proto::link::bt::mouse_report(buttons, mouse.x, mouse.y, mouse.wheel, mouse.pan);
            self.bt_input(n, btreport::MOUSE, &r);
        }
        if m.consumer.is_some() {
            let mut c = self.mirr.consumer();
            if c.usages.iter().all(|u| *u == 0) {
                c = self.norm.consumer();
            }
            self.send_bt_consumer(n, bt::consumer(&c));
        }
    }

    fn send_bt_consumer(&mut self, n: u8, usage: u16) {
        if usage != self.bt.sent_consumer {
            self.bt.sent_consumer = usage;
            self.bt_input(n, btreport::CONSUMER, &usage.to_le_bytes());
        }
    }

    /// The keyboard while a passkey is typed: digits, Backspace, Enter, Escape; nothing reaches any target. What is
    /// held when it ends stays hidden until released.
    fn passkey_keys(&mut self) {
        let keys = self.all_keys();
        let Some(entry) = self.bt.passkey.as_mut() else { return };
        let host = entry.host;
        let typed = (entry.digits, entry.value);
        let value = match entry.feed(&keys) {
            PasskeyStep::Typing => {
                // Hosts show the digits as they are typed (`Status.bt_passkey_digits`): tell them of each one.
                if self.bt.passkey.is_some_and(|e| (e.digits, e.value) != typed) {
                    self.event(Event::BtChanged(0));
                }
                return;
            }
            PasskeyStep::Done(v) => Some(v),
            PasskeyStep::Cancel => None,
        };
        self.bt.passkey = None;
        self.suppressed_keys = keys;
        for p in self.pinned.iter_mut() {
            p.suppressed_keys = p.norm.keys().union(&p.mirr.keys());
        }
        self.emit(Output::Bt(BtOut::Passkey { host, value }));
        self.event(Event::BtChanged(0));
    }

    /// Input while the active PC sleeps: nothing reaches it. A newly pressed key or button (not motion) wakes it if it
    /// allowed that, and what is held then stays hidden from it until released, as after a switch.
    fn input_while_asleep(&mut self, wake: bool, now: u32) {
        let keys = self.norm.keys().union(&self.mirr.keys());
        let buttons = self.norm.buttons() | self.mirr.buttons();
        let media = self.norm.consumer().usages.iter().any(|u| *u != 0) || self.norm.system() != 0;
        let pressed = !keys.without(&self.suppressed_keys).is_empty() || buttons & !self.suppressed_buttons != 0;
        if (pressed || media) && wake && self.standby.may_wake(now, self.active) {
            self.emit(Output::RemoteWakeup(self.active));
        }
        self.suppressed_keys = keys;
        self.suppressed_buttons = buttons;
    }

    /// Runs the hotkey engine on the combined keyboard state, or the learner while a hotkey is being learned; returns
    /// the keys it is hiding.
    fn run_hotkeys(&mut self, now: u32) -> KeyState {
        let union = self.all_keys();
        if let Some(l) = self.learning.as_mut() {
            let learned = l.learner.process(&union, now);
            let hidden = l.learner.withheld();
            if let Some(h) = learned {
                self.end_learning(LearnEnd::Learned, h);
            }
            return hidden;
        }
        let hk = self.hotkeys.process(&union, now);
        if let Some(k) = hk.replay {
            self.replay_tap(k);
        }
        let swallowed = union.without(&hk.keys);
        if let Some(a) = hk.action {
            self.hotkey_action(a);
        }
        swallowed
    }

    /// The keys the hotkey engine, or the learner, hides from the PCs.
    fn hidden_keys(&self) -> KeyState {
        match &self.learning {
            Some(l) => l.learner.withheld(),
            None => self.hotkeys.swallowed(),
        }
    }

    /// Learning a hotkey ended: tells the PC that asked. Keys pressed meanwhile stay hidden until released.
    fn end_learning(&mut self, end: LearnEnd, hotkey: Hotkey) {
        let Some(l) = self.learning.take() else { return };
        let held = self.all_keys();
        self.suppressed_keys = self.suppressed_keys.union(&l.learner.withheld());
        self.hotkeys.resume(&held);
        let e = Event::HotkeyLearned(end, hotkey);
        match l.pc {
            Some(pc) => self.event_to(pc, e),
            None => self.event(e),
        }
    }

    fn replay_tap(&mut self, k: u8) {
        if let Some(n) = self.bt.active {
            let mut down = self.forwarded_keys;
            down.press(k);
            self.bt_input(n, btreport::KEYBOARD, &bt::keyboard(&down));
            let up = bt::keyboard(&self.forwarded_keys);
            self.bt.sent_keyboard = up;
            self.bt_input(n, btreport::KEYBOARD, &up);
            return;
        }
        let pc = self.active;
        let out = self.pcs[pc.index()].out;
        let mut down = self.forwarded_keys;
        down.press(k);
        let (ep, r) = out.keyboard(&down, &self.presentation);
        self.send(pc, ep, r);
        let (ep, r) = out.keyboard(&self.forwarded_keys, &self.presentation);
        self.send(pc, ep, r);
    }

    fn forward_merged(&mut self, m: crate::merge::Merged, swallowed: Option<KeyState>) {
        let union = self.norm.keys().union(&self.mirr.keys());
        self.suppressed_keys = self.suppressed_keys.intersect(&union);
        let swallowed = swallowed.unwrap_or(KeyState::EMPTY);
        let keys = self.norm.keys().without(&swallowed).without(&self.suppressed_keys);
        if let Some(mouse) = m.mouse {
            self.suppressed_buttons &= mouse.buttons | self.mirr.buttons();
        }
        // The host software learning a control: told, and (when it asked) kept from the hosts.
        if self.watch_controls(&keys, &m) {
            return;
        }
        match self.keymap_now {
            Some(k) => self.map_input(k as usize, &keys, &m),
            None => {
                self.send_keyboard(keys);
                if let Some(mouse) = m.mouse {
                    self.send_mouse(mouse);
                }
                if m.gamepad.is_some() {
                    self.pad_direct();
                }
            }
        }
        if m.consumer.is_some() || m.system.is_some() {
            let extra =
                if self.keymap_now.is_some() { *self.vial.remap.output() } else { kvm_map::remap::Out::default() };
            self.send_media(&extra);
        }
    }

    /// Through the active profile's keymap `k`: the axis rules first (their directions are keymap positions), then
    /// everything held down through the keymap engine.
    fn map_input(&mut self, k: usize, keys: &KeyState, m: &crate::merge::Merged) {
        let layers = self.vial.remap.layers(&self.vial.keymap);
        let rel = m.mouse.map_or([0; kvm_map::axes::RELATIVE], |mo| [mo.x, mo.y, mo.wheel, mo.pan]);
        let abs = m.gamepad.map(|g| g.axes);
        let ax = self.axes.input(self.vial.keymap.axes(), k, layers, rel, abs.as_ref(), self.now);
        self.apply_axes(&ax);
        self.axis_buttons = ax.buttons;
        let pos = self.positions(keys);
        let steps = self.vial.remap.input(&mut self.vial.keymap, &pos, self.now);
        self.apply_steps(&steps, keys);
        self.note_keymap_changes();
    }

    /// Every control of the normalized devices held down now, as keymap positions (`kvm_map::pos`).
    fn positions(&self, keys: &KeyState) -> kvm_map::PosSet {
        use kvm_map::pos;
        let mut p = kvm_map::PosSet::from_keys(keys);
        p.add_mask(pos::MOUSE, (self.norm.buttons() & !self.suppressed_buttons) as u32, pos::MOUSE_BUTTONS);
        let pad = self.norm.gamepad();
        p.add_mask(pos::PAD, pad.buttons, pos::PAD_BUTTONS);
        p.add_mask(pos::HAT, pad.dpad() as u32, 4);
        p.union(&self.axis_buttons)
    }

    /// What the axis rules make: mouse motion now, the virtual gamepad's axes.
    fn apply_axes(&mut self, ax: &kvm_map::axes::Out) {
        if ax.mouse != [0; 4] {
            let [x, y, wheel, pan] = ax.mouse;
            self.send_pointer(MouseInput { buttons: 0, x, y, wheel, pan }, true);
        }
        if let Some(a) = ax.pad {
            self.pad.axes = a;
            self.send_pad();
        }
    }

    /// Without a keymap: the gamepads' input 1:1 to the virtual gamepad (the brake and accelerator onto Z and Rz, its
    /// triggers, where the gamepad has none of its own there).
    fn pad_direct(&mut self) {
        use kvm_hid::gamepad::axis;
        let g = self.norm.gamepad();
        let mut axes = [0i16; 8];
        for (i, a) in axes.iter_mut().enumerate() {
            *a = g.axes[i].clamp(-32767, 32767) as i16;
        }
        for (from, to) in [(axis::BRAKE, axis::Z), (axis::ACCELERATOR, axis::RZ)] {
            if g.present & (1 << from) != 0 && g.present & (1 << to) == 0 {
                axes[to] = g.axes[from].clamp(0, 32767) as i16;
            }
        }
        self.pad = normalized::Pad { buttons: g.buttons, dpad: g.dpad(), axes };
        self.send_pad();
    }

    /// The virtual gamepad's state to the active PC, if it changed and the PC has one (a Bluetooth host doesn't yet).
    fn send_pad(&mut self) {
        if self.pad == self.pad_sent {
            return;
        }
        if let Some(n) = self.bt.active {
            // A Bluetooth host that took the gamepad's report gets it; one that didn't has the profile's fallback.
            if self.takes_gamepad() {
                self.pad_sent = self.pad;
                self.bt_input(n, btreport::GAMEPAD, &bt::gamepad(&self.pad));
            }
            return;
        }
        let pc = self.active;
        if let Some((ep, r)) = self.pcs[pc.index()].out.gamepad(&self.pad, &self.presentation) {
            self.pad_sent = self.pad;
            self.send(pc, ep, r);
        }
    }

    /// Sends the keymap engine's steps to the active PC; `keys` are the physical keys it was given.
    fn apply_steps(&mut self, steps: &[kvm_map::Step], keys: &KeyState) {
        let mut kvm: Vec<u8, 4> = Vec::new();
        // Usages beyond the matrix (0xE8–0xFF, never sent in practice) pass through as they are.
        let mut beyond = KeyState::EMPTY;
        for k in keys.keys().filter(|k| *k as usize >= kvm_map::KEYBOARD) {
            beyond.press(k);
        }
        for step in steps {
            match step {
                kvm_map::Step::State(o) => {
                    self.send_keyboard(o.keys.union(&beyond));
                    self.send_media(o);
                    if o.buttons != self.keymap_buttons {
                        self.keymap_buttons = o.buttons;
                        self.send_mouse(MouseInput::default());
                    }
                    if (o.pad, o.dpad) != (self.pad.buttons, self.pad.dpad) {
                        (self.pad.buttons, self.pad.dpad) = (o.pad, o.dpad);
                        self.send_pad();
                    }
                }
                // Mouse keys accelerate themselves.
                kvm_map::Step::Motion(mo) => self.send_pointer(
                    MouseInput {
                        buttons: 0,
                        x: mo.x as i32,
                        y: mo.y as i32,
                        wheel: mo.wheel as i32,
                        pan: mo.pan as i32,
                    },
                    false,
                ),
                kvm_map::Step::Kvm(n) => {
                    let _ = kvm.push(*n);
                }
            }
        }
        let last = self.vial.remap.output().keys.union(&beyond);
        self.send_keyboard(last);
        // KVM actions after what came before them reached the host (a switch lets go of everything there).
        for n in kvm {
            if let Some(a) = HotkeyAction::ALL.get(n as usize).copied().filter(|a| *a != HotkeyAction::None) {
                self.hotkey_action(a);
            }
        }
    }

    fn send_keyboard(&mut self, keys: KeyState) {
        if let Some(n) = self.bt.active {
            self.forwarded_keys = keys;
            // Mirrored keyboards' keys go along (forward_mirrored_bt): one keyboard report for the host.
            let all = keys.union(&self.mirr.keys().without(&self.suppressed_keys));
            let kb = bt::keyboard(&all);
            if kb != self.bt.sent_keyboard {
                self.bt.sent_keyboard = kb;
                self.bt_input(n, btreport::KEYBOARD, &kb);
            }
            return;
        }
        if keys != self.forwarded_keys {
            self.forwarded_keys = keys;
            let pc = self.active;
            let (ep, r) = self.pcs[pc.index()].out.keyboard(&keys, &self.presentation);
            self.send(pc, ep, r);
        }
    }

    /// Media and system keys: the physical ones plus the keymap's.
    fn send_media(&mut self, extra: &kvm_map::remap::Out) {
        let pc = self.active;
        let out = self.pcs[pc.index()].out;
        let mut c = self.norm.consumer();
        for u in extra.consumer.iter().filter(|u| **u != 0) {
            if !c.usages.contains(u)
                && let Some(slot) = c.usages.iter_mut().find(|s| **s == 0)
            {
                *slot = *u;
            }
        }
        if let Some(n) = self.bt.active {
            // A Bluetooth host gets the media keys (one at a time); system control (power, sleep) stays with the PCs.
            self.forwarded_consumer = c;
            self.send_bt_consumer(n, bt::consumer(&c));
            return;
        }
        if c != self.forwarded_consumer {
            self.forwarded_consumer = c;
            let (ep, r) = out.consumer(&c, &self.presentation);
            self.send(pc, ep, r);
        }
        let system = self.norm.system() | extra.system;
        if system != self.forwarded_system {
            self.forwarded_system = system;
            let (ep, r) = out.system(system, &self.presentation);
            self.send(pc, ep, r);
        }
    }

    /// A mouse report with `m`'s motion (a mouse's: screen hop accelerates it) and the buttons held now.
    fn send_mouse(&mut self, m: MouseInput) {
        self.send_pointer(m, true);
    }

    /// A mouse report with `m`'s motion and the buttons held now: the physical ones (minus those held over from the
    /// other PC) and the keymap's. With screen hop the KVM moves the pointer itself (`accelerate`: as a mouse moves it).
    fn send_pointer(&mut self, m: MouseInput, accelerate: bool) {
        let pc = self.active;
        // With a keymap the buttons are what it makes of them.
        let physical = if self.keymap_now.is_some() { 0 } else { self.norm.buttons() & !self.suppressed_buttons };
        let mut buttons = physical | self.keymap_buttons;
        if self.bt.active.is_some() {
            // A Bluetooth host's mouse also carries a mirrored device's buttons.
            buttons |= self.mirr.buttons() & !self.suppressed_buttons;
        }
        if self.hop_state() == HopState::Active {
            self.hop_motion(MouseInput { buttons, ..m }, accelerate);
            return;
        }
        if let Some(n) = self.bt.active {
            let r = kvm_proto::link::bt::mouse_report(buttons, m.x, m.y, m.wheel, m.pan);
            self.bt_input(n, btreport::MOUSE, &r);
            return;
        }
        self.send_pointer_report(pc, hop::Report::Rel([m.x, m.y]), buttons, m.wheel, m.pan);
    }

    /// Screen hop's report for `host`: a PC's through its pointer interfaces, a Bluetooth host's as its absolute
    /// pointer or its mouse.
    fn send_host_pointer(&mut self, host: Host, motion: hop::Report, buttons: u16, wheel: i32, pan: i32) {
        let n = match (host.as_pc(), host.bt_slot()) {
            (Some(pc), _) => {
                if let hop::Report::Place(at) = motion {
                    self.place_cursor(pc, at);
                }
                return self.send_pointer_report(pc, motion, buttons, wheel, pan);
            }
            (None, Some(n)) => n,
            (None, None) => return,
        };
        // Absolute on the main screen only: buttons go on the mouse, as on a PC (`PcOutput::pointer`), so a window
        // dragged onto another screen is let go there.
        let buttons_rel = self.config.mapping(host) == ctrl::Mapping::MainScreen;
        match motion {
            hop::Report::Abs([x, y]) => {
                self.bt_pointer_at = Some([x, y]);
                let r = kvm_proto::link::bt::pointer_report(if buttons_rel { 0 } else { buttons }, x, y, wheel, pan);
                self.bt_input(n, btreport::POINTER, &r);
                if buttons_rel && buttons != self.bt_mouse_buttons {
                    self.bt_mouse_buttons = buttons;
                    self.bt_input(n, btreport::MOUSE, &kvm_proto::link::bt::mouse_report(buttons, 0, 0, 0, 0));
                }
            }
            hop::Report::Rel(mut d) => {
                // The mouse report carries ±32767 each way: a longer move (a park) takes several.
                let (mut wheel, mut pan) = (wheel, pan);
                loop {
                    let step = d.map(|v| v.clamp(-32767, 32767));
                    let r = kvm_proto::link::bt::mouse_report(buttons, step[0], step[1], wheel, pan);
                    self.bt_input(n, btreport::MOUSE, &r);
                    self.bt_mouse_buttons = buttons;
                    d = [d[0] - step[0], d[1] - step[1]];
                    (wheel, pan) = (0, 0);
                    if d == [0, 0] {
                        break;
                    }
                }
            }
            // A Bluetooth host has no host software to place its cursor.
            hop::Report::Place(_) => {}
        }
    }

    /// Tells `pc`'s host software where to put its cursor (`Event::PlaceCursor`): at once, unless one went this
    /// millisecond already (the control interface takes a report a millisecond, a mouse may send eight), then the
    /// latest with the next.
    fn place_cursor(&mut self, pc: Pc, at: [i32; 2]) {
        let at = at.map(|v| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
        let k = pc.index();
        if self.place_sent[k] == Some(self.now) {
            self.place_pending[k] = Some(at);
        } else {
            self.place_pending[k] = None;
            self.place_sent[k] = Some(self.now);
            self.event_to(pc, Event::PlaceCursor(at));
        }
    }

    /// A place that waited for the next millisecond ([`Engine::place_cursor`]).
    fn place_waiting(&mut self) {
        for pc in [Pc::Pc1, Pc::Pc2] {
            let k = pc.index();
            if self.place_sent[k] != Some(self.now)
                && let Some(at) = self.place_pending[k].take()
            {
                self.place_sent[k] = Some(self.now);
                self.event_to(pc, Event::PlaceCursor(at));
            }
        }
    }

    /// The screens `pc`'s host software places the cursor on now: none once its lease ran out.
    fn placement(&self, pc: Pc) -> ctrl::Placement {
        match self.placer[pc.index()] {
            Some((p, until)) if until.wrapping_sub(self.now) as i32 > 0 => p,
            _ => ctrl::Placement::Off,
        }
    }

    fn send_pointer_report(&mut self, pc: Pc, motion: hop::Report, buttons: u16, wheel: i32, pan: i32) {
        // Absolute on the main screen only (Windows): the relative pointer presses, the absolute one only places.
        let buttons_rel = self.config.mapping(Host::pc(pc)) == ctrl::Mapping::MainScreen;
        let reports = self.pcs[pc.index()].out.pointer(motion, buttons, wheel, pan, buttons_rel, &self.presentation);
        for (ep, r) in reports {
            self.send(pc, ep, r);
        }
    }

    /// How `host` takes the pointer: its mapping, its desktop, and for a Bluetooth host whether it is absolute.
    fn hop_map(&self, host: Host) -> hop::Map {
        hop::Map {
            mapping: self.config.mapping(host),
            units: self.config.units(host),
            relative: self.config.bt_pointer(host) == BtPointer::Relative,
            placed: host.as_pc().map_or(ctrl::Placement::Off, |pc| self.placement(pc)),
        }
    }

    /// Whether the pointer may hop onto `host`: a PC connected, awake and past its firmware setup; a Bluetooth host
    /// bonded, connected, encrypted and subscribed.
    fn hop_open(&self, host: Host) -> bool {
        match (host.as_pc(), host.bt_slot()) {
            (Some(pc), _) => {
                let s = &self.pcs[pc.index()];
                s.link == LinkState::Connected && !s.out.boot_mouse
            }
            (None, Some(n)) => self.bt.selectable(n) && self.bt.host(n).is_some_and(|h| h.ready()),
            (None, None) => false,
        }
    }

    /// Screen hop (software.md §4.14): the motion moves the KVM's pointer across the active profile's arrangement; the
    /// active host gets where it is, and a push across an edge onto another host's screen switches to that host.
    fn hop_motion(&mut self, m: MouseInput, accelerate: bool) {
        let host = self.active_host();
        let h = self.config.hop;
        let d = if accelerate {
            self.hop.accel.apply(m.x, m.y, self.now, h.accel, h.speed)
        } else {
            [m.x as f32, m.y as f32]
        };
        let host_at = |i: usize| Host::from_index(i).unwrap_or(Host::Pc1);
        let rules = hop::Rules {
            resistance: h.resistance,
            corner: h.corner,
            buttons: m.buttons != 0,
            open: core::array::from_fn(|i| self.hop_open(host_at(i))),
            maps: core::array::from_fn(|i| self.hop_map(host_at(i))),
            // A host that accelerates relative motion itself as `hop-accel` does gets the mouse's counts there.
            raw: (accelerate && h.accel != ctrl::Accel::Flat).then_some(hop::Raw {
                counts: [m.x, m.y],
                accel: h.accel,
                speed: h.speed,
            }),
        };
        let layout = *self.config.layout();
        let step = self.hop.motion(&layout, host, d, self.now, &rules);
        let reports =
            if step.reports.is_empty() { self.hop.here(&layout, host, self.hop_map(host)) } else { step.reports };
        // The wheel goes with the first report.
        let mut scroll = (m.wheel, m.pan);
        for r in reports {
            self.send_host_pointer(host, r, m.buttons, scroll.0, scroll.1);
            scroll = (0, 0);
        }
        if let Some(to) = step.hop {
            self.switch(to.target(), SwitchSource::Edge);
            if self.active_host() == to {
                for r in self.hop.here(&layout, to, self.hop_map(to)) {
                    self.send_host_pointer(to, r, 0, 0, 0);
                }
            }
        }
    }

    /// A Vial request (SET_REPORT on the Vial interface); the reply goes back on its IN endpoint.
    fn vial_request(&mut self, pc: Pc, data: &[u8]) {
        let Some(v) = self.presentation.vial else { return };
        let physical = self.norm.keys().union(&self.mirr.keys());
        let ctx = kvm_map::Context {
            pc: pc.index(),
            active: pc == self.active,
            writable: self.config.sw_config_allowed,
            physical: &physical,
            now: self.now,
        };
        let reply = self.vial.request(&ctx, data);
        self.note_keymap_changes();
        self.send(pc, v.ep_in, report(&reply));
    }

    /// The real device answered a forwarded control request (`None` = it stalled).
    pub fn device_ctrl_reply(&mut self, pc: Pc, tag: u8, port: Port, setup: [u8; 8], data: Option<&[u8]>) {
        let is_get_report = setup[0] == 0xA1 && setup[1] == 0x01;
        if let (true, Some(d)) = (is_get_report, data) {
            let value = u16::from_le_bytes([setup[2], setup[3]]);
            let iface = setup[4];
            self.cache.retain(|c| !(c.port == port && c.iface == iface && c.value == value));
            if self.cache.is_full() {
                self.cache.remove(0);
            }
            let _ = self.cache.push(CacheEntry {
                port,
                iface,
                value,
                data: CtrlData::from_slice(&d[..d.len().min(MAX_CTRL_DATA)]).unwrap_or_default(),
            });
        }
        let data = data.and_then(|d| CtrlData::from_slice(&d[..d.len().min(MAX_CTRL_DATA)]).ok());
        self.emit(Output::CtrlReply { pc, tag, stall: data.is_none(), data: data.unwrap_or_default() });
    }

    // ── PC side ──────────────────────────────────────────────────────────────

    pub fn pc_hello(&mut self, pc: Pc, fw: u32) {
        self.pcs[pc.index()].fw = fw;
        if self.pcs[pc.index()].link == LinkState::LinkDown {
            self.pcs[pc.index()].link = LinkState::Detached;
        }
    }

    pub fn pc_link_down(&mut self, pc: Pc) {
        let s = &mut self.pcs[pc.index()];
        if s.link != LinkState::LinkDown {
            s.link = LinkState::LinkDown;
            s.status = None;
            self.event(Event::Fault(FaultKind::LinkDown, None));
        }
        self.update_power();
    }

    pub fn pc_status(&mut self, pc: Pc, st: AtStatus) {
        let s = &mut self.pcs[pc.index()];
        let link = usb_to_link(crate::standby::host_usb_state(&st));
        if matches!(st.usb_state, UsbState::Default | UsbState::Unpowered | UsbState::Detached) {
            // A bus reset or unplug: the PC starts over in report protocol without multipliers, its cursor anywhere,
            // and its host software asks again to place it.
            s.out.reset();
            self.hop.forget_cursor(Host::pc(pc));
            self.placer[pc.index()] = None;
            self.place_pending[pc.index()] = None;
        }
        s.link = link;
        s.status = Some(st);
        self.supply.pc(self.now, pc.index(), st.vbus_mv, st.vbus_min_mv);
        self.judge_supply();
        self.update_power();
    }

    /// Auto-select and the standby plan (software.md §4.12), after anything that may change them.
    fn update_power(&mut self) {
        let presence = [0, 1].map(|k| Presence::of(self.pcs[k].link, self.pcs[k].status.as_ref()));
        self.standby.observe(self.now, presence);
        // Not while a Bluetooth host has the input: the PCs being switched off is no reason to take it away.
        if self.config.auto_select
            && self.bt.active.is_none()
            && let Some(pc) = self.standby.auto_target(self.now, self.active)
        {
            self.switch(if pc == Pc::Pc1 { Target::Pc1 } else { Target::Pc2 }, SwitchSource::Power);
        }
        // A Bluetooth host in use, or pairing, keeps the KVM awake: its keyboard and mouse are on the front ports.
        let busy = self.now < self.identify_until
            || !matches!(self.fw.0, FwState::Idle | FwState::Done | FwState::Failed)
            || self.selftest_showing()
            || self.bt.active.is_some()
            || self.bt.pairing_until.is_some();
        let before = self.standby.plan();
        let plan = self.standby.update(self.now, self.active, self.config.standby_wake, busy, self.wake_ports());
        if plan != before {
            self.emit(Output::Power(plan));
        }
    }

    /// Front ports worth powering while a sleeping PC waits for a key press: those with a keyboard or mouse, and
    /// empty ones (a keyboard plugged in then can wake it too), of those the board has.
    fn wake_ports(&self) -> [bool; PORTS] {
        core::array::from_fn(|i| {
            Port::from_index(i).is_some_and(|p| self.has_port(p))
                && self.ports[i]
                    .dev
                    .as_ref()
                    .is_none_or(|d| d.summary.classes() & (ctrl::class::KEYBOARD | ctrl::class::MOUSE) != 0)
        })
    }

    /// SET_REPORT on a locally handled interface.
    pub fn pc_set_report(&mut self, pc: Pc, iface: u8, report_type: u8, report_id: u8, data: &[u8]) {
        let role = self.presentation.role_of_iface(iface);
        let s = &mut self.pcs[pc.index()];
        match (role, report_type) {
            (Some(Role::BootKeyboard | Role::Nkro), 2) => {
                let leds = data.get(if report_id != 0 && data.len() > 1 { 1 } else { 0 }).copied().unwrap_or(0);
                if s.out.leds != leds {
                    s.out.leds = leds;
                    self.apply_leds_of(Some(Host::pc(pc)));
                }
            }
            (Some(Role::Mouse), 3) => {
                // With the absolute pointer the interface has report IDs, and the data starts with the ID.
                let at = if report_id != 0 && data.len() > 1 { 1 } else { 0 };
                let (w, p) = kvm_hid::normalized::mouse_multipliers(data.get(at).copied().unwrap_or(0));
                s.out.wheel_hires = w;
                s.out.pan_hires = p;
            }
            (Some(Role::Vial), 2) => self.vial_request(pc, data),
            _ => {}
        }
    }

    pub fn pc_protocol(&mut self, pc: Pc, iface: u8, protocol: u8) {
        let boot = protocol == 0;
        match self.presentation.role_of_iface(iface) {
            Some(Role::BootKeyboard) => self.pcs[pc.index()].out.boot_keyboard = boot,
            Some(Role::Mouse) => self.pcs[pc.index()].out.boot_mouse = boot,
            _ => {}
        }
    }

    /// Data from one of `pc`'s OUT endpoints (control interface or a mirror).
    pub fn pc_out_report(&mut self, pc: Pc, ep: u8, data: &[u8], now: u32) {
        self.now = now;
        if ep == self.presentation.control.ep_out {
            if let Some(resp) = self.control(Asker::Pc(pc), data) {
                let ep_in = self.presentation.control.ep_in;
                self.send(pc, ep_in, report(&resp));
            }
        } else if let Some(m) = self.presentation.mirror_by_out_ep(ep).copied() {
            // Rumble, LEDs and vendor commands reach the device only from the PC its input goes to.
            if pc == self.mirror_pc(m.port) {
                self.emit(Output::DeviceOut { port: m.port, iface: m.src_iface, data: report(data) });
            }
        }
    }

    /// The PC a mirrored device's own requests come from: the one it is pinned to, else the active (home) PC.
    fn mirror_pc(&self, port: Port) -> Pc {
        self.passthrough_pc(port)
    }

    /// A control request the AT32F405 forwarded (class/vendor request on a mirrored interface).
    pub fn pc_ctrl_request(&mut self, pc: Pc, tag: u8, mut setup: [u8; 8], data: &[u8]) {
        let recipient_iface = setup[0] & 0x1F == 1;
        let mirror = if recipient_iface {
            self.presentation.role_of_iface(setup[4]).and_then(|r| match r {
                Role::Mirror(m) => Some(m),
                _ => None,
            })
        } else {
            // Device-recipient vendor requests go to the identity device (or the only mirrored one).
            let id = self.presentation.identity;
            self.presentation.mirrors.iter().find(|m| id.is_none_or(|p| p == m.port)).copied()
        };
        let Some(m) = mirror else {
            self.emit(Output::CtrlReply { pc, tag, stall: true, data: CtrlData::new() });
            return;
        };
        if recipient_iface {
            setup[4] = m.src_iface;
        }
        let device_to_host = setup[0] & 0x80 != 0;
        if pc == self.mirror_pc(m.port) {
            let data = CtrlData::from_slice(&data[..data.len().min(MAX_CTRL_DATA)]).unwrap_or_default();
            self.emit(Output::DeviceControl { pc, tag, port: m.port, setup, data });
        } else if !device_to_host {
            // Inactive PC: acknowledge and keep; its keyboard LEDs are applied when it becomes active.
            if setup[0] == 0x21 && setup[1] == 0x09 && setup[3] == 2 {
                self.pcs[pc.index()].out.leds = data.last().copied().unwrap_or(0);
            }
            self.emit(Output::CtrlReply { pc, tag, stall: false, data: CtrlData::new() });
        } else {
            let value = u16::from_le_bytes([setup[2], setup[3]]);
            let cached = self.cache.iter().find(|c| c.port == m.port && c.iface == m.src_iface && c.value == value);
            let reply = match cached {
                Some(c) => Output::CtrlReply { pc, tag, stall: false, data: c.data.clone() },
                None => Output::CtrlReply { pc, tag, stall: true, data: CtrlData::new() },
            };
            self.emit(reply);
        }
    }

    // ── device UI, power, time ───────────────────────────────────────────────

    /// The KVM's own key (debounced by the firmware): a tap and a long press do what the config says (by default the
    /// tap toggles the PC and the long press selects the next profile).
    pub fn key(&mut self, ev: KeyEvent) {
        // In standby the first press only wakes the KVM (LEDs, front ports); it neither switches nor changes a mode.
        let was = self.standby.plan().mode;
        self.standby.keep_awake(self.now);
        if was != PowerMode::Active {
            self.update_power();
            return;
        }
        // A tap that confirms something waiting for the key does only that.
        let confirms = ev == KeyEvent::Tap && (self.fw.0 == FwState::AwaitingKey || self.unlock.is_some());
        match ev {
            KeyEvent::Tap if confirms => {
                if self.fw.0 == FwState::AwaitingKey {
                    self.fw = (FwState::Writing, self.fw.1);
                    self.emit(Output::Firmware(FwOp::Confirm));
                }
                if let Some(u) = self.unlock.take() {
                    self.allow_software(u.switching, u.config);
                    if let Some(n) = u.host {
                        self.allow_host(n);
                    }
                }
            }
            KeyEvent::Tap => self.key_action(self.config.key_tap),
            KeyEvent::Long => self.key_action(self.config.key_long),
        }
    }

    /// Turns switching and/or configuration from software back on: only at the device (the key confirming a request,
    /// or held while the KVM powers up).
    pub fn allow_software(&mut self, switching: bool, config: bool) {
        if switching {
            let _ = self.set_config(ConfigValue::SwSwitchAllowed(true), Origin::Device);
        }
        if config {
            let _ = self.set_config(ConfigValue::SwConfigAllowed(true), Origin::Device);
        }
    }

    /// Lets Bluetooth host slot `n` control the KVM: only at the device (the key confirming its request), and only
    /// while it holds a bond.
    fn allow_host(&mut self, n: u8) {
        let Some(i) = n.checked_sub(1).filter(|i| (*i as usize) < ctrl::BT_HOSTS) else { return };
        if self.bt.hosts[i as usize].bonded() {
            let _ = self.set_config(ConfigValue::BtControl(i, true), Origin::Device);
        }
    }

    /// Software asks to turn a lock off (`v` false: done now) or back on (armed until a tap on the key, or it times out).
    fn request_lock(&mut self, v: ConfigValue) -> Result<Response, ErrorCode> {
        if let ConfigValue::BtControl(i, on) = v {
            return self.request_host(i, on);
        }
        let (switching, on) = match v {
            ConfigValue::SwSwitchAllowed(on) => (true, on),
            ConfigValue::SwConfigAllowed(on) => (false, on),
            _ => return Err(ErrorCode::InvalidValue),
        };
        let now_on = if switching { self.config.sw_switch_allowed } else { self.config.sw_config_allowed };
        if on {
            if now_on {
                return Ok(Response::Config(v));
            }
            // A Bluetooth host's request waits for the key: one tap, one thing.
            if self.unlock.is_some_and(|u| u.host.is_some()) {
                return Err(ErrorCode::Busy);
            }
            let u = self.unlock.get_or_insert(Unlock { switching: false, config: false, host: None, until: 0 });
            if switching {
                u.switching = true;
            } else {
                u.config = true;
            }
            u.until = self.now.wrapping_add(UNLOCK_WAIT_MS);
            return Err(ErrorCode::NeedsKeyPress);
        }
        // Turned off before the key confirmed turning it on: that request is withdrawn.
        if let Some(u) = self.unlock.as_mut() {
            if switching {
                u.switching = false;
            } else {
                u.config = false;
            }
        }
        self.unlock = self.unlock.filter(|u| u.switching || u.config || u.host.is_some());
        Ok(Response::Config(self.set_config(v, Origin::Software)?))
    }

    /// Bluetooth host slot index `i` (0–3) is to control the KVM (`on`: armed until a tap on the key, as the locks) or
    /// not any more (at once, a configuration change like any other).
    fn request_host(&mut self, i: u8, on: bool) -> Result<Response, ErrorCode> {
        let Some(&now_on) = self.config.bt_control.get(i as usize) else { return Err(ErrorCode::InvalidValue) };
        let n = i + 1;
        if on {
            if now_on {
                return Ok(Response::Config(ConfigValue::BtControl(i, true)));
            }
            if !self.bt.present {
                return Err(ErrorCode::Unsupported);
            }
            if !self.bt.hosts[i as usize].bonded() {
                return Err(ErrorCode::NoDevice);
            }
            // The tap grants exactly what the one asking is told: not the locks a PC waits for, nor another host.
            if self.unlock.is_some_and(|u| u.switching || u.config || u.host.is_some_and(|h| h != n)) {
                return Err(ErrorCode::Busy);
            }
            let until = self.now.wrapping_add(UNLOCK_WAIT_MS);
            self.unlock = Some(Unlock { switching: false, config: false, host: Some(n), until });
            return Err(ErrorCode::NeedsKeyPress);
        }
        if !self.config.sw_config_allowed {
            return Err(ErrorCode::Locked);
        }
        if self.unlock.is_some_and(|u| u.host == Some(n)) {
            self.unlock = None;
        }
        Ok(Response::Config(self.set_config(ConfigValue::BtControl(i, false), Origin::Software)?))
    }

    fn key_action(&mut self, a: KeyAction) {
        match a {
            KeyAction::None => {}
            KeyAction::TogglePc => self.switch(Target::Toggle, SwitchSource::Key),
            KeyAction::NextTarget => self.switch(Target::Next, SwitchSource::Key),
            KeyAction::NextProfile => self.select_profile(self.config.next_profile()),
            KeyAction::Aux2PortMode => self.cycle_aux2_port(),
        }
    }

    /// What a hotkey of the list does (software.md §4.5). Like the KVM's key, it works whatever the software locks.
    fn hotkey_action(&mut self, a: HotkeyAction) {
        let count = self.config.profile_count;
        match a {
            HotkeyAction::None => {}
            HotkeyAction::TogglePc => self.switch(Target::Toggle, SwitchSource::Hotkey),
            HotkeyAction::NextHost => self.switch(Target::Next, SwitchSource::Hotkey),
            HotkeyAction::PrevHost => {
                let t = self.prev_target();
                self.switch(t, SwitchSource::Hotkey);
            }
            // A host slot with no host bonded leaves the input where it is.
            HotkeyAction::SelectHost(h) => self.switch(h.target(), SwitchSource::Hotkey),
            HotkeyAction::NextProfile => self.select_profile(self.config.next_profile()),
            HotkeyAction::PrevProfile => self.select_profile((self.config.profile + count - 1) % count),
            // A profile past those in use is refused, as from the software.
            HotkeyAction::SelectProfile(n) => self.select_profile(n),
            HotkeyAction::Aux2PortMode => self.cycle_aux2_port(),
        }
    }

    fn select_profile(&mut self, n: u8) {
        let _ = self.set_config(ConfigValue::Profile(n), Origin::Device);
    }

    /// The Aux 2 port's mode and routing: Auto → passthrough → passthrough pinned to PC1 → to PC2 → Auto, in one
    /// step (the router flips once).
    fn cycle_aux2_port(&mut self) {
        let d = Device::Aux2;
        let (mode, route) = match (self.config.mode(d), self.config.route(d)) {
            (PortMode::Auto, _) => (PortMode::Passthrough, Routing::Follow),
            (PortMode::Passthrough, Routing::Follow) => (PortMode::Passthrough, Routing::Pin(Host::Pc1)),
            (PortMode::Passthrough, Routing::Pin(Host::Pc1)) => (PortMode::Passthrough, Routing::Pin(Host::Pc2)),
            _ => (PortMode::Auto, Routing::Follow),
        };
        let before = self.config;
        let changed = [ConfigValue::Mode(d, mode), ConfigValue::Route(d, route)];
        for v in changed {
            let _ = self.config.set(v, Origin::Device);
        }
        if self.config != before {
            self.config_effects(&before, &changed);
        }
    }

    pub fn set_power(&mut self, p: PowerInfo) {
        self.power = p;
        self.judge_supply();
    }

    /// The 5 V the KVM runs from, judged again after a reading (software.md §4.7).
    fn judge_supply(&mut self) {
        let pwr = (self.power.source == PowerSource::PwrPort).then_some(self.power.vbus_pwr_mv);
        self.supply.judge(self.now, pwr);
    }

    /// The self-test's findings (`ctrl::selftest` bits); without `RUNNING` the result shows on the LEDs for a while.
    pub fn set_selftest(&mut self, bits: u32) {
        if self.selftest & ctrl::selftest::RUNNING != 0 && bits & ctrl::selftest::RUNNING == 0 {
            self.selftest_done_at = Some(self.now);
        }
        self.selftest = bits;
    }

    pub fn selftest(&self) -> u32 {
        self.selftest
    }

    /// What the firmware knows of the Bluetooth module (fitted, its link, its programming), for `Status` and the
    /// diagnostics. Hosts hear of a change at once (`BtChanged(0)`): the window shows a module being programmed.
    pub fn set_bt_module(&mut self, m: BtModule) {
        if self.bt_module != m {
            self.bt_module = m;
            self.bt_progress = 0;
            self.event(Event::BtChanged(0));
        }
    }

    /// The module's chip, from its `Hello` or its ROM (`Status.bt_chip`: the host software sends that chip's image).
    pub fn set_bt_chip(&mut self, chip: kvm_proto::link::bt::Chip) {
        if self.bt_chip != chip {
            self.bt_chip = chip;
            self.event(Event::BtChanged(0));
        }
    }

    /// How far programming the module has got, in percent (`Status.bt_progress`; the window polls it).
    pub fn set_bt_progress(&mut self, percent: u8) {
        self.bt_progress = percent.min(100);
    }

    /// Frames lost on each link since boot (saturating), for `Status`.
    pub fn set_link_errors(&mut self, errors: [u16; 2]) {
        self.link_errors = errors;
    }

    /// The firmware's measurements for the diagnostics (`Request::GetDiag`).
    pub fn set_board_diag(&mut self, d: BoardDiag) {
        self.board = Some(d);
    }

    /// Every diagnostics item, graded (software.md §8.4). Readings that aren't there yet are left out.
    pub fn diag_items(&self) -> Vec<Item, { cdiag::MAX_ITEMS }> {
        let mut v: Vec<Item, { cdiag::MAX_ITEMS }> = Vec::new();
        let mut add = |id: u8, grade: Grade, value: i32| {
            let _ = v.push(Item::new(id, grade, value));
        };
        let status = self.status(None);
        let b = self.board;

        add(id::UPTIME, Grade::Info, (self.now / 1000) as i32);
        if let Some(b) = &b {
            add(id::RESET_CAUSE, diag::reset(b.reset), b.reset as i32);
            if let Some(t) = b.temp_dc {
                add(id::RP_TEMP, diag::temp(t, 70, 85), t as i32);
            }
        }
        add(id::SELFTEST, diag::selftest_bits(self.selftest), self.selftest as i32);
        let dropped = self.dropped_outputs.min(i32::MAX as u32) as i32;
        add(id::DROPPED, if dropped == 0 { Grade::Ok } else { Grade::Warn }, dropped);
        if let Some(b) = &b {
            add(id::FW_UPDATES, diag::fw_updates(b.fw_updates), b.fw_updates);
        }

        if let Some(b) = &b {
            add(id::XOSC, if b.xosc_hz != 0 { Grade::Ok } else { Grade::Bad }, b.xosc_hz as i32);
            // 1 % for the system clock; the frequency counter resolves 2 kHz, 42 ppm at 48 MHz.
            let sys = |hz: u32| diag::freq(hz, b.clk_sys_set_hz, 10_000);
            let usb = |hz: u32| diag::freq(hz, diag::USB_HZ, 2_500);
            add(id::PLL_SYS, sys(b.pll_sys_hz), b.pll_sys_hz as i32);
            add(id::PLL_USB, usb(b.pll_usb_hz), b.pll_usb_hz as i32);
            let e12 = diag::sys_vs_usb(b.clk_sys_hz, b.clk_usb_hz);
            add(id::CLK_SYS, sys(b.clk_sys_hz).worse(e12), b.clk_sys_hz as i32);
            add(id::CLK_USB, usb(b.clk_usb_hz), b.clk_usb_hz as i32);
            add(id::CLK_PERI, if b.clk_peri_hz != 0 { Grade::Ok } else { Grade::Bad }, b.clk_peri_hz as i32);
            add(id::CLK_ADC, diag::freq(b.clk_adc_hz, diag::USB_HZ, 10_000), b.clk_adc_hz as i32);
            add(id::ROSC, Grade::Info, b.rosc_hz as i32);
        }
        for k in 0..2 {
            let c = if self.selftest & [ctrl::selftest::AT32_XTAL1, ctrl::selftest::AT32_XTAL2][k] != 0 {
                ClockSource::Internal
            } else if self.pcs[k].fw != 0 {
                ClockSource::Crystal
            } else {
                ClockSource::Unknown
            };
            add(id::AT_CLOCK[k], diag::clock_source(c), c as i32);
        }

        if let Some(b) = &b {
            add(id::HUB0_SPEED, diag::hub0_speed(b.hub0_speed, b.hub0_held), b.hub0_speed as i32);
            let up = match (b.hub0_up, b.hub0_held) {
                (true, _) => Grade::Ok,
                (false, true) => Grade::Info,
                (false, false) => Grade::Bad,
            };
            add(id::HUB0_UP, up, b.hub0_up as i32);
            add(id::PHY_SWAP, Grade::Info, b.phy_swap as i32 | (b.phy_swap_found as i32) << 1);
            add(id::HUB0_ERRORS, Grade::Info, b.hub0_errors.min(i32::MAX as u32) as i32);
            add(id::ENUM_FAILS, Grade::Info, b.enum_fails.min(i32::MAX as u32) as i32);
        }
        for p in Port::ALL.iter().filter(|p| self.has_port(**p)) {
            add(id::PORT_SPEED[p.index()], Grade::Info, self.port_info(*p).speed as i32);
        }
        for k in 0..2 {
            let link = self.pcs[k].link;
            // Only a PC that reset the AT32F405 has set a speed: before, its core reports full speed.
            let speed = match self.pcs[k].status.filter(|s| s.resets > 0).map(|s| s.speed) {
                Some(UsbSpeed::High) => Speed::High,
                Some(UsbSpeed::Full) => Speed::Full,
                _ => Speed::Unknown,
            };
            add(id::PC_SPEED[k], diag::pc_speed(speed, link), speed as i32);
            add(id::PC_LINK[k], diag::link(link), link as i32);
            if let Some(s) = self.pcs[k].status {
                add(id::PC_RESETS[k], Grade::Info, s.resets as i32);
            }
            // Counted since boot, including while an AT32F405 is being programmed: a number, not a judgement.
            add(id::LINK_ERRORS[k], Grade::Info, self.link_errors[k] as i32);
        }

        let pwr = self.power.vbus_pwr_mv;
        add(id::PWR_VBUS, diag::vbus(pwr), pwr as i32);
        if let Some(b) = &b {
            add(id::PWR_CC, diag::cc(b.pwr_cc_mv, pwr), b.pwr_cc_mv as i32);
        }
        for k in 0..2 {
            let Some(s) = self.pcs[k].status else { continue };
            add(id::PC_VBUS[k], diag::vbus(s.vbus_mv), s.vbus_mv as i32);
            let cc = s.cc1_mv.max(s.cc2_mv);
            add(id::PC_CC[k], diag::cc(cc, s.vbus_mv), cc as i32);
            if let Some(mv) = s.vdda_mv {
                add(id::AT_3V3[k], diag::rail_3v3(mv), mv as i32);
            }
        }
        // The estimate is information; the measured supply is the judgement.
        add(id::BUDGET, Grade::Info, status.budget_ma as i32);
        let supply = match self.supply.level() {
            ctrl::Supply::Unknown => Grade::Info,
            ctrl::Supply::Good => Grade::Ok,
            ctrl::Supply::Sags => Grade::Warn,
            ctrl::Supply::Low => Grade::Bad,
        };
        add(id::SUPPLY, supply, status.supply_lowest_mv as i32);
        add(id::SUPPLY_DIPS, Grade::Info, self.supply.dips() as i32);
        for k in 0..2 {
            add(id::PC_DROPOUTS[k], Grade::Info, self.supply.dropouts(k) as i32);
        }
        for p in Port::ALL.iter().filter(|p| self.has_port(**p)) {
            let faults = self.port_info(*p).faults;
            let grade = if self.selftest & ctrl::selftest::PORTS[p.index()] != 0 {
                Grade::Bad
            } else if faults != 0 {
                Grade::Warn
            } else {
                Grade::Ok
            };
            add(id::PORT_FAULTS[p.index()], grade, faults as i32);
        }

        for k in 0..2 {
            if let Some(t) = self.pcs[k].status.and_then(|s| s.temp_dc) {
                add(id::AT_TEMP[k], diag::temp(t, 85, 105), t as i32);
            }
        }
        for k in 0..2 {
            let fw = self.pcs[k].fw;
            let grade = match fw {
                0 => Grade::Info,
                v if v == self.info.rp_fw => Grade::Ok,
                // Older (being reprogrammed, or programming gave up) or newer than the RP2354B's.
                _ => Grade::Warn,
            };
            add(id::AT_FW[k], grade, fw as i32);
        }

        // The Bluetooth module, only where one is fitted: a board without it shows no Bluetooth anywhere.
        if self.bt_module != BtModule::Absent {
            add(id::BT_MODULE, diag::bt_module(self.bt_module), self.bt_module as i32);
            // Up means the module runs the RP2354B's copy; the version is for reading, not for judging.
            add(id::BT_FW, Grade::Info, self.bt.fw as i32);
            let errors = b.map_or(0, |b| b.bt_link_errors.min(i32::MAX as u32));
            add(id::BT_LINK_ERRORS, Grade::Info, errors as i32);
            let ready = self.bt.mask(|h| h.ready()).count_ones();
            add(id::BT_HOSTS, Grade::Info, ready as i32);
            if self.bt.settings_held != 0 {
                add(id::BT_SETTINGS_HELD, Grade::Warn, self.bt.settings_held.count_ones() as i32);
            }
        }
        // A board on a front port as the module, once one was seen: what its sessions did (bring-up).
        if let Some(m) = b.map(|b| b.board).filter(|m| m.sessions > 0) {
            let n = |v: u32| v.min(i32::MAX as u32) as i32;
            add(id::BOARD_SESSIONS, Grade::Info, n(m.sessions));
            add(id::BOARD_STEPS, Grade::Info, m.steps as i32);
            add(id::BOARD_ROM_ERROR, Grade::Info, m.rom_error as i32);
            add(id::BOARD_BULK_IN, Grade::Info, n(m.bulk_in));
            add(id::BOARD_BULK_OUT, Grade::Info, n(m.bulk_out));
            add(id::BOARD_NAKS, Grade::Info, n(m.naks));
            add(id::BOARD_OUT_NAKS, Grade::Info, n(m.out_naks));
            add(id::BOARD_ERRORS, Grade::Info, n(m.errors));
            add(id::BOARD_LAST_ERROR, Grade::Info, m.last_error as i32);
        }
        v
    }

    /// The last status AT32F405 `pc` reported, if its link is up.
    pub fn pc_usb_status(&self, pc: Pc) -> Option<AtStatus> {
        self.pcs[pc.index()].status
    }

    /// Progress reported by the firmware's update writer.
    pub fn set_fw_state(&mut self, state: FwState, progress: u32) {
        self.fw = (state, progress);
    }

    pub fn set_fw_busy(&mut self, busy: bool) {
        self.fw_busy = busy;
    }

    pub fn fw_state(&self) -> (FwState, u32) {
        self.fw
    }

    /// Next time `tick` must run, if anything is pending (hotkey replay, autosave).
    pub fn deadline(&self) -> Option<u32> {
        let save = self.dirty_since.map(|t| {
            let quiet = t.wrapping_add(AUTOSAVE_MS);
            let paced = self.save_paced.wrapping_add(SAVE_PACE_MS);
            // The later of the two, when the allowance is spent.
            if self.save_allowance == 0 && paced.wrapping_sub(quiet) as i32 > 0 { paced } else { quiet }
        });
        let (keymap, axes) = match self.keymap_now {
            Some(_) => (self.vial.remap.deadline(&self.vial.keymap), self.axes.deadline()),
            None => (None, None),
        };
        let unlock = self.unlock.map(|u| u.until);
        let learn = self.learning.as_ref().map(|l| l.until);
        let learn_tap = self.learning.as_ref().and_then(|l| l.learner.deadline());
        // A place waiting for the next millisecond.
        let place = self.place_pending.iter().any(Option::is_some).then(|| self.now.wrapping_add(1));
        let scan = self.wireless.scan_until;
        let watch = self.watch.map(|w| w.until);
        let monitor = self.monitor.map(|m| m.until);
        [
            self.hotkeys.deadline(),
            save,
            keymap,
            axes,
            unlock,
            learn,
            learn_tap,
            self.bt.pairing_until,
            scan,
            place,
            watch,
            monitor,
        ]
        .into_iter()
        .flatten()
        .min_by_key(|t| t.wrapping_sub(self.now) as i32)
    }

    pub fn tick(&mut self, now: u32) {
        self.now = now;
        self.place_waiting();
        if let Some(k) = self.hotkeys.tick(now) {
            self.replay_tap(k);
        }
        if let Some(k) = self.keymap_now {
            let keys = self.norm.keys().without(&self.hidden_keys()).without(&self.suppressed_keys);
            if self.axes.deadline().is_some_and(|t| now.wrapping_sub(t) as i32 >= 0) {
                let layers = self.vial.remap.layers(&self.vial.keymap);
                let ax = self.axes.tick(self.vial.keymap.axes(), k as usize, layers, now);
                self.apply_axes(&ax);
                if ax.buttons != self.axis_buttons {
                    self.axis_buttons = ax.buttons;
                    let pos = self.positions(&keys);
                    let steps = self.vial.remap.input(&mut self.vial.keymap, &pos, now);
                    self.apply_steps(&steps, &keys);
                }
            }
            let steps = self.vial.remap.tick(&mut self.vial.keymap, now);
            if !steps.is_empty() {
                self.apply_steps(&steps, &keys);
            }
            self.note_keymap_changes();
        }
        if now.wrapping_sub(self.rate_epoch) >= 1000 {
            let dt = now.wrapping_sub(self.rate_epoch);
            let rate = |n: &mut u32| {
                let r = (*n as u64 * 1000 / dt as u64).min(u16::MAX as u64) as u16;
                *n = 0;
                r
            };
            for p in self.ports.iter_mut() {
                p.rate_hz = rate(&mut p.reports);
            }
            for w in self.wireless.slots.iter_mut() {
                w.rate_hz = rate(&mut w.reports);
            }
            self.rate_epoch = now;
        }
        if self.watch.is_some_and(|w| now.wrapping_sub(w.until) as i32 >= 0) {
            self.end_watch();
        }
        if self.monitor.is_some_and(|m| now.wrapping_sub(m.until) as i32 >= 0) {
            self.monitor = None;
        }
        if self.unlock.is_some_and(|u| now.wrapping_sub(u.until) as i32 >= 0) {
            // Nobody pressed the key: the locks stay.
            self.unlock = None;
        }
        for &port in Port::ALL {
            let s = &self.ports[port.index()];
            if s.state == PortState::Probing && s.probe_until.is_some_and(|t| now.wrapping_sub(t) as i32 >= 0) {
                // Nothing came on Hub-0: the device was unplugged in passthrough, where the KVM couldn't see it. Until
                // 0.1.11 the port stayed probing (blinking yellow) until a device was plugged in (first board,
                // 2026-11-12, a flash drive on Aux 2).
                self.port_detached(port);
            }
        }
        if let Some(l) = self.learning.as_mut() {
            if let Some(h) = l.learner.tick(now) {
                self.end_learning(LearnEnd::Learned, h);
            } else if now.wrapping_sub(l.until) as i32 >= 0 {
                self.end_learning(LearnEnd::TimedOut, Hotkey::NONE);
            }
        }
        if self.bt.pairing_until.is_some_and(|t| now.wrapping_sub(t) as i32 >= 0) {
            // The module closes pairing by itself; its event may come later than this.
            self.bt.pairing_until = None;
            self.bt.passkey = None;
            self.event(Event::BtChanged(0));
        }
        if self.wireless.scan_until.is_some_and(|t| now.wrapping_sub(t) as i32 >= 0) {
            // As for pairing: the module stops by itself.
            self.wireless.scan_until = None;
            self.event(Event::BtDeviceChanged(0));
        }
        let again = if self.bt.present { self.wireless.maps_overdue(now) } else { 0 };
        for i in (0..self.wireless.slots.len()).filter(|i| again & 1 << i != 0) {
            self.emit(Output::Bt(BtOut::RequestMap(i as u8 + 1)));
        }
        self.pace_saves();
        if self.dirty_since.is_some_and(|t| now.wrapping_sub(t) >= AUTOSAVE_MS) && self.save_allowance > 0 {
            self.save_allowance -= 1;
            self.dirty_since = None;
            self.emit(Output::Save);
        }
        self.update_power();
    }

    /// Brings the save allowance up to date: one more per [`SAVE_PACE_MS`], up to [`SAVE_BURST`].
    fn pace_saves(&mut self) {
        if self.save_allowance >= SAVE_BURST {
            self.save_paced = self.now;
            return;
        }
        let due = self.now.wrapping_sub(self.save_paced) / SAVE_PACE_MS;
        if due > 0 {
            self.save_allowance = (u32::from(self.save_allowance) + due).min(u32::from(SAVE_BURST)) as u8;
            self.save_paced = self.save_paced.wrapping_add(due * SAVE_PACE_MS);
        }
    }

    /// `SaveConfig` and a factory reset: save without the autosave's wait, if the allowance has a save left; otherwise
    /// as soon as it has one (`tick`), with [`status_flags::CONFIG_DIRTY`] set until then.
    fn save_now(&mut self) {
        self.pace_saves();
        if self.save_allowance > 0 {
            self.save_allowance -= 1;
            self.dirty_since = None;
            self.emit(Output::Save);
        } else {
            self.dirty_since = Some(self.now.wrapping_sub(AUTOSAVE_MS));
        }
    }

    /// The firmware wrote everything [`Engine::take_save_item`] gave it. A change made since then is still to save, and
    /// keeps its autosave.
    pub fn config_saved(&mut self) {
        if !self.config_dirty && self.arrangements_dirty == 0 && !self.vial.keymap.is_dirty() && !self.cal.is_dirty() {
            self.dirty_since = None;
        }
    }

    /// After [`Output::Save`]: the next changed item to write (the arrangements first, then the config, then the
    /// keymap's), encoded into `buf` (at least [`SAVE_ITEM_MAX`] bytes). It counts as written from here. An
    /// arrangement goes before the config so that one moved out of an old config item is in flash before the config
    /// item that no longer holds it.
    pub fn take_save_item(&mut self, buf: &mut [u8]) -> Option<(u16, usize)> {
        if self.arrangements_dirty != 0 {
            let a = self.arrangements_dirty.trailing_zeros() as u8;
            self.arrangements_dirty &= !(1 << a);
            let mut item = [0u8; config::ARRANGEMENT_ITEM_MAX];
            let n = self.config.encode_arrangement(a, &mut item);
            buf[..n].copy_from_slice(&item[..n]);
            return Some((config::ARRANGEMENT_ITEM + a as u16, n));
        }
        if core::mem::take(&mut self.config_dirty) {
            let mut item = [0u8; config::ITEM_MAX];
            let n = self.config.encode(&mut item);
            buf[..n].copy_from_slice(&item[..n]);
            return Some((config::ITEM, n));
        }
        self.vial.keymap.take_dirty(buf).or_else(|| self.cal.take_dirty(buf))
    }

    /// At boot: an item read from flash that is not the config (an arrangement, keymap, macros, keymap settings).
    pub fn restore_item(&mut self, key: u16, data: &[u8]) {
        if let Some(a) = key.checked_sub(config::ARRANGEMENT_ITEM).filter(|a| (*a as usize) < MAX_ARRANGEMENTS) {
            self.config.restore_arrangement(a as u8, data);
            self.arrangements_dirty &= !(1 << a);
            return;
        }
        if (crate::known::ITEM..crate::known::ITEM + crate::known::MAX_DEVICES as u16).contains(&key) {
            self.cal.restore(key, data);
            return;
        }
        self.vial.keymap.restore(key, data);
        self.keymap_changes = self.vial.keymap.changes();
    }

    /// Store keys [`Engine::restore_item`] takes.
    pub fn item_keys() -> impl Iterator<Item = u16> {
        (0..MAX_ARRANGEMENTS as u16)
            .map(|a| config::ARRANGEMENT_ITEM + a)
            .chain(kvm_map::Keymap::item_keys())
            .chain(crate::known::KnownDevices::item_keys())
    }

    pub fn set_config(&mut self, v: ConfigValue, origin: Origin) -> Result<ConfigValue, ErrorCode> {
        let before = self.config;
        let v = self.config.set(v, origin)?;
        if self.config != before {
            self.config_effects(&before, &[v]);
        }
        Ok(v)
    }

    /// What follows from config values `changed` (already set; the config was `before`).
    fn config_effects(&mut self, before: &Config, changed: &[ConfigValue]) {
        for v in changed.iter().copied() {
            self.config_effect(v);
        }
        // A mode or routing, the active profile or its settings changed: the ports are decided again together, so a
        // profile switch moves every port at once (a port still probing is decided when that finishes), and every
        // device's input goes where it goes now.
        let ports = |c: &Config, i: usize| (c.modes()[i], c.route(Device::from_index(i).unwrap_or(Device::Aux2)));
        let devices = |c: &Config| core::array::from_fn::<_, DEVICES, _>(|i| ports(c, i));
        if devices(before) != devices(&self.config) {
            self.redecide(core::array::from_fn(|i| ports(before, i) != ports(&self.config, i)));
            self.release_dongle_for_passthrough();
        }
        if self.config.arrangement() != before.arrangement() {
            // Another arrangement: the pointer starts over on the active host's main screen there.
            self.hop.forget_all();
        }
        self.host_changed();
        if self.config.profile != before.profile {
            self.profile_changed_at = Some(self.now);
            if changed.iter().all(|v| v.key() != ctrl::key::PROFILE) {
                // Fewer profiles in use than the active one's number.
                self.config_changed(ctrl::key::PROFILE);
            }
        }
        for v in changed {
            self.config_changed(v.key());
        }
    }

    /// What a single value's change does beyond the general ones.
    fn config_effect(&mut self, v: ConfigValue) {
        match v {
            ConfigValue::Hotkey(..) | ConfigValue::DoubleTapMs(_) => self.hotkeys.configure(&self.config),
            ConfigValue::BtPolicy(i, policy) => self.emit(Output::Bt(BtOut::Policy { host: i + 1, policy })),
            // The mouse interface changes: both PCs enumerate it again.
            ConfigValue::PcPolling(_) | ConfigValue::AbsPointer(_) => self.rebuild_presentation(),
            ConfigValue::Vial(_) => {
                // The endpoint budget changed: every port is decided again, then the interfaces are rebuilt.
                self.redecide([false; PORTS]);
                self.rebuild_presentation();
            }
            // The virtual gamepad comes or goes: both PCs enumerate the interfaces again.
            ConfigValue::Gamepad(_) => self.rebuild_presentation(),
            _ => {}
        }
    }

    // ── control protocol ─────────────────────────────────────────────────────

    /// Handles one control-protocol report from `asker`; returns the response.
    pub fn control(&mut self, asker: Asker, raw: &[u8]) -> Option<[u8; ctrl::REPORT_LEN]> {
        let request_type = raw.first().copied().unwrap_or(0);
        let (id, req) = match Request::decode(raw) {
            Ok(r) => r,
            Err(_) => {
                return Response::Error(ErrorCode::Malformed)
                    .encode(request_type, raw.get(1).copied().unwrap_or(0))
                    .ok();
            }
        };
        let resp = match self.handle(asker, &req) {
            Ok(r) => r,
            Err(e) => Response::Error(e),
        };
        resp.encode(req.type_byte(), id).ok()
    }

    /// A control-protocol request Bluetooth host `host` wrote to the module's control service (software.md §8.1, "Over
    /// Bluetooth"); the answer goes back the same way.
    pub fn bt_control(&mut self, host: u8, data: &[u8], now: u32) {
        self.now = now;
        if !(1..=ctrl::BT_HOSTS as u8).contains(&host) {
            return;
        }
        let raw = btcontrol::pad(data);
        let resp = self.control(Asker::Bt(host), &raw);
        self.bt_reply(host, resp);
    }

    /// What a Bluetooth host may ask: `Some` answers it here, `None` lets the request through. One that may not control
    /// the KVM gets its identity (to see whether it speaks this protocol) and may ask to; one that may doesn't get what
    /// only the USB control interface can see through (firmware: the module that carries the request is the one being
    /// replaced; a restart into a bootloader, which only USB brings back), nor what streams to a PC or needs its
    /// cursor.
    fn bt_gate(&self, n: u8, req: &Request) -> Result<Option<Response>, ErrorCode> {
        let allowed = (n as usize).checked_sub(1).is_some_and(|i| self.config.bt_control.get(i) == Some(&true));
        Ok(match req {
            Request::GetInfo | Request::AskControl => None,
            _ if !allowed => return Err(ErrorCode::NotAllowed),
            Request::FwBegin { .. } | Request::FwData { .. } | Request::FwCommit => return Err(ErrorCode::Unsupported),
            Request::Reboot(t) if *t != RebootTarget::Normal => return Err(ErrorCode::Unsupported),
            // Stopping what it can't start: nothing to stop.
            Request::LearnHotkey(false) | Request::WatchControls(0) | Request::MonitorDevice(_, false) => {
                Some(Response::Ok)
            }
            Request::LearnHotkey(true)
            | Request::WatchControls(_)
            | Request::MonitorDevice(_, true)
            | Request::PointerAt(_)
            | Request::PlaceCursor(_) => return Err(ErrorCode::NotActive),
            _ => None,
        })
    }

    fn handle(&mut self, asker: Asker, req: &Request) -> Result<Response, ErrorCode> {
        if let Asker::Bt(n) = asker
            && let Some(r) = self.bt_gate(n, req)?
        {
            return Ok(r);
        }
        let pc = asker.pc();
        // Allowing software control again goes through the key, so it is asked for even while locked; so does letting
        // a Bluetooth host control the KVM.
        if let Request::SetConfig(
            v @ (ConfigValue::SwSwitchAllowed(true) | ConfigValue::SwConfigAllowed(true) | ConfigValue::BtControl(..)),
        ) = req
        {
            return self.request_lock(*v);
        }
        if req.is_config_change() && !self.config.sw_config_allowed {
            return Err(ErrorCode::Locked);
        }
        Ok(match req {
            Request::GetInfo => {
                let mut info = self.info;
                info.at_fw = [self.pcs[0].fw, self.pcs[1].fw];
                info.proto = ctrl::PROTOCOL_VERSION;
                if self.bt.present {
                    info.caps |= ctrl::caps::BLUETOOTH;
                    info.bt_fw = self.bt.fw;
                }
                Response::Info(info)
            }
            Request::GetStatus => Response::Status(self.status(asker.host())),
            // Not a configuration change: it only arms the key, as allowing software control again does.
            Request::AskControl => {
                let Asker::Bt(n) = asker else { return Err(ErrorCode::Unsupported) };
                self.request_host(n - 1, true)?;
                Response::Ok
            }
            Request::GetPort(p) => Response::Port(self.port_info(*p)),
            Request::GetPortString(p, kind) => {
                let d = self.ports[p.index()].dev.as_ref().ok_or(ErrorCode::NoDevice)?;
                let s = match kind {
                    StringKind::Manufacturer => &d.probed.manufacturer,
                    StringKind::Product => &d.probed.product,
                    StringKind::Serial => &d.probed.serial,
                };
                Response::PortString(*p, *kind, ctrl::short_string(s))
            }
            Request::SetActive(t) => {
                if !self.config.sw_switch_allowed {
                    return Err(ErrorCode::Locked);
                }
                if let Some(n) = t.bt_host() {
                    if !self.bt.present {
                        return Err(ErrorCode::Unsupported);
                    }
                    if !self.bt.selectable(n) {
                        return Err(ErrorCode::NoDevice);
                    }
                }
                self.switch(*t, SwitchSource::Software);
                Response::Ok
            }
            Request::GetConfig(k) => Response::Config(self.config.get(*k).ok_or(ErrorCode::InvalidValue)?),
            Request::SetConfig(v @ (ConfigValue::SwSwitchAllowed(_) | ConfigValue::SwConfigAllowed(_))) => {
                self.request_lock(*v)?
            }
            Request::SetConfig(v) => Response::Config(self.set_config(*v, Origin::Software)?),
            Request::SaveConfig => {
                self.config_dirty = true;
                self.save_now();
                Response::Ok
            }
            Request::FactoryReset => {
                let keep_locks = (self.config.sw_switch_allowed, self.config.sw_config_allowed);
                let before = self.config;
                self.config = Config::default();
                (self.config.sw_switch_allowed, self.config.sw_config_allowed) = keep_locks;
                self.hotkeys.configure(&self.config);
                self.vial.keymap.reset();
                let _ = self.vial.remap.release_all();
                self.keymap_buttons = 0;
                self.keymap_changes = self.vial.keymap.changes();
                let port =
                    |c: &Config, i: usize| (c.modes()[i], c.route(Device::from_index(i).unwrap_or(Device::Aux2)));
                self.redecide(core::array::from_fn(|i| port(&before, i) != port(&self.config, i)));
                self.rebuild_presentation();
                self.hop.forget_all();
                self.config_dirty = true;
                // Every arrangement's item goes too (empty now), or the old ones would come back at the next boot.
                self.arrangements_dirty = (1 << MAX_ARRANGEMENTS) - 1;
                self.save_now();
                Response::Ok
            }
            Request::Reboot(t) => {
                self.emit(Output::Reboot(*t));
                Response::Ok
            }
            Request::Identify(secs) => {
                self.identify_until = self.now + *secs as u32 * 1000;
                Response::Ok
            }
            Request::Redetect(p) => {
                self.release_port(*p);
                self.reprobe(*p);
                self.event(Event::PortChanged(*p));
                Response::Ok
            }
            Request::FwBegin { target, size, crc32 } => {
                let cap = match target {
                    FwTarget::Rp2354 => ctrl::caps::FW_UPDATE,
                    FwTarget::Module => ctrl::caps::BT_UPDATE,
                };
                if self.info.caps & cap == 0 {
                    return Err(ErrorCode::Unsupported);
                }
                if *target == FwTarget::Module && self.bt_module == BtModule::Absent {
                    return Err(ErrorCode::NoDevice);
                }
                if self.fw_busy || self.out.is_full() {
                    return Err(ErrorCode::Busy);
                }
                self.fw_target = *target;
                self.emit(Output::Firmware(FwOp::Begin { target: *target, size: *size, crc32: *crc32 }));
                self.fw = (FwState::Receiving, 0);
                Response::Fw { state: self.fw.0, progress: 0 }
            }
            Request::FwData { offset, data } => {
                if self.fw.0 != FwState::Receiving {
                    return Err(ErrorCode::InvalidValue);
                }
                if self.fw_busy || self.out.len() + 2 > OUTPUT_QUEUE {
                    return Err(ErrorCode::Busy);
                }
                self.emit(Output::Firmware(FwOp::Data { offset: *offset, data: data.clone() }));
                Response::Fw { state: self.fw.0, progress: offset + data.len() as u32 }
            }
            Request::FwCommit => {
                self.emit(Output::Firmware(FwOp::Commit));
                // The module's firmware needs no key: the module checks and writes its first sector now.
                let next = if self.fw_target == FwTarget::Module { FwState::Writing } else { FwState::AwaitingKey };
                self.fw = (next, self.fw.1);
                Response::Fw { state: self.fw.0, progress: self.fw.1 }
            }
            Request::FwStatus => Response::Fw { state: self.fw.0, progress: self.fw.1 },
            Request::GetDiag(page) => {
                let all = self.diag_items();
                let pages = all.len().div_ceil(cdiag::PER_PAGE).max(1);
                let start = *page as usize * cdiag::PER_PAGE;
                if *page as usize >= pages {
                    return Err(ErrorCode::InvalidValue);
                }
                let end = (start + cdiag::PER_PAGE).min(all.len());
                let items = Vec::from_slice(&all[start..end]).map_err(|_| ErrorCode::Unknown)?;
                Response::Diag { page: *page, pages: pages as u8, items }
            }
            Request::LearnHotkey(true) => {
                // The inactive PC would see what is typed on the active one (or on a Bluetooth host).
                if pc.is_some_and(|p| p != self.active) || self.bt.active.is_some() {
                    return Err(ErrorCode::NotActive);
                }
                if self.learning.as_ref().is_some_and(|l| l.pc != pc)
                    || matches!(self.standby.presence(self.active), Presence::Asleep { .. })
                {
                    return Err(ErrorCode::Busy);
                }
                // Again from the same PC: starts over, and what the last try withheld stays hidden.
                if let Some(old) = self.learning.take() {
                    self.suppressed_keys = self.suppressed_keys.union(&old.learner.withheld());
                }
                let held = self.all_keys();
                self.hotkeys.resume(&held);
                let learner = Learner::new(self.config.double_tap_ms as u32, &held);
                self.learning = Some(Learning { pc, until: self.now.wrapping_add(LEARN_MS), learner });
                Response::Ok
            }
            Request::LearnHotkey(false) => {
                if self.learning.as_ref().is_some_and(|l| l.pc != pc && pc.is_some()) {
                    return Err(ErrorCode::Busy);
                }
                self.end_learning(LearnEnd::Cancelled, Hotkey::NONE);
                Response::Ok
            }
            Request::GetBtHost(n) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                Response::BtHost(self.bt.info(*n).ok_or(ErrorCode::InvalidValue)?)
            }
            Request::BtPair(on) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                if *on {
                    // The module pairs one way at a time.
                    self.stop_scan();
                }
                let seconds = if *on { ctrl::BT_PAIR_SECONDS } else { 0 };
                self.emit(Output::Bt(BtOut::Pair { on: *on, seconds }));
                self.bt.pairing_until = on.then(|| self.now.wrapping_add(seconds as u32 * 1000));
                if !*on {
                    self.bt.passkey = None;
                }
                self.event(Event::BtChanged(0));
                Response::Ok
            }
            Request::BtForget(n) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                if *n != 0xFF && self.bt.host(*n).is_none() {
                    return Err(ErrorCode::InvalidValue);
                }
                if self.bt.active.is_some_and(|a| *n == 0xFF || a == *n) {
                    self.switch(Target::Toggle, SwitchSource::Software);
                }
                self.emit(Output::Bt(BtOut::Forget(*n)));
                Response::Ok
            }
            Request::BtPasskey(v) => {
                let e = self.bt.passkey.take().ok_or(ErrorCode::InvalidValue)?;
                if *v > 999_999 {
                    self.bt.passkey = Some(e);
                    return Err(ErrorCode::InvalidValue);
                }
                self.emit(Output::Bt(BtOut::Passkey { host: e.host, value: Some(*v) }));
                self.event(Event::BtChanged(0));
                Response::Ok
            }
            Request::GetBtDevice(n) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                Response::BtDevice(self.wireless.info(*n).ok_or(ErrorCode::InvalidValue)?)
            }
            Request::BtScan(on) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                if !*on {
                    self.stop_scan();
                    return Ok(Response::Ok);
                }
                if self.bt.pairing_until.is_some() {
                    // Host pairing closes: the module pairs one way at a time.
                    self.emit(Output::Bt(BtOut::Pair { on: false, seconds: 0 }));
                    self.bt.pairing_until = None;
                    self.bt.passkey = None;
                    self.event(Event::BtChanged(0));
                }
                let seconds = ctrl::BT_PAIR_SECONDS;
                self.emit(Output::Bt(BtOut::Scan { on: true, seconds }));
                self.wireless.scan_until = Some(self.now.wrapping_add(seconds as u32 * 1000));
                self.wireless.found.clear();
                if !self.device_pairing_busy() {
                    self.wireless.pairing = DevicePairing::Idle;
                    self.wireless.passkey = None;
                }
                self.event(Event::BtDeviceChanged(0));
                Response::Ok
            }
            Request::GetBtFound(i) => Response::BtFound(self.wireless.found.get(*i as usize).cloned()),
            Request::GetModuleLog(at) => {
                let (first, end, at, data) = self.module_log.read(*at, ctrl::MAP_PIECE);
                Response::ModuleLog { first, end, at, data: Vec::from_slice(&data).unwrap_or_default() }
            }
            Request::GetDeviceMap { device, index, offset } => {
                let (iface, interfaces, map): (u8, u8, &[u8]) = match (device.port(), device.wireless()) {
                    (Some(p), _) => {
                        let descs = self.ports[p.index()].dev.as_ref().map_or(&[][..], |d| &d.probed.report_descs[..]);
                        match descs.get(*index as usize) {
                            Some((i, m)) => (*i, descs.len() as u8, &m[..]),
                            None => (0, descs.len() as u8, &[]),
                        }
                    }
                    (None, Some(n)) => {
                        if self.wireless.slot(n).is_none() {
                            return Err(ErrorCode::InvalidValue);
                        }
                        (0, 1, if *index == 0 { self.wireless.map_of(n) } else { &[] })
                    }
                    (None, None) => return Err(ErrorCode::InvalidValue),
                };
                let from = (*offset as usize).min(map.len());
                let data = Vec::from_slice(&map[from..(from + ctrl::MAP_PIECE).min(map.len())]).unwrap_or_default();
                let (total, offset) = (map.len() as u16, from as u16);
                Response::DeviceMap { device: *device, index: *index, iface, interfaces, total, offset, data }
            }
            Request::BtPairDevice(addr) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                if self.device_pairing_busy() {
                    return Err(ErrorCode::Busy);
                }
                // The module stops looking to connect.
                self.wireless.scan_until = None;
                self.wireless.pairing = DevicePairing::Connecting;
                self.wireless.passkey = None;
                self.emit(Output::Bt(BtOut::PairDevice(*addr)));
                self.event(Event::BtDeviceChanged(0));
                Response::Ok
            }
            Request::BtForgetDevice(n) => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                if *n != 0xFF && self.wireless.slot(*n).is_none() {
                    return Err(ErrorCode::InvalidValue);
                }
                self.emit(Output::Bt(BtOut::ForgetDevice(*n)));
                Response::Ok
            }
            Request::AdoptModule(p) => {
                // Only for a KVM without a module: none on its board, no board on a front port that is one.
                if self.bt_fitted || matches!(self.dongle, Dongle::Claimed(_)) {
                    return Err(ErrorCode::Busy);
                }
                if self.ports[p.index()].dev.is_none() {
                    return Err(ErrorCode::NoDevice);
                }
                // A port set to passthrough keeps one's own board from being taken: this one is to be taken, also
                // when it comes back after a restart, so the port goes back to Auto.
                let device = Device::of_port(*p);
                if self.config.mode(device).is_passthrough() {
                    self.set_config(ConfigValue::Mode(device, PortMode::Auto), Origin::Software)?;
                }
                self.adopt = Some(*p);
                self.module_log.push(self.now, "RP2354B: adopting the ESP32 board on a front port as the module");
                if self.dongle == Dongle::Probing(*p) {
                    // Being found out right now: the firmware asks `dongle_adopt` before it lets the board go.
                    return Ok(Response::Ok);
                }
                self.release_port(*p);
                self.reprobe(*p);
                self.event(Event::PortChanged(*p));
                Response::Ok
            }
            Request::BtEraseAll => {
                if !self.bt.present {
                    return Err(ErrorCode::Unsupported);
                }
                self.module_log.push(self.now, "RP2354B: erasing every pairing the module holds");
                // The module restarts: its hosts and devices come back empty with its next `Hello`.
                self.emit(Output::Bt(BtOut::EraseAll));
                Response::Ok
            }
            Request::GetKeycodes { keymap, layer, first } => {
                let (k, l) = keymap_layer(*keymap, *layer)?;
                let codes = (*first as usize..kvm_map::POSITIONS)
                    .take(ctrl::KEYCODES_PER)
                    .map(|p| self.vial.keymap.get_in(k, l, p))
                    .collect();
                Response::Keycodes { keymap: *keymap, layer: *layer, first: *first, codes }
            }
            Request::SetKeycodes { keymap, layer, first, codes } => {
                let (k, l) = keymap_layer(*keymap, *layer)?;
                if *first as usize + codes.len() > kvm_map::POSITIONS {
                    return Err(ErrorCode::InvalidValue);
                }
                for (i, c) in codes.iter().enumerate() {
                    self.vial.keymap.set_in(k, l, *first as usize + i, *c);
                }
                self.note_keymap_changes();
                Response::Ok
            }
            Request::GetAxisRules { keymap, layer, first } => {
                let (k, l) = keymap_layer(*keymap, *layer)?;
                let rules = (*first as usize..kvm_map::axes::INPUTS)
                    .take(ctrl::RULES_PER)
                    .map(|i| self.vial.keymap.axes().get(k, l, i).encode())
                    .collect();
                Response::AxisRules { keymap: *keymap, layer: *layer, first: *first, rules }
            }
            Request::SetAxisRule { keymap, layer, input, rule } => {
                let (k, l) = keymap_layer(*keymap, *layer)?;
                let r = kvm_map::axes::Rule::decode(rule).ok_or(ErrorCode::InvalidValue)?;
                if *input as usize >= kvm_map::axes::INPUTS {
                    return Err(ErrorCode::InvalidValue);
                }
                self.vial.keymap.set_rule(k, l, *input as usize, r);
                self.note_keymap_changes();
                Response::Ok
            }
            Request::WatchControls(0) => {
                if self.watch.is_some_and(|w| w.pc == pc) {
                    self.end_watch();
                }
                Response::Ok
            }
            Request::WatchControls(mode) => {
                // As with learning a hotkey: the inactive PC would see what is typed on the active one.
                if pc.is_some_and(|p| p != self.active) || self.bt.active.is_some() {
                    return Err(ErrorCode::NotActive);
                }
                let until = self.now.wrapping_add(WATCH_MS);
                if let Some(w) = self.watch.as_mut().filter(|w| w.pc == pc) {
                    // Renewed: what was told stays told.
                    if w.swallow && *mode != 2 {
                        self.suppressed_keys = self.norm.keys();
                        self.suppressed_buttons = self.norm.buttons();
                    }
                    w.swallow = *mode == 2;
                    w.until = until;
                } else {
                    self.end_watch();
                    let mut w = Watch {
                        pc,
                        swallow: *mode == 2,
                        held: kvm_map::PosSet::EMPTY,
                        axes: [0; kvm_map::axes::INPUTS],
                        moved: [0; kvm_map::axes::RELATIVE],
                        told_at: [self.now.wrapping_sub(WATCH_AXIS_MS); kvm_map::axes::INPUTS],
                        until,
                    };
                    // What is held now isn't news; absolute axes start from where they are.
                    w.held = kvm_map::PosSet::from_keys(&self.norm.keys());
                    w.held.add_mask(kvm_map::pos::MOUSE, self.norm.buttons() as u32, kvm_map::pos::MOUSE_BUTTONS);
                    let pad = self.norm.gamepad();
                    w.held.add_mask(kvm_map::pos::PAD, pad.buttons, kvm_map::pos::PAD_BUTTONS);
                    for (i, a) in pad.axes.iter().enumerate() {
                        w.axes[kvm_map::axes::RELATIVE + i] = *a;
                    }
                    self.watch = Some(w);
                }
                Response::Ok
            }
            Request::GetControls(d) => Response::Controls(*d, self.controls(*d).ok_or(ErrorCode::NoDevice)?),
            Request::GetCalibration(d, axis) => {
                let id = self.identity(*d).ok_or(ErrorCode::NoDevice)?;
                if *axis as usize >= kvm_hid::gamepad::AXES {
                    return Err(ErrorCode::InvalidValue);
                }
                Response::Calibration(*d, *axis, self.cal.get(id, *axis as usize))
            }
            Request::SetCalibration(d, axis, c) => {
                let id = self.identity(*d).ok_or(ErrorCode::NoDevice)?;
                if !self.cal.set(id, *axis as usize, *c) {
                    return Err(ErrorCode::InvalidValue);
                }
                self.dirty_since = Some(self.now);
                Response::Ok
            }
            Request::GetOverride(d, n) => {
                let id = self.identity(*d).ok_or(ErrorCode::NoDevice)?;
                if *n as usize >= ctrl::MAX_OVERRIDES {
                    return Err(ErrorCode::InvalidValue);
                }
                Response::Override(*d, *n, self.cal.get_override(id, *n as usize))
            }
            Request::SetOverride(d, n, o) => {
                let id = self.identity(*d).ok_or(ErrorCode::NoDevice)?;
                if !self.cal.set_override(id, *n as usize, *o) {
                    return Err(ErrorCode::InvalidValue);
                }
                self.dirty_since = Some(self.now);
                Response::Ok
            }
            Request::GetMapEntry { kind, index } => {
                let k = *kvm_map::keymap::Dynamic::ALL.get(*kind as usize).ok_or(ErrorCode::InvalidValue)?;
                let mut b = [0u8; ctrl::MAP_ENTRY_MAX];
                let n = self.vial.keymap.dynamic(k, *index as usize, &mut b).ok_or(ErrorCode::InvalidValue)?;
                Response::MapEntry { kind: *kind, index: *index, data: Vec::from_slice(&b[..n]).unwrap_or_default() }
            }
            Request::SetMapEntry { kind, index, data } => {
                let k = *kvm_map::keymap::Dynamic::ALL.get(*kind as usize).ok_or(ErrorCode::InvalidValue)?;
                if !self.vial.keymap.set_dynamic(k, *index as usize, data) {
                    return Err(ErrorCode::InvalidValue);
                }
                self.note_keymap_changes();
                Response::Ok
            }
            Request::GetMacros(offset) => {
                let buf = self.vial.keymap.macros();
                let from = (*offset as usize).min(buf.len());
                let data = Vec::from_slice(&buf[from..(from + ctrl::MACRO_PIECE).min(buf.len())]).unwrap_or_default();
                Response::Macros { offset: from as u16, total: buf.len() as u16, data }
            }
            Request::SetMacros { offset, data } => {
                if *offset as usize + data.len() > kvm_map::keymap::MACRO_BUFFER {
                    return Err(ErrorCode::InvalidValue);
                }
                self.vial.keymap.write_macros(*offset as usize, data);
                self.note_keymap_changes();
                Response::Ok
            }
            Request::GetMapSetting(q) => {
                let mut b = [0u8; 4];
                let n = self.vial.keymap.settings.get(*q, &mut b).ok_or(ErrorCode::InvalidValue)?;
                b[n..].fill(0);
                Response::MapSetting(*q, u32::from_le_bytes(b))
            }
            Request::SetMapSetting(q, v) => {
                if !self.vial.keymap.settings.set(*q, &v.to_le_bytes()) {
                    return Err(ErrorCode::InvalidValue);
                }
                self.vial.keymap.settings_changed();
                self.note_keymap_changes();
                Response::Ok
            }
            Request::MonitorDevice(_, false) => {
                if self.monitor.is_some_and(|m| m.pc == pc) {
                    self.monitor = None;
                }
                Response::Ok
            }
            Request::MonitorDevice(d, true) => {
                // As watching the controls: the inactive PC would see what is typed on the active one.
                if pc.is_some_and(|p| p != self.active) || self.bt.active.is_some() {
                    return Err(ErrorCode::NotActive);
                }
                let until = self.now.wrapping_add(WATCH_MS);
                match self.monitor.as_mut().filter(|m| m.pc == pc && m.device == *d) {
                    Some(m) => m.until = until,
                    None => {
                        self.monitor =
                            Some(Monitor { pc, device: *d, until, sent: [(0, 0, 0, 0); MONITOR_SLOTS], used: 0 })
                    }
                }
                Response::Ok
            }
            Request::GetOutputs(h) => {
                let (offered, taken) = self.outputs(*h);
                Response::Outputs { host: *h, offered, taken }
            }
            Request::PlaceCursor(placement) => {
                // Only a PC has a cursor (not the service port).
                let pc = pc.ok_or(ErrorCode::NotActive)?;
                self.placer[pc.index()] = match placement {
                    ctrl::Placement::Off => None,
                    p => Some((*p, self.now.wrapping_add(ctrl::PLACE_LEASE_MS))),
                };
                Response::Ok
            }
            Request::PointerAt(at) => {
                // Only a PC has a cursor (not the service port).
                let pc = pc.ok_or(ErrorCode::NotActive)?;
                if self.config.hop_on() {
                    let host = Host::pc(pc);
                    let active = host == self.active_host();
                    let map = self.hop_map(host);
                    let layout = *self.config.layout();
                    self.hop.sync(&layout, host, map, *at, self.now, active);
                }
                Response::Ok
            }
        })
    }

    // ── Bluetooth module (software.md §4.13) ─────────────────────────────────

    /// A device is being paired (connecting, pairing, reading its map).
    fn device_pairing_busy(&self) -> bool {
        matches!(self.wireless.pairing, DevicePairing::Connecting | DevicePairing::Pairing | DevicePairing::Reading)
    }

    /// Stops looking for devices, if the module is looking.
    fn stop_scan(&mut self) {
        if self.wireless.scan_until.take().is_some() {
            self.emit(Output::Bt(BtOut::Scan { on: false, seconds: 0 }));
            self.event(Event::BtDeviceChanged(0));
        }
    }

    /// A device slot changed on the module (`name`: what the device calls itself).
    pub fn bt_device(&mut self, state: DeviceState, name: &str) {
        let Some(d) = Device::wireless_slot(state.dev) else { return };
        let Some(c) = self.wireless.update(state, name, self.now) else { return };
        if c.stopped || !state.bonded() {
            self.release_dev(d);
        }
        if c.want_map {
            self.emit(Output::Bt(BtOut::RequestMap(state.dev)));
        }
        if c.usable {
            // One of the KVM's devices now, or no longer: the endpoint budget, where input goes, the pinned hosts.
            self.redecide([false; PORTS]);
        } else if c.started {
            let target = self.target_of(d);
            self.apply_leds_of(Some(target));
        }
        if c.changed || c.usable {
            self.event(Event::BtDeviceChanged(state.dev));
        }
    }

    /// A line of the module's log (`BtMsg::Log`).
    pub fn bt_log(&mut self, text: &str, now: u32) {
        self.now = now;
        self.module_log.push(now, text);
    }

    /// A piece of device slot `dev`'s report map.
    pub fn bt_device_map(&mut self, dev: u8, offset: u16, total: u16, data: &[u8]) {
        match self.wireless.map_piece(dev, offset, total, data) {
            Some(crate::bt::MapState::Held) => {
                self.redecide([false; PORTS]);
                self.event(Event::BtDeviceChanged(dev));
            }
            Some(_) => self.event(Event::BtDeviceChanged(dev)),
            None => {}
        }
    }

    /// A device found while looking for devices to pair.
    pub fn bt_found(&mut self, addr: [u8; 7], rssi: i8, appearance: u16, name: &str) {
        if self.wireless.scan_until.is_some() && self.wireless.found(addr, rssi, appearance, name) {
            self.event(Event::BtDeviceChanged(0));
        }
    }

    /// The module answered (its `Hello`, after a reset or once programmed): tell it the policies, ask for the hosts,
    /// and select the active host again (a module that restarted forgot it).
    pub fn bt_hello(&mut self, fw: u32) {
        let mut line: heapless::String<64> = heapless::String::new();
        let (major, minor, patch) = (fw >> 16, fw >> 8 & 0xFF, fw & 0xFF);
        let _ = core::fmt::Write::write_fmt(
            &mut line,
            format_args!("RP2354B: the module said hello, firmware {major}.{minor}.{patch}"),
        );
        self.module_log.push(self.now, &line);
        self.bt.present = true;
        self.bt.fw = fw;
        self.bt_module = BtModule::Up;
        // What the module holds comes again with its answer to `RequestState`.
        self.bt.settings_held = 0;
        for (i, policy) in self.config.bt_policies.into_iter().enumerate() {
            self.emit(Output::Bt(BtOut::Policy { host: i as u8 + 1, policy }));
        }
        self.emit(Output::Bt(BtOut::RequestState));
        self.bt.selected = None;
        self.bt_select();
        // A module that restarted looks for no devices and pairs none.
        self.wireless.scan_until = None;
        if self.device_pairing_busy() {
            self.wireless.pairing = DevicePairing::Failed;
            self.wireless.passkey = None;
        }
        self.event(Event::BtChanged(0));
        self.event(Event::BtDeviceChanged(0));
    }

    /// The module stopped answering (or is being reprogrammed): no Bluetooth targets until it says hello again; input
    /// goes back to the home PC.
    pub fn bt_link_down(&mut self) {
        if !self.bt.present {
            return;
        }
        self.module_log.push(self.now, "RP2354B: the module stopped answering");
        if self.bt.active.is_some() {
            self.switch(Target::Toggle, SwitchSource::Power);
        }
        let active = self.bt.active;
        self.bt = BtState { active, ..BtState::EMPTY };
        // Its devices go with it.
        for d in Device::ALL.iter().copied().filter(|d| d.wireless().is_some()) {
            self.release_dev(d);
        }
        self.wireless = Devices::EMPTY;
        self.redecide([false; PORTS]);
        self.event(Event::BtChanged(0));
        self.event(Event::BtDeviceChanged(0));
    }

    /// A host slot changed on the module.
    pub fn bt_host(&mut self, h: BtHostState) {
        let Some(slot) = (h.host as usize).checked_sub(1).filter(|i| *i < ctrl::BT_HOSTS) else { return };
        let was = self.bt.hosts[slot];
        self.bt.hosts[slot] = h;
        // A host that lost its bond (forgotten here or on the host) can't stay the target.
        if self.bt.active == Some(h.host) && !h.bonded() {
            self.switch(Target::Toggle, SwitchSource::Power);
        }
        // Nor may it control the KVM: the next host to pair into the slot would have that. Any state without a bond
        // ends it (the module sends every slot's after each `Hello`), so a change lost on the way, or before a save,
        // doesn't stay.
        if !h.bonded() && self.config.bt_control[slot] {
            let _ = self.set_config(ConfigValue::BtControl(slot as u8, false), Origin::Device);
            if self.unlock.is_some_and(|u| u.host == Some(h.host)) {
                self.unlock = None;
            }
        }
        if was != h {
            self.event(Event::BtChanged(h.host));
        }
        if self.bt.active == Some(h.host) && was.reports != h.reports {
            self.host_changed();
        }
    }

    /// An output report from host `host` (its keyboard LEDs).
    pub fn bt_output(&mut self, host: u8, id: u8, data: &[u8]) {
        if id != btreport::KEYBOARD || data.is_empty() {
            return;
        }
        let Some(slot) = (host as usize).checked_sub(1).filter(|i| *i < ctrl::BT_HOSTS) else { return };
        self.bt.leds[slot] = data[0];
        if let Some(h) = Host::bt(host) {
            self.apply_leds_of(Some(h));
        }
    }

    /// A pairing or bond event from the module.
    pub fn bt_event(&mut self, kind: BtEventKind, host: u8, value: u32) {
        if self.wireless.event(kind, value, self.now) {
            self.event(Event::BtDeviceChanged(if kind == BtEventKind::DeviceBondLost { host } else { 0 }));
            return;
        }
        let held = self.all_keys();
        let settings_held = self.bt.settings_held;
        self.bt.event(kind, host, value, self.now, held);
        self.event(Event::BtChanged(host));
        // The hosts' software tells the user: a host that keeps changing its settings would wear the module's flash.
        if self.bt.settings_held != settings_held {
            self.event(Event::Fault(FaultKind::BtSettingsHeld, None));
        }
    }
}

/// A keymap and layer as a request names them, checked.
fn keymap_layer(keymap: u8, layer: u8) -> Result<(usize, usize), ErrorCode> {
    if (keymap as usize) < kvm_map::KEYMAPS && (layer as usize) < kvm_map::LAYERS {
        Ok((keymap as usize, layer as usize))
    } else {
        Err(ErrorCode::InvalidValue)
    }
}
#[cfg(test)]
// Bitmap offsets like `2 + 0x04 / 8` spell out which byte a usage lives in.
#[allow(clippy::identity_op)]
mod tests {
    use super::*;
    use crate::usbdesc::fixtures::GAMING_MOUSE_CONFIG;
    use kvm_hid::normalized;
    use kvm_proto::ctrl::{AxisCal, DeviceMsg, GamepadOut, Hotkey};
    use kvm_proto::descset::DescSet;

    /// Interface 0 of the gaming-mouse config with a boot keyboard report descriptor, interface 1 a mouse.
    fn keyboard_and_mouse() -> Probed {
        let kb = &[
            0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
            0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01,
            0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
            0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
        ][..];
        let ms = &[
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00,
            0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30,
            0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0,
        ][..];
        Probed {
            device: DeviceDesc { vid: 0x1234, pid: 0x5678, ..Default::default() },
            speed: Some(Speed::Full),
            config: Vec::from_slice(GAMING_MOUSE_CONFIG).unwrap(),
            report_descs: Vec::from_iter([(0, Vec::from_slice(kb).unwrap()), (1, Vec::from_slice(ms).unwrap())]),
            product: String::try_from("Combo").unwrap(),
            ..Default::default()
        }
    }

    fn engine() -> Engine {
        let info =
            Info { proto: 1, rp_fw: 0x000100, at_fw: [0; 2], hw: 0x000C00, serial: *b"TEST0001", caps: 0, bt_fw: 0 };
        let mut e = Engine::new(Config::default(), info, "TEST0001");
        e.pc_hello(Pc::Pc1, 1);
        e.pc_hello(Pc::Pc2, 1);
        e
    }

    fn drain(e: &mut Engine) -> std::vec::Vec<Output> {
        core::iter::from_fn(|| e.poll_output()).collect()
    }

    fn reports(out: &[Output]) -> std::vec::Vec<(Pc, u8, std::vec::Vec<u8>)> {
        out.iter()
            .filter_map(|o| match o {
                Output::Report { pc, ep, data } if *ep != 0x84 => Some((*pc, *ep, data.to_vec())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn initial_descriptor_set_is_valid() {
        let mut e = engine();
        assert_eq!(drain(&mut e), [Output::DescriptorSetChanged]);
        let set = DescSet::parse(e.descriptor_set()).unwrap();
        assert!(set.device().is_some());
        assert_eq!(e.presentation().control.ep_out, 0x04);
    }

    #[test]
    fn normalized_keys_go_to_the_active_pc_and_switch_releases_them() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::Route { .. })), "already on Hub-0");
        assert_eq!(e.port_info(Port::Keyboard).effective, PortMode::Normalized);

        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 10);
        let r = reports(&drain(&mut e));
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].0, r[0].1), (Pc::Pc1, 0x83));
        assert_eq!(r[0].2[0], normalized::NKRO_ID);

        // Switch with 'a' held: PC1 gets releases, PC2 never sees 'a' until it is released and pressed again.
        e.switch(Target::Pc2, SwitchSource::Software);
        let r = reports(&drain(&mut e));
        assert!(r.iter().all(|(pc, ..)| *pc == Pc::Pc1));
        assert!(r.iter().any(|(_, ep, d)| *ep == 0x83 && d[1..].iter().all(|b| *b == 0)));
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0x05, 0, 0, 0, 0], 20);
        let r = reports(&drain(&mut e));
        let nkro = &r.last().unwrap().2;
        assert_eq!(r.last().unwrap().0, Pc::Pc2);
        assert_eq!(nkro[2] & (1 << 0x04), 0, "held 'a' is suppressed");
        assert_ne!(nkro[2] & (1 << 0x05), 0, "new 'b' goes through");
        e.device_report(Port::Keyboard, 0, &[0; 8], 30);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 40);
        let r = reports(&drain(&mut e));
        let nkro = &r.last().unwrap().2;
        assert_ne!(nkro[2] & (1 << 0x04), 0, "'a' pressed anew reaches PC2");
    }

    /// Every page of the diagnostics, and the number of pages the first one announced.
    fn diag(e: &mut Engine) -> (std::vec::Vec<Item>, u8) {
        let mut items = std::vec::Vec::new();
        let mut page = 0;
        loop {
            let Response::Diag { page: p, pages, items: got } = ask(e, Request::GetDiag(page)) else { panic!() };
            assert_eq!(p, page);
            items.extend(got);
            page += 1;
            if page >= pages {
                return (items, pages);
            }
        }
    }

    #[test]
    fn a_pc_that_never_reset_its_at32_is_not_asleep() {
        // First board, 2026-11-15: PC2 shown "asleep", full speed, 0 bus resets. The AT32F405's core reports the idle bus
        // as a suspend whether or not the PC ever took it up.
        let mut e = engine();
        let never =
            AtStatus { speed: kvm_proto::link::msg::UsbSpeed::Full, resets: 0, ..usb(UsbState::Suspended, false) };
        e.pc_status(Pc::Pc2, never);
        assert_eq!(e.status(None).links[1], LinkState::Enumerating);
        assert_eq!(e.standby.presence(Pc::Pc2), Presence::NoHost);
        let (items, _) = diag(&mut e);
        let pc2_speed = items.iter().find(|i| i.id == id::PC2_SPEED).unwrap();
        assert_eq!((pc2_speed.value, pc2_speed.grade), (Speed::Unknown as i32, Grade::Info));
        // Once it did, a suspend is sleep.
        e.pc_status(Pc::Pc2, AtStatus { resets: 1, ..never });
        assert_eq!(e.status(None).links[1], LinkState::Suspended);
        assert_eq!(e.standby.presence(Pc::Pc2), Presence::Asleep { wake: false });
    }

    #[test]
    fn diagnostics_grade_what_the_engine_and_the_firmware_know() {
        let mut e = engine();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, true));
        e.pc_status(
            Pc::Pc2,
            AtStatus { speed: kvm_proto::link::msg::UsbSpeed::Full, ..usb(UsbState::Configured, true) },
        );
        e.set_selftest(0);
        let (before, _) = diag(&mut e);
        assert!(before.iter().all(|i| i.id != id::PLL_USB), "no clocks before the firmware measured them");
        e.set_board_diag(BoardDiag { hub0_speed: Speed::Low, hub0_up: false, ..BoardDiag::TYPICAL });
        let (items, pages) = diag(&mut e);
        assert_eq!(pages as usize, items.len().div_ceil(cdiag::PER_PAGE));
        assert!(matches!(ask(&mut e, Request::GetDiag(pages)), Response::Error(ErrorCode::InvalidValue)));
        let grade = |id: u8| items.iter().find(|i| i.id == id).map(|i| i.grade);
        for i in &items {
            assert!(cdiag::meta(i.id).is_some(), "{:#x}", i.id);
        }
        assert_eq!(grade(id::PLL_USB), Some(Grade::Ok));
        assert_eq!(grade(id::CLK_SYS), Some(Grade::Ok));
        assert_eq!(grade(id::HUB0_SPEED), Some(Grade::Bad), "a hub seen as low speed: E1");
        assert_eq!(grade(id::HUB0_UP), Some(Grade::Bad));
        assert_eq!(grade(id::PC1_SPEED), Some(Grade::Ok));
        assert_eq!(grade(id::PC2_SPEED), Some(Grade::Warn), "full speed to the PC");
        assert_eq!(grade(id::PC1_VBUS), Some(Grade::Ok));
        assert_eq!(grade(id::AT1_3V3), Some(Grade::Ok));
        assert_eq!(grade(id::AT1_FW), Some(Grade::Warn), "the AT32F405s said version 1, the RP2354B is 0x100");
        assert_eq!(grade(id::PWR_VBUS), Some(Grade::Info), "nothing on the PWR port");
        // The PWR port with 5 V on CC.
        e.set_power(PowerInfo { source: PowerSource::PwrPort, adv_pwr: Advertised::A3_0, vbus_pwr_mv: 5100 });
        e.set_board_diag(BoardDiag { pwr_cc_mv: 4900, ..BoardDiag::TYPICAL });
        let (items, _) = diag(&mut e);
        let grade = |id: u8| items.iter().find(|i| i.id == id).map(|i| i.grade);
        assert_eq!(grade(id::PWR_VBUS), Some(Grade::Ok));
        assert_eq!(grade(id::PWR_CC), Some(Grade::Bad));
    }

    fn usb(usb_state: UsbState, wake_enabled: bool) -> AtStatus {
        let vbus_mv = if usb_state == UsbState::Unpowered { 0 } else { 5000 };
        AtStatus {
            usb_state,
            speed: kvm_proto::link::msg::UsbSpeed::High,
            vbus_mv,
            cc1_mv: 0,
            cc2_mv: 0,
            frame: 0,
            // The PC reset it on its way to any of the states after Powered.
            resets: if matches!(usb_state, UsbState::Unpowered | UsbState::Powered | UsbState::Detached) {
                0
            } else {
                1
            },
            desc_crc: 0,
            wake_enabled,
            vdda_mv: Some(3300),
            temp_dc: Some(350),
            vbus_min_mv: vbus_mv,
        }
    }

    /// An engine whose self-test has passed and no longer shows on the LEDs (it keeps the KVM out of standby), at `T0`.
    fn settled() -> Engine {
        let mut e = engine();
        e.set_selftest(0);
        e.tick(T0);
        drain(&mut e);
        e
    }

    const T0: u32 = SELFTEST_SHOW_MS;

    fn powers(out: &[Output]) -> std::vec::Vec<PowerPlan> {
        out.iter().filter_map(|o| if let Output::Power(p) = o { Some(*p) } else { None }).collect()
    }

    #[test]
    fn the_supply_warning_comes_from_the_measured_5_v_not_the_declared_maximum() {
        let mut e = settled();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        let st = |mv: u16, min: u16| AtStatus { vbus_mv: mv, vbus_min_mv: min, ..usb(UsbState::Configured, false) };
        let mut t = T0;
        let mut feed = |e: &mut Engine, ms: u32, mv: u16, min: u16| {
            for _ in 0..ms / 100 {
                t += 100;
                e.tick(t);
                e.pc_status(Pc::Pc1, st(mv, min));
            }
        };
        // PC1 alone, holding its 5 V: good, whatever the devices declare; an inrush dip is only counted.
        feed(&mut e, 3000, 4950, 4950);
        feed(&mut e, 100, 4950, 4300);
        let s = e.status(None);
        assert_eq!((s.supply, s.flags & status_flags::SUPPLY_SAGS), (ctrl::Supply::Good, 0));
        assert_eq!((s.supply_mv, s.supply_lowest_mv), (4950, 4300));
        assert!(s.declared_ma > 0);
        assert!(!e.led_inputs().supply_sags);
        // Sagging under 4.5 V for 2 s: the warning, and the connected PC's LED amber.
        feed(&mut e, 2500, 4450, 4400);
        let s = e.status(None);
        assert_eq!((s.supply, s.flags & status_flags::SUPPLY_SAGS), (ctrl::Supply::Sags, status_flags::SUPPLY_SAGS));
        assert!(e.led_inputs().supply_sags);
        // PC2 gives a healthy 5 V too: the ideal diodes feed the board from it, so it is good again (after 5 s).
        for _ in 0..60 {
            t += 100;
            e.tick(t);
            e.pc_status(Pc::Pc1, st(4450, 4400));
            e.pc_status(Pc::Pc2, st(5050, 5000));
        }
        assert_eq!(e.status(None).supply, ctrl::Supply::Good);
        // On the PWR port, only its reading counts.
        for _ in 0..30 {
            t += 100;
            e.tick(t);
            e.set_power(PowerInfo { source: PowerSource::PwrPort, adv_pwr: Advertised::A3_0, vbus_pwr_mv: 4350 });
        }
        assert_eq!(e.status(None).supply, ctrl::Supply::Low);
        let (items, _) = diag(&mut e);
        let item = items.iter().find(|i| i.id == ctrl::diag::id::SUPPLY).unwrap();
        assert_eq!((item.grade, item.value), (Grade::Bad, 4300));
    }

    #[test]
    fn the_running_pc_becomes_active_without_saving_anything() {
        let mut e = engine();
        drain(&mut e);
        // The KVM came up because PC2 was switched on; PC1 is off.
        e.pc_status(Pc::Pc1, usb(UsbState::Unpowered, false));
        assert_eq!(e.active(), Pc::Pc1, "PC2 not heard from yet");
        e.pc_status(Pc::Pc2, usb(UsbState::Powered, false));
        assert_eq!(e.active(), Pc::Pc2);
        let switched = Event::Switched(Pc::Pc2, SwitchSource::Power).encode().unwrap();
        let out = drain(&mut e);
        assert!(out.iter().any(|o| matches!(o, Output::Report { ep: 0x84, data, .. } if data.as_slice() == switched)));
        assert!(!out.iter().any(|o| matches!(o, Output::Save)));
        // PC1 comes on later: PC2 stays active. PC2 goes off: after a few seconds PC1 takes over.
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Configured, false));
        e.tick(1_000);
        e.pc_status(Pc::Pc2, usb(UsbState::Unpowered, false));
        e.tick(1_000 + crate::standby::VBUS_GONE_MS - 100);
        assert_eq!(e.active(), Pc::Pc2, "a reboot may drop 5 V briefly");
        e.tick(1_000 + crate::standby::VBUS_GONE_MS);
        assert_eq!(e.active(), Pc::Pc1);
        // Off in the config: nothing moves.
        let mut e = engine();
        e.set_config(ConfigValue::AutoSelect(false), Origin::Software).unwrap();
        e.pc_status(Pc::Pc1, usb(UsbState::Unpowered, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Configured, false));
        assert_eq!(e.active(), Pc::Pc1);
    }

    #[test]
    fn standby_powers_down_and_the_key_wakes_the_kvm_without_switching() {
        let mut e = settled();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Unpowered, false));
        e.tick(T0 + 1_000);
        drain(&mut e);
        e.pc_status(Pc::Pc1, usb(UsbState::Unpowered, false));
        e.tick(T0 + 2_000);
        e.tick(T0 + 2_000 + crate::standby::STANDBY_AFTER_MS);
        let deep = PowerPlan { mode: PowerMode::DeepStandby, ports: [false; PORTS], hub0: false };
        assert_eq!(powers(&drain(&mut e)), [deep]);
        assert!(e.led_inputs().standby);
        assert!(crate::leds::render(&e.led_inputs(), 0).iter().all(|c| *c == ctrl::Rgb::default()));
        e.key(KeyEvent::Tap);
        assert_eq!(e.active(), Pc::Pc1, "the first press only wakes the KVM");
        assert_eq!(powers(&drain(&mut e)), [PowerPlan::ACTIVE]);
        e.key(KeyEvent::Tap);
        assert_eq!(e.active(), Pc::Pc2);
    }

    #[test]
    fn a_key_press_wakes_the_sleeping_pc_and_never_reaches_it() {
        let mut e = settled();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Unpowered, false));
        drain(&mut e);
        e.pc_status(Pc::Pc1, usb(UsbState::Suspended, true));
        e.tick(T0 + 100);
        e.tick(T0 + 100 + crate::standby::STANDBY_AFTER_MS);
        let plan = e.power_plan();
        assert_eq!(plan.mode, PowerMode::WakeStandby);
        assert_eq!(plan.ports, [true, true, false, true], "a keyboard, and two empty ports (no Aux 1 on this board)");
        drain(&mut e);

        let t = T0 + 20_000;
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], t);
        let out = drain(&mut e);
        assert_eq!(out.iter().filter(|o| matches!(o, Output::RemoteWakeup(Pc::Pc1))).count(), 1);
        assert!(reports(&out).is_empty(), "the wake key is not typed");
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0x05, 0, 0, 0, 0], t + 100);
        assert!(drain(&mut e).is_empty(), "one wakeup per second");
        // The PC resumes with 'a' and 'b' still held: they stay hidden until released.
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, true));
        assert_eq!(powers(&drain(&mut e)), [PowerPlan::ACTIVE]);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0x05, 0x06, 0, 0, 0], t + 200);
        let r = reports(&drain(&mut e));
        let nkro = &r.last().unwrap().2;
        assert_eq!(nkro[2] & (1 << 0x04 | 1 << 0x05), 0);
        assert_ne!(nkro[2] & (1 << 0x06), 0);
        // Not allowed to wake: nothing is sent at all.
        e.pc_status(Pc::Pc1, usb(UsbState::Suspended, false));
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x07, 0, 0, 0, 0, 0], t + 5_000);
        assert!(drain(&mut e).is_empty());
    }

    #[test]
    fn double_scroll_lock_switches_without_reaching_either_pc() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        let sl = [0, 0, 0x47, 0, 0, 0, 0, 0];
        e.device_report(Port::Keyboard, 0, &sl, 0);
        e.device_report(Port::Keyboard, 0, &[0; 8], 50);
        e.device_report(Port::Keyboard, 0, &sl, 120);
        e.device_report(Port::Keyboard, 0, &[0; 8], 170);
        assert_eq!(e.active(), Pc::Pc2);
        let out = drain(&mut e);
        for (_, ep, d) in reports(&out) {
            if ep == 0x83 && d[0] == normalized::NKRO_ID {
                assert_eq!(d[2 + 0x47 / 8] & (1 << (0x47 % 8)), 0, "scroll lock leaked");
            }
        }
        let switched = Event::Switched(Pc::Pc2, SwitchSource::Hotkey).encode().unwrap();
        assert!(out.iter().any(|o| matches!(o, Output::Report { ep: 0x84, data, .. } if data.as_slice() == switched)));
    }

    #[test]
    fn mouse_reports_and_boot_protocol() {
        let mut e = engine();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        drain(&mut e);
        e.device_report(Port::Mouse, 1, &[0x01, 0x05, 0xFB], 0);
        let r = reports(&drain(&mut e));
        assert_eq!(
            r,
            [(
                Pc::Pc1,
                0x82,
                normalized::mouse_report(&MouseInput { buttons: 1, x: 5, y: -5, wheel: 0, pan: 0 }, false, false)
                    .to_vec()
            )]
        );
        e.pc_protocol(Pc::Pc1, 1, 0);
        e.device_report(Port::Mouse, 1, &[0x00, 0x01, 0x00], 1);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, std::vec![0, 1, 0])]);
    }

    #[test]
    fn mirrored_port_forwards_raw_and_rebuilds_the_descriptor_set() {
        let mut e = engine();
        drain(&mut e);
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        let out = drain(&mut e);
        assert!(out.contains(&Output::DescriptorSetChanged));
        assert_eq!(e.presentation().mirrors.len(), 2);
        let m = e.presentation().mirror_of(Port::Mouse, 1).unwrap().f;
        e.device_report(Port::Mouse, 1, &[0x02, 0x10, 0x00], 5);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, m.ep_in, std::vec![0x02, 0x10, 0x00])]);

        // A held button is stripped on the new PC after a switch.
        e.switch(Target::Pc2, SwitchSource::Key);
        let neutral = reports(&drain(&mut e));
        assert!(neutral.contains(&(Pc::Pc1, m.ep_in, std::vec![0, 0, 0])));
        e.device_report(Port::Mouse, 1, &[0x02, 0x01, 0x00], 6);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc2, m.ep_in, std::vec![0x00, 0x01, 0x00])]);

        // Unplugging the mirrored device shrinks the descriptor set again.
        e.port_detached(Port::Mouse);
        assert!(drain(&mut e).contains(&Output::DescriptorSetChanged));
        assert!(e.presentation().mirrors.is_empty());
    }

    #[test]
    fn a_mirrored_port_back_from_passthrough_gets_its_mirror_again() {
        // First board, 2026-11-12: after passthrough and back, the keyboard was hosted and its reports arrived, but
        // the PCs never got a descriptor set with its mirror again, so they went nowhere.
        let mut e = engine();
        drain(&mut e);
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        drain(&mut e);
        assert_eq!(e.presentation().mirrors.len(), 2);

        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Passthrough), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::DescriptorSetChanged));
        assert!(e.presentation().mirrors.is_empty());

        // Back: the KVM probes the device again (it attaches anew on Hub-0), then mirrors it.
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::Reprobe(Port::Mouse)));
        e.port_probing(Port::Mouse);
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        let out = drain(&mut e);
        assert!(out.contains(&Output::DescriptorSetChanged), "{out:?}");
        assert_eq!(e.presentation().mirrors.len(), 2);
        let m = e.presentation().mirror_of(Port::Mouse, 1).unwrap().f;
        e.device_report(Port::Mouse, 1, &[0x02, 0x10, 0x00], 5);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, m.ep_in, std::vec![0x02, 0x10, 0x00])]);
    }

    #[test]
    fn forwarded_requests_go_to_the_device_only_from_the_active_pc() {
        let mut e = engine();
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        let iface = e.presentation().mirror_of(Port::Keyboard, 0).unwrap().f.iface;
        let set_leds = [0x21, 0x09, 0x00, 0x02, iface, 0, 1, 0];
        e.pc_ctrl_request(Pc::Pc1, 7, set_leds, &[0x02]);
        let out = drain(&mut e);
        assert!(
            matches!(&out[..], [Output::DeviceControl { pc: Pc::Pc1, tag: 7, port: Port::Keyboard, setup, .. }] if setup[4] == 0)
        );
        e.pc_ctrl_request(Pc::Pc2, 8, set_leds, &[0x01]);
        assert_eq!(drain(&mut e), [Output::CtrlReply { pc: Pc::Pc2, tag: 8, stall: false, data: CtrlData::new() }]);
        // Switching applies PC2's stored LEDs to the keyboard.
        e.switch(Target::Pc2, SwitchSource::Software);
        let out = drain(&mut e);
        assert!(out.iter().any(|o| matches!(o, Output::DeviceSetReport { port: Port::Keyboard, iface: 0, data, .. } if data.as_slice() == [0x01])));
        // GET_REPORT from the inactive PC is answered from the cache once the active PC has asked.
        let get = [0xA1, 0x01, 0x00, 0x03, iface, 0, 8, 0];
        e.device_ctrl_reply(Pc::Pc2, 9, Port::Keyboard, [0xA1, 0x01, 0x00, 0x03, 0, 0, 8, 0], Some(&[1, 2, 3]));
        drain(&mut e);
        e.pc_ctrl_request(Pc::Pc1, 10, get, &[]);
        assert_eq!(
            drain(&mut e),
            [Output::CtrlReply { pc: Pc::Pc1, tag: 10, stall: false, data: CtrlData::from_slice(&[1, 2, 3]).unwrap() }]
        );
    }

    #[test]
    fn webcam_goes_passthrough_and_follows_the_active_pc() {
        let mut e = engine();
        drain(&mut e);
        let cam =
            Probed { config: Vec::from_slice(crate::usbdesc::fixtures::WEBCAM_CONFIG).unwrap(), ..Default::default() };
        e.port_attached(Port::Aux2, &cam);
        assert_eq!(
            drain(&mut e)[0],
            Output::Route { port: Port::Aux2, route: Route::Pc(Pc::Pc1), power_cycle: false }
        );
        e.port_detached(Port::Aux2); // Hub-0 sees it leave: ignored
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
        e.switch(Target::Toggle, SwitchSource::Key);
        assert!(drain(&mut e).contains(&Output::Route {
            port: Port::Aux2,
            route: Route::Pc(Pc::Pc2),
            power_cycle: false
        }));
        // Pinning it to PC1 moves it back once and then it stays.
        e.set_config(ConfigValue::Route(Device::Aux2, Routing::Pin(Host::Pc1)), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::Route {
            port: Port::Aux2,
            route: Route::Pc(Pc::Pc1),
            power_cycle: false
        }));
        e.switch(Target::Toggle, SwitchSource::Key);
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::Route { .. })));
    }

    #[test]
    fn aux1_is_served_on_a_board_with_it_and_never_routed() {
        // A board before 0.19.0: three ports, six LEDs, no Aux 1 diagnostics.
        let mut e = engine();
        assert!(!e.has_port(Port::Aux1) && !e.led_inputs().aux1);
        let ids: std::vec::Vec<u8> = diag(&mut e).0.iter().map(|i| i.id).collect();
        assert!(ids.contains(&id::A2_SPEED) && !ids.contains(&id::A1_SPEED) && !ids.contains(&id::A1_FAULTS));

        // 0.19.0: the firmware found Aux 1.
        let info = Info { caps: ctrl::caps::AUX1, ..e.info };
        let mut e = Engine::new(Config::default(), info, "TEST0001");
        e.pc_hello(Pc::Pc1, 1);
        e.pc_hello(Pc::Pc2, 1);
        drain(&mut e);
        assert!(e.has_port(Port::Aux1) && e.led_inputs().aux1);
        let ids: std::vec::Vec<u8> = diag(&mut e).0.iter().map(|i| i.id).collect();
        assert!(ids.contains(&id::A1_SPEED) && ids.contains(&id::A1_FAULTS));
        // A webcam there has no router to go to: it stays off, and nothing is routed.
        let cam =
            Probed { config: Vec::from_slice(crate::usbdesc::fixtures::WEBCAM_CONFIG).unwrap(), ..Default::default() };
        e.port_attached(Port::Aux1, &cam);
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::Route { .. })));
        let i = e.port_info(Port::Aux1);
        assert_eq!((i.state, i.effective, i.reason), (PortState::Hosted, PortMode::Off, AutoReason::NeedsPassthrough));
        // The gaming profile passes the other ports through and leaves Aux 1 in Auto.
        e.set_config(ConfigValue::Profile(1), Origin::Software).unwrap();
        drain(&mut e);
        assert_eq!(e.port_info(Port::Aux1).effective, PortMode::Off);
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::Route { port: Port::Aux1, .. })));
        // A keyboard there is mirrored, as on Aux 2.
        e.port_detached(Port::Aux1);
        e.port_attached(Port::Aux1, &keyboard_and_mouse());
        let i = e.port_info(Port::Aux1);
        assert_eq!((i.state, i.effective), (PortState::Hosted, PortMode::Mirrored));
    }

    #[test]
    fn a_high_speed_device_in_passthrough_shows_the_speed_it_runs_at_on_the_pc() {
        // First board, 2026-11-12: a USB 2.0 flash drive in passthrough showed as full speed, Hub-0's speed.
        let mut e = engine();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, true));
        e.pc_status(Pc::Pc2, AtStatus { speed: UsbSpeed::Full, ..usb(UsbState::Configured, true) });
        let webcam = |high_speed| Probed {
            speed: Some(Speed::Full),
            high_speed,
            config: Vec::from_slice(crate::usbdesc::fixtures::WEBCAM_CONFIG).unwrap(),
            ..Default::default()
        };
        e.port_attached(Port::Aux2, &webcam(true));
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
        assert_eq!(e.port_info(Port::Aux2).speed, Speed::High);
        // PC2's hub runs at full speed, and so does everything on it.
        e.switch(Target::Toggle, SwitchSource::Key);
        assert_eq!(e.port_info(Port::Aux2).speed, Speed::Full);
        e.set_config(ConfigValue::Route(Device::Aux2, Routing::Pin(Host::Pc1)), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Aux2).speed, Speed::High, "pinned to PC1");
        // A full-speed device stays at full speed.
        e.port_attached(Port::Aux2, &webcam(false));
        assert_eq!(e.port_info(Port::Aux2).speed, Speed::Full);
        // Hosted, a high-speed device runs at Hub-0's full speed.
        e.port_attached(Port::Mouse, &Probed { high_speed: true, ..keyboard_and_mouse() });
        assert_eq!(e.port_info(Port::Mouse).state, PortState::Hosted);
        assert_eq!(e.port_info(Port::Mouse).speed, Speed::Full);
    }

    #[test]
    fn a_device_unplugged_in_passthrough_leaves_the_port_empty_on_redetect() {
        // First board, 2026-11-12: a flash drive unplugged in passthrough; re-detect left the port probing (blinking
        // yellow) until a device was plugged in again.
        let mut e = engine();
        e.tick(T0);
        let drive =
            Probed { config: Vec::from_slice(crate::usbdesc::fixtures::WEBCAM_CONFIG).unwrap(), ..Default::default() };
        e.port_attached(Port::Aux2, &drive);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
        // Unplugged at the PC's hub, which the KVM can't see; then re-detect.
        assert_eq!(ask(&mut e, Request::Redetect(Port::Aux2)), Response::Ok);
        assert!(drain(&mut e).contains(&Output::Reprobe(Port::Aux2)));
        e.tick(T0 + REPROBE_MS - 1);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Probing);
        e.tick(T0 + REPROBE_MS);
        assert_eq!(e.port_info(Port::Aux2), PortInfo::empty(Port::Aux2, PortMode::Auto));
        assert!(events_to(&drain(&mut e), Pc::Pc1).contains(&Event::PortChanged(Port::Aux2)));
        // A device that is there comes on Hub-0 in time and is probed as usual, however long that takes.
        assert_eq!(ask(&mut e, Request::Redetect(Port::Aux2)), Response::Ok);
        e.tick(T0 + REPROBE_MS + 300);
        e.port_probing(Port::Aux2);
        e.tick(T0 + 3 * REPROBE_MS);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Probing);
        e.port_attached(Port::Aux2, &drive);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
        // Back from passthrough with the device gone: empty as well.
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Passthrough), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        assert_eq!(e.port_info(Port::Mouse).state, PortState::Passthrough);
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Auto), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Mouse).state, PortState::Probing);
        e.tick(T0 + 4 * REPROBE_MS);
        assert_eq!(e.port_info(Port::Mouse).state, PortState::Empty);
    }

    /// Sends a Vial request from `pc` and returns the reply on the Vial endpoint.
    fn vial(e: &mut Engine, pc: Pc, req: &[u8]) -> std::vec::Vec<u8> {
        let f = e.presentation().vial.unwrap();
        e.pc_set_report(pc, f.iface, 2, 0, req);
        drain(e)
            .into_iter()
            .find_map(|o| match o {
                Output::Report { pc: p, ep, data } if p == pc && ep == f.ep_in => Some(data.to_vec()),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn vial_interface_follows_the_setting_and_the_mirrors() {
        use crate::presentation::strings;
        use crate::usbdesc::decode_string;
        use crate::usbdesc::fixtures::XINPUT_CONFIG;
        use kvm_proto::descset::tag;
        let mut e = engine();
        drain(&mut e);
        assert!(e.presentation().vial.is_none());
        e.set_config(ConfigValue::Vial(VialMode::Auto), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::DescriptorSetChanged));
        let set = DescSet::parse(e.descriptor_set()).unwrap();
        let serial = decode_string::<64>(set.find(tag::STRING, strings::SERIAL).unwrap());
        assert_eq!(serial.as_str(), "TEST0001 vial:f64c2b3c");
        // Auto: mirrors come first. A mirrored mouse (2 IN) leaves one endpoint, which Vial takes...
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        assert!(e.presentation().vial.is_some());
        // ...until an XInput pad needs it; unplugging the pad gives it back.
        let pad = Probed { config: Vec::from_slice(XINPUT_CONFIG).unwrap(), ..Default::default() };
        e.port_attached(Port::Aux2, &pad);
        assert_eq!(e.port_info(Port::Aux2).effective, PortMode::Mirrored);
        assert!(e.presentation().vial.is_none());
        e.port_detached(Port::Aux2);
        assert!(e.presentation().vial.is_some());
        // On: Vial keeps its endpoint and the pad goes to passthrough instead.
        e.set_config(ConfigValue::Vial(VialMode::On), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &pad);
        let other = e.port_info(Port::Aux2);
        assert_eq!((other.effective, other.reason), (PortMode::Passthrough, AutoReason::EndpointBudget));
        assert!(e.presentation().vial.is_some());
        e.set_config(ConfigValue::Vial(VialMode::Off), Origin::Software).unwrap();
        assert!(e.presentation().vial.is_none());
    }

    #[test]
    fn vial_remaps_a_normalized_keyboard_and_the_keymap_is_saved() {
        let mut e = engine();
        e.set_config(ConfigValue::Vial(VialMode::On), Origin::Software).unwrap();
        e.set_config(ConfigValue::ProfileKeymap(0, Some(0)), Origin::Software).unwrap();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        let id = vial(&mut e, Pc::Pc2, &[0xFE, 0x00]);
        assert_eq!(&id[..4], &[6, 0, 0, 0]);
        // Caps Lock (row 7, column 1) → Left Ctrl.
        vial(&mut e, Pc::Pc1, &[0x05, 0, 7, 1, 0x00, 0xE0]);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x39, 0, 0, 0, 0, 0], 100);
        let r = reports(&drain(&mut e));
        let nkro = &r.last().unwrap().2;
        assert_eq!((nkro[0], nkro[1]), (normalized::NKRO_ID, 0x01));
        assert_eq!(nkro[2 + 0x39 / 8] & (1 << (0x39 % 8)), 0, "Caps Lock itself is not sent");
        e.device_report(Port::Keyboard, 0, &[0; 8], 150);
        drain(&mut e);
        // Saved 5 s after the last change: the config first, then layer 0.
        e.tick(5000);
        assert!(drain(&mut e).contains(&Output::Save));
        let mut buf = [0u8; SAVE_ITEM_MAX];
        let items: std::vec::Vec<u16> = core::iter::from_fn(|| e.take_save_item(&mut buf).map(|(k, _)| k)).collect();
        assert_eq!(items, [config::ITEM, kvm_map::keymap::item::LAYER]);

        // MT(MOD_LCTL, KC_ESC): nothing until decided; held past the tapping term it is Ctrl.
        vial(&mut e, Pc::Pc1, &[0x05, 0, 7, 1, 0x21, 0x29]);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x39, 0, 0, 0, 0, 0], 6000);
        assert!(reports(&drain(&mut e)).is_empty());
        assert_eq!(e.deadline(), Some(6200));
        e.tick(6200);
        let r = reports(&drain(&mut e));
        assert_eq!(r.last().unwrap().2[1], 0x01);
        e.device_report(Port::Keyboard, 0, &[0; 8], 6300);
        drain(&mut e);

        // Without a keymap in the profile, Caps Lock is Caps Lock again.
        e.set_config(ConfigValue::ProfileKeymap(0, None), Origin::Software).unwrap();
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x39, 0, 0, 0, 0, 0], 6400);
        let r = reports(&drain(&mut e));
        let nkro = &r.last().unwrap().2;
        assert_ne!(nkro[2 + 0x39 / 8] & (1 << (0x39 % 8)), 0);
    }

    #[test]
    fn control_protocol_end_to_end() {
        let mut e = engine();
        drain(&mut e);
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        let ask = |e: &mut Engine, req: Request| {
            let raw = req.encode(3).unwrap();
            match DeviceMsg::decode(&e.control(Asker::Pc(Pc::Pc2), &raw).unwrap()).unwrap() {
                DeviceMsg::Response { id: 3, response, .. } => response,
                other => panic!("{other:?}"),
            }
        };
        let Response::Status(s) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((s.active, s.asking), (Pc::Pc1, Some(Host::Pc2)));
        let Response::Port(p) = ask(&mut e, Request::GetPort(Port::Keyboard)) else { panic!() };
        assert_eq!(
            (p.vid, p.state, p.effective, p.reason),
            (0x1234, PortState::Hosted, PortMode::Normalized, AutoReason::FitsNormalized)
        );
        assert_eq!(
            ask(&mut e, Request::GetPortString(Port::Keyboard, StringKind::Product)),
            Response::PortString(Port::Keyboard, StringKind::Product, ctrl::short_string("Combo"))
        );
        assert_eq!(ask(&mut e, Request::SetActive(Target::Toggle)), Response::Ok);
        assert_eq!(e.active(), Pc::Pc2);
        let hk = ConfigValue::Hotkey(
            1,
            ctrl::HotkeyBinding::new(HotkeyAction::SelectHost(Host::Pc1), Hotkey::combo(0x05, 0x1E)),
        );
        assert_eq!(ask(&mut e, Request::SetConfig(hk)), Response::Config(hk));
        assert_eq!(
            ask(&mut e, Request::SetConfig(ConfigValue::SwSwitchAllowed(false))),
            Response::Config(ConfigValue::SwSwitchAllowed(false))
        );
        assert_eq!(ask(&mut e, Request::SetActive(Target::Pc1)), Response::Error(ErrorCode::Locked));
        assert_eq!(
            ask(&mut e, Request::SetConfig(ConfigValue::SwSwitchAllowed(true))),
            Response::Error(ErrorCode::NeedsKeyPress)
        );
        // Ctrl+Alt+1 still works from the keyboard.
        e.device_report(Port::Keyboard, 0, &[0x05, 0, 0x1E, 0, 0, 0, 0, 0], 100);
        assert_eq!(e.active(), Pc::Pc1);
        e.tick(5200);
        assert!(drain(&mut e).contains(&Output::Save));
    }

    #[test]
    fn mirroring_keyboard_and_mouse_takes_the_kvms_own_endpoints() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        // 2 + 2 IN endpoints don't fit EP5–EP7: the KVM's own keyboard, mouse and media interfaces make way.
        for port in [Port::Keyboard, Port::Mouse] {
            let p = e.port_info(port);
            assert_eq!((p.effective, p.reason), (PortMode::Mirrored, AutoReason::NotAuto));
        }
        let pres = e.presentation();
        assert_eq!((pres.boot_keyboard.ep_in, pres.mouse.ep_in, pres.nkro.ep_in), (0, 0, 0));
        let eps: std::vec::Vec<_> = pres.mirrors.iter().map(|m| m.f.ep_in).collect();
        assert_eq!(eps, [0x85, 0x86, 0x87, 0x81]);
        let set = DescSet::parse(e.descriptor_set()).unwrap();
        assert_eq!(crate::usbdesc::parse_config(set.config().unwrap()).unwrap().interfaces.len(), 5);
        // The keyboard back to Normalized: the interfaces return, and the mouse alone fits EP5–EP7.
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Normalized), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Mouse).effective, PortMode::Mirrored);
        assert_eq!(e.presentation().boot_keyboard.ep_in, 0x81);
        let eps: std::vec::Vec<_> = e.presentation().mirrors.iter().map(|m| m.f.ep_in).collect();
        assert_eq!(eps, [0x85, 0x86]);
    }

    #[test]
    fn a_mirror_squeezed_out_comes_back_when_the_endpoints_free_up() {
        // A normalized device on Aux 2 keeps the KVM's own interfaces: two 2-endpoint mirrors don't both fit.
        let mut e = engine();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &keyboard_and_mouse());
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        let mouse = e.port_info(Port::Mouse);
        assert_eq!((mouse.effective, mouse.reason), (PortMode::Normalized, AutoReason::EndpointBudget));
        // The keyboard lets go of its mirror: the mouse gets it.
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Normalized), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Mouse).effective, PortMode::Mirrored);
        // Back to Mirrored: the mouse keeps its endpoints, the keyboard is the one left out.
        drain(&mut e);
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Keyboard).reason, AutoReason::EndpointBudget);
        // Unplugging the normalized device on Aux 2 makes room for both.
        e.port_detached(Port::Aux2);
        assert_eq!(e.port_info(Port::Keyboard).effective, PortMode::Mirrored);
        assert_eq!(e.presentation().mirrors.len(), 4);
    }

    #[test]
    fn identity_stays_with_the_port_that_has_it() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::MirroredIdentity), Origin::Software).unwrap();
        assert_eq!(e.presentation().identity, Some(Port::Mouse));
        // The keyboard asks for identity too: one VID/PID per device, and the mouse had it first.
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::MirroredIdentity), Origin::Software).unwrap();
        let kb = e.port_info(Port::Keyboard);
        assert_eq!((kb.effective, kb.reason), (PortMode::Mirrored, AutoReason::IdentityTaken));
        assert_eq!(e.port_info(Port::Mouse).effective, PortMode::MirroredIdentity);
        assert_eq!(e.presentation().identity, Some(Port::Mouse));
        // The mouse lets go of it: the keyboard gets it.
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Keyboard).effective, PortMode::MirroredIdentity);
        assert_eq!(e.presentation().identity, Some(Port::Keyboard));
    }

    #[test]
    fn a_long_press_moves_every_port_to_the_next_profile() {
        let mut e = settled();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Configured, false));
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        drain(&mut e);
        assert_eq!(e.port_info(Port::Keyboard).effective, PortMode::Normalized);

        // Profile 1 ("gaming") runs every port in passthrough: both ports go to the active PC in one step.
        e.key(KeyEvent::Long);
        assert_eq!(e.config().profile, 1);
        let out = drain(&mut e);
        for port in [Port::Keyboard, Port::Mouse] {
            assert!(out.contains(&Output::Route { port, route: Route::Pc(Pc::Pc1), power_cycle: false }));
            assert_eq!(e.port_info(port).mode, PortMode::Passthrough);
        }
        let changed = Event::ConfigChanged(ctrl::key::PROFILE).encode().unwrap();
        assert!(out.iter().any(|o| matches!(o, Output::Report { ep: 0x84, data, .. } if data.as_slice() == changed)));
        assert_eq!(e.led_inputs().profile, Some(1));
        e.tick(T0 + PROFILE_SHOW_MS);
        assert_eq!(e.led_inputs().profile, None);
        // A tap still switches PCs, and the passthrough ports follow.
        e.key(KeyEvent::Tap);
        assert_eq!(e.active(), Pc::Pc2);
        drain(&mut e);

        // Back to profile 0: the KVM has to see both devices again first.
        e.key(KeyEvent::Long);
        assert_eq!(e.config().profile, 0);
        let out = drain(&mut e);
        assert!(out.contains(&Output::Reprobe(Port::Keyboard)) && out.contains(&Output::Reprobe(Port::Mouse)));
        assert_eq!(e.port_info(Port::Keyboard).state, PortState::Probing);
    }

    #[test]
    fn a_profile_change_writes_flash_only_when_the_profile_is_remembered() {
        let mut e = settled();
        assert!(!e.config().remember_profile, "off by default");
        e.key(KeyEvent::Long);
        assert_eq!(e.config().profile, 1);
        e.tick(T0 + AUTOSAVE_MS);
        assert!(!drain(&mut e).contains(&Output::Save));
        assert_eq!(e.status(None).flags & status_flags::CONFIG_DIRTY, 0);
        e.set_config(ConfigValue::RememberProfile(true), Origin::Software).unwrap();
        e.tick(T0 + 2 * AUTOSAVE_MS);
        assert!(drain(&mut e).contains(&Output::Save), "the setting itself is saved");
        // (The KVM is in standby by now, where the key only wakes it: from software.)
        e.set_config(ConfigValue::Profile(0), Origin::Software).unwrap();
        e.tick(T0 + 3 * AUTOSAVE_MS);
        assert!(drain(&mut e).contains(&Output::Save), "and now each change of profile");
    }

    /// What the firmware's storage task does with [`Output::Save`]: takes the items, then reports back. Returns the
    /// saves among `out`.
    fn firmware_saves(e: &mut Engine) -> usize {
        let saves = drain(e).iter().filter(|o| matches!(o, Output::Save)).count();
        if saves > 0 {
            let mut buf = [0u8; SAVE_ITEM_MAX];
            while e.take_save_item(&mut buf).is_some() {}
            e.config_saved();
        }
        saves
    }

    fn unsaved(e: &Engine) -> bool {
        e.status(None).flags & status_flags::CONFIG_DIRTY != 0
    }

    #[test]
    fn saves_asked_for_in_a_loop_go_ahead_up_to_a_burst_then_once_a_minute() {
        let mut e = settled();
        let mut saves = 0;
        for k in 0..u32::from(SAVE_BURST) + 10 {
            e.tick(T0 + 10 * k);
            e.set_config(ConfigValue::LedBrightness((k % 200) as u8 + 1), Origin::Software).unwrap();
            assert_eq!(ask(&mut e, Request::SaveConfig), Response::Ok);
            saves += firmware_saves(&mut e);
        }
        assert_eq!(saves, usize::from(SAVE_BURST));
        assert!(unsaved(&e), "the last change waits, and the PCs can see it");
        assert_eq!(e.deadline(), Some(T0 + SAVE_PACE_MS));
        e.tick(T0 + SAVE_PACE_MS - 1);
        assert_eq!(firmware_saves(&mut e), 0);
        e.tick(T0 + SAVE_PACE_MS);
        assert_eq!(firmware_saves(&mut e), 1);
        assert!(!unsaved(&e));
        // Left alone, the allowance comes back in full.
        e.tick(T0 + (u32::from(SAVE_BURST) + 2) * SAVE_PACE_MS);
        e.set_config(ConfigValue::LedBrightness(7), Origin::Software).unwrap();
        assert_eq!(ask(&mut e, Request::SaveConfig), Response::Ok);
        assert_eq!(firmware_saves(&mut e), 1);
    }

    #[test]
    fn a_script_changing_a_setting_every_6_s_is_saved_once_a_minute() {
        let mut e = settled();
        let (hours, every) = (3u32, 6_000u32);
        let mut saves = 0;
        for k in 0..hours * 3_600_000 / every {
            let t = T0 + k * every;
            e.tick(t);
            e.set_config(ConfigValue::LedBrightness((k % 200) as u8 + 1), Origin::Software).unwrap();
            saves += firmware_saves(&mut e);
            // Quiet long enough for the autosave.
            e.tick(t + AUTOSAVE_MS + 500);
            saves += firmware_saves(&mut e);
        }
        let paced = usize::from(SAVE_BURST) + (hours * 60) as usize;
        assert!((paced - 5..=paced + 1).contains(&saves), "{saves} saves; unpaced it would be 1800");
        // When the script stops, its last value is saved.
        let end = T0 + hours * 3_600_000;
        e.tick(end + SAVE_PACE_MS);
        assert_eq!(firmware_saves(&mut e), 1);
        assert!(!unsaved(&e));
    }

    #[test]
    fn a_change_made_while_the_firmware_writes_is_saved_too() {
        let mut e = settled();
        e.set_config(ConfigValue::LedBrightness(10), Origin::Software).unwrap();
        e.tick(T0 + AUTOSAVE_MS);
        assert!(drain(&mut e).contains(&Output::Save));
        let mut buf = [0u8; SAVE_ITEM_MAX];
        while e.take_save_item(&mut buf).is_some() {}
        // The firmware is still writing when the next change comes.
        e.set_config(ConfigValue::LedBrightness(20), Origin::Software).unwrap();
        e.config_saved();
        assert!(unsaved(&e));
        e.tick(T0 + 2 * AUTOSAVE_MS);
        assert_eq!(firmware_saves(&mut e), 1);
        assert!(!unsaved(&e));
    }

    #[test]
    fn the_keys_actions_follow_the_config() {
        let mut e = settled();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Configured, false));
        e.set_config(ConfigValue::KeyTap(KeyAction::NextProfile), Origin::Software).unwrap();
        e.set_config(ConfigValue::KeyLong(KeyAction::Aux2PortMode), Origin::Software).unwrap();
        e.key(KeyEvent::Tap);
        assert_eq!((e.config().profile, e.active()), (1, Pc::Pc1));
        // The long press changes the Aux 2 port's mode in the active profile only.
        e.key(KeyEvent::Long);
        let other = Device::Aux2.index();
        let p = e.config().profiles;
        assert_eq!((p[1].modes[other], p[1].routes[other]), (PortMode::Passthrough, Routing::Pin(Host::Pc1)));
        assert_eq!((p[0].modes[other], p[0].routes[other]), (PortMode::Auto, Routing::Follow));
        e.set_config(ConfigValue::KeyTap(KeyAction::None), Origin::Software).unwrap();
        e.key(KeyEvent::Tap);
        assert_eq!((e.config().profile, e.active()), (1, Pc::Pc1));
    }

    /// A control request from PC1; its response.
    fn ask(e: &mut Engine, req: Request) -> Response {
        ask_from(e, Some(Pc::Pc1), req)
    }

    fn ask_from(e: &mut Engine, pc: Option<Pc>, req: Request) -> Response {
        let raw = req.encode(7).unwrap();
        match DeviceMsg::decode(&e.control(pc.map_or(Asker::Service, Asker::Pc), &raw).unwrap()).unwrap() {
            DeviceMsg::Response { response, .. } => response,
            other => panic!("{other:?}"),
        }
    }

    /// The events `out` sends to `pc`.
    fn events_to(out: &[Output], pc: Pc) -> std::vec::Vec<Event> {
        out.iter()
            .filter_map(|o| match o {
                Output::Report { pc: p, ep: 0x84, data } if *p == pc => match DeviceMsg::decode(data) {
                    Ok(DeviceMsg::Event(e)) => Some(e),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// The keys and modifiers the last keyboard report in `out` holds down, if it sends one.
    fn typed(out: &[Output]) -> Option<std::vec::Vec<u8>> {
        reports(out).iter().rev().find(|(_, ep, d)| *ep == 0x83 && d[0] == normalized::NKRO_ID).map(|(_, _, d)| {
            let mut held: std::vec::Vec<u8> = (0..8).filter(|b| d[1] & (1 << b) != 0).map(|b| 0xE0 + b).collect();
            held.extend((0..0xE0u8).filter(|k| d[2 + *k as usize / 8] & (1 << (k % 8)) != 0));
            held
        })
    }

    // ── Bluetooth hosts (software.md §4.13) ──────────────────────────────────

    fn bt_host(n: u8, flags: u8) -> BtHostState {
        BtHostState { host: n, flags, interval: 12, latency: 0, addr: [0, 1, 2, 3, 4, 5, n], rssi: -55, reports: 0 }
    }

    const BT_READY: u8 = kvm_proto::link::bt::host_flags::BONDED
        | kvm_proto::link::bt::host_flags::CONNECTED
        | kvm_proto::link::bt::host_flags::ENCRYPTED
        | kvm_proto::link::bt::host_flags::SUBSCRIBED;

    /// An engine with a keyboard + mouse on the Keyboard port and the module up, host 2 ready, host 3 bonded only.
    fn with_bt() -> Engine {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.bt_hello(0x000100);
        e.bt_host(bt_host(2, BT_READY));
        e.bt_host(bt_host(3, kvm_proto::link::bt::host_flags::BONDED));
        drain(&mut e);
        e
    }

    /// A control request from Bluetooth host `n` over the module's control service; its response, as the module would
    /// notify it.
    fn ask_bt(e: &mut Engine, n: u8, req: Request) -> Response {
        let raw = req.encode(9).unwrap();
        e.bt_control(n, btcontrol::trim(&raw), T0);
        let out = drain(e);
        let mut answers = out.iter().filter_map(|o| match o {
            Output::Bt(BtOut::Control { host, data }) if *host == n => match DeviceMsg::decode(data).unwrap() {
                DeviceMsg::Response { id: 9, response, .. } => Some(response),
                _ => None,
            },
            _ => None,
        });
        let r = answers.next().expect("an answer");
        assert!(answers.next().is_none());
        r
    }

    /// The events `out` sends to Bluetooth host `n`'s control session.
    fn bt_events(out: &[Output], n: u8) -> std::vec::Vec<Event> {
        out.iter()
            .filter_map(|o| match o {
                Output::Bt(BtOut::Control { host, data }) if *host == n => match DeviceMsg::decode(data).ok()? {
                    DeviceMsg::Event(e) => Some(e),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    const BT_SESSION: u8 = BT_READY | kvm_proto::link::bt::host_flags::CONTROL;

    #[test]
    fn a_bluetooth_host_controls_the_kvm_only_once_the_key_allowed_it() {
        let mut e = with_bt();
        e.bt_host(bt_host(2, BT_SESSION));
        drain(&mut e);
        // Not allowed yet: its identity, to see that it speaks this protocol, and nothing else.
        let Response::Info(info) = ask_bt(&mut e, 2, Request::GetInfo) else { panic!() };
        assert_eq!(info.proto, ctrl::PROTOCOL_VERSION);
        assert_eq!(ask_bt(&mut e, 2, Request::GetStatus), Response::Error(ErrorCode::NotAllowed));
        assert_eq!(ask_bt(&mut e, 2, Request::SetActive(Target::Pc2)), Response::Error(ErrorCode::NotAllowed));
        // It can't allow itself by setting the value.
        let on = ConfigValue::BtControl(1, true);
        assert_eq!(ask_bt(&mut e, 2, Request::SetConfig(on)), Response::Error(ErrorCode::NotAllowed));
        // It asks; the KVM waits for its key, and the tap allows it.
        assert_eq!(ask_bt(&mut e, 2, Request::AskControl), Response::Error(ErrorCode::NeedsKeyPress));
        assert!(!e.config().bt_control[1]);
        e.key(KeyEvent::Tap);
        assert!(e.config().bt_control[1]);
        // A host that may hears of the change, as the PCs do.
        let out = drain(&mut e);
        assert!(bt_events(&out, 2).contains(&Event::ConfigChanged(ctrl::key::BT_CONTROL_BASE + 1)));
        assert!(events_to(&out, Pc::Pc1).contains(&Event::ConfigChanged(ctrl::key::BT_CONTROL_BASE + 1)));
        assert_eq!(ask_bt(&mut e, 2, Request::AskControl), Response::Ok);
        let Response::Status(st) = ask_bt(&mut e, 2, Request::GetStatus) else { panic!() };
        assert_eq!((st.asking, st.asking_bt(), st.asking_pc()), (Some(Host::Bt2), Some(2), None));
        assert_eq!(ask_bt(&mut e, 2, Request::SetActive(Target::Pc2)), Response::Ok);
        assert_eq!(e.active(), Pc::Pc2);
        // AskControl is for Bluetooth hosts only.
        assert_eq!(ask(&mut e, Request::AskControl), Response::Error(ErrorCode::Unsupported));
    }

    #[test]
    fn a_bluetooth_host_gets_neither_firmware_nor_what_streams_to_a_pc() {
        let mut e = with_bt();
        e.bt_host(bt_host(2, BT_SESSION));
        e.set_config(ConfigValue::BtControl(1, true), Origin::Device).unwrap();
        drain(&mut e);
        let refused = [
            (Request::FwBegin { target: FwTarget::Rp2354, size: 4096, crc32: 0 }, ErrorCode::Unsupported),
            (Request::FwCommit, ErrorCode::Unsupported),
            (Request::Reboot(RebootTarget::Bootloader), ErrorCode::Unsupported),
            (Request::LearnHotkey(true), ErrorCode::NotActive),
            (Request::WatchControls(1), ErrorCode::NotActive),
            (Request::MonitorDevice(Device::Keyboard, true), ErrorCode::NotActive),
            (Request::PointerAt([10, 10]), ErrorCode::NotActive),
        ];
        for (req, code) in refused {
            assert_eq!(ask_bt(&mut e, 2, req.clone()), Response::Error(code), "{req:?}");
        }
        // Stopping what it can't start is no error, and stops nobody else's.
        assert_eq!(ask(&mut e, Request::WatchControls(1)), Response::Ok);
        assert_eq!(ask_bt(&mut e, 2, Request::WatchControls(0)), Response::Ok);
        assert!(e.watch.is_some(), "PC1's watch goes on");
        assert!(matches!(ask_bt(&mut e, 2, Request::FwStatus), Response::Fw { .. }));
    }

    #[test]
    fn events_reach_only_the_bluetooth_hosts_that_may_and_listen() {
        let mut e = with_bt();
        e.bt_host(bt_host(2, BT_SESSION));
        e.bt_host(bt_host(3, kvm_proto::link::bt::host_flags::BONDED | kvm_proto::link::bt::host_flags::CONTROL));
        e.set_config(ConfigValue::BtControl(1, true), Origin::Device).unwrap();
        drain(&mut e);
        e.set_config(ConfigValue::DoubleTapMs(400), Origin::Software).unwrap();
        let out = drain(&mut e);
        assert_eq!(bt_events(&out, 2), [Event::ConfigChanged(ctrl::key::DOUBLE_TAP_MS)]);
        assert!(bt_events(&out, 3).is_empty(), "host 3 may not control the KVM");
        // Without its session (its software left), nothing goes to host 2 either.
        e.bt_host(bt_host(2, BT_READY));
        drain(&mut e);
        e.set_config(ConfigValue::DoubleTapMs(300), Origin::Software).unwrap();
        assert!(bt_events(&drain(&mut e), 2).is_empty());
    }

    #[test]
    fn a_slot_without_its_bond_may_not_control_the_kvm_any_more() {
        let mut e = with_bt();
        e.set_config(ConfigValue::BtControl(1, true), Origin::Device).unwrap();
        // Still bonded: it keeps it across states.
        e.bt_host(bt_host(2, kvm_proto::link::bt::host_flags::BONDED));
        assert!(e.config().bt_control[1]);
        // Forgotten: the next host to pair into slot 2 starts without.
        e.bt_host(bt_host(2, 0));
        assert!(!e.config().bt_control[1]);
        // A free slot can't be allowed, from a PC either.
        let on = ConfigValue::BtControl(1, true);
        assert_eq!(ask(&mut e, Request::SetConfig(on)), Response::Error(ErrorCode::NoDevice));
    }

    #[test]
    fn a_pc_allows_a_host_with_the_key_and_one_tap_grants_one_thing() {
        let mut e = with_bt();
        let on = ConfigValue::BtControl(1, true);
        assert_eq!(ask(&mut e, Request::SetConfig(on)), Response::Error(ErrorCode::NeedsKeyPress));
        // While host 2 waits, neither the locks nor another host can share the tap.
        assert_eq!(
            ask(&mut e, Request::SetConfig(ConfigValue::SwConfigAllowed(true))),
            Response::Config(ConfigValue::SwConfigAllowed(true)),
            "already on: nothing to wait for"
        );
        e.set_config(ConfigValue::SwSwitchAllowed(false), Origin::Software).unwrap();
        assert_eq!(ask(&mut e, Request::SetConfig(ConfigValue::SwSwitchAllowed(true))), Response::Error(ErrorCode::Busy));
        assert_eq!(
            ask(&mut e, Request::SetConfig(ConfigValue::BtControl(2, true))),
            Response::Error(ErrorCode::Busy)
        );
        e.key(KeyEvent::Tap);
        assert!(e.config().bt_control[1]);
        assert!(!e.config().sw_switch_allowed);
        // Off at once, like any other setting.
        let off = ConfigValue::BtControl(1, false);
        assert_eq!(ask(&mut e, Request::SetConfig(off)), Response::Config(off));
        assert!(!e.config().bt_control[1]);
        // Nobody tapped: it stays off.
        assert_eq!(ask(&mut e, Request::SetConfig(on)), Response::Error(ErrorCode::NeedsKeyPress));
        e.tick(T0 + UNLOCK_WAIT_MS + 1000);
        e.key(KeyEvent::Tap);
        assert!(!e.config().bt_control[1]);
    }

    fn bt_inputs(out: &[Output]) -> std::vec::Vec<(u8, u8, std::vec::Vec<u8>)> {
        out.iter()
            .filter_map(|o| match o {
                Output::Bt(BtOut::Input { host, id, data }) => Some((*host, *id, data.to_vec())),
                _ => None,
            })
            .collect()
    }

    /// Sets entry `i` of the hotkey list to `action` on Ctrl+Alt+`key`.
    fn bind(e: &mut Engine, i: u8, action: HotkeyAction, key: u8) {
        let v = ConfigValue::Hotkey(i, ctrl::HotkeyBinding::new(action, Hotkey::combo(0x05, key)));
        assert_eq!(ask(e, Request::SetConfig(v)), Response::Config(v));
    }

    /// Ctrl+Alt+`key` on the KVM's keyboard, pressed and released.
    fn chord(e: &mut Engine, key: u8, t: u32) {
        e.device_report(Port::Keyboard, 0, &[0x05, 0, key, 0, 0, 0, 0, 0], t);
        e.device_report(Port::Keyboard, 0, &[0; 8], t + 10);
    }

    #[test]
    fn hotkeys_select_hosts_and_cycle_them_both_ways() {
        // Host 2 ready, host 3 bonded only.
        let mut e = with_bt();
        bind(&mut e, 1, HotkeyAction::SelectHost(Host::Bt2), 0x1F);
        bind(&mut e, 2, HotkeyAction::SelectHost(Host::Bt3), 0x20);
        bind(&mut e, 3, HotkeyAction::SelectHost(Host::Bt1), 0x1E);
        bind(&mut e, 4, HotkeyAction::NextHost, 0x11);
        bind(&mut e, 5, HotkeyAction::PrevHost, 0x13);
        chord(&mut e, 0x1F, 100);
        assert_eq!((e.active(), e.bt_active()), (Pc::Pc1, Some(2)));
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::Select { host: 2, pinned: 0 })));
        // Host 3 is bonded but not connected: selected all the same, as from the app.
        chord(&mut e, 0x20, 200);
        assert_eq!(e.bt_active(), Some(3));
        // Slot 1 has no host: the input stays where it is.
        chord(&mut e, 0x1E, 300);
        assert_eq!(e.bt_active(), Some(3));
        // The hotkeys' keys reached no host.
        assert!(bt_inputs(&drain(&mut e)).iter().all(|(_, _, d)| !d.contains(&0x1E) && !d.contains(&0x20)));
        // Previous from host 3: the ready host 2 (3 isn't ready), PC2, PC1, and round to host 2 again.
        let mut seen = std::vec::Vec::new();
        for t in 0..4 {
            chord(&mut e, 0x13, 400 + t * 50);
            seen.push((e.active(), e.bt_active()));
        }
        assert_eq!(seen, [(Pc::Pc1, Some(2)), (Pc::Pc2, None), (Pc::Pc1, None), (Pc::Pc1, Some(2))]);
        // Next from host 2: PC1, PC2, host 2.
        let mut seen = std::vec::Vec::new();
        for t in 0..3 {
            chord(&mut e, 0x11, 700 + t * 50);
            seen.push((e.active(), e.bt_active()));
        }
        assert_eq!(seen, [(Pc::Pc1, None), (Pc::Pc2, None), (Pc::Pc2, Some(2))]);
    }

    #[test]
    fn hotkeys_select_and_cycle_profiles_and_the_other_ports_mode() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.set_config(ConfigValue::ProfileCount(3), Origin::Software).unwrap();
        // Software configuration locked: like the KVM's key, the hotkeys still work.
        e.set_config(ConfigValue::SwConfigAllowed(false), Origin::Software).unwrap();
        for (i, (action, key)) in [
            (HotkeyAction::NextProfile, 0x11),
            (HotkeyAction::PrevProfile, 0x13),
            (HotkeyAction::SelectProfile(2), 0x1F),
            (HotkeyAction::SelectProfile(3), 0x20),
            (HotkeyAction::Aux2PortMode, 0x12),
        ]
        .into_iter()
        .enumerate()
        {
            let v = ConfigValue::Hotkey(i as u8 + 1, ctrl::HotkeyBinding::new(action, Hotkey::combo(0x05, key)));
            e.set_config(v, Origin::Device).unwrap();
        }
        drain(&mut e);
        let mut t = 100;
        let mut press = |e: &mut Engine, key: u8| {
            chord(e, key, t);
            t += 50;
            e.config().profile
        };
        assert_eq!(press(&mut e, 0x13), 2, "previous from 0: the last in use");
        assert_eq!(press(&mut e, 0x11), 0, "next from the last: the first");
        assert_eq!(press(&mut e, 0x1F), 2);
        assert_eq!(press(&mut e, 0x20), 2, "profile 3 isn't in use");
        press(&mut e, 0x12);
        assert_eq!(e.config().mode(Device::Aux2), PortMode::Passthrough);
    }

    #[test]
    fn without_the_module_there_are_no_bluetooth_targets() {
        let mut e = engine();
        drain(&mut e);
        assert_eq!(ask(&mut e, Request::SetActive(Target::Bt1)), Response::Error(ErrorCode::Unsupported));
        assert_eq!(ask(&mut e, Request::BtPair(true)), Response::Error(ErrorCode::Unsupported));
        let Response::Info(i) = ask(&mut e, Request::GetInfo) else { panic!() };
        assert_eq!(i.caps & ctrl::caps::BLUETOOTH, 0);
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_active, st.bt_ready, st.bt_bonded), (0, 0, 0));
        // Next cycles the two PCs only.
        e.switch(Target::Next, SwitchSource::Hotkey);
        assert_eq!((e.active(), e.bt_active()), (Pc::Pc2, None));
        e.switch(Target::Next, SwitchSource::Hotkey);
        assert_eq!((e.active(), e.bt_active()), (Pc::Pc1, None));
        assert!(drain(&mut e).iter().all(|o| !matches!(o, Output::Bt(_))));
    }

    #[test]
    fn a_host_whose_settings_the_module_holds_in_ram_is_a_fault_and_a_warning() {
        let mut e = with_bt();
        e.set_bt_module(BtModule::Up);
        let held = |e: &mut Engine| {
            let (items, _) = diag(e);
            items.into_iter().find(|i| i.id == id::BT_SETTINGS_HELD).map(|i| (i.grade, i.value))
        };
        assert_eq!(held(&mut e), None, "only while there are any");
        e.bt_event(BtEventKind::SettingsHeld, 2, 0);
        let out = drain(&mut e);
        for pc in [Pc::Pc1, Pc::Pc2] {
            assert!(events_to(&out, pc).contains(&Event::Fault(FaultKind::BtSettingsHeld, None)));
        }
        assert_eq!(held(&mut e), Some((Grade::Warn, 1)));
        // Told again (the module answers every `RequestState`): no second fault.
        e.bt_event(BtEventKind::SettingsHeld, 2, 0);
        assert!(!events_to(&drain(&mut e), Pc::Pc1).iter().any(|ev| matches!(ev, Event::Fault(..))));
        // A module that says hello anew tells what it holds again; until then nothing is held.
        e.bt_hello(0x000100);
        assert_eq!(held(&mut e), None);
        // Out of range: ignored.
        e.bt_event(BtEventKind::SettingsHeld, 9, 0);
        assert_eq!(held(&mut e), None);
    }

    #[test]
    fn the_diagnostics_show_the_module_only_where_one_is_fitted() {
        let bt_items = |e: &mut Engine| {
            let (items, _) = diag(e);
            items
                .into_iter()
                .filter(|i| cdiag::Group::of(i.id) == Some(cdiag::Group::Bluetooth))
                .collect::<std::vec::Vec<_>>()
        };
        let mut plain = engine();
        plain.set_board_diag(BoardDiag::TYPICAL);
        assert!(bt_items(&mut plain).is_empty(), "no module: no Bluetooth items");

        let mut e = with_bt();
        e.set_bt_module(BtModule::Up);
        e.set_board_diag(BoardDiag { bt_link_errors: 3, ..BoardDiag::TYPICAL });
        let items = bt_items(&mut e);
        let item = |id: u8| items.iter().find(|i| i.id == id).map(|i| (i.grade, i.value));
        assert_eq!(item(id::BT_MODULE), Some((Grade::Ok, BtModule::Up as i32)));
        assert_eq!(item(id::BT_FW), Some((Grade::Info, 0x000100)));
        assert_eq!(item(id::BT_LINK_ERRORS), Some((Grade::Info, 3)));
        assert_eq!(item(id::BT_HOSTS), Some((Grade::Info, 1)), "host 2 is ready, host 3 only bonded");

        let Response::Info(i) = ask(&mut e, Request::GetInfo) else { panic!() };
        assert_eq!(i.bt_fw, 0x000100);
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!(st.bt_module, BtModule::Up);

        // Fitted but on another firmware: amber, and `Status` says so, though there is no Bluetooth (no `Hello`, no
        // capability).
        let mut silent = engine();
        silent.set_bt_module(BtModule::NeedsUpdate);
        let items = bt_items(&mut silent);
        assert_eq!(items.iter().find(|i| i.id == id::BT_MODULE).map(|i| i.grade), Some(Grade::Warn));
        let Response::Info(i) = ask(&mut silent, Request::GetInfo) else { panic!() };
        assert_eq!((i.caps & ctrl::caps::BLUETOOTH, i.bt_fw), (0, 0));
        let Response::Status(st) = ask(&mut silent, Request::GetStatus) else { panic!() };
        assert_eq!(st.bt_module, BtModule::NeedsUpdate);
        // It still draws its current.
        let pwr = PowerInfo { source: PowerSource::PwrPort, adv_pwr: Advertised::A3_0, vbus_pwr_mv: 5100 };
        plain.set_power(pwr);
        silent.set_power(pwr);
        let budget = |e: &Engine| e.status(None).budget_ma;
        assert_eq!(budget(&plain) - budget(&silent), BT_CURRENT_MA);
    }

    /// An ESP32-C3 board's USB serial port, as Hub-0 probes it.
    fn esp_board() -> Probed {
        use crate::usbdesc::fixtures::{USB_SERIAL_JTAG_CONFIG, USB_SERIAL_JTAG_DEVICE};
        Probed {
            device: DeviceDesc::parse(&USB_SERIAL_JTAG_DEVICE).unwrap(),
            speed: Some(Speed::Full),
            config: Vec::from_slice(USB_SERIAL_JTAG_CONFIG).unwrap(),
            ..Default::default()
        }
    }

    #[test]
    fn an_esp32_board_on_a_kvm_without_the_module_becomes_the_module() {
        let mut e = engine();
        drain(&mut e);
        e.port_attached(Port::Aux2, &esp_board());
        // Probing while the firmware finds out; nothing routed away meanwhile.
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Probing);
        assert!(e.dongle_wanted(Port::Aux2));
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::Route { .. })));
        e.dongle_claimed(Port::Aux2);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Module);
        assert_eq!(e.dongle_port(), Some(Port::Aux2));
        assert_eq!(e.led_inputs().ports[Port::Aux2.index()].0, PortState::Module);
        e.set_bt_module(BtModule::Up);
        e.bt_hello(0x000100);
        let Response::Info(i) = ask(&mut e, Request::GetInfo) else { panic!() };
        assert_ne!(i.caps & ctrl::caps::BLUETOOTH, 0);
        // Its current comes from its port: the KVM's own draw stays that of a board without the module.
        let pwr = PowerInfo { source: PowerSource::PwrPort, adv_pwr: Advertised::A3_0, vbus_pwr_mv: 5100 };
        let mut plain = engine();
        plain.set_power(pwr);
        e.set_power(pwr);
        assert_eq!(e.status(None).budget_ma, plain.status(None).budget_ma);
        // A second board while it is there: an ordinary device.
        e.port_attached(Port::Mouse, &esp_board());
        assert!(!e.dongle_wanted(Port::Mouse));
        assert_eq!(e.port_info(Port::Mouse).state, PortState::Passthrough);
        // Unplugged: the port is empty, Bluetooth is gone, and the next board may be the module.
        e.port_detached(Port::Aux2);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Empty);
        assert_eq!(e.status(None).bt_module, BtModule::Absent);
        let Response::Info(i) = ask(&mut e, Request::GetInfo) else { panic!() };
        assert_eq!(i.caps & ctrl::caps::BLUETOOTH, 0);
        e.port_attached(Port::Aux2, &esp_board());
        assert!(e.dongle_wanted(Port::Aux2));
    }

    #[test]
    fn an_esp32_board_that_is_not_the_module_is_an_ordinary_device() {
        // Another firmware: declined, then decided as any device (a non-HID device goes to the active PC).
        let mut e = engine();
        drain(&mut e);
        e.port_attached(Port::Aux2, &esp_board());
        e.dongle_declined(Port::Aux2);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
        assert!(drain(&mut e).contains(&Output::Route {
            port: Port::Aux2,
            route: Route::Pc(Pc::Pc1),
            power_cycle: false
        }));
        // A KVM with the module on board never takes one.
        let mut fitted = engine();
        fitted.set_bt_fitted(true);
        fitted.port_attached(Port::Aux2, &esp_board());
        assert!(!fitted.dongle_wanted(Port::Aux2));
        assert_eq!(fitted.port_info(Port::Aux2).state, PortState::Passthrough);
        // Nor a port set to passthrough: one's own ESP32 board stays reachable from a PC.
        let mut pinned = engine();
        pinned.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Passthrough), Origin::Software).unwrap();
        pinned.port_attached(Port::Aux2, &esp_board());
        assert!(!pinned.dongle_wanted(Port::Aux2));
        // Setting the module's port to passthrough plugs it in anew, as the ordinary device it then is.
        let mut e = engine();
        e.port_attached(Port::Aux2, &esp_board());
        e.dongle_claimed(Port::Aux2);
        drain(&mut e);
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Passthrough), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::Reprobe(Port::Aux2)));
        e.port_detached(Port::Aux2);
        e.port_attached(Port::Aux2, &esp_board());
        assert!(!e.dongle_wanted(Port::Aux2));
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
    }

    #[test]
    fn a_declined_esp32_board_is_adopted_when_asked() {
        // Declined (another firmware): an ordinary device, until it is adopted.
        let mut e = engine();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Passthrough), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &esp_board());
        assert!(!e.dongle_wanted(Port::Aux2));
        drain(&mut e);
        assert_eq!(ask(&mut e, Request::AdoptModule(Port::Aux2)), Response::Ok);
        assert!(drain(&mut e).contains(&Output::Reprobe(Port::Aux2)));
        // Probed again: taken, the firmware told to program it, and its port no longer passthrough.
        e.port_detached(Port::Aux2);
        e.port_attached(Port::Aux2, &esp_board());
        assert!(e.dongle_wanted(Port::Aux2) && e.dongle_adopt(Port::Aux2));
        assert_eq!(e.config().mode(Device::Aux2), PortMode::Auto);
        e.dongle_claimed(Port::Aux2);
        assert!(!e.dongle_adopt(Port::Aux2));
        // With a module now: no second one.
        assert_eq!(ask(&mut e, Request::AdoptModule(Port::Mouse)), Response::Error(ErrorCode::Busy));
        // An empty port has nothing to adopt; a device that can't be the module ends the adoption.
        let mut e = engine();
        assert_eq!(ask(&mut e, Request::AdoptModule(Port::Aux2)), Response::Error(ErrorCode::NoDevice));
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        assert_eq!(ask(&mut e, Request::AdoptModule(Port::Keyboard)), Response::Ok);
        e.port_detached(Port::Keyboard);
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        assert!(!e.dongle_adopt(Port::Keyboard) && !e.dongle_wanted(Port::Keyboard));
    }

    #[test]
    fn module_hello_sends_policies_and_selection() {
        let mut e = engine();
        drain(&mut e);
        e.set_config(ConfigValue::BtPolicy(3, ctrl::BtPolicy::ConnectWhenSelected), Origin::Software).unwrap();
        drain(&mut e);
        e.bt_hello(7);
        let out = drain(&mut e);
        let bt: std::vec::Vec<&BtOut> =
            out.iter().filter_map(|o| if let Output::Bt(b) = o { Some(b) } else { None }).collect();
        assert!(bt.contains(&&BtOut::Policy { host: 4, policy: ctrl::BtPolicy::ConnectWhenSelected }));
        assert!(bt.contains(&&BtOut::Policy { host: 1, policy: ctrl::BtPolicy::StayConnected }));
        assert!(bt.contains(&&BtOut::RequestState));
        assert!(bt.contains(&&BtOut::Select { host: 0, pinned: 0 }));
        let Response::Info(i) = ask(&mut e, Request::GetInfo) else { panic!() };
        assert_ne!(i.caps & ctrl::caps::BLUETOOTH, 0);
    }

    #[test]
    fn a_bluetooth_host_gets_the_keyboard_and_mouse_and_the_pc_is_released() {
        let mut e = with_bt();
        // 'a' held on PC1, then switch to BT host 2.
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 10);
        drain(&mut e);
        assert_eq!(ask(&mut e, Request::SetActive(Target::Bt2)), Response::Ok);
        let out = drain(&mut e);
        assert!(out.contains(&Output::Bt(BtOut::Select { host: 2, pinned: 0 })));
        let r = reports(&out);
        assert!(r.iter().any(|(pc, ep, d)| *pc == Pc::Pc1 && *ep == 0x83 && d[1..].iter().all(|b| *b == 0)));
        assert!(events_to(&out, Pc::Pc1).contains(&Event::BtActive(2)));
        assert_eq!((e.active(), e.bt_active()), (Pc::Pc1, Some(2)));
        // Held 'a' stays hidden; 'b' reaches the host as a boot-layout keyboard report, and nothing reaches the PCs.
        e.device_report(Port::Keyboard, 0, &[0x02, 0, 0x04, 0x05, 0, 0, 0, 0], 20);
        let out = drain(&mut e);
        assert!(reports(&out).is_empty());
        let bt = bt_inputs(&out);
        assert_eq!(bt.last().unwrap(), &(2, btreport::KEYBOARD, vec![0x02, 0, 0x05, 0, 0, 0, 0, 0]));
        // The mouse too.
        e.device_report(Port::Keyboard, 1, &[0x01, 5, 0xFB], 30);
        let bt = bt_inputs(&drain(&mut e));
        assert_eq!(bt.last().unwrap(), &(2, btreport::MOUSE, vec![1, 0, 5, 0, 0xFB, 0xFF, 0, 0]));
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_active, st.bt_ready, st.bt_bonded), (2, 0b0010, 0b0110));
        // The host's Caps Lock reaches the physical keyboard.
        e.bt_output(2, btreport::KEYBOARD, &[0b10]);
        assert!(
            drain(&mut e)
                .iter()
                .any(|o| matches!(o, Output::DeviceSetReport { data, .. } if data.as_slice() == [0b10]))
        );
    }

    #[test]
    fn back_from_a_bluetooth_host_releases_it_and_toggle_returns_home() {
        let mut e = with_bt();
        e.switch(Target::Pc2, SwitchSource::Software);
        e.switch(Target::Bt2, SwitchSource::Software);
        drain(&mut e);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x06, 0, 0, 0, 0, 0], 10);
        drain(&mut e);
        // Toggle from the host: back to PC2 (the home PC), not over to PC1.
        e.switch(Target::Toggle, SwitchSource::Key);
        let out = drain(&mut e);
        assert_eq!((e.active(), e.bt_active()), (Pc::Pc2, None));
        assert!(out.contains(&Output::Bt(BtOut::Select { host: 0, pinned: 0 })));
        let bt = bt_inputs(&out);
        assert!(bt.contains(&(2, btreport::KEYBOARD, vec![0; 8])), "the host gets every key released");
        assert!(events_to(&out, Pc::Pc2).contains(&Event::BtActive(0)));
        assert!(events_to(&out, Pc::Pc2).iter().any(|ev| matches!(ev, Event::Switched(Pc::Pc2, _))));
        // 'c' held across the switch stays hidden from PC2.
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x06, 0x07, 0, 0, 0, 0], 20);
        let held = typed(&drain(&mut e)).unwrap();
        assert!(held.contains(&0x07) && !held.contains(&0x06));
    }

    #[test]
    fn next_cycles_pcs_then_ready_hosts() {
        let mut e = with_bt();
        let mut order = std::vec::Vec::new();
        for _ in 0..4 {
            e.switch(Target::Next, SwitchSource::Hotkey);
            order.push((e.active(), e.bt_active()));
        }
        // Host 3 is bonded but not connected: skipped.
        assert_eq!(order, [(Pc::Pc2, None), (Pc::Pc2, Some(2)), (Pc::Pc1, None), (Pc::Pc2, None)]);
        // Selecting a bonded host directly works even while it isn't connected (connect-when-selected hosts).
        e.switch(Target::Bt3, SwitchSource::Software);
        assert_eq!(e.bt_active(), Some(3));
        // An empty slot can't be selected.
        assert_eq!(ask(&mut e, Request::SetActive(Target::Bt1)), Response::Error(ErrorCode::NoDevice));
    }

    #[test]
    fn the_passkey_is_typed_on_the_kvms_keyboard() {
        let mut e = with_bt();
        assert_eq!(ask(&mut e, Request::BtPair(true)), Response::Ok);
        let out = drain(&mut e);
        assert!(out.contains(&Output::Bt(BtOut::Pair { on: true, seconds: ctrl::BT_PAIR_SECONDS })));
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!(st.bt_pairing, ctrl::BT_PAIR_SECONDS);
        e.bt_event(BtEventKind::PasskeyRequest, 1, 0);
        drain(&mut e);
        for (k, t) in [0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23].into_iter().zip(10u32..) {
            e.device_report(Port::Keyboard, 0, &[0, 0, k, 0, 0, 0, 0, 0], t * 10);
            e.device_report(Port::Keyboard, 0, &[0; 8], t * 10 + 5);
            if t == 12 {
                // Hosts see the digits so far, and hear of each one.
                let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
                assert_eq!((st.bt_passkey, st.bt_passkey_digits, st.bt_passkey_typed), (1, 3, 123));
                assert_eq!(events_to(&drain(&mut e), Pc::Pc1).iter().filter(|e| **e == Event::BtChanged(0)).count(), 3);
            }
        }
        let out = drain(&mut e);
        assert!(reports(&out).is_empty(), "the digits reach no PC");
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x28, 0, 0, 0, 0, 0], 500);
        let out = drain(&mut e);
        assert!(out.contains(&Output::Bt(BtOut::Passkey { host: 1, value: Some(123456) })));
        assert!(reports(&out).is_empty(), "nor does Enter");
        // Enter, still held, stays hidden from PC1 until released.
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x28, 0x04, 0, 0, 0, 0], 510);
        let held = typed(&drain(&mut e)).unwrap();
        assert!(held.contains(&0x04) && !held.contains(&0x28));
        // Pairing ends with the bond: the host can be selected.
        e.bt_event(BtEventKind::Paired, 1, 0);
        e.bt_host(bt_host(1, BT_READY));
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_pairing, st.bt_passkey, st.bt_bonded), (0, 0, 0b0111));
    }

    #[test]
    fn a_forgotten_or_vanished_host_hands_the_input_back() {
        let mut e = with_bt();
        e.switch(Target::Bt2, SwitchSource::Software);
        drain(&mut e);
        assert_eq!(ask(&mut e, Request::BtForget(2)), Response::Ok);
        let out = drain(&mut e);
        assert!(out.contains(&Output::Bt(BtOut::Forget(2))));
        assert_eq!(e.bt_active(), None);
        e.switch(Target::Bt3, SwitchSource::Software);
        e.bt_link_down();
        assert_eq!((e.bt_active(), e.active()), (None, Pc::Pc1));
        assert_eq!(ask(&mut e, Request::GetBtHost(3)), Response::Error(ErrorCode::Unsupported));
    }

    #[test]
    fn a_hotkey_is_learned_from_the_keyboard_without_reaching_the_pc() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        assert_eq!(ask(&mut e, Request::LearnHotkey(true)), Response::Ok);
        // Ctrl+Alt+K: the PC sees none of it; PC1, which asked, hears what it was, PC2 nothing.
        for (t, r) in [[0x05, 0, 0, 0, 0, 0, 0, 0], [0x05, 0, 0x0E, 0, 0, 0, 0, 0], [0x05, 0, 0, 0, 0, 0, 0, 0], [0; 8]]
            .iter()
            .enumerate()
        {
            e.device_report(Port::Keyboard, 0, r, 100 + t as u32 * 30);
        }
        let out = drain(&mut e);
        assert_eq!(typed(&out), None);
        assert_eq!(events_to(&out, Pc::Pc1), [Event::HotkeyLearned(LearnEnd::Learned, Hotkey::combo(0x05, 0x0E))]);
        assert!(events_to(&out, Pc::Pc2).is_empty());
        // Nothing changed, and typing reaches the PC again.
        assert_eq!(e.config().hotkeys, Config::default().hotkeys);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 300);
        assert_eq!(typed(&drain(&mut e)), Some(std::vec![0x04]));

        // The old hotkey, double Scroll Lock, is learned rather than switching.
        e.device_report(Port::Keyboard, 0, &[0; 8], 310);
        assert_eq!(ask(&mut e, Request::LearnHotkey(true)), Response::Ok);
        let sl = [0, 0, 0x47, 0, 0, 0, 0, 0];
        for (t, r) in [(400, sl), (450, [0; 8]), (520, sl), (570, [0; 8])] {
            e.device_report(Port::Keyboard, 0, &r, t);
        }
        let out = drain(&mut e);
        assert_eq!(e.active(), Pc::Pc1);
        assert_eq!(events_to(&out, Pc::Pc1), [Event::HotkeyLearned(LearnEnd::Learned, Hotkey::double_tap(0x47, true))]);
        assert!(!reports(&out).iter().any(|(_, ep, d)| *ep == 0x83 && d.iter().skip(1).any(|b| *b != 0)));
    }

    #[test]
    fn only_the_active_pc_may_learn_and_one_at_a_time() {
        let mut e = engine();
        assert_eq!(ask_from(&mut e, Some(Pc::Pc2), Request::LearnHotkey(true)), Response::Error(ErrorCode::NotActive));
        assert_eq!(ask_from(&mut e, None, Request::LearnHotkey(true)), Response::Ok);
        assert_eq!(ask(&mut e, Request::LearnHotkey(true)), Response::Error(ErrorCode::Busy));
        assert_eq!(ask(&mut e, Request::LearnHotkey(false)), Response::Error(ErrorCode::Busy));
        assert_eq!(ask_from(&mut e, None, Request::LearnHotkey(false)), Response::Ok);
        // The service port's session ends on both PCs.
        let out = drain(&mut e);
        for pc in [Pc::Pc1, Pc::Pc2] {
            assert_eq!(events_to(&out, pc), [Event::HotkeyLearned(LearnEnd::Cancelled, Hotkey::NONE)]);
        }
        // Stopping when nothing is learned is fine; with configuration locked, learning isn't.
        assert_eq!(ask(&mut e, Request::LearnHotkey(false)), Response::Ok);
        assert!(drain(&mut e).is_empty());
        e.set_config(ConfigValue::SwConfigAllowed(false), Origin::Software).unwrap();
        assert_eq!(ask(&mut e, Request::LearnHotkey(true)), Response::Error(ErrorCode::Locked));
    }

    #[test]
    fn learning_times_out_or_ends_with_a_switch_and_held_keys_stay_hidden() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        // Enter is still down from starting it: its release reaches the PC as usual.
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x28, 0, 0, 0, 0, 0], 0);
        drain(&mut e);
        assert_eq!(ask(&mut e, Request::LearnHotkey(true)), Response::Ok);
        e.device_report(Port::Keyboard, 0, &[0x01, 0, 0x28, 0, 0, 0, 0, 0], 100);
        assert_eq!(typed(&drain(&mut e)), None, "ctrl is withheld");
        e.device_report(Port::Keyboard, 0, &[0x01, 0, 0, 0, 0, 0, 0, 0], 150);
        assert_eq!(typed(&drain(&mut e)), Some(std::vec![]));
        // Ctrl is still held when the time is up: it stays hidden until released.
        assert_eq!(e.deadline(), Some(LEARN_MS));
        e.tick(LEARN_MS);
        let out = drain(&mut e);
        assert_eq!(events_to(&out, Pc::Pc1), [Event::HotkeyLearned(LearnEnd::TimedOut, Hotkey::NONE)]);
        e.device_report(Port::Keyboard, 0, &[0x01, 0, 0x04, 0, 0, 0, 0, 0], LEARN_MS + 10);
        assert_eq!(typed(&drain(&mut e)), Some(std::vec![0x04]));
        e.device_report(Port::Keyboard, 0, &[0; 8], LEARN_MS + 20);
        e.device_report(Port::Keyboard, 0, &[0x01, 0, 0, 0, 0, 0, 0, 0], LEARN_MS + 30);
        assert_eq!(typed(&drain(&mut e)), Some(std::vec![0xE0]));

        // A switch, here by the KVM's key, stops it.
        assert_eq!(ask(&mut e, Request::LearnHotkey(true)), Response::Ok);
        e.key(KeyEvent::Tap);
        let out = drain(&mut e);
        assert_eq!(e.active(), Pc::Pc2);
        assert_eq!(events_to(&out, Pc::Pc1)[0], Event::HotkeyLearned(LearnEnd::Cancelled, Hotkey::NONE));
        assert!(!events_to(&out, Pc::Pc2).iter().any(|e| matches!(e, Event::HotkeyLearned(..))));
    }

    #[test]
    fn software_control_comes_back_only_with_a_press_on_the_key() {
        let mut e = settled();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Configured, false));
        let off = ConfigValue::SwConfigAllowed(false);
        assert_eq!(ask(&mut e, Request::SetConfig(off)), Response::Config(off));
        assert_eq!(ask(&mut e, Request::SetConfig(ConfigValue::Profile(1))), Response::Error(ErrorCode::Locked));
        // Asking for it back arms the key; the key LED pulses as for a firmware update.
        let on = Request::SetConfig(ConfigValue::SwConfigAllowed(true));
        assert_eq!(ask(&mut e, on.clone()), Response::Error(ErrorCode::NeedsKeyPress));
        assert!(e.led_inputs().awaiting_key);
        // The tap confirms and does nothing else.
        e.key(KeyEvent::Tap);
        assert!(e.config().sw_config_allowed);
        assert_eq!(e.active(), Pc::Pc1);
        assert!(!e.led_inputs().awaiting_key);
        assert_eq!(ask(&mut e, on.clone()), Response::Config(ConfigValue::SwConfigAllowed(true)), "already on");

        // Not confirmed in time: the lock stays, and the next tap switches PCs as usual.
        ask(&mut e, Request::SetConfig(off));
        assert_eq!(ask(&mut e, on), Response::Error(ErrorCode::NeedsKeyPress));
        e.tick(T0 + UNLOCK_WAIT_MS);
        assert!(!e.led_inputs().awaiting_key);
        e.key(KeyEvent::Tap);
        assert!(!e.config().sw_config_allowed);
        assert_eq!(e.active(), Pc::Pc2);
    }

    #[test]
    fn the_key_held_at_power_up_allows_software_control_again() {
        let mut cfg = Config::default();
        cfg.set(ConfigValue::SwSwitchAllowed(false), Origin::Software).unwrap();
        cfg.set(ConfigValue::SwConfigAllowed(false), Origin::Software).unwrap();
        let info =
            Info { proto: 1, rp_fw: 0x000100, at_fw: [0; 2], hw: 0x000C00, serial: *b"TEST0001", caps: 0, bt_fw: 0 };
        let mut e = Engine::new(cfg, info, "TEST0001");
        e.allow_software(true, true);
        assert!(e.config().sw_switch_allowed && e.config().sw_config_allowed);
        e.tick(AUTOSAVE_MS);
        assert!(drain(&mut e).contains(&Output::Save));
    }

    // ── Screen hop (software.md §4.14) ──────────────────────────────────────

    /// Both PCs connected, a normalized mouse, the absolute pointer, screen hop on in the active profile with flat
    /// acceleration and a 50-pixel push, and a 1920×1080 screen of each PC, PC2's to the right.
    fn hopping() -> Engine {
        let mut e = settled();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.pc_status(Pc::Pc2, usb(UsbState::Configured, false));
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        for v in [
            ConfigValue::AbsPointer(true),
            ConfigValue::ProfileArrangement(0, Some(0)),
            ConfigValue::HopAccel(ctrl::Accel::Flat),
            ConfigValue::HopResistance(50),
            ConfigValue::Screen(0, 0, ctrl::Screen::new(Host::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(0, 1, ctrl::Screen::new(Host::Pc2, 1920, 1080, [1920, 0])),
        ] {
            e.set_config(v, Origin::Software).unwrap();
        }
        drain(&mut e);
        assert_eq!(e.active(), Pc::Pc1);
        e
    }

    fn abs(buttons: u16, at: [u16; 2]) -> std::vec::Vec<u8> {
        normalized::mouse_abs_report(buttons, at, 0, 0).to_vec()
    }

    #[test]
    fn screen_hop_moves_an_absolute_pointer_and_hops_to_the_other_pc() {
        let mut e = hopping();
        assert_eq!(e.hop_state(), HopState::Active);
        assert!(e.presentation().mouse_abs);
        // From the middle of PC1's screen, 100 to the right: 1060 of 1919.
        e.device_report(Port::Mouse, 1, &[0x00, 100, 0], T0 + 1);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, abs(0, [18100, 16399]))]);
        // On to the edge (1919): 9 more take it 41 beyond, short of the 50; the 10th pushes it over to PC2.
        let mut out = std::vec::Vec::new();
        for t in 0..9 {
            e.device_report(Port::Mouse, 1, &[0x00, 100, 0], T0 + 2 + t);
            out.extend(drain(&mut e));
        }
        assert_eq!(e.active(), Pc::Pc1);
        for t in 9..10 {
            e.device_report(Port::Mouse, 1, &[0x00, 100, 0], T0 + 2 + t);
            out.extend(drain(&mut e));
        }
        assert_eq!(e.active(), Pc::Pc2);
        assert_eq!(events_to(&out, Pc::Pc2).last(), Some(&Event::Switched(Pc::Pc2, SwitchSource::Edge)));
        let r = reports(&out);
        // PC1 keeps its cursor at the edge, released; PC2 gets the pointer at its left edge, at the same height.
        assert!(r.contains(&(Pc::Pc1, 0x82, abs(0, [32767, 16399]))), "{r:?}");
        assert_eq!(r.last(), Some(&(Pc::Pc2, 0x82, abs(0, [0, 16399]))));
        assert_eq!(e.status(None).pointer, [1920, 540]);
        // The keyboard went along: the hop is a switch like any other.
        assert_eq!(e.status(None).active, Pc::Pc2);
    }

    #[test]
    fn a_button_held_keeps_the_pointer_on_its_pc() {
        let mut e = hopping();
        for t in 0..20 {
            e.device_report(Port::Mouse, 1, &[0x01, 100, 0], T0 + 1 + t);
        }
        let out = drain(&mut e);
        assert_eq!(e.active(), Pc::Pc1);
        assert_eq!(reports(&out).last(), Some(&(Pc::Pc1, 0x82, abs(1, [32767, 16399]))));
        // Let go, push again: over.
        e.device_report(Port::Mouse, 1, &[0x00, 0, 0], T0 + 30);
        e.device_report(Port::Mouse, 1, &[0x00, 60, 0], T0 + 31);
        assert_eq!(e.active(), Pc::Pc2);
    }

    #[test]
    fn screen_hop_says_what_it_needs() {
        let mut e = settled();
        e.pc_status(Pc::Pc1, usb(UsbState::Configured, false));
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        assert_eq!(e.hop_state(), HopState::Off);
        e.set_config(ConfigValue::ProfileArrangement(0, Some(0)), Origin::Software).unwrap();
        assert_eq!(e.hop_state(), HopState::NoAbsolute);
        // Without the absolute pointer the mouse stays relative, without report IDs.
        drain(&mut e);
        e.device_report(Port::Mouse, 1, &[0x00, 5, 0], T0 + 1);
        let plain = normalized::mouse_report(&MouseInput { x: 5, ..Default::default() }, false, false);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, plain.to_vec())]);
        // The absolute pointer changes the mouse interface: both PCs enumerate it again.
        e.set_config(ConfigValue::AbsPointer(true), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::DescriptorSetChanged));
        assert_eq!(e.hop_state(), HopState::NoScreens);
        // Relative reports now carry their report ID.
        e.device_report(Port::Mouse, 1, &[0x00, 5, 0], T0 + 2);
        let with_id = normalized::mouse_rel_report(&MouseInput { x: 5, ..Default::default() }, false, false);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, with_id.to_vec())]);
        e.set_config(ConfigValue::Screen(0, 3, ctrl::Screen::new(Host::Pc1, 2560, 1440, [0, 0])), Origin::Software)
            .unwrap();
        assert_eq!(e.hop_state(), HopState::Active);
        e.pc_protocol(Pc::Pc1, 1, 0);
        assert_eq!(e.hop_state(), HopState::BootProtocol);
        e.pc_protocol(Pc::Pc1, 1, 1);
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Passthrough), Origin::Software).unwrap();
        assert_eq!(e.hop_state(), HopState::NotNormalized);
        // The feature report of the multipliers carries the report ID too.
        e.pc_set_report(Pc::Pc1, 1, 3, normalized::MOUSE_REL_ID, &[normalized::MOUSE_REL_ID, 0x05]);
        assert!(e.pcs[0].out.wheel_hires && e.pcs[0].out.pan_hires);
    }

    #[test]
    fn a_pc_whose_software_places_its_cursor_gets_the_place_instead_of_relative_motion() {
        let mut e = hopping();
        // PC1 maps its main screen only (Windows); its second screen is left of it.
        let second = ctrl::Screen { desk: [-1920, 0], ..ctrl::Screen::new(Host::Pc1, 1920, 1080, [-1920, 0]) };
        for v in [ConfigValue::HopMapping(Host::Pc1, ctrl::Mapping::MainScreen), ConfigValue::Screen(0, 2, second)] {
            e.set_config(v, Origin::Software).unwrap();
        }
        drain(&mut e);
        let left = |e: &mut Engine, t| e.device_report(Port::Mouse, 1, &[0x00, (-100i8) as u8, 0], t);
        let rel =
            |buttons, x| normalized::mouse_rel_report(&MouseInput { buttons, x, ..Default::default() }, false, false);
        let placed = |out: &[Output]| {
            events_to(out, Pc::Pc1)
                .into_iter()
                .filter(|ev| matches!(ev, Event::PlaceCursor(_)))
                .collect::<std::vec::Vec<_>>()
        };
        // Without host software: relative motion on the second screen, 40 pixels into it after 1000.
        for t in 1..=10 {
            left(&mut e, T0 + t);
        }
        let out = drain(&mut e);
        assert!(placed(&out).is_empty());
        assert_eq!(reports(&out).last(), Some(&(Pc::Pc1, 0x82, rel(0, -100).to_vec())));
        // The host software asks: the place, in PC1's pixels, on its control interface only; nothing on the pointers.
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::PlaceCursor(ctrl::Placement::Others)), Response::Ok);
        left(&mut e, T0 + 11);
        let out = drain(&mut e);
        assert_eq!(placed(&out), [Event::PlaceCursor([-140, 540])]);
        assert!(reports(&out).is_empty());
        assert!(events_to(&out, Pc::Pc2).is_empty());
        // Two in one millisecond: the first at once, the latest with the next.
        left(&mut e, T0 + 12);
        left(&mut e, T0 + 12);
        assert_eq!(placed(&drain(&mut e)), [Event::PlaceCursor([-240, 540])]);
        assert_eq!(e.deadline(), Some(T0 + 13));
        e.tick(T0 + 13);
        assert_eq!(placed(&drain(&mut e)), [Event::PlaceCursor([-340, 540])]);
        // A press and its release there: on the relative pointer, without motion.
        e.device_report(Port::Mouse, 1, &[0x01, 0, 0], T0 + 14);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, rel(1, 0).to_vec())]);
        e.device_report(Port::Mouse, 1, &[0x00, 0, 0], T0 + 15);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, rel(0, 0).to_vec())]);
        // The lease runs out (the software stopped without saying so): relative motion again, from where it was put.
        e.tick(T0 + 11 + ctrl::PLACE_LEASE_MS);
        left(&mut e, T0 + 12 + ctrl::PLACE_LEASE_MS);
        let out = drain(&mut e);
        assert!(placed(&out).is_empty());
        assert_eq!(reports(&out), [(Pc::Pc1, 0x82, rel(0, -100).to_vec())]);
        // Renewed, then stopped (`Off`): the same.
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::PlaceCursor(ctrl::Placement::Others)), Response::Ok);
        left(&mut e, T0 + 13 + ctrl::PLACE_LEASE_MS);
        assert_eq!(placed(&drain(&mut e)), [Event::PlaceCursor([-540, 540])]);
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::PlaceCursor(ctrl::Placement::Off)), Response::Ok);
        left(&mut e, T0 + 14 + ctrl::PLACE_LEASE_MS);
        assert!(placed(&drain(&mut e)).is_empty());
        // The service port has no cursor.
        let r = ask_from(&mut e, None, Request::PlaceCursor(ctrl::Placement::All));
        assert_eq!(r, Response::Error(ErrorCode::NotActive));
    }

    #[test]
    fn the_pcs_own_cursor_position_is_taken_over_while_the_pointer_rests() {
        let mut e = hopping();
        e.device_report(Port::Mouse, 1, &[0x00, 10, 0], T0 + 1);
        drain(&mut e);
        e.tick(T0 + 1 + hop::SYNC_QUIET_MS);
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::PointerAt([200, 300])), Response::Ok);
        assert_eq!(e.status(None).pointer, [200, 300]);
        // The next motion goes on from there.
        e.device_report(Port::Mouse, 1, &[0x00, 1, 0], T0 + 500);
        assert_eq!(reports(&drain(&mut e)), [(Pc::Pc1, 0x82, abs(0, [3432, 9110]))]);
        // Not from the service port: it has no cursor.
        assert_eq!(ask_from(&mut e, None, Request::PointerAt([0, 0])), Response::Error(ErrorCode::NotActive));
    }

    /// Pushes the mouse (`dx` a report) until `done`, at most 60 reports; returns what came out.
    fn push_until(e: &mut Engine, dx: i8, t0: u32, done: impl Fn(&Engine) -> bool) -> std::vec::Vec<Output> {
        let mut out = std::vec::Vec::new();
        for t in 0..60 {
            e.device_report(Port::Mouse, 1, &[0x00, dx as u8, 0], t0 + t);
            out.extend(drain(e));
            if done(e) {
                return out;
            }
        }
        panic!("never got there");
    }

    /// `hopping()` with Bluetooth host 2 ready, and its 1024×768 tablet screen left of PC1's.
    fn hopping_with_a_tablet(pointer: ctrl::BtPointer) -> Engine {
        let mut e = hopping();
        e.bt_hello(0x000100);
        e.bt_host(bt_host(2, BT_READY));
        let tablet = ctrl::Screen::new(Host::Bt2, 1024, 768, [-1024, 0]);
        e.set_config(ConfigValue::Screen(0, 2, tablet), Origin::Software).unwrap();
        e.set_config(ConfigValue::BtPointer(1, pointer), Origin::Software).unwrap();
        drain(&mut e);
        e
    }

    #[test]
    fn screen_hop_takes_the_pointer_onto_a_bluetooth_host_and_back() {
        let mut e = hopping_with_a_tablet(ctrl::BtPointer::Absolute);
        assert_eq!(e.hop_state(), HopState::Active);
        let out = push_until(&mut e, -100, T0 + 1, |e| e.bt_active() == Some(2));
        assert!(out.contains(&Output::Bt(BtOut::Select { host: 2, pinned: 0 })));
        assert_eq!(events_to(&out, Pc::Pc1).last(), Some(&Event::BtActive(2)));
        // The tablet gets its absolute pointer at its right edge, at the same height: 540 of its 768 lines.
        let to_tablet = bt_inputs(&out);
        let at = (540.0f32 / 767.0 * 32767.0 + 0.5) as u16;
        assert_eq!(
            to_tablet.last(),
            Some(&(2, btreport::POINTER, kvm_proto::link::bt::pointer_report(0, 32767, at, 0, 0).to_vec()))
        );
        assert_eq!((e.status(None).pointer, e.hop_state()), ([-1, 540], HopState::Active));
        // On the tablet the pointer moves as on a PC: 100 to the left.
        e.device_report(Port::Mouse, 1, &[0x00, (-100i8) as u8, 0], T0 + 100);
        let x = (923.0f32 / 1023.0 * 32767.0 + 0.5) as u16;
        let want = (2, btreport::POINTER, kvm_proto::link::bt::pointer_report(0, x, at, 0, 0).to_vec());
        assert_eq!(bt_inputs(&drain(&mut e)), [want]);
        // Back to PC1: the tablet's pointer is released where it is, PC1 gets the pointer at its left edge.
        let out = push_until(&mut e, 100, T0 + 200, |e| e.bt_active().is_none());
        assert_eq!(e.active(), Pc::Pc1);
        assert!(bt_inputs(&out).contains(&(
            2,
            btreport::POINTER,
            kvm_proto::link::bt::pointer_report(0, 32767, at, 0, 0).to_vec()
        )));
        assert_eq!(reports(&out).last(), Some(&(Pc::Pc1, 0x82, abs(0, [0, 16399]))));
    }

    #[test]
    fn a_bluetooth_host_without_an_absolute_pointer_gets_its_cursor_parked_and_moved() {
        let mut e = hopping_with_a_tablet(ctrl::BtPointer::Relative);
        let out = push_until(&mut e, -100, T0 + 1, |e| e.bt_active() == Some(2));
        let mouse =
            |dx: i32, dy: i32| (2, btreport::MOUSE, kvm_proto::link::bt::mouse_report(0, dx, dy, 0, 0).to_vec());
        // Pushed into its top-left corner, then to the pointer: its right edge (1023), 540 down.
        let to_tablet = bt_inputs(&out);
        assert_eq!(to_tablet[to_tablet.len() - 2..], [mouse(-hop::PARK, -hop::PARK), mouse(1023, 540)]);
        assert!(to_tablet.iter().all(|(_, id, _)| *id == btreport::MOUSE), "no absolute report");
        // Against the tablet's left edge, a wall: the push goes on to the tablet, so its cursor meets that edge too.
        for t in 0..20 {
            e.device_report(Port::Mouse, 1, &[0x00, (-100i8) as u8, 0], T0 + 100 + t);
        }
        let moved: i32 =
            bt_inputs(&drain(&mut e)).iter().map(|(_, _, d)| i16::from_le_bytes([d[2], d[3]]) as i32).sum();
        assert_eq!((moved, e.status(None).pointer), (-2000, [-1024, 540]));
        // Away and back again: parked anew.
        push_until(&mut e, 100, T0 + 200, |e| e.bt_active().is_none());
        let out = push_until(&mut e, -100, T0 + 300, |e| e.bt_active() == Some(2));
        assert!(bt_inputs(&out).contains(&mouse(-hop::PARK, -hop::PARK)));
    }

    #[test]
    fn each_profile_hops_across_its_own_arrangement_and_switching_writes_nothing() {
        let mut e = hopping();
        // Arrangement 1, for profile 1: PC2 left of PC1.
        for v in [
            ConfigValue::Screen(1, 0, ctrl::Screen::new(Host::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(1, 1, ctrl::Screen::new(Host::Pc2, 1920, 1080, [-1920, 0])),
            ConfigValue::ArrangementName(1, ctrl::ProfileName::new("left")),
            ConfigValue::ProfileArrangement(1, Some(1)),
            ConfigValue::ProfileModes(1, [PortMode::Auto; DEVICES]),
        ] {
            e.set_config(v, Origin::Software).unwrap();
        }
        assert_eq!(ask(&mut e, Request::SaveConfig), Response::Ok);
        let mut buf = [0u8; SAVE_ITEM_MAX];
        let mut keys = std::vec::Vec::new();
        while let Some((k, _)) = e.take_save_item(&mut buf) {
            keys.push(k);
        }
        // The arrangements first, each its own item, then the config.
        assert_eq!(keys, [config::ARRANGEMENT_ITEM, config::ARRANGEMENT_ITEM + 1, config::ITEM]);
        e.config_saved();
        drain(&mut e);
        e.set_config(ConfigValue::Profile(1), Origin::Device).unwrap();
        e.tick(T0 + 2 * AUTOSAVE_MS);
        assert!(!drain(&mut e).contains(&Output::Save));
        assert!(e.take_save_item(&mut buf).is_none(), "a profile change writes nothing");
        // Profile 1's arrangement: PC2 is to the left now.
        assert_eq!(e.status(None).pointer, [960, 540]);
        push_until(&mut e, -100, T0 + 3 * AUTOSAVE_MS, |e| e.active() == Pc::Pc2);
        // Profile 0's again: PC2 to the right.
        e.set_config(ConfigValue::Profile(0), Origin::Device).unwrap();
        e.switch(Target::Pc1, SwitchSource::Hotkey);
        push_until(&mut e, 100, T0 + 4 * AUTOSAVE_MS, |e| e.active() == Pc::Pc2);
    }

    #[test]
    fn a_restored_arrangement_is_not_written_again() {
        let info =
            Info { proto: 1, rp_fw: 0x000100, at_fw: [0; 2], hw: 0x000C00, serial: *b"TEST0001", caps: 0, bt_fw: 0 };
        let mut stored = Config::default();
        stored.arrangements[0].screens[0] = ctrl::Screen::new(Host::Pc1, 1920, 1080, [0, 0]);
        let mut buf = [0u8; SAVE_ITEM_MAX];
        let mut item = [0u8; config::ARRANGEMENT_ITEM_MAX];
        let n = stored.encode_arrangement(0, &mut item);
        let mut e = Engine::new(Config::default(), info, "TEST0001");
        e.restore_item(config::ARRANGEMENT_ITEM, &item[..n]);
        assert_eq!(e.config().arrangements[0], stored.arrangements[0]);
        assert!(e.take_save_item(&mut buf).is_none());
        assert_eq!(Engine::item_keys().filter(|k| (0x0010..0x0014).contains(k)).count(), MAX_ARRANGEMENTS);
    }

    /// The boot keyboard of `keyboard_and_mouse`, as a Bluetooth keyboard's report map (no report IDs).
    const KB_MAP: &[u8] = &[
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
        0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01,
        0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
        0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
    ];

    fn wireless_state(n: u8, map: &[u8]) -> DeviceState {
        use kvm_proto::link::bt::device_flags as f;
        DeviceState {
            dev: n,
            flags: f::BONDED | f::CONNECTED | f::ENCRYPTED | f::READY,
            rssi: -50,
            battery: 90,
            appearance: kvm_proto::link::bt::appearance::KEYBOARD,
            addr: [0, 9, 9, 9, 9, 9, n],
            map_len: map.len() as u16,
            map_crc: crc32(map),
            vid: 0,
            pid: 0,
        }
    }

    /// Bluetooth device slot `n` bonded and connected, with `map`: the engine asks for the map, and it comes in pieces.
    fn wireless(e: &mut Engine, n: u8, map: &[u8]) {
        e.bt_device(wireless_state(n, map), "Desk keyboard");
        assert!(drain(e).contains(&Output::Bt(BtOut::RequestMap(n))));
        let chunk = kvm_proto::link::bt::MAP_CHUNK;
        for (i, piece) in map.chunks(chunk).enumerate() {
            e.bt_device_map(n, (i * chunk) as u16, map.len() as u16, piece);
        }
    }

    fn nkro_has(r: &[u8], k: u8) -> bool {
        r[2 + k as usize / 8] & (1 << (k % 8)) != 0
    }

    /// The last NKRO report each PC got.
    fn last_keys(out: &[Output], pc: Pc) -> Option<std::vec::Vec<u8>> {
        reports(out).into_iter().rev().find(|(p, ep, _)| *p == pc && *ep == 0x83).map(|(_, _, d)| d)
    }

    #[test]
    fn a_bluetooth_keyboard_types_into_the_active_target_as_a_front_port_keyboard_does() {
        let mut e = with_bt();
        wireless(&mut e, 1, KB_MAP);
        let out = drain(&mut e);
        assert!(events_to(&out, Pc::Pc1).contains(&Event::BtDeviceChanged(1)));
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_devices_bonded, st.bt_devices_ready), (0b0001, 0b0001));
        let Response::BtDevice(info) = ask(&mut e, Request::GetBtDevice(1)) else { panic!() };
        assert_eq!((info.name.as_str(), info.battery, info.slot), ("Desk keyboard", 90, 1));
        // 'a' on it reaches PC1 through the KVM's keyboard.
        e.bt_device_input(1, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 10);
        assert!(nkro_has(&last_keys(&drain(&mut e), Pc::Pc1).unwrap(), 0x04));
        // Released, then to Bluetooth host 2 once it is the target.
        e.bt_device_input(1, 0, &[0; 8], 20);
        assert_eq!(ask(&mut e, Request::SetActive(Target::Bt2)), Response::Ok);
        drain(&mut e);
        e.bt_device_input(1, 0, &[0, 0, 0x05, 0, 0, 0, 0, 0], 30);
        let out = drain(&mut e);
        assert_eq!(bt_inputs(&out).last().unwrap(), &(2, btreport::KEYBOARD, std::vec![0, 0, 0x05, 0, 0, 0, 0, 0]));
        // Off in this profile: what it holds is let go, and its input goes nowhere.
        e.set_config(ConfigValue::Mode(Device::Wireless1, PortMode::Off), Origin::Software).unwrap();
        assert_eq!(bt_inputs(&drain(&mut e)).last().unwrap().2, std::vec![0; 8]);
        e.bt_device_input(1, 0, &[0, 0, 0x06, 0, 0, 0, 0, 0], 40);
        let out = drain(&mut e);
        assert!(bt_inputs(&out).is_empty() && reports(&out).is_empty());
        // Forgotten: no longer one of the KVM's devices.
        e.bt_device(DeviceState { flags: 0, map_len: 0, map_crc: 0, ..wireless_state(1, KB_MAP) }, "");
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!(st.bt_devices_bonded, 0);
    }

    #[test]
    fn a_keyboard_pinned_to_pc2_types_there_whichever_pc_is_active() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.set_config(ConfigValue::Route(Device::Keyboard, Routing::Pin(Host::Pc2)), Origin::Software).unwrap();
        drain(&mut e);
        assert_eq!(e.port_info(Port::Keyboard).target, Some(Host::Pc2));
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 10);
        let out = drain(&mut e);
        assert!(nkro_has(&last_keys(&out, Pc::Pc2).unwrap(), 0x04));
        assert!(last_keys(&out, Pc::Pc1).is_none(), "nothing for PC1");
        // A switch to PC2 puts it with the devices that follow the active target: 'a', held across, is let go there and
        // stays hidden until released.
        e.switch(Target::Pc2, SwitchSource::Software);
        assert!(!nkro_has(&last_keys(&drain(&mut e), Pc::Pc2).unwrap(), 0x04));
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0x05, 0, 0, 0, 0], 20);
        let keys = last_keys(&drain(&mut e), Pc::Pc2).unwrap();
        assert!(!nkro_has(&keys, 0x04) && nkro_has(&keys, 0x05));
        // Back on PC1 it stays with PC2.
        e.device_report(Port::Keyboard, 0, &[0; 8], 30);
        e.switch(Target::Pc1, SwitchSource::Software);
        drain(&mut e);
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x06, 0, 0, 0, 0, 0], 40);
        let out = drain(&mut e);
        assert!(nkro_has(&last_keys(&out, Pc::Pc2).unwrap(), 0x06) && last_keys(&out, Pc::Pc1).is_none());
        // PC2's Caps Lock reaches the keyboard, PC1's doesn't.
        let kb_iface = (0..8).find(|i| matches!(e.presentation().role_of_iface(*i), Some(Role::BootKeyboard))).unwrap();
        e.pc_set_report(Pc::Pc1, kb_iface, 2, 0, &[0b10]);
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::DeviceSetReport { .. })));
        e.pc_set_report(Pc::Pc2, kb_iface, 2, 0, &[0b10]);
        let leds = drain(&mut e);
        assert!(leds.iter().any(|o| matches!(o, Output::DeviceSetReport { data, .. } if data.as_slice() == [0b10])));
        // Its hotkeys work wherever its input goes: double Scroll Lock toggles to PC2.
        e.device_report(Port::Keyboard, 0, &[0; 8], 50);
        for t in [100, 200] {
            e.device_report(Port::Keyboard, 0, &[0, 0, 0x47, 0, 0, 0, 0, 0], t);
            e.device_report(Port::Keyboard, 0, &[0; 8], t + 20);
        }
        assert_eq!(e.active(), Pc::Pc2);
    }

    #[test]
    fn a_mouse_pinned_to_a_bluetooth_host_moves_its_pointer_while_a_pc_is_active() {
        let mut e = with_bt();
        e.set_config(ConfigValue::Route(Device::Keyboard, Routing::Pin(Host::Bt3)), Origin::Software).unwrap();
        // The module keeps host 3 connected and takes its input.
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::Select { host: 0, pinned: 0b0100 })));
        e.device_report(Port::Keyboard, 1, &[0x01, 5, 0xFB], 10);
        let out = drain(&mut e);
        assert_eq!(bt_inputs(&out), [(3, btreport::MOUSE, std::vec![1, 0, 5, 0, 0xFB, 0xFF, 0, 0])]);
        assert!(reports(&out).iter().all(|(_, ep, _)| *ep != 0x82), "no mouse report for a PC");
        // Following again: the module hears it, and the pointer is PC1's.
        e.device_report(Port::Keyboard, 1, &[0, 0, 0], 20);
        e.set_config(ConfigValue::Route(Device::Keyboard, Routing::Follow), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::Select { host: 0, pinned: 0 })));
        e.device_report(Port::Keyboard, 1, &[0, 1, 0], 30);
        assert!(reports(&drain(&mut e)).iter().any(|(pc, ep, _)| (*pc, *ep) == (Pc::Pc1, 0x82)));
    }

    #[test]
    fn looking_for_devices_and_pairing_one() {
        let mut e = with_bt();
        assert_eq!(ask(&mut e, Request::BtPair(true)), Response::Ok);
        drain(&mut e);
        // Looking for devices closes host pairing: the module pairs one way at a time.
        assert_eq!(ask(&mut e, Request::BtScan(true)), Response::Ok);
        let out = drain(&mut e);
        assert!(out.contains(&Output::Bt(BtOut::Pair { on: false, seconds: 0 })));
        assert!(out.contains(&Output::Bt(BtOut::Scan { on: true, seconds: ctrl::BT_PAIR_SECONDS })));
        e.bt_found([1, 1, 2, 3, 4, 5, 6], -60, kvm_proto::link::bt::appearance::MOUSE, "Travel mouse");
        e.bt_found([1, 1, 2, 3, 4, 5, 6], -58, kvm_proto::link::bt::appearance::MOUSE, "Travel mouse");
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_scan, st.bt_found, st.bt_pairing), (ctrl::BT_PAIR_SECONDS, 1, 0));
        let Response::BtFound(Some(f)) = ask(&mut e, Request::GetBtFound(0)) else { panic!() };
        assert_eq!((f.rssi, f.name.as_str()), (-58, "Travel mouse"));
        assert_eq!(ask(&mut e, Request::GetBtFound(1)), Response::BtFound(None));
        // Pairing it: the module stops looking; a second one waits for the first.
        assert_eq!(ask(&mut e, Request::BtPairDevice(f.addr)), Response::Ok);
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::PairDevice(f.addr))));
        assert_eq!(ask(&mut e, Request::BtPairDevice(f.addr)), Response::Error(ErrorCode::Busy));
        e.bt_event(BtEventKind::DevicePasskey, 0, 4242);
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_scan, st.bt_device_pairing, st.bt_device_passkey), (0, DevicePairing::Pairing, Some(4242)));
        e.bt_event(BtEventKind::DeviceStage, 2, DevicePairing::Paired as u32);
        let Response::Status(st) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((st.bt_device_pairing, st.bt_device_passkey), (DevicePairing::Paired, None));
        // Forgetting goes to the module.
        assert_eq!(ask(&mut e, Request::BtForgetDevice(2)), Response::Ok);
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::ForgetDevice(2))));
        // Every pairing the module holds, and a restart of the module.
        assert_eq!(ask(&mut e, Request::BtEraseAll), Response::Ok);
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::EraseAll)));
        assert_eq!(ask(&mut e, Request::BtForgetDevice(5)), Response::Error(ErrorCode::InvalidValue));
    }

    #[test]
    fn a_normalized_bluetooth_device_keeps_the_kvms_own_interfaces() {
        let mut e = with_bt();
        let mut both = keyboard_and_mouse();
        both.device.pid = 0x9999;
        e.set_config(ConfigValue::Mode(Device::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &both);
        drain(&mut e);
        // Both mirrored need more IN endpoints than EP5–EP7: the KVM's own interfaces make way.
        let wide = |e: &Engine| e.presentation().nkro.ep_in == 0;
        assert!(wide(&e));
        wireless(&mut e, 2, KB_MAP);
        drain(&mut e);
        assert!(!wide(&e), "the Bluetooth keyboard needs the KVM's keyboard interface");
        assert_eq!(e.wireless().slot(2).map(|w| w.live()), Some(true));
    }

    // ── the mapping engine (hid_mapping.md) ─────────────────────────────────────

    /// A report of the KVM's own interfaces read back through its descriptor.
    fn read_back(desc: &[u8], data: &[u8]) -> kvm_hid::Translation {
        Translator::new(Layout::parse(desc).unwrap()).translate(data)
    }

    /// The last report on `ep` in `out`.
    fn last_on(out: &[Output], ep: u8) -> Option<std::vec::Vec<u8>> {
        reports(out).into_iter().filter(|(_, e, _)| *e == ep).map(|(_, _, d)| d).next_back()
    }

    fn mouse_desc(e: &Engine) -> &'static [u8] {
        if e.presentation().mouse_abs { normalized::MOUSE_ABS_DESC } else { normalized::MOUSE_DESC }
    }

    /// A generic gamepad (14 buttons, a hat, X, Y, Z, Rz of 8 bits) on interface 0, a mouse on interface 1.
    fn gamepad() -> Probed {
        let pad = &[
            0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x15, 0x00, 0x25, 0x01, 0x35, 0x00, 0x45, 0x01, 0x75, 0x01, 0x95, 0x0E,
            0x05, 0x09, 0x19, 0x01, 0x29, 0x0E, 0x81, 0x02, 0x95, 0x02, 0x81, 0x01, 0x05, 0x01, 0x25, 0x07, 0x46, 0x3B,
            0x01, 0x75, 0x04, 0x95, 0x01, 0x65, 0x14, 0x09, 0x39, 0x81, 0x42, 0x65, 0x00, 0x95, 0x01, 0x81, 0x01, 0x26,
            0xFF, 0x00, 0x46, 0xFF, 0x00, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x75, 0x08, 0x95, 0x04, 0x81,
            0x02, 0xC0,
        ][..];
        let mut p = keyboard_and_mouse();
        p.device = DeviceDesc { vid: 0x0F0D, pid: 0x00C1, ..Default::default() };
        p.report_descs[0].1 = Vec::from_slice(pad).unwrap();
        p
    }

    #[test]
    fn a_keymap_maps_across_device_kinds() {
        use kvm_map::keycode::KC_BTN1;
        use kvm_map::pos;
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        let on = Request::SetConfig(ConfigValue::ProfileKeymap(0, Some(1)));
        assert_eq!(ask(&mut e, on), Response::Config(ConfigValue::ProfileKeymap(0, Some(1))));
        let set = |e: &mut Engine, first: u16, kc: u16| {
            let codes = Vec::from_slice(&[kc]).unwrap();
            ask(e, Request::SetKeycodes { keymap: 1, layer: 0, first, codes })
        };
        // Keymap 2: mouse button 3 types Escape, A clicks button 1.
        assert_eq!(set(&mut e, pos::MOUSE + 2, 0x29), Response::Ok);
        assert_eq!(set(&mut e, 0x04, KC_BTN1), Response::Ok);
        let Response::Keycodes { codes, .. } = ask(&mut e, Request::GetKeycodes { keymap: 1, layer: 0, first: 0 })
        else {
            panic!()
        };
        assert_eq!((codes[4], codes[5]), (KC_BTN1, 0x05));
        drain(&mut e);
        let (nkro, mouse) = (e.presentation().nkro.ep_in, e.presentation().mouse.ep_in);
        e.device_report(Port::Keyboard, 1, &[0b100, 0, 0], 10);
        let out = drain(&mut e);
        let keys = read_back(normalized::NKRO_DESC, &last_on(&out, nkro).unwrap()).keyboard.unwrap();
        assert!(keys.is_pressed(0x29) && keys.key_count() == 1);
        assert!(last_on(&out, mouse).is_none_or(|m| read_back(mouse_desc(&e), &m).mouse.unwrap().buttons == 0));
        e.device_report(Port::Keyboard, 1, &[0, 0, 0], 20);
        // Motion still moves: the axis rules' default.
        e.device_report(Port::Keyboard, 1, &[0, 5, 0xFB], 30);
        let m = read_back(mouse_desc(&e), &last_on(&drain(&mut e), mouse).unwrap()).mouse.unwrap();
        assert_eq!((m.x, m.y, m.buttons), (5, -5, 0));
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x04, 0, 0, 0, 0, 0], 40);
        let out = drain(&mut e);
        assert_eq!(read_back(mouse_desc(&e), &last_on(&out, mouse).unwrap()).mouse.unwrap().buttons, 1);
        assert!(last_on(&out, nkro).is_none_or(|k| read_back(normalized::NKRO_DESC, &k).keyboard.unwrap().is_empty()));
        // Saved 5 s after the last change: the config and keymap 2's layer 0.
        e.device_report(Port::Keyboard, 0, &[0; 8], 50);
        e.tick(5100);
        assert!(drain(&mut e).contains(&Output::Save));
        let mut buf = [0u8; SAVE_ITEM_MAX];
        let items: std::vec::Vec<u16> = core::iter::from_fn(|| e.take_save_item(&mut buf).map(|(k, _)| k)).collect();
        assert_eq!(items, [config::ITEM, kvm_map::keymap::item::LAYER + 8]);
    }

    #[test]
    fn a_gamepad_goes_to_the_virtual_gamepad() {
        let mut e = engine();
        e.set_config(ConfigValue::Gamepad(GamepadOut::Hid), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &gamepad());
        drain(&mut e);
        let nkro = e.presentation().nkro.ep_in;
        assert_eq!(e.presentation().gamepad, GamepadOut::Hid);
        // Without a keymap, 1:1: button 2, the hat right, X full left, Rz three quarters.
        e.device_report(Port::Aux2, 0, &[0b10, 0, 2, 0, 128, 128, 224], 10);
        let r = last_on(&drain(&mut e), nkro).unwrap();
        let g = read_back(&normalized::NKRO_PAD_DESC, &r).gamepad.unwrap();
        assert_eq!((g.buttons, g.hat, g.axes[0], g.axes[1], g.axes[5]), (0b10, 2, -32767, 128, 24800));
        // With a keymap: button 2 types B, the rest is as before.
        e.set_config(ConfigValue::ProfileKeymap(0, Some(0)), Origin::Software).unwrap();
        let codes = Vec::from_slice(&[0x05]).unwrap();
        ask(&mut e, Request::SetKeycodes { keymap: 0, layer: 0, first: kvm_map::pos::PAD + 1, codes });
        e.device_report(Port::Aux2, 0, &[0, 0, 0x0F, 128, 128, 128, 128], 20);
        drain(&mut e);
        e.device_report(Port::Aux2, 0, &[0b10, 0, 0x0F, 128, 128, 128, 128], 30);
        let out = drain(&mut e);
        assert!(
            read_back(normalized::NKRO_DESC, &last_on(&out, nkro).unwrap())
                .keyboard
                .is_some_and(|k| k.is_pressed(0x05))
        );
    }

    #[test]
    fn the_mouse_pushes_a_stick_of_the_virtual_gamepad() {
        use kvm_map::axes::{Rule, Target, input};
        let mut e = engine();
        e.set_config(ConfigValue::Gamepad(GamepadOut::XInput), Origin::Software).unwrap();
        e.set_config(ConfigValue::ProfileKeymap(0, Some(0)), Origin::Software).unwrap();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        let x = e.presentation().xinput.unwrap();
        // Mouse X → the right stick's X (Rx), springing back by a quarter per tick.
        let rule = Rule { target: Target::Pad(3), flags: 0, scale: 256, deadzone: 0, param: 64 }.encode();
        assert_eq!(
            ask(&mut e, Request::SetAxisRule { keymap: 0, layer: 0, input: input::MOUSE_X as u8, rule }),
            Response::Ok
        );
        drain(&mut e);
        e.device_report(Port::Keyboard, 1, &[0, 20, 0], 10);
        let out = drain(&mut e);
        let r = last_on(&out, x.ep_in).unwrap();
        assert_eq!(i16::from_le_bytes([r[10], r[11]]), 20 * 256, "XInput's right stick X");
        assert!(last_on(&out, e.presentation().mouse.ep_in).is_none(), "the pointer doesn't move");
        let t = e.deadline().unwrap();
        e.tick(t);
        let r = last_on(&drain(&mut e), x.ep_in).unwrap();
        assert_eq!(i16::from_le_bytes([r[10], r[11]]), 15 * 256);
    }

    #[test]
    fn learning_a_control_tells_it_and_keeps_it_from_the_host() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::WatchControls(2)), Response::Ok);
        e.device_report(Port::Keyboard, 1, &[0b10, 0, 0], 10);
        let out = drain(&mut e);
        assert_eq!(events_to(&out, Pc::Pc1), [Event::Control(kvm_map::pos::MOUSE + 1, true)]);
        assert!(events_to(&out, Pc::Pc2).is_empty());
        assert!(reports(&out).is_empty(), "nothing reaches the PC");
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::WatchControls(0)), Response::Ok);
        // Held while learning: hidden until released.
        e.device_report(Port::Keyboard, 1, &[0b10, 1, 0], 20);
        let m = read_back(mouse_desc(&e), &last_on(&drain(&mut e), e.presentation().mouse.ep_in).unwrap());
        assert_eq!(m.mouse.unwrap().buttons, 0);
    }

    #[test]
    fn only_the_active_pc_watches_the_controls_and_not_for_ever() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        // The inactive PC would see what is typed on the active one.
        assert_eq!(ask_from(&mut e, Some(Pc::Pc2), Request::WatchControls(1)), Response::Error(ErrorCode::NotActive));
        // Asked again within WATCH_MS, it goes on; otherwise it ends, and what it kept reaches the PC again.
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::WatchControls(2)), Response::Ok);
        e.tick(WATCH_MS - 1000);
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::WatchControls(2)), Response::Ok);
        e.tick(WATCH_MS + 1000);
        e.device_report(Port::Keyboard, 1, &[0b1, 0, 0], WATCH_MS + 1000);
        assert!(reports(&drain(&mut e)).is_empty(), "renewed: still kept");
        e.device_report(Port::Keyboard, 1, &[0, 0, 0], WATCH_MS + 1010);
        drain(&mut e);
        assert_eq!(e.deadline(), Some(2 * WATCH_MS - 1000));
        e.tick(2 * WATCH_MS - 1000);
        e.device_report(Port::Keyboard, 1, &[0b1, 0, 0], 2 * WATCH_MS);
        let out = drain(&mut e);
        assert!(events_to(&out, Pc::Pc1).is_empty());
        let m = read_back(mouse_desc(&e), &last_on(&out, e.presentation().mouse.ep_in).unwrap());
        assert_eq!(m.mouse.unwrap().buttons, 1);
        // A switch ends it too.
        assert_eq!(ask_from(&mut e, Some(Pc::Pc1), Request::WatchControls(1)), Response::Ok);
        assert_eq!(ask(&mut e, Request::SetActive(Target::Pc2)), Response::Ok);
        drain(&mut e);
        e.device_report(Port::Keyboard, 1, &[0, 0, 0], 2 * WATCH_MS + 10);
        assert!(events_to(&drain(&mut e), Pc::Pc1).is_empty());
    }

    #[test]
    fn a_gamepads_calibration_follows_it_and_is_saved() {
        let mut e = engine();
        e.set_config(ConfigValue::Gamepad(GamepadOut::Hid), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &gamepad());
        let Response::Controls(_, c) = ask(&mut e, Request::GetControls(Device::Aux2)) else { panic!() };
        assert_eq!((c.pad_buttons, c.hat, c.pad_axes, c.mouse_buttons), ((1 << 14) - 1, true, 0b10_0111, 0b111));
        assert_ne!(c.identity, 0);
        // X rests at 140 (raw) and reaches only 20 and 235.
        let cal = AxisCal { min: -27500, centre: 1542, max: 27500, deadzone: 8 };
        assert_eq!(ask(&mut e, Request::SetCalibration(Device::Aux2, 0, cal)), Response::Ok);
        assert_eq!(
            ask(&mut e, Request::GetCalibration(Device::Aux2, 0)),
            Response::Calibration(Device::Aux2, 0, cal)
        );
        drain(&mut e);
        e.device_report(Port::Aux2, 0, &[0, 0, 0x0F, 134, 128, 128, 128], 10);
        let r = last_on(&drain(&mut e), e.presentation().nkro.ep_in).unwrap();
        assert_eq!(read_back(&normalized::NKRO_PAD_DESC, &r).gamepad.unwrap().axes[0], 0, "at rest");
        e.device_report(Port::Aux2, 0, &[0, 0, 0x0F, 235, 128, 128, 128], 20);
        let r = last_on(&drain(&mut e), e.presentation().nkro.ep_in).unwrap();
        assert_eq!(read_back(&normalized::NKRO_PAD_DESC, &r).gamepad.unwrap().axes[0], 32767);
        e.tick(5100);
        let mut buf = [0u8; SAVE_ITEM_MAX];
        let items: std::vec::Vec<u16> = core::iter::from_fn(|| e.take_save_item(&mut buf).map(|(k, _)| k)).collect();
        assert!(items.contains(&crate::known::ITEM), "{items:?}");
    }

    #[test]
    fn a_kvm_action_on_a_mouse_button_switches_the_pc() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.set_config(ConfigValue::ProfileKeymap(0, Some(0)), Origin::Software).unwrap();
        // Mouse button 3: select-pc2, the hotkey list's fifth action.
        assert_eq!(HotkeyAction::ALL[4], HotkeyAction::SelectHost(Host::Pc2));
        let codes = Vec::from_slice(&[kvm_map::keycode::KVM_ACTION + 4]).unwrap();
        ask(&mut e, Request::SetKeycodes { keymap: 0, layer: 0, first: kvm_map::pos::MOUSE + 2, codes });
        drain(&mut e);
        e.device_report(Port::Keyboard, 1, &[0b100, 0, 0], 10);
        drain(&mut e);
        assert_eq!(e.active(), Pc::Pc2);
    }

    #[test]
    fn a_field_override_makes_undeclared_bits_a_button_and_an_axis() {
        let mut e = engine();
        e.set_config(ConfigValue::Gamepad(GamepadOut::Hid), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &gamepad());
        drain(&mut e);
        let nkro = e.presentation().nkro.ep_in;
        // The padding after the 14 buttons (bit 14) as button 20, the nibble after the hat (bits 20–23) as the slider.
        let button = ctrl::FieldOverride { iface: 0, report_id: 0, bit: 14, bits: 1, flags: 0, target: 19 };
        let slider =
            ctrl::FieldOverride { iface: 0, report_id: 0, bit: 20, bits: 4, flags: 0, target: ctrl::field::AXIS + 6 };
        assert_eq!(ask(&mut e, Request::SetOverride(Device::Aux2, 0, Some(button))), Response::Ok);
        assert_eq!(ask(&mut e, Request::SetOverride(Device::Aux2, 1, Some(slider))), Response::Ok);
        let Response::Controls(_, c) = ask(&mut e, Request::GetControls(Device::Aux2)) else { panic!() };
        assert!(c.pad_buttons & (1 << 19) != 0 && c.pad_axes & (1 << 6) != 0, "{c:?}");
        e.device_report(Port::Aux2, 0, &[0, 0b0100_0000, 0xFF, 128, 128, 128, 128], 10);
        let g = read_back(&normalized::NKRO_PAD_DESC, &last_on(&drain(&mut e), nkro).unwrap()).gamepad.unwrap();
        assert_eq!((g.buttons, g.axes[6]), (1 << 19, 32767));
        assert_eq!(
            ask(&mut e, Request::GetOverride(Device::Aux2, 0)),
            Response::Override(Device::Aux2, 0, Some(button))
        );
        assert_eq!(ask(&mut e, Request::SetOverride(Device::Mouse, 0, None)), Response::Error(ErrorCode::NoDevice));
    }

    #[test]
    fn the_monitor_tells_the_active_pc_a_devices_changed_reports() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        drain(&mut e);
        let start = |e: &mut Engine, pc| ask_from(e, Some(pc), Request::MonitorDevice(Device::Keyboard, true));
        assert_eq!(start(&mut e, Pc::Pc2), Response::Error(ErrorCode::NotActive));
        assert_eq!(start(&mut e, Pc::Pc1), Response::Ok);
        let told = |e: &mut Engine, d: &[u8], t| {
            e.device_report(Port::Keyboard, 1, d, t);
            events_to(&drain(e), Pc::Pc1).into_iter().filter(|ev| matches!(ev, Event::Report { .. })).count()
        };
        assert_eq!(told(&mut e, &[1, 5, 0], 100), 1);
        assert_eq!(told(&mut e, &[1, 5, 0], 200), 0, "unchanged");
        assert_eq!(told(&mut e, &[1, 6, 0], 210), 1);
        assert_eq!(told(&mut e, &[1, 7, 0], 215), 0, "too soon after the last told");
        assert_eq!(told(&mut e, &[1, 8, 0], 240), 1);
        let r = ask(&mut e, Request::GetDeviceMap { device: Device::Keyboard, index: 1, offset: 0 });
        let Response::DeviceMap { iface, interfaces, total, .. } = r else { panic!("{r:?}") };
        assert_eq!((iface, interfaces, total), (1, 2, 50));
        // Not asked again: it ends.
        e.tick(WATCH_MS + 300);
        assert_eq!(told(&mut e, &[1, 9, 0], WATCH_MS + 400), 0);
    }

    #[test]
    fn a_bluetooth_host_gets_the_gamepad_or_the_profiles_fallback() {
        let mut e = with_bt();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &gamepad());
        // Host 2 subscribed to every report, the gamepad's too.
        e.bt_host(BtHostState { reports: btreports::KNOWN | btreports::ALL, ..bt_host(2, BT_READY) });
        assert_eq!(ask(&mut e, Request::SetActive(Target::Bt2)), Response::Ok);
        drain(&mut e);
        e.device_report(Port::Aux2, 0, &[0b1, 0, 0x0F, 128, 128, 128, 128], 10);
        let out = bt_inputs(&drain(&mut e));
        let pad = out.iter().rev().find(|(h, id, _)| *h == 2 && *id == btreport::GAMEPAD).expect("a gamepad report");
        assert_eq!((pad.2[0], pad.2[4]), (1, kvm_hid::gamepad::HAT_CENTRE), "button 1, the hat at rest");
        use ctrl::outputs as o;
        let all = o::KEYBOARD | o::MOUSE | o::MEDIA | o::POINTER | o::GAMEPAD;
        assert_eq!(
            ask(&mut e, Request::GetOutputs(Host::Bt2)),
            Response::Outputs { host: Host::Bt2, offered: all, taken: all }
        );
        // Now it takes no gamepad: profile 0's fallback keymap 1 types A for button 1.
        e.set_config(ConfigValue::ProfileFallback(0, Some(1)), Origin::Software).unwrap();
        ask(
            &mut e,
            Request::SetKeycodes {
                keymap: 1,
                layer: 0,
                first: kvm_map::pos::PAD,
                codes: Vec::from_slice(&[0x04]).unwrap(),
            },
        );
        let some = btreports::KNOWN | btreports::KEYBOARD | btreports::MOUSE | btreports::CONSUMER;
        e.bt_host(BtHostState { reports: some, ..bt_host(2, BT_READY) });
        e.device_report(Port::Aux2, 0, &[0, 0, 0x0F, 128, 128, 128, 128], 20);
        drain(&mut e);
        e.device_report(Port::Aux2, 0, &[0b1, 0, 0x0F, 128, 128, 128, 128], 30);
        let out = bt_inputs(&drain(&mut e));
        assert!(out.iter().all(|(_, id, _)| *id != btreport::GAMEPAD), "{out:?}");
        assert!(out.iter().any(|(h, id, d)| *h == 2 && *id == btreport::KEYBOARD && d[2] == 0x04), "{out:?}");
        let Response::Outputs { taken, .. } = ask(&mut e, Request::GetOutputs(Host::Bt2)) else { panic!() };
        assert_eq!(taken & o::GAMEPAD, 0);
    }

    #[test]
    fn the_keymaps_shared_entries_macros_and_settings_over_the_control_protocol() {
        let mut e = engine();
        let combo: Vec<u8, { ctrl::MAP_ENTRY_MAX }> =
            Vec::from_slice(&[0x04, 0, 0x05, 0, 0, 0, 0, 0, 0x29, 0]).unwrap();
        assert_eq!(ask(&mut e, Request::SetMapEntry { kind: 1, index: 3, data: combo.clone() }), Response::Ok);
        assert_eq!(
            ask(&mut e, Request::GetMapEntry { kind: 1, index: 3 }),
            Response::MapEntry { kind: 1, index: 3, data: combo }
        );
        assert_eq!(ask(&mut e, Request::GetMapEntry { kind: 4, index: 0 }), Response::Error(ErrorCode::InvalidValue));
        assert_eq!(
            ask(&mut e, Request::SetMacros { offset: 0, data: Vec::from_slice(b"hi\0").unwrap() }),
            Response::Ok
        );
        let Response::Macros { total, data, .. } = ask(&mut e, Request::GetMacros(0)) else { panic!() };
        assert_eq!((total as usize, &data[..3]), (kvm_map::keymap::MACRO_BUFFER, &b"hi\0"[..]));
        assert_eq!(ask(&mut e, Request::SetMapSetting(7, 250)), Response::Ok);
        assert_eq!(ask(&mut e, Request::GetMapSetting(7)), Response::MapSetting(7, 250));
        assert_eq!(ask(&mut e, Request::GetMapSetting(999)), Response::Error(ErrorCode::InvalidValue));
    }

    #[test]
    fn macro_text_follows_the_hosts_keyboard_layout() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.set_config(ConfigValue::ProfileKeymap(0, Some(0)), Origin::Software).unwrap();
        let de = ConfigValue::HostLayout(Host::Pc1, kvm_hid::layouts::Layout::De as u8);
        e.set_config(de, Origin::Software).unwrap();
        ask(&mut e, Request::SetMacros { offset: 0, data: Vec::from_slice(b"z\0").unwrap() });
        ask(
            &mut e,
            Request::SetKeycodes { keymap: 0, layer: 0, first: 0x3A, codes: Vec::from_slice(&[0x7700]).unwrap() },
        );
        drain(&mut e);
        let nkro = e.presentation().nkro.ep_in;
        e.device_report(Port::Keyboard, 0, &[0, 0, 0x3A, 0, 0, 0, 0, 0], 10);
        let mut typed = std::vec::Vec::new();
        for t in (10..40).step_by(2) {
            e.tick(t);
            for (_, ep, d) in reports(&drain(&mut e)) {
                if ep == nkro
                    && let Some(k) = read_back(normalized::NKRO_DESC, &d).keyboard
                {
                    typed.extend(k.keys());
                }
            }
        }
        // A German host's z is on the key where a US one has y.
        assert!(typed.contains(&0x1C) && !typed.contains(&0x1D), "{typed:?}");
    }

    #[test]
    fn a_saved_profile_keymap_runs_from_the_start() {
        let mut config = Config::default();
        config.set(ConfigValue::ProfileKeymap(0, Some(2)), crate::config::Origin::Software).unwrap();
        let info =
            Info { proto: 1, rp_fw: 0x000100, at_fw: [0; 2], hw: 0x000C00, serial: *b"TEST0001", caps: 0, bt_fw: 0 };
        let e = Engine::new(config, info, "TEST0001");
        assert_eq!((e.keymap_now, e.vial.keymap.active()), (Some(2), 2));
    }

    /// Device outputs (reports to a device's OUT endpoint) in a run of outputs: (port, interface, data).
    fn device_outs(out: &[Output]) -> std::vec::Vec<(Port, u8, std::vec::Vec<u8>)> {
        out.iter()
            .filter_map(|o| match o {
                Output::DeviceOut { port, iface, data } => Some((*port, *iface, data.to_vec())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_wireless_receivers_pad_is_read_when_normalized() {
        use crate::usbdesc::fixtures::RECEIVER_CONFIG;
        use kvm_hid::xinput;
        let mut e = engine();
        e.set_config(ConfigValue::Gamepad(GamepadOut::Hid), Origin::Software).unwrap();
        let rx = Probed {
            device: DeviceDesc { vid: 0x045E, pid: 0x0719, ..Default::default() },
            speed: Some(Speed::Full),
            config: Vec::from_slice(RECEIVER_CONFIG).unwrap(),
            ..Default::default()
        };
        // Auto: the PC gets the receiver as it is.
        e.port_attached(Port::Aux2, &rx);
        assert_eq!(e.port_info(Port::Aux2).state, PortState::Passthrough);
        // Normalized: the KVM reads its first pad, and asks the receiver whether one is linked.
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Aux2, &rx);
        let out = drain(&mut e);
        assert_eq!(
            (e.port_info(Port::Aux2).state, e.port_info(Port::Aux2).effective),
            (PortState::Hosted, PortMode::Normalized)
        );
        assert!(device_outs(&out).contains(&(Port::Aux2, 0, xinput::INQUIRY.to_vec())), "{:?}", device_outs(&out));
        // A pad links: its player LED.
        e.device_report(Port::Aux2, 0, &[0x08, 0x80], 10);
        assert_eq!(device_outs(&drain(&mut e)), [(Port::Aux2, 0, xinput::led(0).to_vec())]);
        // Its input: A and the left stick to the right, on the virtual gamepad.
        let mut r = [0u8; 29];
        r[1] = 0x01;
        r[6] = 0x00;
        r[7] = 0x10;
        r[10..12].copy_from_slice(&20000i16.to_le_bytes());
        e.device_report(Port::Aux2, 0, &r, 20);
        let nkro = e.presentation().nkro.ep_in;
        let g = read_back(&normalized::NKRO_PAD_DESC, &last_on(&drain(&mut e), nkro).unwrap()).gamepad.unwrap();
        assert_eq!((g.buttons, g.axes[0]), (1, 20000));
        // What it has, in the standard order.
        let Response::Controls(_, c) = ask(&mut e, Request::GetControls(Device::Aux2)) else { panic!() };
        assert_eq!((c.pad_buttons, c.hat), (xinput::BUTTONS_PRESENT, true));
        // The pad leaves: everything let go.
        e.device_report(Port::Aux2, 0, &[0x08, 0x00], 30);
        let g = read_back(&normalized::NKRO_PAD_DESC, &last_on(&drain(&mut e), nkro).unwrap()).gamepad.unwrap();
        assert_eq!((g.buttons, g.axes[0]), (0, 0));
    }

    #[test]
    fn a_symmetric_pad_reaches_the_virtual_gamepad_in_the_standard_order() {
        // As the 5 series lays out report 1: X Y Z Rz Rx Ry, a vendor byte, the hat and 15 buttons, vendor data.
        #[rustfmt::skip]
        let desc = &[
            0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x85, 0x01,
            0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x09, 0x33, 0x09, 0x34, 0x15, 0x00, 0x26, 0xFF, 0x00,
            0x75, 0x08, 0x95, 0x06, 0x81, 0x02,
            0x06, 0x00, 0xFF, 0x09, 0x20, 0x95, 0x01, 0x81, 0x02,
            0x05, 0x01, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07, 0x35, 0x00, 0x46, 0x3B, 0x01, 0x65, 0x14, 0x75, 0x04,
            0x95, 0x01, 0x81, 0x42,
            0x65, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x0F, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x0F, 0x81, 0x02,
            0x06, 0x00, 0xFF, 0x09, 0x21, 0x95, 0x0D, 0x81, 0x02,
            0x06, 0x00, 0xFF, 0x09, 0x22, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x34, 0x81, 0x02,
            0xC0,
        ][..];
        let mut e = engine();
        e.set_config(ConfigValue::Gamepad(GamepadOut::Hid), Origin::Software).unwrap();
        e.set_config(ConfigValue::Mode(Device::Aux2, PortMode::Normalized), Origin::Software).unwrap();
        let mut p = keyboard_and_mouse();
        p.device = DeviceDesc { vid: 0x054C, pid: 0x0CE6, ..Default::default() };
        p.report_descs[0].1 = Vec::from_slice(desc).unwrap();
        e.port_attached(Port::Aux2, &p);
        drain(&mut e);
        let nkro = e.presentation().nkro.ep_in;
        // Cross (its button 2) and the right stick pushed left (its Z), a finger on the touch pad.
        let mut r = [0u8; 64];
        r[0] = 1;
        r[1..5].copy_from_slice(&[128, 128, 0, 128]);
        r[8] = 0x08 | 0x20;
        r[1 + 32] = 0x01;
        e.device_report(Port::Aux2, 0, &r, 10);
        let g = read_back(&normalized::NKRO_PAD_DESC, &last_on(&drain(&mut e), nkro).unwrap()).gamepad.unwrap();
        assert_eq!(g.buttons, 1 | 1 << (crate::pads::TOUCH_BUTTON - 1), "the bottom face button, the touch");
        assert_eq!(g.axes[3], -32767, "the right stick on Rx");
        let Response::Controls(_, c) = ask(&mut e, Request::GetControls(Device::Aux2)) else { panic!() };
        assert!(c.pad_buttons & (1 << 16) != 0 && c.pad_buttons & (1 << 19) != 0, "Home and the touch: {c:?}");
    }

    #[test]
    fn a_symmetric_pad_over_bluetooth_classic_reads_as_on_a_usb_port() {
        // The ESP32 module's form of it (`kvm_classic::pad`), with the USB IDs that pick its profile.
        let mut e = with_bt();
        e.set_config(ConfigValue::Gamepad(GamepadOut::Hid), Origin::Software).unwrap();
        let map = kvm_classic::pad::DESCRIPTOR;
        let state = DeviceState { vid: 0x054C, pid: 0x0CE6, ..wireless_state(2, map) };
        e.bt_device(state, "Pad");
        assert!(drain(&mut e).contains(&Output::Bt(BtOut::RequestMap(2))));
        let chunk = kvm_proto::link::bt::MAP_CHUNK;
        for (i, piece) in map.chunks(chunk).enumerate() {
            e.bt_device_map(2, (i * chunk) as u16, map.len() as u16, piece);
        }
        drain(&mut e);
        let nkro = e.presentation().nkro.ep_in;
        // Cross (its button 2) and the right stick pushed left (its Z), a finger on the touch pad.
        let mut d = [0u8; kvm_classic::pad::USB_LEN];
        d[..4].copy_from_slice(&[128, 128, 0, 128]);
        d[7] = 0x08 | 0x20;
        d[32] = 0x01;
        e.bt_device_input(2, 1, &d, 10);
        let g = read_back(&normalized::NKRO_PAD_DESC, &last_on(&drain(&mut e), nkro).unwrap()).gamepad.unwrap();
        assert_eq!(g.buttons, 1 | 1 << (crate::pads::TOUCH_BUTTON - 1), "the bottom face button, the touch");
        assert_eq!(g.axes[3], -32767, "the right stick on Rx");
    }
}
