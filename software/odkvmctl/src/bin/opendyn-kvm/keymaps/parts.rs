//! The Keymaps page's other sections (hid_mapping.md §5, §6, §9): what every keymap shares (macros, tap dances, combos,
//! key overrides, alternate repeat keys, the QMK settings), the monitor with its field overrides, and the hosts (what
//! each takes, its keyboard layout, each profile's fallback keymap).
//!
//! Keycodes are typed as `odkvmctl keymap` takes them (a field turns red while it doesn't read as one) and go to the KVM
//! when the field is left. Every change is a configuration change, as from the CLI.

use eframe::egui::{self, RichText, Sense, Stroke, Ui, Vec2};
use kvm_hid::layouts::Layout;
use kvm_map::keymap::{Dynamic, MACRO_BUFFER, MACRO_COUNT, arep, ko};
use kvm_proto::ctrl::{ConfigValue, Device, FieldOverride, Host, MAX_OVERRIDES, PortState, field, outputs};
use odkvmctl::client::{DeviceSlot, Snapshot};
use odkvmctl::keycodes::{self, MacroStep};
use odkvmctl::{fl, names};

use crate::look::{BAD, GOOD, NEUTRAL, WARN, card, chip, ink, note, palette};
use crate::worker::{MapParts, MonitorView, UiCmd};
use crate::{App, config};

/// The page's sections: the layers editor, then what the keymaps share, then the devices' and the hosts' side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Layers,
    Macros,
    Dances,
    Combos,
    Overrides,
    Repeats,
    Settings,
    Monitor,
    Hosts,
}

impl Section {
    pub const ALL: [Section; 9] = [
        Section::Layers,
        Section::Macros,
        Section::Dances,
        Section::Combos,
        Section::Overrides,
        Section::Repeats,
        Section::Settings,
        Section::Monitor,
        Section::Hosts,
    ];

    pub fn title(self) -> String {
        match self {
            Section::Layers => fl!("map-sec-layers"),
            Section::Macros => fl!("map-sec-macros"),
            Section::Dances => fl!("map-sec-dances"),
            Section::Combos => fl!("map-sec-combos"),
            Section::Overrides => fl!("map-sec-overrides"),
            Section::Repeats => fl!("map-sec-repeats"),
            Section::Settings => fl!("map-sec-settings"),
            Section::Monitor => fl!("map-sec-monitor"),
            Section::Hosts => fl!("map-sec-hosts"),
        }
    }

    /// The sections that show the keymaps' shared parts.
    fn uses_parts(self) -> bool {
        !matches!(self, Section::Layers | Section::Monitor | Section::Hosts)
    }
}

/// The monitor's selection: a run of bits of one report, for a field override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    iface: u8,
    report_id: u8,
    /// First and last bit, counted from the first byte after the report ID.
    first: u16,
    last: u16,
}

/// What the sections keep between frames.
#[derive(Default)]
pub struct PartsState {
    pub section: Section,
    asked_parts: bool,
    asked_outputs: Option<f64>,
    monitor_device: Option<Device>,
    monitored_at: f64,
    selection: Option<Selection>,
    target: u8,
    signed: bool,
    invert: bool,
}

impl PartsState {
    #[cfg(test)]
    pub fn show_monitor(&mut self, d: Option<Device>) {
        self.monitor_device = d;
    }

    /// Leaving the page: the device monitored, which stops; the shared parts are read anew next time.
    pub fn leave(&mut self) -> Option<Device> {
        self.asked_parts = false;
        self.asked_outputs = None;
        self.selection = None;
        self.monitor_device.take()
    }
}

/// A keycode in a text field: its name, taken when the field is left; red while it doesn't read as a keycode.
fn keycode_field(ui: &mut Ui, id: egui::Id, kc: u16, width: f32) -> Option<u16> {
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| keycodes::keycode_name(kc));
    let parsed = keycodes::parse_keycode(&text);
    let mut edit = egui::TextEdit::singleline(&mut text).id(id);
    if parsed.is_err() {
        edit = edit.text_color(ink(ui, BAD));
    }
    let r = ui.add_sized(Vec2::new(width, 20.0), edit);
    let r = match &parsed {
        Err(e) => r.on_hover_text(e),
        Ok(_) => r,
    };
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    }
    if r.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        if let Ok(k) = keycodes::parse_keycode(&text)
            && k != kc
        {
            return Some(k);
        }
    }
    None
}

