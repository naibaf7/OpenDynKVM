//! The KVM's own interfaces, present on both PCs at all times (software.md §7.1), and their report encoders.

use crate::keyboard::{KeyState, NKRO_LEN};
use crate::mouse::{MouseInput, clamp_i8, clamp_i16};

/// Interface 0: boot keyboard, 8-byte input, 1-byte LED output, no report IDs.
#[rustfmt::skip]
pub const BOOT_KEYBOARD_DESC: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01,             // Generic Desktop / Keyboard, Application
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7,             //   modifiers E0–E7
    0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01,             //   reserved byte
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05,             //   LEDs 1–5
    0x95, 0x05, 0x75, 0x01, 0x91, 0x02,
    0x95, 0x01, 0x75, 0x03, 0x91, 0x01,             //   LED padding
    0x05, 0x07, 0x19, 0x00, 0x2A, 0xFF, 0x00,       //   6 keys, usages 0–255
    0x15, 0x00, 0x26, 0xFF, 0x00, 0x95, 0x06, 0x75, 0x08, 0x81, 0x00,
    0xC0,
];
pub const BOOT_KEYBOARD_LEN: usize = 8;

/// Interface 1: mouse. 16 buttons, 16-bit X/Y, 16-bit wheel and AC Pan, each with a resolution multiplier
/// (feature report, 1 byte). Boot subclass: in boot protocol the PC gets the 3-byte boot format instead.
#[rustfmt::skip]
pub const MOUSE_DESC: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01,             // Generic Desktop / Mouse, Application
    0x09, 0x01, 0xA1, 0x00,                         //   Pointer, Physical
    0x05, 0x09, 0x19, 0x01, 0x29, 0x10,             //     buttons 1–16
    0x15, 0x00, 0x25, 0x01, 0x95, 0x10, 0x75, 0x01, 0x81, 0x02,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31,             //     X, Y: int16 relative
    0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06,
    0xA1, 0x02,                                     //     Logical: wheel + its multiplier
    0x09, 0x48, 0x15, 0x00, 0x25, 0x01, 0x35, 0x01, 0x45, 0x08,
    0x75, 0x02, 0x95, 0x01, 0xB1, 0x02,             //       resolution multiplier 1 or 8
    0x35, 0x00, 0x45, 0x00,
    0x09, 0x38, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x01, 0x81, 0x06,
    0xC0,
    0xA1, 0x02,                                     //     Logical: AC Pan + its multiplier
    0x09, 0x48, 0x15, 0x00, 0x25, 0x01, 0x35, 0x01, 0x45, 0x08,
    0x75, 0x02, 0x95, 0x01, 0xB1, 0x02,
    0x35, 0x00, 0x45, 0x00,
    0x05, 0x0C, 0x0A, 0x38, 0x02, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x01, 0x81, 0x06,
    0xC0,
    0x75, 0x04, 0x95, 0x01, 0xB1, 0x01,             //     feature padding
    0xC0,
    0xC0,
];
pub const MOUSE_LEN: usize = 10;
pub const BOOT_MOUSE_LEN: usize = 3;

/// Report IDs of the mouse interface with the absolute pointer ([`MOUSE_ABS_DESC`]).
pub const MOUSE_REL_ID: u8 = 1;
pub const MOUSE_ABS_ID: u8 = 2;
/// The absolute pointer's range on both axes: the PC maps it onto its screens.
pub const ABS_MAX: u16 = 32767;
/// The largest report of [`MOUSE_ABS_DESC`]: report ID, then 10 bytes.
pub const MOUSE_ABS_LEN: usize = 11;

