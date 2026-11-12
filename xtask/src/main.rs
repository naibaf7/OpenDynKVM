//! `cargo xtask dist [--out DIR]`: builds the three firmwares and writes everything needed to flash and update a KVM
//! (software.md §4.9, §4.13, §10):
//!
//! | File | Use |
//! |---|---|
//! | `kvm-first.uf2` | Blank board: copy onto the `RP2350` drive. Partition table plus the firmware in partition A. |
//! | `kvm-rp2354.uf2` | Board with the partition table: copy onto the drive; the ROM writes the partition not booted. |
//! | `kvm-rp2354.bin` | `odkvmctl update kvm-rp2354.bin` |
//! | `kvm-at32f405.bin` | AT32F405 flash image at 0x0800_0000 (USB DFU or SWD; normally the RP2354B flashes it itself) |
//! | `kvm-esp32c3.bin` | The Bluetooth module's flash from offset 0 (bootloader, partition table, firmware), as the RP2354B writes it; for `espflash write-bin 0x0` on the bench |
//! | `pt.json` | The partition table for `picotool partition create` |
//!
//! The Bluetooth module's image is made with espflash (`cargo install espflash`); without espflash the RP2354B firmware
//! is built without it and uses a module as it finds it.
//!
//! `cargo xtask package [--out DIR] [--native]` builds the host software's installers instead (`package.rs`), after a
//! `dist` into the same folder: each package carries that firmware (`kvm-rp2354.bin`, `kvm-rp2354.uf2`,
//! `kvm-first.uf2`), which the window and `odkvmctl update` install by default.
//!
//! Run it with rustup's cargo (`~/.cargo/bin/cargo xtask dist`): the firmwares need the thumb targets.

mod package;
mod pt;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail, ensure};

const AT32_FLASH: (u32, u32) = (0x0800_0000, 256 * 1024);
const RP_XIP: u32 = 0x1000_0000;
/// A and B of 984 KB, then the 68 KB data partition, then the last sector left free (RP2350-E10); the same constants
/// tell the host software a KVM's partition table from another board's in the boot ROM's bootloader.
use kvm_proto::rpimage::layout::{DATA_SECTORS, PARTITION_A_SECTOR, PARTITION_SECTORS, SECTOR};
const PT_VERSION: (u16, u16) = (1, 0);

// The host software builds the same UF2s (an update UF2 from a `.bin`, the boot ROM's drive): one writer for both.
use odkvmctl::image::{UF2_FAMILY_ABSOLUTE, UF2_FAMILY_RP2350_ARM_S, uf2};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// `cargo build --release` in a firmware directory, with its own toolchain file and target directory.
fn build_firmware(dir: &Path, envs: &[(&str, &Path)]) -> Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.current_dir(dir).args(["build", "--release", "--target-dir"]).arg(dir.join("target"));
    // Set by the cargo running xtask; they would override the firmware's own toolchain and flags.
    for v in ["RUSTUP_TOOLCHAIN", "CARGO_TARGET_DIR", "RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET"] {
        cmd.env_remove(v);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let status = cmd.status().with_context(|| format!("running cargo in {}", dir.display()))?;
    ensure!(status.success(), "firmware build in {} failed", dir.display());
    Ok(())
}

fn u16_at(b: &[u8], at: usize) -> usize {
    u16::from_le_bytes([b[at], b[at + 1]]) as usize
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Flash contents of an ELF32 little-endian file: every loadable segment at its load address, relative to
/// `base`, which the image must not leave.
fn flash_image(elf: &[u8], base: u32, len: u32) -> Result<Vec<u8>> {
    ensure!(elf.len() >= 52 && elf[..4] == *b"\x7fELF" && elf[4] == 1 && elf[5] == 1, "not an ELF32 LE file");
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
        let at = paddr.wrapping_sub(base) as usize;
        ensure!(paddr >= base && at + filesz <= len as usize, "segment at {paddr:#010x} is outside the flash");
        if image.len() < at + filesz {
            image.resize(at + filesz, 0xFF);
        }
        image[at..at + filesz].copy_from_slice(elf.get(offset..offset + filesz).context("truncated ELF")?);
    }
    image.resize(image.len().next_multiple_of(4), 0xFF);
    Ok(image)
}

fn partitions() -> [pt::Partition; 3] {
    let (a, n) = (PARTITION_A_SECTOR, PARTITION_SECTORS);
    let fam = pt::ACCEPTS_RP2350_ARM_S;
    let data = a + 2 * n;
    [
        pt::Partition { name: "A", id: 0, first_sector: a, last_sector: a + n - 1, flags: fam, link_a: None },
        pt::Partition {
            name: "B",
            id: 1,
            first_sector: a + n,
            last_sector: a + 2 * n - 1,
            flags: fam,
            link_a: Some(0),
        },
        // Config, keymap and macros (kvm_core::store). No UF2 family lands here, and the ROM never boots it.
        pt::Partition {
            name: "data",
            id: 2,
            first_sector: data,
            last_sector: data + DATA_SECTORS - 1,
            flags: pt::NOT_BOOTABLE_ARM | pt::NOT_BOOTABLE_RISCV,
            link_a: None,
        },
    ]
}

