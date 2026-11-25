//! What the KVM keeps per device rather than per profile (hid_mapping.md §8, §9): the calibration of its gamepad
//! axes and its field overrides (bits of its reports made controls), applied to its reports before they are merged.
//!
//! A device's identity is a hash: of its VID, PID and serial number on a front port, of its address as a Bluetooth
//! device. The KVM remembers [`MAX_DEVICES`]; one more takes the place of the one set longest ago. Each is a flash item
//! of its own ([`ITEM`] + slot): a version, the identity, every axis's [`AxisCal`], then the overrides.

use kvm_hid::gamepad::{self, AXES, GamepadInput};
use kvm_proto::ctrl::{AxisCal, FieldOverride, MAX_OVERRIDES, field};

pub const MAX_DEVICES: usize = 16;
/// Store key of slot 0's item.
pub const ITEM: u16 = 0x0200;
const VERSION: u8 = 2;
const CAL_LEN: usize = 7;
const OVERRIDE_LEN: usize = 7;
const OVERRIDES_AT: usize = 5 + AXES * CAL_LEN;
pub const ITEM_LEN: usize = OVERRIDES_AT + MAX_OVERRIDES * OVERRIDE_LEN;

/// FNV-1a: a device's identity from what names it.
pub fn identity(parts: &[&[u8]]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for p in parts {
        for b in p.iter().chain([&0xFFu8]) {
            h ^= *b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    // 0 means none.
    h.max(1)
}

/// A normalized value through an axis's calibration: the measured range onto the full one, the deadzone around the
/// centre (a trigger has neither centre nor deadzone: its minimum is 0).
pub fn calibrate(v: i32, c: &AxisCal, trigger: bool) -> i32 {
    let (min, centre, max) = (c.min as i64, c.centre as i64, c.max as i64);
    let v = v as i64;
    let full = gamepad::FULL as i64;
    if trigger {
        if max <= min {
            return v as i32;
        }
        return ((v - min) * full / (max - min)).clamp(0, full) as i32;
    }
    let dz = (c.deadzone as i64) * full / 256;
    let out = if v >= centre {
        if max <= centre { 0 } else { (v - centre) * full / (max - centre) }
    } else if centre <= min {
        0
    } else {
        (v - centre) * full / (centre - min)
    };
    let out = out.clamp(-full, full);
    if out.abs() <= dz || dz >= full {
        return 0;
    }
    (out.signum() * (out.abs() - dz) * full / (full - dz)) as i32
}

/// A field's raw value from a report (`data` after its report ID), bits counted from the first byte's lowest.
fn field_value(data: &[u8], bit: usize, bits: usize, signed: bool) -> Option<i32> {
    if bits == 0 || bits > 16 || (bit + bits).div_ceil(8) > data.len() {
        return None;
    }
    let mut v: u32 = 0;
    for i in 0..bits {
        let b = bit + i;
        v |= ((data[b / 8] >> (b % 8)) as u32 & 1) << i;
    }
    Some(if signed && v & (1 << (bits - 1)) != 0 { v as i32 - (1 << bits) } else { v as i32 })
}

/// A field's value as a normalized axis value: its whole range onto the axis's (a trigger's from 0).
fn field_axis(v: i32, bits: usize, signed: bool, trigger: bool) -> i32 {
    let (min, max) = if signed { (-(1 << (bits - 1)), (1 << (bits - 1)) - 1) } else { (0, (1 << bits) - 1) };
    gamepad::normalize(v, min, max, trigger)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    identity: u32,
    axes: [AxisCal; AXES],
    overrides: [Option<FieldOverride>; MAX_OVERRIDES],
}

impl Entry {
    const fn new(identity: u32) -> Entry {
        Entry { identity, axes: [AxisCal::NONE; AXES], overrides: [None; MAX_OVERRIDES] }
    }
}

#[derive(Debug, Clone)]
pub struct KnownDevices {
    entries: [Option<Entry>; MAX_DEVICES],
    /// The slot a new identity takes when all are used.
    next: u8,
    /// Slots to write.
    dirty: u16,
}

impl Default for KnownDevices {
    fn default() -> Self {
        Self::new()
    }
}

impl KnownDevices {
    pub const fn new() -> KnownDevices {
        KnownDevices { entries: [None; MAX_DEVICES], next: 0, dirty: 0 }
    }

    fn slot(&self, identity: u32) -> Option<usize> {
        self.entries.iter().position(|e| e.is_some_and(|e| e.identity == identity))
    }

    pub fn get(&self, identity: u32, axis: usize) -> AxisCal {
        self.slot(identity)
            .and_then(|s| self.entries[s])
            .and_then(|e| e.axes.get(axis).copied())
            .unwrap_or(AxisCal::NONE)
    }

    /// The slot of `identity`, taken (from the one set longest ago, when all are used) if it has none.
    fn slot_for(&mut self, identity: u32) -> usize {
        match self.slot(identity) {
            Some(s) => s,
            None => {
                let s = self.entries.iter().position(Option::is_none).unwrap_or_else(|| {
                    let s = self.next as usize;
                    self.next = ((s + 1) % MAX_DEVICES) as u8;
                    s
                });
                self.entries[s] = Some(Entry::new(identity));
                s
            }
        }
    }

    /// Sets an axis's calibration; false for an axis past the last.
    pub fn set(&mut self, identity: u32, axis: usize, c: AxisCal) -> bool {
        if axis >= AXES || identity == 0 {
            return false;
        }
        let s = self.slot_for(identity);
        if let Some(e) = self.entries[s].as_mut()
            && e.axes[axis] != c
        {
            e.axes[axis] = c;
            self.dirty |= 1 << s;
        }
        true
    }

    pub fn get_override(&self, identity: u32, n: usize) -> Option<FieldOverride> {
        self.slot(identity).and_then(|s| self.entries[s]).and_then(|e| e.overrides.get(n).copied().flatten())
    }

    /// Sets field override `n` (None removes it); false past the last.
    pub fn set_override(&mut self, identity: u32, n: usize, o: Option<FieldOverride>) -> bool {
        if n >= MAX_OVERRIDES || identity == 0 {
            return false;
        }
        if o.is_none() && self.slot(identity).is_none() {
            return true;
        }
        let s = self.slot_for(identity);
        if let Some(e) = self.entries[s].as_mut()
            && e.overrides[n] != o
        {
            e.overrides[n] = o;
            self.dirty |= 1 << s;
        }
        true
    }

    /// The device's overrides that read interface `iface`'s reports.
    pub fn overrides(&self, identity: u32, iface: u8) -> impl Iterator<Item = FieldOverride> + '_ {
        self.slot(identity)
            .and_then(|s| self.entries[s].as_ref())
            .into_iter()
            .flat_map(|e| e.overrides.iter().flatten().copied())
            .filter(move |o| o.iface == iface)
    }

    /// A report of interface `iface` (`raw` as it came, its report ID first when `ids`) through the device's
    /// overrides: the fields they name become its gamepad's buttons and axes. Returns whether any applied.
    pub fn apply_overrides(
        &self,
        identity: u32,
        iface: u8,
        raw: &[u8],
        ids: bool,
        g: &mut Option<GamepadInput>,
    ) -> bool {
        let mut any = false;
        for o in self.overrides(identity, iface) {
            let (id, data) =
                if ids { (raw.first().copied().unwrap_or(0), raw.get(1..).unwrap_or(&[])) } else { (0, raw) };
            if id != o.report_id {
                continue;
            }
            let signed = o.flags & field::SIGNED != 0;
            let Some(v) = field_value(data, o.bit as usize, o.bits as usize, signed) else { continue };
            let pad = g.get_or_insert_with(GamepadInput::default);
            let invert = o.flags & field::INVERT != 0;
            if (o.target as usize) < 32 {
                let on = (v != 0) != invert;
                let bit = 1u32 << o.target;
                if on { pad.buttons |= bit } else { pad.buttons &= !bit }
            } else if let Some(a) = o.target.checked_sub(field::AXIS).map(usize::from).filter(|a| *a < AXES) {
                let trigger = gamepad::axis::is_trigger(a);
                let mut x = field_axis(v, o.bits as usize, signed, trigger);
                if invert {
                    x = if trigger { gamepad::FULL - x } else { -x };
                }
                pad.axes[a] = x;
                pad.present |= 1 << a;
            }
            any = true;
        }
        any
    }

    /// The buttons (bit per button) and axes (bit per axis) the device's overrides give it.
    pub fn override_controls(&self, identity: u32) -> (u32, u16) {
        let Some(e) = self.slot(identity).and_then(|s| self.entries[s]) else { return (0, 0) };
        e.overrides.iter().flatten().fold((0, 0), |(b, a), o| match o.target.checked_sub(field::AXIS) {
            Some(x) => (b, a | 1 << x),
            None => (b | 1 << o.target, a),
        })
    }

    /// The device's gamepad axes through its calibration.
    pub fn apply(&self, identity: u32, g: &mut GamepadInput) {
        let Some(e) = self.slot(identity).and_then(|s| self.entries[s]) else { return };
        for (a, v) in g.axes.iter_mut().enumerate() {
            if e.axes[a] != AxisCal::NONE {
                *v = calibrate(*v, &e.axes[a], gamepad::axis::is_trigger(a));
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty != 0
    }

    /// The next changed slot's item, into `out` (at least [`ITEM_LEN`] bytes); it counts as written from here.
    pub fn take_dirty(&mut self, out: &mut [u8]) -> Option<(u16, usize)> {
        if self.dirty == 0 {
            return None;
        }
        let s = self.dirty.trailing_zeros() as usize;
        self.dirty &= !(1 << s);
        let e = self.entries[s]?;
        out[0] = VERSION;
        out[1..5].copy_from_slice(&e.identity.to_le_bytes());
        for (i, c) in e.axes.iter().enumerate() {
            let at = 5 + CAL_LEN * i;
            out[at..at + 2].copy_from_slice(&c.min.to_le_bytes());
            out[at + 2..at + 4].copy_from_slice(&c.centre.to_le_bytes());
            out[at + 4..at + 6].copy_from_slice(&c.max.to_le_bytes());
            out[at + 6] = c.deadzone;
        }
        for (i, o) in e.overrides.iter().enumerate() {
            let at = OVERRIDES_AT + OVERRIDE_LEN * i;
            let o = o.unwrap_or(FieldOverride { iface: 0, report_id: 0, bit: 0, bits: 0, flags: 0, target: 0 });
            let b = o.bit.to_le_bytes();
            out[at..at + OVERRIDE_LEN].copy_from_slice(&[o.iface, o.report_id, b[0], b[1], o.bits, o.flags, o.target]);
        }
        Some((ITEM + s as u16, ITEM_LEN))
    }

    /// Takes back an item read from flash; one of another version or size is ignored.
    pub fn restore(&mut self, key: u16, d: &[u8]) {
        let Some(s) = key.checked_sub(ITEM).map(|s| s as usize).filter(|s| *s < MAX_DEVICES) else { return };
        if d.len() != ITEM_LEN || d[0] != VERSION {
            return;
        }
        let i16_at = |at: usize| i16::from_le_bytes([d[at], d[at + 1]]);
        let mut axes = [AxisCal::NONE; AXES];
        for (i, c) in axes.iter_mut().enumerate() {
            let at = 5 + CAL_LEN * i;
            *c = AxisCal { min: i16_at(at), centre: i16_at(at + 2), max: i16_at(at + 4), deadzone: d[at + 6] };
        }
        let mut overrides = [None; MAX_OVERRIDES];
        for (i, o) in overrides.iter_mut().enumerate() {
            let b = &d[OVERRIDES_AT + OVERRIDE_LEN * i..][..OVERRIDE_LEN];
            if b[4] != 0 {
                let bit = u16::from_le_bytes([b[2], b[3]]);
                *o = Some(FieldOverride { iface: b[0], report_id: b[1], bit, bits: b[4], flags: b[5], target: b[6] });
            }
        }
        let identity = u32::from_le_bytes([d[1], d[2], d[3], d[4]]);
        self.entries[s] = Some(Entry { identity, axes, overrides });
        self.next = ((s + 1) % MAX_DEVICES) as u8;
    }

    pub fn item_keys() -> impl Iterator<Item = u16> {
        (0..MAX_DEVICES as u16).map(|s| ITEM + s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worn_stick_reaches_full_deflection_and_rests_at_centre() {
        let c = AxisCal { min: -28000, centre: 1500, max: 30000, deadzone: 16 };
        assert_eq!(calibrate(1500, &c, false), 0);
        assert_eq!(calibrate(2500, &c, false), 0, "within the deadzone");
        assert_eq!(calibrate(30000, &c, false), 32767);
        assert_eq!(calibrate(32767, &c, false), 32767);
        assert_eq!(calibrate(-28000, &c, false), -32767);
        // A trigger: its measured range onto 0…32767.
        let t = AxisCal { min: 2000, centre: 0, max: 30000, deadzone: 0 };
        assert_eq!(
            (calibrate(1000, &t, true), calibrate(16000, &t, true), calibrate(30000, &t, true)),
            (0, 16383, 32767)
        );
    }

    #[test]
    fn calibrations_follow_the_identity_and_round_trip() {
        let mut c = KnownDevices::new();
        let pad = identity(&[b"045E", b"028E", b""]);
        assert_ne!(pad, identity(&[b"045E", b"028F", b""]));
        let x = AxisCal { min: -30000, centre: 500, max: 31000, deadzone: 8 };
        assert!(c.set(pad, gamepad::axis::X, x));
        assert!(!c.set(pad, AXES, x));
        let mut g = GamepadInput { present: 1, ..GamepadInput::default() };
        g.axes[0] = 31000;
        c.apply(pad, &mut g);
        assert_eq!(g.axes[0], 32767);
        let mut buf = [0u8; ITEM_LEN];
        let (key, n) = c.take_dirty(&mut buf).unwrap();
        assert!(!c.is_dirty());
        let mut d = KnownDevices::new();
        d.restore(key, &buf[..n]);
        assert_eq!(d.get(pad, gamepad::axis::X), x);
        assert_eq!(d.get(pad, gamepad::axis::Y), AxisCal::NONE);
        // More devices than slots: the one set longest ago makes room.
        for n in 0..MAX_DEVICES as u32 {
            c.set(1000 + n, 0, x);
        }
        assert_eq!(c.get(pad, 0), AxisCal::NONE);
    }

    #[test]
    fn a_touch_pad_field_becomes_an_axis_and_its_contact_a_button() {
        let mut k = KnownDevices::new();
        let pad = identity(&[b"054C", b"0CE6", b""]);
        // Report 1: bit 7 of byte 32 is 1 while no finger is on (pressed when 0), X 12 bits from byte 33.
        let contact =
            FieldOverride { iface: 3, report_id: 1, bit: 32 * 8 + 7, bits: 1, flags: field::INVERT, target: 13 };
        let x = FieldOverride { iface: 3, report_id: 1, bit: 33 * 8, bits: 12, flags: 0, target: field::AXIS + 6 };
        assert!(k.set_override(pad, 0, Some(contact)) && k.set_override(pad, 1, Some(x)));
        assert!(!k.set_override(pad, MAX_OVERRIDES, None));
        let mut r = [0u8; 64];
        r[0] = 1;
        r[1 + 32] = 0x00; // a finger on
        r[1 + 33] = 0xFF;
        r[1 + 34] = 0x0F; // X at 4095 of 0…4095
        let mut g = None;
        assert!(k.apply_overrides(pad, 3, &r, true, &mut g));
        let g = g.unwrap();
        assert_eq!((g.buttons, g.axes[6], g.present), (1 << 13, 32767, 1 << 6));
        // Another interface, or another report ID: untouched.
        let mut other = None;
        assert!(!k.apply_overrides(pad, 0, &r, true, &mut other) && other.is_none());
        r[0] = 2;
        assert!(!k.apply_overrides(pad, 3, &r, true, &mut other));
        assert_eq!(k.override_controls(pad), (1 << 13, 1 << 6));
        // Saved with the calibration.
        let mut buf = [0u8; ITEM_LEN];
        let (key, n) = k.take_dirty(&mut buf).unwrap();
        let mut back = KnownDevices::new();
        back.restore(key, &buf[..n]);
        assert_eq!((back.get_override(pad, 0), back.get_override(pad, 1)), (Some(contact), Some(x)));
        assert_eq!(field_value(&[0xFF], 4, 4, true), Some(-1));
    }
}
