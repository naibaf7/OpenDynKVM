//! The DDC/CI and MCCS pieces both backends share: packets and checksums (VESA DDC/CI 1.1), the capability string, the
//! EDID identity and the names of the input sources (MCCS 2.2, VCP code 0x60).

/// VCP code of the input source.
pub const INPUT_SOURCE: u8 = 0x60;
/// The display's DDC/CI address on its I²C bus (0x6E written as 8-bit).
pub const DISPLAY_ADDR: u16 = 0x37;
const HOST: u8 = 0x51;
const DEST_WRITE: u8 = 0x6E;
const DEST_READ: u8 = 0x50;

/// Standard MCCS names of the input sources; displays may use other values too (some makers: 0x90 and up).
pub fn input_name(v: u16) -> String {
    let name = match v {
        0x01 => "VGA 1",
        0x02 => "VGA 2",
        0x03 => "DVI 1",
        0x04 => "DVI 2",
        0x05 => "Composite 1",
        0x06 => "Composite 2",
        0x07 => "S-Video 1",
        0x08 => "S-Video 2",
        0x09 => "Tuner 1",
        0x0A => "Tuner 2",
        0x0B => "Tuner 3",
        0x0C => "Component 1",
        0x0D => "Component 2",
        0x0E => "Component 3",
        0x0F => "DisplayPort 1",
        0x10 => "DisplayPort 2",
        0x11 => "HDMI 1",
        0x12 => "HDMI 2",
        0x1B => "USB-C",
        _ => return crate::fl!("input-unknown", value = format!("0x{v:02X}")),
    };
    format!("{name} (0x{v:02X})")
}

fn xor(init: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(init, |a, b| a ^ b)
}

/// A request as written to the display: source address, length, payload, checksum.
fn request(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![HOST, 0x80 | payload.len() as u8];
    out.extend_from_slice(payload);
    out.push(xor(DEST_WRITE, &out));
    out
}

pub fn get_vcp_request(code: u8) -> Vec<u8> {
    request(&[0x01, code])
}

pub fn set_vcp_request(code: u8, value: u16) -> Vec<u8> {
    let [hi, lo] = value.to_be_bytes();
    request(&[0x03, code, hi, lo])
}

pub fn capabilities_request(offset: u16) -> Vec<u8> {
    let [hi, lo] = offset.to_be_bytes();
    request(&[0xF3, hi, lo])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyError {
    /// No reply, or the "null message" a busy display sends.
    Empty,
    BadChecksum,
    Malformed,
    /// The display says it does not support this VCP code.
    Unsupported,
}

impl std::fmt::Display for ReplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&match self {
            ReplyError::Empty => crate::fl!("ddc-no-answer"),
            ReplyError::BadChecksum => crate::fl!("ddc-garbled"),
            ReplyError::Malformed => crate::fl!("ddc-unexpected"),
            ReplyError::Unsupported => crate::fl!("ddc-unsupported-code"),
        })
    }
}

/// The payload of a reply read from the display, checked; `buf` starts with the display's address 0x6E.
fn reply(buf: &[u8]) -> Result<&[u8], ReplyError> {
    if buf.len() < 3 || buf[0] != DEST_WRITE || buf[1] & 0x80 == 0 {
        return Err(ReplyError::Malformed);
    }
    let len = (buf[1] & 0x7F) as usize;
    if len == 0 {
        return Err(ReplyError::Empty);
    }
    if buf.len() < 3 + len {
        return Err(ReplyError::Malformed);
    }
    if xor(DEST_READ, &buf[..2 + len]) != buf[2 + len] {
        return Err(ReplyError::BadChecksum);
    }
    Ok(&buf[2..2 + len])
}

/// A "Get VCP Feature" reply: (current value, maximum).
pub fn parse_vcp_reply(buf: &[u8], code: u8) -> Result<(u16, u16), ReplyError> {
    let p = reply(buf)?;
    if p.len() < 8 || p[0] != 0x02 || p[2] != code {
        return Err(ReplyError::Malformed);
    }
    if p[1] != 0 {
        return Err(ReplyError::Unsupported);
    }
    Ok((u16::from_be_bytes([p[6], p[7]]), u16::from_be_bytes([p[4], p[5]])))
}

