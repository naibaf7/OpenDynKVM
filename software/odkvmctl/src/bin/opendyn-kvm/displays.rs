//! The Displays page: monitors whose input follows the KVM (DDC/CI, `odkvmctl::ddc`). A setting of this computer,
//! saved in `odkvmctl::local` as soon as it changes; the worker applies it on every switch and change of profile the
//! KVM reports.

use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText, Ui};
use kvm_proto::ctrl::{MAX_PROFILES, Pc};
use odkvmctl::ddc::{self, Display, mccs};
use odkvmctl::fl;
use odkvmctl::local::{self, DisplayMode, Local};

use crate::look::{BAD, GOOD, WARN, card, chip, ink, note};
use crate::worker::Wake;

#[derive(Default)]
enum Scan {
    #[default]
    NotYet,
    Running,
    Done(Result<Vec<Display>, String>),
}

#[derive(Default)]
pub struct Displays {
    scan: Arc<Mutex<Scan>>,
    /// Result of the last "Show PCn" test.
    test: Arc<Mutex<Option<Result<String, String>>>>,
    /// Text of the custom-value fields, per monitor and PC.
    custom: Vec<(String, usize, String)>,
}

/// The KVM's profiles as the page lists them: number, name, active.
pub type ProfileRows = Vec<(usize, String, bool)>;

/// The paired Bluetooth hosts as the page lists them: slot, name, address.
pub type HostRows = Vec<(u8, String, String)>;

/// The paired Bluetooth hosts; none without a KVM or without the module.
pub fn host_rows(s: Option<&odkvmctl::client::Snapshot>) -> HostRows {
    s.map(|s| s.bt_bonded().map(|h| (h.slot, h.name.clone(), h.address())).collect()).unwrap_or_default()
}

/// One input row of a monitor's card: for a PC, or a Bluetooth host.
struct InputRow {
    /// Index of its "other" field: 0, 1 for the PCs, 1 + slot for a host.
    key: usize,
    label: RichText,
    /// The name its "Show" button uses.
    name: String,
    /// The monitor's current input, offered as "Use current" (this computer's own row only).
    current: Option<u16>,
    /// What no input means for it.
    none: String,
}

/// The profiles in use and the active one; without a KVM, every profile by number.
pub fn profile_rows(s: Option<&odkvmctl::client::Snapshot>) -> ProfileRows {
    match s.and_then(|s| s.profiles()) {
        Some((active, count, list)) => {
            list.into_iter().take(count as usize).enumerate().map(|(i, p)| (i, p.name, i == active as usize)).collect()
        }
        None => (0..MAX_PROFILES).map(|i| (i, fl!("profile-n", n = i), false)).collect(),
    }
}

fn mode_label(m: DisplayMode) -> String {
    match m {
        DisplayMode::Follow => fl!("display-follow"),
        DisplayMode::Pc1 => fl!("display-pc1"),
        DisplayMode::Pc2 => fl!("display-pc2"),
        DisplayMode::Off => fl!("display-off"),
    }
}

fn input_label(v: Option<u16>, none: &str) -> String {
    v.map_or_else(|| none.into(), mccs::input_name)
}

fn parse_hex(s: &str) -> Option<u16> {
    let s = s.trim();
    u16::from_str_radix(s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s), 16).ok()
}

impl Displays {
    /// As if a scan had found `list` (tests: no real monitors are asked).
    #[cfg(test)]
    pub fn found(list: Vec<Display>) -> Displays {
        Displays { scan: Arc::new(Mutex::new(Scan::Done(Ok(list)))), ..Displays::default() }
    }

    /// Looks for monitors in the background (a second or so per monitor).
    pub fn start_scan(&self, wake: Wake) {
        *self.scan.lock().unwrap() = Scan::Running;
        let scan = self.scan.clone();
        std::thread::spawn(move || {
            let r = ddc::displays(true).map_err(|e| e.to_string());
            *scan.lock().unwrap() = Scan::Done(r);
            wake();
        });
    }

    fn test(&self, id: String, name: String, value: u16, wake: Wake) {
        let test = self.test.clone();
        *test.lock().unwrap() = None;
        std::thread::spawn(move || {
            let r = ddc::set_input(&id, value)
                .map(|()| fl!("display-switched", monitor = name.as_str(), input = mccs::input_name(value)))
                .map_err(|e| format!("{name}: {e}"));
            *test.lock().unwrap() = Some(r);
            wake();
        });
    }

