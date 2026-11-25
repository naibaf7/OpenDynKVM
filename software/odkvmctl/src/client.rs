//! Request/response client over a [`Transport`], with typed helpers.

use crate::time::{Duration, Instant};
use std::collections::VecDeque;
use std::fmt;

use kvm_proto::crc::crc32;
use kvm_proto::ctrl::{
    Accel, AxisCal, BT_DEVICES, BT_HOSTS, BtDeviceInfo, BtFound, BtHostInfo, BtPointer, BtPolicy, ConfigValue,
    Controls, DEVICES, Device, DeviceMsg, ErrorCode, Event, FW_CHUNK, FieldOverride, FwState, FwTarget, HOSTS, Host,
    Hotkey, Info, KEYCODES_PER, LearnEnd, MACRO_PIECE, MAX_ARRANGEMENTS, MAX_PROFILES, MAX_SCREENS, Mapping,
    PROTOCOL_VERSION, Pc, Port, PortInfo, PortMode, PortState, RebootTarget, Request, Response, Routing, Screen,
    Status, StringKind, Target, Units, caps, diag, key, status_flags,
};
use kvm_proto::link::bt::{device_flags, host_flags};

use crate::fl;
use crate::transport::{Link, Transport};

/// How long a firmware request is retried while the KVM answers Busy.
const FW_BUSY: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Transport(String),
    Timeout,
    /// The KVM refused the request.
    Device(ErrorCode),
    Protocol(String),
    /// The KVM waited for its key, and nobody pressed it.
    NotConfirmed,
    /// The KVM's firmware speaks another protocol than these programs (its identity): only its firmware can be updated.
    Mismatch(Info),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "{e}"),
            Error::Timeout => f.write_str(&fl!("error-timeout")),
            Error::Device(c) => f.write_str(&fl!("error-device", why = crate::names::error(*c))),
            Error::Protocol(e) => f.write_str(&fl!("error-protocol", error = e.as_str())),
            Error::NotConfirmed => f.write_str(&fl!("error-not-confirmed")),
            Error::Mismatch(info) => f.write_str(&fl!(
                "error-mismatch",
                firmware = crate::names::version(info.rp_fw),
                app = env!("CARGO_PKG_VERSION")
            )),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// An answer of the wrong kind.
fn unexpected(r: Response) -> Error {
    Error::Protocol(fl!("protocol-unexpected", answer = format!("{r:?}")))
}

pub struct Client {
    t: Box<dyn Transport>,
    next_id: u8,
    events: VecDeque<Event>,
    pub timeout: Duration,
    /// Requests in a row that got no answer in time ([`Client::lost`]).
    missed: u8,
}

/// Over the wireless link, answers missed in a row before the KVM counts as gone. The module drops an answer it has no
/// room for (the events of a switch came between): one missed answer is no lost link. Until 0.1.53 it was, and the
/// window let go of the KVM on a switch (2026-11-24).
const MISSED_LOST: u8 = 3;

/// A request that only reads: asked again, it does nothing twice.
fn only_reads(req: &Request) -> bool {
    matches!(
        req,
        Request::GetInfo
            | Request::GetStatus
            | Request::GetPort(_)
            | Request::GetPortString(..)
            | Request::GetConfig(_)
            | Request::GetDiag(_)
            | Request::GetBtHost(_)
            | Request::GetBtDevice(_)
            | Request::GetBtFound(_)
            | Request::GetDeviceMap { .. }
            | Request::GetModuleLog(_)
            | Request::GetKeycodes { .. }
            | Request::GetAxisRules { .. }
            | Request::GetControls(_)
            | Request::GetCalibration(..)
            | Request::GetMapEntry { .. }
            | Request::GetMacros(_)
            | Request::GetMapSetting(_)
            | Request::GetOverride(..)
            | Request::GetOutputs(_)
    )
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
    /// The Bluetooth device slots 1–4, the same way.
    pub devices: Vec<BtDeviceInfo>,
    /// The devices found while the KVM looks for devices to pair (`Status.bt_found` of them).
    pub found: Vec<BtFound>,
}

/// What a Bluetooth device slot holds, from its flags.
pub trait DeviceSlot {
    fn bonded(&self) -> bool;
    fn connected(&self) -> bool;
    /// Connected and reporting, and the KVM holds its report map: its input reaches the KVM.
    fn ready(&self) -> bool;
    /// The KVM can't parse its report map: connected or not, its input goes nowhere.
    fn unreadable(&self) -> bool;
}

impl DeviceSlot for BtDeviceInfo {
    fn bonded(&self) -> bool {
        self.flags & device_flags::BONDED != 0
    }

    fn connected(&self) -> bool {
        self.flags & device_flags::CONNECTED != 0
    }

    fn ready(&self) -> bool {
        let need = device_flags::READY | device_flags::MAP_HELD;
        self.flags & need == need
    }

    fn unreadable(&self) -> bool {
        self.flags & device_flags::MAP_UNREADABLE != 0
    }
}

/// One Bluetooth host slot as the UIs show it: the module's view plus the KVM's settings for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtHostView {
    pub slot: u8,
    /// The name set for it, or "BT n".
    pub name: String,
    pub info: BtHostInfo,
    pub policy: BtPolicy,
    /// It may control the KVM over the module's control service (`ConfigValue::BtControl`).
    pub may_control: bool,
}

