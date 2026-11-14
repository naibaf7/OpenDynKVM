//! The link to the RP2354B: the AT32F405 links' 72-byte frames and go-back-N ARQ ([`kvm_proto::link`]), messages from
//! [`kvm_proto::link::bt`]. The RP2354B is the master: for every frame it sends, this side answers with one, as an SPI
//! slave would.
//!
//! One firmware for both places the module can be (software.md §4.13): UART0 on the KVM's board, or the chip's USB
//! serial port when it is an ESP32-C3 board on a front port. It listens on both and answers, for good, on the one the
//! first valid frame came over (the CRC makes noise on an unconnected UART pin harmless). The log on IO3 starts only
//! if that is the UART ([`crate::log`]).

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::Async;
use esp_hal::peripherals::{GPIO3, GPIO20, GPIO21, UART0, UART1, USB_DEVICE};
use esp_hal::uart::Uart;
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
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

/// Restarts into the ROM bootloader (UART download), as the RP2354B asks before it reprograms the module on its
/// board: IO9, the BOOT strap, is on the RP2354B's TX line and can't be held low across a reset of a running module.
/// (A board on a front port is restarted into its ROM by its USB serial port's DTR/RTS instead.)
fn enter_rom() -> ! {
    log::line(format_args!("restarting into the ROM bootloader"));
    // RTC_CNTL_OPTION1.FORCE_DOWNLOAD_BOOT: the next boot is a download boot, whatever the straps say.
    esp_hal::peripherals::LPWR::regs().option1().modify(|_, w| w.force_download_boot().set_bit());
    esp_hal::system::software_reset()
}

/// The line to the RP2354B.
enum Wire {
    Uart(Uart<'static, Async>),
    Usb(UsbSerialJtag<'static, Async>),
}

impl Wire {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            Wire::Uart(u) => u.read_async(buf).await.map_err(|_| ()),
            Wire::Usb(u) => embedded_io_async::Read::read(u, buf).await.map_err(|_| ()),
        }
    }

    /// One frame. On USB it goes as a full packet and a short one, which ends the RP2354B's read (no flush: that would
    /// add a zero-length packet).
    async fn write(&mut self, buf: &[u8]) {
        match self {
            Wire::Uart(u) => {
                let _ = u.write_async(buf).await;
            }
            Wire::Usb(u) => {
                let _ = embedded_io_async::Write::write_all(u, buf).await;
            }
        }
    }
}

/// Reads one frame: skips to the magic, then reads the rest. A frame whose CRC fails is dropped (the ARQ treats it as
/// lost), and the next magic resynchronises.
async fn read_frame(wire: &mut Wire, raw: &mut [u8; FRAME_LEN]) -> bool {
    let mut b = [0u8; 1];
    loop {
        match wire.read(&mut b).await {
            Ok(1) if b[0] == MAGIC => break,
            Ok(_) => continue,
            Err(()) => return false,
        }
    }
    raw[0] = MAGIC;
    let mut got = 1;
    while got < FRAME_LEN {
        match with_timeout(Duration::from_millis(5), wire.read(&mut raw[got..])).await {
            Ok(Ok(n)) => got += n,
            _ => return false,
        }
    }
    Frame::decode(raw).is_ok()
}

/// The next valid frame on `wire` (bad ones and read errors are skipped).
async fn valid_frame(wire: &mut Wire) -> [u8; FRAME_LEN] {
    let mut raw = [0u8; FRAME_LEN];
    loop {
        if read_frame(wire, &mut raw).await {
            return raw;
        }
    }
}

/// Waits for the first valid frame on either line: that line is the link from now on. Each line reads on by itself
/// until one has a frame, so a read error on one (noise on an unconnected UART pin) never throws away a frame half
/// read on the other.
async fn choose(uart: Uart<'static, Async>, usb: UsbSerialJtag<'static, Async>) -> (Wire, [u8; FRAME_LEN]) {
    let mut uart = Wire::Uart(uart);
    let mut usb = Wire::Usb(usb);
    let r = select(valid_frame(&mut uart), valid_frame(&mut usb)).await;
    match r {
        Either::First(raw) => (uart, raw),
        Either::Second(raw) => (usb, raw),
    }
}

/// The peripherals of both lines and of the log, for the link task to set up on its own executor (esp-hal's async
/// drivers stay with the executor they were made on).
pub struct Lines {
    pub uart: UART0<'static>,
    pub rx: GPIO20<'static>,
    pub tx: GPIO21<'static>,
    pub usb: USB_DEVICE<'static>,
    pub log: UART1<'static>,
    pub log_pin: GPIO3<'static>,
}

#[embassy_executor::task]
pub async fn run(lines: Lines) {
    let Lines { uart, rx, tx, usb, log: log_uart, log_pin } = lines;
    let uart = Uart::new(uart, esp_hal::uart::Config::default().with_baudrate(kvm_proto::link::bt::BAUD))
        .expect("UART0")
        .with_rx(rx)
        .with_tx(tx)
        .into_async();
    let usb = UsbSerialJtag::new(usb).into_async();
    let (mut wire, first) = choose(uart, usb).await;
    match wire {
        Wire::Uart(_) => {
            log::start(log_uart, log_pin);
            log::line(format_args!("link: UART0 (the module on the KVM's board)"));
        }
        Wire::Usb(_) => log::off(),
    }
    let mut arq: Arq<8> = Arq::new(0x5A);
    let mut tx = [0u8; FRAME_LEN];
    let hello = BtMsg::Hello { proto: PROTOCOL_VERSION, fw_version: crate::FW_VERSION, role: ROLE, flags: 0 };
    send(&hello);
    let mut pending = Some(first);
    loop {
        // Only answer: the RP2354B starts every exchange (every 5 ms at least).
        let raw = match pending.take() {
            Some(raw) => raw,
            None => {
                let mut raw = [0u8; FRAME_LEN];
                if !read_frame(&mut wire, &mut raw).await {
                    continue;
                }
                raw
            }
        };
        while arq.free(Prio::High) > 0 || arq.free(Prio::Low) > 0 {
            let Ok(m) = TO_RP.try_receive() else { break };
            let prio = BtMsg::decode(&m).map_or(Prio::Low, |b| b.prio());
            if arq.push(&m, prio).is_err() {
                break;
            }
        }
        let got = arq.receive(&raw);
        arq.next_frame().encode(&mut tx);
        wire.write(&tx).await;
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
                    if let Either::Second(()) = select(FROM_RP.send(m), Timer::after_millis(50)).await {
                        log::line(format_args!("link: Bluetooth side busy, message dropped"));
                    }
                }
                Err(_) => {}
            }
        }
    }
}
