//! Standby and the automatic choice of the active PC (software.md §4.12).
//!
//! The AT32F405s tell what each PC does ([`Presence`]). When no PC has used the KVM for [`STANDBY_AFTER_MS`], the front
//! ports, Hub-0 and the LEDs go off ([`PowerMode::DeepStandby`]), unless the active PC sleeps and lets a key press wake
//! it: then the front ports with a keyboard or mouse stay powered ([`PowerMode::WakeStandby`]). With auto-select, the
//! other PC becomes active when the active one is switched off. Nothing here is saved: after a power loss the KVM
//! finds the running PC again from the PCs' states.

use kvm_proto::ctrl::{LinkState, Pc};
use kvm_proto::link::msg::{Status as AtStatus, UsbState};

/// No PC up for this long: standby.
pub const STANDBY_AFTER_MS: u32 = 10_000;
/// The active PC gave no 5 V for this long (a reboot may drop it briefly): the other PC becomes active.
pub const VBUS_GONE_MS: u32 = 3_000;
/// The active PC gives 5 V but has no host, or sleeps without allowing a wakeup, for this long (shut down with standby
/// power; a reboot's POST is shorter): the other PC becomes active if it is up.
pub const DORMANT_MS: u32 = 30_000;
/// At most one remote-wakeup signal per PC this often.
pub const WAKE_REPEAT_MS: u32 = 1_000;

/// What a PC does, as far as its AT32F405 can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Presence {
    /// No link to its AT32F405 (yet).
    Unknown,
    /// No 5 V from the PC: switched off, or its port is unpowered.
    NoVbus,
    /// 5 V but no host: shut down with standby power, or booting before its BIOS starts USB.
    NoHost,
    /// The PC suspended the bus (sleep, or shut down with standby power); `wake`: it allowed remote wakeup.
    Asleep { wake: bool },
    /// The host talks to the KVM.
    Up,
}

impl Presence {
    pub fn of(link: LinkState, status: Option<&AtStatus>) -> Presence {
        match (link, status) {
            (LinkState::LinkDown, _) | (_, None) => Presence::Unknown,
            (_, Some(s)) => match s.usb_state {
                UsbState::Unpowered => Presence::NoVbus,
                UsbState::Powered | UsbState::Detached => Presence::NoHost,
                UsbState::Suspended => Presence::Asleep { wake: s.wake_enabled },
                UsbState::Default | UsbState::Addressed | UsbState::Configured => Presence::Up,
            },
        }
    }

    pub fn has_vbus(self) -> bool {
        matches!(self, Presence::NoHost | Presence::Asleep { .. } | Presence::Up)
    }

    /// No host, or a sleep that a key press cannot end: the keyboard is no use to this PC.
    fn dormant(self) -> bool {
        matches!(self, Presence::NoHost | Presence::Asleep { wake: false })
    }

