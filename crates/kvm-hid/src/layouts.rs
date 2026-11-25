//! Keyboard layouts of the hosts (hid_mapping.md §7.1): what each key of the main block types on a host set to a
//! layout, by HID usage. The host software draws its legends from them, and the KVM types a macro's text through the
//! layout of the host it goes to (`kvm_map::remap`), so text comes out right on a PC set to German or French.
//!
//! A key's [`Legend`] is what it types alone, with Shift, with AltGr and with both; a dead key (`´`, `^`, `¨`, …)
//! types its character only when followed by Space, which [`Layout::stroke`] says. Only the keys whose characters
//! depend on the layout are here: letters, digits and punctuation (usages 0x04–0x38), the ISO key left of Z (0x64), and
//! JIS's and ABNT2's extra keys (0x87, 0x89). Everything else (Enter, the modifiers, the function keys, the keypad)
//! is the same everywhere.

/// HID modifier bits.
pub mod mods {
    pub const LSHIFT: u8 = 0x02;
    pub const RALT: u8 = 0x40;
}

/// A host's keyboard layout. On the wire as its number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Layout {
    #[default]
    Us = 0,
    Gb = 1,
    De = 2,
    /// Swiss German.
    Ch = 3,
    Fr = 4,
    Es = 5,
    It = 6,
    Pt = 7,
    No = 8,
    /// Swedish and Finnish.
    Se = 9,
    Dk = 10,
    Jp = 11,
    /// Brazilian (ABNT2).
    Br = 12,
}

/// The physical shape of a keyboard's main block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(u8)]
pub enum Physical {
    /// Wide Backspace, a backslash key above a one-row Enter.
    #[default]
    Ansi = 0,
    /// A two-row Enter, the key left of it (0x32) and one left of Z (0x64).
    Iso = 1,
    /// ISO's Enter, Yen (0x89) by Backspace, Ro (0x87) by right Shift, and the conversion keys around a short Space.
    Jis = 2,
    /// ISO with a key (0x87) between the slash key and right Shift.
    Abnt2 = 3,
}

impl Physical {
    pub const ALL: [Physical; 4] = [Physical::Ansi, Physical::Iso, Physical::Jis, Physical::Abnt2];

    pub const fn name(self) -> &'static str {
        match self {
            Physical::Ansi => "ansi",
            Physical::Iso => "iso",
            Physical::Jis => "jis",
            Physical::Abnt2 => "abnt2",
        }
    }

    pub fn from_name(name: &str) -> Option<Physical> {
        Self::ALL.iter().copied().find(|p| p.name() == name)
    }
}

/// Which of a key's characters are dead keys ([`Legend::dead`]).
pub mod dead {
    pub const BASE: u8 = 1 << 0;
    pub const SHIFT: u8 = 1 << 1;
    pub const ALTGR: u8 = 1 << 2;
    pub const SHIFT_ALTGR: u8 = 1 << 3;
}

/// What a key types: alone, with Shift, with AltGr, with Shift and AltGr; `'\0'` for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Legend {
    pub base: char,
    pub shift: char,
    pub altgr: char,
    pub shift_altgr: char,
    /// [`dead`] bits.
    pub dead: u8,
}

impl Legend {
    /// The four characters with the modifiers that type each, and whether it is dead.
    fn chars(&self) -> [(char, u8, bool); 4] {
        [
            (self.base, 0, self.dead & dead::BASE != 0),
            (self.shift, mods::LSHIFT, self.dead & dead::SHIFT != 0),
            (self.altgr, mods::RALT, self.dead & dead::ALTGR != 0),
            (self.shift_altgr, mods::LSHIFT | mods::RALT, self.dead & dead::SHIFT_ALTGR != 0),
        ]
    }
}

/// How to type a character: a key with modifiers, and Space after it when the key is dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Stroke {
    pub usage: u8,
    pub mods: u8,
    pub dead: bool,
}

type E = (u8, [char; 4], u8);

const fn k(u: u8, base: char, shift: char) -> E {
    (u, [base, shift, '\0', '\0'], 0)
}

