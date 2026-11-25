//! Keycodes (`kvm_map::keycode`, QMK's numbering) and controls (`kvm_map::pos`) in words, for the mapping editor and
//! `odkvmctl keymap`: a name that [`parse_keycode`] / [`parse_control`] take back for everything they name.
//!
//! The names are QMK's where it has them, lower case: `a`, `escape`, `ctrl+c`, `mo(1)`, `lt(1,space)`, `mt(shift,a)`,
//! `mouse1`, `pad5`, `dpad-up`, `none`, `trans`; the KVM's actions by the hotkey list's names (`select-pc2`,
//! `next-profile`); anything else as its number (`0x7c73`).

use kvm_hid::keys;
use kvm_map::axes::{self, input};
use kvm_map::keycode::{KC_BTN1, KC_NO, KC_TRNS, KVM_ACTION, KVM_ACTIONS, KVM_BTN9, KVM_DPAD, QK_JOYSTICK};
use kvm_map::pos;
use kvm_proto::ctrl::HotkeyAction;

const MODS: [(&str, u8); 8] = [
    ("ctrl", 0x01),
    ("shift", 0x02),
    ("alt", 0x04),
    ("gui", 0x08),
    ("rctrl", 0x10),
    ("rshift", 0x20),
    ("ralt", 0x40),
    ("rgui", 0x80),
];
const DPAD: [&str; 4] = ["up", "right", "down", "left"];
/// The axis inputs' names (`kvm_map::axes::input` order).
pub const AXES: [&str; axes::INPUTS] =
    ["mouse-x", "mouse-y", "wheel", "pan", "x", "y", "z", "rx", "ry", "rz", "slider", "dial", "brake", "accelerator"];
/// QMK's mouse keys 0xCD–0xDF, then its media and system keys 0xA5–0xC2 (their consumer usages: `kvm_map::keycode`).
const MOUSE_KEYS: [(&str, u16); 13] = [
    ("ms-up", 0xCD),
    ("ms-down", 0xCE),
    ("ms-left", 0xCF),
    ("ms-right", 0xD0),
    ("wh-up", 0xD9),
    ("wh-down", 0xDA),
    ("wh-left", 0xDB),
    ("wh-right", 0xDC),
    ("acl0", 0xDD),
    ("acl1", 0xDE),
    ("acl2", 0xDF),
    ("power", 0xA5),
    ("sleep", 0xA6),
];
const MEDIA: [(&str, u16); 12] = [
    ("wake", 0xA7),
    ("mute", 0xA8),
    ("vol-up", 0xA9),
    ("vol-down", 0xAA),
    ("next", 0xAB),
    ("prev", 0xAC),
    ("stop", 0xAD),
    ("play", 0xAE),
    ("calc", 0xB2),
    ("www-back", 0xB6),
    ("bright-up", 0xBD),
    ("bright-down", 0xBE),
];
const LAYERS: [(&str, u16); 6] =
    [("to", 0x5200), ("mo", 0x5220), ("df", 0x5240), ("tg", 0x5260), ("osl", 0x5280), ("tt", 0x52C0)];

fn mods_name(m: u8) -> String {
    MODS.iter().filter(|(_, b)| m & b != 0).map(|(n, _)| *n).collect::<Vec<_>>().join("+")
}

/// HID modifier bits in words (`ctrl+shift`), or `none`.
pub fn mods_name_or_none(m: u8) -> String {
    if m == 0 { "none".into() } else { mods_name(m) }
}

/// HID modifier bits from words (`ctrl+shift`, `none`).
pub fn parse_mod_names(s: &str) -> Result<u8, String> {
    let s = s.trim().to_ascii_lowercase();
    if s == "none" {
        return Ok(0);
    }
    parse_mods(&s).ok_or_else(|| {
        let names = MODS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
        crate::fl!("parse-one-of", value = s, choices = names)
    })
}

fn parse_mods(s: &str) -> Option<u8> {
    s.split('+').try_fold(0u8, |acc, p| MODS.iter().find(|(n, _)| *n == p).map(|(_, b)| acc | b))
}

/// A keyboard usage's name.
fn key_name(u: u8) -> String {
    keys::name_of(u).map_or_else(|| format!("0x{u:02x}"), str::to_string)
}

