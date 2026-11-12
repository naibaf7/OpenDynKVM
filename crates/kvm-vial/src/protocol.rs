//! The Vial app's requests: VIA protocol 9 plus Vial protocol 6, 32-byte reports (software.md §4.11).
//!
//! The command set and reply layouts follow RMK's `host/via/mod.rs` and `host/via/vial.rs` (themselves after
//! vial-qmk's `via.c` and `vial.c`): VIA values are big-endian, Vial's little-endian, and every reply starts as a
//! copy of its request. Rules of this KVM on top:
//!
//! - Every write needs "configuration from software" (config key 0x0041); without it, writes are ignored.
//! - **Unlock** (Vial's guard for macros, the matrix tester and QK_BOOT): hold both Shift keys for
//!   [`UNLOCK_MS`]. It is per PC, and lasts until that PC's app locks again or the KVM restarts.
//! - The matrix tester answers only the active PC: the inactive one would otherwise read what is typed on the other.
//! - The bootloader jump is ignored; the KVM is updated with `odkvmctl`.

use kvm_hid::KeyState;

use crate::keycode::{KC_NO, QK_BOOT};
use crate::keymap::{Keymap, MACRO_BUFFER, MACRO_COUNT, Settings, qsid};
use crate::remap::Remap;
use crate::{COLS, DEFINITION, KEYBOARD_UID, LAYERS, POSITIONS, REPORT_LEN, ROWS};

pub const VIA_PROTOCOL_VERSION: u16 = 0x0009;
pub const VIAL_PROTOCOL_VERSION: u32 = 6;
const VIA_FIRMWARE_VERSION: u32 = 1;
/// Both Shift keys (row, column): present on every layout, and harmless to hold.
pub const UNLOCK_KEYS: [(u8, u8); 2] = [(0xE1 / 8, 0xE1 % 8), (0xE5 / 8, 0xE5 % 8)];
pub const UNLOCK_MS: u32 = 3000;
const UNHANDLED: u8 = 0xFF;

/// Who asks, and what they may do.
pub struct Context<'a> {
    /// 0 = PC1, 1 = PC2.
    pub pc: usize,
    pub active: bool,
    /// Configuration from software is allowed.
    pub writable: bool,
    /// The physical keys, before hotkeys and the keymap.
    pub physical: &'a KeyState,
    pub now: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Session {
    unlocked: bool,
    unlocking: bool,
    held_since: Option<u32>,
}

pub struct Vial {
    pub keymap: Keymap,
    pub remap: Remap,
    sessions: [Session; 2],
}

impl Default for Vial {
    fn default() -> Self {
        Self::new()
    }
}

impl Vial {
    pub const fn new() -> Self {
        const LOCKED: Session = Session { unlocked: false, unlocking: false, held_since: None };
        Self { keymap: Keymap::new(), remap: Remap::new(), sessions: [LOCKED; 2] }
    }

    /// Locks both PCs' sessions (the interface went away).
    pub fn lock(&mut self) {
        self.sessions = [Session::default(); 2];
    }

    pub fn is_unlocked(&self, pc: usize) -> bool {
        self.sessions[pc & 1].unlocked
    }

