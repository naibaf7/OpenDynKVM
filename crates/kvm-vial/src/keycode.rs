//! QMK keycodes as Vial stores them (protocol 6, QMK 0.19+ numbering) and what each does here.
//!
//! The ranges and their decoding follow RMK's `host/via/keycode_convert.rs` (`from_via_keycode`); the media and system
//! keys follow QMK's `KEYCODE2CONSUMER` / `KEYCODE2SYSTEM`.

pub const KC_NO: u16 = 0x0000;
pub const KC_TRNS: u16 = 0x0001;
pub const QK_BOOT: u16 = 0x7C00;

/// Normalized system-control bits (`kvm_hid::normalized::system`).
pub mod system {
    pub use kvm_hid::normalized::system::{POWER_DOWN, SLEEP, WAKE_UP};
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Tap {
    /// HID usage plus HID modifier bits.
    Key(u8, u8),
    ToggleLayer(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Hold {
    Mods(u8),
    Layer(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Mouse {
    Up,
    Down,
    Left,
    Right,
    WheelUp,
    WheelDown,
    WheelLeft,
    WheelRight,
    /// Fixed speeds 0–2 while held.
    Accel(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Layer {
    /// MO: on while held.
    Momentary(u8),
    /// LM: on while held, with modifiers.
    WithMods(u8, u8),
    /// TG
    Toggle(u8),
    /// TO: this layer only (plus the default layer).
    To(u8),
    /// DF / PDF: the default layer (PDF also stores it).
    Default(u8, bool),
    /// OSL
    OneShot(u8),
    /// QK_TRI_LAYER_LOWER / UPPER: layers 1 / 2, and 3 while both are held.
    TriLower,
    TriUpper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Action {
    No,
    Transparent,
    /// HID usage (0 = modifiers only) plus HID modifier bits.
    Key(u8, u8),
    Consumer(u16),
    /// Normalized system-control bit.
    System(u8),
    /// Mouse button 0–7.
    Button(u8),
    Mouse(Mouse),
    Layer(Layer),
    OneShotMods(u8),
    TapHold(Tap, Hold),
    Macro(u8),
    /// Escape, or ` while Shift or GUI is held.
    GraveEscape,
    /// Recognised but not done here (tap dance, RGB, bootloader, …).
    Unsupported,
}

/// QMK's 5-bit modifier field (Ctrl, Shift, Alt, GUI, right-hand flag) as HID modifier bits.
pub const fn hid_mods(m5: u16) -> u8 {
    let m = (m5 & 0x0F) as u8;
    if m5 & 0x10 != 0 { m << 4 } else { m }
}

/// HID modifier bits as QMK's 5-bit field (right-hand if only right modifiers are set).
pub const fn qmk_mods(hid: u8) -> u16 {
    if hid & 0x0F == 0 && hid != 0 { 0x10 | (hid >> 4) as u16 } else { (hid & 0x0F) as u16 }
}

/// Consumer usage of QMK's media keycodes 0xA8–0xC2 (system keys 0xA5–0xA7 are separate).
fn consumer(kc: u16) -> Option<u16> {
    Some(match kc {
        0xA8 => 0x00E2, // Mute
        0xA9 => 0x00E9, // Volume Up
        0xAA => 0x00EA, // Volume Down
        0xAB => 0x00B5, // Next Track
        0xAC => 0x00B6, // Previous Track
        0xAD => 0x00B7, // Stop
        0xAE => 0x00CD, // Play/Pause
        0xAF => 0x0183, // Media Select (AL Consumer Control Configuration)
        0xB0 => 0x00CC, // Stop/Eject
        0xB1 => 0x018A, // Mail
        0xB2 => 0x0192, // Calculator
        0xB3 => 0x0194, // My Computer (AL Local Machine Browser)
        0xB4 => 0x0221, // WWW Search
        0xB5 => 0x0223, // WWW Home
        0xB6 => 0x0224, // WWW Back
        0xB7 => 0x0225, // WWW Forward
        0xB8 => 0x0226, // WWW Stop
        0xB9 => 0x0227, // WWW Refresh
        0xBA => 0x022A, // WWW Favorites
        0xBB => 0x00B3, // Fast Forward
        0xBC => 0x00B4, // Rewind
        0xBD => 0x006F, // Brightness Up
        0xBE => 0x0070, // Brightness Down
        0xBF => 0x019F, // Control Panel
        0xC0 => 0x01CB, // Assistant
        0xC1 => 0x029F, // Mission Control (AC Desktop Show All Windows)
        0xC2 => 0x02A0, // Launchpad (AC Soft Key Left)
        _ => return None,
    })
}

/// A basic keycode (0x00–0xFF) as an action.
fn basic(kc: u16) -> Action {
    match kc {
        KC_NO => Action::No,
        KC_TRNS => Action::Transparent,
        0x04..=0xA4 | 0xE0..=0xE7 => Action::Key(kc as u8, 0),
        0xA5 => Action::System(system::POWER_DOWN),
        0xA6 => Action::System(system::SLEEP),
        0xA7 => Action::System(system::WAKE_UP),
        0xCD => Action::Mouse(Mouse::Up),
        0xCE => Action::Mouse(Mouse::Down),
        0xCF => Action::Mouse(Mouse::Left),
        0xD0 => Action::Mouse(Mouse::Right),
        0xD1..=0xD8 => Action::Button((kc - 0xD1) as u8),
        0xD9 => Action::Mouse(Mouse::WheelUp),
        0xDA => Action::Mouse(Mouse::WheelDown),
        0xDB => Action::Mouse(Mouse::WheelLeft),
        0xDC => Action::Mouse(Mouse::WheelRight),
        0xDD..=0xDF => Action::Mouse(Mouse::Accel((kc - 0xDD) as u8)),
        _ => consumer(kc).map_or(Action::Unsupported, Action::Consumer),
    }
}

/// The HID usage a keycode's low byte names, if it is a plain key (mod-tap and layer-tap taps, modded keys).
fn tap_usage(kc: u16) -> u8 {
    match kc & 0xFF {
        u @ (0x04..=0xA4 | 0xE0..=0xE7) => u as u8,
        _ => 0,
    }
}

pub fn decode(kc: u16) -> Action {
    match kc {
        0x0000..=0x00FF => basic(kc),
        // LCTL(kc) and friends.
        0x0100..=0x1FFF => Action::Key(tap_usage(kc), hid_mods(kc >> 8)),
        // MT(mod, kc)
        0x2000..=0x3FFF => Action::TapHold(Tap::Key(tap_usage(kc), 0), Hold::Mods(hid_mods((kc >> 8) & 0x1F))),
        // LT(layer, kc)
        0x4000..=0x4FFF => Action::TapHold(Tap::Key(tap_usage(kc), 0), Hold::Layer(((kc >> 8) & 0x0F) as u8)),
        // LM(layer, mod)
        0x5000..=0x51FF => Action::Layer(Layer::WithMods(((kc >> 5) & 0x0F) as u8, hid_mods(kc & 0x1F))),
        0x5200..=0x521F => Action::Layer(Layer::To((kc & 0x1F) as u8)),
        0x5220..=0x523F => Action::Layer(Layer::Momentary((kc & 0x1F) as u8)),
        0x5240..=0x525F => Action::Layer(Layer::Default((kc & 0x1F) as u8, false)),
        0x5260..=0x527F => Action::Layer(Layer::Toggle((kc & 0x1F) as u8)),
        0x5280..=0x529F => Action::Layer(Layer::OneShot((kc & 0x1F) as u8)),
        0x52A0..=0x52BF => Action::OneShotMods(hid_mods(kc & 0x1F)),
        // TT(layer): tap toggles, hold is momentary.
        0x52C0..=0x52DF => {
            let l = (kc & 0x1F) as u8;
            Action::TapHold(Tap::ToggleLayer(l), Hold::Layer(l))
        }
        0x52E0..=0x52FF => Action::Layer(Layer::Default((kc & 0x1F) as u8, true)),
        0x7700..=0x77FF => Action::Macro(kc as u8),
        0x7C16 => Action::GraveEscape,
        // Space Cadet: ( or ) on tap, a modifier on hold.
        0x7C18..=0x7C1D => {
            let (paren, hold) = match kc {
                0x7C18 => (0x26, 0x01), // LCPO: (  / LCtrl
                0x7C19 => (0x27, 0x10), // RCPC: )  / RCtrl
                0x7C1A => (0x26, 0x02), // LSPO: (  / LShift
                0x7C1B => (0x27, 0x20), // RSPC: )  / RShift
                0x7C1C => (0x26, 0x04), // LAPO: (  / LAlt
                _ => (0x27, 0x40),      // RAPC: )  / RAlt
            };
            Action::TapHold(Tap::Key(paren, 0x02), Hold::Mods(hold))
        }
        // SC_SENT: Enter on tap, right Shift on hold.
        0x7C1E => Action::TapHold(Tap::Key(0x28, 0), Hold::Mods(0x20)),
        0x7C77 => Action::Layer(Layer::TriLower),
        0x7C78 => Action::Layer(Layer::TriUpper),
        _ => Action::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_common_keycodes() {
        assert_eq!(decode(0x0004), Action::Key(0x04, 0));
        assert_eq!(decode(0x00E5), Action::Key(0xE5, 0));
        assert_eq!(decode(0x00A9), Action::Consumer(0x00E9));
        assert_eq!(decode(0x00A6), Action::System(system::SLEEP));
        assert_eq!(decode(0x00D2), Action::Button(1));
        // LCTL(KC_C), RALT(KC_E), MEH(KC_A)
        assert_eq!(decode(0x0106), Action::Key(0x06, 0x01));
        assert_eq!(decode(0x1408), Action::Key(0x08, 0x40));
        assert_eq!(decode(0x0704), Action::Key(0x04, 0x07));
        // MT(MOD_LCTL, KC_ESC), LT(2, KC_SPC), TT(3)
        assert_eq!(decode(0x2129), Action::TapHold(Tap::Key(0x29, 0), Hold::Mods(0x01)));
        assert_eq!(decode(0x422C), Action::TapHold(Tap::Key(0x2C, 0), Hold::Layer(2)));
        assert_eq!(decode(0x52C3), Action::TapHold(Tap::ToggleLayer(3), Hold::Layer(3)));
        // MO(3), TG(1), TO(2), DF(1), OSL(1), OSM(MOD_RSFT), LM(1, MOD_LALT)
        assert_eq!(decode(0x5223), Action::Layer(Layer::Momentary(3)));
        assert_eq!(decode(0x5261), Action::Layer(Layer::Toggle(1)));
        assert_eq!(decode(0x5202), Action::Layer(Layer::To(2)));
        assert_eq!(decode(0x5241), Action::Layer(Layer::Default(1, false)));
        assert_eq!(decode(0x5281), Action::Layer(Layer::OneShot(1)));
        assert_eq!(decode(0x52B2), Action::OneShotMods(0x20));
        assert_eq!(decode(0x5024), Action::Layer(Layer::WithMods(1, 0x04)));
        assert_eq!(decode(0x7703), Action::Macro(3));
        assert_eq!(decode(QK_BOOT), Action::Unsupported);
        assert_eq!(decode(0x5700), Action::Unsupported, "tap dance");
    }

    #[test]
    fn modifier_fields_round_trip() {
        for m5 in 0..0x20u16 {
            let hid = hid_mods(m5);
            if m5 & 0x0F != 0 {
                assert_eq!(qmk_mods(hid), m5, "{m5:#x}");
            }
        }
    }
}
