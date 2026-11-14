//! OpenDynKVM firmware for the Bluetooth LE coprocessor, an ESP32-C3-MINI-1 on boards 0.16.0 and later (software.md
//! §4.13, design.md §13).
//!
//! The module is a Bluetooth LE keyboard and mouse towards up to four bonded hosts (PCs, tablets, phones), all
//! connected at once. The RP2354B decides which host gets input and sends it the reports over UART0
//! ([`kvm_proto::link::bt`]); this firmware only keeps the hosts: advertising, pairing (LE Secure Connections,
//! passkey typed on the KVM's keyboard), bonds in flash, HID over GATT, and the keyboard LEDs back.
//!
//! - [`link`]: UART0 (RXD0/IO20, TXD0/IO21: the ROM bootloader's pins, which the RP2354B also programs it through), the
//!   AT32F405 links' frames and ARQ, the module answering each frame the RP2354B sends.
//! - [`ble`]: the trouble-host stack on esp-radio's controller.
//! - [`bonds`]: the four host slots in the last two sectors of the flash, which the RP2354B's image never reaches.
//!
//! IO9 (the BOOT strap) is on the RP2354B's TX line: it is never driven here. IO3 carries a plain-text log (UART1 TX,
//! 115200) to the board's test pad TP1601.

#![no_std]
#![no_main]

extern crate alloc;

mod ble;
mod bonds;
mod link;
mod log;

use embassy_executor::Spawner;
use esp_rtos::embassy::InterruptExecutor;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::{MwdtStage, TimerGroup, Wdt};

esp_bootloader_esp_idf::esp_app_desc!();

/// This firmware's version, as `Hello` reports it and the RP2354B compares it with its embedded image's.
pub const FW_VERSION: u32 = {
    let (major, minor, patch) = kvm_proto::FIRMWARE_VERSION;
    kvm_proto::version_u32(major, minor, patch)
};

/// The marker the RP2354B's `build.rs` finds the version by in the flash image: magic, then the version.
#[used]
#[unsafe(no_mangle)]
static KVM_ESP_FW_ID: [u8; 12] = {
    let v = FW_VERSION.to_le_bytes();
    [b'K', b'V', b'M', b'E', b'S', b'P', b'3', b'2', v[0], v[1], v[2], v[3]]
};

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    log::line(format_args!("panic: {info}"));
    // The RP2354B sees the link drop and the module come back.
    esp_hal::system::software_reset()
}

static LINK_EXECUTOR: static_cell::StaticCell<InterruptExecutor<1>> = static_cell::StaticCell::new();

/// A hang this long restarts the module (a system reset); the watchdog is fed every second.
const WATCHDOG_S: u64 = 5;

/// Feeds the module's watchdog from the main executor, where the Bluetooth side runs: when that executor stops, the
/// module restarts, and the RP2354B sees it say `Hello` again (or catches it in its ROM with a break on TX).
#[embassy_executor::task]
async fn watchdog(mut wdt: Wdt<esp_hal::peripherals::TIMG0<'static>>) {
    loop {
        wdt.feed();
        embassy_time::Timer::after_secs(1).await;
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    // The Bluetooth controller's own memory (esp-radio allocates it).
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    let mut wdt = timg0.wdt;
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    // The module's own watchdog: on the KVM's board nothing else can restart a module that hangs (its EN has only its
    // RC; boards before 0.18.0 have no line to it from the RP2354B). esp-hal's init leaves every watchdog off.
    wdt.set_timeout(MwdtStage::Stage0, esp_hal::time::Duration::from_secs(WATCHDOG_S));
    wdt.enable();
    spawner.spawn(watchdog(wdt).expect("watchdog task"));
    // Kept until the link says where the module is (`log`): printed on IO3 only on the KVM's board.
    log::line(format_args!("OpenDynKVM Bluetooth module, firmware {FW_VERSION:#08x}"));

    // The link: UART0 on the KVM's board, or the chip's USB serial port on a front port (software.md §4.13).
    // It runs one interrupt level above the Bluetooth side: pairing's P-256 maths and a bond being written can
    // keep the main executor busy for long stretches, and the RP2354B takes a link that doesn't answer for gone (first
    // dongle, 2026-11-14: pairing broke off with the module restarted).
    let link_executor = LINK_EXECUTOR.init(InterruptExecutor::new(p.FROM_CPU_INTR1));
    let link_spawner = link_executor.start(esp_hal::interrupt::Priority::Priority1);
    let lines = link::Lines { uart: p.UART0, rx: p.GPIO20, tx: p.GPIO21, usb: p.USB_DEVICE, log: p.UART1, log_pin: p.GPIO3 };
    link_spawner.spawn(link::run(lines).expect("link task"));

    let bonds = bonds::Bonds::load(esp_storage::FlashStorage::new(p.FLASH));
    ble::run(spawner, p.BT, bonds).await
}
