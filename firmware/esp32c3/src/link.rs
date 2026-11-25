//! The link to the RP2354B: the AT32F405 links' 72-byte frames and go-back-N ARQ ([`kvm_proto::link`]), messages from
//! [`kvm_proto::link::bt`]. The RP2354B is the master: for every frame it sends, this side answers with one, as an SPI
//! slave would.
//!
//! One firmware for both places an ESP32-C3 module can be (software.md §4.13): UART0 on the KVM's board, or the chip's
//! USB serial port when it is an ESP32-C3 board on a front port. It listens on both and answers, for good, on the one
//! the first valid frame came over (the CRC makes noise on an unconnected UART pin harmless). The log on IO3 starts
//! only if that is the UART ([`crate::log`]).
//!
//! The ESP32 (`firmware/esp32`, which builds this file too) has no USB of its own, so both places are its UART0 (GPIO1
//! TX, GPIO3 RX): the KVM's board at [`kvm_proto::link::bt::BAUD`], or a board on a front port whose USB-to-UART bridge
//! is on it, at [`kvm_proto::link::bt::BRIDGE_BAUD`] (a CP2104 goes to 2 Mbaud). It listens at each speed in turn and
//! keeps the one the first valid frame came at.

use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::Async;
use esp_hal::peripherals::UART0;
#[cfg(target_arch = "xtensa")]
use esp_hal::peripherals::{GPIO1, GPIO3};
#[cfg(target_arch = "riscv32")]
use esp_hal::peripherals::{GPIO3, GPIO20, GPIO21, UART1, USB_DEVICE};
use esp_hal::uart::Uart;
#[cfg(target_arch = "riscv32")]
use esp_hal::usb::usb_serial_jtag::UsbSerialJtag;
use kvm_proto::link::bt::{BtMsg, Chip, ROLE};
use kvm_proto::link::frame::MAGIC;
use kvm_proto::link::{Arq, FRAME_LEN, Frame, MAX_MSG, Msg, PROTOCOL_VERSION, Prio};

use crate::log;

/// Messages for the RP2354B: input reports ahead of the rest, as on the RP2354B's side. Until 0.1.31 there was one
/// queue, and a control message taken from it while the ARQ's low queue was full was lost: one per exchange while more
/// than 8 waited, which a report map in pieces after a device's state changes was (a keyboard that said connected and
/// never typed, 2026-11-19).
static TO_RP_HI: Channel<CriticalSectionRawMutex, Msg, 16> = Channel::new();
/// Host and device states, events, report maps, devices found.
static TO_RP_LO: Channel<CriticalSectionRawMutex, Msg, 32> = Channel::new();
/// Messages from the RP2354B, for the Bluetooth side.
pub static FROM_RP: Channel<CriticalSectionRawMutex, Msg, 24> = Channel::new();

/// The newest report of each device whose report is its whole state (a pad's), for the next exchange: only the newest
/// ever waits. Queued as they came, a pad's 250 reports a second filled every exchange of a USB bridge's link (some
/// 250 a second), and another device's reports and state waited behind them (2026-11-25).
/// Each with the exchanges it waited: it gives way to the other devices' reports (a mouse's motion can't be put
/// together), [`LATEST_WAIT`] exchanges at most.
static LATEST: embassy_sync::blocking_mutex::Mutex<
    CriticalSectionRawMutex,
    core::cell::RefCell<[Option<(Msg, u8)>; kvm_proto::link::bt::MAX_DEVICES]>,
> = embassy_sync::blocking_mutex::Mutex::new(core::cell::RefCell::new([const { None }; kvm_proto::link::bt::MAX_DEVICES]));
const LATEST_WAIT: u8 = 2;
/// The ARQ's queue, per priority.
const ARQ_QUEUE: usize = 8;

/// Device `dev`'s report `m`, a whole state: it replaces the one still waiting, if one is.
#[cfg_attr(target_arch = "riscv32", allow(dead_code))]
pub fn send_latest(dev: u8, m: &BtMsg) {
    let mut buf = [0u8; MAX_MSG];
    if let Some(i) = (dev as usize).checked_sub(1).filter(|i| *i < kvm_proto::link::bt::MAX_DEVICES)
        && let Ok(n) = m.encode(&mut buf)
        && let Ok(msg) = Msg::from_slice(&buf[..n])
    {
        LATEST.lock(|l| {
            let mut l = l.borrow_mut();
            let waited = l[i].as_ref().map_or(0, |(_, w)| *w);
            l[i] = Some((msg, waited));
        });
    }
}

