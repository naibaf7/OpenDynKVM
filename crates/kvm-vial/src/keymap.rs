//! Layers, macros, settings and Vial's dynamic entries (tap dance, combos, key overrides, alternate repeat keys) as
//! Vial edits them, and as flash items (`kvm_core::store`).
//!
//! Keycodes are kept exactly as Vial wrote them, so that every keycode reads back unchanged, including the ones the
//! engine does not act on. The dynamic entries are kept in Vial's own wire layout (little-endian), field for field.
//!
//! | Item | Key | Contents |
//! |---|---|---|
//! | settings | 0x0100 | version 1, then [`Settings`] as [`Settings::encode`] lays it out (13 bytes before the dynamic entries, 47 since) |
//! | macros | 0x0101 | the macro buffer up to its last used byte |
//! | tap dances | 0x0102 | version 1, then [`TAP_DANCES`] × 10 bytes |
//! | combos | 0x0103 | version 1, then [`COMBOS`] × 10 bytes |
//! | key overrides | 0x0104 | version 1, then [`KEY_OVERRIDES`] × 10 bytes |
//! | alternate repeat keys | 0x0105 | version 1, then [`ALT_REPEATS`] × 6 bytes |
//! | layer n | 0x0110 + n | version 1, then the keycode of every position, u16 LE |

use crate::keycode::{KC_NO, KC_TRNS, magic};
use crate::{LAYERS, POSITIONS, in_layout};

pub const MACRO_COUNT: u8 = 16;
pub const MACRO_BUFFER: usize = 2048;
/// Entries of each kind: what vial-qmk gives a board with more than 4 KB of EEPROM.
pub const TAP_DANCES: usize = 32;
pub const COMBOS: usize = 32;
pub const KEY_OVERRIDES: usize = 32;
pub const ALT_REPEATS: usize = 32;

pub mod item {
    pub const SETTINGS: u16 = 0x0100;
    pub const MACROS: u16 = 0x0101;
    pub const TAP_DANCES: u16 = 0x0102;
    pub const COMBOS: u16 = 0x0103;
    pub const KEY_OVERRIDES: u16 = 0x0104;
    pub const ALT_REPEATS: u16 = 0x0105;
    pub const LAYER: u16 = 0x0110;
}

const VERSION: u8 = 1;
const DIRTY_MACROS: u16 = 1 << 8;
const DIRTY_SETTINGS: u16 = 1 << 9;
/// Then one bit per [`Dynamic`] kind, in its order.
const DIRTY_DYNAMIC: u16 = 1 << 10;
const DIRTY_ALL: u16 = (1 << 14) - 1;
pub const LAYER_ITEM_LEN: usize = 1 + 2 * POSITIONS;
pub const SETTINGS_LEN: usize = 47;

/// QMK settings (Vial's "QMK Settings" tab) by qsid: every one vial-qmk has, except RGB and the pre-0.19 tapping
/// flags (qsid 8).
pub mod qsid {
    pub const GRAVE_ESC_OVERRIDE: u16 = 1;
    pub const COMBO_TERM: u16 = 2;
    pub const AUTO_SHIFT: u16 = 3;
    pub const AUTO_SHIFT_TIMEOUT: u16 = 4;
    pub const ONESHOT_TAP_TOGGLE: u16 = 5;
    pub const ONESHOT_TIMEOUT: u16 = 6;
    pub const TAPPING_TERM: u16 = 7;
    /// 9–17: mouse keys (delay, interval, move delta, max speed, time to max, then the same for the wheel without its
    /// delta).
    pub const MOUSE_FIRST: u16 = 9;
    pub const MOUSE_LAST: u16 = 17;
    pub const TAP_CODE_DELAY: u16 = 18;
    pub const TAP_HOLD_CAPS_DELAY: u16 = 19;
    pub const TAPPING_TOGGLE: u16 = 20;
    pub const MAGIC: u16 = 21;
    pub const PERMISSIVE_HOLD: u16 = 22;
    pub const HOLD_ON_OTHER_KEY_PRESS: u16 = 23;
    pub const RETRO_TAPPING: u16 = 24;
    pub const QUICK_TAP_TERM: u16 = 25;
    pub const CHORDAL_HOLD: u16 = 26;
    pub const FLOW_TAP_TERM: u16 = 27;
    pub const ALL: [u16; 26] =
        [1, 2, 3, 4, 5, 6, 7, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27];
}

