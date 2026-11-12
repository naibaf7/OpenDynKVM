//! RP2354B pins on the rev-5 board (schematic block 5, `hardware/tools/gen_schematic.py` as of 2026-10-23).
//!
//! | GPIO | Net | | GPIO | Net |
//! |---|---|---|---|---|
//! | 16–19 | AT1 MISO/CS/SCK/MOSI (SPI0) | | 44–47 | AT2 MISO/CS/SCK/MOSI (SPI1) |
//! | 12 / 13 / 14 | AT1 IRQ / NRST / BOOT0 | | 38 / 39 / 43 | AT2 IRQ / NRST / BOOT0 |
//! | 6 | HUB1_RST (open drain, shared with AT1 PB1) | | 35 | HUB2_RST (open drain, shared with AT2 PB1) |
//! | 20 / 21 / 22 / 23 | KB router SEL0 / SEL1, VBUS enable / fault | | 24 / 25 / 26 / 27 | MS, likewise |
//! | 34 / 37 / 32 / 33 | OT router SEL0 / SEL1, VBUS enable / fault | | 2 / 3 | Hub-0 RESET_N / VBUS_DET |
//! | 15 | LED data (via 74AHCT1G125) | | 11 | KEY (MX switch to GND) |
//! | 0 | SVC_SEL: 0 = Hub-0, 1 = USB to the PWR/SERVICE port (the switch's SEL0: USB1 = 0,0 vs USB2 = 0,1) | | 1 | PWR_ST: high = powered by the PWR port |
//! | 40 / 41 / 42 | ADC: VSNS_PWR (VBUS × 47/147), RP_CC1, RP_CC2 (10 kΩ in series) | | 36 | DBG_TX (UART1 TX) |

use embassy_rp::gpio::{Flex, Level, Output};
use kvm_core::Route;
use kvm_proto::ctrl::Pc;

/// One front port: its TS3USB3031 select lines and TPS2553 enable.
pub struct FrontPort {
    pub sel0: Output<'static>,
    pub sel1: Output<'static>,
    pub en: Output<'static>,
}

impl FrontPort {
    /// SEL1/SEL0: MHL = Hub-0 `1,0` (power-up default, the KVM path), USB2 = Hub-1/PC1 `0,1`, USB1 = Hub-2/PC2
    /// `0,0`, off `1,1`.
    pub fn route(&mut self, r: Route) {
        let (s1, s0) = match r {
            Route::Kvm => (true, false),
            Route::Pc(Pc::Pc1) => (false, true),
            Route::Pc(Pc::Pc2) => (false, false),
            Route::Off => (true, true),
        };
        // Off first, so the device never sees two hubs at once.
        self.sel1.set_high();
        self.sel0.set_high();
        self.sel1.set_level(Level::from(s1));
        self.sel0.set_level(Level::from(s0));
    }

    pub fn power(&mut self, on: bool) {
        self.en.set_level(Level::from(on));
    }
}

/// Reset and boot-mode lines of one AT32F405, and the reset of the PC hub it sits on.
pub struct At32Lines {
    /// Released (input) normally; driven low to reset. The AT32 has its own pull-up and a 100 nF on NRST.
    nrst: Flex<'static>,
    pub boot0: Output<'static>,
    /// `HUBn_RST`, shared with the AT32's PB1 and the hub's 10 kΩ/100 nF power-on reset: released normally.
    hub_rst: Flex<'static>,
}

/// Drives an open-drain line low, or releases it.
fn open_drain(pin: &mut Flex<'static>, low: bool) {
    if low {
        pin.set_low();
        pin.set_as_output();
    } else {
        pin.set_as_input();
    }
}

impl At32Lines {
    pub fn new(mut nrst: Flex<'static>, boot0: Output<'static>, mut hub_rst: Flex<'static>) -> Self {
        open_drain(&mut nrst, false);
        open_drain(&mut hub_rst, false);
        At32Lines { nrst, boot0, hub_rst }
    }

    pub fn set_reset(&mut self, asserted: bool) {
        open_drain(&mut self.nrst, asserted);
    }

    /// Holds the PC hub in reset, so the PC cannot see the AT32's ROM DFU device while it is being flashed.
    pub fn hold_hub(&mut self, asserted: bool) {
        open_drain(&mut self.hub_rst, asserted);
    }
}

pub struct Board {
    pub ports: [FrontPort; 3],
    pub hub0_rst: Output<'static>,
    /// Held high: tells Hub-0 its upstream is powered, so it connects to the RP2354B.
    pub _hub0_vbus: Output<'static>,
    pub svc_sel: Output<'static>,
}
