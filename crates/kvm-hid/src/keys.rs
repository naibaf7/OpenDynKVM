//! Names of keyboard-page usages, for hotkey specs in `odkvmctl` and the GUI.

const NAMES: &[(&str, u8)] = &[
    ("enter", 0x28),
    ("escape", 0x29),
    ("backspace", 0x2A),
    ("tab", 0x2B),
    ("space", 0x2C),
    ("minus", 0x2D),
    ("equal", 0x2E),
    ("leftbracket", 0x2F),
    ("rightbracket", 0x30),
    ("backslash", 0x31),
    ("semicolon", 0x33),
    ("quote", 0x34),
    ("grave", 0x35),
    ("comma", 0x36),
    ("period", 0x37),
    ("slash", 0x38),
    ("capslock", 0x39),
    ("printscreen", 0x46),
    ("scrolllock", 0x47),
    ("pause", 0x48),
    ("insert", 0x49),
    ("home", 0x4A),
    ("pageup", 0x4B),
    ("delete", 0x4C),
    ("end", 0x4D),
    ("pagedown", 0x4E),
    ("right", 0x4F),
    ("left", 0x50),
    ("down", 0x51),
    ("up", 0x52),
    ("numlock", 0x53),
    ("kp_divide", 0x54),
    ("kp_multiply", 0x55),
    ("kp_minus", 0x56),
    ("kp_plus", 0x57),
    ("kp_enter", 0x58),
    ("kp1", 0x59),
    ("kp2", 0x5A),
    ("kp3", 0x5B),
    ("kp4", 0x5C),
    ("kp5", 0x5D),
    ("kp6", 0x5E),
    ("kp7", 0x5F),
    ("kp8", 0x60),
    ("kp9", 0x61),
    ("kp0", 0x62),
    ("kp_period", 0x63),
    ("menu", 0x65),
    ("lctrl", 0xE0),
    ("lshift", 0xE1),
    ("lalt", 0xE2),
    ("lgui", 0xE3),
    ("rctrl", 0xE4),
    ("rshift", 0xE5),
    ("ralt", 0xE6),
    ("rgui", 0xE7),
];

const ALIASES: &[(&str, u8)] = &[
    ("return", 0x28),
    ("esc", 0x29),
    ("prtsc", 0x46),
    ("ins", 0x49),
    ("pgup", 0x4B),
    ("del", 0x4C),
    ("pgdn", 0x4E),
    ("app", 0x65),
    ("dot", 0x37),
];

const LETTERS: &[&str] = &[
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s", "t", "u", "v", "w",
    "x", "y", "z",
];
const DIGITS: &[&str] = &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];
const FKEYS: &[&str] = &[
    "f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12", "f13", "f14", "f15", "f16", "f17",
    "f18", "f19", "f20", "f21", "f22", "f23", "f24",
];

/// Lower-case name of a keyboard usage.
pub fn name_of(usage: u8) -> Option<&'static str> {
    match usage {
        0x04..=0x1D => Some(LETTERS[(usage - 0x04) as usize]),
        0x1E..=0x27 => Some(DIGITS[(usage - 0x1E) as usize]),
        0x3A..=0x45 => Some(FKEYS[(usage - 0x3A) as usize]),
        0x68..=0x73 => Some(FKEYS[(usage - 0x68 + 12) as usize]),
        _ => NAMES.iter().find(|(_, u)| *u == usage).map(|(n, _)| *n),
    }
}

fn lower<'a>(name: &str, buf: &'a mut [u8; 16]) -> Option<&'a str> {
    let n = name.len();
    if n == 0 || n > buf.len() {
        return None;
    }
    for (d, s) in buf.iter_mut().zip(name.bytes()) {
        *d = s.to_ascii_lowercase();
    }
    core::str::from_utf8(&buf[..n]).ok()
}

/// Usage for a key name (case-insensitive ASCII), including aliases.
pub fn usage_of(name: &str) -> Option<u8> {
    let mut buf = [0u8; 16];
    let lower = lower(name, &mut buf)?;
    if let Some(i) = LETTERS.iter().position(|l| *l == lower) {
        return Some(0x04 + i as u8);
    }
    if let Some(i) = DIGITS.iter().position(|d| *d == lower) {
        return Some(0x1E + i as u8);
    }
    if let Some(i) = FKEYS.iter().position(|f| *f == lower) {
        return Some(if i < 12 { 0x3A + i as u8 } else { 0x68 + (i - 12) as u8 });
    }
    NAMES.iter().chain(ALIASES).find(|(n, _)| *n == lower).map(|(_, u)| *u)
}

/// Modifier names for combos. `ctrl`, `shift`, `alt` and `gui` mean either side (left-side bits only, see
/// `kvm-core::hotkey`); `l…`/`r…` are exact.
pub fn modifier_of(name: &str) -> Option<u8> {
    let mut buf = [0u8; 16];
    Some(match lower(name, &mut buf)? {
        "ctrl" | "control" | "lctrl" => 0x01,
        "shift" | "lshift" => 0x02,
        "alt" | "lalt" | "option" => 0x04,
        "gui" | "win" | "super" | "meta" | "cmd" | "lgui" => 0x08,
        "rctrl" => 0x10,
        "rshift" => 0x20,
        "ralt" | "altgr" => 0x40,
        "rgui" => 0x80,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_roundtrip() {
        for u in 0x04..=0xE7u8 {
            if let Some(n) = name_of(u) {
                assert_eq!(usage_of(n), Some(u), "{n}");
            }
        }
        assert_eq!(usage_of("ScrollLock"), Some(0x47));
        assert_eq!(usage_of("F13"), Some(0x68));
        assert_eq!(usage_of("esc"), Some(0x29));
        assert_eq!(usage_of("nope"), None);
        assert_eq!(modifier_of("Win"), Some(0x08));
    }
}
