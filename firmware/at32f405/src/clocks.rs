//! 144 MHz from the 12 MHz crystal (RM v2.02 §4.1, §5.8): PLL 12 MHz / 1 × 48 / 4, 4 flash wait states, auto
//! step-by-step switch. The LDO stays at its reset value of 1.2 V (good for up to 192 MHz, DS v2.03 Table 15).
//! 144 MHz rather than 216 MHz saves about 22 mA per chip (DS Table 21; analysis.md finding 6) and still leaves the
//! HS device plenty of headroom. APB2 runs at 144 MHz, APB1 at 72 MHz (max 120 MHz; the OTGHS needs AHB > 30 MHz).
//!
//! If the crystal doesn't start within [`HEXT_WAIT_MS`], the PLL runs from the internal HICK instead (48 MHz / 6 =
//! 8 MHz into the PLL with `PLLRCS` = 0, RM §4.1.1; × 72 / 4 = 144 MHz, calibrated to 1 %): the SPI link and the
//! timers work as usual, so the chip can tell the RP2354B, but the HS PHY needs the crystal and USB stays off.

use at32f4xx_pac::at32f405 as pac;

pub const SYSCLK_HZ: u32 = 144_000_000;

const PLL_MS: u8 = 1; // 12 MHz (HEXT) or 8 MHz (HICK / 6) into the VCO stage (2–16 MHz)
const PLL_NS_HEXT: u16 = 48; // VCO 576 MHz (500–1000 MHz)
const PLL_NS_HICK: u16 = 72; // VCO 576 MHz
/// The crystal starts in a few ms; much longer means it won't.
const HEXT_WAIT_MS: u32 = 100;
/// The reset clock (HICK / 6) runs the CPU at 8 MHz while it waits.
const RESET_CLOCK_KHZ: u32 = 8_000;
const PLL_FP_DIV4: u8 = 0b0010; // PLLP = VCO / 4 = 144 MHz
const WAIT_STATES_144MHZ: u8 = 4; // 128–160 MHz
const SCLKSEL_PLL: u8 = 0b10;
const APB_DIV1: u8 = 0b000;
const APB_DIV2: u8 = 0b100;

/// Sets up 144 MHz; returns whether the crystal runs (false: on the HICK, without USB).
pub fn init() -> bool {
    // SAFETY: runs once at boot, before anything else touches these peripherals.
    let crm = unsafe { &*pac::CRM::ptr() };
    let flash = unsafe { &*pac::FLASH::ptr() };

    crm.ctrl().modify(|_, w| w.hexten().set_bit());
    let mut crystal = false;
    for _ in 0..HEXT_WAIT_MS {
        if crm.ctrl().read().hextstbl().bit_is_set() {
            crystal = true;
            break;
        }
        cortex_m::asm::delay(RESET_CLOCK_KHZ);
    }
    if !crystal {
        crm.ctrl().modify(|_, w| w.hexten().clear_bit());
    }

    flash.psr().modify(|_, w| unsafe { w.wtcyc().bits(WAIT_STATES_144MHZ) });

    crm.pllcfg().modify(|_, w| unsafe {
        // PLLRCS: 1 = HEXT, 0 = HICK (HICK / 6 with HICKDIV at its reset value).
        if crystal {
            w.pllrcs().set_bit();
        } else {
            w.pllrcs().clear_bit();
        }
        w.pll_ms().bits(PLL_MS);
        w.pll_ns().bits(if crystal { PLL_NS_HEXT } else { PLL_NS_HICK });
        w.pll_fp().bits(PLL_FP_DIV4)
    });
    crm.ctrl().modify(|_, w| w.pllen().set_bit());
    while crm.ctrl().read().pllstbl().bit_is_clear() {}

    crm.cfg().modify(|_, w| unsafe {
        w.ahbdiv().bits(0);
        w.apb1div().bits(APB_DIV2);
        w.apb2div().bits(APB_DIV1)
    });
    crm.misc2().modify(|_, w| unsafe { w.auto_step_en().bits(0b11) });
    crm.cfg().modify(|_, w| unsafe { w.sclksel().bits(SCLKSEL_PLL) });
    while crm.cfg().read().sclksts().bits() != SCLKSEL_PLL {}
    crm.misc2().modify(|_, w| unsafe { w.auto_step_en().bits(0) });
    crystal
}
