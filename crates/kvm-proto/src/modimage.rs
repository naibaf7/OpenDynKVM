//! The Bluetooth module's firmware as the host software streams it to the module through the RP2354B (software.md
//! §4.13): `kvm-module-<chip>.bin` from `cargo xtask dist`, sent with `FwTarget::Module`. The RP2354B keeps none of
//! it: it passes it on to the module's ROM loader as it arrives.
//!
//! A [`Header`], then the module's flash image from its chip's bootloader offset ([`Header::base`]: 0 on the
//! ESP32-C3, 0x1000 on the ESP32; bootloader, partition table, firmware) in two parts, in the order they are written:
//! everything after the image's first flash sector, compressed with zlib as the ROM loader takes it (FLASH_DEFL_*),
//! then that sector as it is (FLASH_DATA), which holds the bootloader's header. Not a second compressed region: the
//! ESP32's ROM wrote one wrong, 32 KB from its offset with what the first left behind (2026-11-24). It is written last, and only once the whole file has arrived
//! and is the one the RP2354B firmware was built with for that chip (its SHA-256): a stream cut short, or another
//! image, leaves it erased, and the module doesn't boot what came before it.
//!
//! One file per chip ([`crate::link::bt::Chip`]): `kvm-module-esp32c3.bin`, `kvm-module-esp32.bin`. The host software
//! sends the one for the chip the KVM reports, and the RP2354B checks the header's chip against the module's ROM.

/// Starts the file.
pub const MAGIC: [u8; 8] = *b"KVMBTMOD";
pub const HEADER_LEN: usize = 72;
/// The image's first flash sector: erased first, written last.
pub const HEAD: u32 = 4096;
/// The module's flash.
pub const FLASH: u32 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Header {
    /// The module firmware's version (its `KVMESP32` marker), the same as the RP2354B firmware it goes with.
    pub version: u32,
    /// The flash image's length, uncompressed.
    pub len: u32,
    /// The first part's length (the first sector, or less in an image shorter than that), and as stored: as it is, so
    /// the same. The rest's length compressed.
    pub head_len: u32,
    pub head_stored: u32,
    pub tail_z: u32,
    /// CRC-32 of the compressed data as it follows the header: the rest, then the first sector.
    pub crc32: u32,
    /// MD5 of the flash image as the ROM loader reports it (32 hex digits).
    pub md5: [u8; 32],
    /// The chip it is for ([`crate::link::bt::Chip`] as a number).
    pub chip: u8,
    /// The flash offset the image starts at: its chip's bootloader offset.
    pub base: u32,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[..8].copy_from_slice(&MAGIC);
        let words = [self.version, self.len, self.head_len, self.head_stored, self.tail_z, self.crc32];
        for (i, v) in words.iter().enumerate() {
            b[8 + 4 * i..12 + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        b[32..64].copy_from_slice(&self.md5);
        b[64..68].copy_from_slice(&(self.chip as u32).to_le_bytes());
        b[68..72].copy_from_slice(&self.base.to_le_bytes());
        b
    }

    /// The header at the start of `b`, if it is one and its sizes make sense.
    pub fn decode(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER_LEN || b[..8] != MAGIC {
            return None;
        }
        let mut md5 = [0u8; 32];
        md5.copy_from_slice(&b[32..64]);
        let h = Header {
            version: u32_at(b, 8),
            len: u32_at(b, 12),
            head_len: u32_at(b, 16),
            head_stored: u32_at(b, 20),
            tail_z: u32_at(b, 24),
            crc32: u32_at(b, 28),
            md5,
            chip: u32_at(b, 64).try_into().ok()?,
            base: u32_at(b, 68),
        };
        let sane = h.len > 0
            && h.chip != 0
            && h.base.is_multiple_of(HEAD)
            && h.base + h.len <= FLASH
            && h.head_len == h.len.min(HEAD)
            && h.head_stored == h.head_len
            && (h.tail_z > 0) == (h.len > HEAD)
            && h.tail_z <= FLASH
            && h.md5.iter().all(u8::is_ascii_hexdigit);
        sane.then_some(h)
    }

    /// The compressed data's length, and the whole file's.
    pub fn data_len(&self) -> usize {
        self.head_stored as usize + self.tail_z as usize
    }

    pub fn file_len(&self) -> usize {
        HEADER_LEN + self.data_len()
    }
}

/// The header of a whole file, if its data is all there and matches its CRC.
pub fn check(file: &[u8]) -> Option<Header> {
    let h = Header::decode(file)?;
    (file.len() == h.file_len() && crate::crc::crc32(&file[HEADER_LEN..]) == h.crc32).then_some(h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(version: u32) -> std::vec::Vec<u8> {
        // The rest compressed (5 bytes here), then the first sector as it is.
        let mut data = std::vec![0x78u8, 0x9c, 1, 2, 3];
        data.extend_from_slice(&[0xE9; 4096]);
        let h = Header {
            version,
            len: 9000,
            head_len: 4096,
            head_stored: 4096,
            tail_z: 5,
            crc32: crate::crc::crc32(&data),
            md5: *b"0123456789abcdef0123456789abcdef",
            chip: crate::link::bt::Chip::Esp32 as u8,
            base: 0x1000,
        };
        let mut f = h.encode().to_vec();
        f.extend_from_slice(&data);
        f
    }

    #[test]
    fn a_file_reads_back() {
        let f = file(0x0000_0133);
        let h = check(&f).unwrap();
        assert_eq!(
            (h.version, h.len, h.head_len, h.head_stored, h.tail_z, h.file_len()),
            (0x133, 9000, 4096, 4096, 5, f.len())
        );
        assert_eq!((h.chip, h.base), (2, 0x1000));
        assert_eq!(Header::decode(&h.encode()), Some(h));
    }

    #[test]
    fn damage_is_found() {
        let f = file(1);
        assert_eq!(check(&f[..f.len() - 1]), None, "cut short");
        let mut bad = f.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert_eq!(check(&bad), None, "data");
        let mut bad = f.clone();
        bad[0] = b'X';
        assert_eq!(check(&bad), None, "magic");
        let mut bad = f.clone();
        bad[16..20].copy_from_slice(&256u32.to_le_bytes());
        assert_eq!(Header::decode(&bad), None, "a first part shorter than the first sector");
        let mut bad = f.clone();
        bad[20..24].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(Header::decode(&bad), None, "a first sector stored at another length (compressed)");
        assert_eq!(Header::decode(&[0xFF; HEADER_LEN]), None, "erased");
        let mut bad = f.clone();
        bad[64..68].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(Header::decode(&bad), None, "no chip");
        let mut bad = f.clone();
        bad[68..72].copy_from_slice(&0x800u32.to_le_bytes());
        assert_eq!(Header::decode(&bad), None, "a base inside a sector");
    }
}
