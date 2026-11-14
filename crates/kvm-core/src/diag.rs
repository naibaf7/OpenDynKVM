//! Diagnostics (software.md §8.4): what only the RP2354B's firmware can measure ([`BoardDiag`]), and how each reading
//! is graded. The engine puts the items together (`Engine::diag_items`) from these and from what it knows itself.
//!
//! The limits, and where they come from:
//! - **Temperatures:** the RP2354B is specified to 85 °C ambient; the AT32F405's sensor is off by up to 20 °C from
//!   chip to chip (DS v2.03 Table 54), so it only warns late.
//! - **Clocks:** measured by the RP2354B's frequency counter against its 12 MHz crystal (datasheet §8.1.4, 2 kHz
//!   resolution). USB full speed allows ±0.25 % (USB 2.0 §7.1.11). RP2350-E12: `clk_sys` at least 10 % above `clk_usb`.
//! - **VBUS:** a source gives 4.75–5.25 V; USB 2.0 allows 4.40 V at the far end of a cable (§7.2.1).
//! - **CC:** 5.1 kΩ Rd against a source's Rp shows at most 2.04 V; more means 5 V on CC (bad cable, bent contact).
//!   Below 0.2 V with VBUS present, the source has no Rp: an old USB-A to C cable without its 56 kΩ.
//! - **3.3 V:** measured by each AT32F405 against its 1.20 V ± 3.3 % reference (DS Table 53), so ±5 % shows as good.

use kvm_proto::ctrl::diag::{BtModule, ClockSource, FwUpdates, Grade, ResetCause};
use kvm_proto::ctrl::{LinkState, Speed, selftest};

/// What the RP2354B's firmware measures for the diagnostics (`Engine::set_board_diag`), every 2 s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BoardDiag {
    pub reset: ResetCause,
    /// Die temperature in tenths of a °C.
    pub temp_dc: Option<i16>,
    /// The crystal's frequency while it runs (XOSC `STATUS.STABLE`), else 0.
    pub xosc_hz: u32,
    /// Measured by the frequency counter; 0 = not measured.
    pub pll_sys_hz: u32,
    pub pll_usb_hz: u32,
    pub clk_sys_hz: u32,
    pub clk_usb_hz: u32,
    pub clk_peri_hz: u32,
    pub clk_adc_hz: u32,
    pub rosc_hz: u32,
    /// What `clk_sys` (and the system PLL) is set up for.
    pub clk_sys_set_hz: u32,
    /// The RP2354B's root port: what Hub-0 attached as (`Unknown`: nothing attached).
    pub hub0_speed: Speed,
    pub hub0_up: bool,
    /// Hub-0 is held in reset on purpose (deep standby).
    pub hub0_held: bool,
    /// The USB PHY swaps D+/D− (errata E1), and whether the firmware found that out at run time.
    pub phy_swap: bool,
    pub phy_swap_found: bool,
    /// The PWR port's CC, the higher of CC1 and CC2.
    pub pwr_cc_mv: u16,
    /// The UART exchanges with the Bluetooth module (boards 0.16.0 on) that failed since start.
    pub bt_link_errors: u32,
    /// What the boot-time check of the partition table found: low byte [`FwUpdates`], the bits above what it read.
    pub fw_updates: i32,
    /// Hub-0 status changes the hub class couldn't handle, since start.
    pub hub0_errors: u32,
    /// Front-port enumerations or probes that failed, since start.
    pub enum_fails: u32,
    /// The module as a board on a front port, since start (all 0 until one was seen).
    pub board: BoardModule,
}

/// What the firmware counts of the module as a board on a front port (software.md §4.13), for bring-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct BoardModule {
    pub sessions: u32,
    /// A hexadecimal digit per step, the newest last.
    pub steps: u32,
    pub rom_error: u32,
    pub bulk_in: u32,
    pub bulk_out: u32,
    pub naks: u32,
    pub out_naks: u32,
    pub errors: u32,
    pub last_error: u32,
}

pub const USB_HZ: u32 = 48_000_000;

