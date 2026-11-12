//! The RP2354B's boot ROM on the PWR/SERVICE port (software.md §4.9 "Service port"): installing firmware on a new
//! board, or on one whose firmware doesn't come up on the PC ports.
//!
//! The KVM's firmware routes the RP2354B's USB to Hub-0, so a computer on PWR/SERVICE sees the KVM only while the
//! RP2354B is in the boot ROM's bootloader: a blank board, the pinhole held while plugging in, or `odkvmctl reboot
//! bootloader`. The ROM then shows a USB drive and its PICOBOOT interface (RP2350 datasheet §5.5, §5.6).
//!
//! - **Which board** ([`Scanner`]): PICOBOOT's GET_INFO tells where an RP2350 Arm image would go. One of the KVM's two
//!   firmware partitions: a KVM, whose other partition holds the firmware installed (its `VERSION` item, in the first
//!   4 KB). No partition table and erased flash: a new board. No partition table but the KVM's firmware (a board
//!   flashed by a debug probe): a board to set up as a new one. Anything else (another partition table, other firmware)
//!   is not a KVM, and is left alone.
//! - **Installing** ([`install`]): the UF2 the board takes (`firmware::for_bootloader`), copied onto the ROM's drive,
//!   as dragging it there would: the ROM picks the partition, writes it, and boots it whatever its version, erasing the
//!   other one's start (§5.1.16). PICOBOOT's UF2_STATUS counts the blocks the ROM has written; the device leaves the
//!   bus when the ROM restarts.
//! - **Access:** Linux needs the udev rule for 2e8a:000f (`udev/70-opendyn-kvm.rules`) and the drive mounted (desktops
//!   do it; else [`install`] asks udisks); Windows binds WinUSB to PICOBOOT itself, from the ROM's descriptors. Without
//!   PICOBOOT the board's state is unknown, and the window lets the user say which file it takes.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kvm_proto::rpimage::{self, layout};
use nusb::descriptors::TransferType;
use nusb::transfer::{Buffer, Bulk, ControlOut, ControlType, Direction, In, Out, Recipient, TransferError};
use nusb::{DeviceInfo, MaybeFuture};

use crate::fl;

/// The boot ROM's USB IDs (datasheet Table 470; the KVM doesn't white-label them).
pub const VID: u16 = 0x2e8a;
pub const PID: u16 = 0x000f;
const MAGIC: u32 = 0x431f_d10b;
const XIP_BASE: u32 = 0x1000_0000;
const FAMILY_RP2350_ARM_S: u32 = 0xe48b_ff59;
const TIMEOUT: Duration = Duration::from_secs(3);
/// The name the UF2 gets on the ROM's drive (any name does).
const UF2_NAME: &str = "OPENDYN.UF2";
/// The location of [`simulated`] boards.
const SIM: &str = "sim";

mod cmd {
    pub const REBOOT2: u8 = 0x0a;
    pub const READ: u8 = 0x84;
    pub const GET_INFO: u8 = 0x8b;
}

mod info {
    pub const UF2_TARGET_PARTITION: u8 = 3;
    pub const UF2_STATUS: u8 = 4;
}

const INTERFACE_RESET: u8 = 0x41;

/// What the board in the bootloader is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Board {
    /// The KVM's partition table. `installed`: the firmware version in the partition it boots (None: no image there);
    /// an update goes into the other one, `slot` (0 = A, 1 = B).
    Kvm { installed: Option<u32>, slot: u8 },
    /// No partition table. `installed`: the KVM's firmware at the start of flash (a board flashed without its
    /// partition table), or None: erased. Either way it takes `kvm-first.uf2`.
    New { installed: Option<u32> },
    /// Not a KVM: another partition table, or firmware that isn't the KVM's. Left alone.
    Other,
}

impl Board {
    /// Whether it takes the first-install UF2 (partition table and partition A) rather than an update's.
    pub fn takes_first(&self) -> bool {
        matches!(self, Board::New { .. })
    }
}

