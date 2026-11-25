//! The Keymaps page (hid_mapping.md §7): the mapping editor. Four keymaps, each used by the profiles that pick it;
//! eight layers each; a picture of a keyboard, a mouse or a gamepad whose controls show what they do on the layer, and
//! the axis rules beside them.
//!
//! - **Choosing a control:** click it, or press it on the real device while *Learn* is on (the KVM then streams the
//!   controls and keeps them from the hosts: `WatchControls(2)`).
//! - **What it does:** the quick choices (none, transparent, as it says), a list of mouse, gamepad, layer and media
//!   actions, a keycode typed (`odkvmctl::keycodes`), or *Press what it should do*: the next control pressed, on any
//!   device, gives its own keycode.
//! - **Gamepads:** the device's controls (`GetControls`), its axes live with their calibration (a wizard measures
//!   them), and names for its extra buttons, kept on this computer by the device's identity.

use eframe::egui::{self, CornerRadius, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use kvm_core::pads::Style;
use kvm_hid::layouts::{Layout, Physical};
use kvm_map::axes::{self, Rule, Target, flags};
use kvm_map::keycode::{KC_NO, KC_TRNS};
use kvm_map::keymap::identity;
use kvm_map::pos;
use kvm_proto::ctrl::{AxisCal, ConfigValue, Device, GamepadOut, MAX_PROFILES, PortState};
use odkvmctl::client::{DeviceSlot, Snapshot};
use odkvmctl::time::Instant;
use odkvmctl::{fl, keycodes, names};

use crate::App;
use crate::look::{GOOD, WARN, card, ink, note, palette};
use crate::worker::{DeviceView, KeymapLayer, UiCmd};

mod parts;

use parts::PartsState;
pub use parts::{Section, layout_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Keyboard,
    Mouse,
    Gamepad,
}

/// What the next control pressed does while learning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Learning {
    /// Selects it.
    Select,
    /// Gives the selected control its keycode.
    Assign,
}

/// Calibrating a gamepad: its axes' limits seen so far, then their rest.
#[derive(Debug, Clone)]
struct Wizard {
    device: Device,
    seen: [(i16, i16); kvm_hid::gamepad::AXES],
    /// When the user said "done": the axes rest after this.
    resting_since: Option<Instant>,
}

#[derive(Default)]
pub struct Editor {
    keymap: u8,
    layer: u8,
    kind: Kind,
    selected: Option<u16>,
    learning: Option<Learning>,
    /// The controls are watched (`WatchControls`): 1 for the live picture, 2 while learning; asked again every
    /// [`WATCH_RENEW_S`], since the KVM ends a watch nobody renews.
    watching: u8,
    watched_at: f64,
    /// This computer is the PC the input goes to: only it watches the controls (learning, calibration, the live picture).
    here: bool,
    /// The last control seen pressed (`Shared::touched`), so each is taken once.
    touched_seen: Option<Instant>,
    typed: String,
    typed_error: Option<String>,
    pad_device: Option<Device>,
    wizard: Option<Wizard>,
    /// What was asked of the KVM and not answered yet: a keymap's layer, a device's controls.
    asked_layer: Option<(u8, u8)>,
    asked_pad: Option<Device>,
    /// The other sections (`parts.rs`).
    parts: PartsState,
    /// Learning ends at this time (the window's clock) unless a control is used: it restarts with each one.
    learn_until: f64,
    /// Escape went down while learning (when): held [`ESC_HOLD_S`] it stops learning, let go sooner it counts as the
    /// control pressed.
    esc_down: Option<f64>,
    /// How the gamepad's buttons are named and drawn: the chosen device's style (`kvm_core::pads`), or the one the user
    /// picked for it (kept on this computer by `pad_key`, the device's identity); and the device's own.
    pad_style: Style,
    pad_auto: Style,
    pad_key: String,
    /// The keyboard picture's legends: the layout of the host this computer is, and that host's name.
    legends: Layout,
    legends_host: String,
}

impl Editor {
    /// The screenshots: a picture with a control selected.
    #[cfg(test)]
    pub fn show(&mut self, kind: Kind, selected: Option<u16>) {
        self.kind = kind;
        self.selected = selected;
    }

    /// The screenshots: learning, its countdown starting at the next frame.
    #[cfg(test)]
    pub fn learning(&mut self, on: bool) {
        self.learning = on.then_some(Learning::Select);
        self.learn_until = 0.0;
    }

    /// The screenshots: a section, and the device the monitor shows.
    #[cfg(test)]
    pub fn section(&mut self, sec: Section, monitor: Option<Device>) {
        self.parts.section = sec;
        self.parts.show_monitor(monitor);
    }

    /// Leaving the page: the controls are no longer watched (and reach the hosts again), a device no longer
    /// monitored, and the shared parts read anew next time.
    pub fn leave(&mut self) -> Vec<UiCmd> {
        self.learning = None;
        self.wizard = None;
        self.asked_layer = None;
        self.asked_pad = None;
        let mut out = Vec::new();
        if std::mem::take(&mut self.watching) != 0 {
            out.push(UiCmd::Watch(0));
        }
        if let Some(d) = self.parts.leave() {
            out.push(UiCmd::Monitor(d, false));
        }
        out
    }
}

/// Short legends of the keyboard's keys for the picture.
fn key_legend(u: u8) -> String {
    let named = match u {
        0x04..=0x1D => return ((b'A' + u - 0x04) as char).to_string(),
        0x1E..=0x26 => return ((b'1' + u - 0x1E) as char).to_string(),
        0x27 => "0",
        0x28 => "Enter",
        0x29 => "Esc",
        0x2A => "Bksp",
        0x2B => "Tab",
        0x2C => "Space",
        0x2D => "-",
        0x2E => "=",
        0x2F => "[",
        0x30 => "]",
        0x31 | 0x64 => "\\",
        0x32 => "#",
        0x33 => ";",
        0x34 => "'",
        0x35 => "`",
        0x36 => ",",
        0x37 => ".",
        0x38 => "/",
        0x39 => "Caps",
        0x3A..=0x45 => return format!("F{}", u - 0x39),
        0x68..=0x73 => return format!("F{}", u - 0x5B),
        0x46 => "PrtSc",
        0x47 => "ScrLk",
        0x48 => "Pause",
        0x49 => "Ins",
        0x4A => "Home",
        0x4B => "PgUp",
        0x4C => "Del",
        0x4D => "End",
        0x4E => "PgDn",
        0x4F => "→",
        0x50 => "←",
        0x51 => "↓",
        0x52 => "↑",
        0x53 => "Num",
        0x54 => "/",
        0x55 => "*",
        0x56 => "-",
        0x57 => "+",
        0x58 => "Ent",
        0x59..=0x61 => return ((b'1' + u - 0x59) as char).to_string(),
        0x62 => "0",
        0x63 => ".",
        0x65 => "Menu",
        0xE0 | 0xE4 => "Ctrl",
        0xE1 | 0xE5 => "Shift",
        0xE2 | 0xE6 => "Alt",
        0xE3 | 0xE7 => "GUI",
        0x87..=0x8B => return format!("Int{}", u - 0x86),
        0x90..=0x91 => return format!("Lang{}", u - 0x8F),
        _ => return keycodes::control_name(u as u16),
    };
    named.into()
}

/// The standard gamepad's buttons 1–17 (W3C order, as the virtual gamepad numbers them).
fn pad_name(n: usize, style: Style) -> Option<String> {
    let arrows = |n: usize| ["↑", "↓", "←", "→"].get(n.checked_sub(12)?).map(|a| a.to_string());
    Some(match (style, n) {
        // A generic pad's buttons are as the device numbers them.
        (Style::Generic, _) => (n + 1).to_string(),
        (_, 16) => fl!("pad-home"),
        (Style::Offset, 8) => fl!("pad-back"),
        (Style::Offset, 9) => fl!("pad-start"),
        (Style::Offset, _) => match ["A", "B", "X", "Y", "LB", "RB", "LT", "RT", "", "", "LS", "RS"].get(n) {
            Some(s) => s.to_string(),
            None => arrows(n)?,
        },
        // Shapes, painted (`control`): the bottom, right, left and top face buttons.
        (Style::Symmetric, 0..=3) => SHAPES[n].to_string(),
        (Style::Symmetric, 8) => fl!("pad-share"),
        (Style::Symmetric, 9) => fl!("pad-options"),
        (Style::Symmetric, 17) => fl!("pad-touch-pad"),
        (Style::Symmetric, 18) => fl!("pad-mute"),
        (Style::Symmetric, 19) => fl!("pad-touch"),
        (Style::Symmetric, _) => match ["", "", "", "", "L1", "R1", "L2", "R2", "", "", "L3", "R3"].get(n) {
            Some(s) => s.to_string(),
            None => arrows(n)?,
        },
    })
    .filter(|s| !s.is_empty())
}

