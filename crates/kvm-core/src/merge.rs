//! Merges normalized input from every source (device, interface, report ID) into one keyboard, one mouse, one consumer,
//! one system and one gamepad state. Keyboards that report through several interfaces (boot + NKRO), or several report
//! IDs, end up as one key set. Gamepads (hid_mapping.md §6.2): buttons OR, the hat of the last one pressed, each axis
//! from the source that last moved it.

use heapless::Vec;
use kvm_hid::gamepad::{self, GamepadInput, HAT_CENTRE};
use kvm_hid::translate::{ConsumerState, Translation};
use kvm_hid::{KeyState, MouseInput};
use kvm_proto::ctrl::Device;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Source {
    pub dev: Device,
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
    /// The merged gamepad, present on change.
    pub gamepad: Option<GamepadInput>,
}

impl Merged {
    pub fn is_empty(&self) -> bool {
        self.keyboard.is_none()
            && self.mouse.is_none()
            && self.consumer.is_none()
            && self.system.is_none()
            && self.gamepad.is_none()
    }
}

/// An axis moved by less than this (of ±32767) is noise, and doesn't take the axis over from another source.
const AXIS_NOISE: i32 = 1024;

#[derive(Default)]
pub struct Merger {
    keys: Vec<(Source, KeyState), MAX_SOURCES>,
    buttons: Vec<(Source, u16), MAX_SOURCES>,
    consumer: Vec<(Source, ConsumerState), MAX_SOURCES>,
    system: Vec<(Source, u8), MAX_SOURCES>,
    pads: Vec<(Source, GamepadInput), MAX_SOURCES>,
    /// The source whose hat was pressed last, and the one that last moved each axis.
    hat_owner: Option<Source>,
    axis_owner: [Option<Source>; gamepad::AXES],
    last_pad: GamepadInput,
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
            pads: Vec::new(),
            hat_owner: None,
            axis_owner: [None; gamepad::AXES],
            last_pad: GamepadInput { buttons: 0, hat: HAT_CENTRE, axes: [0; gamepad::AXES], present: 0 },
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

    pub fn gamepad(&self) -> GamepadInput {
        self.last_pad
    }

    pub fn apply(&mut self, dev: Device, iface: u8, t: &Translation) -> Merged {
        let src = Source { dev, iface, report_id: t.report_id };
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
        if let Some(g) = t.gamepad {
            let before = self.pads.iter().find(|(k, _)| *k == src).map(|(_, p)| *p);
            if g.hat != HAT_CENTRE && before.is_none_or(|b| b.hat != g.hat) {
                self.hat_owner = Some(src);
            }
            for a in (0..gamepad::AXES).filter(|a| g.present & (1 << a) != 0) {
                let was = before.map_or(0, |b| b.axes[a]);
                if self.axis_owner[a].is_none() || (g.axes[a] - was).abs() > AXIS_NOISE {
                    self.axis_owner[a] = Some(src);
                }
            }
            upsert(&mut self.pads, src, g);
        }
        let mut out = self.recompute();
        if let Some(m) = t.mouse
            && m.has_motion()
        {
            out.mouse = Some(MouseInput { buttons: self.last_buttons, ..m });
        }
        out
    }

    /// Forget everything a device contributed (unplugged, re-probed, its input going elsewhere now); returns the
    /// resulting releases.
    pub fn remove(&mut self, dev: Device) -> Merged {
        self.keys.retain(|(s, _)| s.dev != dev);
        self.buttons.retain(|(s, _)| s.dev != dev);
        self.consumer.retain(|(s, _)| s.dev != dev);
        self.system.retain(|(s, _)| s.dev != dev);
        self.pads.retain(|(s, _)| s.dev != dev);
        if self.hat_owner.is_some_and(|s| s.dev == dev) {
            self.hat_owner = None;
        }
        for o in self.axis_owner.iter_mut().filter(|o| o.is_some_and(|s| s.dev == dev)) {
            *o = None;
        }
        self.recompute()
    }

