//! Port modes: what a device looks like ([`DeviceSummary`]) and which mode it gets ([`decide`], design.md §4).

use heapless::Vec;
use kvm_hid::translate::{Verdict, caps};
use kvm_proto::ctrl::{AutoReason, Port, PortMode, Speed, class as cls};

use crate::usbdesc::{ConfigInfo, DeviceDesc, MAX_INTERFACES, class};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum IfaceKind {
    Hid(Verdict),
    XInput,
    Audio,
    Video,
    Storage,
    Hub,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct IfaceSummary {
    pub number: u8,
    pub kind: IfaceKind,
    pub has_in: bool,
    pub has_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct DeviceSummary {
    pub vid: u16,
    pub pid: u16,
    pub speed: Option<Speed>,
    pub max_power_ma: u16,
    pub interfaces: Vec<IfaceSummary, MAX_INTERFACES>,
}

impl DeviceSummary {
    /// `verdict(n)` gives the HID verdict of interface `n` (from its parsed report descriptor), `None` if it couldn't
    /// be read or parsed.
    pub fn new(dev: &DeviceDesc, cfg: &ConfigInfo, speed: Speed, verdict: impl Fn(u8) -> Option<Verdict>) -> Self {
        let mut s = DeviceSummary {
            vid: dev.vid,
            pid: dev.pid,
            speed: Some(speed),
            max_power_ma: cfg.max_power_ma,
            interfaces: Vec::new(),
        };
        for i in &cfg.interfaces {
            let kind = if i.is_hid() {
                // An unparseable report descriptor can still be mirrored byte for byte, but never normalized.
                IfaceKind::Hid(verdict(i.number).unwrap_or(Verdict { caps: caps::OTHER, misfit: u16::MAX }))
            } else if i.is_xinput() {
                IfaceKind::XInput
            } else {
                match i.class {
                    class::AUDIO => IfaceKind::Audio,
                    class::VIDEO => IfaceKind::Video,
                    class::MASS_STORAGE => IfaceKind::Storage,
                    class::HUB => IfaceKind::Hub,
                    _ => IfaceKind::Other,
                }
            };
            let _ = s.interfaces.push(IfaceSummary {
                number: i.number,
                kind,
                has_in: i.in_ep().is_some(),
                has_out: i.out_ep().is_some(),
            });
        }
        if dev.class == class::HUB {
            s.interfaces.clear();
            let _ = s.interfaces.push(IfaceSummary { number: 0, kind: IfaceKind::Hub, has_in: true, has_out: false });
        }
        s
    }

    fn hid(&self) -> impl Iterator<Item = &Verdict> {
        self.interfaces.iter().filter_map(|i| match &i.kind {
            IfaceKind::Hid(v) => Some(v),
            _ => None,
        })
    }

    pub fn has_hid(&self) -> bool {
        self.hid().next().is_some()
    }

    /// Any interface that is neither HID nor XInput.
    pub fn has_non_hid(&self) -> bool {
        self.interfaces.iter().any(|i| !matches!(i.kind, IfaceKind::Hid(_) | IfaceKind::XInput))
    }

    pub fn has_xinput(&self) -> bool {
        self.interfaces.iter().any(|i| i.kind == IfaceKind::XInput)
    }

    pub fn all_hid_fit(&self) -> bool {
        self.has_hid() && self.hid().all(Verdict::fits)
    }

    /// IN and OUT endpoints a mirror of every HID and XInput interface needs.
    pub fn mirror_cost(&self) -> (u8, u8) {
        self.interfaces
            .iter()
            .filter(|i| matches!(i.kind, IfaceKind::Hid(_) | IfaceKind::XInput))
            .fold((0, 0), |(a, b), i| (a + i.has_in as u8, b + i.has_out as u8))
    }

    /// `ctrl::class` bits for the port summary.
    pub fn classes(&self) -> u16 {
        let mut c = 0;
        for i in &self.interfaces {
            c |= match i.kind {
                IfaceKind::Hid(v) => {
                    let mut b = 0;
                    for (hid, ctl) in [
                        (caps::KEYBOARD, cls::KEYBOARD),
                        (caps::MOUSE, cls::MOUSE),
                        (caps::CONSUMER, cls::CONSUMER),
                        (caps::GAMEPAD, cls::GAMEPAD),
                        (caps::VENDOR_INPUT, cls::VENDOR_HID),
                        (caps::VENDOR_CHANNEL, cls::VENDOR_CHANNEL),
                        (caps::OTHER | caps::DIGITIZER, cls::OTHER_HID),
                    ] {
                        if v.caps & hid != 0 {
                            b |= ctl;
                        }
                    }
                    b
                }
                IfaceKind::XInput => cls::XINPUT | cls::GAMEPAD,
                IfaceKind::Audio => cls::AUDIO | cls::NON_HID,
                IfaceKind::Video => cls::VIDEO | cls::NON_HID,
                IfaceKind::Storage => cls::STORAGE | cls::NON_HID,
                IfaceKind::Hub => cls::HUB | cls::NON_HID,
                IfaceKind::Other => cls::NON_HID,
            };
        }
        c
    }
}

/// Mirror resources left once the other ports are accounted for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Budget {
    pub free_in: u8,
    pub free_out: u8,
    pub identity_free: bool,
}

impl Budget {
    /// Everything, before any port is mirrored: EP5–EP7 IN, plus EP1–EP3 IN when `wide` (the KVM's own keyboard,
    /// mouse and media interfaces are left out), less EP7 IN while `vial` (the Vial interface is on). OUT is always
    /// EP1–EP3 and EP5–EP7: the KVM's own interfaces have no OUT endpoint there.
    pub const fn room(wide: bool, vial: bool) -> Budget {
        Budget { free_in: if wide { 6 } else { 3 } - vial as u8, free_out: 6, identity_free: true }
    }
}

/// Whether the KVM's own keyboard, mouse and media interfaces (EP1–EP3 IN) make way for mirrors: only while the
/// keyboard and mouse ports are both mirrored, no port runs Normalized (so nothing uses them), and the mirrors need
/// more IN endpoints than EP5–EP7 have. A setup that fits without it keeps those interfaces, so a device plugged in
/// later is normalized without re-enumerating the PCs. `modes`: each port's effective mode while the KVM hosts its
/// device; `vial`: the Vial interface is on.
pub fn wide(modes: [Option<PortMode>; 3], mirror_in: u8, vial: bool) -> bool {
    let mirrored = |p: Port| matches!(modes[p.index()], Some(PortMode::Mirrored | PortMode::MirroredIdentity));
    mirrored(Port::Keyboard)
        && mirrored(Port::Mouse)
        && !modes.contains(&Some(PortMode::Normalized))
        && mirror_in > Budget::room(false, vial).free_in
}

/// A port with a device, for [`decide_all`].
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    pub port: Port,
    pub setting: PortMode,
    pub dev: &'a DeviceSummary,
}

