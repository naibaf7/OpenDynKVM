//! Persistent configuration (software.md §8.3) and its flash item (§4.8).
//!
//! Item ([`ITEM`] in [`crate::store`], which adds the CRC): a list of entries `key u16 | len u8 | value`. Unknown keys
//! are skipped, so a newer firmware's item still loads in an older one.

use core::ops::RangeInclusive;

use kvm_proto::ctrl::{
    Accel, BT_HOSTS, BtPolicy, ConfigValue, ErrorCode, HOP_CORNER_MAX, HOP_RESISTANCE_MAX, HOP_SPEED, Hotkey,
    KeyAction, MAX_PROFILES, MAX_SCREENS, Mapping, Pc, Polling, Port, PortMode, ProfileName, Rgb, Screen, Units,
    VialMode, key,
};
use kvm_proto::wire::{Reader, Writer};

/// Store key of the config.
pub const ITEM: u16 = 0x0001;
/// Room for every key with margin.
/// 0.16.0 added the Bluetooth settings (a hotkey, four names, four policies): up to 83 bytes more.
/// Screen hop added the absolute pointer, its settings, four profile switches and eight screens: 194 bytes more.
/// Each PC's units for screen hop (protocol 8): 8 bytes more.
pub const ITEM_MAX: usize = 576;
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
    /// The screen-hop arrangement.
    pub screens: [Screen; MAX_SCREENS],
}

/// How screen hop moves the pointer (software.md §4.14); each profile turns it on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct HopConfig {
    pub accel: Accel,
    pub speed: i8,
    pub resistance: u16,
    pub corner: u16,
    /// How PC1 and PC2 map the absolute pointer onto their screens.
    pub mapping: [Mapping; 2],
    /// What PC1's and PC2's desktop coordinates count.
    pub units: [Units; 2],
}

/// A set of port modes to switch between with the key, e.g. emulated for work and passthrough for games.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Profile {
    pub name: ProfileName,
    /// Keyboard, Mouse, Other.
    pub port_modes: [PortMode; 3],
    /// Screen hop moves the pointer in this profile.
    pub hop: bool,
}

