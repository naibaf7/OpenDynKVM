//! Descriptor set: everything an AT32F405 needs to present a USB device to its PC (software.md §7.2).
//!
//! ```text
//! "KDS1" | total_len u16 | record_count u8 | flags u8
//! record: tag u8 | arg u8 | len u16 | data[len]
//! ```

use crate::wire::WireError;

pub const MAGIC: &[u8; 4] = b"KDS1";
pub const HEADER_LEN: usize = 8;
/// Largest set an AT32F405 accepts.
pub const MAX_LEN: usize = 4096;

pub mod tag {
    /// Device descriptor (18 bytes).
    pub const DEVICE: u8 = 0x01;
    /// Full configuration descriptor with high-speed values.
    pub const CONFIG: u8 = 0x02;
    /// String descriptor; `arg` = index (0 = language IDs, 0xEE = Microsoft OS string).
    pub const STRING: u8 = 0x03;
    /// HID report descriptor; `arg` = interface number.
    pub const HID_REPORT: u8 = 0x04;
    /// Interface handling flags ([`super::iface`]); `arg` = interface number.
    pub const IFACE: u8 = 0x05;
    pub const BOS: u8 = 0x06;
    /// Microsoft OS 2.0 descriptor set; `arg` = vendor code.
    pub const MSOS20: u8 = 0x07;
    /// Microsoft OS 1.0 extended compat-ID descriptor; `arg` = vendor code.
    pub const MSOS10_COMPAT: u8 = 0x08;
}

/// `IFACE` record flags.
pub mod iface {
    /// HID class requests are answered by the AT32F405 (idle, protocol, report caches).
    pub const LOCAL_HID: u8 = 1 << 0;
    /// Class and vendor requests go to the RP2354B.
    pub const FORWARD: u8 = 1 << 1;
    /// Boot-protocol keyboard: `SET_PROTOCOL` is reported to the RP2354B.
    pub const BOOT_KEYBOARD: u8 = 1 << 2;
    /// Boot-protocol mouse: likewise.
    pub const BOOT_MOUSE: u8 = 1 << 3;
}

/// Header flags.
pub mod flags {
    /// The set came from a mirrored device's identity (VID/PID); informational.
    pub const IDENTITY: u8 = 1 << 0;
}

pub struct Writer<'a> {
    buf: &'a mut [u8],
    len: usize,
    count: u8,
    flags: u8,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8], flags: u8) -> Result<Self, WireError> {
        if buf.len() < HEADER_LEN {
            return Err(WireError::Overflow);
        }
        Ok(Self { buf, len: HEADER_LEN, count: 0, flags })
    }

    pub fn record(&mut self, tag: u8, arg: u8, data: &[u8]) -> Result<(), WireError> {
        let end = self.len + 4 + data.len();
        if end > self.buf.len() || end > MAX_LEN || self.count == u8::MAX {
            return Err(WireError::Overflow);
        }
        self.buf[self.len] = tag;
        self.buf[self.len + 1] = arg;
        self.buf[self.len + 2..self.len + 4].copy_from_slice(&(data.len() as u16).to_le_bytes());
        self.buf[self.len + 4..end].copy_from_slice(data);
        self.len = end;
        self.count += 1;
        Ok(())
    }

    /// Reserves a record of `len` bytes and returns it for in-place filling.
    pub fn record_with(&mut self, tag: u8, arg: u8, len: usize) -> Result<&mut [u8], WireError> {
        let start = self.len + 4;
        self.record(tag, arg, &[])?;
        let end = start + len;
        if end > self.buf.len() || end > MAX_LEN {
            return Err(WireError::Overflow);
        }
        self.buf[start - 2..start].copy_from_slice(&(len as u16).to_le_bytes());
        self.len = end;
        Ok(&mut self.buf[start..end])
    }

    pub fn finish(self) -> &'a [u8] {
        self.buf[..4].copy_from_slice(MAGIC);
        self.buf[4..6].copy_from_slice(&(self.len as u16).to_le_bytes());
        self.buf[6] = self.count;
        self.buf[7] = self.flags;
        &self.buf[..self.len]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Record<'a> {
    pub tag: u8,
    pub arg: u8,
    pub data: &'a [u8],
}