impl BtHostView {
    pub fn bonded(&self) -> bool {
        self.info.flags & host_flags::BONDED != 0
    }

    pub fn connected(&self) -> bool {
        self.info.flags & host_flags::CONNECTED != 0
    }

    /// Its host software talks to the KVM over the module right now (a control session).
    pub fn in_session(&self) -> bool {
        self.info.flags & host_flags::CONTROL != 0
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
    /// Every device's mode and routing, in [`Device`] order.
    pub modes: [PortMode; DEVICES],
    pub routes: [Routing; DEVICES],
    /// The arrangement screen hop uses in it; None: screen hop is off there.
    pub arrangement: Option<u8>,
    /// The keymap its normalized devices go through (hid_mapping.md); None: none.
    pub keymap: Option<u8>,
}

/// Screen hop's settings and arrangements (software.md §4.14).
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

    /// Bluetooth device slot `n`, if the KVM has the module.
    pub fn device(&self, n: u8) -> Option<&BtDeviceInfo> {
        self.devices.iter().find(|d| d.slot == n)
    }

    /// The devices the UIs list for modes and routings: the three front ports, and each Bluetooth device slot that
    /// holds a bond (a free slot is nothing to set).
    pub fn routed_devices(&self) -> Vec<Device> {
        Device::ALL
            .iter()
            .copied()
            .filter(|d| d.wireless().is_none_or(|n| self.device(n).is_some_and(|i| i.bonded())))
            .collect()
    }

    /// Screen hop's settings and arrangements.
    pub fn hop(&self) -> HopView {
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
        v
    }

    /// The PC this computer is (the one the KVM answered), if it says: None also over the wireless module.
    pub fn this_pc(&self) -> Option<Pc> {
        self.status.asking_pc()
    }

    /// The wireless host slot (1–4) this computer is to the KVM, when it reaches it over the module.
    pub fn this_host_slot(&self) -> Option<u8> {
        self.status.asking_bt()
    }

    /// The active profile and every stored one (the first `count` in use); None from a snapshot without the config.
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
                    _ => [PortMode::Auto; DEVICES],
                };
                let routes = match find(key::PROFILE_ROUTES_BASE + i) {
                    Some(ConfigValue::ProfileRoutes(_, r)) => *r,
                    _ => [Routing::Follow; DEVICES],
                };
                let arrangement = match find(key::PROFILE_ARRANGEMENT_BASE + i) {
                    Some(ConfigValue::ProfileArrangement(_, a)) => *a,
                    _ => None,
                };
                let keymap = match find(key::PROFILE_KEYMAP_BASE + i) {
                    Some(ConfigValue::ProfileKeymap(_, k)) => *k,
                    _ => None,
                };
                ProfileView { name, modes, routes, arrangement, keymap }
            })
            .collect();
        Some((*active, *count, list))
    }
}

#[maybe_async::maybe_async]
impl Client {
    pub fn new(t: Box<dyn Transport>) -> Self {
        // Every program that has the control interface open reads every answer (hidraw on Linux): a window and
        // `odkvmctl` running together each start numbering somewhere else, and an answer counts only with both its id
        // and its request type (`request`). Both counting from 1, `odkvmctl update` once took the window's
        // `GetConfig` answer for its own and gave up (2026-11-12).
        let seed =
            crate::time::SystemTime::now().duration_since(crate::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        // A browser has no process ids (and runs one app).
        #[cfg(not(target_arch = "wasm32"))]
        let pid = std::process::id();
        #[cfg(target_arch = "wasm32")]
        let pid = 0u32;
        let next_id = ((seed ^ pid.rotate_left(8)) % 255) as u8 + 1;
        Self { t, next_id, events: VecDeque::new(), timeout: Duration::from_millis(1000), missed: 0 }
    }

    /// The first KVM found, or the simulator.
    pub fn connect(sim: bool) -> Result<Self> {
        if sim {
            return Ok(Self::new(Box::new(crate::sim::SimTransport::new())));
        }
        #[cfg(not(target_arch = "wasm32"))]
        return Ok(Self::new(crate::transport::open_first().map_err(Error::Transport)?));
        // In a browser there is only the simulator.
        #[cfg(target_arch = "wasm32")]
        Err(Error::Transport(crate::fl!("no-kvm-found")))
    }

    pub fn describe(&self) -> String {
        self.t.describe()
    }

    /// With the simulator: where to send what its keyboard types (`Transport::simulated_keyboard`).
    pub fn simulated_keyboard(&self) -> Option<std::sync::mpsc::Sender<[u8; 8]>> {
        self.t.simulated_keyboard()
    }

    /// How this client reaches the KVM.
    pub fn link(&self) -> Link {
        self.t.link()
    }

    fn take_id(&mut self) -> u8 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        id
    }

    /// The KVM is gone, by what a request ended with: the link failed, or (over USB, which loses nothing) the KVM didn't
    /// answer. Over the wireless link only after [`MISSED_LOST`] answers missed in a row: the module drops one it has no
    /// room for.
    pub fn lost(&self, e: &Error) -> bool {
        match e {
            Error::Transport(_) => true,
            Error::Timeout => self.t.link() != Link::Bluetooth || self.missed >= MISSED_LOST,
            _ => false,
        }
    }

    /// Over the wireless link, a read whose answer didn't come is asked once more.
    fn asks_again(&self, req: &Request) -> bool {
        self.t.link() == Link::Bluetooth && only_reads(req)
    }