    /// Whether anything of `dev` is merged here.
    pub fn has(&self, dev: Device) -> bool {
        self.keys.iter().any(|(s, _)| s.dev == dev)
            || self.buttons.iter().any(|(s, _)| s.dev == dev)
            || self.consumer.iter().any(|(s, _)| s.dev == dev)
            || self.system.iter().any(|(s, _)| s.dev == dev)
            || self.pads.iter().any(|(s, _)| s.dev == dev)
    }

    /// The keys and buttons `dev` holds now.
    pub fn held_by(&self, dev: Device) -> (KeyState, u16) {
        let keys = self.keys.iter().filter(|(s, _)| s.dev == dev).fold(KeyState::EMPTY, |acc, (_, k)| acc.union(k));
        let buttons = self.buttons.iter().filter(|(s, _)| s.dev == dev).fold(0, |acc, (_, b)| acc | b);
        (keys, buttons)
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
        let of = |o: Option<Source>| o.and_then(|o| self.pads.iter().find(|(s, _)| *s == o).map(|(_, p)| *p));
        let mut pad = GamepadInput {
            buttons: self.pads.iter().fold(0, |acc, (_, p)| acc | p.buttons),
            hat: of(self.hat_owner).map_or(HAT_CENTRE, |p| p.hat),
            present: self.pads.iter().fold(0, |acc, (_, p)| acc | p.present),
            ..GamepadInput::default()
        };
        for a in 0..gamepad::AXES {
            pad.axes[a] = of(self.axis_owner[a]).map_or(0, |p| p.axes[a]);
        }
        if pad != self.last_pad {
            self.last_pad = pad;
            out.gamepad = Some(pad);
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
        let a = m.apply(Device::Keyboard, 0, &kb(0, &[0x04]));
        assert_eq!(a.keyboard.unwrap().keys().collect::<std::vec::Vec<_>>(), [0x04]);
        let b = m.apply(Device::Keyboard, 1, &kb(2, &[0x05]));
        assert_eq!(b.keyboard.unwrap().key_count(), 2);
        assert!(m.apply(Device::Keyboard, 1, &kb(2, &[0x05])).is_empty(), "unchanged state is not re-sent");
        let gone = m.remove(Device::Keyboard);
        assert!(gone.keyboard.unwrap().is_empty());
    }

    #[test]
    fn gamepads_merge_buttons_and_the_last_moved_axis() {
        let pad = |buttons: u32, hat: u8, x: i32| Translation {
            gamepad: Some(GamepadInput { buttons, hat, axes: [x, 0, 0, 0, 0, 0, 0, 0, 0, 0], present: 1 }),
            ..Translation::default()
        };
        let mut m = Merger::new();
        assert_eq!(m.apply(Device::Other, 0, &pad(1, HAT_CENTRE, 20000)).gamepad.unwrap().axes[0], 20000);
        let both = m.apply(Device::Wireless1, 0, &pad(4, 2, 0)).gamepad.unwrap();
        assert_eq!((both.buttons, both.hat, both.axes[0]), (5, 2, 20000), "the second one's X rests: no takeover");
        let moved = m.apply(Device::Wireless1, 0, &pad(4, 2, -9000)).gamepad.unwrap();
        assert_eq!(moved.axes[0], -9000);
        let gone = m.remove(Device::Wireless1).gamepad.unwrap();
        assert_eq!((gone.buttons, gone.hat, gone.axes[0]), (1, HAT_CENTRE, 0));
    }

    #[test]
    fn mouse_motion_carries_merged_buttons() {
        let mut m = Merger::new();
        let press = Translation { mouse: Some(MouseInput { buttons: 1, ..Default::default() }), ..Default::default() };
        assert_eq!(m.apply(Device::Mouse, 0, &press).mouse.unwrap().buttons, 1);
        let mv =
            Translation { mouse: Some(MouseInput { buttons: 1, x: 5, ..Default::default() }), ..Default::default() };
        assert_eq!(m.apply(Device::Mouse, 0, &mv).mouse, Some(MouseInput { buttons: 1, x: 5, ..Default::default() }));
        let idle = Translation { mouse: Some(MouseInput { buttons: 1, ..Default::default() }), ..Default::default() };
        assert_eq!(m.apply(Device::Mouse, 0, &idle).mouse, None);
    }
}