impl BoardDiag {
    /// A healthy 0.12 board at room temperature, on PC power, as the simulator shows it.
    pub const TYPICAL: BoardDiag = BoardDiag {
        reset: ResetCause::PowerOn,
        temp_dc: Some(412),
        xosc_hz: 12_000_000,
        pll_sys_hz: 150_000_000,
        pll_usb_hz: 48_000_000,
        clk_sys_hz: 150_000_000,
        clk_usb_hz: 48_000_000,
        clk_peri_hz: 150_000_000,
        clk_adc_hz: 48_000_000,
        rosc_hz: 11_264_000,
        clk_sys_set_hz: 150_000_000,
        hub0_speed: Speed::Full,
        hub0_up: true,
        hub0_held: false,
        phy_swap: true,
        phy_swap_found: false,
        pwr_cc_mv: 0,
        bt_link_errors: 0,
        fw_updates: FwUpdates::PartitionA as i32,
        hub0_errors: 0,
        enum_fails: 0,
        board: BoardModule {
            sessions: 0,
            steps: 0,
            rom_error: 0,
            bulk_in: 0,
            bulk_out: 0,
            naks: 0,
            out_naks: 0,
            errors: 0,
            last_error: 0,
        },
    };
}

/// Within `ppm` of `want`; 0 Hz (stopped, or not measured) is bad.
pub fn freq(hz: u32, want: u32, ppm: u32) -> Grade {
    let tol = (want as u64 * ppm as u64 / 1_000_000) as u32;
    if hz == 0 || hz.abs_diff(want) > tol { Grade::Bad } else { Grade::Ok }
}

/// `clk_sys` must run at least 10 % faster than `clk_usb` (RP2350-E12).
pub fn sys_vs_usb(sys_hz: u32, usb_hz: u32) -> Grade {
    if (sys_hz as u64) * 10 < (usb_hz as u64) * 11 { Grade::Bad } else { Grade::Ok }
}

/// Die temperature in tenths of a °C: good below `warn_c`, bad from `bad_c`.
pub fn temp(dc: i16, warn_c: i16, bad_c: i16) -> Grade {
    match dc / 10 {
        t if t >= bad_c => Grade::Bad,
        t if t >= warn_c => Grade::Warn,
        _ => Grade::Ok,
    }
}

/// VBUS of a port: under 0.8 V nothing supplies it, which is no fault.
pub fn vbus(mv: u16) -> Grade {
    match mv {
        0..800 => Grade::Info,
        4750..=5500 => Grade::Ok,
        4400..4750 | 5501..=5700 => Grade::Warn,
        _ => Grade::Bad,
    }
}

/// A USB-C port's CC (the higher of CC1 and CC2) while `vbus_mv` is there.
pub fn cc(cc_mv: u16, vbus_mv: u16) -> Grade {
    if vbus_mv < 800 {
        return if cc_mv > 2300 { Grade::Bad } else { Grade::Info };
    }
    match cc_mv {
        0..200 => Grade::Warn,
        200..=2300 => Grade::Ok,
        _ => Grade::Bad,
    }
}

/// Firmware updates over USB: on is good; off without a partition table is a board flashed by a debug probe, as
/// intended; off for any other reason is a fault to report.
pub fn fw_updates(v: i32) -> Grade {
    match FwUpdates::from_u8(v as u8) {
        Some(FwUpdates::PartitionA | FwUpdates::PartitionB) => Grade::Ok,
        Some(FwUpdates::NoTable) => Grade::Info,
        _ => Grade::Warn,
    }
}

/// The 3.3 V rail.
pub fn rail_3v3(mv: u16) -> Grade {
    match mv {
        3135..=3465 => Grade::Ok,
        3000..3135 | 3466..=3600 => Grade::Warn,
        _ => Grade::Bad,
    }
}

pub fn reset(c: ResetCause) -> Grade {
    match c {
        ResetCause::PowerOn => Grade::Ok,
        ResetCause::BrownOut | ResetCause::Glitch => Grade::Warn,
        // Restarts from software, an update (the boot ROM's watchdog), the RUN pin or a debugger: on purpose.
        ResetCause::Software | ResetCause::Watchdog | ResetCause::RunPin | ResetCause::Debugger => Grade::Info,
    }
}

pub fn selftest_bits(bits: u32) -> Grade {
    let found = bits & !selftest::RUNNING;
    if found & selftest::ON_KEY != 0 {
        Grade::Bad
    } else if found != 0 {
        Grade::Warn
    } else if bits & selftest::RUNNING != 0 {
        Grade::Info
    } else {
        Grade::Ok
    }
}

