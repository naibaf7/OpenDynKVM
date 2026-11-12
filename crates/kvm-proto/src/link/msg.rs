//! Link messages (software.md §6.3). The first byte is the type.

use super::frame::MAX_MSG;
use crate::wire::{Reader, WireError, Writer, wire_enum};

pub mod ty {
    pub const HELLO: u8 = 0x01;
    /// 0x11–0x17: endpoint data, endpoint number = type − 0x10.
    pub const REPORT_BASE: u8 = 0x10;
    pub const DESC_BEGIN: u8 = 0x20;
    pub const DESC_DATA: u8 = 0x21;
    pub const DESC_COMMIT: u8 = 0x22;
    pub const CTRL_REQUEST: u8 = 0x30;
    pub const CTRL_REPLY: u8 = 0x31;
    pub const SET_REPORT: u8 = 0x32;
    pub const PROTOCOL_CHANGED: u8 = 0x33;
    pub const COMMAND: u8 = 0x40;
    pub const STATUS: u8 = 0x50;
}

/// Largest payload of each fragmented message.
pub const REPORT_MAX: usize = MAX_MSG - 1;
pub const DESC_CHUNK: usize = MAX_MSG - 3;
pub const CTRL_REQUEST_CHUNK: usize = MAX_MSG - 12;
pub const CTRL_REPLY_CHUNK: usize = MAX_MSG - 7;
pub const SET_REPORT_MAX: usize = MAX_MSG - 4;

/// `Hello.role` values.
pub mod role {
    pub const RP2354B: u8 = 0;
    pub const PC1: u8 = 1;
    pub const PC2: u8 = 2;
}

/// `Hello.flags` bits.
pub mod hello_flags {
    /// The AT32F405's 12 MHz crystal didn't start: it runs on its internal oscillator, without USB.
    pub const NO_CRYSTAL: u8 = 1 << 0;
}

wire_enum! {
    pub enum CtrlStatus { Ok = 0, Stall = 1 }
}

wire_enum! {
    pub enum Command {
        /// Connect the D+ pull-up (after a descriptor set is in place).
        Attach = 1,
        /// Soft-disconnect from the PC.
        Detach = 2,
        /// Signal remote wakeup if the PC allowed it.
        RemoteWakeup = 3,
        /// Pulse the PC hub's RESET_N (recovery).
        HubReset = 4,
        /// Send a `Status` now.
        RequestStatus = 5,
    }
}

wire_enum! {
    pub enum UsbState {
        /// No VBUS from the PC.
        Unpowered = 0,
        /// VBUS present, not (yet) reset by the hub.
        Powered = 1,
        Default = 2,
        Addressed = 3,
        Configured = 4,
        Suspended = 5,
        /// Soft-disconnected (no descriptor set yet, or swapping sets).
        Detached = 6,
    }
}

wire_enum! {
    pub enum UsbSpeed { Unknown = 0, Full = 1, High = 2 }
}

/// AT32F405 → RP2354B status (software.md §5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Status {
    pub usb_state: UsbState,
    pub speed: UsbSpeed,
    pub vbus_mv: u16,
    pub cc1_mv: u16,
    pub cc2_mv: u16,
    pub frame: u16,
    /// Bus resets seen since boot.
    pub resets: u16,
    /// CRC-32 of the active descriptor set (0 = none).
    pub desc_crc: u32,
    /// The PC enabled remote wakeup (SET_FEATURE DEVICE_REMOTE_WAKEUP), so a key press may wake it from sleep.
    pub wake_enabled: bool,
    /// The AT32F405's supply (the board's 3.3 V), measured against its 1.20 V reference (VINTRV, ±3.3 %).
    pub vdda_mv: Option<u16>,
    /// The AT32F405's die temperature in tenths of a °C (its sensor's offset varies by up to 20 °C between chips).
    pub temp_dc: Option<i16>,
}

/// `Status.vdda_mv` and `temp_dc` on the wire when unknown.
const NO_VDDA: u16 = 0;
const NO_TEMP: i16 = i16::MIN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LinkMsg<'a> {
    Hello {
        proto: u8,
        fw_version: u32,
        role: u8,
        /// [`hello_flags`] bits.
        flags: u8,
    },
    /// RP → AT: a report for IN endpoint `ep`. AT → RP: data received on OUT endpoint `ep`.
    Report {
        ep: u8,
        data: &'a [u8],
    },
    DescBegin {
        total_len: u16,
        crc32: u32,
    },
    DescData {
        offset: u16,
        data: &'a [u8],
    },
    DescCommit,
    /// A forwarded control request; requests with an OUT data stage arrive in fragments of
    /// [`CTRL_REQUEST_CHUNK`] bytes, each repeating `tag` and `setup`.
    CtrlRequest {
        tag: u8,
        setup: [u8; 8],
        offset: u16,
        data: &'a [u8],
    },
    /// The answer to `CtrlRequest` `tag`; an IN data stage arrives in fragments of [`CTRL_REPLY_CHUNK`] bytes.
    CtrlReply {
        tag: u8,
        status: CtrlStatus,
        total_len: u16,
        offset: u16,
        data: &'a [u8],
    },
    SetReport {
        iface: u8,
        report_type: u8,
        report_id: u8,
        data: &'a [u8],
    },
    ProtocolChanged {
        iface: u8,
        protocol: u8,
    },
    Command(Command),
    Status(Status),
}

