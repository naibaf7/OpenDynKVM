//! Generates the Vial keyboard definition (`vial.json`, xz-compressed the way vial-qmk does it) and the table of
//! matrix positions that appear in it.
//!
//! The "matrix" is the HID keyboard page: position = usage, row = usage / 8, column = usage % 8 (lib.rs). The layout
//! is a full-size keyboard with two layout options (ISO Enter, ISO left Shift) and a block of extra keys (F13–F24,
//! international and language keys) that some keyboards have.

use std::io::Write;
use std::path::Path;
use std::{env, fs};

/// One key: HID usage, position and size in key units, and an optional layout option `(option, choice)`.
struct Key {
    usage: u8,
    x: f64,
    w: f64,
    h: f64,
    option: Option<(u8, u8)>,
    /// ISO Enter shape: (x2, w2, h2).
    iso: Option<(f64, f64, f64)>,
}

fn k(usage: u8, x: f64) -> Key {
    Key { usage, x, w: 1.0, h: 1.0, option: None, iso: None }
}

fn kw(usage: u8, x: f64, w: f64) -> Key {
    Key { w, ..k(usage, x) }
}

fn kh(usage: u8, x: f64, h: f64) -> Key {
    Key { h, ..k(usage, x) }
}

fn opt(mut key: Key, option: u8, choice: u8) -> Key {
    key.option = Some((option, choice));
    key
}

fn row(start: u8, x: f64, n: u8) -> impl Iterator<Item = Key> {
    (0..n).map(move |i| k(start + i, x + i as f64))
}

fn layout() -> Vec<(f64, Vec<Key>)> {
    let mut rows = Vec::new();
    // Function row: Esc, F1–F12, Print Screen, Scroll Lock, Pause.
    let mut r = vec![k(0x29, 0.0)];
    r.extend(row(0x3A, 2.0, 4));
    r.extend(row(0x3E, 6.5, 4));
    r.extend(row(0x42, 11.0, 4));
    r.extend(row(0x46, 15.25, 3));
    rows.push((0.0, r));
    // Number row, navigation, keypad top.
    let mut r = vec![k(0x35, 0.0)];
    r.extend(row(0x1E, 1.0, 10));
    r.extend([k(0x2D, 11.0), k(0x2E, 12.0), kw(0x2A, 13.0, 2.0)]);
    r.extend(row(0x49, 15.25, 3));
    r.extend(row(0x53, 18.5, 4));
    rows.push((1.25, r));
    // Q row (ANSI backslash = layout option 0, choice 0).
    let mut r = vec![kw(0x2B, 0.0, 1.5)];
    for (i, u) in [0x14, 0x1A, 0x08, 0x15, 0x17, 0x1C, 0x18, 0x0C, 0x12, 0x13, 0x2F, 0x30].into_iter().enumerate() {
        r.push(k(u, 1.5 + i as f64));
    }
    r.push(opt(kw(0x31, 13.5, 1.5), 0, 0));
    r.extend([k(0x4C, 15.25), k(0x4D, 16.25), k(0x4E, 17.25)]);
    r.extend([k(0x5F, 18.5), k(0x60, 19.5), k(0x61, 20.5), kh(0x57, 21.5, 2.0)]);
    rows.push((2.25, r));
    // A row (ANSI Enter = option 0, choice 0).
    let mut r = vec![kw(0x39, 0.0, 1.75)];
    for (i, u) in [0x04, 0x16, 0x07, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x33, 0x34].into_iter().enumerate() {
        r.push(k(u, 1.75 + i as f64));
    }
    r.push(opt(kw(0x28, 12.75, 2.25), 0, 0));
    r.extend([k(0x5C, 18.5), k(0x5D, 19.5), k(0x5E, 20.5)]);
    rows.push((3.25, r));
    // Z row (ANSI left Shift = option 1, choice 0).
    let mut r = vec![opt(kw(0xE1, 0.0, 2.25), 1, 0)];
    for (i, u) in [0x1D, 0x1B, 0x06, 0x19, 0x05, 0x11, 0x10, 0x36, 0x37, 0x38].into_iter().enumerate() {
        r.push(k(u, 2.25 + i as f64));
    }
    r.push(kw(0xE5, 12.25, 2.75));
    r.push(k(0x52, 16.25));
    r.extend([k(0x59, 18.5), k(0x5A, 19.5), k(0x5B, 20.5), kh(0x58, 21.5, 2.0)]);
    rows.push((4.25, r));
    // Bottom row.
    let mut r = vec![kw(0xE0, 0.0, 1.25), kw(0xE3, 1.25, 1.25), kw(0xE2, 2.5, 1.25), kw(0x2C, 3.75, 6.25)];
    r.extend([kw(0xE6, 10.0, 1.25), kw(0xE7, 11.25, 1.25), kw(0x65, 12.5, 1.25), kw(0xE4, 13.75, 1.25)]);
    r.extend([k(0x50, 15.25), k(0x51, 16.25), k(0x4F, 17.25), kw(0x62, 18.5, 2.0), k(0x63, 20.5)]);
    rows.push((5.25, r));
    // Extra keys: F13–F24; Power, Mute, Volume Up/Down (keyboard page); International 1–5, Lang 1–2, keypad = and ,.
    let mut r: Vec<Key> = row(0x68, 0.0, 12).collect();
    r.extend([k(0x66, 12.5), k(0x7F, 13.5), k(0x80, 14.5), k(0x81, 15.5)]);
    rows.push((6.75, r));
    let mut r: Vec<Key> = row(0x87, 0.0, 5).collect();
    r.extend([k(0x90, 5.5), k(0x91, 6.5), k(0x67, 8.0), k(0x85, 9.0)]);
    rows.push((7.75, r));
    // Option 0, choice 1: ISO Enter and Non-US #. Vial moves each choice onto choice 0 by their top-left corners, so
    // these sit 7 units lower, keeping their offsets.
    rows.push((
        9.25,
        vec![Key { option: Some((0, 1)), iso: Some((-0.25, 1.5, 1.0)), h: 2.0, w: 1.25, ..k(0x28, 13.75) }],
    ));
    rows.push((10.25, vec![opt(k(0x32, 12.75), 0, 1)]));
    // Option 1, choice 1: ISO left Shift and Non-US backslash.
    rows.push((11.25, vec![opt(kw(0xE1, 0.0, 1.25), 1, 1), opt(k(0x64, 1.25), 1, 1)]));
    rows
}