const fn ka(u: u8, base: char, shift: char, altgr: char) -> E {
    (u, [base, shift, altgr, '\0'], 0)
}

const fn kd(u: u8, base: char, shift: char, altgr: char, dead: u8) -> E {
    (u, [base, shift, altgr, '\0'], dead)
}

const B: u8 = dead::BASE;
const S: u8 = dead::SHIFT;
const A: u8 = dead::ALTGR;

const US: &[E] = &[
    k(0x1E, '1', '!'),
    k(0x1F, '2', '@'),
    k(0x20, '3', '#'),
    k(0x21, '4', '$'),
    k(0x22, '5', '%'),
    k(0x23, '6', '^'),
    k(0x24, '7', '&'),
    k(0x25, '8', '*'),
    k(0x26, '9', '('),
    k(0x27, '0', ')'),
    k(0x2D, '-', '_'),
    k(0x2E, '=', '+'),
    k(0x2F, '[', '{'),
    k(0x30, ']', '}'),
    k(0x31, '\\', '|'),
    k(0x32, '\\', '|'),
    k(0x33, ';', ':'),
    k(0x34, '\'', '"'),
    k(0x35, '`', '~'),
    k(0x36, ',', '<'),
    k(0x37, '.', '>'),
    k(0x38, '/', '?'),
    k(0x64, '\\', '|'),
];

const GB: &[E] = &[
    k(0x1E, '1', '!'),
    k(0x1F, '2', '"'),
    k(0x20, '3', '£'),
    ka(0x21, '4', '$', '€'),
    k(0x22, '5', '%'),
    k(0x23, '6', '^'),
    k(0x24, '7', '&'),
    k(0x25, '8', '*'),
    k(0x26, '9', '('),
    k(0x27, '0', ')'),
    k(0x2D, '-', '_'),
    k(0x2E, '=', '+'),
    k(0x2F, '[', '{'),
    k(0x30, ']', '}'),
    k(0x31, '#', '~'),
    k(0x32, '#', '~'),
    k(0x33, ';', ':'),
    k(0x34, '\'', '@'),
    ka(0x35, '`', '¬', '¦'),
    k(0x36, ',', '<'),
    k(0x37, '.', '>'),
    k(0x38, '/', '?'),
    k(0x64, '\\', '|'),
];

const DE: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    ka(0x10, 'm', 'M', 'µ'),
    ka(0x14, 'q', 'Q', '@'),
    k(0x1C, 'z', 'Z'),
    k(0x1D, 'y', 'Y'),
    k(0x1E, '1', '!'),
    ka(0x1F, '2', '"', '²'),
    ka(0x20, '3', '§', '³'),
    k(0x21, '4', '$'),
    k(0x22, '5', '%'),
    k(0x23, '6', '&'),
    ka(0x24, '7', '/', '{'),
    ka(0x25, '8', '(', '['),
    ka(0x26, '9', ')', ']'),
    ka(0x27, '0', '=', '}'),
    ka(0x2D, 'ß', '?', '\\'),
    kd(0x2E, '´', '`', '\0', B | S),
    k(0x2F, 'ü', 'Ü'),
    ka(0x30, '+', '*', '~'),
    k(0x31, '#', '\''),
    k(0x32, '#', '\''),
    k(0x33, 'ö', 'Ö'),
    k(0x34, 'ä', 'Ä'),
    kd(0x35, '^', '°', '\0', B),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    ka(0x64, '<', '>', '|'),
];

const CH: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    k(0x1C, 'z', 'Z'),
    k(0x1D, 'y', 'Y'),
    ka(0x1E, '1', '+', '¦'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '*', '#'),
    k(0x21, '4', 'ç'),
    k(0x22, '5', '%'),
    ka(0x23, '6', '&', '¬'),
    ka(0x24, '7', '/', '|'),
    ka(0x25, '8', '(', '¢'),
    k(0x26, '9', ')'),
    k(0x27, '0', '='),
    kd(0x2D, '\'', '?', '´', A),
    kd(0x2E, '^', '`', '~', B | S | A),
    ka(0x2F, 'ü', 'è', '['),
    kd(0x30, '¨', '!', ']', B),
    ka(0x31, '$', '£', '}'),
    ka(0x32, '$', '£', '}'),
    k(0x33, 'ö', 'é'),
    ka(0x34, 'ä', 'à', '{'),
    k(0x35, '§', '°'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    ka(0x64, '<', '>', '\\'),
];

