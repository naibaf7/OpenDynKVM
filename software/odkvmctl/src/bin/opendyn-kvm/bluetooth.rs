//! Bluetooth hosts in the window (KVMs with the Bluetooth module, boards from 0.16.0): the Bluetooth page, the
//! compact row on the Overview, and the small host buttons under PC1 | PC2 in the sidebar.
//!
//! Bluetooth hosts are lesser targets than the two PCs: nothing about them shows unless the KVM has the module, and
//! outside the Bluetooth page only the hosts that are paired show, smaller than the PCs and in one colour.

use eframe::egui::{self, Color32, RichText, Ui};
use kvm_core::leds::BT_VIOLET;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::ctrl::{BT_PAIR_SECONDS, BtPolicy, ConfigValue, ProfileName, selftest, status_flags};
use odkvmctl::client::{BtHostView, Snapshot};
use odkvmctl::{fl, names};

use crate::look::{self, BAD, GOOD, WARN, card, chip, ink, note};
use crate::worker::UiCmd;
use crate::{App, optimistic};

/// The Bluetooth hosts' colour, as the KVM's key shows it.
pub fn violet() -> Color32 {
    look::color32(BT_VIOLET)
}

fn state_text(h: &BtHostView) -> (String, Color32) {
    if h.ready() {
        (fl!("link-connected"), GOOD)
    } else if h.connected() {
        (fl!("host-connecting"), WARN)
    } else {
        (fl!("link-detached"), Color32::GRAY)
    }
}

impl App {
    /// Selects Bluetooth host `slot` right away in the window, then asks the KVM.
    pub(crate) fn switch_bt(&self, slot: u8) {
        if let Some(t) = kvm_proto::ctrl::Target::bt(slot) {
            self.switch_to(t);
        }
    }

    /// Under PC1 | PC2 in the sidebar: the paired hosts, two to a row, smaller than the PCs; nothing while none is
    /// paired. The sidebar's foot is laid out bottom-up, so the rows go in last first, and the gap to PC1 | PC2 last.
    pub(crate) fn sidebar_bt(&self, ui: &mut Ui, s: &Snapshot) {
        let hosts: Vec<&BtHostView> = s.bt_bonded().collect();
        if hosts.is_empty() {
            return;
        }
        let switchable = s.status.flags & status_flags::SW_SWITCH_ALLOWED != 0;
        let c = violet();
        let rows: Vec<&[&BtHostView]> = hosts.chunks(2).collect();
        for (k, row) in rows.iter().rev().enumerate() {
            if k > 0 {
                ui.add_space(2.0);
            }
            ui.horizontal(|ui| {
                let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                for h in *row {
                    let active = s.status.bt_active == h.slot;
                    let ink = if active { Color32::WHITE } else { look::ink_on(c, look::palette(ui).sidebar) };
                    let text = RichText::new(&h.name).color(ink);
                    let b = egui::Button::new(text)
                        .truncate()
                        .fill(if active { c } else { c.gamma_multiply(0.10) })
                        .stroke(egui::Stroke::new(1.0, if h.ready() || active { c } else { c.gamma_multiply(0.4) }))
                        .min_size(egui::vec2(width, 26.0));
                    let r = ui
                        .add_enabled(switchable || active, b)
                        .on_hover_text(fl!("host-hover", state = state_text(h).0));
                    if r.clicked() && !active {
                        self.switch_bt(h.slot);
                    }
                }
            });
        }
        ui.add_space(10.0);
    }

