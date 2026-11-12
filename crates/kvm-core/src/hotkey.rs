//! Hotkeys (software.md §4.5) and their text form for `odkvmctl` (`double:scrolllock`, `ctrl+alt+1`, `none`).

use core::fmt::Write;

use heapless::String;
use kvm_hid::KeyState;
use kvm_hid::keys::{modifier_of, name_of, usage_of};
use kvm_proto::ctrl::{Hotkey, HotkeyKind};

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Action {
    Toggle,
    SelectPc1,
    SelectPc2,
    /// The next target: PC1, PC2, then the Bluetooth hosts that are ready.
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tap {
    Idle,
    FirstDown { at: u32 },
    FirstUp { at: u32 },
    SecondDown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotkeyOutput {
    /// Key state to forward to the active PC.
    pub keys: KeyState,
    pub action: Option<Action>,
    /// A swallowed single tap to replay (press + release) to the active PC before `keys`.
    pub replay: Option<u8>,
}

pub struct HotkeyEngine {
    bindings: [(Action, Hotkey); 4],
    window_ms: u32,
    tap: [Tap; 4],
    swallowed: KeyState,
    prev: KeyState,
}

fn is_modifier(k: u8) -> bool {
    (0xE0..=0xE7).contains(&k)
}

/// `want` with only left-side bits matches either side; otherwise the set must match exactly.
fn modifiers_match(have: u8, want: u8) -> bool {
    if want & 0xF0 == 0 { (have | have >> 4) & 0x0F == want } else { have == want }
}

/// Only `k` is held.
fn alone(state: &KeyState, k: u8) -> bool {
    if is_modifier(k) {
        state.key_count() == 0 && state.modifiers == 1 << (k - 0xE0)
    } else {
        state.modifiers == 0 && state.key_count() == 1 && state.is_pressed(k)
    }
}

impl HotkeyEngine {
    /// No bindings yet: [`HotkeyEngine::configure`] sets them.
    pub const EMPTY: HotkeyEngine = HotkeyEngine {
        bindings: [
            (Action::Toggle, Hotkey::NONE),
            (Action::SelectPc1, Hotkey::NONE),
            (Action::SelectPc2, Hotkey::NONE),
            (Action::Next, Hotkey::NONE),
        ],
        window_ms: 300,
        tap: [Tap::Idle; 4],
        swallowed: KeyState::EMPTY,
        prev: KeyState::EMPTY,
    };

    pub fn new(cfg: &Config) -> Self {
        let mut e = Self::EMPTY;
        e.configure(cfg);
        e
    }

    pub fn configure(&mut self, cfg: &Config) {
        self.bindings[0].1 = cfg.hotkey_toggle;
        self.bindings[1].1 = cfg.hotkey_pc1;
        self.bindings[2].1 = cfg.hotkey_pc2;
        self.bindings[3].1 = cfg.hotkey_next;
        self.window_ms = cfg.double_tap_ms as u32;
        self.tap = [Tap::Idle; 4];
    }

    /// Keys currently hidden from the PCs (a swallowed hotkey key that is still held).
    pub fn swallowed(&self) -> KeyState {
        self.swallowed
    }

    /// When `tick` next needs to run (a single tap is waiting to be replayed).
    pub fn deadline(&self) -> Option<u32> {
        self.tap.iter().find_map(|t| match t {
            Tap::FirstUp { at } => Some(at.wrapping_add(self.window_ms) + 1),
            _ => None,
        })
    }

    pub fn process(&mut self, raw: &KeyState, now: u32) -> HotkeyOutput {
        let pressed = raw.without(&self.prev);
        let mut out = HotkeyOutput::default();
        let window = self.window_ms;

        for i in 0..self.bindings.len() {
            let (action, hk) = self.bindings[i];
            let k = hk.key;
            match hk.kind {
                HotkeyKind::None => {}
                HotkeyKind::DoubleTap => {
                    let down = raw.is_pressed(k);
                    self.tap[i] = match self.tap[i] {
                        Tap::Idle if pressed.is_pressed(k) && alone(raw, k) => {
                            if hk.swallow {
                                self.swallowed.press(k);
                            }
                            Tap::FirstDown { at: now }
                        }
                        Tap::FirstDown { at } if !down => {
                            if now.wrapping_sub(at) <= window {
                                Tap::FirstUp { at }
                            } else {
                                // Held too long to be a tap: it was an ordinary key press.
                                if hk.swallow {
                                    out.replay = Some(k);
                                }
                                Tap::Idle
                            }
                        }
                        Tap::FirstDown { .. } if !alone(raw, k) => {
                            // A chord with another key: let the key through, late.
                            self.swallowed.release(k);
                            Tap::Idle
                        }
                        Tap::FirstUp { at }
                            if pressed.is_pressed(k) && alone(raw, k) && now.wrapping_sub(at) <= window =>
                        {
                            out.action = Some(action);
                            if hk.swallow {
                                self.swallowed.press(k);
                            }
                            Tap::SecondDown
                        }
                        Tap::FirstUp { .. } if !pressed.is_empty() => {
                            // Something else was typed: the single tap was meant as one.
                            if hk.swallow {
                                out.replay = Some(k);
                            }
                            Tap::Idle
                        }
                        Tap::SecondDown if !down => Tap::Idle,
                        t => t,
                    };
                }
                HotkeyKind::Combo => {
                    let others = raw.key_count() == 1 && raw.is_pressed(k);
                    if pressed.is_pressed(k) && others && modifiers_match(raw.modifiers, hk.modifiers) {
                        out.action = Some(action);
                        if hk.swallow {
                            self.swallowed.press(k);
                        }
                    }
                }
            }
        }

        out.keys = raw.without(&self.swallowed);
        self.swallowed = self.swallowed.intersect(raw);
        self.prev = *raw;
        out
    }

    /// Replays a single tap whose double-tap window has passed.
    pub fn tick(&mut self, now: u32) -> Option<u8> {
        for i in 0..self.bindings.len() {
            if let Tap::FirstUp { at } = self.tap[i]
                && now.wrapping_sub(at) > self.window_ms
            {
                self.tap[i] = Tap::Idle;
                let hk = self.bindings[i].1;
                if hk.swallow {
                    return Some(hk.key);
                }
            }
        }
        None
    }

    /// Starts afresh with `held` down (after learning a hotkey, when it didn't see the keys): no tap under way, and
    /// only keys pressed from now on count as pressed.
    pub fn resume(&mut self, held: &KeyState) {
        self.tap = [Tap::Idle; 4];
        self.swallowed = self.swallowed.intersect(held);
        self.prev = *held;
    }
}

/// The one key held (a modifier too), if only one is.
fn single(state: &KeyState) -> Option<u8> {
    match (state.key_count(), state.modifiers.count_ones()) {
        (1, 0) => state.keys().next(),
        (0, 1) => Some(0xE0 + state.modifiers.trailing_zeros() as u8),
        _ => None,
    }
}

/// Keys pressed from the moment none was held until all are released again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stroke {
    /// When its first key went down.
    at: u32,
    /// The key held, if it was the only one throughout.
    alone: Option<u8>,
    /// The last key (not a modifier) pressed, with the modifiers held then.
    combo: Option<(u8, u8)>,
    /// Two keys (not modifiers) were held at once: no combo.
    mixed: bool,
}

/// Learns a hotkey from the keys pressed (software.md §4.5), with [`HotkeyEngine`]'s timing, so that what it learns is
/// what the engine then recognises:
/// - modifiers and one key, then everything released: that combo (`ctrl+alt+k`);
/// - one key alone, a modifier too, tapped twice: a double tap. The first tap is released, and the second one
///   pressed, within the double-tap window of the first press;
/// - one key alone (not a modifier), pressed once: that key by itself (`f13`), once the window has passed or straight
///   away if it was held longer.
///
/// What makes none of them (modifiers alone, two keys at once) is forgotten, and learning goes on. Keys already held
/// when learning began are left out until released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Learner {
    window_ms: u32,
    ignored: KeyState,
    prev: KeyState,
    stroke: Option<Stroke>,
    /// A key just tapped alone, and when it was pressed: a second tap makes it a double tap.
    tap: Option<(u8, u32)>,
}

