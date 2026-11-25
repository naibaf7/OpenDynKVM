//! The HCI events of Bluetooth Classic that the module's host reads (Core 5.4, Vol 4, Part E, §7.7), and where each of
//! the controller's events goes ([`route_event`]): the LE stack and the Classic host share one controller, and the LE
//! stack must not see Classic's connections (a Disconnection Complete of a handle it doesn't know is a warning to it,
//! an ACL packet of one an error).

/// A BR/EDR address as HCI carries it: least significant byte first.
pub type BdAddr = [u8; 6];

/// Event codes.
pub mod code {
    pub const INQUIRY_COMPLETE: u8 = 0x01;
    pub const INQUIRY_RESULT: u8 = 0x02;
    pub const CONNECTION_COMPLETE: u8 = 0x03;
    pub const CONNECTION_REQUEST: u8 = 0x04;
    pub const DISCONNECTION_COMPLETE: u8 = 0x05;
    pub const AUTHENTICATION_COMPLETE: u8 = 0x06;
    pub const REMOTE_NAME_REQUEST_COMPLETE: u8 = 0x07;
    pub const ENCRYPTION_CHANGE: u8 = 0x08;
    pub const READ_REMOTE_VERSION_COMPLETE: u8 = 0x0C;
    pub const NUMBER_OF_COMPLETED_PACKETS: u8 = 0x13;
    pub const PIN_CODE_REQUEST: u8 = 0x16;
    pub const LINK_KEY_REQUEST: u8 = 0x17;
    pub const LINK_KEY_NOTIFICATION: u8 = 0x18;
    pub const INQUIRY_RESULT_WITH_RSSI: u8 = 0x22;
    pub const EXTENDED_INQUIRY_RESULT: u8 = 0x2F;
    pub const ENCRYPTION_KEY_REFRESH_COMPLETE: u8 = 0x30;
    pub const IO_CAPABILITY_REQUEST: u8 = 0x31;
    pub const IO_CAPABILITY_RESPONSE: u8 = 0x32;
    pub const USER_CONFIRMATION_REQUEST: u8 = 0x33;
    pub const USER_PASSKEY_REQUEST: u8 = 0x34;
    pub const SIMPLE_PAIRING_COMPLETE: u8 = 0x36;
    pub const ENCRYPTION_CHANGE_V2: u8 = 0x59;
}

/// Events only BR/EDR sends (LE's come as LE Meta events, or name an LE connection's handle).
pub fn classic_only(c: u8) -> bool {
    matches!(
        c,
        0x01..=0x04
            | 0x06
            | 0x07
            | 0x0B
            | 0x12
            | 0x14
            | 0x16..=0x18
            | 0x1B
            | 0x1D
            | 0x20
            | 0x22
            | 0x23
            | 0x2C
            | 0x2F
            | 0x31..=0x36
            | 0x38
            | 0x3B..=0x3D
    )
}

/// The connection handle an event shared by LE and BR/EDR names (status byte first, then the handle).
pub fn shared_handle(c: u8, data: &[u8]) -> Option<u16> {
    match c {
        code::DISCONNECTION_COMPLETE
        | code::ENCRYPTION_CHANGE
        | code::ENCRYPTION_CHANGE_V2
        | code::ENCRYPTION_KEY_REFRESH_COMPLETE
        | code::READ_REMOTE_VERSION_COMPLETE => data.get(1..3).map(|h| u16::from_le_bytes([h[0], h[1]]) & 0x0FFF),
        _ => None,
    }
}

/// Where an event goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Le,
    Classic,
    /// Number of Completed Packets: each stack takes its own handles' counts (the LE stack ignores others).
    Both,
}

/// Where the event `c` with `data` goes; `classic(handle)`: whether a handle is one of Classic's connections.
pub fn route_event(c: u8, data: &[u8], classic: impl Fn(u16) -> bool) -> Route {
    if classic_only(c) {
        return Route::Classic;
    }
    if c == code::NUMBER_OF_COMPLETED_PACKETS {
        return Route::Both;
    }
    match shared_handle(c, data) {
        Some(h) if classic(h) => Route::Classic,
        _ => Route::Le,
    }
}