fn num(v: f64) -> String {
    format!("{v}")
}

/// KLE rows as Vial reads them: legend 0 = "row,col", legend 8 (the fourth line) = "option,choice".
fn kle(rows: &[(f64, Vec<Key>)]) -> String {
    let mut out = Vec::new();
    let mut prev_y = -1.0;
    for (y, keys) in rows {
        let mut items = Vec::new();
        let mut cursor = 0.0;
        for (i, key) in keys.iter().enumerate() {
            let mut props = Vec::new();
            if i == 0 && (y - (prev_y + 1.0)).abs() > 1e-9 {
                props.push(format!("\"y\":{}", num(y - (prev_y + 1.0))));
            }
            if (key.x - cursor).abs() > 1e-9 {
                props.push(format!("\"x\":{}", num(key.x - cursor)));
            }
            if key.w != 1.0 {
                props.push(format!("\"w\":{}", num(key.w)));
            }
            if key.h != 1.0 {
                props.push(format!("\"h\":{}", num(key.h)));
            }
            if let Some((x2, w2, h2)) = key.iso {
                props.push(format!("\"x2\":{},\"w2\":{},\"h2\":{}", num(x2), num(w2), num(h2)));
            }
            if !props.is_empty() {
                items.push(format!("{{{}}}", props.join(",")));
            }
            let mut legend = format!("{},{}", key.usage / 8, key.usage % 8);
            if let Some((o, c)) = key.option {
                legend += &format!("\\n\\n\\n{o},{c}");
            }
            items.push(format!("\"{legend}\""));
            cursor = key.x + key.w;
        }
        out.push(format!("[{}]", items.join(",")));
        prev_y = *y;
    }
    format!("[{}]", out.join(","))
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let rows = layout();
    let json = format!(
        concat!(
            "{{\"name\":\"OpenDynKVM\",\"vendorId\":\"0x1209\",\"productId\":\"0x0001\",\"lighting\":\"none\",",
            "\"matrix\":{{\"rows\":29,\"cols\":8}},\"layouts\":{{\"labels\":[[\"Enter\",\"ANSI\",\"ISO\"],",
            "[\"Left Shift\",\"ANSI\",\"ISO\"]],\"keymap\":{}}}}}"
        ),
        kle(&rows)
    );
    let mut used = [0u32; 8];
    for (_, keys) in &rows {
        for key in keys {
            assert!(key.usage <= 0xE7, "usage {:#x} is outside the matrix", key.usage);
            used[key.usage as usize / 32] |= 1 << (key.usage % 32);
        }
    }

    let out = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out);
    fs::write(out.join("vial.json"), &json).unwrap();
    let mut xz = xz2::write::XzEncoder::new(Vec::new(), 9);
    xz.write_all(json.as_bytes()).unwrap();
    fs::write(out.join("vial.json.xz"), xz.finish().unwrap()).unwrap();
    let words: Vec<String> = used.iter().map(|w| format!("{w:#010x}")).collect();
    fs::write(
        out.join("layout.rs"),
        format!(
            "/// Positions (HID usages) that are keys of the Vial layout; the others pass through unmapped.\n\
             pub const IN_LAYOUT: [u32; 8] = [{}];\n",
            words.join(", ")
        ),
    )
    .unwrap();
}
