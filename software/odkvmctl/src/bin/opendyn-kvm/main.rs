//! `opendyn-kvm`: window and tray icon for the OpenDynKVM (software.md §9).
//!
//! Closing the window keeps the tray icon; *Quit* in the tray menu ends the program. `--tray` starts with the tray
//! icon only (how login starts it, `odkvmctl::autostart`). One copy runs per user: starting another opens the running
//! copy's window. The look follows the Creative App (`look.rs`): a sidebar with the pages and quick controls, the
//! content in cards, dark or light.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod bluetooth;
mod displays;
mod install;
mod look;
mod pages;
mod screens;
mod single;
mod tray;
mod worker;

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, Frame, Margin, RichText};
use kvm_proto::ctrl::{ConfigValue, MAX_PROFILES, Pc, Port, Rgb, Target, status_flags};
use odkvmctl::client::Snapshot;
use odkvmctl::local::{self, Local};
use odkvmctl::{autostart, fl, icon};

use look::{BAD, GOOD, WARN, chip, ink};
use tray::{Tray, TrayAction, TrayState};
use worker::{Shared, UiCmd, Wake};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Page {
    Overview,
    Ports,
    Displays,
    Keyboard,
    Settings,
    Firmware,
    /// Only for a KVM with the Bluetooth module.
    Bluetooth,
    /// Screen hop; only for firmware that has it.
    Screens,
}

impl Page {
    const ALL: [Page; 8] = [
        Page::Overview,
        Page::Ports,
        Page::Bluetooth,
        Page::Displays,
        Page::Screens,
        Page::Keyboard,
        Page::Settings,
        Page::Firmware,
    ];

    /// Shown in the sidebar: the Bluetooth page only for a KVM with the module fitted (up, or starting, being
    /// updated or failed: then the page says so), the Screens page only for firmware with screen hop.
    fn shown(self, s: Option<&Snapshot>) -> bool {
        match self {
            Page::Bluetooth => s.is_some_and(|s| s.status.bt_module != kvm_proto::ctrl::diag::BtModule::Absent),
            Page::Screens => s.is_some_and(|s| s.has_hop()),
            _ => true,
        }
    }

    fn title(self) -> String {
        match self {
            Page::Overview => fl!("page-overview"),
            Page::Ports => fl!("page-ports"),
            Page::Displays => fl!("page-displays"),
            Page::Keyboard => fl!("page-keyboard"),
            Page::Settings => fl!("page-settings"),
            Page::Firmware => fl!("page-firmware"),
            Page::Bluetooth => fl!("page-wireless"),
            Page::Screens => fl!("page-screens"),
        }
    }

    /// Icon and its colour, as the Creative App marks its sections.
    fn icon(self) -> (&'static str, Color32) {
        match self {
            Page::Overview => ("🏠", Color32::from_rgb(76, 154, 255)),
            Page::Ports => ("🔌", Color32::from_rgb(245, 166, 35)),
            Page::Displays => ("🖥", Color32::from_rgb(46, 204, 113)),
            Page::Keyboard => ("⌨", Color32::from_rgb(176, 124, 255)),
            Page::Settings => ("⚙", Color32::from_rgb(154, 165, 177)),
            Page::Firmware => ("⬆", Color32::from_rgb(255, 107, 107)),
            Page::Bluetooth => ("📶", bluetooth::violet()),
            Page::Screens => ("🖱", Color32::from_rgb(26, 188, 156)),
        }
    }
}

struct App {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<UiCmd>,
    tray_rx: Receiver<TrayAction>,
    tray: Option<Tray>,
    tray_state: Option<TrayState>,
    page: Page,
    hotkeys: [String; 4],
    hotkey_error: [Option<String>; 4],
    /// The Bluetooth page: a passkey typed in the window, and the host names being edited.
    bt_passkey: String,
    bt_names: [Option<String>; 4],
    hotkeys_loaded: bool,
    /// The hotkey row being learned from the KVM's keyboard, since when; how learning one last ended (Err: not learned).
    learning: Option<(usize, std::time::Instant)>,
    learn_note: Option<(usize, Result<String, String>)>,
    /// With the simulator, while learning: the modifiers and keys its keyboard holds, as played in the window.
    sim_keys: Option<(u8, Vec<u8>)>,
    /// Profile names as typed, and the one being edited (the others follow the KVM).
    profile_names: [String; MAX_PROFILES],
    profile_editing: Option<usize>,
    /// A software lock waiting for "are you sure" before it is sent.
    confirm_lock: Option<ConfigValue>,
    /// Allowing software control again: since when the KVM waits for its key, and the status flag that says it's done.
    unlock_wait: Option<(std::time::Instant, u8)>,
    unlock_note: Option<String>,
    /// The firmware file the Firmware page installs.
    fw: install::Files,
    /// When the Firmware page last asked for the diagnostics.
    diag_asked: Option<std::time::Instant>,
    quitting: bool,
    /// Close this window and open a new one with this activation token (Wayland: the only way to raise it).
    reopen: Option<String>,
    /// Start at login (a setting of this computer); the last error setting it.
    autostart: bool,
    autostart_error: Option<String>,
    /// Appearance and monitor bindings (`odkvmctl::local`), and the last error saving them.
    local: Local,
    local_error: Option<String>,
    displays: displays::Displays,
    /// The Screens page's selection and drag.
    screens: screens::Editor,
    wake: Wake,
    /// The accent and theme the window's style was last set for.
    styled: Option<(Color32, local::Theme)>,
    /// The sidebar's icon and wordmark, and the window (context) and display scale they were drawn for: a window opened
    /// again is a new context, without the old one's textures.
    logo: Option<(egui::Context, f32, egui::TextureHandle, egui::TextureHandle)>,
}