/// The effective mode of every port with a device, indexed by port. Candidates are served in order, so the ports
/// already mirrored go first and keep their endpoints against a newcomer. `vial`: the Vial interface is on.
pub fn decide_all(candidates: &[Candidate], vial: bool) -> [Option<(PortMode, AutoReason)>; 3] {
    let serve = |mut b: Budget| {
        let mut out = [None; 3];
        let mut mirror_in = 0;
        for c in candidates {
            let (mode, why) = decide(c.setting, c.port, c.dev, &b);
            if matches!(mode, PortMode::Mirrored | PortMode::MirroredIdentity) {
                let (i, o) = c.dev.mirror_cost();
                b.free_in -= i;
                b.free_out -= o;
                b.identity_free &= mode != PortMode::MirroredIdentity;
                mirror_in += i;
            }
            out[c.port.index()] = Some((mode, why));
        }
        (out, mirror_in)
    };
    // Try with EP1–EP3 first; keep that only if the result is a layout that actually leaves them out.
    let (out, mirror_in) = serve(Budget::room(true, vial));
    if wide(out.map(|d| d.map(|(mode, _)| mode)), mirror_in, vial) { out } else { serve(Budget::room(false, vial)).0 }
}

/// The effective mode of a port and why.
pub fn decide(setting: PortMode, port: Port, d: &DeviceSummary, b: &Budget) -> (PortMode, AutoReason) {
    let (need_in, need_out) = d.mirror_cost();
    let mirror_fits = need_in > 0 && need_in <= b.free_in && need_out <= b.free_out;
    let normalizable = d.hid().any(|v| v.caps & (caps::KEYBOARD | caps::MOUSE | caps::CONSUMER | caps::SYSTEM) != 0);
    match setting {
        PortMode::Auto => {
            if d.has_non_hid() || (!d.has_hid() && !d.has_xinput()) {
                (PortMode::Passthrough, AutoReason::NonHid)
            } else if d.has_xinput() {
                if mirror_fits {
                    (PortMode::Mirrored, AutoReason::XInput)
                } else {
                    (PortMode::Passthrough, AutoReason::EndpointBudget)
                }
            } else if port != Port::Other && d.all_hid_fit() {
                (PortMode::Normalized, AutoReason::FitsNormalized)
            } else if mirror_fits {
                let why = if port == Port::Other { AutoReason::OtherPortHid } else { AutoReason::ExtraInputs };
                (PortMode::Mirrored, why)
            } else if port != Port::Other && normalizable {
                // Standard keys and buttons still work; the extras are lost until endpoints free up.
                (PortMode::Normalized, AutoReason::EndpointBudget)
            } else {
                (PortMode::Passthrough, AutoReason::EndpointBudget)
            }
        }
        PortMode::Normalized if normalizable => (PortMode::Normalized, AutoReason::NotAuto),
        PortMode::Normalized => (PortMode::Passthrough, AutoReason::NonHid),
        PortMode::Mirrored | PortMode::MirroredIdentity => {
            if d.has_non_hid() {
                (PortMode::Passthrough, AutoReason::NonHid)
            } else if !mirror_fits {
                if normalizable {
                    (PortMode::Normalized, AutoReason::EndpointBudget)
                } else {
                    (PortMode::Passthrough, AutoReason::EndpointBudget)
                }
            } else if setting == PortMode::MirroredIdentity && !b.identity_free {
                (PortMode::Mirrored, AutoReason::IdentityTaken)
            } else {
                (setting, AutoReason::NotAuto)
            }
        }
        PortMode::Passthrough | PortMode::PassthroughPc1 | PortMode::PassthroughPc2 => (setting, AutoReason::NotAuto),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usbdesc::fixtures::*;
    use crate::usbdesc::parse_config;
    use kvm_hid::translate::misfit;

    fn summary(config: &[u8], verdict: impl Fn(u8) -> Option<Verdict>) -> DeviceSummary {
        let dev = DeviceDesc { vid: 1, pid: 2, ..Default::default() };
        DeviceSummary::new(&dev, &parse_config(config).unwrap(), Speed::Full, verdict)
    }

    const FITS_MOUSE: Verdict = Verdict { caps: caps::MOUSE, misfit: 0 };
    const KB_VENDOR: Verdict = Verdict { caps: caps::KEYBOARD | caps::VENDOR_INPUT, misfit: misfit::VENDOR_INPUT };
    /// EP5–EP7, no Vial.
    const ROOM: Budget = Budget::room(false, false);

    #[test]
    fn mouse_that_fits_is_normalized() {
        let d = summary(GAMING_MOUSE_CONFIG, |_| Some(FITS_MOUSE));
        assert_eq!(decide(PortMode::Auto, Port::Mouse, &d, &ROOM), (PortMode::Normalized, AutoReason::FitsNormalized));
        assert_eq!(d.mirror_cost(), (2, 1));
        // On the Other port the same device is mirrored.
        assert_eq!(decide(PortMode::Auto, Port::Other, &d, &ROOM), (PortMode::Mirrored, AutoReason::OtherPortHid));
    }

    #[test]
    fn extra_inputs_are_mirrored_until_the_budget_runs_out() {
        let d = summary(GAMING_MOUSE_CONFIG, |n| Some(if n == 0 { FITS_MOUSE } else { KB_VENDOR }));
        assert_eq!(decide(PortMode::Auto, Port::Mouse, &d, &ROOM), (PortMode::Mirrored, AutoReason::ExtraInputs));
        let tight = Budget { free_in: 1, free_out: 3, identity_free: true };
        assert_eq!(decide(PortMode::Auto, Port::Mouse, &d, &tight), (PortMode::Normalized, AutoReason::EndpointBudget));
        assert_eq!(
            decide(PortMode::MirroredIdentity, Port::Mouse, &d, &Budget { identity_free: false, ..ROOM }),
            (PortMode::Mirrored, AutoReason::IdentityTaken)
        );
        assert_eq!(d.classes(), cls::MOUSE | cls::KEYBOARD | cls::VENDOR_HID);
    }

    #[test]
    fn webcam_passes_through_and_xinput_is_mirrored() {
        let cam = summary(WEBCAM_CONFIG, |_| None);
        assert_eq!(decide(PortMode::Auto, Port::Other, &cam, &ROOM), (PortMode::Passthrough, AutoReason::NonHid));
        assert_eq!(decide(PortMode::Mirrored, Port::Other, &cam, &ROOM), (PortMode::Passthrough, AutoReason::NonHid));
        assert_eq!(
            decide(PortMode::PassthroughPc1, Port::Other, &cam, &ROOM),
            (PortMode::PassthroughPc1, AutoReason::NotAuto)
        );
        assert!(cam.classes() & cls::VIDEO != 0);

        let pad = summary(XINPUT_CONFIG, |_| None);
        assert_eq!(decide(PortMode::Auto, Port::Other, &pad, &ROOM), (PortMode::Mirrored, AutoReason::XInput));
        let none = Budget { free_in: 0, free_out: 0, identity_free: true };
        assert_eq!(
            decide(PortMode::Auto, Port::Other, &pad, &none),
            (PortMode::Passthrough, AutoReason::EndpointBudget)
        );
    }

    #[test]
    fn keyboard_and_mouse_both_mirrored_take_the_kvms_own_endpoints() {
        let dev = summary(GAMING_MOUSE_CONFIG, |_| Some(FITS_MOUSE));
        let pad = summary(XINPUT_CONFIG, |_| None);
        let c = |port, setting| Candidate { port, setting, dev: &dev };
        let mirrored = Some((PortMode::Mirrored, AutoReason::NotAuto));
        let squeezed = Some((PortMode::Normalized, AutoReason::EndpointBudget));
        // 2 + 2 IN endpoints: more than EP5–EP7 have, and nothing runs Normalized.
        let both = [c(Port::Keyboard, PortMode::Mirrored), c(Port::Mouse, PortMode::Mirrored)];
        assert_eq!(decide_all(&both, false), [mirrored, mirrored, None]);
        assert!(wide([Some(PortMode::Mirrored), Some(PortMode::Mirrored), None], 4, false));
        // With Vial on, 5 are left: an XInput pad on Other still fits.
        let with_pad = [both[0], both[1], Candidate { port: Port::Other, setting: PortMode::Auto, dev: &pad }];
        let all = decide_all(&with_pad, true);
        assert_eq!(all, [mirrored, mirrored, Some((PortMode::Mirrored, AutoReason::XInput))]);
        // A port running Normalized needs the KVM's interfaces: the port served last loses its mirror.
        let other = c(Port::Other, PortMode::Normalized);
        let normalized = Some((PortMode::Normalized, AutoReason::NotAuto));
        assert_eq!(decide_all(&[both[0], both[1], other], false), [mirrored, squeezed, normalized]);
        assert_eq!(decide_all(&[both[1], both[0], other], false), [squeezed, mirrored, normalized]);
        // So does a mouse in Auto that fits the normalized format.
        let auto = [both[0], c(Port::Mouse, PortMode::Auto)];
        assert_eq!(
            decide_all(&auto, false),
            [mirrored, Some((PortMode::Normalized, AutoReason::FitsNormalized)), None]
        );
        // A setup that fits EP5–EP7 keeps the KVM's interfaces.
        assert!(!wide([Some(PortMode::Mirrored), Some(PortMode::Mirrored), None], 3, false));
    }
}