impl<'a> LinkMsg<'a> {
    /// Encodes into `out` and returns the length.
    pub fn encode(&self, out: &mut [u8; MAX_MSG]) -> Result<usize, WireError> {
        let mut w = Writer::new(out);
        match *self {
            LinkMsg::Hello { proto, fw_version, role, flags } => {
                w.u8(ty::HELLO)?;
                w.u8(proto)?;
                w.u32(fw_version)?;
                w.u8(role)?;
                w.u8(flags)?;
            }
            LinkMsg::Report { ep, data } => {
                if !(1..=7).contains(&ep) {
                    return Err(WireError::Invalid);
                }
                w.u8(ty::REPORT_BASE + ep)?;
                w.bytes(data)?;
            }
            LinkMsg::DescBegin { total_len, crc32 } => {
                w.u8(ty::DESC_BEGIN)?;
                w.u16(total_len)?;
                w.u32(crc32)?;
            }
            LinkMsg::DescData { offset, data } => {
                w.u8(ty::DESC_DATA)?;
                w.u16(offset)?;
                w.bytes(data)?;
            }
            LinkMsg::DescCommit => w.u8(ty::DESC_COMMIT)?,
            LinkMsg::CtrlRequest { tag, setup, offset, data } => {
                w.u8(ty::CTRL_REQUEST)?;
                w.u8(tag)?;
                w.bytes(&setup)?;
                w.u16(offset)?;
                w.bytes(data)?;
            }
            LinkMsg::CtrlReply { tag, status, total_len, offset, data } => {
                w.u8(ty::CTRL_REPLY)?;
                w.u8(tag)?;
                w.u8(status as u8)?;
                w.u16(total_len)?;
                w.u16(offset)?;
                w.bytes(data)?;
            }
            LinkMsg::SetReport { iface, report_type, report_id, data } => {
                w.u8(ty::SET_REPORT)?;
                w.u8(iface)?;
                w.u8(report_type)?;
                w.u8(report_id)?;
                w.bytes(data)?;
            }
            LinkMsg::ProtocolChanged { iface, protocol } => {
                w.u8(ty::PROTOCOL_CHANGED)?;
                w.u8(iface)?;
                w.u8(protocol)?;
            }
            LinkMsg::Command(c) => {
                w.u8(ty::COMMAND)?;
                w.u8(c as u8)?;
            }
            LinkMsg::Status(s) => {
                w.u8(ty::STATUS)?;
                w.u8(s.usb_state as u8)?;
                w.u8(s.speed as u8)?;
                w.u16(s.vbus_mv)?;
                w.u16(s.cc1_mv)?;
                w.u16(s.cc2_mv)?;
                w.u16(s.frame)?;
                w.u16(s.resets)?;
                w.u32(s.desc_crc)?;
                w.bool(s.wake_enabled)?;
                w.u16(s.vdda_mv.unwrap_or(NO_VDDA))?;
                w.u16(s.temp_dc.unwrap_or(NO_TEMP) as u16)?;
            }
        }
        Ok(w.len())
    }

