//! Axis rules: what each axis of the input devices does, per keymap and layer (hid_mapping.md §5.1).
//!
//! The inputs are the axes of every normalized device routed to the active host, merged (`kvm-core`): the mouse's
//! motion, wheel and pan as deltas, a gamepad's axes as values (calibrated, −32767…32767; the two triggers of the
//! simulation page 0…32767). Each has a [`Rule`] per layer; the highest active layer whose rule isn't
//! [`Target::Transparent`] decides, and on layer 0 a transparent rule is the input's default ([`Rule::default_for`]):
//! the mouse to the mouse, a gamepad axis to the same axis of the virtual gamepad.
//!
//! What a rule does depends on the kinds on both sides:
//! - **relative → relative** (mouse → mouse, motion → wheel): scaled, the remainder kept for the next report;
//! - **absolute → absolute** (stick → gamepad axis): deadzone, curve, scale;
//! - **absolute → relative** (stick → mouse): the deflection is a speed, moved every [`TICK_MS`];
//! - **relative → absolute** (mouse → stick): the motion deflects the axis, which springs back to centre at the rule's
//!   rate (0: it stays where it was pushed);
//! - **→ buttons:** the axis's two directions become positions of the keymap ([`crate::pos::AXIS`]): an absolute axis
//!   past the threshold holds one; a relative one taps it for each report that moves it.
//!
//! Several inputs on one gamepad axis: the last one that moved wins (hid_mapping.md §6.2).

use crate::{KEYMAPS, LAYERS, PosSet, pos};

pub const INPUTS: usize = 14;
/// The mouse's inputs, relative.
pub const RELATIVE: usize = 4;
pub const PAD_AXES: usize = 8;
/// Of the absolute inputs, these are triggers: from 0 up, no centre.
pub const TRIGGERS: [usize; 2] = [input::BRAKE, input::ACCELERATOR];

pub mod input {
    pub const MOUSE_X: usize = 0;
    pub const MOUSE_Y: usize = 1;
    pub const WHEEL: usize = 2;
    pub const PAN: usize = 3;
    /// A gamepad's axes by HID usage (generic desktop X–Dial), then the simulation page's brake and accelerator.
    pub const X: usize = 4;
    pub const Y: usize = 5;
    pub const Z: usize = 6;
    pub const RX: usize = 7;
    pub const RY: usize = 8;
    pub const RZ: usize = 9;
    pub const SLIDER: usize = 10;
    pub const DIAL: usize = 11;
    pub const BRAKE: usize = 12;
    pub const ACCELERATOR: usize = 13;
}

/// Where an input goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Target {
    /// The layer below decides; on layer 0 the input's default.
    Transparent = 0,
    /// Nowhere: blocked.
    None = 1,
    MouseX = 2,
    MouseY = 3,
    Wheel = 4,
    Pan = 5,
    /// The virtual gamepad's axes X, Y, Z, Rx, Ry, Rz, slider, dial (X/Y the left stick, Rx/Ry the right one, Z and
    /// Rz the triggers, as XInput takes them).
    Pad(u8),
    /// The input's two directions as keymap positions.
    Buttons = 14,
}

impl Target {
    pub fn code(self) -> u8 {
        match self {
            Target::Pad(n) => 6 + n.min(7),
            Target::Transparent => 0,
            Target::None => 1,
            Target::MouseX => 2,
            Target::MouseY => 3,
            Target::Wheel => 4,
            Target::Pan => 5,
            Target::Buttons => 14,
        }
    }

    pub fn from_code(c: u8) -> Option<Target> {
        Some(match c {
            0 => Target::Transparent,
            1 => Target::None,
            2 => Target::MouseX,
            3 => Target::MouseY,
            4 => Target::Wheel,
            5 => Target::Pan,
            6..=13 => Target::Pad(c - 6),
            14 => Target::Buttons,
            _ => return None,
        })
    }

    fn relative(self) -> bool {
        matches!(self, Target::MouseX | Target::MouseY | Target::Wheel | Target::Pan)
    }
}

