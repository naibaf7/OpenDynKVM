//! Where the module hung. A timer interrupt at level 3, above the Bluetooth controller's and the link's (level 1), notes
//! every 10 ms where the CPU was (EPC3, and EPS3's interrupt level) in RTC fast RAM, and the watchdog task notes how
//! many samples there were at each feed. When the watchdog restarts the module (a system reset, which keeps RTC fast
//! RAM), the next start logs the samples from after the last feed: the code it was stuck in, or how soon the samples
//! stopped (the interrupts masked). Before, the RP2354B power-cycled a silent module after 3 s (`LINK_TIMEOUT`),
//! which clears RTC RAM too (2026-11-25). An NMI sampler (0.1.60) took no samples at all and may have hung the module
//! by itself: level 3 it is.

use esp_hal::Blocking;
use esp_hal::timer::PeriodicTimer;

use crate::log;

const SAMPLES: usize = 32;
const MAGIC: u32 = u32::from_le_bytes(*b"HNG3");
const PERIOD_MS: u64 = 10;

/// Magic, samples taken, samples taken at the last feed, then (PC, PS) for each sample, a ring.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut HANG: [u32; 3 + 2 * SAMPLES] = [0; 3 + 2 * SAMPLES];

/// Set before the interrupt is on, then used by the interrupt alone.
static mut TIMER: Option<PeriodicTimer<'static, Blocking>> = None;

#[esp_hal::handler(priority = esp_hal::interrupt::Priority::Priority3)]
fn sample() {
    let (pc, ps): (u32, u32);
    // SAFETY: reading two special registers. In a level-3 handler they hold the interrupted code's PC and PS.
    unsafe { core::arch::asm!("rsr.epc3 {0}", "rsr.eps3 {1}", out(reg) pc, out(reg) ps, options(nomem, nostack)) };
    // SAFETY: this interrupt is the only writer of the ring and the count (`fed` writes its own word).
    unsafe {
        let h = &raw mut HANG;
        let n = (*h)[1];
        let at = 3 + 2 * (n as usize % SAMPLES);
        (*h)[at] = pc;
        (*h)[at + 1] = ps;
        (*h)[1] = n.wrapping_add(1);
        if let Some(t) = (*(&raw mut TIMER)).as_mut() {
            t.clear_interrupt();
        }
    }
}

/// The watchdog was fed: samples from here on are the ones a hang leaves.
pub fn fed() {
    // SAFETY: a word read and a word written; the interrupt only reads neither.
    unsafe {
        let h = &raw mut HANG;
        (*h)[2] = core::ptr::read_volatile(&(*h)[1]);
    }
}

/// At start: what the last run left, if the watchdog ended it; then sampling from scratch.
pub fn start(timer: esp_hal::timer::timg::Timer<'static>, hung: bool) {
    // SAFETY: before the interrupt is on.
    let h = unsafe { &mut *(&raw mut HANG) };
    if hung {
        if h[0] != MAGIC {
            log::line(format_args!("the last run hung, and left no samples"));
        } else {
            let (n, at_feed) = (h[1], h[2]);
            let after = n.wrapping_sub(at_feed);
            log::line(format_args!(
                "the last run hung: {after} samples ({} ms) after the last feed, the last (PC/level):",
                after as u64 * PERIOD_MS
            ));
            let shown = (after as usize).min(SAMPLES);
            let mut line: heapless::String<100> = heapless::String::new();
            for k in (0..shown).rev() {
                let i = n.wrapping_sub(1 + k as u32) as usize % SAMPLES;
                let (pc, ps) = (h[3 + 2 * i], h[3 + 2 * i + 1]);
                let mut item: heapless::String<16> = heapless::String::new();
                let _ = core::fmt::Write::write_fmt(&mut item, format_args!("{pc:08x}/{} ", ps & 0xF));
                if line.push_str(&item).is_err() {
                    log::line(format_args!("  {line}"));
                    line.clear();
                    let _ = line.push_str(&item);
                }
            }
            if !line.is_empty() {
                log::line(format_args!("  {line}"));
            }
        }
    }
    h.fill(0);
    h[0] = MAGIC;
    let mut t = PeriodicTimer::new(timer);
    t.set_interrupt_handler(sample);
    // SAFETY: the interrupt isn't on yet; from here on only it touches the timer.
    unsafe {
        let slot = &mut *(&raw mut TIMER);
        *slot = Some(t);
        if let Some(t) = slot.as_mut() {
            let _ = t.start(esp_hal::time::Duration::from_millis(PERIOD_MS));
            t.listen();
        }
    }
}
