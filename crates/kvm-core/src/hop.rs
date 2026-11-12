//! Screen hop (design.md §4 "Screen hop", software.md §4.14): the KVM moves the pointer itself, across an arrangement
//! of both PCs' screens, and switches to the other PC when the pointer is pushed across an edge onto one of its
//! screens.
//!
//! - **Arrangement:** up to [`MAX_SCREENS`] screens, each a PC's, placed freely in one plane of *logical* pixels (a
//!   screen at 150 % scaling takes two thirds of its pixels each way). The pointer lives in that plane.
//! - **Edges:** an edge onto the same PC's next screen in that direction lets the pointer through (across a gap too).
//!   An edge onto the other PC's screen holds it until it was pushed [`Rules::resistance`] pixels further (a pause
//!   starts the push over), never near the screen's corners ([`Rules::corner`]), never while a button is held, and
//!   only to a PC that is connected. Any other edge is a wall.
//! - **Reports:** what the active PC gets is an absolute position, mapped onto its screens as it maps the absolute
//!   pointer ([`Mapping`]): across all of them (Linux), or onto its main screen only (Windows), whose other screens the
//!   KVM reaches with relative motion from where it last put the cursor.
//! - **Acceleration:** the PCs don't accelerate an absolute pointer, so the KVM does ([`Accelerator`]): flat, the curve
//!   of Windows' "Enhance pointer precision", or libinput's adaptive profile, with the speed measured over the last
//!   reports as libinput measures it.

use heapless::Vec;
use kvm_proto::ctrl::{Accel, MAX_SCREENS, Mapping, Pc, Screen};

pub use kvm_hid::normalized::ABS_MAX;

/// The arrangement: the config's screen slots.
pub type Layout = [Screen; MAX_SCREENS];

/// A push against an edge to the other PC that pauses this long starts over.
pub const PUSH_PAUSE_MS: u32 = 400;
/// The PC's own cursor position ([`Pointer::sync`]) counts only once the KVM hasn't moved the pointer for this long.
pub const SYNC_QUIET_MS: u32 = 150;
/// Screens may overlap this much (logical pixels) and still be neighbours.
const OVERLAP: f32 = 8.0;

/// What the active PC needs sent for the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Report {
    /// The absolute position, 0–[`ABS_MAX`] each way.
    Abs([u16; 2]),
    /// Relative motion, in the PC's pixels: a screen the PC maps no absolute position onto.
    Rel([i32; 2]),
}

pub type Reports = Vec<Report, 2>;

/// What the edges allow at the moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules {
    pub resistance: u16,
    pub corner: u16,
    /// A mouse button is held: the edges to the other PC hold (a drag stays on its PC).
    pub buttons: bool,
    /// The PCs the pointer may hop to: connected, awake, in report protocol.
    pub open: [bool; 2],
    pub mapping: [Mapping; 2],
}

/// The outcome of a motion.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    /// For the active PC, in order (empty: the pointer didn't move).
    pub reports: Reports,
    /// The pointer was pushed across onto this PC's screen: switch to it, then send [`Pointer::here`].
    pub hop: Option<Pc>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Rect {
    min: [f32; 2],
    /// Exclusive: the last pixel is `max - 1`.
    max: [f32; 2],
}

fn rect(s: &Screen) -> Rect {
    let [w, h] = s.size();
    let min = [s.at[0] as f32, s.at[1] as f32];
    Rect { min, max: [min[0] + w.max(1) as f32, min[1] + h.max(1) as f32] }
}

/// `f32::round` and `f32::floor` need `std`; the values here are pixels, well within `i32`.
fn round(v: f32) -> f32 {
    if v >= 0.0 { (v + 0.5) as i32 as f32 } else { -((0.5 - v) as i32 as f32) }
}

fn floor(v: f32) -> f32 {
    let t = v as i32 as f32;
    if t > v { t - 1.0 } else { t }
}

fn in_span(v: f32, min: f32, max: f32) -> bool {
    v >= min && v <= max - 1.0
}

fn clamp_in(v: f32, min: f32, max: f32) -> f32 {
    v.clamp(min, (max - 1.0).max(min))
}

fn usable(s: &Screen, pc: Pc) -> bool {
    s.pc == Some(pc) && s.is_valid()
}

/// The PC's screens, by slot.
fn screens_of(layout: &Layout, pc: Pc) -> impl Iterator<Item = (usize, &Screen)> {
    layout.iter().enumerate().filter(move |(_, s)| usable(s, pc))
}