/// Queues `m` for the RP2354B; dropped if the queue is full (the RP2354B asks for the state again after a restart).
pub fn send(m: &BtMsg) {
    let mut buf = [0u8; MAX_MSG];
    if let Ok(n) = m.encode(&mut buf)
        && let Ok(msg) = Msg::from_slice(&buf[..n])
    {
        let _ = match m.prio() {
            Prio::High => TO_RP_HI.try_send(msg),
            Prio::Low => TO_RP_LO.try_send(msg),
        };
    }
}

/// `Hello.flags`: the chip, and the ESP32's Classic host.
#[cfg(target_arch = "riscv32")]
const HELLO_FLAGS: u8 = Chip::Esp32C3 as u8;
#[cfg(target_arch = "xtensa")]
const HELLO_FLAGS: u8 = Chip::Esp32 as u8 | kvm_proto::link::bt::hello_flags::CLASSIC;

/// Restarts into the ROM bootloader (UART download), as the RP2354B asks before it reprograms the module on its
/// board: IO9, the BOOT strap, is on the RP2354B's TX line and can't be held low across a reset of a running module.
/// (A board on a front port is restarted into its ROM by its USB serial port's DTR/RTS instead.)
#[cfg(target_arch = "riscv32")]
fn enter_rom() -> ! {
    log::line(format_args!("restarting into the ROM bootloader"));
    // RTC_CNTL_OPTION1.FORCE_DOWNLOAD_BOOT: the next boot is a download boot, whatever the straps say.
    esp_hal::peripherals::LPWR::regs().option1().modify(|_, w| w.force_download_boot().set_bit());
    esp_hal::system::software_reset()
}

/// The ESP32 has no FORCE_DOWNLOAD_BOOT: its GPIO0 strap alone picks the ROM, which its board's bridge drives (DTR),
/// so the RP2354B restarts it into its ROM with DTR/RTS when it doesn't go by itself.
#[cfg(target_arch = "xtensa")]
fn enter_rom() {
    log::line(format_args!("the ESP32 can't restart into its ROM by itself: its board's DTR/RTS do that"));
}

