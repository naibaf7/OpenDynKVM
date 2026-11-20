//! A plain-text log on IO3 (UART1 TX, 115200 8N1), the board's test pad TP1601: boot, pairing, connections. UART0
//! is the link to the RP2354B, so nothing else may print there.
//!
//! IO3 is only driven once the link has come over UART0, i.e. on the KVM's board ([`start`]): on an ESP32-C3 board on
//! a front port (software.md §4.13) IO3 may be wired to anything. Until then lines are kept (the first [`EARLY`]
//! bytes), and printed when the log starts.
//!
//! Every line also goes to the RP2354B (`BtMsg::Log`), which keeps the latest in RAM for `odkvmctl device log`: on a
//! dongle IO3 goes nowhere.

use core::cell::RefCell;
use core::fmt::Write;

use critical_section::Mutex;
use esp_hal::Blocking;
use esp_hal::peripherals::{GPIO3, UART1};
use esp_hal::uart::{Config, UartTx};
use kvm_proto::link::bt::{BtMsg, LOG_MAX};

const EARLY: usize = 1024;

// One, in a static: the kept lines' size doesn't matter.
#[allow(clippy::large_enum_variant)]
enum Log {
    /// Not started: the lines so far.
    Early(heapless::String<EARLY>),
    On(UartTx<'static, Blocking>),
    /// Never started (the link is on USB): lines are dropped.
    Off,
}

static LOG: Mutex<RefCell<Log>> = Mutex::new(RefCell::new(Log::Early(heapless::String::new())));

/// Starts the log on IO3 and prints what was kept.
pub fn start(uart: UART1<'static>, pin: GPIO3<'static>) {
    if let Ok(tx) = UartTx::new(uart, Config::default().with_baudrate(115_200)) {
        let mut tx = tx.with_tx(pin);
        critical_section::with(|cs| {
            let mut log = LOG.borrow_ref_mut(cs);
            if let Log::Early(kept) = &*log {
                let _ = tx.write(kept.as_bytes());
            }
            *log = Log::On(tx);
        });
    }
}

/// The log stays off (the link is on USB, and IO3 is not the board's test pad).
pub fn off() {
    critical_section::with(|cs| *LOG.borrow_ref_mut(cs) = Log::Off);
}

struct Writer<'a>(&'a mut UartTx<'static, Blocking>);

impl Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let _ = self.0.write(s.as_bytes());
        Ok(())
    }
}

/// The Bluetooth stack's warnings and errors (through the `log` crate) into the log, at most [`STACK_PER_S`] a second:
/// why an encryption with a bond failed, for one (a device's key refused, 2026-11-20). Nothing else logs through it.
pub struct Stack;
const STACK_PER_S: u32 = 10;
/// The second lines are counted in, and how many came in it (no atomics on the ESP32-C3: a critical section).
static STACK_COUNT: Mutex<core::cell::Cell<(u32, u32)>> = Mutex::new(core::cell::Cell::new((0, 0)));

impl ::log::Log for Stack {
    fn enabled(&self, m: &::log::Metadata) -> bool {
        m.level() <= ::log::Level::Warn && m.target().starts_with("trouble_host")
    }

    fn log(&self, r: &::log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        let second = (embassy_time::Instant::now().as_millis() / 1000) as u32;
        let take = critical_section::with(|cs| {
            let c = STACK_COUNT.borrow(cs);
            let (window, count) = c.get();
            let count = if window == second { count + 1 } else { 1 };
            c.set((second, count));
            count <= STACK_PER_S
        });
        if take {
            line(format_args!("stack {}: {}", r.level(), r.args()));
        }
    }

    fn flush(&self) {}
}

static STACK: Stack = Stack;

/// The stack's warnings and errors into the log from here on ([`Stack`]).
pub fn stack_lines() {
    // SAFETY: once, at start, before the stack runs (the racy forms: the ESP32-C3 has no atomics).
    unsafe {
        if ::log::set_logger_racy(&STACK).is_ok() {
            ::log::set_max_level_racy(::log::LevelFilter::Warn);
        }
    }
}

/// One line of the log (what doesn't fit 160 bytes is cut).
pub fn line(args: core::fmt::Arguments) {
    let mut text: heapless::String<160> = heapless::String::new();
    let _ = text.write_fmt(args);
    critical_section::with(|cs| match &mut *LOG.borrow_ref_mut(cs) {
        Log::On(tx) => {
            let mut w = Writer(tx);
            let _ = w.write_str(&text);
            let _ = w.write_str("\r\n");
        }
        // What doesn't fit is cut.
        Log::Early(kept) => {
            let _ = kept.write_str(&text);
            let _ = kept.write_str("\r\n");
        }
        Log::Off => {}
    });
    // To the RP2354B, in pieces of a link message each (cut at a character's start).
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut n = rest.len().min(LOG_MAX);
        while !rest.is_char_boundary(n) {
            n -= 1;
        }
        crate::link::send(&BtMsg::Log { text: &rest[..n] });
        rest = &rest[n..];
    }
}
