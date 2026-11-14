//! RP2354B ↔ Bluetooth LE coprocessor (ESP32-C3-MINI-1, boards 0.16.0 on) link messages (software.md §4.13).
//!
//! The same 72-byte [`Frame`](super::Frame) and go-back-N [`Arq`](super::Arq) as the AT32F405 links, carried over
//! UART0 at [`BAUD`] instead of SPI (two wires: the module's BOOT strap shares the RP2354B's TX line). The RP2354B is the master: it sends one frame and the module answers with one,
//! which is the SPI link's lockstep exchange, so the ARQ is unchanged. A frame whose CRC fails is dropped like a
//! corrupted SPI frame, and both sides resynchronise on the next [`MAGIC`](super::frame::MAGIC) whose frame checks.
//!
//! The coprocessor holds the Bluetooth hosts (PCs, tablets, phones) in up to [`MAX_HOSTS`] slots, numbered 1–4: a
//! bond each, and a connection while the host is in range. The RP2354B decides which one gets input
//! ([`BtMsg::Select`]) and sends it keyboard, mouse and media reports in the fixed Bluetooth report format below
//! ([`report`]); the coprocessor knows nothing of the KVM's ports, modes or keymap.

use super::frame::MAX_MSG;
use crate::wire::{Reader, WireError, Writer, wire_enum};

/// UART0 speed (8N1). 72 bytes take 0.24 ms, far below a Bluetooth connection interval (≥ 7.5 ms).
pub const BAUD: u32 = 3_000_000;
/// Host slots on the coprocessor (one bond each).
pub const MAX_HOSTS: usize = 4;
/// `Hello.role` of the coprocessor.
pub const ROLE: u8 = 3;
/// Largest report body after `host` and `id`.
pub const REPORT_MAX: usize = MAX_MSG - 3;

pub mod ty {
    pub const HELLO: u8 = 0x01;
    pub const INPUT: u8 = 0x60;
    pub const SELECT: u8 = 0x61;
    pub const PAIR: u8 = 0x62;
    pub const FORGET: u8 = 0x63;
    pub const PASSKEY: u8 = 0x64;
    pub const POLICY: u8 = 0x65;
    pub const REQUEST_STATE: u8 = 0x66;
    pub const ENTER_ROM: u8 = 0x67;
    pub const OUTPUT: u8 = 0x70;
    pub const HOST: u8 = 0x71;
    pub const EVENT: u8 = 0x72;
}

/// The reports a Bluetooth host gets: one HID service whose report map ([`report::MAP`]) every host accepts
/// (HID over GATT with a boot-style report map is built into desktop and mobile systems alike), so no host needs a driver and nothing depends on the
/// front-port devices. Mirroring a device's own descriptor over Bluetooth is not done (design.md §13).
pub mod report {
    /// Report IDs.
    pub const KEYBOARD: u8 = 1;
    pub const MOUSE: u8 = 2;
    pub const CONSUMER: u8 = 3;
    /// The absolute pointer of screen hop (protocol 9), for a host whose pointer is `BtPointer::Absolute`.
    pub const POINTER: u8 = 4;
    /// Keyboard: modifiers, reserved, 6 key usages (the boot layout; more than 6 keys → 6 × ErrorRollOver).
    pub const KEYBOARD_LEN: usize = 8;
    /// Mouse: buttons (16 bits), X, Y (16 bits, signed), wheel, pan (8 bits, signed).
    pub const MOUSE_LEN: usize = 8;
    /// Consumer: one 16-bit usage (0 = none).
    pub const CONSUMER_LEN: usize = 2;
    /// Absolute pointer: buttons (16 bits), X, Y (0–32767), wheel, pan (8 bits, signed).
    pub const POINTER_LEN: usize = 8;
    /// Keyboard output report (LEDs: Num, Caps, Scroll, Compose, Kana).
    pub const LEDS_LEN: usize = 1;

