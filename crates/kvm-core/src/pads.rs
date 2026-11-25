//! Known gamepads: how the host software draws one, and how its buttons and axes become the standard gamepad's
//! (`kvm_hid::gamepad`: 1 the bottom face button, 2 right, 3 left, 4 top, 5–8 the shoulders and triggers, 9 Back,
//! 10 Start, 11–12 the stick buttons, 17 Home; the sticks X/Y and Rx/Ry, the triggers brake and accelerator), which
//! the virtual gamepad and the keymaps' pictures assume.
//!
//! - **XInput pads** (wired, or behind a wireless receiver) are decoded in that order already (`kvm_hid::xinput`):
//!   offset sticks.
//! - **Symmetric-stick pads with a touch pad** (vendor 0x054C, the 4 and 5 series): their HID descriptor numbers the
//!   face buttons left, bottom, right, top and puts the right stick on Z/Rz and the triggers on Rx/Ry, so the profile
//!   reorders them; it also reads the 5's touch pad, which its descriptor declares as vendor data: the first finger's
//!   contact as button 20, its position as the slider and the dial (a keymap's axis rule with the delta flag makes it
//!   a mouse).
//! - **Any other pad** is passed as its descriptor numbers it: generic.

use kvm_hid::GamepadInput;
use kvm_hid::gamepad::{self, axis};

/// How the host software draws a pad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Style {
    /// Buttons by number, as the device numbers them.
    #[default]
    Generic = 0,
    /// The left stick above the d-pad, lettered face buttons.
    Offset = 1,
    /// The d-pad above the left stick, the sticks side by side, shapes on the face buttons, a touch pad.
    Symmetric = 2,
}

impl Style {
    pub const ALL: [Style; 3] = [Style::Generic, Style::Offset, Style::Symmetric];

    pub const fn name(self) -> &'static str {
        match self {
            Style::Generic => "generic",
            Style::Offset => "offset",
            Style::Symmetric => "symmetric",
        }
    }

    pub fn from_name(s: &str) -> Option<Style> {
        Self::ALL.iter().copied().find(|x| x.name() == s)
    }
}

/// The 5 series' touch pad in its USB input report 1: the first finger's contact byte (bit 7 set while none touches),
/// then 12 bits of X and 12 of Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TouchPad {
    report_id: u8,
    /// Byte of the contact, after the report ID; X and Y follow it.
    at: usize,
    x_max: i32,
    y_max: i32,
}

/// The standard button the touch pad's contact is.
pub const TOUCH_BUTTON: u8 = 20;

/// What the KVM knows of a pad: how it is drawn, and how its controls are put in the standard order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Profile {
    pub style: Style,
    /// HID button n + 1 → standard button (0: as it is), for a pad that numbers them otherwise.
    buttons: Option<[u8; 15]>,
    /// The right stick on Z/Rz and the triggers on Rx/Ry, centred: moved to Rx/Ry and the brake and accelerator.
    swap_axes: bool,
    touch: Option<TouchPad>,
}

/// The symmetric-stick pads' buttons: square, cross, circle, triangle, L1, R1, L2, R2, the two small ones, L3, R3,
/// Home, the touch pad's click, mute.
const SYMMETRIC_BUTTONS: [u8; 15] = [3, 1, 2, 4, 5, 6, 7, 8, 9, 10, 11, 12, 17, 18, 19];

/// The profile of a device: by VID/PID, and XInput by its interfaces (`xinput`: it has one).
pub fn profile(vid: u16, pid: u16, xinput: bool) -> Profile {
    if xinput {
        return Profile { style: Style::Offset, ..Profile::default() };
    }
    match (vid, pid) {
        // The 4 series (two models) and the 5 series (with and without back buttons).
        (0x054C, 0x05C4 | 0x09CC) => {
            Profile { style: Style::Symmetric, buttons: Some(SYMMETRIC_BUTTONS), swap_axes: true, touch: None }
        }
        (0x054C, 0x0CE6 | 0x0DF2) => Profile {
            style: Style::Symmetric,
            buttons: Some(SYMMETRIC_BUTTONS),
            swap_axes: true,
            touch: Some(TouchPad { report_id: 1, at: 32, x_max: 1919, y_max: 1079 }),
        },
        _ => Profile::default(),
    }
}

fn bit(data: &[u8], at: usize, bits: usize) -> Option<i32> {
    let mut v = 0i32;
    for i in 0..bits {
        let b = at + i;
        v |= ((*data.get(b / 8)? >> (b % 8)) as i32 & 1) << i;
    }
    Some(v)
}

