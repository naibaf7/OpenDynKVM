//! A simulated KVM: the firmware's own `kvm_core::Engine`, with three fake front-port devices and both PCs
//! connected. This computer plays PC1.
//!
//! The mouse streams 1 kHz reports through the engine (so report rates and normalization are real); re-detection,
//! firmware updates and descriptor-set changes are acted out with plausible delays. Someone presses the KVM's key when
//! it waits for it. Its keyboard types what is sent to [`SimTransport::simulated_keyboard`]: the GUI sends the keys
//! pressed in its window while a hotkey is learned.
//!
//! With `OPENDYN_SIM_BT=1` it is a 0.16.0 board with the Bluetooth module: a tablet in slot 1, connected, and a phone
//! in slot 2, bonded and away (connect-when-selected). Pairing finds a "Laptop" that asks for a passkey (any six digits
//! do), selecting the phone connects it after a moment. With `OPENDYN_SIM_BT=gave-up` the module is fitted, but
//! programming it gave up after 5 attempts; `reboot bt-retry` (the GUI's Try again) acts out programming it, and then
//! it is up as above. Without either the module isn't fitted, as on most boards.
//!
//! With `OPENDYN_SIM_HOP=1` screen hop is set up and on in the first profile: the absolute pointer, PC1 with a
//! 2560×1440 main screen and a 1920×1080 one left of it, PC2 with a 3840×2160 screen at 150 % to the right.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use heapless::{String as HString, Vec as HVec};
use kvm_core::bt::BtOut;
use kvm_core::config::Config;
use kvm_core::diag::BoardDiag;
use kvm_core::engine::{Engine, FwOp, KeyEvent, Output, PowerInfo, Probed};
use kvm_core::usbdesc::DeviceDesc;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::ctrl::{
    Advertised, DeviceMsg, ErrorCode, FwState, Info, Pc, Port, PowerSource, REPORT_LEN, RebootTarget, Response, Speed,
    caps,
};
use kvm_proto::link::bt::{EventKind as BtEvent, HostState, host_flags};
use kvm_proto::link::msg::{Status, UsbSpeed, UsbState};
use kvm_proto::version_u32;

use crate::transport::{Report, Transport};

/// The simulated KVM runs the current firmware on the current board.
pub const FW_VERSION: u32 = {
    let (major, minor, patch) = kvm_proto::FIRMWARE_VERSION;
    version_u32(major, minor, patch)
};
const HW_VERSION: u32 = {
    let (major, minor, patch) = kvm_proto::HARDWARE_VERSION;
    version_u32(major, minor, patch)
};

#[rustfmt::skip]
const BOOT_KEYBOARD: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
    0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01,
    0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
    0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
];

#[rustfmt::skip]
const MEDIA_KEYS: &[u8] = &[
    0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x03, 0x75, 0x10, 0x95, 0x02, 0x15, 0x01, 0x26, 0xFF, 0x02, 0x19,
    0x01, 0x2A, 0xFF, 0x02, 0x81, 0x00, 0xC0,
];

/// 16 buttons, 16-bit X/Y, wheel, AC Pan; report ID 2.
#[rustfmt::skip]
const GAMING_MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x10,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x10, 0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F,
    0x75, 0x10, 0x95, 0x02, 0x09, 0x30, 0x09, 0x31, 0x81, 0x06, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01,
    0x09, 0x38, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0,
];

/// A vendor command channel in the style of HID++ (report ID 0x10 on a vendor page).
#[rustfmt::skip]
const HIDPP: &[u8] = &[
    0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x10, 0x75, 0x08, 0x95, 0x06, 0x15, 0x00, 0x26, 0xFF, 0x00,
    0x09, 0x01, 0x81, 0x00, 0x09, 0x01, 0x91, 0x00, 0xC0,
];

/// One interface of a fake configuration: class triple, HID report length, endpoints (address, size, interval).
struct Iface<'a> {
    class: (u8, u8, u8),
    report_len: Option<usize>,
    eps: &'a [(u8, u16, u8)],
    alt_with_iso: bool,
}

