//! The KVM's logic without I/O (software.md §4.3–§4.5): ports, switching, hotkeys, presentation, control requests.
//!
//! The RP2354B firmware feeds events in (device reports, link messages, the key, timers) and executes the
//! [`Output`]s that come back. `odkvmctl --sim` drives the very same engine with fake devices.

use heapless::{Deque, String, Vec};
use kvm_hid::neutral::neutral_report;
use kvm_hid::parser::ReportType;
use kvm_hid::translate::{ConsumerState, caps};
use kvm_hid::{KeyState, Layout, MouseInput, Translator};
use kvm_proto::crc::crc32;
use kvm_proto::ctrl::diag::{self as cdiag, BtModule, ClockSource, Grade, Item, id};
use kvm_proto::ctrl::{
    self, Advertised, AutoReason, ConfigValue, ErrorCode, Event, FaultKind, FwState, HopState, Hotkey, Info, KeyAction,
    LearnEnd, LinkState, Pc, Port, PortInfo, PortMode, PortState, PowerSource, RebootTarget, Request, Response, Speed,
    StringKind, SwitchSource, Target, VialMode, status_flags,
};
use kvm_proto::link::bt::{EventKind as BtEventKind, HostState as BtHostState, report as btreport};
use kvm_proto::link::msg::{Status as AtStatus, UsbSpeed, UsbState};

