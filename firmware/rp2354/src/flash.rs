//! The 2 MB flash, shared by the data store and the update writer (both on core 0).
//!
//! Layout (physical offsets, from `kvm_proto::rpimage::layout`; `cargo xtask dist` writes the partition table,
//! software.md §4.9):
//!
//! | Offset | Size | Use |
//! |---|---|---|
//! | 0 | 8 KB | partition table (boot ROM) |
//! | 8 KB | 896 KB | partition A: firmware |
//! | 904 KB | 896 KB | partition B: firmware (A/B pair with A) |
//! | 1800 KB | 244 KB | partition "data": config, keymap and macros (`kvm_core::store`, 61 sectors) |
//! | 2044 KB | 4 KB | left free (RP2350-E10 workaround block on A2 silicon) |
//!
//! Booted from a partition, XIP address 0x1000_0000 maps to that partition's start, but the ROM's erase and program
//! work on physical offsets. Reads of anything outside the running image therefore go through the untranslated XIP
//! alias ([`read`]).
//!
//! The data partition is used only once the partition table is known to be this one, or there is none (a debug
//! probe's image at the start of flash): on a board with the table of firmware before 0.1.51 it lies in that table's
//! partition B ([`set_layout_ours`]). The Bluetooth module's firmware is not kept here: the host software streams it
//! to the module through the RP2354B (`bt.rs`).

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::peripherals::FLASH;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use kvm_proto::rpimage::layout;

pub const SIZE: usize = 2 * 1024 * 1024;
pub const SECTOR: u32 = layout::SECTOR;
/// The partitions the partition table is expected to hold.
pub const PARTITION_A: u32 = layout::PARTITION_A_SECTOR * SECTOR;
pub const PARTITION_LEN: u32 = layout::PARTITION_SECTORS * SECTOR;
pub const DATA: u32 = layout::DATA_SECTOR * SECTOR;
pub const DATA_LEN: u32 = layout::DATA_SECTORS * SECTOR;
pub const DATA_SECTORS: usize = layout::DATA_SECTORS as usize;
const _: () = assert!(PARTITION_A + 2 * PARTITION_LEN == DATA);

/// Whether the data partition is where this firmware has it (`update::init`).
static OURS: AtomicBool = AtomicBool::new(false);

/// The partition table is this firmware's, or there is none: the data partition may be used.
pub fn set_layout_ours(ours: bool) {
    OURS.store(ours, Ordering::Relaxed);
}

pub fn layout_ours() -> bool {
    OURS.load(Ordering::Relaxed)
}

/// Uncached, untranslated XIP window (RP2350 datasheet §4.4).
const XIP_NOCACHE_NOALLOC_NOTRANSLATE_BASE: usize = 0x1C00_0000;

pub type RpFlash = Flash<'static, FLASH, Blocking, SIZE>;

static FLASH_DEV: Mutex<ThreadModeRawMutex, RefCell<Option<RpFlash>>> = Mutex::new(RefCell::new(None));

pub fn init(f: RpFlash) {
    FLASH_DEV.lock(|c| *c.borrow_mut() = Some(f));
}

/// Erase and program (physical offsets). Each call stalls XIP, and so both cores, for the duration.
pub fn with<R>(f: impl FnOnce(&mut RpFlash) -> R) -> R {
    FLASH_DEV.lock(|c| f(c.borrow_mut().as_mut().expect("flash not initialised")))
}

/// Reads at a physical offset, bypassing partition address translation and the XIP cache.
pub fn read(offset: u32, out: &mut [u8]) {
    let base = (XIP_NOCACHE_NOALLOC_NOTRANSLATE_BASE + offset as usize) as *const u8;
    for (i, b) in out.iter_mut().enumerate() {
        // SAFETY: inside the 16 MB XIP window; flash reads have no side effects.
        *b = unsafe { core::ptr::read_volatile(base.add(i)) };
    }
}

/// Erases and programs one whole sector, then reads it back.
pub fn write_sector(offset: u32, data: &[u8; SECTOR as usize]) -> bool {
    let ok = with(|f| f.blocking_erase(offset, offset + SECTOR).is_ok() && f.blocking_write(offset, data).is_ok());
    let mut back = [0u8; 256];
    ok && data.chunks(256).enumerate().all(|(i, chunk)| {
        read(offset + (i * 256) as u32, &mut back);
        back == chunk
    })
}

/// The data partition, as the store's flash (offsets relative to its start).
pub struct DataFlash;

impl kvm_core::store::Nor for DataFlash {
    fn read(&mut self, at: u32, buf: &mut [u8]) {
        read(DATA + at, buf);
    }

    fn erase(&mut self, at: u32) -> bool {
        layout_ours() && at + SECTOR <= DATA_LEN && with(|f| f.blocking_erase(DATA + at, DATA + at + SECTOR).is_ok())
    }

    fn program(&mut self, at: u32, data: &[u8]) -> bool {
        // embassy-rp pads partial pages with 0xFF, which leaves the bytes around `data` as they are.
        layout_ours()
            && at as usize + data.len() <= DATA_LEN as usize
            && with(|f| f.blocking_write(DATA + at, data).is_ok())
    }
}
