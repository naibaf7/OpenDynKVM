//! `cargo xtask dist [--out DIR]`: builds the three firmwares and writes everything needed to flash and update a KVM
//! (software.md §4.9, §4.13, §10):
//!
//! | File | Use |
//! |---|---|
//! | `kvm-first.uf2` | Blank board, or one with the partition table of firmware before 0.1.51: copy onto the `RP2350` drive. Partition table and the firmware in partition A, B's start erased; the data partition is left as it is. |
//! | `kvm-rp2354.uf2` | Board with the partition table: copy onto the drive; the ROM writes the partition not booted. |
//! | `kvm-rp2354.bin` | `odkvmctl update kvm-rp2354.bin` |
//! | `kvm-module-esp32c3.bin`, `kvm-module-esp32.bin` | The Bluetooth module's firmware for each chip as the host software streams it to the module (`kvm_proto::modimage`); the RP2354B firmware takes only these (their SHA-256) |
//! | `kvm-at32f405.bin` | AT32F405 flash image at 0x0800_0000 (USB DFU or SWD; normally the RP2354B flashes it itself) |
//! | `kvm-esp32c3.bin`, `kvm-esp32.bin` | The Bluetooth module's flash from offset 0 (bootloader, partition table, firmware); for `espflash write-bin 0x0` on the bench |
//! | `pt.json` | The partition table for `picotool partition create` |
//!
//! The Bluetooth module's images are made with espflash (`cargo install espflash`), the ESP32's firmware with the Xtensa
//! toolchain (`espup install --targets esp32`); without either there is no image for that chip, and the RP2354B
//! firmware updates no such module.
//!
//! `cargo xtask package [--out DIR] [--native]` builds the host software's installers instead (`package.rs`), after a
//! `dist` into the same folder: each package carries that firmware (`kvm-rp2354.bin`, `kvm-module-*.bin`,
//! `kvm-rp2354.uf2`, `kvm-first.uf2`), which the window and `odkvmctl update` install by default.
//!
//! Run it with rustup's cargo (`~/.cargo/bin/cargo xtask dist`): the firmwares need the thumb targets.

mod package;
mod pt;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use anyhow::{Context, Result, bail, ensure};
use kvm_proto::link::bt::Chip;

const AT32_FLASH: (u32, u32) = (0x0800_0000, 256 * 1024);
const RP_XIP: u32 = 0x1000_0000;
/// A and B of 896 KB, the 244 KB data partition, then the last sector left free (RP2350-E10); the same constants
/// tell the host software a KVM's partition table from another board's in the boot ROM's bootloader.
use kvm_proto::rpimage::layout::{DATA_SECTOR, DATA_SECTORS, PARTITION_A_SECTOR, PARTITION_SECTORS, SECTOR};
/// Raised with the layout (0.1.51): of two tables in the first 8 KB, the boot ROM takes the higher version.
const PT_VERSION: (u16, u16) = (2, 0);

// The host software builds the same UF2s (an update UF2 from a `.bin`, the boot ROM's drive): one writer for both.
use odkvmctl::image::{UF2_FAMILY_RP2350_ARM_S, first_uf2, uf2};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// `cargo build --release` in a firmware directory, with its own toolchain file and target directory.
fn build_firmware(dir: &Path, envs: &[(&str, &std::ffi::OsStr)]) -> Result<()> {
    // The cargo running xtask is its toolchain's own, which ignores a firmware's `rust-toolchain.toml`; rustup's proxy
    // reads it (the ESP32's is the Xtensa toolchain, `esp`).
    let proxy = env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
        .map(|h| h.join("bin/cargo"))
        .filter(|c| c.exists());
    let cargo = proxy.map(Into::into).or_else(|| env::var_os("CARGO")).unwrap_or_else(|| "cargo".into());
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
            first_sector: DATA_SECTOR,
            last_sector: DATA_SECTOR + DATA_SECTORS - 1,
            flags: pt::NOT_BOOTABLE_ARM | pt::NOT_BOOTABLE_RISCV,
            link_a: None,
        },
    ]
}