/// A keycode in words.
pub fn keycode_name(k: u16) -> String {
    let qmk_mods = |m5: u16| kvm_map::keycode::hid_mods(m5);
    match k {
        KC_NO => "none".into(),
        KC_TRNS => "trans".into(),
        0x04..=0xA4 | 0xE0..=0xE7 => key_name(k as u8),
        _ if let Some((n, _)) = MOUSE_KEYS.iter().chain(&MEDIA).find(|(_, c)| *c == k) => n.to_string(),
        0xD1..=0xD8 => format!("mouse{}", k - KC_BTN1 + 1),
        0x0100..=0x1FFF if (0x04..=0xE7).contains(&(k & 0xFF)) => {
            format!("{}+{}", mods_name(qmk_mods(k >> 8)), key_name(k as u8))
        }
        0x2000..=0x3FFF if (0x04..=0xE7).contains(&(k & 0xFF)) => {
            format!("mt({},{})", mods_name(qmk_mods((k >> 8) & 0x1F)), keycode_name(k & 0xFF))
        }
        0x4000..=0x4FFF if (0x04..=0xE7).contains(&(k & 0xFF)) => {
            format!("lt({},{})", (k >> 8) & 0x0F, keycode_name(k & 0xFF))
        }
        _ if let Some((n, b)) = LAYERS.iter().find(|(_, b)| (*b..*b + 0x20).contains(&k)) => format!("{n}({})", k - b),
        0x5700..=0x57FF => format!("td({})", k & 0xFF),
        0x7700..=0x77FF => format!("macro({})", k & 0xFF),
        0x7400..=0x741F => format!("pad{}", k - QK_JOYSTICK + 1),
        0x7E00..=0x7E07 => format!("mouse{}", k - KVM_BTN9 + 9),
        0x7E08..=0x7E0B => format!("dpad-{}", DPAD[(k - KVM_DPAD) as usize]),
        _ if (KVM_ACTION..KVM_ACTION + KVM_ACTIONS).contains(&k) => {
            HotkeyAction::ALL[(k - KVM_ACTION) as usize].name().into()
        }
        _ => format!("0x{k:04x}"),
    }
}

/// A keycode from its name ([`keycode_name`]), or a number.
pub fn parse_keycode(s: &str) -> Result<u16, String> {
    let s = s.trim().to_ascii_lowercase();
    let arg = |prefix: &str| s.strip_prefix(prefix).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')'));
    let num = |t: &str, max: u16| t.trim().parse::<u16>().ok().filter(|n| *n <= max);
    let key = |t: &str| keys::usage_of(t.trim()).map(u16::from);
    // A tap-hold key's tap: a key, or a basic keycode such as mouse1 or play.
    let basic = |t: &str| key(t).or_else(|| parse_keycode(t).ok().filter(|k| (0x04..=0xE7).contains(k)));
    let found = match s.as_str() {
        "none" | "no" => Some(KC_NO),
        "trans" | "transparent" => Some(KC_TRNS),
        _ if let Some(h) = s.strip_prefix("0x") => u16::from_str_radix(h, 16).ok(),
        _ if let Some(n) = s.strip_prefix("mouse").and_then(|n| num(n, 16)).filter(|n| *n >= 1) => {
            Some(if n <= 8 { KC_BTN1 + n - 1 } else { KVM_BTN9 + n - 9 })
        }
        _ if let Some(n) = s.strip_prefix("pad").and_then(|n| num(n, 32)).filter(|n| *n >= 1) => {
            Some(QK_JOYSTICK + n - 1)
        }
        _ if let Some(d) = s.strip_prefix("dpad-").and_then(|d| DPAD.iter().position(|x| *x == d)) => {
            Some(KVM_DPAD + d as u16)
        }
        _ if let Some((_, c)) = MOUSE_KEYS.iter().chain(&MEDIA).find(|(n, _)| *n == s) => Some(*c),
        _ if let Some(a) = arg("lt") => {
            a.split_once(',').and_then(|(l, k)| Some(0x4000 | num(l, 15)? << 8 | basic(k)?))
        }
        _ if let Some(a) = arg("mt") => a
            .split_once(',')
            .and_then(|(m, k)| Some(0x2000 | kvm_map::keycode::qmk_mods(parse_mods(m.trim())?) << 8 | basic(k)?)),
        _ if let Some(i) = HotkeyAction::from_name(&s)
            .filter(|a| *a != HotkeyAction::None)
            .and_then(|a| HotkeyAction::ALL.iter().position(|b| *b == a)) =>
        {
            Some(KVM_ACTION + i as u16)
        }
        _ if let Some(n) = arg("td") => num(n, 255).map(|n| 0x5700 | n),
        _ if let Some(n) = arg("macro") => num(n, 255).map(|n| 0x7700 | n),
        _ if let Some((_, b, n)) = LAYERS.iter().find_map(|(name, b)| arg(name).map(|n| (name, b, n))) => {
            num(n, 31).map(|n| b + n)
        }
        _ if let Some((m, k)) = s.rsplit_once('+') => {
            parse_mods(m).zip(key(k)).map(|(m, k)| kvm_map::keycode::qmk_mods(m) << 8 | k)
        }
        _ => key(&s),
    };
    found.ok_or_else(|| crate::fl!("parse-keycode", value = s))
}

