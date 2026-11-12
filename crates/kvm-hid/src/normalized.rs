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