/// Interface 1 with the absolute pointer (screen hop): report ID 1 is the relative mouse of [`MOUSE_DESC`], report
/// ID 2 an absolute pointer with 16 buttons, X/Y 0–[`ABS_MAX`], wheel and AC Pan. The pointer is a top-level
/// collection of its own and of another usage (Pointer, not Mouse): Linux makes it an input device of its own, as an
/// absolute pointer, and Windows a second mouse.
#[rustfmt::skip]
pub const MOUSE_ABS_DESC: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01,             // Generic Desktop / Mouse, Application
    0x85, MOUSE_REL_ID,
    0x09, 0x01, 0xA1, 0x00,                         //   Pointer, Physical
    0x05, 0x09, 0x19, 0x01, 0x29, 0x10,             //     buttons 1–16
    0x15, 0x00, 0x25, 0x01, 0x95, 0x10, 0x75, 0x01, 0x81, 0x02,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31,             //     X, Y: int16 relative
    0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06,
    0xA1, 0x02,                                     //     Logical: wheel + its multiplier
    0x09, 0x48, 0x15, 0x00, 0x25, 0x01, 0x35, 0x01, 0x45, 0x08,
    0x75, 0x02, 0x95, 0x01, 0xB1, 0x02,             //       resolution multiplier 1 or 8
    0x35, 0x00, 0x45, 0x00,
    0x09, 0x38, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x01, 0x81, 0x06,
    0xC0,
    0xA1, 0x02,                                     //     Logical: AC Pan + its multiplier
    0x09, 0x48, 0x15, 0x00, 0x25, 0x01, 0x35, 0x01, 0x45, 0x08,
    0x75, 0x02, 0x95, 0x01, 0xB1, 0x02,
    0x35, 0x00, 0x45, 0x00,
    0x05, 0x0C, 0x0A, 0x38, 0x02, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x01, 0x81, 0x06,
    0xC0,
    0x75, 0x04, 0x95, 0x01, 0xB1, 0x01,             //     feature padding
    0xC0,
    0xC0,
    0x05, 0x01, 0x09, 0x01, 0xA1, 0x01,             // Generic Desktop / Pointer, Application
    0x85, MOUSE_ABS_ID,
    0x09, 0x01, 0xA1, 0x00,                         //   Pointer, Physical
    0x05, 0x09, 0x19, 0x01, 0x29, 0x10,             //     buttons 1–16
    0x15, 0x00, 0x25, 0x01, 0x95, 0x10, 0x75, 0x01, 0x81, 0x02,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31,             //     X, Y: 0–32767 absolute
    0x15, 0x00, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02,
    0x09, 0x38,                                     //     wheel: int16 relative, in detents
    0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x01, 0x81, 0x06,
    0x05, 0x0C, 0x0A, 0x38, 0x02,                   //     AC Pan
    0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x01, 0x81, 0x06,
    0xC0,
    0xC0,
];
/// Hi-res wheel units per detent when the PC has enabled the resolution multiplier.
pub const WHEEL_MULTIPLIER: i32 = 8;

pub const NKRO_ID: u8 = 1;
pub const CONSUMER_ID: u8 = 2;
pub const SYSTEM_ID: u8 = 3;
/// Usages a consumer report holds at once.
pub const CONSUMER_SLOTS: usize = 4;

/// Interface 2: NKRO keyboard (ID 1), consumer control (ID 2), system control (ID 3).
#[rustfmt::skip]
pub const NKRO_DESC: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, NKRO_ID,
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7,             //   modifiers
    0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
    0x19, 0x00, 0x29, 0xDF, 0x95, 0xE0, 0x81, 0x02, //   bitmap of usages 0x00–0xDF
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x95, 0x05, 0x91, 0x02,
    0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
    0xC0,
    0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, CONSUMER_ID,
    0x19, 0x00, 0x2A, 0xFF, 0x03, 0x15, 0x00, 0x26, 0xFF, 0x03,
    0x75, 0x10, 0x95, CONSUMER_SLOTS as u8, 0x81, 0x00,
    0xC0,
    0x05, 0x01, 0x09, 0x80, 0xA1, 0x01, 0x85, SYSTEM_ID,
    0x09, 0x81, 0x09, 0x82, 0x09, 0x83,             //   power down, sleep, wake up
    0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x03, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x05, 0x81, 0x01,
    0xC0,
];

/// The virtual gamepad (hid_mapping.md §6.1) as report ID 4 of the NKRO interface: 32 buttons, a hat, 8 axes (X, Y the
/// left stick, Rx, Ry the right one, Z, Rz the triggers, slider, dial), each −32767…32767.
pub const GAMEPAD_ID: u8 = 4;
pub const GAMEPAD_LEN: usize = 1 + 4 + 1 + 16;
#[rustfmt::skip]
pub const GAMEPAD_DESC: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x85, GAMEPAD_ID,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x20, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x20, 0x81, 0x02, // 32 buttons
    0x05, 0x01, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07, 0x35, 0x00, 0x46, 0x3B, 0x01, 0x65, 0x14,     // hat, 0–315°
    0x75, 0x04, 0x95, 0x01, 0x81, 0x42,                                                               //   null state
    0x65, 0x00, 0x45, 0x00, 0x81, 0x01,                                                               // 4 bits pad
    0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x33, 0x09, 0x34, 0x09, 0x35, 0x09, 0x36, 0x09, 0x37,
    0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x08, 0x81, 0x02,                         // 8 axes
    0xC0,
];

