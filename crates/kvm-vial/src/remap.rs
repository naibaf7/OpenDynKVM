//! The keymap engine: the physical keyboard state in, the keyboard, media and mouse output out, the way QMK's action
//! layer does it (software.md §4.11).
//!
//! Every change of the physical state becomes key events (releases first, then presses, by usage), which pass three
//! stages, in QMK's order:
//!
//! 1. **Combos** ([`Remap::combo_event`]): a key whose keycode belongs to a combo waits until a combo is complete and
//!    no larger one is still possible, the combo term runs out, a key outside the combo goes down or one of the keys
//!    comes up. A complete combo presses its output on a slot of its own, held until the last of its keys is up; the
//!    other keys go on as they were pressed, with their own times.
//! 2. **Tap-hold** ([`Remap::event`]): a mod-tap, layer-tap or Space Cadet key is undecided until it comes up within
//!    the tapping term (tap), the term runs out (hold), another key goes down (hold on other key press; with chordal
//!    hold, a key on the same hand makes it a tap) or another key goes down and up (permissive hold). Events in the
//!    meantime are held back and replayed after the decision. Quick tap (pressed again soon after a tap) and flow
//!    tap (pressed soon after a letter) decide at once, as taps.
//! 3. **Actions** ([`Remap::press`], [`Remap::release`]): a key's action is looked up when it goes down and kept until
//!    it comes up, so a layer change never strands a key. Tap dance, auto shift, caps word, the repeat keys, layer
//!    lock and the magic keycodes live here.
//!
//! The output is recomputed from everything held after each event, key overrides are applied to it
//! ([`Remap::overrides`]), and every change becomes a [`Step`]; a tap becomes two. Macros play one step every
//! [`MACRO_STEP_MS`] at least: the AT32F405 keeps 4 reports per endpoint, and a PC polling at 1 ms would otherwise
//! lose taps.

use heapless::{Deque, Vec};
use kvm_hid::KeyState;

use crate::keycode::{self, Action, AutoShift, Hold, KC_NO, KC_TRNS, Layer, Mouse, Switch, Tap, mods};
use crate::keymap::{COMBOS, KeyOverride, Keymap, TapDance, arep, auto_shift, ko};
use crate::{LAYERS, POSITIONS, in_layout};

pub const CONSUMER_SLOTS: usize = 4;
pub const MAX_STEPS: usize = 40;
const BUFFER: usize = 16;
pub const MACRO_STEP_MS: u32 = 2;
/// The keyboard's positions, then one slot per combo for its output.
const SLOTS: usize = POSITIONS + COMBOS;
/// Keys the combo stage can hold back: four for a combo, more for overlapping ones.
const COMBO_WAIT: usize = 8;
const NO_COMBO: u8 = 0xFF;
/// No trigger key: an override of modifiers alone.
const NO_SLOT: u16 = u16::MAX;
/// QMK's `CAPS_WORD_IDLE_TIMEOUT`.
const CAPS_WORD_IDLE_MS: u32 = 5000;
/// QMK's `KEY_OVERRIDE_REPEAT_DELAY`.
const KO_REPEAT_DELAY_MS: u32 = 500;
const DELAYED: usize = 4;
const WHEEL_DELTA: i32 = 1;

/// What the keymap makes of the keyboard; media keys, system keys and buttons are merged with the physical ones by
/// the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Out {
    pub keys: KeyState,
    /// Consumer usages, 0 = empty slot.
    pub consumer: [u16; CONSUMER_SLOTS],
    /// Normalized system-control bits.
    pub system: u8,
    pub buttons: u8,
}

/// Mouse movement from the mouse keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Motion {
    pub x: i16,
    pub y: i16,
    pub wheel: i8,
    pub pan: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Step {
    State(Out),
    Motion(Motion),
}

pub type Steps = Vec<Step, MAX_STEPS>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Held {
    /// Not in the layout: the physical key itself.
    Pass(u8),
    Key(u8, u8),
    Consumer(u16),
    System(u8),
    Button(u8),
    Mouse(Mouse),
    Layer(u8),
    LayerMods(u8, u8),
    /// Tri-layer key: true = lower.
    Tri(bool),
    /// One-shot key held down, with the press count at the time (unchanged at release = tapped alone) and the taps
    /// of it just before (for tap toggle).
    OneShotMods(u8, u32, u8),
    OneShotLayer(u8, u32, u8),
    /// A tap-hold key settled as held, with its tap and the press count after its press (unchanged at release = no
    /// other key in between, for retro tapping).
    ModHold(u8, Tap, u32),
    LayerHold(u8, Tap, u32),
    /// TT: its layer while held; the taps of it just before, and when it went down.
    TapToggle(u8, u8, u32),
    /// A tap dance key while its dance runs, or an auto shift key not yet decided.
    Waiting,
    Nothing,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    pos: u8,
    tap: Tap,
    hold: Hold,
    at: u32,
    presses: u32,
}

#[derive(Debug, Clone, Copy)]
struct Ev {
    slot: u16,
    down: bool,
    at: u32,
    /// The keycode of a combo's output; a key's is looked up when it goes down.
    kc: Option<u16>,
    /// Pressed while a tap-hold key was undecided (no flow tap then).
    held_back: bool,
}

impl Ev {
    const fn key(pos: u8, down: bool, at: u32) -> Ev {
        Ev { slot: pos as u16, down, at, kc: None, held_back: false }
    }
}

#[derive(Debug, Clone, Copy)]
enum Decision {
    Hold,
    /// Tapped: pressed and released.
    Tap,
    /// Settled as tapped while still down (chordal hold): the tap is held until the key comes up.
    TapHeld,
}

#[derive(Debug, Clone, Copy)]
struct Play {
    pos: usize,
    end: usize,
    next_at: u32,
    /// A tap's release, due at `next_at`.
    release: Option<(u8, u8, u16)>,
    /// Text of the KVM's own to type instead of a macro (the auto shift report).
    text: Option<[u8; 6]>,
}

#[derive(Debug, Clone, Copy, Default)]
struct MouseKeys {
    repeat: i32,
    next: Option<u32>,
    wheel_repeat: i32,
    next_wheel: Option<u32>,
}

/// The last tap of a key, for quick tap and for counting taps (TT, one-shot tap toggle, auto shift).
#[derive(Debug, Clone, Copy)]
struct LastTap {
    slot: u16,
    at: u32,
    presses: u32,
    count: u8,
}

#[derive(Debug, Clone, Copy)]
struct Dance {
    idx: u8,
    slot: u16,
    count: u8,
    pressed: bool,
    /// The last press.
    at: u32,
}

#[derive(Debug, Clone, Copy)]
struct Shifting {
    slot: u16,
    key: u8,
    at: u32,
}

/// A key held back by the combo stage.
#[derive(Debug, Clone, Copy)]
struct Waiting {
    pos: u8,
    kc: u16,
    at: u32,
}

#[derive(Debug, Clone, Copy)]
struct Overrides {
    /// The active override and its trigger's slot.
    active: Option<(u8, u16)>,
    /// A trigger key kept out of the report after its override ended.
    suppress: Option<u16>,
    /// The last non-modifier key pressed: slot, usage (0 = not a key) and when; `count` counts them.
    last: Option<(u16, u8, u32)>,
    count: u32,
    /// `count` and the modifiers at the last evaluation.
    seen: u32,
    mods: u8,
    /// An activation waiting for the key repeat delay.
    defer: Option<u32>,
}

impl Overrides {
    const NONE: Overrides =
        Overrides { active: None, suppress: None, last: None, count: 0, seen: 0, mods: 0, defer: None };
}

pub struct Remap {
    phys: [u32; 8],
    held: [Option<Held>; SLOTS],
    toggled: u32,
    /// Layers Layer Lock (or a one-shot tap toggle) turned on, also in `toggled`.
    locked: u32,
    osl_locked: u32,
    /// Locked layers whose momentary key, pressed while locked, unlocks them when it comes up.
    unlock_at_release: u32,
    oneshot_mods: u8,
    oneshot_locked: u8,
    oneshot_layer: Option<u8>,
    oneshot_at: u32,
    presses: u32,
    pending: Option<Pending>,
    buffer: Deque<Ev, BUFFER>,
    waiting: Vec<Waiting, COMBO_WAIT>,
    combo_until: Option<u32>,
    /// The combo each position's key went into, while it is down.
    combo_owner: [u8; POSITIONS],
    combos_off: bool,
    last_tap: Option<LastTap>,
    /// The last press (when, its tap keycode), for flow tap.
    prev_press: Option<(u32, u16)>,
    dance: Option<Dance>,
    shifting: Option<Shifting>,
    /// Caps Word is on; its last key press.
    caps_word: Option<u32>,
    /// The Repeat Key's memory: keycode and modifiers.
    last_key: Option<(u16, u8)>,
    repeating: bool,
    /// A tap-hold key's tap is being sent (auto shift leaves it alone).
    tapping: bool,
    ko: Overrides,
    /// Taps whose release waits for the tap code delay: key, modifiers, when.
    delayed: Vec<(u8, u8, u32), DELAYED>,
    macro_keys: KeyState,
    macro_consumer: u16,
    play: Option<Play>,
    mouse: MouseKeys,
    last: Out,
    steps: Steps,
    now: u32,
}

