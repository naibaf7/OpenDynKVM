//! The Screens page (screen hop, software.md §4.14 and §9): whether screen hop runs and what it needs, which profiles
//! use it, the arrangement of both PCs' screens (dragged into place on a canvas, each screen's resolution, scaling and
//! place on its own PC), and how the pointer moves.
//!
//! The arrangement is the KVM's: every change is sent as the screen slot it changes. While a screen is dragged the
//! page shows its own copy, and sends the slot when the drag ends. A screen sent stays shown as sent until the KVM's
//! copy has it too: a refresh read just before the change would otherwise put it back for a moment.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use kvm_proto::ctrl::{
    Accel, ConfigValue, HOP_CORNER_MAX, HOP_SPEED, HopState, MAX_SCREENS, Mapping, Pc, Port, PortMode, SCREEN_PX,
    Screen,
};
use odkvmctl::client::{HopView, Snapshot};
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

/// How long a screen sent stays shown as sent while the KVM's copy doesn't have it yet.
const HOLD: Duration = Duration::from_secs(3);

/// What the page keeps between frames.
#[derive(Default)]
pub struct Editor {
    selected: Option<usize>,
    /// The screen being dragged: its slot, the arrangement as it is being changed, the canvas's mapping frozen at the
    /// start of the drag, and where in the screen it was grabbed (arrangement pixels).
    drag: Option<Drag>,
    /// How detecting this computer's screens went.
    detect_note: Option<Result<String, String>>,
    /// The screen whose numbers are being dragged: sent once the drag ends (shown from `held` meanwhile).
    pending: Option<usize>,
    /// The edit being sent typed a place on its computer: keep it, don't derive it from the arrangement.
    desk_typed: bool,
    /// Screens sent, or being edited, that the KVM's copy may not show yet, and since when.
    held: [Option<(Screen, Instant)>; MAX_SCREENS],
    /// Each screen as drawn last, and where (the tests drag them).
    #[cfg(test)]
    drawn: Vec<(usize, Rect, Screen)>,
}

impl Editor {
    /// The arrangement as shown: the KVM's, with what the page sent in its place until the KVM has it too.
    fn shown(&mut self, kvm: &[Screen; MAX_SCREENS]) -> [Screen; MAX_SCREENS] {
        let mut out = *kvm;
        for (i, h) in self.held.iter_mut().enumerate() {
            match *h {
                Some((sc, _)) if sc == kvm[i] => *h = None,
                Some((_, since)) if since.elapsed() > HOLD && self.pending != Some(i) => *h = None,
                Some((sc, _)) => out[i] = sc,
                None => {}
            }
        }
        out
    }
}