pub mod flags {
    pub const INVERT: u8 = 1 << 0;
    /// Response curve, bits 1–2: 0 linear, 1 quadratic, 2 cubic.
    pub const CURVE_SHIFT: u8 = 1;
    pub const CURVE: u8 = 3 << CURVE_SHIFT;
    /// An absolute input read by how far it moved, not where it is (a touch pad onto the mouse): each change becomes
    /// motion, a full sweep of the range 2048 counts at a scale of 1.0; a jump of more than the parameter (in 256ths
    /// of the range; 0: a quarter) is a finger put down elsewhere, and moves nothing.
    pub const DELTA: u8 = 1 << 3;
}

/// One input's rule on one layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Rule {
    pub target: Target,
    pub flags: u8,
    /// 8.8 fixed point: 256 is 1.0.
    pub scale: i16,
    /// Of the full deflection, in 256ths: absolute inputs read 0 within it.
    pub deadzone: u8,
    /// Absolute → relative: the speed at full deflection, in counts per [`TICK_MS`]. Relative → absolute: how fast
    /// the axis springs back to centre, in 256ths per tick (0: not at all). → buttons: the threshold, in 256ths.
    pub param: u8,
}

pub const RULE_LEN: usize = 6;

impl Rule {
    pub const TRANSPARENT: Rule = Rule { target: Target::Transparent, flags: 0, scale: 256, deadzone: 0, param: 0 };

    /// What an input does when no layer says otherwise: the mouse's go to the mouse, a gamepad's to the same axis of
    /// the virtual gamepad (the brake and accelerator to Z and Rz, its triggers).
    pub const fn default_for(input: usize) -> Rule {
        let target = match input {
            input::MOUSE_X => Target::MouseX,
            input::MOUSE_Y => Target::MouseY,
            input::WHEEL => Target::Wheel,
            input::PAN => Target::Pan,
            input::BRAKE => Target::Pad(2),
            input::ACCELERATOR => Target::Pad(5),
            i if i < INPUTS => Target::Pad((i - input::X) as u8),
            _ => Target::None,
        };
        Rule { target, flags: 0, scale: 256, deadzone: 0, param: DEFAULT_PARAM }
    }

    pub fn encode(&self) -> [u8; RULE_LEN] {
        let s = self.scale.to_le_bytes();
        [self.target.code(), self.flags, s[0], s[1], self.deadzone, self.param]
    }

    pub fn decode(b: &[u8]) -> Option<Rule> {
        if b.len() < RULE_LEN {
            return None;
        }
        Some(Rule {
            target: Target::from_code(b[0])?,
            flags: b[1],
            scale: i16::from_le_bytes([b[2], b[3]]),
            deadzone: b[4],
            param: b[5],
        })
    }
}

/// The parameter a rule starts with: 16 counts per tick at full deflection (2 000 a second), a spring back of 32/256
/// per tick, the half way threshold.
pub const DEFAULT_PARAM: u8 = 16;
/// How often a deflected stick moves the pointer, and a pushed one springs back.
pub const TICK_MS: u32 = 8;
/// How long a relative axis's direction stays down as a button: one tap per report.
const TAP_MS: u32 = 20;
const FULL: i32 = 32767;

/// The rules of the four keymaps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rules {
    rules: [[[Rule; INPUTS]; LAYERS]; KEYMAPS],
}

impl Default for Rules {
    fn default() -> Self {
        Self::new()
    }
}

impl Rules {
    pub const fn new() -> Rules {
        Rules { rules: [[[Rule::TRANSPARENT; INPUTS]; LAYERS]; KEYMAPS] }
    }

    pub fn get(&self, keymap: usize, layer: usize, input: usize) -> Rule {
        self.rules
            .get(keymap)
            .and_then(|k| k.get(layer))
            .and_then(|l| l.get(input))
            .copied()
            .unwrap_or(Rule::TRANSPARENT)
    }

    /// Sets a rule; false if nothing changed (or out of range).
    pub fn set(&mut self, keymap: usize, layer: usize, input: usize, r: Rule) -> bool {
        match self.rules.get_mut(keymap).and_then(|k| k.get_mut(layer)).and_then(|l| l.get_mut(input)) {
            Some(slot) if *slot != r => {
                *slot = r;
                true
            }
            _ => false,
        }
    }

