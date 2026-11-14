//! The Bluetooth module (ESP32-C3): whether it is there, its link, and its firmware (software.md §4.13). It is on the
//! board (ESP32-C3-MINI-1, boards 0.16.0 on, when fitted), or an ESP32-C3 board on a front port (a *dongle*, for a
//! KVM without it: [`dongle_task`]). Both run the same module firmware, and everything above the line is the same.
//!
//! - **Fitted or not:** boards 0.12–0.15 leave GPIO28/29 unconnected, and a 0.16.0 board can be built without the
//!   module (assembly variant without `Fit = BT`). At boot `main` samples `ESP_TXD` (GPIO29) for up to 0.5 s with the
//!   pad's pull-down on: only the module's TXD0, idling high, lifts it. Without it [`run`] stops at once, nothing is
//!   sent on GPIO28, and the KVM looks for a dongle instead.
//! - **Link:** UART0 (GPIO28 TX → the module's RXD0, GPIO29 RX ← its TXD0) at [`BAUD`], or the dongle's USB serial
//!   port ([`crate::dongle`]): the AT32F405 links' frames and ARQ ([`kvm_proto::link::bt`]), the RP2354B as master, one
//!   frame out and one back per exchange.
//! - **Firmware:** the image is embedded (`build.rs`, from `cargo xtask dist`). A module that says `Hello` with another
//!   version is reprogrammed by [`crate::espisp`], as is one that says nothing within [`HELLO_TIMEOUT`]:
//!   - on the board, it is sent into its ROM bootloader (`EnterRom`), or caught there with a break on TX (blank, or a
//!     firmware that doesn't come up and resets). Attempts as for the AT32F405s (`kvm_core::at32prog`): 5 failed in a
//!     row, counted in flash, then no more until the image changes, the key is held at power-up, or
//!     `RebootTarget::BtRetry`. The module's BOOT strap is on TX; its EN has a line from GPIO9 from 0.18.0 on
//!     (through 1 kΩ, hardware/tools/gen_schematic.py block 16), with which a reset under a break catches it in its ROM
//!     at once; before, only its RC, and the break waits for a reset the module makes by itself.
//!   - a dongle is sent `EnterRom` too, and restarted into its ROM by its DTR/RTS if it doesn't answer (DTR/RTS didn't
//!     get one whose firmware runs there reliably). One that never said `Hello` is someone's ESP32 board, not the
//!     module: it is only programmed if it is an ESP32-C3 with a blank first sector ([`espisp::blank_c3`]: new, erased,
//!     or the module after a programming that broke off), else it is left as it is, an ordinary device. Attempts are
//!     counted per board in RAM, the same 5.
//! - **State:** [`State`] goes to the engine (`Status.bt_module`, the diagnostics) and to the self-test
//!   (`selftest::BT_FW`, `BT_GAVE_UP`).

use core::cell::RefCell;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either, Either4, select, select4};
use embassy_rp::gpio::Flex;
use embassy_rp::uart::BufferedUart;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::String;
use kvm_core::at32prog::{self, Attempt, Next};
use kvm_core::bt::BtOut;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::link::bt::{BAUD, BtMsg, Motion, ROLE};
use kvm_proto::link::{Arq, FRAME_LEN, Frame, MAX_MSG, Msg, PROTOCOL_VERSION, Prio};

use crate::dongle::Dongle;
use crate::espisp::{self, Image, Line};
use crate::uartlog::uart_log;
use crate::{kvm, storage};

const KEEPALIVE: Duration = Duration::from_millis(5);
/// A frame takes 0.24 ms at 3 Mbaud; the module answers within a millisecond.
const REPLY_TIMEOUT: Duration = Duration::from_millis(4);
/// On a dongle a frame is two USB packets each way, one per millisecond (`dongle.rs`).
const REPLY_TIMEOUT_USB: Duration = Duration::from_millis(10);
/// Exchanges that fail this long make the link lost. The module answers within a millisecond, but its Bluetooth side can
/// keep its flash busy a while (a bond being written).
const LINK_TIMEOUT: Duration = Duration::from_secs(3);
/// How long a just-reset module may take to say `Hello` (its boot: ROM, second-stage bootloader, radio start).
const HELLO_TIMEOUT: Duration = Duration::from_secs(4);

