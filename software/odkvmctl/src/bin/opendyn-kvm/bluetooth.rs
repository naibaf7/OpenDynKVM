//! Bluetooth hosts in the window (KVMs with the Bluetooth module, boards from 0.16.0): the Bluetooth page, the
//! compact row on the Overview, and the small host buttons under PC1 | PC2 in the sidebar. The Bluetooth page also
//! pairs and lists the module's devices (keyboards, mice), whose modes and routings are on the Ports page.
//!
//! Bluetooth hosts are lesser targets than the two PCs: nothing about them shows unless the KVM has the module, and
//! outside the Bluetooth page only the hosts that are paired show, smaller than the PCs and in one colour.

use eframe::egui::{self, Color32, RichText, Ui};
use kvm_core::leds::BT_VIOLET;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::ctrl::{
    BT_PAIR_SECONDS, BtDeviceInfo, BtPolicy, ConfigValue, DevicePairing, FwState, ProfileName, status_flags,
};
use odkvmctl::client::{BtHostView, DeviceSlot, Snapshot};
use odkvmctl::time::Instant;
use odkvmctl::{fl, names};

use crate::look::{self, BAD, GOOD, WARN, card, chip, ink, note};
use crate::worker::UiCmd;
use crate::{App, optimistic};

/// The Bluetooth hosts' colour, as the KVM's key shows it.
pub fn violet() -> Color32 {
    look::color32(BT_VIOLET)
}

pub(crate) fn device_state_text(d: &BtDeviceInfo) -> (String, Color32) {
    if d.unreadable() {
        (fl!("device-unsupported"), BAD)
    } else if d.ready() {
        (fl!("link-connected"), GOOD)
    } else if d.connected() {
        (fl!("host-connecting"), WARN)
    } else {
        (fl!("device-away"), Color32::GRAY)
    }
}