/// A control (a keymap position) in words: a key, `mouse4`, `pad12`, `hat-up`, an axis direction (`x-`, `wheel+`).
pub fn control_name(p: u16) -> String {
    match p {
        _ if (p as usize) < kvm_map::KEYBOARD => key_name(p as u8),
        _ if p < pos::PAD => format!("mouse{}", p - pos::MOUSE + 1),
        _ if p < pos::HAT => format!("pad{}", p - pos::PAD + 1),
        _ if p < pos::AXIS => format!("hat-{}", DPAD[(p - pos::HAT) as usize]),
        _ if p < pos::END => {
            let i = (p - pos::AXIS) as usize;
            format!("{}{}", AXES[i / 2], if i.is_multiple_of(2) { '-' } else { '+' })
        }
        _ => format!("{p}"),
    }
}

/// A control from its name ([`control_name`]).
pub fn parse_control(s: &str) -> Result<u16, String> {
    let s = s.trim().to_ascii_lowercase();
    let num = |t: &str, max: u16| t.parse::<u16>().ok().filter(|n| (1..=max).contains(n));
    let found = if let Some(n) = s.strip_prefix("mouse").and_then(|n| num(n, pos::MOUSE_BUTTONS)) {
        Some(pos::MOUSE + n - 1)
    } else if let Some(n) = s.strip_prefix("pad").and_then(|n| num(n, pos::PAD_BUTTONS)) {
        Some(pos::PAD + n - 1)
    } else if let Some(d) = s.strip_prefix("hat-").and_then(|d| DPAD.iter().position(|x| *x == d)) {
        Some(pos::HAT + d as u16)
    } else if let Some((a, sign)) = s.strip_suffix('-').map(|a| (a, 0)).or_else(|| s.strip_suffix('+').map(|a| (a, 1)))
        && let Some(i) = AXES.iter().position(|x| *x == a)
    {
        Some(pos::AXIS + 2 * i as u16 + sign)
    } else {
        keys::usage_of(&s).map(u16::from)
    };
    found.ok_or_else(|| crate::fl!("parse-control", value = s))
}

/// An axis input from its name ([`AXES`]).
pub fn parse_axis(s: &str) -> Result<usize, String> {
    let s = s.trim().to_ascii_lowercase();
    AXES.iter().position(|a| *a == s).ok_or_else(|| crate::fl!("parse-one-of", value = s, choices = AXES.join(", ")))
}

/// An axis rule's target in words (`odkvmctl keymap axis`).
pub fn target_name(t: axes::Target) -> String {
    match t {
        axes::Target::Transparent => "trans".into(),
        axes::Target::None => "none".into(),
        axes::Target::MouseX => "mouse-x".into(),
        axes::Target::MouseY => "mouse-y".into(),
        axes::Target::Wheel => "wheel".into(),
        axes::Target::Pan => "pan".into(),
        axes::Target::Pad(n) => format!("pad-{}", AXES[input::X + n as usize]),
        axes::Target::Buttons => "buttons".into(),
    }
}

pub fn parse_target(s: &str) -> Result<axes::Target, String> {
    let s = s.trim().to_ascii_lowercase();
    let pad = s.strip_prefix("pad-").and_then(|a| AXES[input::X..=input::DIAL].iter().position(|x| *x == a));
    Ok(match s.as_str() {
        "trans" | "transparent" => axes::Target::Transparent,
        "none" => axes::Target::None,
        "mouse-x" => axes::Target::MouseX,
        "mouse-y" => axes::Target::MouseY,
        "wheel" => axes::Target::Wheel,
        "pan" => axes::Target::Pan,
        "buttons" => axes::Target::Buttons,
        _ if let Some(n) = pad => axes::Target::Pad(n as u8),
        _ => {
            let choices = "trans, none, mouse-x, mouse-y, wheel, pan, buttons, pad-x … pad-dial";
            return Err(crate::fl!("parse-one-of", value = s, choices = choices));
        }
    })
}

