//! Tray icon with a quick-switch menu: `ksni` (StatusNotifierItem over D-Bus) on Linux, `tray-icon` on Windows.
//!
//! The menu switches to PC1 or PC2 and, on a KVM with the Bluetooth module, to each paired Bluetooth host (below the
//! PCs, only while any is paired). The icon is the logo on a tile in the active target's colour.

use std::sync::mpsc::Sender;

use kvm_proto::ctrl::{Pc, Rgb, Target};

use crate::worker::Wake;
use odkvmctl::{fl, icon};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    Switch(Target),
    /// With the XDG activation token the tray host handed over for this click, if any (Wayland: lets the window
    /// come to the front despite focus-stealing prevention).
    Open(Option<String>),
    Quit,
}

/// What the tray shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TrayState {
    /// The active PC; None without a KVM.
    pub active: Option<Pc>,
    pub colors: [Rgb; 2],
    /// The paired Bluetooth hosts (slot, name), and the one with the input (0: a PC has it).
    pub hosts: Vec<(u8, String)>,
    pub bt_active: u8,
    /// The language its texts are in: a change of language rebuilds the menu.
    pub language: String,
}

impl TrayState {
    fn connected(&self) -> bool {
        self.active.is_some()
    }

    fn active_host(&self) -> Option<&str> {
        self.hosts.iter().find(|(slot, _)| *slot == self.bt_active).map(|(_, name)| name.as_str())
    }

    fn tile(&self) -> Rgb {
        match (self.active, self.bt_active) {
            (None, _) => icon::DISCONNECTED,
            (Some(_), 1..) => kvm_core::leds::BT_VIOLET,
            (Some(pc), 0) => icon::pc_tile(pc, self.colors),
        }
    }

    fn tooltip(&self) -> String {
        match (self.active, self.active_host()) {
            (None, _) => fl!("tray-not-connected"),
            (Some(_), Some(host)) => fl!("tray-host-active", host = host),
            (Some(pc), None) => fl!("tray-pc-active", pc = pc.name().to_uppercase()),
        }
    }

