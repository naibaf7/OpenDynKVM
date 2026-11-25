use super::*;
use crate::keycode::magic;
use crate::keymap::{Dynamic, auto_shift, ko};
use std::vec;
use std::vec::Vec as StdVec;

fn keys(ks: &[u8]) -> KeyState {
    let mut s = KeyState::EMPTY;
    for k in ks {
        s.press(*k);
    }
    s
}

/// The keyboard states (keys pressed, modifiers) in a run of steps.
fn states(steps: &[Step]) -> StdVec<(StdVec<u8>, u8)> {
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
    held: StdVec<u8>,
    /// Controls past the keyboard held down.
    controls: StdVec<u16>,
    log: StdVec<Step>,
}

impl Rig {
    fn new() -> Self {
        Rig { km: Keymap::new(), r: Remap::new(), held: StdVec::new(), controls: StdVec::new(), log: StdVec::new() }
    }
    fn control(&mut self, p: u16, down: bool, t: u32) {
        self.controls.retain(|c| *c != p);
        if down {
            self.controls.push(p);
        }
        self.input(t);
    }
    fn down(&mut self, k: u8, t: u32) {
        self.held.push(k);
        self.input(t);
    }
    fn up(&mut self, k: u8, t: u32) {
        self.held.retain(|h| *h != k);
        self.input(t);
    }
    /// Down at `t`, up 5 ms later.
    fn tap(&mut self, k: u8, t: u32) {
        self.down(k, t);
        self.up(k, t + 5);
    }
    fn input(&mut self, t: u32) {
        let mut p = crate::PosSet::from_keys(&keys(&self.held));
        for c in &self.controls {
            p.set(*c, true);
        }
        let s = self.r.input(&mut self.km, &p, t);
        self.log.extend(s);
    }
    fn tick(&mut self, t: u32) {
        let s = self.r.tick(&mut self.km, t);
        self.log.extend(s);
    }
    /// Runs every timer due up to `t`.
    fn until(&mut self, t: u32) {
        for _ in 0..1000 {
            match self.r.deadline(&self.km) {
                Some(d) if (d.wrapping_sub(t) as i32) <= 0 => self.tick(d),
                _ => return,
            }
        }
        panic!("a timer that never ends");
    }
    fn take(&mut self) -> StdVec<(StdVec<u8>, u8)> {
        states(&core::mem::take(&mut self.log))
    }
    fn combo(&mut self, i: usize, input: &[u8], output: u8) {
        let mut b = [0u8; 10];
        for (n, k) in input.iter().enumerate() {
            b[2 * n] = *k;
        }
        b[8] = output;
        assert!(self.km.set_dynamic(Dynamic::Combo, i, &b));
    }
    fn dance(&mut self, i: usize, kcs: [u16; 4], term: u16) {
        let mut b = [0u8; 10];
        for (n, k) in kcs.iter().chain([term].iter()).enumerate() {
            b[2 * n..2 * n + 2].copy_from_slice(&k.to_le_bytes());
        }
        assert!(self.km.set_dynamic(Dynamic::TapDance, i, &b));
    }
    fn key_override(&mut self, i: usize, trigger: u8, replacement: u8, mods: [u8; 3], options: u8) {
        let b = [trigger, 0, replacement, 0, 0xFF, 0xFF, mods[0], mods[1], mods[2], options];
        assert!(self.km.set_dynamic(Dynamic::KeyOverride, i, &b));
    }
    fn alt_repeat(&mut self, i: usize, kc: u16, alt: u16, allowed: u8, options: u8) {
        let (k, a) = (kc.to_le_bytes(), alt.to_le_bytes());
        assert!(self.km.set_dynamic(Dynamic::AltRepeat, i, &[k[0], k[1], a[0], a[1], allowed, options]));
    }
}

const A: u8 = 0x04;
const B: u8 = 0x05;
const C: u8 = 0x06;
const J: u8 = 0x0D;
const S: u8 = 0x16;
const Y: u8 = 0x1C;
const Z: u8 = 0x1D;
const ONE: u8 = 0x1E;
const ESC: u8 = 0x29;
const BSPC: u8 = 0x2A;
const TAB: u8 = 0x2B;
const SPACE: u8 = 0x2C;
const MINS: u8 = 0x2D;
const GRV: u8 = 0x35;
const CAPS: u8 = 0x39;
const F1: u8 = 0x3A;
const F2: u8 = 0x3B;
const DEL: u8 = 0x4C;
const RIGHT: u8 = 0x4F;
const LEFT: u8 = 0x50;
const LCTL: u8 = 0xE0;
const LSFT: u8 = 0xE1;
const LGUI: u8 = 0xE3;
const RALT: u8 = 0xE6;

#[test]
fn identity_keymap_changes_nothing() {
    let mut t = Rig::new();
    t.down(A, 0);
    t.down(LSFT, 1);
    t.up(A, 2);
    assert_eq!(t.take(), [(vec![A], 0), (vec![A], 0x02), (vec![], 0x02)]);
    // Keypad extras are not in the layout and pass through untouched.
    t.down(0xB0, 3);
    assert_eq!(t.take(), [(vec![0xB0], 0x02)]);
}

