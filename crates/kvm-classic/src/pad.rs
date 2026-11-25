//! The 5 series of the symmetric-stick pads (vendor 0x054C) over Bluetooth Classic. Over Bluetooth it sends a short
//! input report 1 until the host reads its feature report 5 (calibration), then its full report 0x31: an ID, a
//! sequence byte, the 63 bytes its USB input report 1 has after the ID, nine more, and a CRC-32. The module passes
//! that on in the USB form ([`input`]), declared by [`DESCRIPTOR`] (report 1: the sticks and triggers, a counter, the
//! hat, 15 buttons, then 51 vendor bytes, the touch pad's among them; the USB report's last byte, reserved, is left
//! out, so that the report fits one link message), and says the USB IDs ([`VID`], [`PID`]): the
//! KVM reads it then as it reads the pad on a USB port (`kvm_core::pads`: the buttons put in the standard order, the
//! right stick and the triggers moved, the touch pad).
//!
//! The output report 0x31 (on the interrupt channel) sets its light bar and player lights ([`output`]); it carries a
//! CRC-32 too. Both CRCs cover the HID header byte first (0xA1 in, 0xA2 out), then the report up to the CRC.

use kvm_proto::crc::Crc32;

use crate::hid;

/// The USB IDs of the pad's USB form, which pick its profile.
pub const VID: u16 = 0x054C;
pub const PID: u16 = 0x0CE6;
/// Its full input report and output report over Bluetooth, and their length from the ID.
pub const BT_REPORT: u8 = 0x31;
pub const BT_REPORT_LEN: usize = 78;
/// Its short input report (before the feature report is read): ID 1, the sticks, three button bytes, the triggers.
pub const SHORT_REPORT: u8 = 0x01;
const SHORT_REPORT_LEN: usize = 10;
/// The USB form's report 1 after its ID: the USB report's but its last (reserved) byte, as much as one link message
/// holds (`kvm_proto::link::bt::REPORT_MAX`).
pub const USB_LEN: usize = 62;
const _: () = assert!(USB_LEN == kvm_proto::link::bt::REPORT_MAX);
/// The GET_REPORT of feature report 5 (calibration), on the control channel: after it the pad sends full reports.
pub const ASK_FULL: [u8; 2] = [hid::GET_FEATURE, 0x05];

/// Whether `report` (from its ID, CRC last) checks with `header` before it.
pub fn crc_ok(header: u8, report: &[u8]) -> bool {
    let Some(n) = report.len().checked_sub(4) else { return false };
    let mut c = Crc32::new();
    c.update(&[header]);
    c.update(&report[..n]);
    c.finish().to_le_bytes() == report[n..]
}

/// An input report as it came on the interrupt channel (from its ID), in the USB form: the full report's body, or the
/// short report's sticks, triggers and buttons with the rest neutral (no finger on the touch pad).
pub fn input(report: &[u8]) -> Option<[u8; USB_LEN]> {
    let mut usb = [0u8; USB_LEN];
    match *report.first()? {
        BT_REPORT if report.len() >= BT_REPORT_LEN && crc_ok(hid::DATA_INPUT, &report[..BT_REPORT_LEN]) => {
            usb.copy_from_slice(&report[2..2 + USB_LEN]);
        }
        SHORT_REPORT if report.len() >= SHORT_REPORT_LEN => {
            let r = &report[1..SHORT_REPORT_LEN];
            usb[..4].copy_from_slice(&r[..4]);
            usb[4] = r[7];
            usb[5] = r[8];
            usb[7] = r[4];
            usb[8] = r[5];
            // Home and the touch pad's click; the rest of that byte is a counter.
            usb[9] = r[6] & 0x03;
            // Touch points: bit 7 set, nobody touches.
            usb[32] = 0x80;
            usb[36] = 0x80;
        }
        _ => return None,
    }
    Some(usb)
}

/// What an output report sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Output {
    /// The player lights (bit 0 the leftmost of five; 0x04 the middle one).
    pub player: u8,
    /// The light bar's colour.
    pub rgb: [u8; 3],
    /// Fade out the light bar's start-up pulse first (the first report after it connects).
    pub fade_out: bool,
}

/// The output report 0x31 (from its ID) for `o`; `seq` counts reports (its low 4 bits go out).
pub fn output(seq: u8, o: &Output) -> [u8; BT_REPORT_LEN] {
    let mut r = [0u8; BT_REPORT_LEN];
    r[0] = BT_REPORT;
    r[1] = (seq & 0x0F) << 4;
    r[2] = 0x10;
    // The common part (47 bytes) from byte 3: valid flags 0 and 1, the motors, audio, … valid flag 2 at 38, the light
    // bar's setup at 41, the player lights at 43, the colour at 44.
    let c = &mut r[3..3 + 47];
    if o.fade_out {
        c[38] = 1 << 1;
        c[41] = 1 << 1;
    } else {
        c[1] = 1 << 2 | 1 << 4;
        c[43] = o.player & 0x1F;
        c[44..47].copy_from_slice(&o.rgb);
    }
    let mut crc = Crc32::new();
    crc.update(&[hid::DATA_OUTPUT]);
    crc.update(&r[..BT_REPORT_LEN - 4]);
    r[BT_REPORT_LEN - 4..].copy_from_slice(&crc.finish().to_le_bytes());
    r
}

