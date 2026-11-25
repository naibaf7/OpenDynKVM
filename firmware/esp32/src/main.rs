//! OpenDynKVM firmware for the Bluetooth module on an original ESP32 (software.md §4.13): an ESP32 board on a front
//! port whose USB-to-UART bridge (a CP2104, say) is the module's line, as an ESP32-C3 board's own USB serial port is,
//! or (on a board to come) an ESP32 wired to the RP2354B's UART as the ESP32-C3 module is.
//!
//! It does all the ESP32-C3 firmware does, with that firmware's modules (`firmware/esp32c3/src`, by path): the
//! Bluetooth LE hosts and LE devices, the link, the bonds, the log. The ESP32's radio is dual mode, and so it is a
//! Bluetooth Classic host too ([`classic`]): for gamepads that pair only over Classic, whose reports go to the RP2354B
//! as an LE device's do, in the same device slots.
//!
//! - [`link`]: UART0 (GPIO1 TX, GPIO3 RX) at the board's 3 Mbaud or the bridge's `BRIDGE_BAUD`, whichever the first
//!   valid frame comes at; no log pin.
//! - [`radio`]: the controller as the LE stack's HCI, with the Classic traffic taken out for [`classic`].
//! - [`classic`]: inquiry, pairing (Secure Simple Pairing, Just Works), the link keys in the device slots, L2CAP's HID
//!   channels, and the pads' reports.
//!
//! The ESP32's GPIO0 is its BOOT strap, which the bridge's DTR drives (its RTS drives EN): the RP2354B restarts it into
//! its ROM with those, as esptool does.

#![no_std]
#![no_main]
// The hang sampler reads EPC3/EPS3 (`hang`).
#![feature(asm_experimental_arch)]

extern crate alloc;

#[path = "../../esp32c3/src/ble.rs"]
mod ble;
#[path = "../../esp32c3/src/bonds.rs"]
mod bonds;
#[cfg_attr(not(feature = "classic"), allow(dead_code))]
mod classic;
#[path = "../../esp32c3/src/devices.rs"]
mod devices;
mod hang;
#[path = "../../esp32c3/src/link.rs"]
mod link;
#[path = "../../esp32c3/src/log.rs"]
mod log;
mod radio;
#[path = "../../esp32c3/src/stall.rs"]
mod stall;

use embassy_executor::Spawner;
use esp_hal::clock::CpuClock;
use esp_hal::timer::timg::{MwdtStage, TimerGroup, Wdt};
use esp_rtos::embassy::InterruptExecutor;

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
static mut LAST_PANIC: [u8; 4 + 1 + PANIC_LEN] = [0; 4 + 1 + PANIC_LEN];
const PANIC_MAGIC: [u8; 4] = *b"PNC!";
const PANIC_LEN: usize = 250;

/// The panic's message as formatted, before `panic` cuts it down: an exception's is its registers, some 700
/// characters. Static, as the stack may be what failed.
static mut PANIC_TEXT: heapless::String<1024> = heapless::String::new();

unsafe extern "C" {
    /// The ROM's printf, on UART0 at whatever speed it runs (until [`rom_out`] takes what it prints).
    fn esp_rom_printf(fmt: *const core::ffi::c_char, ...) -> i32;
    /// The ROM's printf's character output.
    fn ets_install_putc1(putc: unsafe extern "C" fn(core::ffi::c_char));
}

/// What the ROM's printf prints once the link has UART0: the Bluetooth controller's own lines, its asserts among them
/// ("ASSERT_PARAM(%d %d), in %s at line %d", then it stops), which on UART0 would only be noise on the link. Kept in
/// RTC fast RAM, a reset doesn't clear it: [`rom_out`] sends its lines to the log as they come, and the next start
/// what the last run had no time for (2026-11-25: the controller asserted in its page scan, and said where only on
/// UART0).
#[repr(C)]
struct RomOut {
    magic: u32,
    /// Bytes printed, and how many of them went to the log.
    head: u32,
    logged: u32,
    buf: [u8; ROM_OUT_LEN],
}
const ROM_OUT_LEN: usize = 512;
const ROM_OUT_MAGIC: u32 = u32::from_le_bytes(*b"ROM!");

/// [`RomOut`]'s room, as words (what esp-hal keeps across a reset).
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut ROM_OUT_WORDS: [u32; 3 + ROM_OUT_LEN / 4] = [0; 3 + ROM_OUT_LEN / 4];