    /// Several requests, their answers in the same order, with up to the transport's window under way at once
    /// (`Transport::window`): over Bluetooth each takes two connection events, and a window's refresh one after another
    /// would take seconds. The first refusal ends it.
    pub async fn request_all(&mut self, reqs: &[Request]) -> Result<Vec<Response>> {
        let window = self.t.window().max(1);
        let mut out: Vec<Option<Response>> = (0..reqs.len()).map(|_| None).collect();
        // Under way: which request, its id and type, when it is late, and whether it was asked again.
        let mut waiting: VecDeque<(usize, u8, u8, Instant, bool)> = VecDeque::new();
        let mut next = 0;
        while next < reqs.len() || !waiting.is_empty() {
            while waiting.len() < window && next < reqs.len() {
                let id = self.take_id();
                let raw = reqs[next].encode(id).map_err(|e| Error::Protocol(format!("{e:?}")))?;
                self.t.write(&raw).await.map_err(Error::Transport)?;
                waiting.push_back((next, id, reqs[next].type_byte(), Instant::now() + self.timeout, false));
                next += 1;
            }
            let Some(&(i, _, _, late, again)) = waiting.front() else { break };
            let left = late.saturating_duration_since(Instant::now());
            let r = if left.is_zero() { None } else { self.t.read(left).await.map_err(Error::Transport)? };
            let Some(r) = r else {
                // The first one's answer didn't come.
                self.missed = self.missed.saturating_add(1);
                if again || !self.asks_again(&reqs[i]) {
                    return Err(Error::Timeout);
                }
                waiting.pop_front();
                let id = self.take_id();
                let raw = reqs[i].encode(id).map_err(|e| Error::Protocol(format!("{e:?}")))?;
                self.t.write(&raw).await.map_err(Error::Transport)?;
                waiting.push_back((i, id, reqs[i].type_byte(), Instant::now() + self.timeout, true));
                continue;
            };
            match DeviceMsg::decode(&r) {
                Ok(DeviceMsg::Event(Event::PlaceCursor(_))) => {}
                Ok(DeviceMsg::Event(e)) => self.events.push_back(e),
                Ok(DeviceMsg::Response { id, request, response }) => {
                    let Some(at) = waiting.iter().position(|w| w.1 == id && w.2 == request) else { continue };
                    let Some((i, ..)) = waiting.remove(at) else { continue };
                    self.missed = 0;
                    match response {
                        Response::Error(code) => return Err(Error::Device(code)),
                        r => out[i] = Some(r),
                    }
                }
                _ => {}
            }
        }
        out.into_iter().map(|r| r.ok_or(Error::Timeout)).collect()
    }

    pub async fn request(&mut self, req: &Request) -> Result<Response> {
        match self.request_once(req).await {
            Err(Error::Timeout) if self.asks_again(req) => self.request_once(req).await,
            r => r,
        }
    }

