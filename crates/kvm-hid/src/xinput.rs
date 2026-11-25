//! XInput pads, which have no HID report descriptor: the wired protocol (interface class 0xFF/0x5D/0x01) and the
//! wireless receiver's (0xFF/0x5D/0x81, one interface per pad), decoded into [`GamepadInput`] in the standard
//! gamepad's order (`gamepad.rs`: 1 A, 2 B, 3 X, 4 Y, 5 LB, 6 RB, 9 Back, 10 Start, 11 LS, 12 RS, 17 Home; the d-pad
//! as the hat; the sticks X/Y and Rx/Ry, the triggers brake and accelerator).
//!
//! The wired input report: type 0x00, length 0x14, then the buttons (16 bits: d-pad up, down, left, right, Start,
//! Back, LS, RS, LB, RB, Home, –, A, B, X, Y), LT, RT (8 bits), LX, LY, RX, RY (16 bits, signed, Y up). The receiver
//! wraps the same report in a packet of its own: 0x08 with bit 7 of the second byte telling whether a pad is linked,
//! or 0x00 0x01 with the wired report (its header too) from the fifth byte on. The commands it takes (`inquiry`, `led`) are the receiver's
//! documented ones, as open-source drivers send them.

use crate::gamepad::{GamepadInput, axis};

/// Which XInput protocol an interface speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Kind {
    Wired,
    /// One pad's interface of a wireless receiver.
    Wireless,
}

/// What a report of a receiver's pad interface says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Wireless {
    Input(GamepadInput),
    /// The pad linked (true) or left.
    Link(bool),
    /// Anything else the receiver sends (presence and battery announcements).
    Other,
}

/// The buttons in the order of the report's 16 bits: the standard button each is (0: the d-pad's, or none).
const BUTTONS: [u8; 16] = [0, 0, 0, 0, 10, 9, 11, 12, 5, 6, 17, 0, 1, 2, 3, 4];

/// The body both protocols share (from the button bytes on): None if it is too short.
fn body(b: &[u8]) -> Option<GamepadInput> {
    if b.len() < 12 {
        return None;
    }
    let bits = u16::from_le_bytes([b[0], b[1]]);
    let mut g = GamepadInput::default();
    for (i, n) in BUTTONS.iter().enumerate() {
        if *n != 0 && bits & (1 << i) != 0 {
            g.buttons |= 1 << (n - 1);
        }
    }
    // d-pad bits up, down, left, right as the hat's up, right, down, left.
    let dpad =
        (bits & 1) as u8 | ((bits >> 3 & 1) as u8) << 1 | ((bits >> 1 & 1) as u8) << 2 | ((bits >> 2 & 1) as u8) << 3;
    g.hat = GamepadInput::hat_of(dpad);
    let trigger = |v: u8| v as i32 * crate::gamepad::FULL / 255;
    g.axes[axis::BRAKE] = trigger(b[2]);
    g.axes[axis::ACCELERATOR] = trigger(b[3]);
    let stick = |at: usize| (i16::from_le_bytes([b[at], b[at + 1]]) as i32).max(-crate::gamepad::FULL);
    // XInput's Y axes point up, the normalized ones down.
    g.axes[axis::X] = stick(4);
    g.axes[axis::Y] = -stick(6);
    g.axes[axis::RX] = stick(8);
    g.axes[axis::RY] = -stick(10);
    g.present = present();
    Some(g)
}

/// The axes an XInput pad has.
pub const fn present() -> u16 {
    1 << axis::X | 1 << axis::Y | 1 << axis::RX | 1 << axis::RY | 1 << axis::BRAKE | 1 << axis::ACCELERATOR
}

/// The buttons an XInput pad has (bit n = button n + 1): 1–12 and Home; its d-pad is the hat.
pub const BUTTONS_PRESENT: u32 = 0x0FFF | 1 << 16;

/// A wired pad's input report; None for its other messages (LED and rumble states).
pub fn wired(r: &[u8]) -> Option<GamepadInput> {
    if r.len() < 14 || r[0] != 0x00 || r[1] < 0x14 {
        return None;
    }
    body(&r[2..])
}

/// A report of a receiver's pad interface.
pub fn wireless(r: &[u8]) -> Wireless {
    match r {
        [0x08, s, ..] => Wireless::Link(s & 0x80 != 0),
        // A wired report, its own two header bytes too, from the fifth byte on.
        [0x00, 0x01, _, _, _, _, rest @ ..] => body(rest).map_or(Wireless::Other, Wireless::Input),
        _ => Wireless::Other,
    }
}

/// Asks the receiver whether a pad is linked to this interface: it answers with a [`Wireless::Link`].
pub const INQUIRY: [u8; 12] = [0x08, 0x00, 0x0F, 0xC0, 0, 0, 0, 0, 0, 0, 0, 0];

/// The player LED of a receiver's pad: `slot` 0–3, its quadrant flashing, then on.
pub const fn led(slot: u8) -> [u8; 12] {
    [0x00, 0x00, 0x08, 0x40 + 2 + (slot & 3), 0, 0, 0, 0, 0, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wired_report(buttons: u16, lt: u8, rt: u8, sticks: [i16; 4]) -> [u8; 20] {
        let mut r = [0u8; 20];
        r[1] = 0x14;
        r[2..4].copy_from_slice(&buttons.to_le_bytes());
        r[4] = lt;
        r[5] = rt;
        for (i, s) in sticks.iter().enumerate() {
            r[6 + 2 * i..8 + 2 * i].copy_from_slice(&s.to_le_bytes());
        }
        r
    }

    #[test]
    fn a_wired_pad_reads_in_the_standard_order() {
        // A, Y, Start, the d-pad's up and right; LT full; left stick right and up.
        let r = wired_report(1 << 12 | 1 << 15 | 1 << 4 | 1 | 1 << 3, 255, 0, [32767, 32767, 0, -32768]);
        let g = wired(&r).unwrap();
        assert_eq!(g.buttons, 1 | 1 << 3 | 1 << 9);
        assert_eq!(g.hat, 1, "up and right");
        assert_eq!((g.axes[axis::X], g.axes[axis::Y], g.axes[axis::RY]), (32767, -32767, 32767));
        assert_eq!((g.axes[axis::BRAKE], g.axes[axis::ACCELERATOR]), (32767, 0));
        assert_eq!(wired(&[0x01, 0x03, 0x02]), None, "an LED state");
    }

    #[test]
    fn the_receiver_links_and_wraps_the_wired_body() {
        assert_eq!(wireless(&[0x08, 0x80]), Wireless::Link(true));
        assert_eq!(wireless(&[0x08, 0x00]), Wireless::Link(false));
        let body = wired_report(1 << 13 | 1 << 10, 0, 128, [0; 4]);
        let mut r = [0u8; 29];
        r[1] = 0x01;
        r[4..4 + 20].copy_from_slice(&body);
        match wireless(&r) {
            Wireless::Input(g) => {
                assert_eq!(g.buttons, 1 << 1 | 1 << 16, "B and Home");
                assert_eq!(g.hat, crate::gamepad::HAT_CENTRE);
                assert_eq!(g.axes[axis::ACCELERATOR], 16447);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(wireless(&[0x00, 0x0F, 0x00, 0xF0]), Wireless::Other, "a presence announcement");
        assert_eq!(led(0)[3], 0x42);
    }
}
