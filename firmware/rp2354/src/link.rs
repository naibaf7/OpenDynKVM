//! RP2354B side of the link to the AT32F405s (software.md §6).
//!
//! Core 1 runs one SPI master loop per AT32F405 ([`run`]); core 0 runs a dispatcher (link messages → engine) and a
//! descriptor-set uploader per PC. They meet in the queues below.

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either5, select, select5};
use embassy_rp::gpio::{Input, Output};
use embassy_rp::spi::{self, Async, Spi};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use kvm_core::at32prog::{self, Attempt, Next};
use kvm_proto::ctrl::Pc;
use kvm_proto::descset::MAX_LEN;
use kvm_proto::link::msg::{DESC_CHUNK, Reassembly, fragments, hello_flags};
use kvm_proto::link::{Arq, FRAME_LEN, LinkMsg, MAX_MSG, Msg, PROTOCOL_VERSION, Prio};

use crate::board::At32Lines;
use crate::uartlog::uart_log;
use crate::{at32isp, kvm, storage};

/// SPI clock (clk_peri 150 MHz / 6): the AT32F405 slave receives at up to 36 MHz but transmits at only 25 MHz
/// (DS v2.03 Table 41, analysis.md finding 14).
pub const SPI_HZ: u32 = 25_000_000;
/// Gap between frames for the AT32F405's frame interrupt (software.md §5.3).
const FRAME_GAP: Duration = Duration::from_micros(15);
const KEEPALIVE: Duration = Duration::from_millis(5);
const LINK_TIMEOUT: Duration = Duration::from_millis(200);

type Queue = Channel<CriticalSectionRawMutex, Msg, 32>;

/// To each AT32F405: reports (sent first) and everything else.
static TO_AT_HI: [Queue; 2] = [const { Channel::new() }; 2];
static TO_AT_LO: [Queue; 2] = [const { Channel::new() }; 2];
/// From each AT32F405.
static FROM_AT: [Queue; 2] = [const { Channel::new() }; 2];
pub static LINK_UP: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
/// The link has been up at least once since boot (even if only until an outdated AT32F405 was taken down).
pub static LINK_SEEN: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
pub static LINK_ERRORS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
/// The AT32F405 said `Hello` with `NO_CRYSTAL`: it runs on its internal oscillator, without USB.
pub static NO_CRYSTAL: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
/// The descriptor set changed (or an AT32F405 restarted): upload it.
pub static DESC_DIRTY: [Signal<CriticalSectionRawMutex, ()>; 2] = [const { Signal::new() }; 2];

fn encode(m: &LinkMsg) -> Option<Msg> {
    let mut buf = [0u8; MAX_MSG];
    let n = m.encode(&mut buf).ok()?;
    Msg::from_slice(&buf[..n]).ok()
}

/// Queues a report for IN endpoint `ep` of PC `k`; false if the queue is full.
pub fn send_report(k: usize, ep: u8, data: &[u8]) -> bool {
    encode(&LinkMsg::Report { ep, data }).is_some_and(|m| TO_AT_HI[k].try_send(m).is_ok())
}

pub fn send_control(k: usize, m: &LinkMsg) -> bool {
    encode(m).is_some_and(|m| TO_AT_LO[k].try_send(m).is_ok())
}

async fn send_control_wait(k: usize, m: &LinkMsg<'_>) {
    if let Some(m) = encode(m) {
        TO_AT_LO[k].send(m).await;
    }
}

/// Requests to the core-1 loop of an AT32F405.
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum LinkCtl {
    /// Hold the AT32F405 in its ROM bootloader, which offers USB DFU to its PC until the next reset.
    Dfu,
}

pub static CTL: [Signal<CriticalSectionRawMutex, LinkCtl>; 2] = [const { Signal::new() }; 2];

/// The boot-time check of each AT32F405's firmware against the embedded image (software.md §4.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
#[repr(u8)]
pub enum FwCheck {
    Pending = 0,
    /// Runs the embedded version (or nothing is embedded to compare with).
    Current = 1,
    /// Was reprogrammed this boot.
    Updated = 2,
    /// Reprogramming failed this boot: it runs whatever it had, or nothing. Tried again at the next power-up.
    Failed = 3,
    /// Held in its ROM bootloader for USB DFU on request; not checked.
    Skipped = 4,
    /// [`at32prog::MAX_FAILS`] attempts in a row failed: not programmed again (`kvm_core::at32prog`).
    GaveUp = 5,
}

pub static FW_CHECK: [AtomicU8; 2] = [const { AtomicU8::new(FwCheck::Pending as u8) }; 2];