    /// The rule in force for `input` in `keymap` with the layers `layers` (bit per layer) on.
    pub fn effective(&self, keymap: usize, layers: u32, input: usize) -> Rule {
        (0..LAYERS)
            .rev()
            .filter(|l| layers & (1 << l) != 0 || *l == 0)
            .map(|l| self.get(keymap, l, input))
            .find(|r| r.target != Target::Transparent)
            .unwrap_or(Rule::default_for(input))
    }

    /// Whether every rule of `keymap` is transparent.
    pub fn is_default(&self, keymap: usize) -> bool {
        self.rules.get(keymap).is_none_or(|k| k.iter().flatten().all(|r| r.target == Target::Transparent))
    }

    /// The item of `keymap`: a version, then every layer's rules in input order.
    pub fn encode(&self, keymap: usize, out: &mut [u8]) -> usize {
        out[0] = VERSION;
        let mut n = 1;
        for l in &self.rules[keymap] {
            for r in l {
                out[n..n + RULE_LEN].copy_from_slice(&r.encode());
                n += RULE_LEN;
            }
        }
        n
    }

    pub const ITEM_LEN: usize = 1 + LAYERS * INPUTS * RULE_LEN;

    /// Takes back an item of `keymap`; one of another version or size is ignored.
    pub fn restore(&mut self, keymap: usize, d: &[u8]) {
        if keymap >= KEYMAPS || d.len() != Self::ITEM_LEN || d[0] != VERSION {
            return;
        }
        let mut rules = [[Rule::TRANSPARENT; INPUTS]; LAYERS];
        for (i, chunk) in d[1..].chunks(RULE_LEN).enumerate() {
            match Rule::decode(chunk) {
                Some(r) => rules[i / INPUTS][i % INPUTS] = r,
                None => return,
            }
        }
        self.rules[keymap] = rules;
    }
}

const VERSION: u8 = 1;

/// What the axis rules make of one input or tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Out {
    /// Mouse motion, wheel and pan to send now.
    pub mouse: [i32; 4],
    /// The virtual gamepad's axes, when one changed.
    pub pad: Option<[i16; PAD_AXES]>,
    /// The axis positions held down ([`pos::AXIS`]).
    pub buttons: PosSet,
}

/// The axis engine's state.
#[derive(Debug, Clone)]
pub struct Axes {
    /// The absolute inputs' last values, and when each last changed.
    abs: [i32; INPUTS],
    /// Fractions of a count kept per input (relative → relative, absolute → relative), in 1/256.
    rest: [i32; INPUTS],
    /// The gamepad's axes: their values, and which input last moved each.
    pad: [i32; PAD_AXES],
    pad_owner: [u8; PAD_AXES],
    sent_pad: [i16; PAD_AXES],
    /// Mouse → stick deflections, per gamepad axis.
    pushed: [i32; PAD_AXES],
    /// Axis directions held as buttons: (until, for a tap of a relative input).
    held: PosSet,
    tap_until: [Option<u32>; 2 * RELATIVE],
    next_tick: Option<u32>,
}

impl Default for Axes {
    fn default() -> Self {
        Self::new()
    }
}

fn apply_deadzone(v: i32, dz: u8) -> i32 {
    let dz = dz as i32 * FULL / 256;
    if v.abs() <= dz || dz >= FULL {
        return 0;
    }
    v.signum() * ((v.abs() - dz) as i64 * FULL as i64 / (FULL - dz) as i64) as i32
}

fn apply_curve(v: i32, flags: u8) -> i32 {
    let v = v as i64;
    let f = FULL as i64;
    (match (flags & flags::CURVE) >> flags::CURVE_SHIFT {
        1 => v * v.abs() / f,
        2 => v * v * v / (f * f),
        _ => v,
    }) as i32
}

