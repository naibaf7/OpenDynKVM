//! The status LEDs (design.md §7). One data line, in chain order: the side-emitting LEDs left to right along the front
//! (PC1, Keyboard, Mouse, Aux 1, Aux 2, PC2), then the hand-fitted LED under the key, last so that a missing or damaged
//! key LED leaves the status LEDs working. A board before 0.19.0 has no Aux 1 port and no LED for it: six LEDs, the
//! same order without that one.

use kvm_proto::ctrl::{LinkState, PORTS, Pc, PortMode, PortState, Rgb, selftest};

/// LEDs on the longest chain (boards from 0.19.0). [`render`] always gives this many; on a shorter chain the last
/// colour goes nowhere.
pub const COUNT: usize = 7;

/// Position of each LED in the chain of a board with the Aux 1 port; [`render`] closes the gap on one without.
pub mod chain {
    pub const PC1: usize = 0;
    pub const KEYBOARD: usize = 1;
    pub const MOUSE: usize = 2;
    pub const AUX1: usize = 3;
    pub const AUX2: usize = 4;
    pub const PC2: usize = 5;
    pub const KEY: usize = 6;
    pub const PCS: [usize; 2] = [PC1, PC2];
    /// By `Port` index.
    pub const PORTS: [usize; super::PORTS] = [KEYBOARD, MOUSE, AUX1, AUX2];
}

/// The LEDs a board has, as [`chain`] positions in chain order: all of them, or all but Aux 1's.
fn layout(aux1: bool) -> &'static [usize] {
    use chain::*;
    if aux1 { &[PC1, KEYBOARD, MOUSE, AUX1, AUX2, PC2, KEY] } else { &[PC1, KEYBOARD, MOUSE, AUX2, PC2, KEY] }
}

