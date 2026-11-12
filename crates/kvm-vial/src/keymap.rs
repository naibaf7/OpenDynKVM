//! Layers, macros and settings as Vial edits them, and as flash items (`kvm_core::store`).
//!
//! Keycodes are kept exactly as Vial wrote them, so that every keycode reads back unchanged, including the ones the
//! engine does not act on.
//!
//! | Item | Key | Contents |
//! |---|---|---|
//! | settings | 0x0100 | version 1, layout options u32, default layer, tapping term u16, flags, one-shot timeout u16, tap code delay u16 |
//! | macros | 0x0101 | the macro buffer up to its last used byte |
//! | layer n | 0x0110 + n | version 1, then the keycode of every position, u16 LE |

use crate::keycode::{KC_NO, KC_TRNS};
use crate::{LAYERS, POSITIONS, in_layout};

pub const MACRO_COUNT: u8 = 16;
pub const MACRO_BUFFER: usize = 2048;

pub mod item {
    pub const SETTINGS: u16 = 0x0100;
    pub const MACROS: u16 = 0x0101;
    pub const LAYER: u16 = 0x0110;
}

const VERSION: u8 = 1;
const DIRTY_MACROS: u16 = 1 << 8;
const DIRTY_SETTINGS: u16 = 1 << 9;
pub const LAYER_ITEM_LEN: usize = 1 + 2 * POSITIONS;

/// QMK settings (Vial's "QMK Settings" tab) the engine uses, by qsid.
pub mod qsid {
    pub const ONESHOT_TIMEOUT: u16 = 6;
    pub const TAPPING_TERM: u16 = 7;
    pub const TAP_CODE_DELAY: u16 = 18;
    pub const PERMISSIVE_HOLD: u16 = 22;
    pub const HOLD_ON_OTHER_KEY_PRESS: u16 = 23;
    pub const ALL: [u16; 5] = [ONESHOT_TIMEOUT, TAPPING_TERM, TAP_CODE_DELAY, PERMISSIVE_HOLD, HOLD_ON_OTHER_KEY_PRESS];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Settings {
    pub layout_options: u32,
    pub default_layer: u8,
    pub tapping_term_ms: u16,
    pub permissive_hold: bool,
    pub hold_on_other_key_press: bool,
    /// 0 = one-shot keys wait for the next key however long it takes (QMK's default).
    pub oneshot_timeout_ms: u16,
    pub tap_code_delay_ms: u16,
}

impl Default for Settings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        layout_options: 0,
        default_layer: 0,
        tapping_term_ms: 200,
        permissive_hold: false,
        hold_on_other_key_press: false,
        oneshot_timeout_ms: 0,
        tap_code_delay_ms: 0,
    };

    /// Value of a QMK setting (little-endian, its width).
    pub fn get(&self, qsid: u16, out: &mut [u8]) -> Option<usize> {
        let (v, width) = match qsid {
            qsid::ONESHOT_TIMEOUT => (self.oneshot_timeout_ms as u32, 2),
            qsid::TAPPING_TERM => (self.tapping_term_ms as u32, 2),
            qsid::TAP_CODE_DELAY => (self.tap_code_delay_ms as u32, 2),
            qsid::PERMISSIVE_HOLD => (self.permissive_hold as u32, 1),
            qsid::HOLD_ON_OTHER_KEY_PRESS => (self.hold_on_other_key_press as u32, 1),
            _ => return None,
        };
        out.get_mut(..width)?.copy_from_slice(&v.to_le_bytes()[..width]);
        Some(width)
    }

    pub fn set(&mut self, qsid: u16, v: &[u8]) -> bool {
        let u16_at = || v.get(..2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        match qsid {
            qsid::ONESHOT_TIMEOUT => self.oneshot_timeout_ms = u16_at().unwrap_or(0),
            qsid::TAPPING_TERM => self.tapping_term_ms = u16_at().unwrap_or(200).clamp(10, 10000),
            qsid::TAP_CODE_DELAY => self.tap_code_delay_ms = u16_at().unwrap_or(0).min(1000),
            qsid::PERMISSIVE_HOLD => self.permissive_hold = v.first().is_some_and(|b| *b != 0),
            qsid::HOLD_ON_OTHER_KEY_PRESS => self.hold_on_other_key_press = v.first().is_some_and(|b| *b != 0),
            _ => return false,
        }
        true
    }

    fn encode(&self, out: &mut [u8]) -> usize {
        let flags = self.permissive_hold as u8 | (self.hold_on_other_key_press as u8) << 1;
        let mut b = [0u8; 13];
        b[0] = VERSION;
        b[1..5].copy_from_slice(&self.layout_options.to_le_bytes());
        b[5] = self.default_layer;
        b[6..8].copy_from_slice(&self.tapping_term_ms.to_le_bytes());
        b[8] = flags;
        b[9..11].copy_from_slice(&self.oneshot_timeout_ms.to_le_bytes());
        b[11..13].copy_from_slice(&self.tap_code_delay_ms.to_le_bytes());
        out[..b.len()].copy_from_slice(&b);
        b.len()
    }

    fn decode(d: &[u8]) -> Option<Settings> {
        if d.len() < 13 || d[0] != VERSION {
            return None;
        }
        let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        Some(Settings {
            layout_options: u32::from_le_bytes([d[1], d[2], d[3], d[4]]),
            default_layer: d[5].min(LAYERS as u8 - 1),
            tapping_term_ms: u16_at(6).clamp(10, 10000),
            permissive_hold: d[8] & 1 != 0,
            hold_on_other_key_press: d[8] & 2 != 0,
            oneshot_timeout_ms: u16_at(9),
            tap_code_delay_ms: u16_at(11).min(1000),
        })
    }
}