impl Profile {
    const fn new(name: &str, mode: PortMode) -> Profile {
        Profile { name: ProfileName::new(name), port_modes: [mode; 3], hop: false }
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
            mapping: [Mapping::Desktop; 2],
            units: [Units::Physical; 2],
        },
        screens: [Screen::NONE; MAX_SCREENS],
    };

    /// The port modes in force: the active profile's.
    pub fn port_modes(&self) -> [PortMode; 3] {
        self.profiles[self.profile as usize].port_modes
    }

    /// The profile after the active one, among those in use.
    pub fn next_profile(&self) -> u8 {
        (self.profile + 1) % self.profile_count
    }

    /// Screen hop is on in the active profile.
    pub fn hop_on(&self) -> bool {
        self.profiles[self.profile as usize].hop
    }

    pub fn mapping(&self, pc: Pc) -> Mapping {
        self.hop.mapping[pc.index()]
    }

    pub fn units(&self, pc: Pc) -> Units {
        self.hop.units[pc.index()]
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
            k if (key::HOP_MAPPING_BASE..key::HOP_MAPPING_BASE + 2).contains(&k) => {
                let pc = Pc::from_index((k - key::HOP_MAPPING_BASE) as usize);
                ConfigValue::HopMapping(pc, self.mapping(pc))
            }
            k if (key::HOP_UNITS_BASE..key::HOP_UNITS_BASE + 2).contains(&k) => {
                let pc = Pc::from_index((k - key::HOP_UNITS_BASE) as usize);
                ConfigValue::HopUnits(pc, self.units(pc))
            }
            k if (key::PROFILE_HOP_BASE..key::PROFILE_HOP_BASE + MAX_PROFILES as u16).contains(&k) => {
                let i = k - key::PROFILE_HOP_BASE;
                ConfigValue::ProfileHop(i as u8, self.profiles[i as usize].hop)
            }
            k if (key::SCREEN_BASE..key::SCREEN_BASE + MAX_SCREENS as u16).contains(&k) => {
                let i = k - key::SCREEN_BASE;
                ConfigValue::Screen(i as u8, self.screens[i as usize])
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
            ConfigValue::ProfileHop(i, _) if i as usize >= MAX_PROFILES => return Err(ErrorCode::InvalidValue),
            ConfigValue::HopSpeed(v) if !HOP_SPEED.contains(&v) => return Err(ErrorCode::InvalidValue),
            ConfigValue::HopResistance(v) if v > HOP_RESISTANCE_MAX => return Err(ErrorCode::InvalidValue),
            ConfigValue::HopCorner(v) if v > HOP_CORNER_MAX => return Err(ErrorCode::InvalidValue),
            ConfigValue::Screen(i, s) if i as usize >= MAX_SCREENS || !s.is_valid() => {
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
            ConfigValue::HopMapping(pc, m) => self.hop.mapping[pc.index()] = m,
            ConfigValue::HopUnits(pc, u) => self.hop.units[pc.index()] = u,
            ConfigValue::ProfileHop(i, b) => self.profiles[i as usize].hop = b,
            ConfigValue::Screen(i, s) => {
                // A free slot is stored as such, whatever else came with it.
                let s = if s.pc.is_some() { s } else { Screen::NONE };
                self.screens[i as usize] = s;
                return Ok(ConfigValue::Screen(i, s));
            }
        }
        Ok(v)
    }

    /// Encodes the flash item; returns its length.
    pub fn encode(&self, out: &mut [u8; ITEM_MAX]) -> usize {
        let mut w = Writer::new(out);
        for v in self.all() {
            let mut tmp = [0u8; 2 + VALUE_MAX];
            let mut tw = Writer::new(&mut tmp);
            // A value is at most a few bytes; these writes cannot overflow.
            let _ = v.encode(&mut tw);
            let enc = tw.finish();
            let _ = w.u16(v.key());
            let _ = w.lv(&enc[2..]);
        }
        w.len()
    }

    /// Decodes an item; entries that don't parse keep their default.
    pub fn decode(item: &[u8]) -> Config {
        let mut cfg = Config::default();
        let mut er = Reader::new(item);
        while er.remaining() > 0 {
            let (Ok(k), Ok(val)) = (er.u16(), er.lv()) else { break };
            if val.len() > VALUE_MAX {
                continue;
            }
            let mut tmp = [0u8; 2 + VALUE_MAX];
            tmp[..2].copy_from_slice(&k.to_le_bytes());
            tmp[2..2 + val.len()].copy_from_slice(val);
            if let Ok(v) = ConfigValue::decode(&mut Reader::new(&tmp[..2 + val.len()])) {
                // Stored values were validated when set; the device origin keeps the lock bits as stored.
                let _ = cfg.set(v, Origin::Device);
            }
        }
        if !cfg.remember_profile {
            cfg.profile = 0;
        }
        cfg
    }
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
            c.set(ConfigValue::ProfileHop(i, i % 2 == 0), Origin::Software).unwrap();
        }
        // Screen hop, with every screen slot used.
        c.set(ConfigValue::AbsPointer(true), Origin::Software).unwrap();
        c.set(ConfigValue::HopAccel(Accel::Adaptive), Origin::Software).unwrap();
        c.set(ConfigValue::HopSpeed(-3), Origin::Software).unwrap();
        c.set(ConfigValue::HopResistance(400), Origin::Software).unwrap();
        c.set(ConfigValue::HopCorner(0), Origin::Software).unwrap();
        c.set(ConfigValue::HopMapping(Pc::Pc2, Mapping::MainScreen), Origin::Software).unwrap();
        c.set(ConfigValue::HopUnits(Pc::Pc1, Units::Logical), Origin::Software).unwrap();
        for i in 0..MAX_SCREENS as u8 {
            let s = Screen {
                pc: Some(if i < 4 { Pc::Pc1 } else { Pc::Pc2 }),
                w: 3840,
                h: 2160,
                scale: 150,
                desk: [-3840 * (i as i16 % 4), 0],
                at: [-20_000 + 2560 * i as i16, -1440],
                off: i % 3 == 0,
                fixed: i % 2 == 0,
            };
            c.set(ConfigValue::Screen(i, s), Origin::Software).unwrap();
        }
        let mut buf = [0xFFu8; ITEM_MAX];
        let n = c.encode(&mut buf);
        // Everything at its longest, with room left for what comes next.
        assert!(n + 32 <= ITEM_MAX, "item {n} bytes of {ITEM_MAX}");
        let d = Config::decode(&buf[..n]);
        assert_eq!(d, c);
        assert!(d.screens[3].off && d.screens[4].fixed && !d.screens[4].off);
        assert_eq!(d.port_modes(), [PortMode::Auto, PortMode::Mirrored, PortMode::Auto]);
        assert_eq!(d.profiles[0].port_modes[Port::Other.index()], PortMode::PassthroughPc1);
        assert_eq!(Config::decode(&[]), Config::default());
    }

    #[test]
    fn a_freed_screen_slot_has_no_flags() {
        let mut c = Config::default();
        let off = Screen { pc: None, off: true, fixed: true, ..Screen::new(Pc::Pc1, 3840, 2160, [-2560, 360]) };
        assert_eq!(c.set(ConfigValue::Screen(3, off), Origin::Software), Ok(ConfigValue::Screen(3, Screen::NONE)));
    }

    #[test]
    fn screen_hop_values_are_checked() {
        let mut c = Config::default();
        let bad = [
            ConfigValue::HopSpeed(11),
            ConfigValue::HopResistance(HOP_RESISTANCE_MAX + 1),
            ConfigValue::HopCorner(HOP_CORNER_MAX + 1),
            ConfigValue::ProfileHop(4, true),
            ConfigValue::Screen(8, Screen::new(Pc::Pc1, 1920, 1080, [0, 0])),
            ConfigValue::Screen(0, Screen::new(Pc::Pc1, 100, 1080, [0, 0])),
            ConfigValue::Screen(0, Screen { scale: 50, ..Screen::new(Pc::Pc1, 1920, 1080, [0, 0]) }),
        ];
        for v in bad {
            assert_eq!(c.set(v, Origin::Software), Err(ErrorCode::InvalidValue), "{v:?}");
        }
        // A free slot is stored as the free slot, whatever else it had.
        let freed = Screen { pc: None, ..Screen::new(Pc::Pc1, 1920, 1080, [5, 5]) };
        assert_eq!(c.set(ConfigValue::Screen(2, freed), Origin::Software), Ok(ConfigValue::Screen(2, Screen::NONE)));
        assert!(!c.hop_on());
        c.set(ConfigValue::ProfileHop(0, true), Origin::Software).unwrap();
        assert!(c.hop_on());
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
