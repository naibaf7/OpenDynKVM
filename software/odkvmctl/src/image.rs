//! Firmware files: the raw RP2354B image (`kvm-rp2354.bin` from `cargo xtask dist`), its UF2 (`kvm-rp2354.uf2`, for
//! the boot ROM's drive) and the first-install UF2 (`kvm-first.uf2`: partition table and partition A). `odkvmctl
//! update` and the window send the image over a PC port, or copy a UF2 onto the boot ROM's drive (`bootsel`).

use kvm_proto::rpimage::{self, ImageError, layout};

use crate::{fl, names};

const UF2_MAGIC: [u32; 2] = [0x0A32_4655, 0x9E5D_5157];
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
/// UF2 family IDs the RP2350's boot ROM knows (datasheet Table 469).
pub const UF2_FAMILY_ABSOLUTE: u32 = 0xe48b_ff57;
pub const UF2_FAMILY_RP2350_ARM_S: u32 = 0xe48b_ff59;
pub const XIP_BASE: u32 = 0x1000_0000;
/// Where partition A starts, as the boot ROM's bootloader addresses flash.
const A_XIP: u32 = XIP_BASE + layout::PARTITION_A_SECTOR * layout::SECTOR;
/// One A/B partition.
const MAX_IMAGE: usize = (layout::PARTITION_SECTORS * layout::SECTOR) as usize;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// A UF2 file (one family, as the RP2350 ROM wants): `regions` are (flash address, bytes), 256 bytes a block.
pub fn uf2(regions: &[(u32, &[u8])], family: u32) -> Vec<u8> {
    const PAYLOAD: usize = 256;
    let blocks: Vec<(u32, &[u8])> = regions
        .iter()
        .flat_map(|(addr, data)| data.chunks(PAYLOAD).enumerate().map(move |(i, c)| (addr + (i * PAYLOAD) as u32, c)))
        .collect();
    let mut out = Vec::with_capacity(blocks.len() * 512);
    for (n, (addr, chunk)) in blocks.iter().enumerate() {
        let mut b = [0u8; 512];
        let mut put = |at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(0, UF2_MAGIC[0]);
        put(4, UF2_MAGIC[1]);
        put(8, 0x0000_2000); // family ID present
        put(12, *addr);
        put(16, PAYLOAD as u32);
        put(20, n as u32);
        put(24, blocks.len() as u32);
        put(28, family);
        put(508, UF2_MAGIC_END);
        b[32..32 + chunk.len()].copy_from_slice(chunk);
        b[32 + chunk.len()..32 + PAYLOAD].fill(0xFF);
        out.extend_from_slice(&b);
    }
    out
}

/// The UF2 for the boot ROM's drive that an update image makes (what `kvm-rp2354.uf2` holds).
pub fn update_uf2(image: &[u8]) -> Vec<u8> {
    uf2(&[(XIP_BASE, image)], UF2_FAMILY_RP2350_ARM_S)
}

pub fn is_uf2(file: &[u8]) -> bool {
    file.len() >= 8 && [u32_at(file, 0), u32_at(file, 4)] == UF2_MAGIC
}

/// The family of a UF2's blocks, if it is a UF2 that names one.
pub fn uf2_family(file: &[u8]) -> Option<u32> {
    (is_uf2(file) && file.len() >= 512 && u32_at(file, 8) & 0x2000 != 0).then(|| u32_at(file, 28))
}

