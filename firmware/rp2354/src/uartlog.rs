//! A plain-text event log on `DBG_TX` (GPIO36, UART1 TX, 115200 8N1), which comes out on pin 6 of the RP2354B's
//! Tag-Connect pads (analysis.md finding 14). It carries the few events that matter at bring-up without a debug probe
//! (boot, self-test, AT32F405 programming, firmware updates) to any 3.3 V USB-UART adapter. Everything else goes to
//! defmt over RTT.

use core::fmt::{self, Write};

use embassy_rp::uart::{Async, UartTx};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use heapless::String;

pub const BAUD: u32 = 115_200;
const LINE: usize = 96;

static LINES: Channel<CriticalSectionRawMutex, String<LINE>, 8> = Channel::new();

/// Formats one line and queues it (from either core); dropped if the queue is full, truncated if too long.
pub fn log(args: fmt::Arguments) {
    struct Truncating(String<{ LINE - 2 }>);
    impl Write for Truncating {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for c in s.chars() {
                if self.0.push(c).is_err() {
                    break;
                }
            }
            Ok(())
        }
    }
    let mut t = Truncating(String::new());
    let _ = t.write_fmt(args);
    let mut line = String::new();
    let _ = line.push_str(&t.0);
    let _ = line.push_str("\r\n");
    let _ = LINES.try_send(line);
}

/// `uart_log!("self-test {:#06x}", bits)`: one line on `DBG_TX`.
macro_rules! uart_log {
    ($($arg:tt)*) => { $crate::uartlog::log(format_args!($($arg)*)) };
}
pub(crate) use uart_log;

#[embassy_executor::task]
pub async fn run(mut tx: UartTx<'static, Async>) -> ! {
    loop {
        let line = LINES.receive().await;
        let _ = tx.write(line.as_bytes()).await;
    }
}