    /// The switch entries: label, target, enabled (not the active one).
    fn entries(&self) -> Vec<(String, Target, bool)> {
        let pc = |p: Pc, t| {
            let active = self.active == Some(p) && self.bt_active == 0;
            (fl!("tray-switch-to", target = p.name().to_uppercase()), t, self.connected() && !active)
        };
        let mut out = vec![pc(Pc::Pc1, Target::Pc1), pc(Pc::Pc2, Target::Pc2)];
        for (slot, name) in &self.hosts {
            if let Some(t) = Target::bt(*slot) {
                out.push((fl!("tray-switch-to", target = name.as_str()), t, *slot != self.bt_active));
            }
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use ksni::blocking::{Handle, TrayMethods};
    use ksni::menu::StandardItem;

    use super::*;

    pub struct KsniTray {
        state: TrayState,
        tx: Sender<TrayAction>,
        wake: Wake,
    }

    impl KsniTray {
        fn send(&self, a: TrayAction) {
            let _ = self.tx.send(a);
            (self.wake)();
        }
    }

    impl ksni::Tray for KsniTray {
        fn id(&self) -> String {
            "opendyn-kvm".into()
        }

        fn title(&self) -> String {
            fl!("product-name")
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            vec![ksni::Icon {
                width: icon::SIZE as i32,
                height: icon::SIZE as i32,
                data: icon::argb(self.state.tile()),
            }]
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip { title: self.state.tooltip(), ..Default::default() }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            self.send(TrayAction::Open(ksni::current_xdg_activation_token()));
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            let item = |label: &str, a: TrayAction, enabled: bool| -> ksni::MenuItem<Self> {
                StandardItem {
                    label: label.into(),
                    enabled,
                    activate: Box::new(move |t: &mut Self| {
                        // The token only exists while the host's click is being handled.
                        let a = match &a {
                            TrayAction::Open(_) => TrayAction::Open(ksni::current_xdg_activation_token()),
                            other => other.clone(),
                        };
                        t.send(a)
                    }),
                    ..Default::default()
                }
                .into()
            };
            let mut menu: Vec<_> = self
                .state
                .entries()
                .into_iter()
                .map(|(label, t, enabled)| item(&label, TrayAction::Switch(t), enabled))
                .collect();
            menu.push(ksni::MenuItem::Separator);
            menu.push(item(&fl!("tray-open"), TrayAction::Open(None), true));
            menu.push(item(&fl!("tray-quit"), TrayAction::Quit, true));
            menu
        }
    }

    pub struct Tray(Handle<KsniTray>);

    impl Tray {
        pub fn new(tx: Sender<TrayAction>, wake: Wake) -> Option<Tray> {
            let t = KsniTray { state: TrayState { colors: icon::PC_COLORS, ..Default::default() }, tx, wake };
            // No StatusNotifierWatcher (e.g. GNOME without the AppIndicator extension): run without a tray.
            t.spawn().ok().map(Tray)
        }

        pub fn update(&mut self, state: &TrayState) {
            let state = state.clone();
            self.0.update(|t| t.state = state);
        }
    }
}

#[cfg(windows)]
mod imp {
    use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};

    use super::*;

    pub struct Tray {
        icon: TrayIcon,
        /// The switch entries the menu was last built with: rebuilt when they change (a host paired or forgotten).
        entries: Vec<(String, Target, bool)>,
    }

    fn make_icon(tile: Rgb) -> Option<Icon> {
        Icon::from_rgba(icon::rgba(tile), icon::SIZE as u32, icon::SIZE as u32).ok()
    }

    /// The menu items' ids: fixed, so that a rebuilt menu keeps working with the one event handler.
    fn target_id(t: Target) -> String {
        format!("switch-{}", t as u8)
    }

    fn action(id: &MenuId) -> Option<TrayAction> {
        match id.as_ref() {
            "open" => Some(TrayAction::Open(None)),
            "quit" => Some(TrayAction::Quit),
            other => {
                let t = other.strip_prefix("switch-")?.parse::<u8>().ok()?;
                Target::try_from(t).ok().map(TrayAction::Switch)
            }
        }
    }

    fn menu(entries: &[(String, Target, bool)]) -> Option<Menu> {
        let menu = Menu::new();
        for (label, t, enabled) in entries {
            menu.append(&MenuItem::with_id(target_id(*t), label, *enabled, None)).ok()?;
        }
        menu.append(&PredefinedMenuItem::separator()).ok()?;
        menu.append(&MenuItem::with_id("open", fl!("tray-open"), true, None)).ok()?;
        menu.append(&MenuItem::with_id("quit", fl!("tray-quit"), true, None)).ok()?;
        Some(menu)
    }

    impl Tray {
        /// Must run on the thread with the window's event loop (the main thread).
        pub fn new(tx: Sender<TrayAction>, wake: Wake) -> Option<Tray> {
            let state = TrayState { colors: icon::PC_COLORS, ..Default::default() };
            let entries = state.entries();
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu(&entries)?))
                .with_tooltip(state.tooltip())
                .with_icon(make_icon(state.tile())?)
                .build()
                .ok()?;
            let (tx2, wake2) = (tx.clone(), wake.clone());
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Some(a) = action(&e.id) {
                    let _ = tx.send(a);
                    wake();
                }
            }));
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } = e {
                    let _ = tx2.send(TrayAction::Open(None));
                    wake2();
                }
            }));
            Some(Tray { icon, entries })
        }

        pub fn update(&mut self, state: &TrayState) {
            let _ = self.icon.set_icon(make_icon(state.tile()));
            let _ = self.icon.set_tooltip(Some(state.tooltip()));
            let entries = state.entries();
            if entries != self.entries
                && let Some(m) = menu(&entries)
            {
                self.icon.set_menu(Some(Box::new(m)));
                self.entries = entries;
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::*;

    pub struct Tray;

    impl Tray {
        pub fn new(_tx: Sender<TrayAction>, _wake: Wake) -> Option<Tray> {
            None
        }

        pub fn update(&mut self, _state: &TrayState) {}
    }
}

pub use imp::Tray;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_lists_the_paired_hosts_and_names_the_active_one() {
        let none = TrayState::default();
        assert_eq!(none.tooltip(), "OpenDynKVM: not connected");
        assert!(none.entries().iter().all(|(_, _, enabled)| !enabled));

        let mut s = TrayState { active: Some(Pc::Pc1), colors: icon::PC_COLORS, ..Default::default() };
        assert_eq!(s.tooltip(), "OpenDynKVM: PC1 active");
        assert_eq!(s.tile(), icon::PC_COLORS[0]);
        let labels = |s: &TrayState| s.entries().into_iter().map(|(l, _, on)| (l, on)).collect::<Vec<_>>();
        assert_eq!(labels(&s), [("Switch to PC1".into(), false), ("Switch to PC2".into(), true)]);

        s.hosts = vec![(1, "Tablet".into()), (3, "Laptop".into())];
        s.bt_active = 3;
        assert_eq!(s.tooltip(), "OpenDynKVM: Laptop active (wireless)");
        assert_eq!(s.tile(), kvm_core::leds::BT_VIOLET);
        assert_eq!(
            labels(&s),
            [
                ("Switch to PC1".into(), true),
                ("Switch to PC2".into(), true),
                ("Switch to Tablet".into(), true),
                ("Switch to Laptop".into(), false),
            ]
        );
        assert_eq!(s.entries()[3].1, Target::Bt3);
    }
}
