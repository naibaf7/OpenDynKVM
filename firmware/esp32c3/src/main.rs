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
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::TimerGroup;

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

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    // The Bluetooth controller's own memory (esp-radio allocates it).
    esp_alloc::heap_allocator!(size: 72 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    log::init(p.UART1, p.GPIO3);
    log::line(format_args!("OpenDynKVM Bluetooth module, firmware {FW_VERSION:#08x}"));

    let uart = esp_hal::uart::Uart::new(
        p.UART0,
        esp_hal::uart::Config::default().with_baudrate(kvm_proto::link::bt::BAUD),
    )
    .expect("UART0")
    .with_rx(p.GPIO20)
    .with_tx(p.GPIO21)
    .into_async();
    spawner.spawn(link::run(uart).expect("link task"));

    let bonds = bonds::Bonds::load(esp_storage::FlashStorage::new(p.FLASH));
    ble::run(spawner, p.BT, bonds).await
}
