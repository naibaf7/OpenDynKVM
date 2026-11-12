//! USB standard descriptors: parsing what a front-port device reports, and encoding what the KVM presents.

use heapless::{String, Vec};

pub mod dt {
    pub const DEVICE: u8 = 0x01;
    pub const CONFIGURATION: u8 = 0x02;
    pub const STRING: u8 = 0x03;
    pub const INTERFACE: u8 = 0x04;
    pub const ENDPOINT: u8 = 0x05;
    pub const DEVICE_QUALIFIER: u8 = 0x06;
    pub const OTHER_SPEED_CONFIGURATION: u8 = 0x07;
    pub const INTERFACE_ASSOCIATION: u8 = 0x0B;
    pub const BOS: u8 = 0x0F;
    pub const HID: u8 = 0x21;
    pub const HID_REPORT: u8 = 0x22;
}

pub mod class {
    pub const AUDIO: u8 = 0x01;
    pub const CDC: u8 = 0x02;
    pub const HID: u8 = 0x03;
    pub const IMAGE: u8 = 0x06;
    pub const PRINTER: u8 = 0x07;
    pub const MASS_STORAGE: u8 = 0x08;
    pub const HUB: u8 = 0x09;
    pub const VIDEO: u8 = 0x0E;
    pub const VENDOR: u8 = 0xFF;
}

/// XInput (Xbox 360 wired protocol) interface triple.
pub const XINPUT: (u8, u8, u8) = (0xFF, 0x5D, 0x01);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DeviceDesc {
    pub usb: u16,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub max_packet0: u8,
    pub vid: u16,
    pub pid: u16,
    pub bcd: u16,
    pub i_manufacturer: u8,
    pub i_product: u8,
    pub i_serial: u8,
    pub configurations: u8,
}

fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

impl DeviceDesc {
    pub const EMPTY: DeviceDesc = DeviceDesc {
        usb: 0,
        class: 0,
        subclass: 0,
        protocol: 0,
        max_packet0: 0,
        vid: 0,
        pid: 0,
        bcd: 0,
        i_manufacturer: 0,
        i_product: 0,
        i_serial: 0,
        configurations: 0,
    };

    pub fn parse(d: &[u8]) -> Option<Self> {
        if d.len() < 18 || d[1] != dt::DEVICE {
            return None;
        }
        Some(Self {
            usb: le16(d, 2),
            class: d[4],
            subclass: d[5],
            protocol: d[6],
            max_packet0: d[7],
            vid: le16(d, 8),
            pid: le16(d, 10),
            bcd: le16(d, 12),
            i_manufacturer: d[14],
            i_product: d[15],
            i_serial: d[16],
            configurations: d[17],
        })
    }

