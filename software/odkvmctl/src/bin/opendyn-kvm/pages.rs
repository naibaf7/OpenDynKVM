//! The pages that show and change the KVM: Overview, Ports, Keyboard, Settings, Firmware.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Ui};
use kvm_core::config::{LONG_PRESS_RANGE_MS, TAP_MAX_MS};
use kvm_core::engine::{LEARN_MS, UNLOCK_WAIT_MS};
use kvm_core::hotkey::format_spec;
use kvm_proto::ctrl::{
    ConfigValue, Hotkey, KeyAction, LearnEnd, LinkState, MAX_PROFILES, Pc, Polling, Port, PortMode, PortState,
    ProfileName, Rgb, Target, VialMode, caps, class, diag, key, selftest, status_flags,
};
use odkvmctl::client::Snapshot;
use odkvmctl::{autostart, fl, i18n, local, names};

use crate::look::{self, BAD, GOOD, WARN, card, chip, ink, note};
use crate::worker::UiCmd;
use crate::{App, accent_of, config, optimistic, pc_colors, switch_target};

/// How often the Firmware page reads the diagnostics while it is shown.
const DIAG_EVERY: Duration = Duration::from_secs(2);

/// A diagnostics grade's colour (`Info`: the card's weak text).
fn grade_color(g: diag::Grade, weak: Color32) -> Color32 {
    match g {
        diag::Grade::Info => weak,
        diag::Grade::Ok => GOOD,
        diag::Grade::Warn => WARN,
        diag::Grade::Bad => BAD,
    }
}

/// A grade's dot: filled in its colour, or a ring for `Info`.
fn grade_dot(ui: &mut Ui, g: diag::Grade, weak: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
    let c = grade_color(g, weak);
    if g == diag::Grade::Info {
        ui.painter().circle_stroke(rect.center(), 3.5, egui::Stroke::new(1.0, c));
    } else {
        ui.painter().circle_filled(rect.center(), 4.5, c);
    }
}

/// Diagnostics groups in one grid, so that their values line up: a heading row per group, then its items.
fn diag_column(ui: &mut Ui, id: &str, groups: &[(Option<diag::Group>, Vec<&diag::Item>)], weak: Color32) {
    egui::Grid::new(id).num_columns(3).spacing([8.0, 5.0]).show(ui, |ui| {
        for (k, (group, list)) in groups.iter().enumerate() {
            if k > 0 {
                ui.end_row();
            }
            ui.label("");
            ui.label(RichText::new(names::diag_group(*group)).strong());
            ui.end_row();
            for i in list {
                grade_dot(ui, i.grade, weak);
                let name = ui.label(names::diag_name(i.id));
                let help = names::diag_help(i.id);
                if !help.is_empty() {
                    name.on_hover_text(help);
                }
                let value = RichText::new(names::diag_value(i));
                ui.label(if matches!(i.grade, diag::Grade::Warn | diag::Grade::Bad) {
                    value.color(ink(ui, grade_color(i.grade, weak))).strong()
                } else {
                    value
                });
                ui.end_row();
            }
        }
    });
}

/// The hotkey rows on the Keyboard page; the last only for a KVM with the Bluetooth module.
fn hotkeys() -> [(String, u16); 4] {
    [
        (fl!("hotkey-toggle"), key::HOTKEY_TOGGLE),
        (fl!("hotkey-pc1"), key::HOTKEY_PC1),
        (fl!("hotkey-pc2"), key::HOTKEY_PC2),
        (fl!("hotkey-next"), key::HOTKEY_NEXT),
    ]
}

/// The keyboard usage of a key in the window (for the simulated keyboard).
fn hid_usage(k: egui::Key) -> Option<u8> {
    use egui::Key as K;
    Some(match k {
        K::Enter => 0x28,
        K::Escape => 0x29,
        K::Backspace => 0x2A,
        K::Tab => 0x2B,
        K::Space => 0x2C,
        K::Minus => 0x2D,
        K::Equals => 0x2E,
        K::OpenBracket => 0x2F,
        K::CloseBracket => 0x30,
        K::Backslash => 0x31,
        K::Semicolon => 0x33,
        K::Quote => 0x34,
        K::Backtick => 0x35,
        K::Comma => 0x36,
        K::Period => 0x37,
        K::Slash => 0x38,
        K::Insert => 0x49,
        K::Home => 0x4A,
        K::PageUp => 0x4B,
        K::Delete => 0x4C,
        K::End => 0x4D,
        K::PageDown => 0x4E,
        K::ArrowRight => 0x4F,
        K::ArrowLeft => 0x50,
        K::ArrowDown => 0x51,
        K::ArrowUp => 0x52,
        // Letters, digits and F1–F24, by name.
        k => return kvm_hid::keys::usage_of(k.name()).filter(|u| matches!(u, 0x04..=0x27 | 0x3A..=0x45 | 0x68..=0x73)),
    })
}

/// Why a hotkey can't be learned now, if it can't.
fn cant_learn(s: &Snapshot) -> Option<String> {
    if s.info.proto < 4 {
        Some(fl!("cant-learn-firmware"))
    } else if s.status.asking.is_some_and(|pc| pc != s.status.active) {
        Some(fl!("cant-learn-not-active"))
    } else if !s.ports.iter().any(|p| p.state == PortState::Hosted && p.classes & class::KEYBOARD != 0) {
        Some(fl!("cant-learn-no-keyboard"))
    } else {
        None
    }
}