    /// The HID report map (descriptor) of the Bluetooth keyboard and mouse, 229 bytes. The absolute pointer is a
    /// top-level collection of its own, of usage Pointer rather than Mouse, as on the PCs' USB interface: a host makes
    /// it a device of its own, and one that takes no absolute pointer leaves it alone.
    #[rustfmt::skip]
    pub const MAP: &[u8] = &[
        // Keyboard, report 1
        0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, KEYBOARD,
        0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
        0x75, 0x08, 0x95, 0x01, 0x81, 0x01,
        0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x75, 0x01, 0x95, 0x05, 0x91, 0x02, 0x75, 0x03, 0x95, 0x01, 0x91, 0x01,
        0x05, 0x07, 0x19, 0x00, 0x2A, 0xFF, 0x00, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x06, 0x81, 0x00,
        0xC0,
        // Mouse, report 2: 16 buttons, X/Y 16 bit, wheel, AC pan
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, MOUSE, 0x09, 0x01, 0xA1, 0x00,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x10, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x10, 0x81, 0x02,
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x06,
        0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
        0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06,
        0xC0, 0xC0,
        // Consumer control, report 3
        0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, CONSUMER,
        0x15, 0x00, 0x26, 0xFF, 0x03, 0x19, 0x00, 0x2A, 0xFF, 0x03, 0x75, 0x10, 0x95, 0x01, 0x81, 0x00,
        0xC0,
        // Absolute pointer, report 4: 16 buttons, X/Y 0–32767, wheel, AC pan
        0x05, 0x01, 0x09, 0x01, 0xA1, 0x01, 0x85, POINTER, 0x09, 0x01, 0xA1, 0x00,
        0x05, 0x09, 0x19, 0x01, 0x29, 0x10, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x10, 0x81, 0x02,
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x00, 0x26, 0xFF, 0x7F, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02,
        0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06,
        0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06,
        0xC0, 0xC0,
    ];

    /// The input report IDs and their lengths, in the order of the report map.
    pub const INPUTS: [(u8, usize); 4] =
        [(KEYBOARD, KEYBOARD_LEN), (MOUSE, MOUSE_LEN), (CONSUMER, CONSUMER_LEN), (POINTER, POINTER_LEN)];

    /// Length of input report `id`, if it is one.
    pub fn input_len(id: u8) -> Option<usize> {
        INPUTS.iter().find(|(i, _)| *i == id).map(|(_, n)| *n)
    }
}

wire_enum! {
    /// What the coprocessor does with a bonded host while another target is active.
    pub enum Policy {
        /// Keep the link: switching to it takes effect at its next connection event (PCs).
        StayConnected = 0,
        /// Disconnect while not selected, reconnect when selected: many phones and tablets hide their on-screen
        /// keyboard while a hardware keyboard is connected (design.md §13).
        ConnectWhenSelected = 1,
    }
}

impl Policy {
    pub const fn name(self) -> &'static str {
        match self {
            Policy::StayConnected => "stay",
            Policy::ConnectWhenSelected => "on-select",
        }
    }

    pub fn from_name(s: &str) -> Option<Policy> {
        match s {
            "stay" | "stay-connected" => Some(Policy::StayConnected),
            "on-select" | "connect-when-selected" => Some(Policy::ConnectWhenSelected),
            _ => None,
        }
    }
}

wire_enum! {
    pub enum EventKind {
        /// Pairing is open (discoverable, bondable), `value` = seconds left.
        PairingOpen = 1,
        /// Pairing closed without a new bond (timed out or stopped).
        PairingClosed = 2,
        /// The host in `host` asks for the passkey it shows (KeyboardOnly): type it on the KVM's keyboard.
        PasskeyRequest = 3,
        /// The host in `host` is bonded now.
        Paired = 4,
        /// Pairing with a host failed, `value` = SMP reason code.
        PairFailed = 5,
        /// A bonded host lost its bond (it forgot the KVM); the slot is free again.
        BondLost = 6,
    }
}

