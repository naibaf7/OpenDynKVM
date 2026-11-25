//! Background thread that owns the connection: runs UI commands, refreshes the snapshot, collects events, and
//! reconnects when the KVM goes away.
//!
//! In a browser (the web build) a page has one thread: [`Web`] does the same rounds, one per frame, with the
//! simulator; the threads here (and the cursor and monitor sync) are for the desktop.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use odkvmctl::time::{Duration, Instant};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use kvm_core::hotkey::format_spec;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::ctrl::{
    ConfigValue, ErrorCode, Event, FwState, HopState, Hotkey, LearnEnd, Port, RebootTarget, Target, diag,
};
use kvm_proto::link::bt::Chip;
use odkvmctl::bootsel::{self, BootDevice};
use odkvmctl::client::{DeviceSlot, Snapshot};
use odkvmctl::ddc::KvmState;
use odkvmctl::{Client, Error, firmware, fl, image, names};

pub enum UiCmd {
    /// From a computer that reaches the KVM over its wireless module: ask to control it (a tap on the KVM's key).
    AskControl,
    /// In a browser: ask it for a KVM, over USB (WebHID) or the wireless module (Web Bluetooth), and use it.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    WebConnect(odkvmctl::transport::Link),
    /// In a browser: back to the simulator.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    WebSimulator,
    Switch(Target),
    Redetect(Port),
    Config(ConfigValue),
    Save,
    FactoryReset,
    Identify,
    /// Install the RP2354B firmware of a release (a file, or its folder); its module's firmware follows once the KVM
    /// is back ([`Shared::module_pending`]).
    Update(PathBuf),
    /// Stream the Bluetooth module's firmware of a release to the module.
    UpdateModule(PathBuf),
    /// Give the AT32F405s up to 5 more programming attempts (the KVM restarts).
    RetryAt32,
    /// Start (true) or stop learning a hotkey from the KVM's keyboard.
    Learn(bool),
    /// Fetch the diagnostics (`Shared::diag`).
    Diagnose,
    /// Open (true) or close Bluetooth pairing.
    BtPair(bool),
    /// Forget the Bluetooth host in a slot.
    BtForget(u8),
    /// Erase every pairing the wireless module holds, and restart it.
    BtEraseAll,
    /// Make the ESP32 board on this port the wireless module (`Request::AdoptModule`).
    AdoptModule(Port),
    /// The passkey a pairing host shows.
    BtPasskey(u32),
    /// Look for Bluetooth devices to pair (true), or stop.
    BtScan(bool),
    /// Pair the device found at this address.
    BtPairDevice([u8; 7]),
    /// Forget the Bluetooth device in a slot.
    BtForgetDevice(u8),
    /// The mapping editor: read keymap 0–3's layer 0–7 (`Shared::keymap`), set one control's keycode, one axis rule.
    LoadKeymap(u8, u8),
    SetKeycode(u8, u8, u16, u16),
    SetAxisRule(u8, u8, u8, kvm_map::axes::Rule),
    /// Watch the controls (`Shared::held`, `Shared::touched`, `Shared::axes`): 0 stop, 1 watch, 2 and keep them from
    /// the hosts.
    Watch(u8),
    /// A device's controls and its gamepad axes' calibration (`Shared::pad`), and setting one.
    LoadDevice(kvm_proto::ctrl::Device),
    SetCalibration(kvm_proto::ctrl::Device, u8, kvm_proto::ctrl::AxisCal),
    /// The keymaps' shared parts (`Shared::parts`): every tap dance, combo, key override and alternate repeat key, the
    /// macros, the QMK settings; and setting one.
    LoadParts,
    SetEntry(kvm_map::keymap::Dynamic, u8, Vec<u8>),
    /// The macro buffer: as it was, and as it is to be.
    SetMacros(Vec<u8>, Vec<u8>),
    SetSetting(u16, u32),
    /// The monitor (`Shared::monitor`): a device's descriptors and field overrides, its reports streamed (true) or not;
    /// asked again to go on.
    Monitor(kvm_proto::ctrl::Device, bool),
    SetOverride(kvm_proto::ctrl::Device, u8, Option<kvm_proto::ctrl::FieldOverride>),
    /// What each host is offered and took (`Shared::outputs`).
    LoadOutputs,
}

/// The keymaps' shared parts as the editor shows them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MapParts {
    /// Every entry of each kind (`kvm_map::keymap::Dynamic` order), in Vial's layout.
    pub entries: [Vec<Vec<u8>>; 4],
    pub macros: Vec<u8>,
    /// Each setting of `keycodes::SETTINGS`, by qsid.
    pub settings: Vec<(u16, u32)>,
}

/// One report of the monitored device: the last one, and how often each bit changed.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReportView {
    pub bytes: Vec<u8>,
    pub changes: Vec<u32>,
    pub count: u32,
}

/// The monitored device: its interfaces' descriptors, its field overrides, its reports by interface and report ID.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorView {
    pub device: kvm_proto::ctrl::Device,
    pub maps: Vec<(u8, Vec<u8>)>,
    pub overrides: [Option<kvm_proto::ctrl::FieldOverride>; kvm_proto::ctrl::MAX_OVERRIDES],
    pub reports: std::collections::BTreeMap<(u8, u8), ReportView>,
    /// A report coming in pieces, per interface.
    pieces: std::collections::BTreeMap<u8, Vec<u8>>,
    /// Streaming now.
    pub on: bool,
}

impl MonitorView {
    /// A piece of a report; a complete one becomes the interface's report, its changed bits counted.
    fn piece(&mut self, iface: u8, offset: u8, total: u8, data: &[u8]) {
        let buf = self.pieces.entry(iface).or_default();
        if offset == 0 {
            buf.clear();
        }
        buf.extend_from_slice(data);
        if buf.len() < total as usize {
            return;
        }
        let report = std::mem::take(buf);
        let ids = self.maps.iter().find(|(i, _)| *i == iface).and_then(|(_, m)| kvm_hid::Layout::parse(m).ok());
        let id = if ids.is_some_and(|l| l.uses_report_ids) { report.first().copied().unwrap_or(0) } else { 0 };
        let v = self.reports.entry((iface, id)).or_default();
        if v.changes.len() < report.len() * 8 {
            v.changes.resize(report.len() * 8, 0);
        }
        for (i, b) in report.iter().enumerate() {
            let was = v.bytes.get(i).copied().unwrap_or(*b);
            for bit in 0..8 {
                if (was ^ b) >> bit & 1 != 0 {
                    v.changes[i * 8 + bit] += 1;
                }
            }
        }
        v.bytes = report;
        v.count += 1;
    }
}

/// A keymap's layer as the editor shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct KeymapLayer {
    pub keymap: u8,
    pub layer: u8,
    /// Every position's keycode, and every axis input's rule.
    pub codes: Vec<u16>,
    pub rules: Vec<kvm_map::axes::Rule>,
}

