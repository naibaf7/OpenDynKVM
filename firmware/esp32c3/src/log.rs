//! A plain-text log on IO3 (UART1 TX, 115200 8N1), the board's test pad TP1601: boot, pairing, connections. UART0
//! is the link to the RP2354B, so nothing else may print there.

use core::cell::RefCell;
use core::fmt::Write;

use critical_section::Mutex;
use esp_hal::Blocking;
use esp_hal::peripherals::{GPIO3, UART1};
use esp_hal::uart::{Config, UartTx};

static LOG: Mutex<RefCell<Option<UartTx<'static, Blocking>>>> = Mutex::new(RefCell::new(None));

pub fn init(uart: UART1<'static>, pin: GPIO3<'static>) {
    if let Ok(tx) = UartTx::new(uart, Config::default().with_baudrate(115_200)) {
        let tx = tx.with_tx(pin);
        critical_section::with(|cs| *LOG.borrow_ref_mut(cs) = Some(tx));
    }
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
    critical_section::with(|cs| {
        if let Some(tx) = LOG.borrow_ref_mut(cs).as_mut() {
            let mut w = Writer(tx);
            let _ = w.write_fmt(args);
            let _ = w.write_str("\r\n");
        }
    });
}
