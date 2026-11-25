//! OpenDynKVM firmware for the Bluetooth LE coprocessor, an ESP32-C3-MINI-1 on boards 0.16.0 and later (software.md
//! §4.13, design.md §13).
//!
//! The module is a Bluetooth LE keyboard and mouse towards up to four bonded hosts (PCs, tablets, phones), all
//! connected at once. The RP2354B decides which host gets input and sends it the reports over UART0
//! ([`kvm_proto::link::bt`]); this firmware only keeps the hosts: advertising, pairing (LE Secure Connections,
//! passkey typed on the KVM's keyboard), bonds in flash, HID over GATT, and the keyboard LEDs back. At the same time it
//! is the central of up to four Bluetooth devices (keyboards, mice), whose reports it passes to the RP2354B.
//!
//! - [`link`]: UART0 (RXD0/IO20, TXD0/IO21: the ROM bootloader's pins, which the RP2354B also programs it through), the
//!   AT32F405 links' frames and ARQ, the module answering each frame the RP2354B sends.
//! - [`ble`]: the trouble-host stack on esp-radio's controller.
//! - [`devices`]: the Bluetooth devices: looking for them, pairing, reconnecting, their reports.
//! - [`bonds`]: the four host slots in the last two sectors of the flash, the four device slots in the two before,
//!   which the RP2354B's image never reaches.
//! - [`radio`]: the controller as the stack's HCI.
//! - [`stall`]: a report that doesn't go out for long restarts the module.
//!
//! IO9 (the BOOT strap) is on the RP2354B's TX line: it is never driven here. IO3 carries a plain-text log (UART1 TX,
//! 115200) to the board's test pad TP1601.

#![no_std]
#![no_main]

extern crate alloc;

mod ble;
mod bonds;
mod devices;
mod link;
mod log;
mod radio;
mod stall;

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

/// The last panic's place and message, kept across the reset it ends in (RTC fast RAM is not cleared by it): its line
/// can't reach the RP2354B before the reset, so the next start sends it (`odkvmctl device log`). Magic, length, text.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut LAST_PANIC: [u8; 132] = [0; 132];
const PANIC_MAGIC: [u8; 4] = *b"PNC!";

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    log::line(format_args!("panic: {info}"));
    /// Takes what fits and drops the rest (a dependency's path alone is longer than the room).
    struct Cut<'a>(&'a mut heapless::String<127>);
    impl Write for Cut<'_> {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for c in s.chars() {
                if self.0.push(c).is_err() {
                    break;
                }
            }
            Ok(())
        }
    }
    let mut text: heapless::String<127> = heapless::String::new();
    let mut w = Cut(&mut text);
    // The end of the file's path (the crate and file), the line, then the message.
    if let Some(l) = info.location() {
        let file = l.file();
        let mut from = file.len().saturating_sub(48);
        while !file.is_char_boundary(from) {
            from += 1;
        }
        let _ = write!(w, "{}:{}: ", &file[from..], l.line());
    }
    let _ = write!(w, "{}", info.message());
    // SAFETY: nothing else runs any more; the reset follows.
    let kept = unsafe { &mut *core::ptr::addr_of_mut!(LAST_PANIC) };
    kept[..4].copy_from_slice(&PANIC_MAGIC);
    kept[4] = text.len() as u8;
    kept[5..5 + text.len()].copy_from_slice(text.as_bytes());
    // The RP2354B sees the link drop and the module come back.
    esp_hal::system::software_reset()
}

/// The message of the panic the last run ended in, once (it is cleared).
fn last_panic() -> Option<heapless::String<127>> {
    // SAFETY: read and cleared at start, before anything else runs.
    let kept = unsafe { &mut *core::ptr::addr_of_mut!(LAST_PANIC) };
    if kept[..4] != PANIC_MAGIC {
        return None;
    }
    kept[..4].fill(0);
    let len = (kept[4] as usize).min(127);
    heapless::String::try_from(core::str::from_utf8(&kept[5..5 + len]).unwrap_or("(unreadable)")).ok()
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
    // The Bluetooth controller's own memory (esp-radio allocates it), for ten activities at once.
    esp_alloc::heap_allocator!(size: 96 * 1024);
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
    // A panic or a hang restarts the module: the next start says so (Software or a watchdog).
    log::line(format_args!("started after a reset: {:?}", esp_hal::system::reset_reason()));
    if let Some(text) = last_panic() {
        log::line(format_args!("the last run ended in a panic: {text}"));
    }
    log::stack_lines();

    // The link: UART0 on the KVM's board, or the chip's USB serial port on a front port (software.md §4.13).
    // It runs one interrupt level above the Bluetooth side: pairing's P-256 maths and a bond being written can
    // keep the main executor busy for long stretches, and the RP2354B takes a link that doesn't answer for gone (first
    // dongle, 2026-11-14: pairing broke off with the module restarted).
    let link_executor = LINK_EXECUTOR.init(InterruptExecutor::new(p.FROM_CPU_INTR1));
    let link_spawner = link_executor.start(esp_hal::interrupt::Priority::Priority1);
    let lines = link::Lines { uart: p.UART0, rx: p.GPIO20, tx: p.GPIO21, usb: p.USB_DEVICE, log: p.UART1, log_pin: p.GPIO3 };
    link_spawner.spawn(link::run(lines).expect("link task"));

    // Two handles on the flash, used one at a time on the Bluetooth side's executor: the hosts' sectors and the
    // devices'. Both made before either reads: a handle learns the flash's size from the chip's ID (RDID through SPI1),
    // and after a read through the ROM's routines that query comes back wrong. Until 0.1.41 the devices' handle was
    // made after the hosts' bonds were read, took the flash for 0 bytes long, and refused every read and write
    // (OutOfBounds): no device's bond was ever saved, and every module restart lost them all (2026-11-20).
    let device_flash = unsafe { p.FLASH.clone_unchecked() };
    let host_storage = esp_storage::FlashStorage::new(p.FLASH);
    let device_storage = esp_storage::FlashStorage::new(device_flash);
    if device_storage.capacity() != host_storage.capacity() {
        log::line(format_args!(
            "flash: {} bytes for the hosts' bonds, {} for the devices'",
            host_storage.capacity(),
            device_storage.capacity()
        ));
    }
    let bonds = bonds::Bonds::load(host_storage);
    let device_bonds = bonds::DeviceBonds::load(device_storage);
    ble::run(spawner, p.BT, bonds, device_bonds).await
}