/// A device as the editor's gamepad view shows it: what it has, and its axes' calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceView {
    pub device: kvm_proto::ctrl::Device,
    pub controls: kvm_proto::ctrl::Controls,
    pub cal: [kvm_proto::ctrl::AxisCal; kvm_hid::gamepad::AXES],
}

#[derive(Default)]
pub struct Shared {
    pub snap: Option<Snapshot>,
    /// Description of the connected KVM.
    pub device: Option<String>,
    pub error: Option<String>,
    pub events: VecDeque<String>,
    pub update: Option<(FwState, usize, usize)>,
    pub update_result: Option<Result<(), String>>,
    /// The Bluetooth module's update: its progress, and how it ended.
    pub module_update: Option<(FwState, usize, usize)>,
    pub module_result: Option<Result<(), String>>,
    /// The release whose module firmware goes to the module once the KVM is back from an update (and its module says
    /// it needs it).
    pub module_pending: Option<PathBuf>,
    /// The mapping editor: the layer shown, and the device its gamepad view shows.
    pub keymap: Option<KeymapLayer>,
    pub pad: Option<Result<DeviceView, String>>,
    pub parts: Option<MapParts>,
    pub monitor: Option<MonitorView>,
    /// Each host, what it is offered and what it took (`ctrl::outputs` bits).
    pub outputs: Vec<(kvm_proto::ctrl::Host, u8, u8)>,
    /// While watching the controls: the ones held down, the last one used (and when), every axis input's value; and
    /// how they are watched now (`UiCmd::Watch`).
    pub held: std::collections::BTreeSet<u16>,
    pub watching: u8,
    pub touched: Option<(u16, Instant)>,
    pub axes: [i16; kvm_map::axes::INPUTS],
    /// An update under way, until it is completely done ([`finish_update`]).
    pub expected: Option<Expected>,
    /// The last update is completely done: the Firmware page starts afresh, with one line saying so.
    pub finished: Option<Finished>,
    /// How learning a hotkey ended, or why it couldn't start.
    pub learn: Option<Result<(LearnEnd, Hotkey), String>>,
    /// With the simulator: its keyboard, which the window's keys play while a hotkey is learned.
    pub sim_keyboard: Option<Sender<[u8; 8]>>,
    /// The last diagnostics and when they came (Err: why there are none).
    pub diag: Option<(Instant, Result<Vec<diag::Item>, String>)>,
    /// RP2350 boot ROMs on USB (a KVM in its bootloader, on the PWR/SERVICE port), looked for while no KVM answers on
    /// a PC port ([`spawn_boot_scan`]).
    pub boot: Vec<BootDevice>,
    /// Installing onto one of them: its location, the 512-byte blocks done and their total; and how the last ended.
    pub boot_install: Option<(String, usize, usize)>,
    pub boot_result: Option<(String, Result<(), String>)>,
    /// The firmware file dialog is open; the file it returned, until the window takes it.
    pub picking: bool,
    pub picked: Option<PathBuf>,
    /// Talk to the built-in simulator instead of a real KVM (`--sim`, or chosen in the window while none is there):
    /// the worker changes over as soon as it sees this change.
    pub simulate: bool,
    /// While simulating: a real KVM is plugged in (its control interface is there; it isn't opened).
    pub kvm_present: bool,
    /// The KVM's firmware speaks another protocol than this program: its identity, for the update (nothing else is
    /// read from it then).
    pub mismatch: Option<kvm_proto::ctrl::Info>,
    /// How the KVM is reached (USB, the wireless module, the simulator), while connected.
    pub link: Option<odkvmctl::transport::Link>,
    /// Over the wireless module, this computer may not control the KVM yet: Some(true) while the KVM waits for its
    /// key after it asked.
    pub not_allowed: Option<bool>,
}

/// What a completed update looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    /// The firmware the KVM answers with.
    pub version: u32,
    /// It restarts first (an RP2354B update over a PC port): the connection must go and come back.
    pub restart: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finished {
    pub version: u32,
    /// Its Bluetooth module is up with the same version.
    pub module: bool,
    pub at: Instant,
}

pub type Wake = Arc<dyn Fn() + Send + Sync>;

const REFRESH: Duration = Duration::from_millis(1000);
/// How often the boot ROMs on USB are looked for while no KVM answers.
const BOOT_SCAN: Duration = Duration::from_secs(1);
const RECONNECT: Duration = Duration::from_secs(2);
/// While simulating, how often a real KVM is looked for.
const LOOK_FOR_KVM: Duration = Duration::from_secs(2);
/// Screen hop: this computer's cursor goes to the KVM once it has rested this long (software.md §4.14).
const CURSOR_REST: Duration = Duration::from_millis(250);

/// This computer's cursor for screen hop: reported to the KVM once each time it comes to rest, so the KVM takes over
/// where something else (a touchpad, a program, the system's own acceleration) put it. Only where the system tells
/// programs the cursor position (Windows).
#[derive(Default)]
struct CursorSync {
    seen: Option<[i32; 2]>,
    since: Option<Instant>,
    sent: bool,
}

#[maybe_async::maybe_async]
impl CursorSync {
    async fn poll(&mut self, c: &mut Client, hop: bool) {
        if !hop {
            return;
        }
        let Some(p) = odkvmctl::screens::cursor() else { return };
        if self.seen != Some(p) {
            *self = CursorSync { seen: Some(p), since: Some(Instant::now()), sent: false };
            return;
        }
        if !self.sent && self.since.is_some_and(|t| t.elapsed() >= CURSOR_REST) {
            self.sent = true;
            let _ = c.pointer_at(p.map(|v| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16)).await;
        }
    }
}

/// An event as the Overview's list shows it; None for the Bluetooth events, which [`bt_changes`] tells with names.
fn describe(e: &Event) -> Option<String> {
    Some(match e {
        Event::Switched(pc, src) => {
            fl!("ev-switched", target = pc.name().to_uppercase(), source = names::switch_source(*src))
        }
        Event::PortChanged(p) => fl!("ev-port-changed", port = names::port_title(*p)),
        Event::Fault(kind, Some(p)) => {
            fl!("ev-fault-port", port = names::port_title(*p), fault = names::fault_kind(*kind))
        }
        Event::Fault(kind, None) => fl!("ev-fault", fault = names::fault_kind(*kind)),
        Event::ConfigChanged(k) => fl!("ev-setting-changed", name = names::config_name(*k)),
        Event::HotkeyLearned(LearnEnd::Learned, h) => fl!("ev-hotkey-learned", hotkey = format_spec(h).to_string()),
        Event::HotkeyLearned(end, _) => fl!("ev-hotkey-not-learned", why = names::learn_end(*end)),
        // The mapping editor's learn stream is not news for the list.
        Event::BtActive(_)
        | Event::BtChanged(_)
        | Event::BtDeviceChanged(_)
        | Event::PlaceCursor(_)
        | Event::Control(..)
        | Event::Axis(..)
        | Event::Report { .. } => return None,
    })
}