const FR: &[E] = &[
    k(0x04, 'q', 'Q'),
    ka(0x08, 'e', 'E', '€'),
    k(0x10, ',', '?'),
    k(0x14, 'a', 'A'),
    k(0x1A, 'z', 'Z'),
    k(0x1D, 'w', 'W'),
    k(0x1E, '&', '1'),
    kd(0x1F, 'é', '2', '~', A),
    ka(0x20, '"', '3', '#'),
    ka(0x21, '\'', '4', '{'),
    ka(0x22, '(', '5', '['),
    ka(0x23, '-', '6', '|'),
    kd(0x24, 'è', '7', '`', A),
    ka(0x25, '_', '8', '\\'),
    ka(0x26, 'ç', '9', '^'),
    ka(0x27, 'à', '0', '@'),
    ka(0x2D, ')', '°', ']'),
    ka(0x2E, '=', '+', '}'),
    kd(0x2F, '^', '¨', '\0', B | S),
    ka(0x30, '$', '£', '¤'),
    k(0x31, '*', 'µ'),
    k(0x32, '*', 'µ'),
    k(0x33, 'm', 'M'),
    k(0x34, 'ù', '%'),
    k(0x35, '²', '\0'),
    k(0x36, ';', '.'),
    k(0x37, ':', '/'),
    k(0x38, '!', '§'),
    k(0x64, '<', '>'),
];

const ES: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    ka(0x1E, '1', '!', '|'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '·', '#'),
    kd(0x21, '4', '$', '~', A),
    ka(0x22, '5', '%', '€'),
    ka(0x23, '6', '&', '¬'),
    k(0x24, '7', '/'),
    k(0x25, '8', '('),
    k(0x26, '9', ')'),
    k(0x27, '0', '='),
    k(0x2D, '\'', '?'),
    k(0x2E, '¡', '¿'),
    kd(0x2F, '`', '^', '[', B | S),
    ka(0x30, '+', '*', ']'),
    ka(0x31, 'ç', 'Ç', '}'),
    ka(0x32, 'ç', 'Ç', '}'),
    k(0x33, 'ñ', 'Ñ'),
    kd(0x34, '´', '¨', '{', B | S),
    ka(0x35, 'º', 'ª', '\\'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    k(0x64, '<', '>'),
];

const IT: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    k(0x1E, '1', '!'),
    k(0x1F, '2', '"'),
    k(0x20, '3', '£'),
    k(0x21, '4', '$'),
    ka(0x22, '5', '%', '€'),
    k(0x23, '6', '&'),
    k(0x24, '7', '/'),
    k(0x25, '8', '('),
    k(0x26, '9', ')'),
    k(0x27, '0', '='),
    k(0x2D, '\'', '?'),
    k(0x2E, 'ì', '^'),
    (0x2F, ['è', 'é', '[', '{'], 0),
    (0x30, ['+', '*', ']', '}'], 0),
    k(0x31, 'ù', '§'),
    k(0x32, 'ù', '§'),
    ka(0x33, 'ò', 'ç', '@'),
    ka(0x34, 'à', '°', '#'),
    k(0x35, '\\', '|'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    k(0x64, '<', '>'),
];