fn config<T>(s: &Snapshot, f: impl Fn(&ConfigValue) -> Option<T>) -> Option<T> {
    s.config.iter().find_map(f)
}

pub(crate) fn pc_colors(s: &Snapshot) -> [Rgb; 2] {
    [
        config(s, |v| if let ConfigValue::ColorPc1(c) = v { Some(*c) } else { None }).unwrap_or_default(),
        config(s, |v| if let ConfigValue::ColorPc2(c) = v { Some(*c) } else { None }).unwrap_or_default(),
    ]
}

/// Applies a change to the local copy right away, so the UI doesn't snap back until the next refresh.
pub(crate) fn optimistic(shared: &Mutex<Shared>, v: ConfigValue) {
    fn put(config: &mut [ConfigValue], v: ConfigValue) {
        if let Some(slot) = config.iter_mut().find(|c| c.key() == v.key()) {
            *slot = v;
        }
    }
    let mut sh = shared.lock().unwrap();
    let Some(s) = sh.snap.as_mut() else { return };
    put(&mut s.config, v);
    let Some((active, count, list)) = s.profiles() else {
        if let ConfigValue::PortMode(p, m) = v {
            s.ports[p.index()].mode = m;
        }
        return;
    };
    // The port modes are the active profile's.
    let mut modes = list[active as usize].modes;
    if let ConfigValue::PortMode(p, m) = v {
        modes[p.index()] = m;
        put(&mut s.config, ConfigValue::ProfileModes(active, modes));
    }
    if active >= count {
        put(&mut s.config, ConfigValue::Profile(0));
        modes = list[0].modes;
    }
    for p in Port::ALL {
        s.ports[p.index()].mode = modes[p.index()];
        put(&mut s.config, ConfigValue::PortMode(*p, modes[p.index()]));
    }
}

/// The active target's colour: the active PC's, or violet while a Bluetooth host has the input.
fn accent_of(s: &Snapshot) -> Color32 {
    if s.status.bt_active != 0 { bluetooth::violet() } else { look::color32(pc_colors(s)[s.status.active.index()]) }
}

fn switch_target(pc: Pc) -> Target {
    if pc == Pc::Pc1 { Target::Pc1 } else { Target::Pc2 }
}

/// The sidebar's icon and wordmark, in points: the wordmark's height takes in its descenders.
const LOGO_ICON: f32 = 28.0;
const LOGO_WORDMARK: f32 = 15.0;

/// Windows hides the window on close and keeps it; Wayland can't hide windows, so on Linux the window really closes
/// and the tray's *Open* makes a new one (see `main`).
const HIDE_ON_CLOSE: bool = cfg!(windows);

impl App {
    fn new(shared: Arc<Mutex<Shared>>, tx: Sender<UiCmd>, tray_rx: Receiver<TrayAction>, wake: Wake) -> App {
        App {
            shared,
            tx,
            tray_rx,
            tray: None,
            tray_state: None,
            page: Page::Overview,
            hotkeys: Default::default(),
            hotkey_error: Default::default(),
            bt_passkey: String::new(),
            bt_names: Default::default(),
            hotkeys_loaded: false,
            learning: None,
            learn_note: None,
            sim_keys: None,
            profile_names: Default::default(),
            profile_editing: None,
            confirm_lock: None,
            unlock_wait: None,
            unlock_note: None,
            fw: install::Files::new(),
            diag_asked: None,
            quitting: false,
            reopen: None,
            autostart: autostart::is_enabled(),
            autostart_error: None,
            local: local::load(),
            local_error: None,
            displays: displays::Displays::default(),
            screens: screens::Editor::default(),
            wake,
            styled: None,
            logo: None,
        }
    }

    fn send(&self, cmd: UiCmd) {
        let _ = self.tx.send(cmd);
    }

    fn set(&self, v: ConfigValue) {
        optimistic(&self.shared, v);
        self.send(UiCmd::Config(v));
    }

    /// This computer's settings changed: save them, restyle.
    fn local_changed(&mut self, ctx: &egui::Context) {
        self.local_error = displays::save(&self.local);
        self.styled = None;
        ctx.request_repaint();
    }

    /// Keeps the tray icon in the active target's colour, and its menu's Bluetooth hosts current.
    fn update_tray(&mut self) {
        let Some(t) = self.tray.as_mut() else { return };
        let state = {
            let sh = self.shared.lock().unwrap();
            sh.snap.as_ref().map_or_else(
                || TrayState { language: odkvmctl::i18n::current(), ..Default::default() },
                |s| TrayState {
                    active: Some(s.status.active),
                    colors: pc_colors(s),
                    hosts: s.bt_bonded().map(|h| (h.slot, h.name.clone())).collect(),
                    bt_active: s.status.bt_active,
                    language: odkvmctl::i18n::current(),
                },
            )
        };
        if self.tray_state.as_ref() != Some(&state) {
            t.update(&state);
            self.tray_state = Some(state);
        }
    }