impl Default for Remap {
    fn default() -> Self {
        Self::new()
    }
}

fn is_modifier(k: u8) -> bool {
    (0xE0..=0xE7).contains(&k)
}

fn push_consumer(out: &mut Out, u: u16) {
    if u != 0
        && !out.consumer.contains(&u)
        && let Some(slot) = out.consumer.iter_mut().find(|s| **s == 0)
    {
        *slot = u;
    }
}

/// US-layout key and Shift for a character of a macro's text.
fn ascii(c: u8) -> Option<(u8, bool)> {
    const SHIFTED_DIGITS: &[u8; 10] = b")!@#$%^&*(";
    Some(match c {
        b'a'..=b'z' => (0x04 + c - b'a', false),
        b'A'..=b'Z' => (0x04 + c - b'A', true),
        b'1'..=b'9' => (0x1E + c - b'1', false),
        b'0' => (0x27, false),
        b'\n' => (0x28, false),
        0x1B => (0x29, false),
        0x08 => (0x2A, false),
        b'\t' => (0x2B, false),
        b' ' => (0x2C, false),
        0x7F => (0x4C, false),
        _ => {
            if let Some(i) = SHIFTED_DIGITS.iter().position(|d| *d == c) {
                return Some((if i == 0 { 0x27 } else { 0x1D + i as u8 }, true));
            }
            const PUNCT: &[(u8, u8, u8)] = &[
                (b'-', b'_', 0x2D),
                (b'=', b'+', 0x2E),
                (b'[', b'{', 0x2F),
                (b']', b'}', 0x30),
                (b'\\', b'|', 0x31),
                (b';', b':', 0x33),
                (b'\'', b'"', 0x34),
                (b'`', b'~', 0x35),
                (b',', b'<', 0x36),
                (b'.', b'>', 0x37),
                (b'/', b'?', 0x38),
            ];
            let &(plain, _, k) = PUNCT.iter().find(|(p, s, _)| *p == c || *s == c)?;
            (k, c != plain)
        }
    })
}

fn mouse_unit(repeat: i32, accel: Option<u8>, delta: i32, max: i32, time_to_max: i32) -> i32 {
    let unit = match accel {
        Some(0) => delta * max / 4,
        Some(1) => delta * max / 2,
        Some(_) => delta * max,
        None if repeat == 0 => delta,
        None if repeat >= time_to_max => delta * max,
        None => delta * max * repeat / time_to_max,
    };
    unit.clamp(1, 127)
}

/// The keycode of a key and its modifiers, as a QMK keycode (for the Repeat Key's memory).
fn keycode_of(k: u8, m: u8) -> u16 {
    if m == 0 { k as u16 } else { keycode::qmk_mods(m) << 8 | k as u16 }
}

/// Keys auto shift covers (QMK's alpha, numeric and special groups).
fn shiftable(k: u8, flags: u8) -> bool {
    match k {
        0x04..=0x1D => flags & auto_shift::NO_ALPHA == 0,
        0x1E..=0x27 => flags & auto_shift::NO_NUMERIC == 0,
        0x2B | 0x2D..=0x38 | 0x64 => flags & auto_shift::NO_SPECIAL == 0,
        _ => false,
    }
}

