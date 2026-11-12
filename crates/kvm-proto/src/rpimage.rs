//! RP2350 image metadata (RP2350 datasheet §5.9): enough to check that an update image is an RP2350 Arm executable
//! and to mark it *try before you buy* (§5.1.17), used by the RP2354B's update writer and by `odkvmctl`/`xtask`; and
//! the [`FwId`] that says which firmware and board an RP2354B image is.

/// The boot ROM looks for the first block in this many bytes of an image.
pub const SEARCH_LEN: usize = 4096;

/// The RP2354B's flash layout in 4 KB sectors (`firmware/rp2354/src/flash.rs`, which a board keeps for good once
/// flashed; `cargo xtask dist` writes the partition table from these): the partition table, A and B, the data
/// partition, then the last sector left free (RP2350-E10).
pub mod layout {
    pub const SECTOR: u32 = 4096;
    pub const PARTITION_A_SECTOR: u32 = 2;
    pub const PARTITION_SECTORS: u32 = 246;
    pub const PARTITION_B_SECTOR: u32 = PARTITION_A_SECTOR + PARTITION_SECTORS;
    pub const DATA_SECTORS: u32 = 17;
}

const BLOCK_START: u32 = 0xffff_ded3;
const BLOCK_END: u32 = 0xab12_3579;
const ITEM_IMAGE_TYPE: u8 = 0x42;
const ITEM_VERSION: u8 = 0x48;
const ITEM_LAST: u8 = 0xff;
const SIZE_FLAG: u32 = 0x80;

