//! HID report-descriptor parser (HID 1.11 §6.2.2) producing a flat list of data fields with bit positions.
//!
//! Constant (padding) main items are not stored but still advance the bit offset. Fixed capacities keep it `no_std`;
//! a descriptor that exceeds them is rejected, and the port falls back to forwarding raw reports (Mirrored).

use heapless::Vec;

use crate::usage::usage;

pub const MAX_FIELDS: usize = 64;
pub const MAX_RANGES: usize = 8;
pub const MAX_REPORTS: usize = 24;
pub const MAX_APPLICATIONS: usize = 8;
const MAX_LOCAL_USAGES: usize = 24;
const MAX_DEPTH: usize = 8;
const MAX_PUSH: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ReportType {
    Input = 1,
    Output = 2,
    Feature = 3,
}

impl ReportType {
    /// From `wValue`'s high byte of GET/SET_REPORT.
    pub fn from_request(v: u8) -> Option<Self> {
        match v {
            1 => Some(ReportType::Input),
            2 => Some(ReportType::Output),
            3 => Some(ReportType::Feature),
            _ => None,
        }
    }
}

/// Main item data bits.
pub mod flags {
    pub const CONSTANT: u16 = 1 << 0;
    pub const VARIABLE: u16 = 1 << 1;
    pub const RELATIVE: u16 = 1 << 2;
    pub const WRAP: u16 = 1 << 3;
    pub const NONLINEAR: u16 = 1 << 4;
    pub const NO_PREFERRED: u16 = 1 << 5;
    pub const NULL_STATE: u16 = 1 << 6;
    pub const VOLATILE: u16 = 1 << 7;
    pub const BUFFERED_BYTES: u16 = 1 << 8;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ParseError {
    Truncated,
    TooManyFields,
    TooManyUsages,
    TooManyReports,
    UnbalancedCollection,
    PushPop,
    ReportTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct UsageRange {
    pub min: u32,
    pub max: u32,
}

impl UsageRange {
    fn len(&self) -> u32 {
        self.max.saturating_sub(self.min) + 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Field {
    pub ty: ReportType,
    /// 0 when the descriptor uses no report IDs.
    pub report_id: u8,
    /// Offset in bits from the start of the report payload (after the report ID byte).
    pub bit_offset: u16,
    pub bit_size: u8,
    pub count: u16,
    pub flags: u16,
    pub logical_min: i32,
    pub logical_max: i32,
    pub physical_min: i32,
    pub physical_max: i32,
    /// For a variable field, element *i* has the *i*-th usage (the last one repeats); for an array field, value *v*
    /// stands for the (*v* − logical_min)-th usage.
    pub usages: Vec<UsageRange, MAX_RANGES>,
    /// Usage of the enclosing top-level application collection.
    pub application: u32,
}

impl Field {
    pub fn is_variable(&self) -> bool {
        self.flags & flags::VARIABLE != 0
    }

    pub fn is_array(&self) -> bool {
        !self.is_variable()
    }

    pub fn is_relative(&self) -> bool {
        self.flags & flags::RELATIVE != 0
    }

    pub fn has_null_state(&self) -> bool {
        self.flags & flags::NULL_STATE != 0
    }

    pub fn is_signed(&self) -> bool {
        self.logical_min < 0
    }

    /// Readable as an integer (sizes up to 32 bits).
    pub fn is_readable(&self) -> bool {
        (1..=32).contains(&self.bit_size)
    }

    fn nth_usage(&self, mut n: u32) -> Option<u32> {
        for r in &self.usages {
            if n < r.len() {
                return Some(r.min + n);
            }
            n -= r.len();
        }
        None
    }

    /// Usage of element `index` of a variable field.
    pub fn usage(&self, index: usize) -> Option<u32> {
        let total: u32 = self.usages.iter().map(UsageRange::len).sum();
        if total == 0 {
            return None;
        }
        self.nth_usage((index as u32).min(total - 1))
    }

    /// Usage selected by an array field's value, `None` for "no event" (out of range).
    pub fn array_usage(&self, value: i32) -> Option<u32> {
        if value < self.logical_min || value > self.logical_max {
            return None;
        }
        self.nth_usage((value - self.logical_min) as u32)
    }

    /// Every usage this field can report (variable elements or array choices).
    pub fn all_usages(&self) -> impl Iterator<Item = u32> + '_ {
        self.usages.iter().flat_map(|r| r.min..=r.max)
    }

    fn element_bit(&self, index: usize) -> usize {
        self.bit_offset as usize + index * self.bit_size as usize
    }

    /// Raw (unsigned) value of element `index`; 0 if the report is too short.
    pub fn read_raw(&self, payload: &[u8], index: usize) -> u32 {
        read_bits(payload, self.element_bit(index), self.bit_size as usize)
    }

    /// Value of element `index`, sign-extended when the logical range is signed.
    pub fn read(&self, payload: &[u8], index: usize) -> i32 {
        let raw = self.read_raw(payload, index);
        if self.is_signed() && self.bit_size < 32 {
            let shift = 32 - self.bit_size as u32;
            ((raw << shift) as i32) >> shift
        } else {
            raw as i32
        }
    }

    pub fn write(&self, payload: &mut [u8], index: usize, value: i32) {
        write_bits(payload, self.element_bit(index), self.bit_size as usize, value as u32);
    }
}

/// Little-endian bit field read (HID reports are LSB first).
pub fn read_bits(buf: &[u8], bit: usize, size: usize) -> u32 {
    let mut v = 0u64;
    let first = bit / 8;
    let last = (bit + size).div_ceil(8);
    if size == 0 || size > 32 || last > buf.len() {
        return 0;
    }
    for (i, b) in buf[first..last].iter().enumerate() {
        v |= (*b as u64) << (8 * i);
    }
    ((v >> (bit % 8)) & ((1u64 << size) - 1)) as u32
}

pub fn write_bits(buf: &mut [u8], bit: usize, size: usize, value: u32) {
    if size == 0 || size > 32 || (bit + size).div_ceil(8) > buf.len() {
        return;
    }
    for i in 0..size {
        let pos = bit + i;
        let mask = 1u8 << (pos % 8);
        if value >> i & 1 != 0 {
            buf[pos / 8] |= mask;
        } else {
            buf[pos / 8] &= !mask;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
struct ReportSize {
    ty: ReportType,
    id: u8,
    bits: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Layout {
    pub fields: Vec<Field, MAX_FIELDS>,
    pub uses_report_ids: bool,
    /// Usages of the top-level application collections, in order.
    pub applications: Vec<u32, MAX_APPLICATIONS>,
    sizes: Vec<ReportSize, MAX_REPORTS>,
}

impl Layout {
    pub fn parse(desc: &[u8]) -> Result<Layout, ParseError> {
        Parser::default().run(desc)
    }

    /// Payload length in bytes (without the report ID byte) of report `id` of type `ty`; 0 if it doesn't exist.
    pub fn report_len(&self, ty: ReportType, id: u8) -> usize {
        self.sizes.iter().find(|s| s.ty == ty && s.id == id).map_or(0, |s| s.bits.div_ceil(8) as usize)
    }

    /// Report IDs of type `ty` (one entry, 0, when report IDs are not used).
    pub fn report_ids(&self, ty: ReportType) -> impl Iterator<Item = u8> + '_ {
        self.sizes.iter().filter(move |s| s.ty == ty && s.bits > 0).map(|s| s.id)
    }

    pub fn fields_of(&self, ty: ReportType, id: u8) -> impl Iterator<Item = &Field> {
        self.fields.iter().filter(move |f| f.ty == ty && f.report_id == id)
    }

    /// Splits a received input report into its ID and payload.
    pub fn split_report<'a>(&self, report: &'a [u8]) -> Option<(u8, &'a [u8])> {
        if self.uses_report_ids { report.split_first().map(|(id, rest)| (*id, rest)) } else { Some((0, report)) }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Globals {
    usage_page: u16,
    logical_min: i32,
    logical_max: i32,
    physical_min: i32,
    physical_max: i32,
    report_size: u32,
    report_count: u32,
    report_id: u8,
    /// Logical Maximum as an unsigned value, for the signedness fix in `data_item`.
    logical_max_raw: u32,
}

#[derive(Debug, Clone, Copy)]
enum LocalUsage {
    /// Value and whether it carried its own page (4-byte item).
    Single(u32, bool),
    Range(u32, u32, bool),
}

#[derive(Default)]
struct Parser {
    globals: Globals,
    stack: Vec<Globals, MAX_PUSH>,
    usages: Vec<LocalUsage, MAX_LOCAL_USAGES>,
    usage_min: Option<(u32, bool)>,
    delimiter_depth: u8,
    delimiter_seen: bool,
    collections: Vec<u32, MAX_DEPTH>,
    layout: Layout,
}

fn sign_extend(v: u32, size: usize) -> i32 {
    match size {
        1 => v as u8 as i8 as i32,
        2 => v as u16 as i16 as i32,
        _ => v as i32,
    }
}

impl Parser {
    fn run(mut self, desc: &[u8]) -> Result<Layout, ParseError> {
        let mut i = 0;
        while i < desc.len() {
            let prefix = desc[i];
            if prefix == 0xFE {
                // Long item: skip (no long item tags are defined).
                let size = *desc.get(i + 1).ok_or(ParseError::Truncated)? as usize;
                i += 3 + size;
                continue;
            }
            let size = [0, 1, 2, 4][(prefix & 3) as usize];
            let data = desc.get(i + 1..i + 1 + size).ok_or(ParseError::Truncated)?;
            let mut v = 0u32;
            for (k, b) in data.iter().enumerate() {
                v |= (*b as u32) << (8 * k);
            }
            let tag = prefix >> 4;
            match (prefix >> 2) & 3 {
                0 => self.main(tag, v)?,
                1 => self.global(tag, v, size)?,
                2 => self.local(tag, v, size)?,
                _ => {}
            }
            i += 1 + size;
        }
        if !self.collections.is_empty() {
            return Err(ParseError::UnbalancedCollection);
        }
        Ok(self.layout)
    }

    fn resolve(&self, v: u32, extended: bool) -> u32 {
        if extended { v } else { usage(self.globals.usage_page, v as u16) }
    }

    fn reset_locals(&mut self) {
        self.usages.clear();
        self.usage_min = None;
        self.delimiter_seen = false;
    }

    fn main(&mut self, tag: u8, v: u32) -> Result<(), ParseError> {
        match tag {
            0x8 => self.data_item(ReportType::Input, v)?,
            0x9 => self.data_item(ReportType::Output, v)?,
            0xB => self.data_item(ReportType::Feature, v)?,
            0xA => {
                let u = match self.usages.first() {
                    Some(LocalUsage::Single(u, ext)) => self.resolve(*u, *ext),
                    Some(LocalUsage::Range(min, _, ext)) => self.resolve(*min, *ext),
                    None => 0,
                };
                // Collection type 1 = application; only top-level ones are recorded.
                if self.collections.is_empty() && (v & 0xFF) == 1 && self.layout.applications.push(u).is_err() {
                    return Err(ParseError::TooManyReports);
                }
                self.collections.push(u).map_err(|_| ParseError::UnbalancedCollection)?;
            }
            0xC => {
                self.collections.pop().ok_or(ParseError::UnbalancedCollection)?;
            }
            _ => {}
        }
        self.reset_locals();
        Ok(())
    }

    fn data_item(&mut self, ty: ReportType, data: u32) -> Result<(), ParseError> {
        let g = self.globals;
        let id = g.report_id;
        let pos = match self.layout.sizes.iter().position(|s| s.ty == ty && s.id == id) {
            Some(p) => p,
            None => {
                self.layout.sizes.push(ReportSize { ty, id, bits: 0 }).map_err(|_| ParseError::TooManyReports)?;
                self.layout.sizes.len() - 1
            }
        };
        let offset = self.layout.sizes[pos].bits;
        let bits = g.report_size.checked_mul(g.report_count).ok_or(ParseError::ReportTooLong)?;
        if offset + bits > u16::MAX as u32 {
            return Err(ParseError::ReportTooLong);
        }
        self.layout.sizes[pos].bits = offset + bits;

        let flags = (data & 0x1FF) as u16;
        if flags & flags::CONSTANT != 0 || bits == 0 {
            return Ok(());
        }

        let mut usages = Vec::new();
        for lu in self.usages.iter() {
            let r = match *lu {
                LocalUsage::Single(u, ext) => {
                    let u = self.resolve(u, ext);
                    UsageRange { min: u, max: u }
                }
                LocalUsage::Range(a, b, ext) => UsageRange { min: self.resolve(a, ext), max: self.resolve(b, ext) },
            };
            // Merge consecutive single usages into ranges to save space.
            match usages.last_mut() {
                Some(UsageRange { max, .. }) if *max + 1 == r.min && r.min == r.max => *max = r.min,
                _ => usages.push(r).map_err(|_| ParseError::TooManyUsages)?,
            }
        }

        // Logical Maximum is signed, but descriptors often mean an unsigned value (e.g. `25 FF` for 255).
        let mut logical_max = g.logical_max;
        if g.logical_min >= 0 && logical_max < g.logical_min {
            logical_max = g.logical_max_raw as i32;
        }

        let field = Field {
            ty,
            report_id: id,
            bit_offset: offset as u16,
            bit_size: g.report_size.min(255) as u8,
            count: g.report_count.min(u16::MAX as u32) as u16,
            flags,
            logical_min: g.logical_min,
            logical_max,
            physical_min: g.physical_min,
            physical_max: g.physical_max,
            usages,
            application: self.collections.first().copied().unwrap_or(0),
        };
        self.layout.fields.push(field).map_err(|_| ParseError::TooManyFields)
    }

    fn global(&mut self, tag: u8, v: u32, size: usize) -> Result<(), ParseError> {
        let g = &mut self.globals;
        match tag {
            0x0 => g.usage_page = v as u16,
            0x1 => g.logical_min = sign_extend(v, size),
            0x2 => {
                g.logical_max = sign_extend(v, size);
                g.logical_max_raw = v;
            }
            0x3 => g.physical_min = sign_extend(v, size),
            0x4 => g.physical_max = sign_extend(v, size),
            0x7 => g.report_size = v,
            0x8 => {
                g.report_id = v as u8;
                self.layout.uses_report_ids = true;
            }
            0x9 => g.report_count = v,
            0xA => self.stack.push(*g).map_err(|_| ParseError::PushPop)?,
            0xB => *g = self.stack.pop().ok_or(ParseError::PushPop)?,
            _ => {} // unit, unit exponent
        }
        Ok(())
    }

    fn local(&mut self, tag: u8, v: u32, size: usize) -> Result<(), ParseError> {
        // Inside a delimiter set only the first alternative is kept.
        if self.delimiter_depth > 0 && self.delimiter_seen && tag != 0xA {
            return Ok(());
        }
        let extended = size == 4;
        match tag {
            0x0 => {
                self.usages.push(LocalUsage::Single(v, extended)).map_err(|_| ParseError::TooManyUsages)?;
            }
            0x1 => self.usage_min = Some((v, extended)),
            0x2 => {
                let (min, ext_min) = self.usage_min.take().unwrap_or((0, extended));
                let ext = ext_min || extended;
                let (min, max) = if ext && !(ext_min && extended) {
                    // Mixed sizes: take the page from whichever carried one.
                    let page = if ext_min { min & 0xFFFF_0000 } else { v & 0xFFFF_0000 };
                    (page | (min & 0xFFFF), page | (v & 0xFFFF))
                } else {
                    (min, v)
                };
                self.usages.push(LocalUsage::Range(min, max, ext)).map_err(|_| ParseError::TooManyUsages)?;
            }
            0xA => {
                if v == 1 {
                    self.delimiter_depth += 1;
                } else {
                    self.delimiter_depth = self.delimiter_depth.saturating_sub(1);
                    self.delimiter_seen = true;
                }
            }
            _ => {} // designators, strings
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Report descriptors used across this crate's tests.

    /// HID 1.11 appendix B.1 boot keyboard.
    pub const BOOT_KEYBOARD: &[u8] = &[
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
        0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01,
        0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
        0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
    ];

    /// HID 1.11 appendix B.2 boot mouse.
    pub const BOOT_MOUSE: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30,
        0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06, 0xC0, 0xC0,
    ];

    /// A gaming mouse: report ID 2, 16 buttons, 16-bit X/Y, wheel, AC Pan.
    pub const GAMING_MOUSE: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x10,
        0x15, 0x00, 0x25, 0x01, 0x95, 0x10, 0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F,
        0x75, 0x10, 0x95, 0x02, 0x09, 0x30, 0x09, 0x31, 0x81, 0x06, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01,
        0x09, 0x38, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0,
    ];

    /// Mouse with 5 buttons and 12-bit X/Y packed across byte boundaries.
    pub const MOUSE_12BIT: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x03, 0x81, 0x01, 0x05, 0x01, 0x16, 0x01,
        0xF8, 0x26, 0xFF, 0x07, 0x75, 0x0C, 0x95, 0x02, 0x09, 0x30, 0x09, 0x31, 0x81, 0x06, 0x15, 0x81, 0x25, 0x7F,
        0x75, 0x08, 0x95, 0x01, 0x09, 0x38, 0x81, 0x06, 0xC0, 0xC0,
    ];

    /// QMK-style NKRO keyboard: report ID 6, modifiers + LEDs + bitmap of usages 0x00–0xF7.
    pub const NKRO_KEYBOARD: &[u8] = &[
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, 0x06, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
        0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x95, 0x05, 0x75, 0x01, 0x91, 0x02,
        0x95, 0x01, 0x75, 0x03, 0x91, 0x03, 0x05, 0x07, 0x19, 0x00, 0x29, 0xF7, 0x15, 0x00, 0x25, 0x01, 0x95, 0xF8,
        0x75, 0x01, 0x81, 0x02, 0xC0,
    ];

    /// Generic DirectInput gamepad: 14 buttons, hat with null state, four 8-bit absolute axes.
    pub const GAMEPAD: &[u8] = &[
        0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x15, 0x00, 0x25, 0x01, 0x35, 0x00, 0x45, 0x01, 0x75, 0x01, 0x95, 0x0E,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x0E, 0x81, 0x02, 0x95, 0x02, 0x81, 0x01, 0x05, 0x01, 0x25, 0x07, 0x46, 0x3B,
        0x01, 0x75, 0x04, 0x95, 0x01, 0x65, 0x14, 0x09, 0x39, 0x81, 0x42, 0x65, 0x00, 0x95, 0x01, 0x81, 0x01, 0x26,
        0xFF, 0x00, 0x46, 0xFF, 0x00, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x75, 0x08, 0x95, 0x04, 0x81,
        0x02, 0xC0,
    ];

    /// HID++-style short and long report collections (vendor page, input + output).
    pub const HIDPP: &[u8] = &[
        0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x10, 0x75, 0x08, 0x95, 0x06, 0x15, 0x00, 0x26, 0xFF, 0x00,
        0x09, 0x01, 0x81, 0x00, 0x09, 0x01, 0x91, 0x00, 0xC0, 0x06, 0x00, 0xFF, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x11,
        0x75, 0x08, 0x95, 0x13, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x09, 0x02, 0x81, 0x00, 0x09, 0x02, 0x91, 0x00, 0xC0,
    ];

    /// Consumer control (2 × 16-bit array) and system control (2-bit array with null state) with report IDs.
    pub const MEDIA_KEYS: &[u8] = &[
        0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x03, 0x75, 0x10, 0x95, 0x02, 0x15, 0x01, 0x26, 0xFF, 0x02, 0x19,
        0x01, 0x2A, 0xFF, 0x02, 0x81, 0x00, 0xC0, 0x05, 0x01, 0x09, 0x80, 0xA1, 0x01, 0x85, 0x04, 0x75, 0x02, 0x95,
        0x01, 0x15, 0x01, 0x25, 0x03, 0x09, 0x82, 0x09, 0x81, 0x09, 0x83, 0x81, 0x60, 0x75, 0x06, 0x81, 0x03, 0xC0,
    ];

    /// Mouse that reports two extra buttons on a vendor page inside its Mouse collection.
    pub const MOUSE_VENDOR_BUTTONS: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00,
        0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x06, 0x00, 0xFF, 0x09, 0x40, 0x09, 0x41, 0x95, 0x02, 0x81,
        0x02, 0x95, 0x03, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95,
        0x02, 0x81, 0x06, 0xC0, 0xC0,
    ];
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::usage::{gd, page};

    #[test]
    fn boot_keyboard_layout() {
        let l = Layout::parse(BOOT_KEYBOARD).unwrap();
        assert!(!l.uses_report_ids);
        assert_eq!(l.applications.as_slice(), &[usage(page::GENERIC_DESKTOP, gd::KEYBOARD)]);
        assert_eq!(l.report_len(ReportType::Input, 0), 8);
        assert_eq!(l.report_len(ReportType::Output, 0), 1);
        let inputs: std::vec::Vec<_> = l.fields_of(ReportType::Input, 0).collect();
        assert_eq!(inputs.len(), 2);
        assert_eq!((inputs[0].bit_offset, inputs[0].count, inputs[0].usage(0)), (0, 8, Some(usage(7, 0xE0))));
        assert_eq!(inputs[0].usage(7), Some(usage(7, 0xE7)));
        let keys = inputs[1];
        assert!(keys.is_array());
        assert_eq!((keys.bit_offset, keys.bit_size, keys.count), (16, 8, 6));
        assert_eq!(keys.array_usage(0x04), Some(usage(7, 0x04)));
        assert_eq!(keys.array_usage(0x66), None);
    }

    #[test]
    fn gaming_mouse_signed_16_bit_axes() {
        let l = Layout::parse(GAMING_MOUSE).unwrap();
        assert!(l.uses_report_ids);
        assert_eq!(l.report_len(ReportType::Input, 2), 2 + 4 + 1 + 1);
        let xy = l.fields.iter().find(|f| f.usage(0) == Some(usage(1, gd::X))).unwrap();
        assert_eq!((xy.bit_offset, xy.bit_size, xy.logical_min, xy.logical_max), (16, 16, -32767, 32767));
        let report = [0x01, 0x00, 0xFE, 0xFF, 0x05, 0x00, 0xFF, 0x00];
        assert_eq!((xy.read(&report, 0), xy.read(&report, 1)), (-2, 5));
        let pan = l.fields.iter().find(|f| f.usage(0) == Some(usage(0x0C, 0x238))).unwrap();
        assert_eq!(pan.read(&report, 0), 0);
    }

    #[test]
    fn twelve_bit_axes_cross_byte_boundaries() {
        let l = Layout::parse(MOUSE_12BIT).unwrap();
        let xy = l.fields.iter().find(|f| f.usage(0) == Some(usage(1, gd::X))).unwrap();
        assert_eq!((xy.bit_offset, xy.bit_size), (8, 12));
        let mut report = [0u8; 5];
        xy.write(&mut report, 0, -300);
        xy.write(&mut report, 1, 2047);
        assert_eq!((xy.read(&report, 0), xy.read(&report, 1)), (-300, 2047));
    }

    #[test]
    fn unsigned_logical_max_fix_and_null_state() {
        let l = Layout::parse(GAMEPAD).unwrap();
        let axes = l.fields.iter().find(|f| f.usage(0) == Some(usage(1, gd::X))).unwrap();
        assert_eq!((axes.logical_min, axes.logical_max), (0, 255));
        assert_eq!(axes.usage(3), Some(usage(1, gd::RZ)));
        let hat = l.fields.iter().find(|f| f.usage(0) == Some(usage(1, gd::HAT_SWITCH))).unwrap();
        assert!(hat.has_null_state());
        assert_eq!(l.report_len(ReportType::Input, 0), 2 + 1 + 4);
    }

    #[test]
    fn vendor_collections_and_report_ids() {
        let l = Layout::parse(HIDPP).unwrap();
        assert_eq!(l.applications.len(), 2);
        assert_eq!(l.report_len(ReportType::Input, 0x10), 6);
        assert_eq!(l.report_len(ReportType::Output, 0x11), 19);
        assert_eq!(l.report_ids(ReportType::Input).collect::<std::vec::Vec<_>>(), [0x10, 0x11]);
        assert_eq!(l.split_report(&[0x11, 1, 2]), Some((0x11, &[1u8, 2][..])));
    }

    #[test]
    fn rejects_broken_descriptors() {
        assert_eq!(Layout::parse(&[0x05]), Err(ParseError::Truncated));
        assert_eq!(Layout::parse(&[0xA1, 0x01]), Err(ParseError::UnbalancedCollection));
        assert_eq!(Layout::parse(&[0xC0]), Err(ParseError::UnbalancedCollection));
        assert_eq!(Layout::parse(&[0xB4]), Err(ParseError::PushPop));
    }

    #[test]
    fn bit_helpers() {
        let mut b = [0u8; 4];
        write_bits(&mut b, 3, 10, 0x2AB);
        assert_eq!(read_bits(&b, 3, 10), 0x2AB);
        assert_eq!(read_bits(&b, 0, 3), 0);
        assert_eq!(read_bits(&b, 30, 8), 0, "out of range reads are 0");
    }
}