/// The (handle, packets) pairs of a Number of Completed Packets event.
pub fn completed(data: &[u8]) -> impl Iterator<Item = (u16, u16)> + '_ {
    let n = data.first().copied().unwrap_or(0) as usize;
    let entries = data.get(1..1 + 4 * n).unwrap_or(&[]);
    entries
        .as_chunks::<4>()
        .0
        .iter()
        .map(|e| (u16::from_le_bytes([e[0], e[1]]) & 0x0FFF, u16::from_le_bytes([e[2], e[3]])))
}

/// The Classic events the host acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event<'a> {
    InquiryComplete,
    /// A device answered an inquiry (any of the three result events; the extended one may carry its name).
    Found {
        addr: BdAddr,
        class: u32,
        rssi: i8,
        name: Option<&'a str>,
        scan_mode: u8,
        clock_offset: u16,
    },
    ConnectionRequest {
        addr: BdAddr,
        class: u32,
        link_type: u8,
    },
    ConnectionComplete {
        status: u8,
        handle: u16,
        addr: BdAddr,
        link_type: u8,
    },
    Disconnected {
        handle: u16,
        reason: u8,
    },
    AuthenticationComplete {
        status: u8,
        handle: u16,
    },
    RemoteName {
        status: u8,
        addr: BdAddr,
        name: &'a str,
    },
    EncryptionChange {
        status: u8,
        handle: u16,
        on: bool,
    },
    PinCodeRequest {
        addr: BdAddr,
    },
    LinkKeyRequest {
        addr: BdAddr,
    },
    LinkKeyNotification {
        addr: BdAddr,
        key: [u8; 16],
        kind: u8,
    },
    IoCapabilityRequest {
        addr: BdAddr,
    },
    IoCapabilityResponse {
        addr: BdAddr,
        io: u8,
        auth: u8,
    },
    UserConfirmationRequest {
        addr: BdAddr,
        value: u32,
    },
    UserPasskeyRequest {
        addr: BdAddr,
    },
    SimplePairingComplete {
        status: u8,
        addr: BdAddr,
    },
}

fn addr(d: &[u8], at: usize) -> Option<BdAddr> {
    d.get(at..at + 6)?.try_into().ok()
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn class(d: &[u8], at: usize) -> Option<u32> {
    let c = d.get(at..at + 3)?;
    Some(u32::from_le_bytes([c[0], c[1], c[2], 0]))
}

/// A name field: UTF-8 up to the first NUL.
fn name(d: &[u8]) -> &str {
    let end = d.iter().position(|b| *b == 0).unwrap_or(d.len());
    match core::str::from_utf8(&d[..end]) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&d[..e.valid_up_to()]).unwrap_or(""),
    }
}

/// The local name in extended inquiry response data (complete, else shortened).
pub fn eir_name(eir: &[u8]) -> Option<&str> {
    let mut short = None;
    let mut i = 0;
    while i < eir.len() {
        let len = eir[i] as usize;
        if len == 0 || i + 1 + len > eir.len() {
            break;
        }
        let (ty, v) = (eir[i + 1], &eir[i + 2..i + 1 + len]);
        match ty {
            0x09 => return Some(name(v)),
            0x08 => short = Some(name(v)),
            _ => {}
        }
        i += 1 + len;
    }
    short
}