/// The rule's shaping of an absolute value: deadzone, curve, invert, scale; clamped.
fn shape(v: i32, r: &Rule) -> i32 {
    let mut v = apply_curve(apply_deadzone(v, r.deadzone), r.flags);
    if r.flags & flags::INVERT != 0 {
        v = -v;
    }
    (v as i64 * r.scale as i64 / 256).clamp(-FULL as i64, FULL as i64) as i32
}

impl Axes {
    pub const fn new() -> Axes {
        Axes {
            abs: [0; INPUTS],
            rest: [0; INPUTS],
            pad: [0; PAD_AXES],
            pad_owner: [u8::MAX; PAD_AXES],
            sent_pad: [0; PAD_AXES],
            pushed: [0; PAD_AXES],
            held: PosSet::EMPTY,
            tap_until: [None; 2 * RELATIVE],
            next_tick: None,
        }
    }

    /// Everything back to rest (a switch of host or keymap): no pushed sticks, no buttons, the gamepad centred.
    pub fn reset(&mut self) -> Out {
        let abs = self.abs;
        *self = Axes::new();
        self.abs = abs;
        Out { pad: Some([0; PAD_AXES]), ..Out::default() }
    }

    /// New input: `rel` the mouse's deltas of this report, `abs` the gamepad's values now (`None`: unchanged).
    pub fn input(
        &mut self,
        rules: &Rules,
        keymap: usize,
        layers: u32,
        rel: [i32; RELATIVE],
        abs: Option<&[i32; INPUTS - RELATIVE]>,
        now: u32,
    ) -> Out {
        let mut out = Out::default();
        for (i, d) in rel.iter().enumerate().filter(|(_, d)| **d != 0) {
            let r = rules.effective(keymap, layers, i);
            self.relative(i, *d, &r, &mut out, now);
        }
        if let Some(abs) = abs {
            for (j, v) in abs.iter().enumerate() {
                let i = RELATIVE + j;
                if self.abs[i] != *v {
                    let old = core::mem::replace(&mut self.abs[i], *v);
                    let r = rules.effective(keymap, layers, i);
                    if r.flags & flags::DELTA != 0 {
                        self.delta(i, *v - old, &r, &mut out);
                    } else {
                        self.absolute(i, &r);
                    }
                }
            }
        }
        self.finish(rules, keymap, layers, now, &mut out);
        out
    }

    /// Timers: sticks moving the pointer, pushed sticks springing back, taps of relative inputs ending. Call at
    /// [`Axes::deadline`].
    pub fn tick(&mut self, rules: &Rules, keymap: usize, layers: u32, now: u32) -> Out {
        let mut out = Out::default();
        if self.next_tick.is_some_and(|t| now.wrapping_sub(t) as i32 >= 0) {
            self.next_tick = None;
            for i in RELATIVE..INPUTS {
                let r = rules.effective(keymap, layers, i);
                if r.target.relative() && self.abs[i] != 0 && r.flags & flags::DELTA == 0 {
                    // Speed at full deflection: param counts per tick, scaled.
                    let v = shape(self.abs[i], &Rule { scale: 256, ..r }) as i64;
                    let counts = v * r.param as i64 * r.scale as i64 / FULL as i64 + self.rest[i] as i64;
                    let whole = counts / 256;
                    self.rest[i] = (counts - whole * 256) as i32;
                    out.mouse[r.target.code() as usize - 2] += whole as i32;
                }
            }
            for a in 0..PAD_AXES {
                if self.pushed[a] != 0 {
                    let owner = self.pad_owner[a] as usize;
                    let spring = if owner < RELATIVE { rules.effective(keymap, layers, owner).param } else { 0 };
                    self.pushed[a] -= self.pushed[a] * spring as i32 / 256;
                    if self.pushed[a].abs() < 64 {
                        self.pushed[a] = 0;
                    }
                    if owner < RELATIVE {
                        self.pad[a] = self.pushed[a];
                    }
                }
            }
        }
        self.finish(rules, keymap, layers, now, &mut out);
        out
    }

    /// When [`Axes::tick`] must run next.
    pub fn deadline(&self) -> Option<u32> {
        let taps = self.tap_until.iter().flatten().copied();
        self.next_tick.into_iter().chain(taps).reduce(|a, b| if (b.wrapping_sub(a) as i32) < 0 { b } else { a })
    }

