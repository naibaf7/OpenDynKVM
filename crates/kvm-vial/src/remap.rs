//! The keymap engine: the physical keyboard state in, the keyboard, media and mouse output out, the way QMK's action
//! layer does it.
//!
//! - Every change of the physical state becomes key events (releases first, then presses, by usage). A key's action
//!   is looked up when it goes down and kept until it comes up, so a layer change never strands a key.
//! - **Tap-hold** (mod-tap, layer-tap, TT, Space Cadet): undecided until the key comes up within the tapping term
//!   (tap), the term runs out (hold), another key goes down (hold-on-other-key-press) or another key goes down and up
//!   (permissive hold). Events in the meantime are held back and replayed after the decision.
//! - The output is recomputed from everything held, and every change becomes a [`Step`]; a tap becomes two.
//! - **Macros** play one step every [`MACRO_STEP_MS`] at least: the AT32F405 keeps 4 reports per endpoint, and a PC
//!   polling at 1 ms would otherwise lose taps.

use heapless::{Deque, Vec};
use kvm_hid::KeyState;

use crate::keycode::{self, Action, Hold, KC_NO, KC_TRNS, Layer, Mouse, Tap};
use crate::keymap::Keymap;
use crate::{LAYERS, POSITIONS, in_layout};

pub const CONSUMER_SLOTS: usize = 4;
pub const MAX_STEPS: usize = 40;
const BUFFER: usize = 16;
pub const MACRO_STEP_MS: u32 = 2;

// QMK's mouse key defaults (docs/feature_mouse_keys.md, accelerated mode).
const MOUSE_DELAY_MS: u32 = 10;
const MOUSE_INTERVAL_MS: u32 = 16;
const MOUSE_DELTA: i32 = 8;
const MOUSE_MAX_SPEED: i32 = 10;
const MOUSE_TIME_TO_MAX: i32 = 30;
const WHEEL_DELAY_MS: u32 = 10;
const WHEEL_INTERVAL_MS: u32 = 80;
const WHEEL_DELTA: i32 = 1;
const WHEEL_MAX_SPEED: i32 = 8;
const WHEEL_TIME_TO_MAX: i32 = 40;

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
    /// One-shot key held down, with the press count at the time: unchanged at release = tapped alone.
    OneShotMods(u8, u32),
    OneShotLayer(u8, u32),
    Nothing,
}

#[derive(Debug, Clone, Copy)]
struct Pending {
    pos: u8,
    tap: Tap,
    hold: Hold,
    at: u32,
}

#[derive(Debug, Clone, Copy)]
struct Ev {
    pos: u8,
    down: bool,
    at: u32,
}

#[derive(Debug, Clone, Copy)]
struct Play {
    pos: usize,
    end: usize,
    next_at: u32,
    /// A tap's release, due at `next_at`.
    release: Option<(u8, u8, u16)>,
}

#[derive(Debug, Clone, Copy, Default)]
struct MouseKeys {
    repeat: i32,
    next: Option<u32>,
    wheel_repeat: i32,
    next_wheel: Option<u32>,
}

pub struct Remap {
    phys: [u32; 8],
    held: [Option<Held>; POSITIONS],
    toggled: u32,
    oneshot_mods: u8,
    oneshot_layer: Option<u8>,
    oneshot_at: u32,
    presses: u32,
    pending: Option<Pending>,
    buffer: Deque<Ev, BUFFER>,
    overlay: Option<(u8, u8)>,
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

impl Out {
    pub const EMPTY: Out = Out { keys: KeyState::EMPTY, consumer: [0; CONSUMER_SLOTS], system: 0, buttons: 0 };
}

impl Remap {
    pub const fn new() -> Self {
        Self {
            phys: [0; 8],
            held: [None; POSITIONS],
            toggled: 0,
            oneshot_mods: 0,
            oneshot_layer: None,
            oneshot_at: 0,
            presses: 0,
            pending: None,
            buffer: Deque::new(),
            overlay: None,
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
                let (was, is) =
                    (self.phys[pos as usize / 32] >> (pos % 32) & 1, new[pos as usize / 32] >> (pos % 32) & 1);
                if was != is && (is == 1) == down {
                    self.event(km, Ev { pos, down, at: now });
                }
            }
        }
        self.phys = new;
        core::mem::take(&mut self.steps)
    }