    pub fn encode(&self) -> [u8; 18] {
        let [u0, u1] = self.usb.to_le_bytes();
        let [v0, v1] = self.vid.to_le_bytes();
        let [p0, p1] = self.pid.to_le_bytes();
        let [b0, b1] = self.bcd.to_le_bytes();
        [
            18,
            dt::DEVICE,
            u0,
            u1,
            self.class,
            self.subclass,
            self.protocol,
            self.max_packet0,
            v0,
            v1,
            p0,
            p1,
            b0,
            b1,
            self.i_manufacturer,
            self.i_product,
            self.i_serial,
            self.configurations,
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct EndpointDesc {
    pub address: u8,
    pub attributes: u8,
    pub max_packet: u16,
    pub interval: u8,
}

impl EndpointDesc {
    pub fn is_in(&self) -> bool {
        self.address & 0x80 != 0
    }

    pub fn number(&self) -> u8 {
        self.address & 0x0F
    }

    pub fn is_interrupt(&self) -> bool {
        self.attributes & 0x03 == 3
    }
}

pub const MAX_INTERFACES: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct InterfaceInfo<'a> {
    pub number: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
    pub i_interface: u8,
    /// `wDescriptorLength` of the HID report descriptor.
    pub hid_report_len: Option<u16>,
    pub endpoints: Vec<EndpointDesc, 4>,
    /// Alternate settings other than 0 exist (isochronous streaming: audio, video).
    pub has_alt_settings: bool,
    /// Every descriptor of alternate setting 0, from the interface descriptor up to the next interface.
    pub raw: &'a [u8],
}

impl InterfaceInfo<'_> {
    pub fn is_hid(&self) -> bool {
        self.class == class::HID
    }

    pub fn is_xinput(&self) -> bool {
        (self.class, self.subclass, self.protocol) == XINPUT
    }

    pub fn in_ep(&self) -> Option<&EndpointDesc> {
        self.endpoints.iter().find(|e| e.is_in() && e.is_interrupt())
    }

    pub fn out_ep(&self) -> Option<&EndpointDesc> {
        self.endpoints.iter().find(|e| !e.is_in() && e.is_interrupt())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ConfigInfo<'a> {
    pub value: u8,
    pub attributes: u8,
    pub max_power_ma: u16,
    pub interfaces: Vec<InterfaceInfo<'a>, MAX_INTERFACES>,
}

/// Iterates `(type, descriptor)` over a configuration descriptor's contents.
pub fn descriptors(raw: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut pos = 0;
    core::iter::from_fn(move || {
        let len = *raw.get(pos)? as usize;
        if len < 2 || pos + len > raw.len() {
            return None;
        }
        let d = &raw[pos..pos + len];
        pos += len;
        Some((d[1], d))
    })
}

fn finish<'a>(raw: &'a [u8], cur: Option<(usize, InterfaceInfo<'a>)>, end: usize, cfg: &mut ConfigInfo<'a>) {
    if let Some((start, mut i)) = cur {
        i.raw = &raw[start..end];
        // Over capacity: the device is reported with the interfaces that fit (it won't be mirrored anyway).
        let _ = cfg.interfaces.push(i);
    }
}

pub fn parse_config(raw: &[u8]) -> Option<ConfigInfo<'_>> {
    if raw.len() < 9 || raw[1] != dt::CONFIGURATION {
        return None;
    }
    let total = (le16(raw, 2) as usize).min(raw.len());
    let raw = &raw[..total];
    let mut cfg =
        ConfigInfo { value: raw[5], attributes: raw[7], max_power_ma: raw[8] as u16 * 2, interfaces: Vec::new() };
    let mut pos = 0usize;
    let mut current: Option<(usize, InterfaceInfo)> = None;
    for (ty, d) in descriptors(raw) {
        let at = pos;
        pos += d.len();
        match ty {
            dt::INTERFACE if d.len() >= 9 => {
                finish(raw, current.take(), at, &mut cfg);
                if d[3] != 0 {
                    // An alternate setting ends alternate setting 0; its endpoints belong to no one here.
                    if let Some(i) = cfg.interfaces.iter_mut().find(|i| i.number == d[2]) {
                        i.has_alt_settings = true;
                    }
                    continue;
                }
                current = Some((
                    at,
                    InterfaceInfo {
                        number: d[2],
                        class: d[5],
                        subclass: d[6],
                        protocol: d[7],
                        i_interface: d[8],
                        hid_report_len: None,
                        endpoints: Vec::new(),
                        has_alt_settings: false,
                        raw: &[],
                    },
                ));
            }
            dt::HID if d.len() >= 9 => {
                if let Some((_, i)) = current.as_mut()
                    && i.class == class::HID
                    && d[6] == dt::HID_REPORT
                {
                    i.hid_report_len = Some(le16(d, 7));
                }
            }
            dt::ENDPOINT if d.len() >= 7 => {
                if let Some((_, i)) = current.as_mut() {
                    let _ = i.endpoints.push(EndpointDesc {
                        address: d[2],
                        attributes: d[3],
                        max_packet: le16(d, 4) & 0x7FF,
                        interval: d[6],
                    });
                }
            }
            dt::INTERFACE_ASSOCIATION => finish(raw, current.take(), at, &mut cfg),
            _ => {}
        }
    }
    finish(raw, current.take(), pos, &mut cfg);
    Some(cfg)
}

/// UTF-16LE string descriptor → UTF-8 (unpaired surrogates become U+FFFD, overflow is cut).
pub fn decode_string<const N: usize>(desc: &[u8]) -> String<N> {
    let mut s = String::new();
    if desc.len() < 2 || desc[1] != dt::STRING {
        return s;
    }
    let len = (desc[0] as usize).min(desc.len());
    let units = desc[2..len].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes([c[0], c[1]]));
    for c in char::decode_utf16(units) {
        if s.push(c.unwrap_or(char::REPLACEMENT_CHARACTER)).is_err() {
            break;
        }
    }
    s
}

/// UTF-8 → string descriptor; returns its length (at most 255, cut on a character boundary).
pub fn encode_string(s: &str, out: &mut [u8]) -> usize {
    let mut n = 2;
    for unit in s.encode_utf16() {
        if n + 2 > out.len() || n + 2 > 254 {
            break;
        }
        out[n..n + 2].copy_from_slice(&unit.to_le_bytes());
        n += 2;
    }
    out[0] = n as u8;
    out[1] = dt::STRING;
    n
}