#[test]
fn remap_and_momentary_layer() {
    let mut t = Rig::new();
    t.km.set(0, CAPS as usize, 0x00E0); // Caps Lock → Left Ctrl
    t.km.set(0, RALT as usize, 0x5221); // Right Alt → MO(1)
    t.km.set(1, A as usize, 0x0050); // layer 1: A → Left arrow
    t.down(CAPS, 0);
    assert_eq!(t.take(), [(vec![], 0x01)]);
    t.up(CAPS, 1);
    t.down(RALT, 2);
    t.down(A, 3);
    // The layer key itself sends nothing; A is the arrow while the layer is on, and stays so until released.
    t.up(RALT, 4);
    t.up(A, 5);
    assert_eq!(t.take(), [(vec![], 0), (vec![LEFT], 0), (vec![], 0)]);
}

#[test]
fn mod_tap_tap_hold_and_permissive_hold() {
    let mut t = Rig::new();
    t.km.set(0, CAPS as usize, 0x2129); // MT(MOD_LCTL, KC_ESC)
    // Tap: Escape pressed and released.
    t.down(CAPS, 0);
    t.up(CAPS, 50);
    assert_eq!(t.take(), [(vec![ESC], 0), (vec![], 0)]);
    // Held past the tapping term (and past the quick tap term after the tap): Ctrl, then Ctrl+A.
    t.down(CAPS, 300);
    t.tick(500);
    t.down(A, 510);
    t.up(A, 520);
    t.up(CAPS, 530);
    assert_eq!(t.take(), [(vec![], 1), (vec![A], 1), (vec![], 1), (vec![], 0)]);
    // Rolled within the term, default settings: still a tap, A follows.
    t.down(CAPS, 800);
    t.down(A, 820);
    t.up(CAPS, 840);
    t.up(A, 860);
    assert_eq!(t.take(), [(vec![ESC], 0), (vec![], 0), (vec![A], 0), (vec![], 0)]);
    // Permissive hold: A pressed and released inside the term makes it Ctrl+A.
    t.km.settings.permissive_hold = true;
    t.down(CAPS, 1500);
    t.down(A, 1510);
    t.up(A, 1520);
    t.up(CAPS, 1530);
    assert_eq!(t.take(), [(vec![], 1), (vec![A], 1), (vec![], 1), (vec![], 0)]);
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
    assert_eq!(t.take(), [(vec![ESC], 0), (vec![], 0)]);
    t.down(SPACE, 100);
    t.up(SPACE, 110);
    assert_eq!(t.take(), [(vec![SPACE], 0), (vec![], 0)]);
}

#[test]
fn quick_tap_repeats_the_tap_and_retro_tapping_sends_it_after_a_lone_hold() {
    let mut t = Rig::new();
    t.km.set(0, CAPS as usize, 0x2129); // MT(MOD_LCTL, KC_ESC)
    // Tapped, then pressed again at once: Escape, held (so the PC repeats it), not Ctrl.
    t.down(CAPS, 0);
    t.up(CAPS, 50);
    t.down(CAPS, 100);
    t.until(400);
    t.up(CAPS, 410);
    assert_eq!(t.take(), [(vec![ESC], 0), (vec![], 0), (vec![ESC], 0), (vec![], 0)]);
    // With quick tap off it is Ctrl.
    t.km.settings.quick_tap_term_ms = 0;
    t.down(CAPS, 1000);
    t.up(CAPS, 1050);
    t.down(CAPS, 1100);
    t.until(1400);
    assert_eq!(t.take(), [(vec![ESC], 0), (vec![], 0), (vec![], 1)]);
    t.up(CAPS, 1410);
    // Retro tapping: held past the term with no other key, then let go: Ctrl, then Escape.
    t.km.settings.retro_tapping = true;
    t.down(CAPS, 2000);
    t.until(2300);
    t.up(CAPS, 2310);
    assert_eq!(t.take(), [(vec![], 0), (vec![], 1), (vec![], 0), (vec![ESC], 0), (vec![], 0)]);
}

#[test]
fn a_caps_lock_tap_is_held_for_the_caps_delay() {
    let mut t = Rig::new();
    t.km.set(0, CAPS as usize, 0x2139); // MT(MOD_LCTL, KC_CAPS)
    t.down(CAPS, 0);
    t.up(CAPS, 30);
    assert_eq!(t.take(), [(vec![CAPS], 0)]);
    t.until(109);
    assert!(t.take().is_empty());
    t.until(110);
    assert_eq!(t.take(), [(vec![], 0)]);
}

#[test]
fn chordal_hold_taps_on_the_same_hand_and_holds_across() {
    let mut t = Rig::new();
    t.km.settings.chordal_hold = true;
    t.km.settings.permissive_hold = true;
    t.km.set(0, A as usize, 0x2204); // MT(MOD_LSFT, KC_A)
    // C is on the same hand: A is tapped the moment C goes down.
    t.down(A, 0);
    t.down(C, 20);
    t.up(C, 30);
    t.up(A, 40);
    assert_eq!(t.take(), [(vec![A], 0), (vec![A, C], 0), (vec![A], 0), (vec![], 0)]);
    // J is on the other hand: permissive hold makes it Shift+J.
    t.down(A, 1000);
    t.down(J, 1020);
    t.up(J, 1030);
    t.up(A, 1040);
    assert_eq!(t.take(), [(vec![], 2), (vec![J], 2), (vec![], 2), (vec![], 0)]);
}