    /// States that count as one for the timers: dormant ones may alternate.
    fn class(self) -> u8 {
        match self {
            Presence::Unknown => 0,
            Presence::NoVbus => 1,
            p if p.dormant() => 2,
            Presence::Asleep { .. } => 3,
            _ => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum PowerMode {
    Active,
    /// No PC up, but the active one sleeps and a key press can wake it: front ports with a keyboard or mouse (or
    /// nothing yet) stay powered, Hub-0 runs, the LEDs are off.
    WakeStandby,
    /// No PC can be woken: front ports, Hub-0 and the LEDs are off.
    DeepStandby,
}

/// What the firmware powers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct PowerPlan {
    pub mode: PowerMode,
    /// Front ports that may have VBUS.
    pub ports: [bool; 3],
    /// Hub-0 out of reset.
    pub hub0: bool,
}

impl PowerPlan {
    pub const ACTIVE: PowerPlan = PowerPlan { mode: PowerMode::Active, ports: [true; 3], hub0: true };
}

/// Follows both PCs over time and decides the power plan.
pub struct Tracker {
    last: [Presence; 2],
    /// When each PC's presence last changed class.
    since: [u32; 2],
    /// The PC gave 5 V at some point since boot.
    seen_vbus: [bool; 2],
    /// Since when no PC has been up (and nothing kept the KVM awake).
    idle_since: Option<u32>,
    /// The front ports to power in wake standby, decided when standby started.
    wake_ports: [bool; 3],
    wake_sent: [Option<u32>; 2],
    plan: PowerPlan,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    pub const fn new() -> Self {
        Tracker {
            last: [Presence::Unknown; 2],
            since: [0; 2],
            seen_vbus: [false; 2],
            idle_since: None,
            wake_ports: [true; 3],
            wake_sent: [None; 2],
            plan: PowerPlan::ACTIVE,
        }
    }

    /// Records what the PCs do; call on every status and tick.
    pub fn observe(&mut self, now: u32, p: [Presence; 2]) {
        for (k, p) in p.into_iter().enumerate() {
            if p.class() != self.last[k].class() {
                self.since[k] = now;
            }
            self.last[k] = p;
            self.seen_vbus[k] |= p.has_vbus();
        }
    }

    pub fn presence(&self, pc: Pc) -> Presence {
        self.last[pc.index()]
    }

    pub fn plan(&self) -> PowerPlan {
        self.plan
    }

    /// The PC to make active instead of `active` (auto-select), if `active` is off and the other one runs:
    /// - `active` gives no 5 V for [`VBUS_GONE_MS`], or never did since boot (the KVM came up with the other PC), and
    ///   the other one gives 5 V;
    /// - `active` is dormant (no host, or asleep and not wakeable) for [`DORMANT_MS`], and the other one is up.
    pub fn auto_target(&self, now: u32, active: Pc) -> Option<Pc> {
        let (a, o) = (active.index(), active.other().index());
        let for_ms = now.wrapping_sub(self.since[a]);
        let other = self.last[o];
        let switch = match self.last[a] {
            Presence::NoVbus => other.has_vbus() && (!self.seen_vbus[a] || for_ms >= VBUS_GONE_MS),
            p if p.dormant() => other == Presence::Up && for_ms >= DORMANT_MS,
            _ => false,
        };
        switch.then_some(active.other())
    }

    /// Starts the idle time again (the KVM's key was pressed): the next [`Tracker::update`] leaves standby.
    pub fn keep_awake(&mut self, now: u32) {
        self.idle_since = Some(now);
    }

    /// The plan for now.
    /// - `wake`: config `standby-wake`; `busy`: something shows on the LEDs (identify, an update, the self-test);
    /// - `wake_ports`: the front ports worth keeping for a wakeup, used when standby starts.
    pub fn update(&mut self, now: u32, active: Pc, wake: bool, busy: bool, wake_ports: [bool; 3]) -> PowerPlan {
        let up = self.last.contains(&Presence::Up);
        let idle_ms = if up || busy {
            self.idle_since = None;
            0
        } else {
            now.wrapping_sub(*self.idle_since.get_or_insert(now))
        };
        if idle_ms < STANDBY_AFTER_MS {
            self.plan = PowerPlan::ACTIVE;
            return self.plan;
        }
        if self.plan.mode == PowerMode::Active {
            self.wake_ports = wake_ports;
        }
        self.plan = if wake && self.last[active.index()] == (Presence::Asleep { wake: true }) {
            PowerPlan { mode: PowerMode::WakeStandby, ports: self.wake_ports, hub0: true }
        } else {
            PowerPlan { mode: PowerMode::DeepStandby, ports: [false; 3], hub0: false }
        };
        self.plan
    }

    /// Whether a remote-wakeup signal for `pc` may go out now (at most one per [`WAKE_REPEAT_MS`]); records it.
    pub fn may_wake(&mut self, now: u32, pc: Pc) -> bool {
        let sent = &mut self.wake_sent[pc.index()];
        if sent.is_some_and(|t| now.wrapping_sub(t) < WAKE_REPEAT_MS) {
            return false;
        }
        *sent = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UP: Presence = Presence::Up;
    const OFF: Presence = Presence::NoVbus;
    const NO_HOST: Presence = Presence::NoHost;
    const WAKEABLE: Presence = Presence::Asleep { wake: true };
    const ASLEEP: Presence = Presence::Asleep { wake: false };

    fn tracker(p: [Presence; 2]) -> Tracker {
        let mut t = Tracker::new();
        t.observe(0, p);
        t
    }

    #[test]
    fn presence_follows_the_usb_state() {
        let st = |usb_state, wake_enabled| AtStatus {
            usb_state,
            speed: kvm_proto::link::msg::UsbSpeed::High,
            vbus_mv: 5000,
            cc1_mv: 0,
            cc2_mv: 0,
            frame: 0,
            resets: 0,
            desc_crc: 0,
            wake_enabled,
            vdda_mv: None,
            temp_dc: None,
        };
        let of = |s| Presence::of(LinkState::Connected, Some(&s));
        assert_eq!(Presence::of(LinkState::LinkDown, Some(&st(UsbState::Configured, false))), Presence::Unknown);
        assert_eq!(Presence::of(LinkState::Connected, None), Presence::Unknown);
        assert_eq!(of(st(UsbState::Unpowered, false)), OFF);
        assert_eq!(of(st(UsbState::Powered, false)), NO_HOST);
        assert_eq!(of(st(UsbState::Suspended, true)), WAKEABLE);
        assert_eq!(of(st(UsbState::Default, false)), UP);
        assert!(!OFF.has_vbus() && NO_HOST.has_vbus() && WAKEABLE.has_vbus());
    }

    #[test]
    fn auto_select_follows_the_pc_that_is_on() {
        // The KVM came up with PC2: PC1 never gave 5 V, so no wait.
        let t = tracker([OFF, NO_HOST]);
        assert_eq!(t.auto_target(0, Pc::Pc1), Some(Pc::Pc2));
        assert_eq!(t.auto_target(0, Pc::Pc2), None);
        // Neither PC on: nothing to follow.
        assert_eq!(tracker([OFF, OFF]).auto_target(99_999, Pc::Pc1), None);

        // PC1 had 5 V, then lost it: a short dip (a reboot) doesn't count.
        let mut t = tracker([UP, UP]);
        t.observe(1_000, [OFF, UP]);
        assert_eq!(t.auto_target(1_000 + VBUS_GONE_MS - 1, Pc::Pc1), None);
        assert_eq!(t.auto_target(1_000 + VBUS_GONE_MS, Pc::Pc1), Some(Pc::Pc2));
        t.observe(2_000, [UP, UP]);
        assert_eq!(t.auto_target(9_000, Pc::Pc1), None);
    }

    #[test]
    fn a_dormant_pc_gives_way_only_to_one_that_is_up() {
        let mut t = tracker([UP, UP]);
        // Shut down with standby power: no host, then a suspended bus without wakeup; one dormant stretch.
        t.observe(1_000, [NO_HOST, UP]);
        t.observe(5_000, [ASLEEP, UP]);
        assert_eq!(t.auto_target(1_000 + DORMANT_MS - 1, Pc::Pc1), None);
        assert_eq!(t.auto_target(1_000 + DORMANT_MS, Pc::Pc1), Some(Pc::Pc2));
        // Not if the other one sleeps too, nor away from a PC that a key press can wake.
        t.observe(6_000, [ASLEEP, WAKEABLE]);
        assert_eq!(t.auto_target(99_000, Pc::Pc1), None);
        t.observe(7_000, [WAKEABLE, UP]);
        assert_eq!(t.auto_target(99_000, Pc::Pc1), None);
    }

    #[test]
    fn standby_starts_after_the_idle_time_and_ends_with_a_pc() {
        let ports = [true, false, true];
        let mut t = tracker([UP, OFF]);
        assert_eq!(t.update(0, Pc::Pc1, true, false, ports), PowerPlan::ACTIVE);
        t.observe(1_000, [OFF, OFF]);
        assert_eq!(t.update(1_000, Pc::Pc1, true, false, ports).mode, PowerMode::Active);
        assert_eq!(t.update(1_000 + STANDBY_AFTER_MS - 1, Pc::Pc1, true, false, ports).mode, PowerMode::Active);
        let deep = t.update(1_000 + STANDBY_AFTER_MS, Pc::Pc1, true, false, ports);
        assert_eq!(deep, PowerPlan { mode: PowerMode::DeepStandby, ports: [false; 3], hub0: false });
        // The KVM's key wakes it for another idle time.
        t.keep_awake(20_000);
        assert_eq!(t.update(20_000, Pc::Pc1, true, false, ports), PowerPlan::ACTIVE);
        assert_eq!(t.update(20_000 + STANDBY_AFTER_MS - 1, Pc::Pc1, true, false, ports).mode, PowerMode::Active);
        assert_eq!(t.update(20_000 + STANDBY_AFTER_MS, Pc::Pc1, true, false, ports).mode, PowerMode::DeepStandby);
        // Something on the LEDs keeps it awake; a PC coming up ends standby at once.
        assert_eq!(t.update(40_000, Pc::Pc1, true, true, ports).mode, PowerMode::Active);
        t.observe(41_000, [OFF, UP]);
        assert_eq!(t.update(41_000, Pc::Pc1, true, false, ports), PowerPlan::ACTIVE);
    }

    #[test]
    fn a_wakeable_pc_keeps_the_ports_that_can_wake_it() {
        let mut t = tracker([UP, OFF]);
        t.update(0, Pc::Pc1, true, false, [true; 3]);
        t.observe(1_000, [WAKEABLE, OFF]);
        t.update(1_000, Pc::Pc1, true, false, [true; 3]);
        let plan = t.update(1_000 + STANDBY_AFTER_MS, Pc::Pc1, true, false, [true, true, false]);
        assert_eq!(plan, PowerPlan { mode: PowerMode::WakeStandby, ports: [true, true, false], hub0: true });
        // The ports were chosen when standby started; devices dropping off later change nothing.
        let plan = t.update(20_000, Pc::Pc1, true, false, [true; 3]);
        assert_eq!(plan.ports, [true, true, false]);
        // Without standby-wake, or with the sleeping PC not active, it is deep standby.
        assert_eq!(t.update(21_000, Pc::Pc1, false, false, [true; 3]).mode, PowerMode::DeepStandby);
        assert_eq!(t.update(22_000, Pc::Pc2, true, false, [true; 3]).mode, PowerMode::DeepStandby);
    }

    #[test]
    fn wakeups_are_rate_limited_per_pc() {
        let mut t = Tracker::new();
        assert!(t.may_wake(0, Pc::Pc1));
        assert!(!t.may_wake(WAKE_REPEAT_MS - 1, Pc::Pc1));
        assert!(t.may_wake(10, Pc::Pc2));
        assert!(t.may_wake(WAKE_REPEAT_MS, Pc::Pc1));
    }
}
