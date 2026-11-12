//! The Bluetooth module (ESP32-C3-MINI-1, boards 0.16.0 on, when fitted): whether it is there, its link, and its
//! firmware (software.md §4.13).
//!
//! - **Fitted or not:** boards 0.12–0.15 leave GPIO28/29 unconnected, and a 0.16.0 board can be built without the
//!   module (assembly variant without `Fit = BT`). At boot `main` reads `ESP_TXD` (GPIO29) with the pad's pull-down on:
//!   only the module's TXD0, idling high, lifts it. Without it this task stops at once, and the KVM is the two-PC KVM
//!   it always was: no capability bit, no Bluetooth status, nothing sent on GPIO28.
//! - **Link:** UART0 (GPIO28 TX → the module's RXD0, GPIO29 RX ← its TXD0) at [`BAUD`]: the AT32F405 links' frames and
//!   ARQ ([`kvm_proto::link::bt`]), the RP2354B as master, one frame out and one back per exchange.
//! - **Firmware:** the image is embedded (`build.rs`, from `cargo xtask dist`). A module that says `Hello` with another
//!   version is sent into its ROM bootloader (`EnterRom`); one that says nothing within [`HELLO_TIMEOUT`] (blank, or a
//!   firmware that doesn't come up and resets) is caught there with a break on TX. Then [`crate::espisp`] programs it,
//!   as often as `kvm_core::at32prog` allows (as for the AT32F405s: 5 failed attempts in a row, counted in flash, then
//!   no more until the image changes, the key is held at power-up, or `RebootTarget::BtRetry`). Two wires only (TX,
//!   RX): the module's BOOT strap is on TX, and its EN has no line from the RP2354B (hardware/tools/gen_schematic.py
//!   block 16).
//! - **State:** [`State`] goes to the engine (`Status.bt_module`, the diagnostics) and to the self-test
//!   (`selftest::BT_FW`, `BT_GAVE_UP`).

use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either3, select3};
use embassy_rp::uart::BufferedUart;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{Read, Write};
use kvm_core::at32prog::{self, Attempt, Next};
use kvm_core::bt::BtOut;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::link::bt::{BAUD, BtMsg, ROLE};
use kvm_proto::link::{Arq, FRAME_LEN, Frame, MAX_MSG, Msg, PROTOCOL_VERSION, Prio};

use crate::espisp::{self, Image};
use crate::{kvm, storage};
use crate::uartlog::uart_log;

const KEEPALIVE: Duration = Duration::from_millis(5);
/// A frame takes 0.24 ms at 3 Mbaud; the module answers within a millisecond.
const REPLY_TIMEOUT: Duration = Duration::from_millis(4);
const LINK_TIMEOUT: Duration = Duration::from_millis(500);
/// How long a just-reset module may take to say `Hello` (its boot: ROM, second-stage bootloader, radio start).
const HELLO_TIMEOUT: Duration = Duration::from_secs(4);

/// What the RP2354B knows of the module (`Status.bt_module`, the diagnostics, the self-test; the log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
pub enum State {
    /// Not looked yet.
    Unknown = 0,
    /// Not fitted (or a board before 0.16.0).
    Absent = 1,
    /// Fitted, the link is up and its firmware is the embedded one.
    Up = 2,
    /// Fitted, being programmed.
    Programming = 3,
    /// Fitted, but it doesn't answer or couldn't be programmed this boot: Bluetooth is off until the next power-up.
    Failed = 4,
    /// Fitted, waiting for its `Hello` (after a reset, or after programming).
    Starting = 5,
    /// Programming failed [`at32prog::MAX_FAILS`] times in a row: not tried again (`kvm_core::at32prog`).
    GaveUp = 6,
}

pub static STATE: AtomicU8 = AtomicU8::new(State::Unknown as u8);
pub static LINK_ERRORS: AtomicU32 = AtomicU32::new(0);