const PT: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    k(0x1E, '1', '!'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '#', '£'),
    ka(0x21, '4', '$', '§'),
    ka(0x22, '5', '%', '€'),
    k(0x23, '6', '&'),
    ka(0x24, '7', '/', '{'),
    ka(0x25, '8', '(', '['),
    ka(0x26, '9', ')', ']'),
    ka(0x27, '0', '=', '}'),
    k(0x2D, '\'', '?'),
    k(0x2E, '«', '»'),
    kd(0x2F, '+', '*', '¨', A),
    kd(0x30, '´', '`', '\0', B | S),
    kd(0x31, '~', '^', '\0', B | S),
    kd(0x32, '~', '^', '\0', B | S),
    k(0x33, 'ç', 'Ç'),
    k(0x34, 'º', 'ª'),
    k(0x35, '\\', '|'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    k(0x64, '<', '>'),
];

const NO: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    ka(0x10, 'm', 'M', 'µ'),
    k(0x1E, '1', '!'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '#', '£'),
    ka(0x21, '4', '¤', '$'),
    ka(0x22, '5', '%', '€'),
    k(0x23, '6', '&'),
    ka(0x24, '7', '/', '{'),
    ka(0x25, '8', '(', '['),
    ka(0x26, '9', ')', ']'),
    ka(0x27, '0', '=', '}'),
    k(0x2D, '+', '?'),
    kd(0x2E, '\\', '`', '´', S | A),
    k(0x2F, 'å', 'Å'),
    kd(0x30, '¨', '^', '~', B | S | A),
    k(0x31, '\'', '*'),
    k(0x32, '\'', '*'),
    k(0x33, 'ø', 'Ø'),
    k(0x34, 'æ', 'Æ'),
    k(0x35, '|', '§'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    k(0x64, '<', '>'),
];

const SE: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    ka(0x10, 'm', 'M', 'µ'),
    k(0x1E, '1', '!'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '#', '£'),
    ka(0x21, '4', '¤', '$'),
    ka(0x22, '5', '%', '€'),
    k(0x23, '6', '&'),
    ka(0x24, '7', '/', '{'),
    ka(0x25, '8', '(', '['),
    ka(0x26, '9', ')', ']'),
    ka(0x27, '0', '=', '}'),
    ka(0x2D, '+', '?', '\\'),
    kd(0x2E, '´', '`', '\0', B | S),
    k(0x2F, 'å', 'Å'),
    kd(0x30, '¨', '^', '~', B | S | A),
    k(0x31, '\'', '*'),
    k(0x32, '\'', '*'),
    k(0x33, 'ö', 'Ö'),
    k(0x34, 'ä', 'Ä'),
    k(0x35, '§', '½'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    ka(0x64, '<', '>', '|'),
];

const DK: &[E] = &[
    ka(0x08, 'e', 'E', '€'),
    ka(0x10, 'm', 'M', 'µ'),
    k(0x1E, '1', '!'),
    ka(0x1F, '2', '"', '@'),
    ka(0x20, '3', '#', '£'),
    ka(0x21, '4', '¤', '$'),
    ka(0x22, '5', '%', '€'),
    k(0x23, '6', '&'),
    ka(0x24, '7', '/', '{'),
    ka(0x25, '8', '(', '['),
    ka(0x26, '9', ')', ']'),
    ka(0x27, '0', '=', '}'),
    k(0x2D, '+', '?'),
    kd(0x2E, '´', '`', '|', B | S),
    k(0x2F, 'å', 'Å'),
    kd(0x30, '¨', '^', '~', B | S | A),
    k(0x31, '\'', '*'),
    k(0x32, '\'', '*'),
    k(0x33, 'æ', 'Æ'),
    k(0x34, 'ø', 'Ø'),
    k(0x35, '½', '§'),
    k(0x36, ',', ';'),
    k(0x37, '.', ':'),
    k(0x38, '-', '_'),
    ka(0x64, '<', '>', '\\'),
];

const JP: &[E] = &[
    k(0x1E, '1', '!'),
    k(0x1F, '2', '"'),
    k(0x20, '3', '#'),
    k(0x21, '4', '$'),
    k(0x22, '5', '%'),
    k(0x23, '6', '&'),
    k(0x24, '7', '\''),
    k(0x25, '8', '('),
    k(0x26, '9', ')'),
    k(0x27, '0', '\0'),
    k(0x2D, '-', '='),
    k(0x2E, '^', '~'),
    k(0x2F, '@', '`'),
    k(0x30, '[', '{'),
    k(0x31, ']', '}'),
    k(0x32, ']', '}'),
    k(0x33, ';', '+'),
    k(0x34, ':', '*'),
    // Zenkaku/Hankaku: no character.
    k(0x35, '\0', '\0'),
    k(0x36, ',', '<'),
    k(0x37, '.', '>'),
    k(0x38, '/', '?'),
    k(0x87, '\\', '_'),
    k(0x89, '¥', '|'),
];