/// What changed for the Bluetooth hosts between two snapshots, in words and with the hosts' names: the KVM's events
/// only say which slot changed.
fn bt_changes(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    if !new.has_bluetooth() {
        return out;
    }
    let (o, n) = (&old.status, &new.status);
    if o.bt_pairing == 0 && n.bt_pairing != 0 {
        out.push(fl!("ev-pairing-open"));
    }
    if o.bt_passkey == 0 && n.bt_passkey != 0 {
        out.push(fl!("ev-passkey-wanted"));
    }
    let mut paired = false;
    for h in &new.bt {
        let before = old.bt.iter().find(|b| b.slot == h.slot);
        let was = |f: fn(&odkvmctl::client::BtHostView) -> bool| before.is_some_and(f);
        if h.bonded() && !was(|b| b.bonded()) {
            out.push(fl!("ev-paired", host = h.name.as_str(), slot = h.slot));
            paired = true;
        } else if !h.bonded() && was(|b| b.bonded()) {
            let name = before.map_or(h.name.as_str(), |b| b.name.as_str());
            out.push(fl!("ev-forgot", host = name));
        } else if h.bonded() && h.ready() && !was(|b| b.ready()) {
            out.push(fl!("ev-host-connected", host = h.name.as_str()));
        } else if h.bonded() && !h.connected() && was(|b| b.connected()) {
            out.push(fl!("ev-host-disconnected", host = h.name.as_str()));
        }
    }
    if o.bt_pairing != 0 && n.bt_pairing == 0 && !paired {
        out.push(fl!("ev-pairing-closed"));
    }
    if n.bt_active != 0 && n.bt_active != o.bt_active {
        let name = new.bt.iter().find(|h| h.slot == n.bt_active).map_or_else(|| fl!("a-host"), |h| h.name.clone());
        out.push(fl!("ev-switched-host", host = name));
    }
    out.extend(device_changes(old, new));
    out
}

/// The same for the Bluetooth devices: paired, forgotten, connected, gone.
fn device_changes(old: &Snapshot, new: &Snapshot) -> Vec<String> {
    let mut out = Vec::new();
    let name = |d: &kvm_proto::ctrl::BtDeviceInfo| {
        if d.name.is_empty() { names::device_kind(d.appearance) } else { d.name.to_string() }
    };
    for d in &new.devices {
        let before = old.devices.iter().find(|b| b.slot == d.slot);
        let was = |f: fn(&kvm_proto::ctrl::BtDeviceInfo) -> bool| before.is_some_and(f);
        if d.bonded() && !was(|b| b.bonded()) {
            out.push(fl!("ev-device-paired", device = name(d), slot = d.slot));
        } else if !d.bonded() && was(|b| b.bonded()) {
            out.push(fl!("ev-device-forgot", device = before.map_or_else(|| name(d), name)));
        } else if d.bonded() && d.ready() && !was(|b| b.ready()) {
            out.push(fl!("ev-host-connected", host = name(d)));
        } else if d.bonded() && !d.connected() && was(|b| b.connected()) {
            out.push(fl!("ev-host-disconnected", host = name(d)));
        }
    }
    if old.status.bt_device_pairing != new.status.bt_device_pairing
        && new.status.bt_device_pairing == kvm_proto::ctrl::DevicePairing::Failed
    {
        out.push(names::device_pairing(kvm_proto::ctrl::DevicePairing::Failed));
    }
    out
}

/// Sets the monitors' inputs when the KVM's active PC or profile changes (`odkvmctl::ddc`), on its own thread: DDC/CI
/// takes 50 ms or more per monitor, and the worker must keep polling the KVM. The first state it hears of is only noted:
/// the monitors are assumed to show it already.
fn follower(shared: Arc<Mutex<Shared>>, wake: Wake) -> Sender<KvmState> {
    let (tx, rx) = channel::<KvmState>();
    std::thread::spawn(move || {
        let mut last = None;
        while let Ok(mut to) = rx.recv() {
            // Several changes in a row: only the last one counts.
            while let Ok(s) = rx.try_recv() {
                to = s;
            }
            if let Some(from) = last.as_ref().filter(|from| **from != to) {
                let local = odkvmctl::local::load();
                for e in odkvmctl::ddc::follow(from, &to, &local.displays) {
                    shared.lock().unwrap().events.push_front(fl!("ev-monitor-not-switched", error = e));
                    wake();
                }
            }
            last = Some(to);
        }
    });
    tx
}

/// The monitors' view of a snapshot: active PC, the Bluetooth host with the input (by address), profile.
fn kvm_state(s: &Snapshot) -> KvmState {
    let bt = s.bt.iter().find(|h| h.slot == s.status.bt_active && s.status.bt_active != 0).map(|h| h.address());
    KvmState { pc: s.status.active.index(), bt, profile: s.profiles().map_or(0, |p| p.0 as usize) }
}

/// Passes the KVM's active PC, Bluetooth host and profile on to the follower when they change.
fn tell(follow: &Option<Sender<KvmState>>, last: &mut Option<KvmState>, state: KvmState) {
    if last.as_ref() != Some(&state) {
        *last = Some(state.clone());
        if let Some(f) = follow {
            let _ = f.send(state);
        }
    }
}


