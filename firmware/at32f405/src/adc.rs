//! ADC1 readings of this PC's USB-C port: VBUS through a 2:1 divider (PA0, IN0), CC1 (PA1, IN1), CC2 (PB0, IN8); and
//! for the diagnostics the chip's own temperature sensor (IN16) and 1.20 V reference VINTRV (IN17), which give VDDA:
//! the board's 3.3 V (DS v2.03 §2.15, Tables 53–54).

use at32f4xx_pac::at32f405 as pac;

pub const CH_VDET: u8 = 0;
pub const CH_CC1: u8 = 1;
pub const CH_CC2: u8 = 8;
const CH_TEMP: u8 = 16;
const CH_VINTRV: u8 = 17;
const VDDA_MV: u32 = 3300;
/// VINTRV, typical (1.16–1.24 V).
const VINTRV_MV: u32 = 1200;
/// The temperature sensor: 490 mV at 25 °C, rising 1.69 mV/°C (the offset varies by up to 20 °C between chips).
const TS_V25_UV: i32 = 490_000;
const TS_UV_PER_DECI_C: i32 = 169;
/// ADCCLK = HCLK / 6 = 24 MHz (max 28 MHz; ADCDIV n divides by n + 2, RM v2.02 §19.5.16).
const ADCDIV_HCLK_6: u8 = 4;
/// Longest sample time (CC lines are high-impedance through 5.1 kΩ).
const SAMPLE_TIME_MAX: u32 = 0b111;
/// Ordinary-channel trigger: software (`ADC12_ORDINARY_TRIG_SOFTWARE` in Artery's library). On this ADC `OCSWTRG`
/// starts a conversion only while the trigger is enabled (`OCTEN`) and set to software, as Artery's
/// `adc_ordinary_conversion_trigger_set(.., ADC12_ORDINARY_TRIG_SOFTWARE, TRUE)` does. Without them no conversion
/// ever ran: every reading timed out as 0, and with VBUS "at 0 V" the PC hub went into reset a second after start-up
/// (found on the first board, 2026-11-11).
const OCTESEL_SOFTWARE: u8 = 0b111;

fn adc() -> &'static pac::adc1::RegisterBlock {
    unsafe { &*pac::ADC1::ptr() }
}

pub fn init() {
    let crm = unsafe { &*pac::CRM::ptr() };
    crm.apb2en().modify(|_, w| w.adc1().set_bit());
    let com = unsafe { &*pac::ADCCOM::ptr() };
    com.cctrl().modify(|_, w| unsafe { w.adcdiv().bits(ADCDIV_HCLK_6) });
    let a = adc();
    let spt = (SAMPLE_TIME_MAX << (3 * CH_VDET as u32))
        | (SAMPLE_TIME_MAX << (3 * CH_CC1 as u32))
        | (SAMPLE_TIME_MAX << (3 * CH_CC2 as u32));
    a.spt2().write(|w| unsafe { w.bits(spt) });
    // IN16 and IN17 need at least 5.1 µs of sampling (DS Tables 53–54): the longest is 10 µs at 24 MHz.
    let spt1 = (SAMPLE_TIME_MAX << (3 * (CH_TEMP as u32 - 10))) | (SAMPLE_TIME_MAX << (3 * (CH_VINTRV as u32 - 10)));
    a.spt1().write(|w| unsafe { w.bits(spt1) });
    a.ctrl2().modify(|_, w| unsafe {
        w.adcen().set_bit().itsrven().set_bit().octesel().bits(OCTESEL_SOFTWARE).octen().set_bit()
    });
    cortex_m::asm::delay(crate::clocks::SYSCLK_HZ / 50_000); // ≥ 20 µs power-up
    a.ctrl2().modify(|_, w| w.adcalinit().set_bit());
    while a.ctrl2().read().adcalinit().bit_is_set() {}
    a.ctrl2().modify(|_, w| w.adcal().set_bit());
    while a.ctrl2().read().adcal().bit_is_set() {}
}

/// One conversion of `channel`, in mV at the pin (taking VDDA as 3.3 V); `None` if the ADC didn't convert.
pub fn read_mv(channel: u8) -> Option<u16> {
    read_raw(channel).map(|r| (r as u32 * VDDA_MV / 4095) as u16)
}

/// One conversion of `channel`: 0–4095 of VDDA; `None` if it doesn't finish.
fn read_raw(channel: u8) -> Option<u16> {
    let a = adc();
    a.osq3().write(|w| unsafe { w.osn1().bits(channel) });
    a.osq1().modify(|_, w| unsafe { w.oclen().bits(0) });
    a.sts().modify(|_, w| w.occe().clear_bit());
    a.ctrl2().modify(|_, w| w.ocswtrg().set_bit());
    let mut spins = 0u32;
    while a.sts().read().occe().bit_is_clear() {
        spins += 1;
        if spins > 100_000 {
            return None;
        }
    }
    Some((a.odt().read().bits() & 0xFFF) as u16)
}

/// VDDA (the board's 3.3 V) in mV and the die temperature in tenths of a °C, for the diagnostics.
pub fn read_internal() -> (Option<u16>, Option<i16>) {
    let vintrv = match read_raw(CH_VINTRV) {
        Some(v) if v > 0 => v as u32,
        _ => return (None, None),
    };
    let vdda_mv = VINTRV_MV * 4095 / vintrv;
    let Some(ts) = read_raw(CH_TEMP) else {
        return (Some(vdda_mv.min(u16::MAX as u32) as u16), None);
    };
    let ts_uv = (ts as u32 * vdda_mv * 1000 / 4095) as i32;
    let temp_dc = 250 + (ts_uv - TS_V25_UV) / TS_UV_PER_DECI_C;
    (Some(vdda_mv.min(u16::MAX as u32) as u16), Some(temp_dc.clamp(i16::MIN as i32 + 1, i16::MAX as i32) as i16))
}

pub struct Port {
    /// `None`: no reading, which says nothing about the PC's 5 V.
    pub vbus_mv: Option<u16>,
    pub cc1_mv: u16,
    pub cc2_mv: u16,
}

impl Port {
    /// The PC's 5 V is measured to be missing (not merely unread).
    pub fn no_vbus(&self) -> bool {
        self.vbus_mv.is_some_and(|v| v < 4000)
    }
}

pub fn read_port() -> Port {
    Port {
        vbus_mv: read_mv(CH_VDET).map(|v| v.saturating_mul(2)),
        cc1_mv: read_mv(CH_CC1).unwrap_or(0),
        cc2_mv: read_mv(CH_CC2).unwrap_or(0),
    }
}
