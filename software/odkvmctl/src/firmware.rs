//! The firmware that ships with the host software, and the file each way of installing takes (software.md §9
//! "Firmware page").
//!
//! - **Bundled** ([`bundled`]): the packages carry the release's `kvm-rp2354.bin`, `kvm-rp2354.uf2` and
//!   `kvm-first.uf2`, in `/usr/share/opendyn-kvm/firmware` (Linux) or in `firmware\` next to the programs (Windows).
//!   `OPENDYN_FIRMWARE` names another folder; programs run from a build tree (`target/...`) take its `dist/`.
//! - **A path** is one of those files, or a folder holding them. Over a PC port the KVM gets an image ([`for_pc`]: a
//!   `.bin` or an update UF2; a folder's `kvm-rp2354.bin`). The boot ROM's drive gets the UF2 its board takes
//!   ([`for_bootloader`]): a KVM an update UF2 (made from the `.bin` if need be), a new board `kvm-first.uf2`, taken
//!   from the same folder when the path names another file of the release.

use std::path::{Path, PathBuf};

use crate::{fl, image};

pub const UPDATE_BIN: &str = "kvm-rp2354.bin";
pub const UPDATE_UF2: &str = "kvm-rp2354.uf2";
pub const FIRST_UF2: &str = "kvm-first.uf2";

/// A firmware file chosen for installing, and checked to be RP2354B firmware.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chosen {
    /// The file it came from.
    pub file: PathBuf,
    /// The firmware version, and the hardware major version it is for.
    pub version: u32,
    pub hw_major: u8,
    /// What to send: the image (PC port) or the UF2 (boot ROM's drive).
    pub data: Vec<u8>,
}

/// The firmware folder that shipped with these programs, if there is one.
pub fn bundled() -> Option<PathBuf> {
    let env = std::env::var_os("OPENDYN_FIRMWARE").map(PathBuf::from);
    let exe = std::env::current_exe().ok().and_then(|e| e.canonicalize().ok());
    env.into_iter().chain(exe.iter().flat_map(|e| bundle_dirs(e))).find(|d| d.join(UPDATE_BIN).is_file())
}

/// Where a release's firmware lies for the executable `exe`: next to it (Windows), in the `share` beside its `bin`
/// (Linux packages), or in a build tree's `dist/` (`target/<profile>/`, `target/<triple>/<profile>/`).
fn bundle_dirs(exe: &Path) -> Vec<PathBuf> {
    let Some(dir) = exe.parent() else { return Vec::new() };
    let mut out = vec![dir.join("firmware")];
    if let Some(prefix) = dir.parent() {
        out.push(prefix.join("share").join("opendyn-kvm").join("firmware"));
    }
    if let Some(root) =
        dir.ancestors().take(3).find(|a| a.file_name().is_some_and(|n| n == "target")).and_then(Path::parent)
    {
        out.push(root.join("dist"));
    }
    out
}

fn read(file: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(file).map_err(|e| fl!("fw-read", file = file.display().to_string(), error = e.to_string()))
}

/// The first of `names` in `dir`.
fn in_folder(dir: &Path, names: &[&str]) -> Result<PathBuf, String> {
    names
        .iter()
        .map(|n| dir.join(n))
        .find(|p| p.is_file())
        .ok_or_else(|| fl!("fw-not-in-folder", name = names[0], folder = dir.display().to_string()))
}

/// Whether `file` is a first-install UF2 (absolute addresses).
fn is_first(file: &Path) -> bool {
    std::fs::read(file).is_ok_and(|b| image::uf2_family(&b) == Some(image::UF2_FAMILY_ABSOLUTE))
}

/// The image to send over a PC port: `path` (a `.bin` or an update UF2), or a folder's `kvm-rp2354.bin`. The
/// caller checks it against the KVM's hardware ([`image::check_hardware`]).
pub fn for_pc(path: &Path) -> Result<Chosen, String> {
    let file = if path.is_dir() {
        in_folder(path, &[UPDATE_BIN, UPDATE_UF2])?
    } else if is_first(path) {
        // The first-install UF2 is for the boot ROM; over a PC port its folder's image.
        path.parent()
            .map(|d| in_folder(d, &[UPDATE_BIN, UPDATE_UF2]))
            .ok_or_else(|| fl!("img-uf2-partition-table"))??
    } else {
        path.to_path_buf()
    };
    let image = image::update_image(&read(&file)?)?;
    let (version, hw_major) = image::check_image(&image)?;
    Ok(Chosen { file, version, hw_major, data: image })
}

