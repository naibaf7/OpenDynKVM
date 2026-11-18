//! QMK keycodes as Vial stores them (protocol 6, QMK 0.19+ numbering) and what each does here.
//!
//! The ranges and their decoding follow RMK's `host/via/keycode_convert.rs` (`from_via_keycode`); the media and system
//! keys follow QMK's `KEYCODE2CONSUMER` / `KEYCODE2SYSTEM`.

pub const KC_NO: u16 = 0x0000;
pub const KC_TRNS: u16 = 0x0001;
pub const QK_TAP_DANCE: u16 = 0x5700;
pub const QK_BOOT: u16 = 0x7C00;
pub const QK_CAPS_WORD_TOGGLE: u16 = 0x7C73;
pub const QK_REPEAT_KEY: u16 = 0x7C79;
pub const QK_ALT_REPEAT_KEY: u16 = 0x7C7A;
pub const QK_LAYER_LOCK: u16 = 0x7C7B;

/// HID modifier bits.
pub mod mods {
    pub const LCTRL: u8 = 0x01;
    pub const LSHIFT: u8 = 0x02;
    pub const LALT: u8 = 0x04;
    pub const LGUI: u8 = 0x08;
    pub const RALT: u8 = 0x40;
    pub const SHIFT: u8 = 0x22;
    pub const CTRL: u8 = 0x11;
    pub const ALT: u8 = 0x44;
    pub const GUI: u8 = 0x88;
}

/// Bits of the magic setting (Vial's qsid 21, QMK's `keymap_config`), and one of this KVM's for the Escape / Caps Lock
/// swap, which Vial's settings page does not show but the magic keycodes set.
pub mod magic {
    pub const SWAP_CONTROL_CAPS: u16 = 1 << 0;
    pub const CAPS_AS_CONTROL: u16 = 1 << 1;
    pub const SWAP_LALT_LGUI: u16 = 1 << 2;
    pub const SWAP_RALT_RGUI: u16 = 1 << 3;
    pub const NO_GUI: u16 = 1 << 4;
    pub const SWAP_GRAVE_ESC: u16 = 1 << 5;
    pub const SWAP_BACKSLASH_BACKSPACE: u16 = 1 << 6;
    /// Kept for Vial; the KVM's keyboard is always NKRO.
    pub const NKRO: u16 = 1 << 7;
    pub const SWAP_LCTL_LGUI: u16 = 1 << 8;
    pub const SWAP_RCTL_RGUI: u16 = 1 << 9;
    pub const SWAP_ESC_CAPS: u16 = 1 << 10;
    pub const ALL: u16 = (1 << 11) - 1;
}