/// The blocks of a UF2 of `family`: (address, payload).
fn uf2_blocks(file: &[u8], family: u32) -> Result<Vec<(u32, &[u8])>, String> {
    let mut out = Vec::new();
    for b in file.chunks(512) {
        if b.len() < 512 || [u32_at(b, 0), u32_at(b, 4)] != UF2_MAGIC {
            return Err(fl!("img-damaged-uf2"));
        }
        let (flags, addr, len, fam) = (u32_at(b, 8), u32_at(b, 12), u32_at(b, 16) as usize, u32_at(b, 28));
        if flags & 0x2000 == 0 || fam != family {
            return Err(if fam == UF2_FAMILY_ABSOLUTE {
                fl!("img-uf2-partition-table")
            } else {
                fl!("img-uf2-other-chip", family = format!("{fam:#010x}"))
            });
        }
        if len > 476 {
            return Err(fl!("img-uf2-out-of-range", address = format!("{addr:#010x}")));
        }
        out.push((addr, &b[32..32 + len]));
    }
    Ok(out)
}

/// The bytes the blocks put from `base` on, within `max` bytes.
fn assemble(blocks: &[(u32, &[u8])], base: u32, max: usize) -> Result<Vec<u8>, String> {
    let mut image = Vec::new();
    for (addr, data) in blocks {
        let at = addr.checked_sub(base).map(|a| a as usize).filter(|a| a + data.len() <= max);
        let Some(at) = at else {
            return Err(fl!("img-uf2-out-of-range", address = format!("{addr:#010x}")));
        };
        if image.len() < at + data.len() {
            image.resize(at + data.len(), 0xFF);
        }
        image[at..at + data.len()].copy_from_slice(data);
    }
    Ok(image)
}

fn from_uf2(file: &[u8]) -> Result<Vec<u8>, String> {
    assemble(&uf2_blocks(file, UF2_FAMILY_RP2350_ARM_S)?, XIP_BASE, MAX_IMAGE)
}

/// The firmware image a first-install UF2 (`kvm-first.uf2`: absolute addresses) puts into partition A; it must also
/// carry the partition table (blocks before A).
pub fn first_image(file: &[u8]) -> Result<Vec<u8>, String> {
    let blocks = uf2_blocks(file, UF2_FAMILY_ABSOLUTE)
        .map_err(|e| if uf2_family(file) == Some(UF2_FAMILY_RP2350_ARM_S) { fl!("img-not-first") } else { e })?;
    if !blocks.iter().any(|(a, _)| (XIP_BASE..A_XIP).contains(a)) {
        return Err(fl!("img-not-first"));
    }
    let image: Vec<(u32, &[u8])> = blocks.into_iter().filter(|(a, _)| *a >= A_XIP).collect();
    assemble(&image, A_XIP, MAX_IMAGE)
}

/// The image from a `.bin` or an update UF2.
pub fn update_image(file: &[u8]) -> Result<Vec<u8>, String> {
    if is_uf2(file) { from_uf2(file) } else { Ok(file.to_vec()) }
}

/// Checks that `image` is RP2354B firmware; returns its version and the hardware major version it is for.
pub fn check_image(image: &[u8]) -> Result<(u32, u8), String> {
    if image.len() > MAX_IMAGE {
        let (kb, max) = (image.len() / 1024, MAX_IMAGE / 1024);
        return Err(fl!("img-too-big", kb = kb, max = max));
    }
    match rpimage::check_rp2350_arm(image) {
        Ok(_) => {}
        Err(ImageError::NoImageDef) => return Err(fl!("img-not-rp2354b")),
        Err(ImageError::WrongKind(f)) => return Err(fl!("img-wrong-kind", flags = format!("{f:#06x}"))),
    }
    rpimage::fw_id(image).ok_or_else(|| fl!("img-no-fw-id"))
}

/// The image to upload and its firmware version, checked to be RP2354B firmware for the board `hw` (the KVM's
/// `Info::hw`): each hardware major version is a different board with its own image (DEVELOPMENT.md "Versions").
pub fn load_update(file: &[u8], hw: u32) -> Result<(Vec<u8>, u32), String> {
    let image = update_image(file)?;
    let (version, major) = check_image(&image)?;
    check_hardware(version, major, hw)?;
    Ok((image, version))
}