/// Whether `pc` has a screen in the arrangement.
pub fn has_screens(layout: &Layout, pc: Pc) -> bool {
    screens_of(layout, pc).next().is_some()
}

/// The PC's main screen: the one at 0,0 of its own arrangement (where Windows puts it), else its first.
pub fn main_screen(layout: &Layout, pc: Pc) -> Option<usize> {
    screens_of(layout, pc).find(|(_, s)| s.desk == [0, 0]).or_else(|| screens_of(layout, pc).next()).map(|(i, _)| i)
}

/// Where `p` (arrangement) is in the PC's own pixels, on screen `s`.
fn desk_of(s: &Screen, p: [f32; 2]) -> [f32; 2] {
    let k = s.scale.max(1) as f32 / 100.0;
    [s.desk[0] as f32 + (p[0] - s.at[0] as f32) * k, s.desk[1] as f32 + (p[1] - s.at[1] as f32) * k]
}

/// The PC's own pixels a screen covers: (top-left, bottom-right exclusive).
fn desk_rect(s: &Screen) -> ([f32; 2], [f32; 2]) {
    let min = [s.desk[0] as f32, s.desk[1] as f32];
    (min, [min[0] + s.w as f32, min[1] + s.h as f32])
}

/// What 0–[`ABS_MAX`] spans on `pc`, in its own pixels.
fn abs_area(layout: &Layout, pc: Pc, mapping: Mapping) -> Option<([f32; 2], [f32; 2])> {
    match mapping {
        Mapping::MainScreen => main_screen(layout, pc).map(|m| desk_rect(&layout[m])),
        Mapping::Desktop => screens_of(layout, pc)
            .map(|(_, s)| desk_rect(s))
            .reduce(|(a0, a1), (b0, b1)| ([a0[0].min(b0[0]), a0[1].min(b0[1])], [a1[0].max(b1[0]), a1[1].max(b1[1])])),
    }
}

/// The absolute position of `desk` (the PC's own pixels) within `area`.
fn to_abs(desk: [f32; 2], (min, max): ([f32; 2], [f32; 2])) -> [u16; 2] {
    core::array::from_fn(|a| {
        let span = (max[a] - min[a] - 1.0).max(1.0);
        let v = (desk[a] - min[a]) / span * ABS_MAX as f32 + 0.5;
        v.clamp(0.0, ABS_MAX as f32) as u16
    })
}

/// The next screen from `slot` in direction `dir` along `axis`, among those that span `across` on the other axis:
/// the nearest, however far (a gap between screens is jumped).
fn neighbour(layout: &Layout, slot: usize, axis: usize, dir: i8, across: f32) -> Option<usize> {
    let r = rect(&layout[slot]);
    let o = 1 - axis;
    let mut best: Option<(usize, f32)> = None;
    for (i, s) in layout.iter().enumerate() {
        if i == slot || s.pc.is_none() || !s.is_valid() {
            continue;
        }
        let n = rect(s);
        if !in_span(across, n.min[o], n.max[o]) {
            continue;
        }
        let gap = if dir > 0 { n.min[axis] - r.max[axis] } else { r.min[axis] - n.max[axis] };
        if gap >= -OVERLAP && best.is_none_or(|(_, g)| gap < g) {
            best = Some((i, gap));
        }
    }
    best.map(|(i, _)| i)
}

/// The pointer of screen hop.
#[derive(Debug, Clone)]
pub struct Pointer {
    /// In the arrangement, on screen `slot`.
    pos: [f32; 2],
    slot: Option<usize>,
    /// Where the pointer was left on each PC.
    left: [Option<[f32; 2]>; 2],
    /// Where each PC's own cursor is as far as the KVM knows (its own pixels): after an absolute report exactly there.
    cursor: [Option<[f32; 2]>; 2],
    push: f32,
    /// The edge pushed against: slot, axis, direction.
    push_edge: Option<(usize, usize, i8)>,
    push_at: u32,
    moved_at: Option<u32>,
    pub accel: Accelerator,
}

impl Default for Pointer {
    fn default() -> Self {
        Self::NEW
    }
}

impl Pointer {
    pub const NEW: Pointer = Pointer {
        pos: [0.0; 2],
        slot: None,
        left: [None; 2],
        cursor: [None; 2],
        push: 0.0,
        push_edge: None,
        push_at: 0,
        moved_at: None,
        accel: Accelerator::NEW,
    };

