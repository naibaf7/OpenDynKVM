//! The Screens page (screen hop, software.md §4.14 and §9): whether screen hop runs and what it needs, which profiles
//! use it, the arrangement of both PCs' screens (dragged into place on a canvas, each screen's resolution, scaling and
//! place on its own PC), and how the pointer moves.
//!
//! The arrangement is the KVM's. The page edits a draft of it (screens, each PC's mapping and units): drags, screens
//! added, removed or edited, and detection all go there, and to the KVM only with Apply, all of it at once. After Apply
//! the draft stays until the KVM's copy has it, so that a refusal keeps the edits.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use kvm_proto::ctrl::{
    Accel, ConfigValue, HOP_CORNER_MAX, HOP_SPEED, HopState, MAX_SCREENS, Mapping, Pc, Port, PortMode, SCREEN_PX,
    Screen, Units,
};
use odkvmctl::client::{HopView, Snapshot};
use odkvmctl::screens::Detection;
use odkvmctl::{fl, names};

use crate::look::{self, BAD, GOOD, WARN, card, chip, ink, note};
use crate::{App, pc_colors};

/// Resolutions to pick from; any other can be typed.
const RESOLUTIONS: &[(u16, u16)] = &[
    (1280, 720),
    (1280, 1024),
    (1366, 768),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1080),
    (2560, 1440),
    (2560, 1600),
    (2880, 1800),
    (3000, 2000),
    (3440, 1440),
    (3840, 1600),
    (3840, 2160),
    (5120, 1440),
    (5120, 2160),
    (5120, 2880),
    (7680, 2160),
];
const SCALES: &[u16] = &[100, 125, 150, 175, 200, 225, 250, 300];
/// The canvas's height range (it takes the arrangement's shape), and how close (in its points) a dragged screen's
/// edge snaps to another's.
const CANVAS_HEIGHT: std::ops::RangeInclusive<f32> = 180.0..=340.0;
const SNAP: f32 = 10.0;
/// Edge resistance the slider offers (the KVM takes up to `HOP_RESISTANCE_MAX`).
const RESISTANCE_SLIDER: u16 = 1000;

/// How often this computer's display settings are read again while the page shows (to compare with the KVM's copy).
const RECHECK: Duration = Duration::from_secs(3);

/// After Apply, the draft stays at least this long, until the KVM's copy has it: the window's own copy takes a setting
/// as soon as it is sent, before the KVM answered (and it may refuse).
const APPLY_GRACE: Duration = Duration::from_secs(2);

/// The arrangement as the page edits it: the screens, and each PC's mapping and units. Edits go to a draft of it, and
/// to the KVM only with Apply, all at once.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Arrangement {
    screens: [Screen; MAX_SCREENS],
    mapping: [Mapping; 2],
    units: [Units; 2],
}

impl Arrangement {
    fn of(hop: &HopView) -> Arrangement {
        Arrangement { screens: hop.screens, mapping: hop.mapping, units: hop.units }
    }
}

/// Edits not yet applied, or applied and not yet confirmed by the KVM's copy.
#[derive(Debug, Clone, Copy)]
struct Draft {
    edit: Arrangement,
    applied: Option<Instant>,
}

/// What the page keeps between frames.
#[derive(Default)]
pub struct Editor {
    selected: Option<usize>,
    /// The screen being dragged: its slot, the arrangement as it is being changed, the canvas's mapping frozen at the
    /// start of the drag, and where in the screen it was grabbed (arrangement pixels).
    drag: Option<Drag>,
    /// How detecting this computer's screens went.
    detect_note: Option<Result<String, String>>,
    /// The edit typed a place on its computer: keep it, don't derive it from the arrangement.
    desk_typed: bool,
    /// The arrangement being edited, until applied and confirmed (or discarded).
    draft: Option<Draft>,
    /// This computer's screens and desktop as its system reports them, and when they were read.
    now: Option<(Instant, Result<Detection, String>)>,
    /// Each screen as drawn last, and where (the tests drag them).
    #[cfg(test)]
    drawn: Vec<(usize, Rect, Screen)>,
}

impl Editor {
    /// The arrangement as shown and edited: the draft, or the KVM's.
    fn current(&self, hop: &HopView) -> Arrangement {
        self.draft.map_or_else(|| Arrangement::of(hop), |d| d.edit)
    }

    /// Drops the draft once the KVM's copy is what it says (after Apply, and its grace).
    fn settle(&mut self, hop: &HopView) {
        let Some(d) = self.draft else { return };
        let same = d.edit == Arrangement::of(hop);
        let settled = match d.applied {
            None => same,
            Some(at) => same && at.elapsed() > APPLY_GRACE,
        };
        if settled {
            self.draft = None;
        }
    }
}