/// The keymaps' QMK settings by name (Vial's qsids, `kvm_map::keymap::qsid`), and whether each is on/off.
pub const SETTINGS: [(&str, u16, bool); 26] = [
    ("tapping-term", 7, false),
    ("permissive-hold", 22, true),
    ("hold-on-other-key-press", 23, true),
    ("retro-tapping", 24, true),
    ("chordal-hold", 26, true),
    ("quick-tap-term", 25, false),
    ("flow-tap-term", 27, false),
    ("tapping-toggle", 20, false),
    ("tap-code-delay", 18, false),
    ("tap-hold-caps-delay", 19, false),
    ("combo-term", 2, false),
    ("oneshot-timeout", 6, false),
    ("oneshot-tap-toggle", 5, false),
    ("auto-shift", 3, false),
    ("auto-shift-timeout", 4, false),
    ("grave-esc-override", 1, false),
    ("magic", 21, false),
    ("mouse-delay", 9, false),
    ("mouse-interval", 10, false),
    ("mouse-move-delta", 11, false),
    ("mouse-max-speed", 12, false),
    ("mouse-time-to-max", 13, false),
    ("wheel-delay", 14, false),
    ("wheel-interval", 15, false),
    ("wheel-max-speed", 16, false),
    ("wheel-time-to-max", 17, false),
];

/// A macro step the host software holds (`kvm_map::macros::Step`, its text owned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroStep {
    Text(String),
    Tap(u16),
    Down(u16),
    Up(u16),
    Delay(u16),
}

impl MacroStep {
    /// The steps of one macro's bytes.
    pub fn read(m: &[u8]) -> Vec<MacroStep> {
        use kvm_map::macros::Step;
        kvm_map::macros::steps(m)
            .map(|s| match s {
                Step::Text(t) => MacroStep::Text(String::from_utf8_lossy(t).into_owned()),
                Step::Tap(k) => MacroStep::Tap(k),
                Step::Down(k) => MacroStep::Down(k),
                Step::Up(k) => MacroStep::Up(k),
                Step::Delay(ms) => MacroStep::Delay(ms),
            })
            .collect()
    }

    /// The bytes of a macro of `steps` (without its NUL); None if it doesn't fit the buffer or has a character Vial's
    /// format can't carry.
    pub fn write(steps: &[MacroStep]) -> Option<Vec<u8>> {
        use kvm_map::macros::Step;
        if steps.iter().any(|s| matches!(s, MacroStep::Text(t) if !t.is_ascii())) {
            return None;
        }
        let v: Vec<Step<'_>> = steps
            .iter()
            .map(|s| match s {
                MacroStep::Text(t) => Step::Text(t.as_bytes()),
                MacroStep::Tap(k) => Step::Tap(*k),
                MacroStep::Down(k) => Step::Down(*k),
                MacroStep::Up(k) => Step::Up(*k),
                MacroStep::Delay(ms) => Step::Delay(*ms),
            })
            .collect();
        kvm_map::macros::encode::<{ kvm_map::keymap::MACRO_BUFFER }>(&v).map(|b| b.to_vec())
    }

    /// In words: `text:hello`, `tap:ctrl+c`, `down:shift`, `up:shift`, `delay:100`.
    pub fn name(&self) -> String {
        match self {
            MacroStep::Text(t) => format!("text:{t}"),
            MacroStep::Tap(k) => format!("tap:{}", keycode_name(*k)),
            MacroStep::Down(k) => format!("down:{}", keycode_name(*k)),
            MacroStep::Up(k) => format!("up:{}", keycode_name(*k)),
            MacroStep::Delay(ms) => format!("delay:{ms}"),
        }
    }