    fn relative(&mut self, i: usize, d: i32, r: &Rule, out: &mut Out, now: u32) {
        let sign = if r.flags & flags::INVERT != 0 { -d } else { d };
        match r.target {
            Target::Transparent | Target::None => {}
            t if t.relative() => {
                let v = sign as i64 * r.scale as i64 + self.rest[i] as i64;
                let whole = v / 256;
                self.rest[i] = (v - whole * 256) as i32;
                out.mouse[t.code() as usize - 2] += whole as i32;
            }
            Target::Pad(a) => {
                // Mouse → stick: 256 per count at 1.0, a full push in 128 counts.
                let a = a as usize;
                self.pushed[a] = (self.pushed[a] + sign * r.scale as i32).clamp(-FULL, FULL);
                self.pad[a] = self.pushed[a];
                self.pad_owner[a] = i as u8;
            }
            Target::Buttons => {
                let k = 2 * i + usize::from(sign > 0);
                self.tap_until[k] = Some(now.wrapping_add(TAP_MS));
            }
            _ => {}
        }
    }

    /// An absolute input read by its change ([`flags::DELTA`]): motion for a relative target.
    fn delta(&mut self, i: usize, d: i32, r: &Rule, out: &mut Out) {
        let jump = if r.param == 0 { FULL / 2 } else { r.param as i32 * 2 * FULL / 256 };
        if !r.target.relative() || d.abs() > jump {
            return;
        }
        let d = if r.flags & flags::INVERT != 0 { -d } else { d };
        // 2·FULL (a sweep) → 2048 counts at 1.0: a count per 32 units, in 256ths of the scale.
        let v = d as i64 * r.scale as i64 / 32 + self.rest[i] as i64;
        let whole = v / 256;
        self.rest[i] = (v - whole * 256) as i32;
        out.mouse[r.target.code() as usize - 2] += whole as i32;
    }

    fn absolute(&mut self, i: usize, r: &Rule) {
        if let Target::Pad(a) = r.target {
            let a = a as usize;
            let mut v = shape(self.abs[i], r);
            // A trigger (0…32767) onto a centred axis keeps its range; the gamepad reads it as it is.
            if TRIGGERS.contains(&i) {
                v = v.max(0);
            }
            self.pad[a] = v;
            self.pad_owner[a] = i as u8;
        }
    }

    /// Buttons from the absolute inputs and the taps, the gamepad's axes when they changed, the next tick.
    fn finish(&mut self, rules: &Rules, keymap: usize, layers: u32, now: u32, out: &mut Out) {
        let mut held = PosSet::EMPTY;
        let mut moving = false;
        for i in 0..INPUTS {
            let r = rules.effective(keymap, layers, i);
            let (minus, plus) = (pos::AXIS + 2 * i as u16, pos::AXIS + 2 * i as u16 + 1);
            if i < RELATIVE {
                for (k, p) in [(2 * i, minus), (2 * i + 1, plus)] {
                    match self.tap_until[k] {
                        Some(t) if (now.wrapping_sub(t) as i32) < 0 => held.set(p, true),
                        Some(_) => self.tap_until[k] = None,
                        None => {}
                    }
                }
                continue;
            }
            match r.target {
                Target::Buttons => {
                    let v = if r.flags & flags::INVERT != 0 { -self.abs[i] } else { self.abs[i] };
                    let threshold = r.param.max(1) as i32 * FULL / 256;
                    // 1/16 of the range of hysteresis: a key doesn't chatter at the threshold.
                    let keep = threshold - FULL / 16;
                    let was = |p: u16| self.held.has(p);
                    held.set(plus, v > threshold || (was(plus) && v > keep));
                    held.set(minus, v < -threshold || (was(minus) && v < -keep));
                }
                t if t.relative() && r.flags & flags::DELTA == 0 && shape(self.abs[i], &r) != 0 => moving = true,
                _ => {}
            }
        }
        self.held = held;
        out.buttons = held;
        if moving || self.pushed.iter().any(|p| *p != 0) {
            if self.next_tick.is_none() {
                self.next_tick = Some(now.wrapping_add(TICK_MS));
            }
        } else {
            self.next_tick = None;
        }
        let pad = self.pad.map(|v| v.clamp(-FULL, FULL) as i16);
        if pad != self.sent_pad {
            self.sent_pad = pad;
            out.pad = Some(pad);
        }
    }