const fn concat<const N: usize>(a: &[u8], b: &[u8]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut i = 0;
    while i < a.len() {
        out[i] = a[i];
        i += 1;
    }
    while i < N {
        out[i] = b[i - a.len()];
        i += 1;
    }
    out
}

/// The NKRO interface with the virtual gamepad.
pub const NKRO_PAD_DESC: [u8; NKRO_DESC.len() + GAMEPAD_DESC.len()] = concat(NKRO_DESC, GAMEPAD_DESC);

/// The virtual gamepad's state: buttons 1–32 (bit n = button n + 1), the d-pad (bits up, right, down, left) and the axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Pad {
    pub buttons: u32,
    pub dpad: u8,
    pub axes: [i16; 8],
}

pub fn gamepad_report(p: &Pad) -> [u8; GAMEPAD_LEN] {
    let mut r = [0u8; GAMEPAD_LEN];
    r[0] = GAMEPAD_ID;
    r[1..5].copy_from_slice(&p.buttons.to_le_bytes());
    // The hat's null state is any value past 7.
    r[5] = crate::gamepad::GamepadInput::hat_of(p.dpad) & 0x0F;
    for (i, a) in p.axes.iter().enumerate() {
        r[6 + 2 * i..8 + 2 * i].copy_from_slice(&a.max(&-32767).to_le_bytes());
    }
    r
}

/// The virtual gamepad as an XInput controller (the wired Xbox 360 protocol's 20-byte input report). Its buttons are
/// numbered as the standard gamepad's (W3C): 1 A, 2 B, 3 X, 4 Y, 5 LB, 6 RB, 7 LT, 8 RT, 9 Back, 10 Start, 11 LS,
/// 12 RS, 13–16 the d-pad up, down, left, right, 17 Home; the axes X, Y the left stick, Rx, Ry the right one, Z, Rz
/// the triggers (from 0 up).
pub const XINPUT_LEN: usize = 20;

pub fn xinput_report(p: &Pad) -> [u8; XINPUT_LEN] {
    let b = |n: u32| p.buttons >> (n - 1) & 1 != 0;
    let dpad = p.dpad | u8::from(b(13)) | u8::from(b(16)) << 1 | u8::from(b(14)) << 2 | u8::from(b(15)) << 3;
    let mut w: u16 = 0;
    for (bit, on) in [
        (0, dpad & 1 != 0),
        (1, dpad & 4 != 0),
        (2, dpad & 8 != 0),
        (3, dpad & 2 != 0),
        (4, b(10)),
        (5, b(9)),
        (6, b(11)),
        (7, b(12)),
        (8, b(5)),
        (9, b(6)),
        (10, b(17)),
        (12, b(1)),
        (13, b(2)),
        (14, b(3)),
        (15, b(4)),
    ] {
        w |= u16::from(on) << bit;
    }
    let trigger = |v: i16, button: bool| if button { 255 } else { (v.max(0) >> 7) as u8 };
    let stick = |v: i16| v.max(-32767);
    let mut r = [0u8; XINPUT_LEN];
    r[1] = XINPUT_LEN as u8;
    r[2..4].copy_from_slice(&w.to_le_bytes());
    r[4] = trigger(p.axes[2], b(7));
    r[5] = trigger(p.axes[5], b(8));
    // XInput's Y axes point up.
    for (i, v) in [stick(p.axes[0]), -stick(p.axes[1]), stick(p.axes[3]), -stick(p.axes[4])].iter().enumerate() {
        r[6 + 2 * i..8 + 2 * i].copy_from_slice(&v.to_le_bytes());
    }
    r
}

/// Interface 3: KVM control, vendor page 0xFF4B, 64-byte input and output reports.
#[rustfmt::skip]
pub const CONTROL_DESC: &[u8] = &[
    0x06, 0x4B, 0xFF, 0x09, 0x01, 0xA1, 0x01,
    0x09, 0x01, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x81, 0x02,
    0x09, 0x02, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x40, 0x91, 0x02,
    0xC0,
];
pub const CONTROL_LEN: usize = 64;

