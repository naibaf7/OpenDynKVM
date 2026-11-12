//! Config, keymap and macros in the data partition, through the wear-levelled store (software.md §4.8; layout in
//! [`crate::flash`]), and the counts of failed programming attempts of the AT32F405s (§4.9) and the Bluetooth module
//! (§4.13), [`kvm_core::at32prog`].

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};
use kvm_core::Engine;
use kvm_core::at32prog::{self, Record};
use kvm_core::config::{self, Config};
use kvm_core::engine::SAVE_ITEM_MAX;
use kvm_core::store::Store;

use crate::flash::{DATA_SECTORS, DataFlash};
use crate::kvm;

/// Failed programming attempts in a row per AT32F405, for the embedded image ([`IMAGE_CRC`]).
static FAILS: [AtomicU8; 2] = [const { AtomicU8::new(0) }; 2];
static IMAGE_CRC: AtomicU32 = AtomicU32::new(0);
/// The same for the Bluetooth module, for its embedded image; [`BT_IMAGE_CRC`] stays 0 without a module (or without
/// an image), and then its record is never written.
static BT_FAILS: AtomicU8 = AtomicU8::new(0);
static BT_IMAGE_CRC: AtomicU32 = AtomicU32::new(0);
/// Writes of the records asked for and done; the storage task (core 0) does them, as all flash writes.
static RECORD_ASKED: AtomicU32 = AtomicU32::new(0);
static RECORD_DONE: AtomicU32 = AtomicU32::new(0);
static RECORD_SAVE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Longer than a write with a garbage collection; without a store nothing is written and nobody waits.
const RECORD_WAIT: Duration = Duration::from_secs(2);

/// Mounts the store; returns it with the saved config (defaults if there is none, or no usable store).
pub fn open() -> (Option<Store>, Config) {
    let mut f = DataFlash;
    match Store::mount(&mut f, DATA_SECTORS) {
        Ok(s) => {
            info!("store: {} bytes current, at most {} erases per sector", s.live_bytes(), s.max_erases());
            let mut buf = [0u8; config::ITEM_MAX];
            let cfg = match s.read(&mut f, config::ITEM, &mut buf) {
                Some(n) => Config::decode(&buf[..n.min(buf.len())]),
                None => {
                    info!("no saved config: defaults");
                    Config::default()
                }
            };
            (Some(s), cfg)
        }
        Err(e) => {
            warn!("store unusable ({}): defaults, nothing is saved", e);
            (None, Config::default())
        }
    }
}

/// Loads the programming record for the embedded AT32F405 image (`image_crc`); a record of another image counts as
/// none. `forget`: the KVM's key was held at power-up, so the count starts again at 0.
pub fn load_at32_record(store: &Option<Store>, image_crc: u32, forget: bool) {
    IMAGE_CRC.store(image_crc, Ordering::Relaxed);
    let mut buf = [0u8; Record::LEN];
    let saved = store.as_ref().and_then(|s| s.read(&mut DataFlash, at32prog::ITEM, &mut buf));
    let r = saved.map_or(Record { image_crc, fails: [0; 2] }, |n| Record::decode(&buf[..n.min(Record::LEN)], image_crc));
    let fails = if forget { [0; 2] } else { r.fails };
    for k in 0..2 {
        FAILS[k].store(fails[k], Ordering::Relaxed);
    }
    if fails != [0; 2] {
        info!("AT32F405 programming: {} / {} failed attempts in a row so far", fails[0], fails[1]);
    }
    if forget && r.fails != [0; 2] {
        RECORD_ASKED.fetch_add(1, Ordering::Relaxed);
        RECORD_SAVE.signal(());
    }
}

pub fn at32_fails(k: usize) -> u8 {
    FAILS[k].load(Ordering::Relaxed)
}

/// Asks the storage task to write the records, and waits until it has.
async fn save_records() {
    let asked = RECORD_ASKED.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    RECORD_SAVE.signal(());
    let written = async {
        while (RECORD_DONE.load(Ordering::Relaxed).wrapping_sub(asked) as i32) < 0 {
            Timer::after_millis(5).await;
        }
    };
    if with_timeout(RECORD_WAIT, written).await.is_err() {
        warn!("programming record not written in time");
    }
}

/// Sets AT32F405 `k`'s count and waits until it is in flash (before an attempt, so that a power loss during it counts).
pub async fn set_at32_fails(k: usize, n: u8) {
    FAILS[k].store(n, Ordering::Relaxed);
    save_records().await;
}

