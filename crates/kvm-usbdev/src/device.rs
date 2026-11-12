//! Runs the request dispatcher over an `embassy-usb-driver` implementation.
//!
//! All 7 IN and 7 OUT endpoints are allocated once as 64-byte interrupt endpoints (software.md §5.2), so a new
//! descriptor set only needs a soft disconnect, never a new driver.

use embassy_futures::select::{Either3, select3};
use embassy_usb_driver::{
    Bus, ControlPipe, Direction, Driver, EndpointAddress, EndpointAllocError, EndpointType, Event,
};
use heapless::Vec;
use kvm_proto::descset::{self, DescSet};
use kvm_proto::link::msg::UsbState;

use crate::requests::{self, Action, DeviceState, Notify, Setup, config_endpoints};

pub const ENDPOINTS: usize = 7;
pub const EP_MPS: u16 = 64;
const CONTROL_BUF: usize = 1024;
/// How long D+ stays released when the descriptor set changes, so the hub reports a disconnect.
pub const RECONNECT_MS: u32 = 50;

pub struct Endpoints<'d, D: Driver<'d>> {
    /// EP1–EP7 IN; index 0 is EP1.
    pub ins: Vec<D::EndpointIn, ENDPOINTS>,
    /// EP1–EP7 OUT.
    pub outs: Vec<D::EndpointOut, ENDPOINTS>,
}

/// What the RP2354B asks of the USB core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Cmd {
    /// A new descriptor set is committed; fetch it with [`Host::load_set`].
    NewSet,
    Attach,
    Detach,
    RemoteWakeup,
}

/// The firmware side of the USB core.
pub trait Host {
    async fn command(&mut self) -> Cmd;
    /// Copies the committed descriptor set into `buf`; returns its length.
    fn load_set(&mut self, buf: &mut [u8]) -> usize;
    /// Drives the D+ pull-up (DCTL.SDIS on the AT32F405's OTGHS).
    fn soft_connect(&mut self, connected: bool);
    /// The bus came up at high speed (from the controller's enumeration status).
    fn is_high_speed(&self) -> bool;
    async fn delay_ms(&mut self, ms: u32);
    /// Runs a control request on the RP2354B side; `None` = stall. `reply` receives the IN data stage.
    async fn forward(&mut self, setup: [u8; 8], data: &[u8], reply: &mut [u8]) -> Option<usize>;
    fn set_report(&mut self, iface: u8, report_type: u8, report_id: u8, data: &[u8]);
    fn protocol_changed(&mut self, iface: u8, protocol: u8);
    fn state_changed(&mut self, state: UsbState);
    /// The PC turned remote wakeup on or off (SET/CLEAR_FEATURE DEVICE_REMOTE_WAKEUP; a bus reset turns it off).
    fn wake_enabled_changed(&mut self, enabled: bool);
}

pub struct UsbCore<'d, D: Driver<'d>> {
    bus: D::Bus,
    pipe: D::ControlPipe,
    st: DeviceState,
    set: [u8; descset::MAX_LEN],
    set_len: usize,
    buf: [u8; CONTROL_BUF],
    reply: [u8; CONTROL_BUF],
    started: bool,
    connected: bool,
    suspended: bool,
    state: UsbState,
    /// Remote wakeup as last told to the host side.
    wake_enabled: bool,
}

/// Allocates EP1–EP7 in both directions.
pub fn alloc_endpoints<'d, D: Driver<'d>>(d: &mut D) -> Result<Endpoints<'d, D>, EndpointAllocError> {
    let mut eps = Endpoints { ins: Vec::new(), outs: Vec::new() };
    for n in 1..=ENDPOINTS {
        let ep = d.alloc_endpoint_in(
            EndpointType::Interrupt,
            Some(EndpointAddress::from_parts(n, Direction::In)),
            EP_MPS,
            1,
        )?;
        let _ = eps.ins.push(ep);
        let ep = d.alloc_endpoint_out(
            EndpointType::Interrupt,
            Some(EndpointAddress::from_parts(n, Direction::Out)),
            EP_MPS,
            1,
        )?;
        let _ = eps.outs.push(ep);
    }
    Ok(eps)
}