/// An RP2350 in its boot ROM's bootloader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootDevice {
    /// Bus and address, to tell several apart and to find it again.
    pub location: String,
    /// What it is, or why PICOBOOT couldn't say (no permission, no driver).
    pub board: Result<Board, String>,
    /// Its drive, where the UF2 is copied; None: not mounted, or not told apart from another RP2350's.
    pub drive: Option<PathBuf>,
    /// The USB device's sysfs directory, under which its drive's block device lies.
    #[cfg(target_os = "linux")]
    sysfs: PathBuf,
}

/// The ROM's report on the UF2 being written (GET_INFO UF2_STATUS).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Uf2Status {
    blocks: usize,
    total: usize,
    /// Abort reasons (exclusively locked, bad address, write error, reboot failure) and status flags (ignored family).
    abort: u8,
    ignored: bool,
}

impl Uf2Status {
    fn from_words(w: &[u32]) -> Option<Uf2Status> {
        let w0 = *w.first()?;
        Some(Uf2Status {
            blocks: *w.get(2)? as usize,
            total: *w.get(3)? as usize,
            abort: ((w0 >> 4) & 0xF) as u8,
            ignored: w0 & 0x1 != 0,
        })
    }

    fn error(&self) -> Option<String> {
        Some(match self.abort {
            0 if self.ignored => fl!("boot-rom-ignored"),
            0 => return None,
            a if a & 0x1 != 0 => fl!("boot-rom-locked"),
            a if a & 0x2 != 0 => fl!("boot-rom-bad-address"),
            a if a & 0x4 != 0 => fl!("boot-rom-write-error"),
            _ => fl!("boot-rom-reboot-failure"),
        })
    }
}

/// A PICOBOOT session on one device.
struct Picoboot {
    iface: nusb::Interface,
    out: nusb::Endpoint<Bulk, Out>,
    inp: nusb::Endpoint<Bulk, In>,
    token: u32,
}

impl Picoboot {
    fn open(d: &DeviceInfo) -> Result<Picoboot, String> {
        let number = d
            .interfaces()
            .find(|i| i.class() == 0xFF && i.subclass() == 0 && i.protocol() == 0)
            .map(|i| i.interface_number())
            .ok_or_else(|| fl!("boot-no-picoboot"))?;
        let access = |e: nusb::Error| {
            if e.kind() == nusb::ErrorKind::PermissionDenied {
                fl!("boot-no-permission")
            } else {
                fl!("boot-no-access", error = e.to_string())
            }
        };
        let dev = d.open().wait().map_err(access)?;
        let iface = dev.claim_interface(number).wait().map_err(access)?;
        let (mut out_ep, mut in_ep) = (None, None);
        for ep in iface.descriptor().into_iter().flat_map(|d| d.endpoints().collect::<Vec<_>>()) {
            if ep.transfer_type() == TransferType::Bulk {
                match ep.direction() {
                    Direction::Out => out_ep = out_ep.or(Some(ep.address())),
                    Direction::In => in_ep = in_ep.or(Some(ep.address())),
                }
            }
        }
        let (Some(o), Some(i)) = (out_ep, in_ep) else { return Err(fl!("boot-no-picoboot")) };
        let out = iface.endpoint::<Bulk, Out>(o).map_err(|e| fl!("boot-no-access", error = e.to_string()))?;
        let inp = iface.endpoint::<Bulk, In>(i).map_err(|e| fl!("boot-no-access", error = e.to_string()))?;
        Ok(Picoboot { iface, out, inp, token: 1 })
    }

    /// One command (`args` at most 16 bytes); `data_in` bytes come back, a multiple of the 64-byte packet, or none.
    fn command(&mut self, id: u8, args: &[u8], data_in: usize) -> Result<Vec<u8>, String> {
        let fail = |e: TransferError| fl!("boot-usb", error = e.to_string());
        let mut c = [0u8; 32];
        c[..4].copy_from_slice(&MAGIC.to_le_bytes());
        c[4..8].copy_from_slice(&self.token.to_le_bytes());
        c[8] = id;
        c[9] = args.len() as u8;
        c[12..16].copy_from_slice(&(data_in as u32).to_le_bytes());
        c[16..16 + args.len()].copy_from_slice(args);
        self.token = self.token.wrapping_add(1);
        self.out.transfer_blocking(Buffer::from(&c[..]), TIMEOUT).status.map_err(fail)?;
        if data_in == 0 {
            // Success is an empty IN packet.
            self.inp.transfer_blocking(Buffer::new(64), TIMEOUT).status.map_err(fail)?;
            return Ok(Vec::new());
        }
        let done = self.inp.transfer_blocking(Buffer::new(data_in), TIMEOUT);
        done.status.map_err(fail)?;
        let mut data = done.buffer.into_vec();
        data.truncate(done.actual_len);
        // And the host acknowledges with an empty OUT packet.
        self.out.transfer_blocking(Buffer::new(0), TIMEOUT).status.map_err(fail)?;
        Ok(data)
    }