#[maybe_async::maybe_async]
async fn run(c: &mut Client, cmd: UiCmd, shared: &Mutex<Shared>, wake: &Wake) -> Result<(), Error> {
    match cmd {
        UiCmd::AskControl => {
            // Allowed already: the next snapshot shows it. Else the KVM waits for its key, and the page says so.
            if !c.request_control().await? {
                shared.lock().unwrap().not_allowed = Some(true);
                wake();
            }
            Ok(())
        }
        // The browser's worker takes these itself (`spawn_web`).
        UiCmd::WebConnect(_) | UiCmd::WebSimulator => Ok(()),
        UiCmd::Switch(t) => c.switch(t).await,
        UiCmd::Redetect(p) => c.redetect(p).await,
        UiCmd::Config(v) => match c.set_config(v).await {
            // Allowing software control again: the KVM now waits for its key, and the Settings page says so.
            Err(Error::Device(ErrorCode::NeedsKeyPress)) => Ok(()),
            r => r.map(|_| ()),
        },
        UiCmd::Save => c.save().await,
        UiCmd::BtPair(on) => c.bt_pair(on).await,
        UiCmd::BtForget(n) => c.bt_forget(n).await,
        UiCmd::BtEraseAll => c.bt_erase_all().await,
        UiCmd::AdoptModule(p) => c.adopt_module(p).await,
        UiCmd::BtPasskey(v) => c.bt_passkey(v).await,
        UiCmd::BtScan(on) => c.bt_scan(on).await,
        UiCmd::BtPairDevice(addr) => c.bt_pair_device(addr).await,
        UiCmd::BtForgetDevice(n) => c.bt_forget_device(n).await,
        UiCmd::FactoryReset => c.factory_reset().await,
        UiCmd::Identify => c.identify(5).await,
        UiCmd::RetryAt32 => c.reboot(RebootTarget::At32Retry).await,
        UiCmd::Learn(false) => c.stop_learning().await,
        UiCmd::Diagnose => {
            let r = match c.diag().await {
                Ok(items) => Ok(items),
                Err(e) if c.lost(&e) => return Err(e),
                Err(e) => Err(e.to_string()),
            };
            shared.lock().unwrap().diag = Some((Instant::now(), r));
            wake();
            Ok(())
        }
        UiCmd::Learn(true) => {
            let why = match c.start_learning().await {
                Ok(()) => return Ok(()),
                Err(Error::Device(code)) => fl!("kvm-refused", why = names::error(code)),
                Err(e) => return Err(e),
            };
            shared.lock().unwrap().learn = Some(Err(why));
            wake();
            Ok(())
        }
        UiCmd::Update(path) => {
            // The same checks as `odkvmctl update`: RP2354B firmware (a file, or a release's folder), for this board and
            // its flash layout, with the module's firmware of the same release if it has one.
            let info = c.info().await?;
            shared.lock().unwrap().finished = None;
            let chosen = firmware::for_pc(&path).and_then(|f| {
                image::check_hardware(f.version, f.hw_major, info.hw)?;
                image::check_layout(info.rp_fw, f.version)?;
                // Its version only (every chip's image is the release's): the module's turn comes after the KVM's.
                let module = firmware::module_for(&path, Chip::Unknown)?;
                match &module {
                    Some(m) if m.version != f.version => Err(module_mismatch(m, f.version)),
                    _ => Ok((f, module.is_some())),
                }
            });
            let r = match chosen {
                Ok((f, module)) => {
                    let r = c.update_firmware(&f.data, |state, done, total| {
                        shared.lock().unwrap().update = Some((state, done, total));
                        wake();
                    }).await;
                    let mut s = shared.lock().unwrap();
                    if r.is_ok() && module {
                        s.module_pending = Some(path.clone());
                        s.module_update = None;
                        s.module_result = None;
                    }
                    if r.is_ok() {
                        // The simulator acts out no restart.
                        let restart = info.caps & kvm_proto::ctrl::caps::SIMULATED == 0;
                        s.expected = Some(Expected { version: f.version, restart });
                    }
                    r
                }
                Err(e) => Err(Error::Transport(e)),
            };
            shared.lock().unwrap().update_result = Some(r.clone().map_err(|e| e.to_string()));
            r
        }
        UiCmd::LoadKeymap(k, l) => {
            let codes = c.keycodes(k, l).await?;
            let rules = c.axis_rules(k, l).await?;
            shared.lock().unwrap().keymap = Some(KeymapLayer { keymap: k, layer: l, codes, rules });
            wake();
            Ok(())
        }
        UiCmd::SetKeycode(k, l, p, kc) => {
            c.set_keycodes(k, l, p, &[kc]).await?;
            let mut s = shared.lock().unwrap();
            if let Some(m) = s.keymap.as_mut().filter(|m| (m.keymap, m.layer) == (k, l))
                && let Some(slot) = m.codes.get_mut(p as usize)
            {
                *slot = kc;
            }
            Ok(())
        }
        UiCmd::SetAxisRule(k, l, i, r) => {
            c.set_axis_rule(k, l, i, r).await?;
            let mut s = shared.lock().unwrap();
            if let Some(m) = s.keymap.as_mut().filter(|m| (m.keymap, m.layer) == (k, l))
                && let Some(slot) = m.rules.get_mut(i as usize)
            {
                *slot = r;
            }
            Ok(())
        }
        UiCmd::Watch(mode) => {
            c.watch_controls(mode).await?;
            let mut s = shared.lock().unwrap();
            // Starting or stopping starts afresh; a renewal, or learning starting or ending, keeps what is held, as the
            // KVM does (it tells a control once). Clearing it on each renewal took a held Escape for let go, so holding
            // it never stopped learning.
            if (s.watching == 0) != (mode == 0) {
                s.held.clear();
                s.touched = None;
            }
            s.watching = mode;
            Ok(())
        }
        UiCmd::LoadDevice(d) => {
            // A loop rather than closures: in a browser each request is awaited.
            let view = match c.controls(d).await {
                Ok(controls) => {
                    let mut cal = [kvm_proto::ctrl::AxisCal::NONE; kvm_hid::gamepad::AXES];
                    let mut read = Ok(());
                    for (a, slot) in cal.iter_mut().enumerate().filter(|(a, _)| controls.pad_axes & (1 << a) != 0) {
                        match c.calibration(d, a as u8).await {
                            Ok(v) => *slot = v,
                            Err(e) => {
                                read = Err(e);
                                break;
                            }
                        }
                    }
                    read.map(|()| DeviceView { device: d, controls, cal })
                }
                Err(e) => Err(e),
            };
            let r = match view {
                Ok(v) => Ok(v),
                Err(e) if c.lost(&e) => return Err(e),
                Err(e) => Err(e.to_string()),
            };
            shared.lock().unwrap().pad = Some(r);
            wake();
            Ok(())
        }
        UiCmd::LoadParts => {
            use kvm_map::keymap::Dynamic;
            let mut parts = MapParts::default();
            for (k, kind) in Dynamic::ALL.iter().enumerate() {
                for i in 0..kind.count() as u8 {
                    parts.entries[k].push(c.map_entry(*kind, i).await?);
                }
            }
            parts.macros = c.macros().await?;
            for (_, q, _) in odkvmctl::keycodes::SETTINGS {
                parts.settings.push((q, c.map_setting(q).await?));
            }
            shared.lock().unwrap().parts = Some(parts);
            Ok(())
        }
        UiCmd::SetEntry(kind, i, data) => {
            c.set_map_entry(kind, i, &data).await?;
            let back = c.map_entry(kind, i).await?;
            if let Some(p) = shared.lock().unwrap().parts.as_mut()
                && let Some(e) = p.entries[kind as usize].get_mut(i as usize)
            {
                *e = back;
            }
            Ok(())
        }
        UiCmd::SetMacros(before, after) => {
            c.set_macros(&before, &after).await?;
            let back = c.macros().await?;
            if let Some(p) = shared.lock().unwrap().parts.as_mut() {
                p.macros = back;
            }
            Ok(())
        }
        UiCmd::SetSetting(q, v) => {
            c.set_map_setting(q, v).await?;
            let back = c.map_setting(q).await?;
            if let Some(p) = shared.lock().unwrap().parts.as_mut()
                && let Some(e) = p.settings.iter_mut().find(|(k, _)| *k == q)
            {
                e.1 = back;
            }
            Ok(())
        }
        UiCmd::Monitor(d, on) => {
            let fresh = shared.lock().unwrap().monitor.as_ref().is_none_or(|m| m.device != d);
            if fresh {
                let maps = c.device_maps(d).await?;
                let mut overrides = [None; kvm_proto::ctrl::MAX_OVERRIDES];
                for (n, o) in overrides.iter_mut().enumerate() {
                    *o = c.field_override(d, n as u8).await.unwrap_or(None);
                }
                let view = MonitorView {
                    device: d,
                    maps,
                    overrides,
                    reports: Default::default(),
                    pieces: Default::default(),
                    on,
                };
                shared.lock().unwrap().monitor = Some(view);
            }
            c.monitor(d, on).await?;
            if let Some(m) = shared.lock().unwrap().monitor.as_mut() {
                m.on = on;
            }
            Ok(())
        }
        UiCmd::SetOverride(d, n, o) => {
            c.set_field_override(d, n, o).await?;
            let back = c.field_override(d, n).await?;
            if let Some(m) = shared.lock().unwrap().monitor.as_mut().filter(|m| m.device == d)
                && let Some(slot) = m.overrides.get_mut(n as usize)
            {
                *slot = back;
            }
            Ok(())
        }
        UiCmd::LoadOutputs => {
            let mut out = Vec::new();
            for h in kvm_proto::ctrl::Host::ALL.iter().copied() {
                let (offered, taken) = c.outputs(h).await?;
                out.push((h, offered, taken));
            }
            shared.lock().unwrap().outputs = out;
            Ok(())
        }
        UiCmd::SetCalibration(d, a, cal) => {
            c.set_calibration(d, a, cal).await?;
            if let Some(Ok(v)) = shared.lock().unwrap().pad.as_mut().filter(|v| v.as_ref().is_ok_and(|v| v.device == d))
                && let Some(slot) = v.cal.get_mut(a as usize)
            {
                *slot = cal;
            }
            Ok(())
        }
        UiCmd::UpdateModule(path) => {
            // Only the KVM's own release's: its firmware takes no other.
            let rp_fw = c.info().await?.rp_fw;
            let chip = c.status().await?.bt_chip;
            let chosen = firmware::module_for(&path, chip).and_then(|m| match m {
                Some(m) if m.version == rp_fw => Ok(m),
                Some(m) => Err(module_mismatch(&m, rp_fw)),
                None => Err(fl!("fw-no-module", folder = path.display().to_string())),
            });
            {
                let mut s = shared.lock().unwrap();
                s.module_update = None;
                s.module_result = None;
                s.finished = None;
            }
            let r = match chosen {
                Ok(m) => c.update_module(&m.data, |state, done, total| {
                    shared.lock().unwrap().module_update = Some((state, done, total));
                    wake();
                }).await,
                Err(e) => Err(Error::Transport(e)),
            };
            if r.is_ok() {
                let mut s = shared.lock().unwrap();
                if s.expected.is_none() {
                    s.expected = Some(Expected { version: rp_fw, restart: false });
                }
            }
            shared.lock().unwrap().module_result = Some(r.clone().map_err(|e| e.to_string()));
            r
        }
    }
}

