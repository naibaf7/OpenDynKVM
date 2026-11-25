//! Installing firmware, on the Firmware page (software.md §9 "Firmware page"): the file (shipped with the app, or
//! picked), over this computer's PC port to a KVM that runs, or onto the boot ROM's drive of a board in its bootloader
//! on the PWR/SERVICE port (`odkvmctl::bootsel`).

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use eframe::egui::{self, RichText, Ui};
use kvm_proto::ctrl::{FwState, Info};
use kvm_proto::link::bt::Chip;
use odkvmctl::bootsel::{self, Board, BootDevice};
use odkvmctl::client::Snapshot;
use odkvmctl::{firmware, fl, image, names};

use crate::App;
use crate::look::{BAD, GOOD, WARN, card, ink, note};
use crate::worker::UiCmd;

/// What the firmware file is, read once per path (the files are up to 1.5 MB).
type Described = Result<(u32, u8), String>;
/// What a board in the bootloader would get from the file: the file's name, version, hardware major version.
type ForBoot = Result<(String, u32, u8), String>;

/// The firmware file and what is known about it.
pub struct Files {
    /// As typed or picked; starts as the firmware shipped with the app.
    pub path: String,
    bundled: Option<PathBuf>,
    described: Option<(String, Described)>,
    for_boot: Vec<((String, bool), ForBoot)>,
    /// The release's module firmware: its version (None: the release has none).
    module: Option<(String, Result<Option<u32>, String>)>,
    /// The completed update ([`Shared::finished`](crate::worker::Shared)) after which the files were last read anew.
    finished: Option<odkvmctl::time::Instant>,
}

impl Files {
    pub fn new() -> Files {
        let bundled = firmware::bundled();
        let path = bundled.as_ref().map(|b| b.display().to_string()).unwrap_or_default();
        Files { path, bundled, described: None, for_boot: Vec::new(), module: None, finished: None }
    }

    /// Reads the files anew when next asked (a release folder may have been rebuilt meanwhile).
    fn forget(&mut self) {
        self.described = None;
        self.for_boot.clear();
        self.module = None;
    }

    pub(crate) fn path(&self) -> PathBuf {
        PathBuf::from(self.path.trim())
    }

    /// Whether the release has the module firmware a KVM with firmware `rp_fw` takes (its own release's), for the
    /// module's `chip`.
    pub(crate) fn module(&mut self, rp_fw: u32, chip: Chip) -> Result<(), String> {
        let key = self.path.trim().to_string();
        let cached = format!("{key}\n{}", chip.name());
        if self.module.as_ref().is_none_or(|(p, _)| *p != cached) {
            let m = firmware::module_for(Path::new(&key), chip).map(|m| m.map(|m| m.version));
            self.module = Some((cached, m));
        }
        match &self.module.as_ref().unwrap().1 {
            Ok(Some(v)) if *v == rp_fw => Ok(()),
            Ok(Some(v)) => Err(fl!(
                "fw-module-mismatch",
                file = firmware::module_bin(chip),
                module = names::version(*v),
                version = names::version(rp_fw)
            )),
            Ok(None) => Err(fl!("fw-no-module", folder = key)),
            Err(e) => Err(e.clone()),
        }
    }

    fn described(&mut self) -> Described {
        let key = self.path.trim().to_string();
        if self.described.as_ref().is_none_or(|(p, _)| *p != key) {
            let d = if key.is_empty() { Err(String::new()) } else { firmware::describe(Path::new(&key)) };
            self.described = Some((key, d));
        }
        self.described.as_ref().unwrap().1.clone()
    }

    /// The UF2 a board in the bootloader would get (`first`: a new board's).
    fn for_boot(&mut self, first: bool) -> ForBoot {
        let key = (self.path.trim().to_string(), first);
        if let Some((_, r)) = self.for_boot.iter().find(|(k, _)| *k == key) {
            return r.clone();
        }
        let r = firmware::for_bootloader(Path::new(&key.0), first).map(|c| {
            let name = c.file.file_name().map_or_else(|| c.file.display().to_string(), |n| n.to_string_lossy().into());
            (name, c.version, c.hw_major)
        });
        // Only the current path's answers are kept.
        self.for_boot.retain(|((p, _), _)| *p == key.0);
        self.for_boot.push((key, r.clone()));
        r
    }

