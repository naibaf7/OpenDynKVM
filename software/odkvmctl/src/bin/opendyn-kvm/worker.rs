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
use kvm_proto::ctrl::{
    ConfigValue, ErrorCode, Event, FwState, HopState, Hotkey, LearnEnd, Port, RebootTarget, Target, diag,
};
use odkvmctl::bootsel::{self, BootDevice};
use odkvmctl::client::{DeviceSlot, Snapshot};
use odkvmctl::ddc::KvmState;
use odkvmctl::{Client, Error, firmware, fl, image, names};

pub enum UiCmd {
    Switch(Target),
    Redetect(Port),
    Config(ConfigValue),
    Save,
    FactoryReset,
    Identify,
    Update(PathBuf),
    /// Give the AT32F405s up to 5 more programming attempts (the KVM restarts).
    RetryAt32,
    /// Give the Bluetooth module up to 5 more programming attempts (the KVM restarts).
    RetryBt,
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
    /// The passkey a pairing host shows.
    BtPasskey(u32),
    /// Look for Bluetooth devices to pair (true), or stop.
    BtScan(bool),
    /// Pair the device found at this address.
    BtPairDevice([u8; 7]),
    /// Forget the Bluetooth device in a slot.
    BtForgetDevice(u8),
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

impl CursorSync {
    fn poll(&mut self, c: &mut Client, hop: bool) {
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
            let _ = c.pointer_at(p.map(|v| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16));
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
        Event::BtActive(_) | Event::BtChanged(_) | Event::BtDeviceChanged(_) | Event::PlaceCursor(_) => return None,
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

/// Lost the device (as opposed to the device refusing a request).
fn is_disconnect(e: &Error) -> bool {
    matches!(e, Error::Transport(_) | Error::Timeout)
}

fn run(c: &mut Client, cmd: UiCmd, shared: &Mutex<Shared>, wake: &Wake) -> Result<(), Error> {
    match cmd {
        UiCmd::Switch(t) => c.switch(t),
        UiCmd::Redetect(p) => c.redetect(p),
        UiCmd::Config(v) => match c.set_config(v) {
            // Allowing software control again: the KVM now waits for its key, and the Settings page says so.
            Err(Error::Device(ErrorCode::NeedsKeyPress)) => Ok(()),
            r => r.map(|_| ()),
        },
        UiCmd::Save => c.save(),
        UiCmd::BtPair(on) => c.bt_pair(on),
        UiCmd::BtForget(n) => c.bt_forget(n),
        UiCmd::BtEraseAll => c.bt_erase_all(),
        UiCmd::BtPasskey(v) => c.bt_passkey(v),
        UiCmd::BtScan(on) => c.bt_scan(on),
        UiCmd::BtPairDevice(addr) => c.bt_pair_device(addr),
        UiCmd::BtForgetDevice(n) => c.bt_forget_device(n),
        UiCmd::FactoryReset => c.factory_reset(),
        UiCmd::Identify => c.identify(5),
        UiCmd::RetryAt32 => c.reboot(RebootTarget::At32Retry),
        UiCmd::RetryBt => c.reboot(RebootTarget::BtRetry),
        UiCmd::Learn(false) => c.stop_learning(),
        UiCmd::Diagnose => {
            let r = match c.diag() {
                Ok(items) => Ok(items),
                Err(e) if is_disconnect(&e) => return Err(e),
                Err(e) => Err(e.to_string()),
            };
            shared.lock().unwrap().diag = Some((Instant::now(), r));
            wake();
            Ok(())
        }
        UiCmd::Learn(true) => {
            let why = match c.start_learning() {
                Ok(()) => return Ok(()),
                Err(Error::Device(code)) => fl!("kvm-refused", why = names::error(code)),
                Err(e) => return Err(e),
            };
            shared.lock().unwrap().learn = Some(Err(why));
            wake();
            Ok(())
        }
        UiCmd::Update(path) => {
            // The same checks as `odkvmctl update`: RP2354B firmware (a file, or a release's folder), for this board.
            let hw = c.info()?.hw;
            let chosen =
                firmware::for_pc(&path).and_then(|f| image::check_hardware(f.version, f.hw_major, hw).map(|()| f));
            let r = match chosen {
                Ok(f) => c.update_firmware(&f.data, |state, done, total| {
                    shared.lock().unwrap().update = Some((state, done, total));
                    wake();
                }),
                Err(e) => Err(Error::Transport(e)),
            };
            shared.lock().unwrap().update_result = Some(r.clone().map_err(|e| e.to_string()));
            r
        }
    }
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
}

/// Runs one of the window's commands. Ok(true): it changes what the snapshot shows (reading the diagnostics doesn't);
/// Err: the KVM is gone.
fn command(c: &mut Client, cmd: UiCmd, shared: &Mutex<Shared>, wake: &Wake) -> Result<bool, ()> {
    let refresh = !matches!(cmd, UiCmd::Diagnose);
    if let Err(e) = run(c, cmd, shared, wake) {
        let lost = is_disconnect(&e);
        shared.lock().unwrap().error = Some(e.to_string());
        if lost {
            return Err(());
        }
    }
    Ok(refresh)
}

/// Reads the KVM's state into `shared`. Err: the KVM is gone.
fn refresh(
    c: &mut Client,
    shared: &Mutex<Shared>,
    follow: &Option<Sender<KvmState>>,
    seen: &mut Seen,
) -> Result<(), ()> {
    match c.snapshot() {
        Ok(snap) => {
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
            if s.error.as_deref().is_some_and(|e| e == Error::Timeout.to_string()) {
                s.error = None;
            }
            Ok(())
        }
        Err(e) if is_disconnect(&e) => Err(()),
        Err(Error::Mismatch(info)) => {
            let mut s = shared.lock().unwrap();
            s.error = Some(Error::Mismatch(info).to_string());
            s.snap = None;
            s.mismatch = Some(info);
            Ok(())
        }
        Err(e) => {
            shared.lock().unwrap().error = Some(e.to_string());
            Ok(())
        }
    }
}

/// Takes the KVM's events into `shared`, waiting up to `wait` for the first. Ok(true): there were some (the snapshot
/// is due); Err: the KVM is gone.
fn take_events(
    c: &mut Client,
    wait: Duration,
    shared: &Mutex<Shared>,
    follow: &Option<Sender<KvmState>>,
    seen: &mut Seen,
) -> Result<bool, ()> {
    let evs = c.events(wait).map_err(|_| ())?;
    if evs.is_empty() {
        return Ok(false);
    }
    let mut s = shared.lock().unwrap();
    for e in &evs {
        if let Some(text) = describe(e) {
            s.events.push_front(text);
        }
        if let Event::HotkeyLearned(end, h) = e {
            s.learn = Some(Ok((*end, *h)));
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
    Ok(true)
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
                        s.snap = None;
                        s.mismatch = None;
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
                            Ok(true) => next_refresh = Instant::now(),
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
                }
                match take_events(&mut c, Duration::from_millis(100), &shared, &follow, &mut seen) {
                    Ok(true) => next_refresh = Instant::now(),
                    Ok(false) => {}
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
                let home = seen.last.as_ref().and_then(|s| s.status.asking).map_or(0, |pc| pc.index());
                tell(&follow, &mut seen.kvm, KvmState { pc: home, bt: None, ..now });
            }
            {
                let mut s = shared.lock().unwrap();
                s.device = None;
                s.snap = None;
                s.mismatch = None;
                s.diag = None;
                s.sim_keyboard = None;
                s.kvm_present = false;
            }
            wake();
            // A change between the simulator and the KVM goes over at once.
            if !changed_over(sim) {
                std::thread::sleep(RECONNECT);
            }
        }
    })
}

/// The worker in a browser (the web build): the simulator only, one round per frame of the window instead of a
/// thread of its own (a browser page has one), and no monitors to follow.
#[cfg(target_arch = "wasm32")]
pub struct Web {
    c: Client,
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<UiCmd>,
    wake: Wake,
    next_refresh: Instant,
    seen: Seen,
}

#[cfg(target_arch = "wasm32")]
impl Web {
    pub fn new(shared: Arc<Mutex<Shared>>, rx: Receiver<UiCmd>, wake: Wake) -> Web {
        let c = Client::connect(true).expect("the simulator is always there");
        {
            let mut s = shared.lock().unwrap();
            s.device = Some(c.describe());
            s.sim_keyboard = c.simulated_keyboard();
        }
        Web { c, shared, rx, wake, next_refresh: Instant::now(), seen: Seen::default() }
    }

    /// The window's commands, the snapshot once a [`REFRESH`] (or after a change), the events; never waits.
    pub fn step(&mut self) {
        while let Ok(cmd) = self.rx.try_recv() {
            if let Ok(true) = command(&mut self.c, cmd, &self.shared, &self.wake) {
                self.next_refresh = Instant::now();
            }
        }
        if Instant::now() >= self.next_refresh {
            let _ = refresh(&mut self.c, &self.shared, &None, &mut self.seen);
            self.next_refresh = Instant::now() + REFRESH;
            (self.wake)();
        }
        if let Ok(true) = take_events(&mut self.c, Duration::ZERO, &self.shared, &None, &mut self.seen) {
            self.next_refresh = Instant::now();
            (self.wake)();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvm_proto::link::bt::host_flags;
    use odkvmctl::sim::SimTransport;

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