/// The symmetric style's face buttons, painted as shapes: bottom, right, left, top.
const SHAPES: [&str; 4] = ["✕", "○", "□", "△"];

/// Face button shape `n` ([`SHAPES`]) around `c`, `r` from its centre to its edge.
fn paint_shape(painter: &egui::Painter, c: Pos2, r: f32, n: usize, color: egui::Color32) {
    let stroke = Stroke::new(1.6, color);
    match n {
        0 => {
            painter.line_segment([c + Vec2::new(-r, -r), c + Vec2::new(r, r)], stroke);
            painter.line_segment([c + Vec2::new(-r, r), c + Vec2::new(r, -r)], stroke);
        }
        1 => {
            painter.circle_stroke(c, r, stroke);
        }
        2 => {
            painter.rect_stroke(Rect::from_center_size(c, Vec2::splat(2.0 * r)), 0.0, stroke, StrokeKind::Middle);
        }
        _ => {
            let pts = [c + Vec2::new(0.0, -r), c + Vec2::new(r, 0.8 * r), c + Vec2::new(-r, 0.8 * r)];
            painter.add(egui::Shape::closed_line(pts.to_vec(), stroke));
        }
    }
}

/// How often the page asks the KVM again to go on watching the controls (it ends a watch after 10 s, `WATCH_MS`).
const WATCH_RENEW_S: f64 = 4.0;
/// Learning ends this long after it started or a control was last used: while it runs, the controls reach no host.
const LEARN_S: f64 = 20.0;
/// Escape held this long stops learning (the mouse, which could click Stop, is learned from too).
const ESC_HOLD_S: f64 = 3.0;
const ESC: u16 = 0x29;
/// A control pressed and let go at once still shows lit this long.
const FLASH_MS: u64 = 300;
const ARROWS: [&str; 4] = ["↑", "→", "↓", "←"];

/// What a keycode shows on its control: a key's legend, else its name.
fn code_legend(kc: u16, layout: Layout, style: Style) -> String {
    use kvm_map::keycode::{KC_BTN1, KVM_ACTION, KVM_ACTIONS, KVM_BTN9, KVM_DPAD, QK_JOYSTICK};
    use kvm_proto::ctrl::{Host, HotkeyAction};
    match kc {
        KC_TRNS => "▽".into(),
        KC_NO => "—".into(),
        // What the key types on the host's layout: a letter, or its shifted character over its own.
        0x04..=0x38 | 0x64 | 0x87 | 0x89 if let Some(l) = layout.legend(kc as u8).filter(|l| l.base != ' ') => {
            if l.base.is_alphabetic() && l.shift == l.base.to_uppercase().next().unwrap_or(l.shift) {
                l.shift.to_string()
            } else if l.shift != '\0' && l.shift != l.base {
                format!("{}\n{}", l.shift, l.base)
            } else {
                l.base.to_string()
            }
        }
        0x04..=0xA4 | 0xE0..=0xE7 => key_legend(kc as u8),
        _ if (KVM_ACTION..KVM_ACTION + KVM_ACTIONS).contains(&kc) => {
            match HotkeyAction::ALL[(kc - KVM_ACTION) as usize] {
                HotkeyAction::TogglePc => "⇄".into(),
                HotkeyAction::NextHost => "H→".into(),
                HotkeyAction::PrevHost => "←H".into(),
                HotkeyAction::SelectHost(Host::Pc1) => "PC1".into(),
                HotkeyAction::SelectHost(Host::Pc2) => "PC2".into(),
                HotkeyAction::SelectHost(h) => format!("H{}", h.bt_slot().unwrap_or(0)),
                HotkeyAction::NextProfile => "P→".into(),
                HotkeyAction::PrevProfile => "←P".into(),
                HotkeyAction::SelectProfile(n) => format!("P{n}"),
                HotkeyAction::Aux2PortMode => "A2".into(),
                HotkeyAction::None => "—".into(),
            }
        }
        0x00D1..=0x00D8 => format!("M{}", kc - KC_BTN1 + 1),
        0x7E00..=0x7E07 => format!("M{}", kc - KVM_BTN9 + 9),
        0x7400..=0x741F => {
            pad_name((kc - QK_JOYSTICK) as usize, style).unwrap_or_else(|| format!("P{}", kc - QK_JOYSTICK + 1))
        }
        0x7E08..=0x7E0B => format!("D{}", ARROWS[(kc - KVM_DPAD) as usize]),
        _ => keycodes::keycode_name(kc),
    }
}

/// A full-size keyboard, row by row: (usage, width in quarter keys, gap before it in quarter keys).
const KEYBOARD: [&[(u8, u8, u8)]; 6] = [
    &[
        (0x29, 4, 0),
        (0x3A, 4, 4),
        (0x3B, 4, 0),
        (0x3C, 4, 0),
        (0x3D, 4, 0),
        (0x3E, 4, 2),
        (0x3F, 4, 0),
        (0x40, 4, 0),
        (0x41, 4, 0),
        (0x42, 4, 2),
        (0x43, 4, 0),
        (0x44, 4, 0),
        (0x45, 4, 0),
        (0x46, 4, 1),
        (0x47, 4, 0),
        (0x48, 4, 0),
    ],
    &[
        (0x35, 4, 0),
        (0x1E, 4, 0),
        (0x1F, 4, 0),
        (0x20, 4, 0),
        (0x21, 4, 0),
        (0x22, 4, 0),
        (0x23, 4, 0),
        (0x24, 4, 0),
        (0x25, 4, 0),
        (0x26, 4, 0),
        (0x27, 4, 0),
        (0x2D, 4, 0),
        (0x2E, 4, 0),
        (0x2A, 8, 0),
        (0x49, 4, 1),
        (0x4A, 4, 0),
        (0x4B, 4, 0),
        (0x53, 4, 1),
        (0x54, 4, 0),
        (0x55, 4, 0),
        (0x56, 4, 0),
    ],
    &[
        (0x2B, 6, 0),
        (0x14, 4, 0),
        (0x1A, 4, 0),
        (0x08, 4, 0),
        (0x15, 4, 0),
        (0x17, 4, 0),
        (0x1C, 4, 0),
        (0x18, 4, 0),
        (0x0C, 4, 0),
        (0x12, 4, 0),
        (0x13, 4, 0),
        (0x2F, 4, 0),
        (0x30, 4, 0),
        (0x31, 6, 0),
        (0x4C, 4, 1),
        (0x4D, 4, 0),
        (0x4E, 4, 0),
        (0x5F, 4, 1),
        (0x60, 4, 0),
        (0x61, 4, 0),
        (0x57, 4, 0),
    ],
    &[
        (0x39, 7, 0),
        (0x04, 4, 0),
        (0x16, 4, 0),
        (0x07, 4, 0),
        (0x09, 4, 0),
        (0x0A, 4, 0),
        (0x0B, 4, 0),
        (0x0D, 4, 0),
        (0x0E, 4, 0),
        (0x0F, 4, 0),
        (0x33, 4, 0),
        (0x34, 4, 0),
        (0x28, 9, 0),
        (0x5C, 4, 14),
        (0x5D, 4, 0),
        (0x5E, 4, 0),
    ],
    &[
        (0xE1, 9, 0),
        (0x1D, 4, 0),
        (0x1B, 4, 0),
        (0x06, 4, 0),
        (0x19, 4, 0),
        (0x05, 4, 0),
        (0x11, 4, 0),
        (0x10, 4, 0),
        (0x36, 4, 0),
        (0x37, 4, 0),
        (0x38, 4, 0),
        (0xE5, 11, 0),
        (0x52, 4, 5),
        (0x59, 4, 5),
        (0x5A, 4, 0),
        (0x5B, 4, 0),
        (0x58, 4, 0),
    ],
    &[
        (0xE0, 5, 0),
        (0xE3, 5, 0),
        (0xE2, 5, 0),
        (0x2C, 25, 0),
        (0xE6, 5, 0),
        (0xE7, 5, 0),
        (0x65, 5, 0),
        (0xE4, 5, 0),
        (0x50, 4, 1),
        (0x51, 4, 0),
        (0x4F, 4, 0),
        (0x62, 8, 1),
        (0x63, 4, 0),
    ],
];
/// The picture's rows for a keyboard's shape: ANSI's, with ISO's, JIS's or ABNT2's keys put in.
fn keyboard_rows(p: Physical) -> Vec<Vec<(u8, u8, u8)>> {
    let mut rows: Vec<Vec<(u8, u8, u8)>> = KEYBOARD.iter().map(|r| r.to_vec()).collect();
    let replace = |row: &mut Vec<(u8, u8, u8)>, usage: u8, with: &[(u8, u8, u8)]| {
        if let Some(i) = row.iter().position(|k| k.0 == usage) {
            row.splice(i..=i, with.iter().copied());
        }
    };
    if p != Physical::Ansi {
        // The two-row Enter: on the Q row where ANSI's backslash is, on the home row after the key left of it.
        replace(&mut rows[2], 0x31, &[(0x28, 6, 0)]);
        replace(&mut rows[3], 0x28, &[(0x32, 4, 0), (0x28, 5, 0)]);
    }
    match p {
        Physical::Iso | Physical::Abnt2 => replace(&mut rows[4], 0xE1, &[(0xE1, 5, 0), (0x64, 4, 0)]),
        Physical::Jis => {
            replace(&mut rows[1], 0x2A, &[(0x89, 4, 0), (0x2A, 4, 0)]);
            replace(&mut rows[5], 0x2C, &[(0x8B, 5, 0), (0x2C, 10, 0), (0x8A, 5, 0), (0x88, 5, 0)]);
        }
        Physical::Ansi => {}
    }
    if matches!(p, Physical::Jis | Physical::Abnt2) {
        replace(&mut rows[4], 0xE5, &[(0x87, 4, 0), (0xE5, 7, 0)]);
    }
    rows
}