/// [`ROM_OUT_WORDS`] as the [`RomOut`] it holds (the same size and alignment, all of it plain bytes).
fn rom_out_ptr() -> *mut RomOut {
    const { assert!(core::mem::size_of::<RomOut>() == 4 * (3 + ROM_OUT_LEN / 4)) };
    (&raw mut ROM_OUT_WORDS).cast::<RomOut>()
}
/// The ROM's printf goes to [`RomOut`] (not UART0) from here on.
static ROM_HOOKED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The ROM's printf's character output from [`hook_rom_out`] on (in IRAM: the controller's interrupt prints).
#[esp_hal::ram]
unsafe extern "C" fn rom_putc(c: core::ffi::c_char) {
    // SAFETY: a byte and a counter in RAM; two printers at once may mix their bytes, nothing worse.
    unsafe {
        let r = rom_out_ptr();
        let head = (*r).head;
        (*r).buf[head as usize % ROM_OUT_LEN] = c as u8;
        (*r).head = head.wrapping_add(1);
    }
}

/// The lines of [`RomOut`] not yet logged, each with `prefix`; `all`: the last one too, without its line end.
fn log_rom_out(prefix: &str, all: bool) {
    // SAFETY: read by this task alone; `rom_putc` only appends.
    let r = unsafe { &mut *rom_out_ptr() };
    let head = unsafe { core::ptr::read_volatile(&r.head) };
    if head.wrapping_sub(r.logged) as usize > ROM_OUT_LEN {
        r.logged = head.wrapping_sub(ROM_OUT_LEN as u32);
    }
    let mut line: heapless::String<100> = heapless::String::new();
    let mut at = r.logged;
    while at != head {
        let c = r.buf[at as usize % ROM_OUT_LEN];
        at = at.wrapping_add(1);
        if c == b'\n' || line.len() == line.capacity() {
            if !line.trim().is_empty() {
                log::line(format_args!("{prefix}{}", line.trim_end()));
            }
            line.clear();
            r.logged = at;
            if c == b'\n' {
                continue;
            }
        }
        if c != b'\r' {
            let _ = line.push(if c.is_ascii_graphic() || c == b' ' { c as char } else { '?' });
        }
    }
    if all {
        if !line.trim().is_empty() {
            log::line(format_args!("{prefix}{}", line.trim_end()));
        }
        r.logged = head;
    }
}

/// The ROM's printf into [`RomOut`], after the lines the last run left there went to the log.
fn hook_rom_out() {
    // SAFETY: at start, before anything prints through the hook.
    let r = unsafe { &mut *rom_out_ptr() };
    if r.magic == ROM_OUT_MAGIC && r.head != r.logged {
        log_rom_out("before the restart, the controller printed: ", true);
    }
    r.magic = ROM_OUT_MAGIC;
    r.head = 0;
    r.logged = 0;
    // SAFETY: a function that stays.
    unsafe { ets_install_putc1(rom_putc) };
    ROM_HOOKED.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// [`RomOut`]'s lines into the log as they come.
#[embassy_executor::task]
async fn rom_out() {
    loop {
        embassy_time::Timer::after_millis(200).await;
        log_rom_out("controller: ", false);
    }
}

/// Takes what fits and drops the rest (a dependency's path alone is longer than some of the room).
struct Cut<'a, const N: usize>(&'a mut heapless::String<N>);

impl<const N: usize> core::fmt::Write for Cut<'_, N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            if self.0.push(c).is_err() {
                break;
            }
        }
        Ok(())
    }
}

/// A register of an exception's dump (esp-hal's `Context { PC: 1074316071, PS: 396, A0: …`), by name.
fn register(dump: &str, name: &str) -> Option<u32> {
    let mut rest = dump;
    while let Some(at) = rest.find(name) {
        let before = rest[..at].chars().next_back();
        rest = &rest[at + name.len()..];
        if matches!(before, Some(' ' | '{')) && rest.starts_with(": ") {
            let digits = rest[2..].split(|c: char| !c.is_ascii_digit()).next()?;
            return digits.parse().ok();
        }
    }
    None
}