    /// Handles one request; returns the reply.
    pub fn request(&mut self, ctx: &Context, req: &[u8]) -> [u8; REPORT_LEN] {
        let mut q = [0u8; REPORT_LEN];
        let n = req.len().min(REPORT_LEN);
        q[..n].copy_from_slice(&req[..n]);
        let mut r = q;
        let be16 = |i: usize| u16::from_be_bytes([q[i], q[i + 1]]);
        let unlocked = self.sessions[ctx.pc & 1].unlocked;
        let km = &mut self.keymap;
        match q[0] {
            // id_get_protocol_version
            0x01 => r[1..3].copy_from_slice(&VIA_PROTOCOL_VERSION.to_be_bytes()),
            // id_get_keyboard_value
            0x02 => match q[1] {
                0x01 => r[2..6].copy_from_slice(&ctx.now.to_be_bytes()),
                0x02 => r[2..6].copy_from_slice(&km.settings.layout_options.to_be_bytes()),
                0x03 => {
                    r[2..].fill(0);
                    if unlocked && ctx.active {
                        for row in 0..ROWS {
                            let mut bits = 0u8;
                            for col in 0..COLS {
                                if ctx.physical.is_pressed((row * COLS + col) as u8) {
                                    bits |= 1 << col;
                                }
                            }
                            r[2 + row] = bits;
                        }
                    }
                }
                0x04 => r[2..6].copy_from_slice(&VIA_FIRMWARE_VERSION.to_be_bytes()),
                _ => r[0] = UNHANDLED,
            },
            // id_set_keyboard_value
            0x03 => match q[1] {
                0x02 => {
                    if ctx.writable {
                        km.settings.layout_options = u32::from_be_bytes([q[2], q[3], q[4], q[5]]);
                        km.settings_changed();
                    }
                }
                _ => r[0] = UNHANDLED,
            },
            // id_dynamic_keymap_get_keycode
            0x04 => {
                let kc = km.get(q[1] as usize, q[2] as usize * COLS + q[3] as usize);
                r[4..6].copy_from_slice(&kc.to_be_bytes());
            }
            // id_dynamic_keymap_set_keycode
            0x05 => {
                if ctx.writable && (q[2] as usize) < ROWS && (q[3] as usize) < COLS {
                    km.set(q[1] as usize, q[2] as usize * COLS + q[3] as usize, firewall(be16(4), unlocked));
                }
            }
            // id_dynamic_keymap_reset
            0x06 => {
                if ctx.writable {
                    km.reset_layers();
                }
            }
            // id_eeprom_reset
            0x0A => {
                if ctx.writable && unlocked {
                    km.reset();
                }
            }
            // id_bootloader_jump: not here.
            0x0B => {}
            0x0C => r[1] = MACRO_COUNT,
            0x0D => r[1..3].copy_from_slice(&(MACRO_BUFFER as u16).to_be_bytes()),
            // id_dynamic_keymap_macro_get_buffer
            0x0E => {
                let (offset, size) = (be16(1) as usize, q[3] as usize);
                if size <= REPORT_LEN - 4 {
                    r[4..].fill(0);
                    let src = km.macros().get(offset..).unwrap_or(&[]);
                    let k = src.len().min(size);
                    r[4..4 + k].copy_from_slice(&src[..k]);
                } else {
                    r[0] = UNHANDLED;
                }
            }
            // id_dynamic_keymap_macro_set_buffer: only unlocked (vial-qmk's rule)
            0x0F => {
                let (offset, size) = (be16(1) as usize, q[3] as usize);
                if size > REPORT_LEN - 4 {
                    r[0] = UNHANDLED;
                } else if ctx.writable && unlocked {
                    km.write_macros(offset, &q[4..4 + size]);
                }
            }
            0x10 => {
                if ctx.writable && unlocked {
                    km.reset_macros();
                }
            }
            0x11 => r[1] = LAYERS as u8,
            // id_dynamic_keymap_get_buffer: the keymap as one big-endian array, layer by layer.
            0x12 => {
                let (offset, size) = (be16(1) as usize, q[3] as usize);
                if size <= REPORT_LEN - 4 {
                    r[4..].fill(0);
                    for i in 0..size {
                        let b = offset + i;
                        let kc = flat(km, b / 2);
                        r[4 + i] = if b % 2 == 0 { (kc >> 8) as u8 } else { kc as u8 };
                    }
                } else {
                    r[0] = UNHANDLED;
                }
            }
            // id_dynamic_keymap_set_buffer
            0x13 => {
                let (offset, size) = (be16(1) as usize, q[3] as usize);
                if size > REPORT_LEN - 4 {
                    r[0] = UNHANDLED;
                } else if ctx.writable {
                    for i in 0..size {
                        let b = offset + i;
                        let idx = b / 2;
                        if idx >= LAYERS * POSITIONS {
                            break;
                        }
                        let kc = flat(km, idx);
                        let kc = if b % 2 == 0 {
                            (kc & 0x00FF) | (q[4 + i] as u16) << 8
                        } else {
                            (kc & 0xFF00) | q[4 + i] as u16
                        };
                        km.set(idx / POSITIONS, idx % POSITIONS, firewall(kc, unlocked));
                    }
                }
            }
            // Encoders: none.
            0x14 | 0x15 => r[1..].fill(0),
            0xFE => self.vial(ctx, &q, &mut r),
            _ => r[0] = UNHANDLED,
        }
        r
    }