pub fn state() -> State {
    match STATE.load(Ordering::Relaxed) {
        1 => State::Absent,
        2 => State::Up,
        3 => State::Programming,
        4 => State::Failed,
        5 => State::Starting,
        6 => State::GaveUp,
        _ => State::Unknown,
    }
}

/// The state as hosts see it.
pub fn module(s: State) -> BtModule {
    match s {
        State::Unknown | State::Absent => BtModule::Absent,
        State::Starting => BtModule::Starting,
        State::Up => BtModule::Up,
        State::Programming => BtModule::Programming,
        State::Failed => BtModule::Failed,
        State::GaveUp => BtModule::GaveUp,
    }
}

fn set_state(s: State) {
    STATE.store(s as u8, Ordering::Relaxed);
    kvm::with(|e| e.set_bt_module(module(s)));
}

type Queue = Channel<CriticalSectionRawMutex, Msg, 24>;
static TO_BT_HI: Queue = Channel::new();
static TO_BT_LO: Queue = Channel::new();

/// The module's image embedded by `build.rs` (empty without one: then a module is used as it is, never programmed).
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/esp_fw.rs"));
    pub static ZLIB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/esp.bin.z"));
}

fn image() -> Image {
    Image { zlib: embedded::ZLIB, len: embedded::ESP_IMAGE_LEN, md5: embedded::ESP_IMAGE_MD5 }
}

/// Identifies the embedded image for the programming record (CRC-32 of its MD5; 0 without an image).
pub fn image_crc() -> u32 {
    if embedded::ZLIB.is_empty() { 0 } else { kvm_proto::crc::crc32(embedded::ESP_IMAGE_MD5) }
}

