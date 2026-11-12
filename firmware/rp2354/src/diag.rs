//! The RP2354B's own measurements for the diagnostics (software.md §8.4), handed to the engine every 2 s:
//! - its clocks, counted by the frequency counter against the 12 MHz crystal (datasheet §8.1.4);
//! - its die temperature and the PWR port's CC (read by [`crate::power::monitor`]);
//! - why it last reset (POWMAN `CHIP_RESET`, read once at boot);
//! - Hub-0 on the USB root port, and the D+/D− swap of errata E1 ([`crate::usbh`]);
//! - the UART errors on the Bluetooth module's link ([`crate::bt`]; its state goes to the engine directly).

use core::sync::atomic::{AtomicBool, AtomicI16, AtomicU8, AtomicU16, Ordering};

use embassy_rp::pac;
use embassy_rp::pac::clocks::vals::Fc0src;
use embassy_time::Timer;
use kvm_core::diag::BoardDiag;
use kvm_proto::ctrl::Speed;
use kvm_proto::ctrl::diag::ResetCause;

use crate::{bt, host, kvm, power, update};

/// Tenths of a °C; `i16::MIN` until the first reading.
pub static TEMP_DC: AtomicI16 = AtomicI16::new(i16::MIN);
/// The PWR port's CC, the higher of CC1 and CC2.
pub static PWR_CC_MV: AtomicU16 = AtomicU16::new(0);
/// What Hub-0 attached as on the root port (`ctrl::Speed`; 0 = nothing attached).
pub static ROOT_SPEED: AtomicU8 = AtomicU8::new(0);
/// The USB PHY swaps D+/D− (errata E1), and the root port found that out at run time.
pub static PHY_SWAP: AtomicBool = AtomicBool::new(false);
pub static PHY_SWAP_FOUND: AtomicBool = AtomicBool::new(false);
static RESET: AtomicU8 = AtomicU8::new(ResetCause::Software as u8);

const PERIOD_S: u64 = 2;
/// The frequency counter's test interval: 2^10 µs ≈ 1 ms, resolving 2 kHz (datasheet Table 574).
const FC0_INTERVAL: u8 = 10;

/// Why the chip last started. Called once, early at boot.
pub fn note_reset_cause() {
    let r = pac::POWMAN.chip_reset().read();
    let cause = if r.had_glitch_detect() {
        ResetCause::Glitch
    } else if r.had_bor() {
        ResetCause::BrownOut
    } else if r.had_por() {
        ResetCause::PowerOn
    } else if r.had_run_low() {
        ResetCause::RunPin
    } else if r.had_watchdog_reset_powman()
        || r.had_watchdog_reset_powman_async()
        || r.had_watchdog_reset_swcore()
        || r.had_watchdog_reset_rsm()
    {
        ResetCause::Watchdog
    } else if r.had_dp_reset_req() || r.had_rescue() || r.had_hzd_sys_reset_req() {
        ResetCause::Debugger
    } else {
        ResetCause::Software
    };
    RESET.store(cause as u8, Ordering::Relaxed);
}

/// Counts `src` for about 1 ms against `clk_ref` (the crystal); Hz, or 0 if the counter doesn't finish.
async fn count(src: Fc0src) -> u32 {
    let c = pac::CLOCKS;
    for _ in 0..20 {
        if !c.fc0_status().read().running() {
            break;
        }
        Timer::after_micros(250).await;
    }
    c.fc0_ref_khz().write(|w| w.set_fc0_ref_khz(embassy_rp::clocks::clk_ref_freq() / 1000));
    c.fc0_interval().write(|w| w.set_fc0_interval(FC0_INTERVAL));
    c.fc0_min_khz().write(|w| w.set_fc0_min_khz(0));
    c.fc0_max_khz().write(|w| w.set_fc0_max_khz(0x01FF_FFFF));
    // Writing the source starts the count.
    c.fc0_src().write(|w| w.set_fc0_src(src));
    for _ in 0..20 {
        Timer::after_micros(250).await;
        let s = c.fc0_status().read();
        if s.done() && !s.running() {
            let r = c.fc0_result().read();
            return r.khz() * 1000 + r.frac() as u32 * 1000 / 32;
        }
    }
    0
}

#[embassy_executor::task]
pub async fn run() -> ! {
    loop {
        let xosc = pac::XOSC.status().read();
        let temp = TEMP_DC.load(Ordering::Relaxed);
        let d = BoardDiag {
            reset: ResetCause::from_u8(RESET.load(Ordering::Relaxed)).unwrap_or(ResetCause::Software),
            temp_dc: (temp != i16::MIN).then_some(temp),
            xosc_hz: if xosc.enabled() && xosc.stable() { embassy_rp::clocks::xosc_freq() } else { 0 },
            pll_sys_hz: count(Fc0src::PLL_SYS_CLKSRC_PRIMARY).await,
            pll_usb_hz: count(Fc0src::PLL_USB_CLKSRC_PRIMARY).await,
            clk_sys_hz: count(Fc0src::CLK_SYS).await,
            clk_usb_hz: count(Fc0src::CLK_USB).await,
            clk_peri_hz: count(Fc0src::CLK_PERI).await,
            clk_adc_hz: count(Fc0src::CLK_ADC).await,
            rosc_hz: count(Fc0src::ROSC_CLKSRC).await,
            clk_sys_set_hz: embassy_rp::clocks::clk_sys_freq(),
            hub0_speed: Speed::from_u8(ROOT_SPEED.load(Ordering::Relaxed)).unwrap_or(Speed::Unknown),
            hub0_up: host::HUB0_UP.load(Ordering::Relaxed),
            hub0_held: power::HUB0_HELD.load(Ordering::Relaxed),
            phy_swap: PHY_SWAP.load(Ordering::Relaxed),
            phy_swap_found: PHY_SWAP_FOUND.load(Ordering::Relaxed),
            pwr_cc_mv: PWR_CC_MV.load(Ordering::Relaxed),
            bt_link_errors: bt::LINK_ERRORS.load(Ordering::Relaxed),
            fw_updates: update::FOUND.load(Ordering::Relaxed),
            hub0_errors: host::HUB0_ERRORS.load(Ordering::Relaxed),
            enum_fails: host::ENUM_FAILS.load(Ordering::Relaxed),
        };
        kvm::with(|e| e.set_board_diag(d));
        Timer::after_secs(PERIOD_S).await;
    }
}
