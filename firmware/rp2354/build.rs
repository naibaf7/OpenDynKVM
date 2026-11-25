//! Puts memory.x where the linker finds it, embeds the AT32F405 firmware (software.md §4.9), and the SHA-256 of the
//! Bluetooth module's, one per chip (software.md §4.13): the host software streams the one for the module's chip through
//! to the module, and the RP2354B takes only these (`modstream.rs`). They come from `KVM_MODULE_IMAGE_ESP32C3` and
//! `KVM_MODULE_IMAGE_ESP32`, or `cargo xtask dist`'s `../esp32c3/target/.../release/kvm-module-esp32c3.bin` and
//! `../esp32/target/.../release/kvm-module-esp32.bin`; without one the firmware updates no module of that chip.
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

    module_hash(&out);
}

/// The SHA-256 of each Bluetooth module image (`kvm_proto::modimage`, one per chip) this firmware takes.
fn module_hash(out: &std::path::Path) {
    use sha2::Digest as _;
    let images = [
        ("ESP32C3", "../esp32c3/target/riscv32imc-unknown-none-elf/release/kvm-module-esp32c3.bin"),
        ("ESP32", "../esp32/target/xtensa-esp32-none-elf/release/kvm-module-esp32.bin"),
    ];
    let mut code = String::new();
    for (chip, default) in images {
        let var = format!("KVM_MODULE_IMAGE_{chip}");
        let path = env::var_os(&var).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default));
        println!("cargo:rerun-if-env-changed={var}");
        println!("cargo:rerun-if-changed={}", path.display());
        let hash = match fs::read(&path) {
            Ok(file) => Some(<[u8; 32]>::from(sha2::Sha256::digest(&file))),
            Err(e) => {
                println!(
                    "cargo:warning=no Bluetooth module image for the {chip} ({}: {e}): this firmware updates no such \
                     module",
                    path.display()
                );
                None
            }
        };
        code += &format!(
            "/// SHA-256 of the {chip} module image this firmware takes (None: built without one).\npub const \
             MODULE_SHA256_{chip}: Option<[u8; 32]> = {hash:?};\n"
        );
    }
    fs::write(out.join("module_fw.rs"), code).unwrap();
}
