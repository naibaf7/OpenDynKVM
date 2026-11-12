//! What a PC enumerates: builds the descriptor set (software.md §7) and the map from interfaces/endpoints to functions.

use heapless::Vec;
use kvm_hid::normalized;
use kvm_proto::ctrl::{INTERFACE_STRING, Polling, Port, VIAL_INTERFACE_STRING};
use kvm_proto::descset::{self, iface as ifl, tag};
use kvm_proto::wire::WireError;

use crate::usbdesc::{DeviceDesc, InterfaceInfo, dt, encode_string};

/// pid.codes test VID:PID until a PID is allocated (software.md §12).
pub const KVM_VID: u16 = 0x1209;
pub const KVM_PID: u16 = 0x0001;
pub const KVM_BCD: u16 = 0x0100;
pub const MAX_MIRRORS: usize = MIRROR_EPS.len();
/// Microsoft OS 1.0 vendor request code used for the XInput compat ID.
pub const MSOS_VENDOR_CODE: u8 = 0x20;
/// Endpoint numbers mirrors take, in this order: EP5–EP7, then EP1–EP3 where the KVM's own interfaces leave them free
/// (OUT always, IN only without the normalized interfaces, `Params::normalized`).
const MIRROR_EPS: [u8; 6] = [5, 6, 7, 1, 2, 3];
const LAST_EP: u8 = 7;
/// The Vial interface's IN endpoint when present (it has no OUT endpoint: its requests come as SET_REPORT).
const VIAL_EP: u8 = LAST_EP;

pub mod strings {
    pub const MANUFACTURER: u8 = 1;
    pub const PRODUCT: u8 = 2;
    pub const SERIAL: u8 = 3;
    pub const CONTROL: u8 = 4;
    pub const VIAL: u8 = 5;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Function {
    pub iface: u8,
    pub ep_in: u8,
    /// 0 = none.
    pub ep_out: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MirrorMap {
    pub port: Port,
    pub src_iface: u8,
    pub src_ep_in: u8,
    pub src_ep_out: u8,
    pub f: Function,
}

/// Where each function ended up for one PC.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Presentation {
    /// The normalized interfaces have `ep_in` 0 while they are left out (`Params::normalized`).
    pub boot_keyboard: Function,
    pub mouse: Function,
    pub nkro: Function,
    pub control: Function,
    /// The Vial raw HID interface (software.md §4.11), when the config shows it.
    pub vial: Option<Function>,
    /// The mouse interface has the absolute pointer ([`normalized::MOUSE_ABS_DESC`]): its reports carry report IDs.
    pub mouse_abs: bool,
    pub mirrors: Vec<MirrorMap, MAX_MIRRORS>,
    pub identity: Option<Port>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Role {
    BootKeyboard,
    Mouse,
    Nkro,
    Control,
    Vial,
    Mirror(MirrorMap),
}

impl Presentation {
    pub const EMPTY: Presentation = {
        let none = Function { iface: 0, ep_in: 0, ep_out: 0 };
        Presentation {
            boot_keyboard: none,
            mouse: none,
            nkro: none,
            control: none,
            vial: None,
            mouse_abs: false,
            mirrors: Vec::new(),
            identity: None,
        }
    };

    pub fn role_of_iface(&self, iface: u8) -> Option<Role> {
        [
            (self.boot_keyboard, Role::BootKeyboard),
            (self.mouse, Role::Mouse),
            (self.nkro, Role::Nkro),
            (self.control, Role::Control),
        ]
        .into_iter()
        .find(|(f, _)| f.ep_in != 0 && f.iface == iface)
        .map(|(_, r)| r)
        .or_else(|| self.vial.filter(|v| v.iface == iface).map(|_| Role::Vial))
        .or_else(|| self.mirrors.iter().find(|m| m.f.iface == iface).map(|m| Role::Mirror(*m)))
    }

    pub fn mirror_by_out_ep(&self, ep: u8) -> Option<&MirrorMap> {
        self.mirrors.iter().find(|m| m.f.ep_out == ep && ep != 0)
    }

    pub fn mirror_of(&self, port: Port, src_iface: u8) -> Option<&MirrorMap> {
        self.mirrors.iter().find(|m| m.port == port && m.src_iface == src_iface)
    }
}

pub struct MirrorSource<'a> {
    pub port: Port,
    pub iface: &'a InterfaceInfo<'a>,
    /// HID report descriptor (empty for XInput).
    pub report_desc: &'a [u8],
}

pub struct IdentitySource<'a> {
    pub port: Port,
    pub device: DeviceDesc,
    pub manufacturer: &'a str,
    pub product: &'a str,
    pub serial: &'a str,
}