    pub fn decode(msg: &'a [u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(msg);
        let t = r.u8()?;
        Ok(match t {
            ty::HELLO => LinkMsg::Hello {
                proto: r.u8()?,
                fw_version: r.u32()?,
                role: r.u8()?,
                // Not sent by AT32F405 firmware 0.1.0.
                flags: if r.remaining() > 0 { r.u8()? } else { 0 },
            },
            0x11..=0x17 => LinkMsg::Report { ep: t - ty::REPORT_BASE, data: r.rest() },
            ty::DESC_BEGIN => LinkMsg::DescBegin { total_len: r.u16()?, crc32: r.u32()? },
            ty::DESC_DATA => LinkMsg::DescData { offset: r.u16()?, data: r.rest() },
            ty::DESC_COMMIT => LinkMsg::DescCommit,
            ty::CTRL_REQUEST => {
                LinkMsg::CtrlRequest { tag: r.u8()?, setup: r.array()?, offset: r.u16()?, data: r.rest() }
            }
            ty::CTRL_REPLY => LinkMsg::CtrlReply {
                tag: r.u8()?,
                status: r.u8()?.try_into()?,
                total_len: r.u16()?,
                offset: r.u16()?,
                data: r.rest(),
            },
            ty::SET_REPORT => {
                LinkMsg::SetReport { iface: r.u8()?, report_type: r.u8()?, report_id: r.u8()?, data: r.rest() }
            }
            ty::PROTOCOL_CHANGED => LinkMsg::ProtocolChanged { iface: r.u8()?, protocol: r.u8()? },
            ty::COMMAND => LinkMsg::Command(r.u8()?.try_into()?),
            ty::STATUS => LinkMsg::Status(Status {
                usb_state: r.u8()?.try_into()?,
                speed: r.u8()?.try_into()?,
                vbus_mv: r.u16()?,
                cc1_mv: r.u16()?,
                cc2_mv: r.u16()?,
                frame: r.u16()?,
                resets: r.u16()?,
                desc_crc: r.u32()?,
                // Not sent by AT32F405 firmware 0.1.0.
                wake_enabled: if r.remaining() > 0 { r.bool()? } else { false },
                // Not sent by AT32F405 firmware before the diagnostics (protocol 5 of the control protocol).
                vdda_mv: if r.remaining() >= 2 { Some(r.u16()?).filter(|v| *v != NO_VDDA) } else { None },
                temp_dc: if r.remaining() >= 2 { Some(r.u16()? as i16).filter(|v| *v != NO_TEMP) } else { None },
            }),
            _ => return Err(WireError::Invalid),
        })
    }

    /// Reports go ahead of everything else (software.md §6.2).
    pub fn prio(&self) -> super::Prio {
        match self {
            LinkMsg::Report { .. } => super::Prio::High,
            _ => super::Prio::Low,
        }
    }
}

/// Collects in-order fragments (the ARQ guarantees order) of a descriptor set or control data stage.
pub struct Reassembly<const N: usize> {
    buf: [u8; N],
    total: usize,
    filled: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ReassemblyError {
    TooLarge,
    OutOfOrder,
}

impl<const N: usize> Reassembly<N> {
    pub const fn new() -> Self {
        Self { buf: [0; N], total: 0, filled: 0 }
    }

    pub fn start(&mut self, total: usize) -> Result<(), ReassemblyError> {
        if total > N {
            return Err(ReassemblyError::TooLarge);
        }
        self.total = total;
        self.filled = 0;
        Ok(())
    }

    /// Adds a fragment; returns true once all `total` bytes are in.
    pub fn put(&mut self, offset: usize, data: &[u8]) -> Result<bool, ReassemblyError> {
        if offset != self.filled {
            return Err(ReassemblyError::OutOfOrder);
        }
        let end = offset + data.len();
        if end > self.total {
            return Err(ReassemblyError::TooLarge);
        }
        self.buf[offset..end].copy_from_slice(data);
        self.filled = end;
        Ok(self.is_complete())
    }

    pub fn is_complete(&self) -> bool {
        self.filled == self.total
    }

    pub fn data(&self) -> &[u8] {
        &self.buf[..self.filled]
    }