#[test]
fn flow_tap_taps_a_mod_tap_typed_right_after_a_letter() {
    let mut t = Rig::new();
    t.km.settings.flow_tap_term_ms = 150;
    t.km.set(0, A as usize, 0x2204); // MT(MOD_LSFT, KC_A)
    t.tap(S, 0);
    t.down(A, 50);
    assert_eq!(t.take(), [(vec![S], 0), (vec![], 0), (vec![A], 0)]);
    t.up(A, 60);
    // After a pause it is a mod-tap again.
    t.down(A, 1000);
    t.until(1300);
    assert_eq!(t.take(), [(vec![], 0), (vec![], 2)]);
}

#[test]
fn one_shot_modifier_and_toggle() {
    let mut t = Rig::new();
    t.km.set(0, LSFT as usize, 0x52A2); // OSM(MOD_LSFT)
    t.km.set(0, GRV as usize, 0x5261); // ` → TG(1)
    t.km.set(1, B as usize, 0x0006); // layer 1: B → C
    t.down(LSFT, 0);
    t.up(LSFT, 10);
    t.down(A, 20);
    t.up(A, 30);
    t.down(A, 40);
    // Shift is armed after the tap, applies to the next key only.
    assert_eq!(t.take(), [(vec![], 0x02), (vec![A], 0x02), (vec![], 0), (vec![A], 0)]);
    t.up(A, 50);
    t.down(GRV, 60);
    t.up(GRV, 70);
    t.down(B, 80);
    assert_eq!(t.take().last().unwrap(), &(vec![C], 0));
}

#[test]
fn a_one_shot_key_tapped_five_times_locks() {
    let mut t = Rig::new();
    t.km.set(0, LSFT as usize, 0x52A2); // OSM(MOD_LSFT)
    for i in 0..5 {
        t.tap(LSFT, i * 20);
    }
    t.take();
    t.tap(A, 200);
    t.tap(B, 210);
    assert_eq!(t.take(), [(vec![A], 2), (vec![], 2), (vec![B], 2), (vec![], 2)]);
    // One more tap unlocks.
    t.tap(LSFT, 300);
    t.tap(C, 310);
    assert_eq!(t.take(), [(vec![], 0), (vec![C], 0), (vec![], 0)]);
}

#[test]
fn tap_toggle_is_momentary_and_toggles_after_its_taps() {
    let mut t = Rig::new();
    t.km.settings.tapping_toggle = 2;
    t.km.set(0, F1 as usize, 0x52C1); // TT(1)
    t.down(F1, 0);
    assert_eq!(t.r.layers(&t.km), 0b11, "on at once while held");
    t.up(F1, 300);
    assert_eq!(t.r.layers(&t.km), 0b01);
    t.tap(F1, 400);
    assert_eq!(t.r.layers(&t.km), 0b01, "one tap of two");
    t.tap(F1, 450);
    assert_eq!(t.r.layers(&t.km), 0b11, "toggled");
    t.tap(F1, 1000);
    t.tap(F1, 1050);
    assert_eq!(t.r.layers(&t.km), 0b01, "toggled back");
}

#[test]
fn layer_lock_keeps_a_momentary_layer_on() {
    let mut t = Rig::new();
    t.km.set(0, RALT as usize, 0x5221); // Right Alt → MO(1)
    t.km.set(1, F1 as usize, 0x7C7B); // layer 1: F1 → Layer Lock
    t.km.set(1, A as usize, 0x0050); // layer 1: A → Left arrow
    t.down(RALT, 0);
    t.tap(F1, 10);
    t.up(RALT, 20);
    assert_eq!(t.r.layers(&t.km), 0b11);
    t.tap(A, 30);
    assert_eq!(t.take(), [(vec![LEFT], 0), (vec![], 0)]);
    // MO(1) again: it unlocks the layer when it comes up.
    t.down(RALT, 40);
    assert_eq!(t.r.layers(&t.km), 0b11);
    t.up(RALT, 50);
    assert_eq!(t.r.layers(&t.km), 0b01);
}

#[test]
fn macro_types_text_with_paced_steps() {
    let mut t = Rig::new();
    t.km.write_macros(0, b"\0hA\x01\x05\x06\x01"); // macro 1: "hA", then tap LCTL(KC_C) as a 16-bit keycode
    t.km.set(0, F1 as usize, 0x7701); // F1 → M1
    t.down(F1, 0);
    for k in 0..12 {
        t.tick(k * MACRO_STEP_MS);
    }
    assert_eq!(
        t.take(),
        [(vec![0x0B], 0), (vec![], 0), (vec![0x04], 0x02), (vec![], 0), (vec![0x06], 0x01), (vec![], 0)]
    );
    assert_eq!(t.r.deadline(&t.km), None, "macro done");
}