fn module_mismatch(m: &firmware::Module, version: u32) -> String {
    fl!(
        "fw-module-mismatch",
        file = m.file.display().to_string(),
        module = names::version(m.version),
        version = names::version(version)
    )
}

/// After an update, the module's turn: its firmware, once the module says it needs it; nothing once it is up (or there
/// is none). Taken from [`Shared::module_pending`].
fn module_due(shared: &Mutex<Shared>) -> Option<UiCmd> {
    let mut s = shared.lock().unwrap();
    let m = s.snap.as_ref()?.status.bt_module;
    match m {
        BtModule::NeedsUpdate => s.module_pending.take().map(UiCmd::UpdateModule),
        BtModule::Up | BtModule::Absent => {
            s.module_pending = None;
            None
        }
        BtModule::Starting | BtModule::Programming => None,
    }
}

/// Whether the update under way is completely done: the KVM answers with the version installed (after its restart),
/// no module update waits or runs, and the module isn't starting or being programmed. Then its progress and results
/// go, the diagnostics are read anew, and [`Shared::finished`] says so; a module update that failed stays shown.
fn finish_update(shared: &Mutex<Shared>) -> bool {
    let mut s = shared.lock().unwrap();
    let (Some(e), Some(snap)) = (s.expected, s.snap.as_ref()) else { return false };
    let (rp_fw, bt_fw, m) = (snap.info.rp_fw, snap.info.bt_fw, snap.status.bt_module);
    let module_busy = matches!(s.module_update, Some((FwState::Receiving | FwState::Writing, ..)));
    if e.restart || rp_fw != e.version || s.module_pending.is_some() || module_busy {
        return false;
    }
    if matches!(m, BtModule::Starting | BtModule::Programming) {
        return false;
    }
    s.expected = None;
    if matches!(s.module_result, Some(Err(_))) {
        return false;
    }
    s.update = None;
    s.update_result = None;
    s.module_update = None;
    s.module_result = None;
    s.boot_result = None;
    s.diag = None;
    s.finished = Some(Finished { version: rp_fw, module: m == BtModule::Up && bt_fw == rp_fw, at: Instant::now() });
    true
}

/// Looks for RP2350 boot ROMs every [`BOOT_SCAN`], into [`Shared::boot`]: also while a KVM answers on a PC port, as a
/// second board may wait on the PWR/SERVICE port. The simulator shows the board `OPENDYN_SIM_BOOT` names (`kvm`,
/// `new`, `other`, `unknown`) instead, to try the window; without it, none.
pub fn spawn_boot_scan(sim: bool, shared: Arc<Mutex<Shared>>, wake: Wake) {
    let simulated = std::env::var("OPENDYN_SIM_BOOT").ok().filter(|_| sim).and_then(|b| bootsel::simulated(&b));
    if sim && simulated.is_none() {
        return;
    }
    std::thread::spawn(move || {
        let mut scanner = bootsel::Scanner::default();
        loop {
            let found = match &simulated {
                Some(d) => vec![d.clone()],
                None => scanner.scan(),
            };
            let changed = {
                let mut s = shared.lock().unwrap();
                let changed = s.boot != found;
                s.boot = found;
                changed
            };
            if changed {
                wake();
            }
            std::thread::sleep(BOOT_SCAN);
        }
    });
}

/// What the worker keeps between rounds: the KVM's state as last passed to the follower, and the last snapshot read
/// (for what changed with the Bluetooth hosts; the window's copy may be edited).
#[derive(Default)]
struct Seen {
    kvm: Option<KvmState>,
    last: Option<Snapshot>,
    /// When the settings were last read; they are read again after a `ConfigChanged` event or one of the window's
    /// commands (`stale`), and at least every [`CONFIG_REFRESH`].
    config_at: Option<Instant>,
    stale: bool,
}

/// The settings are read in full at least this often, should an event about them be missed.
const CONFIG_REFRESH: Duration = Duration::from_secs(10);