/// The event `c` with `data`, if it is one the host acts on.
pub fn parse(c: u8, d: &[u8]) -> Option<Event<'_>> {
    Some(match c {
        code::INQUIRY_COMPLETE => Event::InquiryComplete,
        // One response each (what controllers send); the fields of several would come one array after another.
        code::INQUIRY_RESULT if d.first() == Some(&1) => Event::Found {
            addr: addr(d, 1)?,
            scan_mode: *d.get(7)?,
            class: class(d, 10)?,
            clock_offset: u16_at(d, 13)?,
            rssi: 0,
            name: None,
        },
        code::INQUIRY_RESULT_WITH_RSSI if d.first() == Some(&1) => Event::Found {
            addr: addr(d, 1)?,
            scan_mode: *d.get(7)?,
            class: class(d, 9)?,
            clock_offset: u16_at(d, 12)?,
            rssi: *d.get(14)? as i8,
            name: None,
        },
        code::EXTENDED_INQUIRY_RESULT => Event::Found {
            addr: addr(d, 1)?,
            scan_mode: *d.get(7)?,
            class: class(d, 9)?,
            clock_offset: u16_at(d, 12)?,
            rssi: *d.get(14)? as i8,
            name: eir_name(d.get(15..).unwrap_or(&[])),
        },
        code::CONNECTION_REQUEST => {
            Event::ConnectionRequest { addr: addr(d, 0)?, class: class(d, 6)?, link_type: *d.get(9)? }
        }
        code::CONNECTION_COMPLETE => Event::ConnectionComplete {
            status: *d.first()?,
            handle: u16_at(d, 1)? & 0x0FFF,
            addr: addr(d, 3)?,
            link_type: *d.get(9)?,
        },
        code::DISCONNECTION_COMPLETE => Event::Disconnected { handle: u16_at(d, 1)? & 0x0FFF, reason: *d.get(3)? },
        code::AUTHENTICATION_COMPLETE => {
            Event::AuthenticationComplete { status: *d.first()?, handle: u16_at(d, 1)? & 0x0FFF }
        }
        code::REMOTE_NAME_REQUEST_COMPLETE => {
            Event::RemoteName { status: *d.first()?, addr: addr(d, 1)?, name: name(d.get(7..).unwrap_or(&[])) }
        }
        code::ENCRYPTION_CHANGE | code::ENCRYPTION_CHANGE_V2 => {
            Event::EncryptionChange { status: *d.first()?, handle: u16_at(d, 1)? & 0x0FFF, on: *d.get(3)? != 0 }
        }
        code::PIN_CODE_REQUEST => Event::PinCodeRequest { addr: addr(d, 0)? },
        code::LINK_KEY_REQUEST => Event::LinkKeyRequest { addr: addr(d, 0)? },
        code::LINK_KEY_NOTIFICATION => {
            Event::LinkKeyNotification { addr: addr(d, 0)?, key: d.get(6..22)?.try_into().ok()?, kind: *d.get(22)? }
        }
        code::IO_CAPABILITY_REQUEST => Event::IoCapabilityRequest { addr: addr(d, 0)? },
        code::IO_CAPABILITY_RESPONSE => {
            Event::IoCapabilityResponse { addr: addr(d, 0)?, io: *d.get(6)?, auth: *d.get(8)? }
        }
        code::USER_CONFIRMATION_REQUEST => Event::UserConfirmationRequest {
            addr: addr(d, 0)?,
            value: u32::from_le_bytes(d.get(6..10)?.try_into().ok()?),
        },
        code::USER_PASSKEY_REQUEST => Event::UserPasskeyRequest { addr: addr(d, 0)? },
        code::SIMPLE_PAIRING_COMPLETE => Event::SimplePairingComplete { status: *d.first()?, addr: addr(d, 1)? },
        _ => return None,
    })
}