    fn is_bundled(&self) -> bool {
        self.bundled.as_ref().is_some_and(|b| self.path().starts_with(b))
    }
}

impl App {
    /// The Firmware page's file, update and bootloader cards (after the versions, before the diagnostics).
    pub(crate) fn install_cards(&mut self, ui: &mut Ui, s: Option<&Snapshot>) {
        if let Some(p) = self.shared.lock().unwrap().picked.take() {
            self.fw.path = p.display().to_string();
        }
        // An update is completely done: the page starts afresh (the worker cleared its progress and results).
        let finished = self.shared.lock().unwrap().finished;
        if let Some(f) = finished
            && self.fw.finished != Some(f.at)
        {
            self.fw.finished = Some(f.at);
            self.fw.forget();
            self.diag_asked = None;
        }
        // The KVM on this PC's port: read whole, or only its identity when its firmware speaks another protocol.
        let info = s.map(|s| s.info).or(self.shared.lock().unwrap().mismatch);
        card(ui, &fl!("fw-file-title"), |ui| self.firmware_file(ui, info.as_ref()));
        if let Some(info) = &info {
            ui.add_space(8.0);
            self.update_card(ui, info);
        }
        let boot = self.shared.lock().unwrap().boot.clone();
        for d in &boot {
            ui.add_space(8.0);
            self.boot_card(ui, d);
        }
        let result = self.shared.lock().unwrap().boot_result.clone();
        if let Some((_, r)) = result {
            ui.add_space(8.0);
            match r {
                Ok(()) => ui.label(RichText::new(fl!("boot-installed")).color(ink(ui, GOOD))),
                Err(e) => ui.label(RichText::new(e).color(ink(ui, BAD))),
            };
        }
        if info.is_none() && boot.is_empty() {
            ui.add_space(8.0);
            card(ui, &fl!("boot-none-title"), |ui| {
                note(ui, fl!("boot-none-note"));
                if cfg!(target_os = "linux") {
                    note(ui, fl!("udev-hint"));
                }
            });
        }
    }

    fn firmware_file(&mut self, ui: &mut Ui, info: Option<&Info>) {
        let picking = self.shared.lock().unwrap().picking;
        ui.horizontal(|ui| {
            let shipped = self.fw.bundled.clone().filter(|_| !self.fw.is_bundled());
            let buttons = if shipped.is_some() { 260.0 } else { 110.0 };
            ui.add(
                egui::TextEdit::singleline(&mut self.fw.path)
                    .hint_text(fl!("fw-file-hint"))
                    .desired_width((ui.available_width() - buttons).max(160.0)),
            );
            #[cfg(not(target_arch = "wasm32"))]
            if ui.add_enabled(!picking, egui::Button::new(fl!("browse"))).clicked() {
                self.browse();
            }
            if let Some(b) = shipped
                && ui.button(fl!("fw-shipped")).on_hover_text(b.display().to_string()).clicked()
            {
                self.fw.path = b.display().to_string();
            }
        });
        match self.fw.described() {
            Ok((version, major)) => {
                let mut line = fl!("fw-file-is", version = names::version(version), major = u32::from(major));
                if self.fw.is_bundled() {
                    line = format!("{line} · {}", fl!("fw-shipped-note"));
                }
                note(ui, line);
                if let Some(info) = info {
                    match image::check_hardware(version, major, info.hw) {
                        Err(e) => {
                            ui.label(RichText::new(e).color(ink(ui, BAD)));
                        }
                        Ok(()) => {
                            let kvm = names::version(info.rp_fw);
                            let (text, color) = match version.cmp(&info.rp_fw) {
                                Ordering::Equal => (fl!("fw-same"), None),
                                Ordering::Greater => (fl!("fw-newer", kvm = kvm), Some(GOOD)),
                                Ordering::Less => (fl!("fw-older", kvm = kvm), Some(WARN)),
                            };
                            match color {
                                Some(c) => ui.label(RichText::new(text).color(ink(ui, c))),
                                None => ui.label(text),
                            };
                        }
                    }
                }
            }
            Err(e) if !e.is_empty() => {
                ui.label(RichText::new(e).color(ink(ui, WARN)));
            }
            Err(_) => note(ui, fl!("fw-no-file")),
        }
    }

