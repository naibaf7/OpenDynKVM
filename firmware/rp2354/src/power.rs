//! Power source, the PWR port's advertised current, the front ports' VBUS switches (software.md §4.7) and the
//! standby plan (§4.12).

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use defmt::info;
use embassy_futures::select::{Either, Either4, select, select4};
use embassy_rp::adc::{Adc, Async, Channel};
use embassy_rp::gpio::{Input, Level};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Ticker, Timer};
use kvm_core::engine::PowerInfo;
use kvm_core::standby::PowerPlan;
use kvm_proto::ctrl::{Advertised, Port, PowerSource};

use crate::kvm;
use crate::uartlog::uart_log;

const PERIOD: Duration = Duration::from_millis(200);
/// A port that faulted stays off this long before it is tried again.
const FAULT_BACKOFF: Duration = Duration::from_millis(1000);
/// VBUS off time for a deliberate power cycle (passthrough switch, re-detect).
const CYCLE_OFF: Duration = Duration::from_millis(1000);
/// A data-line replug: long enough for Hub-0 to see the device leave (a connect takes its 100 ms debounce).
const REPLUG_OFF: Duration = Duration::from_millis(250);

/// Self-test findings, filled in during the first second after boot.
/// Bit k: port k's power switch faulted when first switched on, or its FAULT line was low while it was off.
pub static PORT_SELFTEST: AtomicU8 = AtomicU8::new(0);
/// PWR-port readings: 0 not yet judged, 1 plausible, 2 implausible.
pub static POWER_SELFTEST: AtomicU8 = AtomicU8::new(0);
/// FAULT must stay high this long after the first switch-on (TPS2553 deglitch ≈ 7.5 ms, plus inrush).
const FIRST_ON_WATCH: Duration = Duration::from_millis(100);

/// Whether each front port may have VBUS under the standby plan; [`port_power`] follows it.
static PORT_ALLOWED: [AtomicBool; 3] = [const { AtomicBool::new(true) }; 3];
static PLAN_CHANGED: [Signal<CriticalSectionRawMutex, ()>; 3] = [const { Signal::new() }; 3];
/// Hub-0 is held in reset for deep standby: the hub task's recovery must not let it go.
pub static HUB0_HELD: AtomicBool = AtomicBool::new(false);

/// Carries out the engine's power plan (`Output::Power`): front-port VBUS through the port tasks, Hub-0's reset here.
/// The LEDs follow the engine's `led_inputs` by themselves.
pub fn apply(plan: PowerPlan) {
    for (i, allowed) in plan.ports.into_iter().enumerate() {
        PORT_ALLOWED[i].store(allowed, Ordering::Relaxed);
        PLAN_CHANGED[i].signal(());
    }
    HUB0_HELD.store(!plan.hub0, Ordering::Relaxed);
    kvm::with_board(|b| b.hub0_rst.set_level(Level::from(plan.hub0)));
    info!("power plan: {}", plan);
    uart_log!("power: {:?}, front ports {:?}, Hub-0 {}", plan.mode, plan.ports, if plan.hub0 { "on" } else { "off" });
}

/// Whether the PWR port's readings make sense together. A USB-C source pulls CC to at most 2.04 V through our
/// 5.1 kΩ Rd (Rp 10 kΩ to 5 V: 1.69 V); much more means 5 V on CC (bent contact, bad cable) or a missing Rd.
fn power_plausible(pwr_st: bool, vbus_mv: u16, cc_mv: u16) -> bool {
    let vbus_ok = if pwr_st { (4400..=5600).contains(&vbus_mv) } else { vbus_mv < 4800 };
    vbus_ok && cc_mv <= 2300
}

fn mv(raw: u16) -> u32 {
    raw as u32 * 3300 / 4095
}

pub struct Inputs {
    pub adc: Adc<'static, Async>,
    pub vsns: Channel<'static>,
    pub cc1: Channel<'static>,
    pub cc2: Channel<'static>,
    /// The die's temperature sensor (for the diagnostics).
    pub temp: Channel<'static>,
    pub pwr_st: Input<'static>,
}

/// The temperature sensor's reading in tenths of a °C: 0.706 V at 27 °C, falling 1.721 mV/°C (datasheet §12.4.6).
fn temp_dc(raw: u16) -> i16 {
    let tenth_mv = raw as i32 * 33_000 / 4095;
    (270 - (tenth_mv - 7_060) * 1_000 / 1_721) as i16
}

