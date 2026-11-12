//! RP2354B firmware updates into the other A/B partition (software.md §4.9; RP2350 datasheet §5.1.16–5.1.17).
//!
//! - `odkvmctl update` sends a raw image linked at 0x1000_0000 (`cargo xtask dist` → `kvm-rp2354.bin`). Its first
//!   sector must hold an RP2350 Arm IMAGE_DEF.
//! - The image goes into the partition that did not boot. Every sector but the first is written as it arrives. The
//!   first sector holds the IMAGE_DEF; it gets the *try before you buy* flag here and is written only once the key on
//!   the KVM confirms the update, so until then the partition holds no bootable image.
//! - A FLASH_UPDATE reboot then starts the new image under the boot ROM's 16.7 s watchdog. The image buys itself
//!   ([`buy`]) once Hub-0 is up and a link answers ([`crate::selftest`]), and the ROM then erases the old image's first
//!   sector. If it doesn't, the watchdog reboots into the old image, which stays the preferred one.
//!
//! The same image without the flag (UF2 drag-and-drop, `picotool`, a debug probe) boots normally and needs no buy.
//! Without a partition table (flashed by a debug probe), updates are not offered (`caps::FW_UPDATE`).

use core::cell::Cell;
use core::sync::atomic::{AtomicI32, Ordering};

use defmt::{info, warn};
use embassy_rp::rom_data;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use kvm_core::engine::FwOp;
use kvm_proto::crc::Crc32;
use kvm_proto::ctrl::FwState;
use kvm_proto::ctrl::diag::FwUpdates;
use kvm_proto::rpimage;

use crate::flash::{self, DATA, DATA_LEN, PARTITION_A, PARTITION_LEN, SECTOR};
use crate::kvm;
use crate::uartlog::uart_log;

/// Requests from the engine; FwData is refused with Busy while a sector is being written, so this never fills.
pub static OPS: Channel<CriticalSectionRawMutex, FwOp, 16> = Channel::new();
/// The self-test asks for the buy through here, so the 4 KB work area is used by one task only.
static BUY: embassy_sync::signal::Signal<CriticalSectionRawMutex, ()> = embassy_sync::signal::Signal::new();

const XIP_BASE: u32 = 0x1000_0000;
const SECTOR_LEN: usize = SECTOR as usize;

/// Where the running image lives and where an update goes (physical offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub struct Layout {
    pub running: u32,
    pub other: u32,
}

static LAYOUT: Mutex<ThreadModeRawMutex, Cell<Option<Layout>>> = Mutex::new(Cell::new(None));

pub fn layout() -> Option<Layout> {
    LAYOUT.lock(|l| l.get())
}

#[repr(C, align(4))]
struct Work([u8; SECTOR_LEN]);

mod pt {
    pub const PT_INFO: u32 = 0x0001;
    pub const PARTITION_LOCATION_AND_FLAGS: u32 = 0x0010;
    pub const LINK_TYPE_A_PARTITION: u32 = 1 << 1;
    pub const LINK_MASK: u32 = 0x7E;
}

fn sectors(location: u32) -> (u32, u32) {
    let first = location & 0x1FFF;
    let last = (location >> 13) & 0x1FFF;
    (first * SECTOR, (last + 1 - first) * SECTOR)
}

/// What [`discover`] found, for the diagnostics (`diag::id::FW_UPDATES`): low byte [`FwUpdates`], the bits above what
/// the check read. The first board left updates off without saying why (2026-11-11), and the defmt log needs a probe.
pub static FOUND: AtomicI32 = AtomicI32::new(FwUpdates::ReadFailed as i32);

fn found(f: FwUpdates, detail: u32) {
    FOUND.store(((detail & 0x00FF_FFFF) << 8) as i32 | f as i32, Ordering::Relaxed);
}

/// The partition the boot ROM last booted, from `get_sys_info(BOOT_INFO)` (datasheet §5.4.8.16): word 0 is
/// `0xttppbbdd`, `pp` the partition (0xFF: none).
fn boot_partition() -> Option<u8> {
    const BOOT_INFO: u32 = 0x0040;
    let mut info = [0u32; 5];
    let n = unsafe { rom_data::get_sys_info(info.as_mut_ptr(), info.len(), BOOT_INFO) };
    (n >= 2 && info[0] & BOOT_INFO != 0).then(|| (info[1] >> 16) as u8).filter(|&p| p != 0xFF)
}