impl Profile {
    /// A report's gamepad state (`raw` as it came, its report ID first when `ids`) in the standard order.
    pub fn apply(&self, raw: &[u8], ids: bool, g: &mut Option<GamepadInput>) {
        if let Some(pad) = g.as_mut() {
            if let Some(map) = self.buttons {
                let mut b = pad.buttons & !((1 << map.len()) - 1);
                for (i, to) in map.iter().enumerate() {
                    if pad.buttons & (1 << i) != 0 {
                        b |= 1 << (to - 1);
                    }
                }
                pad.buttons = b;
            }
            if self.swap_axes {
                let a = pad.axes;
                let half = |v: i32| (v + gamepad::FULL) / 2;
                pad.axes[axis::RX] = a[axis::Z];
                pad.axes[axis::RY] = a[axis::RZ];
                pad.axes[axis::BRAKE] = half(a[axis::RX]);
                pad.axes[axis::ACCELERATOR] = half(a[axis::RY]);
                pad.axes[axis::Z] = 0;
                pad.axes[axis::RZ] = 0;
                let p = pad.present;
                let has = |x: usize| p & (1 << x) != 0;
                let mut q = p & !(1 << axis::Z | 1 << axis::RZ | 1 << axis::RX | 1 << axis::RY);
                for (from, to) in
                    [(axis::Z, axis::RX), (axis::RZ, axis::RY), (axis::RX, axis::BRAKE), (axis::RY, axis::ACCELERATOR)]
                {
                    if has(from) {
                        q |= 1 << to;
                    }
                }
                pad.present = q;
            }
        }
        let Some(t) = self.touch else { return };
        let (id, data) = if ids { (raw.first().copied().unwrap_or(0), raw.get(1..).unwrap_or(&[])) } else { (0, raw) };
        if id != t.report_id {
            return;
        }
        let (Some(contact), Some(x), Some(y)) =
            (bit(data, t.at * 8, 8), bit(data, t.at * 8 + 8, 12), bit(data, t.at * 8 + 20, 12))
        else {
            return;
        };
        let pad = g.get_or_insert_with(GamepadInput::default);
        if contact & 0x80 == 0 {
            pad.buttons |= 1 << (TOUCH_BUTTON - 1);
        } else {
            pad.buttons &= !(1 << (TOUCH_BUTTON - 1));
        }
        pad.axes[axis::SLIDER] = gamepad::normalize(x, 0, t.x_max, false);
        pad.axes[axis::DIAL] = gamepad::normalize(y, 0, t.y_max, false);
        pad.present |= 1 << axis::SLIDER | 1 << axis::DIAL;
    }

    /// What a device has, as its descriptor says (`buttons` bit n = button n + 1, `axes` bit per axis), in the
    /// standard order.
    pub fn controls(&self, buttons: u32, axes: u16) -> (u32, u16) {
        let mut g = GamepadInput { buttons, hat: gamepad::HAT_CENTRE, axes: [0; gamepad::AXES], present: axes };
        let touch = Profile { touch: None, ..*self };
        let mut some = Some(g);
        touch.apply(&[], false, &mut some);
        g = some.unwrap_or(g);
        if self.touch.is_some() {
            g.buttons |= 1 << (TOUCH_BUTTON - 1);
            g.present |= 1 << axis::SLIDER | 1 << axis::DIAL;
        }
        (g.buttons, g.present)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symmetric_pad_is_put_in_the_standard_order_and_its_touch_pad_read() {
        let p = profile(0x054C, 0x0CE6, false);
        assert_eq!(p.style, Style::Symmetric);
        // As its descriptor has it: square (1) and Home (13); the right stick on Z/Rz, L2 on Rx half pressed.
        let mut axes = [0; gamepad::AXES];
        axes[axis::Z] = 1000;
        axes[axis::RZ] = -2000;
        axes[axis::RX] = 0;
        axes[axis::RY] = -32767;
        let present = 1 << axis::X | 1 << axis::Y | 1 << axis::Z | 1 << axis::RZ | 1 << axis::RX | 1 << axis::RY;
        let mut g = Some(GamepadInput { buttons: 1 | 1 << 12, hat: 8, axes, present });
        // Report 1 with a finger at x 1919, y 0.
        let mut raw = [0u8; 64];
        raw[0] = 1;
        raw[1 + 32] = 0x01;
        raw[1 + 33] = 0x7F;
        raw[1 + 34] = 0x07;
        p.apply(&raw, true, &mut g);
        let g = g.unwrap();
        assert_eq!(g.buttons, 1 << 2 | 1 << 16 | 1 << (TOUCH_BUTTON - 1), "X (left), Home, the touch");
        assert_eq!((g.axes[axis::RX], g.axes[axis::RY]), (1000, -2000));
        assert_eq!((g.axes[axis::BRAKE], g.axes[axis::ACCELERATOR]), (16383, 0));
        assert_eq!((g.axes[axis::SLIDER], g.axes[axis::DIAL]), (32767, -32767));
        assert_eq!(g.present & (1 << axis::Z | 1 << axis::RZ), 0);
        // No finger: the contact lets go.
        raw[1 + 32] = 0x81;
        let mut again = Some(g);
        p.apply(&raw, true, &mut again);
        assert_eq!(again.unwrap().buttons & 1 << (TOUCH_BUTTON - 1), 0);
        // What it has: 15 buttons as the descriptor numbers them, in the standard order, and the touch pad.
        let (b, a) = p.controls(0x7FFF, present);
        assert_eq!(b, 0x0F_FFFF & !(1 << 12 | 1 << 13 | 1 << 14 | 1 << 15));
        assert_eq!(
            a,
            1 << axis::X
                | 1 << axis::Y
                | 1 << axis::RX
                | 1 << axis::RY
                | 1 << axis::BRAKE
                | 1 << axis::ACCELERATOR
                | 1 << axis::SLIDER
                | 1 << axis::DIAL
        );
    }

    #[test]
    fn other_pads_keep_their_numbering() {
        assert_eq!(profile(0x045E, 0x0719, true).style, Style::Offset);
        let p = profile(0x1234, 0x5678, false);
        assert_eq!(p.style, Style::Generic);
        let before = GamepadInput { buttons: 5, ..GamepadInput::default() };
        let mut g = Some(before);
        p.apply(&[1, 2, 3], true, &mut g);
        assert_eq!(g, Some(before));
        assert_eq!(Style::from_name("symmetric"), Some(Style::Symmetric));
    }
}