pub struct Keymap {
    layers: [[u16; POSITIONS]; LAYERS],
    macros: [u8; MACRO_BUFFER],
    pub settings: Settings,
    /// Bit n: layer n; then macros and settings.
    dirty: u16,
    /// Counts every change (for an autosave that waits for the last one).
    changes: u32,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::new()
    }
}

/// The keycode that reproduces the key at `pos` (layer 0 of a fresh keymap).
const fn identity(pos: usize) -> u16 {
    if in_layout(pos as u8) { pos as u16 } else { KC_NO }
}

impl Keymap {
    /// Every key does what it says on layer 0; layers 1–7 are transparent.
    pub const fn new() -> Self {
        let mut layers = [[KC_TRNS; POSITIONS]; LAYERS];
        let mut p = 0;
        while p < POSITIONS {
            layers[0][p] = identity(p);
            p += 1;
        }
        Keymap { layers, macros: [0; MACRO_BUFFER], settings: Settings::DEFAULT, dirty: 0, changes: 0 }
    }

    /// Back to [`Keymap::new`], and every item due to be written.
    pub fn reset(&mut self) {
        let changes = self.changes;
        *self = Keymap { changes, ..Keymap::new() };
        self.mark(((1 << LAYERS) - 1) | DIRTY_MACROS | DIRTY_SETTINGS);
    }

    fn mark(&mut self, bits: u16) {
        self.dirty |= bits;
        self.changes = self.changes.wrapping_add(1);
    }

    /// Changes so far: compare two readings to see whether anything changed in between.
    pub fn changes(&self) -> u32 {
        self.changes
    }

    /// Clears everything but the layers (QMK's macro reset and settings reset come separately).
    pub fn reset_layers(&mut self) {
        let fresh = Keymap::new();
        self.layers = fresh.layers;
        self.mark((1 << LAYERS) - 1);
    }

    pub fn get(&self, layer: usize, pos: usize) -> u16 {
        self.layers.get(layer).and_then(|l| l.get(pos)).copied().unwrap_or(KC_NO)
    }

    pub fn set(&mut self, layer: usize, pos: usize, kc: u16) {
        if let Some(slot) = self.layers.get_mut(layer).and_then(|l| l.get_mut(pos))
            && *slot != kc
        {
            *slot = kc;
            self.mark(1 << layer);
        }
    }

    /// Whether every layer is as [`Keymap::new`] made it (then the keymap changes nothing).
    pub fn is_identity(&self) -> bool {
        self.layers[0].iter().enumerate().all(|(p, kc)| *kc == identity(p))
            && self.layers[1..].iter().all(|l| l.iter().all(|kc| *kc == KC_TRNS))
    }

    pub fn macros(&self) -> &[u8] {
        &self.macros
    }

    /// Macro `n`: the bytes between the n-th and (n+1)-th NUL.
    pub fn macro_bytes(&self, n: u8) -> &[u8] {
        self.macros.split(|b| *b == 0).nth(n as usize).unwrap_or(&[])
    }