    /// The Overview's Bluetooth row: the paired hosts as small cards, below the two PCs. Nothing while none is paired.
    pub(crate) fn overview_bt(&self, ui: &mut Ui, s: &Snapshot) {
        let hosts: Vec<&BtHostView> = s.bt_bonded().collect();
        if hosts.is_empty() {
            return;
        }
        let switchable = s.status.flags & status_flags::SW_SWITCH_ALLOWED != 0;
        let c = violet();
        let p = look::palette(ui);
        ui.add_space(8.0);
        ui.horizontal_wrapped(|ui| {
            for h in hosts {
                let active = s.status.bt_active == h.slot;
                egui::Frame::new()
                    .fill(if active { c.gamma_multiply(0.16) } else { p.card })
                    .stroke(egui::Stroke::new(if active { 2.0 } else { 1.0 }, if active { c } else { p.card_stroke }))
                    .corner_radius(egui::CornerRadius::same(10))
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&h.name).strong().color(ink(ui, c)));
                            let (text, color) = state_text(h);
                            ui.label(RichText::new(text).small().color(ink(ui, color)));
                            if active {
                                chip(ui, &fl!("chip-active"), c);
                            } else {
                                let b = egui::Button::new(fl!("switch-here")).small();
                                if ui.add_enabled(switchable && h.bonded(), b).clicked() {
                                    self.switch_bt(h.slot);
                                }
                            }
                        });
                    });
            }
        });
    }

    /// What has been typed on the KVM's keyboard so far, `4 2 1 _ _ _`: the KVM takes the keyboard while a host waits
    /// for its passkey, so nothing shows anywhere else.
    fn passkey_typed(ui: &mut Ui, st: &kvm_proto::ctrl::Status) {
        // The digits typed so far, leading zeros included (the value of "042" is 42).
        let n = (st.bt_passkey_digits as usize).min(6);
        let typed = if n == 0 { String::new() } else { format!("{:0n$}", st.bt_passkey_typed) };
        let mut digits: Vec<String> = typed.chars().map(String::from).collect();
        digits.resize(6, "_".into());
        let text = fl!("passkey-typed", digits = digits.join(" "));
        ui.label(RichText::new(text).monospace().strong().color(ink(ui, violet())));
    }

    /// The six digits a pairing host shows, typed in the window instead of on the KVM's keyboard.
    fn passkey_field(&mut self, ui: &mut Ui, st: Option<&kvm_proto::ctrl::Status>) {
        if let Some(st) = st {
            Self::passkey_typed(ui, st);
        }
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut self.bt_passkey).desired_width(90.0).hint_text("123456"));
            let digits = self.bt_passkey.trim();
            let value =
                digits.parse::<u32>().ok().filter(|_| digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()));
            let go = ui.add_enabled(value.is_some(), egui::Button::new(fl!("send"))).clicked()
                || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && value.is_some());
            if go && let Some(v) = value {
                self.send(UiCmd::BtPasskey(v));
                self.bt_passkey.clear();
            }
            if ui.button(fl!("cancel-pairing")).clicked() {
                self.send(UiCmd::BtPair(false));
                self.bt_passkey.clear();
            }
        });
    }

    /// On every page but Bluetooth, while a pairing host waits for its passkey: the KVM's keyboard goes to nobody else
    /// until it is typed, which would otherwise look like a stuck keyboard.
    pub(crate) fn passkey_banner(&mut self, ui: &mut Ui, st: Option<&kvm_proto::ctrl::Status>) {
        let c = violet();
        egui::Frame::new()
            .fill(c.gamma_multiply(0.14))
            .stroke(egui::Stroke::new(1.0, c))
            .corner_radius(egui::CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 10))
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new(fl!("host-pairing")).strong().color(ink(ui, c)));
                ui.label(fl!("passkey-text"));
                self.passkey_field(ui, st);
            });
        ui.add_space(8.0);
    }

    /// While the module doesn't run its firmware: what it is doing, and for a failed one what the KVM does about it.
    /// On the Firmware page, and in place of the Bluetooth page's content. Nothing while it is up or not fitted.
    pub(crate) fn bt_module_card(&self, ui: &mut Ui, s: &Snapshot) {
        let m = s.status.bt_module;
        if matches!(m, BtModule::Absent | BtModule::Up) {
            return;
        }
        // An ESP32-C3 board on a front port (software.md §4.13): it comes back when plugged in again.
        let board = s.module_port();
        ui.add_space(8.0);
        card(ui, &fl!("wireless-module"), |ui| match m {
            BtModule::Starting | BtModule::Programming => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(fl!("module-is", state = names::bt_module(m))).color(ink(ui, WARN)));
                });
                note(ui, fl!("module-programming-note"));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            }
            BtModule::Failed => {
                ui.label(RichText::new(names::selftest(selftest::BT_FW)).color(ink(ui, BAD)));
                if board.is_some() {
                    // A board stays off until the KVM restarts, which *Try again* does.
                    note(ui, fl!("module-failed-note-board"));
                    if ui.button(fl!("try-again")).clicked() {
                        self.send(UiCmd::RetryBt);
                    }
                } else {
                    note(ui, fl!("module-failed-note"));
                }
            }
            _ => {
                ui.label(RichText::new(names::selftest(selftest::BT_GAVE_UP)).color(ink(ui, BAD)));
                note(ui, if board.is_some() { fl!("gave-up-note-board") } else { fl!("gave-up-note-module") });
                if ui.button(fl!("try-again")).clicked() {
                    self.send(UiCmd::RetryBt);
                }
            }
        });
    }

    /// The Bluetooth page: pairing, and each of the four host slots (name, state, policy, forget).
    pub(crate) fn bluetooth(&mut self, ui: &mut Ui, s: &Snapshot) {
        if let Some(p) = s.module_port() {
            note(ui, fl!("module-on-port", port = names::port_title(p)));
        }
        if !s.has_bluetooth() {
            // Fitted, but not up (starting, being updated, failed): no hosts to show.
            self.bt_module_card(ui, s);
            return;
        }
        let locked = Self::locked_note(ui, s);
        let st = &s.status;
        card(ui, &fl!("pair-new-host"), |ui| {
            note(ui, fl!("pair-note"));
            ui.horizontal(|ui| {
                if st.bt_pairing == 0 {
                    let free = s.bt.iter().any(|h| !h.bonded());
                    let b = ui.add_enabled(!locked && free, egui::Button::new(RichText::new(fl!("pair")).strong()));
                    if b.on_disabled_hover_text(fl!("slots-full")).clicked() {
                        self.send(UiCmd::BtPair(true));
                        if let Some(snap) = self.shared.lock().unwrap().snap.as_mut() {
                            snap.status.bt_pairing = BT_PAIR_SECONDS;
                        }
                    }
                } else {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
                    chip(ui, &fl!("discoverable", seconds = st.bt_pairing), violet());
                    // While a host waits for its passkey, "Cancel pairing" below does the same.
                    if st.bt_passkey == 0 && ui.button(fl!("stop")).clicked() {
                        self.send(UiCmd::BtPair(false));
                    }
                }
            });
            if st.bt_passkey != 0 {
                ui.add_space(4.0);
                ui.label(RichText::new(fl!("passkey-text")).color(ink(ui, WARN)));
                self.passkey_field(ui, Some(st));
            }
        });
        ui.add_space(8.0);
        card(ui, &fl!("hosts"), |ui| {
            note(ui, fl!("hosts-note"));
            ui.add_space(4.0);
            egui::Grid::new("bt-hosts").num_columns(5).spacing([12.0, 8.0]).show(ui, |ui| {
                for h in &s.bt {
                    let i = h.slot as usize - 1;
                    ui.label(RichText::new(format!("{}", h.slot)).strong().color(ink(ui, violet())));
                    if !h.bonded() {
                        ui.label(RichText::new(fl!("slot-free")).color(look::palette(ui).weak));
                        ui.label("");
                        ui.label("");
                        ui.label("");
                        ui.end_row();
                        continue;
                    }
                    // The name, applied when the field loses focus.
                    let field = &mut self.bt_names[i];
                    if field.is_none() {
                        *field = Some(h.name.clone());
                    }
                    let text = field.as_mut().unwrap();
                    let r = ui.add_enabled(
                        !locked,
                        egui::TextEdit::singleline(text).min_size(egui::vec2(130.0, 0.0)).char_limit(12),
                    );
                    if r.lost_focus() && text.trim() != h.name {
                        let name = text.trim().to_string();
                        let v = ConfigValue::BtName(i as u8, ProfileName::new(&name));
                        optimistic(&self.shared, v);
                        self.set(v);
                    }
                    let (state, color) = state_text(h);
                    let mut line = state;
                    if st.bt_active == h.slot {
                        line += &format!(" · {}", fl!("active"));
                    }
                    let r = ui.label(RichText::new(line).color(ink(ui, color)));
                    if h.connected() {
                        r.on_hover_text(link_text(h));
                    }
                    let mut policy = h.policy;
                    ui.add_enabled_ui(!locked, |ui| {
                        egui::ComboBox::from_id_salt(("bt-policy", h.slot))
                            .selected_text(policy_text(policy))
                            .width(170.0)
                            .show_ui(ui, |ui| {
                                for p in BtPolicy::ALL {
                                    ui.selectable_value(&mut policy, *p, policy_text(*p));
                                }
                            });
                    });
                    if policy != h.policy {
                        let v = ConfigValue::BtPolicy(i as u8, policy);
                        optimistic(&self.shared, v);
                        self.set(v);
                    }
                    let forget = ui
                        .add_enabled(!locked, egui::Button::new(RichText::new(fl!("forget")).color(ink(ui, BAD))))
                        .on_hover_text(fl!("forget-hover"));
                    if forget.clicked() {
                        self.send(UiCmd::BtForget(h.slot));
                        self.bt_names[i] = None;
                    }
                    ui.end_row();
                }
            });
        });
    }
}

/// A connected host's link, for its hover text: interval, latency, signal.
fn link_text(h: &BtHostView) -> String {
    let mut s = fl!("link-every", ms = odkvmctl::i18n::decimal(h.info.interval as f64 * 1.25, 2));
    if h.info.latency != 0 {
        s += &format!(", {}", fl!("link-may-skip", count = h.info.latency));
    }
    if h.info.rssi != 0 {
        s += &format!(" · {}", fl!("link-signal", dbm = h.info.rssi));
    }
    s
}

fn policy_text(p: BtPolicy) -> String {
    match p {
        BtPolicy::StayConnected => fl!("policy-stay"),
        BtPolicy::ConnectWhenSelected => fl!("policy-on-select"),
    }
}