const BR: &[E] = &[
    ka(0x1E, '1', '!', '¹'),
    ka(0x1F, '2', '@', '²'),
    ka(0x20, '3', '#', '³'),
    ka(0x21, '4', '$', '£'),
    ka(0x22, '5', '%', '¢'),
    kd(0x23, '6', '¨', '¬', S),
    k(0x24, '7', '&'),
    k(0x25, '8', '*'),
    k(0x26, '9', '('),
    k(0x27, '0', ')'),
    k(0x2D, '-', '_'),
    ka(0x2E, '=', '+', '§'),
    kd(0x2F, '´', '`', '\0', B | S),
    ka(0x30, '[', '{', 'ª'),
    ka(0x31, ']', '}', 'º'),
    ka(0x32, ']', '}', 'º'),
    k(0x33, 'ç', 'Ç'),
    kd(0x34, '~', '^', '\0', B | S),
    k(0x35, '\'', '"'),
    k(0x36, ',', '<'),
    k(0x37, '.', '>'),
    k(0x38, ';', ':'),
    k(0x64, '\\', '|'),
    ka(0x87, '/', '?', '°'),
];

/// The usages a layout gives characters, in the order [`Layout::stroke`] looks for one.
const USAGES: [u8; 55] = {
    let mut a = [0u8; 55];
    let mut i = 0;
    while i < 26 {
        a[i] = 0x04 + i as u8;
        i += 1;
    }
    while i < 36 {
        a[i] = 0x1E + (i - 26) as u8;
        i += 1;
    }
    let rest = [0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x64, 0x87, 0x89];
    let mut j = 0;
    while j < rest.len() {
        a[i] = rest[j];
        i += 1;
        j += 1;
    }
    // Space types itself everywhere.
    a[i] = 0x2C;
    let _ = i;
    a
};

impl Layout {
    pub const ALL: [Layout; 13] = [
        Layout::Us,
        Layout::Gb,
        Layout::De,
        Layout::Ch,
        Layout::Fr,
        Layout::Es,
        Layout::It,
        Layout::Pt,
        Layout::No,
        Layout::Se,
        Layout::Dk,
        Layout::Jp,
        Layout::Br,
    ];

    pub fn from_u8(v: u8) -> Option<Layout> {
        Self::ALL.get(v as usize).copied()
    }