/// Normalized system-control bits (`kvm_hid::normalized::system`).
pub mod system {
    pub use kvm_hid::normalized::system::{POWER_DOWN, SLEEP, WAKE_UP};
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Tap {
    /// HID usage plus HID modifier bits.
    Key(u8, u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Switch {
    On,
    Off,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum AutoShift {
    /// Timeout 5 ms shorter / longer.
    Down,
    Up,
    /// Types the timeout.
    Report,
    Set(Switch),
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
    /// TT: on while held; tapped "tapping toggle" times in a row, it toggles.
    TapToggle(u8),
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
    TapDance(u8),
    CapsWord,
    Repeat,
    AltRepeat,
    LayerLock,
    /// QK_MAGIC + n: changes the magic setting.
    Magic(u8),
    Combos(Switch),
    AutoShift(AutoShift),
    /// Recognised but not done here (RGB, bootloader, MIDI, …).
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
        0x52C0..=0x52DF => Action::Layer(Layer::TapToggle((kc & 0x1F) as u8)),
        0x52E0..=0x52FF => Action::Layer(Layer::Default((kc & 0x1F) as u8, true)),
        0x5700..=0x57FF => Action::TapDance(kc as u8),
        0x7000..=0x7022 => Action::Magic((kc - 0x7000) as u8),
        0x7700..=0x77FF => Action::Macro(kc as u8),
        0x7C10 => Action::AutoShift(AutoShift::Down),
        0x7C11 => Action::AutoShift(AutoShift::Up),
        0x7C12 => Action::AutoShift(AutoShift::Report),
        0x7C13 => Action::AutoShift(AutoShift::Set(Switch::On)),
        0x7C14 => Action::AutoShift(AutoShift::Set(Switch::Off)),
        0x7C15 => Action::AutoShift(AutoShift::Set(Switch::Toggle)),
        0x7C16 => Action::GraveEscape,
        0x7C50 => Action::Combos(Switch::On),
        0x7C51 => Action::Combos(Switch::Off),
        0x7C52 => Action::Combos(Switch::Toggle),
        QK_CAPS_WORD_TOGGLE => Action::CapsWord,
        QK_REPEAT_KEY => Action::Repeat,
        QK_ALT_REPEAT_KEY => Action::AltRepeat,
        QK_LAYER_LOCK => Action::LayerLock,
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

/// The key a magic setting makes of `k` (QMK's `keycode_config`): 0 = no key.
pub fn magic_key(k: u8, m: u16) -> u8 {
    use magic::*;
    let on = |bit: u16| m & bit != 0;
    let gui = |g: u8| if on(NO_GUI) { 0 } else { g };
    match k {
        0x39 | 0x82 if on(SWAP_CONTROL_CAPS) || on(CAPS_AS_CONTROL) => 0xE0,
        0x39 | 0x82 if on(SWAP_ESC_CAPS) => 0x29,
        0xE0 if on(SWAP_CONTROL_CAPS) => 0x39,
        0xE0 if on(SWAP_LCTL_LGUI) => gui(0xE3),
        0xE2 if on(SWAP_LALT_LGUI) => gui(0xE3),
        0xE3 if on(SWAP_LALT_LGUI) => 0xE2,
        0xE3 if on(SWAP_LCTL_LGUI) => 0xE0,
        0xE3 => gui(0xE3),
        0xE4 if on(SWAP_RCTL_RGUI) => gui(0xE7),
        0xE6 if on(SWAP_RALT_RGUI) => gui(0xE7),
        0xE7 if on(SWAP_RALT_RGUI) => 0xE6,
        0xE7 if on(SWAP_RCTL_RGUI) => 0xE4,
        0xE7 => gui(0xE7),
        0x35 if on(SWAP_GRAVE_ESC) => 0x29,
        0x29 if on(SWAP_GRAVE_ESC) => 0x35,
        0x29 if on(SWAP_ESC_CAPS) => 0x39,
        0x31 if on(SWAP_BACKSLASH_BACKSPACE) => 0x2A,
        0x2A if on(SWAP_BACKSLASH_BACKSPACE) => 0x31,
        _ => k,
    }
}

/// The modifier bits a magic setting makes of `bits` (QMK's `mod_config`: the Caps Lock swaps leave modifiers alone).
pub fn magic_mods(bits: u8, m: u16) -> u8 {
    let m = m & !(magic::SWAP_CONTROL_CAPS | magic::CAPS_AS_CONTROL | magic::SWAP_ESC_CAPS);
    (0..8u8)
        .filter(|i| bits & (1 << i) != 0)
        .map(|i| magic_key(0xE0 + i, m))
        .filter(|u| (0xE0..=0xE7).contains(u))
        .fold(0, |out, u| out | 1 << (u - 0xE0))
}

/// The magic setting after magic keycode QK_MAGIC + `n` (QMK's `process_magic`).
pub fn magic_change(m: u16, n: u8) -> u16 {
    use magic::*;
    let set = |bits: u16, on: bool| if on { m | bits } else { m & !bits };
    const ALT_GUI: u16 = SWAP_LALT_LGUI | SWAP_RALT_RGUI;
    const CTL_GUI: u16 = SWAP_LCTL_LGUI | SWAP_RCTL_RGUI;
    match n {
        0x00 | 0x01 => set(SWAP_CONTROL_CAPS, n == 0x00),
        0x02 => m ^ SWAP_CONTROL_CAPS,
        0x03 | 0x04 => set(CAPS_AS_CONTROL, n == 0x04),
        0x05 | 0x06 => set(SWAP_LALT_LGUI, n == 0x05),
        0x07 | 0x08 => set(SWAP_RALT_RGUI, n == 0x07),
        0x09 | 0x0A => set(NO_GUI, n == 0x0A),
        0x0B => m ^ NO_GUI,
        0x0C | 0x0D => set(SWAP_GRAVE_ESC, n == 0x0C),
        0x0E | 0x0F => set(SWAP_BACKSLASH_BACKSPACE, n == 0x0E),
        0x10 => m ^ SWAP_BACKSLASH_BACKSPACE,
        0x11 | 0x12 => set(NKRO, n == 0x11),
        0x13 => m ^ NKRO,
        0x14 | 0x15 => set(ALT_GUI, n == 0x14),
        0x16 => set(ALT_GUI, m & SWAP_LALT_LGUI == 0),
        0x17 | 0x18 => set(SWAP_LCTL_LGUI, n == 0x17),
        0x19 | 0x1A => set(SWAP_RCTL_RGUI, n == 0x19),
        0x1B | 0x1C => set(CTL_GUI, n == 0x1B),
        0x1D => set(CTL_GUI, m & SWAP_LCTL_LGUI == 0),
        0x20 | 0x21 => set(SWAP_ESC_CAPS, n == 0x20),
        0x22 => m ^ SWAP_ESC_CAPS,
        // 0x1E, 0x1F: EE_HANDS, for split keyboards.
        _ => m,
    }
}

/// `a` with the magic setting applied to its keys and modifiers.
pub fn with_magic(a: Action, m: u16) -> Action {
    if m == 0 {
        return a;
    }
    match a {
        Action::Key(k, mods) => Action::Key(magic_key(k, m), magic_mods(mods, m)),
        Action::TapHold(Tap::Key(k, tm), hold) => {
            let hold = match hold {
                Hold::Mods(h) => Hold::Mods(magic_mods(h, m)),
                Hold::Layer(l) => Hold::Layer(l),
            };
            Action::TapHold(Tap::Key(magic_key(k, m), magic_mods(tm, m)), hold)
        }
        Action::Layer(Layer::WithMods(l, mods)) => Action::Layer(Layer::WithMods(l, magic_mods(mods, m))),
        Action::OneShotMods(mods) => Action::OneShotMods(magic_mods(mods, m)),
        other => other,
    }
}

/// The keycode a key sends when tapped: the tap of a mod-tap or layer-tap, else the keycode itself.
pub const fn tap_keycode(kc: u16) -> u16 {
    match kc {
        0x2000..=0x4FFF => kc & 0xFF,
        _ => kc,
    }
}

/// QMK's built-in alternates for the Alternate Repeat Key (`get_alt_repeat_key_keycode`): `basic` was pressed with
/// HID modifiers `mods`; the result is a basic keycode, or none.
pub fn default_alt(basic: u8, mods: u8) -> Option<u8> {
    const WITH_MODS: &[(u8, u8)] = &[(0x09, 0x05), (0x07, 0x18), (0x11, 0x13), (0x04, 0x08), (0x12, 0x0C)];
    const PLAIN: &[(u8, u8)] = &[(0x0D, 0x0E), (0x0B, 0x0F), (0x1A, 0x05), (0x08, 0x05)];
    const ANY: &[(u8, u8)] = &[
        (0x50, 0x4F), // Left / Right
        (0x52, 0x51), // Up / Down
        (0x4A, 0x4D), // Home / End
        (0x4B, 0x4E), // Page Up / Down
        (0x2A, 0x4C), // Backspace / Delete
        (0x2F, 0x30), // [ ]
        (0xB6, 0xB7), // Browser Back / Forward
        (0xAB, 0xAC), // Next / Previous Track
        (0xBB, 0xBC), // Fast Forward / Rewind
        (0xA9, 0xAA), // Volume Up / Down
        (0xBD, 0xBE), // Brightness Up / Down
        (0xCF, 0xD0), // Mouse Left / Right
        (0xCD, 0xCE), // Mouse Up / Down
        (0xDB, 0xDC), // Wheel Left / Right
        (0xD9, 0xDA), // Wheel Up / Down
    ];
    let find = |pairs: &[(u8, u8)]| {
        pairs.iter().find_map(|&(a, b)| {
            if a == basic {
                Some(b)
            } else if b == basic {
                Some(a)
            } else {
                None
            }
        })
    };
    let hotkey = mods & (mods::CTRL | mods::ALT | mods::GUI) != 0;
    find(if hotkey { WITH_MODS } else { PLAIN }).or_else(|| find(ANY))
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
        assert_eq!(decode(0x52C3), Action::Layer(Layer::TapToggle(3)));
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
        // TD(5), caps word, repeat keys, layer lock, magic, combo and auto shift keys (vial-gui keycodes_v6).
        assert_eq!(decode(0x5705), Action::TapDance(5));
        assert_eq!(decode(0x7C73), Action::CapsWord);
        assert_eq!((decode(0x7C79), decode(0x7C7A)), (Action::Repeat, Action::AltRepeat));
        assert_eq!(decode(0x7C7B), Action::LayerLock);
        assert_eq!(decode(0x7017), Action::Magic(0x17));
        assert_eq!(decode(0x7C52), Action::Combos(Switch::Toggle));
        assert_eq!(decode(0x7C15), Action::AutoShift(AutoShift::Set(Switch::Toggle)));
        assert_eq!(decode(0x7840), Action::Unsupported, "RGB");
    }

    #[test]
    fn magic_swaps_keys_and_modifiers() {
        use magic::*;
        assert_eq!(magic_key(0x39, CAPS_AS_CONTROL), 0xE0);
        assert_eq!((magic_key(0x39, SWAP_CONTROL_CAPS), magic_key(0xE0, SWAP_CONTROL_CAPS)), (0xE0, 0x39));
        assert_eq!((magic_key(0xE2, SWAP_LALT_LGUI), magic_key(0xE3, SWAP_LALT_LGUI)), (0xE3, 0xE2));
        assert_eq!(magic_key(0xE3, NO_GUI), 0);
        assert_eq!(magic_key(0xE2, SWAP_LALT_LGUI | NO_GUI), 0, "Alt swapped onto a disabled GUI");
        assert_eq!((magic_key(0x35, SWAP_GRAVE_ESC), magic_key(0x29, SWAP_GRAVE_ESC)), (0x29, 0x35));
        // LCTL(KC_C) with Ctrl and GUI swapped is GUI+C; the Caps Lock swap leaves modifiers alone.
        assert_eq!(with_magic(decode(0x0106), SWAP_LCTL_LGUI), Action::Key(0x06, 0x08));
        assert_eq!(with_magic(decode(0x0106), SWAP_CONTROL_CAPS), Action::Key(0x06, 0x01));
        assert_eq!(magic_mods(0x88, NO_GUI), 0);
    }

    #[test]
    fn default_alternates_follow_qmk() {
        assert_eq!(default_alt(0x50, 0), Some(0x4F), "Left → Right");
        assert_eq!(default_alt(0x4F, 0x03), Some(0x50), "with any modifiers");
        assert_eq!(default_alt(0x09, 0x01), Some(0x05), "Ctrl+F → Ctrl+B");
        assert_eq!(default_alt(0x05, 0), Some(0x1A), "B → W without hotkey modifiers");
        assert_eq!(default_alt(0x09, 0x02), None, "Shift+F has none");
        assert_eq!(default_alt(0xA9, 0), Some(0xAA), "Volume Up → Down");
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