#[cfg(test)]
impl Editor {
    /// Edits wait for Apply, or for the KVM to confirm them.
    pub fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    /// Selects screen slot `slot` (the screenshots show its editor).
    pub fn select(&mut self, slot: Option<usize>) {
        self.selected = slot;
    }

    /// Screen slot `slot` as drawn in the last frame, and where.
    pub fn drawn(&self, slot: usize) -> Option<(Rect, Screen)> {
        self.drawn.iter().find(|(i, ..)| *i == slot).map(|(_, r, s)| (*r, *s))
    }
}

struct Drag {
    /// The screen moved, and the one whose area the drag started on (on top of it, where screens overlap).
    slot: usize,
    widget: usize,
    /// The screen as it was when the drag started: sent only if it moved.
    from: Screen,
    screens: [Screen; MAX_SCREENS],
    view: View,
    /// The canvas's height, kept too.
    height: f32,
    grab: Vec2,
}

/// Arrangement pixels to canvas points.
#[derive(Debug, Clone, Copy)]
struct View {
    origin: Pos2,
    min: Vec2,
    scale: f32,
}

/// The part of the arrangement the canvas shows: the screens, with room around them to drag them into.
fn shown(screens: &[Screen]) -> (Vec2, Vec2) {
    let (min, max) = screens
        .iter()
        .filter(|s| s.pc.is_some())
        .map(world)
        .reduce(|(a0, a1), (b0, b1)| (a0.min(b0), a1.max(b1)))
        .unwrap_or((Vec2::ZERO, Vec2::new(3840.0, 1080.0)));
    let pad = (max - min) * 0.12 + Vec2::splat(200.0);
    (min - pad, max + pad)
}

impl View {
    fn fit(canvas: Rect, screens: &[Screen]) -> View {
        let (min, max) = shown(screens);
        let size = max - min;
        let scale = (canvas.width() / size.x).min(canvas.height() / size.y);
        let used = size * scale;
        let origin = canvas.min + (canvas.size() - used) / 2.0;
        View { origin, min, scale }
    }

    fn canvas(self, p: Vec2) -> Pos2 {
        self.origin + (p - self.min) * self.scale
    }

    fn world(self, p: Pos2) -> Vec2 {
        (p - self.origin) / self.scale + self.min
    }
}

/// A screen's place and size in the arrangement.
fn world(s: &Screen) -> (Vec2, Vec2) {
    let [w, h] = s.size();
    let min = Vec2::new(s.at[0] as f32, s.at[1] as f32);
    (min, min + Vec2::new(w as f32, h as f32))
}

/// A screen in use that screen hop goes onto.
fn on_for_hop(s: &Screen) -> bool {
    s.pc.is_some() && !s.off
}

fn overlaps(a: &Screen, b: &Screen) -> bool {
    let ((a0, a1), (b0, b1)) = (world(a), world(b));
    // A few pixels are allowed: the KVM still finds the neighbour.
    a0.x < b1.x - 8.0 && b0.x < a1.x - 8.0 && a0.y < b1.y - 8.0 && b0.y < a1.y - 8.0
}

/// `at` for screen `slot` near `want`, its edges snapped to the others' within `tol` (arrangement pixels).
fn snap(screens: &[Screen; MAX_SCREENS], slot: usize, want: Vec2, tol: f32) -> [i16; 2] {
    let [w, h] = screens[slot].size();
    let size = Vec2::new(w as f32, h as f32);
    let mut best = [(tol, want.x), (tol, want.y)];
    for (i, o) in screens.iter().enumerate() {
        if i == slot || o.pc.is_none() {
            continue;
        }
        let (o0, o1) = world(o);
        for (axis, (lo, hi, len)) in [(o0.x, o1.x, size.x), (o0.y, o1.y, size.y)].into_iter().enumerate() {
            for c in [hi, lo - len, lo, hi - len] {
                let d = (c - want[axis]).abs();
                if d < best[axis].0 {
                    best[axis] = (d, c);
                }
            }
        }
    }
    let clamp = |v: f32| v.round().clamp(-30_000.0, 30_000.0) as i16;
    [clamp(best[0].1), clamp(best[1].1)]
}