/// Keys the picture leaves out: ISO's two, F13–F24, the international and language keys.
const OTHER_KEYS: [u8; 21] = [
    0x64, 0x32, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0x73, 0x87, 0x88, 0x89, 0x8A, 0x8B,
    0x90, 0x91,
];

/// Mouse buttons drawn on the mouse: (button 1–5, where in the shape, 0…1 each way).
const MOUSE_DRAWN: [(u16, [f32; 4]); 5] = [
    (1, [0.0, 0.0, 0.46, 0.38]),
    (2, [0.54, 0.0, 1.0, 0.38]),
    (3, [0.42, 0.08, 0.58, 0.3]),
    (4, [0.0, 0.42, 0.26, 0.54]),
    (5, [0.0, 0.57, 0.26, 0.69]),
];

/// The standard gamepad's buttons drawn on the gamepad: (button 1–17, centre, 0…1 each way, radius in points).
const PAD_DRAWN: [(u16, [f32; 2], f32); 13] = [
    (1, [0.78, 0.5], 11.0),
    (2, [0.86, 0.38], 11.0),
    (3, [0.7, 0.38], 11.0),
    (4, [0.78, 0.26], 11.0),
    (5, [0.22, 0.04], 13.0),
    (6, [0.78, 0.04], 13.0),
    (7, [0.14, -0.1], 11.0),
    (8, [0.86, -0.1], 11.0),
    (9, [0.42, 0.3], 9.0),
    (10, [0.58, 0.3], 9.0),
    (11, [0.25, 0.38], 22.0),
    (12, [0.63, 0.66], 22.0),
    (17, [0.5, 0.42], 9.0),
];
/// The hat as a d-pad: (direction 0–3, centre).
const DPAD_DRAWN: [(u16, [f32; 2]); 4] = [(0, [0.37, 0.56]), (1, [0.43, 0.65]), (2, [0.37, 0.74]), (3, [0.31, 0.65])];

/// The symmetric style: the d-pad above the left stick, the sticks side by side, a touch pad between Share and
/// Options, Home and Mute under it.
const SYMMETRIC_DRAWN: [(u16, [f32; 2], f32); 14] = [
    (1, [0.8, 0.44], 11.0),
    (2, [0.88, 0.32], 11.0),
    (3, [0.72, 0.32], 11.0),
    (4, [0.8, 0.2], 11.0),
    (5, [0.22, 0.04], 13.0),
    (6, [0.78, 0.04], 13.0),
    (7, [0.14, -0.1], 11.0),
    (8, [0.86, -0.1], 11.0),
    (9, [0.33, 0.1], 10.0),
    (10, [0.67, 0.1], 10.0),
    (11, [0.37, 0.66], 22.0),
    (12, [0.63, 0.66], 22.0),
    (17, [0.5, 0.5], 9.0),
    (19, [0.5, 0.62], 7.0),
];

/// Where a style draws a pad's controls.
struct PadLayout {
    buttons: &'static [(u16, [f32; 2], f32)],
    dpad: [(u16, [f32; 2]); 4],
    /// The sticks' centres: left, right.
    sticks: [[f32; 2]; 2],
    /// The touch pad: its button, centre and size.
    touch: Option<(u16, [f32; 2], [f32; 2])>,
}

const OFFSET_LAYOUT: PadLayout =
    PadLayout { buttons: &PAD_DRAWN, dpad: DPAD_DRAWN, sticks: [[0.25, 0.38], [0.63, 0.66]], touch: None };
const SYMMETRIC_LAYOUT: PadLayout = PadLayout {
    buttons: &SYMMETRIC_DRAWN,
    dpad: [(0, [0.2, 0.2]), (1, [0.26, 0.32]), (2, [0.2, 0.44]), (3, [0.14, 0.32])],
    sticks: [[0.37, 0.66], [0.63, 0.66]],
    touch: Some((18, [0.5, 0.22], [0.24, 0.28])),
};

fn pad_layout(style: Style) -> &'static PadLayout {
    match style {
        Style::Symmetric => &SYMMETRIC_LAYOUT,
        _ => &OFFSET_LAYOUT,
    }
}

/// A style's name in the window's language.
fn style_name(style: Style) -> String {
    match style {
        Style::Generic => fl!("pad-style-generic"),
        Style::Offset => fl!("pad-style-offset"),
        Style::Symmetric => fl!("pad-style-symmetric"),
    }
}

/// The choices of the action list: groups of (keycode, its name).
fn choices() -> Vec<(String, Vec<u16>)> {
    use kvm_map::keycode::{KC_BTN1, KVM_BTN9, KVM_DPAD, QK_JOYSTICK};
    let mouse = (0..8).map(|n| KC_BTN1 + n).chain((0..8).map(|n| KVM_BTN9 + n)).chain(0xCD..=0xDC).collect();
    let pad = (0..32).map(|n| QK_JOYSTICK + n).chain((0..4).map(|n| KVM_DPAD + n)).collect();
    let layers =
        [0x5220u16, 0x5260, 0x5200, 0x5280, 0x52C0].iter().flat_map(|op| (1..8).map(move |l| op + l)).collect();
    let media = (0xA5..=0xBE).collect();
    let kvm = (0..kvm_map::keycode::KVM_ACTIONS).map(|n| kvm_map::keycode::KVM_ACTION + n).collect();
    vec![
        (fl!("map-group-mouse"), mouse),
        (fl!("map-group-gamepad"), pad),
        (fl!("map-group-layers"), layers),
        (fl!("map-group-media"), media),
        (fl!("map-group-kvm"), kvm),
    ]
}

/// The axis inputs a picture's rules list shows.
fn inputs_of(kind: Kind) -> std::ops::Range<usize> {
    match kind {
        Kind::Keyboard => 0..0,
        Kind::Mouse => 0..axes::RELATIVE,
        Kind::Gamepad => axes::RELATIVE..axes::INPUTS,
    }
}