    async fn request_once(&mut self, req: &Request) -> Result<Response> {
        let id = self.take_id();
        let raw = req.encode(id).map_err(|e| Error::Protocol(format!("{e:?}")))?;
        self.t.write(&raw).await.map_err(Error::Transport)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let r = if left.is_zero() { None } else { self.t.read(left).await.map_err(Error::Transport)? };
            let Some(r) = r else {
                self.missed = self.missed.saturating_add(1);
                return Err(Error::Timeout);
            };
            match DeviceMsg::decode(&r) {
                // Screen hop's placements are the placer's (`crate::placer`), on its own handle.
                Ok(DeviceMsg::Event(Event::PlaceCursor(_))) => {}
                Ok(DeviceMsg::Event(e)) => self.events.push_back(e),
                Ok(DeviceMsg::Response { id: rid, request, response }) if rid == id && request == req.type_byte() => {
                    self.missed = 0;
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
    pub async fn events(&mut self, timeout: Duration) -> Result<Vec<Event>> {
        let mut wait = timeout;
        while let Some(r) = self.t.read(wait).await.map_err(Error::Transport)? {
            if let Ok(DeviceMsg::Event(e)) = DeviceMsg::decode(&r)
                && !matches!(e, Event::PlaceCursor(_))
            {
                self.events.push_back(e);
            }
            wait = Duration::ZERO;
        }
        Ok(self.events.drain(..).collect())
    }

    async fn expect<T>(&mut self, req: Request, f: impl FnOnce(Response) -> Option<T>) -> Result<T> {
        let r = self.request(&req).await?;
        let what = format!("{r:?}");
        f(r).ok_or_else(|| Error::Protocol(fl!("protocol-unexpected", answer = what)))
    }

    async fn ok(&mut self, req: Request) -> Result<()> {
        self.expect(req, |r| matches!(r, Response::Ok).then_some(())).await
    }

    pub async fn info(&mut self) -> Result<Info> {
        self.expect(Request::GetInfo, |r| if let Response::Info(i) = r { Some(i) } else { None }).await
    }

    pub async fn status(&mut self) -> Result<Status> {
        self.expect(Request::GetStatus, |r| if let Response::Status(s) = r { Some(s) } else { None }).await
    }

    /// Every diagnostics item, in the firmware's order. Pages
    /// fetched while the list changes may repeat an item: the later reading counts.
    pub async fn diag(&mut self) -> Result<Vec<diag::Item>> {
        let mut items: Vec<diag::Item> = Vec::new();
        let mut page = 0u8;
        loop {
            let (pages, got) = self.expect(Request::GetDiag(page), |r| match r {
                Response::Diag { pages, items, .. } => Some((pages, items)),
                _ => None,
            }).await?;
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

    pub async fn port(&mut self, p: Port) -> Result<PortInfo> {
        self.expect(Request::GetPort(p), |r| if let Response::Port(i) = r { Some(i) } else { None }).await
    }

    /// The product string of a port's device ("" when the port is empty).
    pub async fn port_string(&mut self, p: Port, kind: StringKind) -> Result<String> {
        match self.request(&Request::GetPortString(p, kind)).await {
            Ok(Response::PortString(_, _, s)) => Ok(s.to_string()),
            Err(Error::Device(ErrorCode::NoDevice)) => Ok(String::new()),
            Ok(other) => Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
            Err(e) => Err(e),
        }
    }

    pub async fn switch(&mut self, t: Target) -> Result<()> {
        self.ok(Request::SetActive(t)).await
    }

    /// The active profile's mode of `d`.
    pub async fn set_mode(&mut self, d: Device, m: PortMode) -> Result<()> {
        self.set_config(ConfigValue::Mode(d, m)).await.map(|_| ())
    }

    /// The active profile's routing of `d`.
    pub async fn set_route(&mut self, d: Device, r: Routing) -> Result<()> {
        self.set_config(ConfigValue::Route(d, r)).await.map(|_| ())
    }

    pub async fn redetect(&mut self, p: Port) -> Result<()> {
        self.ok(Request::Redetect(p)).await
    }

    pub async fn get_config(&mut self, k: u16) -> Result<ConfigValue> {
        self.expect(Request::GetConfig(k), |r| if let Response::Config(v) = r { Some(v) } else { None }).await
    }

    pub async fn set_config(&mut self, v: ConfigValue) -> Result<ConfigValue> {
        self.expect(Request::SetConfig(v), |r| if let Response::Config(v) = r { Some(v) } else { None }).await
    }

    /// Every setting.
    pub async fn all_config(&mut self) -> Result<Vec<ConfigValue>> {
        let reqs: Vec<Request> = key::ALL.iter().map(|k| Request::GetConfig(*k)).collect();
        let answers = self.request_all(&reqs).await?;
        answers.into_iter().map(|r| if let Response::Config(v) = r { Ok(v) } else { Err(unexpected(r)) }).collect()
    }

    /// Turns switching or configuration from software back on (`v`: `SwSwitchAllowed(true)` / `SwConfigAllowed(true)`).
    /// The KVM then waits for a press on its key: `waiting` is called once if it does. Returns once the key confirmed it.
    pub async fn allow_software(&mut self, v: ConfigValue, waiting: impl FnOnce()) -> Result<()> {
        let flag = match v {
            ConfigValue::SwSwitchAllowed(true) => status_flags::SW_SWITCH_ALLOWED,
            ConfigValue::SwConfigAllowed(true) => status_flags::SW_CONFIG_ALLOWED,
            _ => return Err(Error::Protocol(format!("{v:?} doesn't allow anything"))),
        };
        match self.set_config(v).await {
            Err(Error::Device(ErrorCode::NeedsKeyPress)) => waiting(),
            r => return r.map(|_| ()),
        }
        let until = Instant::now() + Duration::from_millis(kvm_core::engine::UNLOCK_WAIT_MS as u64 + 1000);
        while Instant::now() < until {
            crate::time::pause(Duration::from_millis(250)).await;
            if self.status().await?.flags & flag != 0 {
                return Ok(());
            }
        }
        Err(Error::NotConfirmed)
    }

    /// From a computer that reaches the KVM over its wireless module (software.md §8.1 "Over Bluetooth"): asks to
    /// control it. `Ok(true)`: it may already; `Ok(false)`: the KVM waits for a tap on its key.
    pub async fn request_control(&mut self) -> Result<bool> {
        match self.ok(Request::AskControl).await {
            Ok(()) => Ok(true),
            Err(Error::Device(ErrorCode::NeedsKeyPress)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// [`Client::request_control`], then waits for the tap: `waiting` is called once the KVM waits for it. Returns once
    /// this computer may control the KVM.
    pub async fn ask_control(&mut self, waiting: impl FnOnce()) -> Result<()> {
        if self.request_control().await? {
            return Ok(());
        }
        waiting();
        let until = Instant::now() + Duration::from_millis(kvm_core::engine::UNLOCK_WAIT_MS as u64 + 1000);
        while Instant::now() < until {
            crate::time::pause(Duration::from_millis(250)).await;
            // Asking again would wait anew: the status answers once it may.
            match self.status().await {
                Ok(_) => return Ok(()),
                Err(Error::Device(ErrorCode::NotAllowed)) => {}
                Err(e) => return Err(e),
            }
        }
        Err(Error::NotConfirmed)
    }

    /// From a PC: lets wireless host slot 1–4 control the KVM over the module (`allow`), confirmed by a tap on the
    /// KVM's key (`waiting` is called once it waits for it), or not any more (at once).
    pub async fn allow_host(&mut self, slot: u8, allow: bool, waiting: impl FnOnce()) -> Result<()> {
        let i = slot.checked_sub(1).filter(|i| (*i as usize) < BT_HOSTS).ok_or(Error::Device(ErrorCode::InvalidValue))?;
        match self.set_config(ConfigValue::BtControl(i, allow)).await {
            Err(Error::Device(ErrorCode::NeedsKeyPress)) => waiting(),
            r => return r.map(|_| ()),
        }
        let until = Instant::now() + Duration::from_millis(kvm_core::engine::UNLOCK_WAIT_MS as u64 + 1000);
        while Instant::now() < until {
            crate::time::pause(Duration::from_millis(250)).await;
            if self.get_config(key::BT_CONTROL_BASE + i as u16).await? == ConfigValue::BtControl(i, true) {
                return Ok(());
            }
        }
        Err(Error::NotConfirmed)
    }

    /// Has the KVM learn a hotkey from its keyboards (software.md §4.5), or start over: the result comes as
    /// `Event::HotkeyLearned` within `LEARN_MS`. Only the active PC may.
    pub async fn start_learning(&mut self) -> Result<()> {
        self.ok(Request::LearnHotkey(true)).await
    }

    pub async fn stop_learning(&mut self) -> Result<()> {
        self.ok(Request::LearnHotkey(false)).await
    }

    /// Learns a hotkey: `listening` is called once the KVM waits for the keys. Changes no setting.
    pub async fn learn_hotkey(&mut self, listening: impl FnOnce()) -> Result<(LearnEnd, Hotkey)> {
        self.start_learning().await?;
        listening();
        let until = Instant::now() + Duration::from_millis(kvm_core::engine::LEARN_MS as u64 + 1000);
        while Instant::now() < until {
            for e in self.events(Duration::from_millis(200)).await? {
                if let Event::HotkeyLearned(end, h) = e {
                    return Ok((end, h));
                }
            }
        }
        let _ = self.stop_learning().await;
        Err(Error::Timeout)
    }

    pub async fn save(&mut self) -> Result<()> {
        self.ok(Request::SaveConfig).await
    }

    pub async fn factory_reset(&mut self) -> Result<()> {
        self.ok(Request::FactoryReset).await
    }

    pub async fn identify(&mut self, seconds: u8) -> Result<()> {
        self.ok(Request::Identify(seconds)).await
    }

    pub async fn reboot(&mut self, t: RebootTarget) -> Result<()> {
        self.ok(Request::Reboot(t)).await
    }

    /// The KVM's identity, if its firmware speaks these programs' protocol ([`Error::Mismatch`] otherwise: then only
    /// the firmware update works).
    pub async fn info_checked(&mut self) -> Result<Info> {
        let info = self.info().await?;
        if info.proto != PROTOCOL_VERSION {
            return Err(Error::Mismatch(info));
        }
        Ok(info)
    }

    pub async fn snapshot(&mut self) -> Result<Snapshot> {
        self.snapshot_keeping(None).await
    }

    /// A snapshot that takes `config` (the last snapshot's) instead of reading every setting again: the settings
    /// are most of a snapshot's requests, and they change only with a `ConfigChanged` event or this program's own
    /// requests.
    pub async fn snapshot_keeping(&mut self, config: Option<Vec<ConfigValue>>) -> Result<Snapshot> {
        let info = self.info_checked().await?;
        let status = self.status().await?;
        let mut ports = [PortInfo::empty(Port::Keyboard, PortMode::Auto); 3];
        let mut products: [String; 3] = Default::default();
        let reqs: Vec<Request> = Port::ALL.iter().map(|p| Request::GetPort(*p)).collect();
        for r in self.request_all(&reqs).await? {
            match r {
                Response::Port(i) => ports[i.port.index()] = i,
                other => return Err(unexpected(other)),
            }
        }
        for p in Port::ALL {
            products[p.index()] = self.port_string(*p, StringKind::Product).await?;
        }
        let config = match config {
            Some(c) => c,
            None => self.all_config().await?,
        };
        // The module can stop between GetInfo and the host slots (restarting, being updated): the KVM refuses those
        // then. That is no hosts for now, not a failed snapshot, which would leave the window on an old one (first
        // dongle, 2026-11-14: no wireless host offered on the Screen hop page).
        let bt = match info.caps & caps::BLUETOOTH {
            0 => Vec::new(),
            _ => match self.bt_hosts(&config).await {
                Err(Error::Device(ErrorCode::Unsupported)) => Vec::new(),
                other => other?,
            },
        };
        let (devices, found) = match info.caps & caps::BLUETOOTH {
            0 => (Vec::new(), Vec::new()),
            _ => match self.bt_devices().await {
                Err(Error::Device(ErrorCode::Unsupported)) => (Vec::new(), Vec::new()),
                Err(e) => return Err(e),
                Ok(d) => match self.bt_found(status.bt_found).await {
                    Err(Error::Device(ErrorCode::Unsupported)) => (Vec::new(), Vec::new()),
                    Err(e) => return Err(e),
                    Ok(f) => (d, f),
                },
            },
        };
        Ok(Snapshot { info, status, ports, products, config, bt, devices, found })
    }

    /// Every Bluetooth device slot.
    pub async fn bt_devices(&mut self) -> Result<Vec<BtDeviceInfo>> {
        let reqs: Vec<Request> = (1..=BT_DEVICES as u8).map(Request::GetBtDevice).collect();
        let answers = self.request_all(&reqs).await?;
        answers.into_iter().map(|r| if let Response::BtDevice(d) = r { Ok(d) } else { Err(unexpected(r)) }).collect()
    }

    /// The wireless module's recent log as the KVM keeps it (empty without one), piece by piece: lines that came while
    /// it was read are left for the next time.
    pub async fn module_log(&mut self) -> Result<String> {
        let mut out = Vec::new();
        let mut at = 0u32;
        let mut until = None;
        loop {
            let (first, end, from, data) = self.expect(Request::GetModuleLog(at), |r| {
                if let Response::ModuleLog { first, end, at, data } = r { Some((first, end, at, data)) } else { None }
            }).await?;
            if from != at && !out.is_empty() {
                // Lines made room while it was read: start over from the first kept.
                out.clear();
                at = first;
                continue;
            }
            let until = *until.get_or_insert(end);
            out.extend_from_slice(&data);
            at = from + data.len() as u32;
            if data.is_empty() || at >= until {
                break;
            }
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// The report descriptor of device `d`'s `index`-th HID interface (a Bluetooth device's report map, index 0),
    /// piece by piece: its interface number, how many HID interfaces the device has, and the bytes (empty while the
    /// KVM holds none).
    pub async fn device_map(&mut self, d: Device, index: u8) -> Result<(u8, u8, Vec<u8>)> {
        let mut map = Vec::new();
        loop {
            let req = Request::GetDeviceMap { device: d, index, offset: map.len() as u16 };
            let (iface, interfaces, total, data) = self.expect(req, |r| match r {
                Response::DeviceMap { iface, interfaces, total, data, .. } => Some((iface, interfaces, total, data)),
                _ => None,
            }).await?;
            if data.is_empty() || map.len() + data.len() > total as usize {
                // Done, or the map changed under the reading (another device): read what is there now.
                if map.len() == total as usize {
                    return Ok((iface, interfaces, map));
                }
                map.clear();
                if total == 0 {
                    return Ok((iface, interfaces, map));
                }
                continue;
            }
            map.extend_from_slice(&data);
        }
    }

    /// Every HID interface of device `d`: its number and report descriptor.
    pub async fn device_maps(&mut self, d: Device) -> Result<Vec<(u8, Vec<u8>)>> {
        let (iface, n, map) = self.device_map(d, 0).await?;
        let mut out = vec![(iface, map)];
        for i in 1..n {
            let (iface, _, map) = self.device_map(d, i).await?;
            out.push((iface, map));
        }
        out.retain(|(_, m)| !m.is_empty());
        Ok(out)
    }

    /// The first `count` devices found while looking for devices to pair (fewer if the list shrank meanwhile).
    pub async fn bt_found(&mut self, count: u8) -> Result<Vec<BtFound>> {
        let reqs: Vec<Request> = (0..count).map(Request::GetBtFound).collect();
        let mut out = Vec::new();
        for r in self.request_all(&reqs).await? {
            match r {
                Response::BtFound(Some(f)) => out.push(f),
                Response::BtFound(None) => break,
                other => return Err(unexpected(other)),
            }
        }
        Ok(out)
    }

    /// Starts (or stops) looking for Bluetooth devices to pair.
    pub async fn bt_scan(&mut self, on: bool) -> Result<()> {
        self.ok(Request::BtScan(on)).await
    }

    /// Pairs the device found at `addr` into a free device slot.
    pub async fn bt_pair_device(&mut self, addr: [u8; 7]) -> Result<()> {
        self.ok(Request::BtPairDevice(addr)).await
    }

    /// Forgets device slot 1–4, or every one (0xFF).
    pub async fn bt_forget_device(&mut self, slot: u8) -> Result<()> {
        self.ok(Request::BtForgetDevice(slot)).await
    }

    /// Every Bluetooth host slot, with its name and policy from `config`.
    pub async fn bt_hosts(&mut self, config: &[ConfigValue]) -> Result<Vec<BtHostView>> {
        let reqs: Vec<Request> = (1..=BT_HOSTS as u8).map(Request::GetBtHost).collect();
        let answers = self.request_all(&reqs).await?;
        (1..=BT_HOSTS as u8)
            .zip(answers)
            .map(|(slot, r)| {
                let info = match r {
                    Response::BtHost(h) => h,
                    other => return Err(unexpected(other)),
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
                let may_control = config.contains(&ConfigValue::BtControl(i as u8, true));
                Ok(BtHostView { slot, name, info, policy, may_control })
            })
            .collect()
    }

    /// Opens (or closes) Bluetooth pairing on the KVM.
    pub async fn bt_pair(&mut self, on: bool) -> Result<()> {
        self.ok(Request::BtPair(on)).await
    }

    /// Forgets the bond in slot 1–4, or every bond (0xFF).
    pub async fn bt_forget(&mut self, slot: u8) -> Result<()> {
        self.ok(Request::BtForget(slot)).await
    }

    /// Erases every pairing the wireless module holds, hosts and devices, and restarts it.
    pub async fn bt_erase_all(&mut self) -> Result<()> {
        self.ok(Request::BtEraseAll).await
    }

    /// Makes the ESP32 board on `port` the wireless module, replacing its firmware (`Request::AdoptModule`): it then
    /// waits in its ROM for the module's firmware.
    pub async fn adopt_module(&mut self, port: Port) -> Result<()> {
        self.ok(Request::AdoptModule(port)).await
    }

    /// The passkey a pairing host shows, typed here instead of on the KVM's keyboard.
    pub async fn bt_passkey(&mut self, value: u32) -> Result<()> {
        self.ok(Request::BtPasskey(value)).await
    }

    /// Tells the KVM where this computer's cursor is, in its own pixels (screen hop takes it over while it rests).
    pub async fn pointer_at(&mut self, at: [i16; 2]) -> Result<()> {
        self.ok(Request::PointerAt(at)).await
    }

    /// A firmware request, retried while the KVM answers Busy (a sector being written, the module's stream full, the
    /// module being put into its ROM loader), for up to [`FW_BUSY`].
    async fn fw(&mut self, req: Request) -> Result<(FwState, u32)> {
        let started = Instant::now();
        loop {
            match self.request(&req).await {
                Ok(Response::Fw { state, progress }) => return Ok((state, progress)),
                Ok(other) => return Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
                Err(Error::Device(ErrorCode::Busy)) if started.elapsed() < FW_BUSY => {
                    crate::time::pause(Duration::from_millis(5)).await
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// `FwBegin`, the image in `FwData` chunks, `FwCommit`: `progress(Receiving, done, total)` as it goes. Returns the
    /// state after the commit.
    async fn send_image(
        &mut self,
        target: FwTarget,
        image: &[u8],
        progress: &mut impl FnMut(FwState, usize, usize),
    ) -> Result<FwState> {
        let total = image.len();
        self.fw(Request::FwBegin { target, size: total as u32, crc32: crc32(image) }).await?;
        for (i, chunk) in image.chunks(FW_CHUNK).enumerate() {
            let data = heapless::Vec::from_slice(chunk).map_err(|_| Error::Protocol("chunk".into()))?;
            self.fw(Request::FwData { offset: (i * FW_CHUNK) as u32, data }).await?;
            progress(FwState::Receiving, (i * FW_CHUNK + chunk.len()).min(total), total);
        }
        Ok(self.fw(Request::FwCommit).await?.0)
    }

    /// Polls the update's state until it ends (Done or Failed), at most `limit`.
    async fn wait_update(
        &mut self,
        mut state: FwState,
        total: usize,
        limit: Duration,
        progress: &mut impl FnMut(FwState, usize, usize),
    ) -> Result<()> {
        let started = Instant::now();
        while matches!(state, FwState::AwaitingKey | FwState::Writing | FwState::Receiving) {
            progress(state, total, total);
            if started.elapsed() > limit {
                return Err(Error::Timeout);
            }
            crate::time::pause(Duration::from_millis(250)).await;
            state = self.fw(Request::FwStatus).await?.0;
        }
        progress(state, total, total);
        match state {
            FwState::Done => Ok(()),
            s => Err(Error::Protocol(fl!("protocol-update-ended", state = format!("{s:?}")))),
        }
    }

    // ── keymaps (hid_mapping.md) ─────────────────────────────────────────────

    /// Every keycode of keymap 0–3, layer 0–7, by position (`kvm_map::pos`).
    pub async fn keycodes(&mut self, keymap: u8, layer: u8) -> Result<Vec<u16>> {
        let mut out = Vec::with_capacity(kvm_map::POSITIONS);
        while out.len() < kvm_map::POSITIONS {
            let first = out.len() as u16;
            match self.request(&Request::GetKeycodes { keymap, layer, first }).await? {
                Response::Keycodes { codes, .. } if !codes.is_empty() => out.extend_from_slice(&codes),
                other => return Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
            }
        }
        Ok(out)
    }

    /// Sets keycodes of keymap 0–3, layer 0–7, from position `first`.
    pub async fn set_keycodes(&mut self, keymap: u8, layer: u8, first: u16, codes: &[u16]) -> Result<()> {
        for (i, chunk) in codes.chunks(KEYCODES_PER).enumerate() {
            let first = first + (i * KEYCODES_PER) as u16;
            let codes = heapless::Vec::from_slice(chunk).map_err(|_| Error::Protocol("chunk".into()))?;
            self.ok(Request::SetKeycodes { keymap, layer, first, codes }).await?;
        }
        Ok(())
    }

    /// Every axis rule of keymap 0–3, layer 0–7 (`kvm_map::axes::input` order).
    pub async fn axis_rules(&mut self, keymap: u8, layer: u8) -> Result<Vec<kvm_map::axes::Rule>> {
        let mut out = Vec::with_capacity(kvm_map::axes::INPUTS);
        while out.len() < kvm_map::axes::INPUTS {
            let first = out.len() as u8;
            match self.request(&Request::GetAxisRules { keymap, layer, first }).await? {
                Response::AxisRules { rules, .. } if !rules.is_empty() => {
                    for r in rules {
                        out.push(kvm_map::axes::Rule::decode(&r).unwrap_or(kvm_map::axes::Rule::TRANSPARENT));
                    }
                }
                other => return Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
            }
        }
        Ok(out)
    }

    pub async fn set_axis_rule(&mut self, keymap: u8, layer: u8, input: u8, rule: kvm_map::axes::Rule) -> Result<()> {
        self.ok(Request::SetAxisRule { keymap, layer, input, rule: rule.encode() }).await
    }

    /// Watches the controls of the normalized devices (`Event::Control`, `Event::Axis`): 0 stop, 1 watch, 2 watch and
    /// keep them from the hosts meanwhile.
    pub async fn watch_controls(&mut self, mode: u8) -> Result<()> {
        self.ok(Request::WatchControls(mode)).await
    }

    /// What the device in slot `d` has.
    pub async fn controls(&mut self, d: Device) -> Result<Controls> {
        match self.request(&Request::GetControls(d)).await? {
            Response::Controls(_, c) => Ok(c),
            other => Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
        }
    }

    pub async fn calibration(&mut self, d: Device, axis: u8) -> Result<AxisCal> {
        match self.request(&Request::GetCalibration(d, axis)).await? {
            Response::Calibration(_, _, c) => Ok(c),
            other => Err(Error::Protocol(fl!("protocol-unexpected", answer = format!("{other:?}")))),
        }
    }

    pub async fn set_calibration(&mut self, d: Device, axis: u8, c: AxisCal) -> Result<()> {
        self.ok(Request::SetCalibration(d, axis, c)).await
    }

    /// Entry `index` of one kind of the keymaps' shared entries, in Vial's layout (`kvm_map::keymap::Dynamic`).
    pub async fn map_entry(&mut self, kind: kvm_map::keymap::Dynamic, index: u8) -> Result<Vec<u8>> {
        self.expect(Request::GetMapEntry { kind: kind as u8, index }, |r| match r {
            Response::MapEntry { data, .. } => Some(data.to_vec()),
            _ => None,
        }).await
    }

    pub async fn set_map_entry(&mut self, kind: kvm_map::keymap::Dynamic, index: u8, data: &[u8]) -> Result<()> {
        let data = heapless::Vec::from_slice(data).map_err(|_| Error::Protocol("entry".into()))?;
        self.ok(Request::SetMapEntry { kind: kind as u8, index, data }).await
    }

    /// The whole macro buffer (`kvm_map::macros`).
    pub async fn macros(&mut self) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        loop {
            let (total, data) = self.expect(Request::GetMacros(out.len() as u16), |r| match r {
                Response::Macros { total, data, .. } => Some((total, data)),
                _ => None,
            }).await?;
            out.extend_from_slice(&data);
            if data.is_empty() || out.len() >= total as usize {
                return Ok(out);
            }
        }
    }

    /// Writes the macro buffer where it differs from `before`.
    pub async fn set_macros(&mut self, before: &[u8], after: &[u8]) -> Result<()> {
        for (i, chunk) in after.chunks(MACRO_PIECE).enumerate() {
            let at = i * MACRO_PIECE;
            if before.get(at..at + chunk.len()) == Some(chunk) {
                continue;
            }
            let data = heapless::Vec::from_slice(chunk).map_err(|_| Error::Protocol("macros".into()))?;
            self.ok(Request::SetMacros { offset: at as u16, data }).await?;
        }
        Ok(())
    }

    /// A QMK setting of the keymaps by Vial's id (`kvm_map::keymap::qsid`).
    pub async fn map_setting(&mut self, qsid: u16) -> Result<u32> {
        self.expect(Request::GetMapSetting(qsid), |r| match r {
            Response::MapSetting(_, v) => Some(v),
            _ => None,
        }).await
    }

    pub async fn set_map_setting(&mut self, qsid: u16, value: u32) -> Result<()> {
        self.ok(Request::SetMapSetting(qsid, value)).await
    }

    /// Streams device `d`'s raw reports (`Event::Report`) for 10 s; ask again within that to go on, `false` to stop.
    pub async fn monitor(&mut self, d: Device, on: bool) -> Result<()> {
        self.ok(Request::MonitorDevice(d, on)).await
    }

    pub async fn field_override(&mut self, d: Device, n: u8) -> Result<Option<FieldOverride>> {
        self.expect(Request::GetOverride(d, n), |r| match r {
            Response::Override(_, _, o) => Some(o),
            _ => None,
        }).await
    }

    pub async fn set_field_override(&mut self, d: Device, n: u8, o: Option<FieldOverride>) -> Result<()> {
        self.ok(Request::SetOverride(d, n, o)).await
    }

    /// What host `h` is offered and took (`ctrl::outputs` bits).
    pub async fn outputs(&mut self, h: Host) -> Result<(u8, u8)> {
        self.expect(Request::GetOutputs(h), |r| match r {
            Response::Outputs { offered, taken, .. } => Some((offered, taken)),
            _ => None,
        }).await
    }

    /// Uploads an RP2354B image; `progress(state, done, total)` is called as it goes, and once more while the KVM waits
    /// for its key to be pressed. Returns when the image is installed (the KVM then reboots).
    pub async fn update_firmware(&mut self, image: &[u8], mut progress: impl FnMut(FwState, usize, usize)) -> Result<()> {
        let state = self.send_image(FwTarget::Rp2354, image, &mut progress).await?;
        self.wait_update(state, image.len(), Duration::from_secs(120), &mut progress).await
    }

    /// Streams the Bluetooth module's firmware (`kvm-module-<chip>.bin`) through the KVM to its module, which needs no
    /// key: the KVM takes only the images its own firmware was built with. `progress` as for [`Client::update_firmware`]; the
    /// data goes to the module as it comes, so `Receiving` is the module being written. Returns once the module's flash
    /// checks out (it then restarts into it, `Status.bt_module`).
    pub async fn update_module(&mut self, image: &[u8], mut progress: impl FnMut(FwState, usize, usize)) -> Result<()> {
        let state = self.send_image(FwTarget::Module, image, &mut progress).await?;
        self.wait_update(state, image.len(), Duration::from_secs(60), &mut progress).await
    }
}
