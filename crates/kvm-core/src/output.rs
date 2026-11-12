//! Per-PC report generation for the normalized interfaces: which endpoint, which format (boot or report protocol),
//! wheel scaling, and which of the mouse interface's two pointers (relative, absolute) a button is held on.

use heapless::Vec;
use kvm_hid::translate::ConsumerState;
use kvm_hid::{KeyState, MouseInput, normalized};

use crate::hop;
use crate::presentation::Presentation;

pub const MAX_REPORT: usize = 64;
pub type Report = Vec<u8, MAX_REPORT>;

fn report(data: &[u8]) -> Report {
    Report::from_slice(data).unwrap_or_default()
}

/// What a PC has told its emulated device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PcOutput {
    /// SET_PROTOCOL(boot) on the boot keyboard interface: keys go there instead of NKRO.
    pub boot_keyboard: bool,
    pub boot_mouse: bool,
    pub wheel_hires: bool,
    pub pan_hires: bool,
    /// Last keyboard LED output report (Num/Caps/Scroll…).
    pub leds: u8,
    /// The buttons held on the PC by the relative and by the absolute pointer: to the PC they are two devices, and a
    /// button pressed on one is released on that one (Linux keeps it down otherwise).
    pub held: [u16; 2],
    /// Where the absolute pointer last put the cursor.
    pub abs_at: Option<[u16; 2]>,
}

/// The pointers of the mouse interface, as `PcOutput::held` indexes them.
const REL: usize = 0;
const ABS: usize = 1;

impl PcOutput {
    pub const DEFAULT: PcOutput = PcOutput {
        boot_keyboard: false,
        boot_mouse: false,
        wheel_hires: false,
        pan_hires: false,
        leds: 0,
        held: [0; 2],
        abs_at: None,
    };

    /// Back to power-on defaults (the PC reset or re-enumerated the device).
    pub fn reset(&mut self) {
        *self = PcOutput { leds: self.leds, ..PcOutput::default() };
    }

    pub fn keyboard(&self, keys: &KeyState, p: &Presentation) -> (u8, Report) {
        if self.boot_keyboard {
            (p.boot_keyboard.ep_in, report(&normalized::boot_keyboard_report(keys)))
        } else {
            (p.nkro.ep_in, report(&normalized::nkro_report(keys)))
        }
    }

    /// A relative mouse report.
    pub fn mouse(&self, m: &MouseInput, p: &Presentation) -> (u8, Report) {
        let data = if self.boot_mouse {
            report(&normalized::boot_mouse_report(m))
        } else if p.mouse_abs {
            report(&normalized::mouse_rel_report(m, self.wheel_hires, self.pan_hires))
        } else {
            report(&normalized::mouse_report(m, self.wheel_hires, self.pan_hires))
        };
        (p.mouse.ep_in, data)
    }

    fn abs_mouse(&mut self, buttons: u16, at: [u16; 2], wheel: i32, pan: i32, p: &Presentation) -> (u8, Report) {
        self.abs_at = Some(at);
        (p.mouse.ep_in, report(&normalized::mouse_abs_report(buttons, at, wheel, pan)))
    }

    /// The pointer reports for `motion` with `buttons` held, wheel and pan: on the absolute pointer for an absolute
    /// position (where the interface has it and the PC is in report protocol), on the relative one otherwise. A button
    /// stays on the pointer it was pressed on; when one held on the other pointer is released, that one reports it
    /// first.
    pub fn pointer(
        &mut self,
        motion: hop::Report,
        buttons: u16,
        wheel: i32,
        pan: i32,
        p: &Presentation,
    ) -> Vec<(u8, Report), 2> {
        let abs = match motion {
            hop::Report::Abs(at) if p.mouse_abs && !self.boot_mouse => Some(at),
            _ => None,
        };
        let (this, other) = if abs.is_some() { (ABS, REL) } else { (REL, ABS) };
        let held = self.held[REL] | self.held[ABS];
        let released = held & !buttons;
        let other_lets_go = self.held[other] & released != 0;
        self.held[REL] &= !released;
        self.held[ABS] &= !released;
        self.held[this] |= buttons & !held;
        let mut out = Vec::new();
        if other_lets_go {
            let r = if other == ABS {
                let at = self.abs_at.unwrap_or([0, 0]);
                self.abs_mouse(self.held[ABS], at, 0, 0, p)
            } else {
                self.mouse(&MouseInput { buttons: self.held[REL], ..MouseInput::default() }, p)
            };
            let _ = out.push(r);
        }
        let r = match (abs, motion) {
            (Some(at), _) => self.abs_mouse(self.held[ABS], at, wheel, pan, p),
            (None, hop::Report::Rel([x, y])) => {
                self.mouse(&MouseInput { buttons: self.held[REL], x, y, wheel, pan }, p)
            }
            (None, hop::Report::Abs(_)) => {
                self.mouse(&MouseInput { buttons: self.held[REL], wheel, pan, ..MouseInput::default() }, p)
            }
        };
        let _ = out.push(r);
        out
    }