#[test]
fn mouse_keys_move_and_accelerate() {
    let mut t = Rig::new();
    t.km.set(0, 0x52, 0x00CD); // Up arrow → mouse up
    let run = |t: &mut Rig, from: u32| {
        t.down(0x52, from);
        let mut ys = StdVec::new();
        let mut now = from;
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
        t.up(0x52, now + 1);
        t.tick(now + 100);
        assert_eq!(t.r.deadline(&t.km), None);
        ys
    };
    let ys = run(&mut t, 0);
    assert_eq!(ys[0], -8);
    assert!(ys.iter().all(|y| *y < 0) && *ys.last().unwrap() == -80, "{ys:?}");
    // Vial's mouse key settings: step 4, top speed 5 (×4 = 20).
    t.km.settings.mouse.move_delta = 4;
    t.km.settings.mouse.max_speed = 5;
    let ys = run(&mut t, 10_000);
    assert_eq!((ys[0], *ys.last().unwrap()), (-4, -20), "{ys:?}");
}

#[test]
fn release_all_clears_held_keys_but_keeps_toggles() {
    let mut t = Rig::new();
    t.km.set(0, GRV as usize, 0x5261); // TG(1)
    t.down(GRV, 0);
    t.up(GRV, 1);
    t.down(A, 2);
    let s = t.r.release_all();
    assert_eq!(states(&s), [(vec![], 0)]);
    assert_eq!(t.r.layers(&t.km), 0b11);
}

#[test]
fn combo_presses_its_output_until_the_last_key_is_up() {
    let mut t = Rig::new();
    t.combo(0, &[A, B], ESC);
    t.down(A, 0);
    t.down(B, 10);
    assert_eq!(t.take(), [(vec![ESC], 0)]);
    t.up(A, 20);
    assert!(t.take().is_empty(), "held until the last key is up");
    t.up(B, 30);
    assert_eq!(t.take(), [(vec![], 0)]);
    // A alone: held back for the combo term, then pressed.
    t.down(A, 100);
    assert!(t.take().is_empty());
    t.until(150);
    t.up(A, 160);
    assert_eq!(t.take(), [(vec![A], 0), (vec![], 0)]);
    // A, then a key outside the combo: A goes on at once, then C.
    t.down(A, 200);
    t.down(C, 210);
    assert_eq!(t.take(), [(vec![A], 0), (vec![A, C], 0)]);
    t.up(A, 220);
    t.up(C, 230);
    // Turned off by CM_TOGG: A goes through at once.
    t.km.set(0, F1 as usize, 0x7C52);
    t.tap(F1, 300);
    t.take();
    t.down(A, 400);
    assert_eq!(t.take(), [(vec![A], 0)]);
}

#[test]
fn the_largest_complete_combo_wins() {
    let mut t = Rig::new();
    t.combo(0, &[A, B], ESC);
    t.combo(1, &[A, B, C], TAB);
    t.down(A, 0);
    t.down(B, 5);
    assert!(t.take().is_empty(), "A B C still possible");
    t.down(C, 8);
    assert_eq!(t.take(), [(vec![TAB], 0)]);
    t.up(A, 20);
    t.up(B, 21);
    t.up(C, 22);
    assert_eq!(t.take(), [(vec![], 0)]);
    // A and B, then the term runs out: A B.
    t.down(A, 100);
    t.down(B, 105);
    t.until(160);
    assert_eq!(t.take(), [(vec![ESC], 0)]);
    t.up(A, 170);
    t.up(B, 171);
    assert_eq!(t.take(), [(vec![], 0)]);
    // A and B, then A comes up before the term: A B, tapped.
    t.down(A, 200);
    t.down(B, 205);
    t.up(A, 210);
    assert_eq!(t.take(), [(vec![ESC], 0)]);
    t.up(B, 220);
    assert_eq!(t.take(), [(vec![], 0)]);
}

#[test]
fn a_combo_of_tap_hold_keys_and_a_tap_hold_key_alone() {
    let mut t = Rig::new();
    t.km.set(0, A as usize, 0x2204); // MT(MOD_LSFT, KC_A)
    let mut b = [0u8; 10];
    b[0..2].copy_from_slice(&0x2204u16.to_le_bytes());
    b[2] = S;
    b[8] = ESC;
    assert!(t.km.set_dynamic(Dynamic::Combo, 0, &b));
    t.down(A, 0);
    t.down(S, 10);
    assert_eq!(t.take(), [(vec![ESC], 0)]);
    t.up(A, 20);
    t.up(S, 30);
    t.take();
    // Alone and held: the combo wait counts towards the tapping term, from when it went down.
    t.down(A, 1000);
    t.until(1199);
    assert!(t.take().is_empty());
    t.until(1200);
    assert_eq!(t.take(), [(vec![], 2)]);
}