fn config_descriptor(ifaces: &[Iface], max_power_ma: u16) -> HVec<u8, { kvm_core::engine::MAX_CONFIG_DESC }> {
    let mut c: Vec<u8> = vec![9, 2, 0, 0, ifaces.len() as u8, 1, 0, 0xA0, (max_power_ma / 2) as u8];
    for (n, i) in ifaces.iter().enumerate() {
        c.extend([9, 4, n as u8, 0, i.eps.len() as u8, i.class.0, i.class.1, i.class.2, 0]);
        if let Some(len) = i.report_len {
            c.extend([9, 0x21, 0x11, 0x01, 0, 1, 0x22, len as u8, (len >> 8) as u8]);
        }
        for (addr, mps, interval) in i.eps {
            c.extend([7, 5, *addr, 3, *mps as u8, (*mps >> 8) as u8, *interval]);
        }
        if i.alt_with_iso {
            c.extend([9, 4, n as u8, 1, 1, i.class.0, i.class.1, i.class.2, 0]);
            c.extend([7, 5, 0x81 + n as u8, 5, 0x00, 0x14, 1]);
        }
    }
    let total = c.len() as u16;
    c[2..4].copy_from_slice(&total.to_le_bytes());
    HVec::from_slice(&c).unwrap_or_default()
}

fn hs<const N: usize>(s: &str) -> HString<N> {
    HString::try_from(s).unwrap_or_default()
}

fn probed(
    vid: u16,
    pid: u16,
    manufacturer: &str,
    product: &str,
    config: HVec<u8, 512>,
    reports: &[(u8, &[u8])],
) -> Probed {
    let mut report_descs = HVec::new();
    for (iface, rd) in reports {
        let _ = report_descs.push((*iface, HVec::from_slice(rd).unwrap_or_default()));
    }
    Probed {
        device: DeviceDesc {
            usb: 0x0200,
            max_packet0: 64,
            vid,
            pid,
            bcd: 0x0100,
            configurations: 1,
            ..Default::default()
        },
        speed: Some(Speed::Full),
        config,
        report_descs,
        manufacturer: hs(manufacturer),
        product: hs(product),
        serial: hs(""),
    }
}

