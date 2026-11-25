//! RP2354B pins on boards 0.12.0 to 0.19.0 (schematic block 5, `hardware/tools/gen_schematic.py`).
//!
//! | GPIO | Net | | GPIO | Net |
//! |---|---|---|---|---|
//! | 16–19 | AT1 MISO/CS/SCK/MOSI (SPI0) | | 44–47 | AT2 MISO/CS/SCK/MOSI (SPI1) |
//! | 12 / 13 / 14 | AT1 IRQ / NRST / BOOT0 | | 38 / 39 / 43 | AT2 IRQ / NRST / BOOT0 |
//! | 6 | HUB1_RST (open drain, shared with AT1 PB1) | | 35 | HUB2_RST (open drain, shared with AT2 PB1) |
//! | 20 / 21 / 22 / 23 | KB router SEL0 / SEL1, VBUS enable / fault | | 24 / 25 / 26 / 27 | MS, likewise |
//! | 34 / 37 / 32 / 33 | Aux 2 router SEL0 / SEL1, VBUS enable / fault | | 2 / 3 | Hub-0 RESET_N / VBUS_DET |
//! | 8 / 10 | Aux 1 VBUS enable / fault (0.19.0 on; no router) | | 4 / 5 / 7 | the rear extension's I2C0 SDA / SCL and power enable (0.19.0 on; unused, left to their pull-downs) |
//! | 15 | LED data (via 74AHCT1G125) | | 11 | KEY (MX switch to GND) |
//! | 0 | SVC_SEL: 0 = Hub-0, 1 = USB to the PWR/SERVICE port (the switch's SEL0: USB1 = 0,0 vs USB2 = 0,1) | | 1 | PWR_ST: high = powered by the PWR port |
//! | 40 / 41 / 42 | ADC: VSNS_PWR (VBUS × 47/147), RP_CC1, RP_CC2 (10 kΩ in series) | | 36 | DBG_TX (UART1 TX) |
//!
//! Boards before 0.19.0 leave GPIO4/5/7/8/10 unconnected; [`has_aux1`] tells the two apart.

use embassy_rp::gpio::{Flex, Level, Output, Pull};
use embassy_time::Timer;
use kvm_core::Route;
use kvm_proto::ctrl::Pc;

/// A front port's TS3USB3031 select lines.
pub struct Router {
    pub sel0: Output<'static>,
    pub sel1: Output<'static>,
}

/// One front port: its router (none on Aux 1, which sits on Hub-0 alone) and its TPS2553 enable.
pub struct FrontPort {
    pub router: Option<Router>,
    pub en: Output<'static>,
}

impl FrontPort {
    /// SEL1/SEL0: MHL = Hub-0 `1,0` (power-up default, the KVM path), USB2 = Hub-1/PC1 `0,1`, USB1 = Hub-2/PC2
    /// `0,0`, off `1,1`. Nothing on a port without a router: its device is always on Hub-0 (the engine never routes
    /// it elsewhere).
    pub fn route(&mut self, r: Route) {
        let Some(router) = &mut self.router else { return };
        let (s1, s0) = match r {
            Route::Kvm => (true, false),
            Route::Pc(Pc::Pc1) => (false, true),
            Route::Pc(Pc::Pc2) => (false, false),
            Route::Off => (true, true),
        };
        // Off first, so the device never sees two hubs at once.
        router.sel1.set_high();
        router.sel0.set_high();
        router.sel1.set_level(Level::from(s1));
        router.sel0.set_level(Level::from(s0));
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

/// Whether the board has the Aux 1 port (0.19.0 on): its TPS2553's FAULT line (GPIO10) has a 10 kΩ pull-up to 3.3 V
/// there and is released while the switch is off, so it reads high against the pad's pull-down (36–113 kΩ: at least
/// 2.6 V); a board before 0.19.0 leaves GPIO10 unconnected, and the pull-down holds it low. Read at boot, before the
/// switch is ever on; a stuck-low FAULT line would make a 0.19.0 board look like one before it (three ports).
pub async fn has_aux1(fault: &mut Flex<'static>) -> bool {
    fault.set_as_input();
    fault.set_pull(Pull::Down);
    Timer::after_millis(2).await;
    let mut high = true;
    for _ in 0..5 {
        high &= fault.is_high();
        Timer::after_micros(200).await;
    }
    // Pulled up from here on where it is (the power task reads it), held low where it isn't.
    fault.set_pull(if high { Pull::None } else { Pull::Down });
    high
}

pub struct Board {
    /// By `Port` index. Aux 1's enable (GPIO8) is driven low on a board without it too, where the pin is unconnected.
    pub ports: [FrontPort; kvm_proto::ctrl::PORTS],
    pub hub0_rst: Output<'static>,
    /// Held high: tells Hub-0 its upstream is powered, so it connects to the RP2354B.
    pub _hub0_vbus: Output<'static>,
    pub svc_sel: Output<'static>,
}