#[test]
fn tap_dance_single_tap_hold_double_tap_and_interrupt() {
    let mut t = Rig::new();
    t.dance(0, [A as u16, LCTL as u16, ESC as u16, 0], 0);
    t.km.set(0, F1 as usize, 0x5700); // TD(0)
    // One tap: A once the tapping term has passed (a double tap was still possible).
    t.down(F1, 0);
    t.up(F1, 50);
    assert!(t.take().is_empty());
    t.until(200);
    assert_eq!(t.take(), [(vec![A], 0), (vec![], 0)]);
    // Held: Left Ctrl until it comes up.
    t.down(F1, 1000);
    t.until(1200);
    assert_eq!(t.take(), [(vec![], 0x01)]);
    t.up(F1, 1300);
    assert_eq!(t.take(), [(vec![], 0)]);
    // Two taps: Escape, at the second release.
    t.down(F1, 2000);
    t.up(F1, 2050);
    t.down(F1, 2100);
    t.up(F1, 2150);
    assert_eq!(t.take(), [(vec![ESC], 0), (vec![], 0)]);
    // A tap, then B: A at once, then B.
    t.down(F1, 3000);
    t.up(F1, 3050);
    t.down(B, 3100);
    assert_eq!(t.take(), [(vec![A], 0), (vec![], 0), (vec![B], 0)]);
}

#[test]
fn tap_dance_decides_early_and_counts_taps() {
    let mut t = Rig::new();
    // Tap and hold only: a tap is known at its release.
    t.dance(1, [A as u16, LCTL as u16, 0, 0], 0);
    t.km.set(0, F1 as usize, 0x5701);
    t.down(F1, 0);
    t.up(F1, 30);
    assert_eq!(t.take(), [(vec![A], 0), (vec![], 0)]);
    // Tap and tap-hold: three taps type it three times, at the third press.
    t.dance(2, [B as u16, 0, 0, C as u16], 150);
    t.km.set(0, F2 as usize, 0x5702);
    t.tap(F2, 1000);
    t.tap(F2, 1050);
    t.down(F2, 1100);
    assert_eq!(t.take(), [(vec![B], 0), (vec![], 0), (vec![B], 0), (vec![], 0), (vec![B], 0), (vec![], 0)]);
    t.up(F2, 1110);
    t.until(1300);
    assert!(t.take().is_empty());
    // Tapped, then held: the tap-hold keycode.
    t.tap(F2, 2000);
    t.down(F2, 2050);
    t.until(2200);
    assert_eq!(t.take(), [(vec![C], 0)]);
    t.up(F2, 2300);
    assert_eq!(t.take(), [(vec![], 0)]);
}

#[test]
fn key_override_replaces_shift_backspace_with_delete() {
    let mut t = Rig::new();
    // Either Shift + Backspace → Delete, Shift suppressed, every activation allowed.
    t.key_override(0, BSPC, DEL, [0x22, 0, 0x22], ko::ENABLED | 0x07);
    t.down(LSFT, 0);
    t.down(BSPC, 10);
    assert_eq!(t.take(), [(vec![], 0x02), (vec![DEL], 0)]);
    t.up(BSPC, 20);
    assert_eq!(t.take(), [(vec![], 0x02)]);
    t.up(LSFT, 30);
    t.tap(BSPC, 40);
    assert_eq!(t.take(), [(vec![], 0), (vec![BSPC], 0), (vec![], 0)]);
    // Backspace first, then Shift: Delete only after the key repeat delay from Backspace's press.
    t.down(BSPC, 1000);
    t.down(LSFT, 1100);
    assert_eq!(t.take(), [(vec![BSPC], 0), (vec![BSPC], 0x02)]);
    t.until(1499);
    assert!(t.take().is_empty());
    t.until(1500);
    assert_eq!(t.take(), [(vec![DEL], 0)]);
    t.up(BSPC, 1600);
    t.up(LSFT, 1610);
    t.take();
    // Another key ends it, and Backspace stays away.
    t.down(LSFT, 2000);
    t.down(BSPC, 2010);
    t.down(A, 2020);
    assert_eq!(t.take(), [(vec![], 0x02), (vec![DEL], 0), (vec![A], 0x02)]);
    t.up(A, 2030);
    assert_eq!(t.take(), [(vec![], 0x02)]);
}

#[test]
fn key_override_negative_mods_and_layers() {
    let mut t = Rig::new();
    // Shift + Backspace → Delete on layer 0, not with Ctrl (a negative modifier).
    let mut b = [BSPC, 0, DEL, 0, 0b01, 0, 0x22, 0x11, 0x22, ko::ENABLED | 0x07];
    assert!(t.km.set_dynamic(Dynamic::KeyOverride, 0, &b));
    t.down(LSFT, 0);
    t.down(BSPC, 1);
    assert_eq!(t.take().last().unwrap(), &(vec![DEL], 0));
    t.up(BSPC, 2);
    t.down(LCTL, 3);
    t.down(BSPC, 4);
    assert_eq!(t.take().last().unwrap(), &(vec![BSPC], 0x03), "Ctrl held");
    t.up(BSPC, 5);
    t.up(LCTL, 6);
    t.up(LSFT, 7);
    t.take();
    // Moved to layer 1 only: not on layer 0.
    b[4] = 0b10;
    assert!(t.km.set_dynamic(Dynamic::KeyOverride, 0, &b));
    t.down(LSFT, 10);
    t.down(BSPC, 11);
    assert_eq!(t.take().last().unwrap(), &(vec![BSPC], 0x02));
    // A disabled entry does nothing either.
    t.up(BSPC, 12);
    b[4] = 0b01;
    b[9] = 0x07;
    assert!(t.km.set_dynamic(Dynamic::KeyOverride, 0, &b));
    t.down(BSPC, 13);
    assert_eq!(t.take().last().unwrap(), &(vec![BSPC], 0x02));
}