impl<'d, D: Driver<'d>> UsbCore<'d, D> {
    pub fn new(mut driver: D) -> Result<(Self, Endpoints<'d, D>), EndpointAllocError> {
        let eps = alloc_endpoints(&mut driver)?;
        let (bus, pipe) = driver.start(64);
        let core = Self {
            bus,
            pipe,
            st: DeviceState::default(),
            set: [0; descset::MAX_LEN],
            set_len: 0,
            buf: [0; CONTROL_BUF],
            reply: [0; CONTROL_BUF],
            started: false,
            connected: false,
            suspended: false,
            state: UsbState::Detached,
            wake_enabled: false,
        };
        Ok((core, eps))
    }

    fn set_state(&mut self, s: UsbState, host: &mut impl Host) {
        if self.state != s {
            self.state = s;
            host.state_changed(s);
        }
        self.sync_wake(host);
    }

    fn sync_wake(&mut self, host: &mut impl Host) {
        if self.wake_enabled != self.st.remote_wakeup {
            self.wake_enabled = self.st.remote_wakeup;
            host.wake_enabled_changed(self.wake_enabled);
        }
    }

    pub async fn run(&mut self, host: &mut impl Host) -> ! {
        loop {
            if !self.started {
                // No descriptor set yet: stay off the bus (the controller connects on its first poll).
                let cmd = host.command().await;
                self.command(cmd, host).await;
                continue;
            }
            let Self { bus, pipe, .. } = self;
            match select3(bus.poll(), pipe.setup(), host.command()).await {
                Either3::First(ev) => self.bus_event(ev, host).await,
                Either3::Second(setup) => self.control(setup, host).await,
                Either3::Third(cmd) => self.command(cmd, host).await,
            }
        }
    }

    async fn command(&mut self, cmd: Cmd, host: &mut impl Host) {
        match cmd {
            Cmd::NewSet => {
                if self.connected {
                    host.soft_connect(false);
                    self.connected = false;
                    self.set_state(UsbState::Detached, host);
                    host.delay_ms(RECONNECT_MS).await;
                }
                self.set_len = host.load_set(&mut self.set);
                if DescSet::parse(&self.set[..self.set_len]).is_ok() {
                    self.st.reset();
                    self.sync_wake(host);
                    if self.started {
                        host.soft_connect(true);
                    }
                    self.started = true;
                    self.connected = true;
                }
            }
            Cmd::Detach => {
                if self.connected {
                    host.soft_connect(false);
                    self.connected = false;
                    self.set_state(UsbState::Detached, host);
                }
            }
            Cmd::Attach => {
                if !self.connected && self.started {
                    host.soft_connect(true);
                    self.connected = true;
                }
            }
            Cmd::RemoteWakeup => {
                if self.suspended && self.st.remote_wakeup {
                    let _ = self.bus.remote_wakeup().await;
                }
            }
        }
    }

    async fn bus_event(&mut self, ev: Event, host: &mut impl Host) {
        match ev {
            Event::PowerDetected => {
                self.bus.enable().await;
                self.set_state(UsbState::Powered, host);
            }
            Event::PowerRemoved => {
                self.bus.disable().await;
                self.set_state(UsbState::Unpowered, host);
            }
            Event::Reset => {
                self.st.reset();
                self.suspended = false;
                self.configure_endpoints(0);
                self.set_state(UsbState::Default, host);
            }
            Event::Suspend => {
                self.suspended = true;
                self.set_state(UsbState::Suspended, host);
            }
            Event::Resume => {
                self.suspended = false;
                let s = match (self.st.configuration, self.st.address) {
                    (c, _) if c != 0 => UsbState::Configured,
                    (_, a) if a != 0 => UsbState::Addressed,
                    _ => UsbState::Default,
                };
                self.set_state(s, host);
            }
        }
    }

    /// Enables exactly the endpoints of the configuration (all off for 0).
    fn configure_endpoints(&mut self, value: u8) {
        for n in 1..=ENDPOINTS {
            self.bus.endpoint_set_enabled(EndpointAddress::from_parts(n, Direction::In), false);
            self.bus.endpoint_set_enabled(EndpointAddress::from_parts(n, Direction::Out), false);
        }
        if value == 0 {
            return;
        }
        let Ok(set) = DescSet::parse(&self.set[..self.set_len]) else { return };
        let Some(cfg) = set.config() else { return };
        for addr in config_endpoints(cfg) {
            let n = (addr & 0x0F) as usize;
            if (1..=ENDPOINTS).contains(&n) {
                let dir = if addr & 0x80 != 0 { Direction::In } else { Direction::Out };
                self.bus.endpoint_set_enabled(EndpointAddress::from_parts(n, dir), true);
            }
        }
    }

    async fn send_in(&mut self, n: usize, length: u16, from_reply: bool) {
        let len = n.min(length as usize);
        let mps = self.pipe.max_packet_size();
        // A short answer that is a multiple of the packet size needs a zero-length packet to end it.
        let zlp = len < length as usize && len.is_multiple_of(mps);
        let data = if from_reply { &self.reply[..len] } else { &self.buf[..len] };
        let chunks = data.len().div_ceil(mps) + zlp as usize;
        if chunks == 0 {
            let _ = self.pipe.data_in(&[], true, true).await;
            return;
        }
        for i in 0..chunks {
            let start = (i * mps).min(len);
            let end = ((i + 1) * mps).min(len);
            let data = if from_reply { &self.reply[start..end] } else { &self.buf[start..end] };
            if self.pipe.data_in(data, i == 0, i + 1 == chunks).await.is_err() {
                return;
            }
        }
    }

    async fn read_out(&mut self, length: u16) -> Option<usize> {
        let want = length as usize;
        if want > self.buf.len() {
            return None;
        }
        let mps = self.pipe.max_packet_size();
        let mut total = 0;
        let mut first = true;
        while total < want {
            let end = (total + mps).min(want);
            let last = end == want;
            let n = self.pipe.data_out(&mut self.buf[total..end], first, last).await.ok()?;
            first = false;
            total += n;
            if n < mps {
                break;
            }
        }
        Some(total)
    }

    async fn control(&mut self, raw: [u8; 8], host: &mut impl Host) {
        let s = Setup::parse(&raw);
        let action = {
            let set = DescSet::parse(&self.set[..self.set_len]).ok();
            requests::dispatch(&s, set.as_ref(), &mut self.st, host.is_high_speed(), &mut self.buf)
        };
        match action {
            Action::In(n) => self.send_in(n, s.length, false).await,
            Action::Accept => self.pipe.accept().await,
            Action::Stall => self.pipe.reject().await,
            Action::SetAddress(a) => {
                self.pipe.accept_set_address(a).await;
                self.set_state(UsbState::Addressed, host);
            }
            Action::Configure(v) => {
                self.configure_endpoints(v);
                self.pipe.accept().await;
                self.set_state(if v != 0 { UsbState::Configured } else { UsbState::Addressed }, host);
            }
            Action::Halt { ep, halted } => {
                let dir = if ep & 0x80 != 0 { Direction::In } else { Direction::Out };
                self.bus.endpoint_set_stalled(EndpointAddress::from_parts((ep & 0x0F) as usize, dir), halted);
                self.pipe.accept().await;
            }
            Action::ReadOut => match self.read_out(s.length).await {
                Some(n) => match requests::out_data(&s, &mut self.st, &self.buf[..n]) {
                    Action::Accept => self.pipe.accept().await,
                    _ => self.pipe.reject().await,
                },
                None => self.pipe.reject().await,
            },
            Action::Forward => {
                let out_len = if !s.is_in() && s.length > 0 {
                    match self.read_out(s.length).await {
                        Some(n) => n,
                        None => {
                            self.pipe.reject().await;
                            return;
                        }
                    }
                } else {
                    0
                };
                let answer = host.forward(raw, &self.buf[..out_len], &mut self.reply).await;
                match (answer, s.is_in()) {
                    (Some(n), true) => self.send_in(n, s.length, true).await,
                    (Some(_), false) => self.pipe.accept().await,
                    (None, _) => self.pipe.reject().await,
                }
            }
        }
        match self.st.notify.take() {
            Some(Notify::SetReport { iface, report_type, report_id }) => {
                let n = (s.length as usize).min(self.buf.len());
                host.set_report(iface, report_type, report_id, &self.buf[..n]);
            }
            Some(Notify::Protocol { iface, protocol }) => host.protocol_changed(iface, protocol),
            None => {}
        }
        self.sync_wake(host);
    }
}
