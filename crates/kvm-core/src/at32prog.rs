//! How often the RP2354B tries to program an AT32F405 (software.md §4.9), and the Bluetooth module (§4.13, the same
//! limits, its own record).
//!
//! Each attempt is counted in flash before it starts, so a power loss in the middle counts too, and the count goes back
//! to 0 once the chip answers with the embedded firmware. After [`MAX_FAILS`] attempts in a row it gives up, until the
//! embedded image changes (the record belongs to one image), the KVM's key is held at power-up, or software asks for
//! another try (`RebootTarget::At32Retry`). Within one boot it tries again at once only if the transfer itself failed:
//! a chip that took its image but doesn't answer waits for the next power-up.

/// Store key of the AT32F405s' [`Record`].
pub const ITEM: u16 = 0x0002;
/// Store key of the Bluetooth module's [`Record`], for its embedded image; its count is `fails[0]`.
pub const BT_ITEM: u16 = 0x0003;
/// Attempts in a row before giving up.
pub const MAX_FAILS: u8 = 5;
/// Attempts within one boot, when the transfer fails.
pub const MAX_PER_BOOT: u8 = 2;

/// Unconfirmed attempts per AT32F405, for the image with CRC-32 `image_crc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Record {
    pub image_crc: u32,
    pub fails: [u8; 2],
}

impl Record {
    pub const LEN: usize = 6;

    pub fn encode(&self) -> [u8; Record::LEN] {
        let c = self.image_crc.to_le_bytes();
        [c[0], c[1], c[2], c[3], self.fails[0], self.fails[1]]
    }

    /// The saved record if it belongs to the image with `image_crc`; otherwise a fresh one for it.
    pub fn decode(data: &[u8], image_crc: u32) -> Record {
        match data {
            [c0, c1, c2, c3, f0, f1, ..] if u32::from_le_bytes([*c0, *c1, *c2, *c3]) == image_crc => {
                Record { image_crc, fails: [*f0, *f1] }
            }
            _ => Record { image_crc, fails: [0; 2] },
        }
    }
}

/// How the last attempt this boot went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Attempt {
    /// The ROM bootloader session failed (sync, erase, write or read-back).
    TransferFailed,
    /// The image went in, but the chip doesn't answer with it.
    NoAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Next {
    /// Count an attempt, then program.
    Program,
    /// Leave it for the next power-up.
    Wait,
    /// [`MAX_FAILS`] attempts failed: no more.
    GiveUp,
}

/// What to do about an AT32F405 that doesn't answer with the embedded firmware. `fails`: attempts in a row so far
/// (saved); `tries`: attempts this boot; `last`: how the last one this boot went.
pub fn next(fails: u8, tries: u8, last: Option<Attempt>) -> Next {
    if fails >= MAX_FAILS {
        return Next::GiveUp;
    }
    match last {
        None => Next::Program,
        Some(Attempt::TransferFailed) if tries < MAX_PER_BOOT => Next::Program,
        Some(_) => Next::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_belongs_to_one_image() {
        let r = Record { image_crc: 0xDEAD_BEEF, fails: [3, 0] };
        assert_eq!(Record::decode(&r.encode(), 0xDEAD_BEEF), r);
        assert_eq!(Record::decode(&r.encode(), 0x1234_5678), Record { image_crc: 0x1234_5678, fails: [0; 2] });
        assert_eq!(Record::decode(&[1, 2], 7).fails, [0; 2]);
    }

    #[test]
    fn retries_once_per_boot_only_after_a_failed_transfer() {
        assert_eq!(next(0, 0, None), Next::Program);
        assert_eq!(next(1, 1, Some(Attempt::TransferFailed)), Next::Program);
        assert_eq!(next(2, 2, Some(Attempt::TransferFailed)), Next::Wait);
        assert_eq!(next(1, 1, Some(Attempt::NoAnswer)), Next::Wait);
    }

    #[test]
    fn gives_up_after_five_in_a_row() {
        // One attempt per boot, the chip never answering: boots 1-5 program, boot 6 gives up.
        let mut fails = 0;
        let mut programmed = 0;
        for _boot in 0..10 {
            if next(fails, 0, None) == Next::Program {
                fails += 1;
                programmed += 1;
                // The fifth gives up at once, in the same boot.
                let then = if fails < MAX_FAILS { Next::Wait } else { Next::GiveUp };
                assert_eq!(next(fails, 1, Some(Attempt::NoAnswer)), then);
            }
        }
        assert_eq!((programmed, next(fails, 0, None)), (MAX_FAILS, Next::GiveUp));
    }
}