/// Bits of the auto shift setting (qsid 3).
pub mod auto_shift {
    pub const ENABLE: u8 = 1 << 0;
    pub const MODIFIERS: u8 = 1 << 1;
    pub const NO_SPECIAL: u8 = 1 << 2;
    pub const NO_NUMERIC: u8 = 1 << 3;
    pub const NO_ALPHA: u8 = 1 << 4;
    pub const REPEAT: u8 = 1 << 5;
    pub const NO_AUTO_REPEAT: u8 = 1 << 6;
}

/// QMK's mouse key settings, in milliseconds and steps (QMK's defaults, accelerated mode).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MouseKeys {
    pub delay_ms: u16,
    pub interval_ms: u16,
    pub move_delta: u16,
    pub max_speed: u16,
    pub time_to_max: u16,
    pub wheel_delay_ms: u16,
    pub wheel_interval_ms: u16,
    pub wheel_max_speed: u16,
    pub wheel_time_to_max: u16,
}

impl MouseKeys {
    pub const DEFAULT: MouseKeys = MouseKeys {
        delay_ms: 10,
        interval_ms: 16,
        move_delta: 8,
        max_speed: 10,
        time_to_max: 30,
        wheel_delay_ms: 10,
        wheel_interval_ms: 80,
        wheel_max_speed: 8,
        wheel_time_to_max: 40,
    };