    /// Clears a stalled command (and would abort a UF2 being copied: never while installing).
    fn reset(&mut self) {
        let number = self.iface.interface_number();
        let req = ControlOut {
            control_type: ControlType::Vendor,
            recipient: Recipient::Interface,
            request: INTERFACE_RESET,
            value: 0,
            index: u16::from(number),
            data: &[],
        };
        let _ = self.iface.control_out(req, TIMEOUT).wait();
        let _ = self.out.clear_halt().wait();
        let _ = self.inp.clear_halt().wait();
    }

    /// GET_INFO: the significant words of the answer.
    fn get_info(&mut self, kind: u8, param0: u32) -> Result<Vec<u32>, String> {
        let mut args = [0u8; 16];
        args[0] = kind;
        args[4..8].copy_from_slice(&param0.to_le_bytes());
        let data = self.command(cmd::GET_INFO, &args, 64)?;
        Ok(info_words(&data))
    }

    fn read(&mut self, addr: u32, len: usize) -> Result<Vec<u8>, String> {
        let mut args = [0u8; 8];
        args[..4].copy_from_slice(&addr.to_le_bytes());
        args[4..].copy_from_slice(&(len as u32).to_le_bytes());
        self.command(cmd::READ, &args, len)
    }

    fn status(&mut self) -> Option<Uf2Status> {
        self.get_info(info::UF2_STATUS, 0).ok().and_then(|w| Uf2Status::from_words(&w))
    }
}

/// GET_INFO's answer: a count, then that many words.
fn info_words(data: &[u8]) -> Vec<u32> {
    let words: Vec<u32> = data.as_chunks::<4>().0.iter().map(|w| u32::from_le_bytes(*w)).collect();
    let n = words.first().map_or(0, |n| (*n as usize).min(words.len() - 1));
    words.get(1..1 + n).map(<[u32]>::to_vec).unwrap_or_default()
}