pub fn fw_check(k: usize) -> FwCheck {
    match FW_CHECK[k].load(Ordering::Relaxed) {
        1 => FwCheck::Current,
        2 => FwCheck::Updated,
        3 => FwCheck::Failed,
        4 => FwCheck::Skipped,
        5 => FwCheck::GaveUp,
        _ => FwCheck::Pending,
    }
}

fn set_fw_check(k: usize, c: FwCheck) {
    FW_CHECK[k].store(c as u8, Ordering::Relaxed);
}

/// How long a just-reset AT32F405 may take to say `Hello` before it counts as blank or incompatible.
const HELLO_TIMEOUT: Duration = Duration::from_secs(3);

/// The AT32F405 firmware embedded by `build.rs`.
mod embedded {
    include!(concat!(env!("OUT_DIR"), "/at32_fw.rs"));
    pub static IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/at32.bin"));
}

/// CRC-32 of the embedded AT32F405 image (0 without one): the programming record belongs to it.
pub fn image_crc() -> u32 {
    if embedded::IMAGE.is_empty() { 0 } else { kvm_proto::crc::crc32(embedded::IMAGE) }
}

/// Why [`link_loop`] returned.
enum Exit {
    Ctl(LinkCtl),
    /// The AT32F405 runs another firmware version (None: it never said `Hello`).
    Outdated(Option<u32>),
}

/// The SPI master loop for AT32F405 `k` (core 1). It owns that chip's reset and boot lines and its PC hub's reset,
/// and reprograms it when its firmware differs from the embedded image, as often as `kvm_core::at32prog` allows.
pub async fn run<T: spi::Instance>(
    k: usize,
    mut spi: Spi<'static, T, Async>,
    mut cs: Output<'static>,
    mut irq: Input<'static>,
    mut lines: At32Lines,
    seed: u8,
) -> ! {
    let mut check = match embedded::AT32_FW_VERSION {
        Some(v) if !embedded::IMAGE.is_empty() => Some(v),
        _ => {
            set_fw_check(k, FwCheck::Current);
            None
        }
    };
    let mut seed = seed;
    let mut tries = 0u8;
    let mut last: Option<Attempt> = None;
    loop {
        let matched = if last.is_some() { FwCheck::Updated } else { FwCheck::Current };
        let exit = link_loop(k, &mut spi, &mut cs, &mut irq, seed, check, matched).await;
        LINK_UP[k].store(false, Ordering::Relaxed);
        seed = seed.wrapping_add(0x55);
        match exit {
            Exit::Ctl(LinkCtl::Dfu) => {
                check = None;
                set_fw_check(k, FwCheck::Skipped);
                at32isp::reset_to_rom(&mut lines).await;
                info!("AT32F405 #{} is in its ROM bootloader", k + 1);
            }
            Exit::Outdated(found) => {
                let fails = storage::at32_fails(k);
                match at32prog::next(fails, tries, last) {
                    Next::GiveUp => {
                        check = None;
                        warn!("AT32F405 #{}: {} attempts in a row failed: not programming it again", k + 1, fails);
                        uart_log!("AT32F405 #{}: {} attempts in a row failed: GAVE UP", k + 1, fails);
                        set_fw_check(k, FwCheck::GaveUp);
                    }
                    Next::Wait => {
                        check = None;
                        uart_log!("AT32F405 #{}: still not running the embedded firmware: next power-up", k + 1);
                        set_fw_check(k, FwCheck::Failed);
                    }
                    Next::Program => {
                        tries += 1;
                        // In flash before the attempt: a power loss during it counts too.
                        storage::set_at32_fails(k, fails + 1).await;
                        info!(
                            "AT32F405 #{}: firmware {:?}, embedded {:?}: programming (attempt {} of {})",
                            k + 1,
                            found,
                            embedded::AT32_FW_VERSION,
                            fails + 1,
                            at32prog::MAX_FAILS
                        );
                        let t = Instant::now();
                        match at32isp::program(&mut spi, &mut cs, &mut lines, embedded::IMAGE, SPI_HZ).await {
                            Ok(()) => {
                                info!("AT32F405 #{} programmed in {} ms", k + 1, t.elapsed().as_millis());
                                uart_log!("AT32F405 #{} programmed in {} ms", k + 1, t.elapsed().as_millis());
                                last = Some(Attempt::NoAnswer);
                            }
                            Err(e) => {
                                warn!("AT32F405 #{} programming failed: {}", k + 1, e);
                                uart_log!("AT32F405 #{} programming FAILED: {:?}", k + 1, e);
                                last = Some(Attempt::TransferFailed);
                            }
                        }
                        // `check` stays: the link now sees whether the chip answers with the embedded firmware.
                    }
                }
            }
        }
    }
}

