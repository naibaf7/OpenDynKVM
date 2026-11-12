//! The link to the RP2354B over UART0: the AT32F405 links' 72-byte frames and go-back-N ARQ
//! ([`kvm_proto::link`]), messages from [`kvm_proto::link::bt`]. The RP2354B is the master: for every frame it sends,
//! this side answers with one, as an SPI slave would.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::Async;
use esp_hal::uart::Uart;
use kvm_proto::link::bt::{BtMsg, ROLE};
use kvm_proto::link::frame::MAGIC;
use kvm_proto::link::{Arq, FRAME_LEN, Frame, MAX_MSG, Msg, PROTOCOL_VERSION, Prio};

use crate::log;

/// Messages for the RP2354B (host states, output reports, events).
pub static TO_RP: Channel<CriticalSectionRawMutex, Msg, 24> = Channel::new();
/// Messages from the RP2354B, for the Bluetooth side.
pub static FROM_RP: Channel<CriticalSectionRawMutex, Msg, 24> = Channel::new();

/// Queues `m` for the RP2354B; dropped if the queue is full (the RP2354B asks for the state again after a restart).
pub fn send(m: &BtMsg) {
    let mut buf = [0u8; MAX_MSG];
    if let Ok(n) = m.encode(&mut buf)
        && let Ok(msg) = Msg::from_slice(&buf[..n])
    {
        let _ = TO_RP.try_send(msg);
    }
}

/// Restarts into the ROM bootloader (UART download), as the RP2354B asks before it reprograms the module: IO9, the
/// BOOT strap, is on the RP2354B's TX line and can't be held low across a reset of a running module.
fn enter_rom() -> ! {
    log::line(format_args!("restarting into the ROM bootloader"));
    // RTC_CNTL_OPTION1.FORCE_DOWNLOAD_BOOT: the next boot is a download boot, whatever the straps say.
    esp_hal::peripherals::LPWR::regs().option1().modify(|_, w| w.force_download_boot().set_bit());
    esp_hal::system::software_reset()
}

/// Reads one frame: skips to the magic, then reads the rest. A frame whose CRC fails is dropped (the ARQ treats it as
/// lost), and the next magic resynchronises.
async fn read_frame(uart: &mut Uart<'static, Async>, raw: &mut [u8; FRAME_LEN]) -> bool {
    let mut b = [0u8; 1];
    loop {
        match uart.read_async(&mut b).await {
            Ok(1) if b[0] == MAGIC => break,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
    raw[0] = MAGIC;
    let mut got = 1;
    while got < FRAME_LEN {
        match with_timeout(Duration::from_millis(5), uart.read_async(&mut raw[got..])).await {
            Ok(Ok(n)) => got += n,
            _ => return false,
        }
    }
    Frame::decode(raw).is_ok()
}

#[embassy_executor::task]
pub async fn run(mut uart: Uart<'static, Async>) {
    let mut arq: Arq<8> = Arq::new(0x5A);
    let mut raw = [0u8; FRAME_LEN];
    let mut tx = [0u8; FRAME_LEN];
    let hello = BtMsg::Hello { proto: PROTOCOL_VERSION, fw_version: crate::FW_VERSION, role: ROLE, flags: 0 };
    send(&hello);
    loop {
        // Only answer: the RP2354B starts every exchange (every 5 ms at least).
        if !read_frame(&mut uart, &mut raw).await {
            continue;
        }
        while arq.free(Prio::High) > 0 || arq.free(Prio::Low) > 0 {
            let Ok(m) = TO_RP.try_receive() else { break };
            let prio = BtMsg::decode(&m).map_or(Prio::Low, |b| b.prio());
            if arq.push(&m, prio).is_err() {
                break;
            }
        }
        let got = arq.receive(&raw);
        arq.next_frame().encode(&mut tx);
        let _ = uart.write_async(&tx).await;
        if let Some(m) = got {
            match BtMsg::decode(&m) {
                Ok(BtMsg::Hello { .. }) => {
                    // The RP2354B (re)started: tell it who this is again.
                    send(&hello);
                    let _ = FROM_RP.try_send(m);
                }
                Ok(BtMsg::EnterRom) => {
                    // Let the acknowledgement go out first.
                    let _ = select(Timer::after_millis(20), core::future::pending::<()>()).await;
                    enter_rom();
                }
                Ok(_) => {
                    if let Either::Second(()) =
                        select(FROM_RP.send(m), Timer::after_millis(50)).await
                    {
                        log::line(format_args!("link: Bluetooth side busy, message dropped"));
                    }
                }
                Err(_) => {}
            }
        }
    }
}