#[cfg(test)]
pub(crate) mod fixtures {
    /// A gaming mouse: interface 0 mouse (EP 0x81), interface 1 keyboard + vendor (EP 0x82 IN, 0x02 OUT).
    #[rustfmt::skip]
    pub const GAMING_MOUSE_CONFIG: &[u8] = &[
        9, 2, 66, 0, 2, 1, 0, 0xA0, 49,
        9, 4, 0, 0, 1, 3, 1, 2, 0,
        9, 0x21, 0x11, 0x01, 0, 1, 0x22, 67, 0,
        7, 5, 0x81, 3, 8, 0, 1,
        9, 4, 1, 0, 2, 3, 0, 0, 0,
        9, 0x21, 0x11, 0x01, 0, 1, 0x22, 151, 0,
        7, 5, 0x82, 3, 20, 0, 1,
        7, 5, 0x02, 3, 20, 0, 1,
    ];

    /// A webcam: IAD, video control, video streaming with an alternate setting, an audio interface.
    #[rustfmt::skip]
    pub const WEBCAM_CONFIG: &[u8] = &[
        9, 2, 67, 0, 3, 1, 0, 0x80, 250,
        8, 0x0B, 0, 2, 0x0E, 0x03, 0, 0,
        9, 4, 0, 0, 1, 0x0E, 1, 0, 0,
        7, 5, 0x83, 3, 16, 0, 8,
        9, 4, 1, 0, 0, 0x0E, 2, 0, 0,
        9, 4, 1, 1, 1, 0x0E, 2, 0, 0,
        7, 5, 0x81, 5, 0x00, 0x14, 1,
        9, 4, 2, 0, 0, 0x01, 1, 0, 0,
    ];

    /// Wired Xbox 360 pad: one XInput interface with its vendor class descriptor.
    #[rustfmt::skip]
    pub const XINPUT_CONFIG: &[u8] = &[
        9, 2, 49, 0, 1, 1, 0, 0xA0, 250,
        9, 4, 0, 0, 2, 0xFF, 0x5D, 0x01, 0,
        17, 0x21, 0x00, 0x01, 0x01, 0x25, 0x81, 0x14, 0x00, 0x00, 0x00, 0x00, 0x13, 0x01, 0x08, 0x00, 0x00,
        7, 5, 0x81, 3, 32, 0, 4,
        7, 5, 0x01, 3, 32, 0, 8,
    ];
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    #[test]
    fn device_roundtrip() {
        let d = DeviceDesc {
            usb: 0x0200,
            max_packet0: 64,
            vid: 0x046D,
            pid: 0xC08B,
            bcd: 0x2703,
            i_product: 2,
            configurations: 1,
            ..Default::default()
        };
        assert_eq!(DeviceDesc::parse(&d.encode()), Some(d));
    }

    #[test]
    fn gaming_mouse_interfaces() {
        let c = parse_config(GAMING_MOUSE_CONFIG).unwrap();
        assert_eq!((c.value, c.max_power_ma, c.interfaces.len()), (1, 98, 2));
        let (a, b) = (&c.interfaces[0], &c.interfaces[1]);
        assert!(a.is_hid() && a.in_ep().unwrap().address == 0x81 && a.out_ep().is_none());
        assert_eq!(a.hid_report_len, Some(67));
        assert_eq!(b.out_ep().unwrap().max_packet, 20);
        assert_eq!(a.raw.len(), 9 + 9 + 7);
        assert_eq!(b.raw.len(), 9 + 9 + 7 + 7);
    }

    #[test]
    fn webcam_alt_settings_and_iad() {
        let c = parse_config(WEBCAM_CONFIG).unwrap();
        assert_eq!(
            c.interfaces.iter().map(|i| (i.number, i.class, i.has_alt_settings)).collect::<std::vec::Vec<_>>(),
            [(0, 0x0E, false), (1, 0x0E, true), (2, 0x01, false)]
        );
    }

    #[test]
    fn xinput_interface() {
        let c = parse_config(XINPUT_CONFIG).unwrap();
        let i = &c.interfaces[0];
        assert!(i.is_xinput() && !i.is_hid());
        assert_eq!((i.in_ep().unwrap().address, i.out_ep().unwrap().address), (0x81, 0x01));
        assert_eq!(i.raw.len(), 9 + 17 + 7 + 7);
    }

    #[test]
    fn strings() {
        let mut buf = [0u8; 64];
        let n = encode_string("Gerät ✓", &mut buf);
        assert_eq!(decode_string::<32>(&buf[..n]).as_str(), "Gerät ✓");
        assert_eq!(decode_string::<4>(&buf[..n]).as_str(), "Ger");
        assert_eq!(decode_string::<8>(&[4, 3, 0x00, 0xD8]).as_str(), "\u{FFFD}");
    }
}