/// Modifiers in a text field (`ctrl+shift`, `none`), taken when the field is left.
fn mods_field(ui: &mut Ui, id: egui::Id, m: u8) -> Option<u8> {
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| keycodes::mods_name_or_none(m));
    let ok = keycodes::parse_mod_names(&text).is_ok();
    let mut edit = egui::TextEdit::singleline(&mut text).id(id);
    if !ok {
        edit = edit.text_color(ink(ui, BAD));
    }
    let r = ui.add_sized(Vec2::new(90.0, 20.0), edit);
    if r.changed() {
        ui.data_mut(|d| d.insert_temp(id, text.clone()));
    }
    if r.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        if let Ok(v) = keycodes::parse_mod_names(&text)
            && v != m
        {
            return Some(v);
        }
    }
    None
}

/// The last cell of a row: × clears an entry in use (the free row has none).
fn clear_cell(ui: &mut Ui, used: bool) -> bool {
    if !used {
        ui.label("");
        return false;
    }
    ui.small_button("×").on_hover_text(fl!("map-clear")).clicked()
}

fn le(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
}

fn put(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// The entries of a kind to show: those in use, then the first free one to add to.
fn shown(entries: &[Vec<u8>], used: impl Fn(&[u8]) -> bool) -> Vec<usize> {
    let mut list: Vec<usize> = (0..entries.len()).filter(|i| used(&entries[*i])).collect();
    if let Some(free) = (0..entries.len()).find(|i| !used(&entries[*i])) {
        list.push(free);
    }
    list
}

impl App {
    /// The section bar, under the page's first card.
    pub(crate) fn map_sections(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            for sec in Section::ALL {
                ui.selectable_value(&mut self.keymaps.parts.section, sec, sec.title());
            }
        });
    }

    /// Every section but the layers.
    pub(crate) fn map_part(&mut self, ui: &mut Ui, s: &Snapshot) {
        let sec = self.keymaps.parts.section;
        let parts = self.shared.lock().unwrap().parts.clone();
        if sec.uses_parts() && !self.keymaps.parts.asked_parts {
            self.keymaps.parts.asked_parts = true;
            self.send(UiCmd::LoadParts);
        }
        if sec != Section::Monitor {
            self.stop_monitor();
        }
        match sec {
            Section::Layers => {}
            Section::Monitor => self.map_monitor(ui, s),
            Section::Hosts => self.map_hosts(ui, s),
            _ => {
                let Some(p) = parts else {
                    ui.spinner();
                    return;
                };
                match sec {
                    Section::Macros => card(ui, &fl!("map-sec-macros"), |ui| self.map_macros(ui, &p)),
                    Section::Dances => card(ui, &fl!("map-sec-dances"), |ui| self.map_dances(ui, &p)),
                    Section::Combos => card(ui, &fl!("map-sec-combos"), |ui| self.map_combos(ui, &p)),
                    Section::Overrides => card(ui, &fl!("map-sec-overrides"), |ui| self.map_key_overrides(ui, &p)),
                    Section::Repeats => card(ui, &fl!("map-sec-repeats"), |ui| self.map_repeats(ui, &p)),
                    _ => card(ui, &fl!("map-sec-settings"), |ui| self.map_settings(ui, &p)),
                }
            }
        }
    }

    /// Leaving the page or the monitor: the KVM stops streaming.
    pub(crate) fn stop_monitor(&mut self) {
        if let Some(d) = self.keymaps.parts.monitor_device.take() {
            self.send(UiCmd::Monitor(d, false));
        }
    }

    fn map_macros(&mut self, ui: &mut Ui, p: &MapParts) {
        note(ui, fl!("map-macros-note"));
        egui::Grid::new("macros").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for i in 0..MACRO_COUNT as usize {
                let m = kvm_map::macros::get(&p.macros, i);
                let words: Vec<String> = MacroStep::read(m).iter().map(MacroStep::name).collect();
                let shown = words.join("\n");
                let id = ui.scope_id().with(("macro", i));
                ui.label(RichText::new(format!("macro({i})")).monospace());
                ui.vertical(|ui| {
                    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| shown.clone());
                    let edit = egui::TextEdit::multiline(&mut text)
                        .desired_rows(1)
                        .desired_width(360.0)
                        .hint_text("text:hello\ntap:enter\ndelay:100");
                    if ui.add(edit).changed() {
                        ui.data_mut(|d| d.insert_temp(id, text.clone()));
                    }
                    if text == shown {
                        return;
                    }
                    let parsed: Result<Vec<MacroStep>, String> =
                        text.lines().filter(|l| !l.trim().is_empty()).map(MacroStep::parse).collect();
                    ui.horizontal(|ui| {
                        match &parsed {
                            Err(e) => {
                                ui.label(RichText::new(e).color(ink(ui, BAD)));
                            }
                            Ok(steps) => {
                                let bytes = MacroStep::write(steps);
                                let after = bytes.as_ref().and_then(|b| {
                                    kvm_map::macros::replace::<MACRO_BUFFER>(&p.macros, MACRO_COUNT as usize, i, b)
                                });
                                match after {
                                    None => {
                                        ui.label(RichText::new(fl!("map-macro-too-long")).color(ink(ui, BAD)));
                                    }
                                    Some(after) => {
                                        if ui.button(fl!("apply")).clicked() {
                                            self.send(UiCmd::SetMacros(p.macros.clone(), after.to_vec()));
                                            ui.data_mut(|d| d.remove::<String>(id));
                                        }
                                    }
                                }
                            }
                        }
                        if ui.button(fl!("map-discard")).clicked() {
                            ui.data_mut(|d| d.remove::<String>(id));
                        }
                    });
                });
                ui.end_row();
            }
        });
    }

    fn map_dances(&mut self, ui: &mut Ui, p: &MapParts) {
        note(ui, fl!("map-dances-note"));
        let list = &p.entries[Dynamic::TapDance as usize];
        egui::Grid::new("dances").num_columns(7).spacing([8.0, 4.0]).show(ui, |ui| {
            for h in [
                "",
                &fl!("map-dance-tap"),
                &fl!("map-dance-hold"),
                &fl!("map-dance-double"),
                &fl!("map-dance-tap-hold"),
            ] {
                ui.label(RichText::new(h).strong());
            }
            ui.label(RichText::new(fl!("map-term")).strong());
            ui.label("");
            ui.end_row();
            for i in shown(list, |b| b.iter().any(|x| *x != 0)) {
                let b = &list[i];
                ui.label(RichText::new(format!("td({i})")).monospace());
                let mut new = b.clone();
                let mut changed = false;
                for k in 0..4 {
                    let id = ui.scope_id().with(("dance", i, k));
                    if let Some(kc) = keycode_field(ui, id, le(b, 2 * k), 110.0) {
                        put(&mut new, 2 * k, kc);
                        changed = true;
                    }
                }
                let mut term = le(b, 8);
                if ui.add(egui::DragValue::new(&mut term).range(0..=1000).suffix(" ms")).changed() {
                    put(&mut new, 8, term);
                    changed = true;
                }
                if clear_cell(ui, b.iter().any(|x| *x != 0)) {
                    new.fill(0);
                    changed = true;
                }
                if changed {
                    self.send(UiCmd::SetEntry(Dynamic::TapDance, i as u8, new));
                }
                ui.end_row();
            }
        });
    }

    fn map_combos(&mut self, ui: &mut Ui, p: &MapParts) {
        note(ui, fl!("map-combos-note"));
        let list = &p.entries[Dynamic::Combo as usize];
        // A column per key, so that every row's fields line up under the headers.
        egui::Grid::new("combos").num_columns(7).spacing([8.0, 4.0]).show(ui, |ui| {
            ui.label("");
            ui.label(RichText::new(fl!("map-combo-keys")).strong());
            for _ in 1..4 {
                ui.label("");
            }
            ui.label(RichText::new(fl!("map-combo-output")).strong());
            ui.label("");
            ui.end_row();
            for i in shown(list, |b| le(b, 0) != 0) {
                let b = &list[i];
                ui.label(RichText::new(format!("{i}")).monospace());
                let mut new = b.clone();
                let mut changed = false;
                for k in 0..4 {
                    let id = ui.scope_id().with(("combo", i, k));
                    if let Some(kc) = keycode_field(ui, id, le(b, 2 * k), 90.0) {
                        put(&mut new, 2 * k, kc);
                        changed = true;
                    }
                }
                if let Some(kc) = keycode_field(ui, ui.scope_id().with(("combo-out", i)), le(b, 8), 110.0) {
                    put(&mut new, 8, kc);
                    changed = true;
                }
                if clear_cell(ui, le(b, 0) != 0) {
                    new.fill(0);
                    changed = true;
                }
                if changed {
                    // A combo's keys end at the first none.
                    let keys: Vec<u16> = (0..4).map(|k| le(&new, 2 * k)).filter(|k| *k != 0).collect();
                    for k in 0..4 {
                        put(&mut new, 2 * k, keys.get(k).copied().unwrap_or(0));
                    }
                    self.send(UiCmd::SetEntry(Dynamic::Combo, i as u8, new));
                }
                ui.end_row();
            }
        });
    }

    fn map_key_overrides(&mut self, ui: &mut Ui, p: &MapParts) {
        note(ui, fl!("map-overrides-note"));
        let list = &p.entries[Dynamic::KeyOverride as usize];
        egui::Grid::new("key-overrides").num_columns(8).spacing([8.0, 4.0]).show(ui, |ui| {
            for h in [
                String::new(),
                fl!("map-ko-mods"),
                fl!("map-ko-trigger"),
                fl!("map-ko-replacement"),
                fl!("map-ko-negative"),
                fl!("map-ko-suppress"),
                fl!("map-entry-on"),
                String::new(),
            ] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for i in shown(list, |b| b.iter().any(|x| *x != 0)) {
                let b = &list[i];
                ui.label(RichText::new(format!("{i}")).monospace());
                let mut new = b.clone();
                let mut changed = false;
                if let Some(m) = mods_field(ui, ui.scope_id().with(("ko-mods", i, 0)), b[6]) {
                    new[6] = m;
                    changed = true;
                }
                for (k, at) in [(0, 0usize), (1, 2)] {
                    if let Some(kc) = keycode_field(ui, ui.scope_id().with(("ko", i, k)), le(b, at), 100.0) {
                        put(&mut new, at, kc);
                        changed = true;
                    }
                }
                for (col, at) in [(1, 7usize), (2, 8)] {
                    if let Some(m) = mods_field(ui, ui.scope_id().with(("ko-mods", i, col)), b[at]) {
                        new[at] = m;
                        changed = true;
                    }
                }
                let mut on = b[9] & ko::ENABLED != 0;
                if ui.checkbox(&mut on, "").changed() {
                    new[9] = if on { new[9] | ko::ENABLED } else { new[9] & !ko::ENABLED };
                    changed = true;
                }
                if clear_cell(ui, b.iter().any(|x| *x != 0)) {
                    new.fill(0);
                    changed = true;
                }
                if changed {
                    // A new entry: on every layer, as Vial makes one.
                    if b.iter().all(|x| *x == 0) {
                        put(&mut new, 4, 0xFFFF);
                        new[9] |= ko::ENABLED | ko::ACTIVATION_TRIGGER_DOWN | ko::ACTIVATION_REQUIRED_MOD_DOWN;
                    }
                    self.send(UiCmd::SetEntry(Dynamic::KeyOverride, i as u8, new));
                }
                ui.end_row();
            }
        });
    }

    fn map_repeats(&mut self, ui: &mut Ui, p: &MapParts) {
        note(ui, fl!("map-repeats-note"));
        let list = &p.entries[Dynamic::AltRepeat as usize];
        egui::Grid::new("repeats").num_columns(6).spacing([8.0, 4.0]).show(ui, |ui| {
            for h in [
                String::new(),
                fl!("map-repeat-after"),
                fl!("map-repeat-alt"),
                fl!("map-repeat-mods"),
                fl!("map-entry-on"),
                String::new(),
            ] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for i in shown(list, |b| b.iter().any(|x| *x != 0)) {
                let b = &list[i];
                ui.label(RichText::new(format!("{i}")).monospace());
                let mut new = b.clone();
                let mut changed = false;
                for (k, at) in [(0, 0usize), (1, 2)] {
                    if let Some(kc) = keycode_field(ui, ui.scope_id().with(("rep", i, k)), le(b, at), 100.0) {
                        put(&mut new, at, kc);
                        changed = true;
                    }
                }
                if let Some(m) = mods_field(ui, ui.scope_id().with(("rep-mods", i)), b[4]) {
                    new[4] = m;
                    changed = true;
                }
                let mut on = b[5] & arep::ENABLED != 0;
                if ui.checkbox(&mut on, "").changed() {
                    new[5] = if on { new[5] | arep::ENABLED } else { new[5] & !arep::ENABLED };
                    changed = true;
                }
                if clear_cell(ui, b.iter().any(|x| *x != 0)) {
                    new.fill(0);
                    changed = true;
                }
                if changed {
                    if b.iter().all(|x| *x == 0) {
                        new[5] |= arep::ENABLED;
                    }
                    self.send(UiCmd::SetEntry(Dynamic::AltRepeat, i as u8, new));
                }
                ui.end_row();
            }
        });
    }

    fn map_settings(&mut self, ui: &mut Ui, p: &MapParts) {
        note(ui, fl!("map-settings-note"));
        egui::Grid::new("map-settings").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
            for (name, q, flag) in keycodes::SETTINGS {
                let Some(v) = p.settings.iter().find(|(k, _)| *k == q).map(|(_, v)| *v) else { continue };
                ui.label(setting_label(name));
                if flag {
                    let mut on = v != 0;
                    if ui.checkbox(&mut on, "").changed() {
                        self.send(UiCmd::SetSetting(q, u32::from(on)));
                    }
                } else {
                    let mut x = v;
                    let r = ui.add(egui::DragValue::new(&mut x).range(0..=10000));
                    if r.drag_stopped() || (r.changed() && !r.dragged()) {
                        self.send(UiCmd::SetSetting(q, x));
                    }
                }
                ui.end_row();
            }
        });
    }

    // ── the monitor ─────────────────────────────────────────────────────────

    fn map_monitor(&mut self, ui: &mut Ui, s: &Snapshot) {
        let view = self.shared.lock().unwrap().monitor.clone();
        card(ui, &fl!("map-sec-monitor"), |ui| {
            note(ui, fl!("map-monitor-note"));
            let devices: Vec<Device> = Device::ALL
                .iter()
                .copied()
                .filter(|d| match (d.port(), d.wireless()) {
                    (Some(p), _) => s.ports[p.index()].state == PortState::Hosted,
                    (None, Some(n)) => s.device(n).is_some_and(|i| i.bonded()),
                    _ => false,
                })
                .collect();
            ui.horizontal(|ui| {
                ui.label(fl!("map-device"));
                let current = self.keymaps.parts.monitor_device.or(view.as_ref().map(|v| v.device));
                let text = current.map_or_else(|| fl!("map-choose"), names::device_title);
                egui::ComboBox::from_id_salt("monitor-device").selected_text(text).show_ui(ui, |ui| {
                    for d in &devices {
                        if ui.selectable_label(current == Some(*d), names::device_title(*d)).clicked() {
                            self.stop_monitor();
                            self.keymaps.parts.monitor_device = Some(*d);
                            self.keymaps.parts.selection = None;
                            self.send(UiCmd::Monitor(*d, true));
                            self.keymaps.parts.monitored_at = ui.input(|i| i.time);
                        }
                    }
                });
                if devices.is_empty() {
                    ui.label(RichText::new(fl!("map-no-device")).color(palette(ui).weak));
                }
            });
            if !self.keymaps.here {
                ui.label(RichText::new(fl!("map-not-active")).color(ink(ui, WARN)));
            }
        });
        // Asked again while it shows: the KVM ends a stream nobody renews.
        let now = ui.input(|i| i.time);
        if let Some(d) = self.keymaps.parts.monitor_device
            && now - self.keymaps.parts.monitored_at >= super::WATCH_RENEW_S
        {
            self.keymaps.parts.monitored_at = now;
            self.send(UiCmd::Monitor(d, true));
        }
        let Some(v) = view.filter(|v| Some(v.device) == self.keymaps.parts.monitor_device) else { return };
        ui.add_space(8.0);
        for ((iface, id), r) in &v.reports {
            let title = fl!("map-monitor-report", iface = u32::from(*iface), id = u32::from(*id), count = r.count);
            card(ui, &title, |ui| self.map_report_bits(ui, &v, *iface, *id, r));
            ui.add_space(8.0);
        }
        if v.reports.is_empty() {
            card(ui, "", |ui| note(ui, fl!("map-monitor-waiting")));
            ui.add_space(8.0);
        }
        card(ui, &fl!("map-fields"), |ui| self.map_field_overrides(ui, &v));
    }

    /// A report as a grid of bits: declared fields tinted, changing bits brighter the more they change, the selection
    /// outlined. Clicking a bit starts a selection, Shift-click extends it.
    fn map_report_bits(&mut self, ui: &mut Ui, v: &MonitorView, iface: u8, id: u8, r: &crate::worker::ReportView) {
        let layout = v.maps.iter().find(|(i, _)| *i == iface).and_then(|(_, m)| kvm_hid::Layout::parse(m).ok());
        let skip = usize::from(layout.as_ref().is_some_and(|l| l.uses_report_ids));
        let data = r.bytes.get(skip..).unwrap_or(&[]);
        let declared = |bit: usize| {
            layout.as_ref().is_some_and(|l| {
                l.fields.iter().any(|f| {
                    f.ty == kvm_hid::ReportType::Input
                        && f.report_id == id
                        && f.flags & kvm_hid::parser::flags::CONSTANT == 0
                        && (f.bit_offset as usize..f.bit_offset as usize + f.bit_size as usize * f.count as usize)
                            .contains(&bit)
                })
            })
        };
        let max = r.changes.iter().copied().max().unwrap_or(0).max(1);
        let cell = 9.0;
        // Whole bytes per row, as many as fit.
        let per_row = (((ui.available_width() - 80.0) / cell) as usize / 8 * 8).clamp(8, 64);
        let rows = (data.len() * 8).div_ceil(per_row);
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(per_row as f32 * cell + 70.0, rows as f32 * (cell + 3.0)), Sense::click());
        let origin = resp.rect.min;
        let p = palette(ui);
        let sel = self.keymaps.parts.selection.filter(|s| (s.iface, s.report_id) == (iface, id));
        for bit in 0..data.len() * 8 {
            let (row, col) = (bit / per_row, bit % per_row);
            // Bytes left to right, a byte's bits from its lowest: the order a field's bits are counted in.
            let x = origin.x + 70.0 + col as f32 * cell + (col / 8) as f32 * 1.0;
            let y = origin.y + row as f32 * (cell + 3.0);
            let rect = egui::Rect::from_min_size(egui::pos2(x, y), Vec2::splat(cell - 1.0));
            let on = data[bit / 8] >> (bit % 8) & 1 != 0;
            let heat = r.changes.get(skip * 8 + bit).copied().unwrap_or(0) as f32 / max as f32;
            let base = if declared(bit) { p.card_stroke } else { ui.visuals().extreme_bg_color };
            let fill = if heat > 0.0 { WARN.gamma_multiply(0.25 + 0.75 * heat) } else { base };
            painter.rect_filled(rect, 1.0, fill);
            if on {
                painter.circle_filled(rect.center(), 2.0, ui.visuals().text_color());
            }
            if sel.is_some_and(|s| (s.first as usize..=s.last as usize).contains(&bit)) {
                painter.rect_stroke(rect, 1.0, Stroke::new(1.5, NEUTRAL), egui::StrokeKind::Inside);
            }
            if col == 0 {
                let label = format!("{:>3}", bit / 8);
                painter.text(
                    egui::pos2(origin.x, y),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::monospace(9.0),
                    p.weak,
                );
            }
        }
        if resp.clicked()
            && let Some(at) = resp.interact_pointer_pos()
        {
            let col = ((at.x - origin.x - 70.0) / cell).floor();
            let row = ((at.y - origin.y) / (cell + 3.0)).floor();
            if col >= 0.0 && row >= 0.0 {
                let bit = (row as usize * per_row + col as usize).min(data.len() * 8 - 1) as u16;
                let extend = ui.input(|i| i.modifiers.shift);
                self.keymaps.parts.selection = Some(match sel {
                    Some(s) if extend => Selection { first: s.first.min(bit), last: s.last.max(bit), ..s },
                    _ => Selection { iface, report_id: id, first: bit, last: bit },
                });
            }
        }
        let hex: Vec<String> = data.iter().map(|b| format!("{b:02x}")).collect();
        ui.label(RichText::new(hex.join(" ")).monospace().small());
    }

    /// The device's field overrides, and making the selected bits one.
    fn map_field_overrides(&mut self, ui: &mut Ui, v: &MonitorView) {
        note(ui, fl!("map-fields-note"));
        let target_name = |t: u8| match t.checked_sub(field::AXIS) {
            Some(a) => keycodes::AXES[kvm_map::axes::RELATIVE + a as usize].to_string(),
            None => format!("pad{}", t + 1),
        };
        for (n, o) in v.overrides.iter().enumerate() {
            let Some(o) = o else { continue };
            ui.horizontal(|ui| {
                let line = fl!(
                    "map-field-entry",
                    iface = u32::from(o.iface),
                    id = u32::from(o.report_id),
                    bit = u32::from(o.bit),
                    bits = u32::from(o.bits),
                    target = target_name(o.target)
                );
                ui.label(line);
                if ui.small_button("×").on_hover_text(fl!("map-remove")).clicked() {
                    self.send(UiCmd::SetOverride(v.device, n as u8, None));
                }
            });
        }
        let Some(sel) = self.keymaps.parts.selection else {
            note(ui, fl!("map-field-select"));
            return;
        };
        let bits = (sel.last - sel.first + 1).min(16) as u8;
        ui.separator();
        let line = fl!("map-field-selected", first = u32::from(sel.first), bits = u32::from(bits));
        ui.label(line);
        ui.horizontal_wrapped(|ui| {
            let ps = &mut self.keymaps.parts;
            let text = target_name(ps.target);
            egui::ComboBox::from_id_salt("field-target").selected_text(text).show_ui(ui, |ui| {
                for b in 0..32u8 {
                    ui.selectable_value(&mut ps.target, b, format!("pad{}", b + 1));
                }
                for a in 0..kvm_hid::gamepad::AXES as u8 {
                    ui.selectable_value(&mut ps.target, field::AXIS + a, target_name(field::AXIS + a));
                }
            });
            ui.checkbox(&mut ps.signed, fl!("map-field-signed"));
            ui.checkbox(&mut ps.invert, fl!("map-field-invert"));
        });
        let free = v.overrides.iter().position(Option::is_none);
        let ps = &self.keymaps.parts;
        let o = FieldOverride {
            iface: sel.iface,
            report_id: sel.report_id,
            bit: sel.first,
            bits,
            flags: if ps.signed { field::SIGNED } else { 0 } | if ps.invert { field::INVERT } else { 0 },
            target: ps.target,
        };
        ui.horizontal(|ui| {
            let ok = free.is_some();
            if ui.add_enabled(ok, egui::Button::new(fl!("map-field-make"))).clicked()
                && let Some(n) = free
            {
                self.send(UiCmd::SetOverride(v.device, n as u8, Some(o)));
                self.keymaps.parts.selection = None;
            }
            if !ok {
                ui.label(RichText::new(fl!("map-field-full", max = (MAX_OVERRIDES as u32))).color(ink(ui, WARN)));
            }
        });
    }

    // ── the hosts ───────────────────────────────────────────────────────────

    fn map_hosts(&mut self, ui: &mut Ui, s: &Snapshot) {
        let now = ui.input(|i| i.time);
        if self.keymaps.parts.asked_outputs.is_none_or(|t| now - t > 3.0) {
            self.keymaps.parts.asked_outputs = Some(now);
            self.send(UiCmd::LoadOutputs);
        }
        let outs = self.shared.lock().unwrap().outputs.clone();
        card(ui, &fl!("map-sec-hosts"), |ui| {
            note(ui, fl!("map-hosts-note"));
            egui::Grid::new("hosts").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new(fl!("map-layout")).strong());
                ui.label(RichText::new(fl!("map-outputs")).strong());
                ui.end_row();
                for h in Host::ALL.iter().copied().filter(|h| h.bt_slot().is_none() || s.has_bluetooth()) {
                    ui.label(crate::screens::host_label(s, h));
                    let l = config(s, |v| match v {
                        ConfigValue::HostLayout(x, l) if *x == h => Layout::from_u8(*l),
                        _ => None,
                    })
                    .unwrap_or_default();
                    let mut chosen = l;
                    egui::ComboBox::from_id_salt(("layout", h as u8)).selected_text(layout_name(l)).show_ui(ui, |ui| {
                        for x in Layout::ALL {
                            ui.selectable_value(&mut chosen, x, layout_name(x));
                        }
                    });
                    if chosen != l {
                        self.set(ConfigValue::HostLayout(h, chosen as u8));
                    }
                    ui.horizontal_wrapped(|ui| {
                        let (offered, taken) = outs.iter().find(|(x, ..)| *x == h).map_or((0, 0), |(_, o, t)| (*o, *t));
                        if offered == 0 {
                            ui.label(RichText::new(fl!("map-outputs-none")).color(palette(ui).weak));
                        }
                        for (bit, name) in output_names() {
                            if offered & bit != 0 {
                                let color = if taken & bit != 0 { GOOD } else { palette(ui).weak };
                                chip(ui, &name, color);
                            }
                        }
                    });
                    ui.end_row();
                }
            });
            note(ui, fl!("map-outputs-note"));
        });
        ui.add_space(8.0);
        let Some((_, count, profiles)) = s.profiles() else { return };
        card(ui, &fl!("map-fallback"), |ui| {
            note(ui, fl!("map-fallback-note"));
            egui::Grid::new("fallbacks").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                ui.label("");
                ui.label(RichText::new(fl!("map-keymap")).strong());
                ui.label(RichText::new(fl!("map-fallback-keymap")).strong());
                ui.end_row();
                for (i, p) in profiles.iter().enumerate().take(count as usize) {
                    ui.label(&p.name);
                    let km = |k: Option<u8>| k.map_or_else(|| fl!("map-keymap-none"), |k| k.to_string());
                    ui.label(km(p.keymap));
                    let fb = config(s, |v| match v {
                        ConfigValue::ProfileFallback(x, k) if *x as usize == i => Some(*k),
                        _ => None,
                    })
                    .flatten();
                    let mut chosen = fb;
                    egui::ComboBox::from_id_salt(("fallback", i)).selected_text(km(fb)).show_ui(ui, |ui| {
                        ui.selectable_value(&mut chosen, None, fl!("map-keymap-none"));
                        for k in 0..kvm_map::KEYMAPS as u8 {
                            ui.selectable_value(&mut chosen, Some(k), k.to_string());
                        }
                    });
                    if chosen != fb {
                        self.set(ConfigValue::ProfileFallback(i as u8, chosen));
                    }
                    ui.end_row();
                }
            });
        });
    }
}

