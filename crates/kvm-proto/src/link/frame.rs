//! The fixed 72-byte link frame (software.md §6.1).

use crate::crc::crc16;

pub const FRAME_LEN: usize = 72;
/// Largest message: a type byte plus a full 64-byte HID report.
pub const MAX_MSG: usize = 65;
pub const MAGIC: u8 = 0xA5;
const HEADER: usize = 5;
const CRC_AT: usize = FRAME_LEN - 2;

/// Header flag bits.
pub mod flags {
    /// The frame carries a message with sequence number `seq`.
    pub const DATA: u8 = 1 << 0;
    /// The sender restarted and numbers its messages afresh.
    pub const SYN: u8 = 1 << 1;
    /// The sender has more queued after this frame.
    pub const MORE: u8 = 1 << 2;
    /// `ack` is meaningful (the sender is synchronised to our sequence numbers).
    pub const ACKV: u8 = 1 << 3;
    /// The sender has seen our `SYN` and re-synchronised to it.
    pub const SYNACK: u8 = 1 << 4;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum FrameError {
    BadMagic,
    BadLength,
    BadCrc,
}

#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Frame {
    pub flags: u8,
    pub seq: u8,
    pub ack: u8,
    pub len: u8,
    pub msg: [u8; MAX_MSG],
}

impl core::fmt::Debug for Frame {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Frame")
            .field("flags", &format_args!("{:#04x}", self.flags))
            .field("seq", &self.seq)
            .field("ack", &self.ack)
            .field("msg", &self.message())
            .finish()
    }
}

impl Frame {
    pub const fn idle() -> Self {
        Self { flags: 0, seq: 0, ack: 0, len: 0, msg: [0; MAX_MSG] }
    }

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }

    pub fn message(&self) -> &[u8] {
        &self.msg[..self.len as usize]
    }

    pub fn encode(&self, out: &mut [u8; FRAME_LEN]) {
        out[0] = MAGIC;
        out[1] = self.flags;
        out[2] = self.seq;
        out[3] = self.ack;
        out[4] = self.len;
        out[HEADER..HEADER + MAX_MSG].copy_from_slice(&self.msg);
        let crc = crc16(&out[..CRC_AT]);
        out[CRC_AT..].copy_from_slice(&crc.to_le_bytes());
    }

    pub fn decode(raw: &[u8; FRAME_LEN]) -> Result<Self, FrameError> {
        if raw[0] != MAGIC {
            return Err(FrameError::BadMagic);
        }
        if crc16(&raw[..CRC_AT]) != u16::from_le_bytes([raw[CRC_AT], raw[CRC_AT + 1]]) {
            return Err(FrameError::BadCrc);
        }
        let len = raw[4];
        if len as usize > MAX_MSG {
            return Err(FrameError::BadLength);
        }
        let mut msg = [0u8; MAX_MSG];
        msg.copy_from_slice(&raw[HEADER..HEADER + MAX_MSG]);
        Ok(Self { flags: raw[1], seq: raw[2], ack: raw[3], len, msg })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_corruption() {
        let mut f = Frame::idle();
        f.flags = flags::DATA | flags::ACKV;
        f.seq = 7;
        f.ack = 200;
        f.len = 3;
        f.msg[..3].copy_from_slice(&[0x11, 0xAA, 0x55]);
        let mut raw = [0u8; FRAME_LEN];
        f.encode(&mut raw);
        assert_eq!(Frame::decode(&raw), Ok(f));

        for i in 0..FRAME_LEN {
            let mut bad = raw;
            bad[i] ^= 0x10;
            assert!(Frame::decode(&bad).is_err(), "flip at byte {i} undetected");
        }
        assert_eq!(Frame::decode(&[0u8; FRAME_LEN]), Err(FrameError::BadMagic));
        assert_eq!(Frame::decode(&[0xFF; FRAME_LEN]), Err(FrameError::BadMagic));
    }
}