    /// The active target's colour, or a neutral blue without a KVM.
    fn accent(&self) -> Color32 {
        let sh = self.shared.lock().unwrap();
        sh.snap.as_ref().map_or(look::NEUTRAL, accent_of)
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(a) = self.tray_rx.try_recv() {
            match a {
                TrayAction::Switch(t) => self.switch_to(t),
                TrayAction::Open(Some(token)) if cfg!(target_os = "linux") => {
                    self.reopen = Some(token);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                TrayAction::Open(_) => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    // Wayland ignores Focus without an activation token; this at least flags the taskbar entry.
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                        egui::UserAttentionType::Informational,
                    ));
                }
                TrayAction::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if HIDE_ON_CLOSE && ctx.input(|i| i.viewport().close_requested()) && self.tray.is_some() && !self.quitting {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.update_tray();
        let want = (self.accent(), self.local.theme);
        if self.styled != Some(want) {
            look::apply(ctx, want.0);
            ctx.set_theme(look::preference(want.1));
            self.styled = Some(want);
        }
    }

    /// The logo's icon and wordmark as textures, and the wordmark's aspect ratio; drawn again for a new window and when
    /// the display scale changes.
    fn logo(&mut self, ctx: &egui::Context) -> (egui::TextureId, egui::TextureId, f32) {
        let ppp = ctx.pixels_per_point();
        if self.logo.as_ref().is_none_or(|l| l.0 != *ctx || l.1 != ppp) {
            let texture = |name: &str, (w, h, rgba): (usize, usize, Vec<u8>)| {
                let image = egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba);
                ctx.load_texture(name, image, egui::TextureOptions::LINEAR)
            };
            let px = (LOGO_ICON * ppp).round() as usize;
            let icon = texture("logo-icon", (px, px, icon::icon_rgba(px, true)));
            let word = texture("logo-wordmark", icon::wordmark_rgba((LOGO_WORDMARK * ppp).round() as usize));
            self.logo = Some((ctx.clone(), ppp, icon, word));
        }
        let (_, _, icon, word) = self.logo.as_ref().unwrap();
        (icon.id(), word.id(), word.aspect_ratio())
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, snap: Option<&Snapshot>) {
        let accent = self.accent();
        ui.add_space(6.0);
        let (icon, word, aspect) = self.logo(ui.ctx());
        ui.horizontal(|ui| {
            ui.add(egui::Image::new((icon, egui::vec2(LOGO_ICON, LOGO_ICON))));
            let size = egui::vec2(LOGO_WORDMARK * aspect, LOGO_WORDMARK);
            ui.add(egui::Image::new((word, size)).tint(ui.visuals().strong_text_color()));
        });
        ui.add_space(14.0);
        if !self.page.shown(snap) {
            self.page = Page::Overview;
        }
        for page in Page::ALL.into_iter().filter(|p| p.shown(snap)) {
            let (glyph, color) = page.icon();
            if look::nav_item(ui, glyph, color, &page.title(), self.page == page, accent) {
                self.page = page;
            }
        }
        // Quick controls at the foot, as the Creative App keeps its output switch and volume there.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(4.0);
            let dark = ui.visuals().dark_mode;
            let label =
                if dark { format!("☀  {}", fl!("light-mode")) } else { format!("🌙  {}", fl!("dark-mode")) };
            if ui.add(egui::Button::new(label).frame(false)).clicked() {
                self.local.theme = if dark { local::Theme::Light } else { local::Theme::Dark };
                self.local_changed(ui.ctx());
            }
            let Some(s) = snap else {
                look::note(ui, fl!("no-kvm-connected"));
                return;
            };
            let locked = s.status.flags & status_flags::SW_CONFIG_ALLOWED == 0;
            let mut b =
                config(s, |v| if let ConfigValue::LedBrightness(b) = v { Some(*b) } else { None }).unwrap_or(64);
            ui.add_enabled_ui(!locked, |ui| {
                ui.spacing_mut().slider_width = ui.available_width() - 50.0;
                if ui.add(egui::Slider::new(&mut b, 0..=255).show_value(false).text(fl!("leds"))).changed() {
                    self.set(ConfigValue::LedBrightness(b));
                }
            });
            ui.add_space(4.0);
            // The paired Bluetooth hosts, small, under PC1 | PC2 (bottom-up: added before what sits above them).
            self.sidebar_bt(ui, s);
            // PC1 | PC2: the switch, coloured like the KVM's LEDs.
            let colors = pc_colors(s);
            let switchable = s.status.flags & status_flags::SW_SWITCH_ALLOWED != 0;
            // `columns` would lay out downwards from the bottom-up cursor, below the panel.
            ui.horizontal(|ui| {
                let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                for (k, pc) in [Pc::Pc1, Pc::Pc2].into_iter().enumerate() {
                    let active = s.status.active == pc && s.status.bt_active == 0;
                    let c = look::color32(colors[k]);
                    let ink = if active { Color32::WHITE } else { look::ink_on(c, look::palette(ui).sidebar) };
                    let text = RichText::new(pc.name().to_uppercase()).strong().color(ink);
                    let button = egui::Button::new(text)
                        .fill(if active { c } else { c.gamma_multiply(0.12) })
                        .stroke(egui::Stroke::new(1.0, c))
                        .min_size(egui::vec2(width, 34.0));
                    if ui.add_enabled(switchable || active, button).clicked() && !active {
                        self.switch(pc);
                    }
                }
            });
            ui.label(RichText::new(fl!("active-pc")).strong());
            if let Some((active, count, list)) = s.profiles() {
                ui.add_space(8.0);
                let mut pick = active;
                ui.add_enabled_ui(!locked, |ui| {
                    egui::ComboBox::from_id_salt("profile")
                        .selected_text(&list[active as usize].name)
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for (i, p) in list.iter().enumerate().take(count as usize) {
                                ui.selectable_value(&mut pick, i as u8, &p.name);
                            }
                        });
                });
                if pick != active {
                    self.set(ConfigValue::Profile(pick));
                }
                ui.label(RichText::new(fl!("profile")).strong());
            }
        });
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let (snap, device, error) = {
            let sh = self.shared.lock().unwrap();
            (sh.snap.clone(), sh.device.clone(), sh.error.clone())
        };
        let palette = look::palette(ui);
        egui::Panel::left("nav")
            .exact_size(212.0)
            .resizable(false)
            .frame(Frame::new().fill(palette.sidebar).inner_margin(Margin::same(12)))
            .show(ui, |ui| self.sidebar(ui, snap.as_ref()));
        let content = Frame::new().fill(ui.visuals().panel_fill).inner_margin(Margin {
            left: 24,
            right: 24,
            top: 18,
            bottom: 12,
        });
        egui::CentralPanel::no_frame().frame(content).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.page.title()).size(24.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match (&device, &snap) {
                    (Some(d), Some(_)) => chip(ui, &fl!("connected-to", device = d.as_str()), GOOD),
                    (Some(_), None) => chip(ui, &fl!("reading"), WARN),
                    (None, _) => chip(ui, &fl!("not-connected"), WARN),
                });
            });
            if let Some(e) = &error {
                ui.label(RichText::new(e).color(ink(ui, BAD)));
            }
            ui.add_space(8.0);
            if snap.as_ref().is_some_and(|s| s.status.bt_passkey != 0) && self.page != Page::Bluetooth {
                self.passkey_banner(ui);
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match (self.page, snap.as_ref()) {
                (Page::Displays, s) => {
                    let this_pc = s.and_then(|s| s.status.asking);
                    let profiles = displays::profile_rows(s);
                    let hosts = displays::host_rows(s);
                    let wake = self.wake.clone();
                    if self.displays.ui(ui, &mut self.local, this_pc, &profiles, &hosts, &wake) {
                        self.local_changed(ui.ctx());
                    }
                }
                (Page::Settings, s) => self.settings(ui, s),
                (Page::Firmware, s) => self.firmware(ui, s),
                (_, None) => self.no_kvm(ui),
                (Page::Overview, Some(s)) => self.overview(ui, s),
                (Page::Ports, Some(s)) => self.ports(ui, s),
                (Page::Keyboard, Some(s)) => self.keyboard(ui, s),
                (Page::Bluetooth, Some(s)) => self.bluetooth(ui, s),
                (Page::Screens, Some(s)) => self.screens(ui, s),
            });
        });
    }
}