    /// A step from its words ([`MacroStep::name`]).
    pub fn parse(s: &str) -> Result<MacroStep, String> {
        let bad = || crate::fl!("parse-macro-step", value = s);
        let (kind, arg) = s.split_once(':').ok_or_else(bad)?;
        Ok(match kind.trim().to_ascii_lowercase().as_str() {
            "text" if arg.is_ascii() && !arg.is_empty() => MacroStep::Text(arg.into()),
            "tap" => MacroStep::Tap(parse_keycode(arg)?),
            "down" => MacroStep::Down(parse_keycode(arg)?),
            "up" => MacroStep::Up(parse_keycode(arg)?),
            "delay" => MacroStep::Delay(
                arg.trim().parse::<u16>().ok().filter(|ms| *ms <= kvm_map::macros::DELAY_MAX).ok_or_else(bad)?,
            ),
            _ => return Err(bad()),
        })
    }
}

/// A share in percent as the KVM stores it, in 256ths (a deadzone, a threshold): rounded, so that every whole percent
/// reads back as itself ([`percent`]). Rounding down both ways turned 1 % into 2/256, which read back as 0 %.
pub fn in_256ths(percent: u32) -> u8 {
    ((percent.min(100) * 256 + 50) / 100).min(255) as u8
}

/// A share in 256ths in whole percent, rounded.
pub fn percent(v: u8) -> u32 {
    (v as u32 * 100 + 128) / 256
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_keycode_parses_back() {
        let samples = [
            0x0000, 0x0001, 0x0004, 0x0029, 0x00E1, 0x00A9, 0x00CD, 0x00D3, 0x0106, 0x0222, 0x2229, 0x412C, 0x5221,
            0x5263, 0x5282, 0x52C7, 0x5703, 0x7702, 0x7400, 0x741F, 0x7E00, 0x7E07, 0x7E09, 0x7C73, 0x7E0C, 0x7E1F,
            0x41D1, 0x22AE,
        ];
        for k in samples {
            let n = keycode_name(k);
            assert_eq!(parse_keycode(&n), Ok(k), "{n}");
        }
        assert_eq!(keycode_name(0x0106), "ctrl+c");
        assert_eq!(keycode_name(0x412C), "lt(1,space)");
        assert_eq!(keycode_name(0x2229), "mt(shift,escape)");
        assert_eq!(parse_keycode("Ctrl+Shift+Escape"), Ok(0x0329));
        assert!(parse_keycode("ctrl+nokey").is_err());
        // The KVM's actions by the hotkey list's names; a layer-tap that clicks.
        assert_eq!(keycode_name(KVM_ACTION + 4), "select-pc2");
        assert_eq!(parse_keycode("next-profile"), Ok(KVM_ACTION + 9));
        assert_eq!(keycode_name(0x41D1), "lt(1,mouse1)");
    }

    #[test]
    fn macro_steps_in_words_round_trip() {
        let steps = ["text:hi there", "tap:ctrl+c", "down:mouse1", "delay:250", "up:mouse1", "tap:pad3"];
        let parsed: Vec<MacroStep> = steps.iter().map(|s| MacroStep::parse(s).unwrap()).collect();
        assert_eq!(parsed.iter().map(MacroStep::name).collect::<Vec<_>>(), steps);
        let bytes = MacroStep::write(&parsed).unwrap();
        assert_eq!(MacroStep::read(&bytes), parsed);
        assert!(MacroStep::parse("delay:x").is_err() && MacroStep::write(&[MacroStep::Text("ä".into())]).is_none());
        assert_eq!(SETTINGS.len(), kvm_map::keymap::qsid::ALL.len());
    }

    #[test]
    fn every_whole_percent_reads_back_as_itself() {
        for p in 0..=100 {
            assert_eq!(percent(in_256ths(p)), p, "{p} %");
        }
        assert_eq!((in_256ths(1), in_256ths(100)), (3, 255));
    }

    #[test]
    fn every_control_parses_back() {
        for p in (0x04..pos::END).filter(|p| *p >= pos::MOUSE || keys::name_of(*p as u8).is_some()) {
            let n = control_name(p);
            assert_eq!(parse_control(&n), Ok(p), "{n}");
        }
        assert_eq!(control_name(pos::MOUSE + 3), "mouse4");
        assert_eq!(control_name(pos::AXIS + 2 * input::RX as u16 + 1), "rx+");
        assert_eq!(parse_target("pad-rz"), Ok(axes::Target::Pad(5)));
        assert_eq!(target_name(axes::Target::Pad(2)), "pad-z");
    }
}
