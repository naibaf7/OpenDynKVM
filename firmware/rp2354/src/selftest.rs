//! The boot-time self-test (design.md §10, step 1.5). Its result goes to `Status.selftest` and, for a few seconds, to
//! the LEDs (`kvm_core::leds`).
//!
//! It also decides when a new firmware image, started on trial, keeps itself ([`update::buy`]): as soon as Hub-0 is
//! up and one link answers, which shows that the USB host and the links work. It doesn't wait for the rest, which
//! may take longer than the boot ROM's 16.7 s trial watchdog (reprogramming an AT32F405, a PC that is off).

use core::sync::atomic::Ordering;

use defmt::{info, warn};
use embassy_time::{Duration, Instant, Timer};
use kvm_proto::ctrl::{Pc, selftest};
use kvm_proto::link::msg::UsbState;

use crate::link::{self, FwCheck};
use crate::uartlog::uart_log;
use crate::{bt, host, kvm, power, update};

/// The result is in by then, whatever is still missing.
const DEADLINE: Duration = Duration::from_secs(12);
/// A powered PC's hub resets the AT32F405 within a second or two of its descriptor set arriving.
const HUB_GRACE: Duration = Duration::from_secs(8);
const POLL: Duration = Duration::from_millis(50);

/// Whether PC `k`'s hub has failed to reset its AT32F405 although the PC powers it. Unknown (false) without a link.
fn hub_missing(k: usize) -> bool {
    kvm::with(|e| e.pc_usb_status(Pc::from_index(k))).is_some_and(|s| {
        s.vbus_mv >= 4000 && s.resets == 0 && matches!(s.usb_state, UsbState::Powered | UsbState::Detached)
    })
}

/// What PC `k`'s AT32F405 firmware check and its `Hello` say: programming failed this boot, gave up, no crystal.
fn fw_bits(k: usize) -> u32 {
    let mut bits = match link::fw_check(k) {
        FwCheck::Failed => [selftest::AT32_FW1, selftest::AT32_FW2][k],
        FwCheck::GaveUp => [selftest::AT32_GAVE_UP1, selftest::AT32_GAVE_UP2][k],
        _ => 0,
    };
    if link::NO_CRYSTAL[k].load(Ordering::Relaxed) {
        bits |= [selftest::AT32_XTAL1, selftest::AT32_XTAL2][k];
    }
    bits
}

/// The Bluetooth module (when fitted): it doesn't run the embedded firmware this boot, or programming gave up.
fn bt_bits() -> u32 {
    match bt::state() {
        bt::State::Failed => selftest::BT_FW,
        bt::State::GaveUp => selftest::BT_GAVE_UP,
        _ => 0,
    }
}

fn findings(started: Instant) -> (u32, bool) {
    let mut bits = 0;
    let mut settled = true;
    if !host::HUB0_UP.load(Ordering::Relaxed) {
        bits |= selftest::HUB0;
    }
    for k in 0..2 {
        let check = link::fw_check(k);
        if check == FwCheck::Pending {
            settled = false;
        }
        bits |= fw_bits(k);
        if !link::LINK_UP[k].load(Ordering::Relaxed) && check != FwCheck::Skipped {
            bits |= [selftest::LINK1, selftest::LINK2][k];
        }
        if started.elapsed() >= HUB_GRACE && hub_missing(k) {
            bits |= [selftest::HUB1, selftest::HUB2][k];
        }
    }
    bits |= bt_bits();
    // A module still starting or being programmed (up to ~20 s) is judged later, after the deadline if need be.
    if matches!(bt::state(), bt::State::Unknown | bt::State::Starting | bt::State::Programming) {
        settled = false;
    }
    let ports = power::PORT_SELFTEST.load(Ordering::Relaxed);
    for (k, bit) in selftest::PORTS.iter().enumerate() {
        if ports & (1 << k) != 0 {
            bits |= bit;
        }
    }
    match power::POWER_SELFTEST.load(Ordering::Relaxed) {
        0 => settled = false,
        2 => bits |= selftest::POWER,
        _ => {}
    }
    settled &= started.elapsed() >= HUB_GRACE;
    (bits, settled)
}

#[embassy_executor::task]
pub async fn run() -> ! {
    let started = Instant::now();
    let mut bought = false;
    let bits = loop {
        Timer::after(POLL).await;
        let (bits, settled) = findings(started);
        let link_seen = link::LINK_SEEN.iter().any(|l| l.load(Ordering::Relaxed));
        if !bought && host::HUB0_UP.load(Ordering::Relaxed) && link_seen {
            bought = true;
            update::buy();
        }
        // A failure may still turn good (a link that comes up late): only the deadline ends a failed test.
        if (settled && bits == 0) || started.elapsed() >= DEADLINE {
            break bits;
        }
        kvm::with(|e| e.set_selftest(bits | selftest::RUNNING));
    };
    kvm::with(|e| e.set_selftest(bits));
    if bits == 0 {
        info!("self-test passed");
    } else {
        warn!("self-test failed: {=u32:#08x}", bits);
    }
    uart_log!("self-test {} ({:#08x})", if bits == 0 { "passed" } else { "FAILED" }, bits);
    // Hub-1/2 can only be judged once a PC powers them, and reprogramming an AT32F405 or the Bluetooth module may
    // outlast the deadline: keep watching, and update the bits (and with them the red key, `selftest::ON_KEY`) when
    // things change.
    let mut bits = bits;
    loop {
        Timer::after_secs(1).await;
        for k in 0..2 {
            let bit = [selftest::HUB1, selftest::HUB2][k];
            let link_bit = [selftest::LINK1, selftest::LINK2][k];
            if hub_missing(k) {
                bits |= bit;
            } else if kvm::with(|e| e.pc_usb_status(Pc::from_index(k))).is_some_and(|s| s.resets > 0) {
                bits &= !bit;
            }
            let check = link::fw_check(k);
            if link::LINK_UP[k].load(Ordering::Relaxed) {
                bits &= !link_bit;
            } else if !matches!(check, FwCheck::Skipped | FwCheck::Pending) {
                bits |= link_bit;
            }
            let fw = [selftest::AT32_FW1 | selftest::AT32_GAVE_UP1 | selftest::AT32_XTAL1, selftest::AT32_FW2
                | selftest::AT32_GAVE_UP2
                | selftest::AT32_XTAL2][k];
            bits = (bits & !fw) | fw_bits(k);
        }
        bits = (bits & !selftest::BT) | bt_bits();
        if host::HUB0_UP.load(Ordering::Relaxed) {
            bits &= !selftest::HUB0;
        }
        if kvm::with(|e| e.selftest()) != bits {
            kvm::with(|e| e.set_selftest(bits));
        }
    }
}