    pub fn write_macros(&mut self, offset: usize, data: &[u8]) {
        let end = (offset + data.len()).min(MACRO_BUFFER);
        if offset < end && self.macros[offset..end] != data[..end - offset] {
            self.macros[offset..end].copy_from_slice(&data[..end - offset]);
            self.mark(DIRTY_MACROS);
        }
    }

    pub fn reset_macros(&mut self) {
        self.macros = [0; MACRO_BUFFER];
        self.mark(DIRTY_MACROS);
    }

    pub fn settings_changed(&mut self) {
        self.mark(DIRTY_SETTINGS);
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty != 0
    }

    /// The next changed item, encoded into `out` (at least [`MACRO_BUFFER`] bytes); it counts as written from here.
    pub fn take_dirty(&mut self, out: &mut [u8]) -> Option<(u16, usize)> {
        let bit = self.dirty.trailing_zeros() as u16;
        if self.dirty == 0 {
            return None;
        }
        self.dirty &= !(1 << bit);
        Some(match bit {
            b if (b as usize) < LAYERS => {
                out[0] = VERSION;
                for (i, kc) in self.layers[b as usize].iter().enumerate() {
                    out[1 + 2 * i..3 + 2 * i].copy_from_slice(&kc.to_le_bytes());
                }
                (item::LAYER + b, LAYER_ITEM_LEN)
            }
            8 => {
                let n = self.macros.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
                out[..n].copy_from_slice(&self.macros[..n]);
                (item::MACROS, n)
            }
            _ => (item::SETTINGS, self.settings.encode(out)),
        })
    }

    /// Takes back an item read from flash; unknown or outdated items are ignored.
    pub fn restore(&mut self, key: u16, d: &[u8]) {
        match key {
            item::SETTINGS => {
                if let Some(s) = Settings::decode(d) {
                    self.settings = s;
                }
            }
            item::MACROS => {
                let n = d.len().min(MACRO_BUFFER);
                self.macros = [0; MACRO_BUFFER];
                self.macros[..n].copy_from_slice(&d[..n]);
            }
            k if (item::LAYER..item::LAYER + LAYERS as u16).contains(&k)
                && d.len() == LAYER_ITEM_LEN
                && d[0] == VERSION =>
            {
                let l = &mut self.layers[(k - item::LAYER) as usize];
                for (i, kc) in l.iter_mut().enumerate() {
                    *kc = u16::from_le_bytes([d[1 + 2 * i], d[2 + 2 * i]]);
                }
            }
            _ => {}
        }
    }

    /// Every item key this keymap uses (for loading at boot).
    pub fn item_keys() -> impl Iterator<Item = u16> {
        [item::SETTINGS, item::MACROS].into_iter().chain((0..LAYERS as u16).map(|l| item::LAYER + l))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_round_trip() {
        let mut k = Keymap::new();
        assert!(k.is_identity() && !k.is_dirty());
        k.set(0, 0x39, 0x00E0); // Caps Lock → Left Ctrl
        k.set(2, 0x1E, 0x7701);
        k.write_macros(0, b"hi\0\x01\x01\x04\0");
        k.settings.tapping_term_ms = 175;
        k.settings_changed();
        let mut copy = Keymap::new();
        let mut buf = [0u8; MACRO_BUFFER];
        let mut keys = std::vec::Vec::new();
        while let Some((key, n)) = k.take_dirty(&mut buf) {
            copy.restore(key, &buf[..n]);
            keys.push(key);
        }
        assert_eq!(keys, [item::LAYER, item::LAYER + 2, item::MACROS, item::SETTINGS]);
        assert_eq!(copy.get(0, 0x39), 0x00E0);
        assert_eq!(copy.get(2, 0x1E), 0x7701);
        assert_eq!(copy.macro_bytes(1), b"\x01\x01\x04");
        assert_eq!(copy.settings.tapping_term_ms, 175);
        assert!(!copy.is_identity());
        assert!(!k.is_dirty());
    }

    #[test]
    fn qmk_settings() {
        let mut s = Settings::default();
        let mut out = [0u8; 4];
        assert_eq!(s.get(qsid::TAPPING_TERM, &mut out), Some(2));
        assert_eq!(u16::from_le_bytes([out[0], out[1]]), 200);
        assert!(s.set(qsid::PERMISSIVE_HOLD, &[1]));
        assert_eq!((s.get(qsid::PERMISSIVE_HOLD, &mut out), out[0]), (Some(1), 1));
        assert!(!s.set(99, &[1]));
    }
}