    /// Where the pointer is in the arrangement (logical pixels).
    pub fn position(&self) -> [i16; 2] {
        self.pos.map(|v| round(v).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
    }

    /// The screen slot the pointer is on.
    pub fn screen(&self) -> Option<usize> {
        self.slot
    }

    /// Where the pointer is on `pc` (logical pixels): where it is, or where it goes when it first moves there.
    pub fn position_on(&self, layout: &Layout, pc: Pc) -> [i16; 2] {
        let mut p = self.clone();
        p.on(layout, pc);
        p.position()
    }

    /// The PC was reset or re-enumerated: where its cursor is is no longer known.
    pub fn forget_cursor(&mut self, pc: Pc) {
        self.cursor[pc.index()] = None;
    }

    fn reset_push(&mut self) {
        self.push = 0.0;
        self.push_edge = None;
    }

    /// Leaves `pc` (a switch away from it): the pointer is found there again on return.
    pub fn leave(&mut self, layout: &Layout, pc: Pc) {
        if self.slot.is_some_and(|i| usable(&layout[i], pc)) {
            self.left[pc.index()] = Some(self.pos);
        }
        self.reset_push();
    }

    /// Puts the pointer on `pc` (a switch to it): where it was left there, or in the middle of its main screen.
    pub fn enter(&mut self, layout: &Layout, pc: Pc) {
        self.reset_push();
        if let Some(p) = self.left[pc.index()]
            && let Some((i, _)) = screens_of(layout, pc).find(|(_, s)| {
                let r = rect(s);
                in_span(p[0], r.min[0], r.max[0]) && in_span(p[1], r.min[1], r.max[1])
            })
        {
            self.pos = p;
            self.slot = Some(i);
            return;
        }
        self.slot = main_screen(layout, pc);
        if let Some(i) = self.slot {
            let r = rect(&layout[i]);
            self.pos = [floor((r.min[0] + r.max[0]) / 2.0), floor((r.min[1] + r.max[1]) / 2.0)];
        }
    }

    /// The screen the pointer is on, on `pc`: the one it is on, kept inside it, or where [`Pointer::enter`] puts it.
    fn on(&mut self, layout: &Layout, pc: Pc) -> Option<usize> {
        match self.slot {
            Some(i) if usable(&layout[i], pc) => {
                let r = rect(&layout[i]);
                self.pos = [clamp_in(self.pos[0], r.min[0], r.max[0]), clamp_in(self.pos[1], r.min[1], r.max[1])];
                Some(i)
            }
            _ => {
                self.enter(layout, pc);
                self.slot
            }
        }
    }

    /// The reports that put `pc`'s cursor where the pointer is.
    pub fn here(&mut self, layout: &Layout, pc: Pc, mapping: Mapping) -> Reports {
        let mut out = Reports::new();
        let Some(slot) = self.on(layout, pc) else { return out };
        let s = &layout[slot];
        let desk = desk_of(s, self.pos);
        let to_px = |d: [f32; 2]| d.map(round);
        let absolute = mapping == Mapping::Desktop || main_screen(layout, pc) == Some(slot);
        if absolute {
            if let Some(area) = abs_area(layout, pc, mapping) {
                let _ = out.push(Report::Abs(to_abs(desk, area)));
                self.cursor[pc.index()] = Some(to_px(desk));
            }
            return out;
        }
        // A screen the PC maps nothing onto: move its cursor there from where it is. Where that isn't known, first put
        // it on the main screen, at the point nearest to where it goes.
        let from = match self.cursor[pc.index()] {
            Some(c) => c,
            None => {
                let Some(m) = main_screen(layout, pc) else { return out };
                let (min, max) = desk_rect(&layout[m]);
                let edge = to_px([clamp_in(desk[0], min[0], max[0]), clamp_in(desk[1], min[1], max[1])]);
                let _ = out.push(Report::Abs(to_abs(edge, (min, max))));
                edge
            }
        };
        let target = to_px(desk);
        let _ = out.push(Report::Rel([(target[0] - from[0]) as i32, (target[1] - from[1]) as i32]));
        self.cursor[pc.index()] = Some(target);
        out
    }

    /// Moves the pointer by `d` (logical pixels, accelerated) on `pc`, the active PC.
    pub fn motion(&mut self, layout: &Layout, pc: Pc, d: [f32; 2], now: u32, rules: &Rules) -> Step {
        let mut step = Step::default();
        let Some(mut slot) = self.on(layout, pc) else { return step };
        if d == [0.0, 0.0] {
            return step;
        }
        self.moved_at = Some(now);
        for axis in 0..2 {
            let delta = d[axis];
            if delta == 0.0 {
                continue;
            }
            let o = 1 - axis;
            let r = rect(&layout[slot]);
            let dir: i8 = if delta > 0.0 { 1 } else { -1 };
            let target = self.pos[axis] + delta;
            if in_span(target, r.min[axis], r.max[axis]) {
                self.pos[axis] = target;
                if self.push_edge.is_some_and(|(_, a, pd)| a == axis && pd != dir) {
                    self.reset_push();
                }
                continue;
            }
            let edge = if dir > 0 { r.max[axis] - 1.0 } else { r.min[axis] };
            let over = (target - edge).abs();
            let across = self.pos[o];
            self.pos[axis] = edge;
            let Some(n) = neighbour(layout, slot, axis, dir, across) else {
                self.reset_push();
                continue;
            };
            let nr = rect(&layout[n]);
            let to = layout[n].pc.unwrap_or(pc);
            if to == pc {
                // The PC's own next screen: on, by what was left of the motion.
                let entry = if dir > 0 { nr.min[axis] + over - 1.0 } else { nr.max[axis] - over };
                self.pos[axis] = clamp_in(entry, nr.min[axis], nr.max[axis]);
                self.pos[o] = clamp_in(across, nr.min[o], nr.max[o]);
                slot = n;
                self.slot = Some(n);
                self.reset_push();
                continue;
            }
            let corner = rules.corner as f32;
            let near_corner = across - r.min[o] < corner || r.max[o] - 1.0 - across < corner;
            if rules.buttons || !rules.open[to.index()] || near_corner {
                self.reset_push();
                continue;
            }
            let key = (slot, axis, dir);
            if self.push_edge != Some(key) || now.wrapping_sub(self.push_at) > PUSH_PAUSE_MS {
                self.push = 0.0;
            }
            self.push_edge = Some(key);
            self.push_at = now;
            self.push += over;
            if self.push < rules.resistance as f32 {
                continue;
            }
            // Over: the PC left keeps its cursor at the edge, the other gets the pointer at the same place along it.
            step.reports = self.here(layout, pc, rules.mapping[pc.index()]);
            self.leave(layout, pc);
            let mut p = self.pos;
            p[axis] = if dir > 0 { nr.min[axis] } else { nr.max[axis] - 1.0 };
            p[o] = clamp_in(across, nr.min[o], nr.max[o]);
            self.pos = p;
            self.slot = Some(n);
            step.hop = Some(to);
            return step;
        }
        step.reports = self.here(layout, pc, rules.mapping[pc.index()]);
        step
    }

    /// `pc`'s own cursor is at `desk` (its own pixels; `PointerAt`): taken over once the KVM hasn't moved the pointer
    /// for [`SYNC_QUIET_MS`] (a touchpad, a program or the PC's acceleration moved it). Returns whether it was.
    pub fn sync(&mut self, layout: &Layout, pc: Pc, desk: [i16; 2], now: u32, active: bool) -> bool {
        if self.moved_at.is_some_and(|t| now.wrapping_sub(t) < SYNC_QUIET_MS) {
            return false;
        }
        let d = [desk[0] as f32, desk[1] as f32];
        let Some((i, s)) = screens_of(layout, pc).find(|(_, s)| {
            let (min, max) = desk_rect(s);
            in_span(d[0], min[0], max[0]) && in_span(d[1], min[1], max[1])
        }) else {
            return false;
        };
        let k = 100.0 / s.scale.max(1) as f32;
        let p = [s.at[0] as f32 + (d[0] - s.desk[0] as f32) * k, s.at[1] as f32 + (d[1] - s.desk[1] as f32) * k];
        self.cursor[pc.index()] = Some(d);
        if active {
            if self.slot == Some(i) && (p[0] - self.pos[0]).abs() < 1.5 && (p[1] - self.pos[1]).abs() < 1.5 {
                return false;
            }
            self.pos = p;
            self.slot = Some(i);
            self.reset_push();
        } else {
            self.left[pc.index()] = Some(p);
        }
        true
    }
}

// ── acceleration ─────────────────────────────────────────────────────────────

const TRACKERS: usize = 16;
/// Reports older than this, or after a pause this long, don't count for the speed.
const TRACK_MS: u32 = 300;
/// The speed is measured back as far as it stays within this of the latest (counts per ms), as libinput does.
const MAX_VELOCITY_DIFF: f32 = 1.0;

/// Windows' "Enhance pointer precision" (SmoothMouseXCurve / SmoothMouseYCurve): mouse speed to pointer speed, in
/// inches per second, with Windows' assumptions of 400 counts and 96 pixels to the inch.
const CURVE_X: [f32; 5] = [0.0, 0.43, 1.25, 3.86, 40.0];
const CURVE_Y: [f32; 5] = [0.0, 1.07, 4.14, 18.98, 443.75];
const COUNTS_PER_INCH: f32 = 400.0;
const PX_PER_INCH: f32 = 96.0;
/// Windows' pointer speed slider, positions 1–20 (10 is 1:1); speed 0 is position 10.
const SLIDER: [f32; 20] = [
    0.03125, 0.0625, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0,
    3.25, 3.5,
];

#[derive(Debug, Clone, Copy, Default)]
struct Track {
    dx: i32,
    dy: i32,
    dt: u32,
    dir: u8,
}

/// Turns mouse counts into pointer pixels.
#[derive(Debug, Clone)]
pub struct Accelerator {
    tracks: [Track; TRACKERS],
    n: usize,
    next: usize,
    last_at: Option<u32>,
    last_v: f32,
}

impl Default for Accelerator {
    fn default() -> Self {
        Self::NEW
    }
}

/// The eight directions as bits (east, then clockwise with y down), with the two next to the one moved in: the speed
/// is measured back only while the direction stays within these.
fn direction(dx: i32, dy: i32) -> u8 {
    let (ax, ay) = (dx.unsigned_abs() as f32, dy.unsigned_abs() as f32);
    let tan_22_5 = 0.414_213_57;
    let octant = if ay <= ax * tan_22_5 {
        if dx > 0 { 0 } else { 4 }
    } else if ax <= ay * tan_22_5 {
        if dy > 0 { 2 } else { 6 }
    } else {
        match (dx > 0, dy > 0) {
            (true, true) => 1,
            (false, true) => 3,
            (false, false) => 5,
            (true, false) => 7,
        }
    };
    let m = 1u8 << octant;
    m | m.rotate_left(1) | m.rotate_right(1)
}

fn hypot(x: i64, y: i64) -> f32 {
    let sq = (x * x + y * y) as u64;
    (sq << 16).isqrt() as f32 / 256.0
}

fn slider(speed: i8) -> f32 {
    SLIDER[(10 + speed as i32).clamp(1, 20) as usize - 1]
}

/// Pixels per count of Windows' curve at `v` counts per ms.
fn precision(v: f32) -> f32 {
    let x = v * 1000.0 / COUNTS_PER_INCH;
    let ratio = if x <= 1e-4 {
        CURVE_Y[1] / CURVE_X[1]
    } else {
        let i = (1..CURVE_X.len() - 1).find(|i| x < CURVE_X[*i]).unwrap_or(CURVE_X.len() - 1);
        let (x0, x1, y0, y1) = (CURVE_X[i - 1], CURVE_X[i], CURVE_Y[i - 1], CURVE_Y[i]);
        (y0 + (x - x0) * (y1 - y0) / (x1 - x0)) / x
    };
    ratio * PX_PER_INCH / COUNTS_PER_INCH
}

/// libinput's adaptive profile at `v` counts per ms (counted as 1000 per inch, libinput's default) and speed setting
/// `s` (−1–1): slower below 0.07, 1:1 up to the threshold, then a straight rise to the cap.
fn adaptive(v: f32, s: f32) -> f32 {
    let threshold = (0.4 - 0.25 * s).max(0.2);
    let max = 2.0 + 1.5 * s;
    let incline = 1.1 + 0.75 * s;
    let f = if v < 0.07 {
        10.0 * v + 0.3
    } else if v < threshold {
        1.0
    } else {
        incline * (v - threshold) + 1.0
    };
    f.min(max)
}

impl Accelerator {
    pub const NEW: Accelerator = Accelerator {
        tracks: [Track { dx: 0, dy: 0, dt: 0, dir: 0 }; TRACKERS],
        n: 0,
        next: 0,
        last_at: None,
        last_v: 0.0,
    };