/// `HostState.flags` bits.
pub mod host_flags {
    /// A bond is stored in this slot.
    pub const BONDED: u8 = 1 << 0;
    /// Connected now.
    pub const CONNECTED: u8 = 1 << 1;
    /// The link is encrypted with the bond's key (only then are reports sent).
    pub const ENCRYPTED: u8 = 1 << 2;
    /// The host enabled notifications of the keyboard input report.
    pub const SUBSCRIBED: u8 = 1 << 3;
}

/// The state of one host slot (coprocessor → RP2354B, on change and on [`BtMsg::RequestState`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct HostState {
    /// 1–[`MAX_HOSTS`].
    pub host: u8,
    /// [`host_flags`].
    pub flags: u8,
    /// Connection interval in units of 1.25 ms (0 = not connected).
    pub interval: u16,
    /// Peripheral latency (connection events the module may skip).
    pub latency: u16,
    /// The host's identity address, type byte (0 public, 1 random) then 6 bytes, little-endian as on air.
    pub addr: [u8; 7],
    /// Received signal strength in dBm (0 = unknown).
    pub rssi: i8,
}

impl HostState {
    pub fn bonded(&self) -> bool {
        self.flags & host_flags::BONDED != 0
    }

    pub fn connected(&self) -> bool {
        self.flags & host_flags::CONNECTED != 0
    }

