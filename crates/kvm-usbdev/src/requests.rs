//! Control-request dispatcher (USB 2.0 §9.4, HID 1.11 §7.2) over a descriptor set. No I/O: the device loop executes
//! the returned [`Action`].

use heapless::Vec;
use kvm_proto::descset::{DescSet, iface as ifl, tag};

pub mod std_req {
    pub const GET_STATUS: u8 = 0x00;
    pub const CLEAR_FEATURE: u8 = 0x01;
    pub const SET_FEATURE: u8 = 0x03;
    pub const SET_ADDRESS: u8 = 0x05;
    pub const GET_DESCRIPTOR: u8 = 0x06;
    pub const GET_CONFIGURATION: u8 = 0x08;
    pub const SET_CONFIGURATION: u8 = 0x09;
    pub const GET_INTERFACE: u8 = 0x0A;
    pub const SET_INTERFACE: u8 = 0x0B;
}

pub mod hid_req {
    pub const GET_REPORT: u8 = 0x01;
    pub const GET_IDLE: u8 = 0x02;
    pub const GET_PROTOCOL: u8 = 0x03;
    pub const SET_REPORT: u8 = 0x09;
    pub const SET_IDLE: u8 = 0x0A;
    pub const SET_PROTOCOL: u8 = 0x0B;
}