impl Learner {
    pub fn new(window_ms: u32, held: &KeyState) -> Self {
        Self { window_ms, ignored: *held, prev: KeyState::EMPTY, stroke: None, tap: None }
    }

    /// The keys it is looking at: those pressed since learning began, still held. They are hidden from the PCs.
    pub fn withheld(&self) -> KeyState {
        self.prev
    }

    /// The keyboards' combined state changed; returns the hotkey once one is learned.
    pub fn process(&mut self, raw: &KeyState, now: u32) -> Option<Hotkey> {
        self.ignored = self.ignored.intersect(raw);
        let keys = raw.without(&self.ignored);
        let pressed = keys.without(&self.prev);
        self.prev = keys;
        if keys.is_empty() {
            let s = self.stroke.take()?;
            return self.stroke_done(s, now);
        }
        let s = self.stroke.get_or_insert(Stroke { at: now, alone: single(&keys), combo: None, mixed: false });
        if s.alone != single(&keys) {
            s.alone = None;
        }
        for k in pressed.keys() {
            if keys.key_count() == 1 {
                s.combo = Some((keys.modifiers, k));
            } else {
                s.mixed = true;
            }
        }
        None
    }

    fn stroke_done(&mut self, s: Stroke, now: u32) -> Option<Hotkey> {
        let window = self.window_ms;
        let Some(k) = s.alone else {
            self.tap = None;
            return s.combo.filter(|_| !s.mixed).map(|(m, k)| Hotkey::combo(m, k));
        };
        if let Some((t, at)) = self.tap.take()
            && t == k
            && s.at.wrapping_sub(at) <= window
        {
            return Some(Hotkey::double_tap(k, true));
        }
        if now.wrapping_sub(s.at) <= window {
            // A tap: it may be the first of two.
            self.tap = Some((k, s.at));
            None
        } else {
            (!is_modifier(k)).then_some(Hotkey::combo(0, k))
        }
    }