/// The devices a gamepad view may show: hosted front-port devices and bonded wireless ones.
fn pad_devices(s: &Snapshot) -> Vec<Device> {
    Device::ALL
        .iter()
        .copied()
        .filter(|d| match (d.port(), d.wireless()) {
            (Some(p), _) => s.ports.get(p.index()).is_some_and(|i| i.state == PortState::Hosted),
            (None, Some(n)) => s.devices.iter().any(|w| w.slot == n && w.bonded()),
            _ => false,
        })
        .collect()
}

/// The device a gamepad view starts with: a front port's gamepad, else the first device.
fn default_pad_device(s: &Snapshot) -> Option<Device> {
    let list = pad_devices(s);
    let gamepad = list.iter().copied().find(|d| {
        d.port().and_then(|p| s.ports.get(p.index())).is_some_and(|i| i.classes & kvm_proto::ctrl::class::GAMEPAD != 0)
    });
    gamepad.or(list.first().copied())
}

/// What a picture shows: the layer's keycodes and the controls held right now.
struct Shown<'a> {
    layer: &'a KeymapLayer,
    held: &'a std::collections::BTreeSet<u16>,
}

impl App {
    pub(crate) fn keymaps(&mut self, ui: &mut Ui, s: &Snapshot) {
        let locked = Self::locked_note(ui, s);
        let (layer, held, touched, live) = {
            let sh = self.shared.lock().unwrap();
            (sh.keymap.clone(), sh.held.clone(), sh.touched, sh.axes)
        };
        let (k, l) = (self.keymaps.keymap, self.keymaps.layer);
        if layer.as_ref().is_none_or(|m| (m.keymap, m.layer) != (k, l)) && self.keymaps.asked_layer != Some((k, l)) {
            self.keymaps.asked_layer = Some((k, l));
            self.send(UiCmd::LoadKeymap(k, l));
        }
        self.pad_style_now(s);
        // The keyboard's legends: the layout of the host this computer is (PC1 without one: the service port).
        let me = s.status.asking.unwrap_or(kvm_proto::ctrl::Host::Pc1);
        self.keymaps.legends = crate::config(s, |v| match v {
            ConfigValue::HostLayout(h, l) if *h == me => Layout::from_u8(*l),
            _ => None,
        })
        .unwrap_or_default();
        self.keymaps.legends_host = crate::screens::host_label(s, me);
        // The live picture: the controls are watched while the page shows, from the PC the input goes to.
        let active = kvm_proto::ctrl::Host::pc(s.status.active);
        self.keymaps.here = s.status.asking.is_none_or(|h| h == active) && s.status.bt_active == 0;
        if !self.keymaps.here || locked {
            self.keymaps.learning = None;
            self.keymaps.wizard = None;
        }
        let layers = self.keymaps.parts.section == Section::Layers;
        let want = match () {
            _ if !self.keymaps.here || locked || !layers => 0,
            _ if self.keymaps.learning.is_some() || self.keymaps.wizard.is_some() => 2,
            _ => 1,
        };
        let now = ui.input(|i| i.time);
        if self.keymaps.watching != want || (want != 0 && now - self.keymaps.watched_at >= WATCH_RENEW_S) {
            self.keymaps.watching = want;
            self.keymaps.watched_at = now;
            self.send(UiCmd::Watch(want));
        }
        if want != 0 {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
        }
        self.take_touch(touched, layer.as_ref(), now);
        // Escape on this computer's own keyboard too: one the KVM passes on without reading (mirrored, passthrough).
        let local_esc = ui.input(|i| i.key_down(egui::Key::Escape));
        self.learn_timers(&held, local_esc, layer.as_ref(), now);
        // A control pressed and let go between two frames still shows.
        let mut held = held;
        if let Some((p, at)) = touched.filter(|(_, at)| at.elapsed() < std::time::Duration::from_millis(FLASH_MS)) {
            held.insert(p);
            let _ = at;
        }
        ui.add_enabled_ui(!locked, |ui| {
            card(ui, &fl!("keymaps"), |ui| {
                note(ui, fl!("keymaps-note"));
                self.map_sections(ui);
                if !layers {
                    return;
                }
                ui.separator();
                if !self.keymaps.here {
                    ui.label(RichText::new(fl!("map-not-active")).color(ink(ui, WARN)));
                }
                self.keymap_header(ui, s);
            });
            ui.add_space(8.0);
            if !layers {
                self.map_part(ui, s);
                return;
            }
            let Some(m) = layer.filter(|m| (m.keymap, m.layer) == (k, l)) else {
                ui.spinner();
                return;
            };
            if self.keymaps.learning.is_some() {
                self.learn_banner(ui, &held, now);
                ui.add_space(8.0);
            }
            self.keymap_reach(ui, s);
            let (keymap_n, layer_n): (u32, u32) = (k as u32, l as u32);
            card(ui, &fl!("keymap-layer-title", keymap = keymap_n, layer = layer_n), |ui| {
                ui.horizontal(|ui| {
                    for (kind, name) in [
                        (Kind::Keyboard, fl!("map-keyboard")),
                        (Kind::Mouse, fl!("map-mouse")),
                        (Kind::Gamepad, fl!("map-gamepad")),
                    ] {
                        ui.selectable_value(&mut self.keymaps.kind, kind, name);
                    }
                    ui.separator();
                    let mut learn = self.keymaps.learning.is_some();
                    let here = self.keymaps.here;
                    let toggle = ui.add_enabled_ui(here, |ui| ui.toggle_value(&mut learn, fl!("map-learn"))).inner;
                    if toggle.on_hover_text(fl!("map-learn-hover")).changed() {
                        self.map_learn(learn.then_some(Learning::Select), now);
                    }
                });
                ui.add_space(4.0);
                match self.keymaps.kind {
                    Kind::Keyboard => self.draw_keyboard(ui, &m, &held),
                    Kind::Mouse => self.draw_mouse(ui, &m, &held),
                    Kind::Gamepad => self.draw_gamepad(ui, s, &m, &held, &live),
                }
            });
            ui.add_space(8.0);
            if let Some(p) = self.keymaps.selected {
                // A key by its US name, and what it types on this computer's layout where that differs.
                let mut name = keycodes::control_name(p);
                if (p as usize) < kvm_map::KEYBOARD
                    && let Some(l) = self.keymaps.legends.legend(p as u8)
                    && Layout::Us.legend(p as u8).is_some_and(|us| us.base != l.base)
                {
                    name = format!("{name} ({})", l.base);
                }
                card(ui, &fl!("map-control", control = name), |ui| self.action_panel(ui, &m, p));
                ui.add_space(8.0);
            }
            if self.keymaps.kind != Kind::Keyboard {
                card(ui, &fl!("map-axes"), |ui| self.axis_rules(ui, &m, &live));
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
    }

    /// While learning: what the next control does, the time left, how to stop, and what is held right now.
    fn learn_banner(&mut self, ui: &mut Ui, held: &std::collections::BTreeSet<u16>, now: f64) {
        let left = (self.keymaps.learn_until - now).max(0.0).ceil() as u32;
        let title = match (self.keymaps.learning, self.keymaps.selected) {
            (Some(Learning::Assign), Some(p)) => {
                fl!("map-learn-assign", control = keycodes::control_name(p), seconds = left)
            }
            _ => fl!("map-learn-select", seconds = left),
        };
        card(ui, &title, |ui| {
            let names: Vec<String> = held.iter().map(|p| keycodes::control_name(*p)).collect();
            let text = if names.is_empty() {
                fl!("map-learn-none-held")
            } else {
                fl!("map-learn-held", controls = names.join(" + "))
            };
            ui.label(RichText::new(text).strong().size(16.0));
            match self.keymaps.esc_down {
                Some(t) => {
                    let rest = (ESC_HOLD_S - (now - t)).max(0.0).ceil() as u32;
                    ui.label(RichText::new(fl!("map-learn-escape-held", seconds = rest)).color(ink(ui, WARN)));
                }
                None => note(ui, fl!("map-learn-escape")),
            }
            if ui.button(fl!("map-learn-stop")).clicked() {
                self.map_learn(None, now);
            }
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// Whether this keymap reaches anything: the active profile uses it, and the devices go through keymaps (only
    /// Normalized ones do); a button for each that doesn't.
    fn keymap_reach(&mut self, ui: &mut Ui, s: &Snapshot) {
        use kvm_proto::ctrl::{PortMode, PortState};
        let Some((active, _, profiles)) = s.profiles() else { return };
        let Some(profile) = profiles.get(active as usize) else { return };
        let k = self.keymaps.keymap;
        let unused = profile.keymap != Some(k);
        // Front ports with a device that runs in another mode; a wireless device always runs Normalized unless off.
        let others: Vec<(Device, PortMode)> = s
            .ports
            .iter()
            .filter(|p| matches!(p.state, PortState::Hosted | PortState::Passthrough))
            .filter(|p| p.effective != PortMode::Normalized && p.effective != PortMode::Off)
            .map(|p| (Device::from(p.port), p.effective))
            .collect();
        if !unused && others.is_empty() {
            return;
        }
        card(ui, "", |ui| {
            if unused {
                let uses =
                    profile.keymap.map_or_else(|| fl!("map-keymap-no"), |n| fl!("map-keymap-n", n = u32::from(n)));
                let line = fl!("map-keymap-unused", profile = profile.name.clone(), uses = uses);
                ui.label(RichText::new(line).color(ink(ui, WARN)));
                if ui.button(fl!("map-use-here", profile = profile.name.clone())).clicked() {
                    self.set(ConfigValue::ProfileKeymap(active, Some(k)));
                }
            }
            if !others.is_empty() {
                let list: Vec<String> =
                    others.iter().map(|(d, m)| format!("{} ({})", names::device_title(*d), names::mode(*m))).collect();
                ui.label(RichText::new(fl!("map-not-normalized", devices = list.join(", "))).color(ink(ui, WARN)));
                ui.horizontal_wrapped(|ui| {
                    for (d, _) in &others {
                        if ui.button(fl!("map-normalize", device = names::device_title(*d))).clicked() {
                            self.set(ConfigValue::Mode(*d, PortMode::Normalized));
                        }
                    }
                });
            }
        });
        ui.add_space(8.0);
    }

    /// The keymap, the profiles that use it, the layer, the virtual gamepad.
    fn keymap_header(&mut self, ui: &mut Ui, s: &Snapshot) {
        ui.horizontal_wrapped(|ui| {
            ui.label(fl!("map-keymap"));
            for k in 0..kvm_map::KEYMAPS as u8 {
                if ui.selectable_label(self.keymaps.keymap == k, k.to_string()).clicked() {
                    self.keymaps.keymap = k;
                    self.keymaps.selected = None;
                }
            }
            ui.separator();
            ui.label(fl!("map-layer"));
            for l in 0..kvm_map::LAYERS as u8 {
                if ui.selectable_label(self.keymaps.layer == l, l.to_string()).clicked() {
                    self.keymaps.layer = l;
                }
            }
        });
        if let Some((_, count, profiles)) = s.profiles() {
            ui.horizontal_wrapped(|ui| {
                ui.label(fl!("map-used-by"));
                for (i, p) in profiles.iter().enumerate().take((count as usize).min(MAX_PROFILES)) {
                    let mut on = p.keymap == Some(self.keymaps.keymap);
                    if ui.checkbox(&mut on, &p.name).on_hover_text(fl!("map-used-by-hover")).changed() {
                        let k = on.then_some(self.keymaps.keymap);
                        self.set(ConfigValue::ProfileKeymap(i as u8, k));
                    }
                }
            });
        }
        let gamepad = s.config.iter().find_map(|v| match v {
            ConfigValue::Gamepad(g) => Some(*g),
            _ => None,
        });
        if let Some(mut g) = gamepad {
            ui.horizontal(|ui| {
                ui.label(fl!("map-virtual-gamepad"));
                let before = g;
                ui.radio_value(&mut g, GamepadOut::Off, fl!("map-gamepad-off"));
                ui.radio_value(&mut g, GamepadOut::Hid, fl!("map-gamepad-hid"));
                ui.radio_value(&mut g, GamepadOut::XInput, "XInput");
                if g != before {
                    self.set(ConfigValue::Gamepad(g));
                }
            });
            note(ui, fl!("map-virtual-gamepad-note"));
        }
    }

    /// A control pressed: taken by learning, if it runs.
    fn take_touch(&mut self, touched: Option<(u16, Instant)>, m: Option<&KeymapLayer>, now: f64) {
        let Some((p, at)) = touched else { return };
        if self.keymaps.touched_seen == Some(at) {
            return;
        }
        self.keymaps.touched_seen = Some(at);
        if self.keymaps.learning.is_some() {
            self.keymaps.learn_until = now + LEARN_S;
            // Escape waits: held, it stops learning; let go sooner, it is the control pressed ([`Self::escape`]).
            if p == ESC {
                self.keymaps.esc_down = Some(now);
                return;
            }
        }
        self.learned(p, m);
    }

    /// Escape while learning: held long enough, learning stops; let go sooner, it counts as pressed. The countdown too.
    fn learn_timers(
        &mut self,
        held: &std::collections::BTreeSet<u16>,
        local_esc: bool,
        m: Option<&KeymapLayer>,
        now: f64,
    ) {
        let esc = held.contains(&ESC) || local_esc;
        if local_esc && self.keymaps.learning.is_some() && self.keymaps.esc_down.is_none() {
            self.keymaps.esc_down = Some(now);
        }
        if let Some(t) = self.keymaps.esc_down {
            if !esc {
                self.keymaps.esc_down = None;
                self.learned(ESC, m);
            } else if now - t >= ESC_HOLD_S {
                self.keymaps.esc_down = None;
                self.keymaps.learning = None;
            }
        }
        // Started from outside the page (the screenshots): the countdown starts now.
        if self.keymaps.learn_until == 0.0 {
            self.keymaps.learn_until = now + LEARN_S;
        }
        if self.keymaps.learning.is_some() && now >= self.keymaps.learn_until {
            self.keymaps.learning = None;
            self.keymaps.esc_down = None;
        }
    }

    /// Starts learning (`what`), or stops it.
    fn map_learn(&mut self, what: Option<Learning>, now: f64) {
        self.keymaps.learning = what;
        self.keymaps.learn_until = now + LEARN_S;
        self.keymaps.esc_down = None;
    }

    /// A control pressed while learning: selected, or its keycode given to the selected one.
    fn learned(&mut self, p: u16, m: Option<&KeymapLayer>) {
        match self.keymaps.learning {
            Some(Learning::Select) => {
                self.keymaps.selected = Some(p);
                self.keymaps.typed_error = None;
                self.keymaps.kind = match p {
                    _ if (p as usize) < kvm_map::KEYBOARD => Kind::Keyboard,
                    _ if p < pos::PAD => Kind::Mouse,
                    _ if p < pos::AXIS => Kind::Gamepad,
                    _ if p < pos::AXIS + 2 * axes::RELATIVE as u16 => Kind::Mouse,
                    _ => Kind::Gamepad,
                };
            }
            Some(Learning::Assign) => {
                if let (Some(sel), Some(m)) = (self.keymaps.selected, m) {
                    let kc = identity(p as usize);
                    if kc != KC_NO {
                        self.send(UiCmd::SetKeycode(m.keymap, m.layer, sel, kc));
                    }
                }
                self.keymaps.learning = Some(Learning::Select);
            }
            None => {}
        }
    }

    /// One control in a picture: its keycode's legend, coloured when it does something else than it says, lit while
    /// held, outlined when selected. Clicking it selects it.
    fn control(&mut self, ui: &Ui, painter: &egui::Painter, r: Rect, p: u16, v: &Shown, round: f32) {
        let (m, held) = (v.layer, v.held.contains(&p));
        let kc = m.codes.get(p as usize).copied().unwrap_or(KC_NO);
        let fresh = if m.layer == 0 { identity(p as usize) } else { KC_TRNS };
        let dark = ui.visuals().dark_mode;
        let accent = ui.visuals().selection.bg_fill;
        let mut fill = ui.visuals().widgets.inactive.weak_bg_fill;
        if kc != fresh {
            fill = WARN.gamma_multiply(if dark { 0.35 } else { 0.25 });
        }
        if held {
            fill = GOOD.gamma_multiply(if dark { 0.6 } else { 0.45 });
        }
        painter.rect_filled(r, CornerRadius::same(round as u8), fill);
        let selected = self.keymaps.selected == Some(p);
        let stroke = if selected { Stroke::new(2.0, accent) } else { Stroke::new(1.0, palette(ui).card_stroke) };
        painter.rect_stroke(r, CornerRadius::same(round as u8), stroke, StrokeKind::Inside);
        // An axis direction that does nothing: which one it is.
        let (text, weak) = if kc == KC_NO && p >= pos::AXIS {
            (keycodes::control_name(p), true)
        } else {
            (code_legend(kc, self.keymaps.legends, self.keymaps.pad_style), kc == KC_TRNS)
        };
        let color = if weak { palette(ui).weak } else { ui.visuals().text_color() };
        let size = match text.chars().count() {
            _ if text.contains('\n') => 9.5,
            0..=2 => 12.0,
            3 => 10.5,
            _ => 9.0,
        };
        match SHAPES.iter().position(|x| *x == text) {
            // The face buttons' shapes, painted: fonts needn't have them.
            Some(n) => paint_shape(painter, r.center(), r.width().min(r.height()) * 0.24, n, color),
            None => {
                painter.text(r.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(size), color);
            }
        }
        // A control drawn twice (ISO's Enter) is two widgets.
        let id = ui.scope_id().with(("control", p, r.min.x as i32, r.min.y as i32));
        if ui.interact(r, id, Sense::click()).on_hover_text(keycodes::keycode_name(kc)).clicked() {
            self.keymaps.selected = Some(p);
            self.keymaps.typed_error = None;
        }
    }

    fn draw_keyboard(&mut self, ui: &mut Ui, m: &KeymapLayer, held: &std::collections::BTreeSet<u16>) {
        let v = Shown { layer: m, held };
        let shape = self.keyboard_shape(ui);
        let rows = keyboard_rows(shape);
        let unit = ((ui.available_width() - 8.0) / 94.0).clamp(4.0, 10.0);
        let size = Vec2::new(94.0 * unit, 27.5 * unit);
        let (resp, painter) = ui.allocate_painter(size, Sense::hover());
        let origin = resp.rect.min;
        for (row, keys) in rows.iter().enumerate() {
            let mut x = 0.0;
            let y =
                row as f32 * 4.0 * unit * if row == 0 { 1.0 } else { 1.05 } + if row > 0 { 1.5 * unit } else { 0.0 };
            for &(u, w, gap) in keys.iter() {
                x += gap as f32 * unit;
                let r =
                    Rect::from_min_size(origin + Vec2::new(x, y), Vec2::new(w as f32 * unit - 2.0, 4.0 * unit - 2.0));
                self.control(ui, &painter, r, u as u16, &v, 4.0);
                x += w as f32 * unit;
            }
        }
        ui.add_space(6.0);
        note(ui, fl!("map-other-keys"));
        ui.horizontal_wrapped(|ui| {
            for u in OTHER_KEYS.into_iter().filter(|u| !rows.iter().flatten().any(|k| k.0 == *u)) {
                let (r, painter) = ui.allocate_painter(Vec2::new(4.0 * unit + 12.0, 4.0 * unit - 2.0), Sense::hover());
                self.control(ui, &painter, r.rect, u as u16, &v, 4.0);
            }
        });
    }

    /// The keyboard picture's shape and legends, chosen above it: the shape as this computer keeps it (or the one its
    /// layout comes with), the legends of the layout of the host this computer is.
    fn keyboard_shape(&mut self, ui: &mut Ui) -> Physical {
        let layout = self.keymaps.legends;
        let kept = self.local.keyboard.as_deref().and_then(Physical::from_name);
        let mut shape = kept;
        ui.horizontal(|ui| {
            ui.label(fl!("map-physical"));
            let auto = fl!("map-physical-auto", physical = layout.physical().name().to_uppercase());
            let text = shape.map_or_else(|| auto.clone(), |p| p.name().to_uppercase());
            egui::ComboBox::from_id_salt("keyboard-shape").selected_text(text).show_ui(ui, |ui| {
                ui.selectable_value(&mut shape, None, auto);
                for p in Physical::ALL {
                    ui.selectable_value(&mut shape, Some(p), p.name().to_uppercase());
                }
            });
            ui.separator();
            let host = self.keymaps.legends_host.clone();
            ui.label(
                RichText::new(fl!("map-legends", layout = layout_name(layout), host = host)).color(palette(ui).weak),
            );
        });
        if shape != kept {
            self.local.keyboard = shape.map(|p| p.name().to_string());
            let ctx = ui.ctx().clone();
            self.local_changed(&ctx);
        }
        shape.unwrap_or(layout.physical())
    }

    fn draw_mouse(&mut self, ui: &mut Ui, m: &KeymapLayer, held: &std::collections::BTreeSet<u16>) {
        let v = Shown { layer: m, held };
        ui.horizontal(|ui| {
            let size = Vec2::new(190.0, 240.0);
            let (resp, painter) = ui.allocate_painter(size, Sense::hover());
            let body = Rect::from_min_size(resp.rect.min + Vec2::new(45.0, 10.0), Vec2::new(130.0, 220.0));
            painter.rect_filled(body, CornerRadius::same(60), ui.visuals().faint_bg_color);
            painter.rect_stroke(
                body,
                CornerRadius::same(60),
                Stroke::new(1.0, palette(ui).card_stroke),
                StrokeKind::Inside,
            );
            for (b, [x0, y0, x1, y1]) in MOUSE_DRAWN {
                let r = Rect::from_min_max(
                    body.min + Vec2::new(x0 * body.width(), y0 * body.height()),
                    body.min + Vec2::new(x1 * body.width(), y1 * body.height()),
                );
                let p = pos::MOUSE + b - 1;
                self.control(ui, &painter, r.shrink(2.0), p, &v, 8.0);
            }
            ui.vertical(|ui| {
                note(ui, fl!("map-more-buttons"));
                ui.horizontal_wrapped(|ui| {
                    for b in 6..=pos::MOUSE_BUTTONS {
                        let p = pos::MOUSE + b - 1;
                        let (r, painter) = ui.allocate_painter(Vec2::new(64.0, 26.0), Sense::hover());
                        self.control(ui, &painter, r.rect, p, &v, 6.0);
                    }
                });
                note(ui, fl!("map-axis-buttons"));
                ui.horizontal_wrapped(|ui| {
                    for i in 0..axes::RELATIVE as u16 {
                        for d in 0..2 {
                            let p = pos::AXIS + 2 * i + d;
                            let (r, painter) = ui.allocate_painter(Vec2::new(64.0, 26.0), Sense::hover());
                            self.control(ui, &painter, r.rect, p, &v, 6.0);
                        }
                    }
                });
            });
        });
    }

    fn draw_gamepad(
        &mut self,
        ui: &mut Ui,
        s: &Snapshot,
        m: &KeymapLayer,
        held: &std::collections::BTreeSet<u16>,
        live: &[i16; axes::INPUTS],
    ) {
        let v = Shown { layer: m, held };
        let view = self.gamepad_device(ui, s);
        self.pad_look(ui);
        let style = self.keymaps.pad_style;
        let layout = pad_layout(style);
        // Without a gamepad (none chosen, or one without buttons): the standard gamepad's, for a keymap made ahead.
        let pad = view.as_ref().map(|v| v.controls).filter(|c| c.pad_buttons != 0 || c.pad_axes != 0);
        let has = |b: u16| match pad {
            Some(c) => c.pad_buttons & (1 << (b - 1)) != 0,
            None => b <= 17 || (style == Style::Symmetric && b <= kvm_core::pads::TOUCH_BUTTON as u16),
        };
        let size = Vec2::new(ui.available_width().min(460.0), 250.0);
        let (resp, painter) = ui.allocate_painter(size, Sense::hover());
        let area = Rect::from_min_size(resp.rect.min + Vec2::new(0.0, 40.0), Vec2::new(size.x, 190.0));
        let body = |x0: f32, x1: f32| {
            Rect::from_min_max(
                Pos2::new(area.left() + x0 * area.width(), area.top()),
                Pos2::new(area.left() + x1 * area.width(), area.bottom()),
            )
        };
        for r in [body(0.04, 0.44), body(0.56, 0.96), body(0.2, 0.8).with_max_y(area.top() + 0.6 * area.height())] {
            painter.rect_filled(r, CornerRadius::same(48), ui.visuals().faint_bg_color);
        }
        let at = |c: [f32; 2]| Pos2::new(area.left() + c[0] * area.width(), area.top() + c[1] * area.height());
        let mut drawn: Vec<u16> = Vec::new();
        for &(b, c, radius) in layout.buttons {
            if !has(b) {
                continue;
            }
            let r = Rect::from_center_size(at(c), Vec2::splat(2.0 * radius));
            self.control(ui, &painter, r, pos::PAD + b - 1, &v, radius);
            drawn.push(b);
        }
        if let Some((b, c, sz)) = layout.touch
            && has(b)
        {
            let r = Rect::from_center_size(at(c), Vec2::new(sz[0] * area.width(), sz[1] * area.height()));
            self.control(ui, &painter, r, pos::PAD + b - 1, &v, 8.0);
            drawn.push(b);
        }
        // The sticks' deflection over their buttons: X/Y the left one, Rx/Ry the right one.
        for (c, (x, y)) in [
            (layout.sticks[0], (axes::input::X, axes::input::Y)),
            (layout.sticks[1], (axes::input::RX, axes::input::RY)),
        ] {
            let d = Vec2::new(live[x] as f32, live[y] as f32) / 32767.0 * 18.0;
            painter.circle_filled(at(c) + d, 4.0, ui.visuals().selection.bg_fill);
        }
        if pad.is_none_or(|c| c.hat) {
            for (d, c) in layout.dpad {
                let r = Rect::from_center_size(at(c), Vec2::splat(20.0));
                let p = pos::HAT + d;
                self.control(ui, &painter, r, p, &v, 4.0);
            }
        }
        // Every other button the device has, with the names the user gave them.
        let others: Vec<u16> =
            (13..=16).chain(18..=pos::PAD_BUTTONS).filter(|b| has(*b) && !drawn.contains(b)).collect();
        if !others.is_empty() {
            note(ui, fl!("map-extra-buttons"));
            let id = pad.map(|c| format!("{:08x}", c.identity));
            egui::Grid::new("pad-extras").num_columns(4).spacing([8.0, 4.0]).show(ui, |ui| {
                for (n, b) in others.iter().enumerate() {
                    let p = pos::PAD + b - 1;
                    let (r, painter) = ui.allocate_painter(Vec2::new(64.0, 24.0), Sense::hover());
                    self.control(ui, &painter, r.rect, p, &v, 6.0);
                    if let Some(id) = &id {
                        let mut name =
                            self.local.control_names.get(id).and_then(|n| n.get(&p)).cloned().unwrap_or_default();
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut name).hint_text(fl!("map-name-hint")).desired_width(110.0),
                        );
                        if edit.changed() {
                            let names = self.local.control_names.entry(id.clone()).or_default();
                            if name.trim().is_empty() {
                                names.remove(&p);
                            } else {
                                names.insert(p, name);
                            }
                        }
                        if edit.lost_focus() {
                            let ctx = ui.ctx().clone();
                            self.local_changed(&ctx);
                        }
                    }
                    if n % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        }
        if let Some(v) = view {
            ui.add_space(6.0);
            self.calibration(ui, &v, live);
        }
    }

    /// How the gamepad is drawn: as its device is (`kvm_core::pads`), or as the user chose for it.
    fn pad_look(&mut self, ui: &mut Ui) {
        let key = self.keymaps.pad_key.clone();
        let kept = self.local.pad_styles.get(&key).and_then(|s| Style::from_name(s));
        let mut choice = kept;
        ui.horizontal(|ui| {
            ui.label(fl!("map-pad-look"));
            let auto = fl!("map-pad-look-auto", style = style_name(self.keymaps.pad_auto));
            let text = choice.map_or_else(|| auto.clone(), style_name);
            egui::ComboBox::from_id_salt("pad-look").selected_text(text).show_ui(ui, |ui| {
                ui.selectable_value(&mut choice, None, auto);
                for st in Style::ALL {
                    ui.selectable_value(&mut choice, Some(st), style_name(st));
                }
            });
        });
        if choice != kept {
            match choice {
                Some(st) => self.local.pad_styles.insert(key, st.name().to_string()),
                None => self.local.pad_styles.remove(&key),
            };
            self.keymaps.pad_style = choice.unwrap_or(self.keymaps.pad_auto);
            let ctx = ui.ctx().clone();
            self.local_changed(&ctx);
        }
    }

    /// The style of the gamepad the page shows: by the device (its VID/PID, or XInput), unless the user chose one.
    fn pad_style_now(&mut self, s: &Snapshot) {
        if self.keymaps.pad_device.is_none() {
            self.keymaps.pad_device = default_pad_device(s);
        }
        let d = self.keymaps.pad_device;
        let auto = d.and_then(|d| d.port()).and_then(|p| s.ports.get(p.index())).map_or(Style::Generic, |i| {
            kvm_core::pads::profile(i.vid, i.pid, i.classes & kvm_proto::ctrl::class::XINPUT != 0).style
        });
        let identity = match self.shared.lock().unwrap().pad.as_ref() {
            Some(Ok(v)) if Some(v.device) == d => Some(v.controls.identity),
            _ => None,
        };
        self.keymaps.pad_key = identity.map_or_else(|| "none".to_string(), |i| format!("{i:08x}"));
        self.keymaps.pad_auto = auto;
        let kept = self.local.pad_styles.get(&self.keymaps.pad_key).and_then(|s| Style::from_name(s));
        self.keymaps.pad_style = kept.unwrap_or(auto);
    }

    /// The device the gamepad view shows: chosen among the ports' and the wireless devices'; its controls read.
    fn gamepad_device(&mut self, ui: &mut Ui, s: &Snapshot) -> Option<DeviceView> {
        let present: Vec<(Device, String)> = pad_devices(s).into_iter().map(|d| (d, names::device_title(d))).collect();
        if self.keymaps.pad_device.is_none_or(|d| !present.iter().any(|(p, _)| *p == d)) {
            self.keymaps.pad_device = default_pad_device(s);
        }
        let mut chosen = self.keymaps.pad_device;
        ui.horizontal(|ui| {
            ui.label(fl!("map-device"));
            let title = chosen.map_or_else(|| fl!("map-no-device"), names::device_title);
            egui::ComboBox::from_id_salt("pad-device").selected_text(title).show_ui(ui, |ui| {
                for (d, name) in &present {
                    ui.selectable_value(&mut chosen, Some(*d), name);
                }
            });
        });
        self.keymaps.pad_device = chosen;
        let d = chosen?;
        let pad = self.shared.lock().unwrap().pad.clone();
        match pad {
            Some(Ok(v)) if v.device == d => Some(v),
            Some(Err(e)) => {
                note(ui, e);
                None
            }
            _ => {
                if self.keymaps.asked_pad != Some(d) {
                    self.keymaps.asked_pad = Some(d);
                    self.send(UiCmd::LoadDevice(d));
                }
                None
            }
        }
    }

    /// The gamepad's axes: calibration per axis, and the wizard that measures it.
    fn calibration(&mut self, ui: &mut Ui, v: &DeviceView, live: &[i16; axes::INPUTS]) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(fl!("map-calibration")).strong());
            match &mut self.keymaps.wizard {
                None => {
                    if ui.add_enabled(self.keymaps.here, egui::Button::new(fl!("map-calibrate"))).clicked() {
                        for a in 0..kvm_hid::gamepad::AXES as u8 {
                            self.send(UiCmd::SetCalibration(v.device, a, AxisCal::NONE));
                        }
                        self.keymaps.wizard = Some(Wizard {
                            device: v.device,
                            seen: [(i16::MAX, i16::MIN); kvm_hid::gamepad::AXES],
                            resting_since: None,
                        });
                    }
                    if ui.button(fl!("map-calibration-reset")).clicked() {
                        for a in 0..kvm_hid::gamepad::AXES as u8 {
                            self.send(UiCmd::SetCalibration(v.device, a, AxisCal::NONE));
                        }
                    }
                }
                Some(w) => {
                    for (a, seen) in w.seen.iter_mut().enumerate() {
                        let x = live[axes::RELATIVE + a];
                        *seen = (seen.0.min(x), seen.1.max(x));
                    }
                    match w.resting_since {
                        None => {
                            ui.label(RichText::new(fl!("map-calibrate-move")).color(ink(ui, WARN)));
                            if ui.button(fl!("map-calibrate-done")).clicked() {
                                w.resting_since = Some(Instant::now());
                            }
                        }
                        Some(t) if t.elapsed().as_millis() < 1500 => {
                            ui.label(RichText::new(fl!("map-calibrate-let-go")).color(ink(ui, WARN)));
                        }
                        Some(_) => {
                            let (d, seen) = (w.device, w.seen);
                            for (a, (lo, hi)) in seen
                                .iter()
                                .enumerate()
                                .filter(|(a, (lo, hi))| v.controls.pad_axes & (1 << a) != 0 && lo < hi)
                            {
                                let trigger = kvm_hid::gamepad::axis::is_trigger(a);
                                let centre = if trigger { 0 } else { live[axes::RELATIVE + a] };
                                self.send(UiCmd::SetCalibration(
                                    d,
                                    a as u8,
                                    AxisCal { min: *lo, centre, max: *hi, deadzone: 8 },
                                ));
                            }
                            self.keymaps.wizard = None;
                        }
                    }
                }
            }
        });
        egui::Grid::new("pad-cal").num_columns(3).spacing([12.0, 4.0]).show(ui, |ui| {
            for a in (0..kvm_hid::gamepad::AXES).filter(|a| v.controls.pad_axes & (1 << a) != 0) {
                let c = v.cal[a];
                ui.label(keycodes::AXES[axes::RELATIVE + a]);
                ui.add(
                    egui::ProgressBar::new((live[axes::RELATIVE + a] as f32 / 65534.0 + 0.5).clamp(0.0, 1.0))
                        .desired_width(120.0),
                );
                note(ui, format!("{} … {} … {} · {}", c.min, c.centre, c.max, c.deadzone));
                ui.end_row();
            }
        });
    }

    /// What the selected control does: the quick choices, the list, learning, a typed keycode.
    fn action_panel(&mut self, ui: &mut Ui, m: &KeymapLayer, p: u16) {
        let kc = m.codes.get(p as usize).copied().unwrap_or(KC_NO);
        let (k, l) = (m.keymap, m.layer);
        ui.label(fl!("map-does", action = keycodes::keycode_name(kc)));
        let mut set = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button(fl!("map-as-it-says")).clicked() {
                set = Some(if l == 0 { identity(p as usize) } else { KC_TRNS });
            }
            if ui.button(fl!("map-nothing")).clicked() {
                set = Some(KC_NO);
            }
            if l > 0 && ui.button(fl!("map-transparent")).on_hover_text(fl!("map-transparent-hover")).clicked() {
                set = Some(KC_TRNS);
            }
            let assigning = self.keymaps.learning == Some(Learning::Assign);
            if ui
                .selectable_label(assigning, fl!("map-press-target"))
                .on_hover_text(fl!("map-press-target-hover"))
                .clicked()
            {
                let now = ui.input(|i| i.time);
                self.map_learn(if assigning { None } else { Some(Learning::Assign) }, now);
            }
            egui::ComboBox::from_id_salt("map-choices").selected_text(fl!("map-choose")).show_ui(ui, |ui| {
                for (group, codes) in choices() {
                    ui.label(RichText::new(group).strong());
                    for c in codes {
                        if ui.selectable_label(c == kc, keycodes::keycode_name(c)).clicked() {
                            set = Some(c);
                        }
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            let edit = ui.add(
                egui::TextEdit::singleline(&mut self.keymaps.typed)
                    .hint_text("ctrl+c · mo(1) · lt(1,space) · pad5")
                    .desired_width(220.0),
            );
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button(fl!("apply")).clicked() || enter {
                match keycodes::parse_keycode(&self.keymaps.typed) {
                    Ok(c) => {
                        set = Some(c);
                        self.keymaps.typed_error = None;
                    }
                    // Nothing typed: nothing to apply, and nothing wrong.
                    Err(_) if self.keymaps.typed.trim().is_empty() => self.keymaps.typed_error = None,
                    Err(e) => self.keymaps.typed_error = Some(e),
                }
            }
        });
        if let Some(e) = &self.keymaps.typed_error {
            ui.label(RichText::new(e).color(ink(ui, WARN)));
        }
        if let Some(c) = set {
            self.send(UiCmd::SetKeycode(k, l, p, c));
        }
    }

    /// The axis rules of the picture's inputs, on the layer shown.
    fn axis_rules(&mut self, ui: &mut Ui, m: &KeymapLayer, live: &[i16; axes::INPUTS]) {
        note(ui, fl!("map-axes-note"));
        let targets: Vec<Target> = [
            Target::Transparent,
            Target::None,
            Target::MouseX,
            Target::MouseY,
            Target::Wheel,
            Target::Pan,
            Target::Buttons,
        ]
        .into_iter()
        .chain((0..axes::PAD_AXES as u8).map(Target::Pad))
        .collect();
        egui::Grid::new("axis-rules").num_columns(7).spacing([8.0, 6.0]).show(ui, |ui| {
            for h in [
                fl!("map-axis"),
                fl!("map-live"),
                fl!("map-target"),
                fl!("map-scale"),
                fl!("map-invert"),
                fl!("map-deadzone"),
                fl!("map-param"),
            ] {
                ui.label(RichText::new(h).strong());
            }
            ui.end_row();
            for i in inputs_of(self.keymaps.kind) {
                let before = m.rules.get(i).copied().unwrap_or(Rule::TRANSPARENT);
                let mut r = before;
                ui.label(keycodes::AXES[i]);
                ui.add(egui::ProgressBar::new((live[i] as f32 / 65534.0 + 0.5).clamp(0.0, 1.0)).desired_width(70.0));
                egui::ComboBox::from_id_salt(("axis-target", i))
                    .selected_text(keycodes::target_name(r.target))
                    .show_ui(ui, |ui| {
                        for t in &targets {
                            ui.selectable_value(&mut r.target, *t, keycodes::target_name(*t));
                        }
                    });
                let mut scale = r.scale as f32 / 256.0;
                if ui.add(egui::DragValue::new(&mut scale).speed(0.01).range(-8.0..=8.0)).changed() {
                    r.scale = (scale * 256.0).round() as i16;
                }
                let mut invert = r.flags & flags::INVERT != 0;
                if ui.checkbox(&mut invert, "").changed() {
                    r.flags = (r.flags & !flags::INVERT) | (u8::from(invert) * flags::INVERT);
                }
                let mut dz = keycodes::percent(r.deadzone);
                if ui.add(egui::DragValue::new(&mut dz).range(0..=50).suffix(" %")).changed() {
                    r.deadzone = keycodes::in_256ths(dz);
                }
                let mut param = r.param;
                if ui.add(egui::DragValue::new(&mut param)).on_hover_text(fl!("map-param-hover")).changed() {
                    r.param = param;
                }
                if r != before {
                    // A rule set on a transparent one starts from the input's default.
                    if before.target == Target::Transparent && r.target != Target::Transparent && r.param == 0 {
                        r.param = axes::DEFAULT_PARAM;
                    }
                    self.send(UiCmd::SetAxisRule(m.keymap, m.layer, i as u8, r));
                }
                ui.end_row();
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keyboard_picture_shows_every_key_once() {
        let mut seen = std::collections::BTreeSet::new();
        for row in KEYBOARD {
            for (u, ..) in row {
                assert!(seen.insert(*u), "{u:#04x} twice");
            }
        }
        for u in OTHER_KEYS {
            assert!(seen.insert(u), "{u:#04x} twice");
        }
        // 104 keys plus the others.
        assert_eq!(seen.len(), 104 + OTHER_KEYS.len());
        // Every row is as wide as the top one's 22.5 keys (plus the keypad), within a quarter.
        let width = |row: &[(u8, u8, u8)]| row.iter().map(|(_, w, g)| (*w + *g) as u32).sum::<u32>();
        assert!(KEYBOARD.iter().skip(1).all(|r| width(r) <= width(KEYBOARD[1])));
    }
}
