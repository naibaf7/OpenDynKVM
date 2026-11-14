//! The six status LEDs (design.md §7). One data line, chain order as on the rev 5 board: the five side-emitting LEDs
//! left to right along the front (PC1, Keyboard, Mouse, Other, PC2), then the hand-fitted LED under the key, last so
//! that a missing or damaged key LED leaves the status LEDs working.

use kvm_proto::ctrl::{LinkState, Pc, PortMode, PortState, Rgb, selftest};

pub const COUNT: usize = 6;

/// Position of each LED in the chain.
pub mod chain {
    pub const PC1: usize = 0;
    pub const KEYBOARD: usize = 1;
    pub const MOUSE: usize = 2;
    pub const OTHER: usize = 3;
    pub const PC2: usize = 4;
    pub const KEY: usize = 5;
    pub const PCS: [usize; 2] = [PC1, PC2];
    pub const PORTS: [usize; 3] = [KEYBOARD, MOUSE, OTHER];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LedInputs {
    pub active: Pc,
    pub links: [LinkState; 2],
    pub ports: [(PortState, PortMode); 3],
    pub color_pc1: Rgb,
    pub color_pc2: Rgb,
    pub brightness: u8,
    pub identify: bool,
    pub awaiting_key: bool,
    pub budget_low: bool,
    /// While the boot-time self-test runs, and for a few seconds after: its `ctrl::selftest` bits.
    pub selftest: Option<u32>,
    /// Standby (software.md §4.12): everything dark, or with `standby_glow` a slow glow on the key.
    pub standby: bool,
    pub standby_glow: bool,
    /// A part of the KVM doesn't work (`selftest::ON_KEY`): the key blinks red, in standby too.
    pub fault: bool,
    /// Just after the active profile changed: its number, shown on the front LEDs for a moment.
    pub profile: Option<u8>,
    /// A Bluetooth host gets input (software.md §4.13): the key glows violet, both PC LEDs as inactive.
    pub bt_active: bool,
    /// Bluetooth pairing is open: the key breathes violet.
    pub bt_pairing: bool,
    /// A pairing host waits for its passkey to be typed: the key blinks violet.
    pub bt_passkey: bool,
}

/// The Bluetooth hosts' colour (no PC colour default is near it; mirrored ports are cyan).
pub const BT_VIOLET: Rgb = Rgb::new(140, 0, 255);

const OFF: Rgb = Rgb::new(0, 0, 0);
const RED: Rgb = Rgb::new(255, 0, 0);
const AMBER: Rgb = Rgb::new(255, 150, 0);
const GREEN: Rgb = Rgb::new(0, 255, 40);
const CYAN: Rgb = Rgb::new(0, 220, 255);
const MAGENTA: Rgb = Rgb::new(255, 0, 200);
const WHITE: Rgb = Rgb::new(255, 255, 255);
const YELLOW: Rgb = Rgb::new(255, 220, 0);

fn scale(c: Rgb, level: u16) -> Rgb {
    let s = |v: u8| ((v as u16 * level) / 255) as u8;
    Rgb::new(s(c.r), s(c.g), s(c.b))
}

/// Triangle wave 0..=255 with the given period.
fn wave(now_ms: u32, period_ms: u32) -> u16 {
    let p = (now_ms % period_ms) * 512 / period_ms;
    (if p < 256 { p } else { 511 - p }) as u16
}

fn blink(now_ms: u32, period_ms: u32) -> bool {
    now_ms % period_ms < period_ms / 2
}

/// The self-test code (design.md §10, step 1.5). While it runs, one white LED walks along the chain. Then each LED
/// shows its part of the result: green passed, blinking red failed.
/// - PC1 / PC2: that PC's link, hub and AT32F405 firmware;
/// - Keyboard / Mouse / Other: the port's power switch;
/// - key: Hub-0, the PWR-port readings and the Bluetooth module's firmware.
fn selftest_code(bits: u32, now_ms: u32) -> [Rgb; COUNT] {
    if bits & selftest::RUNNING != 0 {
        let lit = (now_ms / 150) as usize % COUNT;
        return core::array::from_fn(|k| if k == lit { WHITE } else { OFF });
    }
    let red = if blink(now_ms, 400) { RED } else { OFF };
    let show = |failed: u32| if bits & failed != 0 { red } else { GREEN };
    let mut out = [OFF; COUNT];
    for k in 0..2 {
        out[chain::PCS[k]] = show(selftest::PCS[k]);
    }
    for k in 0..3 {
        out[chain::PORTS[k]] = show(selftest::PORTS[k]);
    }
    out[chain::KEY] = show(selftest::HUB0 | selftest::POWER | selftest::BT);
    out
}

/// Colours in chain order ([`chain`]), brightness applied.
pub fn render(i: &LedInputs, now_ms: u32) -> [Rgb; COUNT] {
    if let Some(bits) = i.selftest {
        return selftest_code(bits, now_ms).map(|c| scale(c, i.brightness as u16));
    }
    let pc_color = |pc: Pc| if pc == Pc::Pc1 { i.color_pc1 } else { i.color_pc2 };
    let mut out = [OFF; COUNT];
    let fault_on = i.fault && blink(now_ms, 1000);
    if i.standby && !i.identify && !i.awaiting_key {
        if fault_on {
            out[chain::KEY] = RED;
        } else if i.standby_glow && !i.fault {
            // Breathes between a quarter and the full set brightness, in the active PC's colour.
            out[chain::KEY] = scale(pc_color(i.active), 64 + wave(now_ms, 6000) * 3 / 4);
        }
        return out.map(|c| scale(c, i.brightness as u16));
    }

    // The key glows in the active PC's colour, pulses white while a firmware update waits for a press, and blinks red
    // (alternating with the PC's colour) while a part of the KVM doesn't work.
    out[chain::KEY] = if i.awaiting_key {
        scale(WHITE, 64 + wave(now_ms, 600) * 3 / 4)
    } else if i.bt_passkey {
        if blink(now_ms, 250) { BT_VIOLET } else { OFF }
    } else if i.bt_pairing {
        scale(BT_VIOLET, 64 + wave(now_ms, 1500) * 3 / 4)
    } else if fault_on {
        RED
    } else if i.bt_active {
        BT_VIOLET
    } else {
        pc_color(i.active)
    };

    for (k, pc) in [Pc::Pc1, Pc::Pc2].into_iter().enumerate() {
        let c = pc_color(pc);
        let active = pc == i.active && !i.bt_active;
        out[chain::PCS[k]] = match i.links[k] {
            LinkState::Unpowered => OFF,
            LinkState::LinkDown => {
                if blink(now_ms, 500) {
                    RED
                } else {
                    OFF
                }
            }
            LinkState::Suspended => scale(c, 16 + wave(now_ms, 3000) / 3),
            LinkState::Detached | LinkState::Enumerating => scale(c, 40),
            LinkState::Connected if i.budget_low => scale(AMBER, if active { 255 } else { 60 }),
            LinkState::Connected => scale(c, if active { 255 } else { 40 }),
        };
    }

    for (k, (state, mode)) in i.ports.iter().enumerate() {
        out[chain::PORTS[k]] = match state {
            PortState::Empty => OFF,
            PortState::Probing => scale(YELLOW, wave(now_ms, 400)),
            PortState::Fault => {
                if blink(now_ms, 300) {
                    RED
                } else {
                    OFF
                }
            }
            PortState::Unpowered => scale(RED, 40),
            PortState::Unsupported => scale(AMBER, 80),
            // The Bluetooth module on this port (a dongle): the Bluetooth hosts' colour.
            PortState::Module => scale(BT_VIOLET, 160),
            PortState::Hosted | PortState::Passthrough => match mode {
                PortMode::Normalized | PortMode::Auto => GREEN,
                PortMode::Mirrored => CYAN,
                PortMode::MirroredIdentity => MAGENTA,
                PortMode::Passthrough => WHITE,
                PortMode::PassthroughPc1 => i.color_pc1,
                PortMode::PassthroughPc2 => i.color_pc2,
            },
        };
    }

    // A profile just made active: the front LEDs count it from the left, one for profile 0, two for profile 1, …
    if let Some(k) = i.profile {
        for (n, c) in out[..chain::KEY].iter_mut().enumerate() {
            *c = if n <= k as usize { WHITE } else { OFF };
        }
    }

    if i.identify {
        let on = blink(now_ms, 250);
        for c in out.iter_mut() {
            *c = if on { WHITE } else { OFF };
        }
    }
    let level = i.brightness as u16;
    out.map(|c| scale(c, level))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> LedInputs {
        LedInputs {
            active: Pc::Pc2,
            links: [LinkState::Connected, LinkState::Connected],
            ports: [
                (PortState::Hosted, PortMode::Normalized),
                (PortState::Hosted, PortMode::MirroredIdentity),
                (PortState::Empty, PortMode::Auto),
            ],
            color_pc1: Rgb::new(0, 0, 255),
            color_pc2: Rgb::new(255, 0, 0),
            brightness: 255,
            identify: false,
            awaiting_key: false,
            bt_active: false,
            bt_pairing: false,
            bt_passkey: false,
            budget_low: false,
            selftest: None,
            standby: false,
            standby_glow: false,
            fault: false,
            profile: None,
        }
    }

    #[test]
    fn active_pc_is_bright_and_modes_have_colours() {
        let c = render(&inputs(), 0);
        assert_eq!(c[chain::KEY], Rgb::new(255, 0, 0));
        assert_eq!(c[chain::PC1], Rgb::new(0, 0, 40));
        assert_eq!(c[chain::PC2], Rgb::new(255, 0, 0));
        assert_eq!((c[chain::KEYBOARD], c[chain::MOUSE], c[chain::OTHER]), (GREEN, MAGENTA, OFF));
        let mut dim = inputs();
        dim.brightness = 128;
        assert_eq!(render(&dim, 0)[chain::PC2], Rgb::new(128, 0, 0));
    }

    #[test]
    fn chain_order_puts_the_key_last() {
        assert_eq!(
            [chain::PC1, chain::KEYBOARD, chain::MOUSE, chain::OTHER, chain::PC2, chain::KEY],
            [0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn selftest_code_marks_the_failed_parts() {
        let mut i = inputs();
        i.selftest = Some(selftest::RUNNING);
        let c = render(&i, 150);
        assert_eq!(c.iter().filter(|c| **c == WHITE).count(), 1);
        assert_eq!(c[1], WHITE);
        i.selftest = Some(selftest::LINK2 | selftest::PORT_MS);
        let c = render(&i, 0);
        assert_eq!((c[chain::PC1], c[chain::PC2]), (GREEN, RED));
        assert_eq!((c[chain::KEYBOARD], c[chain::MOUSE], c[chain::OTHER], c[chain::KEY]), (GREEN, RED, GREEN, GREEN));
        assert_eq!(render(&i, 200)[chain::PC2], OFF);
        // The Bluetooth module's firmware shows on the key.
        i.selftest = Some(selftest::BT_GAVE_UP);
        let c = render(&i, 0);
        assert_eq!((c[chain::PC1], c[chain::KEY]), (GREEN, RED));
    }

    #[test]
    fn standby_is_dark_or_glows_on_the_key() {
        let mut i = inputs();
        i.standby = true;
        assert!(render(&i, 0).iter().all(|c| *c == OFF));
        i.standby_glow = true;
        let c = render(&i, 3000);
        assert_eq!(c[chain::KEY], Rgb::new(255, 0, 0));
        assert!(c[..chain::KEY].iter().all(|c| *c == OFF));
        assert_eq!(render(&i, 0)[chain::KEY], Rgb::new(64, 0, 0));
    }

    #[test]
    fn a_fault_blinks_the_key_red_even_in_standby() {
        let mut i = inputs();
        i.fault = true;
        assert_eq!(render(&i, 0)[chain::KEY], RED);
        assert_eq!(render(&i, 600)[chain::KEY], Rgb::new(255, 0, 0), "PC2's colour (red in these inputs)");
        i.color_pc2 = Rgb::new(0, 255, 0);
        assert_eq!(render(&i, 600)[chain::KEY], Rgb::new(0, 255, 0));
        i.standby = true;
        i.standby_glow = true;
        assert_eq!(render(&i, 0)[chain::KEY], RED);
        assert_eq!(render(&i, 600)[chain::KEY], OFF);
        assert!(render(&i, 0)[..chain::KEY].iter().all(|c| *c == OFF));
        i.awaiting_key = true;
        i.standby = false;
        assert_ne!(render(&i, 0)[chain::KEY], RED, "an update waiting for the key comes first");
    }

    #[test]
    fn a_new_profile_shows_its_number_on_the_front_leds() {
        let mut i = inputs();
        i.profile = Some(1);
        let c = render(&i, 0);
        assert_eq!(c[..chain::KEY], [WHITE, WHITE, OFF, OFF, OFF]);
        assert_eq!(c[chain::KEY], Rgb::new(255, 0, 0), "the key keeps the active PC's colour");
    }

    #[test]
    fn identify_blinks_everything() {
        let mut i = inputs();
        i.identify = true;
        assert!(render(&i, 0).iter().all(|c| *c == WHITE));
        assert!(render(&i, 130).iter().all(|c| *c == OFF));
    }
}