    /// Over this computer's PC port, to the KVM that answers there.
    fn update_card(&mut self, ui: &mut Ui, info: &Info) {
        let (update, result, module, module_result, pending) = {
            let sh = self.shared.lock().unwrap();
            (
                sh.update,
                sh.update_result.clone(),
                sh.module_update,
                sh.module_result.clone(),
                sh.module_pending.is_some(),
            )
        };
        let described = self.fw.described();
        let fits = described.as_ref().is_ok_and(|(v, m)| image::check_hardware(*v, *m, info.hw).is_ok());
        let layout = described.as_ref().map_or(Ok(()), |(v, _)| image::check_layout(info.rp_fw, *v));
        card(ui, &fl!("update"), |ui| {
            note(ui, fl!("update-over-pc-note"));
            if let Err(e) = &layout {
                ui.label(RichText::new(e).color(ink(ui, BAD)));
            }
            let fits = fits && layout.is_ok();
            let busy =
                matches!(update, Some((st, ..)) if !matches!(st, FwState::Done | FwState::Failed | FwState::Idle));
            if ui.add_enabled(!busy && fits, egui::Button::new(fl!("install"))).clicked() {
                let mut sh = self.shared.lock().unwrap();
                sh.update = None;
                sh.update_result = None;
                sh.finished = None;
                drop(sh);
                self.fw.forget();
                self.send(UiCmd::Update(self.fw.path()));
            }
            if let Some(f) = self.shared.lock().unwrap().finished {
                let v = names::version(f.version);
                let text = if f.module {
                    fl!("update-complete-module", version = v)
                } else {
                    fl!("update-complete", version = v)
                };
                ui.label(RichText::new(text).color(ink(ui, GOOD)));
            }
            if let Some((state, done, total)) = update {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .text(format!("RP2354B: {}", names::fw_state(state))),
                );
                if state == FwState::AwaitingKey {
                    ui.label(RichText::new(fl!("press-key-to-install")).strong());
                }
            }
            match result {
                Some(Ok(())) if pending => {
                    ui.label(RichText::new(fl!("installed-module-next")).color(ink(ui, GOOD)));
                }
                Some(Ok(())) => {
                    ui.label(RichText::new(fl!("installed")).color(ink(ui, GOOD)));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(ink(ui, BAD)));
                }
                None => {}
            }
            // The module's turn, once the KVM is back.
            if let Some((state, done, total)) = module {
                let what = fl!("wireless-module");
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .text(format!("{what}: {}", names::fw_state(state))),
                );
            }
            match module_result {
                Some(Ok(())) => {
                    ui.label(RichText::new(fl!("installed-module")).color(ink(ui, GOOD)));
                }
                Some(Err(e)) => {
                    ui.label(RichText::new(e).color(ink(ui, BAD)));
                }
                None => {}
            }
        });
    }

    /// A board in the boot ROM's bootloader: what it is, the file it would get, installing it or leaving.
    fn boot_card(&mut self, ui: &mut Ui, d: &BootDevice) {
        let installing = self.shared.lock().unwrap().boot_install.clone();
        let busy = installing.is_some();
        card(ui, &fl!("boot-title"), |ui| {
            match &d.board {
                Ok(Board::Kvm { installed: Some(v), slot }) => {
                    ui.label(fl!("boot-kvm", version = names::version(*v), slot = slot_name(*slot)));
                }
                Ok(Board::Kvm { installed: None, slot }) => {
                    ui.label(fl!("boot-kvm-empty", slot = slot_name(*slot)));
                }
                Ok(Board::New { installed: None }) => {
                    ui.label(fl!("boot-new"));
                }
                Ok(Board::New { installed: Some(v) }) => {
                    ui.label(fl!("boot-new-unpartitioned", version = names::version(*v)));
                }
                Ok(Board::Previous { installed }) => {
                    let version = installed.map_or_else(|| "-".to_string(), names::version);
                    ui.label(fl!("boot-kvm-previous", version = version));
                }
                Ok(Board::Other) => {
                    ui.label(RichText::new(fl!("boot-other")).color(ink(ui, BAD)));
                }
                Err(e) => {
                    ui.label(RichText::new(e).color(ink(ui, WARN)));
                    note(ui, fl!("boot-unknown-note"));
                }
            }
            match &d.drive {
                Some(p) => note(ui, fl!("boot-drive", path = p.display().to_string())),
                None => note(ui, fl!("boot-no-drive-note")),
            }
            // What it can get: the UF2 its board takes, or (state unknown) the user's choice of the two.
            let choices: &[bool] = match &d.board {
                Ok(b @ (Board::Kvm { .. } | Board::New { .. } | Board::Previous { .. })) => {
                    if b.takes_first() {
                        &[true]
                    } else {
                        &[false]
                    }
                }
                Ok(Board::Other) => &[],
                Err(_) => &[false, true],
            };
            for &first in choices {
                let what = self.fw.for_boot(first);
                ui.horizontal(|ui| {
                    let label = if first { fl!("boot-set-up") } else { fl!("install") };
                    if ui.add_enabled(what.is_ok() && !busy, egui::Button::new(label)).clicked() {
                        self.boot_install(d.clone(), first);
                    }
                    match &what {
                        Ok((file, v, m)) => note(
                            ui,
                            fl!(
                                "boot-writes",
                                file = file.as_str(),
                                version = names::version(*v),
                                major = u32::from(*m)
                            ),
                        ),
                        Err(e) => {
                            ui.label(RichText::new(e).color(ink(ui, WARN)));
                        }
                    }
                });
            }
            if let Some((_, done, total)) = installing.filter(|(l, ..)| *l == d.location) {
                ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).text(fl!("boot-writing")));
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
            }
            if !matches!(d.board, Ok(Board::Other))
                && ui
                    .add_enabled(!busy, egui::Button::new(fl!("boot-restart")))
                    .on_hover_text(fl!("boot-restart-hover"))
                    .clicked()
            {
                self.boot_restart(d.clone());
            }
        });
    }

    /// The desktop's file dialog, on a thread of its own so that the window keeps drawing. (Not in a browser: the web
    /// build has no files to install.)
    #[cfg(not(target_arch = "wasm32"))]
    fn browse(&mut self) {
        let start = {
            let p = self.fw.path();
            if p.is_dir() { Some(p) } else { p.parent().filter(|d| d.is_dir()).map(Path::to_path_buf) }
        }
        .or_else(|| self.fw.bundled.clone());
        self.shared.lock().unwrap().picking = true;
        let (shared, wake) = (self.shared.clone(), self.wake.clone());
        let (title, filter) = (fl!("pick-firmware"), fl!("firmware-files"));
        std::thread::spawn(move || {
            let mut dialog = rfd::FileDialog::new().set_title(title).add_filter(filter, &["bin", "uf2"]);
            if let Some(dir) = start {
                dialog = dialog.set_directory(dir);
            }
            let picked = dialog.pick_file();
            let mut sh = shared.lock().unwrap();
            sh.picking = false;
            if picked.is_some() {
                sh.picked = picked;
            }
            drop(sh);
            wake();
        });
    }

    fn boot_install(&mut self, d: BootDevice, first: bool) {
        let path = self.fw.path();
        let (shared, wake) = (self.shared.clone(), self.wake.clone());
        {
            let mut sh = shared.lock().unwrap();
            sh.boot_install = Some((d.location.clone(), 0, 1));
            sh.boot_result = None;
            sh.finished = None;
        }
        crate::background(move || {
            let mut version = None;
            let r = firmware::for_bootloader(&path, first).and_then(|c| {
                version = Some(c.version);
                bootsel::install(&d, &c.data, |done, total| {
                    shared.lock().unwrap().boot_install = Some((d.location.clone(), done, total));
                    wake();
                })
            });
            let mut sh = shared.lock().unwrap();
            sh.boot_install = None;
            // Done once the KVM answers on this computer's PC port with it (and its module is through).
            if r.is_ok()
                && let Some(version) = version
            {
                sh.expected = Some(crate::worker::Expected { version, restart: false });
            }
            sh.boot_result = Some((d.location.clone(), r));
            drop(sh);
            wake();
        });
    }

    fn boot_restart(&mut self, d: BootDevice) {
        let (shared, wake) = (self.shared.clone(), self.wake.clone());
        crate::background(move || {
            if let Err(e) = bootsel::restart(&d) {
                shared.lock().unwrap().boot_result = Some((d.location.clone(), Err(e)));
                wake();
            }
        });
    }
}

fn slot_name(slot: u8) -> &'static str {
    if slot == 0 { "A" } else { "B" }
}
