//! Request/response client over a [`Transport`], with typed helpers.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use kvm_proto::crc::crc32;
use kvm_proto::ctrl::{
    Accel, BT_HOSTS, BtHostInfo, BtPointer, BtPolicy, ConfigValue, DeviceMsg, ErrorCode, Event, FW_CHUNK, FwState,
    FwTarget, HOSTS, Host, Hotkey, Info, LearnEnd, MAX_ARRANGEMENTS, MAX_PROFILES, MAX_SCREENS, Mapping, Pc, Port,
    PortInfo, PortMode, PortState, RebootTarget, Request, Response, Screen, Status, StringKind, Target, Units, caps,
    diag, key, status_flags,
};
use kvm_proto::link::bt::host_flags;

use crate::fl;
use crate::transport::{HidTransport, Transport};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    Timeout,
    /// The KVM refused the request.
    Device(ErrorCode),
    Protocol(String),
    /// The KVM waited for its key, and nobody pressed it.
    NotConfirmed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "{e}"),
            Error::Timeout => f.write_str(&fl!("error-timeout")),
            Error::Device(c) => f.write_str(&fl!("error-device", why = crate::names::error(*c))),
            Error::Protocol(e) => f.write_str(&fl!("error-protocol", error = e.as_str())),
            Error::NotConfirmed => f.write_str(&fl!("error-not-confirmed")),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub struct Client {
    t: Box<dyn Transport>,
    next_id: u8,
    events: VecDeque<Event>,
    pub timeout: Duration,
}

/// Everything the UIs show, fetched in one go.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub info: Info,
    pub status: Status,
    pub ports: [PortInfo; 3],
    pub products: [String; 3],
    pub config: Vec<ConfigValue>,
    /// The Bluetooth host slots 1–4, read only from a KVM with the Bluetooth module (`caps::BLUETOOTH`); empty
    /// otherwise, which is how the UIs know to show the two PCs alone.
    pub bt: Vec<BtHostView>,
}

/// One Bluetooth host slot as the UIs show it: the module's view plus the KVM's settings for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtHostView {
    pub slot: u8,
    /// The name set for it, or "BT n".
    pub name: String,
    pub info: BtHostInfo,
    pub policy: BtPolicy,
}

impl BtHostView {
    pub fn bonded(&self) -> bool {
        self.info.flags & host_flags::BONDED != 0
    }

    pub fn connected(&self) -> bool {
        self.info.flags & host_flags::CONNECTED != 0
    }

    /// Connected, encrypted and listening: input reaches it.
    pub fn ready(&self) -> bool {
        let need = host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED;
        self.info.flags & need == need
    }

    pub fn target(&self) -> Target {
        Target::bt(self.slot).unwrap_or(Target::Bt1)
    }

    /// The host's identity address, as hosts show it ("C4:21:3B:6E:5A:01"): what this computer keys its settings for a
    /// host by (`local::DisplayBinding::bt_inputs`), since a slot can be forgotten and given to another host.
    pub fn address(&self) -> String {
        let a = &self.info.addr;
        format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", a[6], a[5], a[4], a[3], a[2], a[1])
    }
}

/// One profile as the UIs show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileView {
    pub name: String,
    /// Keyboard, Mouse, Other.
    pub modes: [PortMode; 3],
    /// The arrangement screen hop uses in it; None: screen hop is off there (and from firmware without it).
    pub arrangement: Option<u8>,
}

/// Screen hop's settings and arrangements (software.md §4.14), from firmware with `caps::SCREEN_HOP`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HopView {
    pub abs_pointer: bool,
    pub accel: Accel,
    pub speed: i8,
    pub resistance: u16,
    pub corner: u16,
    /// Each host's, by `Host::index`: PC1, PC2, the Bluetooth hosts.
    pub mapping: [Mapping; HOSTS],
    /// What each host's desktop coordinates count, by `Host::index`.
    pub units: [Units; HOSTS],
    /// How screen hop moves each Bluetooth host's pointer.
    pub bt_pointer: [BtPointer; BT_HOSTS],
    /// The KVM is the built-in simulator: its screens aren't this computer's.
    pub simulated: bool,
    pub arrangements: [ArrangementView; MAX_ARRANGEMENTS],
    /// The active profile's arrangement (None: screen hop is off in it).
    pub in_use: Option<u8>,
}

/// One stored arrangement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrangementView {
    /// Its name; empty: "Arrangement n" ([`ArrangementView::title`]).
    pub name: String,
    pub screens: [Screen; MAX_SCREENS],
}