/// `IMAGE_TYPE` flags (datasheet Table 494).
pub mod image_type {
    pub const TYPE_MASK: u16 = 0x000F;
    pub const TYPE_EXE: u16 = 0x0001;
    pub const SECURITY_MASK: u16 = 0x0030;
    pub const SECURITY_S: u16 = 0x0020;
    pub const CPU_MASK: u16 = 0x0700;
    pub const CPU_ARM: u16 = 0x0000;
    pub const CHIP_MASK: u16 = 0x7000;
    pub const CHIP_RP2350: u16 = 0x1000;
    pub const TBYB: u16 = 0x8000;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ImageError {
    /// No well-formed block with an `IMAGE_TYPE` item in the first [`SEARCH_LEN`] bytes.
    NoImageDef,
    /// An image, but not a Secure Arm executable for the RP2350.
    WrongKind(u16),
}

/// Marks an RP2354B image, as a static anywhere in it: the firmware version and the hardware major version of the
/// board it is built for, so that an update for another board is refused (DEVELOPMENT.md "Versions").
#[repr(C)]
pub struct FwId {
    pub magic: [u8; 8],
    pub version: u32,
    pub hardware_major: u32,
}

pub const FW_ID_MAGIC: [u8; 8] = *b"KVMRP235";

impl FwId {
    pub const fn new(version: u32, hardware_major: u8) -> FwId {
        FwId { magic: FW_ID_MAGIC, version, hardware_major: hardware_major as u32 }
    }
}

/// The image's [`FwId`]: its firmware version and the hardware major version it is for.
pub fn fw_id(image: &[u8]) -> Option<(u32, u8)> {
    // A word-aligned static: the magic, the version, the hardware major.
    let at = (0..image.len().saturating_sub(15)).step_by(4).find(|&i| image[i..].starts_with(&FW_ID_MAGIC))?;
    Some((word(image, at / 4 + 2)?, word(image, at / 4 + 3)? as u8))
}

fn word(b: &[u8], i: usize) -> Option<u32> {
    b.get(4 * i..4 * i + 4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
}

/// The first well-formed block in the first [`SEARCH_LEN`] bytes of `image` that `pick` finds something in: `pick`
/// sees each item (its word index and header word) and keeps the first answer.
fn first_block<T>(image: &[u8], mut pick: impl FnMut(&[u8], usize, u32) -> Option<T>) -> Option<T> {
    let head = &image[..image.len().min(SEARCH_LEN)];
    let words = head.len() / 4;
    for start in 0..words {
        if word(head, start) != Some(BLOCK_START) {
            continue;
        }
        // Walk the items up to LAST; each states its size in words, header included.
        let mut i = start + 1;
        let mut found = None;
        while let Some(w) = word(head, i) {
            let size = ((w >> 8) & if w & SIZE_FLAG != 0 { 0xFFFF } else { 0xFF }) as usize;
            if w as u8 == ITEM_LAST {
                // LAST, then the link to the next block and the end marker.
                if found.is_some() && word(head, i + 2) == Some(BLOCK_END) && size == i - start - 1 {
                    return found;
                }
                break;
            }
            if size == 0 {
                break;
            }
            if found.is_none() {
                found = pick(head, i, w);
            }
            i += size;
        }
    }
    None
}

/// Byte offset of the `IMAGE_TYPE` item of the first block in `image`, and its flags.
pub fn image_type(image: &[u8]) -> Result<(usize, u16), ImageError> {
    first_block(image, |_, i, w| (w as u8 == ITEM_IMAGE_TYPE).then_some((4 * i, (w >> 16) as u16)))
        .ok_or(ImageError::NoImageDef)
}

/// The version in the image's `VERSION` item (major << 16 | minor << 8 | patch, as the RP2354B's firmware writes it
/// and [`fw_id`] reports it), if it has one: readable from the image's first 4 KB, as in the boot ROM's bootloader.
pub fn image_version(image: &[u8]) -> Option<u32> {
    first_block(image, |head, i, w| if w as u8 == ITEM_VERSION { word(head, i + 1) } else { None })
}

/// Checks that `image` is a Secure Arm executable for the RP2350; returns its `IMAGE_TYPE` flags.
pub fn check_rp2350_arm(image: &[u8]) -> Result<u16, ImageError> {
    use image_type::*;
    let (_, flags) = image_type(image)?;
    let want = TYPE_EXE | SECURITY_S | CPU_ARM | CHIP_RP2350;
    if flags & (TYPE_MASK | SECURITY_MASK | CPU_MASK | CHIP_MASK) != want {
        return Err(ImageError::WrongKind(flags));
    }
    Ok(flags)
}

/// Sets the *try before you buy* flag in the image's `IMAGE_TYPE` item (the image carries no hash over it).
pub fn set_tbyb(image: &mut [u8]) -> Result<(), ImageError> {
    check_rp2350_arm(image)?;
    let (at, _) = image_type(image)?;
    image[at + 3] |= (image_type::TBYB >> 8) as u8;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(words: &[u32]) -> std::vec::Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// The datasheet's minimum Arm IMAGE_DEF (§5.9.5.1), behind a vector table.
    fn minimal() -> std::vec::Vec<u8> {
        let mut w = std::vec![0x2008_2000, 0x1000_00F9];
        w.extend_from_slice(&[BLOCK_START, 0x1021_0142, 0x0000_01ff, 0, BLOCK_END]);
        bytes(&w)
    }

    #[test]
    fn finds_and_flags_the_image_def() {
        let mut img = minimal();
        assert_eq!(check_rp2350_arm(&img), Ok(0x1021));
        set_tbyb(&mut img).unwrap();
        assert_eq!(image_type(&img), Ok((12, 0x9021)));
    }

    #[test]
    fn with_a_version_item() {
        let img = bytes(&[BLOCK_START, 0x1021_0142, 0x0000_0248, 0x0001_0000, 0x0000_03ff, 0, BLOCK_END]);
        assert_eq!(check_rp2350_arm(&img), Ok(0x1021));
        assert_eq!(image_version(&img), Some(0x0001_0000));
        assert_eq!(image_version(&minimal()), None);
        // Erased flash: nothing.
        assert_eq!(image_version(&[0xFF; 4096]), None);
    }

    #[test]
    fn finds_the_firmware_id() {
        let mut img = minimal();
        assert_eq!(fw_id(&img), None);
        img.extend_from_slice(&[0; 4]);
        img.extend_from_slice(&FW_ID_MAGIC);
        img.extend(bytes(&[0x0001_0200, 1]));
        assert_eq!(fw_id(&img), Some((0x0001_0200, 1)));
    }

    #[test]
    fn rejects_others() {
        assert_eq!(check_rp2350_arm(&[0u8; 64]), Err(ImageError::NoImageDef));
        // RISC-V image.
        let img = bytes(&[BLOCK_START, 0x1101_0142, 0x0000_01ff, 0, BLOCK_END]);
        assert_eq!(check_rp2350_arm(&img), Err(ImageError::WrongKind(0x1101)));
        // Truncated block: no end marker.
        let img = bytes(&[BLOCK_START, 0x1021_0142, 0x0000_01ff, 0]);
        assert_eq!(check_rp2350_arm(&img), Err(ImageError::NoImageDef));
    }
}