/// What eframe runs for one window; the [`App`] outlives it.
struct Window<'a>(&'a mut App);

impl eframe::App for Window<'_> {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.0.logic(ctx, frame);
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.0.ui(ui, frame);
    }
}

fn options(sim: bool, visible: bool) -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(if sim { fl!("window-title-sim") } else { fl!("product-name") })
            .with_app_id("opendyn-kvm")
            .with_visible(visible)
            .with_icon(egui::IconData { rgba: icon::icon_rgba(64, true), width: 64, height: 64 })
            .with_inner_size([900.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
        // eframe keeps its event loop, so windows can close and open again in one process.
        run_and_return: true,
        ..Default::default()
    }
}

fn main() -> eframe::Result {
    // The language set on this computer (Settings), or the system's.
    odkvmctl::i18n::init(local::load().language.as_deref());
    let sim = std::env::args().any(|a| a == "--sim");
    let in_tray = std::env::args().any(|a| a == autostart::TRAY_ARG);
    // A start at login while a copy already runs just leaves; any other start opens that copy's window.
    let listener = match single::claim(sim, !in_tray) {
        single::Claim::Running => return Ok(()),
        single::Claim::First(l) => l,
    };
    // The worker and the tray outlive windows; the wake-up goes to whichever window is open.
    let window_ctx: Arc<Mutex<Option<egui::Context>>> = Arc::default();
    let wake: Wake = {
        let slot = window_ctx.clone();
        Arc::new(move || {
            if let Some(ctx) = slot.lock().unwrap().as_ref() {
                ctx.request_repaint();
            }
        })
    };
    let shared = Arc::new(Mutex::new(Shared::default()));
    let (tx, rx) = channel();
    worker::spawn(sim, true, shared.clone(), rx, wake.clone());
    worker::spawn_boot_scan(sim, shared.clone(), wake.clone());
    let (tray_tx, tray_rx) = channel();
    let mut app = App::new(shared, tx, tray_rx, wake.clone());
    if let Some(l) = listener {
        single::listen(l, tray_tx.clone(), wake.clone());
    }
    // Linux: the tray runs on its own D-Bus thread. Windows: it must be created on the window's event-loop thread,
    // which happens with the first window below.
    let mut tray_tx = Some(tray_tx);
    if !cfg!(windows) {
        app.tray = Tray::new(tray_tx.take().unwrap(), wake.clone());
    }

    // In the tray from the start: on Linux no window until Open; on Windows the window exists (its event loop runs the
    // tray icon) but stays hidden. Without a tray there is nothing to start into, so the window shows.
    let mut window = !(in_tray && (cfg!(windows) || app.tray.is_some()));
    loop {
        if window {
            let (slot, wake_w, tray_tx_w) = (window_ctx.clone(), wake.clone(), tray_tx.take());
            let hidden = in_tray && cfg!(windows) && tray_tx_w.is_some();
            let app_ref = &mut app;
            // A new window gets its style set on its first frame.
            app_ref.styled = None;
            eframe::run_native(
                "opendyn-kvm",
                options(sim, !hidden),
                Box::new(move |cc| {
                    *slot.lock().unwrap() = Some(cc.egui_ctx.clone());
                    if let Some(t) = tray_tx_w {
                        app_ref.tray = Tray::new(t, wake_w);
                    }
                    if hidden && app_ref.tray.is_none() {
                        cc.egui_ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    }
                    Ok(Box::new(Window(app_ref)))
                }),
            )?;
            *window_ctx.lock().unwrap() = None;
        }
        window = true;
        if app.quitting || app.tray.is_none() {
            return Ok(());
        }
        // The window is closed but the tray stays: wait for Open or Quit, keeping the icon current.
        let token = match app.reopen.take() {
            Some(t) => Some(t),
            None => loop {
                match app.tray_rx.recv_timeout(std::time::Duration::from_millis(300)) {
                    Ok(TrayAction::Open(t)) => break t,
                    Ok(TrayAction::Quit) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
                    Ok(TrayAction::Switch(t)) => app.switch_to(t),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                }
                app.update_tray();
            },
        };
        if let Some(t) = token {
            // egui-winit hands XDG_ACTIVATION_TOKEN to the new window (and clears it again).
            // SAFETY: the other threads (worker, D-Bus) don't read the environment after start-up.
            unsafe { std::env::set_var("XDG_ACTIVATION_TOKEN", t) };
        }
        // Fresh window: reload the hotkey fields from the device.
        app.hotkeys_loaded = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window's state against the simulator, with one made-up monitor, once the simulator has answered.
    fn simulated() -> (egui::Context, App, Arc<Mutex<Shared>>) {
        let ctx = egui::Context::default();
        let wake: Wake = Arc::new(|| {});
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel();
        worker::spawn(true, false, shared.clone(), rx, wake.clone());
        let (_tray_tx, tray_rx) = channel();
        let mut app = App::new(shared.clone(), tx, tray_rx, wake);
        // Not this computer's settings file: the tests neither depend on it nor write it.
        app.local = local::Local::default();
        app.displays = displays::Displays::found(vec![odkvmctl::ddc::Display {
            id: "XYZ-1A2B-TEST".into(),
            name: "Monitor 24 (HDMI-A-1)".into(),
            inputs: vec![0x0F, 0x10, 0x11, 0x12],
            current: Some(0x12),
            error: None,
        }]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while shared.lock().unwrap().snap.is_none() {
            assert!(std::time::Instant::now() < deadline, "simulator never answered");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        (ctx, app, shared)
    }

    /// Drives every page in both themes headlessly against the simulator: catches panics and checks a switch
    /// round-trips.
    #[test]
    fn every_page_renders_against_the_simulator() {
        let (ctx, mut app, shared) = simulated();
        let mut frame = eframe::Frame::_new_kittest();
        for theme in [local::Theme::Dark, local::Theme::Light] {
            // Not saved: this test must not write the user's settings.
            app.local.theme = theme;
            for page in Page::ALL {
                app.page = page;
                for _ in 0..3 {
                    let input = egui::RawInput::default();
                    app.logic(&ctx, &mut frame);
                    let mut out = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
                    // No renderer here: drop the font texture uploads on purpose.
                    out.textures_delta.clear();
                }
            }
            assert_eq!(ctx.theme(), if theme == local::Theme::Dark { egui::Theme::Dark } else { egui::Theme::Light });
        }
        assert_eq!(app.hotkeys[0], "double:scrolllock");
        app.send(UiCmd::Switch(Target::Pc2));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let active = shared.lock().unwrap().snap.as_ref().map(|s| s.status.active);
            if active == Some(Pc::Pc2) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "switch never showed up");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// A screen dragged on the Screen hop page's canvas stays where it was dropped, in the page's draft: the KVM keeps
    /// its arrangement until Apply, then has the screen there, and the draft goes once the KVM's copy confirms it.
    #[test]
    fn a_screen_dragged_on_the_canvas_stays_where_it_is_dropped() {
        use kvm_proto::ctrl::Screen;
        let (ctx, mut app, shared) = simulated();
        let mut frame = eframe::Frame::_new_kittest();
        for v in [
            ConfigValue::AbsPointer(true),
            ConfigValue::Screen(0, Screen::new(Pc::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(1, Screen::new(Pc::Pc2, 1920, 1080, [1920, 0])),
        ] {
            app.send(UiCmd::Config(v));
        }
        let kvm_at = |shared: &Mutex<Shared>| {
            shared.lock().unwrap().snap.as_ref().and_then(|s| s.hop()).map(|h| h.screens[1].at)
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while kvm_at(&shared) != Some([1920, 0]) {
            assert!(std::time::Instant::now() < deadline, "the screens never reached the simulator");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.page = Page::Screens;
        let mut time = 0.0;
        let mut run = |app: &mut App, events: Vec<egui::Event>| {
            time += 0.05;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 1300.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            app.logic(&ctx, &mut frame);
            ctx.run_ui(input, |ui| app.ui(ui, &mut frame)).textures_delta.clear();
        };
        for _ in 0..4 {
            run(&mut app, vec![]);
        }
        let start = app.screens.drawn(1).expect("PC2's screen is drawn").0.center();
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        run(&mut app, vec![egui::Event::PointerMoved(start), button(start, true)]);
        for k in 1..=10 {
            run(&mut app, vec![egui::Event::PointerMoved(start + egui::vec2(15.0 * k as f32, 0.0))]);
        }
        // Painted where the pointer is, not a frame behind: 150 points right of where it was grabbed.
        let (r, sc) = app.screens.drawn(1).unwrap();
        assert!((r.center().x - (start.x + 150.0)).abs() < 1.0 && sc.at[1] == 0, "dragged to {r:?}, {:?}", sc.at);
        run(&mut app, vec![button(start + egui::vec2(150.0, 0.0), false)]);
        let dropped = app.screens.drawn(1).unwrap().1.at;
        assert_eq!(dropped, sc.at, "dropped where it was dragged to");
        // Not applied: shown where it was dropped, while the KVM keeps it where it was (through refreshes).
        let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
        while std::time::Instant::now() < until {
            run(&mut app, vec![]);
            assert_eq!(app.screens.drawn(1).unwrap().1.at, dropped, "snapped back");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(kvm_at(&shared), Some([1920, 0]), "sent before Apply");
        assert!(app.screens.has_draft());
        // Applied: every frame still shows it there, the KVM gets it, and the draft goes after its grace.
        let hop = shared.lock().unwrap().snap.as_ref().and_then(|s| s.hop()).unwrap();
        app.apply(&hop);
        let until = std::time::Instant::now() + std::time::Duration::from_millis(3500);
        while std::time::Instant::now() < until {
            run(&mut app, vec![]);
            assert_eq!(app.screens.drawn(1).unwrap().1.at, dropped, "snapped back after Apply");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(kvm_at(&shared), Some(dropped));
        assert!(!app.screens.has_draft(), "the draft stayed after the KVM had it");
    }

    /// Turning a software lock off asks first; turning it back on waits for the key, which the simulator presses after
    /// 2.5 s.
    #[test]
    fn a_software_lock_asks_first_and_comes_back_with_the_key() {
        let (ctx, mut app, shared) = simulated();
        let mut frame = eframe::Frame::_new_kittest();
        let mut frames = |app: &mut App| {
            for _ in 0..3 {
                app.logic(&ctx, &mut frame);
                ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui, &mut frame)).textures_delta.clear();
            }
        };
        let switching = |shared: &Mutex<Shared>| {
            shared.lock().unwrap().snap.as_ref().map(|s| s.status.flags & status_flags::SW_SWITCH_ALLOWED != 0)
        };
        let wait_for = |want: bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
            while switching(&shared) != Some(want) {
                assert!(std::time::Instant::now() < deadline, "switching from software never became {want}");
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        app.page = Page::Settings;
        app.confirm_lock = Some(ConfigValue::SwSwitchAllowed(false));
        frames(&mut app);
        assert!(app.confirm_lock.is_some(), "the question stays until answered");
        app.confirm_lock = None;
        app.set(ConfigValue::SwSwitchAllowed(false));
        wait_for(false);
        app.unlock_wait = Some((std::time::Instant::now(), status_flags::SW_SWITCH_ALLOWED));
        app.send(UiCmd::Config(ConfigValue::SwSwitchAllowed(true)));
        frames(&mut app);
        wait_for(true);
        frames(&mut app);
        assert_eq!(app.unlock_wait, None);
        assert_eq!(shared.lock().unwrap().error, None, "waiting for the key isn't an error");
    }

    /// Learn on the Keyboard page with the simulator: nothing is learned until keys are pressed in the window; Ctrl+Alt+K
    /// pressed there plays the simulated keyboard and goes into the field, where Apply would set it.
    #[test]
    fn a_hotkey_is_learned_into_its_field() {
        let (ctx, mut app, _shared) = simulated();
        let mut frame = eframe::Frame::_new_kittest();
        app.page = Page::Keyboard;
        let mut frame_with = |app: &mut App, events: Vec<egui::Event>| {
            app.logic(&ctx, &mut frame);
            let input = egui::RawInput { events, ..Default::default() };
            ctx.run_ui(input, |ui| app.ui(ui, &mut frame)).textures_delta.clear();
        };
        frame_with(&mut app, Vec::new());
        assert_eq!(app.hotkeys[1], "none");
        app.learn(1);
        // Untouched, it learns nothing.
        for _ in 0..30 {
            frame_with(&mut app, Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(app.learning.is_some() && app.learn_note.is_none(), "learned without a key pressed");
        let ctrl_alt = egui::Modifiers { ctrl: true, alt: true, command: true, ..Default::default() };
        let k = |pressed| egui::Event::Key {
            key: egui::Key::K,
            physical_key: Some(egui::Key::K),
            pressed,
            repeat: false,
            modifiers: ctrl_alt,
        };
        frame_with(&mut app, vec![egui::Event::ModifiersChanged(ctrl_alt)]);
        frame_with(&mut app, vec![k(true)]);
        frame_with(&mut app, vec![k(false)]);
        frame_with(&mut app, vec![egui::Event::ModifiersChanged(egui::Modifiers::NONE)]);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.learning.is_some() {
            assert!(std::time::Instant::now() < deadline, "nothing learned");
            frame_with(&mut app, Vec::new());
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(app.hotkeys[1], "ctrl+alt+k");
        assert_eq!(app.learn_note, Some((1, Ok("Learned from the keyboard. Apply uses it.".into()))));
    }

    /// The Firmware page reads the diagnostics while it is shown, and reads them again after `DIAG_EVERY`.
    #[test]
    fn the_firmware_page_reads_the_diagnostics() {
        let (ctx, mut app, shared) = simulated();
        let mut frame = eframe::Frame::_new_kittest();
        app.page = Page::Firmware;
        let diag_at = |shared: &Mutex<Shared>| shared.lock().unwrap().diag.as_ref().map(|d| d.0);
        let mut first = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            app.logic(&ctx, &mut frame);
            ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui, &mut frame)).textures_delta.clear();
            match (first, diag_at(&shared)) {
                (None, Some(t)) => first = Some(t),
                (Some(a), Some(b)) if b > a => break,
                _ => {}
            }
            assert!(std::time::Instant::now() < deadline, "the diagnostics were not read twice");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let sh = shared.lock().unwrap();
        let items = sh.diag.as_ref().unwrap().1.as_ref().unwrap();
        assert!(items.iter().any(|i| i.id == kvm_proto::ctrl::diag::id::CLK_USB));
    }

    #[test]
    fn a_new_window_gets_the_logo_again() {
        // Closing the window and opening it again (Linux, the tray's Open) gives the same App a new context.
        let (_, mut app, _shared) = simulated();
        for ctx in [egui::Context::default(), egui::Context::default()] {
            let (icon, word, _) = app.logo(&ctx);
            let textures = ctx.tex_manager();
            let textures = textures.read();
            assert_eq!(textures.meta(icon).map(|m| m.name.as_str()), Some("logo-icon"));
            assert_eq!(textures.meta(word).map(|m| m.name.as_str()), Some("logo-wordmark"));
        }
    }

    /// Renders every page in both themes to `<theme>-<page>.ppm` in `$OPENDYN_SHOTS` (default: a temp directory), with
    /// a small software rasteriser, to look at the layout without a display:
    /// `OPENDYN_SHOTS=/some/dir cargo test -p odkvmctl --bin opendyn-kvm -- --ignored screenshots`.
    #[test]
    #[ignore = "writes images; run by hand"]
    fn screenshots() {
        let dir = std::env::var_os("OPENDYN_SHOTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("opendyn-kvm-shots"));
        std::fs::create_dir_all(&dir).unwrap();
        // OPENDYN_LANG=de (fr, nb): the pages in that language.
        odkvmctl::i18n::init(std::env::var("OPENDYN_LANG").ok().as_deref().or(Some("en")));
        let (ctx, mut app, shared) = simulated();
        let (w, h) = (900, 680);
        // The whole Diagnostics card, at the foot of the Firmware page.
        let tall = 1700;
        let mut canvas = Canvas::new(w, h);
        let mut frame = eframe::Frame::_new_kittest();
        let mut time = 0.0;
        let mut shot = |app: &mut App, name: String, h: usize| {
            canvas.resize(w, h);
            let mut out = None;
            // Enough frames for layout passes and animations to settle.
            for _ in 0..6 {
                time += 1.0;
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w as f32, h as f32))),
                    time: Some(time),
                    ..Default::default()
                };
                app.logic(&ctx, &mut frame);
                let mut o = ctx.run_ui(input, |ui| app.ui(ui, &mut frame));
                canvas.textures(&o.textures_delta);
                o.textures_delta.clear();
                out = Some(o);
            }
            let out = out.unwrap();
            let ppp = out.pixels_per_point;
            canvas.clear();
            for p in ctx.tessellate(out.shapes, ppp) {
                if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
                    canvas.mesh(p.clip_rect, m, ppp);
                }
            }
            canvas.save(&dir.join(name.to_lowercase().replace(' ', "-")));
        };
        for theme in [local::Theme::Dark, local::Theme::Light] {
            app.local.theme = theme;
            let theme_name = if theme == local::Theme::Dark { "dark" } else { "light" };
            for page in Page::ALL {
                app.page = page;
                shot(&mut app, format!("{theme_name}-{page:?}.ppm"), h);
            }
            // Screen hop's whole page (OPENDYN_SIM_HOP=1 sets it up), with a screen selected for its editor.
            app.page = Page::Screens;
            app.screens.select(Some(1));
            shot(&mut app, format!("{theme_name}-screens-full.ppm"), 1300);
            app.screens.select(None);
            // The "are you sure" before a software lock.
            app.page = Page::Settings;
            app.confirm_lock = Some(ConfigValue::SwConfigAllowed(false));
            shot(&mut app, format!("{theme_name}-settings-lock.ppm"), h);
            app.confirm_lock = None;
            // Learning a hotkey: the countdown, then one learned with a warning.
            app.page = Page::Keyboard;
            app.learning = Some((1, std::time::Instant::now()));
            shot(&mut app, format!("{theme_name}-keyboard-learning.ppm"), h);
            app.learning = None;
            app.hotkeys[1] = "f5".into();
            let warning = odkvmctl::names::hotkey_warning(&kvm_proto::ctrl::Hotkey::combo(0, 0x3E), &[]).unwrap();
            app.learn_note = Some((1, Err(format!("{warning} Apply uses it anyway."))));
            shot(&mut app, format!("{theme_name}-keyboard-learned.ppm"), h);
            app.learn_note = None;
            app.hotkeys[1] = "none".into();
            // With the Bluetooth module (OPENDYN_SIM_BT=1): a host with the input, then a host pairing that waits for
            // its passkey (the banner on the Overview, the Bluetooth page).
            let has_bt = shared.lock().unwrap().snap.as_ref().is_some_and(|s| s.has_bluetooth());
            if has_bt {
                let wait = |what: &str, f: &dyn Fn(&Snapshot) -> bool| {
                    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
                    while !shared.lock().unwrap().snap.as_ref().is_some_and(f) {
                        assert!(std::time::Instant::now() < deadline, "the simulator never got to: {what}");
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                };
                app.page = Page::Overview;
                app.switch_to(Target::Bt1);
                std::thread::sleep(std::time::Duration::from_millis(1200));
                wait("the tablet active", &|s| s.status.bt_active == 1);
                shot(&mut app, format!("{theme_name}-overview-bt-active.ppm"), h);
                app.switch_to(Target::Pc1);
                wait("PC1 active", &|s| s.status.bt_active == 0);
                app.send(UiCmd::BtPair(true));
                wait("a passkey request", &|s| s.status.bt_passkey != 0);
                shot(&mut app, format!("{theme_name}-overview-bt-passkey.ppm"), h);
                app.page = Page::Bluetooth;
                shot(&mut app, format!("{theme_name}-bluetooth-pairing.ppm"), h);
                app.send(UiCmd::BtPair(false));
                wait("pairing closed", &|s| s.status.bt_pairing == 0);
            }
            // The diagnostics, scrolled into view, healthy and then with a warning and a fault.
            app.page = Page::Firmware;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while shared.lock().unwrap().diag.is_none() {
                assert!(std::time::Instant::now() < deadline, "no diagnostics");
                shot(&mut app, format!("{theme_name}-firmware.ppm"), h);
            }
            shot(&mut app, format!("{theme_name}-firmware-diagnostics.ppm"), tall);
            {
                use kvm_proto::ctrl::diag::{Grade, id};
                let mut sh = shared.lock().unwrap();
                if let Some((_, Ok(items))) = sh.diag.as_mut() {
                    for i in items.iter_mut() {
                        match i.id {
                            id::PC2_SPEED => (i.grade, i.value) = (Grade::Warn, kvm_proto::ctrl::Speed::Full as i32),
                            id::HUB0_SPEED => (i.grade, i.value) = (Grade::Bad, kvm_proto::ctrl::Speed::Low as i32),
                            _ => {}
                        }
                    }
                }
            }
            app.diag_asked = Some(std::time::Instant::now());
            shot(&mut app, format!("{theme_name}-firmware-diagnostics-faults.ppm"), tall);
            // Boards in the boot ROM's bootloader, as the PWR/SERVICE port shows them (no release folder: the app's).
            for kind in ["kvm", "new", "other", "unknown"] {
                shared.lock().unwrap().boot = vec![odkvmctl::bootsel::simulated(kind).unwrap()];
                shot(&mut app, format!("{theme_name}-firmware-boot-{kind}.ppm"), 1100);
            }
            shared.lock().unwrap().boot.clear();
        }
        eprintln!("screenshots in {}", dir.display());
    }

    /// Just enough of a renderer for egui's meshes: nearest-texel sampling, premultiplied alpha blending.
    struct Canvas {
        w: usize,
        h: usize,
        px: Vec<[f32; 4]>,
        textures: std::collections::HashMap<egui::TextureId, (usize, usize, Vec<egui::Color32>)>,
    }

    impl Canvas {
        fn new(w: usize, h: usize) -> Canvas {
            Canvas { w, h, px: vec![[0.0, 0.0, 0.0, 1.0]; w * h], textures: Default::default() }
        }

        fn clear(&mut self) {
            self.px.fill([0.0, 0.0, 0.0, 1.0]);
        }

        /// A new size; the textures stay.
        fn resize(&mut self, w: usize, h: usize) {
            (self.w, self.h) = (w, h);
            self.px = vec![[0.0, 0.0, 0.0, 1.0]; w * h];
        }

        fn textures(&mut self, delta: &egui::TexturesDelta) {
            for (id, deltas) in &delta.set {
                for d in deltas {
                    let egui::ImageData::Color(img) = &d.image;
                    let [iw, ih] = img.size;
                    match d.pos {
                        None => {
                            self.textures.insert(*id, (iw, ih, img.pixels.clone()));
                        }
                        Some([x0, y0]) => {
                            let (tw, _, t) = self.textures.get_mut(id).expect("patch of an unknown texture");
                            for y in 0..ih {
                                for x in 0..iw {
                                    t[(y0 + y) * *tw + x0 + x] = img.pixels[y * iw + x];
                                }
                            }
                        }
                    }
                }
            }
            for id in &delta.free {
                self.textures.remove(id);
            }
        }

        fn mesh(&mut self, clip: egui::Rect, mesh: &egui::Mesh, ppp: f32) {
            let Some((tw, th, tex)) = self.textures.get(&mesh.texture_id) else { return };
            let (cx0, cy0) = ((clip.min.x * ppp).max(0.0) as usize, (clip.min.y * ppp).max(0.0) as usize);
            let (cx1, cy1) =
                (((clip.max.x * ppp).ceil() as usize).min(self.w), ((clip.max.y * ppp).ceil() as usize).min(self.h));
            let edge =
                |a: egui::Pos2, b: egui::Pos2, c: egui::Pos2| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            let rgba = |c: egui::Color32| c.to_array().map(|v| f32::from(v) / 255.0);
            for tri in mesh.indices.as_chunks::<3>().0 {
                let v = [0, 1, 2].map(|k| &mesh.vertices[tri[k] as usize]);
                let p = v.map(|v| egui::pos2(v.pos.x * ppp, v.pos.y * ppp));
                let area = edge(p[0], p[1], p[2]);
                if area.abs() < 1e-6 {
                    continue;
                }
                let x0 = (p.iter().map(|p| p.x).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(cx0);
                let y0 = (p.iter().map(|p| p.y).fold(f32::MAX, f32::min).floor().max(0.0) as usize).max(cy0);
                let x1 = (p.iter().map(|p| p.x).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(cx1);
                let y1 = (p.iter().map(|p| p.y).fold(f32::MIN, f32::max).ceil().max(0.0) as usize).min(cy1);
                let col = v.map(|v| rgba(v.color));
                for y in y0..y1 {
                    for x in x0..x1 {
                        let c = egui::pos2(x as f32 + 0.5, y as f32 + 0.5);
                        let b0 = edge(p[1], p[2], c) / area;
                        let b1 = edge(p[2], p[0], c) / area;
                        let b2 = 1.0 - b0 - b1;
                        if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                            continue;
                        }
                        let u = v[0].uv.x * b0 + v[1].uv.x * b1 + v[2].uv.x * b2;
                        let t = v[0].uv.y * b0 + v[1].uv.y * b1 + v[2].uv.y * b2;
                        let tx = ((u * *tw as f32) as usize).min(tw - 1);
                        let ty = ((t * *th as f32) as usize).min(th - 1);
                        let texel = rgba(tex[ty * tw + tx]);
                        let src: [f32; 4] =
                            std::array::from_fn(|k| (col[0][k] * b0 + col[1][k] * b1 + col[2][k] * b2) * texel[k]);
                        let dst = &mut self.px[y * self.w + x];
                        for k in 0..4 {
                            dst[k] = src[k] + dst[k] * (1.0 - src[3]);
                        }
                    }
                }
            }
        }

        fn save(&self, path: &std::path::Path) {
            let mut data = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
            for p in &self.px {
                data.extend(p[..3].iter().map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8));
            }
            std::fs::write(path, data).unwrap();
        }
    }
}