    /// The speed now, in counts per ms: back over the latest reports while the direction holds, within
    /// [`TRACK_MS`], and while it stays close to the latest's.
    fn velocity(&self) -> f32 {
        let (mut sx, mut sy, mut st) = (0i64, 0i64, 0u32);
        let mut dir = 0xFF;
        let (mut v, mut first) = (0.0, None);
        for k in 0..self.n {
            let t = self.tracks[(self.next + TRACKERS - 1 - k) % TRACKERS];
            if k > 0 && st + t.dt > TRACK_MS {
                break;
            }
            dir &= t.dir;
            if dir == 0 {
                break;
            }
            sx += t.dx as i64;
            sy += t.dy as i64;
            st += t.dt;
            let vt = hypot(sx, sy) / st.max(1) as f32;
            match first {
                None => first = Some(vt),
                Some(f) if (vt - f).abs() > MAX_VELOCITY_DIFF => break,
                _ => {}
            }
            v = vt;
        }
        v
    }

    /// The pixels `dx`, `dy` counts at `now` (ms) move the pointer.
    pub fn apply(&mut self, dx: i32, dy: i32, now: u32, accel: Accel, speed: i8) -> [f32; 2] {
        if dx == 0 && dy == 0 {
            return [0.0; 2];
        }
        let dt = self.last_at.map_or(TRACK_MS, |t| now.wrapping_sub(t).clamp(1, TRACK_MS));
        if dt >= TRACK_MS {
            // After a pause the speed starts over (the first report counts as slow, as in libinput).
            self.n = 0;
            self.last_v = 0.0;
        }
        self.last_at = Some(now);
        self.tracks[self.next] = Track { dx, dy, dt, dir: direction(dx, dy) };
        self.next = (self.next + 1) % TRACKERS;
        self.n = (self.n + 1).min(TRACKERS);
        let v = self.velocity();
        let gain = match accel {
            Accel::Flat => slider(speed),
            Accel::Precision => precision(v) * slider(speed),
            Accel::Adaptive => {
                // libinput averages the factor between the last speed and this one (Simpson's rule).
                let s = speed as f32 / 10.0;
                let last = self.last_v;
                (adaptive(v, s) + adaptive(last, s) + 4.0 * adaptive((v + last) / 2.0, s)) / 6.0
            }
        };
        self.last_v = v;
        [dx as f32 * gain, dy as f32 * gain]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(screens: &[Screen]) -> Layout {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[..screens.len()].copy_from_slice(screens);
        l
    }

    /// PC1: one 1920×1080 screen; PC2 to its right: 2560×1440 at 125 % (2048×1152 logical), its top 36 higher.
    fn two_pcs() -> Layout {
        layout(&[
            Screen::new(Pc::Pc1, 1920, 1080, [0, 0]),
            Screen { scale: 125, ..Screen::new(Pc::Pc2, 2560, 1440, [1920, -36]) },
        ])
    }

    fn rules(resistance: u16) -> Rules {
        Rules { resistance, corner: 20, buttons: false, open: [true, true], mapping: [Mapping::Desktop; 2] }
    }

    #[test]
    fn the_pointer_starts_in_the_middle_of_the_main_screen_and_maps_onto_the_desktop() {
        let l = two_pcs();
        let mut p = Pointer::NEW;
        assert_eq!(p.here(&l, Pc::Pc1, Mapping::Desktop).as_slice(), &[Report::Abs([16392, 16399])]);
        assert_eq!(p.position(), [960, 540]);
        let s = p.motion(&l, Pc::Pc1, [-2000.0, -2000.0], 0, &rules(100));
        assert_eq!(s.reports.as_slice(), &[Report::Abs([0, 0])]);
        let s = p.motion(&l, Pc::Pc1, [0.0, 5000.0], 0, &rules(100));
        assert_eq!(s.reports.as_slice(), &[Report::Abs([0, ABS_MAX])]);
        // Nothing to the left or below: walls.
        assert_eq!(p.position(), [0, 1079]);
    }

    #[test]
    fn an_edge_to_the_other_pc_holds_until_pushed_through() {
        let l = two_pcs();
        let mut p = Pointer::NEW;
        p.enter(&l, Pc::Pc1);
        let r = rules(100);
        let s = p.motion(&l, Pc::Pc1, [1000.0, 0.0], 0, &r);
        assert_eq!((s.hop, p.position()), (None, [1919, 540]));
        // 59 + 40 = 99 of 100: still on PC1.
        assert_eq!(p.motion(&l, Pc::Pc1, [40.0, 0.0], 10, &r).hop, None);
        // A pause starts the push over.
        assert_eq!(p.motion(&l, Pc::Pc1, [60.0, 0.0], 10 + PUSH_PAUSE_MS + 1, &r).hop, None);
        assert_eq!(p.motion(&l, Pc::Pc1, [39.0, 0.0], 500, &r).hop, None);
        let s = p.motion(&l, Pc::Pc1, [1.0, 0.0], 510, &r);
        assert_eq!(s.hop, Some(Pc::Pc2));
        // PC1 keeps its cursor at the edge; PC2 gets the pointer at its left edge, at the same height.
        assert_eq!(s.reports.as_slice(), &[Report::Abs([ABS_MAX, 16399])]);
        assert_eq!(p.position(), [1920, 540]);
        // 540 + 36 = 576 of PC2's 1152 logical lines: its 720th of 1440 pixels.
        assert_eq!(p.here(&l, Pc::Pc2, Mapping::Desktop).as_slice(), &[Report::Abs([0, 16395])]);
        // And back: the push starts from nothing on the new PC.
        let s = p.motion(&l, Pc::Pc2, [-1.0, 0.0], 520, &r);
        assert_eq!(s.hop, None);
        let s = p.motion(&l, Pc::Pc2, [-100.0, 0.0], 530, &r);
        assert_eq!(s.hop, Some(Pc::Pc1));
    }

    #[test]
    fn no_hop_near_corners_with_a_button_held_or_to_a_pc_that_is_off() {
        let l = two_pcs();
        let mut p = Pointer::NEW;
        p.enter(&l, Pc::Pc1);
        let mut r = rules(0);
        r.buttons = true;
        assert_eq!(p.motion(&l, Pc::Pc1, [5000.0, 0.0], 0, &r).hop, None);
        r.buttons = false;
        r.open = [true, false];
        assert_eq!(p.motion(&l, Pc::Pc1, [10.0, 0.0], 1, &r).hop, None);
        r.open = [true, true];
        // 10 pixels from the top corner: within the 20 of the corner.
        p.motion(&l, Pc::Pc1, [0.0, -530.0], 2, &r);
        assert_eq!(p.position(), [1919, 10]);
        assert_eq!(p.motion(&l, Pc::Pc1, [10.0, 0.0], 3, &r).hop, None);
        p.motion(&l, Pc::Pc1, [0.0, 10.0], 4, &r);
        assert_eq!(p.motion(&l, Pc::Pc1, [1.0, 0.0], 5, &r).hop, Some(Pc::Pc2));
    }

    #[test]
    fn the_same_pcs_screens_pass_the_pointer_on_across_a_gap_and_map_as_one_desktop() {
        // PC1: two screens side by side in its own arrangement, placed with a 100-pixel gap here; PC2 below the second.
        let l = layout(&[
            Screen::new(Pc::Pc1, 1920, 1080, [0, 0]),
            Screen { desk: [1920, 0], ..Screen::new(Pc::Pc1, 1920, 1080, [2020, 0]) },
            Screen::new(Pc::Pc2, 1920, 1080, [2020, 1080]),
        ]);
        let mut p = Pointer::NEW;
        p.enter(&l, Pc::Pc1);
        let s = p.motion(&l, Pc::Pc1, [1000.0, 0.0], 0, &rules(0));
        // 41 past the first screen's last pixel: the second's 41st, the gap jumped.
        assert_eq!((s.hop, p.screen(), p.position()), (None, Some(1), [2060, 540]));
        // In PC1's own pixels that is 1920 + 40 of 3840.
        assert_eq!(s.reports.as_slice(), &[Report::Abs([16729, 16399])]);
        // Down from the first screen: a wall (PC2 is below the second only).
        let mut q = Pointer::NEW;
        q.enter(&l, Pc::Pc1);
        assert_eq!(q.motion(&l, Pc::Pc1, [0.0, 1000.0], 0, &rules(0)).hop, None);
        assert_eq!(p.motion(&l, Pc::Pc1, [0.0, 1000.0], 0, &rules(0)).hop, Some(Pc::Pc2));
    }

    #[test]
    fn a_pc_that_maps_only_its_main_screen_gets_relative_motion_on_the_others() {
        // PC1's main screen (0,0) on the right, its second one at -1920 of its own arrangement on the left.
        let l = layout(&[
            Screen::new(Pc::Pc1, 1920, 1080, [1920, 0]),
            Screen { desk: [-1920, 0], ..Screen::new(Pc::Pc1, 1920, 1080, [0, 0]) },
        ]);
        let mut r = rules(0);
        r.mapping = [Mapping::MainScreen; 2];
        let mut p = Pointer::NEW;
        p.enter(&l, Pc::Pc1);
        assert_eq!(p.screen(), Some(0));
        assert_eq!(p.here(&l, Pc::Pc1, Mapping::MainScreen).as_slice(), &[Report::Abs([16392, 16399])]);
        // 1000 left: 40 pixels into the second screen; the cursor goes there from the middle of the main one.
        let s = p.motion(&l, Pc::Pc1, [-1000.0, 0.0], 0, &r);
        assert_eq!(s.reports.as_slice(), &[Report::Rel([-1000, 0])]);
        assert_eq!(p.screen(), Some(1));
        let s = p.motion(&l, Pc::Pc1, [-10.5, 3.0], 1, &r);
        assert_eq!(s.reports.as_slice(), &[Report::Rel([-11, 3])]);
        // Back onto the main screen: absolute again, exactly where the pointer is (949.5 of its 1920 pixels).
        let s = p.motion(&l, Pc::Pc1, [1000.0, 0.0], 2, &r);
        assert_eq!(s.reports.as_slice(), &[Report::Abs([16213, 16490])]);
        // A cursor position not known (after a reset): first onto the main screen's nearest point, then across.
        p.motion(&l, Pc::Pc1, [-1500.0, 0.0], 3, &r);
        p.forget_cursor(Pc::Pc1);
        assert_eq!(
            p.here(&l, Pc::Pc1, Mapping::MainScreen).as_slice(),
            &[Report::Abs([0, 16490]), Report::Rel([-551, 0])]
        );
    }

    #[test]
    fn the_pcs_own_cursor_is_taken_over_once_the_pointer_rests() {
        let l = two_pcs();
        let mut p = Pointer::NEW;
        p.enter(&l, Pc::Pc1);
        p.motion(&l, Pc::Pc1, [10.0, 0.0], 1000, &rules(100));
        assert!(!p.sync(&l, Pc::Pc1, [100, 100], 1000 + SYNC_QUIET_MS - 1, true));
        assert!(p.sync(&l, Pc::Pc1, [100, 100], 1000 + SYNC_QUIET_MS, true));
        assert_eq!(p.position(), [100, 100]);
        // The inactive PC: where the pointer is found on return. Its pixels at 125 % are 0.8 logical ones.
        assert!(p.sync(&l, Pc::Pc2, [1000, 500], 5000, false));
        p.leave(&l, Pc::Pc1);
        p.enter(&l, Pc::Pc2);
        assert_eq!(p.position(), [2720, 364]);
        assert!(!p.sync(&l, Pc::Pc2, [9000, 9000], 6000, true), "not on any of its screens");
    }

    #[test]
    fn acceleration_profiles() {
        // Flat: the slider alone.
        let mut a = Accelerator::NEW;
        assert_eq!(a.apply(10, -4, 0, Accel::Flat, 0), [10.0, -4.0]);
        assert_eq!(a.apply(10, 0, 1, Accel::Flat, 4), [20.0, 0.0]);
        assert_eq!(slider(-10), 0.03125);
        // Precision: slow movement slower than 1:1, fast movement faster, and rising in between.
        assert!((precision(0.0) - 0.597).abs() < 0.001);
        let gains: std::vec::Vec<f32> = [0.1, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 40.0].map(precision).to_vec();
        assert!(gains.windows(2).all(|w| w[1] > w[0]), "{gains:?}");
        assert!(gains[0] < 0.7 && gains[7] > 2.5, "{gains:?}");
        // Adaptive: libinput's shape at speed 0.
        assert!((adaptive(0.0, 0.0) - 0.3).abs() < 1e-6);
        assert_eq!(adaptive(0.2, 0.0), 1.0);
        assert!((adaptive(1.0, 0.0) - 1.66).abs() < 1e-4);
        assert_eq!(adaptive(10.0, 0.0), 2.0);
        assert_eq!(adaptive(10.0, -1.0), 0.5);
    }

    #[test]
    fn the_speed_is_measured_over_the_recent_reports() {
        let mut a = Accelerator::NEW;
        // The first report after a pause counts as slow: decelerated.
        let first = a.apply(4, 0, 1000, Accel::Adaptive, 0);
        assert!(first[0] < 4.0, "{first:?}");
        // 4 counts every millisecond for a while: 4 counts/ms, factor near the cap of 2.
        let mut last = [0.0; 2];
        for t in 1001..1020 {
            last = a.apply(4, 0, t, Accel::Adaptive, 0);
        }
        assert!(last[0] > 7.5, "{last:?}");
        // Reversing direction starts the measurement over at this report alone (1 count in 1 ms); the factor still
        // averages with the speed before, as libinput's does: (f(1) + f(4) + 4 f(2.5)) / 6.
        let back = a.apply(-1, 0, 1020, Accel::Adaptive, 0);
        assert_eq!(a.velocity(), 1.0);
        assert!((back[0] + 1.9433).abs() < 1e-3, "{back:?}");
        assert_eq!(direction(5, 0), 0b1000_0011);
        assert_eq!(direction(0, -5), 0b1110_0000);
        assert_eq!(hypot(3, 4), 5.0);
    }
}