/// Reads the partition table through the boot ROM and checks that it is the one `cargo xtask dist` writes: the A/B
/// pair and the data partition. Records what it found in [`FOUND`].
fn discover(work: &mut Work) -> Option<Layout> {
    let mut buf = [0u32; 16];
    let flags = pt::PT_INFO | pt::PARTITION_LOCATION_AND_FLAGS;
    let query = |buf: &mut [u32; 16]| unsafe { rom_data::get_partition_table_info(buf.as_mut_ptr(), buf.len(), flags) };
    let mut n = query(&mut buf);
    if n < 0 {
        // Not loaded after a watchdog boot, or changed since the ROM loaded it (datasheet §5.4.8.16): (re)load it.
        // Without the force, a table the ROM considers loaded is not read again, and the query fails the same way.
        let r = unsafe { rom_data::load_partition_table(work.0.as_mut_ptr(), work.0.len(), true) };
        n = query(&mut buf);
        if n < 0 {
            warn!("partition table: query {}, load {}: firmware updates over USB are off", n, r);
            found(FwUpdates::ReadFailed, (n as u8 as u32) | (r as u8 as u32) << 8);
            return None;
        }
    }
    let read = (buf[1] & 0x1FF) | (buf[0] & 0x7F) << 9 | (n.clamp(0, 255) as u32) << 16;
    if n >= 2 && buf[1] & 0x100 == 0 {
        info!("no partition table: firmware updates over USB are off");
        found(FwUpdates::NoTable, read);
        return None;
    }
    if n < 10 || buf[0] & flags != flags || buf[1] & 0xFF != 3 {
        warn!(
            "not the KVM's partition table ({} words, {=u32:#x} {=u32:#x}): firmware updates over USB are off",
            n, buf[0], buf[1]
        );
        found(FwUpdates::OtherTable, read);
        return None;
    }
    let (a, b, data) = (sectors(buf[4]), sectors(buf[6]), sectors(buf[8]));
    let b_links_a = buf[7] & pt::LINK_MASK == pt::LINK_TYPE_A_PARTITION;
    let differs = (a != (PARTITION_A, PARTITION_LEN)) as u32
        | ((b != (PARTITION_A + PARTITION_LEN, PARTITION_LEN)) as u32) << 1
        | ((data != (DATA, DATA_LEN)) as u32) << 2
        | (!b_links_a as u32) << 3;
    if differs != 0 {
        warn!("unexpected partition table (A {:?}, B {:?}, data {:?}): firmware updates over USB are off", a, b, data);
        found(FwUpdates::OtherLayout, differs | (a.0 / SECTOR) << 4);
        return None;
    }
    // Which partition runs: the QMI address translation the boot ROM set up. `flash_runtime_to_storage_addr` returns
    // the untranslated address in the XIP window, 0x1000_0000 + the flash offset (measured on the first board,
    // 2026-11-12: comparing it with the bare offset turned updates off). The bare offset and the partition the ROM
    // says it booted are fallbacks.
    let running = unsafe { rom_data::flash_runtime_to_storage_addr(XIP_BASE) } as u32;
    let booted = boot_partition();
    let pick = |r: u32| [a.0, b.0].iter().position(|&p| p == r);
    let (k, source) = match (pick(running.wrapping_sub(XIP_BASE)), pick(running), booted) {
        (Some(k), _, _) => (k, 0),
        (None, Some(k), _) => (k, 1),
        (None, None, Some(p @ (0 | 1))) => (p as usize, 2),
        _ => {
            warn!("running from {=u32:#x} (boot partition {:?}), outside the A/B pair", running, booted);
            found(FwUpdates::UnknownPartition, (running >> 12) & 0xFFFF | (booted.unwrap_or(0xFF) as u32) << 16);
            return None;
        }
    };
    let l = if k == 0 { Layout { running: a.0, other: b.0 } } else { Layout { running: b.0, other: a.0 } };
    info!("running from partition {} (source {})", if k == 0 { "A" } else { "B" }, source);
    found(if k == 0 { FwUpdates::PartitionA } else { FwUpdates::PartitionB }, source);
    Some(l)
}

/// Finds the partitions; call once at boot, before `Info.caps` is built.
pub fn init() -> Option<Layout> {
    let mut work = Work([0; SECTOR_LEN]);
    let l = discover(&mut work);
    LAYOUT.lock(|c| c.set(l));
    l
}

/// The self-test is satisfied: keep this image (does nothing unless it was started as a TBYB trial).
pub fn buy() {
    BUY.signal(());
}

fn do_buy(work: &mut Work) {
    // The ROM rewrites flash here: like embassy-rp's own flash calls, run with core 1 parked in RAM and no
    // interrupts (XIP is gone meanwhile).
    embassy_rp::multicore::pause_core1();
    let r = critical_section::with(|_| unsafe { rom_data::explicit_buy(work.0.as_mut_ptr(), work.0.len() as u32) });
    embassy_rp::multicore::resume_core1();
    info!("explicit_buy: {}", r);
    // 0 only when this boot was a trial of a new image, which ran under the ROM's watchdog: this image is now the
    // one to boot, so stop it. Otherwise (not a trial, or the buy failed) leave the watchdog as it is.
    if r == 0 {
        embassy_rp::pac::WATCHDOG.ctrl().modify(|w| w.set_enable(false));
        uart_log!("new firmware kept (explicit_buy)");
    }
}

