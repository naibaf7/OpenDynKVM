//! The window in a browser (the web build, wasm32: software.md §9 "On the web"): the simulator only, in a canvas
//! with the id `opendyn-kvm`. The worker runs a round each frame ([`worker::Web`]); the settings are kept in the
//! browser's storage (`odkvmctl::local`); no tray, no second copy to look for, no monitors to switch. The page's
//! address sets the simulator up as the environment does elsewhere: `?OPENDYN_SIM_BT=1&OPENDYN_SIM_HOP=1`, and
//! `OPENDYN_LANG` picks the language (else the one chosen in Settings, else English).

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use eframe::egui;
use wasm_bindgen::JsCast;

use crate::App;
use crate::worker::{self, Shared, Wake};

/// The window and the worker, as one app for eframe's web runner.
struct WebApp {
    app: App,
    worker: worker::Web,
}

impl eframe::App for WebApp {
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.worker.step();
        self.app.logic(ctx, frame);
        // The simulated KVM moves on between frames (its mouse, its delays): keep them coming.
        ctx.request_repaint_after(odkvmctl::time::Duration::from_millis(50));
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        self.app.ui(ui, frame);
    }
}

pub fn start() {
    let language = odkvmctl::env_var("OPENDYN_LANG").or(odkvmctl::local::load().language);
    odkvmctl::i18n::init(Some(language.as_deref().unwrap_or("en")));
    let canvas = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("opendyn-kvm"))
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .expect("a <canvas id=\"opendyn-kvm\"> in the page");
    wasm_bindgen_futures::spawn_local(async move {
        let slot: Arc<Mutex<Option<egui::Context>>> = Arc::default();
        let wake: Wake = {
            let slot = slot.clone();
            Arc::new(move || {
                if let Some(ctx) = slot.lock().unwrap().as_ref() {
                    ctx.request_repaint();
                }
            })
        };
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = channel();
        // No tray: its channel stays empty.
        let (_, tray_rx) = channel();
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    *slot.lock().unwrap() = Some(cc.egui_ctx.clone());
                    let worker = worker::Web::new(shared.clone(), rx, wake.clone());
                    Ok(Box::new(WebApp { app: App::new(shared, tx, tray_rx, wake), worker }))
                }),
            )
            .await;
        if let Err(e) = started {
            web_sys::console::error_1(&e);
        }
    });
}