/// The board, from where an RP2350 Arm image would go (GET_INFO UF2_TARGET_PARTITION: a partition, 0xFF for "no
/// partition table: at the start of flash", or -1 for nowhere) and the partition's place.
fn classify(target: &[u32]) -> Target {
    use layout::{PARTITION_A_SECTOR as A, PARTITION_B_SECTOR as B, PARTITION_SECTORS as N};
    match target.first().copied() {
        Some(0xFF) => Target::Unpartitioned,
        Some(p) if p < 16 => {
            let loc = target.get(1).copied().unwrap_or(0);
            match (loc & 0x1FFF, (loc >> 13) & 0x1FFF) {
                (first, last) if (first, last) == (A, A + N - 1) => Target::Kvm { slot: 0, booted: B },
                (first, last) if (first, last) == (B, B + N - 1) => Target::Kvm { slot: 1, booted: A },
                _ => Target::Other,
            }
        }
        _ => Target::Other,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// A KVM: an update goes to `slot`; the firmware in use starts at sector `booted`.
    Kvm {
        slot: u8,
        booted: u32,
    },
    Unpartitioned,
    Other,
}

fn identify(pb: &mut Picoboot) -> Result<Board, String> {
    let target = match pb.get_info(info::UF2_TARGET_PARTITION, FAMILY_RP2350_ARM_S) {
        Ok(t) => t,
        Err(_) => {
            // A stalled interface from an earlier session: once more after a reset.
            pb.reset();
            pb.get_info(info::UF2_TARGET_PARTITION, FAMILY_RP2350_ARM_S)?
        }
    };
    match classify(&target) {
        Target::Kvm { slot, booted } => {
            let head = pb.read(XIP_BASE + booted * layout::SECTOR, 4096)?;
            Ok(Board::Kvm { installed: rpimage::image_version(&head), slot })
        }
        Target::Unpartitioned => {
            let head = pb.read(XIP_BASE, 4096)?;
            if head.iter().all(|b| *b == 0xFF) {
                return Ok(Board::New { installed: None });
            }
            // Something at the start of flash: the KVM's firmware has its FwId somewhere in the image.
            let mut image = head;
            while image.len() < (layout::PARTITION_SECTORS * layout::SECTOR) as usize {
                if let Some((version, _)) = rpimage::fw_id(&image) {
                    return Ok(Board::New { installed: Some(version) });
                }
                let more = pb.read(XIP_BASE + image.len() as u32, 16 * 1024)?;
                image.extend_from_slice(&more);
            }
            Ok(rpimage::fw_id(&image).map_or(Board::Other, |(v, _)| Board::New { installed: Some(v) }))
        }
        Target::Other => Ok(Board::Other),
    }
}

/// Finds the boot ROMs on USB, and remembers what each is: identifying one reads its flash, which a new board's
/// unpartitioned image can make take a second.
#[derive(Default)]
pub struct Scanner {
    known: HashMap<String, Result<Board, String>>,
}

impl Scanner {
    pub fn scan(&mut self) -> Vec<BootDevice> {
        let found: Vec<DeviceInfo> = match nusb::list_devices().wait() {
            Ok(list) => list.filter(|d| d.vendor_id() == VID && d.product_id() == PID).collect(),
            Err(_) => return Vec::new(),
        };
        let drives = drives();
        let mut out = Vec::new();
        for d in &found {
            let location = format!("{}-{}", d.bus_id(), d.device_address());
            let board = match self.known.get(&location) {
                Some(b @ Ok(_)) => b.clone(),
                // Not identified yet, or it failed (the udev rule may have come since): ask.
                _ => Picoboot::open(d).and_then(|mut pb| identify(&mut pb)),
            };
            self.known.insert(location.clone(), board.clone());
            out.push(BootDevice {
                location,
                board,
                drive: None,
                #[cfg(target_os = "linux")]
                sysfs: std::fs::canonicalize(d.sysfs_path()).unwrap_or_else(|_| d.sysfs_path().to_path_buf()),
            });
        }
        self.known.retain(|l, _| out.iter().any(|d| &d.location == l));
        match_drives(&mut out, &drives);
        out
    }
}

/// A mounted UF2 drive of an RP2350's boot ROM; on Linux with the sysfs path of its block device.
#[derive(Debug, Clone)]
struct Drive {
    path: PathBuf,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    block: Option<PathBuf>,
}

/// Whether `dir` is the drive of an RP2350's boot ROM: its INFO_UF2.TXT names the board RP2350.
fn is_rp2350_drive(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("INFO_UF2.TXT"))
        .is_ok_and(|t| t.lines().any(|l| l.trim().eq_ignore_ascii_case("Board-ID: RP2350")))
}

/// Gives each device its drive: on Linux the one whose block device lies under the USB device; elsewhere, a single
/// drive goes to a single device (several RP2350s at once can't be told apart there).
fn match_drives(devices: &mut [BootDevice], drives: &[Drive]) {
    #[cfg(target_os = "linux")]
    for d in devices.iter_mut() {
        d.drive =
            drives.iter().find(|x| x.block.as_ref().is_some_and(|b| b.starts_with(&d.sysfs))).map(|x| x.path.clone());
    }
    #[cfg(not(target_os = "linux"))]
    if let ([d], [x]) = (devices, drives) {
        d.drive = Some(x.path.clone());
    }
}