/// Runs one of the window's commands. Ok(true): it changes what the snapshot shows; the diagnostics and the mapping
/// editor's own reads and writes don't (the editor renews its watch every few seconds, which a full snapshot after each
/// would make slow to show what is pressed). Err: the KVM is gone.
#[maybe_async::maybe_async]
async fn command(c: &mut Client, cmd: UiCmd, shared: &Mutex<Shared>, wake: &Wake) -> Result<bool, ()> {
    let refresh = !matches!(
        cmd,
        UiCmd::Diagnose
            | UiCmd::LoadKeymap(..)
            | UiCmd::SetKeycode(..)
            | UiCmd::SetAxisRule(..)
            | UiCmd::Watch(_)
            | UiCmd::LoadDevice(_)
            | UiCmd::SetCalibration(..)
            | UiCmd::LoadParts
            | UiCmd::SetEntry(..)
            | UiCmd::SetMacros(..)
            | UiCmd::SetSetting(..)
            | UiCmd::Monitor(..)
            | UiCmd::SetOverride(..)
            | UiCmd::LoadOutputs
    );
    if let Err(e) = run(c, cmd, shared, wake).await {
        let lost = c.lost(&e);
        shared.lock().unwrap().error = Some(e.to_string());
        if lost {
            return Err(());
        }
    }
    Ok(refresh)
}

/// Reads the KVM's state into `shared`. Err: the KVM is gone.
#[maybe_async::maybe_async]
async fn refresh(
    c: &mut Client,
    shared: &Mutex<Shared>,
    follow: &Option<Sender<KvmState>>,
    seen: &mut Seen,
) -> Result<(), ()> {
    let keep = !seen.stale && seen.config_at.is_some_and(|t| t.elapsed() < CONFIG_REFRESH);
    let config = keep.then(|| seen.last.as_ref().map(|l| l.config.clone())).flatten();
    let full = config.is_none();
    match c.snapshot_keeping(config).await {
        Ok(snap) => {
            if full {
                seen.config_at = Some(Instant::now());
                seen.stale = false;
            }
            // A change of profile shows here: the events below refresh at once.
            tell(follow, &mut seen.kvm, kvm_state(&snap));
            let changes = seen.last.as_ref().map(|old| bt_changes(old, &snap)).unwrap_or_default();
            seen.last = Some(snap.clone());
            let mut s = shared.lock().unwrap();
            for c in changes {
                s.events.push_front(c);
            }
            s.events.truncate(50);
            s.snap = Some(snap);
            s.mismatch = None;
            s.not_allowed = None;
            if s.error.as_deref().is_some_and(|e| e == Error::Timeout.to_string()) {
                s.error = None;
            }
            Ok(())
        }
        Err(e) if c.lost(&e) => Err(()),
        Err(Error::Mismatch(info)) => {
            let mut s = shared.lock().unwrap();
            s.error = Some(Error::Mismatch(info).to_string());
            s.snap = None;
            s.mismatch = Some(info);
            Ok(())
        }
        // Over the wireless module, before the KVM's key allowed this computer: the pages say how to ask.
        Err(Error::Device(ErrorCode::NotAllowed)) => {
            let mut s = shared.lock().unwrap();
            s.snap = None;
            s.error = None;
            s.not_allowed = Some(s.not_allowed.unwrap_or(false));
            Ok(())
        }
        Err(e) => {
            shared.lock().unwrap().error = Some(e.to_string());
            Ok(())
        }
    }
}

/// Takes the KVM's events into `shared`, waiting up to `wait` for the first. Ok(None): there were none; Ok(Some(news)):
/// there were some, and `news` if one changes what the snapshot shows (it is due then); Err: the KVM is gone.
#[maybe_async::maybe_async]
async fn take_events(
    c: &mut Client,
    wait: Duration,
    shared: &Mutex<Shared>,
    follow: &Option<Sender<KvmState>>,
    seen: &mut Seen,
) -> Result<Option<bool>, ()> {
    let evs = c.events(wait).await.map_err(|_| ())?;
    if evs.is_empty() {
        return Ok(None);
    }
    // The controls watched, the monitor's reports and the cursor's places change nothing the snapshot shows: a full
    // snapshot after each made what is pressed show late, and kept the KVM busy answering.
    let news = evs
        .iter()
        .any(|e| !matches!(e, Event::Control(..) | Event::Axis(..) | Event::Report { .. } | Event::PlaceCursor(_)));
    if evs.iter().any(|e| matches!(e, Event::ConfigChanged(_))) {
        seen.stale = true;
    }
    let mut s = shared.lock().unwrap();
    for e in &evs {
        if let Some(text) = describe(e) {
            s.events.push_front(text);
        }
        if let Event::HotkeyLearned(end, h) = e {
            s.learn = Some(Ok((*end, *h)));
        }
        match *e {
            Event::Control(p, down) => {
                if down {
                    s.held.insert(p);
                    s.touched = Some((p, Instant::now()));
                } else {
                    s.held.remove(&p);
                }
            }
            Event::Report { device, iface, offset, total, len, data } => {
                if let Some(m) = s.monitor.as_mut().filter(|m| m.device == device) {
                    m.piece(iface, offset, total, &data[..len as usize]);
                }
            }
            Event::Axis(i, v) => {
                if let Some(a) = s.axes.get_mut(i as usize) {
                    *a = v;
                }
                // A stick or trigger moved well past its rest: a control used, for learning.
                if (i as usize) >= kvm_map::axes::RELATIVE && v.unsigned_abs() > 16000 {
                    let dir = kvm_map::pos::AXIS + 2 * i as u16 + u16::from(v > 0);
                    s.touched = Some((dir, Instant::now()));
                }
            }
            _ => {}
        }
        // Switches go to the monitors straight away, without waiting for the refresh: to a PC, or to a host the last
        // snapshot knows. Back from a host (BtActive(0)) a Switched follows.
        let now = seen.kvm.clone().unwrap_or_default();
        match e {
            Event::Switched(pc, _) => {
                tell(follow, &mut seen.kvm, KvmState { pc: pc.index(), bt: None, ..now });
            }
            Event::BtActive(n @ 1..) => {
                let host = seen.last.as_ref().and_then(|l| l.bt.iter().find(|h| h.slot == *n));
                if let Some(h) = host {
                    let bt = Some(h.address());
                    tell(follow, &mut seen.kvm, KvmState { bt, ..now });
                }
            }
            _ => {}
        }
    }
    s.events.truncate(50);
    Ok(Some(news))
}

/// `sim`: start with the simulator (`Shared::simulate`, which the window may change later). `follow`: switch the
/// monitors bound on the Displays page along (also with the simulator, so it can be tried before a KVM exists; the
/// tests turn it off so that they never move this computer's monitors).
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn(sim: bool, follow: bool, shared: Arc<Mutex<Shared>>, rx: Receiver<UiCmd>, wake: Wake) -> JoinHandle<()> {
    shared.lock().unwrap().simulate = sim;
    let present = || odkvmctl::transport::find().is_ok_and(|f| !f.is_empty());
    spawn_with(follow, shared, rx, wake, Client::connect, present)
}