/// A Class of Device's major class and minor bits as the GAP appearance the RP2354B shows devices by
/// (`kvm_proto::link::bt::appearance`): peripherals only (major class 5); 0 for anything else.
pub fn appearance(class: u32) -> u16 {
    let major = (class >> 8) & 0x1F;
    let minor = (class >> 2) & 0x3F;
    if major != 5 {
        return 0;
    }
    match (minor >> 4, minor & 0x0F) {
        (_, 1) => 0x03C3, // joystick
        (_, 2) => 0x03C4, // gamepad
        (1, _) => 0x03C1, // keyboard
        (2, _) => 0x03C2, // mouse
        (3, _) => 0x03C1, // keyboard with a pointing device
        _ => 0x03C0,      // generic HID
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: BdAddr = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];

    #[test]
    fn classic_events_and_handles_go_to_the_classic_host() {
        let classic = |h: u16| h == 0x0080;
        assert_eq!(route_event(code::CONNECTION_REQUEST, &[], classic), Route::Classic);
        assert_eq!(route_event(code::LINK_KEY_REQUEST, &[], classic), Route::Classic);
        assert_eq!(route_event(0x3E, &[0x01], classic), Route::Le, "LE Meta");
        assert_eq!(route_event(code::DISCONNECTION_COMPLETE, &[0, 0x80, 0x00, 0x13], classic), Route::Classic);
        assert_eq!(route_event(code::DISCONNECTION_COMPLETE, &[0, 0x01, 0x00, 0x13], classic), Route::Le);
        assert_eq!(route_event(code::ENCRYPTION_CHANGE, &[0, 0x80, 0x20, 1], classic), Route::Classic, "flag bits");
        assert_eq!(route_event(code::NUMBER_OF_COMPLETED_PACKETS, &[], classic), Route::Both);
        let nocp = [2, 0x80, 0x00, 3, 0, 0x01, 0x00, 1, 0];
        assert_eq!(completed(&nocp).collect::<std::vec::Vec<_>>(), [(0x80, 3), (0x01, 1)]);
    }

    #[test]
    fn inquiry_results_read_with_their_name() {
        // Extended Inquiry Result: one response, the pad's class (peripheral, gamepad) and its name.
        let mut d = std::vec![1];
        d.extend_from_slice(&PAD);
        d.extend_from_slice(&[1, 0, 0x08, 0x25, 0x00, 0x34, 0x12, 0xC4]);
        d.extend_from_slice(&[6, 0x09, b'P', b'a', b'd', b' ', b'5', 0]);
        let Some(Event::Found { addr, class, rssi, name, scan_mode, clock_offset }) =
            parse(code::EXTENDED_INQUIRY_RESULT, &d)
        else {
            panic!()
        };
        assert_eq!((addr, class, rssi, name, scan_mode, clock_offset), (PAD, 0x002508, -60, Some("Pad 5"), 1, 0x1234));
        assert_eq!(appearance(class), 0x03C4);
        // With RSSI, no name.
        let mut d = std::vec![1];
        d.extend_from_slice(&PAD);
        d.extend_from_slice(&[1, 0, 0x40, 0x05, 0x00, 0, 0, 0xB0]);
        let Some(Event::Found { class, rssi, name, .. }) = parse(code::INQUIRY_RESULT_WITH_RSSI, &d) else { panic!() };
        assert_eq!((class, rssi, name, appearance(class)), (0x000540, -80, None, 0x03C1));
    }

    #[test]
    fn pairing_events_read() {
        let mut d = PAD.to_vec();
        d.extend_from_slice(&[7; 16]);
        d.push(5);
        assert_eq!(
            parse(code::LINK_KEY_NOTIFICATION, &d),
            Some(Event::LinkKeyNotification { addr: PAD, key: [7; 16], kind: 5 })
        );
        let mut d = PAD.to_vec();
        d.extend_from_slice(&123456u32.to_le_bytes());
        assert_eq!(
            parse(code::USER_CONFIRMATION_REQUEST, &d),
            Some(Event::UserConfirmationRequest { addr: PAD, value: 123456 })
        );
        let mut d = std::vec![0, 0x80, 0x00];
        d.extend_from_slice(&PAD);
        d.extend_from_slice(&[1, 0]);
        assert_eq!(
            parse(code::CONNECTION_COMPLETE, &d),
            Some(Event::ConnectionComplete { status: 0, handle: 0x80, addr: PAD, link_type: 1 })
        );
        let mut d = std::vec![0];
        d.extend_from_slice(&PAD);
        d.extend_from_slice(b"Wireless pad\0\0\0");
        assert_eq!(
            parse(code::REMOTE_NAME_REQUEST_COMPLETE, &d),
            Some(Event::RemoteName { status: 0, addr: PAD, name: "Wireless pad" })
        );
        assert_eq!(parse(0x3E, &[1, 2, 3]), None);
    }
}
