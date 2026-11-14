//! Persistent configuration (software.md §8.3) and its flash item (§4.8).
//!
//! Item ([`ITEM`] in [`crate::store`], which adds the CRC): a list of entries `key u16 | len u8 | value`. Unknown keys
//! are skipped, so a newer firmware's item still loads in an older one.
//!
//! The screen-hop arrangements (protocol 9) are items of their own ([`ARRANGEMENT_ITEM`] + n, the same entries: the
//! arrangement's name and its screens), so that editing one writes only it, and switching profiles writes nothing.
//! Firmware up to 0.1.12 kept its one arrangement in the config item, under the keys that are arrangement 0's now: such
//! an item loads into arrangement 0, which is written as an item of its own at the next save, before the config item
//! that no longer holds it.

use core::ops::RangeInclusive;

use kvm_proto::ctrl::{
    Accel, BT_HOSTS, BtPointer, BtPolicy, ConfigValue, ErrorCode, HOP_CORNER_MAX, HOP_RESISTANCE_MAX, HOP_SPEED, HOSTS,
    Host, Hotkey, KeyAction, MAX_ARRANGEMENTS, MAX_PROFILES, MAX_SCREENS, Mapping, PROFILE_NAME_MAX, Polling, Port,
    PortMode, ProfileName, Rgb, Screen, Units, VialMode, key,
};
use kvm_proto::wire::{Reader, Writer};

pub use crate::hop::Layout;

/// Store key of the config.
pub const ITEM: u16 = 0x0001;
/// Room for every key with margin.
/// 0.16.0 added the Bluetooth settings (a hotkey, four names, four policies): up to 83 bytes more.
/// Screen hop added the absolute pointer, its settings, four profile switches and eight screens: 194 bytes more.
/// Each PC's units for screen hop (protocol 8): 8 bytes more.
/// Arrangements in items of their own (protocol 9): the eight screens out (144 bytes), the Bluetooth hosts' mapping,
/// units and pointer in (48 bytes).
pub const ITEM_MAX: usize = 576;
/// Store key of arrangement 0's item; arrangement n's is this + n.
pub const ARRANGEMENT_ITEM: u16 = 0x0010;
/// An arrangement's item at its longest: its name (key, length, then the name with its own length) and every screen
/// slot used (key, length, 15 bytes).
pub const ARRANGEMENT_ITEM_MAX: usize = (4 + PROFILE_NAME_MAX) + MAX_SCREENS * (3 + 15);
/// The longest value of an entry (a screen: 15 bytes).
const VALUE_MAX: usize = 22;
/// A press on the KVM's key released before this is a tap.
pub const TAP_MAX_MS: u16 = 600;
/// Long-press durations the config accepts: clear of a tap, short of a wait.
pub const LONG_PRESS_RANGE_MS: RangeInclusive<u16> = 800..=3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Config {
    pub hotkey_toggle: Hotkey,
    pub hotkey_pc1: Hotkey,
    pub hotkey_pc2: Hotkey,
    pub double_tap_ms: u16,
    pub led_brightness: u8,
    pub color_pc1: Rgb,
    pub color_pc2: Rgb,
    pub polling: Polling,
    pub power_cycle_on_switch: bool,
    pub sw_switch_allowed: bool,
    pub sw_config_allowed: bool,
    pub vial: VialMode,
    pub keymap: bool,
    pub auto_select: bool,
    pub standby_wake: bool,
    pub standby_glow: bool,
    pub key_tap: KeyAction,
    pub key_long: KeyAction,
    pub long_press_ms: u16,
    /// The active profile, one of the first `profile_count`; its port modes are the ones in force.
    pub profile: u8,
    pub profile_count: u8,
    /// Power up in the profile last active; otherwise (the default) in profile 0, and choosing a profile writes nothing
    /// to flash. The KVM loses power only when both PCs are off (or it is unplugged), so starting over in the first
    /// profile is what one expects, and it saves a flash write per profile change.
    pub remember_profile: bool,
    pub profiles: [Profile; MAX_PROFILES],
    /// Hotkey for the next target (PC1, PC2, the Bluetooth hosts that are ready).
    pub hotkey_next: Hotkey,
    /// Names of the Bluetooth host slots 1–4 (empty: "BT n").
    pub bt_names: [ProfileName; BT_HOSTS],
    /// What the Bluetooth module does with each host slot while another target is active.
    pub bt_policies: [BtPolicy; BT_HOSTS],
    /// The KVM's mouse interface also has an absolute pointer (screen hop needs it).
    pub abs_pointer: bool,
    pub hop: HopConfig,
    /// The screen-hop arrangements; each profile uses one of them or none.
    pub arrangements: [Arrangement; MAX_ARRANGEMENTS],
}