fn device_for(port: Port) -> Probed {
    match port {
        Port::Keyboard => probed(
            0x04D9,
            0x0024,
            "Simulated",
            "Mechanical keyboard",
            config_descriptor(
                &[
                    Iface {
                        class: (3, 1, 1),
                        report_len: Some(BOOT_KEYBOARD.len()),
                        eps: &[(0x81, 8, 1)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (3, 0, 0),
                        report_len: Some(MEDIA_KEYS.len()),
                        eps: &[(0x82, 8, 1)],
                        alt_with_iso: false,
                    },
                ],
                200,
            ),
            &[(0, BOOT_KEYBOARD), (1, MEDIA_KEYS)],
        ),
        Port::Mouse => probed(
            0x046D,
            0xC08B,
            "Simulated",
            "Gaming mouse (simulated)",
            config_descriptor(
                &[
                    Iface {
                        class: (3, 1, 2),
                        report_len: Some(GAMING_MOUSE.len()),
                        eps: &[(0x81, 8, 1)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (3, 0, 0),
                        report_len: Some(HIDPP.len()),
                        eps: &[(0x82, 20, 1)],
                        alt_with_iso: false,
                    },
                ],
                98,
            ),
            &[(0, GAMING_MOUSE), (1, HIDPP)],
        ),
        Port::Other => probed(
            0x046D,
            0x085E,
            "Simulated",
            "Webcam",
            config_descriptor(
                &[
                    Iface { class: (0x0E, 1, 0), report_len: None, eps: &[(0x83, 16, 8)], alt_with_iso: false },
                    Iface { class: (0x0E, 2, 0), report_len: None, eps: &[], alt_with_iso: true },
                    Iface { class: (0x01, 1, 0), report_len: None, eps: &[], alt_with_iso: false },
                ],
                500,
            ),
            &[],
        ),
    }
}

fn pc_status(cc_mv: u16, vbus_mv: u16, state: UsbState) -> Status {
    Status {
        usb_state: state,
        speed: UsbSpeed::High,
        vbus_mv,
        cc1_mv: cc_mv,
        cc2_mv: 0,
        frame: 0,
        resets: 1,
        desc_crc: 0,
        // As Windows and Linux set it for a keyboard: a key press may wake a sleeping PC.
        wake_enabled: true,
        vdda_mv: Some(3290),
        temp_dc: Some(380),
    }
}

pub struct SimTransport {
    engine: Engine,
    start: Instant,
    last_ms: u32,
    inbox: VecDeque<Report>,
    /// (when, what) for delayed effects.
    timeline: Vec<(u32, Delayed)>,
    /// What the keyboard types.
    keys: (Sender<[u8; 8]>, Receiver<[u8; 8]>),
    /// The simulated Bluetooth module's host slots, when it is fitted (`OPENDYN_SIM_BT`).
    bt: Option<[HostState; 4]>,
}

const BT_READY: u8 = host_flags::BONDED | host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED;

fn sim_host(slot: u8, flags: u8) -> HostState {
    let interval = if flags & host_flags::CONNECTED != 0 { 12 } else { 0 };
    HostState { host: slot, flags, interval, latency: 0, addr: [1, 0x5A, 0x6E, 0x3B, 0x21, 0xC4, slot], rssi: -52 }
}

enum Delayed {
    Attach(Port),
    /// The simulated Bluetooth module: a host shows up while pairing, connects, or bonds.
    BtFound,
    BtHost(HostState),
    PcsConfigured,
    KeyPress,
    FirmwareDone,
    /// The Bluetooth module programmed (after `BtRetry`), then saying hello.
    BtProgrammed,
    BtUp,
}

impl Default for SimTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl SimTransport {
    /// The simulated KVM, with the Bluetooth module if `OPENDYN_SIM_BT=1`, or a module that gave up with `gave-up`.
    pub fn new() -> Self {
        Self::with_module(match std::env::var("OPENDYN_SIM_BT").as_deref() {
            Ok("1") => BtModule::Up,
            Ok("gave-up") => BtModule::GaveUp,
            _ => BtModule::Absent,
        })
    }

    pub fn with_bluetooth(fitted: bool) -> Self {
        Self::with_module(if fitted { BtModule::Up } else { BtModule::Absent })
    }

    /// The simulated KVM with its Bluetooth module `module`: absent, up, or given up (anything else counts as up).
    pub fn with_module(module: BtModule) -> Self {
        let info = Info {
            proto: kvm_proto::ctrl::PROTOCOL_VERSION,
            rp_fw: FW_VERSION,
            at_fw: [0; 2],
            hw: HW_VERSION,
            serial: *b"SIM00001",
            caps: caps::MIRRORED | caps::IDENTITY | caps::PASSTHROUGH | caps::FW_UPDATE | caps::VIAL | caps::SIMULATED,
            bt_fw: 0,
        };
        let mut engine = Engine::new(Config::default(), info, "SIM00001");
        engine.pc_hello(Pc::Pc1, FW_VERSION);
        engine.pc_hello(Pc::Pc2, FW_VERSION);
        engine.pc_status(Pc::Pc1, pc_status(940, 5050, UsbState::Configured));
        engine.pc_status(Pc::Pc2, pc_status(420, 4980, UsbState::Configured));
        engine.set_power(PowerInfo { source: PowerSource::PcBus, adv_pwr: Advertised::None, vbus_pwr_mv: 0 });
        engine.set_selftest(if module == BtModule::GaveUp { kvm_proto::ctrl::selftest::BT_GAVE_UP } else { 0 });
        engine.set_board_diag(BoardDiag::TYPICAL);
        for p in Port::ALL {
            engine.port_attached(*p, &device_for(*p));
        }
        if std::env::var("OPENDYN_SIM_HOP").as_deref() == Ok("1") {
            Self::hop_demo(&mut engine);
        }
        // The simulation starts with everything already enumerated: boot-time outputs are not acted out.
        let bt = match module {
            BtModule::Absent => None,
            BtModule::GaveUp => {
                engine.set_bt_module(BtModule::GaveUp);
                None
            }
            _ => Some(Self::module_up(&mut engine)),
        };
        while engine.poll_output().is_some() {}
        Self {
            engine,
            start: Instant::now(),
            last_ms: 0,
            inbox: VecDeque::new(),
            timeline: Vec::new(),
            keys: channel(),
            bt,
        }
    }

    /// Screen hop set up (`OPENDYN_SIM_HOP=1`).
    fn hop_demo(engine: &mut Engine) {
        use kvm_proto::ctrl::{ConfigValue, Screen};
        let pc2 = Screen { scale: 150, ..Screen::new(Pc::Pc2, 3840, 2160, [2560, -120]) };
        for v in [
            ConfigValue::AbsPointer(true),
            ConfigValue::ProfileHop(0, true),
            ConfigValue::Screen(0, Screen::new(Pc::Pc1, 2560, 1440, [0, 0])),
            ConfigValue::Screen(1, Screen { desk: [-1920, 360], ..Screen::new(Pc::Pc1, 1920, 1080, [-1920, 360]) }),
            ConfigValue::Screen(2, pc2),
        ] {
            let _ = engine.set_config(v, kvm_core::config::Origin::Device);
        }
    }

    /// The module says hello with its hosts: a tablet in slot 1, connected, a phone in slot 2, away.
    fn module_up(engine: &mut Engine) -> [HostState; 4] {
        let hosts = [sim_host(1, BT_READY), sim_host(2, host_flags::BONDED), sim_host(3, 0), sim_host(4, 0)];
        engine.set_bt_module(BtModule::Up);
        engine.bt_hello(FW_VERSION);
        let names = [(0, "Tablet"), (1, "Phone")];
        for (i, name) in names {
            let v = kvm_proto::ctrl::ConfigValue::BtName(i, kvm_proto::ctrl::ProfileName::new(name));
            let _ = engine.set_config(v, kvm_core::config::Origin::Device);
        }
        let policy = kvm_proto::ctrl::ConfigValue::BtPolicy(1, kvm_proto::ctrl::BtPolicy::ConnectWhenSelected);
        let _ = engine.set_config(policy, kvm_core::config::Origin::Device);
        for h in hosts {
            engine.bt_host(h);
        }
        hosts
    }

    /// The simulated module acts on what the engine asks of it.
    fn bt_out(&mut self, o: BtOut) {
        let Some(hosts) = self.bt.as_mut() else { return };
        match o {
            BtOut::Pair { on: true, seconds } => {
                self.engine.bt_event(BtEvent::PairingOpen, 0, seconds as u32);
                self.later(3000, Delayed::BtFound);
            }
            BtOut::Pair { on: false, .. } => self.engine.bt_event(BtEvent::PairingClosed, 0, 0),
            BtOut::Passkey { host, value: Some(_) } => {
                self.engine.bt_event(BtEvent::Paired, host, 0);
                self.later(600, Delayed::BtHost(sim_host(host, BT_READY)));
            }
            BtOut::Passkey { host, value: None } => {
                self.engine.bt_event(BtEvent::PairFailed, host, 0x01);
                self.engine.bt_event(BtEvent::PairingClosed, 0, 0);
            }
            BtOut::Forget(n) => {
                for h in hosts.iter_mut().filter(|h| n == 0xFF || h.host == n) {
                    *h = sim_host(h.host, 0);
                    self.engine.bt_host(*h);
                }
            }
            BtOut::Select(n) => {
                // A host that connects only while selected connects now.
                if let Some(h) = hosts.iter().find(|h| h.host == n && h.bonded() && !h.connected()) {
                    let h = sim_host(h.host, BT_READY);
                    self.later(800, Delayed::BtHost(h));
                }
            }
            BtOut::RequestState => {
                for h in *hosts {
                    self.engine.bt_host(h);
                }
            }
            BtOut::Input { .. } | BtOut::Policy { .. } => {}
        }
    }

    fn now(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn later(&mut self, ms: u32, d: Delayed) {
        let at = self.now() + ms;
        self.timeline.push((at, d));
    }

    fn mouse_report(t: u32) -> [u8; 9] {
        // A slow circle: deltas of at most 1, no buttons. Small enough that screen hop's pointer
        // (`OPENDYN_SIM_HOP=1`) circles in the middle of a screen and never pushes across to the other PC.
        let phase = (t % 2000) as f32 / 2000.0 * std::f32::consts::TAU;
        let (x, y) = ((phase.cos() * 1.5) as i16, (phase.sin() * 1.5) as i16);
        let mut r = [0u8; 9];
        r[0] = 2;
        r[3..5].copy_from_slice(&x.to_le_bytes());
        r[5..7].copy_from_slice(&y.to_le_bytes());
        r
    }

    /// Advances simulated time: mouse traffic, timers, outputs.
    fn pump(&mut self) {
        let now = self.now();
        // Catch up on reports missed while nobody asked (bounded, so a long pause doesn't stall the caller).
        let from = self.last_ms.max(now.saturating_sub(3000));
        for t in from..now {
            self.engine.device_report(Port::Mouse, 0, &Self::mouse_report(t), t);
            // Drain after every call, as the firmware does: the engine's output queue is sized for one call.
            self.drain();
        }
        self.last_ms = now;
        while let Ok(r) = self.keys.1.try_recv() {
            self.engine.device_report(Port::Keyboard, 0, &r, now);
            self.drain();
        }
        self.engine.tick(now);
        self.drain();

        let due: Vec<Delayed> = {
            let (due, keep): (Vec<_>, Vec<_>) = self.timeline.drain(..).partition(|(at, _)| *at <= now);
            self.timeline = keep;
            due.into_iter().map(|(_, d)| d).collect()
        };
        for d in due {
            match d {
                Delayed::Attach(p) => self.engine.port_attached(p, &device_for(p)),
                Delayed::BtFound => {
                    if let Some(free) = self.bt.and_then(|hs| hs.iter().find(|h| !h.bonded()).map(|h| h.host)) {
                        let _ = self.engine.set_config(
                            kvm_proto::ctrl::ConfigValue::BtName(free - 1, kvm_proto::ctrl::ProfileName::new("Laptop")),
                            kvm_core::config::Origin::Device,
                        );
                        self.engine.bt_event(BtEvent::PasskeyRequest, free, 0);
                    }
                }
                Delayed::BtHost(h) => {
                    if let Some(hosts) = self.bt.as_mut() {
                        hosts[h.host as usize - 1] = h;
                    }
                    self.engine.bt_host(h);
                }
                Delayed::PcsConfigured => {
                    self.engine.pc_status(Pc::Pc1, pc_status(940, 5050, UsbState::Configured));
                    self.engine.pc_status(Pc::Pc2, pc_status(420, 4980, UsbState::Configured));
                }
                Delayed::KeyPress => self.engine.key(KeyEvent::Tap),
                Delayed::FirmwareDone => self.engine.set_fw_state(FwState::Done, 0),
                Delayed::BtProgrammed => self.engine.set_bt_module(BtModule::Starting),
                Delayed::BtUp => self.bt = Some(Self::module_up(&mut self.engine)),
            }
            self.drain();
        }
    }

    fn drain(&mut self) {
        while let Some(o) = self.engine.poll_output() {
            match o {
                Output::Report { pc: Pc::Pc1, ep, data } if ep == self.engine.presentation().control.ep_in => {
                    let mut r = [0u8; REPORT_LEN];
                    r[..data.len()].copy_from_slice(&data);
                    self.inbox.push_back(r);
                }
                Output::Reprobe(p) => {
                    self.engine.port_probing(p);
                    self.later(700, Delayed::Attach(p));
                }
                Output::DescriptorSetChanged => {
                    // Both PCs re-enumerate the composite.
                    self.engine.pc_status(Pc::Pc1, pc_status(940, 5050, UsbState::Default));
                    self.engine.pc_status(Pc::Pc2, pc_status(420, 4980, UsbState::Default));
                    self.later(900, Delayed::PcsConfigured);
                }
                Output::Save => {
                    let mut buf = [0u8; kvm_core::engine::SAVE_ITEM_MAX];
                    while self.engine.take_save_item(&mut buf).is_some() {}
                    self.engine.config_saved();
                }
                Output::Firmware(FwOp::Commit) => self.later(2500, Delayed::KeyPress),
                Output::Firmware(FwOp::Confirm) => self.later(1500, Delayed::FirmwareDone),
                Output::Bt(b) => self.bt_out(b),
                // The KVM restarts with the module's count cleared and programs it: acted out without the restart.
                Output::Reboot(RebootTarget::BtRetry) if self.engine.status(None).bt_module == BtModule::GaveUp => {
                    let bits = self.engine.selftest() & !kvm_proto::ctrl::selftest::BT;
                    self.engine.set_selftest(bits);
                    self.engine.set_bt_module(BtModule::Programming);
                    self.later(4000, Delayed::BtProgrammed);
                    self.later(5500, Delayed::BtUp);
                }
                _ => {}
            }
        }
    }
}

impl Transport for SimTransport {
    fn write(&mut self, report: &Report) -> Result<(), String> {
        self.pump();
        if let Some(resp) = self.engine.control(Some(Pc::Pc1), report) {
            // Someone "presses the key" on the simulated KVM when it asks for it.
            if let Ok(DeviceMsg::Response { response: Response::Error(ErrorCode::NeedsKeyPress), .. }) =
                DeviceMsg::decode(&resp)
            {
                self.later(2500, Delayed::KeyPress);
            }
            self.inbox.push_back(resp);
        }
        self.pump();
        Ok(())
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if let Some(r) = self.inbox.pop_front() {
                return Ok(Some(r));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            std::thread::sleep(left.min(Duration::from_millis(5)));
        }
    }

    fn describe(&self) -> String {
        crate::fl!("simulated-kvm")
    }

    fn simulated_keyboard(&self) -> Option<Sender<[u8; 8]>> {
        Some(self.keys.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use kvm_proto::ctrl::{ConfigValue, Event, Hotkey, LearnEnd, PortMode, PortState, SwitchSource, Target};

    #[test]
    fn configuration_comes_back_after_the_key_is_pressed() {
        let mut c = Client::new(Box::new(SimTransport::new()));
        c.set_config(ConfigValue::SwConfigAllowed(false)).unwrap();
        assert_eq!(c.set_config(ConfigValue::LedBrightness(9)), Err(crate::Error::Device(ErrorCode::Locked)));
        let mut asked = false;
        // The simulator presses the key 2.5 s after the KVM asks for it.
        c.allow_software(ConfigValue::SwConfigAllowed(true), || asked = true).unwrap();
        assert!(asked);
        assert_eq!(c.set_config(ConfigValue::LedBrightness(9)), Ok(ConfigValue::LedBrightness(9)));
    }

    #[test]
    fn the_simulated_keyboard_types_a_hotkey_to_learn() {
        let mut c = Client::new(Box::new(SimTransport::new()));
        let keys = c.simulated_keyboard().unwrap();
        // Ctrl+Alt+K, typed once the KVM listens.
        let (end, h) = c
            .learn_hotkey(|| {
                for r in
                    [[0x05, 0, 0, 0, 0, 0, 0, 0], [0x05, 0, 0x0E, 0, 0, 0, 0, 0], [0x05, 0, 0, 0, 0, 0, 0, 0], [0; 8]]
                {
                    keys.send(r).unwrap();
                }
            })
            .unwrap();
        assert_eq!(
            (end, crate::names::format_value(&ConfigValue::HotkeyToggle(h))),
            (LearnEnd::Learned, "ctrl+alt+k".into())
        );
        assert_eq!(
            c.get_config(kvm_proto::ctrl::key::HOTKEY_TOGGLE),
            Ok(ConfigValue::HotkeyToggle(Hotkey::double_tap(0x47, true)))
        );
    }

    #[test]
    fn a_module_that_gave_up_is_programmed_again_on_retry() {
        let mut c = Client::new(Box::new(SimTransport::with_module(BtModule::GaveUp)));
        let s = c.snapshot().unwrap();
        assert!(!s.has_bluetooth() && s.bt.is_empty(), "no Bluetooth while the module doesn't run");
        assert_eq!(s.status.bt_module, BtModule::GaveUp);
        assert_ne!(s.status.selftest & kvm_proto::ctrl::selftest::BT_GAVE_UP, 0);
        c.reboot(RebootTarget::BtRetry).unwrap();
        assert_eq!(c.status().unwrap().bt_module, BtModule::Programming);
        let deadline = Instant::now() + Duration::from_secs(10);
        while c.status().unwrap().bt_module != BtModule::Up {
            assert!(Instant::now() < deadline, "the module never came up");
            std::thread::sleep(Duration::from_millis(100));
        }
        let s = c.snapshot().unwrap();
        assert!(s.has_bluetooth());
        assert_eq!(s.info.bt_fw, FW_VERSION);
        assert_eq!(s.status.selftest & kvm_proto::ctrl::selftest::BT, 0);
        assert_eq!(s.bt_bonded().count(), 2);
    }

    #[test]
    fn bluetooth_hosts_pair_switch_and_go_away_without_the_module() {
        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(false)));
        let s = c.snapshot().unwrap();
        assert!(!s.has_bluetooth() && s.bt.is_empty());

        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(true)));
        let s = c.snapshot().unwrap();
        assert!(s.has_bluetooth());
        let names: Vec<&str> = s.bt_bonded().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["Tablet", "Phone"]);
        // Pairing finds a laptop that asks for its passkey; typed in software here.
        c.bt_pair(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        while c.status().unwrap().bt_passkey == 0 {
            assert!(Instant::now() < deadline, "no passkey request");
            std::thread::sleep(Duration::from_millis(50));
        }
        c.bt_passkey(424242).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let s = c.snapshot().unwrap();
        assert_eq!(s.bt_bonded().count(), 3);
        assert!(s.bt.iter().any(|h| h.name == "Laptop" && h.ready()));
        assert_eq!(s.status.bt_pairing, 0);
        // Switching to a host and back.
        c.switch(kvm_proto::ctrl::Target::Bt1).unwrap();
        assert_eq!(c.status().unwrap().bt_active, 1);
        c.switch(kvm_proto::ctrl::Target::Toggle).unwrap();
        let st = c.status().unwrap();
        assert_eq!((st.bt_active, st.active), (0, Pc::Pc1));
    }

    #[test]
    fn simulated_kvm_answers_like_a_real_one() {
        let mut c = Client::new(Box::new(SimTransport::new()));
        let s = c.snapshot().unwrap();
        assert_eq!(s.status.asking, Some(Pc::Pc1));
        assert_eq!(s.ports[Port::Keyboard.index()].effective, PortMode::Normalized);
        assert_eq!(s.ports[Port::Mouse.index()].effective, PortMode::Normalized, "HID++ alone doesn't force mirroring");
        assert_eq!(s.ports[Port::Other.index()].state, PortState::Passthrough);
        assert_eq!(s.products[Port::Mouse.index()], "Gaming mouse (simulated)");

        c.switch(Target::Pc2).unwrap();
        let ev = c.events(Duration::from_millis(50)).unwrap();
        assert!(ev.contains(&Event::Switched(Pc::Pc2, SwitchSource::Software)));

        std::thread::sleep(Duration::from_millis(1100));
        let rate = c.port(Port::Mouse).unwrap().report_rate_hz;
        assert!((800..=1100).contains(&rate), "mouse rate {rate}");

        // The simulator acts out the key press on the KVM.
        c.update_firmware(&[0x55; 3000], |_, _, _| {}).unwrap();

        let v = c.set_config(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored)).unwrap();
        assert_eq!(v, ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored));
        assert_eq!(c.port(Port::Mouse).unwrap().effective, PortMode::Mirrored);
    }

    #[test]
    fn the_simulated_kvm_is_healthy_in_its_diagnostics() {
        use kvm_proto::ctrl::diag::{Grade, id};
        let mut c = Client::new(Box::new(SimTransport::new()));
        let items = c.diag().unwrap();
        assert!(items.len() > kvm_proto::ctrl::diag::PER_PAGE, "more than one page");
        let bad: Vec<_> = items.iter().filter(|i| matches!(i.grade, Grade::Warn | Grade::Bad)).collect();
        assert!(bad.is_empty(), "{bad:?}");
        let usb = items.iter().find(|i| i.id == id::CLK_USB).unwrap();
        assert_eq!(crate::names::diag_value(usb), "48.000 MHz");
        let t = items.iter().find(|i| i.id == id::AT1_3V3).unwrap();
        assert_eq!(crate::names::diag_value(t), "3.29 V");
    }
}