/// The long-press duration: a slider whose handle sits right above its mark on a bar from 0 to the longest long press.
/// The bar shows what a press does by how long the key is held: a tap up to [`TAP_MAX_MS`], then nothing, then the long
/// press (which fires while the key is still down). Returns the new duration if it was moved.
fn long_press_slider(ui: &mut Ui, mut ms: u16, accent: Color32) -> Option<u16> {
    let (start, end) = (*LONG_PRESS_RANGE_MS.start() as f32, *LONG_PRESS_RANGE_MS.end() as f32);
    let width = ui.available_width().min(460.0);
    // egui insets the handle's travel by its radius, the slider's thickness / 2.5 (`SliderGeometry::handle_radius`).
    let radius = ui.text_style_height(&egui::TextStyle::Body).max(ui.spacing().interact_size.y) / 2.5;
    let moved = ui
        .horizontal(|ui| {
            ui.add_space(width * start / end - radius);
            ui.spacing_mut().slider_width = width * (end - start) / end + 2.0 * radius;
            ui.add(egui::Slider::new(&mut ms, LONG_PRESS_RANGE_MS).step_by(100.0).suffix(" ms")).changed()
        })
        .inner;
    press_timeline(ui, ms, width, accent);
    moved.then_some(ms)
}

fn press_timeline(ui: &mut Ui, long_ms: u16, width: f32, accent: Color32) {
    let max = *LONG_PRESS_RANGE_MS.end() as f32;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 32.0), egui::Sense::hover());
    let x = |ms: u16| rect.left() + rect.width() * ms as f32 / max;
    let bar = |from: f32, to: f32| egui::Rect::from_x_y_ranges(from..=to, rect.top() + 4.0..=rect.top() + 12.0);
    let weak = look::palette(ui).weak;
    let (tap_ink, long_ink) = (ink(ui, GOOD), ink(ui, accent));
    let p = ui.painter();
    p.rect_filled(bar(rect.left(), rect.right()), 4.0, weak.gamma_multiply(0.3));
    p.rect_filled(bar(rect.left(), x(TAP_MAX_MS)), 4.0, GOOD);
    p.rect_filled(bar(x(long_ms), rect.right()), 4.0, accent);
    let font = egui::FontId::proportional(12.0);
    let below = rect.top() + 15.0;
    p.text(egui::pos2(rect.left(), below), egui::Align2::LEFT_TOP, fl!("press-tap"), font.clone(), tap_ink);
    let label = fl!("press-long-from", seconds = i18n::decimal(long_ms as f64 / 1000.0, 1));
    // Near the end the label goes left of the mark, so it stays on the bar.
    let anchor = if x(long_ms) > rect.center().x { egui::Align2::RIGHT_TOP } else { egui::Align2::LEFT_TOP };
    p.text(egui::pos2(x(long_ms), below), anchor, label, font, long_ink);
}