    fn vial(&mut self, ctx: &Context, q: &[u8; REPORT_LEN], r: &mut [u8; REPORT_LEN]) {
        let le16 = |i: usize| u16::from_le_bytes([q[i], q[i + 1]]);
        let s = &mut self.sessions[ctx.pc & 1];
        let km = &mut self.keymap;
        match q[1] {
            // vial_get_keyboard_id: protocol version, keyboard UID, feature flags (no VialRGB).
            0x00 => {
                r.fill(0);
                r[0..4].copy_from_slice(&VIAL_PROTOCOL_VERSION.to_le_bytes());
                r[4..12].copy_from_slice(&KEYBOARD_UID);
            }
            0x01 => {
                r.fill(0);
                r[0..4].copy_from_slice(&(DEFINITION.len() as u32).to_le_bytes());
            }
            // vial_get_def: 32-byte block `page`.
            0x02 => {
                r.fill(0);
                let start = le16(2) as usize * REPORT_LEN;
                if start < DEFINITION.len() {
                    let end = (start + REPORT_LEN).min(DEFINITION.len());
                    r[..end - start].copy_from_slice(&DEFINITION[start..end]);
                }
            }
            0x03 | 0x04 => r.fill(0),
            // vial_get_unlock_status
            0x05 => {
                r.fill(0xFF);
                r[0] = s.unlocked as u8;
                r[1] = s.unlocking as u8;
                for (i, (row, col)) in UNLOCK_KEYS.iter().enumerate() {
                    r[2 + 2 * i] = *row;
                    r[3 + 2 * i] = *col;
                }
            }
            0x06 => {
                s.unlocking = !s.unlocked;
                s.held_since = None;
            }
            // vial_unlock_poll: [0] unlocked, [1] in progress, [2] counter (100 ms units still to hold).
            0x07 => {
                let mut left = UNLOCK_MS;
                if s.unlocking {
                    let held = UNLOCK_KEYS.iter().all(|(row, col)| ctx.physical.is_pressed(row * 8 + col));
                    if held {
                        let since = *s.held_since.get_or_insert(ctx.now);
                        left = UNLOCK_MS.saturating_sub(ctx.now.wrapping_sub(since));
                        if left == 0 {
                            s.unlocked = true;
                            s.unlocking = false;
                        }
                    } else {
                        s.held_since = None;
                    }
                }
                r[0] = s.unlocked as u8;
                r[1] = s.unlocking as u8;
                r[2] = if s.unlocked { 0 } else { left.div_ceil(100) as u8 };
            }
            0x08 => *s = Session::default(),
            // vial_qmk_settings_query: the supported qsids above the given one, 0xFFFF after the last.
            0x09 => {
                let above = le16(2);
                r.fill(0xFF);
                for (i, id) in qsid::ALL.iter().filter(|id| **id > above).enumerate() {
                    r[2 * i..2 * i + 2].copy_from_slice(&id.to_le_bytes());
                }
            }
            0x0A => {
                let id = le16(2);
                r.fill(0);
                r[0] = km.settings.get(id, &mut r[1..]).is_none() as u8;
            }
            0x0B => {
                let id = le16(2);
                let ok = ctx.writable && km.settings.set(id, &q[4..]);
                if ok {
                    km.settings_changed();
                }
                r.fill(0);
                r[0] = !ok as u8;
            }
            0x0C => {
                if ctx.writable {
                    let keep = (km.settings.layout_options, km.settings.default_layer);
                    km.settings = Settings { layout_options: keep.0, default_layer: keep.1, ..Settings::default() };
                    km.settings_changed();
                }
            }
            // vial_dynamic_entry_op: no tap dance, combos, key overrides or alt-repeat entries, no feature flags.
            0x0D => r.fill(0),
            _ => r[0] = UNHANDLED,
        }
    }
}