/// A screen added by hand: right of all the others here, and on its PC right of its other screens; its place there
/// then follows from where it is dragged ([`odkvmctl::screens::desks`]).
fn new_screen(screens: &[Screen; MAX_SCREENS], pc: Pc, units: Units, w: u16, h: u16) -> Screen {
    let right = screens.iter().filter(|s| s.pc.is_some()).map(|s| world(s).1.x as i32).max().unwrap_or(0);
    let top = screens.iter().filter(|s| s.pc.is_some()).map(|s| s.at[1] as i32).min().unwrap_or(0);
    let desk_right =
        screens.iter().filter(|s| s.pc == Some(pc)).map(|s| s.desk[0] as i32 + s.desk_size(units)[0]).max();
    Screen {
        desk: [desk_right.unwrap_or(0).clamp(i16::MIN as i32, i16::MAX as i32) as i16, 0],
        ..Screen::new(pc, w, h, [right.min(30_000) as i16, top as i16])
    }
}

fn state_color(s: HopState) -> Color32 {
    match s {
        HopState::Active => GOOD,
        HopState::Off => Color32::GRAY,
        _ => WARN,
    }
}

impl App {
    /// Changes the draft (made from the KVM's arrangement if there is none); a draft no different from the KVM's goes.
    fn edit(&mut self, hop: &HopView, f: impl FnOnce(&mut Arrangement)) {
        let mut a = self.screens.current(hop);
        f(&mut a);
        self.screens.draft = (a != Arrangement::of(hop)).then_some(Draft { edit: a, applied: None });
    }

    /// `next` as the draft's screens, after giving `pcs`' screens their place on their own computer from the
    /// arrangement where it isn't fixed (`odkvmctl::screens::desks`): where a screen is dragged is where it is on its PC.
    fn arrange(&mut self, hop: &HopView, next: [Screen; MAX_SCREENS], pcs: &[Pc]) {
        self.edit(hop, |a| {
            a.screens = next;
            for pc in pcs {
                a.screens = odkvmctl::screens::desks(&a.screens, *pc, a.units[pc.index()]);
            }
        });
    }

    /// Screen `slot` as edited (`was`: the PC it belonged to), with the places on their computers derived again unless
    /// the edit typed one in.
    fn edit_screen(
        &mut self,
        hop: &HopView,
        screens: &[Screen; MAX_SCREENS],
        slot: usize,
        sc: Screen,
        was: Option<Pc>,
    ) {
        let mut next = *screens;
        next[slot] = sc;
        if core::mem::take(&mut self.screens.desk_typed) {
            self.edit(hop, |a| a.screens = next);
            return;
        }
        let pcs: Vec<Pc> = [was, sc.pc].into_iter().flatten().collect();
        self.arrange(hop, next, &pcs);
    }

    /// Sends the draft: every screen slot, mapping and units that differ from the KVM's. It stays until the KVM's copy
    /// has it all, so that a refusal leaves the edits to try again.
    pub(crate) fn apply(&mut self, hop: &HopView) {
        let Some(d) = self.screens.draft.as_mut() else { return };
        let sent = d.edit;
        d.applied = Some(Instant::now());
        let kvm = Arrangement::of(hop);
        for (i, (a, b)) in kvm.screens.iter().zip(sent.screens.iter()).enumerate() {
            if a != b {
                self.set(ConfigValue::Screen(i as u8, *b));
            }
        }
        for pc in [Pc::Pc1, Pc::Pc2] {
            if kvm.mapping[pc.index()] != sent.mapping[pc.index()] {
                self.set(ConfigValue::HopMapping(pc, sent.mapping[pc.index()]));
            }
            if kvm.units[pc.index()] != sent.units[pc.index()] {
                self.set(ConfigValue::HopUnits(pc, sent.units[pc.index()]));
            }
        }
    }

    pub(crate) fn screens(&mut self, ui: &mut Ui, s: &Snapshot) {
        let Some(hop) = s.hop() else {
            card(ui, "", |ui| note(ui, fl!("hop-firmware-too-old")));
            return;
        };
        self.screens.settle(&hop);
        // Shown while configuration from software is off, not editable.
        let locked = App::locked_note(ui, s);
        ui.add_enabled_ui(!locked, |ui| {
            self.hop_status_card(ui, s, &hop);
            ui.add_space(8.0);
            self.arrangement_card(ui, s, &hop);
            ui.add_space(8.0);
            self.pointer_card(ui, &hop);
        });
    }

