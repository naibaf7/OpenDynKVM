//! Puts memory.x where the linker finds it, and embeds the AT32F405 firmware (software.md §4.9) and the Bluetooth
//! module's (software.md §4.13).
//!
//! The module's image is its whole flash from offset 0 (bootloader, partition table, application), as `cargo xtask
//! dist` writes it with espflash (`KVM_ESP_IMAGE`, or `../esp32c3/target/.../release/kvm-esp32c3.bin`). It is stored
//! zlib-compressed, as the module's ROM bootloader takes it (FLASH_DEFL_*), with its length and MD5 for the check
//! after writing. It is split after its first 4 KB, which `espisp` writes last: a module whose programming broke off
//! still has an erased first sector, and so is still known as blank (software.md §4.13). Without it the RP2354B
//! firmware still builds, and uses a module as it finds it.
//!
//! The AT32F405 image comes from its release ELF: `KVM_AT32_ELF`, or `../at32f405/target/.../release/kvm-at32f405`
//! by default. Build that first (`cargo xtask dist` does both in order). Without it the RP2354B firmware still builds,
//! but cannot (re)program the AT32F405s.

use std::{env, fs, path::PathBuf};

const AT32_FLASH: std::ops::Range<u32> = 0x0800_0000..0x0804_0000;
const FW_ID_MAGIC: &[u8; 8] = b"KVMAT32\x01";

fn u16_at(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The flash contents described by an ELF32 little-endian file: every loadable segment at its load address.
fn flash_image(elf: &[u8]) -> Result<Vec<u8>, String> {
    if elf.len() < 52 || &elf[..4] != b"\x7fELF" || elf[4] != 1 || elf[5] != 1 {
        return Err("not a 32-bit little-endian ELF".into());
    }
    let (phoff, phentsize, phnum) = (u32_at(elf, 0x1C) as usize, u16_at(elf, 0x2A), u16_at(elf, 0x2C));
    let mut image = Vec::new();
    for i in 0..phnum {
        let ph = phoff + i * phentsize;
        let (p_type, offset, paddr, filesz) =
            (u32_at(elf, ph), u32_at(elf, ph + 4) as usize, u32_at(elf, ph + 12), u32_at(elf, ph + 16) as usize);
        const PT_LOAD: u32 = 1;
        if p_type != PT_LOAD || filesz == 0 {
            continue;
        }
        if !AT32_FLASH.contains(&paddr) || !AT32_FLASH.contains(&(paddr + filesz as u32 - 1)) {
            return Err(format!("segment at {paddr:#010x} lies outside the AT32F405 flash"));
        }
        let at = (paddr - AT32_FLASH.start) as usize;
        if image.len() < at + filesz {
            image.resize(at + filesz, 0xFF);
        }
        image[at..at + filesz].copy_from_slice(elf.get(offset..offset + filesz).ok_or("truncated ELF")?);
    }
    // The ROM bootloader writes whole words.
    image.resize(image.len().next_multiple_of(4), 0xFF);
    Ok(image)
}

fn fw_version(image: &[u8]) -> Option<u32> {
    // `FwId` is a word-aligned static: magic, then the version.
    let at = (0..image.len().saturating_sub(11)).step_by(4).find(|&i| image[i..].starts_with(FW_ID_MAGIC))?;
    Some(u32_at(image, at + 8))
}

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    let elf = env::var_os("KVM_AT32_ELF")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../at32f405/target/thumbv7em-none-eabihf/release/kvm-at32f405"));
    println!("cargo:rerun-if-env-changed=KVM_AT32_ELF");
    println!("cargo:rerun-if-changed={}", elf.display());
    let (image, version) = match fs::read(&elf).map_err(|e| e.to_string()).and_then(|b| flash_image(&b)) {
        Ok(image) => match fw_version(&image) {
            Some(v) => (image, Some(v)),
            None => {
                println!("cargo:warning={}: no firmware ID marker; not embedding it", elf.display());
                (Vec::new(), None)
            }
        },
        Err(e) => {
            println!("cargo:warning=no AT32F405 firmware embedded ({}: {e})", elf.display());
            (Vec::new(), None)
        }
    };
    fs::write(out.join("at32.bin"), &image).unwrap();
    fs::write(
        out.join("at32_fw.rs"),
        format!("/// Version of the embedded AT32F405 image (None: not embedded).\npub const AT32_FW_VERSION: Option<u32> = {version:?};\n"),
    )
    .unwrap();

    esp_image(&out);
}

const ESP_FW_ID_MAGIC: &[u8; 8] = b"KVMESP32";
/// The module's first flash sector: written last, and erased first.
const ESP_HEAD: usize = 4096;

/// The module's image, compressed, and what the firmware needs to know of it.
fn esp_image(out: &std::path::Path) {
    use std::io::Write as _;
    let path = env::var_os("KVM_ESP_IMAGE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../esp32c3/target/riscv32imc-unknown-none-elf/release/kvm-esp32c3.bin"));
    println!("cargo:rerun-if-env-changed=KVM_ESP_IMAGE");
    println!("cargo:rerun-if-changed={}", path.display());
    let image = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            println!("cargo:warning=no Bluetooth module firmware embedded ({}: {e})", path.display());
            Vec::new()
        }
    };
    // `FwId` of the module's firmware: magic, then the version (anywhere in the image: its data is stored as is).
    let version = image.windows(12).find(|w| w.starts_with(ESP_FW_ID_MAGIC)).map(|w| u32_at(w, 8));
    if !image.is_empty() && version.is_none() {
        println!("cargo:warning={}: no firmware ID marker", path.display());
    }
    let zlib = |part: &[u8]| {
        if part.is_empty() {
            return Vec::new();
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(part).unwrap();
        z.finish().unwrap()
    };
    let md5 = |data: &[u8]| {
        use md5::Digest as _;
        let d = md5::Md5::digest(data);
        d.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let head_len = image.len().min(ESP_HEAD);
    fs::write(out.join("esp_head.bin.z"), zlib(&image[..head_len])).unwrap();
    fs::write(out.join("esp_tail.bin.z"), zlib(&image[head_len..])).unwrap();
    let blank = md5(&[0xFF; ESP_HEAD]);
    let md5 = md5(&image);
    fs::write(
        out.join("esp_fw.rs"),
        format!(
            "/// Version of the embedded Bluetooth module image (None: not embedded).\n\
             pub const ESP_FW_VERSION: Option<u32> = {version:?};\n\
             /// Its size uncompressed, and its MD5 as the module's ROM reports it.\n\
             pub const ESP_IMAGE_LEN: u32 = {};\n\
             pub const ESP_IMAGE_MD5: &[u8; 32] = b\"{md5}\";\n\
             /// Length of the part written last (the first sector), and the MD5 of that sector erased.\n\
             pub const ESP_HEAD_LEN: u32 = {head_len};\n\
             pub const ESP_BLANK_MD5: &[u8; 32] = b\"{blank}\";\n",
            image.len()
        ),
    )
    .unwrap();
}