#[test]
fn caps_word_shifts_letters_and_ends_at_a_space() {
    let mut t = Rig::new();
    t.km.set(0, CAPS as usize, 0x7C73); // QK_CAPS_WORD_TOGGLE
    t.tap(CAPS, 0);
    assert!(t.r.caps_word());
    t.tap(A, 10);
    t.tap(MINS, 20);
    t.tap(ONE, 30);
    t.tap(SPACE, 40);
    t.tap(B, 50);
    assert_eq!(
        t.take(),
        [
            (vec![A], 2),
            (vec![], 0),
            (vec![MINS], 2),
            (vec![], 0),
            (vec![ONE], 0),
            (vec![], 0),
            (vec![SPACE], 0),
            (vec![], 0),
            (vec![B], 0),
            (vec![], 0)
        ]
    );
    // Idle for 5 s: off.
    t.tap(CAPS, 100);
    t.until(5200);
    assert!(!t.r.caps_word());
    // Ctrl ends it too.
    t.tap(CAPS, 6000);
    t.down(LCTL, 6010);
    t.tap(A, 6020);
    assert!(!t.r.caps_word());
}

#[test]
fn repeat_and_alternate_repeat() {
    let mut t = Rig::new();
    t.km.set(0, F1 as usize, 0x7C79); // QK_REPEAT_KEY
    t.km.set(0, F2 as usize, 0x7C7A); // QK_ALT_REPEAT_KEY
    // Ctrl+Z, then Repeat with Shift held: Ctrl+Shift+Z.
    t.down(LCTL, 0);
    t.tap(Z, 10);
    t.up(LCTL, 20);
    t.take();
    t.down(LSFT, 30);
    t.tap(F1, 40);
    t.up(LSFT, 50);
    assert_eq!(t.take(), [(vec![], 2), (vec![Z], 3), (vec![], 2), (vec![], 0)]);
    // Left, then Alternate Repeat: Right (QMK's own alternate).
    t.tap(LEFT, 100);
    t.tap(F2, 110);
    assert_eq!(t.take(), [(vec![LEFT], 0), (vec![], 0), (vec![RIGHT], 0), (vec![], 0)]);
    // A Vial entry: after Ctrl+Z, Alternate Repeat is Ctrl+Y.
    t.alt_repeat(0, 0x011D, 0x011C, 0, arep::ENABLED);
    t.down(LCTL, 200);
    t.tap(Z, 210);
    t.up(LCTL, 220);
    t.take();
    t.tap(F2, 230);
    assert_eq!(t.take(), [(vec![Y], 1), (vec![], 0)]);
    // Modifier keys are not remembered: Repeat still repeats Ctrl+Z.
    t.tap(LSFT, 300);
    t.tap(F1, 310);
    assert_eq!(t.take(), [(vec![], 2), (vec![], 0), (vec![Z], 1), (vec![], 0)]);
}

#[test]
fn auto_shift_shifts_a_key_held_past_its_timeout() {
    let mut t = Rig::new();
    t.km.settings.auto_shift = auto_shift::ENABLE;
    t.tap(A, 0);
    assert_eq!(t.take(), [(vec![A], 0), (vec![], 0)]);
    t.down(B, 100);
    t.until(274);
    assert!(t.take().is_empty());
    t.until(275);
    t.up(B, 400);
    assert_eq!(t.take(), [(vec![B], 2), (vec![], 0)]);
    // Rolled: A goes down, then B before A's timeout: A is plain at once, B waits.
    t.down(A, 500);
    t.down(B, 520);
    assert_eq!(t.take(), [(vec![A], 0)]);
    t.up(A, 530);
    t.up(B, 540);
    assert_eq!(t.take(), [(vec![], 0), (vec![B], 0), (vec![], 0)]);
    // With repeat: held, the shifted key stays down.
    t.km.settings.auto_shift |= auto_shift::REPEAT;
    t.down(C, 1000);
    t.until(1200);
    assert_eq!(t.take(), [(vec![C], 2)]);
    t.up(C, 1300);
    assert_eq!(t.take(), [(vec![], 0)]);
    // Space and modifiers are not auto-shifted.
    t.down(SPACE, 2000);
    assert_eq!(t.take(), [(vec![SPACE], 0)]);
}

#[test]
fn magic_settings_and_keycodes() {
    let mut t = Rig::new();
    t.km.settings.magic = magic::SWAP_CONTROL_CAPS;
    t.tap(CAPS, 0);
    assert_eq!(t.take(), [(vec![], 1), (vec![], 0)]);
    // A magic keycode turns the GUI keys off, and is saved.
    t.km.set(0, F1 as usize, 0x700A); // QK_MAGIC_GUI_OFF
    let before = t.km.changes();
    t.tap(F1, 10);
    assert!(t.km.settings.magic & magic::NO_GUI != 0 && t.km.changes() != before);
    t.tap(LGUI, 20);
    assert!(t.take().is_empty());
}

