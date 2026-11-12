//! Bluetooth hosts as targets (software.md §4.13, design.md §13): the state the engine keeps of the Bluetooth module
//! (boards 0.16.0 on, when fitted) and what it sends it.
//!
//! The module holds up to four bonded hosts. The engine decides which target gets input: a PC (through its AT32F405,
//! as before) or one Bluetooth host. A Bluetooth host is a lesser target: it gets the KVM's own keyboard, mouse and media
//! reports ([`kvm_proto::link::bt::report`]), never a mirrored device's descriptor, and the front ports in passthrough
//! stay with the PC that was active before (the *home* PC, `Engine::active`).

use heapless::Vec;
use kvm_hid::KeyState;
use kvm_hid::translate::ConsumerState;
use kvm_proto::ctrl::{BT_HOSTS, BtHostInfo, BtPolicy};
use kvm_proto::link::bt::{EventKind, HostState, report};

/// What the engine asks of the Bluetooth module (the firmware sends it as a [`kvm_proto::link::BtMsg`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BtOut {
    Input { host: u8, id: u8, data: Vec<u8, { report::MOUSE_LEN }> },
    Select(u8),
    Pair { on: bool, seconds: u8 },
    Forget(u8),
    Passkey { host: u8, value: Option<u32> },
    Policy { host: u8, policy: BtPolicy },
    RequestState,
}

/// A passkey being typed on the KVM's keyboard for a pairing host (KeyboardOnly pairing: the host shows six digits).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PasskeyEntry {
    pub host: u8,
    pub value: u32,
    pub digits: u8,
    /// Keys held at the last report, so that only new presses count.
    held: KeyState,
}

/// What a key press does to a passkey being entered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasskeyStep {
    /// Still typing.
    Typing,
    /// Enter after six digits: send it.
    Done(u32),
    /// Escape: cancel the pairing.
    Cancel,
}

impl PasskeyEntry {
    pub fn new(host: u8, held: KeyState) -> Self {
        Self { host, value: 0, digits: 0, held }
    }

    /// Feeds the keys held now; digits on the top row or the keypad, Backspace, Enter (or the sixth digit's Enter on
    /// the keypad), Escape.
    pub fn feed(&mut self, keys: &KeyState) -> PasskeyStep {
        let mut step = PasskeyStep::Typing;
        for k in keys.keys() {
            if self.held.is_pressed(k) {
                continue;
            }
            match digit(k) {
                Some(d) if self.digits < 6 => {
                    self.value = self.value * 10 + d as u32;
                    self.digits += 1;
                }
                Some(_) => {}
                None => match k {
                    0x2A => {
                        // Backspace
                        if self.digits > 0 {
                            self.value /= 10;
                            self.digits -= 1;
                        }
                    }
                    0x28 | 0x58 if self.digits == 6 => step = PasskeyStep::Done(self.value),
                    0x29 => step = PasskeyStep::Cancel,
                    _ => {}
                },
            }
        }
        self.held = *keys;
        step
    }
}

/// The digit a keyboard usage types: 1–9, 0 on the top row (0x1E–0x27) and the keypad (0x59–0x62).
fn digit(usage: u8) -> Option<u8> {
    match usage {
        0x1E..=0x26 => Some(usage - 0x1E + 1),
        0x27 => Some(0),
        0x59..=0x61 => Some(usage - 0x59 + 1),
        0x62 => Some(0),
        _ => None,
    }
}

/// The engine's view of the Bluetooth module.
#[derive(Debug, Clone, Copy)]
pub struct BtState {
    /// The module answered (its `Hello`); without it there are no Bluetooth targets (a board without the module, a
    /// board before 0.16.0, or a module that isn't programmed yet).
    pub present: bool,
    pub fw: u32,
    pub hosts: [HostState; BT_HOSTS],
    /// The host slot that gets input, 1–4 (None: the home PC does).
    pub active: Option<u8>,
    /// When pairing closes (ms, engine time); None while closed.
    pub pairing_until: Option<u32>,
    pub passkey: Option<PasskeyEntry>,
    /// Each host's keyboard LEDs (its output report).
    pub leds: [u8; BT_HOSTS],
    /// Last reports sent to the active host, so that only changes go out.
    pub sent_keyboard: [u8; report::KEYBOARD_LEN],
    pub sent_consumer: u16,
}

impl BtState {
    pub const EMPTY: BtState = BtState {
        present: false,
        fw: 0,
        hosts: [HostState { host: 0, flags: 0, interval: 0, latency: 0, addr: [0; 7], rssi: 0 }; BT_HOSTS],
        active: None,
        pairing_until: None,
        passkey: None,
        leds: [0; BT_HOSTS],
        sent_keyboard: [0; report::KEYBOARD_LEN],
        sent_consumer: 0,
    };