    /// `this_pc`: which PC this computer is (the KVM tells), if connected; `profiles`: [`profile_rows`]; `hosts`:
    /// [`host_rows`].
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        local: &mut Local,
        this_pc: Option<Pc>,
        profiles: &ProfileRows,
        hosts: &HostRows,
        wake: &Wake,
    ) -> bool {
        let mut changed = false;
        if matches!(*self.scan.lock().unwrap(), Scan::NotYet) {
            self.start_scan(wake.clone());
        }
        card(ui, &fl!("displays-title"), |ui| {
            note(ui, fl!("displays-note"));
            note(ui, fl!("displays-two-pcs"));
            note(ui, fl!("displays-ddc-on"));
            ui.horizontal(|ui| {
                let running = matches!(*self.scan.lock().unwrap(), Scan::Running);
                if ui.add_enabled(!running, egui::Button::new(fl!("look-again"))).clicked() {
                    self.start_scan(wake.clone());
                }
                if running {
                    ui.spinner();
                    ui.label(fl!("asking-monitors"));
                }
                match this_pc {
                    Some(pc) => chip(ui, &fl!("this-computer-is", pc = pc.name().to_uppercase()), GOOD),
                    None => chip(ui, &fl!("kvm-not-connected"), WARN),
                }
            });
            if let Some(r) = self.test.lock().unwrap().as_ref() {
                match r {
                    Ok(t) => ui.label(RichText::new(t).color(ink(ui, GOOD))),
                    Err(e) => ui.label(RichText::new(e).color(ink(ui, BAD))),
                };
            }
        });
        ui.add_space(8.0);

        let found = match &*self.scan.lock().unwrap() {
            Scan::Done(Ok(list)) => list.clone(),
            Scan::Done(Err(e)) => {
                card(ui, "", |ui| ui.label(RichText::new(e).color(ink(ui, BAD))));
                Vec::new()
            }
            _ => Vec::new(),
        };
        let done = matches!(*self.scan.lock().unwrap(), Scan::Done(_));
        if done && found.is_empty() {
            card(ui, "", |ui| note(ui, fl!("no-monitor")));
        }
        for d in &found {
            changed |= self.monitor(ui, d, local, this_pc, profiles, hosts, wake);
            ui.add_space(8.0);
        }
        // Saved monitors that are not connected now.
        let gone: Vec<(String, String)> = local
            .displays
            .iter()
            .filter(|b| done && !found.iter().any(|d| d.id == b.id))
            .map(|b| (b.id.clone(), b.name.clone()))
            .collect();
        if !gone.is_empty() {
            card(ui, &fl!("saved-not-connected"), |ui| {
                for (id, name) in gone {
                    ui.horizontal(|ui| {
                        ui.label(&name);
                        note(ui, &id);
                        if ui.small_button(fl!("forget")).clicked() {
                            local.displays.retain(|b| b.id != id);
                            changed = true;
                        }
                    });
                }
            });
        }
        changed
    }

    /// One input row of a monitor: the input, an "other" field for one it doesn't list, "Use current", "Show".
    /// Returns whether `value` changed.
    fn input_row(
        &mut self,
        ui: &mut Ui,
        d: &Display,
        options: &[u16],
        value: &mut Option<u16>,
        row: InputRow,
        wake: &Wake,
    ) -> bool {
        let before = *value;
        ui.label(row.label);
        egui::ComboBox::from_id_salt(("input", &d.id, row.key))
            .selected_text(input_label(*value, &row.none))
            .width(200.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(value, None, &row.none);
                for v in options {
                    ui.selectable_value(value, Some(*v), mccs::input_name(*v));
                }
            });
        // A value the monitor doesn't list (some list fewer inputs than they have).
        let slot = match self.custom.iter().position(|(id, k, _)| *id == d.id && *k == row.key) {
            Some(i) => i,
            None => {
                self.custom.push((d.id.clone(), row.key, String::new()));
                self.custom.len() - 1
            }
        };
        let field = &mut self.custom[slot].2;
        let r = ui
            .add(egui::TextEdit::singleline(field).hint_text(fl!("input-other")).min_size(egui::vec2(64.0, 0.0)))
            .on_hover_text(fl!("input-other-hover"));
        if r.lost_focus()
            && let Some(v) = parse_hex(field)
        {
            *value = Some(v);
            field.clear();
        }
        ui.horizontal(|ui| {
            if let Some(cur) = row.current
                && ui.button(fl!("use-current")).on_hover_text(fl!("use-current-hover")).clicked()
            {
                *value = Some(cur);
            }
            if let Some(v) = *value
                && ui.button(fl!("show-target", target = row.name.as_str())).clicked()
            {
                self.test(d.id.clone(), d.name.clone(), v, wake.clone());
            }
        });
        ui.end_row();
        *value != before
    }

    #[allow(clippy::too_many_arguments)]
    fn monitor(
        &mut self,
        ui: &mut Ui,
        d: &Display,
        local: &mut Local,
        this_pc: Option<Pc>,
        profiles: &ProfileRows,
        hosts: &HostRows,
        wake: &Wake,
    ) -> bool {
        let mut changed = false;
        card(ui, &d.name, |ui| {
            ui.horizontal(|ui| {
                note(ui, &d.id);
                match (d.current, &d.error) {
                    (Some(v), _) => chip(ui, &fl!("monitor-shows", input = mccs::input_name(v)), GOOD),
                    (None, Some(_)) => chip(ui, &fl!("not-answering"), BAD),
                    _ => {}
                }
            });
            if let Some(e) = &d.error {
                ui.label(RichText::new(e).color(ink(ui, BAD)));
                return;
            }
            let b = local.binding_mut(&d.id, &d.name);
            // The inputs the monitor lists, the one it shows now, and the ones already chosen.
            let mut options = d.inputs.clone();
            let chosen = b.inputs.iter().flatten().chain(b.bt_inputs.values()).copied();
            for v in d.current.into_iter().chain(chosen) {
                if !options.contains(&v) {
                    options.push(v);
                }
            }
            options.sort_unstable();
            egui::Grid::new(("inputs", &d.id)).num_columns(4).spacing([12.0, 8.0]).show(ui, |ui| {
                for (k, pc) in [Pc::Pc1, Pc::Pc2].into_iter().enumerate() {
                    let name = pc.name().to_uppercase();
                    let row = InputRow {
                        key: k,
                        label: RichText::new(fl!("target-input", target = name.as_str())).strong(),
                        current: d.current.filter(|_| this_pc == Some(pc)),
                        name,
                        none: fl!("input-none-pc"),
                    };
                    changed |= self.input_row(ui, d, &options, &mut b.inputs[k], row, wake);
                }
                // Bluetooth hosts wired to the monitor too (a laptop, a tablet with a video output): optional.
                for (slot, name, addr) in hosts {
                    let mut v = b.bt_inputs.get(addr).copied();
                    let row = InputRow {
                        key: 1 + *slot as usize,
                        label: RichText::new(fl!("target-input", target = name.as_str()))
                            .strong()
                            .color(ink(ui, crate::bluetooth::violet())),
                        name: name.clone(),
                        current: None,
                        none: fl!("input-none-host"),
                    };
                    if self.input_row(ui, d, &options, &mut v, row, wake) {
                        match v {
                            Some(v) => b.bt_inputs.insert(addr.clone(), v),
                            None => b.bt_inputs.remove(addr),
                        };
                        changed = true;
                    }
                }
            });
            if !hosts.is_empty() {
                note(ui, fl!("displays-hosts-note"));
            }
            ui.add_space(4.0);
            note(ui, fl!("in-each-profile"));
            egui::Grid::new(("profiles", &d.id)).num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                for (i, name, active) in profiles {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).strong());
                        if *active {
                            chip(ui, &fl!("active"), GOOD);
                        }
                    });
                    let mode = &mut b.profiles[*i];
                    let before = *mode;
                    egui::ComboBox::from_id_salt(("display-mode", &d.id, *i))
                        .selected_text(mode_label(*mode))
                        .width(200.0)
                        .show_ui(ui, |ui| {
                            for m in DisplayMode::ALL {
                                ui.selectable_value(mode, m, mode_label(m));
                            }
                        });
                    changed |= *mode != before;
                    ui.end_row();
                }
            });
            // An input a profile needs but that isn't chosen.
            let missing = profiles.iter().any(|(i, ..)| match b.profiles[*i] {
                DisplayMode::Follow => b.inputs.iter().any(Option::is_none),
                DisplayMode::Pc1 => b.inputs[0].is_none(),
                DisplayMode::Pc2 => b.inputs[1].is_none(),
                DisplayMode::Off => false,
            });
            if missing {
                ui.label(RichText::new(fl!("input-missing")).color(ink(ui, WARN)));
            }
        });
        changed
    }
}

/// Saves the local settings; returns the error to show.
pub fn save(local: &Local) -> Option<String> {
    local::save(local).err().map(|e| fl!("could-not-save", error = e.to_string()))
}