/// `/proc/self/mountinfo`'s FAT mounts: mount point and source.
#[cfg(any(target_os = "linux", test))]
fn fat_mounts(mountinfo: &str) -> Vec<(PathBuf, String)> {
    // An octal escape (\040 for a space) in the mount point.
    fn unescape(s: &str) -> String {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
                out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
                i += 4;
            } else {
                out.push(b[i]);
                i += 1;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    mountinfo
        .lines()
        .filter_map(|l| {
            let (left, right) = l.split_once(" - ")?;
            let point = left.split(' ').nth(4)?;
            let mut r = right.split(' ');
            let (fstype, source) = (r.next()?, r.next()?);
            matches!(fstype, "vfat" | "msdos" | "fat").then(|| (PathBuf::from(unescape(point)), source.to_string()))
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn drives() -> Vec<Drive> {
    let info = std::fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    fat_mounts(&info)
        .into_iter()
        .filter(|(path, _)| is_rp2350_drive(path))
        .map(|(path, source)| {
            let block = Path::new(&source)
                .file_name()
                .and_then(|n| std::fs::canonicalize(Path::new("/sys/class/block").join(n)).ok());
            Drive { path, block }
        })
        .collect()
}

#[cfg(windows)]
fn drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
    const DRIVE_REMOVABLE: u32 = 2;
    // SAFETY: no arguments.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| format!("{}:\\", (b'A' + i) as char))
        .filter(|root| {
            let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
            // SAFETY: a NUL-terminated root path.
            let kind = unsafe { GetDriveTypeW(wide.as_ptr()) };
            kind == DRIVE_REMOVABLE
        })
        .map(PathBuf::from)
        .filter(|p| is_rp2350_drive(p))
        .map(|path| Drive { path, block: None })
        .collect()
}

#[cfg(not(any(target_os = "linux", windows)))]
fn drives() -> Vec<Drive> {
    Vec::new()
}

/// The device's drive as a block device name: the FAT partition (`sdX1`), or the whole disk if it has none.
#[cfg(target_os = "linux")]
fn block_name(dev: &BootDevice) -> Option<String> {
    let parts: Vec<PathBuf> = std::fs::read_dir("/sys/class/block")
        .ok()?
        .flatten()
        .filter(|e| std::fs::canonicalize(e.path()).is_ok_and(|p| p.starts_with(&dev.sysfs)))
        .map(|e| e.path())
        .collect();
    let part = parts.iter().find(|p| p.join("partition").exists()).or(parts.first())?;
    Some(part.file_name()?.to_string_lossy().into_owned())
}

/// Asks udisks to mount the device's drive (Linux desktops without automounting, or a drive not mounted yet). Just
/// after the board appears neither the kernel's partition scan nor udisks has caught up with the drive (measured on
/// a board: ~0.1 s after the disk showed, udisks refused it), so this tries for a few seconds, while the board stays.
#[cfg(target_os = "linux")]
fn mount(dev: &BootDevice) -> Option<PathBuf> {
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(name) = block_name(dev) {
            let _ = std::process::Command::new("udisksctl")
                .args(["mount", "--no-user-interaction", "-b", &format!("/dev/{name}")])
                .output();
        }
        // Mounted by udisksctl, or meanwhile by the desktop's automounter.
        let mut devices = [dev.clone()];
        match_drives(&mut devices, &drives());
        if devices[0].drive.is_some() || Instant::now() >= until || !present(dev) {
            return devices[0].drive.clone();
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[cfg(not(target_os = "linux"))]
fn mount(_dev: &BootDevice) -> Option<PathBuf> {
    None
}

fn reopen(dev: &BootDevice) -> Option<Picoboot> {
    let list = nusb::list_devices().wait().ok()?;
    let d = list
        .filter(|d| d.vendor_id() == VID && d.product_id() == PID)
        .find(|d| format!("{}-{}", d.bus_id(), d.device_address()) == dev.location)?;
    Picoboot::open(&d).ok()
}

fn present(dev: &BootDevice) -> bool {
    nusb::list_devices().wait().is_ok_and(|mut l| {
        l.any(|d| {
            d.vendor_id() == VID
                && d.product_id() == PID
                && format!("{}-{}", d.bus_id(), d.device_address()) == dev.location
        })
    })
}

/// A board for the simulator (`OPENDYN_SIM_BOOT`): `kvm` (firmware 0.1.0 in B), `new`, `other`, or `unknown`
/// (PICOBOOT out of reach). Installing on it pretends; nothing touches USB.
pub fn simulated(kind: &str) -> Option<BootDevice> {
    let board = match kind {
        "kvm" => Ok(Board::Kvm { installed: Some(0x00_01_00), slot: 0 }),
        "new" => Ok(Board::New { installed: None }),
        "other" => Ok(Board::Other),
        "unknown" => Err(fl!("boot-no-permission")),
        _ => return None,
    };
    let drive = PathBuf::from(if cfg!(windows) { "E:\\" } else { "/run/media/you/RP2350" });
    Some(BootDevice {
        location: SIM.into(),
        board,
        drive: Some(drive),
        #[cfg(target_os = "linux")]
        sysfs: PathBuf::new(),
    })
}

/// Copies `uf2` onto the device's drive, as dragging it there would, and waits for the ROM to write it and restart.
/// `progress` gets the 512-byte blocks done (the ROM's own count where PICOBOOT answers) and their total.
pub fn install(dev: &BootDevice, uf2: &[u8], mut progress: impl FnMut(usize, usize)) -> Result<(), String> {
    if dev.location == SIM {
        let total = uf2.len() / 512;
        for k in 0..=20 {
            progress(total * k / 20, total);
            std::thread::sleep(Duration::from_millis(100));
        }
        return Ok(());
    }
    let drive = dev.drive.clone().or_else(|| mount(dev)).ok_or_else(|| fl!("boot-no-drive"))?;
    let total = uf2.len() / 512;
    // Only for the ROM's count; the copy works without it.
    let mut pb = reopen(dev);
    let path = drive.join(UF2_NAME);
    let mut file = std::fs::File::create(&path).map_err(|e| fl!("boot-write", error = e.to_string()))?;
    const CHUNK: usize = 64 * 1024;
    let chunks = uf2.len().div_ceil(CHUNK);
    let mut seen = 0;
    for (k, chunk) in uf2.chunks(CHUNK).enumerate() {
        // On the drive, not just in the page cache: the ROM writes what reaches it.
        let r = file.write_all(chunk).and_then(|()| file.sync_data());
        if let Err(e) = r {
            // The ROM restarts as soon as it has the last block; flushing the file's last bits can fail then.
            if k + 1 < chunks {
                return Err(fl!("boot-write", error = e.to_string()));
            }
        }
        let status = pb.as_mut().and_then(Picoboot::status);
        if let Some(e) = status.and_then(|s| s.error()) {
            return Err(e);
        }
        seen = status.filter(|s| s.total == total).map_or(((k + 1) * CHUNK / 512).min(total), |s| s.blocks);
        progress(seen, total);
    }
    drop(file);
    // The ROM writes the blocks as they come and restarts when it has them all: the device leaves the bus.
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if !present(dev) {
            progress(total, total);
            return Ok(());
        }
        if let Some(s) = pb.as_mut().and_then(Picoboot::status) {
            if let Some(e) = s.error() {
                return Err(e);
            }
            if s.total == total && s.blocks > seen {
                seen = s.blocks;
                progress(seen, total);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    Err(fl!("boot-no-restart"))
}

/// Leaves the bootloader without installing anything: a normal restart (REBOOT2), into whatever the board has.
pub fn restart(dev: &BootDevice) -> Result<(), String> {
    if dev.location == SIM {
        return Ok(());
    }
    let mut pb = reopen(dev).ok_or_else(|| fl!("boot-no-picoboot"))?;
    let mut args = [0u8; 16];
    // Flags 0: a normal boot; after 100 ms, so that the answer gets out first.
    args[4..8].copy_from_slice(&100u32.to_le_bytes());
    pb.command(cmd::REBOOT2, &args, 0).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(words: &[u32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn get_info_answers_are_a_count_and_that_many_words() {
        let mut data = bytes(&[3, 1, 0x0003_D802, 0x7000_0000, 99]);
        data.resize(64, 0);
        assert_eq!(info_words(&data), [1, 0x0003_D802, 0x7000_0000]);
        assert_eq!(info_words(&bytes(&[200, 5])), [5], "a count beyond the data");
        assert!(info_words(&[]).is_empty());
    }

    #[test]
    fn the_kvms_partitions_are_told_from_other_tables() {
        use layout::{PARTITION_A_SECTOR as A, PARTITION_B_SECTOR as B, PARTITION_SECTORS as N};
        let loc = |first: u32, last: u32| 0xFC00_0000 | last << 13 | first;
        assert_eq!(classify(&[0, loc(A, A + N - 1), 0]), Target::Kvm { slot: 0, booted: B });
        assert_eq!(classify(&[1, loc(B, B + N - 1), 0]), Target::Kvm { slot: 1, booted: A });
        // A Pico 2 with picotool's default A/B table, 1 MB each.
        assert_eq!(classify(&[0, loc(2, 257), 0]), Target::Other);
        assert_eq!(classify(&[0xFF]), Target::Unpartitioned);
        assert_eq!(classify(&[0xFFFF_FFFF]), Target::Other);
        assert_eq!(classify(&[]), Target::Other);
    }

    #[test]
    fn uf2_status_words() {
        let s = Uf2Status::from_words(&[0x0000_0000, 0xe48b_ff59, 100, 2871]).unwrap();
        assert_eq!((s.blocks, s.total, s.error()), (100, 2871, None));
        let s = Uf2Status::from_words(&[0x0000_0040, 0, 7, 9]).unwrap();
        assert_eq!(s.error(), Some(fl!("boot-rom-write-error")));
        let s = Uf2Status::from_words(&[0x0000_0001, 0, 0, 9]).unwrap();
        assert_eq!(s.error(), Some(fl!("boot-rom-ignored")));
        assert_eq!(Uf2Status::from_words(&[0, 0]), None);
    }

    #[test]
    fn fat_mounts_from_mountinfo() {
        let info = "36 35 98:0 / /mnt1 rw,noatime master:1 - ext3 /dev/root rw,errors=continue\n\
                    90 29 8:17 / /run/media/me/RP2350 rw,nosuid,nodev,relatime shared:500 - vfat /dev/sdb1 rw,uid=1000\n\
                    91 29 8:33 / /media/my\\040stick rw - vfat /dev/sdc1 rw\n";
        assert_eq!(
            fat_mounts(info),
            [
                (PathBuf::from("/run/media/me/RP2350"), "/dev/sdb1".to_string()),
                (PathBuf::from("/media/my stick"), "/dev/sdc1".to_string())
            ]
        );
    }

    #[test]
    fn an_rp2350_drive_is_known_by_its_info_file() {
        let dir = std::env::temp_dir().join(format!("odkvmctl-uf2-drive-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_rp2350_drive(&dir));
        std::fs::write(dir.join("INFO_UF2.TXT"), "UF2 Bootloader v1.0\nModel: Raspberry Pi RP2350\nBoard-ID: RP2350\n")
            .unwrap();
        assert!(is_rp2350_drive(&dir));
        std::fs::write(dir.join("INFO_UF2.TXT"), "UF2 Bootloader v3.0\nModel: Raspberry Pi RP2\nBoard-ID: RPI-RP2\n")
            .unwrap();
        assert!(!is_rp2350_drive(&dir), "an RP2040");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_single_drive_goes_to_a_single_device() {
        let dev = BootDevice {
            location: "1-4".into(),
            board: Ok(Board::New { installed: None }),
            drive: None,
            #[cfg(target_os = "linux")]
            sysfs: PathBuf::from("/sys/devices/pci0000:00/usb1/1-4"),
        };
        let drive = Drive {
            path: PathBuf::from("/run/media/me/RP2350"),
            block: Some(PathBuf::from(
                "/sys/devices/pci0000:00/usb1/1-4/1-4:1.0/host6/target6:0:0/6:0:0:0/block/sdb/sdb1",
            )),
        };
        let other = Drive { path: PathBuf::from("/run/media/me/RP2350 1"), block: None };
        let mut devices = [dev];
        match_drives(&mut devices, &[other.clone(), drive.clone()]);
        if cfg!(target_os = "linux") {
            assert_eq!(devices[0].drive.as_deref(), Some(drive.path.as_path()));
        } else {
            assert_eq!(devices[0].drive, None, "two drives: not told apart");
            match_drives(&mut devices, std::slice::from_ref(&drive));
            assert_eq!(devices[0].drive.as_deref(), Some(drive.path.as_path()));
        }
    }
}
