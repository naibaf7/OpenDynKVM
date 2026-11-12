//! "Release all" reports for any layout (software.md §4.4, step 1).
//!
//! Buttons and keys released, relative axes 0, absolute axes centred, hat switches in their null state.

use crate::parser::{Field, Layout, ReportType};
use crate::usage::{gd, id_of, is_centred_axis, page, page_of};

fn fits(field: &Field, v: i64) -> bool {
    let bits = field.bit_size as u32;
    if field.is_signed() {
        let lim = 1i64 << (bits - 1);
        (-lim..lim).contains(&v)
    } else {
        (0..(1i64 << bits)).contains(&v)
    }
}

/// Value just outside the logical range, if the field's bits can hold one.
fn out_of_range(field: &Field) -> Option<i32> {
    let (min, max) = (field.logical_min as i64, field.logical_max as i64);
    [max + 1, min - 1].into_iter().find(|v| fits(field, *v)).map(|v| v as i32)
}

fn neutral_value(field: &Field, element: usize) -> i32 {
    let (min, max) = (field.logical_min, field.logical_max);
    let in_range_or_min = |v: i32| if (min..=max).contains(&v) { v } else { min };
    if field.is_array() {
        // 0 is "no event" whenever it is out of range or maps to usage 0 (reserved).
        if !(min..=max).contains(&0) || field.array_usage(0).is_some_and(|u| id_of(u) == 0) {
            return 0;
        }
        return out_of_range(field).unwrap_or(0);
    }
    if field.is_relative() {
        return 0;
    }
    match field.usage(element) {
        Some(u) if page_of(u) == page::GENERIC_DESKTOP && id_of(u) == gd::HAT_SWITCH => {
            if field.has_null_state() {
                out_of_range(field).unwrap_or(min)
            } else {
                min
            }
        }
        Some(u) if is_centred_axis(u) => ((min as i64 + max as i64) / 2) as i32,
        _ => in_range_or_min(0),
    }
}

/// Writes the neutral payload of input report `id` (without the report ID byte) into `out`; returns its length, or
/// `None` if `out` is too small or the report doesn't exist.
pub fn neutral_report(layout: &Layout, id: u8, out: &mut [u8]) -> Option<usize> {
    let len = layout.report_len(ReportType::Input, id);
    if len == 0 || len > out.len() {
        return None;
    }
    let out = &mut out[..len];
    out.fill(0);
    for f in layout.fields_of(ReportType::Input, id).filter(|f| f.is_readable()) {
        for e in 0..f.count as usize {
            let v = neutral_value(f, e);
            if v != 0 {
                f.write(out, e, v);
            }
        }
    }
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::fixtures::*;

    fn neutral(desc: &[u8], id: u8) -> std::vec::Vec<u8> {
        let l = Layout::parse(desc).unwrap();
        let mut buf = [0xAAu8; 64];
        let n = neutral_report(&l, id, &mut buf).unwrap();
        buf[..n].to_vec()
    }

    #[test]
    fn keyboards_and_mice_are_all_zero() {
        assert_eq!(neutral(BOOT_KEYBOARD, 0), [0; 8]);
        assert_eq!(neutral(GAMING_MOUSE, 2), [0; 8]);
        assert_eq!(neutral(NKRO_KEYBOARD, 6), [0; 32]);
    }

    #[test]
    fn gamepad_centres_axes_and_nulls_the_hat() {
        // 14 buttons + 2 pad bits, hat (4 bits, null = 8) + 4 pad bits, X Y Z Rz at 127.
        assert_eq!(neutral(GAMEPAD, 0), [0, 0, 0x08, 127, 127, 127, 127]);
    }

    #[test]
    fn system_array_with_null_state() {
        assert_eq!(neutral(MEDIA_KEYS, 4), [0]);
        assert_eq!(neutral(MEDIA_KEYS, 3), [0, 0, 0, 0]);
        assert!(neutral_report(&Layout::parse(MEDIA_KEYS).unwrap(), 9, &mut [0; 8]).is_none());
    }
}