    fn field(&mut self, n: u16) -> &mut u16 {
        match n {
            0 => &mut self.delay_ms,
            1 => &mut self.interval_ms,
            2 => &mut self.move_delta,
            3 => &mut self.max_speed,
            4 => &mut self.time_to_max,
            5 => &mut self.wheel_delay_ms,
            6 => &mut self.wheel_interval_ms,
            7 => &mut self.wheel_max_speed,
            _ => &mut self.wheel_time_to_max,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Settings {
    pub layout_options: u32,
    pub default_layer: u8,
    pub tapping_term_ms: u16,
    pub permissive_hold: bool,
    pub hold_on_other_key_press: bool,
    pub retro_tapping: bool,
    pub chordal_hold: bool,
    /// 0 = one-shot keys wait for the next key however long it takes (QMK's default).
    pub oneshot_timeout_ms: u16,
    pub tap_code_delay_ms: u16,
    /// Bits: Escape even with Alt / Ctrl / GUI / Shift held.
    pub grave_esc_override: u8,
    pub combo_term_ms: u16,
    /// [`auto_shift`] bits.
    pub auto_shift: u8,
    pub auto_shift_timeout_ms: u16,
    /// Taps of a one-shot key that lock it (0 = never).
    pub oneshot_tap_toggle: u8,
    pub mouse: MouseKeys,
    pub tap_hold_caps_delay_ms: u16,
    /// Taps of a TT key that toggle its layer.
    pub tapping_toggle: u8,
    /// [`magic`] bits.
    pub magic: u16,
    /// A tap-hold key pressed again this soon after it was tapped repeats its tap (0 = never); at most the tapping
    /// term.
    pub quick_tap_term_ms: u16,
    /// A mod-tap or layer-tap pressed this soon after a letter is tapped at once (0 = off).
    pub flow_tap_term_ms: u16,
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
        retro_tapping: false,
        chordal_hold: false,
        oneshot_timeout_ms: 0,
        tap_code_delay_ms: 0,
        grave_esc_override: 0,
        combo_term_ms: 50,
        auto_shift: 0,
        auto_shift_timeout_ms: 175,
        oneshot_tap_toggle: 5,
        mouse: MouseKeys::DEFAULT,
        tap_hold_caps_delay_ms: 80,
        tapping_toggle: 5,
        magic: 0,
        quick_tap_term_ms: 200,
        flow_tap_term_ms: 0,
    };

    /// Value of a QMK setting (little-endian, its width as vial-qmk has it).
    pub fn get(&self, qsid: u16, out: &mut [u8]) -> Option<usize> {
        let mut m = self.mouse;
        let (v, width) = match qsid {
            qsid::GRAVE_ESC_OVERRIDE => (self.grave_esc_override as u32, 1),
            qsid::COMBO_TERM => (self.combo_term_ms as u32, 2),
            qsid::AUTO_SHIFT => (self.auto_shift as u32, 1),
            qsid::AUTO_SHIFT_TIMEOUT => (self.auto_shift_timeout_ms as u32, 2),
            qsid::ONESHOT_TAP_TOGGLE => (self.oneshot_tap_toggle as u32, 1),
            qsid::ONESHOT_TIMEOUT => (self.oneshot_timeout_ms as u32, 2),
            qsid::TAPPING_TERM => (self.tapping_term_ms as u32, 2),
            qsid::MOUSE_FIRST..=qsid::MOUSE_LAST => (*m.field(qsid - qsid::MOUSE_FIRST) as u32, 2),
            qsid::TAP_CODE_DELAY => (self.tap_code_delay_ms as u32, 2),
            qsid::TAP_HOLD_CAPS_DELAY => (self.tap_hold_caps_delay_ms as u32, 2),
            qsid::TAPPING_TOGGLE => (self.tapping_toggle as u32, 1),
            qsid::MAGIC => (self.magic as u32, 4),
            qsid::PERMISSIVE_HOLD => (self.permissive_hold as u32, 1),
            qsid::HOLD_ON_OTHER_KEY_PRESS => (self.hold_on_other_key_press as u32, 1),
            qsid::RETRO_TAPPING => (self.retro_tapping as u32, 1),
            qsid::QUICK_TAP_TERM => (self.quick_tap_term_ms as u32, 2),
            qsid::CHORDAL_HOLD => (self.chordal_hold as u32, 1),
            qsid::FLOW_TAP_TERM => (self.flow_tap_term_ms as u32, 2),
            _ => return None,
        };
        out.get_mut(..width)?.copy_from_slice(&v.to_le_bytes()[..width]);
        Some(width)
    }

    /// Sets a QMK setting from Vial's bytes; values are kept within vial-gui's limits (and intervals above 0, so a
    /// timer never fires continuously).
    pub fn set(&mut self, qsid: u16, v: &[u8]) -> bool {
        let u8_at = || v.first().copied().unwrap_or(0);
        let u16_at = || v.get(..2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
        let flag = || u8_at() != 0;
        match qsid {
            qsid::GRAVE_ESC_OVERRIDE => self.grave_esc_override = u8_at() & 0x0F,
            qsid::COMBO_TERM => self.combo_term_ms = u16_at().min(10000),
            qsid::AUTO_SHIFT => self.auto_shift = u8_at() & 0x7F,
            qsid::AUTO_SHIFT_TIMEOUT => self.auto_shift_timeout_ms = u16_at().min(1000),
            qsid::ONESHOT_TAP_TOGGLE => self.oneshot_tap_toggle = u8_at().min(50),
            qsid::ONESHOT_TIMEOUT => self.oneshot_timeout_ms = u16_at().min(60000),
            qsid::TAPPING_TERM => self.tapping_term_ms = u16_at().clamp(10, 10000),
            qsid::MOUSE_FIRST..=qsid::MOUSE_LAST => {
                let n = qsid - qsid::MOUSE_FIRST;
                // Intervals (1, 6) at least 1 ms; delays up to 10 s; steps and speeds up to 1000.
                let v = match n {
                    1 | 6 => u16_at().clamp(1, 10000),
                    0 | 5 => u16_at().min(10000),
                    _ => u16_at().min(1000),
                };
                *self.mouse.field(n) = v;
            }
            qsid::TAP_CODE_DELAY => self.tap_code_delay_ms = u16_at().min(1000),
            qsid::TAP_HOLD_CAPS_DELAY => self.tap_hold_caps_delay_ms = u16_at().min(1000),
            qsid::TAPPING_TOGGLE => self.tapping_toggle = u8_at().min(100),
            qsid::MAGIC => self.magic = u16_at() & magic::ALL,
            qsid::PERMISSIVE_HOLD => self.permissive_hold = flag(),
            qsid::HOLD_ON_OTHER_KEY_PRESS => self.hold_on_other_key_press = flag(),
            qsid::RETRO_TAPPING => self.retro_tapping = flag(),
            qsid::QUICK_TAP_TERM => self.quick_tap_term_ms = u16_at().min(10000),
            qsid::CHORDAL_HOLD => self.chordal_hold = flag(),
            qsid::FLOW_TAP_TERM => self.flow_tap_term_ms = u16_at().min(10000),
            _ => return false,
        }
        true
    }

    fn encode(&self, out: &mut [u8]) -> usize {
        let flags = self.permissive_hold as u8
            | (self.hold_on_other_key_press as u8) << 1
            | (self.retro_tapping as u8) << 2
            | (self.chordal_hold as u8) << 3;
        let mut b = [0u8; SETTINGS_LEN];
        b[0] = VERSION;
        b[1..5].copy_from_slice(&self.layout_options.to_le_bytes());
        b[5] = self.default_layer;
        b[6..8].copy_from_slice(&self.tapping_term_ms.to_le_bytes());
        b[8] = flags;
        b[9..11].copy_from_slice(&self.oneshot_timeout_ms.to_le_bytes());
        b[11..13].copy_from_slice(&self.tap_code_delay_ms.to_le_bytes());
        // Appended with the dynamic entries; a shorter item from before reads with the defaults here.
        b[13] = self.grave_esc_override;
        b[14..16].copy_from_slice(&self.combo_term_ms.to_le_bytes());
        b[16] = self.auto_shift;
        b[17..19].copy_from_slice(&self.auto_shift_timeout_ms.to_le_bytes());
        b[19] = self.oneshot_tap_toggle;
        let mut m = self.mouse;
        for n in 0..9 {
            b[20 + 2 * n..22 + 2 * n].copy_from_slice(&m.field(n as u16).to_le_bytes());
        }
        b[38..40].copy_from_slice(&self.tap_hold_caps_delay_ms.to_le_bytes());
        b[40] = self.tapping_toggle;
        b[41..43].copy_from_slice(&self.magic.to_le_bytes());
        b[43..45].copy_from_slice(&self.quick_tap_term_ms.to_le_bytes());
        b[45..47].copy_from_slice(&self.flow_tap_term_ms.to_le_bytes());
        out[..b.len()].copy_from_slice(&b);
        b.len()
    }

    fn decode(d: &[u8]) -> Option<Settings> {
        if d.len() < 13 || d[0] != VERSION {
            return None;
        }
        let u16_at = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
        let mut s = Settings {
            layout_options: u32::from_le_bytes([d[1], d[2], d[3], d[4]]),
            default_layer: d[5].min(LAYERS as u8 - 1),
            tapping_term_ms: u16_at(6).clamp(10, 10000),
            permissive_hold: d[8] & 1 != 0,
            hold_on_other_key_press: d[8] & 2 != 0,
            retro_tapping: d[8] & 4 != 0,
            chordal_hold: d[8] & 8 != 0,
            oneshot_timeout_ms: u16_at(9),
            tap_code_delay_ms: u16_at(11).min(1000),
            ..Settings::DEFAULT
        };
        if d.len() >= SETTINGS_LEN {
            // Through `set`, so a damaged value is kept within its limits like one from Vial.
            let mut set = |q: u16, b: &[u8]| {
                s.set(q, b);
            };
            set(qsid::GRAVE_ESC_OVERRIDE, &d[13..14]);
            set(qsid::COMBO_TERM, &d[14..16]);
            set(qsid::AUTO_SHIFT, &d[16..17]);
            set(qsid::AUTO_SHIFT_TIMEOUT, &d[17..19]);
            set(qsid::ONESHOT_TAP_TOGGLE, &d[19..20]);
            for n in 0..9 {
                set(qsid::MOUSE_FIRST + n as u16, &d[20 + 2 * n..22 + 2 * n]);
            }
            set(qsid::TAP_HOLD_CAPS_DELAY, &d[38..40]);
            set(qsid::TAPPING_TOGGLE, &d[40..41]);
            set(qsid::MAGIC, &d[41..43]);
            set(qsid::QUICK_TAP_TERM, &d[43..45]);
            set(qsid::FLOW_TAP_TERM, &d[45..47]);
        }
        Some(s)
    }
}

/// A Vial tap dance (`vial_tap_dance_entry_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct TapDance {
    pub on_tap: u16,
    pub on_hold: u16,
    pub on_double_tap: u16,
    pub on_tap_hold: u16,
    /// 0 = the tapping term.
    pub tapping_term_ms: u16,
}

/// A Vial combo (`vial_combo_entry_t`): up to four keycodes, ended by `KC_NO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Combo {
    pub input: [u16; 4],
    pub output: u16,
}

impl Combo {
    /// The keycodes to press together (none: an unused entry).
    pub fn keys(&self) -> &[u16] {
        let n = self.input.iter().position(|k| *k == KC_NO).unwrap_or(4);
        &self.input[..n]
    }
}

/// Key override options (`vial_ko_option_*`).
pub mod ko {
    pub const ACTIVATION_TRIGGER_DOWN: u8 = 1 << 0;
    pub const ACTIVATION_REQUIRED_MOD_DOWN: u8 = 1 << 1;
    pub const ACTIVATION_NEGATIVE_MOD_UP: u8 = 1 << 2;
    pub const ONE_MOD: u8 = 1 << 3;
    pub const NO_REREGISTER_TRIGGER: u8 = 1 << 4;
    pub const NO_UNREGISTER_ON_OTHER_KEY_DOWN: u8 = 1 << 5;
    pub const ENABLED: u8 = 1 << 7;
}

/// A Vial key override (`vial_key_override_entry_t`); modifiers are HID bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct KeyOverride {
    pub trigger: u16,
    pub replacement: u16,
    /// Bit per layer: the override applies while the highest active layer is one of them.
    pub layers: u16,
    pub trigger_mods: u8,
    pub negative_mods: u8,
    pub suppressed_mods: u8,
    /// [`ko`] bits.
    pub options: u8,
}