/// What the RP2354B knows of the module (`Status.bt_module`, the diagnostics, the self-test; the log).
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
pub enum State {
    /// Not looked yet.
    Unknown = 0,
    /// Not fitted (or a board before 0.16.0), and no dongle.
    Absent = 1,
    /// The link is up and its firmware is the embedded one.
    Up = 2,
    /// Being programmed.
    Programming = 3,
    /// It doesn't answer or couldn't be programmed: Bluetooth is off until the next power-up (a dongle: until it is
    /// plugged in again).
    Failed = 4,
    /// Waiting for its `Hello` (after a reset, or after programming).
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
/// The selected host's motion, coalesced until the link has room for it ([`Motion`]): one report waits at a time.
static MOTION: embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, RefCell<Motion>> =
    embassy_sync::blocking_mutex::Mutex::new(RefCell::new(Motion::new()));
static MOTION_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn take_motion() -> Option<kvm_proto::link::bt::Out> {
    MOTION.lock(|m| m.borrow_mut().take())
}

/// Queues a message for the module; dropped (and counted) if the queue is full.
fn queue(m: &BtMsg) {
    let mut buf = [0u8; MAX_MSG];
    let Ok(n) = m.encode(&mut buf) else { return };
    let Ok(msg) = Msg::from_slice(&buf[..n]) else { return };
    let q = if m.prio() == Prio::High { &TO_BT_HI } else { &TO_BT_LO };
    if q.try_send(msg).is_err() {
        kvm::DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}

/// The module's image embedded by `build.rs` (empty without one: then a module is used as it is, never programmed).
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/esp_fw.rs"));
    pub static HEAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/esp_head.bin.z"));
    pub static TAIL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/esp_tail.bin.z"));
}

fn image() -> Image {
    Image {
        head: embedded::HEAD,
        head_len: embedded::ESP_HEAD_LEN,
        tail: embedded::TAIL,
        len: embedded::ESP_IMAGE_LEN,
        md5: embedded::ESP_IMAGE_MD5,
        blank_md5: embedded::ESP_BLANK_MD5,
    }
}

fn has_image() -> bool {
    !embedded::HEAD.is_empty()
}

/// The version a module must run: the embedded image's (None without one: any module is used as it is).
fn wanted() -> Option<u32> {
    has_image().then_some(embedded::ESP_FW_VERSION).flatten()
}

/// Identifies the embedded image for the programming record (CRC-32 of its MD5; 0 without an image).
pub fn image_crc() -> u32 {
    if has_image() { kvm_proto::crc::crc32(embedded::ESP_IMAGE_MD5) } else { 0 }
}

/// Queues what the engine asks of the module; dropped while there is no module (or its link is down). Motion is
/// coalesced ([`Motion`]); anything else goes after the motion before it.
pub fn send(o: BtOut) {
    if state() != State::Up {
        return;
    }
    if let BtOut::Input { host, id, data } = &o
        && Motion::is_motion(*id)
    {
        if let Some((host, id, data)) = MOTION.lock(|m| m.borrow_mut().add(*host, *id, data)) {
            queue(&BtMsg::Input { host, id, data: &data });
        }
        MOTION_READY.signal(());
        return;
    }
    while let Some((host, id, data)) = take_motion() {
        queue(&BtMsg::Input { host, id, data: &data });
    }
    let m = match &o {
        BtOut::Input { host, id, data } => BtMsg::Input { host: *host, id: *id, data },
        BtOut::Select(host) => BtMsg::Select { host: *host },
        BtOut::Pair { on, seconds } => BtMsg::Pair { on: *on, seconds: *seconds },
        BtOut::Forget(host) => BtMsg::Forget { host: *host },
        BtOut::Passkey { host, value } => BtMsg::Passkey { host: *host, value: *value },
        BtOut::Policy { host, policy } => BtMsg::Policy { host: *host, policy: *policy },
        BtOut::RequestState => BtMsg::RequestState,
    };
    queue(&m);
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
    /// Another firmware version (on the board's UART the module was told to restart into its ROM bootloader).
    Outdated(u32),
    /// No `Hello` in time: blank, a firmware that doesn't come up, or (a dongle) not the module at all.
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

/// Reads one frame within `timeout`: the next [`FRAME_LEN`] bytes that start with the magic and pass the CRC. Bytes
/// before a magic are skipped, which resynchronises after a lost byte.
async fn read_frame<L: Line>(line: &mut L, raw: &mut [u8; FRAME_LEN], timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let left = || deadline.saturating_duration_since(Instant::now());
    let mut b = [0u8; 1];
    loop {
        match line.read_within(&mut b, left()).await {
            Ok(1) if b[0] == kvm_proto::link::frame::MAGIC => break,
            Ok(0) | Err(()) => return false,
            Ok(_) => {}
        }
    }
    raw[0] = b[0];
    let mut got = 1;
    while got < FRAME_LEN {
        match line.read_within(&mut raw[got..], left()).await {
            Ok(0) | Err(()) => return false,
            Ok(n) => got += n,
        }
    }
    Frame::decode(raw).is_ok()
}

/// Runs the link until it ends. `silent_after`: how long a module may stay without `Hello` (None: as long as it
/// takes). `on_hello(current)` runs on every `Hello` from the module, `current` whether it runs the version wanted.
async fn link_loop<L: Line>(
    line: &mut L,
    seed: u8,
    check: Option<u32>,
    silent_after: Option<Duration>,
    on_hello: &impl Fn(bool),
) -> Exit {
    let mut arq: Arq<8> = Arq::new(seed);
    let mut check = check;
    let started = Instant::now();
    let mut last_ok = started;
    let mut up = false;
    let mut tx = [0u8; FRAME_LEN];
    let mut rx = [0u8; FRAME_LEN];
    // The module's boot log and anything from before.
    let mut junk = [0u8; 64];
    while let Ok(n) = line.read_within(&mut junk, Duration::from_millis(5)).await {
        if n == 0 {
            break;
        }
    }
    push(&mut arq, &BtMsg::Hello { proto: PROTOCOL_VERSION, fw_version: crate::FW_VERSION, role: 0, flags: 0 });
    // Set once the module on the board runs another version: `EnterRom` is on its way, return once it is acknowledged.
    let mut leaving: Option<(u32, Instant)> = None;
    loop {
        if let Some((found, since)) = leaving
            && (!arq.has_pending() || since.elapsed() > Duration::from_millis(300))
        {
            return Exit::Outdated(found);
        }
        if !up && leaving.is_none() && silent_after.is_some_and(|t| started.elapsed() > t) {
            return Exit::Silent;
        }
        if up && last_ok.elapsed() > LINK_TIMEOUT {
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
        // The motion so far, after what was queued before it.
        if arq.free(Prio::High) > 0
            && let Some((host, id, data)) = take_motion()
        {
            push(&mut arq, &BtMsg::Input { host, id, data: &data });
        }
        if !arq.has_pending() {
            let woke = select4(
                TO_BT_HI.ready_to_receive(),
                TO_BT_LO.ready_to_receive(),
                MOTION_READY.wait(),
                Timer::after(KEEPALIVE),
            )
            .await;
            if !matches!(woke, Either4::Fourth(())) {
                continue;
            }
        }
        arq.next_frame().encode(&mut tx);
        if line.write_all(&tx).await.is_err() {
            // A dongle that has no room (or is going away): try again after a while.
            Timer::after(KEEPALIVE).await;
            continue;
        }
        let errors = arq.stats.errors();
        let reply = if L::UART { REPLY_TIMEOUT } else { REPLY_TIMEOUT_USB };
        let got = if read_frame(line, &mut rx, reply).await { arq.receive(&rx) } else { arq.receive(&[0; FRAME_LEN]) };
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
                let current = check.is_none_or(|want| fw_version == want);
                on_hello(current);
                if !current {
                    // It restarts into its ROM by itself (FORCE_DOWNLOAD_BOOT), on the board's UART and on a dongle
                    // alike: DTR/RTS didn't get a dongle whose firmware runs there reliably (first dongle, 2026-11-14),
                    // and on the board there is no other way.
                    push(&mut arq, &BtMsg::EnterRom);
                    leaving = Some((fw_version, Instant::now()));
                    continue;
                }
                check = None;
                up = true;
                set_state(State::Up);
            }
            deliver(&m);
        }
    }
}

/// The module on the board (core 0; Bluetooth hosts are lesser targets, and core 1 keeps to the AT32F405 links).
/// `fitted`: what `main` read on GPIO29 before it became the UART's RX. `en`: GPIO9, the module's EN through 1 kΩ on
/// boards from 0.18.0 (open drain, released), unconnected before: pulling it low does nothing there.
#[embassy_executor::task]
pub async fn run(mut uart: BufferedUart, fitted: bool, seed: u8, mut en: Flex<'static>) -> ! {
    release(&mut en);
    if !fitted {
        // No module on the board: the dongle task has the state from here on.
        if state() == State::Unknown {
            set_state(State::Absent);
        }
        info!("no Bluetooth module on the board");
        uart_log!("no Bluetooth module on the board (GPIO29 low): an ESP32-C3 board on a front port can be one");
        loop {
            Timer::after_secs(3600).await;
        }
    }
    uart_log!("Bluetooth module fitted");
    set_state(State::Starting);
    let check = wanted();
    let mut tries = 0u8;
    let mut last: Option<Attempt> = None;
    let mut seed = seed;
    let on_hello = |current: bool| {
        if current {
            storage::clear_bt_fails();
        }
    };
    loop {
        uart.set_baudrate(BAUD);
        let exit = link_loop(&mut uart, seed, check, check.map(|_| HELLO_TIMEOUT), &on_hello).await;
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
                } else {
                    // Hung in its firmware: restarted by EN where there is one (its watchdog does it within 5 s
                    // anyway).
                    reset(&mut en).await;
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
            // Blank or not coming up: hold its BOOT strap (our TX) low across a reset: one by EN where there is one,
            // else one of those it makes by itself (a ROM that finds no firmware restarts about every 3 s, by its TG0
            // watchdog; the break lasts 6 s).
            catch_in_rom(&mut uart, &mut en).await;
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

/// The module's EN (GPIO9, boards from 0.18.0): released, its 10 kΩ pull-up holds it high.
fn release(en: &mut Flex<'static>) {
    en.set_pull(embassy_rp::gpio::Pull::None);
    en.set_as_input();
}

/// Restarts the module through EN (boards from 0.18.0; nothing happens before): 20 ms low, then up through its RC.
async fn reset(en: &mut Flex<'static>) {
    en.set_low();
    en.set_as_output();
    Timer::after_millis(20).await;
    release(en);
}

/// Into the module's ROM bootloader: its BOOT strap (IO9, on our TX) held low across a reset. First a short break with
/// a reset by EN in it (boards from 0.18.0, as BOOT held at power-up); if no ROM answers, the long break that waits for
/// a reset the module makes by itself.
async fn catch_in_rom(uart: &mut BufferedUart, en: &mut Flex<'static>) {
    uart.set_baudrate(espisp::ROM_BAUD);
    let bits = 300 * espisp::ROM_BAUD / 1000;
    embassy_futures::join::join(uart.send_break(bits), async {
        Timer::after_millis(5).await;
        reset(en).await;
    })
    .await;
    if !espisp::listening(uart).await {
        espisp::break_into_rom(uart, 6000).await;
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

// ── A dongle: an ESP32-C3 board on a front port ─────────────────────────────
//
// Every restart of the chip takes it off USB: its own (an erased chip's ROM finds no firmware and restarts every
// 2.5 s or so, first board 2026-11-14), and the ones the RP2354B makes (DTR/RTS into the ROM, RTS out of it). So a
// session ends with every restart, and what it found out stays with the board ([`Board`], by its USB serial number):
// the next session, when it is back, carries on from there.

/// A front port's ESP32-C3 board for [`dongle_task`] (`host.rs`, when the engine wants it found out).
pub static DONGLE: Channel<CriticalSectionRawMutex, Dongle, 1> = Channel::new();
/// The board on that port left Hub-0 (`host.rs`).
pub static DONGLE_GONE: [Signal<CriticalSectionRawMutex, ()>; 3] = [const { Signal::new() }; 3];

/// Sessions with boards since start, and the last steps they went through (diagnostics: `BOARD_SESSIONS`,
/// `BOARD_STEPS`).
pub static SESSIONS: AtomicU32 = AtomicU32::new(0);
pub static STEPS: AtomicU32 = AtomicU32::new(0);

/// A step of a board's session, one hexadecimal digit in [`STEPS`] (the newest last).
mod step {
    pub const NEW: u32 = 0x1;
    pub const KNOWN: u32 = 0x2;
    pub const HELLO_WAIT: u32 = 0x3;
    pub const SILENT: u32 = 0x4;
    pub const LOST: u32 = 0x5;
    pub const OUTDATED: u32 = 0x6;
    pub const ROM_ANSWERS: u32 = 0x7;
    pub const INTO_ROM: u32 = 0x8;
    pub const LINES_FAILED: u32 = 0x9;
    pub const BLANK: u32 = 0xA;
    pub const DECLINED: u32 = 0xB;
    pub const PROGRAMMING: u32 = 0xC;
    pub const PROGRAMMED: u32 = 0xD;
    pub const PROGRAM_FAILED: u32 = 0xE;
    pub const UP: u32 = 0xF;
}

fn step(code: u32) {
    // Only the dongle task writes it.
    STEPS.store(STEPS.load(Ordering::Relaxed) << 4 | code, Ordering::Relaxed);
}

/// Restarts the chip into its ROM, and notes whether its DTR/RTS went through.
/// Lets a board out of its ROM bootloader into its firmware: by the RTC watchdog, or by RTS if the ROM doesn't
/// answer (RTS from the ROM brought a board back in the ROM, `espisp::watchdog_restart`).
async fn out_of_rom(line: &mut crate::dongle::UsbLine) {
    if espisp::watchdog_restart(line).await.is_err() {
        let _ = line.reset().await;
    }
}

async fn into_rom(line: &mut crate::dongle::UsbLine) {
    step(step::INTO_ROM);
    if line.restart_into_rom().await.is_err() {
        step(step::LINES_FAILED);
    }
}

/// What the KVM knows of a board since power-up, across its restarts.
struct Board {
    serial: String<24>,
    first_seen: Instant,
    /// Since when it should say `Hello` (first seen, programmed, or restarted when it stopped answering): the time
    /// left of [`HELLO_TIMEOUT`] is all the next session waits, as a chip that restarts sooner ends every session.
    waiting_since: Instant,
    /// It said `Hello` as the module, or was blank and is being made one.
    ours: bool,
    /// Restarted into its ROM on purpose: the next session goes there straight away.
    want_rom: bool,
    /// Restarts into the ROM that didn't get there, in a row.
    rom_misses: u8,
    /// Not the module (another firmware or chip): an ordinary device, also when it comes back from its restart.
    declined_at: Option<Instant>,
    /// Programming attempts that failed in a row, this session's tries and the last outcome (`at32prog`).
    fails: u8,
    tries: u8,
    last: Option<Attempt>,
    /// Bluetooth off for this board until the KVM restarts (*Try again*): it is left alone.
    parked: Option<State>,
}

const BOARDS: usize = 4;
/// How long a declined board stays declined: long enough for the restart that gave it back to its firmware.
const DECLINED_FOR: Duration = Duration::from_secs(10);
/// After a restart, how long the board may take to leave USB before its port is plugged in anew.
const REJOIN: Duration = Duration::from_millis(1500);
/// How often a restart into the ROM may miss before the board is taken as not the module.
const ROM_MISSES: u8 = 5;

fn board<'a>(boards: &'a mut heapless::Vec<Board, BOARDS>, serial: &String<24>) -> &'a mut Board {
    if let Some(i) = boards.iter().position(|b| &b.serial == serial) {
        return &mut boards[i];
    }
    if boards.is_full() {
        boards.remove(0);
    }
    let _ = boards.push(Board {
        serial: serial.clone(),
        first_seen: Instant::now(),
        waiting_since: Instant::now(),
        ours: false,
        want_rom: false,
        rom_misses: 0,
        declined_at: None,
        fails: 0,
        tries: 0,
        last: None,
        parked: None,
    });
    let last = boards.len() - 1;
    &mut boards[last]
}

enum Outcome {
    /// Not the module: an ordinary device from now on.
    Declined,
    /// Unplugged, or off USB for a restart.
    Gone,
}

/// The module as an ESP32-C3 board on a front port, for a KVM without it on the board (core 0).
#[embassy_executor::task]
pub async fn dongle_task(seed: u8) -> ! {
    let boards: RefCell<heapless::Vec<Board, BOARDS>> = RefCell::new(heapless::Vec::new());
    let mut seed = seed;
    loop {
        let mut d = DONGLE.receive().await;
        let port = d.port;
        // Its port task signals this only once it has handed the board over: one left from the board before is void.
        DONGLE_GONE[port.index()].reset();
        SESSIONS.fetch_add(1, Ordering::Relaxed);
        let outcome = match select(serve(&mut d, &boards, seed), DONGLE_GONE[port.index()].wait()).await {
            Either::First(o) => o,
            Either::Second(()) => Outcome::Gone,
        };
        seed = seed.wrapping_add(0x55);
        match outcome {
            Outcome::Declined => {
                uart_log!("port {}: the ESP32 board is not the Bluetooth module: an ordinary device", port.index());
                kvm::with(|e| e.dongle_declined(port));
                if !matches!(state(), State::Absent | State::Unknown) {
                    kvm::with(|e| e.bt_link_down());
                    set_state(State::Absent);
                }
            }
            // Off USB: unplugged, or a restart (it comes back as a new device, and its board record carries on). The
            // engine has it from the port (`port_detached`: Bluetooth absent until a board is the module again).
            Outcome::Gone => {
                kvm::with(|e| e.bt_link_down());
                STATE.store(State::Absent as u8, Ordering::Relaxed);
            }
        }
    }
}

/// The chip was just restarted: it leaves USB, and the session for the device it comes back as carries on (the
/// dongle task ends this one when it leaves). If Hub-0 hasn't seen it go after [`REJOIN`], its port is plugged in
/// anew, which makes it attach anew.
async fn rejoin(index: usize) -> Outcome {
    Timer::after(REJOIN).await;
    kvm::REPLUG[index].signal(());
    core::future::pending().await
}

/// One session with a board on a front port, from its attaching to its leaving USB: finds out whether it is the
/// module, and keeps it running as one; programs it when it runs another version, or none (only if it ever said
/// `Hello`, or is blank). Returns `Declined` when it isn't the module.
async fn serve(d: &mut Dongle, boards: &RefCell<heapless::Vec<Board, BOARDS>>, seed: u8) -> Outcome {
    let Dongle { port, line, serial } = d;
    let port = *port;
    let index = port.index();
    let check = wanted();
    let rec = |f: &mut dyn FnMut(&mut Board)| f(board(&mut boards.borrow_mut(), serial));
    let get = |f: &dyn Fn(&Board) -> bool| f(board(&mut boards.borrow_mut(), serial));
    let claim = || {
        let mut newly = false;
        rec(&mut |b| {
            newly = !b.ours;
            b.ours = true;
        });
        if newly {
            uart_log!("port {}: the ESP32 board is the Bluetooth module", index);
        }
        kvm::with(|e| e.dongle_claimed(port));
    };

    if get(&|b| b.declined_at.is_some_and(|t| t.elapsed() < DECLINED_FOR)) {
        step(step::DECLINED);
        return Outcome::Declined;
    }
    if get(&|b| b.ours) {
        claim();
        let mut parked = None;
        rec(&mut |b| parked = b.parked);
        if let Some(s) = parked {
            // Bluetooth off for this board until the KVM restarts: leave it alone.
            set_state(s);
            return core::future::pending().await;
        }
        step(step::KNOWN);
        if state() != State::Programming {
            set_state(State::Starting);
        }
    } else {
        step(step::NEW);
        if get(&|b| b.first_seen.elapsed() < Duration::from_millis(100)) {
            uart_log!("port {}: an ESP32 board: is it the Bluetooth module?", index);
        }
    }

    if !get(&|b| b.want_rom) {
        // What is left of the time to say `Hello` (an erased chip restarts every 2.5 s or so, which ends every session).
        let mut window = HELLO_TIMEOUT;
        rec(&mut |b| window = HELLO_TIMEOUT.checked_sub(b.waiting_since.elapsed()).unwrap_or(Duration::from_ticks(0)));
        let window = window.max(Duration::from_millis(500));
        step(step::HELLO_WAIT);
        let on_hello = |current: bool| {
            claim();
            if current {
                step(step::UP);
                rec(&mut |b| {
                    b.fails = 0;
                    b.tries = 0;
                    b.last = None;
                    b.rom_misses = 0;
                });
            }
        };
        let exit = link_loop(line, seed, check, Some(window), &on_hello).await;
        kvm::with(|e| e.bt_link_down());
        match exit {
            Exit::Lost => {
                step(step::LOST);
                warn!("Bluetooth module (port {}) stopped answering", index);
                uart_log!("Bluetooth module on port {} stopped answering: restarting it", index);
                set_state(State::Starting);
                rec(&mut |b| b.waiting_since = Instant::now());
                let _ = line.reset().await;
                return rejoin(index).await;
            }
            Exit::Outdated(found) => {
                // It restarts into its ROM on EnterRom (DTR/RTS is the way in if it doesn't).
                step(step::OUTDATED);
                info!("Bluetooth module (port {}): firmware {=u32:#x}, embedded {:?}", index, found, check);
                rec(&mut |b| b.want_rom = true);
                return rejoin(index).await;
            }
            Exit::Silent => step(step::SILENT),
        }
    }

    // Its ROM bootloader: it may listen already (restarted into it on purpose, or before by someone); else restart it
    // into it.
    if !espisp::listening(line).await {
        let mut misses = 0;
        rec(&mut |b| {
            b.rom_misses += 1;
            misses = b.rom_misses;
            b.want_rom = true;
        });
        if misses > ROM_MISSES {
            // It won't go into its ROM over USB (a chip without its USB serial port's reset, or not an ESP32).
            uart_log!("port {}: the ESP32 board doesn't start its ROM bootloader", index);
            if !get(&|b| b.ours) {
                step(step::DECLINED);
                rec(&mut |b| b.declined_at = Some(Instant::now()));
                return Outcome::Declined;
            }
            rec(&mut |b| b.parked = Some(State::Failed));
            set_state(State::Failed);
            return core::future::pending().await;
        }
        into_rom(line).await;
        return rejoin(index).await;
    }
    step(step::ROM_ANSWERS);
    rec(&mut |b| {
        b.want_rom = false;
        b.rom_misses = 0;
    });
    if !get(&|b| b.ours) {
        // Someone's own ESP32 board unless it is a blank ESP32-C3.
        let blank = has_image() && matches!(espisp::blank_c3(line, &image()).await, Ok(true));
        if !blank {
            step(step::DECLINED);
            rec(&mut |b| b.declined_at = Some(Instant::now()));
            // Back to its own firmware; it comes back as a new device, which is then declined at once.
            out_of_rom(line).await;
            return rejoin(index).await;
        }
        step(step::BLANK);
        claim();
    }

    let (mut fails, mut tries, mut last) = (0, 0, None);
    rec(&mut |b| (fails, tries, last) = (b.fails, b.tries, b.last));
    let next = if check.is_none() { Next::Wait } else { at32prog::next(fails, tries, last) };
    if !matches!(next, Next::Program) {
        let s = if matches!(next, Next::GiveUp) { State::GaveUp } else { State::Failed };
        uart_log!("Bluetooth module on port {}: not running the embedded firmware: Bluetooth off", index);
        rec(&mut |b| b.parked = Some(s));
        set_state(s);
        out_of_rom(line).await;
        return rejoin(index).await;
    }
    rec(&mut |b| {
        b.tries += 1;
        b.fails += 1;
    });
    set_state(State::Programming);
    step(step::PROGRAMMING);
    uart_log!("Bluetooth module on port {}: programming (attempt {} of {})", index, fails + 1, at32prog::MAX_FAILS);
    let t = Instant::now();
    line.programming(true);
    let result = espisp::program(line, &image()).await;
    line.programming(false);
    step(if result.is_ok() { step::PROGRAMMED } else { step::PROGRAM_FAILED });
    match result {
        Ok(()) => uart_log!("Bluetooth module on port {} programmed in {} ms", index, t.elapsed().as_millis()),
        Err(e) => {
            warn!("Bluetooth module (port {}): programming failed: {}", index, e);
            uart_log!("Bluetooth module on port {}: programming failed", index);
        }
    }
    rec(&mut |b| {
        b.last = Some(if result.is_ok() { Attempt::NoAnswer } else { Attempt::TransferFailed });
        b.waiting_since = Instant::now();
        // A failed transfer is tried again from the ROM straight away.
        b.want_rom = result.is_err();
    });
    set_state(State::Starting);
    // Into its new firmware (programming ended with the watchdog restart): it comes back as a new device, whose session
    // hears its `Hello`. A transfer that failed left it in its ROM.
    if result.is_err() {
        out_of_rom(line).await;
    }
    rejoin(index).await
}