/// System control bits of report ID 3.
pub mod system {
    pub const POWER_DOWN: u8 = 1 << 0;
    pub const SLEEP: u8 = 1 << 1;
    pub const WAKE_UP: u8 = 1 << 2;
}

pub fn boot_keyboard_report(keys: &KeyState) -> [u8; BOOT_KEYBOARD_LEN] {
    keys.to_boot()
}

pub fn nkro_report(keys: &KeyState) -> [u8; 1 + NKRO_LEN] {
    let mut r = [0u8; 1 + NKRO_LEN];
    r[0] = NKRO_ID;
    r[1..].copy_from_slice(&keys.to_nkro());
    r
}

pub fn consumer_report(usages: &[u16; CONSUMER_SLOTS]) -> [u8; 1 + 2 * CONSUMER_SLOTS] {
    let mut r = [0u8; 1 + 2 * CONSUMER_SLOTS];
    r[0] = CONSUMER_ID;
    for (i, u) in usages.iter().enumerate() {
        r[1 + 2 * i..3 + 2 * i].copy_from_slice(&u.to_le_bytes());
    }
    r
}

pub fn system_report(bits: u8) -> [u8; 2] {
    [SYSTEM_ID, bits & 0x07]
}

/// Mouse feature report → (wheel multiplier on, pan multiplier on).
pub fn mouse_multipliers(feature: u8) -> (bool, bool) {
    (feature & 0x03 != 0, feature & 0x0C != 0)
}

/// Report-protocol mouse report. `wheel` / `pan` are in detents; they are scaled when the PC enabled the multiplier.
pub fn mouse_report(m: &MouseInput, wheel_hires: bool, pan_hires: bool) -> [u8; MOUSE_LEN] {
    let scale = |v: i32, hires: bool| if hires { v.saturating_mul(WHEEL_MULTIPLIER) } else { v };
    let mut r = [0u8; MOUSE_LEN];
    r[0..2].copy_from_slice(&m.buttons.to_le_bytes());
    r[2..4].copy_from_slice(&clamp_i16(m.x).to_le_bytes());
    r[4..6].copy_from_slice(&clamp_i16(m.y).to_le_bytes());
    r[6..8].copy_from_slice(&clamp_i16(scale(m.wheel, wheel_hires)).to_le_bytes());
    r[8..10].copy_from_slice(&clamp_i16(scale(m.pan, pan_hires)).to_le_bytes());
    r
}

/// The relative report of [`MOUSE_ABS_DESC`]: [`mouse_report`] after its report ID.
pub fn mouse_rel_report(m: &MouseInput, wheel_hires: bool, pan_hires: bool) -> [u8; MOUSE_ABS_LEN] {
    let mut r = [0u8; MOUSE_ABS_LEN];
    r[0] = MOUSE_REL_ID;
    r[1..].copy_from_slice(&mouse_report(m, wheel_hires, pan_hires));
    r
}

/// The absolute report of [`MOUSE_ABS_DESC`]: buttons, the position (0–[`ABS_MAX`]), wheel and pan in detents.
pub fn mouse_abs_report(buttons: u16, at: [u16; 2], wheel: i32, pan: i32) -> [u8; MOUSE_ABS_LEN] {
    let mut r = [0u8; MOUSE_ABS_LEN];
    r[0] = MOUSE_ABS_ID;
    r[1..3].copy_from_slice(&buttons.to_le_bytes());
    r[3..5].copy_from_slice(&at[0].min(ABS_MAX).to_le_bytes());
    r[5..7].copy_from_slice(&at[1].min(ABS_MAX).to_le_bytes());
    r[7..9].copy_from_slice(&clamp_i16(wheel).to_le_bytes());
    r[9..11].copy_from_slice(&clamp_i16(pan).to_le_bytes());
    r
}