mod dt {
    pub const DEVICE: u8 = 1;
    pub const CONFIGURATION: u8 = 2;
    pub const STRING: u8 = 3;
    pub const INTERFACE: u8 = 4;
    pub const ENDPOINT: u8 = 5;
    pub const DEVICE_QUALIFIER: u8 = 6;
    pub const OTHER_SPEED_CONFIGURATION: u8 = 7;
    pub const BOS: u8 = 0x0F;
    pub const HID: u8 = 0x21;
    pub const HID_REPORT: u8 = 0x22;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Setup {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

impl Setup {
    pub fn parse(b: &[u8; 8]) -> Self {
        Self {
            request_type: b[0],
            request: b[1],
            value: u16::from_le_bytes([b[2], b[3]]),
            index: u16::from_le_bytes([b[4], b[5]]),
            length: u16::from_le_bytes([b[6], b[7]]),
        }
    }

    pub fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }

    /// 0 standard, 1 class, 2 vendor.
    pub fn kind(&self) -> u8 {
        (self.request_type >> 5) & 3
    }

    /// 0 device, 1 interface, 2 endpoint, 3 other.
    pub fn recipient(&self) -> u8 {
        self.request_type & 0x1F
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Action {
    /// IN data stage: the first `n` bytes of the buffer (the loop truncates to `wLength`).
    In(usize),
    /// Status stage only.
    Accept,
    Stall,
    SetAddress(u8),
    /// SET_CONFIGURATION (0 = back to the address state).
    Configure(u8),
    /// Read the OUT data stage, then call [`out_data`].
    ReadOut,
    /// Send to the RP2354B (with the OUT data stage, if any) and relay its answer.
    Forward,
    Halt {
        ep: u8,
        halted: bool,
    },
}

/// Something the RP2354B should hear about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Notify {
    SetReport { iface: u8, report_type: u8, report_id: u8 },
    Protocol { iface: u8, protocol: u8 },
}

const CACHE: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct DeviceState {
    pub address: u8,
    pub configuration: u8,
    pub remote_wakeup: bool,
    idle: Vec<(u8, u8), CACHE>,
    protocol: Vec<(u8, u8), CACHE>,
    reports: Vec<(u8, u8, u8, Vec<u8, 8>), CACHE>,
    /// Halted endpoints: bit n = IN n, bit 8 + n = OUT n.
    halted: u16,
    pub notify: Option<Notify>,
}

fn put<K: PartialEq + Copy, V, const N: usize>(v: &mut Vec<(K, V), N>, k: K, val: V) {
    if let Some(e) = v.iter_mut().find(|e| e.0 == k) {
        e.1 = val;
    } else {
        if v.is_full() {
            v.remove(0);
        }
        let _ = v.push((k, val));
    }
}

impl DeviceState {
    /// USB reset: back to the default state; HID interfaces revert to report protocol.
    pub fn reset(&mut self) {
        *self = DeviceState::default();
    }

    pub fn protocol(&self, iface: u8) -> u8 {
        self.protocol.iter().find(|(i, _)| *i == iface).map_or(1, |(_, p)| *p)
    }

    fn ep_bit(ep: u8) -> u16 {
        let n = (ep & 0x0F) as u16;
        if ep & 0x80 != 0 { 1 << n } else { 1 << (8 + n) }
    }
}

fn copy(buf: &mut [u8], data: &[u8]) -> Action {
    let n = data.len().min(buf.len());
    buf[..n].copy_from_slice(&data[..n]);
    Action::In(n)
}

/// Walks a configuration descriptor's descriptors.
fn descriptors(raw: &[u8]) -> impl Iterator<Item = (usize, &[u8])> {
    let mut pos = 0;
    core::iter::from_fn(move || {
        let len = *raw.get(pos)? as usize;
        if len < 2 || pos + len > raw.len() {
            return None;
        }
        let at = pos;
        pos += len;
        Some((at, &raw[at..at + len]))
    })
}

/// Endpoint addresses of the configuration.
pub fn config_endpoints(config: &[u8]) -> impl Iterator<Item = u8> + '_ {
    descriptors(config).filter(|(_, d)| d[1] == dt::ENDPOINT && d.len() >= 7).map(|(_, d)| d[2])
}

/// The configuration descriptor as seen at the link's speed (`other` = the OTHER_SPEED variant). The set carries
/// high-speed values; at full speed, interrupt intervals are converted to milliseconds.
pub fn config_for(set: &DescSet, high_speed: bool, other: bool, buf: &mut [u8]) -> Option<usize> {
    let cfg = set.config()?;
    let n = cfg.len();
    if n > buf.len() {
        return None;
    }
    buf[..n].copy_from_slice(cfg);
    if other {
        buf[1] = dt::OTHER_SPEED_CONFIGURATION;
    }
    if high_speed == other {
        // Presenting full-speed values.
        let eps: Vec<usize, 32> =
            descriptors(&buf[..n]).filter(|(_, d)| d[1] == dt::ENDPOINT && d.len() >= 7).map(|(at, _)| at).collect();
        for at in eps {
            let d = &mut buf[at..at + 7];
            if d[3] & 3 == 3 && d[6] > 0 {
                let microframes = 1u32 << (d[6].min(16) - 1);
                d[6] = (microframes / 8).clamp(1, 255) as u8;
            }
            let mps = (u16::from_le_bytes([d[4], d[5]]) & 0x7FF).min(64);
            d[4..6].copy_from_slice(&mps.to_le_bytes());
        }
    }
    Some(n)
}

/// The HID class descriptor (type 0x21) that follows interface `iface` in the configuration.
fn hid_descriptor(config: &[u8], iface: u8) -> Option<&[u8]> {
    let mut in_iface = false;
    for (_, d) in descriptors(config) {
        match d[1] {
            dt::INTERFACE if d.len() >= 9 => in_iface = d[2] == iface && d[3] == 0,
            dt::HID if in_iface => return Some(d),
            _ => {}
        }
    }
    None
}

pub fn dispatch(s: &Setup, set: Option<&DescSet>, st: &mut DeviceState, high_speed: bool, buf: &mut [u8]) -> Action {
    let Some(set) = set else { return Action::Stall };
    match s.kind() {
        0 => standard(s, set, st, high_speed, buf),
        1 => class(s, set, st, buf),
        2 => vendor(s, set, buf),
        _ => Action::Stall,
    }
}

fn standard(s: &Setup, set: &DescSet, st: &mut DeviceState, high_speed: bool, buf: &mut [u8]) -> Action {
    use std_req::*;
    let recipient = s.recipient();
    match (s.request, s.is_in()) {
        (GET_DESCRIPTOR, true) => {
            let (ty, index) = ((s.value >> 8) as u8, s.value as u8);
            match (recipient, ty) {
                (0, dt::DEVICE) => set.device().map_or(Action::Stall, |d| copy(buf, d)),
                (0, dt::CONFIGURATION) => config_for(set, high_speed, false, buf).map_or(Action::Stall, Action::In),
                (0, dt::OTHER_SPEED_CONFIGURATION) => {
                    config_for(set, high_speed, true, buf).map_or(Action::Stall, Action::In)
                }
                (0, dt::DEVICE_QUALIFIER) => match set.device() {
                    Some(d) => copy(buf, &[10, dt::DEVICE_QUALIFIER, d[2], d[3], d[4], d[5], d[6], d[7], d[17], 0]),
                    None => Action::Stall,
                },
                (0, dt::STRING) => set.find(tag::STRING, index).map_or(Action::Stall, |d| copy(buf, d)),
                (0, dt::BOS) => set.find(tag::BOS, 0).map_or(Action::Stall, |d| copy(buf, d)),
                (1, dt::HID_REPORT) => set.find(tag::HID_REPORT, s.index as u8).map_or(Action::Stall, |d| copy(buf, d)),
                (1, dt::HID) => {
                    set.config().and_then(|c| hid_descriptor(c, s.index as u8)).map_or(Action::Stall, |d| copy(buf, d))
                }
                _ => Action::Stall,
            }
        }
        (GET_STATUS, true) => {
            let status: u16 = match recipient {
                0 => (st.remote_wakeup as u16) << 1,
                1 => 0,
                2 => (st.halted & DeviceState::ep_bit(s.index as u8) != 0) as u16,
                _ => return Action::Stall,
            };
            copy(buf, &status.to_le_bytes())
        }
        (CLEAR_FEATURE | SET_FEATURE, false) => {
            let on = s.request == SET_FEATURE;
            match (recipient, s.value) {
                (0, 1) => {
                    st.remote_wakeup = on;
                    Action::Accept
                }
                (2, 0) => {
                    let ep = s.index as u8;
                    if on {
                        st.halted |= DeviceState::ep_bit(ep);
                    } else {
                        st.halted &= !DeviceState::ep_bit(ep);
                    }
                    Action::Halt { ep, halted: on }
                }
                _ => Action::Stall,
            }
        }
        (SET_ADDRESS, false) => {
            st.address = s.value as u8 & 0x7F;
            Action::SetAddress(st.address)
        }
        (GET_CONFIGURATION, true) => copy(buf, &[st.configuration]),
        (SET_CONFIGURATION, false) => {
            let v = s.value as u8;
            let ours = set.config().and_then(|c| c.get(5).copied());
            if v == 0 || Some(v) == ours {
                st.configuration = v;
                st.halted = 0;
                Action::Configure(v)
            } else {
                Action::Stall
            }
        }
        (GET_INTERFACE, true) => copy(buf, &[0]),
        (SET_INTERFACE, false) if s.value == 0 => Action::Accept,
        _ => Action::Stall,
    }
}

fn class(s: &Setup, set: &DescSet, st: &mut DeviceState, buf: &mut [u8]) -> Action {
    use hid_req::*;
    if s.recipient() != 1 {
        return Action::Stall;
    }
    let iface = s.index as u8;
    let flags = set.iface_flags(iface);
    if flags & ifl::FORWARD != 0 {
        return Action::Forward;
    }
    if flags & ifl::LOCAL_HID == 0 {
        return Action::Stall;
    }
    let (hi, lo) = ((s.value >> 8) as u8, s.value as u8);
    match (s.request, s.is_in()) {
        (GET_REPORT, true) => {
            let len = (s.length as usize).min(64).min(buf.len());
            buf[..len].fill(0);
            if let Some((.., data)) = st.reports.iter().find(|(i, t, id, _)| *i == iface && *t == hi && *id == lo) {
                let n = data.len().min(len);
                buf[..n].copy_from_slice(&data[..n]);
            } else if lo != 0 && len > 0 {
                // No cached value: an idle report (nothing pressed) with its report ID.
                buf[0] = lo;
            }
            Action::In(len)
        }
        (SET_REPORT, false) => Action::ReadOut,
        (GET_IDLE, true) => copy(buf, &[st.idle.iter().find(|(i, _)| *i == iface).map_or(0, |(_, v)| *v)]),
        (SET_IDLE, false) => {
            put(&mut st.idle, iface, hi);
            Action::Accept
        }
        (GET_PROTOCOL, true) => copy(buf, &[st.protocol(iface)]),
        (SET_PROTOCOL, false) => {
            let protocol = (s.value & 1) as u8;
            put(&mut st.protocol, iface, protocol);
            if flags & (ifl::BOOT_KEYBOARD | ifl::BOOT_MOUSE) != 0 {
                st.notify = Some(Notify::Protocol { iface, protocol });
            }
            Action::Accept
        }
        _ => Action::Stall,
    }
}

fn vendor(s: &Setup, set: &DescSet, buf: &mut [u8]) -> Action {
    if s.is_in() {
        if let Some(r) = set.find_tag(tag::MSOS10_COMPAT)
            && s.request == r.arg
            && s.index == 4
        {
            return copy(buf, r.data);
        }
        if let Some(r) = set.find_tag(tag::MSOS20)
            && s.request == r.arg
            && s.index == 7
        {
            return copy(buf, r.data);
        }
    }
    let forwarded = match s.recipient() {
        1 => set.iface_flags(s.index as u8) & ifl::FORWARD != 0,
        0 => set.records().any(|r| r.tag == tag::IFACE && r.data.first().is_some_and(|f| f & ifl::FORWARD != 0)),
        _ => false,
    };
    if forwarded { Action::Forward } else { Action::Stall }
}

/// Completes an [`Action::ReadOut`] (SET_REPORT on a local HID interface).
pub fn out_data(s: &Setup, st: &mut DeviceState, data: &[u8]) -> Action {
    if s.kind() == 1 && s.request == hid_req::SET_REPORT {
        let (iface, report_type, report_id) = (s.index as u8, (s.value >> 8) as u8, s.value as u8);
        let stored = Vec::from_slice(&data[..data.len().min(8)]).unwrap_or_default();
        match st.reports.iter_mut().find(|(i, t, id, _)| *i == iface && *t == report_type && *id == report_id) {
            Some(e) => e.3 = stored,
            None => {
                if st.reports.is_full() {
                    st.reports.remove(0);
                }
                let _ = st.reports.push((iface, report_type, report_id, stored));
            }
        }
        st.notify = Some(Notify::SetReport { iface, report_type, report_id });
        Action::Accept
    } else {
        Action::Stall
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvm_core::presentation::{MirrorSource, Params, build};
    use kvm_core::usbdesc::parse_config;
    use kvm_proto::ctrl::{Polling, Port};

    /// Two HID interfaces: 0 with EP 0x81, 1 with EP 0x82 IN and 0x02 OUT.
    #[rustfmt::skip]
    const GAMING_MOUSE_CONFIG: &[u8] = &[
        9, 2, 66, 0, 2, 1, 0, 0xA0, 49,
        9, 4, 0, 0, 1, 3, 1, 2, 0, 9, 0x21, 0x11, 0x01, 0, 1, 0x22, 67, 0, 7, 5, 0x81, 3, 8, 0, 1,
        9, 4, 1, 0, 2, 3, 0, 0, 0, 9, 0x21, 0x11, 0x01, 0, 1, 0x22, 151, 0, 7, 5, 0x82, 3, 20, 0, 1, 7, 5, 0x02, 3, 20, 0, 1,
    ];
    /// Offset of the first endpoint's bInterval: configuration, interface, HID, then endpoint byte 6.
    const EP1_INTERVAL: usize = 9 + 9 + 9 + 6;

    fn set_buf(mirrored: bool) -> std::vec::Vec<u8> {
        let mut out = [0u8; 4096];
        let cfg = parse_config(GAMING_MOUSE_CONFIG).unwrap();
        let rd: &[u8] = &[0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0xC0];
        let mirrors = [MirrorSource { port: Port::Mouse, iface: &cfg.interfaces[1], report_desc: rd }];
        let p = Params {
            polling: Polling::Us125,
            serial: "S1",
            mirrors: if mirrored { &mirrors } else { &[] },
            identity: None,
            vial: false,
            normalized: true,
            mouse_abs: false,
        };
        let (n, _) = build(&p, &mut out).unwrap();
        out[..n].to_vec()
    }

    fn setup(rt: u8, req: u8, value: u16, index: u16, length: u16) -> Setup {
        Setup { request_type: rt, request: req, value, index, length }
    }

    fn run(set: &[u8], s: Setup, st: &mut DeviceState, hs: bool) -> (Action, std::vec::Vec<u8>) {
        let set = DescSet::parse(set).unwrap();
        let mut buf = [0u8; 1024];
        let a = dispatch(&s, Some(&set), st, hs, &mut buf);
        let data = match a {
            Action::In(n) => buf[..n].to_vec(),
            _ => std::vec::Vec::new(),
        };
        (a, data)
    }

    #[test]
    fn standard_descriptors() {
        let set = set_buf(false);
        let mut st = DeviceState::default();
        let (_, dev) = run(&set, setup(0x80, 6, 0x0100, 0, 18), &mut st, true);
        assert_eq!((dev.len(), dev[1]), (18, 1));
        let (_, q) = run(&set, setup(0x80, 6, 0x0600, 0, 10), &mut st, true);
        assert_eq!(&q[..2], &[10, 6]);
        let (_, cfg) = run(&set, setup(0x80, 6, 0x0200, 0, 0xFF), &mut st, true);
        assert_eq!(cfg[1], 2);
        assert_eq!(cfg[EP1_INTERVAL], 1);
        // Full-speed link: interval 1 ms, other-speed variant carries the HS value.
        let (_, fs) = run(&set, setup(0x80, 6, 0x0200, 0, 0xFF), &mut st, false);
        assert_eq!(fs[EP1_INTERVAL], 1);
        let (_, other) = run(&set, setup(0x80, 6, 0x0700, 0, 0xFF), &mut st, false);
        assert_eq!((other[1], other[EP1_INTERVAL]), (7, 1));
        let (_, s) = run(&set, setup(0x80, 6, 0x0304, 0x0409, 255), &mut st, true);
        assert_eq!(s[1], 3);
        assert_eq!(run(&set, setup(0x80, 6, 0x0F00, 0, 5), &mut st, true).0, Action::Stall);
        let (_, rd) = run(&set, setup(0x81, 6, 0x2200, 1, 200), &mut st, true);
        assert_eq!(rd.as_slice(), kvm_hid::normalized::MOUSE_DESC);
        let (_, hid) = run(&set, setup(0x81, 6, 0x2100, 3, 9), &mut st, true);
        assert_eq!((hid[1], hid[7] as usize), (0x21, kvm_hid::normalized::CONTROL_DESC.len()));
    }

    #[test]
    fn address_configuration_features() {
        let set = set_buf(false);
        let mut st = DeviceState::default();
        assert_eq!(run(&set, setup(0x00, 5, 7, 0, 0), &mut st, true).0, Action::SetAddress(7));
        assert_eq!(run(&set, setup(0x00, 9, 2, 0, 0), &mut st, true).0, Action::Stall);
        assert_eq!(run(&set, setup(0x00, 9, 1, 0, 0), &mut st, true).0, Action::Configure(1));
        assert_eq!(run(&set, setup(0x80, 8, 0, 0, 1), &mut st, true).1, [1]);
        assert_eq!(run(&set, setup(0x00, 3, 1, 0, 0), &mut st, true).0, Action::Accept);
        assert_eq!(run(&set, setup(0x80, 0, 0, 0, 2), &mut st, true).1, [2, 0]);
        assert_eq!(run(&set, setup(0x02, 3, 0, 0x81, 0), &mut st, true).0, Action::Halt { ep: 0x81, halted: true });
        assert_eq!(run(&set, setup(0x82, 0, 0, 0x81, 2), &mut st, true).1, [1, 0]);
        let eps: std::vec::Vec<u8> = config_endpoints(DescSet::parse(&set).unwrap().config().unwrap()).collect();
        assert_eq!(eps, [0x81, 0x82, 0x83, 0x84, 0x04]);
    }

    #[test]
    fn local_hid_requests() {
        let set = set_buf(false);
        let mut st = DeviceState::default();
        assert_eq!(run(&set, setup(0xA1, 3, 0, 0, 1), &mut st, true).1, [1], "report protocol by default");
        assert_eq!(run(&set, setup(0x21, 0x0B, 0, 0, 0), &mut st, true).0, Action::Accept);
        assert_eq!(st.notify.take(), Some(Notify::Protocol { iface: 0, protocol: 0 }));
        assert_eq!(run(&set, setup(0xA1, 3, 0, 0, 1), &mut st, true).1, [0]);
        assert_eq!(run(&set, setup(0x21, 0x0A, 0x7D00, 0, 0), &mut st, true).0, Action::Accept);
        assert_eq!(run(&set, setup(0xA1, 2, 0, 0, 1), &mut st, true).1, [0x7D]);
        // SET_REPORT (LEDs) is read, cached and reported.
        let s = setup(0x21, 9, 0x0200, 0, 1);
        assert_eq!(run(&set, s, &mut st, true).0, Action::ReadOut);
        assert_eq!(out_data(&s, &mut st, &[0x02]), Action::Accept);
        assert_eq!(st.notify, Some(Notify::SetReport { iface: 0, report_type: 2, report_id: 0 }));
        assert_eq!(run(&set, setup(0xA1, 1, 0x0200, 0, 1), &mut st, true).1, [0x02]);
        // GET_REPORT(input) of an NKRO report: idle report with its ID.
        let (_, r) = run(&set, setup(0xA1, 1, 0x0101, 2, 30), &mut st, true);
        assert_eq!((r.len(), r[0], r[1]), (30, 1, 0));
        st.reset();
        assert_eq!(run(&set, setup(0xA1, 3, 0, 0, 1), &mut st, true).1, [1]);
    }

    #[test]
    fn mirrored_interfaces_are_forwarded() {
        let set = set_buf(true);
        let mut st = DeviceState::default();
        assert_eq!(run(&set, setup(0x21, 9, 0x0300, 4, 20), &mut st, true).0, Action::Forward);
        assert_eq!(run(&set, setup(0xA1, 1, 0x0310, 4, 20), &mut st, true).0, Action::Forward);
        assert_eq!(run(&set, setup(0xC0, 0x42, 0, 0, 4), &mut st, true).0, Action::Forward);
        assert_eq!(run(&set_buf(false), setup(0xC0, 0x42, 0, 0, 4), &mut st, true).0, Action::Stall);
        assert_eq!(run(&set, setup(0x21, 9, 0x0300, 9, 20), &mut st, true).0, Action::Stall);
    }
}