/// The Bluetooth module's firmware as the host software streams it (`kvm_proto::modimage`), from `chip`'s flash image
/// `esp` (from offset 0): a header, then the image from the chip's bootloader offset (the rest after its first sector,
/// then that sector), each zlib-compressed as the module's ROM loader takes them.
fn module_image(esp: &[u8], chip: Chip) -> Result<(Vec<u8>, u32)> {
    use kvm_proto::modimage::{HEAD, Header};
    use std::io::Write as _;
    const MAGIC: &[u8; 8] = b"KVMESP32";
    // Its `FwId`: the magic, then the version (anywhere in the image: its data is stored as is).
    let version = esp
        .windows(12)
        .find(|w| w.starts_with(MAGIC))
        .map(|w| u32_at(w, 8))
        .context("the module's image has no firmware ID marker")?;
    // The ESP32's bootloader is at 0x1000: espflash's merged image pads the sector before it.
    let base: u32 = if chip == Chip::Esp32 { 0x1000 } else { 0 };
    ensure!(
        esp.len() > base as usize && esp[..base as usize].iter().all(|b| *b == 0xFF),
        "the {} image has something before its bootloader",
        chip.name()
    );
    let esp = &esp[base as usize..];
    let zlib = |part: &[u8]| -> Result<Vec<u8>> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        z.write_all(part)?;
        Ok(z.finish()?)
    };
    // The first sector as it is (`modimage`: the ESP32's ROM writes a second compressed region wrong).
    let split = esp.len().min(HEAD as usize);
    let (head, tail) = (esp[..split].to_vec(), if esp.len() > split { zlib(&esp[split..])? } else { Vec::new() });
    let md5: String = <md5::Md5 as md5::Digest>::digest(esp).iter().map(|b| format!("{b:02x}")).collect();
    let mut data = tail.clone();
    data.extend_from_slice(&head);
    let h = Header {
        version,
        len: esp.len() as u32,
        head_len: split as u32,
        head_stored: head.len() as u32,
        tail_z: tail.len() as u32,
        crc32: kvm_proto::crc::crc32(&data),
        md5: md5.as_bytes().try_into()?,
        chip: chip as u8,
        base,
    };
    let mut file = h.encode().to_vec();
    file.extend_from_slice(&data);
    ensure!(kvm_proto::modimage::check(&file) == Some(h), "the module image doesn't read back");
    Ok((file, version))
}

/// A Bluetooth module firmware: its chip, folder (under the repository) and target.
struct Esp {
    chip: Chip,
    dir: &'static str,
    triple: &'static str,
}

const ESPS: [Esp; 2] = [
    Esp { chip: Chip::Esp32C3, dir: "firmware/esp32c3", triple: "riscv32imc-unknown-none-elf" },
    Esp { chip: Chip::Esp32, dir: "firmware/esp32", triple: "xtensa-esp32-none-elf" },
];

/// The Xtensa toolchain's GCC (the ESP32's linker), which espup installs next to its Rust.
fn xtensa_gcc() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let root = home.join(".rustup/toolchains/esp/xtensa-esp-elf");
    let mut versions: Vec<PathBuf> = fs::read_dir(&root).ok()?.flatten().map(|e| e.path()).collect();
    versions.sort();
    let bin = versions.pop()?.join("xtensa-esp-elf/bin");
    bin.exists().then_some(bin)
}

/// A built module firmware: its flash image from offset 0, its streamed image and version, and where that is.
struct Module {
    chip: Chip,
    flash: Vec<u8>,
    image: Option<(Vec<u8>, u32)>,
    file: PathBuf,
}