    /// Timers: tap-hold decisions, one-shot timeout, macro steps, mouse keys. Call at [`Remap::deadline`].
    pub fn tick(&mut self, km: &mut Keymap, now: u32) -> Steps {
        self.now = now;
        self.steps.clear();
        if let Some(p) = self.pending
            && now.wrapping_sub(p.at) >= km.settings.tapping_term_ms as u32
        {
            self.resolve(km, true);
        }
        let timeout = km.settings.oneshot_timeout_ms as u32;
        if timeout != 0
            && (self.oneshot_mods != 0 || self.oneshot_layer.is_some())
            && now.wrapping_sub(self.oneshot_at) >= timeout
        {
            self.oneshot_mods = 0;
            self.oneshot_layer = None;
            self.emit();
        }
        for _ in 0..8 {
            match self.play {
                Some(p) if now.wrapping_sub(p.next_at) as i32 >= 0 => self.macro_step(km),
                _ => break,
            }
        }
        self.mouse_keys(now);
        core::mem::take(&mut self.steps)
    }

    /// When [`Remap::tick`] must run next.
    pub fn deadline(&self, km: &Keymap) -> Option<u32> {
        let mut d: Option<u32> = None;
        let mut at = |t: u32| d = Some(d.map_or(t, |d: u32| if (t.wrapping_sub(d) as i32) < 0 { t } else { d }));
        if let Some(p) = self.pending {
            at(p.at.wrapping_add(km.settings.tapping_term_ms as u32));
        }
        let timeout = km.settings.oneshot_timeout_ms as u32;
        if timeout != 0 && (self.oneshot_mods != 0 || self.oneshot_layer.is_some()) {
            at(self.oneshot_at.wrapping_add(timeout));
        }
        if let Some(p) = self.play {
            at(p.next_at);
        }
        if let Some(t) = self.mouse.next {
            at(t);
        }
        if let Some(t) = self.mouse.next_wheel {
            at(t);
        }
        d
    }

    /// Lets go of everything (the PC switched): held keys, a pending tap-hold, one-shots, a running macro. Toggled
    /// layers and the default layer stay.
    pub fn release_all(&mut self) -> Steps {
        self.steps.clear();
        self.held = [None; POSITIONS];
        self.pending = None;
        self.buffer.clear();
        self.overlay = None;
        self.oneshot_mods = 0;
        self.oneshot_layer = None;
        self.play = None;
        self.macro_keys = KeyState::EMPTY;
        self.macro_consumer = 0;
        self.mouse = MouseKeys::default();
        self.emit();
        core::mem::take(&mut self.steps)
    }

    /// The last output.
    pub fn output(&self) -> &Out {
        &self.last
    }