/// Queues what the engine asks of the module; dropped while there is no module (or its link is down).
pub fn send(o: BtOut) {
    if state() != State::Up {
        return;
    }
    let mut buf = [0u8; MAX_MSG];
    let m = match &o {
        BtOut::Input { host, id, data } => BtMsg::Input { host: *host, id: *id, data },
        BtOut::Select(host) => BtMsg::Select { host: *host },
        BtOut::Pair { on, seconds } => BtMsg::Pair { on: *on, seconds: *seconds },
        BtOut::Forget(host) => BtMsg::Forget { host: *host },
        BtOut::Passkey { host, value } => BtMsg::Passkey { host: *host, value: *value },
        BtOut::Policy { host, policy } => BtMsg::Policy { host: *host, policy: *policy },
        BtOut::RequestState => BtMsg::RequestState,
    };
    let Ok(n) = m.encode(&mut buf) else { return };
    let Ok(msg) = Msg::from_slice(&buf[..n]) else { return };
    let q = if m.prio() == Prio::High { &TO_BT_HI } else { &TO_BT_LO };
    if q.try_send(msg).is_err() {
        kvm::DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// A message from the module into the engine.
fn deliver(m: &[u8]) {
    let Ok(msg) = BtMsg::decode(m) else { return };
    match msg {
        BtMsg::Hello { proto, fw_version, .. } => {
            info!("Bluetooth module: protocol {}, firmware {=u32:#x}", proto, fw_version);
            kvm::with(|e| e.bt_hello(fw_version));
        }
        BtMsg::Host(h) => kvm::with(|e| e.bt_host(h)),
        BtMsg::Output { host, id, data } => kvm::with(|e| e.bt_output(host, id, data)),
        BtMsg::Event { kind, host, value } => kvm::with(|e| e.bt_event(kind, host, value)),
        _ => {}
    }
}

/// Why the link loop ended.
enum Exit {
    /// Another firmware version, and the module was told to restart into its ROM bootloader.
    Outdated(u32),
    /// No `Hello` in time: blank, or a firmware that doesn't come up.
    Silent,
    /// The link stopped answering.
    Lost,
}

/// A frame worth of the message `m`, for the ARQ.
fn push(arq: &mut Arq<8>, m: &BtMsg) {
    let mut buf = [0u8; MAX_MSG];
    if let Ok(n) = m.encode(&mut buf) {
        let _ = arq.push(&buf[..n], m.prio());
    }
}

/// Reads one frame: the next [`FRAME_LEN`] bytes that start with the magic and pass the CRC. Bytes before a magic are
/// skipped, which resynchronises after a lost byte.
async fn read_frame(uart: &mut BufferedUart, raw: &mut [u8; FRAME_LEN]) -> bool {
    let mut b = [0u8; 1];
    loop {
        if uart.read_exact(&mut b).await.is_err() {
            return false;
        }
        if b[0] == kvm_proto::link::frame::MAGIC {
            break;
        }
    }
    raw[0] = b[0];
    uart.read_exact(&mut raw[1..]).await.is_ok() && Frame::decode(raw).is_ok()
}

async fn link_loop(uart: &mut BufferedUart, seed: u8, check: Option<u32>) -> Exit {
    let mut arq: Arq<8> = Arq::new(seed);
    let mut check = check;
    let started = Instant::now();
    let mut last_ok = started;
    let mut tx = [0u8; FRAME_LEN];
    let mut rx = [0u8; FRAME_LEN];
    // The module's boot log and anything from before.
    let mut junk = [0u8; 64];
    while let Ok(Ok(n)) = with_timeout(Duration::from_millis(5), uart.read(&mut junk)).await {
        if n == 0 {
            break;
        }
    }
    push(&mut arq, &BtMsg::Hello { proto: PROTOCOL_VERSION, fw_version: crate::FW_VERSION, role: 0, flags: 0 });
    // Set once the module runs another version: `EnterRom` is on its way, return once it is acknowledged.
    let mut leaving: Option<(u32, Instant)> = None;
    loop {
        if let Some((found, since)) = leaving
            && (!arq.has_pending() || since.elapsed() > Duration::from_millis(300))
        {
            return Exit::Outdated(found);
        }
        if check.is_some() && leaving.is_none() && started.elapsed() > HELLO_TIMEOUT {
            return Exit::Silent;
        }
        if state() == State::Up && last_ok.elapsed() > LINK_TIMEOUT {
            return Exit::Lost;
        }
        while arq.free(Prio::High) > 0 {
            let Ok(m) = TO_BT_HI.try_receive() else { break };
            let _ = arq.push(&m, Prio::High);
        }
        while arq.free(Prio::Low) > 0 {
            let Ok(m) = TO_BT_LO.try_receive() else { break };
            let _ = arq.push(&m, Prio::Low);
        }
        if !arq.has_pending() {
            let woke = select3(TO_BT_HI.ready_to_receive(), TO_BT_LO.ready_to_receive(), Timer::after(KEEPALIVE)).await;
            if !matches!(woke, Either3::Third(())) {
                continue;
            }
        }
        arq.next_frame().encode(&mut tx);
        if uart.write_all(&tx).await.is_err() {
            continue;
        }
        let errors = arq.stats.errors();
        let got = match with_timeout(REPLY_TIMEOUT, read_frame(uart, &mut rx)).await {
            Ok(true) => arq.receive(&rx),
            _ => arq.receive(&[0; FRAME_LEN]),
        };
        if arq.stats.errors() == errors {
            last_ok = Instant::now();
        } else {
            LINK_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(m) = got {
            if let Ok(BtMsg::Hello { fw_version, role, .. }) = BtMsg::decode(&m) {
                if role != ROLE {
                    continue;
                }
                if let Some(want) = check
                    && fw_version != want
                {
                    push(&mut arq, &BtMsg::EnterRom);
                    leaving = Some((fw_version, Instant::now()));
                    continue;
                }
                check = None;
                set_state(State::Up);
                storage::clear_bt_fails();
            }
            deliver(&m);
        }
    }
}

/// The Bluetooth module's task (core 0; Bluetooth hosts are lesser targets, and core 1 keeps to the AT32F405 links).
/// `fitted`: what `main` read on GPIO29 before it became the UART's RX.
#[embassy_executor::task]
pub async fn run(mut uart: BufferedUart, fitted: bool, seed: u8) -> ! {
    if !fitted {
        set_state(State::Absent);
        info!("no Bluetooth module");
        uart_log!("no Bluetooth module (GPIO29 low): two PCs only");
        loop {
            Timer::after_secs(3600).await;
        }
    }
    uart_log!("Bluetooth module fitted");
    set_state(State::Starting);
    let check = (!embedded::ZLIB.is_empty()).then_some(embedded::ESP_FW_VERSION).flatten();
    let mut tries = 0u8;
    let mut last: Option<Attempt> = None;
    let mut seed = seed;
    loop {
        uart.set_baudrate(BAUD);
        let exit = link_loop(&mut uart, seed, check).await;
        seed = seed.wrapping_add(0x55);
        kvm::with(|e| e.bt_link_down());
        let in_rom = match exit {
            Exit::Lost => {
                // Maybe it restarted with TX low and sits in its ROM: then restart it once more, into its firmware.
                warn!("Bluetooth module stopped answering");
                uart_log!("Bluetooth module stopped answering");
                set_state(State::Starting);
                if espisp::leave_rom(&mut uart).await {
                    uart_log!("Bluetooth module was in its ROM bootloader: restarted");
                }
                continue;
            }
            Exit::Outdated(found) => {
                info!("Bluetooth module: firmware {=u32:#x}, embedded {:?}", found, check);
                // It restarts into its ROM on EnterRom.
                Timer::after_millis(300).await;
                true
            }
            Exit::Silent => false,
        };
        let fails = storage::bt_fails();
        let next = if check.is_none() { Next::Wait } else { at32prog::next(fails, tries, last) };
        match next {
            Next::GiveUp => {
                warn!("Bluetooth module: {} attempts in a row failed: not programming it again", fails);
                uart_log!("Bluetooth module: {} attempts in a row failed: GAVE UP, Bluetooth off", fails);
                park(&mut uart, State::GaveUp).await;
            }
            Next::Wait => {
                // Nothing embedded to program with, or programmed but still not answering: the next power-up.
                warn!("Bluetooth module doesn't run the embedded firmware: Bluetooth off");
                uart_log!("Bluetooth module: not running the embedded firmware: Bluetooth off until the next power-up");
                park(&mut uart, State::Failed).await;
            }
            Next::Program => {}
        }
        tries += 1;
        // In flash before the attempt: a power loss during it counts too.
        storage::set_bt_fails(fails + 1).await;
        set_state(State::Programming);
        uart_log!("Bluetooth module: programming (attempt {} of {})", fails + 1, at32prog::MAX_FAILS);
        if !in_rom {
            // Blank or not coming up: it resets over and over; hold its BOOT strap (our TX) low across one of them.
            espisp::break_into_rom(&mut uart, 3000).await;
        }
        let t = Instant::now();
        match espisp::program(&mut uart, &image()).await {
            Ok(()) => {
                uart_log!("Bluetooth module programmed in {} ms", t.elapsed().as_millis());
                last = Some(Attempt::NoAnswer);
            }
            Err(e) => {
                warn!("Bluetooth module: programming failed: {}", e);
                uart_log!("Bluetooth module: programming failed");
                let _ = espisp::leave_rom(&mut uart).await;
                last = Some(Attempt::TransferFailed);
            }
        }
        // The link now sees whether it answers with the embedded firmware (and clears the count if so).
        set_state(State::Starting);
    }
}

/// Bluetooth stays off until the next power-up (or `BtRetry`): the module is let out of its ROM, if it is there, and
/// left alone.
async fn park(uart: &mut BufferedUart, s: State) -> ! {
    set_state(s);
    let _ = espisp::leave_rom(uart).await;
    loop {
        Timer::after_secs(3600).await;
    }
}