/// The link proper. With `check`, it returns once the AT32F405 says `Hello` with another firmware version, or not
/// at all within [`HELLO_TIMEOUT`]; with the right version, the check ends as `matched` and the failure count goes
/// back to 0.
async fn link_loop<T: spi::Instance>(
    k: usize,
    spi: &mut Spi<'static, T, Async>,
    cs: &mut Output<'static>,
    irq: &mut Input<'static>,
    seed: u8,
    mut check: Option<u32>,
    matched: FwCheck,
) -> Exit {
    let mut arq: Arq<16> = Arq::new(seed);
    // A (re)started link says `Hello`, which the AT32F405 answers with its own: that tells this side its version even
    // when only the RP2354B restarted. This side doesn't answer the AT32F405's `Hello` in turn, or the two would
    // answer each other forever (software.md §6.2).
    if let Some(m) =
        encode(&LinkMsg::Hello { proto: PROTOCOL_VERSION, fw_version: crate::FW_VERSION, role: 0, flags: 0 })
    {
        let _ = arq.push(&m, Prio::Low);
    }
    let mut tx = [0u8; FRAME_LEN];
    let mut rx = [0u8; FRAME_LEN];
    let started = Instant::now();
    let mut last_ok = started;
    // Frames before the AT32F405's first good one are not errors: it is still starting (or was just programmed) while
    // this side, with its `Hello` queued, already clocks frames. Counted, they showed as ~100 errors per boot.
    let mut answered = false;
    loop {
        if let Some(c) = CTL[k].try_take() {
            return Exit::Ctl(c);
        }
        if check.is_some() && started.elapsed() > HELLO_TIMEOUT {
            return Exit::Outdated(None);
        }
        while arq.free(Prio::High) > 0 {
            let Ok(m) = TO_AT_HI[k].try_receive() else { break };
            let _ = arq.push(&m, Prio::High);
        }
        while arq.free(Prio::Low) > 0 {
            let Ok(m) = TO_AT_LO[k].try_receive() else { break };
            let _ = arq.push(&m, Prio::Low);
        }
        if !arq.has_pending() && irq.is_low() {
            let woke = select5(
                TO_AT_HI[k].ready_to_receive(),
                TO_AT_LO[k].ready_to_receive(),
                irq.wait_for_high(),
                CTL[k].wait(),
                Timer::after(KEEPALIVE),
            )
            .await;
            if let Either5::Fourth(c) = woke {
                return Exit::Ctl(c);
            }
            if !TO_AT_HI[k].is_empty() || !TO_AT_LO[k].is_empty() {
                continue;
            }
        }

        arq.next_frame().encode(&mut tx);
        cs.set_low();
        let r = spi.transfer(&mut rx, &tx).await;
        cs.set_high();
        let errors = arq.stats.errors();
        // With no room to keep a message, the frame counts as lost: unacknowledged, the AT32F405 sends it again.
        // (Waiting here instead could deadlock against the dispatcher waiting for room in TO_AT_LO.)
        let room = !FROM_AT[k].is_full();
        let got = if r.is_ok() && room { arq.receive(&rx) } else { arq.receive(&[0; FRAME_LEN]) };
        if !room || arq.stats.errors() == errors {
            last_ok = Instant::now();
            answered = true;
            if !LINK_UP[k].swap(true, Ordering::Relaxed) {
                LINK_SEEN[k].store(true, Ordering::Relaxed);
                info!("link {} up", k + 1);
            }
        } else if answered {
            LINK_ERRORS[k].fetch_add(1, Ordering::Relaxed);
            if LINK_UP[k].load(Ordering::Relaxed) && last_ok.elapsed() > LINK_TIMEOUT {
                LINK_UP[k].store(false, Ordering::Relaxed);
                warn!("link {} down", k + 1);
            }
        }
        if let Some(m) = got {
            if let (Some(want), Ok(LinkMsg::Hello { fw_version, .. })) = (check, LinkMsg::decode(&m)) {
                if fw_version != want {
                    return Exit::Outdated(Some(fw_version));
                }
                check = None;
                set_fw_check(k, matched);
                storage::clear_at32_fails(k);
            }
            let _ = FROM_AT[k].try_send(m);
        }
        Timer::after(FRAME_GAP).await;
    }
}

fn pc(k: usize) -> Pc {
    Pc::from_index(k)
}