/// [`spawn`], connecting with `connect` (true: the simulator) and looking for a real KVM with `present`.
#[cfg(not(target_arch = "wasm32"))]
pub fn spawn_with(
    follow: bool,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<UiCmd>,
    wake: Wake,
    connect: impl Fn(bool) -> Result<Client, Error> + Send + 'static,
    present: impl Fn() -> bool + Send + 'static,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let follow = follow.then(|| follower(shared.clone(), wake.clone()));
        let mut seen = Seen::default();
        let changed_over = |sim: bool| shared.lock().unwrap().simulate != sim;
        loop {
            let sim = shared.lock().unwrap().simulate;
            let mut c = match connect(sim) {
                Ok(c) => c,
                Err(e) => {
                    {
                        let mut s = shared.lock().unwrap();
                        s.device = None;
                        s.link = None;
                        s.snap = None;
                        s.mismatch = None;
                        s.not_allowed = None;
                        s.error = Some(e.to_string());
                    }
                    wake();
                    // Commands made while disconnected are dropped; the simulator chosen meanwhile is taken at once.
                    let until = Instant::now() + RECONNECT;
                    while Instant::now() < until && !changed_over(sim) {
                        if let Err(RecvTimeoutError::Disconnected) = rx.recv_timeout(Duration::from_millis(200)) {
                            return;
                        }
                    }
                    continue;
                }
            };
            {
                let mut s = shared.lock().unwrap();
                s.device = Some(c.describe());
                s.link = Some(c.link());
                s.sim_keyboard = c.simulated_keyboard();
                s.error = None;
            }
            let mut next_refresh = Instant::now();
            let mut next_look = Instant::now();
            seen.last = None;
            let mut cursor = CursorSync::default();
            'connected: loop {
                if changed_over(sim) {
                    break 'connected;
                }
                if sim && Instant::now() >= next_look {
                    let there = present();
                    next_look = Instant::now() + LOOK_FOR_KVM;
                    if std::mem::replace(&mut shared.lock().unwrap().kvm_present, there) != there {
                        wake();
                    }
                }
                loop {
                    match rx.try_recv() {
                        Ok(cmd) => match command(&mut c, cmd, &shared, &wake) {
                            Ok(true) => {
                                next_refresh = Instant::now();
                                seen.stale = true;
                            }
                            Ok(false) => {}
                            Err(()) => break 'connected,
                        },
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                if Instant::now() >= next_refresh {
                    if refresh(&mut c, &shared, &follow, &mut seen).is_err() {
                        break 'connected;
                    }
                    next_refresh = Instant::now() + REFRESH;
                    wake();
                    // The module's turn, or (on a snapshot taken after everything ran) the end of the update.
                    if let Some(cmd) = module_due(&shared) {
                        match command(&mut c, cmd, &shared, &wake) {
                            Ok(_) => next_refresh = Instant::now(),
                            Err(()) => break 'connected,
                        }
                    } else if finish_update(&shared) {
                        wake();
                    }
                }
                match take_events(&mut c, Duration::from_millis(100), &shared, &follow, &mut seen) {
                    Ok(Some(true)) => next_refresh = Instant::now(),
                    // What is pressed shows at once.
                    Ok(Some(false)) => wake(),
                    Ok(None) => {}
                    Err(()) => break 'connected,
                }
                cursor.poll(&mut c, seen.last.as_ref().is_some_and(|l| l.status.hop != HopState::Off));
            }
            // Leaving the simulator puts the monitors that follow it back on this computer (the PC it plays), rather
            // than leave them on an input the simulator switched them to.
            if sim
                && changed_over(sim)
                && let Some(now) = seen.kvm.clone()
            {
                let home = seen.last.as_ref().and_then(|s| s.status.asking_pc()).map_or(0, |pc| pc.index());
                tell(&follow, &mut seen.kvm, KvmState { pc: home, bt: None, ..now });
            }
            {
                let mut s = shared.lock().unwrap();
                s.device = None;
                s.link = None;
                s.snap = None;
                s.mismatch = None;
                s.not_allowed = None;
                s.diag = None;
                s.sim_keyboard = None;
                s.kvm_present = false;
                // The KVM went: the restart an update waits for.
                if let Some(e) = s.expected.as_mut() {
                    e.restart = false;
                }
            }
            wake();
            // A change between the simulator and the KVM goes over at once.
            if !changed_over(sim) {
                std::thread::sleep(RECONNECT);
            }
        }
    })
}

/// What `shared` says while `c` is the KVM in use.
#[cfg(target_arch = "wasm32")]
fn connected(c: &Client, shared: &Mutex<Shared>) {
    let mut s = shared.lock().unwrap();
    s.device = Some(c.describe());
    s.link = Some(c.link());
    s.simulate = c.link() == odkvmctl::transport::Link::Simulated;
    s.sim_keyboard = c.simulated_keyboard();
    s.snap = None;
    s.mismatch = None;
    s.not_allowed = None;
    s.diag = None;
}