    fn hop_status_card(&mut self, ui: &mut Ui, s: &Snapshot, hop: &HopView) {
        card(ui, "", |ui| {
            ui.horizontal(|ui| {
                chip(ui, &fl!("hop-line", state = names::hop_state(s.status.hop)), state_color(s.status.hop));
            });
            note(ui, fl!("hop-what"));
            // What it needs, a click away.
            let fix = match s.status.hop {
                HopState::NoAbsolute => Some((fl!("hop-turn-on-absolute"), ConfigValue::AbsPointer(true))),
                HopState::NotNormalized => {
                    Some((fl!("hop-mouse-normalized"), ConfigValue::PortMode(Port::Mouse, PortMode::Normalized)))
                }
                _ => None,
            };
            if let Some((label, v)) = fix
                && ui.button(label).clicked()
            {
                self.set(v);
            }
            ui.add_space(4.0);
            if let Some((active, count, list)) = s.profiles() {
                ui.label(RichText::new(fl!("hop-in-profiles")).strong());
                ui.horizontal_wrapped(|ui| {
                    for (i, p) in list.iter().enumerate().take(count as usize) {
                        let mut on = p.hop;
                        let label = if i as u8 == active {
                            fl!("hop-profile-active", name = p.name.as_str())
                        } else {
                            p.name.clone()
                        };
                        if ui.checkbox(&mut on, label).changed() {
                            self.set(ConfigValue::ProfileHop(i as u8, on));
                        }
                    }
                });
            }
            ui.add_space(4.0);
            let mut abs = hop.abs_pointer;
            if ui.checkbox(&mut abs, fl!("hop-absolute-pointer")).changed() {
                self.set(ConfigValue::AbsPointer(abs));
            }
            note(ui, fl!("hop-absolute-pointer-note"));
        });
    }