#[embassy_executor::task]
pub async fn monitor(mut i: Inputs) -> ! {
    let mut ticker = Ticker::every(PERIOD);
    let mut readings = 0u8;
    loop {
        ticker.next().await;
        // VSNS_PWR = VBUS × 47k / (100k + 47k).
        let vbus = i.adc.read(&mut i.vsns).await.map_or(0, |r| mv(r) * 147 / 47) as u16;
        let cc1 = i.adc.read(&mut i.cc1).await.map_or(0, mv) as u16;
        let cc2 = i.adc.read(&mut i.cc2).await.map_or(0, mv) as u16;
        if let Ok(raw) = i.adc.read(&mut i.temp).await {
            crate::diag::TEMP_DC.store(temp_dc(raw), Ordering::Relaxed);
        }
        crate::diag::PWR_CC_MV.store(cc1.max(cc2), Ordering::Relaxed);
        let source = if i.pwr_st.is_high() && vbus > 4000 { PowerSource::PwrPort } else { PowerSource::PcBus };
        let adv = if vbus > 4000 { Advertised::from_cc_mv(cc1.max(cc2)) } else { Advertised::None };
        kvm::with(|e| e.set_power(PowerInfo { source, adv_pwr: adv, vbus_pwr_mv: vbus }));
        // Judge the fifth reading (1 s in): the power path and the PWR adapter have settled by then.
        readings = readings.saturating_add(1);
        if readings == 5 {
            let ok = power_plausible(i.pwr_st.is_high(), vbus, cc1.max(cc2));
            POWER_SELFTEST.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
        }
    }
}

/// Watches one port's TPS2553 FAULT (active low) and runs power cycles the engine asks for.
#[embassy_executor::task(pool_size = 3)]
pub async fn port_power(index: usize, mut fault: Input<'static>) -> ! {
    let port = Port::from_index(index).unwrap_or(Port::Other);
    // Self-test: FAULT is released while the switch is off, and stays released through the first switch-on.
    Timer::after_millis(1).await;
    let mut bad = fault.is_low();
    kvm::with_board(|b| b.ports[index].power(true));
    if !bad {
        bad = matches!(select(fault.wait_for_low(), Timer::after(FIRST_ON_WATCH)).await, Either::First(()));
    }
    if bad {
        PORT_SELFTEST.fetch_or(1 << index, Ordering::Relaxed);
        kvm::with(|e| e.port_fault(port));
        kvm::with_board(|b| b.ports[index].power(false));
        Timer::after(FAULT_BACKOFF).await;
        kvm::with_board(|b| b.ports[index].power(true));
        Timer::after_millis(20).await;
    }
    loop {
        let allowed = || PORT_ALLOWED[index].load(Ordering::Relaxed);
        if !allowed() {
            // Standby: off until the plan allows the port again; the device detaches and is probed anew after.
            kvm::with_board(|b| b.ports[index].power(false));
            while !allowed() {
                PLAN_CHANGED[index].wait().await;
            }
        } else {
            match select4(
                fault.wait_for_low(),
                kvm::POWER_CYCLE[index].wait(),
                PLAN_CHANGED[index].wait(),
                kvm::REPLUG[index].wait(),
            )
            .await
            {
                Either4::First(()) => {
                    kvm::with(|e| e.port_fault(port));
                    kvm::with_board(|b| b.ports[index].power(false));
                    Timer::after(FAULT_BACKOFF).await;
                }
                Either4::Second(()) => {
                    kvm::with_board(|b| b.ports[index].power(false));
                    Timer::after(CYCLE_OFF).await;
                }
                Either4::Third(()) => continue,
                Either4::Fourth(()) => {
                    // VBUS stays on; the router goes off and back to Hub-0, unless the engine routed the port
                    // elsewhere meanwhile.
                    let generation = kvm::ROUTE_GEN[index].load(Ordering::Relaxed);
                    kvm::with_board(|b| b.ports[index].route(kvm_core::Route::Off));
                    Timer::after(REPLUG_OFF).await;
                    if kvm::ROUTE_GEN[index].load(Ordering::Relaxed) == generation {
                        kvm::with_board(|b| b.ports[index].route(kvm_core::Route::Kvm));
                    }
                    continue;
                }
            }
            if !allowed() {
                continue;
            }
        }
        kvm::with_board(|b| b.ports[index].power(true));
        // Let FAULT settle after inrush before watching it again.
        Timer::after_millis(20).await;
    }
}