/// The worker in a browser (the web build): a task on the page's event loop instead of a thread (a page has one),
/// with no monitors to follow. It starts with the simulator, or a KVM this page was given before; the window's
/// *Connect* asks the browser for one (WebHID, Web Bluetooth: [`odkvmctl::webio`]), and a KVM that goes away leaves
/// the simulator in its place. Every request is awaited, so the window runs between them.
#[cfg(target_arch = "wasm32")]
pub fn spawn_web(shared: Arc<Mutex<Shared>>, rx: Receiver<UiCmd>, wake: Wake) {
    use odkvmctl::transport::Link;
    // The simulator, made here and only once: its engine is a fifth of a megabyte, built on the stack, and the task's
    // own frames on top of it overflow the browser's 1 MB stack. While a real KVM is in use it waits, as it was.
    let mut c = Client::connect(true).expect("the simulator is always there");
    connected(&c, &shared);
    wasm_bindgen_futures::spawn_local(async move {
        let mut parked: Option<Client> = None;
        // A KVM this page was given before takes the simulator's place once the browser has said so.
        if let Some(t) = odkvmctl::webio::granted().await {
            parked = Some(std::mem::replace(&mut c, Client::new(t)));
            connected(&c, &shared);
            wake();
        }
        let mut seen = Seen::default();
        let mut next_refresh = Instant::now();
        loop {
            let mut lost = false;
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    UiCmd::WebConnect(link) => match odkvmctl::webio::request(link).await {
                        Ok(t) => {
                            // The KVM in use before (another one) goes; the simulator waits.
                            let before = std::mem::replace(&mut c, Client::new(t));
                            if before.link() == Link::Simulated {
                                parked = Some(before);
                            }
                            connected(&c, &shared);
                            seen = Seen::default();
                            next_refresh = Instant::now();
                        }
                        // The chooser closed without a choice is no error worth a line.
                        Err(e) if e.contains("NotFoundError") || e.contains("No device selected") => {}
                        Err(e) => shared.lock().unwrap().error = Some(e),
                    },
                    UiCmd::WebSimulator => {
                        if let Some(sim) = parked.take() {
                            c = sim;
                            connected(&c, &shared);
                            seen = Seen::default();
                            next_refresh = Instant::now();
                        }
                    }
                    cmd => match command(&mut c, cmd, &shared, &wake).await {
                        Ok(true) => {
                            next_refresh = Instant::now();
                            seen.stale = true;
                        }
                        Ok(false) => {}
                        Err(()) => lost = true,
                    },
                }
                wake();
            }
            if !lost && Instant::now() >= next_refresh {
                lost = refresh(&mut c, &shared, &None, &mut seen).await.is_err();
                next_refresh = Instant::now() + REFRESH;
                wake();
                if let Some(cmd) = module_due(&shared) {
                    lost |= command(&mut c, cmd, &shared, &wake).await.is_err();
                    next_refresh = Instant::now();
                } else if finish_update(&shared) {
                    wake();
                }
            }
            if !lost {
                match take_events(&mut c, Duration::from_millis(50), &shared, &None, &mut seen).await {
                    Ok(Some(true)) => {
                        next_refresh = Instant::now();
                        wake();
                    }
                    Ok(Some(false)) => wake(),
                    Ok(None) => {}
                    Err(()) => lost = true,
                }
            }
            if lost && let Some(sim) = parked.take() {
                // The KVM went (unplugged, out of range): the simulator again, and a line saying why.
                c = sim;
                connected(&c, &shared);
                shared.lock().unwrap().error = Some(fl!("web-kvm-gone"));
                seen = Seen::default();
                next_refresh = Instant::now();
                wake();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvm_proto::link::bt::host_flags;
    use odkvmctl::sim::SimTransport;

    /// The release `cargo xtask dist` built, through the worker against a simulated KVM whose module needs an update:
    /// the RP2354B's image (the simulator presses the key), then the module's, then the page starts afresh. Skipped
    /// without a `dist/` of this version.
    #[test]
    fn an_update_from_the_window_ends_afresh() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist");
        let fits = firmware::for_pc(&dist).is_ok_and(|f| f.version == FW_VERSION_U32)
            && firmware::module_for(&dist, Chip::Unknown).is_ok_and(|m| m.is_some());
        if !fits {
            return;
        }
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel();
        let connect = |_| Ok(Client::new(Box::new(SimTransport::with_module(BtModule::NeedsUpdate))));
        let wake: Wake = Arc::new(|| {});
        spawn_with(false, shared.clone(), rx, wake, connect, || false);
        let deadline = Instant::now() + Duration::from_secs(5);
        while shared.lock().unwrap().snap.is_none() {
            assert!(Instant::now() < deadline, "no snapshot");
            std::thread::sleep(Duration::from_millis(50));
        }
        tx.send(UiCmd::Update(dist)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let s = shared.lock().unwrap();
            if let Some(f) = s.finished {
                assert_eq!((f.version, f.module), (FW_VERSION_U32, true));
                assert!(s.update.is_none() && s.module_result.is_none() && s.module_pending.is_none());
                assert_eq!(s.snap.as_ref().unwrap().status.bt_module, BtModule::Up);
                break;
            }
            assert!(Instant::now() < deadline, "never done: {:?} {:?}", s.update_result, s.module_result);
            drop(s);
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    const FW_VERSION_U32: u32 = {
        let (a, b, c) = kvm_proto::FIRMWARE_VERSION;
        kvm_proto::version_u32(a, b, c)
    };

    #[test]
    fn an_update_is_done_once_the_kvm_and_its_module_are_back() {
        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(true)));
        let snap = c.snapshot().unwrap();
        let version = snap.info.rp_fw;
        let shared = Mutex::new(Shared { snap: Some(snap), ..Default::default() });
        let set = |f: &dyn Fn(&mut Shared)| f(&mut shared.lock().unwrap());
        set(&|s| {
            s.update = Some((FwState::Done, 10, 10));
            s.update_result = Some(Ok(()));
            s.expected = Some(Expected { version, restart: true });
        });
        assert!(!finish_update(&shared), "the KVM hasn't restarted yet");
        set(&|s| s.expected = Some(Expected { version, restart: false }));
        set(&|s| s.module_pending = Some(PathBuf::from("release")));
        assert!(!finish_update(&shared), "the module's turn is still to come");
        set(&|s| s.module_pending = None);
        set(&|s| s.expected = Some(Expected { version: version + 1, restart: false }));
        assert!(!finish_update(&shared), "another version answers");
        set(&|s| s.expected = Some(Expected { version, restart: false }));
        assert!(finish_update(&shared));
        let s = shared.lock().unwrap();
        assert!(s.update.is_none() && s.update_result.is_none() && s.expected.is_none());
        assert_eq!(s.finished.map(|f| (f.version, f.module)), Some((version, true)));
        drop(s);
        // A module update that failed stays shown, and no "done".
        set(&|s| {
            s.finished = None;
            s.module_result = Some(Err("no".into()));
            s.expected = Some(Expected { version, restart: false });
        });
        assert!(!finish_update(&shared));
        let s = shared.lock().unwrap();
        assert!(s.module_result.is_some() && s.finished.is_none() && s.expected.is_none());
    }

    #[test]
    fn bluetooth_changes_are_told_with_the_hosts_names() {
        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(true)));
        let old = c.snapshot().unwrap();
        assert_eq!(bt_changes(&old, &old), Vec::<String>::new());

        let mut new = old.clone();
        new.status.bt_pairing = 60;
        assert_eq!(bt_changes(&old, &new), ["Wireless pairing open"]);

        // A third host pairs (pairing closes with it), the phone connects, the tablet gets the input.
        let (mut before, mut after) = (new.clone(), new.clone());
        before.status.bt_passkey = 3;
        after.status.bt_pairing = 0;
        after.bt[2].info.flags = host_flags::BONDED;
        after.bt[1].info.flags |= host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED;
        after.status.bt_active = 1;
        assert_eq!(
            bt_changes(&before, &after),
            [
                "Phone connected",
                "Paired: Host 3 (slot 3; name it on the Wireless page)",
                "Switched to Tablet (wireless)"
            ]
        );

        // The tablet drops off and is forgotten; the phone disconnects.
        let mut gone = after.clone();
        gone.bt[0].info.flags = 0;
        gone.bt[1].info.flags = host_flags::BONDED;
        gone.status.bt_active = 0;
        assert_eq!(bt_changes(&after, &gone), ["Forgot the wireless host Tablet", "Phone disconnected"]);

        // Pairing that ends without a new host says so; a KVM without the module says nothing.
        assert_eq!(bt_changes(&new, &old), ["Wireless pairing closed"]);
        let mut plain = Client::new(Box::new(SimTransport::with_bluetooth(false)));
        let plain = plain.snapshot().unwrap();
        assert!(bt_changes(&plain, &plain).is_empty());
    }
}