/// A validated descriptor set.
#[derive(Clone, Copy)]
pub struct DescSet<'a> {
    raw: &'a [u8],
}

impl<'a> DescSet<'a> {
    /// Checks the header and that every record lies inside the set.
    pub fn parse(raw: &'a [u8]) -> Result<Self, WireError> {
        if raw.len() < HEADER_LEN || &raw[..4] != MAGIC {
            return Err(WireError::Invalid);
        }
        let total = u16::from_le_bytes([raw[4], raw[5]]) as usize;
        if total > raw.len() || total < HEADER_LEN {
            return Err(WireError::Truncated);
        }
        let set = Self { raw: &raw[..total] };
        let mut n = 0usize;
        let mut pos = HEADER_LEN;
        while pos < total {
            if pos + 4 > total {
                return Err(WireError::Truncated);
            }
            let len = u16::from_le_bytes([raw[pos + 2], raw[pos + 3]]) as usize;
            pos += 4 + len;
            if pos > total {
                return Err(WireError::Truncated);
            }
            n += 1;
        }
        if n != raw[6] as usize {
            return Err(WireError::Invalid);
        }
        Ok(set)
    }

    pub fn raw(&self) -> &'a [u8] {
        self.raw
    }

    pub fn flags(&self) -> u8 {
        self.raw[7]
    }

    pub fn records(&self) -> impl Iterator<Item = Record<'a>> + 'a {
        let raw = self.raw;
        let mut pos = HEADER_LEN;
        core::iter::from_fn(move || {
            if pos + 4 > raw.len() {
                return None;
            }
            let len = u16::from_le_bytes([raw[pos + 2], raw[pos + 3]]) as usize;
            let r = Record { tag: raw[pos], arg: raw[pos + 1], data: &raw[pos + 4..pos + 4 + len] };
            pos += 4 + len;
            Some(r)
        })
    }

    pub fn find(&self, tag: u8, arg: u8) -> Option<&'a [u8]> {
        self.records().find(|r| r.tag == tag && r.arg == arg).map(|r| r.data)
    }

    /// First record with `tag`, whatever its `arg` (for `MSOS20` / `MSOS10_COMPAT`, whose arg is the vendor code).
    pub fn find_tag(&self, tag: u8) -> Option<Record<'a>> {
        self.records().find(|r| r.tag == tag)
    }

    pub fn device(&self) -> Option<&'a [u8]> {
        self.find(tag::DEVICE, 0).filter(|d| d.len() == 18)
    }

    pub fn config(&self) -> Option<&'a [u8]> {
        self.find(tag::CONFIG, 0)
    }

    pub fn iface_flags(&self, iface: u8) -> u8 {
        self.find(tag::IFACE, iface).and_then(|d| d.first().copied()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_parse_find() {
        let mut buf = [0u8; 256];
        let mut w = Writer::new(&mut buf, flags::IDENTITY).unwrap();
        w.record(tag::DEVICE, 0, &[18; 18]).unwrap();
        w.record(tag::STRING, 2, &[4, 3, b'K', 0]).unwrap();
        w.record_with(tag::HID_REPORT, 1, 3).unwrap().copy_from_slice(&[5, 1, 9]);
        w.record(tag::IFACE, 1, &[iface::LOCAL_HID]).unwrap();
        let raw = w.finish().to_vec();

        let set = DescSet::parse(&raw).unwrap();
        assert_eq!(set.flags(), flags::IDENTITY);
        assert_eq!(set.device(), Some(&[18u8; 18][..]));
        assert_eq!(set.find(tag::STRING, 2), Some(&[4u8, 3, b'K', 0][..]));
        assert_eq!(set.find(tag::HID_REPORT, 1), Some(&[5u8, 1, 9][..]));
        assert_eq!(set.iface_flags(1), iface::LOCAL_HID);
        assert_eq!(set.iface_flags(0), 0);
        assert_eq!(set.records().count(), 4);

        assert!(DescSet::parse(&raw[..raw.len() - 1]).is_err());
        let mut bad = raw.clone();
        bad[6] = 9;
        assert!(DescSet::parse(&bad).is_err());
        bad = raw.clone();
        bad[0] = b'X';
        assert!(DescSet::parse(&bad).is_err());
    }
}