struct Receiving {
    size: u32,
    crc32: u32,
    crc: Crc32,
    next: u32,
    /// The sector being filled, and the first sector (written last).
    sector: [u8; SECTOR_LEN],
    first: [u8; SECTOR_LEN],
}

fn fail(why: &str) {
    warn!("firmware update failed: {}", why);
    uart_log!("firmware update FAILED: {}", why);
    kvm::with(|e| {
        e.set_fw_busy(false);
        e.set_fw_state(FwState::Failed, 0);
    });
}

/// Writes sector `index` of the image into the other partition, with the engine refusing FwData meanwhile.
fn write(l: Layout, index: u32, data: &[u8; SECTOR_LEN]) -> bool {
    kvm::with(|e| e.set_fw_busy(true));
    let ok = flash::write_sector(l.other + index * SECTOR, data);
    kvm::with(|e| e.set_fw_busy(false));
    ok
}

#[embassy_executor::task]
pub async fn run() -> ! {
    let mut work = Work([0; SECTOR_LEN]);
    let mut rx: Option<Receiving> = None;
    loop {
        let op = match embassy_futures::select::select(OPS.receive(), BUY.wait()).await {
            embassy_futures::select::Either::First(op) => op,
            embassy_futures::select::Either::Second(()) => {
                do_buy(&mut work);
                continue;
            }
        };
        let Some(l) = layout() else {
            fail("no A/B partitions");
            continue;
        };
        match op {
            FwOp::Begin { size, crc32 } => {
                rx = None;
                if !(SECTOR..=PARTITION_LEN).contains(&size) {
                    fail("image size");
                    continue;
                }
                // Whatever the other partition held is no longer bootable from here on.
                kvm::with(|e| e.set_fw_busy(true));
                let ok = flash::with(|f| f.blocking_erase(l.other, l.other + SECTOR).is_ok());
                kvm::with(|e| e.set_fw_busy(false));
                if !ok {
                    fail("erase");
                    continue;
                }
                info!("receiving {} bytes for {=u32:#x}", size, l.other);
                rx = Some(Receiving {
                    size,
                    crc32,
                    crc: Crc32::new(),
                    next: 0,
                    sector: [0xFF; SECTOR_LEN],
                    first: [0xFF; SECTOR_LEN],
                });
            }
            FwOp::Data { offset, data } => {
                let Some(r) = rx.as_mut() else { continue };
                if offset != r.next || r.next + data.len() as u32 > r.size {
                    rx = None;
                    fail("data out of order");
                    continue;
                }
                // Chunks don't divide sectors evenly: split at the boundaries.
                let mut rest = &data[..];
                let mut ok = true;
                while ok && !rest.is_empty() {
                    let at = (r.next % SECTOR) as usize;
                    let n = rest.len().min(SECTOR_LEN - at);
                    r.sector[at..at + n].copy_from_slice(&rest[..n]);
                    r.crc.update(&rest[..n]);
                    r.next += n as u32;
                    rest = &rest[n..];
                    if r.next % SECTOR == 0 || r.next == r.size {
                        let index = (r.next - 1) / SECTOR;
                        if index == 0 {
                            r.first = r.sector;
                        } else {
                            ok = write(l, index, &r.sector);
                        }
                        r.sector = [0xFF; SECTOR_LEN];
                    }
                }
                if !ok {
                    rx = None;
                    fail("write");
                    continue;
                }
                let progress = r.next;
                kvm::with(|e| e.set_fw_state(FwState::Receiving, progress));
            }
            FwOp::Commit => {
                let Some(r) = rx.as_mut() else {
                    fail("commit without data");
                    continue;
                };
                if r.next != r.size || r.crc.finish() != r.crc32 {
                    rx = None;
                    fail("size or CRC mismatch");
                    continue;
                }
                if let Err(e) = rpimage::set_tbyb(&mut r.first) {
                    rx = None;
                    warn!("not an RP2350 Arm image: {}", e);
                    fail("image kind");
                    continue;
                }
                // The engine now shows AwaitingKey; the key press arrives as Confirm.
                info!("image complete; waiting for the key");
            }
            FwOp::Confirm => {
                let Some(r) = rx.take() else {
                    fail("confirm without an image");
                    continue;
                };
                if !write(l, 0, &r.first) {
                    fail("write");
                    continue;
                }
                kvm::with(|e| e.set_fw_state(FwState::Done, r.size));
                info!("installed; FLASH_UPDATE reboot into {=u32:#x}", l.other);
                uart_log!("firmware update installed; restarting into {:#x}", l.other);
                // Let `odkvmctl` see Done (it polls every 250 ms).
                Timer::after_millis(800).await;
                const REBOOT_TYPE_FLASH_UPDATE: u32 = 0x0004;
                rom_data::reboot(REBOOT_TYPE_FLASH_UPDATE, 10, XIP_BASE + l.other, 0);
            }
        }
    }
}