    /// Ready for input: connected, encrypted and subscribed to the keyboard report.
    pub fn ready(&self) -> bool {
        let need = host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED;
        self.flags & need == need
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BtMsg<'a> {
    /// Both ways, as [`super::LinkMsg::Hello`]: the coprocessor sends `role` [`ROLE`].
    Hello { proto: u8, fw_version: u32, role: u8, flags: u8 },
    /// RP → BT: input report `id` ([`report`]) for `host`. Only the selected host takes it.
    Input { host: u8, id: u8, data: &'a [u8] },
    /// RP → BT: the host that gets input, 0 = none (a PC is active).
    Select { host: u8 },
    /// RP → BT: open (`on`) or close pairing; it closes by itself after `seconds`.
    Pair { on: bool, seconds: u8 },
    /// RP → BT: forget the bond in `host` (0xFF = all) and drop its connection.
    Forget { host: u8 },
    /// RP → BT: the passkey typed for `host`'s [`EventKind::PasskeyRequest`]; `None` cancels the pairing.
    Passkey { host: u8, value: Option<u32> },
    /// RP → BT: what to do with `host` while another target is active.
    Policy { host: u8, policy: Policy },
    /// RP → BT: send every slot's [`HostState`] now.
    RequestState,
    /// RP → BT: restart into the ROM bootloader (UART download), to be reprogrammed. The module's BOOT strap shares
    /// the RP2354B's TX line, so it can't be held low across a reset of a running module: it restarts into its ROM
    /// by itself (software.md §4.13).
    EnterRom,
    /// BT → RP: output report `id` from `host` (keyboard LEDs).
    Output { host: u8, id: u8, data: &'a [u8] },
    /// BT → RP: a slot changed.
    Host(HostState),
    /// BT → RP: pairing and bond events.
    Event { kind: EventKind, host: u8, value: u32 },
}

const PASSKEY_CANCEL: u32 = u32::MAX;

impl<'a> BtMsg<'a> {
    pub fn encode(&self, out: &mut [u8; MAX_MSG]) -> Result<usize, WireError> {
        let mut w = Writer::new(out);
        match *self {
            BtMsg::Hello { proto, fw_version, role, flags } => {
                w.u8(ty::HELLO)?;
                w.u8(proto)?;
                w.u32(fw_version)?;
                w.u8(role)?;
                w.u8(flags)?;
            }
            BtMsg::Input { host, id, data } | BtMsg::Output { host, id, data } => {
                if data.len() > REPORT_MAX {
                    return Err(WireError::Invalid);
                }
                w.u8(if matches!(self, BtMsg::Input { .. }) { ty::INPUT } else { ty::OUTPUT })?;
                w.u8(host)?;
                w.u8(id)?;
                w.bytes(data)?;
            }
            BtMsg::Select { host } => {
                w.u8(ty::SELECT)?;
                w.u8(host)?;
            }
            BtMsg::Pair { on, seconds } => {
                w.u8(ty::PAIR)?;
                w.bool(on)?;
                w.u8(seconds)?;
            }
            BtMsg::Forget { host } => {
                w.u8(ty::FORGET)?;
                w.u8(host)?;
            }
            BtMsg::Passkey { host, value } => {
                w.u8(ty::PASSKEY)?;
                w.u8(host)?;
                w.u32(value.unwrap_or(PASSKEY_CANCEL))?;
            }
            BtMsg::Policy { host, policy } => {
                w.u8(ty::POLICY)?;
                w.u8(host)?;
                w.u8(policy as u8)?;
            }
            BtMsg::RequestState => w.u8(ty::REQUEST_STATE)?,
            BtMsg::EnterRom => w.u8(ty::ENTER_ROM)?,
            BtMsg::Host(h) => {
                w.u8(ty::HOST)?;
                w.u8(h.host)?;
                w.u8(h.flags)?;
                w.u16(h.interval)?;
                w.u16(h.latency)?;
                w.bytes(&h.addr)?;
                w.u8(h.rssi as u8)?;
            }
            BtMsg::Event { kind, host, value } => {
                w.u8(ty::EVENT)?;
                w.u8(kind as u8)?;
                w.u8(host)?;
                w.u32(value)?;
            }
        }
        Ok(w.len())
    }

    pub fn decode(msg: &'a [u8]) -> Result<Self, WireError> {
        let mut r = Reader::new(msg);
        Ok(match r.u8()? {
            ty::HELLO => BtMsg::Hello {
                proto: r.u8()?,
                fw_version: r.u32()?,
                role: r.u8()?,
                flags: if r.remaining() > 0 { r.u8()? } else { 0 },
            },
            ty::INPUT => BtMsg::Input { host: r.u8()?, id: r.u8()?, data: r.rest() },
            ty::OUTPUT => BtMsg::Output { host: r.u8()?, id: r.u8()?, data: r.rest() },
            ty::SELECT => BtMsg::Select { host: r.u8()? },
            ty::PAIR => BtMsg::Pair { on: r.bool()?, seconds: r.u8()? },
            ty::FORGET => BtMsg::Forget { host: r.u8()? },
            ty::PASSKEY => {
                let host = r.u8()?;
                let v = r.u32()?;
                BtMsg::Passkey { host, value: (v != PASSKEY_CANCEL).then_some(v) }
            }
            ty::POLICY => BtMsg::Policy { host: r.u8()?, policy: r.u8()?.try_into()? },
            ty::REQUEST_STATE => BtMsg::RequestState,
            ty::ENTER_ROM => BtMsg::EnterRom,
            ty::HOST => BtMsg::Host(HostState {
                host: r.u8()?,
                flags: r.u8()?,
                interval: r.u16()?,
                latency: r.u16()?,
                addr: r.array()?,
                rssi: r.u8()? as i8,
            }),
            ty::EVENT => BtMsg::Event { kind: r.u8()?.try_into()?, host: r.u8()?, value: r.u32()? },
            _ => return Err(WireError::Invalid),
        })
    }

    /// Input reports go ahead of everything else, as on the AT32F405 links.
    pub fn prio(&self) -> super::Prio {
        match self {
            BtMsg::Input { .. } => super::Prio::High,
            _ => super::Prio::Low,
        }
    }
}

/// The keyboard report for `keys` (modifier bits, then the pressed usages 0x04–0xA4 in any order).
pub fn keyboard_report(modifiers: u8, keys: impl Iterator<Item = u8>) -> [u8; report::KEYBOARD_LEN] {
    let mut r = [0u8; report::KEYBOARD_LEN];
    r[0] = modifiers;
    for (n, k) in keys.enumerate() {
        if n == 6 {
            // More than six: the boot keyboard's phantom state, every slot ErrorRollOver.
            r[2..].fill(0x01);
            return r;
        }
        r[2 + n] = k;
    }
    r
}

/// The mouse report: buttons, X/Y clamped to 16 bits, wheel and pan to 8.
pub fn mouse_report(buttons: u16, x: i32, y: i32, wheel: i32, pan: i32) -> [u8; report::MOUSE_LEN] {
    let x = x.clamp(-32767, 32767) as i16;
    let y = y.clamp(-32767, 32767) as i16;
    let b = buttons.to_le_bytes();
    let (xb, yb) = (x.to_le_bytes(), y.to_le_bytes());
    [b[0], b[1], xb[0], xb[1], yb[0], yb[1], wheel.clamp(-127, 127) as i8 as u8, pan.clamp(-127, 127) as i8 as u8]
}

/// The absolute pointer's report ([`report::POINTER`]): `x`, `y` 0–32767.
pub fn pointer_report(buttons: u16, x: u16, y: u16, wheel: i32, pan: i32) -> [u8; report::POINTER_LEN] {
    let b = buttons.to_le_bytes();
    let (xb, yb) = (x.min(0x7FFF).to_le_bytes(), y.min(0x7FFF).to_le_bytes());
    [b[0], b[1], xb[0], xb[1], yb[0], yb[1], wheel.clamp(-127, 127) as i8 as u8, pan.clamp(-127, 127) as i8 as u8]
}

/// Motion for a host not sent yet, coalesced: a mouse reports up to 1000 times a second, more than the link to a
/// module on a front port carries and far more than a host takes at its connection events, and a report that waits
/// in a queue is late while one that is dropped loses its motion (first dongle, 2026-11-14: the pointer crawled). So
/// one report waits at a time: relative moves (and wheels) add up, an absolute position replaces the one before. A
/// report with other buttons, or for another host, sends the waiting one first, so that no click is lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Motion {
    waiting: Option<Waiting>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Waiting {
    host: u8,
    id: u8,
    buttons: u16,
    /// Relative: the sum of the moves; absolute: the position.
    x: i32,
    y: i32,
    wheel: i32,
    pan: i32,
}

/// A report for a host: its host slot, its report ID and its bytes.
pub type Out = (u8, u8, [u8; 8]);

impl Motion {
    pub const fn new() -> Self {
        Motion { waiting: None }
    }

    /// Whether report `id` is motion (the mouse, the absolute pointer).
    pub const fn is_motion(id: u8) -> bool {
        id == report::MOUSE || id == report::POINTER
    }

    /// Adds a motion report `data` (`id` [`report::MOUSE`] or [`report::POINTER`]) for `host`. Returns the report that
    /// has to go out before it, if one waited that can't take it in.
    pub fn add(&mut self, host: u8, id: u8, data: &[u8]) -> Option<Out> {
        let mut d = [0u8; 8];
        d[..data.len().min(8)].copy_from_slice(&data[..data.len().min(8)]);
        let buttons = u16::from_le_bytes([d[0], d[1]]);
        let (x, y) = if id == report::POINTER {
            (u16::from_le_bytes([d[2], d[3]]) as i32, u16::from_le_bytes([d[4], d[5]]) as i32)
        } else {
            (i16::from_le_bytes([d[2], d[3]]) as i32, i16::from_le_bytes([d[4], d[5]]) as i32)
        };
        let (wheel, pan) = (d[6] as i8 as i32, d[7] as i8 as i32);
        if let Some(w) = self.waiting.as_mut()
            && (w.host, w.id, w.buttons) == (host, id, buttons)
        {
            if id == report::POINTER {
                (w.x, w.y) = (x, y);
            } else {
                (w.x, w.y) = (w.x.saturating_add(x), w.y.saturating_add(y));
            }
            (w.wheel, w.pan) = (w.wheel.saturating_add(wheel), w.pan.saturating_add(pan));
            return None;
        }
        let before = self.take_all();
        self.waiting = Some(Waiting { host, id, buttons, x, y, wheel, pan });
        before
    }

    /// The waiting report to send now, if there is one. A relative move longer than one report carries comes out in
    /// steps: what doesn't fit keeps waiting.
    pub fn take(&mut self) -> Option<Out> {
        let w = self.waiting.as_mut()?;
        let out = if w.id == report::POINTER {
            let r = pointer_report(w.buttons, w.x.clamp(0, 0x7FFF) as u16, w.y.clamp(0, 0x7FFF) as u16, w.wheel, w.pan);
            let (wheel, pan) = (w.wheel.clamp(-127, 127), w.pan.clamp(-127, 127));
            (w.wheel, w.pan) = (w.wheel - wheel, w.pan - pan);
            (w.host, w.id, r)
        } else {
            let (x, y) = (w.x.clamp(-32767, 32767), w.y.clamp(-32767, 32767));
            let (wheel, pan) = (w.wheel.clamp(-127, 127), w.pan.clamp(-127, 127));
            let r = mouse_report(w.buttons, x, y, wheel, pan);
            (w.x, w.y, w.wheel, w.pan) = (w.x - x, w.y - y, w.wheel - wheel, w.pan - pan);
            (w.host, w.id, r)
        };
        // A position is sent once; a move keeps waiting while some of it is left.
        if w.id == report::POINTER && w.wheel == 0 && w.pan == 0
            || w.id == report::MOUSE && w.x == 0 && w.y == 0 && w.wheel == 0 && w.pan == 0
        {
            self.waiting = None;
        }
        Some(out)
    }

    /// Everything waiting, in one report (a move longer than one report carries is cut: it only happens with motion
    /// that has waited for a very long time).
    fn take_all(&mut self) -> Option<Out> {
        let out = self.take();
        self.waiting = None;
        out
    }

    pub fn is_empty(&self) -> bool {
        self.waiting.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_adds_up_moves_keeps_positions_and_clicks() {
        let mut m = Motion::default();
        // Moves add up into one report.
        for _ in 0..10 {
            assert_eq!(m.add(1, report::MOUSE, &mouse_report(0, 3, -2, 1, 0)), None);
        }
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 30, -20, 10, 0))));
        assert_eq!(m.take(), None);
        // A press goes out after the move before it, and is not merged into it.
        m.add(1, report::MOUSE, &mouse_report(0, 5, 0, 0, 0));
        assert_eq!(
            m.add(1, report::MOUSE, &mouse_report(1, 0, 0, 0, 0)),
            Some((1, report::MOUSE, mouse_report(0, 5, 0, 0, 0)))
        );
        assert_eq!(m.add(1, report::MOUSE, &mouse_report(1, 2, 2, 0, 0)), None);
        assert_eq!(
            m.add(1, report::MOUSE, &mouse_report(0, 0, 0, 0, 0)),
            Some((1, report::MOUSE, mouse_report(1, 2, 2, 0, 0)))
        );
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 0, 0, 0, 0))));
        // Positions: the latest counts.
        m.add(2, report::POINTER, &pointer_report(0, 100, 100, 0, 0));
        m.add(2, report::POINTER, &pointer_report(0, 200, 300, 1, 0));
        assert_eq!(m.take(), Some((2, report::POINTER, pointer_report(0, 200, 300, 1, 0))));
        assert!(m.is_empty());
        // A move longer than a report carries comes out in steps.
        m.add(1, report::MOUSE, &mouse_report(0, 30000, 0, 0, 0));
        m.add(1, report::MOUSE, &mouse_report(0, 30000, 0, 0, 0));
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 32767, 0, 0, 0))));
        assert_eq!(m.take(), Some((1, report::MOUSE, mouse_report(0, 27233, 0, 0, 0))));
        assert!(m.is_empty());
    }

    fn roundtrip(m: BtMsg) {
        let mut buf = [0u8; MAX_MSG];
        let n = m.encode(&mut buf).unwrap();
        assert_eq!(BtMsg::decode(&buf[..n]).unwrap(), m);
    }

    #[test]
    fn messages_roundtrip() {
        roundtrip(BtMsg::Hello { proto: 1, fw_version: 0x000100, role: ROLE, flags: 0 });
        roundtrip(BtMsg::Input { host: 2, id: report::KEYBOARD, data: &[2, 0, 4, 0, 0, 0, 0, 0] });
        roundtrip(BtMsg::Output { host: 1, id: report::KEYBOARD, data: &[0b10] });
        roundtrip(BtMsg::Select { host: 0 });
        roundtrip(BtMsg::Pair { on: true, seconds: 60 });
        roundtrip(BtMsg::Forget { host: 0xFF });
        roundtrip(BtMsg::Passkey { host: 3, value: Some(123456) });
        roundtrip(BtMsg::Passkey { host: 3, value: None });
        roundtrip(BtMsg::Policy { host: 4, policy: Policy::ConnectWhenSelected });
        roundtrip(BtMsg::RequestState);
        roundtrip(BtMsg::EnterRom);
        roundtrip(BtMsg::Host(HostState {
            host: 1,
            flags: host_flags::BONDED | host_flags::CONNECTED,
            interval: 12,
            latency: 0,
            addr: [1, 2, 3, 4, 5, 6, 7],
            rssi: -60,
        }));
        roundtrip(BtMsg::Event { kind: EventKind::PairFailed, host: 2, value: 5 });
    }

    #[test]
    fn hello_is_the_spi_links_hello() {
        let mut buf = [0u8; MAX_MSG];
        let n = BtMsg::Hello { proto: 1, fw_version: 7, role: ROLE, flags: 0 }.encode(&mut buf).unwrap();
        assert!(matches!(
            super::super::LinkMsg::decode(&buf[..n]),
            Ok(super::super::LinkMsg::Hello { role: ROLE, .. })
        ));
    }

    #[test]
    fn map_is_well_formed() {
        // Every collection closed, and the report IDs in the map are the ones `INPUTS` lists.
        let mut depth = 0i32;
        let mut ids = heapless::Vec::<u8, 4>::new();
        let mut i = 0;
        while i < report::MAP.len() {
            let prefix = report::MAP[i];
            let size = match prefix & 3 {
                3 => 4,
                s => s as usize,
            };
            match prefix & 0xFC {
                0xA0 => depth += 1,
                0xC0 => depth -= 1,
                0x84 => ids.push(report::MAP[i + 1]).unwrap(),
                _ => {}
            }
            i += 1 + size;
        }
        assert_eq!(depth, 0);
        assert_eq!(i, report::MAP.len());
        assert_eq!(ids.as_slice(), &[report::KEYBOARD, report::MOUSE, report::CONSUMER, report::POINTER]);
        assert!(report::MAP.len() <= 512, "a GATT attribute holds at most 512 bytes");
        assert_eq!(report::MAP.len(), 229, "the length software.md §4.13 gives");
    }

    #[test]
    fn keyboard_rollover() {
        assert_eq!(keyboard_report(0x02, [4u8, 5].into_iter()), [0x02, 0, 4, 5, 0, 0, 0, 0]);
        assert_eq!(keyboard_report(0, 4u8..11), [0, 0, 1, 1, 1, 1, 1, 1]);
        assert_eq!(mouse_report(1, -40000, 3, 200, -1), [1, 0, 0x01, 0x80, 3, 0, 127, 0xFF]);
        assert_eq!(pointer_report(0x8001, 40000, 300, -2, 0), [1, 0x80, 0xFF, 0x7F, 0x2C, 1, 0xFE, 0]);
    }
}