    /// Its name for `odkvmctl` and the config (`us`, `de`, …).
    pub const fn name(self) -> &'static str {
        match self {
            Layout::Us => "us",
            Layout::Gb => "gb",
            Layout::De => "de",
            Layout::Ch => "ch",
            Layout::Fr => "fr",
            Layout::Es => "es",
            Layout::It => "it",
            Layout::Pt => "pt",
            Layout::No => "no",
            Layout::Se => "se",
            Layout::Dk => "dk",
            Layout::Jp => "jp",
            Layout::Br => "br",
        }
    }

    pub fn from_name(name: &str) -> Option<Layout> {
        Self::ALL.iter().copied().find(|l| l.name() == name)
    }

    /// The keyboards sold for it.
    pub const fn physical(self) -> Physical {
        match self {
            Layout::Us => Physical::Ansi,
            Layout::Jp => Physical::Jis,
            Layout::Br => Physical::Abnt2,
            _ => Physical::Iso,
        }
    }

    const fn table(self) -> &'static [E] {
        match self {
            Layout::Us => US,
            Layout::Gb => GB,
            Layout::De => DE,
            Layout::Ch => CH,
            Layout::Fr => FR,
            Layout::Es => ES,
            Layout::It => IT,
            Layout::Pt => PT,
            Layout::No => NO,
            Layout::Se => SE,
            Layout::Dk => DK,
            Layout::Jp => JP,
            Layout::Br => BR,
        }
    }

    /// What key `usage` types; None for a key that types the same everywhere (or nothing).
    pub fn legend(self, usage: u8) -> Option<Legend> {
        if let Some((_, c, dead)) = self.table().iter().find(|(u, _, _)| *u == usage) {
            return Some(Legend { base: c[0], shift: c[1], altgr: c[2], shift_altgr: c[3], dead: *dead });
        }
        match usage {
            0x04..=0x1D => {
                let c = (b'a' + usage - 0x04) as char;
                Some(Legend { base: c, shift: c.to_ascii_uppercase(), altgr: '\0', shift_altgr: '\0', dead: 0 })
            }
            0x2C => Some(Legend { base: ' ', shift: ' ', altgr: '\0', shift_altgr: '\0', dead: 0 }),
            _ => None,
        }
    }

    /// How to type `c` on a host with this layout: the key with the fewest modifiers that types it (Enter, Tab,
    /// Backspace, Escape and Delete too). None for a character the layout has no key for.
    pub fn stroke(self, c: char) -> Option<Stroke> {
        let plain = |usage| Some(Stroke { usage, mods: 0, dead: false });
        match c {
            '\n' => return plain(0x28),
            '\u{1B}' => return plain(0x29),
            '\u{08}' => return plain(0x2A),
            '\t' => return plain(0x2B),
            '\u{7F}' => return plain(0x4C),
            '\0' => return None,
            _ => {}
        }
        for level in 0..4 {
            for &u in USAGES.iter() {
                if let Some(l) = self.legend(u) {
                    let (ch, mods, dead) = l.chars()[level];
                    if ch == c {
                        return Some(Stroke { usage: u, mods, dead });
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_ascii_character_types_on_the_us_layout_as_before() {
        for c in (0x20u8..0x7F).map(|b| b as char) {
            let s = Layout::Us.stroke(c).unwrap_or_else(|| panic!("{c:?}"));
            assert!(!s.dead && s.mods & mods::RALT == 0, "{c:?}");
        }
        assert_eq!(Layout::Us.stroke('A'), Some(Stroke { usage: 0x04, mods: mods::LSHIFT, dead: false }));
        assert_eq!(Layout::Us.stroke('|'), Some(Stroke { usage: 0x31, mods: mods::LSHIFT, dead: false }));
    }

    #[test]
    fn other_layouts_move_the_keys() {
        // German: y and z swapped, @ on AltGr+Q, ^ a dead key.
        assert_eq!(Layout::De.stroke('z').map(|s| s.usage), Some(0x1C));
        assert_eq!(Layout::De.stroke('@'), Some(Stroke { usage: 0x14, mods: mods::RALT, dead: false }));
        assert_eq!(Layout::De.stroke('^'), Some(Stroke { usage: 0x35, mods: 0, dead: true }));
        // French: digits with Shift, a on the Q key.
        assert_eq!(Layout::Fr.stroke('1'), Some(Stroke { usage: 0x1E, mods: mods::LSHIFT, dead: false }));
        assert_eq!(Layout::Fr.stroke('a').map(|s| s.usage), Some(0x14));
        // Italian: braces need Shift and AltGr.
        assert_eq!(Layout::It.stroke('{').map(|s| s.mods), Some(mods::LSHIFT | mods::RALT));
        // Japanese: backslash on Ro.
        assert_eq!(Layout::Jp.stroke('\\').map(|s| s.usage), Some(0x87));
        // Everything the layouts have for ASCII letters and digits.
        for l in Layout::ALL {
            for c in ('a'..='z').chain('A'..='Z').chain('0'..='9') {
                assert!(l.stroke(c).is_some(), "{l:?} {c:?}");
            }
            assert_eq!(Layout::from_name(l.name()), Some(l));
            assert_eq!(Layout::from_u8(l as u8), Some(l));
        }
    }
}
