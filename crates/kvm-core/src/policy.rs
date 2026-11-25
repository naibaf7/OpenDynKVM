//! Port modes: what a device looks like ([`DeviceSummary`]) and which mode it gets ([`decide`], design.md §4).

use heapless::Vec;
use kvm_hid::translate::{Verdict, caps};
use kvm_proto::ctrl::{AutoReason, PORTS, Port, PortMode, Speed, class as cls};

use crate::usbdesc::{ConfigInfo, DeviceDesc, MAX_INTERFACES, class};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum IfaceKind {
    Hid(Verdict),
    XInput,
    /// A pad's interface of an XInput wireless receiver: normalized (one pad), never mirrored.
    XInputWireless,
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
            } else if i.is_xinput_wireless() {
                IfaceKind::XInputWireless
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

    /// A wireless receiver's pad interface: normalizable, though the receiver's headset interfaces keep it from
    /// being mirrored (Auto passes it through, which gives a PC every pad and the headsets).
    pub fn has_xinput_wireless(&self) -> bool {
        self.interfaces.iter().any(|i| i.kind == IfaceKind::XInputWireless)
    }

    pub fn all_hid_fit(&self) -> bool {
        self.has_hid() && self.hid().all(Verdict::fits)
    }

    /// The KVM can read it into its own keyboard, mouse and gamepad: keys, buttons, axes or media keys on a HID
    /// interface, gamepads too, since the mapping engine reads them (hid_mapping.md): to the virtual gamepad, or as the
    /// keymap says. XInput pads, wired or behind a wireless receiver, are decoded without a descriptor
    /// (`kvm_hid::xinput`).
    pub fn normalizable(&self) -> bool {
        self.hid().any(|v| v.caps & (caps::KEYBOARD | caps::MOUSE | caps::CONSUMER | caps::SYSTEM | caps::GAMEPAD) != 0)
            || self.has_xinput()
            || self.has_xinput_wireless()
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
                IfaceKind::XInput | IfaceKind::XInputWireless => cls::XINPUT | cls::GAMEPAD,
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
/// device; `wireless`: a Bluetooth device is normalized (it needs the KVM's interfaces as a port would); `vial`: the
/// Vial interface is on.
pub fn wide(modes: [Option<PortMode>; PORTS], mirror_in: u8, wireless: bool, vial: bool) -> bool {
    let mirrored = |p: Port| matches!(modes[p.index()], Some(PortMode::Mirrored | PortMode::MirroredIdentity));
    mirrored(Port::Keyboard)
        && mirrored(Port::Mouse)
        && !modes.contains(&Some(PortMode::Normalized))
        && !wireless
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
/// already mirrored go first and keep their endpoints against a newcomer. `wireless`: a Bluetooth device is
/// normalized; `vial`: the Vial interface is on.
pub fn decide_all(candidates: &[Candidate], wireless: bool, vial: bool) -> [Option<(PortMode, AutoReason)>; PORTS] {
    let serve = |mut b: Budget| {
        let mut out = [None; PORTS];
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
    if wide(out.map(|d| d.map(|(mode, _)| mode)), mirror_in, wireless, vial) {
        out
    } else {
        serve(Budget::room(false, vial)).0
    }
}

/// The effective mode of a port and why.
pub fn decide(setting: PortMode, port: Port, d: &DeviceSummary, b: &Budget) -> (PortMode, AutoReason) {
    match decide_routed(setting, port, d, b) {
        // Aux 1 has no router (design.md §4): what would go to a PC's own USB runs Normalized where it can, else off.
        (PortMode::Passthrough, _) if !port.has_router() => {
            (if d.normalizable() { PortMode::Normalized } else { PortMode::Off }, AutoReason::NeedsPassthrough)
        }
        decided => decided,
    }
}

/// [`decide`] for a port with a router: Passthrough is always there to fall back to.
fn decide_routed(setting: PortMode, port: Port, d: &DeviceSummary, b: &Budget) -> (PortMode, AutoReason) {
    let (need_in, need_out) = d.mirror_cost();
    let mirror_fits = need_in > 0 && need_in <= b.free_in && need_out <= b.free_out;
    let normalizable = d.normalizable();
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
            } else if !port.is_aux() && d.all_hid_fit() {
                (PortMode::Normalized, AutoReason::FitsNormalized)
            } else if mirror_fits {
                let why = if port.is_aux() { AutoReason::AuxPortHid } else { AutoReason::ExtraInputs };
                (PortMode::Mirrored, why)
            } else if !port.is_aux() && normalizable {
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
        PortMode::Passthrough | PortMode::Off => (setting, AutoReason::NotAuto),
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
        // On the Aux ports the same device is mirrored.
        assert_eq!(decide(PortMode::Auto, Port::Aux2, &d, &ROOM), (PortMode::Mirrored, AutoReason::AuxPortHid));
        assert_eq!(decide(PortMode::Auto, Port::Aux1, &d, &ROOM), (PortMode::Mirrored, AutoReason::AuxPortHid));
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
        assert_eq!(decide(PortMode::Auto, Port::Aux2, &cam, &ROOM), (PortMode::Passthrough, AutoReason::NonHid));
        assert_eq!(decide(PortMode::Mirrored, Port::Aux2, &cam, &ROOM), (PortMode::Passthrough, AutoReason::NonHid));
        assert_eq!(
            decide(PortMode::Passthrough, Port::Aux2, &cam, &ROOM),
            (PortMode::Passthrough, AutoReason::NotAuto)
        );
        assert_eq!(decide(PortMode::Off, Port::Aux2, &cam, &ROOM), (PortMode::Off, AutoReason::NotAuto));
        assert!(cam.classes() & cls::VIDEO != 0);

        let pad = summary(XINPUT_CONFIG, |_| None);
        assert_eq!(decide(PortMode::Auto, Port::Aux2, &pad, &ROOM), (PortMode::Mirrored, AutoReason::XInput));
        let none = Budget { free_in: 0, free_out: 0, identity_free: true };
        assert_eq!(
            decide(PortMode::Auto, Port::Aux2, &pad, &none),
            (PortMode::Passthrough, AutoReason::EndpointBudget)
        );
    }

    #[test]
    fn a_wireless_receiver_passes_through_unless_normalized() {
        let rx = summary(RECEIVER_CONFIG, |_| None);
        assert!(rx.has_xinput_wireless() && rx.classes() & cls::GAMEPAD != 0);
        // Auto and mirrored: the PC gets it all (four pads, the headsets); Normalized: the KVM reads one pad.
        assert_eq!(decide(PortMode::Auto, Port::Aux2, &rx, &ROOM), (PortMode::Passthrough, AutoReason::NonHid));
        assert_eq!(decide(PortMode::Mirrored, Port::Aux2, &rx, &ROOM), (PortMode::Passthrough, AutoReason::NonHid));
        assert_eq!(decide(PortMode::Normalized, Port::Aux2, &rx, &ROOM), (PortMode::Normalized, AutoReason::NotAuto));
        // A wired pad: mirrored by Auto as before, and now normalizable too.
        let pad = summary(XINPUT_CONFIG, |_| None);
        assert_eq!(decide(PortMode::Normalized, Port::Aux2, &pad, &ROOM), (PortMode::Normalized, AutoReason::NotAuto));
    }

    #[test]
    fn aux1_never_passes_through() {
        let needs = AutoReason::NeedsPassthrough;
        // A webcam has nothing the KVM can read: it stays off, whatever the port is set to.
        let cam = summary(WEBCAM_CONFIG, |_| None);
        for setting in [PortMode::Auto, PortMode::Mirrored, PortMode::Normalized, PortMode::Passthrough] {
            assert_eq!(decide(setting, Port::Aux1, &cam, &ROOM), (PortMode::Off, needs), "{setting:?}");
        }
        assert_eq!(decide(PortMode::Off, Port::Aux1, &cam, &ROOM), (PortMode::Off, AutoReason::NotAuto));
        // A wireless receiver that Auto passes through elsewhere: the KVM reads its pad.
        let rx = summary(RECEIVER_CONFIG, |_| None);
        assert_eq!(decide(PortMode::Auto, Port::Aux1, &rx, &ROOM), (PortMode::Normalized, needs));
        // An XInput pad is mirrored while endpoints are left, then normalized instead of passed through.
        let pad = summary(XINPUT_CONFIG, |_| None);
        assert_eq!(decide(PortMode::Auto, Port::Aux1, &pad, &ROOM), (PortMode::Mirrored, AutoReason::XInput));
        let none = Budget { free_in: 0, free_out: 0, identity_free: true };
        assert_eq!(decide(PortMode::Auto, Port::Aux1, &pad, &none), (PortMode::Normalized, needs));
        // A device with extra inputs that no longer fits a mirror, likewise.
        let mouse = summary(GAMING_MOUSE_CONFIG, |n| Some(if n == 0 { FITS_MOUSE } else { KB_VENDOR }));
        assert_eq!(decide(PortMode::Auto, Port::Aux1, &mouse, &none), (PortMode::Normalized, needs));
        assert_eq!(
            decide(PortMode::Auto, Port::Aux2, &mouse, &none),
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
        assert_eq!(decide_all(&both, false, false), [mirrored, mirrored, None, None]);
        assert!(wide([Some(PortMode::Mirrored), Some(PortMode::Mirrored), None, None], 4, false, false));
        // A normalized Bluetooth keyboard needs the KVM's interfaces as a port's would.
        assert_eq!(decide_all(&both, true, false), [mirrored, squeezed, None, None]);
        // With Vial on, 5 are left: an XInput pad on Aux 2 still fits.
        let with_pad = [both[0], both[1], Candidate { port: Port::Aux2, setting: PortMode::Auto, dev: &pad }];
        let all = decide_all(&with_pad, false, true);
        assert_eq!(all, [mirrored, mirrored, None, Some((PortMode::Mirrored, AutoReason::XInput))]);
        // A port running Normalized needs the KVM's interfaces: the port served last loses its mirror.
        let other = c(Port::Aux2, PortMode::Normalized);
        let normalized = Some((PortMode::Normalized, AutoReason::NotAuto));
        assert_eq!(decide_all(&[both[0], both[1], other], false, false), [mirrored, squeezed, None, normalized]);
        assert_eq!(decide_all(&[both[1], both[0], other], false, false), [squeezed, mirrored, None, normalized]);
        // So does a mouse in Auto that fits the normalized format.
        let auto = [both[0], c(Port::Mouse, PortMode::Auto)];
        assert_eq!(
            decide_all(&auto, false, false),
            [mirrored, Some((PortMode::Normalized, AutoReason::FitsNormalized)), None, None]
        );
        // A setup that fits EP5–EP7 keeps the KVM's interfaces.
        assert!(!wide([Some(PortMode::Mirrored), Some(PortMode::Mirrored), None, None], 3, false, false));
    }
}