/// Alternate repeat key options (`vial_arep_option_*`).
pub mod arep {
    pub const DEFAULT_TO_THIS_ALT_KEY: u8 = 1 << 0;
    pub const BIDIRECTIONAL: u8 = 1 << 1;
    pub const IGNORE_MOD_HANDEDNESS: u8 = 1 << 2;
    pub const ENABLED: u8 = 1 << 3;
}

/// A Vial alternate repeat key (`vial_alt_repeat_key_entry_t`): after `keycode`, the Alternate Repeat Key sends
/// `alt_keycode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct AltRepeat {
    pub keycode: u16,
    pub alt_keycode: u16,
    pub allowed_mods: u8,
    /// [`arep`] bits.
    pub options: u8,
}

/// The kinds of Vial's dynamic entries, in the order of their flash items and of Vial's sub-commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Dynamic {
    TapDance,
    Combo,
    KeyOverride,
    AltRepeat,
}

impl Dynamic {
    pub const ALL: [Dynamic; 4] = [Dynamic::TapDance, Dynamic::Combo, Dynamic::KeyOverride, Dynamic::AltRepeat];

    pub const fn count(self) -> usize {
        match self {
            Dynamic::TapDance => TAP_DANCES,
            Dynamic::Combo => COMBOS,
            Dynamic::KeyOverride => KEY_OVERRIDES,
            Dynamic::AltRepeat => ALT_REPEATS,
        }
    }