fn device_name(d: &BtDeviceInfo) -> String {
    if d.name.is_empty() { names::device_kind(d.appearance) } else { d.name.to_string() }
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

    /// While the module doesn't run this firmware's version: what it is doing, and the update it needs. On the Firmware
    /// page, and in place of the Bluetooth page's content. Nothing while it is up or not fitted.
    pub(crate) fn bt_module_card(&mut self, ui: &mut Ui, s: &Snapshot) {
        let m = s.status.bt_module;
        if matches!(m, BtModule::Absent | BtModule::Up) {
            return;
        }
        let (update, result) = {
            let sh = self.shared.lock().unwrap();
            (sh.module_update, sh.module_result.clone())
        };
        ui.add_space(8.0);
        card(ui, &fl!("wireless-module"), |ui| {
            match m {
                BtModule::NeedsUpdate => {
                    ui.label(RichText::new(fl!("module-needs-update")).color(ink(ui, WARN)));
                    note(ui, fl!("module-needs-update-note"));
                    let busy = matches!(update, Some((st, ..)) if matches!(st, FwState::Receiving | FwState::Writing));
                    let module = self.fw.module(s.info.rp_fw, s.status.bt_chip);
                    if ui.add_enabled(!busy && module.is_ok(), egui::Button::new(fl!("update-module"))).clicked() {
                        self.send(UiCmd::UpdateModule(self.fw.path()));
                    }
                    if let Err(e) = module {
                        ui.label(RichText::new(e).color(ink(ui, WARN)));
                    }
                }
                _ => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(fl!("module-is", state = names::bt_module(m))).color(ink(ui, WARN)));
                    });
                    note(ui, fl!("module-programming-note"));
                }
            }
            if let Some((state, done, total)) = update
                && matches!(m, BtModule::Programming | BtModule::NeedsUpdate)
            {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(names::fw_state(state)));
            }
            if let Some(Err(e)) = result {
                ui.label(RichText::new(e).color(ink(ui, BAD)));
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
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
            egui::Grid::new("bt-hosts").num_columns(6).spacing([12.0, 8.0]).show(ui, |ui| {
                for h in &s.bt {
                    let i = h.slot as usize - 1;
                    ui.label(RichText::new(format!("{}", h.slot)).strong().color(ink(ui, violet())));
                    if !h.bonded() {
                        ui.label(RichText::new(fl!("slot-free")).color(look::palette(ui).weak));
                        ui.label("");
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
                    // Whether its software may control the KVM over the module: on only with a tap on the KVM's key.
                    let mut allow = h.may_control;
                    let mut label = fl!("host-may-control-check");
                    if h.may_control && h.in_session() {
                        label += &format!(" · {}", fl!("host-controls-now"));
                    }
                    let r = ui
                        .add_enabled(!locked, egui::Checkbox::new(&mut allow, label))
                        .on_hover_text(fl!("host-may-control-hover"));
                    if r.changed() {
                        let v = ConfigValue::BtControl(i as u8, allow);
                        if allow {
                            self.bt_allow_asked = Some((h.slot, Instant::now()));
                        } else {
                            optimistic(&self.shared, v);
                        }
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
            // A host just allowed here: the KVM waits for its key (it shows allowed once tapped).
            let wait = kvm_core::engine::UNLOCK_WAIT_MS as f32 / 1000.0;
            if let Some((slot, at)) = self.bt_allow_asked {
                let done = s.bt.iter().any(|h| h.slot == slot && h.may_control);
                if done || at.elapsed().as_secs_f32() > wait {
                    self.bt_allow_asked = None;
                } else {
                    ui.add_space(4.0);
                    let seconds = kvm_core::engine::UNLOCK_WAIT_MS / 1000;
                    ui.label(RichText::new(fl!("cli-press-key-within", seconds = seconds)).color(ink(ui, WARN)));
                }
            }
        });
        // The devices after the hosts: each kind's pairing card above its slots.
        ui.add_space(8.0);
        self.bt_devices(ui, s, locked);
        // Last: erasing every pairing, for state the module shouldn't hold. Two presses.
        ui.add_space(8.0);
        card(ui, &fl!("bt-erase-title"), |ui| {
            note(ui, fl!("bt-erase-note"));
            ui.add_space(4.0);
            ui.add_enabled_ui(!locked, |ui| {
                if !self.bt_erase_armed {
                    if ui.button(fl!("bt-erase-button")).clicked() {
                        self.bt_erase_armed = true;
                    }
                    return;
                }
                ui.label(RichText::new(fl!("bt-erase-really")).color(ink(ui, BAD)));
                ui.horizontal(|ui| {
                    let erase = egui::Button::new(RichText::new(fl!("bt-erase-confirm")).color(ink(ui, BAD)));
                    if ui.add(erase).clicked() {
                        self.send(UiCmd::BtEraseAll);
                        self.bt_erase_armed = false;
                        self.bt_names = Default::default();
                    }
                    if ui.button(fl!("cancel")).clicked() {
                        self.bt_erase_armed = false;
                    }
                });
            });
        });
    }
}

impl App {
    /// Pairing a device (looking for one, the ones found, how far its pairing got, its passkey), and the device slots.
    fn bt_devices(&mut self, ui: &mut Ui, s: &Snapshot, locked: bool) {
        let st = &s.status;
        let free = s.devices.iter().any(|d| !d.bonded());
        let pairing =
            matches!(st.bt_device_pairing, DevicePairing::Connecting | DevicePairing::Pairing | DevicePairing::Reading);
        card(ui, &fl!("pair-new-device"), |ui| {
            note(ui, fl!("pair-device-note"));
            ui.horizontal(|ui| {
                if st.bt_scan == 0 {
                    let b = egui::Button::new(RichText::new(fl!("look-for-devices")).strong());
                    let b = ui.add_enabled(!locked && free && !pairing, b);
                    if b.on_disabled_hover_text(fl!("device-slots-full")).clicked() {
                        self.send(UiCmd::BtScan(true));
                        if let Some(snap) = self.shared.lock().unwrap().snap.as_mut() {
                            snap.status.bt_scan = BT_PAIR_SECONDS;
                            snap.found.clear();
                        }
                    }
                } else {
                    chip(ui, &fl!("looking", seconds = st.bt_scan), violet());
                    if ui.button(fl!("stop")).clicked() {
                        self.send(UiCmd::BtScan(false));
                    }
                }
            });
            if st.bt_scan != 0 || pairing {
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            }
            if st.bt_scan != 0 && s.found.is_empty() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    note(ui, fl!("found-none"));
                });
            }
            if !s.found.is_empty() && st.bt_scan != 0 {
                ui.add_space(4.0);
                egui::Grid::new("bt-found").num_columns(4).spacing([12.0, 6.0]).show(ui, |ui| {
                    for f in &s.found {
                        let kind = names::device_kind(f.appearance);
                        let name = if f.name.is_empty() { kind.clone() } else { f.name.to_string() };
                        ui.label(RichText::new(name).strong());
                        ui.label(kind);
                        ui.label(RichText::new(fl!("link-signal", dbm = f.rssi)).small());
                        let b = ui.add_enabled(!locked && !pairing, egui::Button::new(fl!("pair")));
                        if b.clicked() {
                            self.send(UiCmd::BtPairDevice(f.addr));
                            if let Some(snap) = self.shared.lock().unwrap().snap.as_mut() {
                                snap.status.bt_scan = 0;
                                snap.status.bt_device_pairing = DevicePairing::Connecting;
                            }
                        }
                        ui.end_row();
                    }
                });
            }
            if st.bt_device_pairing != DevicePairing::Idle {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if pairing {
                        ui.spinner();
                    }
                    let color = match st.bt_device_pairing {
                        DevicePairing::Paired => GOOD,
                        DevicePairing::Failed => BAD,
                        _ => WARN,
                    };
                    ui.label(RichText::new(names::device_pairing(st.bt_device_pairing)).color(ink(ui, color)));
                });
            }
            if let Some(v) = st.bt_device_passkey.filter(|_| st.bt_device_pairing == DevicePairing::Pairing) {
                let text = fl!("device-passkey", passkey = format!("{v:06}"));
                ui.label(RichText::new(text).monospace().size(18.0).strong().color(ink(ui, violet())));
            }
        });
        ui.add_space(8.0);
        card(ui, &fl!("devices"), |ui| {
            note(ui, fl!("devices-note"));
            ui.add_space(4.0);
            egui::Grid::new("bt-devices").num_columns(4).spacing([12.0, 8.0]).show(ui, |ui| {
                for d in &s.devices {
                    ui.label(RichText::new(format!("{}", d.slot)).strong().color(ink(ui, violet())));
                    if !d.bonded() {
                        ui.label(RichText::new(fl!("slot-free")).color(look::palette(ui).weak));
                        ui.label("");
                        ui.label("");
                        ui.end_row();
                        continue;
                    }
                    ui.vertical(|ui| {
                        ui.set_width(200.0);
                        ui.add(egui::Label::new(RichText::new(device_name(d)).strong()).truncate());
                        note(ui, names::device_kind(d.appearance));
                    });
                    let (state, color) = device_state_text(d);
                    let mut line = state;
                    if d.battery <= 100 {
                        line += &format!(" · {}", fl!("battery", percent = d.battery));
                    }
                    let r = ui.label(RichText::new(line).color(ink(ui, color)));
                    if d.rssi != 0 {
                        r.on_hover_text(fl!("link-signal", dbm = d.rssi));
                    }
                    let forget = ui
                        .add_enabled(!locked, egui::Button::new(RichText::new(fl!("forget")).color(ink(ui, BAD))))
                        .on_hover_text(fl!("forget-device-hover"));
                    if forget.clicked() {
                        self.send(UiCmd::BtForgetDevice(d.slot));
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