/// Builds `esp`'s firmware, its flash image (espflash: bootloader, partition table, application) and its streamed
/// image. Without its toolchain or espflash: no image, with a warning.
fn build_module(root: &Path, esp: &Esp, espflash: &Path) -> Result<Module> {
    let dir = root.join(esp.dir);
    let name = format!("kvm-{}", esp.chip.name());
    let release = dir.join("target").join(esp.triple).join("release");
    let file = release.join(format!("kvm-module-{}.bin", esp.chip.name()));
    let none = |why: &str| {
        println!("warning: {why}: no Bluetooth module image for the {}", esp.chip.name());
        let _ = fs::remove_file(&file);
        Ok(Module { chip: esp.chip, flash: Vec::new(), image: None, file: file.clone() })
    };
    let mut envs: Vec<(&str, std::ffi::OsString)> = Vec::new();
    if esp.chip == Chip::Esp32 {
        let Some(gcc) = xtensa_gcc() else {
            return none("the Xtensa toolchain is missing (espup install --targets esp32)");
        };
        let mut path = std::ffi::OsString::from(gcc);
        path.push(":");
        path.push(env::var_os("PATH").unwrap_or_default());
        envs.push(("PATH", path));
    }
    let envs: Vec<(&str, &std::ffi::OsStr)> = envs.iter().map(|(k, v)| (*k, v.as_os_str())).collect();
    if esp.chip == Chip::Esp32 {
        if let Err(e) = build_firmware(&dir, &envs) {
            return none(&format!("{e}"));
        }
    } else {
        build_firmware(&dir, &envs)?;
    }
    let bin = release.join(format!("{name}.bin"));
    let status = Command::new(espflash)
        .args(["save-image", "--chip", esp.chip.name(), "--merge", "--flash-size", "4mb", "--skip-padding"])
        .arg(release.join(&name))
        .arg(&bin)
        .status();
    if !matches!(status, Ok(s) if s.success()) {
        let _ = fs::remove_file(&bin);
        return none("espflash failed or is missing");
    }
    let flash = fs::read(&bin)?;
    let image = module_image(&flash, esp.chip)?;
    fs::write(&file, &image.0)?;
    Ok(Module { chip: esp.chip, flash, image: Some(image), file })
}

