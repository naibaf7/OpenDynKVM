//! Macros as steps, in Vial's format (protocol 2 on), for the host software that shows and edits them; the firmware
//! plays the bytes as they are (`remap.rs`).
//!
//! The macro buffer ([`MACRO_BUFFER`](crate::keymap::MACRO_BUFFER)) holds [`MACRO_COUNT`](crate::keymap::MACRO_COUNT)
//! macros, each ended by a NUL. A macro is text (typed through the host's layout) and actions: `0x01` then 1 tap,
//! 2 down, 3 up (an 8-bit keycode), 4 a delay (two bytes, each one more than its digit in base 255, so no NUL), 5–7
//! tap, down, up with a 16-bit keycode (little-endian; one whose low byte is 0 is sent as `0xFF00 | high byte`).

use heapless::Vec;

/// One step of a macro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step<'a> {
    /// Characters to type (ASCII; Enter, Tab, Escape, Backspace by their control codes).
    Text(&'a [u8]),
    Tap(u16),
    Down(u16),
    Up(u16),
    /// Milliseconds, at most 65 024 (254 × 256).
    Delay(u16),
}

/// The most a delay step can wait.
pub const DELAY_MAX: u16 = 254 * 256;

/// The steps of one macro's bytes (between two NULs).
pub fn steps(m: &[u8]) -> impl Iterator<Item = Step<'_>> {
    let mut i = 0;
    core::iter::from_fn(move || {
        while i < m.len() {
            if m[i] != 0x01 {
                let start = i;
                while i < m.len() && m[i] != 0x01 {
                    i += 1;
                }
                return Some(Step::Text(&m[start..i]));
            }
            let at = |n: usize| m.get(i + n).copied().unwrap_or(0);
            let code = at(1);
            let step = match code {
                1..=3 => {
                    let kc = at(2) as u16;
                    i += 3;
                    [Step::Tap(kc), Step::Down(kc), Step::Up(kc)][code as usize - 1]
                }
                4 => {
                    let ms = at(2).saturating_sub(1) as u16 + at(3).saturating_sub(1) as u16 * 255;
                    i += 4;
                    Step::Delay(ms)
                }
                5..=7 => {
                    let raw = u16::from_le_bytes([at(2), at(3)]);
                    let kc = if raw & 0xFF00 == 0xFF00 { (raw & 0xFF) << 8 } else { raw };
                    i += 4;
                    [Step::Tap(kc), Step::Down(kc), Step::Up(kc)][code as usize - 5]
                }
                _ => {
                    i += 2;
                    continue;
                }
            };
            return Some(step);
        }
        None
    })
}

/// A macro's bytes for `steps` (without its NUL); None if it doesn't fit `N` or a text step has a NUL or 0x01 in it.
pub fn encode<const N: usize>(steps: &[Step<'_>]) -> Option<Vec<u8, N>> {
    let mut out: Vec<u8, N> = Vec::new();
    for s in steps {
        match *s {
            Step::Text(t) => {
                if t.iter().any(|b| *b == 0 || *b == 0x01) {
                    return None;
                }
                out.extend_from_slice(t).ok()?;
            }
            Step::Tap(kc) | Step::Down(kc) | Step::Up(kc) => {
                let code = match s {
                    Step::Tap(_) => 1,
                    Step::Down(_) => 2,
                    _ => 3,
                };
                if kc == 0 {
                    continue;
                }
                if kc <= 0xFF {
                    // A basic keycode: one byte (codes 1–3), as Vial writes it.
                    out.extend_from_slice(&[0x01, code, kc as u8]).ok()?;
                } else {
                    let raw = if kc & 0xFF == 0 { 0xFF00 | (kc >> 8) } else { kc };
                    out.extend_from_slice(&[0x01, code + 4, raw as u8, (raw >> 8) as u8]).ok()?;
                }
            }
            Step::Delay(ms) => {
                let ms = ms.min(DELAY_MAX);
                out.extend_from_slice(&[0x01, 4, (ms % 255) as u8 + 1, (ms / 255) as u8 + 1]).ok()?;
            }
        }
    }
    Some(out)
}

/// Macro `n`'s bytes in a buffer (empty past the last).
pub fn get(buf: &[u8], n: usize) -> &[u8] {
    buf.split(|b| *b == 0).nth(n).unwrap_or(&[])
}

/// The buffer with macro `n` replaced by `m`, the others unchanged; None if it doesn't fit.
pub fn replace<const N: usize>(buf: &[u8], count: usize, n: usize, m: &[u8]) -> Option<Vec<u8, N>> {
    let mut out: Vec<u8, N> = Vec::new();
    for i in 0..count {
        out.extend_from_slice(if i == n { m } else { get(buf, i) }).ok()?;
        out.push(0).ok()?;
    }
    out.resize(N, 0).ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_round_trip() {
        let s = [Step::Text(b"hi"), Step::Tap(0x7400), Step::Down(0x00E1), Step::Delay(1000), Step::Up(0x00E1)];
        let b: Vec<u8, 64> = encode(&s).unwrap();
        assert!(!b.contains(&0), "no NUL inside a macro");
        assert_eq!(steps(&b).collect::<std::vec::Vec<_>>(), s);
        assert_eq!(steps(&[0x01, 1, 0x04, b'x']).collect::<std::vec::Vec<_>>(), [Step::Tap(4), Step::Text(b"x")]);
        let buf: Vec<u8, 32> = replace(&[b'a', 0, b'b', 0, 0], 3, 1, b"cd").unwrap();
        assert_eq!(&buf[..7], b"a\0cd\0\0\0");
        assert_eq!(get(&buf, 1), b"cd");
    }
}