/// A capabilities reply fragment: its offset and data (empty: the end).
pub fn parse_capabilities_reply(buf: &[u8]) -> Result<(u16, &[u8]), ReplyError> {
    let p = reply(buf)?;
    if p.len() < 3 || p[0] != 0xE3 {
        return Err(ReplyError::Malformed);
    }
    Ok((u16::from_be_bytes([p[1], p[2]]), &p[3..]))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub model: Option<String>,
    /// The values VCP 0x60 accepts, as the display lists them.
    pub inputs: Vec<u16>,
}

/// The parts of a capability string used here, e.g. `(prot(monitor)type(LCD)model(U2720Q)…vcp(02 10 60(0F 11 1B) …))`.
pub fn parse_capabilities(s: &str) -> Capabilities {
    let mut caps = Capabilities::default();
    if let Some(i) = s.find("model(") {
        let rest = &s[i + 6..];
        caps.model = rest.find(')').map(|j| rest[..j].trim().to_string()).filter(|m| !m.is_empty());
    }
    let Some(i) = s.find("vcp(") else { return caps };
    let b = &s.as_bytes()[i + 4..];
    let mut k = 0;
    let hex = |b: &[u8], k: &mut usize| {
        let start = *k;
        while *k < b.len() && b[*k].is_ascii_hexdigit() {
            *k += 1;
        }
        u16::from_str_radix(std::str::from_utf8(&b[start..*k]).unwrap_or(""), 16).ok()
    };
    while k < b.len() && b[k] != b')' {
        if b[k].is_ascii_whitespace() {
            k += 1;
            continue;
        }
        let Some(code) = hex(b, &mut k) else { break };
        while k < b.len() && b[k] == b' ' {
            k += 1;
        }
        if k < b.len() && b[k] == b'(' {
            k += 1;
            let mut values = Vec::new();
            while k < b.len() && b[k] != b')' {
                if b[k].is_ascii_hexdigit() {
                    if let Some(v) = hex(b, &mut k) {
                        values.push(v);
                    }
                } else {
                    k += 1;
                }
            }
            k += 1;
            if code == INPUT_SOURCE as u16 {
                caps.inputs = values;
            }
        }
    }
    caps
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdidInfo {
    /// The three-letter PNP manufacturer ID, e.g. "ODK".
    pub manufacturer: String,
    pub product: u16,
    pub serial: u32,
    /// The monitor name descriptor (0xFC).
    pub name: Option<String>,
    /// The serial-number descriptor (0xFF).
    pub serial_text: Option<String>,
}

impl EdidInfo {
    /// A key that stays the same for this monitor on any connector.
    pub fn id(&self) -> String {
        let serial = match (&self.serial_text, self.serial) {
            (Some(t), _) => t.clone(),
            (None, 0) => String::new(),
            (None, n) => n.to_string(),
        };
        format!("{}-{:04X}-{serial}", self.manufacturer, self.product)
    }

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("{} {:04X}", self.manufacturer, self.product))
    }
}