impl ArrangementView {
    /// The name to show for arrangement `a`.
    pub fn title(&self, a: u8) -> String {
        if self.name.is_empty() { fl!("arrangement-n", n = a) } else { self.name.clone() }
    }

    pub fn is_empty(&self) -> bool {
        self.screens.iter().all(|s| s.host.is_none())
    }
}

impl HopView {
    pub fn mapping(&self, h: Host) -> Mapping {
        self.mapping[h.index()]
    }

    pub fn units(&self, h: Host) -> Units {
        self.units[h.index()]
    }

    /// The arrangement a command works on when none is named: the active profile's, else the first.
    pub fn current(&self) -> u8 {
        self.in_use.unwrap_or(0)
    }
}

impl Snapshot {
    /// The KVM has the Bluetooth module, so Bluetooth hosts are targets.
    pub fn has_bluetooth(&self) -> bool {
        self.info.caps & caps::BLUETOOTH != 0
    }

    /// The front port whose ESP32-C3 board is the Bluetooth module, on a KVM without it on the board.
    pub fn module_port(&self) -> Option<Port> {
        self.ports.iter().find(|p| p.state == PortState::Module).map(|p| p.port)
    }

    /// The bonded Bluetooth hosts (the slots in use).
    pub fn bt_bonded(&self) -> impl Iterator<Item = &BtHostView> {
        self.bt.iter().filter(|h| h.bonded())
    }

    /// The KVM has screen hop (protocol 7).
    pub fn has_hop(&self) -> bool {
        self.info.caps & caps::SCREEN_HOP != 0
    }

    /// Screen hop's settings and arrangement; None from firmware without it.
    pub fn hop(&self) -> Option<HopView> {
        if !self.has_hop() {
            return None;
        }
        let empty = ArrangementView { name: String::new(), screens: [Screen::NONE; MAX_SCREENS] };
        let mut v = HopView {
            abs_pointer: false,
            accel: Accel::Precision,
            speed: 0,
            resistance: 0,
            corner: 0,
            mapping: [Mapping::Desktop; HOSTS],
            units: [Units::Physical; HOSTS],
            bt_pointer: [BtPointer::Absolute; BT_HOSTS],
            simulated: self.info.caps & caps::SIMULATED != 0,
            arrangements: core::array::from_fn(|_| empty.clone()),
            in_use: self.profiles().and_then(|(active, _, list)| list.get(active as usize)?.arrangement),
        };
        for c in &self.config {
            match *c {
                ConfigValue::AbsPointer(b) => v.abs_pointer = b,
                ConfigValue::HopAccel(a) => v.accel = a,
                ConfigValue::HopSpeed(s) => v.speed = s,
                ConfigValue::HopResistance(r) => v.resistance = r,
                ConfigValue::HopCorner(c) => v.corner = c,
                ConfigValue::HopMapping(h, m) => v.mapping[h.index()] = m,
                ConfigValue::HopUnits(h, u) => v.units[h.index()] = u,
                ConfigValue::BtPointer(i, p) if (i as usize) < BT_HOSTS => v.bt_pointer[i as usize] = p,
                ConfigValue::ArrangementName(a, n) if (a as usize) < MAX_ARRANGEMENTS => {
                    v.arrangements[a as usize].name = n.as_str().to_string();
                }
                ConfigValue::Screen(a, i, s) if (a as usize) < MAX_ARRANGEMENTS && (i as usize) < MAX_SCREENS => {
                    v.arrangements[a as usize].screens[i as usize] = s;
                }
                _ => {}
            }
        }
        Some(v)
    }

    /// The PC this computer is (the one the KVM answered), if it says.
    pub fn this_pc(&self) -> Option<Pc> {
        self.status.asking
    }

    /// The active profile and every stored one (the first `count` in use); None from firmware without profiles.
    pub fn profiles(&self) -> Option<(u8, u8, Vec<ProfileView>)> {
        let find = |k: u16| self.config.iter().find(|v| v.key() == k);
        let Some(ConfigValue::Profile(active)) = find(key::PROFILE) else { return None };
        let Some(ConfigValue::ProfileCount(count)) = find(key::PROFILE_COUNT) else { return None };
        let list = (0..MAX_PROFILES as u16)
            .map(|i| {
                let name = match find(key::PROFILE_NAME_BASE + i) {
                    Some(ConfigValue::ProfileName(_, n)) => n.as_str().to_string(),
                    _ => fl!("profile-n", n = i),
                };
                let modes = match find(key::PROFILE_MODES_BASE + i) {
                    Some(ConfigValue::ProfileModes(_, m)) => *m,
                    _ => [PortMode::Auto; 3],
                };
                let arrangement = match find(key::PROFILE_ARRANGEMENT_BASE + i) {
                    Some(ConfigValue::ProfileArrangement(_, a)) => *a,
                    _ => None,
                };
                ProfileView { name, modes, arrangement }
            })
            .collect();
        Some((*active, *count, list))
    }
}