pub fn boot_mouse_report(m: &MouseInput) -> [u8; BOOT_MOUSE_LEN] {
    [(m.buttons & 0x07) as u8, clamp_i8(m.x) as u8, clamp_i8(m.y) as u8]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{Layout, ReportType};
    use crate::usage::{gd, usage};

    #[test]
    fn descriptors_parse_to_the_documented_report_sizes() {
        let kb = Layout::parse(BOOT_KEYBOARD_DESC).unwrap();
        assert_eq!(kb.report_len(ReportType::Input, 0), BOOT_KEYBOARD_LEN);
        assert_eq!(kb.report_len(ReportType::Output, 0), 1);

        let ms = Layout::parse(MOUSE_DESC).unwrap();
        assert_eq!(ms.report_len(ReportType::Input, 0), MOUSE_LEN);
        assert_eq!(ms.report_len(ReportType::Feature, 0), 1);
        let mult: std::vec::Vec<_> = ms
            .fields
            .iter()
            .filter(|f| f.usage(0) == Some(usage(1, gd::RESOLUTION_MULTIPLIER)))
            .map(|f| (f.physical_min, f.physical_max))
            .collect();
        assert_eq!(mult, [(1, WHEEL_MULTIPLIER), (1, WHEEL_MULTIPLIER)]);

        let abs = Layout::parse(MOUSE_ABS_DESC).unwrap();
        assert!(abs.uses_report_ids);
        assert_eq!(abs.report_len(ReportType::Input, MOUSE_REL_ID), MOUSE_LEN);
        assert_eq!(abs.report_len(ReportType::Input, MOUSE_ABS_ID), MOUSE_ABS_LEN - 1);
        assert_eq!(abs.report_len(ReportType::Feature, MOUSE_REL_ID), 1);
        assert_eq!(
            abs.applications.as_slice(),
            &[usage(1, gd::MOUSE), usage(1, gd::POINTER)],
            "the pointer is a collection of its own and of another usage"
        );

        let pad = Layout::parse(&NKRO_PAD_DESC).unwrap();
        assert_eq!(pad.report_len(crate::ReportType::Input, GAMEPAD_ID), GAMEPAD_LEN - 1);
        let t = crate::Translator::new(pad);
        let p = Pad { buttons: 1 << 4 | 1, dpad: 0b0011, axes: [100, -200, 0, 0, 0, 32767, 0, -32768] };
        let g = t.translate(&gamepad_report(&p)).gamepad.unwrap();
        assert_eq!(
            (g.buttons, g.hat, g.axes[0], g.axes[1], g.axes[5], g.axes[7]),
            (p.buttons, 1, 100, -200, 32767, -32767)
        );
        let x = xinput_report(&p);
        assert_eq!((x[1], u16::from_le_bytes([x[2], x[3]]), x[5]), (20, 1 << 12 | 1 << 8 | 1 | 1 << 3, 255));
        assert_eq!(i16::from_le_bytes([x[8], x[9]]), 200, "Y points up");
        let nk = Layout::parse(NKRO_DESC).unwrap();
        assert_eq!(nk.report_len(ReportType::Input, NKRO_ID), NKRO_LEN);
        assert_eq!(nk.report_len(ReportType::Input, CONSUMER_ID), 2 * CONSUMER_SLOTS);
        assert_eq!(nk.report_len(ReportType::Input, SYSTEM_ID), 1);

        let ctl = Layout::parse(CONTROL_DESC).unwrap();
        assert_eq!(ctl.report_len(ReportType::Input, 0), CONTROL_LEN);
        assert_eq!(ctl.report_len(ReportType::Output, 0), CONTROL_LEN);
    }

    #[test]
    fn mouse_report_roundtrips_through_its_own_descriptor() {
        let layout = Layout::parse(MOUSE_DESC).unwrap();
        let t = crate::translate::Translator::new(layout);
        let m = MouseInput { buttons: 0x8001, x: -1234, y: 99, wheel: -2, pan: 1 };
        let back = t.translate(&mouse_report(&m, false, false)).mouse.unwrap();
        assert_eq!(back, m);
        let hires = t.translate(&mouse_report(&m, true, true)).mouse.unwrap();
        assert_eq!((hires.wheel, hires.pan), (-2 * WHEEL_MULTIPLIER, WHEEL_MULTIPLIER));
        assert_eq!(boot_mouse_report(&MouseInput { x: 500, ..m }), [0x01, 127, 99]);

        // With the absolute pointer, the relative report keeps its layout behind report ID 1.
        let t = crate::translate::Translator::new(Layout::parse(MOUSE_ABS_DESC).unwrap());
        assert_eq!(t.translate(&mouse_rel_report(&m, false, false)).mouse.unwrap(), m);
        let abs = mouse_abs_report(0x0003, [0, 40000], 1, -1);
        assert_eq!(abs, [MOUSE_ABS_ID, 3, 0, 0, 0, 0xFF, 0x7F, 1, 0, 0xFF, 0xFF]);
    }
}