    pub fn total(&self) -> usize {
        self.total
    }
}

impl<const N: usize> Default for Reassembly<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Splits `data` into `(offset, chunk)` fragments of at most `chunk` bytes; an empty `data` yields one empty fragment.
pub fn fragments(data: &[u8], chunk: usize) -> impl Iterator<Item = (u16, &[u8])> {
    let n = data.len().div_ceil(chunk).max(1);
    (0..n).map(move |i| {
        let start = i * chunk;
        (start as u16, &data[start..(start + chunk).min(data.len())])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: LinkMsg) {
        let mut buf = [0u8; MAX_MSG];
        let n = m.encode(&mut buf).unwrap();
        assert_eq!(LinkMsg::decode(&buf[..n]).unwrap(), m);
    }

    #[test]
    fn all_messages_roundtrip() {
        let report = [0x5Au8; REPORT_MAX];
        roundtrip(LinkMsg::Hello { proto: 1, fw_version: 0x010203, role: role::PC2, flags: hello_flags::NO_CRYSTAL });
        roundtrip(LinkMsg::Report { ep: 7, data: &report });
        roundtrip(LinkMsg::Report { ep: 1, data: &[] });
        roundtrip(LinkMsg::DescBegin { total_len: 1234, crc32: 0xDEADBEEF });
        roundtrip(LinkMsg::DescData { offset: 62, data: &report[..DESC_CHUNK] });
        roundtrip(LinkMsg::DescCommit);
        roundtrip(LinkMsg::CtrlRequest { tag: 9, setup: [0x21, 9, 0, 2, 1, 0, 1, 0], offset: 0, data: &[2] });
        roundtrip(LinkMsg::CtrlReply {
            tag: 9,
            status: CtrlStatus::Ok,
            total_len: 100,
            offset: 58,
            data: &report[..CTRL_REPLY_CHUNK],
        });
        roundtrip(LinkMsg::SetReport { iface: 0, report_type: 2, report_id: 0, data: &[0b101] });
        roundtrip(LinkMsg::ProtocolChanged { iface: 1, protocol: 0 });
        roundtrip(LinkMsg::Command(Command::HubReset));
        roundtrip(LinkMsg::Status(Status {
            usb_state: UsbState::Configured,
            speed: UsbSpeed::High,
            vbus_mv: 5012,
            cc1_mv: 410,
            cc2_mv: 0,
            frame: 2047,
            resets: 3,
            desc_crc: 0x1234_5678,
            wake_enabled: true,
            vdda_mv: Some(3291),
            temp_dc: Some(-52),
        }));
        roundtrip(LinkMsg::Status(Status {
            usb_state: UsbState::Unpowered,
            speed: UsbSpeed::Unknown,
            vbus_mv: 0,
            cc1_mv: 0,
            cc2_mv: 0,
            frame: 0,
            resets: 0,
            desc_crc: 0,
            wake_enabled: false,
            vdda_mv: None,
            temp_dc: None,
        }));
    }

    #[test]
    fn hello_from_firmware_0_1_0_has_no_flags() {
        let mut buf = [0u8; MAX_MSG];
        let n = LinkMsg::Hello { proto: 1, fw_version: 0x100, role: role::PC1, flags: 1 }.encode(&mut buf).unwrap();
        let old = LinkMsg::decode(&buf[..n - 1]).unwrap();
        assert_eq!(old, LinkMsg::Hello { proto: 1, fw_version: 0x100, role: role::PC1, flags: 0 });
    }

    #[test]
    fn status_from_firmware_0_1_0_has_no_wake_flag() {
        let s = Status {
            usb_state: UsbState::Suspended,
            speed: UsbSpeed::High,
            vbus_mv: 5000,
            cc1_mv: 0,
            cc2_mv: 0,
            frame: 0,
            resets: 0,
            desc_crc: 0,
            wake_enabled: true,
            vdda_mv: Some(3300),
            temp_dc: Some(400),
        };
        let mut buf = [0u8; MAX_MSG];
        let n = LinkMsg::Status(s).encode(&mut buf).unwrap();
        let old = LinkMsg::decode(&buf[..n - 5]).unwrap();
        assert_eq!(old, LinkMsg::Status(Status { wake_enabled: false, vdda_mv: None, temp_dc: None, ..s }));
        // Firmware with the wake flag but without the measurements.
        let before_diag = LinkMsg::decode(&buf[..n - 4]).unwrap();
        assert_eq!(before_diag, LinkMsg::Status(Status { vdda_mv: None, temp_dc: None, ..s }));
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        let mut buf = [0u8; MAX_MSG];
        assert!(LinkMsg::Report { ep: 1, data: &[0; REPORT_MAX + 1] }.encode(&mut buf).is_err());
        assert!(LinkMsg::Report { ep: 0, data: &[] }.encode(&mut buf).is_err());
        assert!(LinkMsg::Report { ep: 8, data: &[] }.encode(&mut buf).is_err());
        assert!(
            LinkMsg::CtrlRequest { tag: 0, setup: [0; 8], offset: 0, data: &[0; CTRL_REQUEST_CHUNK + 1] }
                .encode(&mut buf)
                .is_err()
        );
    }

    #[test]
    fn fragment_and_reassemble() {
        let data: std::vec::Vec<u8> = (0..=255u8).cycle().take(1000).collect();
        let mut r = Reassembly::<1024>::new();
        r.start(data.len()).unwrap();
        let mut done = false;
        for (off, chunk) in fragments(&data, DESC_CHUNK) {
            assert!(!done);
            done = r.put(off as usize, chunk).unwrap();
        }
        assert!(done);
        assert_eq!(r.data(), &data[..]);
        assert_eq!(fragments(&[], 10).count(), 1);
        assert_eq!(r.put(5, &[1]), Err(ReassemblyError::OutOfOrder));
        assert_eq!(Reassembly::<4>::new().start(5), Err(ReassemblyError::TooLarge));
    }
}