/// The line to the RP2354B.
enum Wire {
    Uart(Uart<'static, Async>),
    #[cfg(target_arch = "riscv32")]
    Usb(UsbSerialJtag<'static, Async>),
}

impl Wire {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        match self {
            Wire::Uart(u) => u.read_async(buf).await.map_err(|_| ()),
            #[cfg(target_arch = "riscv32")]
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
            #[cfg(target_arch = "riscv32")]
            Wire::Usb(u) => {
                let _ = embedded_io_async::Write::write_all(u, buf).await;
            }
        }
    }
}

/// How long the rest of a frame may take once its magic is in. On the UART a frame takes 0.24 ms, and a short wait
/// resynchronises after noise. On USB a frame is two packets, a millisecond or more apart while the RP2354B is busy,
/// and USB delivers both: until 0.1.21 the UART's 5 ms dropped frames whose second packet came late, and the RP2354B
/// waited for a reply that never came (it waits 40 ms on USB).
fn rest_of_frame(wire: &Wire) -> Duration {
    match wire {
        Wire::Uart(_) => Duration::from_millis(5),
        #[cfg(target_arch = "riscv32")]
        Wire::Usb(_) => Duration::from_millis(30),
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
    let rest = rest_of_frame(wire);
    while got < FRAME_LEN {
        match with_timeout(rest, wire.read(&mut raw[got..])).await {
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
#[cfg(target_arch = "riscv32")]
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
#[cfg(target_arch = "riscv32")]
pub struct Lines {
    pub uart: UART0<'static>,
    pub rx: GPIO20<'static>,
    pub tx: GPIO21<'static>,
    pub usb: USB_DEVICE<'static>,
    pub log: UART1<'static>,
    pub log_pin: GPIO3<'static>,
}

/// UART0: to the KVM's board, or to the board's USB-to-UART bridge (the ESP32).
#[cfg(target_arch = "xtensa")]
pub struct Lines {
    pub uart: UART0<'static>,
    pub rx: GPIO3<'static>,
    pub tx: GPIO1<'static>,
}

/// The line the RP2354B is on, and the first valid frame that came over it.
#[cfg(target_arch = "riscv32")]
async fn open(lines: Lines) -> (Wire, [u8; FRAME_LEN]) {
    let Lines { uart, rx, tx, usb, log: log_uart, log_pin } = lines;
    let uart = Uart::new(uart, esp_hal::uart::Config::default().with_baudrate(kvm_proto::link::bt::BAUD))
        .expect("UART0")
        .with_rx(rx)
        .with_tx(tx)
        .into_async();
    let usb = UsbSerialJtag::new(usb).into_async();
    let (wire, first) = choose(uart, usb).await;
    match wire {
        Wire::Uart(_) => {
            log::start(log_uart, log_pin);
            log::line(format_args!("link: UART0 (the module on the KVM's board)"));
        }
        Wire::Usb(_) => log::off(),
    }
    (wire, first)
}

#[cfg(target_arch = "xtensa")]
async fn open(lines: Lines) -> (Wire, [u8; FRAME_LEN]) {
    use kvm_proto::link::bt::{BAUD, BRIDGE_BAUD};
    let Lines { uart, rx, tx } = lines;
    let mut uart = Uart::new(uart, esp_hal::uart::Config::default().with_baudrate(BAUD))
        .expect("UART0")
        .with_rx(rx)
        .with_tx(tx)
        .into_async();
    // The RP2354B sends a frame every few milliseconds (it starts every exchange): a quarter second at each speed
    // catches dozens. At the other speed a frame's CRC fails, and noise never passes it.
    let speeds = [BAUD, BRIDGE_BAUD];
    let mut at = 0;
    loop {
        let _ = uart.apply_config(&esp_hal::uart::Config::default().with_baudrate(speeds[at]));
        let mut wire = Wire::Uart(uart);
        if let Ok(first) = with_timeout(Duration::from_millis(250), valid_frame(&mut wire)).await {
            // No log pin on either (the bridge's line is UART0's): only the RP2354B gets the lines.
            log::off();
            let place = if speeds[at] == BAUD { "the KVM's board" } else { "a USB-to-UART bridge" };
            log::line(format_args!("link: UART0 at {} baud ({place})", speeds[at]));
            return (wire, first);
        }
        let Wire::Uart(u) = wire;
        uart = u;
        at = (at + 1) % speeds.len();
    }
}

#[embassy_executor::task]
pub async fn run(lines: Lines) {
    let (mut wire, first) = open(lines).await;
    let mut arq: Arq<ARQ_QUEUE> = Arq::new(0x5A);
    let mut tx = [0u8; FRAME_LEN];
    let hello = BtMsg::Hello { proto: PROTOCOL_VERSION, fw_version: crate::FW_VERSION, role: ROLE, flags: HELLO_FLAGS };
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
        // The other reports in their order, then the devices' newest whole-state reports: once nothing else waits, or
        // after `LATEST_WAIT` exchanges.
        while arq.free(Prio::High) > 0 {
            let Ok(m) = TO_RP_HI.try_receive() else { break };
            let _ = arq.push(&m, Prio::High);
        }
        let others = arq.free(Prio::High) < ARQ_QUEUE;
        LATEST.lock(|l| {
            for slot in l.borrow_mut().iter_mut().filter(|m| m.is_some()) {
                let due = slot.as_ref().is_some_and(|(_, w)| !others || *w >= LATEST_WAIT);
                if due && arq.free(Prio::High) > 0 {
                    if let Some((m, _)) = slot.take() {
                        let _ = arq.push(&m, Prio::High);
                    }
                } else if let Some((_, w)) = slot.as_mut() {
                    *w = w.saturating_add(1);
                }
            }
        });
        while arq.free(Prio::Low) > 0 {
            let Ok(m) = TO_RP_LO.try_receive() else { break };
            let _ = arq.push(&m, Prio::Low);
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