/// Colours by [`chain`] position, in the order of the LEDs the board has.
fn to_chain(c: [Rgb; COUNT], layout: &[usize]) -> [Rgb; COUNT] {
    core::array::from_fn(|k| layout.get(k).map_or(OFF, |&pos| c[pos]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LedInputs {
    pub active: Pc,
    pub links: [LinkState; 2],
    /// The board has the Aux 1 port, and the LED for it (`caps::AUX1`, boards from 0.19.0).
    pub aux1: bool,
    /// Each port's state, effective mode, and the PC it is pinned to.
    pub ports: [(PortState, PortMode, Option<Pc>); PORTS],
    pub color_pc1: Rgb,
    pub color_pc2: Rgb,
    pub brightness: u8,
    pub identify: bool,
    pub awaiting_key: bool,
    /// The 5 V the KVM runs from sags under the load, as measured (`kvm_core::supply`): the connected PCs' LEDs amber.
    pub supply_sags: bool,
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
/// - Keyboard / Mouse / Aux 1 / Aux 2: the port's power switch;
/// - key: Hub-0, the PWR-port readings and the Bluetooth module's firmware.
fn selftest_code(bits: u32, now_ms: u32, layout: &[usize]) -> [Rgb; COUNT] {
    if bits & selftest::RUNNING != 0 {
        let lit = layout[(now_ms / 150) as usize % layout.len()];
        return core::array::from_fn(|k| if k == lit { WHITE } else { OFF });
    }
    let red = if blink(now_ms, 400) { RED } else { OFF };
    let show = |failed: u32| if bits & failed != 0 { red } else { GREEN };
    let mut out = [OFF; COUNT];
    for k in 0..2 {
        out[chain::PCS[k]] = show(selftest::PCS[k]);
    }
    for k in 0..PORTS {
        out[chain::PORTS[k]] = show(selftest::PORTS[k]);
    }
    // Not the Bluetooth module's firmware: after an update it waits for the host software to send it, as intended.
    out[chain::KEY] = show(selftest::HUB0 | selftest::POWER);
    out
}

/// Colours in the order of the board's chain ([`chain`] on a board with Aux 1, without its LED on one without),
/// brightness applied.
pub fn render(i: &LedInputs, now_ms: u32) -> [Rgb; COUNT] {
    let layout = layout(i.aux1);
    if let Some(bits) = i.selftest {
        return to_chain(selftest_code(bits, now_ms, layout), layout).map(|c| scale(c, i.brightness as u16));
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
        return to_chain(out, layout).map(|c| scale(c, i.brightness as u16));
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
            LinkState::Connected if i.supply_sags => scale(AMBER, if active { 255 } else { 60 }),
            LinkState::Connected => scale(c, if active { 255 } else { 40 }),
        };
    }

    for (k, (state, mode, pinned)) in i.ports.iter().enumerate() {
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
            PortState::Hosted | PortState::Passthrough => match (mode, pinned) {
                (PortMode::Normalized | PortMode::Auto, _) => GREEN,
                (PortMode::Mirrored, _) => CYAN,
                (PortMode::MirroredIdentity, _) => MAGENTA,
                // Passthrough pinned to a PC in that PC's colour, following the active PC in white.
                (PortMode::Passthrough, Some(Pc::Pc1)) => i.color_pc1,
                (PortMode::Passthrough, Some(Pc::Pc2)) => i.color_pc2,
                (PortMode::Passthrough, None) => WHITE,
                (PortMode::Off, _) => OFF,
            },
        };
    }

    // A profile just made active: the front LEDs count it from the left, one for profile 0, two for profile 1, …, in
    // white for the first five and in cyan for profiles 5–7 (one for profile 5), on either board.
    let mut out = to_chain(out, layout);
    let front = layout.len() - 1;
    if let Some(k) = i.profile {
        let (count, colour) = if k < 5 { (k as usize + 1, WHITE) } else { (k as usize - 4, CYAN) };
        for (n, c) in out[..front].iter_mut().enumerate() {
            *c = if n < count { colour } else { OFF };
        }
    }

    if i.identify {
        let on = blink(now_ms, 250);
        for c in out[..layout.len()].iter_mut() {
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
            aux1: true,
            ports: [
                (PortState::Hosted, PortMode::Normalized, None),
                (PortState::Hosted, PortMode::MirroredIdentity, None),
                (PortState::Hosted, PortMode::Mirrored, None),
                (PortState::Empty, PortMode::Auto, None),
            ],
            color_pc1: Rgb::new(0, 0, 255),
            color_pc2: Rgb::new(255, 0, 0),
            brightness: 255,
            identify: false,
            awaiting_key: false,
            bt_active: false,
            bt_pairing: false,
            bt_passkey: false,
            supply_sags: false,
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
        assert_eq!((c[chain::KEYBOARD], c[chain::MOUSE], c[chain::AUX1], c[chain::AUX2]), (GREEN, MAGENTA, CYAN, OFF));
        let mut dim = inputs();
        dim.brightness = 128;
        assert_eq!(render(&dim, 0)[chain::PC2], Rgb::new(128, 0, 0));
    }

    #[test]
    fn chain_order_puts_the_key_last() {
        assert_eq!(
            [chain::PC1, chain::KEYBOARD, chain::MOUSE, chain::AUX1, chain::AUX2, chain::PC2, chain::KEY],
            [0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn a_board_without_aux1_has_six_leds_in_the_same_order() {
        let mut i = inputs();
        i.aux1 = false;
        i.ports[3] = (PortState::Hosted, PortMode::Normalized, None);
        let c = render(&i, 0);
        // PC1, Keyboard, Mouse, Aux 2, PC2, key; nothing after the key.
        assert_eq!(c, [Rgb::new(0, 0, 40), GREEN, MAGENTA, GREEN, Rgb::new(255, 0, 0), Rgb::new(255, 0, 0), OFF]);
        i.profile = Some(4);
        assert_eq!(render(&i, 0)[..5], [WHITE; 5]);
        i.selftest = Some(selftest::PORT_A2);
        assert_eq!(render(&i, 0)[3], RED);
        i.selftest = Some(selftest::RUNNING);
        assert_eq!(render(&i, 5 * 150)[5], WHITE, "the walking light reaches the key, the sixth LED");
        i.selftest = None;
        i.identify = true;
        assert_eq!(render(&i, 0)[6], OFF);
    }

    #[test]
    fn selftest_code_marks_the_failed_parts() {
        let mut i = inputs();
        i.selftest = Some(selftest::RUNNING);
        let c = render(&i, 150);
        assert_eq!(c.iter().filter(|c| **c == WHITE).count(), 1);
        assert_eq!(c[1], WHITE);
        i.selftest = Some(selftest::LINK2 | selftest::PORT_MS | selftest::PORT_A1);
        let c = render(&i, 0);
        assert_eq!((c[chain::PC1], c[chain::PC2]), (GREEN, RED));
        assert_eq!((c[chain::KEYBOARD], c[chain::MOUSE], c[chain::AUX2], c[chain::KEY]), (GREEN, RED, GREEN, GREEN));
        assert_eq!(c[chain::AUX1], RED);
        assert_eq!(render(&i, 200)[chain::PC2], OFF);
        // A Bluetooth module waiting for its firmware is no failure.
        i.selftest = Some(selftest::BT_FW);
        let c = render(&i, 0);
        assert_eq!((c[chain::PC1], c[chain::KEY]), (GREEN, GREEN));
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
        assert_eq!(c[..chain::KEY], [WHITE, WHITE, OFF, OFF, OFF, OFF]);
        assert_eq!(c[chain::KEY], Rgb::new(255, 0, 0), "the key keeps the active PC's colour");
        i.profile = Some(4);
        assert_eq!(render(&i, 0)[..chain::KEY], [WHITE, WHITE, WHITE, WHITE, WHITE, OFF]);
        i.profile = Some(5);
        assert_eq!(render(&i, 0)[..chain::KEY], [CYAN, OFF, OFF, OFF, OFF, OFF]);
        i.profile = Some(7);
        assert_eq!(render(&i, 0)[..chain::KEY], [CYAN, CYAN, CYAN, OFF, OFF, OFF]);
    }

    #[test]
    fn passthrough_shows_where_it_is_pinned() {
        let mut i = inputs();
        i.ports[3] = (PortState::Passthrough, PortMode::Passthrough, Some(Pc::Pc1));
        assert_eq!(render(&i, 0)[chain::AUX2], Rgb::new(0, 0, 255));
        i.ports[3].2 = None;
        assert_eq!(render(&i, 0)[chain::AUX2], WHITE);
        i.ports[0] = (PortState::Hosted, PortMode::Off, None);
        assert_eq!(render(&i, 0)[chain::KEYBOARD], OFF);
    }

    #[test]
    fn identify_blinks_everything() {
        let mut i = inputs();
        i.identify = true;
        assert!(render(&i, 0).iter().all(|c| *c == WHITE));
        assert!(render(&i, 130).iter().all(|c| *c == OFF));
    }
}
