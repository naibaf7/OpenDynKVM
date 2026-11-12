//! Device input reports → normalized input, and the verdict whether a device fits the normalized interfaces.
//!
//! One [`Translator`] per HID interface. Each report yields a [`Translation`] holding only what that report carries;
//! keyboards that split their keys across report IDs or interfaces are merged by the caller (`kvm-core`).

use heapless::Vec;

use crate::keyboard::KeyState;
use crate::mouse::MouseInput;
use crate::normalized::CONSUMER_SLOTS;
use crate::parser::{Field, Layout, MAX_FIELDS, ReportType};
use crate::usage::{consumer, gd, id_of, key, page, page_of, usage};

/// What kind of device an interface looks like (for the port summary and the Auto policy).
pub mod caps {
    pub const KEYBOARD: u16 = 1 << 0;
    pub const MOUSE: u16 = 1 << 1;
    pub const CONSUMER: u16 = 1 << 2;
    pub const SYSTEM: u16 = 1 << 3;
    pub const GAMEPAD: u16 = 1 << 4;
    /// Inputs on a vendor page that are not part of a command channel.
    pub const VENDOR_INPUT: u16 = 1 << 5;
    /// A vendor-page collection with output or feature reports (e.g. HID++).
    pub const VENDOR_CHANNEL: u16 = 1 << 6;
    pub const DIGITIZER: u16 = 1 << 7;
    /// Keyboard LED output report.
    pub const KEYBOARD_LEDS: u16 = 1 << 8;
    pub const OTHER: u16 = 1 << 9;
}