/// Identity from the EDID base block.
pub fn parse_edid(e: &[u8]) -> Option<EdidInfo> {
    if e.len() < 128 || e[..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }
    let m = u16::from_be_bytes([e[8], e[9]]);
    let letter = |shift: u16| (b'A' - 1 + ((m >> shift) & 0x1F) as u8) as char;
    let manufacturer: String = [letter(10), letter(5), letter(0)].iter().collect();
    let text = |d: &[u8]| {
        let t: String = d.iter().take_while(|c| **c != 0x0A).map(|c| *c as char).collect();
        Some(t.trim().to_string()).filter(|t| !t.is_empty())
    };
    let (mut name, mut serial_text) = (None, None);
    for d in e[54..126].chunks(18) {
        if d[0] == 0 && d[1] == 0 {
            match d[3] {
                0xFC => name = text(&d[5..]),
                0xFF => serial_text = text(&d[5..]),
                _ => {}
            }
        }
    }
    Some(EdidInfo {
        manufacturer,
        product: u16::from_le_bytes([e[10], e[11]]),
        serial: u32::from_le_bytes([e[12], e[13], e[14], e[15]]),
        name,
        serial_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_the_ddc_ci_checksum() {
        // ddcutil's well-known "get VCP 0x10" (brightness) packet.
        assert_eq!(get_vcp_request(0x10), [0x51, 0x82, 0x01, 0x10, 0xAC]);
        let set = set_vcp_request(INPUT_SOURCE, 0x0F);
        assert_eq!(&set[..6], &[0x51, 0x84, 0x03, 0x60, 0x00, 0x0F]);
        assert_eq!(set[6], 0x6E ^ 0x51 ^ 0x84 ^ 0x03 ^ 0x60 ^ 0x0F);
        assert_eq!(&capabilities_request(0x0020)[..5], &[0x51, 0x83, 0xF3, 0x00, 0x20]);
    }

    fn with_checksum(mut b: Vec<u8>) -> Vec<u8> {
        b.push(xor(DEST_READ, &b));
        b
    }

    #[test]
    fn vcp_and_capability_replies() {
        let r = with_checksum(vec![0x6E, 0x88, 0x02, 0x00, 0x60, 0x00, 0x00, 0x12, 0x00, 0x0F]);
        assert_eq!(parse_vcp_reply(&r, INPUT_SOURCE), Ok((0x0F, 0x12)));
        let mut bad = r.clone();
        bad[9] ^= 1;
        assert_eq!(parse_vcp_reply(&bad, INPUT_SOURCE), Err(ReplyError::BadChecksum));
        let unsupported = with_checksum(vec![0x6E, 0x88, 0x02, 0x01, 0x60, 0, 0, 0, 0, 0]);
        assert_eq!(parse_vcp_reply(&unsupported, INPUT_SOURCE), Err(ReplyError::Unsupported));
        assert_eq!(parse_vcp_reply(&with_checksum(vec![0x6E, 0x80]), 0x60), Err(ReplyError::Empty));
        let frag = with_checksum(vec![0x6E, 0x86, 0xE3, 0x00, 0x20, b'v', b'c', b'p']);
        assert_eq!(parse_capabilities_reply(&frag), Ok((0x20, &b"vcp"[..])));
    }

    #[test]
    fn capability_string() {
        let s = "(prot(monitor)type(LCD)model(GENERIC 27Q)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 10 12 14(01 04 05 \
                 06 08 09 0B 0C) 16 18 1A 52 60( 0F 11 12 1B) AA(01 02 04) AC AE B2 B6 C6 C8 C9 D6(01 04 05) DC(00 03 05) \
                 DF E0 E1 E2(00 1D 01 02 04 0E 12 14 23 24 27) F0(00 08 0C 0F 10 11 31 32 34 35) F1 F2 FD)mswhql(1)\
                 asset_eep(40)mccs_ver(2.1))";
        let c = parse_capabilities(s);
        assert_eq!(c.model.as_deref(), Some("GENERIC 27Q"));
        assert_eq!(c.inputs, [0x0F, 0x11, 0x12, 0x1B]);
        assert_eq!(parse_capabilities("(vcp(10 12))").inputs, Vec::<u16>::new());
        assert_eq!(input_name(0x11), "HDMI 1 (0x11)");
        assert_eq!(input_name(0xD0), "Input 0xD0");
    }

    #[test]
    fn edid_identity() {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00]);
        // "ODK" = 15, 4, 11 in 5-bit letters.
        let m: u16 = (15 << 10) | (4 << 5) | 11;
        e[8..10].copy_from_slice(&m.to_be_bytes());
        e[10..12].copy_from_slice(&0xA0F3u16.to_le_bytes());
        e[12..16].copy_from_slice(&0x0102_0304u32.to_le_bytes());
        e[72..77].copy_from_slice(&[0, 0, 0, 0xFC, 0]);
        e[77..90].copy_from_slice(b"GENERIC 27Q\n ");
        let info = parse_edid(&e).unwrap();
        assert_eq!((info.manufacturer.as_str(), info.product, info.serial), ("ODK", 0xA0F3, 0x0102_0304));
        assert_eq!(info.display_name(), "GENERIC 27Q");
        assert_eq!(info.id(), "ODK-A0F3-16909060");
        assert_eq!(parse_edid(&[0; 128]), None);
    }
}