    /// Active layers, bit per layer.
    pub fn layers(&self, km: &Keymap) -> u32 {
        let mut bits = 1u32 << km.settings.default_layer | self.toggled;
        let mut tri = 0;
        for h in self.held.iter().flatten() {
            match *h {
                Held::Layer(l) | Held::LayerMods(l, _) | Held::OneShotLayer(l, _) => bits |= 1u32 << (l & 31),
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

    fn event(&mut self, km: &mut Keymap, ev: Ev) {
        if let Some(p) = self.pending {
            if !ev.down && ev.pos == p.pos {
                let tap = ev.at.wrapping_sub(p.at) < km.settings.tapping_term_ms as u32;
                self.resolve(km, !tap);
                // Its release may now have to wait behind another undecided key from the replay.
                self.event(km, ev);
                return;
            }
            if self.buffer.push_back(ev).is_err() {
                self.resolve(km, true);
                self.event(km, ev);
                return;
            }
            let other_tapped = !ev.down && self.buffer.iter().any(|b| b.down && b.pos == ev.pos);
            if (ev.down && km.settings.hold_on_other_key_press) || (other_tapped && km.settings.permissive_hold) {
                self.resolve(km, true);
            }
            return;
        }
        if ev.down {
            self.press(km, ev.pos, ev.at);
        } else {
            self.release(ev.pos);
        }
    }

    /// Decides the pending tap-hold key, then replays what was held back.
    fn resolve(&mut self, km: &mut Keymap, hold: bool) {
        let Some(p) = self.pending.take() else { return };
        if hold {
            self.held[p.pos as usize] = Some(match p.hold {
                Hold::Mods(m) => Held::Key(0, m),
                Hold::Layer(l) => Held::Layer(l),
            });
            self.emit();
        } else {
            match p.tap {
                Tap::Key(k, m) => {
                    let m = m | self.take_oneshot_mods(k);
                    self.overlay = Some((k, m));
                    self.emit();
                    self.overlay = None;
                    self.emit();
                }
                Tap::ToggleLayer(l) => self.toggled ^= 1u32 << (l & 31),
            }
        }
        let replay = core::mem::take(&mut self.buffer);
        for ev in replay {
            self.event(km, ev);
        }
    }

    fn take_oneshot_mods(&mut self, key: u8) -> u8 {
        if key != 0 && !is_modifier(key) { core::mem::take(&mut self.oneshot_mods) } else { 0 }
    }

    fn press(&mut self, km: &mut Keymap, pos: u8, at: u32) {
        self.presses = self.presses.wrapping_add(1);
        if !in_layout(pos) {
            self.held[pos as usize] = Some(Held::Pass(pos));
            self.emit();
            return;
        }
        let action = keycode::decode(self.lookup(km, pos));
        // A one-shot layer lasts for this one key.
        self.oneshot_layer = None;
        let held = match action {
            Action::TapHold(tap, hold) => {
                self.pending = Some(Pending { pos, tap, hold, at });
                return;
            }
            Action::Key(k, m) => Held::Key(k, m | self.take_oneshot_mods(k)),
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
            Action::Layer(Layer::Momentary(l)) => Held::Layer(l),
            Action::Layer(Layer::WithMods(l, m)) => Held::LayerMods(l, m),
            Action::Layer(Layer::Toggle(l)) => {
                self.toggled ^= 1u32 << (l & 31);
                Held::Nothing
            }
            Action::Layer(Layer::To(l)) => {
                self.toggled = 1u32 << (l & 31);
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
            Action::Layer(Layer::OneShot(l)) => Held::OneShotLayer(l, self.presses),
            Action::Layer(Layer::TriLower) => Held::Tri(true),
            Action::Layer(Layer::TriUpper) => Held::Tri(false),
            Action::OneShotMods(m) => Held::OneShotMods(m, self.presses),
            Action::Macro(n) => {
                self.start_macro(km, n, at);
                Held::Nothing
            }
            Action::GraveEscape => {
                let shifted = self.last.keys.modifiers & 0xAA != 0; // Shift or GUI
                Held::Key(if shifted { 0x35 } else { 0x29 }, 0)
            }
            Action::No | Action::Transparent | Action::Unsupported => Held::Nothing,
        };
        self.held[pos as usize] = Some(held);
        self.emit();
    }

    fn release(&mut self, pos: u8) {
        match self.held[pos as usize].take() {
            Some(Held::OneShotMods(m, presses)) if presses == self.presses => {
                self.oneshot_mods |= m;
                self.oneshot_at = self.now;
            }
            Some(Held::OneShotLayer(l, presses)) if presses == self.presses => {
                self.oneshot_layer = Some(l);
                self.oneshot_at = self.now;
            }
            _ => {}
        }
        self.emit();
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
                Held::LayerMods(_, m) | Held::OneShotMods(m, _) => out.keys.modifiers |= m,
                Held::Consumer(u) => push_consumer(&mut out, u),
                Held::System(b) => out.system |= b,
                Held::Button(b) => out.buttons |= 1 << (b & 7),
                _ => {}
            }
        }
        // QMK reports armed one-shot modifiers right away.
        out.keys.modifiers |= self.oneshot_mods;
        out.keys = out.keys.union(&self.macro_keys);
        push_consumer(&mut out, self.macro_consumer);
        if let Some((k, m)) = self.overlay {
            if k != 0 {
                out.keys.press(k);
            }
            out.keys.modifiers |= m;
        }
        out
    }

    fn emit(&mut self) {
        let out = self.compute();
        if out != self.last {
            self.last = out;
            if self.steps.is_full() {
                // Keep the final state at least.
                self.steps.pop();
            }
            let _ = self.steps.push(Step::State(out));
        }
    }

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
            self.play = Some(Play { pos: start, end, next_at: at, release: None });
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
            self.emit();
            return;
        }
        let buf = km.macros();
        if p.pos >= p.end || p.end > buf.len() {
            self.play = None;
            self.macro_keys = KeyState::EMPTY;
            self.macro_consumer = 0;
            self.emit();
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
        self.emit();
    }

    fn mouse_keys(&mut self, now: u32) {
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
                let mut unit = mouse_unit(self.mouse.repeat, accel, MOUSE_DELTA, MOUSE_MAX_SPEED, MOUSE_TIME_TO_MAX);
                if x != 0 && y != 0 {
                    unit = (unit * 7 / 10).max(1);
                }
                motion.x = (x * unit) as i16;
                motion.y = (y * unit) as i16;
                let gap = if self.mouse.repeat == 0 { MOUSE_DELAY_MS } else { MOUSE_INTERVAL_MS };
                self.mouse.repeat += 1;
                self.mouse.next = Some(now.wrapping_add(gap));
            }
            _ if (x, y) == (0, 0) => {
                self.mouse.repeat = 0;
                self.mouse.next = None;
            }
            _ => {}
        }
        match self.mouse.next_wheel {
            Some(t) if (wheel, pan) != (0, 0) && now.wrapping_sub(t) as i32 >= 0 => {
                let unit = mouse_unit(self.mouse.wheel_repeat, accel, WHEEL_DELTA, WHEEL_MAX_SPEED, WHEEL_TIME_TO_MAX);
                motion.wheel = (wheel * unit) as i8;
                motion.pan = (pan * unit) as i8;
                let gap = if self.mouse.wheel_repeat == 0 { WHEEL_DELAY_MS } else { WHEEL_INTERVAL_MS };
                self.mouse.wheel_repeat += 1;
                self.mouse.next_wheel = Some(now.wrapping_add(gap));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(ks: &[u8]) -> KeyState {
        let mut s = KeyState::EMPTY;
        for k in ks {
            s.press(*k);
        }
        s
    }

    /// The keyboard states (keys pressed, modifiers) in a run of steps.
    fn states(steps: &[Step]) -> std::vec::Vec<(std::vec::Vec<u8>, u8)> {
        steps
            .iter()
            .filter_map(|s| match s {
                Step::State(o) => Some((o.keys.keys().collect(), o.keys.modifiers)),
                _ => None,
            })
            .collect()
    }

    struct Rig {
        km: Keymap,
        r: Remap,
        held: std::vec::Vec<u8>,
        log: std::vec::Vec<Step>,
    }

    impl Rig {
        fn new() -> Self {
            Rig { km: Keymap::new(), r: Remap::new(), held: std::vec::Vec::new(), log: std::vec::Vec::new() }
        }
        fn down(&mut self, k: u8, t: u32) {
            self.held.push(k);
            self.input(t);
        }
        fn up(&mut self, k: u8, t: u32) {
            self.held.retain(|h| *h != k);
            self.input(t);
        }
        fn input(&mut self, t: u32) {
            let s = self.r.input(&mut self.km, &keys(&self.held), t);
            self.log.extend(s);
        }
        fn tick(&mut self, t: u32) {
            let s = self.r.tick(&mut self.km, t);
            self.log.extend(s);
        }
        fn take(&mut self) -> std::vec::Vec<(std::vec::Vec<u8>, u8)> {
            states(&core::mem::take(&mut self.log))
        }
    }

    const A: u8 = 0x04;
    const B: u8 = 0x05;
    const CAPS: u8 = 0x39;
    const SPACE: u8 = 0x2C;

    #[test]
    fn identity_keymap_changes_nothing() {
        let mut t = Rig::new();
        t.down(A, 0);
        t.down(0xE1, 1);
        t.up(A, 2);
        assert_eq!(t.take(), [(std::vec![A], 0), (std::vec![A], 0x02), (std::vec![], 0x02)]);
        // Keypad extras are not in the layout and pass through untouched.
        t.down(0xB0, 3);
        assert_eq!(t.take(), [(std::vec![0xB0], 0x02)]);
    }

    #[test]
    fn remap_and_momentary_layer() {
        let mut t = Rig::new();
        t.km.set(0, CAPS as usize, 0x00E0); // Caps Lock → Left Ctrl
        t.km.set(0, 0xE6, 0x5221); // Right Alt → MO(1)
        t.km.set(1, A as usize, 0x0050); // layer 1: A → Left arrow
        t.down(CAPS, 0);
        assert_eq!(t.take(), [(std::vec![], 0x01)]);
        t.up(CAPS, 1);
        t.down(0xE6, 2);
        t.down(A, 3);
        // The layer key itself sends nothing; A is the arrow while the layer is on, and stays so until released.
        t.up(0xE6, 4);
        t.up(A, 5);
        assert_eq!(t.take(), [(std::vec![], 0), (std::vec![0x50], 0), (std::vec![], 0)]);
    }

    #[test]
    fn mod_tap_tap_hold_and_permissive_hold() {
        let mut t = Rig::new();
        t.km.set(0, CAPS as usize, 0x2129); // MT(MOD_LCTL, KC_ESC)
        // Tap: Escape pressed and released.
        t.down(CAPS, 0);
        t.up(CAPS, 50);
        assert_eq!(t.take(), [(std::vec![0x29], 0), (std::vec![], 0)]);
        // Held past the tapping term: Ctrl, then Ctrl+A.
        t.down(CAPS, 100);
        t.tick(300);
        t.down(A, 310);
        t.up(A, 320);
        t.up(CAPS, 330);
        assert_eq!(t.take(), [(std::vec![], 1), (std::vec![A], 1), (std::vec![], 1), (std::vec![], 0)]);
        // Rolled within the term, default settings: still a tap, A follows.
        t.down(CAPS, 400);
        t.down(A, 420);
        t.up(CAPS, 440);
        t.up(A, 460);
        assert_eq!(t.take(), [(std::vec![0x29], 0), (std::vec![], 0), (std::vec![A], 0), (std::vec![], 0)]);
        // Permissive hold: A pressed and released inside the term makes it Ctrl+A.
        t.km.settings.permissive_hold = true;
        t.down(CAPS, 500);
        t.down(A, 510);
        t.up(A, 520);
        t.up(CAPS, 530);
        assert_eq!(t.take(), [(std::vec![], 1), (std::vec![A], 1), (std::vec![], 1), (std::vec![], 0)]);
    }

    #[test]
    fn layer_tap_and_hold_on_other_key_press() {
        let mut t = Rig::new();
        t.km.settings.hold_on_other_key_press = true;
        t.km.set(0, SPACE as usize, 0x412C); // LT(1, KC_SPC)
        t.km.set(1, A as usize, 0x0029); // layer 1: A → Escape
        t.down(SPACE, 0);
        t.down(A, 10);
        t.up(A, 20);
        t.up(SPACE, 30);
        assert_eq!(t.take(), [(std::vec![0x29], 0), (std::vec![], 0)]);
        t.down(SPACE, 100);
        t.up(SPACE, 110);
        assert_eq!(t.take(), [(std::vec![SPACE], 0), (std::vec![], 0)]);
    }

    #[test]
    fn one_shot_modifier_and_toggle() {
        let mut t = Rig::new();
        t.km.set(0, 0xE1, 0x52A2); // OSM(MOD_LSFT)
        t.km.set(0, 0x35, 0x5261); // ` → TG(1)
        t.km.set(1, B as usize, 0x0006); // layer 1: B → C
        t.down(0xE1, 0);
        t.up(0xE1, 10);
        t.down(A, 20);
        t.up(A, 30);
        t.down(A, 40);
        // Shift is armed after the tap, applies to the next key only.
        assert_eq!(t.take(), [(std::vec![], 0x02), (std::vec![A], 0x02), (std::vec![], 0), (std::vec![A], 0)]);
        t.up(A, 50);
        t.down(0x35, 60);
        t.up(0x35, 70);
        t.down(B, 80);
        assert_eq!(t.take().last().unwrap(), &(std::vec![0x06], 0));
    }

    #[test]
    fn macro_types_text_with_paced_steps() {
        let mut t = Rig::new();
        t.km.write_macros(0, b"\0hA\x01\x05\x06\x01"); // macro 1: "hA", then tap LCTL(KC_C) as a 16-bit keycode
        t.km.set(0, 0x3A, 0x7701); // F1 → M1
        t.down(0x3A, 0);
        for k in 0..12 {
            t.tick(k * MACRO_STEP_MS);
        }
        assert_eq!(
            t.take(),
            [
                (std::vec![0x0B], 0),
                (std::vec![], 0),
                (std::vec![0x04], 0x02),
                (std::vec![], 0),
                (std::vec![0x06], 0x01),
                (std::vec![], 0)
            ]
        );
        assert_eq!(t.r.deadline(&t.km), None, "macro done");
    }

    #[test]
    fn mouse_keys_move_and_accelerate() {
        let mut t = Rig::new();
        t.km.set(0, 0x52, 0x00CD); // Up arrow → mouse up
        t.down(0x52, 0);
        let mut ys = std::vec::Vec::new();
        let mut now = 0;
        for _ in 0..40 {
            if let Some(d) = t.r.deadline(&t.km) {
                now = d;
            }
            for s in t.r.tick(&mut t.km, now) {
                if let Step::Motion(m) = s {
                    ys.push(m.y);
                }
            }
        }
        assert_eq!(ys[0], -8);
        assert!(ys.iter().all(|y| *y < 0) && *ys.last().unwrap() == -80, "{ys:?}");
        t.up(0x52, now + 1);
        t.tick(now + 100);
        assert_eq!(t.r.deadline(&t.km), None);
    }

    #[test]
    fn release_all_clears_held_keys_but_keeps_toggles() {
        let mut t = Rig::new();
        t.km.set(0, 0x35, 0x5261); // TG(1)
        t.down(0x35, 0);
        t.up(0x35, 1);
        t.down(A, 2);
        let s = t.r.release_all();
        assert_eq!(states(&s), [(std::vec![], 0)]);
        assert_eq!(t.r.layers(&t.km), 0b11);
    }
}