    /// Bytes per entry on the wire and in flash.
    pub const fn size(self) -> usize {
        match self {
            Dynamic::AltRepeat => 6,
            _ => 10,
        }
    }

    const fn dirty(self) -> u16 {
        DIRTY_DYNAMIC << self as u16
    }

    /// The flash item's key and length: a version byte, then every entry.
    pub const fn item(self) -> (u16, usize) {
        (self.key(), 1 + self.count() * self.size())
    }

    const fn key(self) -> u16 {
        item::TAP_DANCES + self as u16
    }
}

fn le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

pub struct Keymap {
    layers: [[u16; POSITIONS]; LAYERS],
    macros: [u8; MACRO_BUFFER],
    pub settings: Settings,
    tap_dances: [TapDance; TAP_DANCES],
    combos: [Combo; COMBOS],
    key_overrides: [KeyOverride; KEY_OVERRIDES],
    alt_repeats: [AltRepeat; ALT_REPEATS],
    /// Bit n: layer n; then macros, settings and the dynamic kinds.
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

const NO_TAP_DANCE: TapDance = TapDance { on_tap: 0, on_hold: 0, on_double_tap: 0, on_tap_hold: 0, tapping_term_ms: 0 };
const NO_COMBO: Combo = Combo { input: [0; 4], output: 0 };
const NO_KEY_OVERRIDE: KeyOverride = KeyOverride {
    trigger: 0,
    replacement: 0,
    layers: 0,
    trigger_mods: 0,
    negative_mods: 0,
    suppressed_mods: 0,
    options: 0,
};
const NO_ALT_REPEAT: AltRepeat = AltRepeat { keycode: 0, alt_keycode: 0, allowed_mods: 0, options: 0 };

impl Keymap {
    /// Every key does what it says on layer 0; layers 1–7 are transparent; no dynamic entries.
    pub const fn new() -> Self {
        let mut layers = [[KC_TRNS; POSITIONS]; LAYERS];
        let mut p = 0;
        while p < POSITIONS {
            layers[0][p] = identity(p);
            p += 1;
        }
        Keymap {
            layers,
            macros: [0; MACRO_BUFFER],
            settings: Settings::DEFAULT,
            tap_dances: [NO_TAP_DANCE; TAP_DANCES],
            combos: [NO_COMBO; COMBOS],
            key_overrides: [NO_KEY_OVERRIDE; KEY_OVERRIDES],
            alt_repeats: [NO_ALT_REPEAT; ALT_REPEATS],
            dirty: 0,
            changes: 0,
        }
    }