impl Client {
    pub fn new(t: Box<dyn Transport>) -> Self {
        // Every program that has the control interface open reads every answer (hidraw on Linux): a window and
        // `odkvmctl` running together each start numbering somewhere else, and an answer counts only with both its id
        // and its request type (`request`). Both counting from 1, `odkvmctl update` once took the window's
        // `GetConfig` answer for its own and gave up (2026-11-12).
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let next_id = ((seed ^ std::process::id().rotate_left(8)) % 255) as u8 + 1;
        Self { t, next_id, events: VecDeque::new(), timeout: Duration::from_millis(1000) }
    }

    /// The first KVM found, or the simulator.
    pub fn connect(sim: bool) -> Result<Self> {
        if sim {
            return Ok(Self::new(Box::new(crate::sim::SimTransport::new())));
        }
        let t = HidTransport::open_first().map_err(Error::Transport)?;
        Ok(Self::new(Box::new(t)))
    }

    pub fn describe(&self) -> String {
        self.t.describe()
    }

    /// With the simulator: where to send what its keyboard types (`Transport::simulated_keyboard`).
    pub fn simulated_keyboard(&self) -> Option<std::sync::mpsc::Sender<[u8; 8]>> {
        self.t.simulated_keyboard()
    }

    pub fn request(&mut self, req: &Request) -> Result<Response> {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        let raw = req.encode(id).map_err(|e| Error::Protocol(format!("{e:?}")))?;
        self.t.write(&raw).map_err(Error::Transport)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            let Some(r) = self.t.read(left).map_err(Error::Transport)? else { return Err(Error::Timeout) };
            match DeviceMsg::decode(&r) {
                Ok(DeviceMsg::Event(e)) => self.events.push_back(e),
                Ok(DeviceMsg::Response { id: rid, request, response }) if rid == id && request == req.type_byte() => {
                    return match response {
                        Response::Error(code) => Err(Error::Device(code)),
                        r => Ok(r),
                    };
                }
                // A late answer to an earlier, timed-out request, another program's answer, or something this version
                // doesn't know.
                _ => {}
            }
        }
    }

    /// Events received so far plus any that arrive within `timeout`.
    pub fn events(&mut self, timeout: Duration) -> Result<Vec<Event>> {
        let mut wait = timeout;
        while let Some(r) = self.t.read(wait).map_err(Error::Transport)? {
            if let Ok(DeviceMsg::Event(e)) = DeviceMsg::decode(&r) {
                self.events.push_back(e);
            }
            wait = Duration::ZERO;
        }
        Ok(self.events.drain(..).collect())
    }

    fn expect<T>(&mut self, req: Request, f: impl FnOnce(Response) -> Option<T>) -> Result<T> {
        let r = self.request(&req)?;
        let what = format!("{r:?}");
        f(r).ok_or_else(|| Error::Protocol(fl!("protocol-unexpected", answer = what)))
    }

    fn ok(&mut self, req: Request) -> Result<()> {
        self.expect(req, |r| matches!(r, Response::Ok).then_some(()))
    }

    pub fn info(&mut self) -> Result<Info> {
        self.expect(Request::GetInfo, |r| if let Response::Info(i) = r { Some(i) } else { None })
    }

    pub fn status(&mut self) -> Result<Status> {
        self.expect(Request::GetStatus, |r| if let Response::Status(s) = r { Some(s) } else { None })
    }

    /// Every diagnostics item (protocol 5; older firmware refuses with `Malformed`), in the firmware's order. Pages
    /// fetched while the list changes may repeat an item: the later reading counts.
    pub fn diag(&mut self) -> Result<Vec<diag::Item>> {
        let mut items: Vec<diag::Item> = Vec::new();
        let mut page = 0u8;
        loop {
            let (pages, got) = self.expect(Request::GetDiag(page), |r| match r {
                Response::Diag { pages, items, .. } => Some((pages, items)),
                _ => None,
            })?;
            for i in got {
                match items.iter_mut().find(|o| o.id == i.id) {
                    Some(old) => *old = i,
                    None => items.push(i),
                }
            }
            page += 1;
            if page >= pages {
                return Ok(items);
            }
        }
    }

    pub fn port(&mut self, p: Port) -> Result<PortInfo> {
        self.expect(Request::GetPort(p), |r| if let Response::Port(i) = r { Some(i) } else { None })
    }

    /// The product string of a port's device ("" when the port is empty).
    pub fn port_string(&mut self, p: Port, kind: StringKind) -> Result<String> {
        match self.request(&Request::GetPortString(p, kind)) {
            Ok(Response::PortString(_, _, s)) => Ok(s.to_string()),
            Err(Error::Device(ErrorCode::NoDevice)) => Ok(String::new()),
            Ok(other) => Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
            Err(e) => Err(e),
        }
    }

    pub fn switch(&mut self, t: Target) -> Result<()> {
        self.ok(Request::SetActive(t))
    }

    pub fn set_port_mode(&mut self, p: Port, m: PortMode) -> Result<()> {
        self.ok(Request::SetPortMode(p, m))
    }

    pub fn redetect(&mut self, p: Port) -> Result<()> {
        self.ok(Request::Redetect(p))
    }

    pub fn get_config(&mut self, k: u16) -> Result<ConfigValue> {
        self.expect(Request::GetConfig(k), |r| if let Response::Config(v) = r { Some(v) } else { None })
    }

    pub fn set_config(&mut self, v: ConfigValue) -> Result<ConfigValue> {
        self.expect(Request::SetConfig(v), |r| if let Response::Config(v) = r { Some(v) } else { None })
    }

    /// Every setting this firmware knows (an older one refuses the newer keys, which are left out).
    pub fn all_config(&mut self) -> Result<Vec<ConfigValue>> {
        let mut all = Vec::new();
        for k in key::ALL {
            match self.get_config(*k) {
                Ok(v) => all.push(v),
                Err(Error::Device(ErrorCode::InvalidValue)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(all)
    }

    /// Turns switching or configuration from software back on (`v`: `SwSwitchAllowed(true)` / `SwConfigAllowed(true)`).
    /// The KVM then waits for a press on its key: `waiting` is called once if it does. Returns once the key confirmed it.
    pub fn allow_software(&mut self, v: ConfigValue, waiting: impl FnOnce()) -> Result<()> {
        let flag = match v {
            ConfigValue::SwSwitchAllowed(true) => status_flags::SW_SWITCH_ALLOWED,
            ConfigValue::SwConfigAllowed(true) => status_flags::SW_CONFIG_ALLOWED,
            _ => return Err(Error::Protocol(format!("{v:?} doesn't allow anything"))),
        };
        match self.set_config(v) {
            Err(Error::Device(ErrorCode::NeedsKeyPress)) => waiting(),
            r => return r.map(|_| ()),
        }
        let until = Instant::now() + Duration::from_millis(kvm_core::engine::UNLOCK_WAIT_MS as u64 + 1000);
        while Instant::now() < until {
            std::thread::sleep(Duration::from_millis(250));
            if self.status()?.flags & flag != 0 {
                return Ok(());
            }
        }
        Err(Error::NotConfirmed)
    }

    /// Has the KVM learn a hotkey from its keyboards (software.md §4.5), or start over: the result comes as
    /// `Event::HotkeyLearned` within `LEARN_MS`. Only the active PC may; firmware before protocol 4 answers
    /// `Malformed`.
    pub fn start_learning(&mut self) -> Result<()> {
        self.ok(Request::LearnHotkey(true))
    }

    pub fn stop_learning(&mut self) -> Result<()> {
        self.ok(Request::LearnHotkey(false))
    }

    /// Learns a hotkey: `listening` is called once the KVM waits for the keys. Changes no setting.
    pub fn learn_hotkey(&mut self, listening: impl FnOnce()) -> Result<(LearnEnd, Hotkey)> {
        self.start_learning()?;
        listening();
        let until = Instant::now() + Duration::from_millis(kvm_core::engine::LEARN_MS as u64 + 1000);
        while Instant::now() < until {
            for e in self.events(Duration::from_millis(200))? {
                if let Event::HotkeyLearned(end, h) = e {
                    return Ok((end, h));
                }
            }
        }
        let _ = self.stop_learning();
        Err(Error::Timeout)
    }

    pub fn save(&mut self) -> Result<()> {
        self.ok(Request::SaveConfig)
    }

    pub fn factory_reset(&mut self) -> Result<()> {
        self.ok(Request::FactoryReset)
    }

    pub fn identify(&mut self, seconds: u8) -> Result<()> {
        self.ok(Request::Identify(seconds))
    }

    pub fn reboot(&mut self, t: RebootTarget) -> Result<()> {
        self.ok(Request::Reboot(t))
    }

    pub fn snapshot(&mut self) -> Result<Snapshot> {
        let info = self.info()?;
        let status = self.status()?;
        let mut ports = [PortInfo::empty(Port::Keyboard, PortMode::Auto); 3];
        let mut products: [String; 3] = Default::default();
        for p in Port::ALL {
            ports[p.index()] = self.port(*p)?;
            products[p.index()] = self.port_string(*p, StringKind::Product)?;
        }
        let config = self.all_config()?;
        // The module can stop between GetInfo and the host slots (restarting, being updated): the KVM refuses those
        // then. That is no hosts for now, not a failed snapshot, which would leave the window on an old one (first
        // dongle, 2026-11-14: no wireless host offered on the Screen hop page).
        let bt = match info.caps & caps::BLUETOOTH {
            0 => Vec::new(),
            _ => match self.bt_hosts(&config) {
                Err(Error::Device(ErrorCode::Unsupported)) => Vec::new(),
                other => other?,
            },
        };
        Ok(Snapshot { info, status, ports, products, config, bt })
    }

    /// Every Bluetooth host slot, with its name and policy from `config`.
    pub fn bt_hosts(&mut self, config: &[ConfigValue]) -> Result<Vec<BtHostView>> {
        (1..=BT_HOSTS as u8)
            .map(|slot| {
                let info = match self.request(&Request::GetBtHost(slot))? {
                    Response::BtHost(h) => h,
                    other => return Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
                };
                let i = slot as u16 - 1;
                let name = config
                    .iter()
                    .find_map(|v| match v {
                        ConfigValue::BtName(k, n) if *k as u16 == i && !n.as_str().is_empty() => {
                            Some(n.as_str().to_string())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| fl!("host-n", n = slot));
                let policy = config
                    .iter()
                    .find_map(|v| match v {
                        ConfigValue::BtPolicy(k, p) if *k as u16 == i => Some(*p),
                        _ => None,
                    })
                    .unwrap_or(BtPolicy::StayConnected);
                Ok(BtHostView { slot, name, info, policy })
            })
            .collect()
    }

    /// Opens (or closes) Bluetooth pairing on the KVM.
    pub fn bt_pair(&mut self, on: bool) -> Result<()> {
        self.ok(Request::BtPair(on))
    }

    /// Forgets the bond in slot 1–4, or every bond (0xFF).
    pub fn bt_forget(&mut self, slot: u8) -> Result<()> {
        self.ok(Request::BtForget(slot))
    }

    /// The passkey a pairing host shows, typed here instead of on the KVM's keyboard.
    pub fn bt_passkey(&mut self, value: u32) -> Result<()> {
        self.ok(Request::BtPasskey(value))
    }

    /// Tells the KVM where this computer's cursor is, in its own pixels (screen hop takes it over while it rests).
    pub fn pointer_at(&mut self, at: [i16; 2]) -> Result<()> {
        self.ok(Request::PointerAt(at))
    }

    fn fw(&mut self, req: Request) -> Result<(FwState, u32)> {
        loop {
            match self.request(&req) {
                Ok(Response::Fw { state, progress }) => return Ok((state, progress)),
                Ok(other) => return Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
                Err(Error::Device(ErrorCode::Busy)) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => return Err(e),
            }
        }
    }

    /// Uploads an RP2354B image; `progress(done, total)` is called as it goes, and once more while the KVM waits
    /// for its key to be pressed. Returns when the image is installed (the KVM then reboots).
    pub fn update_firmware(&mut self, image: &[u8], mut progress: impl FnMut(FwState, usize, usize)) -> Result<()> {
        let total = image.len();
        self.fw(Request::FwBegin { target: FwTarget::Rp2354, size: total as u32, crc32: crc32(image) })?;
        for (i, chunk) in image.chunks(FW_CHUNK).enumerate() {
            let data = heapless::Vec::from_slice(chunk).map_err(|_| Error::Protocol("chunk".into()))?;
            self.fw(Request::FwData { offset: (i * FW_CHUNK) as u32, data })?;
            progress(FwState::Receiving, (i * FW_CHUNK + chunk.len()).min(total), total);
        }
        let (mut state, _) = self.fw(Request::FwCommit)?;
        let started = Instant::now();
        while matches!(state, FwState::AwaitingKey | FwState::Writing | FwState::Receiving) {
            progress(state, total, total);
            if started.elapsed() > Duration::from_secs(120) {
                return Err(Error::Timeout);
            }
            std::thread::sleep(Duration::from_millis(250));
            state = self.fw(Request::FwStatus)?.0;
        }
        progress(state, total, total);
        match state {
            FwState::Done => Ok(()),
            s => Err(Error::Protocol(fl!("protocol-update-ended", state = format!("{s:?}")))),
        }
    }
}