    /// When `tick` next needs to run: a single tap waits for its window to pass.
    pub fn deadline(&self) -> Option<u32> {
        match (self.stroke, self.tap) {
            (None, Some((_, at))) => Some(at.wrapping_add(self.window_ms) + 1),
            _ => None,
        }
    }

    /// A single tap whose window has passed: that key by itself (a modifier alone makes no hotkey).
    pub fn tick(&mut self, now: u32) -> Option<Hotkey> {
        let (k, at) = self.tap.filter(|_| self.stroke.is_none())?;
        if now.wrapping_sub(at) <= self.window_ms {
            return None;
        }
        self.tap = None;
        (!is_modifier(k)).then_some(Hotkey::combo(0, k))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum SpecError {
    UnknownKey,
    UnknownModifier,
    NoKey,
}

/// Parses `none`, `double:<key>[:noswallow]` or `<modifier>+…+<key>`.
pub fn parse_spec(spec: &str) -> Result<Hotkey, SpecError> {
    let spec = spec.trim();
    if spec.eq_ignore_ascii_case("none") || spec.is_empty() {
        return Ok(Hotkey::NONE);
    }
    if let Some(rest) = spec.strip_prefix("double:") {
        let (name, swallow) = match rest.split_once(':') {
            Some((n, "noswallow")) => (n, false),
            Some(_) => return Err(SpecError::UnknownKey),
            None => (rest, true),
        };
        return usage_of(name).map(|k| Hotkey::double_tap(k, swallow)).ok_or(SpecError::UnknownKey);
    }
    let mut modifiers = 0u8;
    let mut key = None;
    for part in spec.split('+').map(str::trim) {
        if let Some(m) = modifier_of(part).filter(|_| key.is_none()) {
            modifiers |= m;
        } else if key.is_none() {
            key = Some(usage_of(part).filter(|k| !is_modifier(*k)).ok_or(SpecError::UnknownKey)?);
        } else {
            return Err(SpecError::UnknownModifier);
        }
    }
    key.map(|k| Hotkey::combo(modifiers, k)).ok_or(SpecError::NoKey)
}

/// Inverse of [`parse_spec`].
pub fn format_spec(h: &Hotkey) -> String<48> {
    let mut s = String::new();
    let key = name_of(h.key).unwrap_or("?");
    let _ = match h.kind {
        HotkeyKind::None => s.push_str("none").map_err(|_| core::fmt::Error),
        HotkeyKind::DoubleTap => write!(s, "double:{key}")
            .and_then(|_| if h.swallow { Ok(()) } else { s.push_str(":noswallow").map_err(|_| core::fmt::Error) }),
        HotkeyKind::Combo => {
            const NAMES: [&str; 8] = ["ctrl", "shift", "alt", "gui", "rctrl", "rshift", "ralt", "rgui"];
            // Left-only bits mean either side and print without the side; with right bits, left ones get an "l".
            let exact = h.modifiers & 0xF0 != 0;
            for (bit, name) in NAMES.iter().enumerate() {
                if h.modifiers & (1 << bit) != 0 {
                    let _ = if exact && bit < 4 { write!(s, "l{name}+") } else { write!(s, "{name}+") };
                }
            }
            write!(s, "{key}")
        }
    };
    s
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use kvm_hid::usage::key::SCROLL_LOCK;

    fn keys(list: &[u8]) -> KeyState {
        let mut s = KeyState::EMPTY;
        for k in list {
            s.press(*k);
        }
        s
    }

    fn engine() -> HotkeyEngine {
        let mut cfg = Config::default();
        cfg.hotkey_pc1 = Hotkey::combo(0x01 | 0x04, 0x1E); // ctrl+alt+1
        HotkeyEngine::new(&cfg)
    }

    #[test]
    fn double_tap_switches_and_hides_both_taps() {
        let mut e = engine();
        let sl = keys(&[SCROLL_LOCK]);
        let none = KeyState::EMPTY;
        let o = e.process(&sl, 0);
        assert_eq!((o.keys, o.action, o.replay), (none, None, None));
        assert_eq!(e.process(&none, 80).keys, none);
        let o = e.process(&sl, 200);
        assert_eq!((o.keys, o.action), (none, Some(Action::Toggle)));
        assert_eq!(e.process(&none, 260), HotkeyOutput::default());
        assert_eq!(e.tick(1000), None);
    }

    #[test]
    fn single_tap_is_replayed_after_the_window() {
        let mut e = engine();
        e.process(&keys(&[SCROLL_LOCK]), 0);
        e.process(&KeyState::EMPTY, 50);
        // The window runs from the first press.
        assert_eq!(e.deadline(), Some(301));
        assert_eq!(e.tick(300), None);
        assert_eq!(e.tick(301), Some(SCROLL_LOCK));
        assert_eq!(e.deadline(), None);
    }

    #[test]
    fn typing_after_a_tap_replays_it_first() {
        let mut e = engine();
        e.process(&keys(&[SCROLL_LOCK]), 0);
        e.process(&KeyState::EMPTY, 50);
        let o = e.process(&keys(&[0x04]), 100);
        assert_eq!((o.replay, o.action), (Some(SCROLL_LOCK), None));
        assert_eq!(o.keys, keys(&[0x04]));
    }

    #[test]
    fn slow_second_tap_is_not_a_double_tap() {
        let mut e = engine();
        e.process(&keys(&[SCROLL_LOCK]), 0);
        e.process(&KeyState::EMPTY, 50);
        assert_eq!(e.tick(400), Some(SCROLL_LOCK));
        let o = e.process(&keys(&[SCROLL_LOCK]), 450);
        assert_eq!(o.action, None);
    }

    #[test]
    fn chord_lets_the_key_through() {
        let mut e = engine();
        e.process(&keys(&[SCROLL_LOCK]), 0);
        let o = e.process(&keys(&[SCROLL_LOCK, 0x04]), 20);
        assert_eq!(o.keys, keys(&[SCROLL_LOCK, 0x04]));
    }

    #[test]
    fn combo_swallows_only_its_key_and_matches_either_side() {
        let mut e = engine();
        let mut held = keys(&[0xE4, 0xE2]); // right ctrl + left alt
        assert_eq!(e.process(&held, 0).keys, held);
        held.press(0x1E);
        let o = e.process(&held, 10);
        assert_eq!(o.action, Some(Action::SelectPc1));
        assert_eq!(o.keys, keys(&[0xE4, 0xE2]));
        held.release(0x1E);
        assert_eq!(e.process(&held, 20).keys, held);
        // Extra modifier: no match.
        let mut e = engine();
        let o = e.process(&keys(&[0xE0, 0xE2, 0xE1, 0x1E]), 0);
        assert_eq!(o.action, None);
    }

    const CTRL: u8 = 0xE0;
    const ALT: u8 = 0xE2;
    const RCTRL: u8 = 0xE4;
    const K: u8 = 0x0E;
    const F13: u8 = 0x68;

    /// Feeds `steps` (time, keys held) to a learner with a 300 ms window; the hotkey learned, and at which step.
    fn learn(steps: &[(u32, &[u8])]) -> Option<(usize, Hotkey)> {
        let mut l = Learner::new(300, &KeyState::EMPTY);
        steps.iter().enumerate().find_map(|(i, (t, held))| l.process(&keys(held), *t).map(|h| (i, h)))
    }

    #[test]
    fn learns_a_combo_once_everything_is_released() {
        let steps: &[(u32, &[u8])] =
            &[(0, &[CTRL]), (40, &[CTRL, ALT]), (90, &[CTRL, ALT, K]), (150, &[CTRL, ALT]), (160, &[])];
        assert_eq!(learn(steps), Some((4, Hotkey::combo(0x05, K))));
        // The modifiers as held when the key went down; a right-hand one stays right-hand.
        let steps: &[(u32, &[u8])] = &[(0, &[RCTRL]), (50, &[RCTRL, K]), (60, &[K]), (70, &[])];
        assert_eq!(learn(steps), Some((3, Hotkey::combo(0x10, K))));
        assert_eq!(format_spec(&Hotkey::combo(0x10, K)).as_str(), "rctrl+k");
    }

    #[test]
    fn learns_a_double_tap_with_the_matchers_timing() {
        let steps: &[(u32, &[u8])] = &[(0, &[SCROLL_LOCK]), (60, &[]), (200, &[SCROLL_LOCK]), (260, &[])];
        assert_eq!(learn(steps), Some((3, Hotkey::double_tap(SCROLL_LOCK, true))));
        // A modifier alone.
        let steps: &[(u32, &[u8])] = &[(0, &[RCTRL]), (60, &[]), (250, &[RCTRL]), (300, &[])];
        assert_eq!(learn(steps), Some((3, Hotkey::double_tap(RCTRL, true))));
        // The second press comes too late (the window runs from the first press): two single taps.
        let mut l = Learner::new(300, &KeyState::EMPTY);
        assert_eq!(l.process(&keys(&[RCTRL]), 0), None);
        assert_eq!(l.process(&KeyState::EMPTY, 60), None);
        assert_eq!(l.deadline(), Some(301));
        assert_eq!(l.tick(301), None, "a modifier alone is no hotkey");
        assert_eq!(l.process(&keys(&[RCTRL]), 350), None);
        assert_eq!(l.process(&KeyState::EMPTY, 400), None);
    }

    #[test]
    fn learns_a_single_key_after_the_window() {
        let mut l = Learner::new(300, &KeyState::EMPTY);
        assert_eq!(l.process(&keys(&[F13]), 0), None);
        assert_eq!(l.process(&KeyState::EMPTY, 80), None);
        assert_eq!(l.tick(300), None);
        assert_eq!(l.tick(301), Some(Hotkey::combo(0, F13)));
        assert_eq!(format_spec(&Hotkey::combo(0, F13)).as_str(), "f13");
        // Held longer than the window: no second tap can follow.
        assert_eq!(learn(&[(0, &[F13]), (500, &[])]), Some((1, Hotkey::combo(0, F13))));
        // While the second tap is held, the window passing decides nothing.
        let mut l = Learner::new(300, &KeyState::EMPTY);
        l.process(&keys(&[F13]), 0);
        l.process(&KeyState::EMPTY, 50);
        l.process(&keys(&[F13]), 250);
        assert_eq!((l.deadline(), l.tick(400)), (None, None));
        assert_eq!(l.process(&KeyState::EMPTY, 450), Some(Hotkey::double_tap(F13, true)));
    }

    #[test]
    fn learning_skips_what_makes_no_hotkey_and_keys_held_at_the_start() {
        // Modifiers alone, two keys at once: nothing; then a combo.
        let steps: &[(u32, &[u8])] = &[
            (0, &[CTRL, ALT]),
            (50, &[]),
            (400, &[0x04]),
            (420, &[0x04, 0x05]),
            (480, &[]),
            (900, &[CTRL]),
            (950, &[CTRL, K]),
            (990, &[]),
        ];
        assert_eq!(learn(steps), Some((7, Hotkey::combo(0x01, K))));
        // Enter still held from typing the command: left out until released, then it counts.
        let enter = 0x28;
        let mut l = Learner::new(300, &keys(&[enter]));
        assert_eq!(l.process(&keys(&[enter, CTRL]), 0), None);
        assert_eq!(l.withheld(), keys(&[CTRL]));
        assert_eq!(l.process(&keys(&[CTRL]), 30), None);
        assert_eq!(l.process(&keys(&[CTRL, enter]), 60), None);
        assert_eq!(l.process(&KeyState::EMPTY, 90), Some(Hotkey::combo(0x01, enter)));
    }

    #[test]
    fn resume_counts_only_new_presses() {
        let mut e = engine();
        let held = keys(&[0xE0, 0xE2, 0x1E]);
        e.resume(&held);
        assert_eq!(e.process(&held, 0).action, None, "ctrl+alt+1 was down before");
        e.process(&keys(&[0xE0, 0xE2]), 10);
        assert_eq!(e.process(&held, 20).action, Some(Action::SelectPc1));
    }

    #[test]
    fn spec_roundtrip() {
        for s in ["none", "double:scrolllock", "double:rctrl:noswallow", "ctrl+alt+1", "lctrl+rshift+f12"] {
            let h = parse_spec(s).unwrap();
            assert_eq!(format_spec(&h).as_str(), s);
        }
        assert_eq!(parse_spec("ctrl+alt+1").unwrap(), Hotkey::combo(0x05, 0x1E));
        assert_eq!(parse_spec("Ctrl+Alt+F1").unwrap(), Hotkey::combo(0x05, 0x3A));
        assert_eq!(parse_spec("ctrl+alt"), Err(SpecError::NoKey));
        assert_eq!(parse_spec("ctrl+x+y"), Err(SpecError::UnknownModifier));
        assert_eq!(parse_spec("double:nope"), Err(SpecError::UnknownKey));
    }
}
