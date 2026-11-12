//! Pins of AT32F405 #1 / #2 (schematic blocks 10–11, software.md §5.1).
//!
//! | Pin | Use |
//! |---|---|
//! | PA4–PA7 | SPI1 slave to the RP2354B (CS, SCK, MISO, MOSI), MUX5 |
//! | PB12 | `ATn_IRQ` to the RP2354B, push-pull, high = frames pending |
//! | PB1 | `HUBn_RST`, open drain; shared with an RP2354B GPIO, which holds the hub in reset while it flashes this chip. Held low here while the PC gives no 5 V ([`hold_hub`]) |
//! | PC13 | role strap: low = PC1, high = PC2 |
//! | PA0 / PA1 / PB0 | ADC: VBUS/2, CC1, CC2 of this PC's USB-C port |

use core::sync::atomic::{AtomicBool, Ordering};

use at32f4xx_pac::at32f405 as pac;
use kvm_proto::link::msg::role;

const MODE_OUTPUT: u32 = 0b01;
const MODE_MUX: u32 = 0b10;
const MODE_ANALOG: u32 = 0b11;
const PULL_UP: u32 = 0b01;
const DRIVE_STRONG: u32 = 0b01;
const MUX_SPI1: u32 = 5;

const IRQ_PIN: u32 = 12;
const HUB_RST_PIN: u32 = 1;

fn gpioa() -> &'static pac::gpioa::RegisterBlock {
    // SAFETY: MMIO block; all accesses are read-modify-writes done at init or single-bit set/clear writes.
    unsafe { &*pac::GPIOA::ptr() }
}

fn gpiob() -> &'static pac::gpioa::RegisterBlock {
    unsafe { &*pac::GPIOB::ptr() }
}

fn gpioc() -> &'static pac::gpioa::RegisterBlock {
    unsafe { &*pac::GPIOC::ptr() }
}

/// Sets the 2-bit field of each listed pin.
fn field2(old: u32, pins: &[u32], v: u32) -> u32 {
    pins.iter().fold(old, |acc, p| (acc & !(0b11 << (2 * p))) | (v << (2 * p)))
}

/// Configures every pin and returns this chip's role (`role::PC1` or `role::PC2`).
pub fn init() -> u8 {
    let crm = unsafe { &*pac::CRM::ptr() };
    crm.ahben1().modify(|_, w| w.gpioa().set_bit().gpiob().set_bit().gpioc().set_bit());

    let a = gpioa();
    a.muxl().modify(|r, w| unsafe {
        let v = (4..8).fold(r.bits(), |acc, p| (acc & !(0xF << (4 * p))) | (MUX_SPI1 << (4 * p)));
        w.bits(v)
    });
    a.cfgr().modify(|r, w| unsafe {
        let v = field2(r.bits(), &[4, 5, 6, 7], MODE_MUX);
        w.bits(field2(v, &[0, 1], MODE_ANALOG))
    });
    a.odrvr().modify(|r, w| unsafe { w.bits(field2(r.bits(), &[6], DRIVE_STRONG)) });
    // CS idles high while the RP2354B is in reset or flashing.
    a.pull().modify(|r, w| unsafe { w.bits(field2(r.bits(), &[4], PULL_UP)) });

    let b = gpiob();
    b.clr().write(|w| unsafe { w.bits(1 << IRQ_PIN) });
    b.scr().write(|w| unsafe { w.bits(1 << HUB_RST_PIN) });
    b.omode().modify(|r, w| unsafe { w.bits(r.bits() | 1 << HUB_RST_PIN) });
    b.cfgr().modify(|r, w| unsafe {
        let v = field2(r.bits(), &[IRQ_PIN, HUB_RST_PIN], MODE_OUTPUT);
        w.bits(field2(v, &[0], MODE_ANALOG))
    });

    // PC13 stays an input (reset state); the strap resistor sets it.
    if gpioc().idt().read().bits() & (1 << 13) != 0 { role::PC2 } else { role::PC1 }
}

/// `ATn_IRQ`: tells the RP2354B to clock a frame soon.
pub fn set_irq(pending: bool) {
    let b = gpiob();
    if pending {
        b.scr().write(|w| unsafe { w.bits(1 << IRQ_PIN) });
    } else {
        b.clr().write(|w| unsafe { w.bits(1 << IRQ_PIN) });
    }
}

/// [`hold_hub`] is on: a recovery pulse must not release the hub.
static HUB_HELD: AtomicBool = AtomicBool::new(false);

fn drive_hub_rst(low: bool) {
    let b = gpiob();
    if low {
        b.clr().write(|w| unsafe { w.bits(1 << HUB_RST_PIN) });
    } else {
        b.scr().write(|w| unsafe { w.bits(1 << HUB_RST_PIN) });
    }
}

/// Puts this PC's hub in reset and takes it out again (recovery pulse from the RP2354B).
pub fn hub_reset(asserted: bool) {
    drive_hub_rst(asserted || HUB_HELD.load(Ordering::Relaxed));
}

/// Keeps this PC's hub in reset while the PC gives no 5 V: the hub cannot connect without it, and in reset the
/// USB2514B draws 0.75 mA instead of 40–45 mA (datasheet DS00001692C, tables 6-2 and 6-7).
pub fn hold_hub(held: bool) {
    HUB_HELD.store(held, Ordering::Relaxed);
    drive_hub_rst(held);
}
