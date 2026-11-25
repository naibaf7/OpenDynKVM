//! A report that doesn't go out. trouble-host sends every connection's packets through one queue, in order, each once
//! the controller has a buffer free for it, so a packet whose buffer never comes holds up every host: their tasks wait
//! in their next send and stop taking their hosts' requests (three hosts, 2026-11-24: no input anywhere, the stack's
//! "OutOfMemory" for each request that came, until the module was unplugged).
//!
//! The known way into that, credits for more packets than the controller holds, is gone (`trouble-host`, the
//! "OpenDynKVM:" change). For any other, a send that has waited [`STALL`] while the controller reported nothing sent
//! restarts the module, as unplugging it did: the hosts come back with their bonds within a second or two, and the log
//! line says why (`odkvmctl device log`; the RP2354B keeps it across the restart).

use core::future::Future;
use core::pin::pin;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};

use crate::ble::St;
use crate::log;

/// Longer than the supervision timeouts the module asks hosts for (4 and 6 s, `ble::params`): a host gone without a
/// word has freed its buffers by then.
const STALL: Duration = Duration::from_secs(10);
const CHECK: Duration = Duration::from_secs(1);

/// When the controller last reported packets sent, in ms since boot (wrapping).
static SENT_AT: AtomicU32 = AtomicU32::new(0);

fn now_ms() -> u32 {
    Instant::now().as_millis() as u32
}

/// The controller reported packets sent, on any connection (`devices::Handler`).
pub fn sent() {
    SENT_AT.store(now_ms(), Ordering::Relaxed);
    SENT.signal(());
}

/// Packets went out ([`sent`]): what paces a host's pointer motion on the ESP32 (`ble`, `MOTION_QUEUE`).
pub static SENT: embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, ()> =
    embassy_sync::signal::Signal::new();

/// Runs `send` (a report or a reply on its way into the stack's send queue). If it has waited [`STALL`] and the
/// controller sent nothing meanwhile, the stack is stuck: the module restarts.
pub async fn watched<F: Future>(stack: &St, host: u8, send: F) -> F::Output {
    let started = Instant::now();
    let mut send = pin!(send);
    loop {
        match select(send.as_mut(), Timer::after(CHECK)).await {
            Either::First(r) => return r,
            Either::Second(()) => {
                let quiet = Duration::from_millis(now_ms().wrapping_sub(SENT_AT.load(Ordering::Relaxed)).into());
                let waited = started.elapsed();
                if waited >= STALL && quiet >= STALL {
                    restart(stack, host, waited).await;
                }
            }
        }
    }
}

async fn restart(stack: &St, host: u8, waited: Duration) -> ! {
    let (free, all) = stack.link_credits();
    let (packets, pool) = crate::devices::free_packets();
    log::line(format_args!(
        "host {host}: a report waited {} s, nothing sent meanwhile ({free} of {all} controller buffers, {packets} of \
         {pool} packets free): the Bluetooth stack is stuck, restarting",
        waited.as_secs()
    ));
    // The line out to the RP2354B first.
    Timer::after_millis(300).await;
    esp_hal::system::software_reset()
}