/// Keycode `idx` of the flat keymap (layer × position).
fn flat(km: &Keymap, idx: usize) -> u16 {
    if idx < LAYERS * POSITIONS { km.get(idx / POSITIONS, idx % POSITIONS) } else { KC_NO }
}

/// vial-qmk's firewall: QK_BOOT only from an unlocked session.
fn firewall(kc: u16, unlocked: bool) -> u16 {
    if kc == QK_BOOT && !unlocked { KC_NO } else { kc }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(physical: &KeyState, now: u32) -> Context<'_> {
        Context { pc: 0, active: true, writable: true, physical, now }
    }

    fn ask(v: &mut Vial, c: &Context, req: &[u8]) -> [u8; REPORT_LEN] {
        v.request(c, req)
    }

    /// The requests vial-gui sends when it connects (keyboard_comm.py `reload`), in that order.
    #[test]
    fn vial_gui_connect_sequence() {
        let mut v = Vial::new();
        let none = KeyState::EMPTY;
        let c = ctx(&none, 1000);
        let id = ask(&mut v, &c, &[0xFE, 0x00]);
        assert_eq!(u32::from_le_bytes([id[0], id[1], id[2], id[3]]), 6);
        assert_eq!(id[4..12], KEYBOARD_UID);
        let size = ask(&mut v, &c, &[0xFE, 0x01]);
        let size = u32::from_le_bytes([size[0], size[1], size[2], size[3]]) as usize;
        let mut def = std::vec::Vec::new();
        for block in 0..size.div_ceil(REPORT_LEN) {
            let mut req = [0u8; 6];
            req[..2].copy_from_slice(&[0xFE, 0x02]);
            req[2..6].copy_from_slice(&(block as u32).to_le_bytes());
            let d = ask(&mut v, &c, &req);
            def.extend_from_slice(&d[..(size - block * REPORT_LEN).min(REPORT_LEN)]);
        }
        assert_eq!(def, DEFINITION);
        assert_eq!(ask(&mut v, &c, &[0x01])[1..3], [0x00, 0x09]);
        assert_eq!(ask(&mut v, &c, &[0x11])[1], 8);
        assert_eq!(ask(&mut v, &c, &[0x0C])[1], 16);
        assert_eq!(ask(&mut v, &c, &[0x0D])[1..3], [0x08, 0x00]);
        let dynamic = ask(&mut v, &c, &[0xFE, 0x0D, 0x00]);
        assert!(dynamic.iter().all(|b| *b == 0));
        // Keymap buffer, as reload_keymap fetches it: layer 0 holds the identity, big-endian.
        let mut buf = std::vec::Vec::new();
        let total = LAYERS * POSITIONS * 2;
        for offset in (0..total).step_by(28) {
            let sz = (total - offset).min(28) as u8;
            let o = (offset as u16).to_be_bytes();
            let d = ask(&mut v, &c, &[0x12, o[0], o[1], sz]);
            buf.extend_from_slice(&d[4..4 + sz as usize]);
        }
        assert_eq!(buf[2 * 0x04..2 * 0x04 + 2], [0x00, 0x04]);
        assert_eq!(buf[2 * POSITIONS..2 * POSITIONS + 2], [0x00, 0x01], "layer 1 is transparent");
        // QMK settings: the query ends at 0xFFFF; tapping term reads 200.
        let q = ask(&mut v, &c, &[0xFE, 0x09, 0, 0]);
        assert_eq!(q[..12], [6, 0, 7, 0, 18, 0, 22, 0, 23, 0, 0xFF, 0xFF]);
        let tt = ask(&mut v, &c, &[0xFE, 0x0A, 7, 0]);
        assert_eq!(tt[..3], [0, 200, 0]);
    }

    #[test]
    fn set_keycode_and_buffer_write_are_big_endian() {
        let mut v = Vial::new();
        let none = KeyState::EMPTY;
        let c = ctx(&none, 0);
        // Caps Lock (0x39 = row 7, column 1) → LCTL(KC_ESC) on layer 0.
        ask(&mut v, &c, &[0x05, 0, 7, 1, 0x01, 0x29]);
        assert_eq!(v.keymap.get(0, 0x39), 0x0129);
        assert_eq!(ask(&mut v, &c, &[0x04, 0, 7, 1])[4..6], [0x01, 0x29]);
        // Two keycodes through set_buffer at layer 1, position 4.
        let off = ((POSITIONS + 4) * 2) as u16;
        let o = off.to_be_bytes();
        ask(&mut v, &c, &[0x13, o[0], o[1], 4, 0x52, 0x21, 0x00, 0x05]);
        assert_eq!((v.keymap.get(1, 4), v.keymap.get(1, 5)), (0x5221, 0x0005));
        // QK_BOOT is refused while locked.
        ask(&mut v, &c, &[0x05, 0, 0, 4, 0x7C, 0x00]);
        assert_eq!(v.keymap.get(0, 4), KC_NO);
        // Nothing is written without "configuration from software".
        let ro = Context { writable: false, ..ctx(&none, 0) };
        ask(&mut v, &ro, &[0x05, 0, 0, 5, 0x00, 0x06]);
        assert_eq!(v.keymap.get(0, 5), 0x0005);
    }

    #[test]
    fn unlock_needs_both_shifts_held_and_gates_macros_and_matrix() {
        let mut v = Vial::new();
        let none = KeyState::EMPTY;
        let mut shifts = KeyState::EMPTY;
        shifts.press(0xE1);
        shifts.press(0xE5);
        // Locked: the macro write is ignored, the matrix reads empty.
        ask(&mut v, &ctx(&none, 0), &[0x0F, 0, 0, 2, b'h', b'i']);
        assert_eq!(&v.keymap.macros()[..2], [0, 0]);
        assert!(ask(&mut v, &ctx(&shifts, 0), &[0x02, 0x03])[2..].iter().all(|b| *b == 0));
        let status = ask(&mut v, &ctx(&none, 0), &[0xFE, 0x05]);
        assert_eq!(status[..7], [0, 0, 28, 1, 28, 5, 0xFF]);
        ask(&mut v, &ctx(&none, 0), &[0xFE, 0x06]);
        let p = ask(&mut v, &ctx(&shifts, 100), &[0xFE, 0x07]);
        assert_eq!(p[..3], [0, 1, 30]);
        let p = ask(&mut v, &ctx(&none, 1500), &[0xFE, 0x07]);
        assert_eq!(p[..3], [0, 1, 30], "released: starts over");
        ask(&mut v, &ctx(&shifts, 2000), &[0xFE, 0x07]);
        let p = ask(&mut v, &ctx(&shifts, 3500), &[0xFE, 0x07]);
        assert_eq!(p[..3], [0, 1, 15]);
        let p = ask(&mut v, &ctx(&shifts, 5000), &[0xFE, 0x07]);
        assert_eq!(p[..3], [1, 0, 0]);
        ask(&mut v, &ctx(&none, 5100), &[0x0F, 0, 0, 2, b'h', b'i']);
        assert_eq!(&v.keymap.macros()[..2], b"hi");
        // The matrix: Left Shift is row 28, column 1.
        let m = ask(&mut v, &ctx(&shifts, 5200), &[0x02, 0x03]);
        assert_eq!(m[2 + 28], 0b0010_0010);
        // The inactive PC sees nothing, and PC2 has its own lock.
        let inactive = Context { active: false, ..ctx(&shifts, 5300) };
        assert!(ask(&mut v, &inactive, &[0x02, 0x03])[2..].iter().all(|b| *b == 0));
        assert!(!v.is_unlocked(1));
        ask(&mut v, &ctx(&none, 6000), &[0xFE, 0x08]);
        assert!(!v.is_unlocked(0));
    }
}