    pub fn consumer(&self, c: &ConsumerState, p: &Presentation) -> (u8, Report) {
        (p.nkro.ep_in, report(&normalized::consumer_report(&c.usages)))
    }

    pub fn system(&self, bits: u8, p: &Presentation) -> (u8, Report) {
        (p.nkro.ep_in, report(&normalized::system_report(bits)))
    }

    /// Everything released, on every normalized interface (switching away from this PC): both pointers, where the
    /// absolute one was used, and it stays where it is.
    pub fn release_all(&mut self, p: &Presentation) -> Vec<(u8, Report), 6> {
        let none = KeyState::EMPTY;
        self.held = [0; 2];
        let mut out = Vec::new();
        let _ = out.push((p.boot_keyboard.ep_in, report(&normalized::boot_keyboard_report(&none))));
        let _ = out.push((p.nkro.ep_in, report(&normalized::nkro_report(&none))));
        let _ = out.push(self.mouse(&MouseInput::default(), p));
        if let Some(at) = self.abs_at.filter(|_| p.mouse_abs && !self.boot_mouse) {
            let _ = out.push(self.abs_mouse(0, at, 0, 0, p));
        }
        let _ = out.push(self.consumer(&ConsumerState::default(), p));
        let _ = out.push(self.system(0, p));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presentation::Function;

    fn pres() -> Presentation {
        Presentation {
            boot_keyboard: Function { iface: 0, ep_in: 0x81, ep_out: 0 },
            mouse: Function { iface: 1, ep_in: 0x82, ep_out: 0 },
            nkro: Function { iface: 2, ep_in: 0x83, ep_out: 0 },
            ..Presentation::default()
        }
    }

    #[test]
    fn protocol_selects_interface_and_format() {
        let mut o = PcOutput::default();
        let mut k = KeyState::EMPTY;
        k.press(0x04);
        let (ep, r) = o.keyboard(&k, &pres());
        assert_eq!((ep, r.len(), r[0]), (0x83, 30, normalized::NKRO_ID));
        o.boot_keyboard = true;
        assert_eq!(o.keyboard(&k, &pres()), (0x81, report(&[0, 0, 4, 0, 0, 0, 0, 0])));
        o.boot_mouse = true;
        let m = MouseInput { buttons: 1, x: 3, y: -3, wheel: 1, pan: 0 };
        assert_eq!(o.mouse(&m, &pres()), (0x82, report(&[1, 3, 0xFD])));
        o.reset();
        assert_eq!(o.mouse(&m, &pres()).1.len(), normalized::MOUSE_LEN);
        assert_eq!(o.release_all(&pres()).len(), 5);
    }

    #[test]
    fn a_button_is_released_on_the_pointer_it_was_pressed_on() {
        let p = Presentation { mouse_abs: true, ..pres() };
        let mut o = PcOutput::default();
        let rel = |x| hop::Report::Rel([x, 0]);
        // Pressed while relative, then the pointer goes absolute (screen hop on) while it is held.
        let r = o.pointer(rel(1), 1, 0, 0, &p);
        assert_eq!(r.as_slice(), &[(0x82, report(&[1, 1, 0, 1, 0, 0, 0, 0, 0, 0, 0]))]);
        let r = o.pointer(hop::Report::Abs([100, 200]), 1 | 2, 0, 0, &p);
        assert_eq!(r.as_slice(), &[(0x82, report(&[2, 2, 0, 100, 0, 200, 0, 0, 0, 0, 0]))]);
        assert_eq!(o.held, [1, 2]);
        // Button 1 let go: the relative pointer reports it, then the absolute one moves on with button 2.
        let r = o.pointer(hop::Report::Abs([101, 200]), 2, 0, 0, &p);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].1.as_slice(), &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(r[1].1.as_slice(), &[2, 2, 0, 101, 0, 200, 0, 0, 0, 0, 0]);
        // Leaving the PC releases both, the absolute pointer where it is.
        let all = o.release_all(&p);
        assert_eq!(all.len(), 6);
        assert_eq!(all[3].1.as_slice(), &[2, 0, 0, 101, 0, 200, 0, 0, 0, 0, 0]);
        assert_eq!(o.held, [0, 0]);
        // In boot protocol (or without the absolute pointer) a position becomes a relative report without motion.
        o.boot_mouse = true;
        let r = o.pointer(hop::Report::Abs([5, 5]), 4, 0, 0, &p);
        assert_eq!(r.as_slice(), &[(0x82, report(&[4, 0, 0]))]);
    }
}