/// A screen-hop arrangement: a name (empty: the hosts call it "Arrangement n") and its screens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Arrangement {
    pub name: ProfileName,
    pub screens: Layout,
}

impl Arrangement {
    pub const EMPTY: Arrangement = Arrangement { name: ProfileName::new(""), screens: [Screen::NONE; MAX_SCREENS] };
}

/// How screen hop moves the pointer (software.md §4.14); each profile turns it on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct HopConfig {
    pub accel: Accel,
    pub speed: i8,
    pub resistance: u16,
    pub corner: u16,
    /// How each host (`Host::index`) maps the absolute pointer onto its screens.
    pub mapping: [Mapping; HOSTS],
    /// What each host's desktop coordinates count.
    pub units: [Units; HOSTS],
    /// How screen hop moves each Bluetooth host's pointer.
    pub bt_pointer: [BtPointer; BT_HOSTS],
}

/// A set of port modes to switch between with the key, e.g. emulated for work and passthrough for games.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Profile {
    pub name: ProfileName,
    /// Keyboard, Mouse, Other.
    pub port_modes: [PortMode; 3],
    /// The arrangement screen hop moves the pointer across in this profile; None: screen hop is off.
    pub arrangement: Option<u8>,
}

impl Profile {
    const fn new(name: &str, mode: PortMode) -> Profile {
        Profile { name: ProfileName::new(name), port_modes: [mode; 3], arrangement: None }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Config {
    pub const DEFAULT: Config = Config {
        hotkey_toggle: Hotkey::double_tap(kvm_hid::usage::key::SCROLL_LOCK, true),
        hotkey_pc1: Hotkey::NONE,
        hotkey_pc2: Hotkey::NONE,
        double_tap_ms: 300,
        // Dim: at full brightness the six LEDs can draw 0.22 A of a PC port's budget (analysis.md finding 6).
        led_brightness: 32,
        color_pc1: Rgb::new(0, 90, 255),
        color_pc2: Rgb::new(255, 110, 0),
        polling: Polling::Us125,
        power_cycle_on_switch: false,
        sw_switch_allowed: true,
        sw_config_allowed: true,
        // Opt-in: the interface costs a mirror endpoint and changes the serial number.
        vial: VialMode::Off,
        // A fresh keymap changes nothing, so it can always apply.
        keymap: true,
        // Standby (software.md §4.12): the keyboard follows the PC that is on, can wake a sleeping one, and the LEDs
        // go dark.
        auto_select: true,
        standby_wake: true,
        standby_glow: false,
        // A tap switches PCs, a long press goes from the emulated devices to passthrough and back.
        key_tap: KeyAction::TogglePc,
        key_long: KeyAction::NextProfile,
        long_press_ms: 1000,
        profile: 0,
        profile_count: 2,
        remember_profile: false,
        profiles: [
            Profile::new("work", PortMode::Auto),
            Profile::new("gaming", PortMode::Passthrough),
            Profile::new("profile 2", PortMode::Auto),
            Profile::new("profile 3", PortMode::Auto),
        ],
        hotkey_next: Hotkey::NONE,
        bt_names: [ProfileName::new(""); BT_HOSTS],
        bt_policies: [BtPolicy::StayConnected; BT_HOSTS],
        // Opt-in: it changes the KVM's mouse interface, so both PCs reconnect it once.
        abs_pointer: false,
        hop: HopConfig {
            accel: Accel::Precision,
            speed: 0,
            // A deliberate push, and the corners left for the Start button and window controls.
            resistance: 100,
            corner: 20,
            mapping: [Mapping::Desktop; HOSTS],
            units: [Units::Physical; HOSTS],
            bt_pointer: [BtPointer::Absolute; BT_HOSTS],
        },
        arrangements: [Arrangement::EMPTY; MAX_ARRANGEMENTS],
    };

    /// The port modes in force: the active profile's.
    pub fn port_modes(&self) -> [PortMode; 3] {
        self.profiles[self.profile as usize].port_modes
    }

    /// The profile after the active one, among those in use.
    pub fn next_profile(&self) -> u8 {
        (self.profile + 1) % self.profile_count
    }

    /// The arrangement of the active profile (None: screen hop is off in it).
    pub fn arrangement(&self) -> Option<u8> {
        self.profiles[self.profile as usize].arrangement
    }

    /// Screen hop is on in the active profile.
    pub fn hop_on(&self) -> bool {
        self.arrangement().is_some()
    }

    /// The screens of the active profile's arrangement; none with screen hop off.
    pub fn layout(&self) -> &Layout {
        const NONE: Layout = [Screen::NONE; MAX_SCREENS];
        match self.arrangement() {
            Some(a) => &self.arrangements[a as usize].screens,
            None => &NONE,
        }
    }

    pub fn mapping(&self, h: Host) -> Mapping {
        self.hop.mapping[h.index()]
    }

    pub fn units(&self, h: Host) -> Units {
        self.hop.units[h.index()]
    }

    /// How screen hop moves `h`'s pointer: a PC's is always absolute.
    pub fn bt_pointer(&self, h: Host) -> BtPointer {
        h.bt_slot().map_or(BtPointer::Absolute, |n| self.hop.bt_pointer[n as usize - 1])
    }
}

/// Who is changing the config: the lock bits can only be turned on at the device itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Origin {
    Device,
    Software,
}

impl Config {
    pub fn get(&self, k: u16) -> Option<ConfigValue> {
        Some(match k {
            key::HOTKEY_TOGGLE => ConfigValue::HotkeyToggle(self.hotkey_toggle),
            key::HOTKEY_PC1 => ConfigValue::HotkeyPc1(self.hotkey_pc1),
            key::HOTKEY_PC2 => ConfigValue::HotkeyPc2(self.hotkey_pc2),
            key::DOUBLE_TAP_MS => ConfigValue::DoubleTapMs(self.double_tap_ms),
            key::LED_BRIGHTNESS => ConfigValue::LedBrightness(self.led_brightness),
            key::COLOR_PC1 => ConfigValue::ColorPc1(self.color_pc1),
            key::COLOR_PC2 => ConfigValue::ColorPc2(self.color_pc2),
            key::PC_POLLING => ConfigValue::PcPolling(self.polling),
            key::POWER_CYCLE_ON_SWITCH => ConfigValue::PowerCycleOnSwitch(self.power_cycle_on_switch),
            k if (key::PORT_MODE_BASE..key::PORT_MODE_BASE + 3).contains(&k) => {
                let p = Port::from_index((k - key::PORT_MODE_BASE) as usize)?;
                ConfigValue::PortMode(p, self.port_modes()[p.index()])
            }
            key::SW_SWITCH_ALLOWED => ConfigValue::SwSwitchAllowed(self.sw_switch_allowed),
            key::SW_CONFIG_ALLOWED => ConfigValue::SwConfigAllowed(self.sw_config_allowed),
            key::VIAL => ConfigValue::Vial(self.vial),
            key::KEYMAP => ConfigValue::Keymap(self.keymap),
            key::AUTO_SELECT => ConfigValue::AutoSelect(self.auto_select),
            key::STANDBY_WAKE => ConfigValue::StandbyWake(self.standby_wake),
            key::STANDBY_GLOW => ConfigValue::StandbyGlow(self.standby_glow),
            key::KEY_TAP => ConfigValue::KeyTap(self.key_tap),
            key::KEY_LONG => ConfigValue::KeyLong(self.key_long),
            key::LONG_PRESS_MS => ConfigValue::LongPressMs(self.long_press_ms),
            key::REMEMBER_PROFILE => ConfigValue::RememberProfile(self.remember_profile),
            key::PROFILE => ConfigValue::Profile(self.profile),
            key::PROFILE_COUNT => ConfigValue::ProfileCount(self.profile_count),
            k if (key::PROFILE_MODES_BASE..key::PROFILE_MODES_BASE + MAX_PROFILES as u16).contains(&k) => {
                let i = k - key::PROFILE_MODES_BASE;
                ConfigValue::ProfileModes(i as u8, self.profiles[i as usize].port_modes)
            }
            k if (key::PROFILE_NAME_BASE..key::PROFILE_NAME_BASE + MAX_PROFILES as u16).contains(&k) => {
                let i = k - key::PROFILE_NAME_BASE;
                ConfigValue::ProfileName(i as u8, self.profiles[i as usize].name)
            }
            key::HOTKEY_NEXT => ConfigValue::HotkeyNext(self.hotkey_next),
            k if (key::BT_NAME_BASE..key::BT_NAME_BASE + BT_HOSTS as u16).contains(&k) => {
                let i = k - key::BT_NAME_BASE;
                ConfigValue::BtName(i as u8, self.bt_names[i as usize])
            }
            k if (key::BT_POLICY_BASE..key::BT_POLICY_BASE + BT_HOSTS as u16).contains(&k) => {
                let i = k - key::BT_POLICY_BASE;
                ConfigValue::BtPolicy(i as u8, self.bt_policies[i as usize])
            }
            key::ABS_POINTER => ConfigValue::AbsPointer(self.abs_pointer),
            key::HOP_ACCEL => ConfigValue::HopAccel(self.hop.accel),
            key::HOP_SPEED => ConfigValue::HopSpeed(self.hop.speed),
            key::HOP_RESISTANCE => ConfigValue::HopResistance(self.hop.resistance),
            key::HOP_CORNER => ConfigValue::HopCorner(self.hop.corner),
            k if let Some(h) = Host::ALL.iter().copied().find(|h| key::hop_mapping(*h) == k) => {
                ConfigValue::HopMapping(h, self.mapping(h))
            }
            k if let Some(h) = Host::ALL.iter().copied().find(|h| key::hop_units(*h) == k) => {
                ConfigValue::HopUnits(h, self.units(h))
            }
            k if (key::BT_POINTER_BASE..key::BT_POINTER_BASE + BT_HOSTS as u16).contains(&k) => {
                let i = k - key::BT_POINTER_BASE;
                ConfigValue::BtPointer(i as u8, self.hop.bt_pointer[i as usize])
            }
            k if (key::PROFILE_ARRANGEMENT_BASE..key::PROFILE_ARRANGEMENT_BASE + MAX_PROFILES as u16).contains(&k) => {
                let i = k - key::PROFILE_ARRANGEMENT_BASE;
                ConfigValue::ProfileArrangement(i as u8, self.profiles[i as usize].arrangement)
            }
            k if (key::ARRANGEMENT_NAME_BASE..key::ARRANGEMENT_NAME_BASE + MAX_ARRANGEMENTS as u16).contains(&k) => {
                let a = k - key::ARRANGEMENT_NAME_BASE;
                ConfigValue::ArrangementName(a as u8, self.arrangements[a as usize].name)
            }
            k if let Some(a) = key::arrangement_of(k) => {
                let i = (k - key::screen(a, 0)) as usize;
                ConfigValue::Screen(a, i as u8, self.arrangements[a as usize].screens[i])
            }
            _ => return None,
        })
    }

