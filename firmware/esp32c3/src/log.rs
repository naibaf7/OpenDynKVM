//! A plain-text log on IO3 (UART1 TX, 115200 8N1), the board's test pad TP1601: boot, pairing, connections. UART0
//! is the link to the RP2354B, so nothing else may print there.
//!
//! IO3 is only driven once the link has come over UART0, i.e. on the KVM's board ([`start`]): on an ESP32-C3 board on
//! a front port (software.md §4.13) IO3 may be wired to anything. Until then lines are kept (the first [`EARLY`]
//! bytes), and printed when the log starts.

use core::cell::RefCell;
use core::fmt::Write;

use critical_section::Mutex;
use esp_hal::Blocking;
use esp_hal::peripherals::{GPIO3, UART1};
use esp_hal::uart::{Config, UartTx};

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

/// One line of the log.
pub fn line(args: core::fmt::Arguments) {
    critical_section::with(|cs| match &mut *LOG.borrow_ref_mut(cs) {
        Log::On(tx) => {
            let mut w = Writer(tx);
            let _ = w.write_fmt(args);
            let _ = w.write_str("\r\n");
        }
        // What doesn't fit is cut.
        Log::Early(kept) => {
            let _ = kept.write_fmt(args);
            let _ = kept.write_str("\r\n");
        }
        Log::Off => {}
    });
}
