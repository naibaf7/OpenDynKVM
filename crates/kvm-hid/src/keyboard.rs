//! Keyboard state: modifier byte + set of pressed keyboard-page usages.

use crate::usage::key;

/// Bytes of an NKRO report payload: modifiers + bitmap of usages 0x00–0xDF.
pub const NKRO_LEN: usize = 1 + 28;
/// Highest usage the NKRO bitmap carries (0xE0.. are the modifiers).
pub const NKRO_MAX_USAGE: u8 = 0xDF;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct KeyState {
    pub modifiers: u8,
    /// Bit per usage 0x00–0xFF; modifier usages (0xE0–0xE7) are kept in `modifiers` instead.
    bits: [u32; 8],
}

impl core::fmt::Debug for KeyState {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "KeyState(mods={:#04x}, keys=", self.modifiers)?;
        f.debug_list().entries(self.keys().map(|k| crate::keys::name_of(k).unwrap_or("?"))).finish()?;
        write!(f, ")")
    }
}

impl KeyState {
    pub const EMPTY: KeyState = KeyState { modifiers: 0, bits: [0; 8] };

    fn is_modifier(usage: u8) -> bool {
        (key::LEFT_CTRL..=key::RIGHT_GUI).contains(&usage)
    }

    pub fn press(&mut self, usage: u8) {
        if Self::is_modifier(usage) {
            self.modifiers |= 1 << (usage - key::LEFT_CTRL);
        } else if usage > 3 {
            // 0 = no event, 1–3 = error codes.
            self.bits[usage as usize / 32] |= 1 << (usage % 32);
        }
    }

    pub fn release(&mut self, usage: u8) {
        if Self::is_modifier(usage) {
            self.modifiers &= !(1 << (usage - key::LEFT_CTRL));
        } else {
            self.bits[usage as usize / 32] &= !(1 << (usage % 32));
        }
    }

    pub fn is_pressed(&self, usage: u8) -> bool {
        if Self::is_modifier(usage) {
            self.modifiers & (1 << (usage - key::LEFT_CTRL)) != 0
        } else {
            self.bits[usage as usize / 32] & (1 << (usage % 32)) != 0
        }
    }

    /// Pressed non-modifier keys, ascending.
    pub fn keys(&self) -> impl Iterator<Item = u8> + '_ {
        (0..=255u8).filter(|&u| !Self::is_modifier(u) && self.is_pressed(u))
    }

    pub fn key_count(&self) -> usize {
        self.bits.iter().map(|w| w.count_ones() as usize).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.modifiers == 0 && self.bits.iter().all(|w| *w == 0)
    }

    pub fn union(&self, other: &KeyState) -> KeyState {
        let mut out = *self;
        out.modifiers |= other.modifiers;
        for (a, b) in out.bits.iter_mut().zip(other.bits) {
            *a |= b;
        }
        out
    }

    pub fn intersect(&self, other: &KeyState) -> KeyState {
        let mut out = *self;
        out.modifiers &= other.modifiers;
        for (a, b) in out.bits.iter_mut().zip(other.bits) {
            *a &= b;
        }
        out
    }

    /// `self` with every key and modifier in `mask` removed.
    pub fn without(&self, mask: &KeyState) -> KeyState {
        let mut out = *self;
        out.modifiers &= !mask.modifiers;
        for (a, b) in out.bits.iter_mut().zip(mask.bits) {
            *a &= !b;
        }
        out
    }

    /// Boot-protocol report: modifiers, reserved, six keys. More than six keys gives the phantom state
    /// (ErrorRollOver in every slot), as HID 1.11 appendix C requires.
    pub fn to_boot(&self) -> [u8; 8] {
        let mut r = [0u8; 8];
        r[0] = self.modifiers;
        if self.key_count() > 6 {
            r[2..].fill(key::ERROR_ROLL_OVER);
        } else {
            for (slot, k) in r[2..].iter_mut().zip(self.keys()) {
                *slot = k;
            }
        }
        r
    }

    pub fn from_boot(report: &[u8]) -> KeyState {
        let mut s = KeyState { modifiers: report.first().copied().unwrap_or(0), ..KeyState::EMPTY };
        for &k in report.iter().skip(2).take(6) {
            s.press(k);
        }
        s
    }

    /// NKRO payload (without the report ID): modifiers, then one bit per usage 0x00–0xDF.
    pub fn to_nkro(&self) -> [u8; NKRO_LEN] {
        let mut r = [0u8; NKRO_LEN];
        r[0] = self.modifiers;
        for k in self.keys().filter(|&k| k <= NKRO_MAX_USAGE) {
            r[1 + k as usize / 8] |= 1 << (k % 8);
        }
        r
    }
}

/// Keyboard LED output report bits.
pub mod led {
    pub const NUM_LOCK: u8 = 1 << 0;
    pub const CAPS_LOCK: u8 = 1 << 1;
    pub const SCROLL_LOCK: u8 = 1 << 2;
    pub const COMPOSE: u8 = 1 << 3;
    pub const KANA: u8 = 1 << 4;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn press_release_and_set_ops() {
        let mut s = KeyState::EMPTY;
        s.press(0x04);
        s.press(0xE1);
        s.press(0x01); // error code: ignored
        assert!(s.is_pressed(0x04) && s.is_pressed(0xE1) && !s.is_pressed(0x01));
        assert_eq!(s.modifiers, 0x02);
        assert_eq!(s.keys().collect::<std::vec::Vec<_>>(), [0x04]);
        let mut held = KeyState::EMPTY;
        held.press(0x04);
        assert_eq!(s.without(&held).keys().count(), 0);
        assert_eq!(s.without(&held).modifiers, 0x02);
        s.release(0xE1);
        s.release(0x04);
        assert!(s.is_empty());
    }

    #[test]
    fn boot_and_nkro_encoding() {
        let mut s = KeyState::EMPTY;
        for k in [0x04, 0x05, 0x47] {
            s.press(k);
        }
        s.press(0xE0);
        assert_eq!(s.to_boot(), [0x01, 0, 0x04, 0x05, 0x47, 0, 0, 0]);
        assert_eq!(KeyState::from_boot(&s.to_boot()), s);
        let n = s.to_nkro();
        assert_eq!(n[0], 0x01);
        assert_eq!(n[1 + 0x47 / 8], 1 << (0x47 % 8));
        for k in 0x10..0x18 {
            s.press(k);
        }
        assert_eq!(&s.to_boot()[2..], &[1; 6]);
    }
}