    fn arrangement_card(&mut self, ui: &mut Ui, s: &Snapshot, hop: &HopView) {
        let colors = pc_colors(s).map(look::color32);
        let cur = self.screens.current(hop);
        let screens = self.screens.drag.as_ref().map_or(cur.screens, |d| d.screens);
        card(ui, &fl!("hop-arrangement"), |ui| {
            note(ui, fl!("hop-arrangement-note"));
            self.canvas(ui, s, hop, &screens, cur.mapping, colors);
            let overlap = (0..MAX_SCREENS).any(|i| {
                (i + 1..MAX_SCREENS)
                    .any(|j| on_for_hop(&screens[i]) && on_for_hop(&screens[j]) && overlaps(&screens[i], &screens[j]))
            });
            if overlap {
                ui.label(RichText::new(fl!("hop-overlap")).color(ink(ui, WARN)));
            }
            let free = screens.iter().any(|x| x.pc.is_none());
            ui.horizontal_wrapped(|ui| {
                for pc in [Pc::Pc1, Pc::Pc2] {
                    let label = fl!("hop-add-screen", pc = pc.name().to_uppercase());
                    if ui.add_enabled(free, egui::Button::new(label)).clicked()
                        && let Some(slot) = screens.iter().position(|x| x.pc.is_none())
                    {
                        let mut next = screens;
                        next[slot] = new_screen(&screens, pc, cur.units[pc.index()], 1920, 1080);
                        self.arrange(hop, next, &[pc]);
                        self.screens.selected = Some(slot);
                    }
                }
                if let Some(pc) = s.this_pc() {
                    let label = fl!("hop-detect", pc = pc.name().to_uppercase());
                    if ui.button(label).on_hover_text(fl!("hop-detect-hover")).clicked() {
                        self.detect(hop, pc);
                    }
                }
                if let Some(i) = self.screens.selected.filter(|i| screens[*i].pc.is_some())
                    && ui.button(fl!("hop-remove-screen")).clicked()
                {
                    let mut next = screens;
                    next[i] = Screen::NONE;
                    let pcs: Vec<Pc> = screens[i].pc.into_iter().collect();
                    self.arrange(hop, next, &pcs);
                    self.screens.selected = None;
                }
            });
            // Edits wait here until applied: the KVM keeps the arrangement it has meanwhile.
            if let Some(d) = self.screens.draft {
                let waiting = d.applied.is_some();
                let text = if waiting { fl!("hop-applying") } else { fl!("hop-draft") };
                ui.label(RichText::new(text).color(ink(ui, WARN)));
                ui.horizontal(|ui| {
                    if ui.button(fl!("hop-apply")).clicked() {
                        self.apply(hop);
                    }
                    if ui.button(fl!("hop-discard")).clicked() {
                        self.screens.draft = None;
                    }
                });
            }
            if let Some(pc) = s.this_pc() {
                self.freshness(ui, &cur, hop, pc);
            }
            match &self.screens.detect_note {
                Some(Ok(t)) => note(ui, t.clone()),
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(ink(ui, BAD)));
                }
                None => {}
            }
            if let Some(i) = self.screens.selected.filter(|i| screens[*i].pc.is_some()) {
                ui.separator();
                self.screen_editor(ui, hop, &screens, i);
            }
            ui.separator();
            egui::Grid::new("hop-mapping").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for pc in [Pc::Pc1, Pc::Pc2] {
                    ui.label(fl!("hop-mapping-of", pc = pc.name().to_uppercase()));
                    let mut m = cur.mapping[pc.index()];
                    egui::ComboBox::from_id_salt(("hop-mapping", pc.index()))
                        .selected_text(names::mapping(m))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for x in Mapping::ALL {
                                ui.selectable_value(&mut m, *x, names::mapping(*x));
                            }
                        });
                    if m != cur.mapping[pc.index()] {
                        self.edit(hop, |a| a.mapping[pc.index()] = m);
                    }
                    ui.end_row();
                    ui.label(fl!("hop-units-of", pc = pc.name().to_uppercase()));
                    let mut u = cur.units[pc.index()];
                    egui::ComboBox::from_id_salt(("hop-units", pc.index()))
                        .selected_text(names::units(u))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for x in Units::ALL {
                                ui.selectable_value(&mut u, *x, names::units(*x));
                            }
                        });
                    if u != cur.units[pc.index()] {
                        self.edit(hop, |a| a.units[pc.index()] = u);
                    }
                    ui.end_row();
                    // This computer runs Windows, which puts the absolute pointer on its main screen alone: across all
                    // screens, every position lands off and the other screens can't be reached (2026-11-12).
                    if cfg!(windows) && s.this_pc() == Some(pc) && m == Mapping::Desktop {
                        ui.label("");
                        ui.label(RichText::new(fl!("hop-mapping-windows")).color(ink(ui, WARN)));
                        ui.end_row();
                    }
                }
            });
            note(ui, fl!("hop-mapping-note"));
        });
    }

    /// The screens drawn to scale in their PC's colour, the pointer on them while screen hop runs. A screen is selected
    /// by a click and moved by dragging it; its edges snap to the others'.
    fn canvas(
        &mut self,
        ui: &mut Ui,
        s: &Snapshot,
        hop: &HopView,
        screens: &[Screen; MAX_SCREENS],
        mapping: [Mapping; 2],
        colors: [Color32; 2],
    ) {
        let width = ui.available_width();
        let height = match &self.screens.drag {
            Some(d) => d.height,
            None => {
                let (min, max) = shown(screens);
                (width * (max.y - min.y) / (max.x - min.x)).clamp(*CANVAS_HEIGHT.start(), *CANVAS_HEIGHT.end())
            }
        };
        let (canvas, bg) = ui.allocate_exact_size(Vec2::new(width, height), Sense::click());
        let painter = ui.painter_at(canvas);
        let dark = ui.visuals().dark_mode;
        painter.rect_filled(canvas, CornerRadius::same(8), ui.visuals().extreme_bg_color);
        let view = self.screens.drag.as_ref().map_or_else(|| View::fit(canvas, screens), |d| d.view);
        if screens.iter().all(|x| x.pc.is_none()) {
            painter.text(
                canvas.center(),
                egui::Align2::CENTER_CENTER,
                fl!("hop-no-screens"),
                egui::FontId::proportional(14.0),
                look::palette(ui).weak,
            );
        }
        if bg.clicked() {
            self.screens.selected = None;
        }
        // The main screen matters (and is marked) only on a PC that maps the pointer onto it alone.
        let mains = [Pc::Pc1, Pc::Pc2].map(|pc| {
            let only_main = mapping[pc.index()] == Mapping::MainScreen;
            only_main.then(|| kvm_core::hop::main_screen(screens, pc)).flatten()
        });
        // Screens off for hop on top (they lie on another PC's screen), drawn see-through.
        let order: Vec<usize> = (0..MAX_SCREENS)
            .filter(|i| on_for_hop(&screens[*i]))
            .chain((0..MAX_SCREENS).filter(|i| screens[*i].pc.is_some() && screens[*i].off))
            .collect();
        let rect_of = |sc: &Screen| {
            let (min, max) = world(sc);
            Rect::from_min_max(view.canvas(min), view.canvas(max)).shrink(1.0)
        };
        // The screens under a point, the top one first.
        let under = |p: Pos2| -> Vec<usize> {
            order.iter().rev().copied().filter(|j| rect_of(&screens[*j]).contains(p)).collect()
        };
        // Interaction first, so that a dragged screen is painted where the pointer is now, not a frame behind.
        let mut released = None;
        for &i in &order {
            let sc = &screens[i];
            let r = rect_of(sc);
            let resp = ui.interact(r, ui.scope_id().with(("hop-screen", i)), Sense::click_and_drag());
            // A click picks the top screen under it, or the one under the screen picked last: where screens overlap,
            // clicking again reaches each of them.
            if resp.clicked()
                && let Some(p) = resp.interact_pointer_pos()
            {
                let here = under(p);
                let next = match self.screens.selected.and_then(|s| here.iter().position(|j| *j == s)) {
                    Some(k) => here[(k + 1) % here.len()],
                    None => i,
                };
                self.screens.selected = Some(next);
            }
            // Grabbed where the button went down, not where the movement became a drag: the point under the pointer
            // stays under it. The screen picked is moved if it is under that point, else the top one.
            if resp.drag_started()
                && let Some(p) = ui.input(|inp| inp.pointer.press_origin()).or(resp.interact_pointer_pos())
            {
                let slot = self.screens.selected.filter(|s| under(p).contains(s)).unwrap_or(i);
                self.screens.selected = Some(slot);
                self.screens.drag = Some(Drag {
                    slot,
                    widget: i,
                    from: screens[slot],
                    screens: *screens,
                    view,
                    height: canvas.height(),
                    grab: view.world(p) - world(&screens[slot]).0,
                });
            }
            if resp.dragged()
                && let (Some(d), Some(p)) = (self.screens.drag.as_mut(), resp.interact_pointer_pos())
                && d.widget == i
            {
                let want = d.view.world(p) - d.grab;
                let at = snap(&d.screens, d.slot, want, SNAP / d.view.scale);
                d.screens[d.slot].at = at;
            }
            if resp.drag_stopped()
                && let Some(d) = self.screens.drag.take()
            {
                // Dropped where it was picked up: nothing to send.
                if d.screens[d.slot] != d.from {
                    let mut before = d.screens;
                    before[d.slot] = d.from;
                    released = Some((before, d.screens, d.from.pc));
                }
            }
            resp.on_hover_text(fl!("hop-screen-hover"));
        }
        let mut paint = self.screens.drag.as_ref().map_or(*screens, |d| d.screens);
        if let Some((_, next, pc)) = released {
            let pcs: Vec<Pc> = pc.into_iter().collect();
            self.arrange(hop, next, &pcs);
            paint = self.screens.current(hop).screens;
        }
        #[cfg(test)]
        self.screens.drawn.clear();
        let paint_order: Vec<usize> = (0..MAX_SCREENS)
            .filter(|i| on_for_hop(&paint[*i]))
            .chain((0..MAX_SCREENS).filter(|i| paint[*i].pc.is_some() && paint[*i].off))
            .collect();
        for i in paint_order {
            let sc = &paint[i];
            let Some(pc) = sc.pc else { continue };
            let (min, max) = world(sc);
            let r = Rect::from_min_max(view.canvas(min), view.canvas(max)).shrink(1.0);
            #[cfg(test)]
            self.screens.drawn.push((i, r, *sc));
            let c = colors[pc.index()];
            let selected = self.screens.selected == Some(i);
            let stroke =
                if selected { Stroke::new(2.5, ui.visuals().strong_text_color()) } else { Stroke::new(1.5, c) };
            if sc.off {
                // Lying on another PC's screen: that one shows through, this one is a dashed outline a little inside
                // it, so that both outlines show.
                painter.rect_filled(r, CornerRadius::same(4), c.gamma_multiply(0.08));
                let d = r.shrink(3.0);
                let corners = [d.left_top(), d.right_top(), d.right_bottom(), d.left_bottom(), d.left_top()];
                let dashes = if selected { stroke } else { Stroke::new(2.0, c) };
                painter.extend(egui::Shape::dashed_line(&corners, dashes, 6.0, 4.0));
            } else {
                painter.rect_filled(r, CornerRadius::same(4), c.gamma_multiply(if dark { 0.30 } else { 0.22 }));
                painter.rect_stroke(r, CornerRadius::same(4), stroke, StrokeKind::Inside);
            }
            let mut text = format!("{}\n{}×{}", pc.name().to_uppercase(), sc.w, sc.h);
            if sc.scale != 100 {
                text += &format!(" · {}%", sc.scale);
            }
            if sc.off {
                // One line along its bottom edge, clear of the label of the screen it lies on.
                text = format!("{} · {}", pc.name().to_uppercase(), fl!("hop-screen-off-short"));
            }
            if mains[pc.index()] == Some(i) {
                text += &format!("\n{}", fl!("hop-main-screen"));
            }
            let (anchor, align) = if sc.off {
                (r.center_bottom() - Vec2::new(0.0, 6.0), egui::Align2::CENTER_BOTTOM)
            } else {
                (r.center(), egui::Align2::CENTER_CENTER)
            };
            painter.text(
                anchor,
                align,
                text,
                egui::FontId::proportional(if sc.off {
                    (r.height() / 8.0).clamp(8.0, 11.0)
                } else {
                    (r.height() / 6.0).clamp(9.0, 14.0)
                }),
                ui.visuals().strong_text_color(),
            );
        }
        if s.status.hop == HopState::Active && self.screens.drag.is_none() {
            let p = view.canvas(Vec2::new(s.status.pointer[0] as f32, s.status.pointer[1] as f32));
            painter.circle_filled(p, 4.0, ui.visuals().strong_text_color());
            painter.circle_stroke(p, 6.0, Stroke::new(1.0, ui.visuals().strong_text_color()));
        }
    }

    fn screen_editor(&mut self, ui: &mut Ui, hop: &HopView, screens: &[Screen; MAX_SCREENS], i: usize) {
        let before = screens[i];
        let mut sc = before;
        egui::Grid::new("hop-screen").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(fl!("hop-screen-pc"));
            ui.horizontal(|ui| {
                for pc in [Pc::Pc1, Pc::Pc2] {
                    if ui.radio(sc.pc == Some(pc), pc.name().to_uppercase()).clicked() {
                        sc.pc = Some(pc);
                    }
                }
            });
            ui.end_row();
            ui.label(fl!("hop-screen-resolution"));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("hop-resolution")
                    .selected_text(format!("{} × {}", sc.w, sc.h))
                    .width(130.0)
                    .show_ui(ui, |ui| {
                        for (w, h) in RESOLUTIONS {
                            if ui.selectable_label((sc.w, sc.h) == (*w, *h), format!("{w} × {h}")).clicked() {
                                (sc.w, sc.h) = (*w, *h);
                            }
                        }
                    });
                let range = *SCREEN_PX.start()..=*SCREEN_PX.end();
                ui.add(egui::DragValue::new(&mut sc.w).range(range.clone()).speed(4.0));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut sc.h).range(range).speed(4.0));
            });
            ui.end_row();
            ui.label(fl!("hop-screen-scale"));
            egui::ComboBox::from_id_salt("hop-scale").selected_text(format!("{} %", sc.scale)).width(90.0).show_ui(
                ui,
                |ui| {
                    for k in SCALES {
                        ui.selectable_value(&mut sc.scale, *k, format!("{k} %"));
                    }
                },
            );
            ui.end_row();
            ui.label(fl!("hop-screen-desk"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut sc.desk[0]).prefix("x ").speed(4.0));
                ui.add(egui::DragValue::new(&mut sc.desk[1]).prefix("y ").speed(4.0));
            });
            ui.end_row();
            ui.label(fl!("hop-screen-at"));
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut sc.at[0]).prefix("x ").speed(4.0).range(-30_000..=30_000));
                ui.add(egui::DragValue::new(&mut sc.at[1]).prefix("y ").speed(4.0).range(-30_000..=30_000));
            });
            ui.end_row();
            ui.label("");
            let mut used = !sc.off;
            ui.checkbox(&mut used, fl!("hop-screen-used"));
            sc.off = !used;
            ui.end_row();
        });
        note(ui, fl!("hop-screen-desk-note"));
        note(ui, fl!("hop-screen-used-note"));
        // Into the draft at once. A change to anything but the place on its computer also gives the PC's screens their
        // places there from the arrangement again where they aren't fixed (a wider main screen moves the one right of
        // it); a place typed in is fixed.
        if sc != before && sc.is_valid() {
            if sc.desk != before.desk {
                self.screens.desk_typed = true;
                sc.fixed = true;
            }
            self.edit_screen(hop, screens, i, sc, before.pc);
        }
    }

    /// This computer's screens into the draft (`odkvmctl::screens::merge`), with its mapping and units: applied with
    /// the rest of the arrangement.
    fn detect(&mut self, hop: &HopView, pc: Pc) {
        match odkvmctl::screens::detect() {
            Ok(found) => {
                self.edit(hop, |a| {
                    a.screens = odkvmctl::screens::merge(&a.screens, pc, &found);
                    a.mapping[pc.index()] = odkvmctl::screens::MAPPING;
                    a.units[pc.index()] = found.units;
                });
                self.screens.detect_note = Some(Ok(fl!(
                    "hop-detected",
                    count = found.screens.len(),
                    pc = pc.name().to_uppercase(),
                    mapping = names::mapping(odkvmctl::screens::MAPPING)
                )));
                self.screens.now = Some((Instant::now(), Ok(found)));
            }
            Err(e) => self.screens.detect_note = Some(Err(e)),
        }
    }

    /// Whether the arrangement as shown (the draft, or the KVM's) has this computer's screens (their resolution, scaling
    /// and place on it) and units as its system reports them now, read again every [`RECHECK`]; a warning if not (the
    /// Detect button puts it right).
    fn freshness(&mut self, ui: &mut Ui, cur: &Arrangement, hop: &HopView, pc: Pc) {
        let screens = &cur.screens;
        // The simulator's screens aren't this computer's.
        if !screens.iter().any(|s| s.pc == Some(pc)) || hop.simulated {
            return;
        }
        if self.screens.now.as_ref().is_none_or(|(at, _)| at.elapsed() > RECHECK) {
            self.screens.now = Some((Instant::now(), odkvmctl::screens::detect()));
        }
        let Some((_, Ok(now))) = &self.screens.now else { return };
        if odkvmctl::screens::differs(screens, pc, cur.units[pc.index()], now) {
            ui.label(RichText::new(fl!("hop-out-of-date")).color(ink(ui, WARN)));
        }
    }

    fn pointer_card(&mut self, ui: &mut Ui, hop: &HopView) {
        card(ui, &fl!("hop-pointer"), |ui| {
            egui::Grid::new("hop-pointer").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label(fl!("hop-accel"));
                let mut a = hop.accel;
                egui::ComboBox::from_id_salt("hop-accel").selected_text(names::accel(a)).width(260.0).show_ui(
                    ui,
                    |ui| {
                        for x in Accel::ALL {
                            ui.selectable_value(&mut a, *x, names::accel(*x));
                        }
                    },
                );
                if a != hop.accel {
                    self.set(ConfigValue::HopAccel(a));
                }
                ui.end_row();
                ui.label(fl!("hop-speed"));
                let mut v = hop.speed;
                let r = ui.add(egui::Slider::new(&mut v, *HOP_SPEED.start()..=*HOP_SPEED.end()));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.set(ConfigValue::HopSpeed(v));
                } else if r.changed() {
                    crate::optimistic(&self.shared, ConfigValue::HopSpeed(v));
                }
                ui.end_row();
                ui.label(fl!("hop-resistance"));
                let mut v = hop.resistance.min(RESISTANCE_SLIDER);
                let r = ui.add(egui::Slider::new(&mut v, 0..=RESISTANCE_SLIDER).suffix(" px"));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.set(ConfigValue::HopResistance(v));
                } else if r.changed() {
                    crate::optimistic(&self.shared, ConfigValue::HopResistance(v));
                }
                ui.end_row();
                ui.label(fl!("hop-corner"));
                let mut v = hop.corner;
                let r = ui.add(egui::Slider::new(&mut v, 0..=HOP_CORNER_MAX.min(200)).suffix(" px"));
                if r.drag_stopped() || (r.changed() && !r.dragged()) {
                    self.set(ConfigValue::HopCorner(v));
                } else if r.changed() {
                    crate::optimistic(&self.shared, ConfigValue::HopCorner(v));
                }
                ui.end_row();
            });
            note(ui, fl!("hop-pointer-note"));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dragged_screen_snaps_to_its_neighbours_edges() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Pc::Pc1, 1920, 1080, [0, 0]);
        l[1] = Screen::new(Pc::Pc2, 2560, 1440, [3000, 500]);
        // Dropped 30 right of PC1's right edge and 20 below its top: snapped to touch it, tops aligned.
        assert_eq!(snap(&l, 1, Vec2::new(1950.0, 20.0), 40.0), [1920, 0]);
        // Far from everything: where it was dropped.
        assert_eq!(snap(&l, 1, Vec2::new(5000.0, 3000.0), 40.0), [5000, 3000]);
        // Its bottom to PC1's bottom.
        assert_eq!(snap(&l, 1, Vec2::new(1920.0, -350.0), 40.0), [1920, -360]);
    }

    #[test]
    fn new_and_detected_screens_go_right_of_the_others() {
        let mut l = [Screen::NONE; MAX_SCREENS];
        l[0] = Screen::new(Pc::Pc1, 1920, 1080, [0, 0]);
        let n = new_screen(&l, Pc::Pc1, Units::Physical, 2560, 1440);
        assert_eq!((n.at, n.desk), ([1920, 0], [1920, 0]));
        let n = new_screen(&l, Pc::Pc2, Units::Physical, 1920, 1080);
        assert_eq!((n.at, n.desk), ([1920, 0], [0, 0]));
    }
}