/// Link messages from AT32F405 `k` into the engine (core 0).
#[embassy_executor::task(pool_size = 2)]
pub async fn dispatch(k: usize) -> ! {
    let mut ctrl = Reassembly::<{ kvm_core::engine::MAX_CTRL_DATA }>::new();
    let mut ctrl_setup = [0u8; 8];
    let mut ctrl_tag = 0u8;
    let mut was_up = false;
    loop {
        let m = match select(FROM_AT[k].receive(), Timer::after_millis(100)).await {
            embassy_futures::select::Either::First(m) => m,
            embassy_futures::select::Either::Second(()) => {
                let up = LINK_UP[k].load(Ordering::Relaxed);
                if was_up && !up {
                    kvm::with(|e| e.pc_link_down(pc(k)));
                }
                was_up = up;
                continue;
            }
        };
        let Ok(msg) = LinkMsg::decode(&m) else { continue };
        let now = Instant::now().as_millis() as u32;
        match msg {
            LinkMsg::Hello { proto, fw_version, role, flags } => {
                info!("AT32F405 #{}: protocol {}, firmware {=u32:#x}, role {}", k + 1, proto, fw_version, role);
                let no_crystal = flags & hello_flags::NO_CRYSTAL != 0;
                if no_crystal && !NO_CRYSTAL[k].swap(true, Ordering::Relaxed) {
                    warn!("AT32F405 #{}: its 12 MHz crystal doesn't run: no USB to PC{}", k + 1, k + 1);
                    uart_log!("AT32F405 #{}: 12 MHz crystal NOT RUNNING, no USB to PC{}", k + 1, k + 1);
                }
                NO_CRYSTAL[k].store(no_crystal, Ordering::Relaxed);
                kvm::with(|e| e.pc_hello(pc(k), fw_version));
                // Not answered (`link_loop` said this side's `Hello`); a restarted AT32F405 needs the descriptor set.
                DESC_DIRTY[k].signal(());
            }
            LinkMsg::Status(s) => {
                let stale = kvm::with(|e| {
                    e.pc_status(pc(k), s);
                    s.desc_crc != e.descriptor_crc()
                });
                if stale {
                    DESC_DIRTY[k].signal(());
                }
            }
            LinkMsg::Report { ep, data } => kvm::with(|e| e.pc_out_report(pc(k), ep, data, now)),
            LinkMsg::SetReport { iface, report_type, report_id, data } => {
                kvm::with(|e| e.pc_set_report(pc(k), iface, report_type, report_id, data))
            }
            LinkMsg::ProtocolChanged { iface, protocol } => kvm::with(|e| e.pc_protocol(pc(k), iface, protocol)),
            LinkMsg::CtrlRequest { tag, setup, offset, data } => {
                if offset == 0 {
                    ctrl_tag = tag;
                    ctrl_setup = setup;
                    let out_len = if setup[0] & 0x80 == 0 { u16::from_le_bytes([setup[6], setup[7]]) } else { 0 };
                    let _ = ctrl.start(out_len as usize);
                }
                if tag != ctrl_tag || ctrl.put(offset as usize, data).is_err() {
                    continue;
                }
                if ctrl.is_complete() {
                    kvm::with(|e| e.pc_ctrl_request(pc(k), ctrl_tag, ctrl_setup, ctrl.data()));
                }
            }
            // RP2354B → AT32F405 only.
            LinkMsg::DescBegin { .. }
            | LinkMsg::DescData { .. }
            | LinkMsg::DescCommit
            | LinkMsg::CtrlReply { .. }
            | LinkMsg::Command(_) => {}
        }
    }
}

/// Uploads the engine's descriptor set to AT32F405 `k` whenever it changes (core 0).
#[embassy_executor::task(pool_size = 2)]
pub async fn uploader(k: usize) -> ! {
    let mut set = [0u8; MAX_LEN];
    loop {
        DESC_DIRTY[k].wait().await;
        // Let a burst of changes settle into one upload.
        Timer::after_millis(20).await;
        DESC_DIRTY[k].reset();
        let (n, crc) = kvm::with(|e| {
            let d = e.descriptor_set();
            set[..d.len()].copy_from_slice(d);
            (d.len(), e.descriptor_crc())
        });
        send_control_wait(k, &LinkMsg::DescBegin { total_len: n as u16, crc32: crc }).await;
        for (offset, chunk) in fragments(&set[..n], DESC_CHUNK) {
            send_control_wait(k, &LinkMsg::DescData { offset, data: chunk }).await;
        }
        send_control_wait(k, &LinkMsg::DescCommit).await;
        info!("descriptor set {=u32:#010x} ({} bytes) sent to AT32F405 #{}", crc, n, k + 1);
    }
}