/// Refuses firmware built for another board than `hw`.
pub fn check_hardware(version: u32, major: u8, hw: u32) -> Result<(), String> {
    if u32::from(major) != hw >> 16 {
        return Err(fl!(
            "img-other-hardware",
            version = names::version(version),
            major = major,
            hw = names::version(hw)
        ));
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A minimal RP2354B image: firmware 0.1.0 for hardware 0.x.
    pub(crate) fn firmware() -> Vec<u8> {
        let words = [0x2008_0000u32, 0x1000_0149, 0xffff_ded3, 0x1021_0142, 0x0000_01ff, 0, 0xab12_3579];
        let mut b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        b.resize(600, 0x11);
        b.extend_from_slice(&rpimage::FW_ID_MAGIC);
        b.extend([0x0000_0100u32, 0].iter().flat_map(|w| w.to_le_bytes()));
        b
    }

    /// `kvm-first.uf2` for `image`: a stand-in partition table, then the image in A.
    pub(crate) fn first_uf2(image: &[u8]) -> Vec<u8> {
        uf2(&[(XIP_BASE, &[0x5A; 128]), (A_XIP, image)], UF2_FAMILY_ABSOLUTE)
    }

    const HW: u32 = 0x00_0C_00;

    #[test]
    fn uf2_blocks_carry_256_bytes_each() {
        let data = [0xAAu8; 300];
        let f = uf2(&[(XIP_BASE, &data)], UF2_FAMILY_RP2350_ARM_S);
        assert_eq!(f.len(), 1024);
        assert_eq!(u32_at(&f, 512 + 12), XIP_BASE + 256);
        assert_eq!((u32_at(&f, 20), u32_at(&f, 24), u32_at(&f, 512 + 20)), (0, 2, 1));
        assert_eq!((u32_at(&f, 508), u32_at(&f, 28)), (UF2_MAGIC_END, UF2_FAMILY_RP2350_ARM_S));
        assert_eq!(f[512 + 32 + 44], 0xFF, "the rest of the last block is padding");
        assert_eq!(uf2_family(&f), Some(UF2_FAMILY_RP2350_ARM_S));
        assert_eq!(uf2_family(&data), None);
    }

    #[test]
    fn raw_and_uf2_give_the_same_image() {
        let fw = firmware();
        assert_eq!(load_update(&fw, HW).unwrap(), (fw.clone(), 0x100));
        let (from_uf2, _) = load_update(&update_uf2(&fw), HW).unwrap();
        assert_eq!(&from_uf2[..fw.len()], &fw[..]);
    }

    #[test]
    fn the_first_install_uf2_carries_the_table_and_the_image() {
        let fw = firmware();
        let img = first_image(&first_uf2(&fw)).unwrap();
        assert_eq!(&img[..fw.len()], &fw[..]);
        assert_eq!(check_image(&img), Ok((0x100, 0)));
        // An update UF2 is not one; nor is an absolute UF2 without a partition table.
        assert!(first_image(&update_uf2(&fw)).unwrap_err().contains("first"));
        assert!(first_image(&uf2(&[(A_XIP, &fw)], UF2_FAMILY_ABSOLUTE)).unwrap_err().contains("first"));
    }

    #[test]
    fn refuses_other_files() {
        assert!(load_update(&[0u8; 4096], HW).is_err());
        assert!(load_update(&first_uf2(&firmware()), HW).unwrap_err().contains("partition table"));
        // An RP2350 image that isn't ours.
        let other = &firmware()[..600];
        assert!(load_update(other, HW).unwrap_err().contains("no firmware ID"));
    }

    #[test]
    fn refuses_firmware_for_another_board() {
        let e = load_update(&firmware(), 0x01_00_00).unwrap_err();
        assert_eq!(e, "firmware 0.1.0 is for hardware 0.x, this KVM is hardware 1.0.0");
        // Revisions of one board take the same image.
        assert!(load_update(&firmware(), 0x00_0D_02).is_ok());
    }
}
