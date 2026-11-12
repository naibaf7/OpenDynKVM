//! Merges normalized input from every source (port, interface, report ID) into one keyboard, one mouse, one consumer
//! and one system state. Keyboards that report through several interfaces (boot + NKRO), or several report IDs, end up
//! as one key set.

use heapless::Vec;
use kvm_hid::translate::{ConsumerState, Translation};
use kvm_hid::{KeyState, MouseInput};
use kvm_proto::ctrl::Port;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Source {
    pub port: Port,
    pub iface: u8,
    pub report_id: u8,
}

pub const MAX_SOURCES: usize = 16;

/// What changed after one report. Keyboard, consumer and system are full merged states, present only on change; the
/// mouse carries merged buttons plus this report's motion, present when either changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Merged {
    pub keyboard: Option<KeyState>,
    pub mouse: Option<MouseInput>,
    pub consumer: Option<ConsumerState>,
    pub system: Option<u8>,
}

impl Merged {
    pub fn is_empty(&self) -> bool {
        self.keyboard.is_none() && self.mouse.is_none() && self.consumer.is_none() && self.system.is_none()
    }
}

#[derive(Default)]
pub struct Merger {
    keys: Vec<(Source, KeyState), MAX_SOURCES>,
    buttons: Vec<(Source, u16), MAX_SOURCES>,
    consumer: Vec<(Source, ConsumerState), MAX_SOURCES>,
    system: Vec<(Source, u8), MAX_SOURCES>,
    last_keys: KeyState,
    last_buttons: u16,
    last_consumer: ConsumerState,
    last_system: u8,
}

fn upsert<T: Copy>(v: &mut Vec<(Source, T), MAX_SOURCES>, s: Source, val: T) {
    match v.iter_mut().find(|(k, _)| *k == s) {
        Some(e) => e.1 = val,
        None => {
            let _ = v.push((s, val));
        }
    }
}

impl Merger {
    pub const fn new() -> Self {
        Self {
            keys: Vec::new(),
            buttons: Vec::new(),
            consumer: Vec::new(),
            system: Vec::new(),
            last_keys: KeyState::EMPTY,
            last_buttons: 0,
            last_consumer: ConsumerState { usages: [0; kvm_hid::normalized::CONSUMER_SLOTS] },
            last_system: 0,
        }
    }

    pub fn keys(&self) -> KeyState {
        self.last_keys
    }

    pub fn buttons(&self) -> u16 {
        self.last_buttons
    }

    pub fn consumer(&self) -> ConsumerState {
        self.last_consumer
    }

    pub fn system(&self) -> u8 {
        self.last_system
    }

    pub fn apply(&mut self, port: Port, iface: u8, t: &Translation) -> Merged {
        let src = Source { port, iface, report_id: t.report_id };
        if let Some(k) = t.keyboard {
            upsert(&mut self.keys, src, k);
        }
        if let Some(m) = t.mouse {
            upsert(&mut self.buttons, src, m.buttons);
        }
        if let Some(c) = t.consumer {
            upsert(&mut self.consumer, src, c);
        }
        if let Some(s) = t.system {
            upsert(&mut self.system, src, s);
        }
        let mut out = self.recompute();
        if let Some(m) = t.mouse
            && m.has_motion()
        {
            out.mouse = Some(MouseInput { buttons: self.last_buttons, ..m });
        }
        out
    }

    /// Forget everything a port contributed (device unplugged or re-probed); returns the resulting releases.
    pub fn remove_port(&mut self, port: Port) -> Merged {
        self.keys.retain(|(s, _)| s.port != port);
        self.buttons.retain(|(s, _)| s.port != port);
        self.consumer.retain(|(s, _)| s.port != port);
        self.system.retain(|(s, _)| s.port != port);
        self.recompute()
    }

    fn recompute(&mut self) -> Merged {
        let mut out = Merged::default();
        let keys = self.keys.iter().fold(KeyState::EMPTY, |acc, (_, k)| acc.union(k));
        if keys != self.last_keys {
            self.last_keys = keys;
            out.keyboard = Some(keys);
        }
        let buttons = self.buttons.iter().fold(0, |acc, (_, b)| acc | b);
        if buttons != self.last_buttons {
            self.last_buttons = buttons;
            out.mouse = Some(MouseInput { buttons, ..MouseInput::default() });
        }
        let mut consumer = ConsumerState::default();
        let mut slot = 0;
        for u in self.consumer.iter().flat_map(|(_, c)| c.usages).filter(|u| *u != 0) {
            if slot < consumer.usages.len() && !consumer.usages.contains(&u) {
                consumer.usages[slot] = u;
                slot += 1;
            }
        }
        if consumer != self.last_consumer {
            self.last_consumer = consumer;
            out.consumer = Some(consumer);
        }
        let system = self.system.iter().fold(0, |acc, (_, s)| acc | s);
        if system != self.last_system {
            self.last_system = system;
            out.system = Some(system);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kb(id: u8, list: &[u8]) -> Translation {
        let mut k = KeyState::EMPTY;
        for x in list {
            k.press(*x);
        }
        Translation { report_id: id, keyboard: Some(k), ..Translation::default() }
    }

    #[test]
    fn keys_from_two_interfaces_merge_and_release() {
        let mut m = Merger::new();
        let a = m.apply(Port::Keyboard, 0, &kb(0, &[0x04]));
        assert_eq!(a.keyboard.unwrap().keys().collect::<std::vec::Vec<_>>(), [0x04]);
        let b = m.apply(Port::Keyboard, 1, &kb(2, &[0x05]));
        assert_eq!(b.keyboard.unwrap().key_count(), 2);
        assert!(m.apply(Port::Keyboard, 1, &kb(2, &[0x05])).is_empty(), "unchanged state is not re-sent");
        let gone = m.remove_port(Port::Keyboard);
        assert!(gone.keyboard.unwrap().is_empty());
    }

    #[test]
    fn mouse_motion_carries_merged_buttons() {
        let mut m = Merger::new();
        let press = Translation { mouse: Some(MouseInput { buttons: 1, ..Default::default() }), ..Default::default() };
        assert_eq!(m.apply(Port::Mouse, 0, &press).mouse.unwrap().buttons, 1);
        let mv =
            Translation { mouse: Some(MouseInput { buttons: 1, x: 5, ..Default::default() }), ..Default::default() };
        assert_eq!(m.apply(Port::Mouse, 0, &mv).mouse, Some(MouseInput { buttons: 1, x: 5, ..Default::default() }));
        let idle = Translation { mouse: Some(MouseInput { buttons: 1, ..Default::default() }), ..Default::default() };
        assert_eq!(m.apply(Port::Mouse, 0, &idle).mouse, None);
    }
}