    /// The virtual gamepad's axes now.
    pub fn pad(&self) -> [i16; PAD_AXES] {
        self.sent_pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn abs(values: &[(usize, i32)]) -> [i32; INPUTS - RELATIVE] {
        let mut a = [0; INPUTS - RELATIVE];
        for (i, v) in values {
            a[i - RELATIVE] = *v;
        }
        a
    }

    #[test]
    fn by_default_the_mouse_goes_to_the_mouse_and_the_sticks_to_the_gamepad() {
        let rules = Rules::new();
        let mut ax = Axes::new();
        let out = ax.input(&rules, 0, 1, [5, -3, 1, 0], None, 0);
        assert_eq!(out.mouse, [5, -3, 1, 0]);
        assert_eq!(out.pad, None);
        let out = ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::X, 1000), (input::RZ, -20000)])), 1);
        assert_eq!(out.pad, Some([1000, 0, 0, 0, 0, -20000, 0, 0]));
        assert_eq!(out.mouse, [0; 4]);
    }

    #[test]
    fn a_layer_turns_motion_into_scrolling() {
        let mut rules = Rules::new();
        // Layer 1: X → pan, Y → wheel at an eighth, inverted.
        let scroll = |t| Rule { target: t, flags: flags::INVERT, scale: 32, deadzone: 0, param: 0 };
        rules.set(0, 1, input::MOUSE_X, scroll(Target::Pan));
        rules.set(0, 1, input::MOUSE_Y, scroll(Target::Wheel));
        let mut ax = Axes::new();
        assert_eq!(ax.input(&rules, 0, 0b01, [8, 8, 0, 0], None, 0).mouse, [8, 8, 0, 0]);
        // Eighths add up across reports.
        assert_eq!(ax.input(&rules, 0, 0b11, [4, 4, 0, 0], None, 1).mouse, [0, 0, 0, 0]);
        assert_eq!(ax.input(&rules, 0, 0b11, [4, 12, 0, 0], None, 2).mouse, [0, 0, -2, -1]);
    }

    #[test]
    fn a_stick_moves_the_pointer_while_deflected() {
        let mut rules = Rules::new();
        rules.set(0, 0, input::X, Rule { target: Target::MouseX, flags: 0, scale: 256, deadzone: 32, param: 16 });
        let mut ax = Axes::new();
        // Within the deadzone: nothing, and no timer.
        ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::X, 3000)])), 0);
        assert_eq!(ax.deadline(), None);
        // Full deflection: 16 counts per tick.
        let out = ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::X, FULL)])), 0);
        assert_eq!((out.mouse, out.pad), ([0; 4], None), "the gamepad axis is not moved");
        assert_eq!(ax.deadline(), Some(TICK_MS));
        assert_eq!(ax.tick(&rules, 0, 1, TICK_MS).mouse, [16, 0, 0, 0]);
        assert_eq!(ax.deadline(), Some(2 * TICK_MS));
        ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::X, 0)])), 2 * TICK_MS);
        assert_eq!(ax.deadline(), None);
    }

    #[test]
    fn the_mouse_pushes_a_stick_that_springs_back() {
        let mut rules = Rules::new();
        rules.set(0, 0, input::MOUSE_X, Rule { target: Target::Pad(3), flags: 0, scale: 256, deadzone: 0, param: 128 });
        let mut ax = Axes::new();
        let out = ax.input(&rules, 0, 1, [40, 0, 0, 0], None, 0);
        assert_eq!(out.mouse, [0; 4]);
        assert_eq!(out.pad.unwrap()[3], 40 * 256);
        // Half back per tick, until it rests.
        assert_eq!(ax.tick(&rules, 0, 1, TICK_MS).pad.unwrap()[3], 20 * 256);
        let mut t = TICK_MS;
        while ax.deadline().is_some() {
            t += TICK_MS;
            ax.tick(&rules, 0, 1, t);
        }
        assert_eq!(ax.pad()[3], 0);
    }

    #[test]
    fn a_stick_past_its_threshold_holds_a_position() {
        let mut rules = Rules::new();
        rules.set(0, 0, input::Y, Rule { target: Target::Buttons, flags: 0, scale: 256, deadzone: 0, param: 128 });
        let mut ax = Axes::new();
        let minus = pos::AXIS + 2 * input::Y as u16;
        let out = ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::Y, -20000)])), 0);
        assert!(out.buttons.has(minus) && !out.buttons.has(minus + 1));
        // Hysteresis: a little back towards the threshold keeps it.
        assert!(ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::Y, -15000)])), 1).buttons.has(minus));
        assert!(ax.input(&rules, 0, 1, [0; 4], Some(&abs(&[(input::Y, -12000)])), 2).buttons.is_empty());
    }

    #[test]
    fn the_wheel_taps_its_directions() {
        let mut rules = Rules::new();
        rules.set(0, 0, input::WHEEL, Rule { target: Target::Buttons, flags: 0, scale: 256, deadzone: 0, param: 0 });
        let mut ax = Axes::new();
        let up = pos::AXIS + 2 * input::WHEEL as u16 + 1;
        let out = ax.input(&rules, 0, 1, [0, 0, 1, 0], None, 100);
        assert!(out.buttons.has(up) && out.mouse == [0; 4]);
        assert_eq!(ax.deadline(), Some(100 + TAP_MS));
        assert!(ax.tick(&rules, 0, 1, 100 + TAP_MS).buttons.is_empty());
        assert_eq!(ax.deadline(), None);
    }

    #[test]
    fn curves_and_deadzones() {
        let r = Rule { target: Target::Pad(0), flags: 1 << flags::CURVE_SHIFT, scale: 256, deadzone: 0, param: 0 };
        assert_eq!(shape(FULL / 2, &r), FULL / 4);
        assert_eq!(shape(-FULL, &r), -FULL);
        let dz = Rule { deadzone: 128, flags: 0, ..r };
        assert_eq!(shape(FULL / 2, &dz), 0);
        assert_eq!(shape(FULL, &dz), FULL);
    }

    #[test]
    fn items_round_trip() {
        let mut rules = Rules::new();
        assert!(rules.is_default(2));
        let r = Rule { target: Target::Pad(4), flags: 3, scale: -512, deadzone: 9, param: 77 };
        assert!(rules.set(2, 5, input::DIAL, r));
        let mut buf = [0u8; Rules::ITEM_LEN];
        assert_eq!(rules.encode(2, &mut buf), Rules::ITEM_LEN);
        let mut copy = Rules::new();
        copy.restore(2, &buf);
        assert_eq!(copy.get(2, 5, input::DIAL), r);
        assert!(!copy.is_default(2) && copy.is_default(0));
    }

    #[test]
    fn a_touch_pad_moves_the_pointer_by_how_far_the_finger_slides() {
        let mut rules = Rules::new();
        let r = Rule { target: Target::MouseX, flags: flags::DELTA, scale: 256, deadzone: 0, param: 0 };
        rules.set(0, 0, input::SLIDER, r);
        let mut ax = Axes::new();
        ax.input(&rules, 0, 1, [0; RELATIVE], Some(&abs(&[(input::SLIDER, -20000)])), 0);
        // A finger put down elsewhere moves nothing; then sliding 3200 units is 100 counts.
        let out = ax.input(&rules, 0, 1, [0; RELATIVE], Some(&abs(&[(input::SLIDER, 10000)])), 8);
        assert_eq!(out.mouse, [0; 4]);
        let out = ax.input(&rules, 0, 1, [0; RELATIVE], Some(&abs(&[(input::SLIDER, 13200)])), 16);
        assert_eq!(out.mouse, [100, 0, 0, 0]);
        assert_eq!(ax.deadline(), None, "no speed: nothing moves between reports");
    }
}