/// The outputs in the order the hosts section shows them, with their names.
fn output_names() -> [(u8, String); 8] {
    [
        (outputs::KEYBOARD, fl!("map-out-keyboard")),
        (outputs::MOUSE, fl!("map-out-mouse")),
        (outputs::MEDIA, fl!("map-out-media")),
        (outputs::POINTER, fl!("map-out-pointer")),
        (outputs::GAMEPAD, fl!("map-out-gamepad")),
        (outputs::XINPUT, "XInput".into()),
        (outputs::VIAL, "Vial".into()),
        (outputs::MIRRORS, fl!("map-out-mirrors")),
    ]
}

/// A keyboard layout's name in the window's language.
pub fn layout_name(l: Layout) -> String {
    match l {
        Layout::Us => fl!("layout-us"),
        Layout::Gb => fl!("layout-gb"),
        Layout::De => fl!("layout-de"),
        Layout::Ch => fl!("layout-ch"),
        Layout::Fr => fl!("layout-fr"),
        Layout::Es => fl!("layout-es"),
        Layout::It => fl!("layout-it"),
        Layout::Pt => fl!("layout-pt"),
        Layout::No => fl!("layout-no"),
        Layout::Se => fl!("layout-se"),
        Layout::Dk => fl!("layout-dk"),
        Layout::Jp => fl!("layout-jp"),
        Layout::Br => fl!("layout-br"),
    }
}