#[cfg(test)]
impl Editor {
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
    slot: usize,
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

/// Where a new screen of `pc` goes: right of everything in the arrangement, right of the PC's own in its arrangement.
fn new_screen(screens: &[Screen; MAX_SCREENS], pc: Pc, w: u16, h: u16) -> Screen {
    let right = screens.iter().filter(|s| s.pc.is_some()).map(|s| world(s).1.x as i32).max().unwrap_or(0);
    let top = screens.iter().filter(|s| s.pc.is_some()).map(|s| s.at[1] as i32).min().unwrap_or(0);
    let desk_right = screens.iter().filter(|s| s.pc == Some(pc)).map(|s| s.desk[0] as i32 + s.w as i32).max();
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
    /// Sends screen slot `slot`, and shows it as sent until the KVM's copy has it.
    fn send_screen(&mut self, slot: usize, sc: Screen) {
        self.set(ConfigValue::Screen(slot as u8, sc));
        self.screens.held[slot] = Some((sc, Instant::now()));
    }

    /// Sends every slot that differs between `before` and `next`, after giving `pcs`' screens their place on their own
    /// computer from the arrangement (`odkvmctl::screens::desks`): where a screen is dragged is where it is on its PC.
    fn send_arranged(&mut self, before: &[Screen; MAX_SCREENS], mut next: [Screen; MAX_SCREENS], pcs: &[Pc]) {
        for pc in pcs {
            next = odkvmctl::screens::desks(&next, *pc);
        }
        for (i, sc) in next.iter().enumerate() {
            if *sc != before[i] {
                self.send_screen(i, *sc);
            }
        }
    }

    /// Sends screen `slot` as edited (`was`: the PC it belonged to), with the places on their computers derived again
    /// unless the edit typed one in.
    fn send_edited(&mut self, screens: &[Screen; MAX_SCREENS], slot: usize, sc: Screen, was: Option<Pc>) {
        let mut next = *screens;
        next[slot] = sc;
        if core::mem::take(&mut self.screens.desk_typed) {
            self.send_screen(slot, sc);
            return;
        }
        let pcs: Vec<Pc> = [was, sc.pc].into_iter().flatten().collect();
        self.send_arranged(screens, next, &pcs);
    }

    pub(crate) fn screens(&mut self, ui: &mut Ui, s: &Snapshot) {
        let Some(hop) = s.hop() else {
            card(ui, "", |ui| note(ui, fl!("hop-firmware-too-old")));
            return;
        };
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
        let shown = self.screens.shown(&hop.screens);
        let screens = self.screens.drag.as_ref().map_or(shown, |d| d.screens);
        card(ui, &fl!("hop-arrangement"), |ui| {
            note(ui, fl!("hop-arrangement-note"));
            self.canvas(ui, s, &screens, hop.mapping, colors);
            let overlap = (0..MAX_SCREENS).any(|i| {
                (i + 1..MAX_SCREENS)
                    .any(|j| screens[i].pc.is_some() && screens[j].pc.is_some() && overlaps(&screens[i], &screens[j]))
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
                        next[slot] = new_screen(&screens, pc, 1920, 1080);
                        self.send_arranged(&screens, next, &[pc]);
                        self.screens.selected = Some(slot);
                    }
                }
                if let Some(pc) = s.this_pc() {
                    let label = fl!("hop-detect", pc = pc.name().to_uppercase());
                    if ui.button(label).on_hover_text(fl!("hop-detect-hover")).clicked() {
                        self.detect(&screens, pc);
                    }
                }
                if let Some(i) = self.screens.selected.filter(|i| screens[*i].pc.is_some())
                    && ui.button(fl!("hop-remove-screen")).clicked()
                {
                    let mut next = screens;
                    next[i] = Screen::NONE;
                    let pcs: Vec<Pc> = screens[i].pc.into_iter().collect();
                    self.send_arranged(&screens, next, &pcs);
                    self.screens.selected = None;
                }
            });
            match &self.screens.detect_note {
                Some(Ok(t)) => note(ui, t.clone()),
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(ink(ui, BAD)));
                }
                None => {}
            }
            if let Some(i) = self.screens.selected.filter(|i| screens[*i].pc.is_some()) {
                ui.separator();
                self.screen_editor(ui, &screens, i);
            }
            ui.separator();
            egui::Grid::new("hop-mapping").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for pc in [Pc::Pc1, Pc::Pc2] {
                    ui.label(fl!("hop-mapping-of", pc = pc.name().to_uppercase()));
                    let mut m = hop.mapping[pc.index()];
                    egui::ComboBox::from_id_salt(("hop-mapping", pc.index()))
                        .selected_text(names::mapping(m))
                        .width(260.0)
                        .show_ui(ui, |ui| {
                            for x in Mapping::ALL {
                                ui.selectable_value(&mut m, *x, names::mapping(*x));
                            }
                        });
                    if m != hop.mapping[pc.index()] {
                        self.set(ConfigValue::HopMapping(pc, m));
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
        // Interaction first, so that a dragged screen is painted where the pointer is now, not a frame behind.
        let mut released = None;
        for (i, sc) in screens.iter().enumerate() {
            if sc.pc.is_none() {
                continue;
            }
            let (min, max) = world(sc);
            let r = Rect::from_min_max(view.canvas(min), view.canvas(max)).shrink(1.0);
            let resp = ui.interact(r, ui.scope_id().with(("hop-screen", i)), Sense::click_and_drag());
            if resp.clicked() || resp.drag_started() {
                self.screens.selected = Some(i);
            }
            // Grabbed where the button went down, not where the movement became a drag: the point under the pointer
            // stays under it.
            if resp.drag_started()
                && let Some(p) = ui.input(|inp| inp.pointer.press_origin()).or(resp.interact_pointer_pos())
            {
                self.screens.drag = Some(Drag {
                    slot: i,
                    from: *sc,
                    screens: *screens,
                    view,
                    height: canvas.height(),
                    grab: view.world(p) - min,
                });
            }
            if resp.dragged()
                && let (Some(d), Some(p)) = (self.screens.drag.as_mut(), resp.interact_pointer_pos())
                && d.slot == i
            {
                let want = d.view.world(p) - d.grab;
                let at = snap(&d.screens, i, want, SNAP / d.view.scale);
                d.screens[i].at = at;
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
        if let Some((before, next, pc)) = released {
            let pcs: Vec<Pc> = pc.into_iter().collect();
            self.send_arranged(&before, next, &pcs);
            paint = self.screens.shown(&before);
        }
        #[cfg(test)]
        self.screens.drawn.clear();
        for (i, sc) in paint.iter().enumerate() {
            let Some(pc) = sc.pc else { continue };
            let (min, max) = world(sc);
            let r = Rect::from_min_max(view.canvas(min), view.canvas(max)).shrink(1.0);
            #[cfg(test)]
            self.screens.drawn.push((i, r, *sc));
            let c = colors[pc.index()];
            let selected = self.screens.selected == Some(i);
            painter.rect_filled(r, CornerRadius::same(4), c.gamma_multiply(if dark { 0.30 } else { 0.22 }));
            let stroke =
                if selected { Stroke::new(2.5, ui.visuals().strong_text_color()) } else { Stroke::new(1.5, c) };
            painter.rect_stroke(r, CornerRadius::same(4), stroke, StrokeKind::Inside);
            let mut text = format!("{}\n{}×{}", pc.name().to_uppercase(), sc.w, sc.h);
            if sc.scale != 100 {
                text += &format!(" · {}%", sc.scale);
            }
            if mains[pc.index()] == Some(i) {
                text += &format!("\n{}", fl!("hop-main-screen"));
            }
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional((r.height() / 6.0).clamp(9.0, 14.0)),
                ui.visuals().strong_text_color(),
            );
        }
        if s.status.hop == HopState::Active && self.screens.drag.is_none() {
            let p = view.canvas(Vec2::new(s.status.pointer[0] as f32, s.status.pointer[1] as f32));
            painter.circle_filled(p, 4.0, ui.visuals().strong_text_color());
            painter.circle_stroke(p, 6.0, Stroke::new(1.0, ui.visuals().strong_text_color()));
        }
    }

    fn screen_editor(&mut self, ui: &mut Ui, screens: &[Screen; MAX_SCREENS], i: usize) {
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
        });
        note(ui, fl!("hop-screen-desk-note"));
        // Sent once the value settles: a drag on a number sends where it stops, and is shown meanwhile. A change to
        // anything but the place on its computer also gives the PC's screens their places there from the arrangement
        // again (a wider main screen moves the one right of it); a place typed in stays until the arrangement changes.
        let dragging = ui.ctx().input(|inp| inp.pointer.any_down());
        if sc != before && sc.is_valid() {
            if sc.desk != before.desk {
                self.screens.desk_typed = true;
            }
            if dragging {
                self.screens.held[i] = Some((sc, Instant::now()));
                self.screens.pending = Some(i);
            } else {
                self.send_edited(screens, i, sc, before.pc);
            }
        }
        if !dragging
            && let Some(slot) = self.screens.pending.take()
            && let Some((held, _)) = self.screens.held[slot]
        {
            self.send_edited(screens, slot, held, screens[slot].pc);
        }
    }

    fn detect(&mut self, screens: &[Screen; MAX_SCREENS], pc: Pc) {
        match odkvmctl::screens::detect() {
            Ok(found) => {
                let next = odkvmctl::screens::place(screens, pc, &found);
                for (i, (a, b)) in screens.iter().zip(next.iter()).enumerate() {
                    if a != b {
                        self.send_screen(i, *b);
                    }
                }
                self.set(ConfigValue::HopMapping(pc, odkvmctl::screens::MAPPING));
                self.screens.detect_note = Some(Ok(fl!(
                    "hop-detected",
                    count = found.len(),
                    pc = pc.name().to_uppercase(),
                    mapping = names::mapping(odkvmctl::screens::MAPPING)
                )));
            }
            Err(e) => self.screens.detect_note = Some(Err(e)),
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
        let n = new_screen(&l, Pc::Pc1, 2560, 1440);
        assert_eq!((n.at, n.desk), ([1920, 0], [1920, 0]));
        let n = new_screen(&l, Pc::Pc2, 1920, 1080);
        assert_eq!((n.at, n.desk), ([1920, 0], [0, 0]));
    }
}