use crate::bt::{self, BtOut, BtState, PasskeyStep};
use crate::config::{self, Config, Origin};
use crate::diag::{self, BoardDiag};
use crate::hop;
use crate::hotkey::{Action, HotkeyEngine, Learner};
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
pub const SAVE_ITEM_MAX: usize = kvm_vial::keymap::MACRO_BUFFER;
/// Electronics' own draw, subtracted from the advertised current for the budget (design.md §6, to be measured).
pub const SELF_CURRENT_MA: u16 = 300;
/// The Bluetooth module's average draw on 5 V when fitted (design.md §6.1: about 40 mA at 3.3 V with hosts connected,
/// through the 90 % buck); its +9 dBm transmit peaks (190 mA at 3.3 V) last a few hundred microseconds.
pub const BT_CURRENT_MA: u16 = 30;
/// Autosave waits this long after the last change, so that a run of changes (a slider dragged, Vial setting keys)
/// makes one save.
const AUTOSAVE_MS: u32 = 5000;
/// Flash wear cap (the `store` module documentation has the numbers): saves go ahead as they come up to `SAVE_BURST`
/// in a row, then at most one per `SAVE_PACE_MS`, which is also how fast the allowance comes back. Setting the KVM up
/// never meets it. A host script that changes, or changes and saves, the config every few seconds, day and night, is
/// held to one save a minute: 1 440 a day, about 17 years before a sector reaches its rated erases with the config at
/// its largest. Unpaced, a change every 6 s would take under two years, and `SaveConfig` in a loop days.
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

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FwOp {
    Begin {
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
    summary: DeviceSummary,
    /// Length of the last raw report per interface (for neutral XInput reports).
    last_len: Vec<(u8, u8), MAX_HID_IFACES>,
}

impl Hosted {
    fn translator(&self, iface: u8) -> Option<&Translator> {
        self.translators.iter().find(|(n, _)| *n == iface).map(|(_, t)| t)
    }
}

struct PortSlot {
    state: PortState,
    effective: PortMode,
    reason: AutoReason,
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

pub struct Engine {
    config: Config,
    /// Unsaved changes (config or keymap) since then; saved [`AUTOSAVE_MS`] after the last one.
    dirty_since: Option<u32>,
    config_dirty: bool,
    /// Saves that may go ahead now, up to [`SAVE_BURST`]; one more each [`SAVE_PACE_MS`] after `save_paced`.
    save_allowance: u8,
    save_paced: u32,
    /// Keymap, keymap engine and Vial sessions (software.md §4.11).
    vial: kvm_vial::Vial,
    keymap_changes: u32,
    hotkeys: HotkeyEngine,
    /// Normalized sources (forwarded through the KVM's interfaces) and mirrored ones (forwarded raw).
    norm: Merger,
    mirr: Merger,
    active: Pc,
    suppressed_keys: KeyState,
    suppressed_buttons: u16,
    forwarded_keys: KeyState,
    forwarded_consumer: ConsumerState,
    forwarded_system: u8,
    /// Mouse buttons the keymap holds.
    keymap_buttons: u8,
    pcs: [PcSlot; 2],
    ports: [PortSlot; 3],
    presentation: Presentation,
    desc: Vec<u8, { kvm_proto::descset::MAX_LEN }>,
    out: Deque<Output, OUTPUT_QUEUE>,
    power: PowerInfo,
    info: Info,
    serial: String<16>,
    fw: (FwState, u32),
    /// The firmware's update writer is busy with flash: FwBegin/FwData get Busy, and the host retries.
    fw_busy: bool,
    identify_until: u32,
    /// `ctrl::selftest` bits, and when the self-test finished (its result shows on the LEDs for a while).
    selftest: u32,
    /// The Bluetooth module as the firmware sees it (`set_bt_module`): absent, up, being programmed, failed.
    bt_module: BtModule,
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
    /// The Bluetooth module and its hosts (software.md §4.13).
    bt: BtState,
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

/// Which software locks a tap on the key would lift, and until when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unlock {
    switching: bool,
    config: bool,
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
            save_allowance: SAVE_BURST,
            save_paced: 0,
            vial: kvm_vial::Vial::new(),
            keymap_changes: 0,
            hotkeys: HotkeyEngine::EMPTY,
            norm: Merger::new(),
            mirr: Merger::new(),
            active: Pc::Pc1,
            suppressed_keys: KeyState::EMPTY,
            suppressed_buttons: 0,
            forwarded_keys: KeyState::EMPTY,
            forwarded_consumer: ConsumerState { usages: [0; kvm_hid::normalized::CONSUMER_SLOTS] },
            forwarded_system: 0,
            keymap_buttons: 0,
            pcs: [PcSlot::NEW, PcSlot::NEW],
            ports: [PortSlot::new(), PortSlot::new(), PortSlot::new()],
            presentation: Presentation::EMPTY,
            desc: Vec::new(),
            out: Deque::new(),
            power: PowerInfo { source: PowerSource::PcBus, adv_pwr: Advertised::None, vbus_pwr_mv: 0 },
            info: Info { proto: 0, rp_fw: 0, at_fw: [0; 2], hw: 0, serial: [0; 8], caps: 0, bt_fw: 0 },
            serial: String::new(),
            fw: (FwState::Idle, 0),
            fw_busy: false,
            identify_until: 0,
            selftest: ctrl::selftest::RUNNING,
            bt_module: BtModule::Absent,
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
            bt: BtState::EMPTY,
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

    pub fn port_info(&self, port: Port) -> PortInfo {
        let s = &self.ports[port.index()];
        let mut p = PortInfo::empty(port, self.config.port_modes()[port.index()]);
        p.state = s.state;
        p.effective = s.effective;
        p.reason = s.reason;
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
                let pc = self.passthrough_pc(s.effective);
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

    pub fn status(&self, asking: Option<Pc>) -> ctrl::Status {
        let adv_pc =
            |i: usize| self.pcs[i].status.map_or(Advertised::None, |s| Advertised::from_cc_mv(s.cc1_mv.max(s.cc2_mv)));
        let adv = [adv_pc(0), adv_pc(1)];
        let available = match self.power.source {
            PowerSource::PwrPort => self.power.adv_pwr.milliamps(),
            PowerSource::PcBus => adv[0].milliamps().max(adv[1].milliamps()),
        };
        // A fitted module draws its current whether it answers or not (starting, being programmed, failed).
        let own = SELF_CURRENT_MA + if self.bt_module != BtModule::Absent { BT_CURRENT_MA } else { 0 };
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
        if self.budget_low(budget) {
            flags |= status_flags::BUDGET_LOW;
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
            bt_module: self.bt_module,
            hop: self.hop_state(),
            pointer: self.hop.position_on(&self.config.screens, self.active),
        }
    }

    /// Whether screen hop moves the pointer now, and if not, why not (`Status.hop`).
    pub fn hop_state(&self) -> HopState {
        let mouse = &self.ports[Port::Mouse.index()];
        let mouse_elsewhere =
            matches!(mouse.state, PortState::Hosted | PortState::Passthrough) && !mouse.is_normalized();
        if !self.config.hop_on() {
            HopState::Off
        } else if self.bt.active.is_some() {
            HopState::Wireless
        } else if !self.config.abs_pointer {
            HopState::NoAbsolute
        } else if mouse_elsewhere || !self.presentation.mouse_abs {
            HopState::NotNormalized
        } else if self.pcs[self.active.index()].out.boot_mouse {
            HopState::BootProtocol
        } else if !hop::has_screens(&self.config.screens, self.active) {
            HopState::NoScreens
        } else {
            HopState::Active
        }
    }

    /// Where screen hop's pointer is (the slot of its screen, and the position in the arrangement).
    pub fn hop_pointer(&self) -> (Option<usize>, [i16; 2]) {
        (self.hop.screen(), self.hop.position())
    }

    fn budget_low(&self, budget: u16) -> bool {
        let need: u16 = self.ports.iter().filter_map(|p| p.dev.as_ref()).map(|d| d.summary.max_power_ma).sum();
        budget != 0 && need > budget
    }

    pub fn led_inputs(&self) -> LedInputs {
        let port = |i: usize| (self.ports[i].state, self.ports[i].effective);
        LedInputs {
            active: self.active,
            links: [self.pcs[0].link, self.pcs[1].link],
            ports: [port(0), port(1), port(2)],
            color_pc1: self.config.color_pc1,
            color_pc2: self.config.color_pc2,
            brightness: self.config.led_brightness,
            identify: self.now < self.identify_until,
            awaiting_key: self.fw.0 == FwState::AwaitingKey || self.unlock.is_some(),
            budget_low: self.status(None).flags & status_flags::BUDGET_LOW != 0,
            selftest: self.selftest_showing().then_some(self.selftest),
            standby: self.standby.plan().mode != PowerMode::Active,
            standby_glow: self.config.standby_glow,
            fault: self.selftest & ctrl::selftest::RUNNING == 0 && self.selftest & ctrl::selftest::ON_KEY != 0,
            profile: self
                .profile_changed_at
                .filter(|t| self.now.wrapping_sub(*t) < PROFILE_SHOW_MS)
                .map(|_| self.config.profile),
            bt_active: self.bt.active.is_some(),
            bt_pairing: self.bt.pairing_until.is_some(),
            bt_passkey: self.bt.passkey.is_some(),
        }
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
    }

    fn event_to(&mut self, pc: Pc, e: Event) {
        if let Ok(raw) = e.encode() {
            let ep = self.presentation.control.ep_in;
            self.send(pc, ep, report(&raw));
        }
    }

    fn config_changed(&mut self, key: u16) {
        // A profile that isn't remembered across power cycles needs no flash write.
        if key != ctrl::key::PROFILE || self.config.remember_profile {
            self.dirty_since = Some(self.now);
            self.config_dirty = true;
        }
        self.event(Event::ConfigChanged(key));
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
        policy::wide(modes, mirror_in, self.config.vial == VialMode::On)
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
        }
    }

    fn build_presentation(&self, buf: &mut [u8]) -> Option<(usize, Presentation)> {
        let configs: [Option<_>; 3] = core::array::from_fn(|i| {
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
            let port = Port::from_index(i).unwrap_or(Port::Other);
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
                let _ = out.write_str(kvm_vial::SERIAL_MAGIC);
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
        };
        // If the mirrors don't fit, the PC still gets the KVM's own interfaces.
        presentation::build(&params, buf)
            .or_else(|_| presentation::build(&Params { mirrors: &[], identity: None, normalized: true, ..params }, buf))
            .ok()
    }

    // ── switching ────────────────────────────────────────────────────────────

    pub fn switch(&mut self, target: Target, source: SwitchSource) {
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
                self.apply_leds();
                self.event(Event::BtActive(0));
                self.event(Event::Switched(pc, source));
                return;
            }
        } else if pc == self.active {
            return;
        }
        self.end_learning(LearnEnd::Cancelled, Hotkey::NONE);
        let old = self.active;
        if from_bt.is_none() {
            // 1. Release everything on the PC we leave.
            self.release_pc(old);
        }
        self.active = pc;
        // Screen hop's pointer is found where it was left on that PC (a hop put it at the edge already).
        self.hop.leave(&self.config.screens, old);
        if source != SwitchSource::Edge {
            self.hop.enter(&self.config.screens, pc);
        }
        // 3. The keyboard shows the new PC's lock LEDs.
        self.apply_leds();
        // 4. Passthrough ports that follow the active PC move along.
        for i in 0..3 {
            if self.ports[i].state == PortState::Passthrough && self.ports[i].effective == PortMode::Passthrough {
                let port = Port::from_index(i).unwrap_or(Port::Other);
                let power_cycle = self.config.power_cycle_on_switch;
                self.emit(Output::Route { port, route: Route::Pc(pc), power_cycle });
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

    /// Selects Bluetooth host slot `n` (bonded; it gets input once ready).
    fn switch_bt(&mut self, n: u8, source: SwitchSource) {
        if !self.bt.selectable(n) || self.bt.active == Some(n) {
            return;
        }
        let _ = source;
        self.end_learning(LearnEnd::Cancelled, Hotkey::NONE);
        match self.bt.active {
            Some(_) => self.leave_bt(),
            None => self.release_pc(self.active),
        }
        self.bt.active = Some(n);
        self.bt.sent_keyboard = [0; btreport::KEYBOARD_LEN];
        self.bt.sent_consumer = 0;
        self.emit(Output::Bt(BtOut::Select(n)));
        self.apply_leds();
        self.event(Event::BtActive(n));
    }

    /// Releases everything on the active Bluetooth host and deselects it.
    fn leave_bt(&mut self) {
        let Some(n) = self.bt.active.take() else { return };
        self.bt_input(n, btreport::KEYBOARD, &[0; btreport::KEYBOARD_LEN]);
        self.bt_input(n, btreport::MOUSE, &[0; btreport::MOUSE_LEN]);
        self.bt_input(n, btreport::CONSUMER, &[0; btreport::CONSUMER_LEN]);
        self.hold_over();
        self.emit(Output::Bt(BtOut::Select(0)));
    }

    fn bt_input(&mut self, host: u8, id: u8, data: &[u8]) {
        let data = Vec::from_slice(&data[..data.len().min(btreport::MOUSE_LEN)]).unwrap_or_default();
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

    /// Sends the active PC's LED report to every hosted keyboard.
    fn apply_leds(&mut self) {
        let leds = match self.bt.active {
            Some(n) => self.bt.leds[n as usize - 1],
            None => self.pcs[self.active.index()].out.leds,
        };
        for i in 0..3 {
            let Some(d) = self.ports[i].dev.as_ref().filter(|_| self.ports[i].state == PortState::Hosted) else {
                continue;
            };
            let port = Port::from_index(i).unwrap_or(Port::Other);
            let mut targets: Vec<(u8, u8), 4> = Vec::new();
            for (iface, t) in &d.translators {
                if t.verdict().caps & caps::KEYBOARD_LEDS == 0 {
                    continue;
                }
                let id = t.layout().report_ids(ReportType::Output).next().unwrap_or(0);
                let _ = targets.push((*iface, id));
            }
            for (iface, id) in targets {
                let data = if id != 0 { report(&[id, leds]) } else { report(&[leds]) };
                self.emit(Output::DeviceSetReport { port, iface, report_type: 2, report_id: id, data });
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
        let summary = match parse_config(&probed.config) {
            Some(cfg) => DeviceSummary::new(&probed.device, &cfg, probed.speed.unwrap_or(Speed::Unknown), |n| {
                translators.iter().find(|(i, _)| *i == n).map(|(_, t)| t.verdict())
            }),
            None => DeviceSummary::default(),
        };
        let unsupported = summary.interfaces.is_empty();
        let s = &mut self.ports[port.index()];
        s.dev = Some(Hosted { probed: probed.clone(), translators, summary, last_len: Vec::new() });
        s.probe_until = None;
        s.reports = 0;
        s.rate_hz = 0;
        if unsupported {
            s.state = PortState::Unsupported;
            self.event(Event::PortChanged(port));
            return;
        }
        s.state = PortState::Hosted;
        let mut touched = [false; 3];
        touched[port.index()] = true;
        self.redecide(touched);
    }

    /// Decides every port's mode again (attach, unplug, a mode or Vial change). The ports compete for the mirror
    /// endpoints and identity, so a change on one can move another; those already mirrored keep their endpoints, and
    /// the one with identity keeps it. Ports `touched` are reported as changed even if their mode stays.
    fn redecide(&mut self, touched: [bool; 3]) {
        let rank = |p: &PortSlot| match (p.is_mirrored(), p.effective) {
            (true, PortMode::MirroredIdentity) => 0,
            (true, _) => 1,
            _ => 2,
        };
        let mut order: Vec<Port, 3> = Port::ALL.iter().copied().collect();
        order.sort_unstable_by_key(|p| (rank(&self.ports[p.index()]), p.index()));
        let settings = self.config.port_modes();
        let decided = {
            let candidates: Vec<Candidate, 3> = order
                .iter()
                .filter_map(|&port| {
                    let s = &self.ports[port.index()];
                    let d = s.dev.as_ref().filter(|_| matches!(s.state, PortState::Hosted | PortState::Passthrough))?;
                    Some(Candidate { port, setting: settings[port.index()], dev: &d.summary })
                })
                .collect();
            policy::decide_all(&candidates, self.config.vial == VialMode::On)
        };
        let mut changed = [false; 3];
        for &port in Port::ALL {
            let i = port.index();
            let Some((eff, reason)) = decided[i] else { continue };
            let was = (self.ports[i].state, self.ports[i].effective, self.ports[i].reason);
            self.ports[i].effective = eff;
            self.ports[i].reason = reason;
            if eff.is_passthrough() {
                let target = self.passthrough_pc(eff);
                self.ports[i].state = PortState::Passthrough;
                self.release_port(port);
                if was.0 != PortState::Passthrough || was.1 != eff {
                    self.emit(Output::Route { port, route: Route::Pc(target), power_cycle: false });
                }
            } else if was.0 == PortState::Passthrough {
                // Coming back from passthrough: the KVM has to see the device again first.
                self.reprobe(port);
            }
            changed[i] = was != (self.ports[i].state, eff, reason);
        }
        // Always, against what the PCs have: a port coming back from passthrough is marked mirrored while it is still
        // probing, so on attach its mode looked unchanged, the set kept no mirror for it, and its reports went
        // nowhere (first board, 2026-11-12). The PCs only hear of a set whose bytes changed.
        self.rebuild_presentation();
        for &port in Port::ALL {
            if changed[port.index()] || touched[port.index()] {
                self.event(Event::PortChanged(port));
            }
        }
    }

    /// The PC a port in passthrough mode `eff` is routed to: pinned, or the active one.
    fn passthrough_pc(&self, eff: PortMode) -> Pc {
        match eff {
            PortMode::PassthroughPc1 => Pc::Pc1,
            PortMode::PassthroughPc2 => Pc::Pc2,
            _ => self.active,
        }
    }

    fn release_port(&mut self, port: Port) {
        let m = self.norm.remove_port(port);
        self.forward_merged(m, None);
        self.mirr.remove_port(port);
    }

    pub fn port_detached(&mut self, port: Port) {
        let i = port.index();
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
        self.redecide([false; 3]);
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
        let i = port.index();
        if self.ports[i].state != PortState::Hosted {
            return;
        }
        self.ports[i].reports += 1;
        let normalized = self.ports[i].is_normalized();
        let Some(d) = self.ports[i].dev.as_mut() else { return };
        match d.last_len.iter_mut().find(|(n, _)| *n == iface) {
            Some(e) => e.1 = data.len() as u8,
            None => {
                let _ = d.last_len.push((iface, data.len() as u8));
            }
        }
        let t = d.translator(iface).map(|t| t.translate(data)).unwrap_or_default();
        let merged = if normalized { self.norm.apply(port, iface, &t) } else { self.mirr.apply(port, iface, &t) };

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
        } else if let Some(m) = self.presentation.mirror_of(port, iface).copied() {
            let mut raw = report(data);
            let mask = swallowed.union(&self.suppressed_keys);
            if let Some(t) = self.ports[i].dev.as_ref().and_then(|d| d.translator(iface)) {
                t.strip(&mut raw, &mask, self.suppressed_buttons);
            }
            let pc = self.active;
            self.send(pc, m.f.ep_in, raw);
        }
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
        let keys = self.norm.keys().union(&self.mirr.keys());
        let Some(entry) = self.bt.passkey.as_mut() else { return };
        let host = entry.host;
        let value = match entry.feed(&keys) {
            PasskeyStep::Typing => return,
            PasskeyStep::Done(v) => Some(v),
            PasskeyStep::Cancel => None,
        };
        self.bt.passkey = None;
        self.suppressed_keys = keys;
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
        let union = self.norm.keys().union(&self.mirr.keys());
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
            let target = match a {
                Action::Toggle => Target::Toggle,
                Action::SelectPc1 => Target::Pc1,
                Action::SelectPc2 => Target::Pc2,
                Action::Next => Target::Next,
            };
            self.switch(target, SwitchSource::Hotkey);
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
        let held = self.norm.keys().union(&self.mirr.keys());
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
        if self.config.keymap {
            let steps = self.vial.remap.input(&mut self.vial.keymap, &keys, self.now);
            self.apply_steps(&steps, &keys);
            self.note_keymap_changes();
        } else {
            self.send_keyboard(keys);
        }
        if let Some(mouse) = m.mouse {
            self.suppressed_buttons &= mouse.buttons | self.mirr.buttons();
            self.send_mouse(mouse);
        }
        if m.consumer.is_some() || m.system.is_some() {
            let extra = if self.config.keymap { *self.vial.remap.output() } else { kvm_vial::remap::Out::default() };
            self.send_media(&extra);
        }
    }

    /// Sends the keymap engine's steps to the active PC; `keys` are the physical keys it was given.
    fn apply_steps(&mut self, steps: &[kvm_vial::Step], keys: &KeyState) {
        // Usages beyond the matrix (0xE8–0xFF, never sent in practice) pass through as they are.
        let mut beyond = KeyState::EMPTY;
        for k in keys.keys().filter(|k| *k as usize >= kvm_vial::POSITIONS) {
            beyond.press(k);
        }
        for step in steps {
            match step {
                kvm_vial::Step::State(o) => {
                    self.send_keyboard(o.keys.union(&beyond));
                    self.send_media(o);
                    if o.buttons != self.keymap_buttons {
                        self.keymap_buttons = o.buttons;
                        self.send_mouse(MouseInput::default());
                    }
                }
                // Mouse keys accelerate themselves.
                kvm_vial::Step::Motion(mo) => self.send_pointer(
                    MouseInput {
                        buttons: 0,
                        x: mo.x as i32,
                        y: mo.y as i32,
                        wheel: mo.wheel as i32,
                        pan: mo.pan as i32,
                    },
                    false,
                ),
            }
        }
        let last = self.vial.remap.output().keys.union(&beyond);
        self.send_keyboard(last);
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
    fn send_media(&mut self, extra: &kvm_vial::remap::Out) {
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
        let buttons = (self.norm.buttons() & !self.suppressed_buttons) | self.keymap_buttons as u16;
        if let Some(n) = self.bt.active {
            let buttons = buttons | (self.mirr.buttons() & !self.suppressed_buttons);
            let r = kvm_proto::link::bt::mouse_report(buttons, m.x, m.y, m.wheel, m.pan);
            self.bt_input(n, btreport::MOUSE, &r);
            return;
        }
        if self.hop_state() == HopState::Active {
            self.hop_motion(MouseInput { buttons, ..m }, accelerate);
            return;
        }
        self.send_pointer_report(pc, hop::Report::Rel([m.x, m.y]), buttons, m.wheel, m.pan);
    }

    fn send_pointer_report(&mut self, pc: Pc, motion: hop::Report, buttons: u16, wheel: i32, pan: i32) {
        let reports = self.pcs[pc.index()].out.pointer(motion, buttons, wheel, pan, &self.presentation);
        for (ep, r) in reports {
            self.send(pc, ep, r);
        }
    }

    /// How `pc` takes the absolute pointer: its mapping and its desktop.
    fn hop_map(&self, pc: Pc) -> hop::Map {
        hop::Map { mapping: self.config.mapping(pc), units: self.config.units(pc) }
    }

    /// Whether the pointer may hop onto `pc`: it is connected, awake and past its firmware setup.
    fn hop_open(&self, pc: Pc) -> bool {
        let s = &self.pcs[pc.index()];
        s.link == LinkState::Connected && !s.out.boot_mouse
    }

    /// Screen hop (software.md §4.14): the motion moves the KVM's pointer across the arrangement; the active PC gets
    /// where it is, and a push across an edge onto the other PC's screen switches to that PC.
    fn hop_motion(&mut self, m: MouseInput, accelerate: bool) {
        let pc = self.active;
        let h = self.config.hop;
        let d = if accelerate {
            self.hop.accel.apply(m.x, m.y, self.now, h.accel, h.speed)
        } else {
            [m.x as f32, m.y as f32]
        };
        let rules = hop::Rules {
            resistance: h.resistance,
            corner: h.corner,
            buttons: m.buttons != 0,
            open: [self.hop_open(Pc::Pc1), self.hop_open(Pc::Pc2)],
            maps: [self.hop_map(Pc::Pc1), self.hop_map(Pc::Pc2)],
        };
        let screens = self.config.screens;
        let step = self.hop.motion(&screens, pc, d, self.now, &rules);
        let reports =
            if step.reports.is_empty() { self.hop.here(&screens, pc, self.hop_map(pc)) } else { step.reports };
        // The wheel goes with the first report.
        let mut scroll = (m.wheel, m.pan);
        for r in reports {
            self.send_pointer_report(pc, r, m.buttons, scroll.0, scroll.1);
            scroll = (0, 0);
        }
        if let Some(to) = step.hop {
            self.switch(if to == Pc::Pc1 { Target::Pc1 } else { Target::Pc2 }, SwitchSource::Edge);
            if self.active == to && self.bt.active.is_none() {
                for r in self.hop.here(&screens, to, self.hop_map(to)) {
                    self.send_pointer_report(to, r, 0, 0, 0);
                }
            }
        }
    }

    /// A Vial request (SET_REPORT on the Vial interface); the reply goes back on its IN endpoint.
    fn vial_request(&mut self, pc: Pc, data: &[u8]) {
        let Some(v) = self.presentation.vial else { return };
        let physical = self.norm.keys().union(&self.mirr.keys());
        let ctx = kvm_vial::Context {
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
        let link = usb_to_link(st.usb_state);
        if matches!(st.usb_state, UsbState::Default | UsbState::Unpowered | UsbState::Detached) {
            // A bus reset or unplug: the PC starts over in report protocol without multipliers, its cursor anywhere.
            s.out.reset();
            self.hop.forget_cursor(pc);
        }
        s.link = link;
        s.status = Some(st);
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
    /// empty ones (a keyboard plugged in then can wake it too).
    fn wake_ports(&self) -> [bool; 3] {
        core::array::from_fn(|i| {
            self.ports[i]
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
                    if pc == self.active {
                        self.apply_leds();
                    }
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
            if let Some(resp) = self.control(Some(pc), data) {
                let ep_in = self.presentation.control.ep_in;
                self.send(pc, ep_in, report(&resp));
            }
        } else if let Some(m) = self.presentation.mirror_by_out_ep(ep).copied() {
            // Rumble, LEDs and vendor commands reach the device only from the active PC.
            if pc == self.active {
                self.emit(Output::DeviceOut { port: m.port, iface: m.src_iface, data: report(data) });
            }
        }
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
        if pc == self.active {
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

    /// Software asks to turn a lock off (`v` false: done now) or back on (armed until a tap on the key, or it times out).
    fn request_lock(&mut self, v: ConfigValue) -> Result<Response, ErrorCode> {
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
            let u = self.unlock.get_or_insert(Unlock { switching: false, config: false, until: 0 });
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
        self.unlock = self.unlock.filter(|u| u.switching || u.config);
        Ok(Response::Config(self.set_config(v, Origin::Software)?))
    }

    fn key_action(&mut self, a: KeyAction) {
        match a {
            KeyAction::None => {}
            KeyAction::TogglePc => self.switch(Target::Toggle, SwitchSource::Key),
            KeyAction::NextTarget => self.switch(Target::Next, SwitchSource::Key),
            KeyAction::NextProfile => {
                let _ = self.set_config(ConfigValue::Profile(self.config.next_profile()), Origin::Device);
            }
            KeyAction::OtherPortMode => {
                let next = match self.config.port_modes()[Port::Other.index()] {
                    PortMode::Auto => PortMode::Passthrough,
                    PortMode::Passthrough => PortMode::PassthroughPc1,
                    PortMode::PassthroughPc1 => PortMode::PassthroughPc2,
                    _ => PortMode::Auto,
                };
                let _ = self.set_config(ConfigValue::PortMode(Port::Other, next), Origin::Device);
            }
        }
    }

    pub fn set_power(&mut self, p: PowerInfo) {
        self.power = p;
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
            self.event(Event::BtChanged(0));
        }
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
        for p in Port::ALL {
            add(id::PORT_SPEED[p.index()], Grade::Info, self.port_info(*p).speed as i32);
        }
        for k in 0..2 {
            let link = self.pcs[k].link;
            let speed = match self.pcs[k].status.map(|s| s.speed) {
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
        let budget = if status.flags & status_flags::BUDGET_LOW != 0 {
            Grade::Warn
        } else if status.budget_ma == 0 {
            Grade::Info
        } else {
            Grade::Ok
        };
        add(id::BUDGET, budget, status.budget_ma as i32);
        for p in Port::ALL {
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
        let keymap = if self.config.keymap { self.vial.remap.deadline(&self.vial.keymap) } else { None };
        let unlock = self.unlock.map(|u| u.until);
        let learn = self.learning.as_ref().map(|l| l.until);
        let learn_tap = self.learning.as_ref().and_then(|l| l.learner.deadline());
        [self.hotkeys.deadline(), save, keymap, unlock, learn, learn_tap, self.bt.pairing_until]
            .into_iter()
            .flatten()
            .min_by_key(|t| t.wrapping_sub(self.now) as i32)
    }

    pub fn tick(&mut self, now: u32) {
        self.now = now;
        if let Some(k) = self.hotkeys.tick(now) {
            self.replay_tap(k);
        }
        if self.config.keymap {
            let steps = self.vial.remap.tick(&mut self.vial.keymap, now);
            if !steps.is_empty() {
                let keys = self.norm.keys().without(&self.hidden_keys()).without(&self.suppressed_keys);
                self.apply_steps(&steps, &keys);
            }
            self.note_keymap_changes();
        }
        if now.wrapping_sub(self.rate_epoch) >= 1000 {
            let dt = now.wrapping_sub(self.rate_epoch);
            for p in self.ports.iter_mut() {
                p.rate_hz = (p.reports as u64 * 1000 / dt as u64).min(u16::MAX as u64) as u16;
                p.reports = 0;
            }
            self.rate_epoch = now;
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
                // 2026-11-12, a flash drive on Other).
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
        if !self.config_dirty && !self.vial.keymap.is_dirty() {
            self.dirty_since = None;
        }
    }

    /// After [`Output::Save`]: the next changed item to write (config first, then the keymap's), encoded into `buf`
    /// (at least [`SAVE_ITEM_MAX`] bytes). It counts as written from here.
    pub fn take_save_item(&mut self, buf: &mut [u8]) -> Option<(u16, usize)> {
        if core::mem::take(&mut self.config_dirty) {
            let mut item = [0u8; config::ITEM_MAX];
            let n = self.config.encode(&mut item);
            buf[..n].copy_from_slice(&item[..n]);
            return Some((config::ITEM, n));
        }
        self.vial.keymap.take_dirty(buf)
    }

    /// At boot: an item read from flash that is not the config (keymap, macros, keymap settings).
    pub fn restore_item(&mut self, key: u16, data: &[u8]) {
        self.vial.keymap.restore(key, data);
        self.keymap_changes = self.vial.keymap.changes();
    }

    /// Store keys [`Engine::restore_item`] takes.
    pub fn item_keys() -> impl Iterator<Item = u16> {
        kvm_vial::Keymap::item_keys()
    }

    pub fn set_config(&mut self, v: ConfigValue, origin: Origin) -> Result<ConfigValue, ErrorCode> {
        let before = self.config;
        let v = self.config.set(v, origin)?;
        if self.config == before {
            return Ok(v);
        }
        match v {
            ConfigValue::HotkeyToggle(_)
            | ConfigValue::HotkeyPc1(_)
            | ConfigValue::HotkeyPc2(_)
            | ConfigValue::HotkeyNext(_)
            | ConfigValue::DoubleTapMs(_) => self.hotkeys.configure(&self.config),
            ConfigValue::BtPolicy(i, policy) => self.emit(Output::Bt(BtOut::Policy { host: i + 1, policy })),
            // The mouse interface changes: both PCs enumerate it again.
            ConfigValue::PcPolling(_) | ConfigValue::AbsPointer(_) => self.rebuild_presentation(),
            ConfigValue::Vial(_) => {
                // The endpoint budget changed: every port is decided again, then the interfaces are rebuilt.
                self.redecide([false; 3]);
                self.rebuild_presentation();
            }
            ConfigValue::Keymap(false) => {
                let _ = self.vial.remap.release_all();
                self.keymap_buttons = 0;
                let keys = self.norm.keys().without(&self.suppressed_keys);
                self.send_keyboard(keys);
                self.send_media(&kvm_vial::remap::Out::default());
            }
            _ => {}
        }
        // A port mode, the active profile or its modes changed: the ports are decided again together, so a profile
        // switch moves every port at once. (A port still probing is decided when that finishes.)
        let (was, now) = (before.port_modes(), self.config.port_modes());
        if was != now {
            self.redecide(core::array::from_fn(|i| was[i] != now[i]));
        }
        if self.config.profile != before.profile {
            self.profile_changed_at = Some(self.now);
            if v.key() != ctrl::key::PROFILE {
                // Fewer profiles in use than the active one's number.
                self.config_changed(ctrl::key::PROFILE);
            }
        }
        self.config_changed(v.key());
        Ok(v)
    }

    // ── control protocol ─────────────────────────────────────────────────────

    /// Handles one control-protocol report from `pc` (None: service port or simulator); returns the response.
    pub fn control(&mut self, pc: Option<Pc>, raw: &[u8]) -> Option<[u8; ctrl::REPORT_LEN]> {
        let request_type = raw.first().copied().unwrap_or(0);
        let (id, req) = match Request::decode(raw) {
            Ok(r) => r,
            Err(_) => {
                return Response::Error(ErrorCode::Malformed)
                    .encode(request_type, raw.get(1).copied().unwrap_or(0))
                    .ok();
            }
        };
        let resp = match self.handle(pc, &req) {
            Ok(r) => r,
            Err(e) => Response::Error(e),
        };
        resp.encode(req.type_byte(), id).ok()
    }

    fn handle(&mut self, pc: Option<Pc>, req: &Request) -> Result<Response, ErrorCode> {
        // Allowing software control again goes through the key, so it is asked for even while locked.
        if let Request::SetConfig(v @ (ConfigValue::SwSwitchAllowed(true) | ConfigValue::SwConfigAllowed(true))) = req {
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
                info.caps |= ctrl::caps::SCREEN_HOP;
                Response::Info(info)
            }
            Request::GetStatus => Response::Status(self.status(pc)),
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
            Request::SetPortMode(p, m) => {
                self.set_config(ConfigValue::PortMode(*p, *m), Origin::Software)?;
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
                let was = self.config.port_modes();
                self.config = Config::default();
                (self.config.sw_switch_allowed, self.config.sw_config_allowed) = keep_locks;
                self.hotkeys.configure(&self.config);
                self.vial.keymap.reset();
                let _ = self.vial.remap.release_all();
                self.keymap_buttons = 0;
                self.keymap_changes = self.vial.keymap.changes();
                let now = self.config.port_modes();
                self.redecide(core::array::from_fn(|i| was[i] != now[i]));
                self.rebuild_presentation();
                self.config_dirty = true;
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
            Request::FwBegin { size, crc32, .. } => {
                if self.info.caps & ctrl::caps::FW_UPDATE == 0 {
                    return Err(ErrorCode::Unsupported);
                }
                if self.fw_busy || self.out.is_full() {
                    return Err(ErrorCode::Busy);
                }
                self.emit(Output::Firmware(FwOp::Begin { size: *size, crc32: *crc32 }));
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
                self.fw = (FwState::AwaitingKey, self.fw.1);
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
                let held = self.norm.keys().union(&self.mirr.keys());
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
            Request::PointerAt(at) => {
                // Only a PC has a cursor (not the service port).
                let pc = pc.ok_or(ErrorCode::NotActive)?;
                if self.config.hop_on() {
                    let active = pc == self.active && self.bt.active.is_none();
                    let map = self.hop_map(pc);
                    self.hop.sync(&self.config.screens, pc, map, *at, self.now, active);
                }
                Response::Ok
            }
        })
    }

    // ── Bluetooth module (software.md §4.13) ─────────────────────────────────

    /// The module answered (its `Hello`, after a reset or once programmed): tell it the policies, ask for the hosts,
    /// and select the active host again (a module that restarted forgot it).
    pub fn bt_hello(&mut self, fw: u32) {
        self.bt.present = true;
        self.bt.fw = fw;
        self.bt_module = BtModule::Up;
        for (i, policy) in self.config.bt_policies.into_iter().enumerate() {
            self.emit(Output::Bt(BtOut::Policy { host: i as u8 + 1, policy }));
        }
        self.emit(Output::Bt(BtOut::RequestState));
        self.emit(Output::Bt(BtOut::Select(self.bt.active.unwrap_or(0))));
        self.event(Event::BtChanged(0));
    }

    /// The module stopped answering (or is being reprogrammed): no Bluetooth targets until it says hello again; input
    /// goes back to the home PC.
    pub fn bt_link_down(&mut self) {
        if !self.bt.present {
            return;
        }
        if self.bt.active.is_some() {
            self.switch(Target::Toggle, SwitchSource::Power);
        }
        let active = self.bt.active;
        self.bt = BtState { active, ..BtState::EMPTY };
        self.event(Event::BtChanged(0));
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
        if was != h {
            self.event(Event::BtChanged(h.host));
        }
    }

    /// An output report from host `host` (its keyboard LEDs).
    pub fn bt_output(&mut self, host: u8, id: u8, data: &[u8]) {
        if id != btreport::KEYBOARD || data.is_empty() {
            return;
        }
        let Some(slot) = (host as usize).checked_sub(1).filter(|i| *i < ctrl::BT_HOSTS) else { return };
        self.bt.leds[slot] = data[0];
        if self.bt.active == Some(host) {
            self.apply_leds();
        }
    }

    /// A pairing or bond event from the module.
    pub fn bt_event(&mut self, kind: BtEventKind, host: u8, value: u32) {
        let held = self.norm.keys().union(&self.mirr.keys());
        self.bt.event(kind, host, value, self.now, held);
        self.event(Event::BtChanged(host));
    }
}

#[cfg(test)]
// Bitmap offsets like `2 + 0x04 / 8` spell out which byte a usage lives in.
#[allow(clippy::identity_op)]
mod tests {
    use super::*;
    use crate::usbdesc::fixtures::GAMING_MOUSE_CONFIG;
    use kvm_hid::normalized;
    use kvm_proto::ctrl::{DeviceMsg, Hotkey};
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
            resets: 0,
            desc_crc: 0,
            wake_enabled,
            vdda_mv: Some(3300),
            temp_dc: Some(350),
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
        let deep = PowerPlan { mode: PowerMode::DeepStandby, ports: [false; 3], hub0: false };
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
        assert_eq!(plan.ports, [true, true, true], "a keyboard, and two empty ports");
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
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
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
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        drain(&mut e);
        assert_eq!(e.presentation().mirrors.len(), 2);

        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Passthrough), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::DescriptorSetChanged));
        assert!(e.presentation().mirrors.is_empty());

        // Back: the KVM probes the device again (it attaches anew on Hub-0), then mirrors it.
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
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
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
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
        e.port_attached(Port::Other, &cam);
        assert_eq!(
            drain(&mut e)[0],
            Output::Route { port: Port::Other, route: Route::Pc(Pc::Pc1), power_cycle: false }
        );
        e.port_detached(Port::Other); // Hub-0 sees it leave: ignored
        assert_eq!(e.port_info(Port::Other).state, PortState::Passthrough);
        e.switch(Target::Toggle, SwitchSource::Key);
        assert!(drain(&mut e).contains(&Output::Route {
            port: Port::Other,
            route: Route::Pc(Pc::Pc2),
            power_cycle: false
        }));
        // Pinning it to PC1 moves it back once and then it stays.
        e.set_config(ConfigValue::PortMode(Port::Other, PortMode::PassthroughPc1), Origin::Software).unwrap();
        assert!(drain(&mut e).contains(&Output::Route {
            port: Port::Other,
            route: Route::Pc(Pc::Pc1),
            power_cycle: false
        }));
        e.switch(Target::Toggle, SwitchSource::Key);
        assert!(!drain(&mut e).iter().any(|o| matches!(o, Output::Route { .. })));
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
        e.port_attached(Port::Other, &webcam(true));
        assert_eq!(e.port_info(Port::Other).state, PortState::Passthrough);
        assert_eq!(e.port_info(Port::Other).speed, Speed::High);
        // PC2's hub runs at full speed, and so does everything on it.
        e.switch(Target::Toggle, SwitchSource::Key);
        assert_eq!(e.port_info(Port::Other).speed, Speed::Full);
        e.set_config(ConfigValue::PortMode(Port::Other, PortMode::PassthroughPc1), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Other).speed, Speed::High, "pinned to PC1");
        // A full-speed device stays at full speed.
        e.port_attached(Port::Other, &webcam(false));
        assert_eq!(e.port_info(Port::Other).speed, Speed::Full);
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
        e.port_attached(Port::Other, &drive);
        assert_eq!(e.port_info(Port::Other).state, PortState::Passthrough);
        // Unplugged at the PC's hub, which the KVM can't see; then re-detect.
        assert_eq!(ask(&mut e, Request::Redetect(Port::Other)), Response::Ok);
        assert!(drain(&mut e).contains(&Output::Reprobe(Port::Other)));
        e.tick(T0 + REPROBE_MS - 1);
        assert_eq!(e.port_info(Port::Other).state, PortState::Probing);
        e.tick(T0 + REPROBE_MS);
        assert_eq!(e.port_info(Port::Other), PortInfo::empty(Port::Other, PortMode::Auto));
        assert!(events_to(&drain(&mut e), Pc::Pc1).contains(&Event::PortChanged(Port::Other)));
        // A device that is there comes on Hub-0 in time and is probed as usual, however long that takes.
        assert_eq!(ask(&mut e, Request::Redetect(Port::Other)), Response::Ok);
        e.tick(T0 + REPROBE_MS + 300);
        e.port_probing(Port::Other);
        e.tick(T0 + 3 * REPROBE_MS);
        assert_eq!(e.port_info(Port::Other).state, PortState::Probing);
        e.port_attached(Port::Other, &drive);
        assert_eq!(e.port_info(Port::Other).state, PortState::Passthrough);
        // Back from passthrough with the device gone: empty as well.
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Passthrough), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        assert_eq!(e.port_info(Port::Mouse).state, PortState::Passthrough);
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Auto), Origin::Software).unwrap();
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
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        assert!(e.presentation().vial.is_some());
        // ...until an XInput pad needs it; unplugging the pad gives it back.
        let pad = Probed { config: Vec::from_slice(XINPUT_CONFIG).unwrap(), ..Default::default() };
        e.port_attached(Port::Other, &pad);
        assert_eq!(e.port_info(Port::Other).effective, PortMode::Mirrored);
        assert!(e.presentation().vial.is_none());
        e.port_detached(Port::Other);
        assert!(e.presentation().vial.is_some());
        // On: Vial keeps its endpoint and the pad goes to passthrough instead.
        e.set_config(ConfigValue::Vial(VialMode::On), Origin::Software).unwrap();
        e.port_attached(Port::Other, &pad);
        let other = e.port_info(Port::Other);
        assert_eq!((other.effective, other.reason), (PortMode::Passthrough, AutoReason::EndpointBudget));
        assert!(e.presentation().vial.is_some());
        e.set_config(ConfigValue::Vial(VialMode::Off), Origin::Software).unwrap();
        assert!(e.presentation().vial.is_none());
    }

    #[test]
    fn vial_remaps_a_normalized_keyboard_and_the_keymap_is_saved() {
        let mut e = engine();
        e.set_config(ConfigValue::Vial(VialMode::On), Origin::Software).unwrap();
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
        assert_eq!(items, [config::ITEM, kvm_vial::keymap::item::LAYER]);

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

        // With the keymap off, Caps Lock is Caps Lock again.
        e.set_config(ConfigValue::Keymap(false), Origin::Software).unwrap();
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
            match DeviceMsg::decode(&e.control(Some(Pc::Pc2), &raw).unwrap()).unwrap() {
                DeviceMsg::Response { id: 3, response, .. } => response,
                other => panic!("{other:?}"),
            }
        };
        let Response::Status(s) = ask(&mut e, Request::GetStatus) else { panic!() };
        assert_eq!((s.active, s.asking), (Pc::Pc1, Some(Pc::Pc2)));
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
        let hk = ConfigValue::HotkeyPc1(Hotkey::combo(0x05, 0x1E));
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
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
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
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Normalized), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Mouse).effective, PortMode::Mirrored);
        assert_eq!(e.presentation().boot_keyboard.ep_in, 0x81);
        let eps: std::vec::Vec<_> = e.presentation().mirrors.iter().map(|m| m.f.ep_in).collect();
        assert_eq!(eps, [0x85, 0x86]);
    }

    #[test]
    fn a_mirror_squeezed_out_comes_back_when_the_endpoints_free_up() {
        // A normalized device on Other keeps the KVM's own interfaces: two 2-endpoint mirrors don't both fit.
        let mut e = engine();
        e.set_config(ConfigValue::PortMode(Port::Other, PortMode::Normalized), Origin::Software).unwrap();
        e.port_attached(Port::Other, &keyboard_and_mouse());
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        let mouse = e.port_info(Port::Mouse);
        assert_eq!((mouse.effective, mouse.reason), (PortMode::Normalized, AutoReason::EndpointBudget));
        // The keyboard lets go of its mirror: the mouse gets it.
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Normalized), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Mouse).effective, PortMode::Mirrored);
        // Back to Mirrored: the mouse keeps its endpoints, the keyboard is the one left out.
        drain(&mut e);
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        assert_eq!(e.port_info(Port::Keyboard).reason, AutoReason::EndpointBudget);
        // Unplugging the normalized device on Other makes room for both.
        e.port_detached(Port::Other);
        assert_eq!(e.port_info(Port::Keyboard).effective, PortMode::Mirrored);
        assert_eq!(e.presentation().mirrors.len(), 4);
    }

    #[test]
    fn identity_stays_with_the_port_that_has_it() {
        let mut e = engine();
        e.port_attached(Port::Keyboard, &keyboard_and_mouse());
        e.port_attached(Port::Mouse, &keyboard_and_mouse());
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::Mirrored), Origin::Software).unwrap();
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::MirroredIdentity), Origin::Software).unwrap();
        assert_eq!(e.presentation().identity, Some(Port::Mouse));
        // The keyboard asks for identity too: one VID/PID per device, and the mouse had it first.
        e.set_config(ConfigValue::PortMode(Port::Keyboard, PortMode::MirroredIdentity), Origin::Software).unwrap();
        let kb = e.port_info(Port::Keyboard);
        assert_eq!((kb.effective, kb.reason), (PortMode::Mirrored, AutoReason::IdentityTaken));
        assert_eq!(e.port_info(Port::Mouse).effective, PortMode::MirroredIdentity);
        assert_eq!(e.presentation().identity, Some(Port::Mouse));
        // The mouse lets go of it: the keyboard gets it.
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
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
        e.set_config(ConfigValue::KeyLong(KeyAction::OtherPortMode), Origin::Software).unwrap();
        e.key(KeyEvent::Tap);
        assert_eq!((e.config().profile, e.active()), (1, Pc::Pc1));
        // The long press changes the Other port's mode in the active profile only.
        e.key(KeyEvent::Long);
        assert_eq!(e.config().profiles[1].port_modes[Port::Other.index()], PortMode::PassthroughPc1);
        assert_eq!(e.config().profiles[0].port_modes[Port::Other.index()], PortMode::Auto);
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
        match DeviceMsg::decode(&e.control(pc, &raw).unwrap()).unwrap() {
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
        BtHostState { host: n, flags, interval: 12, latency: 0, addr: [0, 1, 2, 3, 4, 5, n], rssi: -55 }
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

    fn bt_inputs(out: &[Output]) -> std::vec::Vec<(u8, u8, std::vec::Vec<u8>)> {
        out.iter()
            .filter_map(|o| match o {
                Output::Bt(BtOut::Input { host, id, data }) => Some((*host, *id, data.to_vec())),
                _ => None,
            })
            .collect()
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

        // Fitted but silent: red, and `Status` says so, though there is no Bluetooth (no `Hello`, no capability).
        let mut silent = engine();
        silent.set_bt_module(BtModule::GaveUp);
        let items = bt_items(&mut silent);
        assert_eq!(items.iter().find(|i| i.id == id::BT_MODULE).map(|i| i.grade), Some(Grade::Bad));
        let Response::Info(i) = ask(&mut silent, Request::GetInfo) else { panic!() };
        assert_eq!((i.caps & ctrl::caps::BLUETOOTH, i.bt_fw), (0, 0));
        let Response::Status(st) = ask(&mut silent, Request::GetStatus) else { panic!() };
        assert_eq!(st.bt_module, BtModule::GaveUp);
        // It still draws its current.
        let pwr = PowerInfo { source: PowerSource::PwrPort, adv_pwr: Advertised::A3_0, vbus_pwr_mv: 5100 };
        plain.set_power(pwr);
        silent.set_power(pwr);
        let budget = |e: &Engine| e.status(None).budget_ma;
        assert_eq!(budget(&plain) - budget(&silent), BT_CURRENT_MA);
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
        assert!(bt.contains(&&BtOut::Select(0)));
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
        assert!(out.contains(&Output::Bt(BtOut::Select(2))));
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
        assert!(out.contains(&Output::Bt(BtOut::Select(0))));
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
        assert_eq!(e.config().hotkey_toggle, Config::default().hotkey_toggle);
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
            ConfigValue::ProfileHop(0, true),
            ConfigValue::HopAccel(ctrl::Accel::Flat),
            ConfigValue::HopResistance(50),
            ConfigValue::Screen(0, ctrl::Screen::new(Pc::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(1, ctrl::Screen::new(Pc::Pc2, 1920, 1080, [1920, 0])),
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
        e.set_config(ConfigValue::ProfileHop(0, true), Origin::Software).unwrap();
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
        e.set_config(ConfigValue::Screen(3, ctrl::Screen::new(Pc::Pc1, 2560, 1440, [0, 0])), Origin::Software).unwrap();
        assert_eq!(e.hop_state(), HopState::Active);
        e.pc_protocol(Pc::Pc1, 1, 0);
        assert_eq!(e.hop_state(), HopState::BootProtocol);
        e.pc_protocol(Pc::Pc1, 1, 1);
        e.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Passthrough), Origin::Software).unwrap();
        assert_eq!(e.hop_state(), HopState::NotNormalized);
        // The feature report of the multipliers carries the report ID too.
        e.pc_set_report(Pc::Pc1, 1, 3, normalized::MOUSE_REL_ID, &[normalized::MOUSE_REL_ID, 0x05]);
        assert!(e.pcs[0].out.wheel_hires && e.pcs[0].out.pan_hires);
        assert!(matches!(ask(&mut e, Request::GetInfo), Response::Info(i) if i.caps & ctrl::caps::SCREEN_HOP != 0));
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
}