/// The report descriptor of the USB form: input report 1 as the pad's own USB descriptor declares it.
pub const DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x05, // Usage (Game Pad)
    0xA1, 0x01, // Collection (Application)
    0x85, 0x01, //   Report ID (1)
    0x09, 0x30, //   Usage (X)
    0x09, 0x31, //   Usage (Y)
    0x09, 0x32, //   Usage (Z): the right stick, X
    0x09, 0x35, //   Usage (Rz): the right stick, Y
    0x09, 0x33, //   Usage (Rx): the left trigger
    0x09, 0x34, //   Usage (Ry): the right trigger
    0x15, 0x00, //   Logical Minimum (0)
    0x26, 0xFF, 0x00, //   Logical Maximum (255)
    0x75, 0x08, //   Report Size (8)
    0x95, 0x06, //   Report Count (6)
    0x81, 0x02, //   Input (Data, Var, Abs)
    0x06, 0x00, 0xFF, //   Usage Page (Vendor 0xFF00)
    0x09, 0x20, //   Usage (0x20): the counter
    0x95, 0x01, //   Report Count (1)
    0x81, 0x02, //   Input (Data, Var, Abs)
    0x05, 0x01, //   Usage Page (Generic Desktop)
    0x09, 0x39, //   Usage (Hat Switch)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x07, //   Logical Maximum (7)
    0x35, 0x00, //   Physical Minimum (0)
    0x46, 0x3B, 0x01, //   Physical Maximum (315)
    0x65, 0x14, //   Unit (Degrees)
    0x75, 0x04, //   Report Size (4)
    0x95, 0x01, //   Report Count (1)
    0x81, 0x42, //   Input (Data, Var, Abs, Null State)
    0x65, 0x00, //   Unit (None)
    0x05, 0x09, //   Usage Page (Button)
    0x19, 0x01, //   Usage Minimum (1)
    0x29, 0x0F, //   Usage Maximum (15)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x0F, //   Report Count (15)
    0x81, 0x02, //   Input (Data, Var, Abs)
    0x06, 0x00, 0xFF, //   Usage Page (Vendor 0xFF00)
    0x09, 0x21, //   Usage (0x21)
    0x95, 0x0D, //   Report Count (13)
    0x81, 0x02, //   Input (Data, Var, Abs)
    0x09, 0x22, //   Usage (0x22): motion, the touch pad, the rest
    0x26, 0xFF, 0x00, //   Logical Maximum (255)
    0x75, 0x08, //   Report Size (8)
    0x95, 0x33, //   Report Count (51)
    0x81, 0x02, //   Input (Data, Var, Abs)
    0xC0, // End Collection
];

#[cfg(test)]
mod tests {
    use super::*;

    fn full_report(body: &[u8; USB_LEN]) -> [u8; BT_REPORT_LEN] {
        let mut r = [0u8; BT_REPORT_LEN];
        r[0] = BT_REPORT;
        r[1] = 0x20;
        r[2..2 + USB_LEN].copy_from_slice(body);
        let mut c = Crc32::new();
        c.update(&[hid::DATA_INPUT]);
        c.update(&r[..74]);
        r[74..].copy_from_slice(&c.finish().to_le_bytes());
        r
    }

    #[test]
    fn a_full_report_becomes_the_usb_one_and_a_damaged_one_nothing() {
        let mut body = [0u8; USB_LEN];
        body[0] = 0xFF;
        body[7] = 0x18; // hat 8 (centre), square
        body[32] = 0x05; // a finger
        let r = full_report(&body);
        assert_eq!(input(&r), Some(body));
        let mut bad = r;
        bad[10] ^= 1;
        assert_eq!(input(&bad), None);
        assert_eq!(input(&r[..70]), None, "cut short");
    }

    #[test]
    fn a_short_report_reads_too() {
        let r = [SHORT_REPORT, 1, 2, 3, 4, 0x28, 0x01, 0xFD, 9, 10];
        let u = input(&r).unwrap();
        assert_eq!(&u[..6], &[1, 2, 3, 4, 9, 10]);
        assert_eq!((u[7], u[8], u[9], u[32]), (0x28, 0x01, 0x01, 0x80), "the counter's bits dropped, no finger");
    }

    #[test]
    fn the_descriptor_reads_as_the_pads_usb_form() {
        let t = kvm_hid::translate::Translator::new(kvm_hid::parser::Layout::parse(DESCRIPTOR).unwrap());
        let mut body = [0u8; USB_LEN];
        body[0] = 0xFF; // left stick right
        body[4] = 0xFF; // left trigger in
        body[7] = 0x10 | 2; // square, the hat right
        body[9] = 0x01; // Home
        let mut report = std::vec![1u8];
        report.extend_from_slice(&body);
        let g = t.translate(&report).gamepad.unwrap();
        assert_eq!(g.buttons, 1 | 1 << 12, "square is button 1, Home button 13");
        assert_eq!(g.hat, 2);
        assert_eq!(g.axes[kvm_hid::gamepad::axis::X], 32767);
        assert_eq!(g.axes[kvm_hid::gamepad::axis::RX], 32767, "the left trigger, as on USB");
    }

    #[test]
    fn the_output_report_checks() {
        let r = output(3, &Output { player: 0x04, rgb: [0, 0, 40], fade_out: false });
        assert_eq!((r[0], r[1], r[2]), (BT_REPORT, 0x30, 0x10));
        assert_eq!((r[4], r[46], r[47..50].to_vec()), (0x14, 0x04, std::vec![0, 0, 40]));
        assert!(crc_ok(hid::DATA_OUTPUT, &r));
        let f = output(0, &Output { fade_out: true, ..Output::default() });
        assert_eq!((f[41], f[44], f[4]), (0x02, 0x02, 0));
    }
}