/// The return addresses of the frames above the one at `sp`, from the windowed ABI's save areas (the caller's a0 and
/// a1 16 bytes below each frame's stack pointer; the exception handler spilled the register windows): as far as they
/// stay in the data RAM, which reads can't fault.
fn backtrace(mut sp: u32, out: &mut heapless::Vec<u32, 8>) {
    let ram = 0x3FFA_E000..0x3FFF_FFF0u32;
    while out.len() < out.capacity() && ram.contains(&sp) && sp.is_multiple_of(4) && sp >= ram.start + 16 {
        // SAFETY: an aligned word in the data RAM.
        let (ra, next) =
            unsafe { (((sp - 16) as *const u32).read_volatile(), ((sp - 12) as *const u32).read_volatile()) };
        if ra == 0 || next <= sp {
            break;
        }
        // The top two bits hold the caller's window increment, not the address's.
        let _ = out.push((ra & 0x3FFF_FFFF) | 0x4000_0000);
        sp = next;
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use core::fmt::Write;
    log::line(format_args!("panic: {info}"));
    // SAFETY: nothing else runs any more; the reset follows.
    let full = unsafe { &mut *core::ptr::addr_of_mut!(PANIC_TEXT) };
    full.clear();
    let _ = write!(Cut(full), "{}", info.message());
    let mut text: heapless::String<PANIC_LEN> = heapless::String::new();
    let mut w = Cut(&mut text);
    if let Some(l) = info.location() {
        let file = l.file();
        let mut from = file.len().saturating_sub(40);
        while !file.is_char_boundary(from) {
            from += 1;
        }
        let _ = write!(w, "{}:{}: ", &file[from..], l.line());
    }
    match full.find("Context {") {
        // An exception (esp-hal's handler panics with its registers): its kind, the registers that say where, and the
        // return addresses above, in hex for addr2line.
        Some(at) => {
            let _ = write!(w, "{}", full[..at].trim().trim_start_matches("Exception occurred on "));
            let dump = &full[at..];
            for name in ["PC", "A0", "A1", "PS", "EXCCAUSE", "EXCVADDR", "A2", "A3", "A4", "A5"] {
                if let Some(v) = register(dump, name) {
                    let _ = write!(w, " {name} {v:08x}");
                }
            }
            let mut frames = heapless::Vec::new();
            backtrace(register(dump, "A1").unwrap_or(0), &mut frames);
            let _ = write!(w, " bt");
            for ra in frames {
                let _ = write!(w, " {ra:08x}");
            }
        }
        None => {
            let _ = write!(w, "{full}");
        }
    }
    // Also on UART0, through the ROM's printf: before the link starts that is the bootloader's 115 200 baud, so that a
    // board on the bench that panics at start says why (its log lines otherwise reach only the RP2354B).
    // Not into `RomOut` once it takes the ROM's printf: the record is enough.
    if !ROM_HOOKED.load(core::sync::atomic::Ordering::Relaxed) {
        let mut z = [0u8; PANIC_LEN + 1];
        z[..text.len()].copy_from_slice(text.as_bytes());
        // SAFETY: a NUL-terminated format and string.
        unsafe { esp_rom_printf(c"OpenDynKVM module panic: %s\n".as_ptr(), z.as_ptr()) };
        // Out of the UART's FIFO before the reset (250 characters take 22 ms).
        esp_hal::delay::Delay::new().delay_millis(40);
    }
    // SAFETY: nothing else runs any more; the reset follows.
    let kept = unsafe { &mut *core::ptr::addr_of_mut!(LAST_PANIC) };
    kept[..4].copy_from_slice(&PANIC_MAGIC);
    kept[4] = text.len() as u8;
    kept[5..5 + text.len()].copy_from_slice(text.as_bytes());
    esp_hal::system::software_reset()
}

/// The message of the panic the last run ended in, once (it is cleared).
fn last_panic() -> Option<heapless::String<PANIC_LEN>> {
    // SAFETY: read and cleared at start, before anything else runs.
    let kept = unsafe { &mut *core::ptr::addr_of_mut!(LAST_PANIC) };
    if kept[..4] != PANIC_MAGIC {
        return None;
    }
    kept[..4].fill(0);
    let len = (kept[4] as usize).min(PANIC_LEN);
    heapless::String::try_from(core::str::from_utf8(&kept[5..5 + len]).unwrap_or("(unreadable)")).ok()
}

static LINK_EXECUTOR: static_cell::StaticCell<InterruptExecutor<1>> = static_cell::StaticCell::new();

/// A hang this long restarts the module (a system reset); the watchdog is fed every second. Less than the RP2354B's
/// `LINK_TIMEOUT` (3 s), after which it power-cycles a silent module and RTC RAM, with [`hang`]'s samples, is gone.
const WATCHDOG_S: u64 = 2;

/// The heap's use once Bluetooth is up, into the log: what the controller takes.
#[embassy_executor::task]
async fn heap_report() {
    embassy_time::Timer::after_secs(10).await;
    log::line(format_args!("heap: {} bytes used, {} free", esp_alloc::HEAP.used(), esp_alloc::HEAP.free()));
}

/// Feeds the module's watchdog from the main executor, where the Bluetooth side runs.
#[embassy_executor::task]
async fn watchdog(mut wdt: Wdt<esp_hal::peripherals::TIMG0<'static>>) {
    loop {
        wdt.feed();
        hang::fed();
        embassy_time::Timer::after_secs(1).await;
    }
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    // The Bluetooth controller's own memory (esp-radio allocates it), in the DRAM after the ROM's data (98 KB), with the
    // LE stack's resources (23 KB, `ble::RESOURCES`): with Bluetooth on, the ESP32's main DRAM is 128 KB (esp-hal's
    // RESERVE_DRAM), which the other statics and the stack share. The ESP32's controller keeps most of its memory in
    // its own reserved region (the ESP32-C3's takes 96 KB of heap); `heap_report` says what it uses.
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 72 * 1024);
    // And 12 KB of the main DRAM: an LE device's connection keeps its state on the heap while it lasts (5 KB, and its
    // GATT client's 5), so that every device slot can hold one, and four of them take 40 KB. What that gave the main
    // stack back (the connection tasks' static room) is more than it needs (some 30 KB at start, `-Z
    // emit-stack-sizes`).
    esp_alloc::heap_allocator!(size: 12 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    let mut wdt = timg0.wdt;
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);
    wdt.set_timeout(MwdtStage::Stage0, esp_hal::time::Duration::from_secs(WATCHDOG_S));
    wdt.enable();
    spawner.spawn(watchdog(wdt).expect("watchdog task"));
    spawner.spawn(heap_report().expect("heap report task"));
    let mode = if cfg!(feature = "classic") { "dual mode" } else { "LE only" };
    log::line(format_args!("OpenDynKVM Bluetooth module (ESP32, {mode}), firmware {FW_VERSION:#08x}"));
    let reason = esp_hal::system::reset_reason();
    log::line(format_args!("started after a reset: {reason:?}"));
    let hung = matches!(
        reason,
        Some(esp_hal::rtc_cntl::SocResetReason::CoreMwdt0 | esp_hal::rtc_cntl::SocResetReason::CpuMwdt0)
    );
    hang::start(TimerGroup::new(p.TIMG1).timer0, hung);
    if let Some(text) = last_panic() {
        // In pieces: a log line holds 160 bytes (`log::line` drops what doesn't fit), the record up to 250.
        log::line(format_args!("the last run ended in a panic:"));
        let mut rest = text.as_str();
        while !rest.is_empty() {
            let mut at = rest.len().min(96);
            while !rest.is_char_boundary(at) {
                at -= 1;
            }
            log::line(format_args!("  {}", &rest[..at]));
            rest = &rest[at..];
        }
        // On UART0 too, while it is still at the bootloader's 115 200 baud: the panic itself may have printed at the
        // link's speed (`panic`).
        let mut z = [0u8; PANIC_LEN + 1];
        z[..text.len()].copy_from_slice(text.as_bytes());
        // SAFETY: a NUL-terminated format and string.
        unsafe { esp_rom_printf(c"OpenDynKVM module: the last run ended in a panic: %s\n".as_ptr(), z.as_ptr()) };
        // Out of the UART's FIFO before the link changes its speed.
        esp_hal::delay::Delay::new().delay_millis(40);
    }
    log::stack_lines();
    hook_rom_out();
    spawner.spawn(rom_out().expect("ROM output task"));

    // The link, one interrupt level above the Bluetooth side, as on the ESP32-C3 (`firmware/esp32c3/src/main.rs`).
    let link_executor = LINK_EXECUTOR.init(InterruptExecutor::new(p.FROM_CPU_INTR1));
    let link_spawner = link_executor.start(esp_hal::interrupt::Priority::Priority1);
    let lines = link::Lines { uart: p.UART0, rx: p.GPIO3, tx: p.GPIO1 };
    link_spawner.spawn(link::run(lines).expect("link task"));

    // Two handles on the flash, made before either reads (`firmware/esp32c3/src/main.rs` says why).
    let device_flash = unsafe { p.FLASH.clone_unchecked() };
    let host_storage = esp_storage::FlashStorage::new(p.FLASH);
    let device_storage = esp_storage::FlashStorage::new(device_flash);
    let bonds = bonds::Bonds::load(host_storage);
    let device_bonds = bonds::DeviceBonds::load(device_storage);
    ble::run(spawner, p.BT, bonds, device_bonds).await
}