/// A QMK setting's name in the window's language.
fn setting_label(name: &str) -> String {
    match name {
        "tapping-term" => fl!("map-set-tapping-term"),
        "permissive-hold" => fl!("map-set-permissive-hold"),
        "hold-on-other-key-press" => fl!("map-set-hold-on-other"),
        "retro-tapping" => fl!("map-set-retro-tapping"),
        "chordal-hold" => fl!("map-set-chordal-hold"),
        "quick-tap-term" => fl!("map-set-quick-tap-term"),
        "flow-tap-term" => fl!("map-set-flow-tap-term"),
        "tapping-toggle" => fl!("map-set-tapping-toggle"),
        "tap-code-delay" => fl!("map-set-tap-code-delay"),
        "tap-hold-caps-delay" => fl!("map-set-caps-delay"),
        "combo-term" => fl!("map-set-combo-term"),
        "oneshot-timeout" => fl!("map-set-oneshot-timeout"),
        "oneshot-tap-toggle" => fl!("map-set-oneshot-tap-toggle"),
        "auto-shift" => fl!("map-set-auto-shift"),
        "auto-shift-timeout" => fl!("map-set-auto-shift-timeout"),
        "grave-esc-override" => fl!("map-set-grave-esc"),
        "magic" => fl!("map-set-magic"),
        "mouse-delay" => fl!("map-set-mouse-delay"),
        "mouse-interval" => fl!("map-set-mouse-interval"),
        "mouse-move-delta" => fl!("map-set-mouse-delta"),
        "mouse-max-speed" => fl!("map-set-mouse-max-speed"),
        "mouse-time-to-max" => fl!("map-set-mouse-time-to-max"),
        "wheel-delay" => fl!("map-set-wheel-delay"),
        "wheel-interval" => fl!("map-set-wheel-interval"),
        "wheel-max-speed" => fl!("map-set-wheel-max-speed"),
        _ => fl!("map-set-wheel-time-to-max"),
    }
}