/// Loads the Bluetooth module's programming record for its embedded image (`image_crc`), as [`load_at32_record`] does;
/// only when a module is fitted.
pub fn load_bt_record(store: &Option<Store>, image_crc: u32, forget: bool) {
    BT_IMAGE_CRC.store(image_crc, Ordering::Relaxed);
    let mut buf = [0u8; Record::LEN];
    let saved = store.as_ref().and_then(|s| s.read(&mut DataFlash, at32prog::BT_ITEM, &mut buf));
    let r = saved.map_or(Record { image_crc, fails: [0; 2] }, |n| Record::decode(&buf[..n.min(Record::LEN)], image_crc));
    let fails = if forget { 0 } else { r.fails[0] };
    BT_FAILS.store(fails, Ordering::Relaxed);
    if fails != 0 {
        info!("Bluetooth module programming: {} failed attempts in a row so far", fails);
    }
    if forget && r.fails[0] != 0 {
        RECORD_ASKED.fetch_add(1, Ordering::Relaxed);
        RECORD_SAVE.signal(());
    }
}

pub fn bt_fails() -> u8 {
    BT_FAILS.load(Ordering::Relaxed)
}

/// Sets the Bluetooth module's count and waits until it is in flash (before an attempt).
pub async fn set_bt_fails(n: u8) {
    BT_FAILS.store(n, Ordering::Relaxed);
    save_records().await;
}

/// Back to 0 for the Bluetooth module (it answers with the embedded firmware); written without waiting.
pub fn clear_bt_fails() {
    if BT_FAILS.swap(0, Ordering::Relaxed) != 0 {
        RECORD_ASKED.fetch_add(1, Ordering::Relaxed);
        RECORD_SAVE.signal(());
    }
}

/// Back to 0 for AT32F405 `k` (it answers with the embedded firmware); written without waiting.
pub fn clear_at32_fails(k: usize) {
    if FAILS[k].swap(0, Ordering::Relaxed) != 0 {
        RECORD_ASKED.fetch_add(1, Ordering::Relaxed);
        RECORD_SAVE.signal(());
    }
}

fn write_record(store: &mut Option<Store>) {
    let asked = RECORD_ASKED.load(Ordering::Relaxed);
    let r = Record {
        image_crc: IMAGE_CRC.load(Ordering::Relaxed),
        fails: [FAILS[0].load(Ordering::Relaxed), FAILS[1].load(Ordering::Relaxed)],
    };
    match store.as_mut().map(|s| s.write(&mut DataFlash, at32prog::ITEM, &r.encode())) {
        Some(Ok(())) => info!("AT32F405 programming record saved: {}", r),
        Some(Err(e)) => warn!("AT32F405 programming record not saved: {}", e),
        None => {}
    }
    let bt_crc = BT_IMAGE_CRC.load(Ordering::Relaxed);
    if bt_crc != 0 {
        let r = Record { image_crc: bt_crc, fails: [BT_FAILS.load(Ordering::Relaxed), 0] };
        match store.as_mut().map(|s| s.write(&mut DataFlash, at32prog::BT_ITEM, &r.encode())) {
            Some(Ok(())) => info!("Bluetooth module programming record saved: {}", r),
            Some(Err(e)) => warn!("Bluetooth module programming record not saved: {}", e),
            None => {}
        }
    }
    RECORD_DONE.store(asked, Ordering::Relaxed);
}

/// Hands the saved keymap items to the engine (after `kvm::init`).
pub fn restore(store: &Option<Store>) {
    let Some(s) = store else { return };
    let mut buf = [0u8; SAVE_ITEM_MAX];
    for key in Engine::item_keys() {
        if let Some(n) = s.read(&mut DataFlash, key, &mut buf) {
            kvm::with(|e| e.restore_item(key, &buf[..n.min(buf.len())]));
        }
    }
}

/// Writes what the engine reports as changed, item by item. Each write stalls flash (XIP) for both cores briefly;
/// embassy-rp pauses core 1 around every program and erase.
#[embassy_executor::task]
pub async fn run(mut store: Option<Store>) -> ! {
    let mut buf = [0u8; SAVE_ITEM_MAX];
    loop {
        if let Either::Second(()) = select(kvm::SAVE.wait(), RECORD_SAVE.wait()).await {
            write_record(&mut store);
            continue;
        }
        while let Some((key, n)) = kvm::with(|e| e.take_save_item(&mut buf)) {
            match store.as_mut().map(|s| s.write(&mut DataFlash, key, &buf[..n])) {
                Some(Ok(())) => info!("saved item {=u16:#x}, {} bytes", key, n),
                Some(Err(e)) => warn!("saving item {=u16:#x} failed: {}", key, e),
                None => warn!("no store: item {=u16:#x} not saved", key),
            }
        }
        kvm::with(|e| e.config_saved());
    }
}