#[test]
fn grave_escape_and_its_overrides() {
    let mut t = Rig::new();
    t.km.set(0, ESC as usize, 0x7C16); // QK_GRAVE_ESCAPE
    t.down(LSFT, 0);
    t.tap(ESC, 10);
    assert_eq!(t.take(), [(vec![], 2), (vec![GRV], 2), (vec![], 2)]);
    // "Always send Escape if Shift is pressed".
    t.km.settings.grave_esc_override = 8;
    t.tap(ESC, 20);
    assert_eq!(t.take(), [(vec![ESC], 2), (vec![], 2)]);
}

#[test]
fn auto_shift_report_types_the_timeout() {
    let mut t = Rig::new();
    t.km.set(0, F1 as usize, 0x7C12); // QK_AUTO_SHIFT_REPORT
    t.km.settings.auto_shift_timeout_ms = 150;
    t.tap(F1, 0);
    t.until(100);
    let typed: StdVec<u8> = t.take().iter().filter_map(|(k, _)| k.first().copied()).collect();
    assert_eq!(typed, [ONE, 0x22, 0x27], "1 5 0");
}

/// The last output state of a run of steps.
fn last_out(steps: &[Step]) -> Option<crate::remap::Out> {
    steps.iter().rev().find_map(|s| match s {
        Step::State(o) => Some(*o),
        _ => None,
    })
}

#[test]
fn mouse_and_gamepad_buttons_go_through_the_keymap() {
    use crate::keycode::{KC_BTN1, KVM_DPAD, QK_JOYSTICK};
    use crate::pos;
    let mut t = Rig::new();
    // By default a mouse button is itself, a gamepad button and the hat the virtual gamepad's.
    t.control(pos::MOUSE + 1, true, 0);
    assert_eq!(last_out(&t.log).unwrap().buttons, 0b10);
    t.control(pos::MOUSE + 1, false, 1);
    t.control(pos::PAD + 3, true, 2);
    t.control(pos::HAT + 2, true, 3);
    let o = last_out(&t.log).unwrap();
    assert_eq!((o.pad, o.dpad, o.buttons), (1 << 3, 1 << 2, 0));
    t.control(pos::PAD + 3, false, 4);
    t.control(pos::HAT + 2, false, 5);
    t.log.clear();
    // Remapped: the fourth mouse button types Escape, a key presses gamepad button 5, a gamepad button clicks.
    t.km.set(0, (pos::MOUSE + 3) as usize, 0x29);
    t.km.set(0, A as usize, QK_JOYSTICK + 4);
    t.km.set(0, pos::PAD as usize, KC_BTN1);
    t.control(pos::MOUSE + 3, true, 10);
    assert_eq!(t.take(), [(vec![0x29], 0)]);
    t.control(pos::MOUSE + 3, false, 11);
    t.down(A, 12);
    let o = last_out(&t.log).unwrap();
    assert_eq!((o.pad, o.keys.keys().count()), (1 << 4, 0));
    t.up(A, 13);
    t.control(pos::PAD, true, 14);
    assert_eq!(last_out(&t.log).unwrap().buttons, 1);
    // The d-pad from a keycode.
    t.km.set(0, B as usize, KVM_DPAD + 1);
    t.down(B, 15);
    assert_eq!(last_out(&t.log).unwrap().dpad, 1 << 1);
}

#[test]
fn a_mouse_button_held_switches_a_layer() {
    use crate::pos;
    let mut t = Rig::new();
    // Mouse button 5: layer 1 while held; on layer 1 button 1 is Ctrl+C.
    t.km.set(0, (pos::MOUSE + 4) as usize, 0x5221);
    t.km.set(1, pos::MOUSE as usize, 0x0106);
    t.control(pos::MOUSE + 4, true, 0);
    t.control(pos::MOUSE, true, 1);
    let o = last_out(&t.log).unwrap();
    assert_eq!((o.keys.keys().collect::<StdVec<_>>(), o.keys.modifiers, o.buttons), (vec![0x06], 0x01, 0));
}

#[test]
fn each_keymap_has_its_own_layers() {
    let mut t = Rig::new();
    t.km.set_in(2, 0, A as usize, B as u16);
    t.tap(A, 0);
    assert_eq!(t.take(), [(vec![A], 0), (vec![], 0)]);
    t.km.set_active(2);
    t.tap(A, 10);
    assert_eq!(t.take(), [(vec![B], 0), (vec![], 0)]);
    assert!(!t.km.is_identity());
    t.km.set_active(1);
    assert!(t.km.is_identity());
}

/// Mouse buttons held in a run of steps, one entry per state.
fn buttons(steps: &[Step]) -> StdVec<u16> {
    steps
        .iter()
        .filter_map(|s| match s {
            Step::State(o) => Some(o.buttons),
            _ => None,
        })
        .collect()
}