    /// Back to [`Keymap::new`], and every item due to be written.
    pub fn reset(&mut self) {
        let changes = self.changes;
        *self = Keymap { changes, ..Keymap::new() };
        self.mark(DIRTY_ALL);
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

    /// Whether the keymap changes nothing: every layer as [`Keymap::new`] made it, no combos, no key overrides, no
    /// magic swaps and no auto shift.
    pub fn is_identity(&self) -> bool {
        self.layers[0].iter().enumerate().all(|(p, kc)| *kc == identity(p))
            && self.layers[1..].iter().all(|l| l.iter().all(|kc| *kc == KC_TRNS))
            && self.combos.iter().all(|c| c.keys().is_empty())
            && self.key_overrides.iter().all(|k| k.options & ko::ENABLED == 0)
            && self.settings.magic & !magic::NKRO == 0
            && self.settings.auto_shift & auto_shift::ENABLE == 0
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

    pub fn tap_dance(&self, n: u8) -> Option<&TapDance> {
        self.tap_dances.get(n as usize)
    }

    pub fn combos(&self) -> &[Combo; COMBOS] {
        &self.combos
    }

    pub fn key_overrides(&self) -> &[KeyOverride; KEY_OVERRIDES] {
        &self.key_overrides
    }

    pub fn alt_repeats(&self) -> &[AltRepeat; ALT_REPEATS] {
        &self.alt_repeats
    }

    /// Entry `i` of a kind in Vial's layout; `None` past the last.
    pub fn dynamic(&self, kind: Dynamic, i: usize, out: &mut [u8]) -> Option<usize> {
        if i >= kind.count() || out.len() < kind.size() {
            return None;
        }
        let w = |out: &mut [u8], at: usize, v: u16| out[at..at + 2].copy_from_slice(&v.to_le_bytes());
        match kind {
            Dynamic::TapDance => {
                let t = &self.tap_dances[i];
                for (n, v) in
                    [t.on_tap, t.on_hold, t.on_double_tap, t.on_tap_hold, t.tapping_term_ms].iter().enumerate()
                {
                    w(out, 2 * n, *v);
                }
            }
            Dynamic::Combo => {
                let c = &self.combos[i];
                for (n, v) in c.input.iter().chain([c.output].iter()).enumerate() {
                    w(out, 2 * n, *v);
                }
            }
            Dynamic::KeyOverride => {
                let k = &self.key_overrides[i];
                w(out, 0, k.trigger);
                w(out, 2, k.replacement);
                w(out, 4, k.layers);
                out[6..10].copy_from_slice(&[k.trigger_mods, k.negative_mods, k.suppressed_mods, k.options]);
            }
            Dynamic::AltRepeat => {
                let a = &self.alt_repeats[i];
                w(out, 0, a.keycode);
                w(out, 2, a.alt_keycode);
                out[4..6].copy_from_slice(&[a.allowed_mods, a.options]);
            }
        }
        Some(kind.size())
    }

    /// Sets entry `i` of a kind from Vial's layout; false past the last or for too few bytes.
    pub fn set_dynamic(&mut self, kind: Dynamic, i: usize, b: &[u8]) -> bool {
        if i >= kind.count() || b.len() < kind.size() {
            return false;
        }
        let changed = match kind {
            Dynamic::TapDance => {
                let t = TapDance {
                    on_tap: le(b, 0),
                    on_hold: le(b, 2),
                    on_double_tap: le(b, 4),
                    on_tap_hold: le(b, 6),
                    tapping_term_ms: le(b, 8),
                };
                core::mem::replace(&mut self.tap_dances[i], t) != t
            }
            Dynamic::Combo => {
                let c = Combo { input: [le(b, 0), le(b, 2), le(b, 4), le(b, 6)], output: le(b, 8) };
                core::mem::replace(&mut self.combos[i], c) != c
            }
            Dynamic::KeyOverride => {
                let k = KeyOverride {
                    trigger: le(b, 0),
                    replacement: le(b, 2),
                    layers: le(b, 4),
                    trigger_mods: b[6],
                    negative_mods: b[7],
                    suppressed_mods: b[8],
                    options: b[9],
                };
                core::mem::replace(&mut self.key_overrides[i], k) != k
            }
            Dynamic::AltRepeat => {
                let a = AltRepeat { keycode: le(b, 0), alt_keycode: le(b, 2), allowed_mods: b[4], options: b[5] };
                core::mem::replace(&mut self.alt_repeats[i], a) != a
            }
        };
        if changed {
            self.mark(kind.dirty());
        }
        true
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
            9 => (item::SETTINGS, self.settings.encode(out)),
            b => {
                let kind = Dynamic::ALL[(b - 10) as usize];
                out[0] = VERSION;
                let mut n = 1;
                for i in 0..kind.count() {
                    n += self.dynamic(kind, i, &mut out[n..]).unwrap_or(0);
                }
                (kind.key(), n)
            }
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
            k if (item::TAP_DANCES..=item::ALT_REPEATS).contains(&k) => {
                let kind = Dynamic::ALL[(k - item::TAP_DANCES) as usize];
                if d.first() == Some(&VERSION) && (d.len() - 1).is_multiple_of(kind.size()) {
                    let dirty = self.dirty;
                    for (i, e) in d[1..].chunks(kind.size()).enumerate() {
                        self.set_dynamic(kind, i, e);
                    }
                    // Restoring is not a change.
                    self.dirty = dirty;
                }
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
        [item::SETTINGS, item::MACROS, item::TAP_DANCES, item::COMBOS, item::KEY_OVERRIDES, item::ALT_REPEATS]
            .into_iter()
            .chain((0..LAYERS as u16).map(|l| item::LAYER + l))
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
        k.settings.mouse.max_speed = 20;
        k.settings.magic = magic::SWAP_LALT_LGUI;
        k.settings_changed();
        assert!(k.set_dynamic(Dynamic::Combo, 3, &[0x04, 0, 0x05, 0, 0, 0, 0, 0, 0x29, 0]));
        assert!(k.set_dynamic(Dynamic::AltRepeat, 31, &[0x06, 0x01, 0x19, 0x01, 0, arep::ENABLED]));
        let mut copy = Keymap::new();
        let mut buf = [0u8; MACRO_BUFFER];
        let mut keys = std::vec::Vec::new();
        while let Some((key, n)) = k.take_dirty(&mut buf) {
            copy.restore(key, &buf[..n]);
            keys.push(key);
        }
        assert_eq!(keys, [item::LAYER, item::LAYER + 2, item::MACROS, item::SETTINGS, item::COMBOS, item::ALT_REPEATS]);
        assert_eq!(copy.get(0, 0x39), 0x00E0);
        assert_eq!(copy.get(2, 0x1E), 0x7701);
        assert_eq!(copy.macro_bytes(1), b"\x01\x01\x04");
        assert_eq!(copy.settings, k.settings);
        assert_eq!(copy.combos()[3], Combo { input: [0x04, 0x05, 0, 0], output: 0x29 });
        assert_eq!(copy.combos()[3].keys(), [0x04, 0x05]);
        assert_eq!(copy.alt_repeats()[31].alt_keycode, 0x0119);
        assert!(!copy.is_identity() && !copy.is_dirty());
        assert!(!k.is_dirty());
    }

    #[test]
    fn settings_from_before_the_dynamic_entries_keep_the_new_defaults() {
        // A settings item from before the dynamic entries: 13 bytes, tapping term 150, permissive hold.
        let old = [1, 0, 0, 0, 0, 0, 150, 0, 1, 0, 0, 0, 0];
        let mut k = Keymap::new();
        k.restore(item::SETTINGS, &old);
        assert_eq!(k.settings.tapping_term_ms, 150);
        assert!(k.settings.permissive_hold);
        assert_eq!(
            (k.settings.combo_term_ms, k.settings.mouse, k.settings.tapping_toggle),
            (50, MouseKeys::DEFAULT, 5)
        );
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
        assert!(!s.set(8, &[1]), "the pre-0.19 tapping flags");
        // Every listed qsid reads and writes, magic as 4 bytes, and a mouse interval never reaches 0.
        for q in qsid::ALL {
            let n = s.get(q, &mut out).unwrap();
            assert!(s.set(q, &out[..n]), "{q}");
        }
        assert_eq!(s.get(qsid::MAGIC, &mut out), Some(4));
        s.set(qsid::MOUSE_FIRST + 1, &[0, 0]);
        assert_eq!(s.mouse.interval_ms, 1);
        s.set(qsid::MOUSE_FIRST + 3, &[20, 0]);
        assert_eq!((s.get(12, &mut out), out[0]), (Some(2), 20));
    }

    #[test]
    fn dynamic_entries_in_vials_layout() {
        let mut k = Keymap::new();
        let td = [0x04, 0, 0xE0, 0, 0x29, 0, 0, 0, 150, 0];
        assert!(k.set_dynamic(Dynamic::TapDance, 0, &td));
        assert_eq!(
            *k.tap_dance(0).unwrap(),
            TapDance { on_tap: 0x04, on_hold: 0xE0, on_double_tap: 0x29, on_tap_hold: 0, tapping_term_ms: 150 }
        );
        let mut out = [0u8; 10];
        assert_eq!(k.dynamic(Dynamic::TapDance, 0, &mut out), Some(10));
        assert_eq!(out, td);
        let ko = [0x2A, 0, 0x4C, 0, 0xFF, 0, 0x22, 0, 0x22, 0x87];
        assert!(k.set_dynamic(Dynamic::KeyOverride, 1, &ko));
        assert_eq!(k.key_overrides()[1].suppressed_mods, 0x22);
        assert_eq!(k.dynamic(Dynamic::KeyOverride, 1, &mut out), Some(10));
        assert_eq!(out, ko);
        assert!(!k.set_dynamic(Dynamic::Combo, COMBOS, &[0; 10]), "past the last");
        assert_eq!(k.dynamic(Dynamic::AltRepeat, ALT_REPEATS, &mut out), None);
    }
}