pub fn link(l: LinkState) -> Grade {
    match l {
        LinkState::Connected => Grade::Ok,
        // The PC sleeps, or is off.
        LinkState::Suspended | LinkState::Unpowered => Grade::Info,
        LinkState::Detached | LinkState::Enumerating => Grade::Warn,
        LinkState::LinkDown => Grade::Bad,
    }
}

/// The USB speed between a PC and its hub, while the PC uses the link: high speed is what the KVM is built for (125 µs
/// polling); full speed works, but means a USB 1.1 port or a poor cable.
pub fn pc_speed(s: Speed, l: LinkState) -> Grade {
    if !matches!(l, LinkState::Connected | LinkState::Suspended) {
        return Grade::Info;
    }
    match s {
        Speed::High => Grade::Ok,
        Speed::Full | Speed::Low => Grade::Warn,
        Speed::Unknown => Grade::Info,
    }
}

/// Hub-0 on the RP2354B's root port is a full-speed hub. As low speed, its D+ pull-up arrives on D− (errata E1).
pub fn hub0_speed(s: Speed, held: bool) -> Grade {
    match s {
        Speed::Full | Speed::High => Grade::Ok,
        Speed::Low => Grade::Bad,
        Speed::Unknown if held => Grade::Info,
        Speed::Unknown => Grade::Bad,
    }
}

pub fn clock_source(c: ClockSource) -> Grade {
    match c {
        ClockSource::Crystal => Grade::Ok,
        // Without its crystal the AT32F405 has no USB (no HS PHY clock).
        ClockSource::Internal => Grade::Bad,
        ClockSource::Unknown => Grade::Info,
    }
}

pub fn bt_module(m: BtModule) -> Grade {
    match m {
        BtModule::Up => Grade::Ok,
        // Programming takes about 10 s and happens after a firmware update.
        BtModule::Programming => Grade::Warn,
        BtModule::Failed | BtModule::GaveUp => Grade::Bad,
        BtModule::Absent | BtModule::Starting => Grade::Info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits() {
        assert_eq!(freq(48_000_000, USB_HZ, 2500), Grade::Ok);
        assert_eq!(freq(48_100_000, USB_HZ, 2500), Grade::Ok, "2083 ppm");
        assert_eq!(freq(48_200_000, USB_HZ, 2500), Grade::Bad, "4167 ppm");
        assert_eq!(freq(0, USB_HZ, 2500), Grade::Bad);
        assert_eq!(sys_vs_usb(150_000_000, USB_HZ), Grade::Ok);
        assert_eq!(sys_vs_usb(48_000_000, USB_HZ), Grade::Bad);
        assert_eq!(temp(455, 70, 85), Grade::Ok);
        assert_eq!(temp(-100, 70, 85), Grade::Ok);
        assert_eq!(temp(712, 70, 85), Grade::Warn);
        assert_eq!(temp(850, 70, 85), Grade::Bad);
        assert_eq!(vbus(0), Grade::Info);
        assert_eq!(vbus(5050), Grade::Ok);
        assert_eq!(vbus(4600), Grade::Warn);
        assert_eq!(vbus(3900), Grade::Bad);
        assert_eq!(vbus(6000), Grade::Bad);
        assert_eq!(cc(420, 5000), Grade::Ok);
        assert_eq!(cc(0, 5000), Grade::Warn);
        assert_eq!(cc(4800, 5000), Grade::Bad);
        assert_eq!(cc(0, 0), Grade::Info);
        assert_eq!(rail_3v3(3290), Grade::Ok);
        assert_eq!(rail_3v3(3050), Grade::Warn);
        assert_eq!(rail_3v3(2800), Grade::Bad);
        assert_eq!(selftest_bits(0), Grade::Ok);
        assert_eq!(selftest_bits(selftest::RUNNING), Grade::Info);
        assert_eq!(selftest_bits(selftest::PORT_MS), Grade::Warn);
        assert_eq!(selftest_bits(selftest::HUB0 | selftest::RUNNING), Grade::Bad);
        assert_eq!(pc_speed(Speed::Full, LinkState::Connected), Grade::Warn);
        assert_eq!(pc_speed(Speed::Full, LinkState::Unpowered), Grade::Info);
        assert_eq!(hub0_speed(Speed::Low, false), Grade::Bad);
        assert_eq!(hub0_speed(Speed::Unknown, true), Grade::Info);
    }
}