fn dist(out: &Path) -> Result<()> {
    let root = root();
    let at32_dir = root.join("firmware/at32f405");
    let rp_dir = root.join("firmware/rp2354");
    let at32_elf = at32_dir.join("target/thumbv7em-none-eabihf/release/kvm-at32f405");
    let rp_elf = rp_dir.join("target/thumbv8m.main-none-eabihf/release/kvm-rp2354");

    build_firmware(&at32_dir, &[])?;
    // The Bluetooth module's firmwares, one per chip, and their images.
    let espflash = env::var_os("ESPFLASH").map(PathBuf::from).unwrap_or_else(|| {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let local = home.join(".cargo/bin/espflash");
        if local.exists() { local } else { PathBuf::from("espflash") }
    });
    let modules: Vec<Module> = ESPS.iter().map(|e| build_module(&root, e, &espflash)).collect::<Result<_>>()?;
    // The RP2354B firmware takes only these (their SHA-256) and embeds the AT32F405 image (firmware/rp2354/build.rs).
    let mut rp_env: Vec<(String, std::ffi::OsString)> = vec![("KVM_AT32_ELF".into(), at32_elf.clone().into())];
    for m in &modules {
        rp_env.push((format!("KVM_MODULE_IMAGE_{}", m.chip.name().to_uppercase()), m.file.clone().into()));
    }
    let rp_env: Vec<(&str, &std::ffi::OsStr)> = rp_env.iter().map(|(k, v)| (k.as_str(), v.as_os_str())).collect();
    build_firmware(&rp_dir, &rp_env)?;

    let at32 = flash_image(&fs::read(&at32_elf)?, AT32_FLASH.0, AT32_FLASH.1)?;
    let rp = flash_image(&fs::read(&rp_elf)?, RP_XIP, PARTITION_SECTORS * SECTOR)?;
    if let Err(e) = kvm_proto::rpimage::check_rp2350_arm(&rp) {
        bail!("the RP2354B image has no usable IMAGE_DEF: {e:?}");
    }
    // `odkvmctl update` refuses an image without it, or for another board.
    let (version, hw_major) = kvm_proto::rpimage::fw_id(&rp).context("the RP2354B image has no firmware ID (FwId)")?;
    for m in &modules {
        if let Some((_, v)) = &m.image {
            ensure!(*v == version, "the {} module's firmware is {v:#x}, the RP2354B's {version:#x}", m.chip.name());
        }
    }

    let parts = partitions();
    let table: Vec<u8> =
        pt::block(pt::ACCEPTS_ABSOLUTE, &parts, PT_VERSION).iter().flat_map(|w| w.to_le_bytes()).collect();
    fs::create_dir_all(out)?;
    let mut files: Vec<(&str, Vec<u8>)> = vec![
        ("kvm-first.uf2", first_uf2(&table, &rp)),
        ("kvm-rp2354.uf2", uf2(&[(RP_XIP, &rp)], UF2_FAMILY_RP2350_ARM_S)),
        ("kvm-rp2354.bin", rp.clone()),
        ("kvm-at32f405.bin", at32.clone()),
        ("pt.json", pt::json(pt::ACCEPTS_ABSOLUTE, &parts, PT_VERSION).into_bytes()),
    ];
    let mut files: Vec<(String, Vec<u8>)> = files.drain(..).map(|(n, d)| (n.to_string(), d)).collect();
    // From before the images were one per chip.
    let _ = fs::remove_file(out.join("kvm-module.bin"));
    for m in &modules {
        let (flash, module) = (format!("kvm-{}.bin", m.chip.name()), odkvmctl::firmware::module_bin(m.chip));
        match &m.image {
            Some((image, _)) => {
                files.push((flash, m.flash.clone()));
                files.push((module, image.clone()));
            }
            None => {
                let _ = fs::remove_file(out.join(&flash));
                let _ = fs::remove_file(out.join(&module));
            }
        }
    }
    for (name, data) in &files {
        fs::write(out.join(name), data)?;
        println!("{:>10} bytes  {}", data.len(), out.join(name).display());
    }
    let sizes: Vec<String> = modules
        .iter()
        .map(|m| {
            let kb = m.image.as_ref().map_or(0, |(i, _)| i.len() / 1024);
            format!("{} module image {kb} KB ({} KB uncompressed)", m.chip.name(), m.flash.len() / 1024)
        })
        .collect();
    println!(
        "firmware {} for hardware {hw_major}.x: RP2354B image {} KB of {} KB, AT32F405 image {} KB, {}",
        odkvmctl::names::version(version),
        rp.len() / 1024,
        PARTITION_SECTORS * 4,
        at32.len() / 1024,
        sizes.join(", ")
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["dist"] => dist(&root().join("dist")),
        ["dist", "--out", dir] => dist(Path::new(dir)),
        ["package", rest @ ..] => {
            let mut opt = package::Options { out: root().join("dist"), native: false, macos: false };
            let mut it = rest.iter();
            while let Some(a) = it.next() {
                match *a {
                    "--native" => opt.native = true,
                    "--macos" => opt.macos = true,
                    "--out" => opt.out = PathBuf::from(it.next().context("--out needs a directory")?),
                    other => bail!("unknown option {other}"),
                }
            }
            fs::create_dir_all(&opt.out)?;
            // The packages carry the firmware of the same tree: built first, into the same folder.
            dist(&opt.out)?;
            if opt.macos { package::macos(&root(), &opt) } else { package::package(&root(), &opt) }
        }
        ["desktop"] => package::desktop(&root(), false),
        ["desktop", "--remove"] => package::desktop(&root(), true),
        _ => {
            eprintln!(
                "usage: cargo xtask dist [--out DIR]\n       \
                 cargo xtask package [--out DIR] [--native | --macos]\n       \
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
        // firmware/rp2354/src/flash.rs: A at 8 KB, 896 KB each, the data partition at 1800 KB up to the last sector.
        let p = partitions();
        let starts: Vec<u32> = p.iter().map(|p| p.first_sector * 4).collect();
        let ends: Vec<u32> = p.iter().map(|p| (p.last_sector + 1) * 4).collect();
        assert_eq!(starts, [8, 904, 1800]);
        assert_eq!(ends, [904, 1800, 2044]);
        let table = pt::block(pt::ACCEPTS_ABSOLUTE, &p, PT_VERSION);
        // Partition-table item: 3 partitions, each 2 words + 2 words of ID + the name.
        assert_eq!(table[1] & 0xFF, 0x0a);
        assert_eq!(table[1] >> 24, 3);
        assert!(pt::json(pt::ACCEPTS_ABSOLUTE, &p, PT_VERSION).contains("\"ignored_during_arm_boot\": true"));
    }
}