pub struct Params<'a> {
    pub polling: Polling,
    pub serial: &'a str,
    pub mirrors: &'a [MirrorSource<'a>],
    pub identity: Option<IdentitySource<'a>>,
    /// Add the Vial interface; mirrors then get one IN endpoint less.
    pub vial: bool,
    /// Add the KVM's own boot keyboard, mouse and NKRO/media interfaces (EP1–EP3 IN). Without them, mirrors take
    /// those endpoints too (`policy::wide`).
    pub normalized: bool,
    /// The mouse interface with the absolute pointer (screen hop, software.md §4.14).
    pub mouse_abs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BuildError {
    TooLarge,
    TooManyEndpoints,
}

impl From<WireError> for BuildError {
    fn from(_: WireError) -> Self {
        BuildError::TooLarge
    }
}

struct Buf<const N: usize> {
    b: [u8; N],
    n: usize,
}

impl<const N: usize> Buf<N> {
    fn new() -> Self {
        Self { b: [0; N], n: 0 }
    }

    fn put(&mut self, d: &[u8]) -> Result<(), BuildError> {
        let end = self.n + d.len();
        self.b.get_mut(self.n..end).ok_or(BuildError::TooLarge)?.copy_from_slice(d);
        self.n = end;
        Ok(())
    }

    fn data(&mut self) -> &mut [u8] {
        &mut self.b[..self.n]
    }
}

fn hs_interval(p: Polling) -> u8 {
    match p {
        Polling::Us125 => 1,
        Polling::Ms1 => 4,
    }
}

fn interface(n: u8, eps: u8, class: u8, sub: u8, proto: u8, string: u8) -> [u8; 9] {
    [9, dt::INTERFACE, n, 0, eps, class, sub, proto, string]
}

fn hid(report_len: usize) -> [u8; 9] {
    let [l0, l1] = (report_len as u16).to_le_bytes();
    [9, dt::HID, 0x11, 0x01, 0, 1, dt::HID_REPORT, l0, l1]
}

fn endpoint(addr: u8, mps: u16, interval: u8) -> [u8; 7] {
    let [m0, m1] = mps.to_le_bytes();
    [7, dt::ENDPOINT, addr, 3, m0, m1, interval]
}