/// Why a device does not fit the normalized interfaces.
pub mod misfit {
    pub const BUTTONS_OVER_16: u16 = 1 << 0;
    pub const ABSOLUTE_POINTER: u16 = 1 << 1;
    pub const VENDOR_INPUT: u16 = 1 << 2;
    pub const GAMEPAD: u16 = 1 << 3;
    pub const DIGITIZER: u16 = 1 << 4;
    pub const UNKNOWN_USAGE: u16 = 1 << 5;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum Kind {
    Ignore,
    KeyBits,
    KeyArray,
    Buttons,
    Axes,
    ConsumerArray,
    ConsumerBits,
    SystemArray,
    SystemBits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Verdict {
    pub caps: u16,
    /// Empty when every input fits; `misfit` bits otherwise.
    pub misfit: u16,
}

impl Verdict {
    pub fn fits(&self) -> bool {
        self.misfit == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct ConsumerState {
    /// Active usages, 0 = empty slot.
    pub usages: [u16; CONSUMER_SLOTS],
}

impl ConsumerState {
    fn push(&mut self, u: u16) {
        if u == 0 || self.usages.contains(&u) {
            return;
        }
        if let Some(slot) = self.usages.iter_mut().find(|s| **s == 0) {
            *slot = u;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.usages.iter().all(|u| *u == 0)
    }
}

/// Normalized content of one input report. `None` = this report doesn't carry that kind of input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Translation {
    pub report_id: u8,
    pub keyboard: Option<KeyState>,
    pub mouse: Option<MouseInput>,
    pub consumer: Option<ConsumerState>,
    /// `normalized::system` bits.
    pub system: Option<u8>,
}

pub struct Translator {
    layout: Layout,
    kinds: Vec<Kind, MAX_FIELDS>,
    verdict: Verdict,
}

fn app_is(field: &Field, gd_usage: u16) -> bool {
    field.application == usage(page::GENERIC_DESKTOP, gd_usage)
}

fn is_gamepad_app(field: &Field) -> bool {
    app_is(field, gd::JOYSTICK) || app_is(field, gd::GAMEPAD) || app_is(field, gd::MULTI_AXIS)
}

fn system_bit(u: u32) -> Option<u8> {
    if page_of(u) != page::GENERIC_DESKTOP {
        return None;
    }
    match id_of(u) {
        gd::SYSTEM_POWER_DOWN => Some(crate::normalized::system::POWER_DOWN),
        gd::SYSTEM_SLEEP => Some(crate::normalized::system::SLEEP),
        gd::SYSTEM_WAKE_UP => Some(crate::normalized::system::WAKE_UP),
        _ => None,
    }
}

/// Collection usages of vendor command channels: vendor-page applications with output or feature reports.
fn vendor_channels(layout: &Layout) -> Vec<u32, 8> {
    let mut out = Vec::new();
    for f in layout.fields.iter().filter(|f| f.ty != ReportType::Input) {
        if page::is_vendor(page_of(f.application)) && !out.contains(&f.application) {
            let _ = out.push(f.application);
        }
    }
    out
}

impl Translator {
    pub fn new(layout: Layout) -> Self {
        let channels = vendor_channels(&layout);
        let mut v = Verdict::default();
        if !channels.is_empty() {
            v.caps |= caps::VENDOR_CHANNEL;
        }
        let mut kinds = Vec::new();
        for f in layout.fields.iter() {
            let kind = if f.ty == ReportType::Input && f.is_readable() {
                classify(f, &channels, &mut v)
            } else {
                if f.ty == ReportType::Output && f.all_usages().any(|u| page_of(u) == page::LED) {
                    v.caps |= caps::KEYBOARD_LEDS;
                }
                Kind::Ignore
            };
            let _ = kinds.push(kind);
        }
        Self { layout, kinds, verdict: v }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn verdict(&self) -> Verdict {
        self.verdict
    }

    pub fn translate(&self, report: &[u8]) -> Translation {
        let Some((id, payload)) = self.layout.split_report(report) else {
            return Translation::default();
        };
        let mut t = Translation { report_id: id, ..Translation::default() };
        let mut rollover = false;
        for (f, kind) in self.layout.fields.iter().zip(&self.kinds) {
            if f.ty != ReportType::Input || f.report_id != id || *kind == Kind::Ignore {
                continue;
            }
            let n = f.count as usize;
            match kind {
                Kind::KeyBits => {
                    let ks = t.keyboard.get_or_insert_default();
                    for e in 0..n {
                        if f.read_raw(payload, e) != 0
                            && let Some(u) = f.usage(e)
                        {
                            ks.press(id_of(u) as u8);
                        }
                    }
                }
                Kind::KeyArray => {
                    let ks = t.keyboard.get_or_insert_default();
                    for e in 0..n {
                        if let Some(u) = f.array_usage(f.read(payload, e)) {
                            if page_of(u) != page::KEYBOARD {
                                continue;
                            }
                            match id_of(u) as u8 {
                                key::ERROR_ROLL_OVER => rollover = true,
                                k => ks.press(k),
                            }
                        }
                    }
                }
                Kind::Buttons => {
                    let m = t.mouse.get_or_insert_default();
                    for e in 0..n {
                        if f.read_raw(payload, e) != 0
                            && let Some(b @ 1..=16) = f.usage(e).map(id_of)
                        {
                            m.buttons |= 1 << (b - 1);
                        }
                    }
                }
                Kind::Axes => {
                    let m = t.mouse.get_or_insert_default();
                    for e in 0..n {
                        let v = f.read(payload, e);
                        match f.usage(e) {
                            Some(u) if u == usage(page::GENERIC_DESKTOP, gd::X) => m.x = v,
                            Some(u) if u == usage(page::GENERIC_DESKTOP, gd::Y) => m.y = v,
                            Some(u) if u == usage(page::GENERIC_DESKTOP, gd::WHEEL) => m.wheel = v,
                            Some(u) if u == usage(page::CONSUMER, consumer::AC_PAN) => m.pan = v,
                            _ => {}
                        }
                    }
                }
                Kind::ConsumerArray => {
                    let c = t.consumer.get_or_insert_default();
                    for e in 0..n {
                        if let Some(u) = f.array_usage(f.read(payload, e))
                            && page_of(u) == page::CONSUMER
                            && id_of(u) <= 0x3FF
                        {
                            c.push(id_of(u));
                        }
                    }
                }
                Kind::ConsumerBits => {
                    let c = t.consumer.get_or_insert_default();
                    for e in 0..n {
                        if f.read_raw(payload, e) != 0
                            && let Some(u) = f.usage(e)
                        {
                            c.push(id_of(u));
                        }
                    }
                }
                Kind::SystemArray => {
                    let s = t.system.get_or_insert(0);
                    for e in 0..n {
                        if let Some(bit) = f.array_usage(f.read(payload, e)).and_then(system_bit) {
                            *s |= bit;
                        }
                    }
                }
                Kind::SystemBits => {
                    let s = t.system.get_or_insert(0);
                    for e in 0..n {
                        if f.read_raw(payload, e) != 0
                            && let Some(bit) = f.usage(e).and_then(system_bit)
                        {
                            *s |= bit;
                        }
                    }
                }
                Kind::Ignore => {}
            }
        }
        if rollover {
            // Phantom state: the keyboard can't tell which keys are down; keep the previous state.
            t.keyboard = None;
        }
        t
    }

    /// Removes `keys` and `buttons` from a raw input report in place, for a mirrored device (swallowed hotkeys,
    /// keys still held from before a switch). Array slots are compacted. Returns whether anything changed.
    pub fn strip(&self, report: &mut [u8], keys: &KeyState, buttons: u16) -> bool {
        if keys.is_empty() && buttons == 0 {
            return false;
        }
        let ids = self.layout.uses_report_ids;
        let (id, payload) = match (ids, report.split_first_mut()) {
            (true, Some((id, rest))) => (*id, rest),
            (false, _) => (0, report),
            (true, None) => return false,
        };
        let mut changed = false;
        for (f, kind) in self.layout.fields.iter().zip(&self.kinds) {
            if f.ty != ReportType::Input || f.report_id != id {
                continue;
            }
            let n = f.count as usize;
            match kind {
                Kind::KeyBits => {
                    for e in 0..n {
                        let hit = f.usage(e).is_some_and(|u| keys.is_pressed(id_of(u) as u8));
                        if hit && f.read_raw(payload, e) != 0 {
                            f.write(payload, e, 0);
                            changed = true;
                        }
                    }
                }
                Kind::KeyArray => {
                    let mut kept: Vec<i32, 32> = Vec::new();
                    let mut removed = false;
                    for e in 0..n.min(32) {
                        let v = f.read(payload, e);
                        match f.array_usage(v) {
                            Some(u) if page_of(u) == page::KEYBOARD && keys.is_pressed(id_of(u) as u8) => {
                                removed = true
                            }
                            _ => {
                                let _ = kept.push(v);
                            }
                        }
                    }
                    if removed {
                        // Freed slots get 0: "no event" on keyboard arrays (out of range, or usage 0).
                        for e in 0..n.min(32) {
                            f.write(payload, e, kept.get(e).copied().unwrap_or(0));
                        }
                        changed = true;
                    }
                }
                Kind::Buttons => {
                    for e in 0..n {
                        if let Some(b @ 1..=16) = f.usage(e).map(id_of)
                            && buttons & (1 << (b - 1)) != 0
                            && f.read_raw(payload, e) != 0
                        {
                            f.write(payload, e, 0);
                            changed = true;
                        }
                    }
                }
                _ => {}
            }
        }
        changed
    }
}

fn classify(f: &Field, channels: &[u32], v: &mut Verdict) -> Kind {
    let first = f.usages.first().map(|r| r.min).unwrap_or(0);
    let pg = page_of(first);

    if page::is_vendor(pg) || page::is_vendor(page_of(f.application)) {
        if channels.contains(&f.application) {
            return Kind::Ignore; // responses of a command channel
        }
        v.caps |= caps::VENDOR_INPUT;
        v.misfit |= misfit::VENDOR_INPUT;
        return Kind::Ignore;
    }
    if is_gamepad_app(f) {
        v.caps |= caps::GAMEPAD;
        v.misfit |= misfit::GAMEPAD;
        return Kind::Ignore;
    }

    match pg {
        page::KEYBOARD => {
            v.caps |= caps::KEYBOARD;
            if f.is_variable() { Kind::KeyBits } else { Kind::KeyArray }
        }
        page::BUTTON => {
            v.caps |= caps::MOUSE;
            if f.is_array() || f.all_usages().any(|u| id_of(u) > 16) {
                v.misfit |= misfit::BUTTONS_OVER_16;
            }
            if f.is_variable() { Kind::Buttons } else { Kind::Ignore }
        }
        page::GENERIC_DESKTOP => {
            if f.is_array() && f.all_usages().any(|u| system_bit(u).is_some()) {
                v.caps |= caps::SYSTEM;
                return Kind::SystemArray;
            }
            if f.is_variable() && f.all_usages().all(|u| system_bit(u).is_some()) {
                v.caps |= caps::SYSTEM;
                return Kind::SystemBits;
            }
            let mouse_axis = |u: u32| matches!(id_of(u), gd::X | gd::Y | gd::WHEEL) && page_of(u) == pg;
            if f.is_variable() && f.all_usages().all(mouse_axis) {
                v.caps |= caps::MOUSE;
                if !f.is_relative() {
                    v.misfit |= misfit::ABSOLUTE_POINTER;
                    return Kind::Ignore;
                }
                return Kind::Axes;
            }
            if f.all_usages().any(|u| matches!(id_of(u), gd::Z..=gd::DIAL | gd::HAT_SWITCH)) {
                v.caps |= caps::GAMEPAD;
                v.misfit |= misfit::GAMEPAD;
            } else {
                v.caps |= caps::OTHER;
                v.misfit |= misfit::UNKNOWN_USAGE;
            }
            Kind::Ignore
        }
        page::CONSUMER => {
            if f.is_variable() && f.is_relative() && f.all_usages().all(|u| id_of(u) == consumer::AC_PAN) {
                v.caps |= caps::MOUSE;
                return Kind::Axes;
            }
            v.caps |= caps::CONSUMER;
            if f.is_array() {
                return Kind::ConsumerArray;
            }
            if f.bit_size == 1 {
                return Kind::ConsumerBits;
            }
            // Multi-bit consumer values (volume as a number, etc.) have no normalized form.
            v.misfit |= misfit::UNKNOWN_USAGE;
            Kind::Ignore
        }
        page::DIGITIZER => {
            v.caps |= caps::DIGITIZER;
            v.misfit |= misfit::DIGITIZER;
            Kind::Ignore
        }
        // Battery strength, LED states, ordinals: harmless to drop.
        page::GENERIC_DEVICE | page::LED | page::ORDINAL => Kind::Ignore,
        _ => {
            v.caps |= caps::OTHER;
            v.misfit |= misfit::UNKNOWN_USAGE;
            Kind::Ignore
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::fixtures::*;

    fn tr(desc: &[u8]) -> Translator {
        Translator::new(Layout::parse(desc).unwrap())
    }

    #[test]
    fn boot_keyboard() {
        let t = tr(BOOT_KEYBOARD);
        assert_eq!(t.verdict(), Verdict { caps: caps::KEYBOARD | caps::KEYBOARD_LEDS, misfit: 0 });
        let out = t.translate(&[0x02, 0, 0x04, 0x05, 0, 0, 0, 0]);
        let ks = out.keyboard.unwrap();
        assert_eq!(ks.modifiers, 0x02);
        assert_eq!(ks.keys().collect::<std::vec::Vec<_>>(), [0x04, 0x05]);
        assert_eq!(out.mouse, None);
        assert_eq!(t.translate(&[0, 0, 1, 1, 1, 1, 1, 1]).keyboard, None, "rollover keeps previous state");
    }

    #[test]
    fn nkro_bitmap_keyboard() {
        let t = tr(NKRO_KEYBOARD);
        assert!(t.verdict().fits());
        let mut report = [0u8; 1 + 1 + 31];
        report[0] = 6;
        report[1] = 0x10; // RCtrl
        report[2 + 0x47 / 8] |= 1 << (0x47 % 8);
        report[2 + 0xF0 / 8] |= 1 << (0xF0 % 8);
        let ks = t.translate(&report).keyboard.unwrap();
        assert!(ks.is_pressed(0xE4) && ks.is_pressed(0x47) && ks.is_pressed(0xF0));
        assert_eq!(ks.key_count(), 2);
        assert_eq!(t.translate(&[7, 0, 0]).keyboard, None, "unknown report ID");
    }

    #[test]
    fn boot_mouse() {
        let t = tr(BOOT_MOUSE);
        assert_eq!(t.verdict(), Verdict { caps: caps::MOUSE, misfit: 0 });
        let m = t.translate(&[0x03, 0xFF, 0x02]).mouse.unwrap();
        assert_eq!(m, MouseInput { buttons: 0b11, x: -1, y: 2, wheel: 0, pan: 0 });
    }

    #[test]
    fn gaming_mouse_fits_with_pan() {
        let t = tr(GAMING_MOUSE);
        assert_eq!(t.verdict(), Verdict { caps: caps::MOUSE, misfit: 0 });
        let m = t.translate(&[2, 0x05, 0x80, 0x18, 0xFC, 0xE8, 0x03, 0x01, 0xFF]).mouse.unwrap();
        assert_eq!(m, MouseInput { buttons: 0x8005, x: -1000, y: 1000, wheel: 1, pan: -1 });
    }

    #[test]
    fn twelve_bit_mouse() {
        let t = tr(MOUSE_12BIT);
        assert!(t.verdict().fits());
        let mut r = [0u8; 5];
        let l = t.layout();
        let xy = l.fields.iter().find(|f| f.usage(0) == Some(usage(1, gd::X))).unwrap();
        xy.write(&mut r, 0, -2047);
        xy.write(&mut r, 1, 7);
        r[0] = 0b10001;
        r[4] = 0xFE;
        let m = t.translate(&r).mouse.unwrap();
        assert_eq!(m, MouseInput { buttons: 0b10001, x: -2047, y: 7, wheel: -2, pan: 0 });
    }

    #[test]
    fn gamepad_and_vendor_inputs_do_not_fit() {
        let g = tr(GAMEPAD).verdict();
        assert!(!g.fits() && g.caps & caps::GAMEPAD != 0 && g.misfit & misfit::GAMEPAD != 0);
        let v = tr(MOUSE_VENDOR_BUTTONS).verdict();
        assert_eq!(v.misfit, misfit::VENDOR_INPUT);
        assert!(v.caps & caps::MOUSE != 0);
    }

    #[test]
    fn hidpp_channel_does_not_block_normalized() {
        let v = tr(HIDPP).verdict();
        assert!(v.fits());
        assert_eq!(v.caps, caps::VENDOR_CHANNEL);
    }

    #[test]
    fn strip_removes_keys_and_buttons_from_raw_reports() {
        let t = tr(BOOT_KEYBOARD);
        let mut r = [0x02, 0, 0x47, 0x04, 0x05, 0, 0, 0];
        let mut sl = KeyState::EMPTY;
        sl.press(0x47);
        assert!(t.strip(&mut r, &sl, 0));
        assert_eq!(r, [0x02, 0, 0x04, 0x05, 0, 0, 0, 0]);
        assert!(!t.strip(&mut r, &sl, 0));

        let n = tr(NKRO_KEYBOARD);
        let mut r = [0u8; 33];
        r[0] = 6;
        r[1] = 0x01; // LCtrl
        r[2 + 0x47 / 8] |= 1 << (0x47 % 8);
        let mut mask = sl;
        mask.press(0xE0);
        assert!(n.strip(&mut r, &mask, 0));
        assert!(r[1..].iter().all(|b| *b == 0));

        let m = tr(GAMING_MOUSE);
        let mut r = [2, 0x03, 0x00, 1, 0, 1, 0, 0, 0];
        assert!(m.strip(&mut r, &KeyState::EMPTY, 0b10));
        assert_eq!(&r[..3], &[2, 0x01, 0x00]);
    }

    #[test]
    fn media_and_system_keys() {
        let t = tr(MEDIA_KEYS);
        assert!(t.verdict().fits());
        assert_eq!(t.verdict().caps, caps::CONSUMER | caps::SYSTEM);
        let c = t.translate(&[3, 0xCD, 0x00, 0xE9, 0x00]).consumer.unwrap();
        assert_eq!(c.usages, [0xCD, 0xE9, 0, 0]);
        assert!(t.translate(&[3, 0, 0, 0, 0]).consumer.unwrap().is_empty());
        // Array value 1 → its first usage, 0x82 (sleep); 0 is outside the logical range → nothing pressed.
        assert_eq!(t.translate(&[4, 0x01]).system, Some(crate::normalized::system::SLEEP));
        assert_eq!(t.translate(&[4, 0x00]).system, Some(0));
    }
}