impl App {
    fn pc_card(&self, ui: &mut Ui, s: &Snapshot, pc: Pc) {
        let st = &s.status;
        let k = pc.index();
        // While a Bluetooth host has the input, neither PC is active.
        let active = st.active == pc && st.bt_active == 0;
        let color = look::color32(pc_colors(s)[k]);
        let p = look::palette(ui);
        egui::Frame::new()
            .fill(if active { color.gamma_multiply(0.16) } else { p.card })
            .stroke(egui::Stroke::new(if active { 2.0 } else { 1.0 }, if active { color } else { p.card_stroke }))
            .corner_radius(egui::CornerRadius::same(12))
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(pc.name().to_uppercase()).size(22.0).color(ink(ui, color)).strong());
                    if active {
                        chip(ui, &fl!("chip-active"), color);
                    }
                    if st.asking == Some(pc) {
                        chip(ui, &fl!("chip-this-computer"), look::NEUTRAL);
                    }
                });
                let link = st.links[k];
                let link_color = match link {
                    LinkState::Connected => GOOD,
                    LinkState::Suspended | LinkState::Enumerating => WARN,
                    LinkState::LinkDown => BAD,
                    _ => p.weak,
                };
                ui.label(RichText::new(names::link(link)).color(ink(ui, link_color)));
                note(
                    ui,
                    fl!(
                        "pc-vbus",
                        volts = i18n::decimal(st.vbus_mv[k + 1] as f64 / 1000.0, 2),
                        offers = names::advertised(st.adv_pc[k])
                    ),
                );
                let switchable = st.flags & status_flags::SW_SWITCH_ALLOWED != 0;
                let button =
                    egui::Button::new(RichText::new(if active { fl!("showing") } else { fl!("switch-here") }).strong())
                        .min_size(egui::vec2(120.0, 30.0));
                if ui.add_enabled(!active && switchable, button).clicked() {
                    self.switch(pc);
                }
            });
    }

    /// Switches right away in the window, then asks the KVM.
    pub(crate) fn switch(&self, pc: Pc) {
        self.switch_to(switch_target(pc));
    }

    /// Switches to `t` right away in the window (a PC, or a Bluetooth host), then asks the KVM.
    pub(crate) fn switch_to(&self, t: Target) {
        if let Some(snap) = self.shared.lock().unwrap().snap.as_mut() {
            let st = &mut snap.status;
            match t {
                Target::Pc1 | Target::Pc2 => {
                    st.active = if t == Target::Pc1 { Pc::Pc1 } else { Pc::Pc2 };
                    // Leaving a Bluetooth host: the PC buttons light up now, not at the next refresh.
                    st.bt_active = 0;
                }
                _ => {
                    if let Some(slot) = t.bt_host() {
                        st.bt_active = slot;
                    }
                }
            }
        }
        self.send(UiCmd::Switch(t));
    }

    pub(crate) fn overview(&self, ui: &mut Ui, s: &Snapshot) {
        ui.columns(2, |cols| {
            self.pc_card(&mut cols[0], s, Pc::Pc1);
            self.pc_card(&mut cols[1], s, Pc::Pc2);
        });
        self.overview_bt(ui, s);
        ui.add_space(8.0);
        card(ui, &fl!("front-ports"), |ui| {
            egui::Grid::new("ports-summary").num_columns(3).spacing([16.0, 6.0]).show(ui, |ui| {
                for port in Port::ALL {
                    let p = &s.ports[port.index()];
                    ui.label(RichText::new(names::port_title(*port)).strong());
                    let product = &s.products[port.index()];
                    ui.label(if p.state == PortState::Empty {
                        "—".to_string()
                    } else if product.is_empty() {
                        fl!("unnamed-device")
                    } else {
                        product.clone()
                    });
                    note(ui, names::mode(p.effective));
                    ui.end_row();
                }
            });
            // A Bluetooth host gets what the normalized and mirrored ports type; passthrough can't go over Bluetooth.
            if let Some(h) = s.bt.iter().find(|h| h.slot == s.status.bt_active) {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(fl!(
                        "host-has-input",
                        host = h.name.as_str(),
                        pc = s.status.active.name().to_uppercase()
                    ))
                    .color(ink(ui, crate::bluetooth::violet())),
                );
            }
        });
        ui.add_space(8.0);
        let st = &s.status;
        card(ui, &fl!("power"), |ui| {
            ui.label(fl!("power-left", source = names::power_source(st.power), milliamps = st.budget_ma));
            if st.flags & status_flags::BUDGET_LOW != 0 {
                ui.label(RichText::new(fl!("budget-low")).color(ink(ui, WARN)));
            }
            if st.flags & status_flags::SELFTEST_FAILED != 0 {
                ui.label(
                    RichText::new(fl!("selftest-line", result = names::selftest(st.selftest))).color(ink(ui, BAD)),
                );
            }
        });
        let events = self.shared.lock().unwrap().events.clone();
        if !events.is_empty() {
            ui.add_space(8.0);
            card(ui, &fl!("recent"), |ui| {
                for e in events.iter().take(6) {
                    note(ui, e);
                }
            });
        }
    }

    fn port_card(&self, ui: &mut Ui, s: &Snapshot, port: Port) {
        let p = &s.ports[port.index()];
        let locked = s.status.flags & status_flags::SW_CONFIG_ALLOWED == 0;
        card(ui, &names::port_title(port), |ui| {
            ui.horizontal(|ui| {
                chip(
                    ui,
                    &names::port_state(p.state),
                    if p.state == PortState::Empty { look::palette(ui).weak } else { GOOD },
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!locked && p.state != PortState::Empty, egui::Button::new(fl!("redetect")))
                        .clicked()
                    {
                        self.send(UiCmd::Redetect(port));
                    }
                    let mut mode = p.mode;
                    ui.add_enabled_ui(!locked, |ui| {
                        egui::ComboBox::from_id_salt(("mode", port.index()))
                            .selected_text(names::mode(mode))
                            .width(230.0)
                            .show_ui(ui, |ui| {
                                for m in PortMode::ALL {
                                    ui.selectable_value(&mut mode, *m, names::mode(*m));
                                }
                            });
                    });
                    if mode != p.mode {
                        optimistic(&self.shared, ConfigValue::PortMode(port, mode));
                        self.send(UiCmd::PortMode(port, mode));
                    }
                    ui.label(fl!("mode"));
                });
            });
            if p.state == PortState::Empty {
                note(ui, fl!("no-device"));
                return;
            }
            let product = &s.products[port.index()];
            let product = if product.is_empty() { fl!("unnamed-device") } else { product.clone() };
            ui.label(RichText::new(product).size(15.0));
            note(ui, format!("{:04x}:{:04x} · {}", p.vid, p.pid, names::speed(p.speed)));
            let mut line = fl!("running-mode", mode = names::mode(p.effective));
            if p.mode == PortMode::Auto || p.mode != p.effective {
                line += &format!(" ({})", names::reason(p.reason));
            }
            ui.label(line);
            let mut details = names::classes(p.classes);
            if p.report_rate_hz > 0 {
                details += &format!(" · {} Hz", p.report_rate_hz);
            }
            if p.max_power_ma > 0 {
                details += &format!(" · {}", fl!("up-to-ma", milliamps = p.max_power_ma));
            }
            note(ui, details);
            if p.faults > 0 {
                ui.label(RichText::new(fl!("over-current-faults", count = p.faults)).color(ink(ui, BAD)));
            }
        });
    }

    /// The profiles in use: which one is active, their names and port modes, and how many are in use.
    fn profiles_card(&mut self, ui: &mut Ui, s: &Snapshot) {
        let Some((active, count, list)) = s.profiles() else { return };
        let locked = s.status.flags & status_flags::SW_CONFIG_ALLOWED == 0;
        card(ui, &fl!("profiles"), |ui| {
            note(ui, fl!("profiles-note"));
            let hop = s.has_hop();
            ui.add_enabled_ui(!locked, |ui| {
                egui::Grid::new("profiles").num_columns(if hop { 6 } else { 5 }).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("");
                    note(ui, fl!("name"));
                    for port in Port::ALL {
                        note(ui, names::port_short(*port));
                    }
                    if hop {
                        note(ui, fl!("profile-hop"));
                    }
                    ui.end_row();
                    for (i, p) in list.iter().enumerate().take(count as usize) {
                        let n = i as u8;
                        if ui.radio(n == active, "").clicked() && n != active {
                            self.set(ConfigValue::Profile(n));
                        }
                        // The name follows the KVM except while it is being edited.
                        if self.profile_editing != Some(i) {
                            self.profile_names[i] = p.name.clone();
                        }
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.profile_names[i]).min_size(egui::vec2(110.0, 0.0)),
                        );
                        if r.has_focus() {
                            self.profile_editing = Some(i);
                        } else if r.lost_focus() {
                            self.profile_editing = None;
                            let name = self.profile_names[i].trim();
                            if !name.is_empty() && name != p.name {
                                self.set(ConfigValue::ProfileName(n, ProfileName::new(name)));
                            }
                        }
                        let mut modes = p.modes;
                        for port in Port::ALL {
                            let m = &mut modes[port.index()];
                            egui::ComboBox::from_id_salt(("profile-mode", i, port.index()))
                                .selected_text(names::mode_short(*m))
                                .width(110.0)
                                .show_ui(ui, |ui| {
                                    for mode in PortMode::ALL {
                                        ui.selectable_value(m, *mode, names::mode(*mode));
                                    }
                                });
                        }
                        if modes != p.modes {
                            self.set(ConfigValue::ProfileModes(n, modes));
                        }
                        let mut on = p.hop;
                        if hop && ui.checkbox(&mut on, "").on_hover_text(fl!("profile-hop-hover")).changed() {
                            self.set(ConfigValue::ProfileHop(n, on));
                        }
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(fl!("profiles-in-use"));
                    let mut n = count;
                    for k in 1..=MAX_PROFILES as u8 {
                        ui.selectable_value(&mut n, k, k.to_string());
                    }
                    if n != count {
                        self.set(ConfigValue::ProfileCount(n));
                    }
                });
                let remember = config(s, |v| if let ConfigValue::RememberProfile(b) = v { Some(*b) } else { None });
                if let Some(mut r) = remember
                    && ui.checkbox(&mut r, fl!("remember-profile")).changed()
                {
                    self.set(ConfigValue::RememberProfile(r));
                }
            });
        });
        ui.add_space(8.0);
    }

    pub(crate) fn ports(&mut self, ui: &mut Ui, s: &Snapshot) {
        self.profiles_card(ui, s);
        for p in Port::ALL {
            self.port_card(ui, s, *p);
            ui.add_space(8.0);
        }
    }

    pub(crate) fn locked_note(ui: &mut Ui, s: &Snapshot) -> bool {
        let locked = s.status.flags & status_flags::SW_CONFIG_ALLOWED == 0;
        if locked {
            card(ui, "", |ui| {
                ui.label(RichText::new(fl!("config-locked")).color(ink(ui, BAD)));
                note(ui, fl!("config-locked-how"));
            });
            ui.add_space(8.0);
        }
        locked
    }

    /// Has the KVM learn hotkey row `i` from its keyboard.
    pub(crate) fn learn(&mut self, i: usize) {
        self.shared.lock().unwrap().learn = None;
        self.learning = Some((i, Instant::now()));
        self.learn_note = None;
        self.hotkey_error[i] = None;
        self.send(UiCmd::Learn(true));
    }

    /// With the simulator, while a hotkey is learned: the keys pressed in the window are what its keyboard types (as a
    /// boot-protocol report each time they change), since this computer's keyboard isn't on the simulated KVM.
    fn play_sim_keyboard(&mut self, ui: &mut Ui, kb: &std::sync::mpsc::Sender<[u8; 8]>) {
        if self.learning.is_none() {
            // Let go of anything still held when learning ended.
            if self.sim_keys.take().is_some_and(|(m, k)| m != 0 || !k.is_empty()) {
                let _ = kb.send([0; 8]);
            }
            return;
        }
        // The keys mustn't type into a field meanwhile.
        ui.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
        let (mut modifiers, mut keys) = self.sim_keys.clone().unwrap_or_default();
        let before = (modifiers, keys.clone());
        ui.input(|i| {
            let m = i.modifiers;
            modifiers = u8::from(m.ctrl) | u8::from(m.shift) << 1 | u8::from(m.alt) << 2 | u8::from(m.mac_cmd) << 3;
            for e in &i.events {
                if let egui::Event::Key { key, physical_key, pressed, repeat: false, .. } = e
                    && let Some(u) = hid_usage(physical_key.unwrap_or(*key))
                {
                    keys.retain(|k| *k != u);
                    if *pressed && keys.len() < 6 {
                        keys.push(u);
                    }
                }
            }
        });
        if (modifiers, keys.clone()) != before || self.sim_keys.is_none() {
            let mut r = [modifiers, 0, 0, 0, 0, 0, 0, 0];
            r[2..2 + keys.len()].copy_from_slice(&keys);
            let _ = kb.send(r);
        }
        self.sim_keys = Some((modifiers, keys));
    }

    /// The KVM's answer to learning: a hotkey goes into its field, to be applied.
    fn learn_outcome(&mut self, s: &Snapshot) {
        let outcome = self.shared.lock().unwrap().learn.take();
        let Some((i, since)) = self.learning else { return };
        let note = match outcome {
            Some(Ok((LearnEnd::Learned, h))) => {
                self.hotkeys[i] = format_spec(&h).to_string();
                let rows = hotkeys();
                let others: Vec<(&str, Hotkey)> = rows
                    .iter()
                    .filter(|(_, k)| *k != rows[i].1)
                    .filter_map(|(label, k)| match config(s, |v| (v.key() == *k).then_some(*v)) {
                        Some(
                            ConfigValue::HotkeyToggle(h)
                            | ConfigValue::HotkeyPc1(h)
                            | ConfigValue::HotkeyPc2(h)
                            | ConfigValue::HotkeyNext(h),
                        ) => Some((label.as_str(), h)),
                        _ => None,
                    })
                    .collect();
                match names::hotkey_warning(&h, &others) {
                    Some(w) => Err(fl!("learned-with-warning", warning = w)),
                    None => Ok(fl!("learned")),
                }
            }
            Some(Ok((end, _))) => Err(fl!("not-learned", why = names::learn_end(end))),
            Some(Err(why)) => Err(why),
            None if since.elapsed() > Duration::from_millis(LEARN_MS as u64 + 2000) => Err(fl!("kvm-no-answer")),
            None => return,
        };
        self.learning = None;
        self.learn_note = Some((i, note));
    }

    pub(crate) fn keyboard(&mut self, ui: &mut Ui, s: &Snapshot) {
        let locked = Self::locked_note(ui, s);
        if !self.hotkeys_loaded {
            for (i, (_, k)) in hotkeys().into_iter().enumerate() {
                if let Some(v) = s.config.iter().find(|v| v.key() == k) {
                    self.hotkeys[i] = names::format_value(v);
                }
            }
            self.hotkeys_loaded = true;
        }
        self.learn_outcome(s);
        let sim_keyboard = self.shared.lock().unwrap().sim_keyboard.clone();
        let sim = sim_keyboard.is_some();
        if let Some(kb) = sim_keyboard {
            self.play_sim_keyboard(ui, &kb);
        }
        let cant_learn = cant_learn(s);
        ui.add_enabled_ui(!locked, |ui| {
            card(ui, &fl!("hotkeys"), |ui| {
                note(ui, "double:scrolllock · double:rctrl:noswallow · ctrl+alt+1 · f13 · none");
                note(ui, if sim { fl!("learn-hint-sim") } else { fl!("learn-hint") });
                // Notes go below the rows: in the grid they would widen the field's column and move the buttons.
                let mut notes = Vec::new();
                egui::Grid::new("hotkeys").num_columns(4).spacing([12.0, 8.0]).show(ui, |ui| {
                    for (i, (label, k)) in hotkeys().into_iter().enumerate() {
                        if k == key::HOTKEY_NEXT && !s.has_bluetooth() {
                            continue;
                        }
                        ui.label(&label);
                        let field = egui::vec2(220.0, 0.0);
                        let learning = self.learning.map(|(row, since)| (row == i, since));
                        if let Some((true, since)) = learning {
                            // The countdown fills the field.
                            let left = (LEARN_MS as u64 / 1000).saturating_sub(since.elapsed().as_secs());
                            ui.ctx().request_repaint_after(Duration::from_millis(250));
                            let mut countdown = fl!("press-the-hotkey", seconds = left);
                            ui.add(egui::TextEdit::singleline(&mut countdown).interactive(false).min_size(field));
                            ui.add_enabled(false, egui::Button::new(fl!("apply")));
                            if ui.button(fl!("cancel")).clicked() {
                                self.learning = None;
                                self.send(UiCmd::Learn(false));
                            }
                        } else {
                            let r = ui.add(egui::TextEdit::singleline(&mut self.hotkeys[i]).min_size(field));
                            let apply = ui.button(fl!("apply")).clicked()
                                || (r.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)));
                            if apply {
                                match names::parse_value(k, &self.hotkeys[i]) {
                                    Ok(v) => {
                                        self.hotkey_error[i] = None;
                                        self.learn_note = None;
                                        self.set(v);
                                    }
                                    Err(e) => self.hotkey_error[i] = Some(e),
                                }
                            }
                            let b = ui
                                .add_enabled(
                                    cant_learn.is_none() && learning.is_none(),
                                    egui::Button::new(fl!("learn")),
                                )
                                .on_hover_text(fl!("learn-hover"));
                            let why = cant_learn.clone().unwrap_or_else(|| fl!("learning-another"));
                            if b.on_disabled_hover_text(why).clicked() {
                                self.learn(i);
                            }
                        }
                        ui.end_row();
                        let row_note = match &self.learn_note {
                            Some((row, Ok(text))) if *row == i => Some((text.clone(), GOOD)),
                            Some((row, Err(text))) if *row == i => Some((text.clone(), WARN)),
                            _ => self.hotkey_error[i].clone().map(|e| (e, BAD)),
                        };
                        if let Some((text, color)) = row_note {
                            notes.push((format!("{label}: {text}"), color));
                        }
                    }
                });
                for (text, color) in notes {
                    ui.label(RichText::new(text).color(ink(ui, color)));
                }
                if s.ports.iter().any(|p| p.state == PortState::Passthrough && p.classes & class::KEYBOARD != 0) {
                    note(ui, fl!("passthrough-keyboard-note"));
                }
                let mut tap =
                    config(s, |v| if let ConfigValue::DoubleTapMs(m) = v { Some(*m) } else { None }).unwrap_or(300);
                if ui.add(egui::Slider::new(&mut tap, 100..=1000).text(fl!("double-tap-window"))).changed() {
                    self.set(ConfigValue::DoubleTapMs(tap));
                }
            });
            let tap = config(s, |v| if let ConfigValue::KeyTap(a) = v { Some(*a) } else { None });
            let long = config(s, |v| if let ConfigValue::KeyLong(a) = v { Some(*a) } else { None });
            if let (Some(tap), Some(long)) = (tap, long) {
                ui.add_space(8.0);
                card(ui, &fl!("kvm-key"), |ui| {
                    egui::Grid::new("key-actions").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        let rows = [
                            (fl!("tap"), tap, ConfigValue::KeyTap as fn(KeyAction) -> ConfigValue),
                            (fl!("long-press"), long, ConfigValue::KeyLong),
                        ];
                        for (k, (label, now, wrap)) in rows.into_iter().enumerate() {
                            ui.label(label);
                            let mut a = now;
                            egui::ComboBox::from_id_salt(("key-action", k))
                                .selected_text(names::key_action(a))
                                .width(230.0)
                                .show_ui(ui, |ui| {
                                    for k in KeyAction::ALL {
                                        // Cycling through Bluetooth hosts means nothing without them.
                                        if *k == KeyAction::NextTarget && !s.has_bluetooth() && now != *k {
                                            continue;
                                        }
                                        ui.selectable_value(&mut a, *k, names::key_action(*k));
                                    }
                                });
                            if a != now {
                                self.set(wrap(a));
                            }
                            ui.end_row();
                        }
                    });
                    let held = config(s, |v| if let ConfigValue::LongPressMs(ms) = v { Some(*ms) } else { None });
                    if let Some(ms) = held {
                        ui.add_space(4.0);
                        ui.label(fl!("held-for-long-press"));
                        let accent = accent_of(s);
                        if let Some(ms) = long_press_slider(ui, ms, accent) {
                            self.set(ConfigValue::LongPressMs(ms));
                        }
                    }
                    note(ui, fl!("kvm-key-note"));
                });
            }
            if s.info.caps & caps::VIAL != 0 {
                ui.add_space(8.0);
                card(ui, &fl!("keymap-vial"), |ui| {
                    note(ui, fl!("keymap-note"));
                    let mut mode = config(s, |v| if let ConfigValue::Vial(m) = v { Some(*m) } else { None })
                        .unwrap_or(VialMode::Off);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(fl!("vial-interface"));
                        let before = mode;
                        ui.radio_value(&mut mode, VialMode::Off, fl!("vial-off"));
                        ui.radio_value(&mut mode, VialMode::Auto, fl!("vial-auto"));
                        ui.radio_value(&mut mode, VialMode::On, fl!("vial-always"));
                        if mode != before {
                            self.set(ConfigValue::Vial(mode));
                        }
                    });
                    let mut km =
                        config(s, |v| if let ConfigValue::Keymap(b) = v { Some(*b) } else { None }).unwrap_or(true);
                    if ui.checkbox(&mut km, fl!("apply-keymap")).changed() {
                        self.set(ConfigValue::Keymap(km));
                    }
                });
            }
        });
    }

    /// Settings of this computer, not of the KVM: shown even without a KVM, and never locked by it.
    pub(crate) fn this_computer(&mut self, ui: &mut Ui) {
        card(ui, &fl!("this-computer"), |ui| {
            if autostart::supported() {
                let mut on = self.autostart;
                if ui.checkbox(&mut on, fl!("autostart")).changed() {
                    match autostart::set(on) {
                        Ok(()) => {
                            self.autostart = on;
                            self.autostart_error = None;
                        }
                        Err(e) => self.autostart_error = Some(fl!("could-not-change", error = e.to_string())),
                    }
                }
                if let Some(e) = &self.autostart_error {
                    ui.label(RichText::new(e).color(ink(ui, BAD)));
                }
            }
            ui.horizontal(|ui| {
                ui.label(fl!("appearance"));
                let before = self.local.theme;
                ui.selectable_value(&mut self.local.theme, local::Theme::System, fl!("theme-system"));
                ui.selectable_value(&mut self.local.theme, local::Theme::Dark, fl!("theme-dark"));
                ui.selectable_value(&mut self.local.theme, local::Theme::Light, fl!("theme-light"));
                if self.local.theme != before {
                    self.local_changed(ui.ctx());
                }
            });
            // The language of the window and of odkvmctl; "System" follows the system's.
            ui.horizontal(|ui| {
                ui.label(fl!("language"));
                let before = self.local.language.clone();
                let shown = |code: &Option<String>| match code {
                    None => fl!("language-system"),
                    Some(c) => i18n::LANGUAGES.iter().find(|(l, _)| l == c).map_or(c.clone(), |(_, n)| n.to_string()),
                };
                egui::ComboBox::from_id_salt("language").selected_text(shown(&self.local.language)).show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.local.language, None, fl!("language-system"));
                    for (code, name) in i18n::LANGUAGES {
                        ui.selectable_value(&mut self.local.language, Some(code.to_string()), *name);
                    }
                });
                if self.local.language != before {
                    i18n::init(self.local.language.as_deref());
                    self.local_changed(ui.ctx());
                }
            });
            if let Some(e) = &self.local_error {
                ui.label(RichText::new(e).color(ink(ui, BAD)));
            }
        });
    }

    pub(crate) fn settings(&mut self, ui: &mut Ui, s: Option<&Snapshot>) {
        self.this_computer(ui);
        let Some(s) = s else { return };
        ui.add_space(8.0);
        let locked = Self::locked_note(ui, s);
        let flags = s.status.flags;
        ui.add_enabled_ui(!locked, |ui| {
            card(ui, &fl!("leds"), |ui| {
                let mut b =
                    config(s, |v| if let ConfigValue::LedBrightness(b) = v { Some(*b) } else { None }).unwrap_or(64);
                if ui.add(egui::Slider::new(&mut b, 0..=255).text(fl!("brightness"))).changed() {
                    self.set(ConfigValue::LedBrightness(b));
                }
                ui.horizontal(|ui| {
                    let colors = pc_colors(s);
                    for (k, pc) in [Pc::Pc1, Pc::Pc2].into_iter().enumerate() {
                        ui.label(fl!("pc-colour", pc = pc.name().to_uppercase()));
                        let mut c = [colors[k].r, colors[k].g, colors[k].b];
                        if ui.color_edit_button_srgb(&mut c).changed() {
                            let rgb = Rgb::new(c[0], c[1], c[2]);
                            self.set(if pc == Pc::Pc1 {
                                ConfigValue::ColorPc1(rgb)
                            } else {
                                ConfigValue::ColorPc2(rgb)
                            });
                        }
                        ui.add_space(12.0);
                    }
                });
            });
            ui.add_space(8.0);
            card(ui, &fl!("pc-side"), |ui| {
                let mut polling = config(s, |v| if let ConfigValue::PcPolling(p) = v { Some(*p) } else { None })
                    .unwrap_or(Polling::Us125);
                ui.horizontal(|ui| {
                    ui.label(fl!("polling"));
                    let before = polling;
                    ui.radio_value(&mut polling, Polling::Us125, fl!("polling-125us"));
                    ui.radio_value(&mut polling, Polling::Ms1, fl!("polling-1ms"));
                    if polling != before {
                        self.set(ConfigValue::PcPolling(polling));
                    }
                });
                let mut pc = config(s, |v| if let ConfigValue::PowerCycleOnSwitch(b) = v { Some(*b) } else { None })
                    .unwrap_or(false);
                if ui.checkbox(&mut pc, fl!("power-cycle")).changed() {
                    self.set(ConfigValue::PowerCycleOnSwitch(pc));
                }
            });
            ui.add_space(8.0);
            card(ui, &fl!("standby"), |ui| {
                note(ui, fl!("standby-note"));
                let mut auto =
                    config(s, |v| if let ConfigValue::AutoSelect(b) = v { Some(*b) } else { None }).unwrap_or(true);
                if ui.checkbox(&mut auto, fl!("auto-select")).changed() {
                    self.set(ConfigValue::AutoSelect(auto));
                }
                let mut wake =
                    config(s, |v| if let ConfigValue::StandbyWake(b) = v { Some(*b) } else { None }).unwrap_or(true);
                if ui.checkbox(&mut wake, fl!("standby-wake")).changed() {
                    self.set(ConfigValue::StandbyWake(wake));
                }
                let mut glow =
                    config(s, |v| if let ConfigValue::StandbyGlow(b) = v { Some(*b) } else { None }).unwrap_or(false);
                if ui.checkbox(&mut glow, fl!("standby-glow")).changed() {
                    self.set(ConfigValue::StandbyGlow(glow));
                }
            });
        });
        ui.add_space(8.0);
        // Outside the locked part: turning configuration back on is asked for from here.
        self.software_control(ui, flags);
        ui.add_space(8.0);
        ui.add_enabled_ui(!locked, |ui| {
            ui.horizontal(|ui| {
                if ui.button(fl!("save-now")).clicked() {
                    self.send(UiCmd::Save);
                }
                if ui.button(fl!("factory-settings")).clicked() {
                    self.hotkeys_loaded = false;
                    self.send(UiCmd::FactoryReset);
                }
                if flags & status_flags::CONFIG_DIRTY != 0 {
                    note(ui, fl!("unsaved-changes"));
                }
            });
        });
    }

    /// The two software locks. Off goes through an "are you sure"; on asks the KVM, which waits for its key.
    fn software_control(&mut self, ui: &mut Ui, flags: u8) {
        let locked = flags & status_flags::SW_CONFIG_ALLOWED == 0;
        card(ui, &fl!("software-control"), |ui| {
            let seconds = UNLOCK_WAIT_MS / 1000;
            ui.label(RichText::new(fl!("software-control-note", seconds = seconds)).color(ink(ui, WARN)));
            let rows = [
                (
                    fl!("allow-switching"),
                    status_flags::SW_SWITCH_ALLOWED,
                    ConfigValue::SwSwitchAllowed as fn(bool) -> ConfigValue,
                ),
                (fl!("allow-config"), status_flags::SW_CONFIG_ALLOWED, ConfigValue::SwConfigAllowed),
            ];
            for (label, flag, value) in rows {
                let was = flags & flag != 0;
                let mut on = was;
                // With configuration locked, nothing else can be turned off either.
                let changeable = !was || !locked;
                if ui.add_enabled(changeable, egui::Checkbox::new(&mut on, label)).changed() {
                    self.unlock_note = None;
                    if on {
                        self.unlock_wait = Some((Instant::now(), flag));
                        self.send(UiCmd::Config(value(true)));
                    } else {
                        self.confirm_lock = Some(value(false));
                    }
                }
            }
            if let Some((since, flag)) = self.unlock_wait {
                let wait = Duration::from_millis(UNLOCK_WAIT_MS as u64);
                if flags & flag != 0 {
                    self.unlock_wait = None;
                } else if since.elapsed() > wait + Duration::from_secs(1) {
                    self.unlock_wait = None;
                    self.unlock_note = Some(fl!("not-turned-on"));
                } else {
                    let left = wait.saturating_sub(since.elapsed()).as_secs();
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(fl!("press-key-to-confirm", seconds = left)).strong());
                    });
                    ui.ctx().request_repaint_after(Duration::from_millis(250));
                }
            }
            if let Some(n) = &self.unlock_note {
                ui.label(RichText::new(n).color(ink(ui, BAD)));
            }
        });
        if let Some(v) = self.confirm_lock {
            let switching = matches!(v, ConfigValue::SwSwitchAllowed(_));
            let title = if switching { fl!("confirm-lock-switching") } else { fl!("confirm-lock-config") };
            let modal = egui::Modal::new(egui::Id::unique("confirm-lock")).show(ui.ctx(), |ui| {
                ui.set_max_width(440.0);
                ui.label(RichText::new(title).size(17.0).strong());
                ui.add_space(6.0);
                ui.label(names::lock_warning(switching));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let off =
                        egui::Button::new(RichText::new(fl!("turn-off")).color(Color32::WHITE).strong()).fill(BAD);
                    if ui.add(off).clicked() {
                        Some(true)
                    } else if ui.button(fl!("cancel")).clicked() {
                        Some(false)
                    } else {
                        None
                    }
                })
                .inner
            });
            match modal.inner {
                Some(true) => {
                    self.confirm_lock = None;
                    self.set(v);
                }
                Some(false) => self.confirm_lock = None,
                None if modal.should_close() => self.confirm_lock = None,
                None => {}
            }
        }
    }

    /// Without a KVM on a PC port the page still installs firmware: onto a board in its bootloader (`install`).
    pub(crate) fn firmware(&mut self, ui: &mut Ui, s: Option<&Snapshot>) {
        if let Some(s) = s {
            // The Bluetooth module, only where one is fitted: its firmware, or what it is doing instead.
            let bt_fw = match s.status.bt_module {
                diag::BtModule::Absent => None,
                diag::BtModule::Up => Some(names::version(s.info.bt_fw)),
                m => Some(names::bt_module(m).to_string()),
            };
            card(ui, &fl!("versions"), |ui| {
                egui::Grid::new("versions").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
                    let rows = [
                        Some(("RP2354B".to_string(), names::version(s.info.rp_fw))),
                        Some((
                            "AT32F405 #1 / #2".to_string(),
                            format!("{} / {}", names::version(s.info.at_fw[0]), names::version(s.info.at_fw[1])),
                        )),
                        bt_fw.map(|v| (fl!("wireless-module"), v)),
                        Some((fl!("hardware"), names::version(s.info.hw))),
                        Some((
                            fl!("serial"),
                            String::from_utf8_lossy(&s.info.serial).trim_end_matches('\0').to_string(),
                        )),
                        Some((fl!("uptime"), names::duration(s.status.uptime_s))),
                    ];
                    for (k, v) in rows.into_iter().flatten() {
                        note(ui, k);
                        ui.label(v);
                        ui.end_row();
                    }
                });
            });
            let gave_up = s.status.selftest & (selftest::AT32_GAVE_UP1 | selftest::AT32_GAVE_UP2);
            if gave_up != 0 {
                ui.add_space(8.0);
                card(ui, &fl!("diag-group-pc-chips"), |ui| {
                    ui.label(RichText::new(names::selftest(gave_up)).color(ink(ui, BAD)));
                    note(ui, fl!("gave-up-note-chip"));
                    if ui.button(fl!("try-again")).clicked() {
                        self.send(UiCmd::RetryAt32);
                    }
                });
            }
            self.bt_module_card(ui, s);
            ui.add_space(8.0);
        }
        self.install_cards(ui, s);
        if s.is_some() {
            ui.add_space(8.0);
            if ui.button(fl!("identify")).clicked() {
                self.send(UiCmd::Identify);
            }
            ui.add_space(8.0);
            self.diagnostics(ui);
        }
    }

    /// The KVM's measurements, each with its grade's colour; read again every [`DIAG_EVERY`] while shown.
    fn diagnostics(&mut self, ui: &mut Ui) {
        if self.diag_asked.is_none_or(|t| t.elapsed() >= DIAG_EVERY) {
            self.diag_asked = Some(Instant::now());
            self.send(UiCmd::Diagnose);
        }
        ui.ctx().request_repaint_after(DIAG_EVERY);
        let diag = self.shared.lock().unwrap().diag.clone();
        let weak = look::palette(ui).weak;
        card(ui, &fl!("diagnostics"), |ui| {
            let items = match &diag {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        note(ui, fl!("measuring"));
                    });
                    return;
                }
                Some((_, Err(e))) => {
                    ui.label(RichText::new(e).color(ink(ui, WARN)));
                    return;
                }
                Some((at, Ok(items))) => {
                    let count = |g| items.iter().filter(|i| i.grade == g).count();
                    ui.horizontal(|ui| {
                        let (warn, bad) = (count(diag::Grade::Warn), count(diag::Grade::Bad));
                        if bad != 0 {
                            chip(ui, &fl!("diag-bad", count = bad), BAD);
                        }
                        if warn != 0 {
                            chip(ui, &fl!("diag-warnings", count = warn), WARN);
                        }
                        if bad == 0 && warn == 0 {
                            chip(ui, &fl!("diag-all-good"), GOOD);
                        }
                        note(ui, fl!("diag-readings", count = items.len(), seconds = at.elapsed().as_secs()));
                        if ui.small_button(fl!("copy")).on_hover_text(fl!("copy-hover")).clicked() {
                            ui.ctx().copy_text(names::diag_text(items));
                        }
                    });
                    items
                }
            };
            ui.add_space(4.0);
            // System, clocks, the PC-side chips and Bluetooth on the left; USB and power on the right.
            let (left, right): (Vec<_>, Vec<_>) = names::diag_groups(items)
                .into_iter()
                .partition(|(g, _)| !matches!(g, Some(diag::Group::Usb | diag::Group::Power)));
            ui.columns(2, |cols| {
                diag_column(&mut cols[0], "diag-left", &left, weak);
                diag_column(&mut cols[1], "diag-right", &right, weak);
            });
        });
    }

    /// What the KVM pages show while no KVM answers; and where to go when one waits in its bootloader.
    pub(crate) fn no_kvm(&mut self, ui: &mut Ui) {
        card(ui, &fl!("looking-for-kvm"), |ui| {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(fl!("plug-in-kvm"));
            });
            if cfg!(target_os = "linux") {
                note(ui, fl!("udev-hint"));
            }
        });
        if !self.shared.lock().unwrap().boot.is_empty() {
            ui.add_space(8.0);
            card(ui, &fl!("boot-title"), |ui| {
                ui.label(fl!("boot-found-hint"));
                if ui.button(fl!("open-firmware-page")).clicked() {
                    self.page = crate::Page::Firmware;
                }
            });
        }
    }
}