    pub fn host(&self, n: u8) -> Option<&HostState> {
        n.checked_sub(1).and_then(|i| self.hosts.get(i as usize))
    }

    /// Slot `n` can be selected: the module is there and holds a bond in it.
    pub fn selectable(&self, n: u8) -> bool {
        self.present && self.host(n).is_some_and(|h| h.bonded())
    }

    /// Bit n−1 for each slot that `f` picks.
    pub fn mask(&self, f: impl Fn(&HostState) -> bool) -> u8 {
        if !self.present {
            return 0;
        }
        self.hosts.iter().enumerate().filter(|(_, h)| f(h)).fold(0, |m, (i, _)| m | 1 << i)
    }

    pub fn info(&self, n: u8) -> Option<BtHostInfo> {
        let h = self.host(n)?;
        Some(BtHostInfo {
            host: n,
            flags: h.flags,
            interval: h.interval,
            latency: h.latency,
            rssi: h.rssi,
            addr: h.addr,
        })
    }

    /// Seconds pairing stays open (0 = closed).
    pub fn pairing_seconds(&self, now: u32) -> u8 {
        self.pairing_until.map_or(0, |t| (t.saturating_sub(now).div_ceil(1000)).min(255) as u8)
    }

    /// Records a module event (`held`: the keys held now, which a passkey being typed ignores until released).
    pub fn event(&mut self, kind: EventKind, host: u8, value: u32, now: u32, held: KeyState) {
        match kind {
            EventKind::PairingOpen => self.pairing_until = Some(now.wrapping_add(value.saturating_mul(1000))),
            EventKind::PairingClosed => {
                self.pairing_until = None;
                self.passkey = None;
            }
            EventKind::PasskeyRequest => self.passkey = Some(PasskeyEntry::new(host, held)),
            EventKind::Paired => {
                self.pairing_until = None;
                self.passkey = None;
            }
            EventKind::PairFailed => self.passkey = None,
            EventKind::BondLost => {}
        }
    }
}

/// The keyboard report of `keys` for a Bluetooth host (the boot layout with report ID [`report::KEYBOARD`]).
pub fn keyboard(keys: &KeyState) -> [u8; report::KEYBOARD_LEN] {
    keys.to_boot()
}

/// The one consumer usage a Bluetooth host gets: the first held.
pub fn consumer(c: &ConsumerState) -> u16 {
    c.usages.iter().copied().find(|u| *u != 0).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(us: &[u8]) -> KeyState {
        let mut k = KeyState::EMPTY;
        for u in us {
            k.press(*u);
        }
        k
    }

    #[test]
    fn passkey_typed_on_top_row_and_keypad() {
        let mut e = PasskeyEntry::new(2, KeyState::EMPTY);
        // 1 2 3 on the top row, 4 5 6 on the keypad, each pressed and released.
        for u in [0x1E, 0x1F, 0x20, 0x5C, 0x5D, 0x5E] {
            assert_eq!(e.feed(&keys(&[u])), PasskeyStep::Typing);
            assert_eq!(e.feed(&KeyState::EMPTY), PasskeyStep::Typing);
        }
        assert_eq!(e.feed(&keys(&[0x28])), PasskeyStep::Done(123456));
    }

    #[test]
    fn passkey_backspace_escape_and_held_keys() {
        // Enter held from the command that started pairing doesn't count.
        let mut e = PasskeyEntry::new(1, keys(&[0x28]));
        assert_eq!(e.feed(&keys(&[0x28])), PasskeyStep::Typing);
        e.feed(&keys(&[0x27]));
        e.feed(&keys(&[0x27, 0x2A]));
        assert_eq!((e.value, e.digits), (0, 0));
        e.feed(&KeyState::EMPTY);
        // Leading zeros count as digits: 000042.
        for u in [0x27, 0x27, 0x27, 0x27, 0x21, 0x1F] {
            e.feed(&keys(&[u]));
            e.feed(&KeyState::EMPTY);
        }
        assert_eq!(e.feed(&keys(&[0x58])), PasskeyStep::Done(42));
        assert_eq!(e.feed(&keys(&[0x29])), PasskeyStep::Cancel);
    }

    #[test]
    fn masks_need_the_module() {
        let mut s = BtState::EMPTY;
        s.hosts[1].flags = kvm_proto::link::bt::host_flags::BONDED;
        assert_eq!(s.mask(|h| h.bonded()), 0);
        s.present = true;
        assert_eq!(s.mask(|h| h.bonded()), 0b10);
        assert!(s.selectable(2) && !s.selectable(1) && !s.selectable(5) && !s.selectable(0));
    }
}