/// The UF2 for the boot ROM's drive: `first` (a new board): the first-install UF2 (`path`, or its folder's);
/// otherwise an update UF2 (`path`, its folder's, or made from the `.bin`).
pub fn for_bootloader(path: &Path, first: bool) -> Result<Chosen, String> {
    let dir = if path.is_dir() { path } else { path.parent().unwrap_or(Path::new(".")) };
    if first {
        let file = if !path.is_dir() && is_first(path) { path.to_path_buf() } else { in_folder(dir, &[FIRST_UF2])? };
        let data = read(&file)?;
        let (version, hw_major) = image::check_image(&image::first_image(&data)?)?;
        return Ok(Chosen { file, version, hw_major, data });
    }
    let file =
        if path.is_dir() || is_first(path) { in_folder(dir, &[UPDATE_UF2, UPDATE_BIN])? } else { path.to_path_buf() };
    let bytes = read(&file)?;
    let image = image::update_image(&bytes)?;
    let (version, hw_major) = image::check_image(&image)?;
    let data = if image::is_uf2(&bytes) { bytes } else { image::update_uf2(&image) };
    Ok(Chosen { file, version, hw_major, data })
}

/// The version and hardware major version of the firmware at `path` (any of the release's files, or its folder).
pub fn describe(path: &Path) -> Result<(u32, u8), String> {
    for_pc(path).or_else(|e| for_bootloader(path, true).map_err(|_| e)).map(|c| (c.version, c.hw_major))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::tests::{firmware, first_uf2};

    struct Release(PathBuf);

    impl Release {
        /// A release folder with the given files, from the test firmware.
        fn new(name: &str, files: &[&str]) -> Release {
            let dir = std::env::temp_dir().join(format!("odkvmctl-fw-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let fw = firmware();
            for f in files {
                let data = match *f {
                    UPDATE_BIN => fw.clone(),
                    UPDATE_UF2 => image::update_uf2(&fw),
                    FIRST_UF2 => first_uf2(&fw),
                    _ => unreachable!(),
                };
                std::fs::write(dir.join(f), data).unwrap();
            }
            Release(dir)
        }
    }

    impl Drop for Release {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_pc_port_takes_the_image() {
        let r = Release::new("pc", &[UPDATE_BIN, UPDATE_UF2, FIRST_UF2]);
        let fw = firmware();
        for path in [r.0.clone(), r.0.join(UPDATE_BIN), r.0.join(UPDATE_UF2), r.0.join(FIRST_UF2)] {
            let c = for_pc(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            assert_eq!((&c.data[..fw.len()], c.version, c.hw_major), (&fw[..], 0x100, 0), "{}", path.display());
        }
        assert_eq!(for_pc(&r.0).unwrap().file, r.0.join(UPDATE_BIN));
        assert_eq!(describe(&r.0), Ok((0x100, 0)));
    }

    #[test]
    fn the_boot_rom_takes_the_uf2_its_board_wants() {
        let r = Release::new("rom", &[UPDATE_BIN, FIRST_UF2]);
        // A KVM: an update UF2, made from the .bin when the release has none.
        for path in [r.0.clone(), r.0.join(UPDATE_BIN), r.0.join(FIRST_UF2)] {
            let c = for_bootloader(&path, false).unwrap();
            assert_eq!(c.file, r.0.join(UPDATE_BIN));
            assert_eq!(image::uf2_family(&c.data), Some(image::UF2_FAMILY_RP2350_ARM_S));
            assert_eq!(c.data, image::update_uf2(&firmware()));
        }
        // A new board: the first-install UF2, whichever of the release's files was picked.
        for path in [r.0.clone(), r.0.join(UPDATE_BIN), r.0.join(FIRST_UF2)] {
            let c = for_bootloader(&path, true).unwrap();
            assert_eq!((c.file, c.version), (r.0.join(FIRST_UF2), 0x100));
        }
    }

    #[test]
    fn a_new_board_needs_the_first_install_uf2() {
        let r = Release::new("nofirst", &[UPDATE_BIN]);
        let e = for_bootloader(&r.0.join(UPDATE_BIN), true).unwrap_err();
        assert!(e.contains(FIRST_UF2), "{e}");
        let empty = Release::new("empty", &[]);
        assert!(for_pc(&empty.0).unwrap_err().contains(UPDATE_BIN));
    }

    #[test]
    fn bundled_firmware_is_looked_for_beside_the_programs() {
        let dirs = |exe: &str| bundle_dirs(Path::new(exe));
        assert_eq!(
            dirs("/usr/bin/opendyn-kvm"),
            [PathBuf::from("/usr/bin/firmware"), PathBuf::from("/usr/share/opendyn-kvm/firmware")]
        );
        assert!(dirs("/home/me/kvm/target/debug/opendyn-kvm").contains(&PathBuf::from("/home/me/kvm/dist")));
        assert!(
            dirs("/home/me/kvm/target/x86_64-pc-windows-gnu/release/opendyn-kvm.exe")
                .contains(&PathBuf::from("/home/me/kvm/dist"))
        );
    }
}