#[test]
fn a_mouse_button_layer_taps_and_holds_like_a_key() {
    use crate::pos;
    let mut t = Rig::new();
    // Mouse button 4: LT(1, mouse button 1). Tapped it clicks; held it is layer 1.
    t.km.set(0, (pos::MOUSE + 3) as usize, 0x4100 | 0xD1);
    t.control(pos::MOUSE + 3, true, 0);
    t.control(pos::MOUSE + 3, false, 50);
    assert_eq!(buttons(&core::mem::take(&mut t.log)), [1, 0]);
    // Pressed again past the quick tap term (which would repeat the click), and held past the tapping term.
    t.control(pos::MOUSE + 3, true, 400);
    t.until(700);
    assert_eq!(t.r.layers(&t.km), 0b11, "held past the tapping term: layer 1");
    t.control(pos::MOUSE + 3, false, 800);
    assert_eq!(t.r.layers(&t.km), 0b01);
}

#[test]
fn gamepad_buttons_make_combos_dances_and_macros() {
    use crate::keycode::{KVM_DPAD, QK_JOYSTICK};
    use crate::pos;
    let mut t = Rig::new();
    // Buttons 1 and 2 together: Escape (a combo by keycode, as Vial's are).
    let mut b = [0u8; 10];
    b[0..2].copy_from_slice(&QK_JOYSTICK.to_le_bytes());
    b[2..4].copy_from_slice(&(QK_JOYSTICK + 1).to_le_bytes());
    b[8] = ESC;
    assert!(t.km.set_dynamic(Dynamic::Combo, 0, &b));
    t.control(pos::PAD, true, 0);
    t.control(pos::PAD + 1, true, 5);
    assert_eq!(last_out(&t.log).map(|o| (o.keys.keys().collect::<StdVec<_>>(), o.pad)), Some((vec![ESC], 0)));
    t.control(pos::PAD, false, 20);
    t.control(pos::PAD + 1, false, 21);
    t.until(100);
    t.log.clear();
    // Button 3: a tap dance, tapped 'a', held mouse button 2.
    t.dance(0, [A as u16, 0xD2, 0, 0], 0);
    t.km.set(0, (pos::PAD + 2) as usize, 0x5700);
    t.control(pos::PAD + 2, true, 200);
    t.until(600);
    assert_eq!(last_out(&t.log).unwrap().buttons, 0b10, "held: mouse button 2");
    t.control(pos::PAD + 2, false, 700);
    t.until(1000);
    t.log.clear();
    // Button 4: a macro that clicks, presses gamepad button 3 and the d-pad's up, and scrolls once.
    let m: heapless::Vec<u8, 32> = crate::macros::encode(&[
        crate::macros::Step::Tap(0xD1),
        crate::macros::Step::Down(QK_JOYSTICK + 2),
        crate::macros::Step::Tap(KVM_DPAD),
        crate::macros::Step::Up(QK_JOYSTICK + 2),
        crate::macros::Step::Tap(0xD9),
    ])
    .unwrap();
    t.km.write_macros(0, &m);
    t.km.set(0, (pos::PAD + 3) as usize, 0x7700);
    t.control(pos::PAD + 3, true, 2000);
    for k in 0..20 {
        t.tick(2000 + k * MACRO_STEP_MS);
    }
    let outs: StdVec<(u16, u32, u8)> = t
        .log
        .iter()
        .filter_map(|s| match s {
            Step::State(o) => Some((o.buttons, o.pad, o.dpad)),
            _ => None,
        })
        .collect();
    assert_eq!(outs, [(1, 0, 0), (0, 0, 0), (0, 1 << 2, 0), (0, 1 << 2, 1), (0, 1 << 2, 0), (0, 0, 0)]);
    assert!(t.log.iter().any(|s| matches!(s, Step::Motion(m) if m.wheel == 1)), "the wheel turned once");
}

#[test]
fn a_kvm_action_key_is_a_step_when_it_goes_down() {
    use crate::keycode::KVM_ACTION;
    use crate::pos;
    let mut t = Rig::new();
    t.km.set(0, (pos::MOUSE + 4) as usize, KVM_ACTION + 3);
    t.control(pos::MOUSE + 4, true, 0);
    t.control(pos::MOUSE + 4, false, 10);
    let kvm: StdVec<u8> = t.log.iter().filter_map(|s| if let Step::Kvm(n) = s { Some(*n) } else { None }).collect();
    assert_eq!(kvm, [3], "once, on the press");
    assert!(buttons(&t.log).iter().all(|b| *b == 0), "the button itself goes nowhere");
}

#[test]
fn macro_text_is_typed_through_the_hosts_layout() {
    let mut t = Rig::new();
    t.r.set_layout(kvm_hid::layouts::Layout::De);
    t.km.write_macros(0, b"z@^");
    t.km.set(0, F1 as usize, 0x7700);
    t.down(F1, 0);
    for k in 0..20 {
        t.tick(k * MACRO_STEP_MS);
    }
    // German: z on the Y key's place, @ with AltGr on Q, ^ a dead key that Space finishes.
    assert_eq!(
        t.take(),
        [
            (vec![0x1C], 0),
            (vec![], 0),
            (vec![0x14], 0x40),
            (vec![], 0),
            (vec![0x35], 0),
            (vec![], 0),
            (vec![SPACE], 0),
            (vec![], 0),
        ]
    );
}