    pub fn all(&self) -> impl Iterator<Item = ConfigValue> + '_ {
        key::ALL.iter().filter_map(|k| self.get(*k))
    }

    /// Validates and applies `v`; returns the value as stored.
    pub fn set(&mut self, v: ConfigValue, origin: Origin) -> Result<ConfigValue, ErrorCode> {
        let valid_hotkey = |h: &Hotkey| match h.kind {
            kvm_proto::ctrl::HotkeyKind::None => true,
            _ => h.key > 3,
        };
        match v {
            ConfigValue::HotkeyToggle(h)
            | ConfigValue::HotkeyPc1(h)
            | ConfigValue::HotkeyPc2(h)
            | ConfigValue::HotkeyNext(h)
                if !valid_hotkey(&h) =>
            {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::DoubleTapMs(ms) if !(100..=1000).contains(&ms) => return Err(ErrorCode::InvalidValue),
            ConfigValue::LongPressMs(ms) if !LONG_PRESS_RANGE_MS.contains(&ms) => return Err(ErrorCode::InvalidValue),
            ConfigValue::Profile(i) if i >= self.profile_count => return Err(ErrorCode::InvalidValue),
            ConfigValue::ProfileCount(n) if !(1..=MAX_PROFILES as u8).contains(&n) => {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::ProfileModes(i, _) | ConfigValue::ProfileName(i, _) if i as usize >= MAX_PROFILES => {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::BtName(i, _) | ConfigValue::BtPolicy(i, _) if i as usize >= BT_HOSTS => {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::ProfileArrangement(i, a)
                if i as usize >= MAX_PROFILES || a.is_some_and(|a| a as usize >= MAX_ARRANGEMENTS) =>
            {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::ArrangementName(a, _) if a as usize >= MAX_ARRANGEMENTS => {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::BtPointer(i, _) if i as usize >= BT_HOSTS => return Err(ErrorCode::InvalidValue),
            ConfigValue::HopSpeed(v) if !HOP_SPEED.contains(&v) => return Err(ErrorCode::InvalidValue),
            ConfigValue::HopResistance(v) if v > HOP_RESISTANCE_MAX => return Err(ErrorCode::InvalidValue),
            ConfigValue::HopCorner(v) if v > HOP_CORNER_MAX => return Err(ErrorCode::InvalidValue),
            ConfigValue::Screen(a, i, s)
                if a as usize >= MAX_ARRANGEMENTS || i as usize >= MAX_SCREENS || !s.is_valid() =>
            {
                return Err(ErrorCode::InvalidValue);
            }
            ConfigValue::SwSwitchAllowed(true) | ConfigValue::SwConfigAllowed(true) if origin == Origin::Software => {
                return Err(ErrorCode::Locked);
            }
            _ => {}
        }
        match v {
            ConfigValue::HotkeyToggle(h) => self.hotkey_toggle = h,
            ConfigValue::HotkeyPc1(h) => self.hotkey_pc1 = h,
            ConfigValue::HotkeyPc2(h) => self.hotkey_pc2 = h,
            ConfigValue::DoubleTapMs(ms) => self.double_tap_ms = ms,
            ConfigValue::LedBrightness(b) => self.led_brightness = b,
            ConfigValue::ColorPc1(c) => self.color_pc1 = c,
            ConfigValue::ColorPc2(c) => self.color_pc2 = c,
            ConfigValue::PcPolling(p) => self.polling = p,
            ConfigValue::PowerCycleOnSwitch(b) => self.power_cycle_on_switch = b,
            // The port modes are the active profile's.
            ConfigValue::PortMode(p, m) => self.profiles[self.profile as usize].port_modes[p.index()] = m,
            ConfigValue::SwSwitchAllowed(b) => self.sw_switch_allowed = b,
            ConfigValue::SwConfigAllowed(b) => self.sw_config_allowed = b,
            ConfigValue::Vial(m) => self.vial = m,
            ConfigValue::Keymap(b) => self.keymap = b,
            ConfigValue::AutoSelect(b) => self.auto_select = b,
            ConfigValue::StandbyWake(b) => self.standby_wake = b,
            ConfigValue::StandbyGlow(b) => self.standby_glow = b,
            ConfigValue::KeyTap(a) => self.key_tap = a,
            ConfigValue::KeyLong(a) => self.key_long = a,
            ConfigValue::LongPressMs(ms) => self.long_press_ms = ms,
            ConfigValue::RememberProfile(b) => self.remember_profile = b,
            ConfigValue::Profile(i) => self.profile = i,
            ConfigValue::ProfileCount(n) => {
                self.profile_count = n;
                if self.profile >= n {
                    self.profile = 0;
                }
            }
            ConfigValue::ProfileModes(i, m) => self.profiles[i as usize].port_modes = m,
            ConfigValue::ProfileName(i, n) => self.profiles[i as usize].name = n,
            ConfigValue::HotkeyNext(h) => self.hotkey_next = h,
            ConfigValue::BtName(i, n) => self.bt_names[i as usize] = n,
            ConfigValue::BtPolicy(i, p) => self.bt_policies[i as usize] = p,
            ConfigValue::AbsPointer(b) => self.abs_pointer = b,
            ConfigValue::HopAccel(a) => self.hop.accel = a,
            ConfigValue::HopSpeed(v) => self.hop.speed = v,
            ConfigValue::HopResistance(v) => self.hop.resistance = v,
            ConfigValue::HopCorner(v) => self.hop.corner = v,
            ConfigValue::HopMapping(h, m) => self.hop.mapping[h.index()] = m,
            ConfigValue::HopUnits(h, u) => self.hop.units[h.index()] = u,
            ConfigValue::BtPointer(i, p) => self.hop.bt_pointer[i as usize] = p,
            ConfigValue::ProfileArrangement(i, a) => self.profiles[i as usize].arrangement = a,
            ConfigValue::ArrangementName(a, n) => self.arrangements[a as usize].name = n,
            ConfigValue::Screen(a, i, s) => {
                // A free slot is stored as such, whatever else came with it.
                let s = if s.host.is_some() { s } else { Screen::NONE };
                self.arrangements[a as usize].screens[i as usize] = s;
                return Ok(ConfigValue::Screen(a, i, s));
            }
        }
        Ok(v)
    }

    /// Encodes the flash item, without the arrangements (their own items); returns its length.
    pub fn encode(&self, out: &mut [u8; ITEM_MAX]) -> usize {
        let mut w = Writer::new(out);
        for v in self.all().filter(|v| key::arrangement_of(v.key()).is_none()) {
            entry(&mut w, &v);
        }
        w.len()
    }

    /// Decodes an item; entries that don't parse keep their default. An item from firmware up to 0.1.12 brings its
    /// one arrangement, as arrangement 0.
    pub fn decode(item: &[u8]) -> Config {
        let mut cfg = Config::default();
        cfg.apply(item, |_| true);
        if !cfg.remember_profile {
            cfg.profile = 0;
        }
        cfg
    }

    /// Encodes arrangement `a`'s item: its name and the screen slots in use (a free slot is the default). Returns its
    /// length.
    pub fn encode_arrangement(&self, a: u8, out: &mut [u8; ARRANGEMENT_ITEM_MAX]) -> usize {
        let arr = &self.arrangements[a as usize];
        let mut w = Writer::new(out);
        entry(&mut w, &ConfigValue::ArrangementName(a, arr.name));
        for (i, s) in arr.screens.iter().enumerate().filter(|(_, s)| s.host.is_some()) {
            entry(&mut w, &ConfigValue::Screen(a, i as u8, *s));
        }
        w.len()
    }

    /// Arrangement `a`'s item, read from flash: it replaces what the config item brought for it.
    pub fn restore_arrangement(&mut self, a: u8, item: &[u8]) {
        if a as usize >= MAX_ARRANGEMENTS {
            return;
        }
        self.arrangements[a as usize] = Arrangement::EMPTY;
        self.apply(item, |k| key::arrangement_of(k) == Some(a));
    }

    /// Sets every entry of `item` whose key `want`s.
    fn apply(&mut self, item: &[u8], want: impl Fn(u16) -> bool) {
        let mut er = Reader::new(item);
        while er.remaining() > 0 {
            let (Ok(k), Ok(val)) = (er.u16(), er.lv()) else { break };
            if val.len() > VALUE_MAX || !want(k) {
                continue;
            }
            let mut tmp = [0u8; 2 + VALUE_MAX];
            tmp[..2].copy_from_slice(&k.to_le_bytes());
            tmp[2..2 + val.len()].copy_from_slice(val);
            if let Ok(v) = ConfigValue::decode(&mut Reader::new(&tmp[..2 + val.len()])) {
                // Stored values were validated when set; the device origin keeps the lock bits as stored.
                let _ = self.set(v, Origin::Device);
            }
        }
    }
}

/// One item entry: `key u16 | len u8 | value`.
fn entry(w: &mut Writer, v: &ConfigValue) {
    let mut tmp = [0u8; 2 + VALUE_MAX];
    let mut tw = Writer::new(&mut tmp);
    // A value is at most a few bytes; these writes cannot overflow.
    let _ = v.encode(&mut tw);
    let enc = tw.finish();
    let _ = w.u16(v.key());
    let _ = w.lv(&enc[2..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_roundtrip() {
        let mut c = Config::default();
        c.set(ConfigValue::PortMode(Port::Other, PortMode::PassthroughPc1), Origin::Software).unwrap();
        c.set(ConfigValue::ColorPc2(Rgb::new(9, 8, 7)), Origin::Software).unwrap();
        c.set(ConfigValue::SwSwitchAllowed(false), Origin::Software).unwrap();
        c.set(ConfigValue::Vial(VialMode::Auto), Origin::Software).unwrap();
        c.set(ConfigValue::AutoSelect(false), Origin::Software).unwrap();
        c.set(ConfigValue::StandbyGlow(true), Origin::Software).unwrap();
        c.set(ConfigValue::KeyLong(KeyAction::OtherPortMode), Origin::Software).unwrap();
        // The last profile active (remembered), every name as long as it can be: the item still fits.
        c.set(ConfigValue::RememberProfile(true), Origin::Software).unwrap();
        c.set(ConfigValue::ProfileCount(4), Origin::Software).unwrap();
        c.set(ConfigValue::Profile(3), Origin::Software).unwrap();
        c.set(ConfigValue::PortMode(Port::Mouse, PortMode::Mirrored), Origin::Software).unwrap();
        for i in 0..4 {
            c.set(ConfigValue::ProfileName(i, ProfileName::new("twelve bytes")), Origin::Software).unwrap();
            c.set(ConfigValue::ProfileArrangement(i, (i % 2 == 0).then_some(3 - i)), Origin::Software).unwrap();
        }
        // Screen hop, with every host's settings changed and every screen slot of every arrangement used.
        c.set(ConfigValue::AbsPointer(true), Origin::Software).unwrap();
        c.set(ConfigValue::HopAccel(Accel::Adaptive), Origin::Software).unwrap();
        c.set(ConfigValue::HopSpeed(-3), Origin::Software).unwrap();
        c.set(ConfigValue::HopResistance(400), Origin::Software).unwrap();
        c.set(ConfigValue::HopCorner(0), Origin::Software).unwrap();
        for h in Host::ALL.iter().copied() {
            c.set(ConfigValue::HopMapping(h, Mapping::MainScreen), Origin::Software).unwrap();
            c.set(ConfigValue::HopUnits(h, Units::Logical), Origin::Software).unwrap();
        }
        for i in 0..BT_HOSTS as u8 {
            c.set(ConfigValue::BtPointer(i, BtPointer::Relative), Origin::Software).unwrap();
        }
        for a in 0..MAX_ARRANGEMENTS as u8 {
            c.set(ConfigValue::ArrangementName(a, ProfileName::new("twelve bytes")), Origin::Software).unwrap();
            for i in 0..MAX_SCREENS as u8 {
                let s = Screen {
                    host: Some(Host::ALL[(a + i) as usize % HOSTS]),
                    w: 3840,
                    h: 2160,
                    scale: 150,
                    desk: [-3840 * (i as i16 % 4), 0],
                    at: [-20_000 + 2560 * i as i16, -1440 * a as i16],
                    off: i % 3 == 0,
                    fixed: i % 2 == 0,
                };
                c.set(ConfigValue::Screen(a, i, s), Origin::Software).unwrap();
            }
        }
        let mut buf = [0xFFu8; ITEM_MAX];
        let n = c.encode(&mut buf);
        // Everything at its longest, with room left for what comes next.
        assert!(n + 32 <= ITEM_MAX, "item {n} bytes of {ITEM_MAX}");
        let mut d = Config::decode(&buf[..n]);
        // The config item holds no arrangement: each comes from its own item.
        assert_eq!(d.arrangements, [Arrangement::EMPTY; MAX_ARRANGEMENTS]);
        for a in 0..MAX_ARRANGEMENTS as u8 {
            let mut item = [0xFFu8; ARRANGEMENT_ITEM_MAX];
            let m = c.encode_arrangement(a, &mut item);
            assert_eq!(m, ARRANGEMENT_ITEM_MAX, "every slot used: the item at its longest");
            d.restore_arrangement(a, &item[..m]);
        }
        assert_eq!(d, c);
        assert!(d.arrangements[2].screens[3].off && d.arrangements[2].screens[4].fixed);
        assert_eq!(d.port_modes(), [PortMode::Auto, PortMode::Mirrored, PortMode::Auto]);
        assert_eq!(d.profiles[0].port_modes[Port::Other.index()], PortMode::PassthroughPc1);
        assert_eq!(Config::decode(&[]), Config::default());
    }

    #[test]
    fn an_arrangement_item_holds_its_name_and_the_slots_in_use_only() {
        let mut c = Config::default();
        c.set(ConfigValue::Screen(1, 5, Screen::new(Host::Bt2, 2388, 1668, [1920, 0])), Origin::Software).unwrap();
        let mut item = [0u8; ARRANGEMENT_ITEM_MAX];
        let n = c.encode_arrangement(1, &mut item);
        assert_eq!(n, 4 + (3 + 15), "the empty name and one screen");
        // Restoring it replaces the arrangement: what was there before is gone, the others stay.
        let mut d = Config::default();
        d.set(ConfigValue::Screen(1, 0, Screen::new(Host::Pc1, 1920, 1080, [0, 0])), Origin::Software).unwrap();
        d.set(ConfigValue::Screen(2, 0, Screen::new(Host::Pc2, 1920, 1080, [0, 0])), Origin::Software).unwrap();
        d.restore_arrangement(1, &item[..n]);
        assert_eq!(d.arrangements[1], c.arrangements[1]);
        assert_eq!(d.arrangements[2].screens[0].host, Some(Host::Pc2));
        // An arrangement item never sets anything else, whatever it holds.
        let mut cfg_item = [0u8; ITEM_MAX];
        let m = Config { led_brightness: 7, ..c }.encode(&mut cfg_item);
        d.restore_arrangement(0, &cfg_item[..m]);
        assert_eq!(d.led_brightness, Config::default().led_brightness);
    }

    #[test]
    fn a_config_item_with_one_arrangement_loads_it_as_arrangement_0() {
        // Firmware up to 0.1.12: the screens at 0x00B0–0x00B7 and each profile's switch at 0x00A8–0x00AB.
        let mut old = [0u8; 64];
        let mut w = Writer::new(&mut old);
        w.u16(key::PROFILE_ARRANGEMENT_BASE + 1).unwrap();
        w.lv(&[1]).unwrap();
        let s = Screen { scale: 150, ..Screen::new(Host::Pc2, 3840, 2160, [-2560, 0]) };
        let mut enc = [0u8; 24];
        let mut ew = Writer::new(&mut enc);
        ConfigValue::Screen(0, 6, s).encode(&mut ew).unwrap();
        let enc = ew.finish();
        assert_eq!(u16::from_le_bytes([enc[0], enc[1]]), 0x00B6);
        w.u16(0x00B6).unwrap();
        w.lv(&enc[2..]).unwrap();
        let n = w.len();
        let c = Config::decode(&old[..n]);
        assert_eq!(c.arrangements[0].screens[6], s);
        assert_eq!(c.profiles[1].arrangement, Some(0));
        assert_eq!(c.profiles[0].arrangement, None);
    }

    #[test]
    fn a_freed_screen_slot_has_no_flags() {
        let mut c = Config::default();
        let off = Screen { host: None, off: true, fixed: true, ..Screen::new(Host::Pc1, 3840, 2160, [-2560, 360]) };
        assert_eq!(
            c.set(ConfigValue::Screen(2, 3, off), Origin::Software),
            Ok(ConfigValue::Screen(2, 3, Screen::NONE))
        );
    }

    #[test]
    fn screen_hop_values_are_checked() {
        let mut c = Config::default();
        let bad = [
            ConfigValue::HopSpeed(11),
            ConfigValue::HopResistance(HOP_RESISTANCE_MAX + 1),
            ConfigValue::HopCorner(HOP_CORNER_MAX + 1),
            ConfigValue::ProfileArrangement(4, None),
            ConfigValue::ProfileArrangement(0, Some(4)),
            ConfigValue::ArrangementName(4, ProfileName::new("x")),
            ConfigValue::BtPointer(4, BtPointer::Relative),
            ConfigValue::Screen(0, 8, Screen::new(Host::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(4, 0, Screen::new(Host::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(0, 0, Screen::new(Host::Pc1, 100, 1080, [0, 0])),
            ConfigValue::Screen(0, 0, Screen { scale: 50, ..Screen::new(Host::Pc1, 1920, 1080, [0, 0]) }),
        ];
        for v in bad {
            assert_eq!(c.set(v, Origin::Software), Err(ErrorCode::InvalidValue), "{v:?}");
        }
        // A free slot is stored as the free slot, whatever else it had.
        let freed = Screen { host: None, ..Screen::new(Host::Pc1, 1920, 1080, [5, 5]) };
        assert_eq!(
            c.set(ConfigValue::Screen(0, 2, freed), Origin::Software),
            Ok(ConfigValue::Screen(0, 2, Screen::NONE))
        );
        assert!(!c.hop_on());
        c.set(ConfigValue::Screen(2, 0, Screen::new(Host::Bt1, 1920, 1080, [0, 0])), Origin::Software).unwrap();
        c.set(ConfigValue::ProfileArrangement(0, Some(2)), Origin::Software).unwrap();
        assert!(c.hop_on());
        assert_eq!(c.layout()[0].host, Some(Host::Bt1));
        // A PC's pointer is absolute whatever the Bluetooth settings say.
        c.set(ConfigValue::BtPointer(0, BtPointer::Relative), Origin::Software).unwrap();
        assert_eq!((c.bt_pointer(Host::Bt1), c.bt_pointer(Host::Pc1)), (BtPointer::Relative, BtPointer::Absolute));
    }

    #[test]
    fn port_modes_saved_before_profiles_go_to_the_first_profile() {
        let mut old = [0u8; 16];
        let mut w = Writer::new(&mut old);
        w.u16(key::PORT_MODE_BASE + 2).unwrap();
        w.lv(&[PortMode::PassthroughPc2 as u8]).unwrap();
        let n = w.len();
        let c = Config::decode(&old[..n]);
        assert_eq!((c.profile, c.profile_count), (0, 2));
        assert_eq!(c.port_modes(), [PortMode::Auto, PortMode::Auto, PortMode::PassthroughPc2]);
        assert_eq!(c.profiles[1].port_modes, [PortMode::Passthrough; 3]);
    }

    #[test]
    fn the_kvm_powers_up_in_profile_0_unless_it_remembers_the_last_one() {
        let mut c = Config::default();
        assert!(!c.remember_profile, "off by default");
        c.set(ConfigValue::Profile(1), Origin::Software).unwrap();
        let mut buf = [0xFFu8; ITEM_MAX];
        let n = c.encode(&mut buf);
        let d = Config::decode(&buf[..n]);
        assert_eq!((d.profile, d.port_modes()), (0, [PortMode::Auto; 3]));
        c.set(ConfigValue::RememberProfile(true), Origin::Software).unwrap();
        let n = c.encode(&mut buf);
        assert_eq!(Config::decode(&buf[..n]).profile, 1);
        assert_eq!(c.set(ConfigValue::LongPressMs(600), Origin::Software), Err(ErrorCode::InvalidValue));
    }

    #[test]
    fn profiles_in_use_bound_the_active_one() {
        let mut c = Config::default();
        assert_eq!(c.set(ConfigValue::Profile(2), Origin::Software), Err(ErrorCode::InvalidValue));
        c.set(ConfigValue::ProfileCount(3), Origin::Software).unwrap();
        c.set(ConfigValue::Profile(2), Origin::Software).unwrap();
        assert_eq!(c.next_profile(), 0);
        // Fewer profiles than the active one's number: the first becomes active.
        c.set(ConfigValue::ProfileCount(2), Origin::Software).unwrap();
        assert_eq!((c.profile, c.next_profile()), (0, 1));
        assert_eq!(c.set(ConfigValue::ProfileCount(5), Origin::Software), Err(ErrorCode::InvalidValue));
        assert_eq!(
            c.set(ConfigValue::ProfileModes(4, [PortMode::Auto; 3]), Origin::Software),
            Err(ErrorCode::InvalidValue)
        );
    }

    #[test]
    fn locks_only_open_at_the_device() {
        let mut c = Config::default();
        c.set(ConfigValue::SwConfigAllowed(false), Origin::Software).unwrap();
        assert_eq!(c.set(ConfigValue::SwConfigAllowed(true), Origin::Software), Err(ErrorCode::Locked));
        assert!(c.set(ConfigValue::SwConfigAllowed(true), Origin::Device).is_ok());
        assert_eq!(c.set(ConfigValue::DoubleTapMs(5), Origin::Software), Err(ErrorCode::InvalidValue));
    }
}