/// Builds the descriptor set into `out`; returns its length and the function map.
pub fn build(p: &Params, out: &mut [u8]) -> Result<(usize, Presentation), BuildError> {
    let interval = hs_interval(p.polling);
    let mut cfg = Buf::<1024>::new();
    cfg.put(&[9, dt::CONFIGURATION, 0, 0, 0, 1, 0, 0xA0, 250])?;
    let mut pres = Presentation { identity: p.identity.as_ref().map(|i| i.port), ..Presentation::default() };
    let mut next_iface = 0u8;
    let mut ins = MIRROR_EPS.into_iter().filter(|&e| !(p.normalized && e <= 3 || p.vial && e == VIAL_EP));
    let mut outs = MIRROR_EPS.into_iter();
    let mut xinput_ifaces: Vec<u8, MAX_MIRRORS> = Vec::new();
    let mut report_descs: Vec<(u8, &[u8]), MAX_MIRRORS> = Vec::new();

    let mut add_mirror = |m: &MirrorSource, cfg: &mut Buf<1024>, n: u8| -> Result<MirrorMap, BuildError> {
        let mut map = MirrorMap {
            port: m.port,
            src_iface: m.iface.number,
            src_ep_in: m.iface.in_ep().map_or(0, |e| e.address),
            src_ep_out: m.iface.out_ep().map_or(0, |e| e.address),
            f: Function { iface: n, ep_in: 0, ep_out: 0 },
        };
        let start = cfg.n;
        cfg.put(m.iface.raw)?;
        let raw = &mut cfg.b[start..cfg.n];
        let mut pos = 0;
        let mut ifaces_seen = 0;
        while pos + 2 <= raw.len() {
            let len = raw[pos] as usize;
            if len < 2 || pos + len > raw.len() {
                break;
            }
            let d = &mut raw[pos..pos + len];
            match d[1] {
                dt::INTERFACE if len >= 9 => {
                    ifaces_seen += 1;
                    d[2] = n;
                    d[8] = 0; // the source's interface string is not carried over
                }
                dt::ENDPOINT if len >= 7 => {
                    if d[2] & 0x80 != 0 {
                        map.f.ep_in = 0x80 | ins.next().ok_or(BuildError::TooManyEndpoints)?;
                        d[2] = map.f.ep_in;
                    } else {
                        map.f.ep_out = outs.next().ok_or(BuildError::TooManyEndpoints)?;
                        d[2] = map.f.ep_out;
                    }
                    // High-speed interval code; the packet size (≤ 64 for a full-speed source) is kept.
                    d[6] = interval;
                    let mps = (u16::from_le_bytes([d[4], d[5]]) & 0x7FF).min(64);
                    d[4..6].copy_from_slice(&mps.to_le_bytes());
                }
                _ => {}
            }
            pos += len;
        }
        debug_assert_eq!(ifaces_seen, 1);
        // XInput's vendor class descriptor names its endpoints at fixed offsets (6: IN, 13: OUT).
        if m.iface.is_xinput() {
            let mut pos = 0;
            while pos + 2 <= raw.len() {
                let len = raw[pos] as usize;
                if len < 2 {
                    break;
                }
                if raw[pos + 1] == dt::HID && len == 17 {
                    if raw[pos + 6] == map.src_ep_in && map.f.ep_in != 0 {
                        raw[pos + 6] = map.f.ep_in;
                    }
                    if raw[pos + 13] == map.src_ep_out && map.f.ep_out != 0 {
                        raw[pos + 13] = map.f.ep_out;
                    }
                }
                pos += len;
            }
        }
        Ok(map)
    };

    // Identity: the cloned interfaces come first, numbered as on the source.
    if let Some(id) = &p.identity {
        for m in p.mirrors.iter().filter(|m| m.port == id.port) {
            let map = add_mirror(m, &mut cfg, next_iface)?;
            next_iface += 1;
            if m.iface.is_xinput() {
                let _ = xinput_ifaces.push(map.f.iface);
            } else {
                let _ = report_descs.push((map.f.iface, m.report_desc));
            }
            let _ = pres.mirrors.push(map);
        }
    }

    let (mouse_desc, mouse_len) = if p.mouse_abs {
        (normalized::MOUSE_ABS_DESC, normalized::MOUSE_ABS_LEN)
    } else {
        (normalized::MOUSE_DESC, normalized::MOUSE_LEN)
    };
    pres.mouse_abs = p.mouse_abs && p.normalized;
    let kvm = [
        (normalized::BOOT_KEYBOARD_DESC, 1, 1, normalized::BOOT_KEYBOARD_LEN, 0u8, 0u8),
        (mouse_desc, 1, 2, mouse_len, 0, 0),
        (normalized::NKRO_DESC, 0, 0, 64, 0, 0),
        (normalized::CONTROL_DESC, 0, 0, normalized::CONTROL_LEN, 4, strings::CONTROL),
    ];
    // (index into `kvm`, interface number)
    let mut kvm_ifaces: Vec<(usize, u8), 4> = Vec::new();
    for (k, (rd, sub, proto, mps, out_ep, string)) in kvm.iter().enumerate() {
        if k < 3 && !p.normalized {
            continue;
        }
        let n = next_iface;
        let _ = kvm_ifaces.push((k, n));
        next_iface += 1;
        let ep = (k + 1) as u8;
        let eps = if *out_ep != 0 { 2 } else { 1 };
        cfg.put(&interface(n, eps, 3, *sub, *proto, *string))?;
        cfg.put(&hid(rd.len()))?;
        let ep_interval = if k == 3 { 4 } else { interval };
        cfg.put(&endpoint(0x80 | ep, *mps as u16, ep_interval))?;
        if *out_ep != 0 {
            cfg.put(&endpoint(*out_ep, *mps as u16, ep_interval))?;
        }
        let f = Function { iface: n, ep_in: 0x80 | ep, ep_out: *out_ep };
        match k {
            0 => pres.boot_keyboard = f,
            1 => pres.mouse = f,
            2 => pres.nkro = f,
            _ => pres.control = f,
        }
    }
    if p.vial {
        let n = next_iface;
        next_iface += 1;
        cfg.put(&interface(n, 1, 3, 0, 0, strings::VIAL))?;
        cfg.put(&hid(kvm_vial::REPORT_DESC.len()))?;
        cfg.put(&endpoint(0x80 | VIAL_EP, kvm_vial::REPORT_LEN as u16, interval))?;
        pres.vial = Some(Function { iface: n, ep_in: 0x80 | VIAL_EP, ep_out: 0 });
    }

    for m in p.mirrors.iter().filter(|m| p.identity.as_ref().is_none_or(|id| id.port != m.port)) {
        let map = add_mirror(m, &mut cfg, next_iface)?;
        next_iface += 1;
        if m.iface.is_xinput() {
            let _ = xinput_ifaces.push(map.f.iface);
        } else {
            let _ = report_descs.push((map.f.iface, m.report_desc));
        }
        pres.mirrors.push(map).map_err(|_| BuildError::TooManyEndpoints)?;
    }

    let total = cfg.n as u16;
    let c = cfg.data();
    c[2..4].copy_from_slice(&total.to_le_bytes());
    c[4] = next_iface;

    // Device descriptor.
    let (manufacturer, product, serial) = match &p.identity {
        Some(id) => (id.manufacturer, id.product, id.serial),
        None => ("OpenDynKVM", "OpenDynKVM", p.serial),
    };
    let mut dev = DeviceDesc {
        usb: 0x0200,
        max_packet0: 64,
        vid: KVM_VID,
        pid: KVM_PID,
        bcd: KVM_BCD,
        i_manufacturer: strings::MANUFACTURER,
        i_product: strings::PRODUCT,
        i_serial: strings::SERIAL,
        configurations: 1,
        ..DeviceDesc::default()
    };
    if let Some(id) = &p.identity {
        dev.vid = id.device.vid;
        dev.pid = id.device.pid;
        dev.bcd = id.device.bcd;
        dev.i_manufacturer = if manufacturer.is_empty() { 0 } else { strings::MANUFACTURER };
        dev.i_product = if product.is_empty() { 0 } else { strings::PRODUCT };
        dev.i_serial = if serial.is_empty() { 0 } else { strings::SERIAL };
    }

    let flags = if p.identity.is_some() { descset::flags::IDENTITY } else { 0 };
    let mut w = descset::Writer::new(out, flags)?;
    w.record(tag::DEVICE, 0, &dev.encode())?;
    w.record(tag::CONFIG, 0, &cfg.b[..cfg.n])?;
    w.record(tag::STRING, 0, &[4, dt::STRING, 0x09, 0x04])?;
    let mut sbuf = [0u8; 256];
    for (idx, s) in [
        (strings::MANUFACTURER, manufacturer),
        (strings::PRODUCT, product),
        (strings::SERIAL, serial),
        (strings::CONTROL, INTERFACE_STRING),
        (strings::VIAL, if pres.vial.is_some() { VIAL_INTERFACE_STRING } else { "" }),
    ] {
        if !s.is_empty() {
            let n = encode_string(s, &mut sbuf);
            w.record(tag::STRING, idx, &sbuf[..n])?;
        }
    }
    for &(k, n) in &kvm_ifaces {
        w.record(tag::HID_REPORT, n, kvm[k].0)?;
        let extra = match k {
            0 => ifl::BOOT_KEYBOARD,
            1 => ifl::BOOT_MOUSE,
            _ => 0,
        };
        w.record(tag::IFACE, n, &[ifl::LOCAL_HID | extra])?;
    }
    if let Some(v) = pres.vial {
        w.record(tag::HID_REPORT, v.iface, kvm_vial::REPORT_DESC)?;
        w.record(tag::IFACE, v.iface, &[ifl::LOCAL_HID])?;
    }
    for (n, rd) in &report_descs {
        w.record(tag::HID_REPORT, *n, rd)?;
        w.record(tag::IFACE, *n, &[ifl::FORWARD])?;
    }
    if !xinput_ifaces.is_empty() {
        for n in &xinput_ifaces {
            w.record(tag::IFACE, *n, &[ifl::FORWARD])?;
        }
        // "MSFT100" + vendor code: Windows then asks for the compat IDs with that vendor request.
        let mut os = [0u8; 18];
        os[0] = 18;
        os[1] = dt::STRING;
        for (i, c) in "MSFT100".encode_utf16().enumerate() {
            os[2 + 2 * i..4 + 2 * i].copy_from_slice(&c.to_le_bytes());
        }
        os[16] = MSOS_VENDOR_CODE;
        w.record(tag::STRING, 0xEE, &os)?;
        let count = xinput_ifaces.len();
        let len = 16 + 24 * count;
        let rec = w.record_with(tag::MSOS10_COMPAT, MSOS_VENDOR_CODE, len)?;
        rec.fill(0);
        rec[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        rec[4..6].copy_from_slice(&0x0100u16.to_le_bytes());
        rec[6..8].copy_from_slice(&0x0004u16.to_le_bytes());
        rec[8] = count as u8;
        for (i, n) in xinput_ifaces.iter().enumerate() {
            let f = &mut rec[16 + 24 * i..16 + 24 * (i + 1)];
            f[0] = *n;
            f[1] = 0x01;
            f[2..8].copy_from_slice(b"XUSB10");
        }
    }
    let len = w.finish().len();
    Ok((len, pres))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usbdesc::fixtures::*;
    use crate::usbdesc::{decode_string, parse_config};
    use kvm_proto::descset::DescSet;

    const RD: &[u8] = &[0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0xC0];

    #[test]
    fn plain_kvm_device() {
        let mut out = [0u8; 4096];
        let (n, pres) = build(
            &Params {
                polling: Polling::Us125,
                serial: "KVM0001",
                mirrors: &[],
                identity: None,
                vial: false,
                normalized: true,
                mouse_abs: false,
            },
            &mut out,
        )
        .unwrap();
        let set = DescSet::parse(&out[..n]).unwrap();
        let dev = DeviceDesc::parse(set.device().unwrap()).unwrap();
        assert_eq!((dev.vid, dev.pid), (KVM_VID, KVM_PID));
        let cfg = parse_config(set.config().unwrap()).unwrap();
        assert_eq!(cfg.interfaces.len(), 4);
        assert_eq!(cfg.interfaces[0].in_ep().unwrap().interval, 1);
        assert_eq!(cfg.interfaces[3].out_ep().unwrap().address, 0x04);
        assert_eq!((pres.boot_keyboard.ep_in, pres.mouse.ep_in, pres.nkro.ep_in), (0x81, 0x82, 0x83));
        assert_eq!(pres.control, Function { iface: 3, ep_in: 0x84, ep_out: 0x04 });
        assert_eq!(set.iface_flags(0), ifl::LOCAL_HID | ifl::BOOT_KEYBOARD);
        assert_eq!(set.find(tag::HID_REPORT, 3), Some(normalized::CONTROL_DESC));
        let s = decode_string::<32>(set.find(tag::STRING, strings::CONTROL).unwrap());
        assert_eq!(s.as_str(), INTERFACE_STRING);
        assert_eq!(cfg.interfaces[3].i_interface, strings::CONTROL);
    }

    #[test]
    fn identity_mirror_comes_first_and_xinput_gets_compat_id() {
        let mouse = parse_config(GAMING_MOUSE_CONFIG).unwrap();
        let pad = parse_config(XINPUT_CONFIG).unwrap();
        let mirrors = [
            MirrorSource { port: Port::Mouse, iface: &mouse.interfaces[0], report_desc: RD },
            MirrorSource { port: Port::Mouse, iface: &mouse.interfaces[1], report_desc: RD },
            MirrorSource { port: Port::Other, iface: &pad.interfaces[0], report_desc: &[] },
        ];
        let identity = IdentitySource {
            port: Port::Mouse,
            device: DeviceDesc { vid: 0x046D, pid: 0xC08B, bcd: 0x2703, ..Default::default() },
            manufacturer: "Example",
            product: "Gaming mouse",
            serial: "",
        };
        let mut out = [0u8; 4096];
        let params = Params {
            polling: Polling::Ms1,
            serial: "x",
            mirrors: &mirrors,
            identity: Some(identity),
            vial: false,
            normalized: true,
            mouse_abs: false,
        };
        let (n, pres) = build(&params, &mut out).unwrap();
        let set = DescSet::parse(&out[..n]).unwrap();
        let dev = DeviceDesc::parse(set.device().unwrap()).unwrap();
        assert_eq!((dev.vid, dev.pid, dev.i_serial), (0x046D, 0xC08B, 0));
        let cfg = parse_config(set.config().unwrap()).unwrap();
        let nums: std::vec::Vec<_> = cfg.interfaces.iter().map(|i| (i.number, i.class)).collect();
        assert_eq!(nums, [(0, 3), (1, 3), (2, 3), (3, 3), (4, 3), (5, 3), (6, 0xFF)]);
        assert_eq!(pres.mirrors[0].f, Function { iface: 0, ep_in: 0x85, ep_out: 0 });
        assert_eq!(pres.mirrors[1].f, Function { iface: 1, ep_in: 0x86, ep_out: 0x05 });
        assert_eq!(pres.mirrors[2].f, Function { iface: 6, ep_in: 0x87, ep_out: 0x06 });
        assert_eq!(pres.control.iface, 5);
        assert_eq!(cfg.interfaces[1].in_ep().unwrap().interval, 4);
        assert_eq!(set.iface_flags(1), ifl::FORWARD);
        assert_eq!(set.find(tag::HID_REPORT, 0), Some(RD));
        // XInput class descriptor endpoints were renumbered with the endpoints.
        let x = &cfg.interfaces[6].raw;
        assert_eq!((x[9 + 6], x[9 + 13]), (0x87, 0x06));
        let compat = set.find(tag::MSOS10_COMPAT, MSOS_VENDOR_CODE).unwrap();
        assert_eq!((compat[16], &compat[18..24]), (6, &b"XUSB10"[..]));
        assert_eq!(set.find(tag::STRING, 0xEE).unwrap()[16], MSOS_VENDOR_CODE);
    }

    #[test]
    fn vial_interface_takes_the_last_in_endpoint() {
        let mouse = parse_config(GAMING_MOUSE_CONFIG).unwrap();
        let m = |i| MirrorSource { port: Port::Other, iface: &mouse.interfaces[i], report_desc: RD };
        let two = [m(0), m(1)];
        let three = [m(0), m(1), m(0)];
        let params = |mirrors| Params {
            polling: Polling::Us125,
            serial: "",
            mirrors,
            identity: None,
            vial: true,
            normalized: true,
            mouse_abs: false,
        };
        let mut out = [0u8; 4096];
        let (n, pres) = build(&params(&two), &mut out).unwrap();
        let set = DescSet::parse(&out[..n]).unwrap();
        assert_eq!(pres.vial, Some(Function { iface: 4, ep_in: 0x87, ep_out: 0 }));
        assert_eq!((pres.mirrors[0].f.ep_in, pres.mirrors[1].f.ep_in), (0x85, 0x86));
        assert_eq!(pres.role_of_iface(4), Some(Role::Vial));
        assert_eq!(set.find(tag::HID_REPORT, 4), Some(kvm_vial::REPORT_DESC));
        assert_eq!(set.iface_flags(4), ifl::LOCAL_HID);
        let cfg = parse_config(set.config().unwrap()).unwrap();
        assert_eq!(cfg.interfaces[4].in_ep().unwrap().max_packet, 32);
        assert!(cfg.interfaces[4].out_ep().is_none());
        // Named, for the udev rule that lets the browser open it.
        assert_eq!(cfg.interfaces[4].i_interface, strings::VIAL);
        let name = decode_string::<32>(set.find(tag::STRING, strings::VIAL).unwrap());
        assert_eq!(name.as_str(), kvm_proto::ctrl::VIAL_INTERFACE_STRING);
        // A third mirror no longer fits.
        let mut out2 = [0u8; 4096];
        assert_eq!(build(&params(&three), &mut out2).err(), Some(BuildError::TooManyEndpoints));
    }

    #[test]
    fn without_the_normalized_interfaces_mirrors_take_ep1_to_ep3() {
        let dev = parse_config(GAMING_MOUSE_CONFIG).unwrap();
        let m = |port, i| MirrorSource { port, iface: &dev.interfaces[i], report_desc: RD };
        let four = [m(Port::Keyboard, 0), m(Port::Keyboard, 1), m(Port::Mouse, 0), m(Port::Mouse, 1)];
        let params = |vial| Params {
            polling: Polling::Us125,
            serial: "",
            mirrors: &four,
            identity: None,
            vial,
            normalized: false,
            mouse_abs: true,
        };
        let mut out = [0u8; 4096];
        let (n, pres) = build(&params(false), &mut out).unwrap();
        let set = DescSet::parse(&out[..n]).unwrap();
        let cfg = parse_config(set.config().unwrap()).unwrap();
        assert_eq!(cfg.interfaces.len(), 5);
        assert_eq!(pres.control, Function { iface: 0, ep_in: 0x84, ep_out: 0x04 });
        assert_eq!(set.find(tag::HID_REPORT, 0), Some(normalized::CONTROL_DESC));
        assert_eq!((pres.boot_keyboard.ep_in, pres.mouse.ep_in, pres.nkro.ep_in), (0, 0, 0));
        let eps: std::vec::Vec<_> = pres.mirrors.iter().map(|m| (m.f.ep_in, m.f.ep_out)).collect();
        assert_eq!(eps, [(0x85, 0), (0x86, 0x05), (0x87, 0), (0x81, 0x06)]);
        // Interface 0 is the control interface, not the boot keyboard that isn't there.
        assert_eq!(pres.role_of_iface(0), Some(Role::Control));
        // With Vial, EP7 IN is its own.
        let (_, pres) = build(&params(true), &mut [0u8; 4096]).unwrap();
        let eps: std::vec::Vec<_> = pres.mirrors.iter().map(|m| m.f.ep_in).collect();
        assert_eq!(eps, [0x85, 0x86, 0x81, 0x82]);
        assert_eq!(pres.vial.map(|v| v.ep_in), Some(0x87));
        // No mouse interface: no absolute pointer either.
        assert!(!pres.mouse_abs);
    }

    #[test]
    fn the_absolute_pointer_changes_the_mouse_interface_only() {
        let params = |mouse_abs| Params {
            polling: Polling::Us125,
            serial: "KVM0001",
            mirrors: &[],
            identity: None,
            vial: false,
            normalized: true,
            mouse_abs,
        };
        let mut plain = [0u8; 4096];
        let (n0, p0) = build(&params(false), &mut plain).unwrap();
        let mut out = [0u8; 4096];
        let (n, pres) = build(&params(true), &mut out).unwrap();
        assert!(pres.mouse_abs && !p0.mouse_abs);
        assert_eq!((pres.mouse, pres.nkro, pres.control), (p0.mouse, p0.nkro, p0.control));
        let set = DescSet::parse(&out[..n]).unwrap();
        assert_eq!(set.find(tag::HID_REPORT, 1), Some(normalized::MOUSE_ABS_DESC));
        assert_eq!(set.iface_flags(1), ifl::LOCAL_HID | ifl::BOOT_MOUSE);
        let cfg = parse_config(set.config().unwrap()).unwrap();
        assert_eq!(cfg.interfaces[1].in_ep().unwrap().max_packet as usize, normalized::MOUSE_ABS_LEN);
        assert_ne!(&out[..n], &plain[..n0]);
    }

    #[test]
    fn too_many_mirrors_is_an_error() {
        let mouse = parse_config(GAMING_MOUSE_CONFIG).unwrap();
        let m = |i| MirrorSource { port: Port::Other, iface: &mouse.interfaces[i], report_desc: RD };
        let mirrors = [m(0), m(1), m(0), m(1)];
        let mut out = [0u8; 4096];
        let r = build(
            &Params {
                polling: Polling::Us125,
                serial: "",
                mirrors: &mirrors,
                identity: None,
                vial: false,
                normalized: true,
                mouse_abs: false,
            },
            &mut out,
        );
        assert_eq!(r.err(), Some(BuildError::TooManyEndpoints));
    }
}