fn dist(out: &Path) -> Result<()> {
    let root = root();
    let at32_dir = root.join("firmware/at32f405");
    let rp_dir = root.join("firmware/rp2354");
    let at32_elf = at32_dir.join("target/thumbv7em-none-eabihf/release/kvm-at32f405");
    let rp_elf = rp_dir.join("target/thumbv8m.main-none-eabihf/release/kvm-rp2354");

    build_firmware(&at32_dir, &[])?;
    // The Bluetooth module's firmware, and its flash image (espflash: bootloader + partition table + application).
    let esp_dir = root.join("firmware/esp32c3");
    let esp_elf = esp_dir.join("target/riscv32imc-unknown-none-elf/release/kvm-esp32c3");
    let esp_bin = esp_dir.join("target/riscv32imc-unknown-none-elf/release/kvm-esp32c3.bin");
    build_firmware(&esp_dir, &[])?;
    let espflash = env::var_os("ESPFLASH").map(PathBuf::from).unwrap_or_else(|| {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let local = home.join(".cargo/bin/espflash");
        if local.exists() { local } else { PathBuf::from("espflash") }
    });
    let status = Command::new(&espflash)
        .args(["save-image", "--chip", "esp32c3", "--merge", "--flash-size", "4mb", "--skip-padding"])
        .arg(&esp_elf)
        .arg(&esp_bin)
        .status();
    let esp = match status {
        Ok(s) if s.success() => fs::read(&esp_bin)?,
        _ => {
            println!("warning: espflash failed or is missing: the RP2354B firmware carries no Bluetooth module image");
            let _ = fs::remove_file(&esp_bin);
            Vec::new()
        }
    };
    // The RP2354B firmware embeds the AT32F405 image and the module's (firmware/rp2354/build.rs).
    build_firmware(&rp_dir, &[("KVM_AT32_ELF", &at32_elf), ("KVM_ESP_IMAGE", &esp_bin)])?;

    let at32 = flash_image(&fs::read(&at32_elf)?, AT32_FLASH.0, AT32_FLASH.1)?;
    let rp = flash_image(&fs::read(&rp_elf)?, RP_XIP, PARTITION_SECTORS * SECTOR)?;
    if let Err(e) = kvm_proto::rpimage::check_rp2350_arm(&rp) {
        bail!("the RP2354B image has no usable IMAGE_DEF: {e:?}");
    }
    // `odkvmctl update` refuses an image without it, or for another board.
    let (version, hw_major) = kvm_proto::rpimage::fw_id(&rp).context("the RP2354B image has no firmware ID (FwId)")?;

    let parts = partitions();
    let table: Vec<u8> =
        pt::block(pt::ACCEPTS_ABSOLUTE, &parts, PT_VERSION).iter().flat_map(|w| w.to_le_bytes()).collect();
    let a_start = RP_XIP + PARTITION_A_SECTOR * SECTOR;

    fs::create_dir_all(out)?;
    let files: [(&str, Vec<u8>); 6] = [
        ("kvm-first.uf2", uf2(&[(RP_XIP, &table), (a_start, &rp)], UF2_FAMILY_ABSOLUTE)),
        ("kvm-rp2354.uf2", uf2(&[(RP_XIP, &rp)], UF2_FAMILY_RP2350_ARM_S)),
        ("kvm-rp2354.bin", rp.clone()),
        ("kvm-at32f405.bin", at32.clone()),
        ("kvm-esp32c3.bin", esp.clone()),
        ("pt.json", pt::json(pt::ACCEPTS_ABSOLUTE, &parts, PT_VERSION).into_bytes()),
    ];
    for (name, data) in &files {
        fs::write(out.join(name), data)?;
        println!("{:>10} bytes  {}", data.len(), out.join(name).display());
    }
    println!(
        "firmware {} for hardware {hw_major}.x: RP2354B image {} KB of {} KB, AT32F405 image {} KB, Bluetooth module \
         image {} KB",
        odkvmctl::names::version(version),
        rp.len() / 1024,
        PARTITION_SECTORS * 4,
        at32.len() / 1024,
        esp.len() / 1024
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["dist"] => dist(&root().join("dist")),
        ["dist", "--out", dir] => dist(Path::new(dir)),
        ["package", rest @ ..] => {
            let mut opt = package::Options { out: root().join("dist"), native: false };
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match *a {
                    "--native" => opt.native = true,
                    "--out" => opt.out = PathBuf::from(it.next().context("--out needs a directory")?),
                    other => bail!("unknown option {other}"),
                }
            }
            fs::create_dir_all(&opt.out)?;
            // The packages carry the firmware of the same tree: built first, into the same folder.
            dist(&opt.out)?;
            package::package(&root(), &opt)
        }
        ["desktop"] => package::desktop(&root(), false),
        ["desktop", "--remove"] => package::desktop(&root(), true),
        _ => {
            eprintln!(
                "usage: cargo xtask dist [--out DIR]\n       cargo xtask package [--out DIR] [--native]\n       \
                 cargo xtask desktop [--remove]"
            );
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_matches_the_firmware() {
        // firmware/rp2354/src/flash.rs: A at 8 KB, 984 KB each, the data partition at 1976 KB up to the last sector.
        let p = partitions();
        assert_eq!((p[0].first_sector * 4, p[1].first_sector * 4, p[2].first_sector * 4), (8, 992, 1976));
        assert_eq!(((p[1].last_sector + 1) * 4, (p[2].last_sector + 1) * 4), (1976, 2044));
        let table = pt::block(pt::ACCEPTS_ABSOLUTE, &p, PT_VERSION);
        // Partition-table item: 3 partitions, each 2 words + 2 words of ID + the name.
        assert_eq!(table[1] & 0xFF, 0x0a);
        assert_eq!(table[1] >> 24, 3);
        assert!(pt::json(pt::ACCEPTS_ABSOLUTE, &p, PT_VERSION).contains("\"ignored_during_arm_boot\": true"));
    }
}