/// Keys flow tap counts as typing: the letters, `, . ; /` and Space.
fn flow_key(kc: u16) -> bool {
    matches!(kc, 0x04..=0x1D | 0x2C | 0x33 | 0x36 | 0x37 | 0x38)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hand {
    Left,
    Right,
    /// Exempt from chordal hold's opposite hands rule: Space, the extra keys, a combo's output.
    Any,
}

/// Which hand types a position, on a QWERTY full-size keyboard: the left half of the main block (with F1–F5 and the
/// left modifiers) is the left hand; Space, F13–F24 and the language and international keys are either; the rest is
/// the right.
fn hand(slot: u16) -> Hand {
    const LEFT: &[u8] = &[
        0x29, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, // Escape, F1–F5
        0x35, 0x1E, 0x1F, 0x20, 0x21, 0x22, // ` 1–5
        0x2B, 0x14, 0x1A, 0x08, 0x15, 0x17, // Tab Q W E R T
        0x39, 0x04, 0x16, 0x07, 0x09, 0x0A, // Caps A S D F G
        0x64, 0x1D, 0x1B, 0x06, 0x19, 0x05, // ISO \ Z X C V B
        0xE0, 0xE1, 0xE2, 0xE3, // left modifiers
    ];
    if slot as usize >= POSITIONS {
        return Hand::Any;
    }
    let u = slot as u8;
    if LEFT.contains(&u) {
        Hand::Left
    } else if u == 0x2C || (0x68..=0x73).contains(&u) || (0x7F..=0x99).contains(&u) {
        Hand::Any
    } else {
        Hand::Right
    }
}

fn same_hand(a: u16, b: u16) -> bool {
    let (x, y) = (hand(a), hand(b));
    x != Hand::Any && x == y
}

/// QMK's key override modifier test: all of `req` (one side of a modifier enough where both are given), or any of
/// it with `one`.
fn mods_ok(active: u8, req: u8, one: bool) -> bool {
    if req == 0 {
        return true;
    }
    if one {
        return active & req != 0;
    }
    (0..4).all(|t| {
        let (l, r) = (req & (1 << t), req & (1 << (t + 4)));
        match (l != 0, r != 0) {
            (true, true) => active & (l | r) != 0,
            (true, false) => active & l != 0,
            (false, true) => active & r != 0,
            _ => true,
        }
    })
}

/// A keycode as a basic keycode plus HID modifiers (vial-qmk's `alt_repeat_key_normalize_keycode`).
fn normalize(kc: u16, m: u8) -> (u16, u8) {
    match kc {
        0x0100..=0x1FFF => (kc & 0xFF, m | keycode::hid_mods(kc >> 8)),
        0x2000..=0x4FFF => (kc & 0xFF, m),
        _ => (kc, m),
    }
}

fn arep_mods_ok(m: u8, required: u8, allowed: u8, options: u8) -> bool {
    let mut allowed = allowed | required;
    let (mut m, mut required) = (m, required);
    if options & arep::IGNORE_MOD_HANDEDNESS != 0 {
        let fold = |x: u8| (x & 0x0F) | (x >> 4);
        (m, required, allowed) = (fold(m), fold(required), fold(allowed));
    }
    m & required == required && m & !allowed == 0
}

/// The Alternate Repeat Key's keycode and modifiers after `kc` pressed with `m`: Vial's entries first (the one
/// requiring most modifiers wins), then QMK's own alternates, which keep the modifiers.
fn alternate(km: &Keymap, kc: u16, m: u8) -> Option<(u16, u8)> {
    let (basic, m) = normalize(kc, m);
    let mut alt = None;
    let mut best = -1i32;
    for e in km.alt_repeats().iter().filter(|e| e.options & arep::ENABLED != 0) {
        let (k, req) = normalize(e.keycode, 0);
        let (a, alt_req) = normalize(e.alt_keycode, 0);
        if k == basic && arep_mods_ok(m, req, e.allowed_mods, e.options) && req.count_ones() as i32 > best {
            alt = Some((a, alt_req));
            best = req.count_ones() as i32;
        }
        if e.options & arep::BIDIRECTIONAL != 0
            && a == basic
            && arep_mods_ok(m, alt_req, e.allowed_mods, e.options)
            && alt_req.count_ones() as i32 > best
        {
            alt = Some((k, req));
            best = alt_req.count_ones() as i32;
        }
        if e.options & arep::DEFAULT_TO_THIS_ALT_KEY != 0
            && best == -1
            && alt.is_none()
            && arep_mods_ok(m, 0, e.allowed_mods, e.options)
        {
            alt = Some((a, alt_req));
        }
    }
    match alt {
        Some(a) => (a.0 != KC_NO).then_some(a),
        None if basic <= 0xFF => keycode::default_alt(basic as u8, m).map(|b| (b as u16, m)),
        None => None,
    }
}

impl Out {
    pub const EMPTY: Out = Out { keys: KeyState::EMPTY, consumer: [0; CONSUMER_SLOTS], system: 0, buttons: 0 };
}

impl Remap {
    pub const fn new() -> Self {
        Self {
            phys: [0; 8],
            held: [None; SLOTS],
            toggled: 0,
            locked: 0,
            osl_locked: 0,
            unlock_at_release: 0,
            oneshot_mods: 0,
            oneshot_locked: 0,
            oneshot_layer: None,
            oneshot_at: 0,
            presses: 0,
            pending: None,
            buffer: Deque::new(),
            waiting: Vec::new(),
            combo_until: None,
            combo_owner: [NO_COMBO; POSITIONS],
            combos_off: false,
            last_tap: None,
            prev_press: None,
            dance: None,
            shifting: None,
            caps_word: None,
            last_key: None,
            repeating: false,
            tapping: false,
            ko: Overrides::NONE,
            delayed: Vec::new(),
            macro_keys: KeyState::EMPTY,
            macro_consumer: 0,
            play: None,
            mouse: MouseKeys { repeat: 0, next: None, wheel_repeat: 0, next_wheel: None },
            last: Out::EMPTY,
            steps: Vec::new(),
            now: 0,
        }
    }

    /// The physical keyboard changed.
    pub fn input(&mut self, km: &mut Keymap, phys: &KeyState, now: u32) -> Steps {
        self.now = now;
        self.steps.clear();
        let mut new = [0u32; 8];
        for pos in 0..POSITIONS as u8 {
            if phys.is_pressed(pos) {
                new[pos as usize / 32] |= 1 << (pos % 32);
            }
        }
        for down in [false, true] {
            for pos in 0..POSITIONS as u8 {
                let bit = |b: &[u32; 8]| b[pos as usize / 32] >> (pos % 32) & 1;
                let (was, is) = (bit(&self.phys), bit(&new));
                if was != is && (is == 1) == down {
                    self.combo_event(km, Ev::key(pos, down, now));
                }
            }
        }
        self.phys = new;
        core::mem::take(&mut self.steps)
    }

    /// Timers: combos, tap-hold decisions, tap dances, auto shift, one-shot and caps word timeouts, key override
    /// delays, delayed tap releases, macro steps, mouse keys. Call at [`Remap::deadline`].
    pub fn tick(&mut self, km: &mut Keymap, now: u32) -> Steps {
        self.now = now;
        self.steps.clear();
        let due = |t: u32| now.wrapping_sub(t) as i32 >= 0;
        if self.combo_until.is_some_and(due) {
            self.combo_flush(km);
        }
        if let Some(p) = self.pending
            && now.wrapping_sub(p.at) >= km.settings.tapping_term_ms as u32
        {
            self.resolve(km, Decision::Hold);
        }
        if let Some(d) = self.dance
            && now.wrapping_sub(d.at) >= self.dance_term(km, d.idx)
        {
            self.dance_finish(km, false);
        }
        if let Some(s) = self.shifting
            && now.wrapping_sub(s.at) >= km.settings.auto_shift_timeout_ms as u32
        {
            self.shift_timeout(km);
        }
        let timeout = km.settings.oneshot_timeout_ms as u32;
        if timeout != 0
            && (self.oneshot_mods != 0 || self.oneshot_layer.is_some())
            && now.wrapping_sub(self.oneshot_at) >= timeout
        {
            self.oneshot_mods = 0;
            self.oneshot_layer = None;
            self.emit(km);
        }
        if self.caps_word.is_some_and(|t| now.wrapping_sub(t) >= CAPS_WORD_IDLE_MS) {
            self.caps_word = None;
        }
        if self.ko.defer.is_some_and(due) {
            self.emit(km);
        }
        if self.delayed.iter().any(|d| due(d.2)) {
            self.delayed.retain(|d| !due(d.2));
            self.emit(km);
        }
        for _ in 0..8 {
            match self.play {
                Some(p) if due(p.next_at) => self.macro_step(km),
                _ => break,
            }
        }
        self.mouse_keys(km, now);
        core::mem::take(&mut self.steps)
    }

    /// When [`Remap::tick`] must run next.
    pub fn deadline(&self, km: &Keymap) -> Option<u32> {
        let s = &km.settings;
        let oneshot = (s.oneshot_timeout_ms != 0 && (self.oneshot_mods != 0 || self.oneshot_layer.is_some()))
            .then(|| self.oneshot_at.wrapping_add(s.oneshot_timeout_ms as u32));
        let soonest = |a: Option<u32>, t: u32| match a {
            Some(a) if (a.wrapping_sub(t) as i32) <= 0 => Some(a),
            _ => Some(t),
        };
        [
            self.pending.map(|p| p.at.wrapping_add(s.tapping_term_ms as u32)),
            self.combo_until,
            self.dance.map(|d| d.at.wrapping_add(self.dance_term(km, d.idx))),
            self.shifting.map(|a| a.at.wrapping_add(s.auto_shift_timeout_ms as u32)),
            oneshot,
            self.caps_word.map(|t| t.wrapping_add(CAPS_WORD_IDLE_MS)),
            self.ko.defer,
            self.delayed.iter().map(|d| d.2).fold(None, soonest),
            self.play.map(|p| p.next_at),
            self.mouse.next,
            self.mouse.next_wheel,
        ]
        .into_iter()
        .flatten()
        .fold(None, soonest)
    }

    /// Lets go of everything (the PC switched): held keys, waiting combo keys, a pending tap-hold, a tap dance,
    /// auto shift, one-shots (locked ones too), caps word, key overrides, a running macro. Toggled and locked layers
    /// and the default layer stay.
    pub fn release_all(&mut self) -> Steps {
        self.steps.clear();
        self.held = [None; SLOTS];
        self.pending = None;
        self.buffer.clear();
        self.waiting.clear();
        self.combo_until = None;
        self.combo_owner = [NO_COMBO; POSITIONS];
        self.dance = None;
        self.shifting = None;
        self.caps_word = None;
        self.last_tap = None;
        self.ko = Overrides::NONE;
        self.delayed.clear();
        self.unlock_at_release = 0;
        self.oneshot_mods = 0;
        self.oneshot_locked = 0;
        self.oneshot_layer = None;
        self.play = None;
        self.macro_keys = KeyState::EMPTY;
        self.macro_consumer = 0;
        self.mouse = MouseKeys::default();
        let out = self.compute();
        self.push(out);
        core::mem::take(&mut self.steps)
    }

    /// The last output.
    pub fn output(&self) -> &Out {
        &self.last
    }

    /// Whether Caps Word is on.
    pub fn caps_word(&self) -> bool {
        self.caps_word.is_some()
    }

    /// Active layers, bit per layer.
    pub fn layers(&self, km: &Keymap) -> u32 {
        let mut bits = 1u32 << km.settings.default_layer | self.toggled;
        let mut tri = 0;
        for h in self.held.iter().flatten() {
            match *h {
                Held::Layer(l)
                | Held::LayerMods(l, _)
                | Held::OneShotLayer(l, ..)
                | Held::LayerHold(l, ..)
                | Held::TapToggle(l, ..) => bits |= 1u32 << (l & 31),
                Held::Tri(lower) => {
                    let l = if lower { 1 } else { 2 };
                    tri |= l;
                    bits |= 1 << l;
                }
                _ => {}
            }
        }
        if tri == 3 {
            bits |= 1 << 3;
        }
        if let Some(l) = self.oneshot_layer {
            bits |= 1u32 << (l & 31);
        }
        bits & ((1 << LAYERS) - 1)
    }

    fn lookup(&self, km: &Keymap, pos: u8) -> u16 {
        let bits = self.layers(km);
        (0..LAYERS)
            .rev()
            .filter(|l| bits & (1 << l) != 0)
            .map(|l| km.get(l, pos as usize))
            .find(|kc| *kc != KC_TRNS)
            .unwrap_or(KC_NO)
    }

    // ---- Combos -------------------------------------------------------------------------------------------------

    fn combo_event(&mut self, km: &mut Keymap, ev: Ev) {
        let pos = ev.slot as usize;
        if !ev.down {
            let owner = self.combo_owner[pos];
            if owner != NO_COMBO {
                // A combo's key: the output stays until the last of them is up.
                self.combo_owner[pos] = NO_COMBO;
                if !self.combo_owner.contains(&owner) {
                    let slot = (POSITIONS + owner as usize) as u16;
                    self.event(km, Ev { slot, down: false, at: ev.at, kc: None, held_back: false });
                }
                return;
            }
            if self.waiting.iter().any(|w| w.pos as usize == pos) {
                // Let go before its combo fired: a complete combo fires (tapped), else the keys go on.
                self.combo_flush(km);
                return self.combo_event(km, ev);
            }
            return self.event(km, ev);
        }
        if !self.combos_off && in_layout(pos as u8) {
            let kc = self.lookup(km, pos as u8);
            if !self.waiting.is_full() && combo_could(km, &self.waiting, kc) {
                let _ = self.waiting.push(Waiting { pos: pos as u8, kc, at: ev.at });
                self.combo_until = Some(ev.at.wrapping_add(km.settings.combo_term_ms as u32));
                if best_combo(km, &self.waiting).is_some() && !combo_larger(km, &self.waiting) {
                    self.combo_flush(km);
                }
                return;
            }
        }
        if !self.waiting.is_empty() {
            self.combo_flush(km);
            return self.combo_event(km, ev);
        }
        self.event(km, ev);
    }

    /// Ends the wait: the largest complete combo fires, the other keys go on as pressed.
    fn combo_flush(&mut self, km: &mut Keymap) {
        self.combo_until = None;
        let waiting = core::mem::take(&mut self.waiting);
        let mut used = [false; COMBO_WAIT];
        if let Some(i) = best_combo(km, &waiting) {
            let combo = km.combos()[i];
            for k in combo.keys() {
                if let Some(j) = (0..waiting.len()).find(|&j| !used[j] && waiting[j].kc == *k) {
                    used[j] = true;
                    self.combo_owner[waiting[j].pos as usize] = i as u8;
                }
            }
            let slot = (POSITIONS + i) as u16;
            self.event(km, Ev { slot, down: true, at: self.now, kc: Some(combo.output), held_back: false });
        }
        for (_, w) in waiting.iter().enumerate().filter(|(j, _)| !used[*j]) {
            self.event(km, Ev::key(w.pos, true, w.at));
        }
    }

    // ---- Tap-hold -----------------------------------------------------------------------------------------------

    fn event(&mut self, km: &mut Keymap, mut ev: Ev) {
        if let Some(p) = self.pending {
            if !ev.down && ev.slot == p.pos as u16 {
                let tap = ev.at.wrapping_sub(p.at) < km.settings.tapping_term_ms as u32;
                self.resolve(km, if tap { Decision::Tap } else { Decision::Hold });
                // Its release may now have to wait behind another undecided key from the replay.
                self.event(km, ev);
                return;
            }
            ev.held_back = true;
            if self.buffer.push_back(ev).is_err() {
                self.resolve(km, Decision::Hold);
                self.event(km, ev);
                return;
            }
            let in_term = ev.at.wrapping_sub(p.at) < km.settings.tapping_term_ms as u32;
            if ev.down {
                // Chordal hold: a key on the same hand settles it as tapped (another tap-hold key waits its turn,
                // so that same-hand modifiers still combine).
                if km.settings.chordal_hold && in_term && same_hand(p.pos as u16, ev.slot) && !self.is_tap_hold(km, ev)
                {
                    self.resolve(km, Decision::TapHeld);
                } else if km.settings.hold_on_other_key_press {
                    self.resolve(km, Decision::Hold);
                }
            } else if km.settings.permissive_hold && self.buffer.iter().any(|b| b.down && b.slot == ev.slot) {
                self.resolve(km, Decision::Hold);
            }
            return;
        }
        if ev.down {
            self.press(km, ev);
        } else {
            self.release(km, ev.slot as usize);
        }
    }

    fn is_tap_hold(&self, km: &Keymap, ev: Ev) -> bool {
        let kc = match ev.kc {
            Some(kc) => kc,
            None if (ev.slot as usize) < POSITIONS => self.lookup(km, ev.slot as u8),
            None => KC_NO,
        };
        matches!(keycode::decode(kc), Action::TapHold(..))
    }

    /// Decides the pending tap-hold key, then replays what was held back.
    fn resolve(&mut self, km: &mut Keymap, d: Decision) {
        let Some(p) = self.pending.take() else { return };
        let slot = p.pos as usize;
        let Tap::Key(k, m) = p.tap;
        match d {
            Decision::Hold => {
                let held = match p.hold {
                    Hold::Mods(h) => {
                        // QMK's caps word: holding a mod-tap other than AltGr ends the word.
                        if h != mods::RALT {
                            self.caps_word = None;
                        }
                        Held::ModHold(h, p.tap, p.presses)
                    }
                    Hold::Layer(l) => {
                        self.note_unlock(l);
                        Held::LayerHold(l, p.tap, p.presses)
                    }
                };
                self.held[slot] = Some(held);
                self.emit(km);
            }
            Decision::Tap => {
                self.tap(km, slot, k, m, p.at);
                self.note_tap(slot, 1);
            }
            Decision::TapHeld => self.press_tap(km, slot, k, m, p.at),
        }
        let replay = core::mem::take(&mut self.buffer);
        for ev in replay {
            self.event(km, ev);
        }
    }

    /// The tap of a tap-hold key, held at `slot` (until the key comes up or [`Remap::tap`] lets go).
    fn press_tap(&mut self, km: &mut Keymap, slot: usize, k: u8, m: u8, at: u32) {
        self.tapping = true;
        self.press_action(km, slot, keycode_of(k, m), Action::Key(k, m), at);
        self.tapping = false;
    }

    /// Taps the tap of a tap-hold key; its release waits for the tap code delay (the Caps Lock delay for Caps Lock).
    fn tap(&mut self, km: &mut Keymap, slot: usize, k: u8, m: u8, at: u32) {
        self.press_tap(km, slot, k, m, at);
        let s = &km.settings;
        let delay = if k == 0x39 { s.tap_hold_caps_delay_ms } else { s.tap_code_delay_ms } as u32;
        match self.held[slot] {
            Some(Held::Key(k, m)) if delay > 0 && !self.delayed.is_full() => {
                self.held[slot] = None;
                let _ = self.delayed.push((k, m, self.now.wrapping_add(delay)));
            }
            _ => self.release(km, slot),
        }
    }

    /// Taps of the key at `slot` just before this press: tapped `count` times in a row, the last one less than
    /// `term` ago, with no other key pressed since.
    fn tap_count(&self, slot: usize, at: u32, term: u32) -> u8 {
        match self.last_tap {
            Some(t)
                if t.slot as usize == slot
                    && t.presses.wrapping_add(1) == self.presses
                    && at.wrapping_sub(t.at) < term =>
            {
                t.count
            }
            _ => 0,
        }
    }

    fn note_tap(&mut self, slot: usize, count: u8) {
        self.last_tap = Some(LastTap { slot: slot as u16, at: self.now, presses: self.presses, count });
    }

    fn quick_term(km: &Keymap) -> u32 {
        km.settings.quick_tap_term_ms.min(km.settings.tapping_term_ms) as u32
    }

    /// Flow tap: a tap-hold key pressed soon after a letter (both typing keys, no hotkey modifier held) is a tap.
    fn flow_tap(&self, km: &Keymap, prev: Option<(u32, u16)>, kc: u16, ev: Ev) -> bool {
        let term = km.settings.flow_tap_term_ms as u32;
        term > 0
            && !ev.held_back
            && self.real_mods() & (mods::CTRL | mods::GUI | mods::LALT) == 0
            && prev.is_some_and(|(t, p)| ev.at.wrapping_sub(t) < term && flow_key(p))
            && flow_key(keycode::tap_keycode(kc))
    }

    // ---- Actions ------------------------------------------------------------------------------------------------

    fn press(&mut self, km: &mut Keymap, ev: Ev) {
        self.presses = self.presses.wrapping_add(1);
        let slot = ev.slot as usize;
        self.interrupt(km, slot);
        if slot < POSITIONS && !in_layout(slot as u8) {
            self.held[slot] = Some(Held::Pass(slot as u8));
            if !is_modifier(slot as u8) {
                self.note_key(slot, 0);
            }
            self.emit(km);
            return;
        }
        let kc = match ev.kc {
            Some(kc) => kc,
            None => self.lookup(km, slot as u8),
        };
        // A one-shot layer lasts for this one key.
        self.oneshot_layer = None;
        let action = keycode::with_magic(keycode::decode(kc), km.settings.magic);
        let prev = self.prev_press.replace((ev.at, keycode::tap_keycode(kc)));
        if let Action::TapHold(tap @ Tap::Key(k, m), hold) = action {
            let quick = self.tap_count(slot, ev.at, Self::quick_term(km)) > 0;
            // A combo's output has no key of its own to hold: it is its tap.
            if slot >= POSITIONS || quick || self.flow_tap(km, prev, kc, ev) {
                self.press_tap(km, slot, k, m, ev.at);
            } else {
                self.pending = Some(Pending { pos: slot as u8, tap, hold, at: ev.at, presses: self.presses });
            }
            return;
        }
        self.press_action(km, slot, kc, action, ev.at);
    }

    /// Another key went down: a running tap dance is interrupted, an undecided auto shift key is its plain self.
    fn interrupt(&mut self, km: &mut Keymap, slot: usize) {
        if self.dance.is_some_and(|d| d.slot as usize != slot) {
            self.dance_finish(km, true);
        }
        if let Some(s) = self.shifting
            && s.slot as usize != slot
        {
            self.shifting = None;
            let at = s.slot as usize;
            self.held[at] = Some(Held::Key(s.key, self.take_oneshot_mods(s.key)));
            self.note_key(at, s.key);
            self.emit(km);
        }
    }

    /// Presses `kc` (decoded as `action`) at `slot`.
    fn press_action(&mut self, km: &mut Keymap, slot: usize, kc: u16, action: Action, at: u32) {
        let action = self.caps_word_press(action, at);
        if !self.repeating && remembered(action) {
            self.last_key = Some((kc, self.real_mods()));
        }
        let quick = Self::quick_term(km);
        let held = match action {
            Action::No | Action::Transparent | Action::Unsupported => Held::Nothing,
            Action::Key(k, m) => {
                if self.auto_shifts(km, slot, k, m, at) {
                    return;
                }
                Held::Key(k, m | self.take_oneshot_mods(k))
            }
            // A dance's or a repeated tap-hold keycode: its tap.
            Action::TapHold(Tap::Key(k, m), _) => Held::Key(k, m | self.take_oneshot_mods(k)),
            Action::Consumer(u) => Held::Consumer(u),
            Action::System(b) => Held::System(b),
            Action::Button(b) => Held::Button(b),
            Action::Mouse(m) => {
                if matches!(m, Mouse::WheelUp | Mouse::WheelDown | Mouse::WheelLeft | Mouse::WheelRight) {
                    self.mouse.next_wheel.get_or_insert(at);
                } else {
                    self.mouse.next.get_or_insert(at);
                }
                Held::Mouse(m)
            }
            Action::Layer(Layer::Momentary(l)) => {
                self.note_unlock(l);
                Held::Layer(l)
            }
            Action::Layer(Layer::WithMods(l, m)) => {
                self.note_unlock(l);
                Held::LayerMods(l, m)
            }
            Action::Layer(Layer::TapToggle(l)) => {
                self.note_unlock(l);
                Held::TapToggle(l, self.tap_count(slot, at, quick), at)
            }
            Action::Layer(Layer::Toggle(l)) => {
                self.toggled ^= 1u32 << (l & 31);
                self.locked &= self.toggled;
                Held::Nothing
            }
            Action::Layer(Layer::To(l)) => {
                self.toggled = 1u32 << (l & 31);
                self.locked &= self.toggled;
                Held::Nothing
            }
            Action::Layer(Layer::Default(l, persist)) => {
                if (l as usize) < LAYERS {
                    km.settings.default_layer = l;
                    if persist {
                        km.settings_changed();
                    }
                }
                Held::Nothing
            }
            Action::Layer(Layer::OneShot(l)) => Held::OneShotLayer(l, self.presses, self.tap_count(slot, at, quick)),
            Action::Layer(Layer::TriLower) => Held::Tri(true),
            Action::Layer(Layer::TriUpper) => Held::Tri(false),
            Action::OneShotMods(m) => Held::OneShotMods(m, self.presses, self.tap_count(slot, at, quick)),
            Action::Macro(n) => {
                self.start_macro(km, n, at);
                Held::Nothing
            }
            Action::GraveEscape => {
                let real = self.real_mods();
                let o = km.settings.grave_esc_override;
                let overridden = [(1, mods::ALT), (2, mods::CTRL), (4, mods::GUI), (8, mods::SHIFT)]
                    .iter()
                    .any(|&(bit, m)| o & bit != 0 && real & m != 0);
                let shifted = real & (mods::SHIFT | mods::GUI) != 0 && !overridden;
                Held::Key(if shifted { 0x35 } else { 0x29 }, 0)
            }
            Action::TapDance(n) => {
                self.dance_press(km, slot, n, at);
                return;
            }
            Action::CapsWord => {
                self.caps_word = if self.caps_word.is_some() { None } else { Some(at) };
                Held::Nothing
            }
            Action::Repeat => match self.last_key {
                Some((rk, rm)) if !self.repeating => return self.repeat(km, slot, rk, rm, at),
                _ => Held::Nothing,
            },
            Action::AltRepeat => match self.last_key.and_then(|(rk, rm)| alternate(km, rk, rm)) {
                Some((ak, am)) if !self.repeating => return self.repeat(km, slot, ak, am, at),
                _ => Held::Nothing,
            },
            Action::LayerLock => {
                self.layer_lock(km);
                Held::Nothing
            }
            Action::Magic(n) => {
                let m = keycode::magic_change(km.settings.magic, n);
                if m != km.settings.magic {
                    km.settings.magic = m;
                    km.settings_changed();
                }
                Held::Nothing
            }
            Action::Combos(s) => {
                self.combos_off = match s {
                    Switch::On => false,
                    Switch::Off => true,
                    Switch::Toggle => !self.combos_off,
                };
                Held::Nothing
            }
            Action::AutoShift(op) => {
                self.auto_shift_op(km, op, at);
                Held::Nothing
            }
        };
        match held {
            Held::Key(k, _) if k != 0 && !is_modifier(k) => self.note_key(slot, k),
            Held::Consumer(_) | Held::System(_) | Held::Button(_) | Held::Mouse(_) => self.note_key(slot, 0),
            _ => {}
        }
        self.held[slot] = Some(held);
        self.emit(km);
    }

    /// Presses a keycode at `slot` on behalf of a tap dance: a tap-hold keycode is its tap, a tap dance nothing.
    fn press_keycode(&mut self, km: &mut Keymap, slot: usize, kc: u16) {
        let action = match keycode::with_magic(keycode::decode(kc), km.settings.magic) {
            Action::TapHold(Tap::Key(k, m), _) => Action::Key(k, m),
            Action::TapDance(_) => Action::No,
            a => a,
        };
        self.press_action(km, slot, kc, action, self.now);
    }

    fn tap_keycode(&mut self, km: &mut Keymap, slot: usize, kc: u16) {
        let keep = self.held[slot].take();
        self.press_keycode(km, slot, kc);
        self.release(km, slot);
        self.held[slot] = keep;
    }

    /// The Repeat Keys: `kc` again, with the modifiers `m` it had (or the alternate's).
    fn repeat(&mut self, km: &mut Keymap, slot: usize, kc: u16, m: u8, at: u32) {
        let action = match keycode::with_magic(keycode::decode(kc), km.settings.magic) {
            Action::Key(k, km_) => Action::Key(k, km_ | m),
            Action::TapHold(Tap::Key(k, km_), _) => Action::Key(k, km_ | m),
            a => a,
        };
        self.repeating = true;
        self.press_action(km, slot, kc, action, at);
        self.repeating = false;
    }

    /// QMK's caps word on a key press: letters and `-` get Shift, digits, Backspace, Delete and `_` keep it going,
    /// layer keys and AltGr are ignored, anything else (or a modifier other than Shift and AltGr) ends it.
    fn caps_word_press(&mut self, action: Action, at: u32) -> Action {
        if self.caps_word.is_none() || matches!(action, Action::CapsWord | Action::Repeat | Action::AltRepeat) {
            return action;
        }
        self.caps_word = Some(at);
        if self.real_mods() & !(mods::SHIFT | mods::RALT) == 0 {
            match action {
                Action::Layer(
                    Layer::Momentary(_)
                    | Layer::To(_)
                    | Layer::Toggle(_)
                    | Layer::TapToggle(_)
                    | Layer::OneShot(_)
                    | Layer::TriLower
                    | Layer::TriUpper,
                )
                | Action::LayerLock
                | Action::Key(0xE6, 0)
                | Action::OneShotMods(mods::RALT) => return action,
                Action::Key(k @ (0x04..=0x1D | 0x2D), 0) => return Action::Key(k, mods::LSHIFT),
                Action::Key(0x1E..=0x27 | 0x2A | 0x4C, 0) | Action::Key(0x2D, mods::LSHIFT) => return action,
                _ => {}
            }
        }
        self.caps_word = None;
        action
    }

    /// Auto shift: a plain key it covers waits for its release (tapped: plain) or the timeout (shifted). Returns
    /// whether the key waits.
    fn auto_shifts(&mut self, km: &Keymap, slot: usize, k: u8, m: u8, at: u32) -> bool {
        let f = km.settings.auto_shift;
        if f & auto_shift::ENABLE == 0 || m != 0 || self.repeating || self.tapping || !shiftable(k, f) {
            return false;
        }
        let real = self.real_mods();
        if real & mods::SHIFT != 0 || (real != 0 && f & auto_shift::MODIFIERS == 0) {
            return false;
        }
        // Tapped and pressed again at once: the plain key, held (so it repeats).
        if f & auto_shift::REPEAT != 0 && self.tap_count(slot, at, km.settings.tapping_term_ms as u32) > 0 {
            return false;
        }
        self.shifting = Some(Shifting { slot: slot as u16, key: k, at });
        self.held[slot] = Some(Held::Waiting);
        true
    }

    fn shift_timeout(&mut self, km: &mut Keymap) {
        let Some(s) = self.shifting.take() else { return };
        let slot = s.slot as usize;
        let f = km.settings.auto_shift;
        self.held[slot] = Some(Held::Key(s.key, mods::LSHIFT | self.take_oneshot_mods(s.key)));
        self.note_key(slot, s.key);
        self.emit(km);
        if f & auto_shift::REPEAT == 0 || f & auto_shift::NO_AUTO_REPEAT != 0 {
            // Tapped, not held.
            self.held[slot] = Some(Held::Nothing);
            self.emit(km);
        }
    }

    fn auto_shift_op(&mut self, km: &mut Keymap, op: AutoShift, at: u32) {
        let s = &mut km.settings;
        match op {
            AutoShift::Down => s.auto_shift_timeout_ms = s.auto_shift_timeout_ms.saturating_sub(5),
            AutoShift::Up => s.auto_shift_timeout_ms = (s.auto_shift_timeout_ms + 5).min(1000),
            AutoShift::Set(sw) => {
                let on = match sw {
                    Switch::On => true,
                    Switch::Off => false,
                    Switch::Toggle => s.auto_shift & auto_shift::ENABLE == 0,
                };
                s.auto_shift = if on { s.auto_shift | auto_shift::ENABLE } else { s.auto_shift & !auto_shift::ENABLE };
            }
            AutoShift::Report => {
                // Types the timeout (at most 1000), as QMK's KC_ASRP does.
                let v = s.auto_shift_timeout_ms;
                let digits = [v / 1000 % 10, v / 100 % 10, v / 10 % 10, v % 10];
                let first = digits.iter().position(|d| *d != 0).unwrap_or(3);
                let mut text = [0u8; 6];
                for (t, d) in text.iter_mut().zip(&digits[first..]) {
                    *t = b'0' + *d as u8;
                }
                if self.play.is_none() {
                    self.play = Some(Play { pos: 0, end: 4 - first, next_at: at, release: None, text: Some(text) });
                }
                return;
            }
        }
        km.settings_changed();
    }

    /// Layer Lock: locks the highest active layer, or unlocks it.
    fn layer_lock(&mut self, km: &Keymap) {
        let active = self.layers(km) & !(1u32 << km.settings.default_layer);
        if active == 0 {
            return;
        }
        let bit = 1u32 << (31 - active.leading_zeros());
        if self.locked & bit != 0 {
            self.locked &= !bit;
            self.toggled &= !bit;
        } else {
            self.locked |= bit;
            self.toggled |= bit;
        }
    }

    /// A momentary key for a locked layer: it unlocks the layer when it comes up.
    fn note_unlock(&mut self, l: u8) {
        let bit = 1u32 << (l & 31);
        if self.locked & bit != 0 {
            self.unlock_at_release |= bit;
        }
    }

    fn unlock(&mut self, l: u8) -> bool {
        let bit = 1u32 << (l & 31);
        if self.unlock_at_release & bit == 0 {
            return false;
        }
        self.unlock_at_release &= !bit;
        self.locked &= !bit;
        self.toggled &= !bit;
        true
    }

    fn dance_entry(&self, km: &Keymap, n: u8) -> TapDance {
        km.tap_dance(n).copied().unwrap_or_default()
    }

    fn dance_term(&self, km: &Keymap, n: u8) -> u32 {
        match self.dance_entry(km, n).tapping_term_ms {
            0 => km.settings.tapping_term_ms as u32,
            t => t as u32,
        }
    }

    fn dance_press(&mut self, km: &mut Keymap, slot: usize, n: u8, at: u32) {
        let td = self.dance_entry(km, n);
        if td == TapDance::default() {
            self.held[slot] = Some(Held::Nothing);
            return;
        }
        match self.dance {
            Some(mut d) if d.slot as usize == slot && d.idx == n => {
                d.count = d.count.saturating_add(1);
                d.pressed = true;
                d.at = at;
                self.dance = Some(d);
                // vial-qmk's on_each_tap: the third tap types the tap keycode three times, each one after once more.
                let times = match d.count {
                    3 => 3,
                    c if c > 3 => 1,
                    _ => 0,
                };
                if td.on_tap != KC_NO {
                    for _ in 0..times {
                        self.tap_keycode(km, slot, td.on_tap);
                    }
                }
            }
            _ => {
                self.dance_finish(km, true);
                self.dance = Some(Dance { idx: n, slot: slot as u16, count: 1, pressed: true, at });
            }
        }
        self.held[slot] = Some(Held::Waiting);
    }

    fn dance_release(&mut self, km: &mut Keymap) {
        let Some(mut d) = self.dance else { return };
        d.pressed = false;
        self.dance = Some(d);
        let td = self.dance_entry(km, d.idx);
        // vial-qmk decides at once where waiting could change nothing.
        let early = (d.count == 1 && td.on_tap != 0 && td.on_hold != 0 && td.on_double_tap == 0 && td.on_tap_hold == 0)
            || (d.count == 2 && td.on_double_tap != 0);
        if early {
            self.dance_finish(km, false);
        }
    }

    /// The dance is decided (vial-qmk's `on_dance_finished`): single or double, tap or hold.
    fn dance_finish(&mut self, km: &mut Keymap, interrupted: bool) {
        let Some(d) = self.dance.take() else { return };
        let td = self.dance_entry(km, d.idx);
        let slot = d.slot as usize;
        let nz = |kc: u16| (kc != KC_NO).then_some(kc);
        let (first, down) = match (d.count, interrupted, d.pressed) {
            (1, true, _) | (1, _, false) => (None, nz(td.on_tap)),
            (1, ..) => (None, nz(td.on_hold).or(nz(td.on_tap))),
            (2, true, _) => (nz(td.on_tap), nz(td.on_tap)),
            (2, false, true) if td.on_tap_hold != 0 => (None, Some(td.on_tap_hold)),
            (2, false, true) if td.on_tap != 0 => (Some(td.on_tap), nz(td.on_hold).or(Some(td.on_tap))),
            (2, false, true) => (None, nz(td.on_hold)),
            (2, false, false) if td.on_double_tap != 0 => (None, Some(td.on_double_tap)),
            (2, false, false) => (nz(td.on_tap), nz(td.on_tap)),
            _ => (None, None),
        };
        if let Some(kc) = first {
            self.tap_keycode(km, slot, kc);
        }
        self.held[slot] = Some(Held::Nothing);
        if let Some(kc) = down {
            self.press_keycode(km, slot, kc);
        }
        if !d.pressed {
            self.release(km, slot);
        }
    }

    fn release(&mut self, km: &mut Keymap, slot: usize) {
        let now = self.now;
        match self.held[slot].take() {
            Some(Held::OneShotMods(m, presses, taps)) if presses == self.presses => {
                let n = taps.saturating_add(1);
                let lock = km.settings.oneshot_tap_toggle;
                if m != 0 && self.oneshot_locked & m == m {
                    self.oneshot_locked &= !m;
                    self.oneshot_mods &= !m;
                    self.last_tap = None;
                } else if lock != 0 && n >= lock {
                    self.oneshot_locked |= m;
                    self.oneshot_mods &= !m;
                    self.last_tap = None;
                } else {
                    self.oneshot_mods |= m;
                    self.oneshot_at = now;
                    self.note_tap(slot, n);
                }
            }
            Some(Held::OneShotLayer(l, presses, taps)) if presses == self.presses => {
                let bit = 1u32 << (l & 31);
                let n = taps.saturating_add(1);
                let lock = km.settings.oneshot_tap_toggle;
                if self.osl_locked & bit != 0 {
                    self.osl_locked &= !bit;
                    self.toggled &= !bit;
                    self.last_tap = None;
                } else if lock != 0 && n >= lock {
                    self.osl_locked |= bit;
                    self.toggled |= bit;
                    self.last_tap = None;
                } else {
                    self.oneshot_layer = Some(l);
                    self.oneshot_at = now;
                    self.note_tap(slot, n);
                }
            }
            Some(Held::TapToggle(l, taps, since)) => {
                if !self.unlock(l) && now.wrapping_sub(since) < km.settings.tapping_term_ms as u32 {
                    let n = taps.saturating_add(1);
                    if n >= km.settings.tapping_toggle.max(1) {
                        self.toggled ^= 1u32 << (l & 31);
                        self.locked &= self.toggled;
                        self.note_tap(slot, 0);
                    } else {
                        self.note_tap(slot, n);
                    }
                }
            }
            Some(Held::Layer(l) | Held::LayerMods(l, _)) => {
                self.unlock(l);
            }
            Some(Held::LayerHold(l, tap, presses)) => {
                self.unlock(l);
                self.retro(km, slot, tap, presses);
            }
            Some(Held::ModHold(_, tap, presses)) => self.retro(km, slot, tap, presses),
            Some(Held::Waiting) => {
                if self.dance.is_some_and(|d| d.slot as usize == slot) {
                    self.held[slot] = Some(Held::Waiting);
                    self.dance_release(km);
                    return;
                }
                if let Some(s) = self.shifting
                    && s.slot as usize == slot
                {
                    // Up before the timeout: the plain key, tapped.
                    self.shifting = None;
                    self.held[slot] = Some(Held::Key(s.key, self.take_oneshot_mods(s.key)));
                    self.note_key(slot, s.key);
                    self.emit(km);
                    self.held[slot] = None;
                    self.note_tap(slot, 1);
                }
            }
            _ => {}
        }
        if self.ko.suppress == Some(slot as u16) {
            self.ko.suppress = None;
        }
        self.emit(km);
    }

    /// Retro tapping: a tap-hold key held past the term and let go without another key sends its tap.
    fn retro(&mut self, km: &mut Keymap, slot: usize, tap: Tap, presses: u32) {
        if km.settings.retro_tapping && presses == self.presses {
            self.emit(km);
            let Tap::Key(k, m) = tap;
            self.tap(km, slot, k, m, self.now);
        }
    }

    fn take_oneshot_mods(&mut self, key: u8) -> u8 {
        if key != 0 && !is_modifier(key) { core::mem::take(&mut self.oneshot_mods) } else { 0 }
    }

    /// The modifiers held as such (QMK's `get_mods() | get_oneshot_mods()`), without those of modded keys.
    fn real_mods(&self) -> u8 {
        let mut m = self.oneshot_mods | self.oneshot_locked;
        for h in self.held.iter().flatten() {
            match *h {
                Held::Key(k, x) if k == 0 || is_modifier(k) => {
                    m |= x;
                    if is_modifier(k) {
                        m |= 1 << (k - 0xE0);
                    }
                }
                Held::Pass(k) if is_modifier(k) => m |= 1 << (k - 0xE0),
                Held::ModHold(x, ..) | Held::LayerMods(_, x) | Held::OneShotMods(x, ..) => m |= x,
                _ => {}
            }
        }
        m
    }

    /// A non-modifier key went down (usage 0: not a keyboard key), for key overrides.
    fn note_key(&mut self, slot: usize, usage: u8) {
        self.ko.last = Some((slot as u16, usage, self.now));
        self.ko.count = self.ko.count.wrapping_add(1);
    }

    fn compute(&self) -> Out {
        let mut out = Out::default();
        for h in self.held.iter().flatten() {
            match *h {
                Held::Pass(k) => out.keys.press(k),
                Held::Key(k, m) => {
                    if k != 0 {
                        out.keys.press(k);
                    }
                    out.keys.modifiers |= m;
                }
                Held::ModHold(m, ..) | Held::LayerMods(_, m) | Held::OneShotMods(m, ..) => out.keys.modifiers |= m,
                Held::Consumer(u) => push_consumer(&mut out, u),
                Held::System(b) => out.system |= b,
                Held::Button(b) => out.buttons |= 1 << (b & 7),
                _ => {}
            }
        }
        // QMK reports armed one-shot modifiers right away.
        out.keys.modifiers |= self.oneshot_mods | self.oneshot_locked;
        for &(k, m, _) in &self.delayed {
            if k != 0 {
                out.keys.press(k);
            }
            out.keys.modifiers |= m;
        }
        out.keys = out.keys.union(&self.macro_keys);
        push_consumer(&mut out, self.macro_consumer);
        out
    }

    /// The key override that applies now, if any: the trigger's slot ([`NO_SLOT`] for modifiers alone).
    fn ko_fits(&self, o: &KeyOverride, real: u8, layer: u32, no_keys: bool) -> Option<u16> {
        if o.options & ko::ENABLED == 0
            || o.layers & (1 << layer) == 0
            || real & o.negative_mods != 0
            || !mods_ok(real, o.trigger_mods, o.options & ko::ONE_MOD != 0)
        {
            return None;
        }
        if o.trigger == KC_NO {
            return no_keys.then_some(NO_SLOT);
        }
        let Action::Key(tk, tm) = keycode::decode(o.trigger) else { return None };
        // The trigger must be the last key pressed, and still down.
        let (slot, usage, _) = self.ko.last?;
        match self.held.get(slot as usize) {
            Some(Some(Held::Key(k, m))) if *k == usage && usage == tk && m & tm == tm => Some(slot),
            _ => None,
        }
    }

    /// Key overrides (QMK's), applied to the output: activation by the trigger, a required modifier going down or a
    /// negative one coming up (the latter two after a key repeat delay); deactivation when a trigger key comes up,
    /// another key goes down or a negative modifier goes down.
    fn overrides(&mut self, km: &Keymap, mut out: Out) -> Out {
        let list = km.key_overrides();
        if self.ko.active.is_none() && self.ko.suppress.is_none() && list.iter().all(|o| o.options & ko::ENABLED == 0) {
            return out;
        }
        let real = self.real_mods();
        let key_down = self.ko.count != self.ko.seen;
        let mod_down = real & !self.ko.mods != 0;
        let neg_up = self.ko.mods & !real;
        self.ko.seen = self.ko.count;
        self.ko.mods = real;
        let due = self.ko.defer.is_some_and(|t| self.now.wrapping_sub(t) as i32 >= 0);
        if due {
            self.ko.defer = None;
        }
        let layer = 31 - self.layers(km).leading_zeros();
        let no_keys = out.keys.key_count() == 0;
        if let Some((i, slot)) = self.ko.active {
            let o = &list[i as usize];
            let other = key_down
                && self.ko.last.is_some_and(|(s, ..)| s != slot)
                && o.options & ko::NO_UNREGISTER_ON_OTHER_KEY_DOWN == 0;
            if other || self.ko_fits(o, real, layer, no_keys) != Some(slot) {
                self.ko.active = None;
                // By default the trigger comes back if it is still down and nothing else was pressed.
                let still = slot != NO_SLOT && matches!(self.held[slot as usize], Some(Held::Key(..)));
                if still && (other || o.options & ko::NO_REREGISTER_TRIGGER != 0) {
                    self.ko.suppress = Some(slot);
                }
            }
        }
        if self.ko.active.is_none() {
            for (i, o) in list.iter().enumerate() {
                let Some(slot) = self.ko_fits(o, real, layer, no_keys) else { continue };
                let by_key = key_down && o.options & ko::ACTIVATION_TRIGGER_DOWN != 0;
                let by_mod = (mod_down && o.options & ko::ACTIVATION_REQUIRED_MOD_DOWN != 0)
                    || (neg_up & o.negative_mods != 0 && o.options & ko::ACTIVATION_NEGATIVE_MOD_UP != 0)
                    || due;
                if !by_key && !by_mod {
                    continue;
                }
                // Like an OS's key repeat delay: a modifier pressed while the trigger is down counts only once the
                // trigger has been down a while.
                if !by_key
                    && slot != NO_SLOT
                    && let Some((_, _, at)) = self.ko.last
                    && self.now.wrapping_sub(at) < KO_REPEAT_DELAY_MS
                {
                    self.ko.defer = Some(at.wrapping_add(KO_REPEAT_DELAY_MS));
                    continue;
                }
                self.ko.active = Some((i as u8, slot));
                self.ko.suppress = None;
                self.ko.defer = None;
                break;
            }
        }
        if let Some(slot) = self.ko.suppress {
            match self.held[slot as usize] {
                Some(Held::Key(k, _)) => out.keys.release(k),
                _ => self.ko.suppress = None,
            }
        }
        if let Some((i, slot)) = self.ko.active {
            let o = &list[i as usize];
            if slot != NO_SLOT
                && let Some(Held::Key(k, _)) = self.held[slot as usize]
            {
                out.keys.release(k);
            }
            out.keys.modifiers &= !o.suppressed_mods;
            match keycode::decode(o.replacement) {
                Action::Key(k, m) => {
                    if k != 0 {
                        out.keys.press(k);
                    }
                    out.keys.modifiers |= m;
                }
                Action::Consumer(u) => push_consumer(&mut out, u),
                _ => {}
            }
        }
        out
    }

    fn emit(&mut self, km: &Keymap) {
        let out = self.compute();
        let out = self.overrides(km, out);
        self.push(out);
    }

    fn push(&mut self, out: Out) {
        if out != self.last {
            self.last = out;
            if self.steps.is_full() {
                // Keep the final state at least.
                self.steps.pop();
            }
            let _ = self.steps.push(Step::State(out));
        }
    }

    // ---- Macros and mouse keys ----------------------------------------------------------------------------------

    fn start_macro(&mut self, km: &Keymap, n: u8, at: u32) {
        if self.play.is_some() {
            return;
        }
        let buf = km.macros();
        let mut start = 0;
        for _ in 0..n {
            match buf[start..].iter().position(|b| *b == 0) {
                Some(i) => start += i + 1,
                None => return,
            }
        }
        let end = buf[start..].iter().position(|b| *b == 0).map_or(buf.len(), |i| start + i);
        if start < end {
            self.play = Some(Play { pos: start, end, next_at: at, release: None, text: None });
        }
    }

    fn macro_key(&mut self, kc: u16, down: bool) {
        let (k, m, c) = match keycode::decode(kc) {
            Action::Key(k, m) => (k, m, 0),
            Action::Consumer(u) => (0, 0, u),
            _ => return,
        };
        if down {
            if k != 0 {
                self.macro_keys.press(k);
            }
            self.macro_keys.modifiers |= m;
            if c != 0 {
                self.macro_consumer = c;
            }
        } else {
            if k != 0 {
                self.macro_keys.release(k);
            }
            self.macro_keys.modifiers &= !m;
            if c != 0 && self.macro_consumer == c {
                self.macro_consumer = 0;
            }
        }
    }

    /// One step of the running macro (Vial's macro format, protocol ≥ 2).
    fn macro_step(&mut self, km: &Keymap) {
        let Some(mut p) = self.play else { return };
        let step = (km.settings.tap_code_delay_ms as u32).max(MACRO_STEP_MS);
        p.next_at = self.now.wrapping_add(step);
        if let Some((k, m, c)) = p.release.take() {
            if k != 0 {
                self.macro_keys.release(k);
            }
            self.macro_keys.modifiers &= !m;
            if c != 0 {
                self.macro_consumer = 0;
            }
            self.play = Some(p);
            self.emit(km);
            return;
        }
        let text = p.text;
        let buf: &[u8] = match &text {
            Some(t) => t,
            None => km.macros(),
        };
        if p.pos >= p.end || p.end > buf.len() {
            self.play = None;
            self.macro_keys = KeyState::EMPTY;
            self.macro_consumer = 0;
            self.emit(km);
            return;
        }
        let at = |i: usize| if i < p.end { buf[i] } else { 0 };
        // Tap = press now, release at the next step.
        let mut tap: Option<u16> = None;
        if at(p.pos) == 0x01 {
            match at(p.pos + 1) {
                code @ 1..=3 => {
                    let kc = at(p.pos + 2) as u16;
                    p.pos += 3;
                    if code == 1 { tap = Some(kc) } else { self.macro_key(kc, code == 2) }
                }
                4 => {
                    let ms = (at(p.pos + 2).saturating_sub(1)) as u32 + (at(p.pos + 3).saturating_sub(1)) as u32 * 255;
                    p.pos += 4;
                    p.next_at = self.now.wrapping_add(ms);
                }
                code @ 5..=7 => {
                    let raw = u16::from_le_bytes([at(p.pos + 2), at(p.pos + 3)]);
                    // Vial avoids NUL bytes: a keycode with a zero low byte is sent as 0xFF00 | high byte.
                    let kc = if raw & 0xFF00 == 0xFF00 { (raw & 0xFF) << 8 } else { raw };
                    p.pos += 4;
                    if code == 5 { tap = Some(kc) } else { self.macro_key(kc, code == 6) }
                }
                _ => p.pos += 2,
            }
        } else {
            let c = at(p.pos);
            p.pos += 1;
            if let Some((k, shift)) = ascii(c) {
                let kc = k as u16 | if shift { 0x0200 } else { 0 };
                tap = Some(kc);
            }
        }
        if let Some(kc) = tap {
            let (k, m, c) = match keycode::decode(kc) {
                Action::Key(k, m) => (k, m, 0),
                Action::Consumer(u) => (0, 0, u),
                _ => (0, 0, 0),
            };
            self.macro_key(kc, true);
            p.release = Some((k, m, c));
        }
        self.play = Some(p);
        self.emit(km);
    }

    fn mouse_keys(&mut self, km: &Keymap, now: u32) {
        let s = km.settings.mouse;
        let (mut x, mut y, mut wheel, mut pan, mut accel) = (0i32, 0i32, 0i32, 0i32, None);
        for h in self.held.iter().flatten() {
            if let Held::Mouse(m) = *h {
                match m {
                    Mouse::Up => y -= 1,
                    Mouse::Down => y += 1,
                    Mouse::Left => x -= 1,
                    Mouse::Right => x += 1,
                    Mouse::WheelUp => wheel += 1,
                    Mouse::WheelDown => wheel -= 1,
                    Mouse::WheelLeft => pan -= 1,
                    Mouse::WheelRight => pan += 1,
                    Mouse::Accel(a) => accel = Some(a),
                }
            }
        }
        let (x, y, wheel, pan) = (x.signum(), y.signum(), wheel.signum(), pan.signum());
        let mut motion = Motion::default();
        match self.mouse.next {
            Some(t) if (x, y) != (0, 0) && now.wrapping_sub(t) as i32 >= 0 => {
                let (delta, max, ttm) = (s.move_delta as i32, s.max_speed as i32, s.time_to_max as i32);
                let mut unit = mouse_unit(self.mouse.repeat, accel, delta, max, ttm);
                if x != 0 && y != 0 {
                    unit = (unit * 7 / 10).max(1);
                }
                motion.x = (x * unit) as i16;
                motion.y = (y * unit) as i16;
                let gap = if self.mouse.repeat == 0 { s.delay_ms } else { s.interval_ms.max(1) };
                self.mouse.repeat += 1;
                self.mouse.next = Some(now.wrapping_add(gap as u32));
            }
            _ if (x, y) == (0, 0) => {
                self.mouse.repeat = 0;
                self.mouse.next = None;
            }
            _ => {}
        }
        match self.mouse.next_wheel {
            Some(t) if (wheel, pan) != (0, 0) && now.wrapping_sub(t) as i32 >= 0 => {
                let (max, ttm) = (s.wheel_max_speed as i32, s.wheel_time_to_max as i32);
                let unit = mouse_unit(self.mouse.wheel_repeat, accel, WHEEL_DELTA, max, ttm);
                motion.wheel = (wheel * unit) as i8;
                motion.pan = (pan * unit) as i8;
                let gap = if self.mouse.wheel_repeat == 0 { s.wheel_delay_ms } else { s.wheel_interval_ms.max(1) };
                self.mouse.wheel_repeat += 1;
                self.mouse.next_wheel = Some(now.wrapping_add(gap as u32));
            }
            _ if (wheel, pan) == (0, 0) => {
                self.mouse.wheel_repeat = 0;
                self.mouse.next_wheel = None;
            }
            _ => {}
        }
        if motion != Motion::default() && !self.steps.is_full() {
            let _ = self.steps.push(Step::Motion(motion));
        }
    }
}

/// Whether the Repeat Key remembers a key (QMK's `remember_last_key`): not modifiers, layer switches, one-shot keys,
/// Layer Lock or the repeat keys themselves.
fn remembered(a: Action) -> bool {
    match a {
        Action::Key(k, _) => k != 0 && !is_modifier(k),
        Action::Layer(
            Layer::Momentary(_)
            | Layer::To(_)
            | Layer::Toggle(_)
            | Layer::TapToggle(_)
            | Layer::OneShot(_)
            | Layer::TriLower
            | Layer::TriUpper,
        )
        | Action::OneShotMods(_)
        | Action::LayerLock
        | Action::Repeat
        | Action::AltRepeat
        | Action::No
        | Action::Transparent => false,
        _ => true,
    }
}

/// Whether the keys held back could still become (part of) a combo with `kc`.
fn combo_could(km: &Keymap, waiting: &[Waiting], kc: u16) -> bool {
    km.combos().iter().any(|c| {
        let keys = c.keys();
        waiting.len() < keys.len() && keys.contains(&kc) && waiting.iter().all(|w| keys.contains(&w.kc))
    })
}

/// The complete combo with the most keys among those held back (the first of equals).
fn best_combo(km: &Keymap, waiting: &[Waiting]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, c) in km.combos().iter().enumerate() {
        let keys = c.keys();
        if !keys.is_empty()
            && keys.iter().all(|k| waiting.iter().any(|w| w.kc == *k))
            && best.is_none_or(|(_, n)| keys.len() > n)
        {
            best = Some((i, keys.len()));
        }
    }
    best.map(|(i, _)| i)
}

/// Whether a combo with more keys than those held back, all of them among its keys, is still possible.
fn combo_larger(km: &Keymap, waiting: &[Waiting]) -> bool {
    km.combos().iter().any(|c| {
        let keys = c.keys();
        keys.len() > waiting.len() && waiting.iter().all(|w| keys.contains(&w.kc))
    })
}

#[cfg(test)]
mod tests;
