//! Human-readable names and text forms of protocol values, shared by the CLI and the GUI.

use kvm_core::hotkey::{format_spec, parse_spec};

use crate::{fl, i18n};
use kvm_hid::layouts::Layout;
use kvm_proto::ctrl::{
    Accel, Advertised, AutoReason, BT_HOSTS, BtPointer, BtPolicy, ConfigValue, DEVICES, Device, DevicePairing,
    ErrorCode, FaultKind, FwState, GamepadOut, HOP_CORNER_MAX, HOP_RESISTANCE_MAX, HOP_SPEED, HopState, Host, Hotkey,
    HotkeyAction, HotkeyBinding, HotkeyKind, KeyAction, LearnEnd, LinkState, MAX_ARRANGEMENTS, MAX_PROFILES, Mapping,
    PROFILE_NAME_MAX, Polling, Port, PortMode, PortState, PowerSource, ProfileName, Rgb, Routing, Screen, Speed,
    Status, Supply, SwitchSource, Units, VialMode, class, diag, key,
};
use kvm_proto::link::bt::appearance;

pub fn error(c: ErrorCode) -> String {
    match c {
        ErrorCode::Unknown => fl!("err-unknown"),
        ErrorCode::Malformed => fl!("err-malformed"),
        ErrorCode::Locked => fl!("err-locked"),
        ErrorCode::Busy => fl!("err-busy"),
        ErrorCode::Unsupported => fl!("err-unsupported"),
        ErrorCode::InvalidValue => fl!("err-invalid-value"),
        ErrorCode::NeedsKeyPress => fl!("err-needs-key-press"),
        ErrorCode::NoDevice => fl!("err-no-device"),
        ErrorCode::NotActive => fl!("err-not-active"),
        ErrorCode::NotAllowed => fl!("err-not-allowed"),
    }
}

/// How learning a hotkey ended, when it didn't learn one.
pub fn learn_end(end: LearnEnd) -> String {
    match end {
        LearnEnd::Learned => fl!("learn-learned"),
        LearnEnd::TimedOut => fl!("learn-timed-out"),
        LearnEnd::Cancelled => fl!("learn-cancelled"),
    }
}

/// What to know before using `h`, a hotkey just learned; `others`: the other hotkeys, by name.
pub fn hotkey_warning(h: &Hotkey, others: &[(&str, Hotkey)]) -> Option<String> {
    if let Some((name, _)) = others.iter().find(|(_, o)| o.kind != HotkeyKind::None && o == h) {
        return Some(fl!("hotkey-warn-twin", name = name.to_string()));
    }
    // Keys alone that nothing else uses: Scroll Lock, Pause, F13–F24.
    let spare = matches!(h.key, 0x47 | 0x48 | 0x68..=0x73);
    (h.kind == HotkeyKind::Combo && h.modifiers == 0 && !spare)
        .then(|| fl!("hotkey-warn-alone", key = format_spec(h).to_uppercase()))
}

/// What turning off switching (`switching`) or configuration from software means, for the "are you sure" questions.
pub fn lock_warning(switching: bool) -> String {
    let what = if switching { fl!("lock-warn-switching") } else { fl!("lock-warn-config") };
    let seconds = kvm_core::engine::UNLOCK_WAIT_MS / 1000;
    format!("{what}\n\n{}", fl!("lock-warn-back-on", seconds = seconds))
}

pub fn port_title(p: Port) -> String {
    match p {
        Port::Keyboard => fl!("port-keyboard"),
        Port::Mouse => fl!("port-mouse"),
        Port::Aux1 => fl!("port-aux1"),
        Port::Aux2 => fl!("port-aux2"),
    }
}

/// A device by where it is: a front port, or Bluetooth device slot n.
pub fn device_title(d: Device) -> String {
    match (d.port(), d.wireless()) {
        (Some(p), _) => port_title(p),
        (None, Some(n)) => fl!("device-wireless-n", n = n),
        (None, None) => String::new(),
    }
}

/// What a Bluetooth device says it is, by its appearance.
pub fn device_kind(a: u16) -> String {
    match a {
        appearance::KEYBOARD => fl!("device-kind-keyboard"),
        appearance::MOUSE => fl!("device-kind-mouse"),
        appearance::JOYSTICK | appearance::GAMEPAD => fl!("device-kind-gamepad"),
        appearance::DIGITIZER | appearance::DIGITAL_PEN => fl!("device-kind-pen"),
        appearance::TOUCHPAD => fl!("device-kind-touchpad"),
        appearance::PRESENTER => fl!("device-kind-presenter"),
        _ => fl!("device-kind-other"),
    }
}

/// How far pairing a Bluetooth device got.
pub fn device_pairing(p: DevicePairing) -> String {
    match p {
        DevicePairing::Idle => String::new(),
        DevicePairing::Connecting => fl!("device-pairing-connecting"),
        DevicePairing::Pairing => fl!("device-pairing-pairing"),
        DevicePairing::Reading => fl!("device-pairing-reading"),
        DevicePairing::Paired => fl!("device-pairing-paired"),
        DevicePairing::Failed => fl!("device-pairing-failed"),
    }
}

/// A routing as the UIs say it, with `host` naming the host it is pinned to.
pub fn route(r: Routing, host: impl Fn(Host) -> String) -> String {
    match r {
        Routing::Follow => fl!("route-follow"),
        Routing::Pin(h) => fl!("route-pin", host = host(h)),
    }
}

/// A routing as the CLI writes it: `follow`, or the host it is pinned to ([`host_name`]).
pub fn route_name(r: Routing) -> String {
    match r {
        Routing::Follow => "follow".into(),
        Routing::Pin(h) => host_name(h),
    }
}

pub fn parse_route(s: &str) -> Result<Routing, String> {
    match s.trim() {
        "follow" => Ok(Routing::Follow),
        h => parse_host(h)
            .map(Routing::Pin)
            .ok_or_else(|| fl!("parse-one-of", value = s, choices = "follow, pc1, pc2, host1, host2, host3, host4")),
    }
}

pub fn fault_kind(f: FaultKind) -> String {
    match f {
        FaultKind::OverCurrent => fl!("fault-over-current"),
        FaultKind::EnumerationFailed => fl!("fault-enumeration"),
        FaultKind::LinkDown => fl!("fault-link-down"),
        FaultKind::PowerBudget => fl!("fault-power-budget"),
        FaultKind::HubError => fl!("fault-hub"),
        FaultKind::BtSettingsHeld => fl!("fault-bt-settings-held"),
    }
}

pub fn link(l: LinkState) -> String {
    match l {
        LinkState::Unpowered => fl!("link-unpowered"),
        LinkState::Detached => fl!("link-detached"),
        LinkState::Enumerating => fl!("link-enumerating"),
        LinkState::Connected => fl!("link-connected"),
        LinkState::Suspended => fl!("link-suspended"),
        LinkState::LinkDown => fl!("link-down"),
    }
}

pub fn port_state(s: PortState) -> String {
    match s {
        PortState::Empty => fl!("port-state-empty"),
        PortState::Probing => fl!("port-state-probing"),
        PortState::Hosted => fl!("port-state-hosted"),
        PortState::Passthrough => fl!("port-state-passthrough"),
        PortState::Unpowered => fl!("port-state-unpowered"),
        PortState::Fault => fl!("port-state-fault"),
        PortState::Unsupported => fl!("port-state-unsupported"),
        PortState::Module => fl!("port-state-module"),
    }
}

pub fn mode(m: PortMode) -> String {
    match m {
        PortMode::Auto => fl!("mode-auto"),
        PortMode::Normalized => fl!("mode-normalized"),
        PortMode::Mirrored => fl!("mode-mirrored"),
        PortMode::MirroredIdentity => fl!("mode-mirrored-identity"),
        PortMode::Passthrough => fl!("mode-passthrough"),
        PortMode::Off => fl!("mode-off"),
    }
}

pub fn key_action(a: KeyAction) -> String {
    match a {
        KeyAction::None => fl!("key-action-none"),
        KeyAction::TogglePc => fl!("key-action-toggle-pc"),
        KeyAction::NextProfile => fl!("key-action-next-profile"),
        KeyAction::Aux2PortMode => fl!("key-action-aux2-port"),
        KeyAction::NextTarget => fl!("key-action-next-target"),
    }
}

pub fn reason(r: AutoReason) -> String {
    match r {
        AutoReason::NotAuto => fl!("reason-not-auto"),
        AutoReason::FitsNormalized => fl!("reason-fits-normalized"),
        AutoReason::ExtraInputs => fl!("reason-extra-inputs"),
        AutoReason::AuxPortHid => fl!("reason-aux-port-hid"),
        AutoReason::NonHid => fl!("reason-non-hid"),
        AutoReason::XInput => fl!("reason-xinput"),
        AutoReason::EndpointBudget => fl!("reason-endpoint-budget"),
        AutoReason::IdentityTaken => fl!("reason-identity-taken"),
        AutoReason::NeedsPassthrough => fl!("reason-needs-passthrough"),
    }
}

pub fn speed(s: Speed) -> String {
    match s {
        Speed::Unknown => String::new(),
        Speed::Low => fl!("speed-low"),
        Speed::Full => fl!("speed-full"),
        Speed::High => fl!("speed-high"),
    }
}

pub fn advertised(a: Advertised) -> String {
    match a {
        Advertised::None => fl!("adv-none"),
        Advertised::Default => fl!("adv-default"),
        Advertised::A1_5 => fl!("adv-1a5"),
        Advertised::A3_0 => fl!("adv-3a"),
    }
}

pub fn power_source(p: PowerSource) -> String {
    match p {
        PowerSource::PwrPort => fl!("power-source-pwr"),
        PowerSource::PcBus => fl!("power-source-pc"),
    }
}

/// The source and what is measured of its 5 V; before the first reading, the estimate left for the front ports.
pub fn power_line(st: &Status) -> String {
    let source = power_source(st.power);
    if st.supply == Supply::Unknown {
        return fl!("power-left", source = source, milliamps = st.budget_ma);
    }
    let v = |mv: u16| crate::i18n::decimal(mv as f64 / 1000.0, 2);
    fl!("power-supply", source = source, volts = v(st.supply_mv), lowest = v(st.supply_lowest_mv))
}

/// The supply warning there is, and whether it is serious: the measured 5 V sagging or too low. `short`: the command
/// line's few words.
pub fn power_warning(st: &Status, short: bool) -> Option<(String, bool)> {
    match (st.supply, short) {
        (Supply::Unknown | Supply::Good, _) => None,
        (Supply::Sags, false) => Some((fl!("supply-sags"), false)),
        (Supply::Sags, true) => Some((fl!("cli-supply-sags"), false)),
        (Supply::Low, false) => Some((fl!("supply-low"), true)),
        (Supply::Low, true) => Some((fl!("cli-supply-low"), true)),
    }
}

/// Why there is no warning though the front devices declare more than the estimate leaves them: the 5 V holds.
pub fn power_note(st: &Status) -> Option<String> {
    (st.supply == Supply::Good && st.declared_ma > st.budget_ma)
        .then(|| fl!("power-declared", declared = st.declared_ma, milliamps = st.budget_ma))
}

pub fn fw_state(s: FwState) -> String {
    match s {
        FwState::Idle => fl!("fw-state-idle"),
        FwState::Receiving => fl!("fw-state-receiving"),
        FwState::AwaitingKey => fl!("fw-state-awaiting-key"),
        FwState::Writing => fl!("fw-state-writing"),
        FwState::Done => fl!("fw-state-done"),
        FwState::Failed => fl!("fw-state-failed"),
    }
}

pub fn classes(bits: u16) -> String {
    let names = [
        (class::KEYBOARD, fl!("class-keyboard")),
        (class::MOUSE, fl!("class-mouse")),
        (class::CONSUMER, fl!("class-consumer")),
        (class::GAMEPAD, fl!("class-gamepad")),
        (class::XINPUT, fl!("class-xinput")),
        (class::VENDOR_HID, fl!("class-vendor-hid")),
        (class::VENDOR_CHANNEL, fl!("class-vendor-channel")),
        (class::OTHER_HID, fl!("class-other-hid")),
        (class::AUDIO, fl!("class-audio")),
        (class::VIDEO, fl!("class-video")),
        (class::STORAGE, fl!("class-storage")),
        (class::HUB, fl!("class-hub")),
    ];
    let v: Vec<String> = names.into_iter().filter(|(b, _)| bits & b != 0).map(|(_, n)| n).collect();
    if v.is_empty() { "-".into() } else { v.join(", ") }
}

/// The wireless module's state in words.
pub fn bt_module(m: diag::BtModule) -> String {
    match m {
        diag::BtModule::Up => fl!("bt-module-up"),
        diag::BtModule::Starting => fl!("bt-module-starting"),
        diag::BtModule::Programming => fl!("bt-module-programming"),
        diag::BtModule::NeedsUpdate => fl!("bt-module-needs-update"),
        diag::BtModule::Absent => fl!("bt-module-absent"),
    }
}

/// The self-test result in words: "passed", "running", or what failed.
pub fn selftest(bits: u32) -> String {
    use kvm_proto::ctrl::selftest::*;
    let names = [
        (HUB0, fl!("selftest-hub0")),
        (LINK1, fl!("selftest-link1")),
        (LINK2, fl!("selftest-link2")),
        (HUB1, fl!("selftest-hub1")),
        (HUB2, fl!("selftest-hub2")),
        (PORT_KB, fl!("selftest-port-kb")),
        (PORT_MS, fl!("selftest-port-ms")),
        (PORT_A1, fl!("selftest-port-a1")),
        (PORT_A2, fl!("selftest-port-a2")),
        (POWER, fl!("selftest-power")),
        (AT32_FW1, fl!("selftest-at32-fw1")),
        (AT32_FW2, fl!("selftest-at32-fw2")),
        (AT32_GAVE_UP1, fl!("selftest-at32-gave-up1")),
        (AT32_GAVE_UP2, fl!("selftest-at32-gave-up2")),
        (AT32_XTAL1, fl!("selftest-at32-xtal1")),
        (AT32_XTAL2, fl!("selftest-at32-xtal2")),
        (BT_FW, fl!("selftest-bt-fw")),
    ];
    let failed: Vec<String> = names.into_iter().filter(|(b, _)| bits & b != 0).map(|(_, n)| n).collect();
    match (failed.is_empty(), bits & RUNNING != 0) {
        (true, true) => fl!("selftest-running"),
        (true, false) => fl!("selftest-passed"),
        (false, true) => fl!("selftest-running-so-far", failed = failed.join(", ")),
        (false, false) => fl!("selftest-failed", failed = failed.join(", ")),
    }
}

/// Why the KVM switched, for event lists.
pub fn switch_source(s: SwitchSource) -> String {
    match s {
        SwitchSource::Key => fl!("switch-source-key"),
        SwitchSource::Hotkey => fl!("switch-source-hotkey"),
        SwitchSource::Software => fl!("switch-source-software"),
        SwitchSource::Boot => fl!("switch-source-boot"),
        SwitchSource::Power => fl!("switch-source-power"),
        SwitchSource::Edge => fl!("switch-source-edge"),
    }
}

/// Whether screen hop moves the pointer, and if not, why not.
pub fn hop_state(s: HopState) -> String {
    match s {
        HopState::Off => fl!("hop-state-off"),
        HopState::Active => fl!("hop-state-active"),
        HopState::NoAbsolute => fl!("hop-state-no-absolute"),
        HopState::NotNormalized => fl!("hop-state-not-normalized"),
        HopState::NoScreens => fl!("hop-state-no-screens"),
        HopState::BootProtocol => fl!("hop-state-boot-protocol"),
    }
}

pub fn accel(a: Accel) -> String {
    match a {
        Accel::Flat => fl!("accel-flat"),
        Accel::Precision => fl!("accel-precision"),
        Accel::Adaptive => fl!("accel-adaptive"),
    }
}

pub fn mapping(m: Mapping) -> String {
    match m {
        Mapping::Desktop => fl!("mapping-desktop"),
        Mapping::MainScreen => fl!("mapping-main-screen"),
    }
}

/// A host as the CLI writes it: `pc1`, `pc2`, or `host1`–`host4` for the wireless hosts (as in `switch` and the
/// `hostN-…` settings).
pub fn host_name(h: Host) -> String {
    match h.bt_slot() {
        None => h.name().into(),
        Some(n) => format!("host{n}"),
    }
}

/// [`host_name`]'s form, or `bt1`–`bt4`, `1`, `2`.
pub fn parse_host(s: &str) -> Option<Host> {
    match s {
        "pc1" | "1" => Some(Host::Pc1),
        "pc2" | "2" => Some(Host::Pc2),
        _ => s.strip_prefix("host").or_else(|| s.strip_prefix("bt")).and_then(|n| Host::bt(n.parse().ok()?)),
    }
}

/// A screen slot as the CLI writes it: `pc1 2560x1440 150% at 1920,0 desk 0,0`, then `no-hop` when it is off for
/// screen hop and `fixed` when its place on its computer is; or `off` for a free slot.
pub fn format_screen(s: &Screen) -> String {
    match s.host {
        None => "off".into(),
        Some(h) => format!(
            "{} {}x{} {}% at {},{} desk {},{}{}{}",
            host_name(h),
            s.w,
            s.h,
            s.scale,
            s.at[0],
            s.at[1],
            s.desk[0],
            s.desk[1],
            if s.off { " no-hop" } else { "" },
            if s.fixed { " fixed" } else { "" }
        ),
    }
}

/// How screen hop moves a wireless host's pointer.
pub fn bt_pointer(p: BtPointer) -> String {
    match p {
        BtPointer::Absolute => fl!("bt-pointer-absolute"),
        BtPointer::Relative => fl!("bt-pointer-relative"),
    }
}

pub fn units(u: Units) -> String {
    match u {
        Units::Physical => fl!("units-physical"),
        Units::Logical => fl!("units-logical"),
    }
}

/// The CLI's form of a screen slot ([`format_screen`]); the scaling (100 %) and both positions (0,0) may be left out.
pub fn parse_screen(text: &str) -> Result<Screen, String> {
    let bad = || fl!("parse-screen", value = text);
    let mut words = text.split_whitespace();
    let host = match words.next() {
        Some("off" | "none") => return Ok(Screen::NONE),
        Some(w) => parse_host(w).ok_or_else(bad)?,
        None => return Err(bad()),
    };
    let point = |w: Option<&str>| -> Result<[i16; 2], String> {
        let (x, y) = w.and_then(|w| w.split_once(',')).ok_or_else(bad)?;
        Ok([x.trim().parse().map_err(|_| bad())?, y.trim().parse().map_err(|_| bad())?])
    };
    let (w, h) = words.next().and_then(|r| r.split_once(['x', '×'])).ok_or_else(bad)?;
    let mut s = Screen::new(host, w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?, [0, 0]);
    while let Some(word) = words.next() {
        match word {
            "at" => s.at = point(words.next())?,
            "desk" => s.desk = point(words.next())?,
            w if w.ends_with('%') => s.scale = w.trim_end_matches('%').parse().map_err(|_| bad())?,
            "no-hop" => s.off = true,
            "fixed" => s.fixed = true,
            _ => return Err(bad()),
        }
    }
    if !s.is_valid() {
        return Err(bad());
    }
    Ok(s)
}

/// Firmware updates over USB (`diag::id::FW_UPDATES`): on, or why not, with what the check read for a bug report.
pub fn fw_updates(v: i32) -> String {
    let detail = format!("{:#x}", (v as u32) >> 8);
    match diag::FwUpdates::from_u8(v as u8) {
        Some(diag::FwUpdates::PartitionA) => fl!("fw-updates-on", partition = "A"),
        Some(diag::FwUpdates::PartitionB) => fl!("fw-updates-on", partition = "B"),
        Some(diag::FwUpdates::NoTable) => fl!("fw-updates-no-table"),
        Some(diag::FwUpdates::ReadFailed) => fl!("fw-updates-read-failed", detail = detail),
        Some(diag::FwUpdates::OtherTable) => fl!("fw-updates-other-table", detail = detail),
        Some(diag::FwUpdates::OtherLayout) => fl!("fw-updates-other-layout", detail = detail),
        Some(diag::FwUpdates::UnknownPartition) | None => {
            fl!("fw-updates-unknown-partition", detail = format!("{v:#x}"))
        }
    }
}

pub fn version(v: u32) -> String {
    if v == 0 { "-".into() } else { format!("{}.{}.{}", v >> 16, (v >> 8) & 0xFF, v & 0xFF) }
}

/// A diagnostics group's heading ("Other" for items of no group this version knows).
pub fn diag_group(g: Option<diag::Group>) -> String {
    match g {
        Some(diag::Group::System) => fl!("diag-group-system"),
        Some(diag::Group::Clocks) => fl!("diag-group-clocks"),
        Some(diag::Group::Usb) => fl!("diag-group-usb"),
        Some(diag::Group::Power) => fl!("diag-group-power"),
        Some(diag::Group::PcChips) => fl!("diag-group-pc-chips"),
        Some(diag::Group::Bluetooth) => fl!("diag-group-wireless"),
        None => fl!("diag-group-other"),
    }
}

/// A diagnostics item's name; the firmware's own name for one this version doesn't translate, "item 0x5a" for one
/// it doesn't know at all.
pub fn diag_name(id: u8) -> String {
    use diag::id::*;
    match id {
        UPTIME => fl!("diag-uptime"),
        RESET_CAUSE => fl!("diag-reset-cause"),
        RP_TEMP => fl!("diag-rp-temp"),
        SELFTEST => fl!("diag-selftest"),
        DROPPED => fl!("diag-dropped"),
        FW_UPDATES => fl!("diag-fw-updates"),
        XOSC => fl!("diag-xosc"),
        PLL_SYS => fl!("diag-pll-sys"),
        PLL_USB => fl!("diag-pll-usb"),
        CLK_SYS => "clk_sys".into(),
        CLK_USB => "clk_usb".into(),
        CLK_PERI => "clk_peri".into(),
        CLK_ADC => "clk_adc".into(),
        ROSC => fl!("diag-rosc"),
        AT1_CLOCK => fl!("diag-at-clock", n = 1),
        AT2_CLOCK => fl!("diag-at-clock", n = 2),
        HUB0_SPEED => fl!("diag-hub0-speed"),
        HUB0_UP => fl!("diag-hub0-up"),
        PHY_SWAP => fl!("diag-phy-swap"),
        KB_SPEED => fl!("diag-kb-speed"),
        MS_SPEED => fl!("diag-ms-speed"),
        A1_SPEED => fl!("diag-a1-speed"),
        A2_SPEED => fl!("diag-a2-speed"),
        PC1_SPEED => fl!("diag-pc-speed", pc = "PC1"),
        PC2_SPEED => fl!("diag-pc-speed", pc = "PC2"),
        PC1_LINK => fl!("diag-pc-link", pc = "PC1"),
        PC2_LINK => fl!("diag-pc-link", pc = "PC2"),
        PC1_RESETS => fl!("diag-pc-resets", pc = "PC1"),
        PC2_RESETS => fl!("diag-pc-resets", pc = "PC2"),
        LINK1_ERRORS => fl!("diag-link-errors", n = 1),
        LINK2_ERRORS => fl!("diag-link-errors", n = 2),
        HUB0_ERRORS => fl!("diag-hub0-errors"),
        ENUM_FAILS => fl!("diag-enum-fails"),
        PWR_VBUS => fl!("diag-vbus", port = "PWR"),
        PWR_CC => fl!("diag-cc", port = "PWR"),
        PC1_VBUS => fl!("diag-vbus", port = "PC1"),
        PC2_VBUS => fl!("diag-vbus", port = "PC2"),
        PC1_CC => fl!("diag-cc", port = "PC1"),
        PC2_CC => fl!("diag-cc", port = "PC2"),
        AT1_3V3 => fl!("diag-at-3v3", n = 1),
        AT2_3V3 => fl!("diag-at-3v3", n = 2),
        BUDGET => fl!("diag-budget"),
        SUPPLY => fl!("diag-supply"),
        SUPPLY_DIPS => fl!("diag-supply-dips"),
        PC1_DROPOUTS => fl!("diag-dropouts", pc = "PC1"),
        PC2_DROPOUTS => fl!("diag-dropouts", pc = "PC2"),
        KB_FAULTS => fl!("diag-kb-faults"),
        MS_FAULTS => fl!("diag-ms-faults"),
        A1_FAULTS => fl!("diag-a1-faults"),
        A2_FAULTS => fl!("diag-a2-faults"),
        AT1_TEMP => fl!("diag-at-temp", n = 1),
        AT2_TEMP => fl!("diag-at-temp", n = 2),
        AT1_FW => fl!("diag-at-fw", n = 1),
        AT2_FW => fl!("diag-at-fw", n = 2),
        BT_MODULE => fl!("diag-bt-module"),
        BT_FW => fl!("diag-bt-fw"),
        BT_LINK_ERRORS => fl!("diag-bt-link-errors"),
        BT_HOSTS => fl!("diag-bt-hosts"),
        BT_SETTINGS_HELD => fl!("diag-bt-settings-held"),
        _ => diag::meta(id).map_or_else(|| fl!("diag-unknown", id = format!("{id:#04x}")), |m| m.name.to_string()),
    }
}

/// A diagnostics item's value in words and units.
pub fn diag_value(i: &diag::Item) -> String {
    use diag::Unit;
    let v = i.value;
    let Some(m) = diag::meta(i.id) else { return v.to_string() };
    match m.unit {
        Unit::Seconds => duration(v.max(0) as u32),
        Unit::ResetCause => match diag::ResetCause::from_u8(v as u8) {
            Some(diag::ResetCause::Software) => fl!("reset-software"),
            Some(diag::ResetCause::PowerOn) => fl!("reset-power-on"),
            Some(diag::ResetCause::BrownOut) => fl!("reset-brown-out"),
            Some(diag::ResetCause::Glitch) => fl!("reset-glitch"),
            Some(diag::ResetCause::RunPin) => fl!("reset-run-pin"),
            Some(diag::ResetCause::Watchdog) => fl!("reset-watchdog"),
            Some(diag::ResetCause::Debugger) => fl!("reset-debugger"),
            None => fl!("reset-other", cause = v),
        },
        Unit::DeciCelsius => format!("{} °C", i18n::decimal(v as f64 / 10.0, 1)),
        Unit::SelfTest => selftest(v as u32),
        Unit::Count => v.to_string(),
        Unit::Hz => match v {
            0 => fl!("hz-stopped"),
            1_000_000.. => format!("{} MHz", i18n::decimal(v as f64 / 1e6, 3)),
            1_000.. => format!("{} kHz", i18n::decimal(v as f64 / 1e3, 1)),
            _ => format!("{v} Hz"),
        },
        Unit::ClockSource => match diag::ClockSource::from_u8(v as u8) {
            Some(diag::ClockSource::Crystal) => fl!("clock-crystal"),
            Some(diag::ClockSource::Internal) => fl!("clock-internal"),
            _ => "-".into(),
        },
        Unit::Speed => match Speed::from_u8(v as u8) {
            Some(Speed::Unknown) | None => "-".into(),
            Some(s) => speed(s),
        },
        Unit::Bool => {
            if v != 0 {
                fl!("yes")
            } else {
                fl!("no")
            }
        }
        Unit::Swap => {
            let on = if v & 1 != 0 { fl!("on") } else { fl!("off") };
            if v & 2 != 0 { fl!("swap-found", state = on) } else { on }
        }
        Unit::LinkState => LinkState::from_u8(v as u8).map_or_else(|| fl!("state-other", state = v), link),
        Unit::MilliVolt => format!("{} V", i18n::decimal(v as f64 / 1000.0, 2)),
        Unit::MilliAmp => format!("{v} mA"),
        Unit::Version => version(v as u32),
        Unit::BtModule => diag::BtModule::from_u8(v as u8).map_or_else(|| fl!("state-other", state = v), bt_module),
        Unit::FwUpdates => fw_updates(v),
        Unit::Hex => format!("{:#x}", v as u32),
    }
}

/// What an item's colours mean, for its hover text ("" if nothing to say).
pub fn diag_help(id: u8) -> String {
    use diag::id::*;
    match id {
        RESET_CAUSE => fl!("diag-help-reset-cause"),
        RP_TEMP => fl!("diag-help-rp-temp"),
        SELFTEST => fl!("diag-help-selftest"),
        DROPPED => fl!("diag-help-dropped"),
        FW_UPDATES => fl!("diag-help-fw-updates"),
        XOSC => fl!("diag-help-xosc"),
        PLL_SYS | CLK_SYS => fl!("diag-help-clk-sys"),
        PLL_USB | CLK_USB => fl!("diag-help-clk-usb"),
        CLK_ADC => fl!("diag-help-clk-adc"),
        AT1_CLOCK | AT2_CLOCK => fl!("diag-help-at-clock"),
        HUB0_SPEED => fl!("diag-help-hub0-speed"),
        PHY_SWAP => fl!("diag-help-phy-swap"),
        PC1_SPEED | PC2_SPEED => fl!("diag-help-pc-speed"),
        PC1_LINK | PC2_LINK => fl!("diag-help-pc-link"),
        LINK1_ERRORS | LINK2_ERRORS => fl!("diag-help-link-errors"),
        HUB0_ERRORS => fl!("diag-help-hub0-errors"),
        ENUM_FAILS => fl!("diag-help-enum-fails"),
        PWR_VBUS | PC1_VBUS | PC2_VBUS => fl!("diag-help-vbus"),
        PWR_CC | PC1_CC | PC2_CC => fl!("diag-help-cc"),
        AT1_3V3 | AT2_3V3 => fl!("diag-help-3v3"),
        BUDGET => fl!("diag-help-budget"),
        SUPPLY => fl!("diag-help-supply"),
        SUPPLY_DIPS => fl!("diag-help-supply-dips"),
        PC1_DROPOUTS | PC2_DROPOUTS => fl!("diag-help-dropouts"),
        KB_FAULTS | MS_FAULTS | A2_FAULTS => fl!("diag-help-faults"),
        AT1_TEMP | AT2_TEMP => fl!("diag-help-at-temp"),
        AT1_FW | AT2_FW => fl!("diag-help-at-fw"),
        BT_MODULE => fl!("diag-help-bt-module"),
        BT_FW => fl!("diag-help-bt-fw"),
        BT_LINK_ERRORS => fl!("diag-help-bt-link-errors"),
        BT_HOSTS => fl!("diag-help-bt-hosts"),
        BT_SETTINGS_HELD => fl!("diag-help-bt-settings-held"),
        _ => String::new(),
    }
}

/// The diagnostics as plain text, grouped as the CLI prints them, with a mark per grade.
pub fn diag_text(items: &[diag::Item]) -> String {
    let mark = |g: diag::Grade| match g {
        diag::Grade::Info => " ",
        diag::Grade::Ok => "✓",
        diag::Grade::Warn => "!",
        diag::Grade::Bad => "✗",
    };
    let width = items.iter().map(|i| diag_name(i.id).chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for (group, list) in diag_groups(items) {
        out += &diag_group(group);
        out += "\n";
        for i in list {
            out += &format!("  {} {:<width$}  {}\n", mark(i.grade), diag_name(i.id), diag_value(i));
        }
    }
    out
}

/// The items by group, in the groups' order; items of no known group last (None).
pub fn diag_groups(items: &[diag::Item]) -> Vec<(Option<diag::Group>, Vec<&diag::Item>)> {
    let mut out: Vec<(Option<diag::Group>, Vec<&diag::Item>)> = diag::Group::ALL
        .iter()
        .map(|g| (Some(*g), items.iter().filter(|i| diag::Group::of(i.id) == Some(*g)).collect()))
        .collect();
    out.push((None, items.iter().filter(|i| diag::Group::of(i.id).is_none()).collect()));
    out.retain(|(_, list)| !list.is_empty());
    out
}

/// "45 s", "12 min 05 s", "3 h 07 min", "2 d 4 h" (the units are SI, the same in every language here).
pub fn duration(s: u32) -> String {
    match s {
        86_400.. => format!("{} d {} h", s / 86_400, s % 86_400 / 3600),
        3600.. => format!("{} h {:02} min", s / 3600, s % 3600 / 60),
        60.. => format!("{} min {:02} s", s / 60, s % 60),
        _ => format!("{s} s"),
    }
}

pub const CONFIG_NAMES: &[(&str, u16)] = &[
    // The hotkey list, entries 1–16 (keys 0x00F0–0x00FF).
    ("hotkey1", key::HOTKEY_BASE),
    ("hotkey2", key::HOTKEY_BASE + 1),
    ("hotkey3", key::HOTKEY_BASE + 2),
    ("hotkey4", key::HOTKEY_BASE + 3),
    ("hotkey5", key::HOTKEY_BASE + 4),
    ("hotkey6", key::HOTKEY_BASE + 5),
    ("hotkey7", key::HOTKEY_BASE + 6),
    ("hotkey8", key::HOTKEY_BASE + 7),
    ("hotkey9", key::HOTKEY_BASE + 8),
    ("hotkey10", key::HOTKEY_BASE + 9),
    ("hotkey11", key::HOTKEY_BASE + 10),
    ("hotkey12", key::HOTKEY_BASE + 11),
    ("hotkey13", key::HOTKEY_BASE + 12),
    ("hotkey14", key::HOTKEY_BASE + 13),
    ("hotkey15", key::HOTKEY_BASE + 14),
    ("hotkey16", key::HOTKEY_BASE + 15),
    ("double-tap-ms", key::DOUBLE_TAP_MS),
    ("key-tap", key::KEY_TAP),
    ("key-long", key::KEY_LONG),
    ("long-press-ms", key::LONG_PRESS_MS),
    ("led-brightness", key::LED_BRIGHTNESS),
    ("color-pc1", key::COLOR_PC1),
    ("color-pc2", key::COLOR_PC2),
    ("polling", key::PC_POLLING),
    ("power-cycle", key::POWER_CYCLE_ON_SWITCH),
    ("mode-keyboard", key::MODE_BASE),
    ("mode-mouse", key::MODE_BASE + 1),
    ("mode-aux1", key::MODE_BASE + 2),
    ("mode-aux2", key::MODE_BASE + 3),
    ("mode-wireless1", key::MODE_BASE + 4),
    ("mode-wireless2", key::MODE_BASE + 5),
    ("mode-wireless3", key::MODE_BASE + 6),
    ("mode-wireless4", key::MODE_BASE + 7),
    ("route-keyboard", key::ROUTE_BASE),
    ("route-mouse", key::ROUTE_BASE + 1),
    ("route-aux1", key::ROUTE_BASE + 2),
    ("route-aux2", key::ROUTE_BASE + 3),
    ("route-wireless1", key::ROUTE_BASE + 4),
    ("route-wireless2", key::ROUTE_BASE + 5),
    ("route-wireless3", key::ROUTE_BASE + 6),
    ("route-wireless4", key::ROUTE_BASE + 7),
    ("software-switching", key::SW_SWITCH_ALLOWED),
    ("software-config", key::SW_CONFIG_ALLOWED),
    ("vial", key::VIAL),
    ("gamepad", key::GAMEPAD),
    ("auto-select", key::AUTO_SELECT),
    ("standby-wake", key::STANDBY_WAKE),
    ("standby-glow", key::STANDBY_GLOW),
    ("profiles", key::PROFILE_COUNT),
    ("profile", key::PROFILE),
    ("remember-profile", key::REMEMBER_PROFILE),
    ("profile0-name", key::PROFILE_NAME_BASE),
    ("profile0-modes", key::PROFILE_MODES_BASE),
    ("profile0-routes", key::PROFILE_ROUTES_BASE),
    ("profile0-arrangement", key::PROFILE_ARRANGEMENT_BASE),
    ("profile0-keymap", key::PROFILE_KEYMAP_BASE),
    ("profile0-fallback", key::PROFILE_FALLBACK_BASE),
    ("profile1-name", key::PROFILE_NAME_BASE + 1),
    ("profile1-modes", key::PROFILE_MODES_BASE + 1),
    ("profile1-routes", key::PROFILE_ROUTES_BASE + 1),
    ("profile1-arrangement", key::PROFILE_ARRANGEMENT_BASE + 1),
    ("profile1-keymap", key::PROFILE_KEYMAP_BASE + 1),
    ("profile1-fallback", key::PROFILE_FALLBACK_BASE + 1),
    ("profile2-name", key::PROFILE_NAME_BASE + 2),
    ("profile2-modes", key::PROFILE_MODES_BASE + 2),
    ("profile2-routes", key::PROFILE_ROUTES_BASE + 2),
    ("profile2-arrangement", key::PROFILE_ARRANGEMENT_BASE + 2),
    ("profile2-keymap", key::PROFILE_KEYMAP_BASE + 2),
    ("profile2-fallback", key::PROFILE_FALLBACK_BASE + 2),
    ("profile3-name", key::PROFILE_NAME_BASE + 3),
    ("profile3-modes", key::PROFILE_MODES_BASE + 3),
    ("profile3-routes", key::PROFILE_ROUTES_BASE + 3),
    ("profile3-arrangement", key::PROFILE_ARRANGEMENT_BASE + 3),
    ("profile3-keymap", key::PROFILE_KEYMAP_BASE + 3),
    ("profile3-fallback", key::PROFILE_FALLBACK_BASE + 3),
    ("profile4-name", key::PROFILE_NAME_BASE + 4),
    ("profile4-modes", key::PROFILE_MODES_BASE + 4),
    ("profile4-routes", key::PROFILE_ROUTES_BASE + 4),
    ("profile4-arrangement", key::PROFILE_ARRANGEMENT_BASE + 4),
    ("profile4-keymap", key::PROFILE_KEYMAP_BASE + 4),
    ("profile4-fallback", key::PROFILE_FALLBACK_BASE + 4),
    ("profile5-name", key::PROFILE_NAME_BASE + 5),
    ("profile5-modes", key::PROFILE_MODES_BASE + 5),
    ("profile5-routes", key::PROFILE_ROUTES_BASE + 5),
    ("profile5-arrangement", key::PROFILE_ARRANGEMENT_BASE + 5),
    ("profile5-keymap", key::PROFILE_KEYMAP_BASE + 5),
    ("profile5-fallback", key::PROFILE_FALLBACK_BASE + 5),
    ("profile6-name", key::PROFILE_NAME_BASE + 6),
    ("profile6-modes", key::PROFILE_MODES_BASE + 6),
    ("profile6-routes", key::PROFILE_ROUTES_BASE + 6),
    ("profile6-arrangement", key::PROFILE_ARRANGEMENT_BASE + 6),
    ("profile6-keymap", key::PROFILE_KEYMAP_BASE + 6),
    ("profile6-fallback", key::PROFILE_FALLBACK_BASE + 6),
    ("profile7-name", key::PROFILE_NAME_BASE + 7),
    ("profile7-modes", key::PROFILE_MODES_BASE + 7),
    ("profile7-routes", key::PROFILE_ROUTES_BASE + 7),
    ("profile7-arrangement", key::PROFILE_ARRANGEMENT_BASE + 7),
    ("profile7-keymap", key::PROFILE_KEYMAP_BASE + 7),
    ("profile7-fallback", key::PROFILE_FALLBACK_BASE + 7),
    ("host1-name", key::BT_NAME_BASE),
    ("host2-name", key::BT_NAME_BASE + 1),
    ("host3-name", key::BT_NAME_BASE + 2),
    ("host4-name", key::BT_NAME_BASE + 3),
    ("host1-policy", key::BT_POLICY_BASE),
    ("host2-policy", key::BT_POLICY_BASE + 1),
    ("host3-policy", key::BT_POLICY_BASE + 2),
    ("host4-policy", key::BT_POLICY_BASE + 3),
    ("host1-control", key::BT_CONTROL_BASE),
    ("host2-control", key::BT_CONTROL_BASE + 1),
    ("host3-control", key::BT_CONTROL_BASE + 2),
    ("host4-control", key::BT_CONTROL_BASE + 3),
    ("layout-pc1", key::LAYOUT_BASE),
    ("layout-pc2", key::LAYOUT_BASE + 1),
    ("layout-host1", key::LAYOUT_BASE + 2),
    ("layout-host2", key::LAYOUT_BASE + 3),
    ("layout-host3", key::LAYOUT_BASE + 4),
    ("layout-host4", key::LAYOUT_BASE + 5),
    ("abs-pointer", key::ABS_POINTER),
    ("hop-accel", key::HOP_ACCEL),
    ("hop-speed", key::HOP_SPEED),
    ("hop-resistance", key::HOP_RESISTANCE),
    ("hop-corner", key::HOP_CORNER),
    ("hop-mapping-pc1", key::HOP_MAPPING_BASE),
    ("hop-mapping-pc2", key::HOP_MAPPING_BASE + 1),
    ("hop-mapping-host1", key::HOP_MAPPING_BT_BASE),
    ("hop-mapping-host2", key::HOP_MAPPING_BT_BASE + 1),
    ("hop-mapping-host3", key::HOP_MAPPING_BT_BASE + 2),
    ("hop-mapping-host4", key::HOP_MAPPING_BT_BASE + 3),
    ("hop-units-pc1", key::HOP_UNITS_BASE),
    ("hop-units-pc2", key::HOP_UNITS_BASE + 1),
    ("hop-units-host1", key::HOP_UNITS_BT_BASE),
    ("hop-units-host2", key::HOP_UNITS_BT_BASE + 1),
    ("hop-units-host3", key::HOP_UNITS_BT_BASE + 2),
    ("hop-units-host4", key::HOP_UNITS_BT_BASE + 3),
    ("host1-pointer", key::BT_POINTER_BASE),
    ("host2-pointer", key::BT_POINTER_BASE + 1),
    ("host3-pointer", key::BT_POINTER_BASE + 2),
    ("host4-pointer", key::BT_POINTER_BASE + 3),
    ("arrangement0-name", key::ARRANGEMENT_NAME_BASE),
    ("arrangement0-screen0", key::screen(0, 0)),
    ("arrangement0-screen1", key::screen(0, 1)),
    ("arrangement0-screen2", key::screen(0, 2)),
    ("arrangement0-screen3", key::screen(0, 3)),
    ("arrangement0-screen4", key::screen(0, 4)),
    ("arrangement0-screen5", key::screen(0, 5)),
    ("arrangement0-screen6", key::screen(0, 6)),
    ("arrangement0-screen7", key::screen(0, 7)),
    ("arrangement1-name", key::ARRANGEMENT_NAME_BASE + 1),
    ("arrangement1-screen0", key::screen(1, 0)),
    ("arrangement1-screen1", key::screen(1, 1)),
    ("arrangement1-screen2", key::screen(1, 2)),
    ("arrangement1-screen3", key::screen(1, 3)),
    ("arrangement1-screen4", key::screen(1, 4)),
    ("arrangement1-screen5", key::screen(1, 5)),
    ("arrangement1-screen6", key::screen(1, 6)),
    ("arrangement1-screen7", key::screen(1, 7)),
    ("arrangement2-name", key::ARRANGEMENT_NAME_BASE + 2),
    ("arrangement2-screen0", key::screen(2, 0)),
    ("arrangement2-screen1", key::screen(2, 1)),
    ("arrangement2-screen2", key::screen(2, 2)),
    ("arrangement2-screen3", key::screen(2, 3)),
    ("arrangement2-screen4", key::screen(2, 4)),
    ("arrangement2-screen5", key::screen(2, 5)),
    ("arrangement2-screen6", key::screen(2, 6)),
    ("arrangement2-screen7", key::screen(2, 7)),
    ("arrangement3-name", key::ARRANGEMENT_NAME_BASE + 3),
    ("arrangement3-screen0", key::screen(3, 0)),
    ("arrangement3-screen1", key::screen(3, 1)),
    ("arrangement3-screen2", key::screen(3, 2)),
    ("arrangement3-screen3", key::screen(3, 3)),
    ("arrangement3-screen4", key::screen(3, 4)),
    ("arrangement3-screen5", key::screen(3, 5)),
    ("arrangement3-screen6", key::screen(3, 6)),
    ("arrangement3-screen7", key::screen(3, 7)),
];

pub fn config_name(k: u16) -> &'static str {
    CONFIG_NAMES.iter().find(|(_, v)| *v == k).map_or("?", |(n, _)| n)
}

pub fn config_key(name: &str) -> Option<u16> {
    CONFIG_NAMES.iter().find(|(n, _)| *n == name).map(|(_, k)| *k)
}

pub fn color(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

pub fn parse_color(s: &str) -> Result<Rgb, String> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return Err(fl!("parse-colour", value = s));
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| fl!("parse-colour", value = s));
    Ok(Rgb::new(byte(0)?, byte(2)?, byte(4)?))
}

pub fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "yes" | "true" | "1" => Ok(true),
        "off" | "no" | "false" | "0" => Ok(false),
        _ => Err(fl!("parse-one-of", value = s, choices = "on, off")),
    }
}

/// An entry of the hotkey list as text: `<action> <hotkey>` (`select-pc1 ctrl+alt+1`), `none` when unused.
pub fn format_binding(b: &HotkeyBinding) -> String {
    if b.is_used() { format!("{} {}", b.action.name(), format_spec(&b.hotkey)) } else { "none".into() }
}

/// The action named `name` (`toggle-pc`, `select-host2`, …; the old hotkey names `toggle`, `pc1`, `next` too).
pub fn parse_action(name: &str) -> Result<HotkeyAction, String> {
    HotkeyAction::from_name(name.trim()).filter(|a| *a != HotkeyAction::None).ok_or_else(|| {
        let names: Vec<&str> =
            HotkeyAction::ALL.iter().filter(|a| **a != HotkeyAction::None).map(|a| a.name()).collect();
        fl!("parse-one-of", value = name, choices = names.join(", "))
    })
}

/// [`format_binding`]'s text back: `none`, or an action and a hotkey (`none` for one not set yet).
pub fn parse_binding(s: &str) -> Result<HotkeyBinding, String> {
    let s = s.trim();
    if s == "none" {
        return Ok(HotkeyBinding::NONE);
    }
    let (action, hotkey) = s.split_once(char::is_whitespace).unwrap_or((s, "none"));
    let hotkey =
        parse_spec(hotkey.trim()).map_err(|e| fl!("parse-hotkey", value = hotkey, error = format!("{e:?}")))?;
    Ok(HotkeyBinding::new(parse_action(action)?, hotkey))
}

pub fn format_value(v: &ConfigValue) -> String {
    let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
    match v {
        ConfigValue::Hotkey(_, b) => format_binding(b),
        ConfigValue::BtName(_, n) | ConfigValue::ArrangementName(_, n) => n.as_str().into(),
        ConfigValue::BtPolicy(_, p) => p.name().into(),
        ConfigValue::BtControl(_, b) => on_off(*b),
        ConfigValue::BtPointer(_, p) => p.name().into(),
        ConfigValue::ProfileArrangement(_, a)
        | ConfigValue::ProfileKeymap(_, a)
        | ConfigValue::ProfileFallback(_, a) => a.map_or("none".into(), |a| a.to_string()),
        ConfigValue::HostLayout(_, l) => Layout::from_u8(*l).unwrap_or_default().name().into(),
        ConfigValue::Gamepad(g) => gamepad_name(*g).into(),
        ConfigValue::DoubleTapMs(ms) | ConfigValue::LongPressMs(ms) => ms.to_string(),
        ConfigValue::LedBrightness(b) => b.to_string(),
        ConfigValue::ColorPc1(c) | ConfigValue::ColorPc2(c) => color(*c),
        ConfigValue::PcPolling(Polling::Us125) => "125us".into(),
        ConfigValue::PcPolling(Polling::Ms1) => "1ms".into(),
        ConfigValue::Mode(_, m) => m.name().into(),
        ConfigValue::Route(_, r) => route_name(*r),
        ConfigValue::Vial(m) => m.name().into(),
        ConfigValue::KeyTap(a) | ConfigValue::KeyLong(a) => a.name().into(),
        ConfigValue::Profile(i) | ConfigValue::ProfileCount(i) => i.to_string(),
        ConfigValue::ProfileModes(_, m) => m.map(|m| m.name()).join(","),
        ConfigValue::ProfileRoutes(_, r) => r.map(route_name).join(","),
        ConfigValue::ProfileName(_, n) => n.as_str().into(),
        ConfigValue::PowerCycleOnSwitch(b)
        | ConfigValue::SwSwitchAllowed(b)
        | ConfigValue::SwConfigAllowed(b)
        | ConfigValue::AutoSelect(b)
        | ConfigValue::StandbyWake(b)
        | ConfigValue::StandbyGlow(b)
        | ConfigValue::RememberProfile(b)
        | ConfigValue::AbsPointer(b) => on_off(*b),
        ConfigValue::HopAccel(a) => a.name().into(),
        ConfigValue::HopSpeed(v) => v.to_string(),
        ConfigValue::HopResistance(v) | ConfigValue::HopCorner(v) => v.to_string(),
        ConfigValue::HopMapping(_, m) => m.name().into(),
        ConfigValue::HopUnits(_, u) => u.name().into(),
        ConfigValue::Screen(_, _, s) => format_screen(s),
    }
}

pub fn parse_value(k: u16, s: &str) -> Result<ConfigValue, String> {
    let num = |max: u32| -> Result<u32, String> {
        s.trim().parse::<u32>().ok().filter(|v| *v <= max).ok_or_else(|| fl!("parse-number", value = s, max = max))
    };
    Ok(match k {
        k if let Some(i) = key::hotkey_of(k) => ConfigValue::Hotkey(i, parse_binding(s)?),
        k if (key::BT_NAME_BASE..key::BT_NAME_BASE + BT_HOSTS as u16).contains(&k) => {
            let s = s.trim();
            if s.len() > PROFILE_NAME_MAX {
                return Err(fl!("parse-host-name", value = s, max = PROFILE_NAME_MAX));
            }
            ConfigValue::BtName((k - key::BT_NAME_BASE) as u8, ProfileName::new(s))
        }
        k if (key::BT_POLICY_BASE..key::BT_POLICY_BASE + BT_HOSTS as u16).contains(&k) => ConfigValue::BtPolicy(
            (k - key::BT_POLICY_BASE) as u8,
            BtPolicy::from_name(s).ok_or_else(|| fl!("parse-one-of", value = s, choices = "stay, on-select"))?,
        ),
        k if (key::BT_CONTROL_BASE..key::BT_CONTROL_BASE + BT_HOSTS as u16).contains(&k) => {
            ConfigValue::BtControl((k - key::BT_CONTROL_BASE) as u8, parse_bool(s)?)
        }
        key::DOUBLE_TAP_MS => ConfigValue::DoubleTapMs(num(1000)? as u16),
        key::LONG_PRESS_MS => ConfigValue::LongPressMs(num(u16::MAX as u32)? as u16),
        key::REMEMBER_PROFILE => ConfigValue::RememberProfile(parse_bool(s)?),
        key::LED_BRIGHTNESS => ConfigValue::LedBrightness(num(255)? as u8),
        key::COLOR_PC1 => ConfigValue::ColorPc1(parse_color(s)?),
        key::COLOR_PC2 => ConfigValue::ColorPc2(parse_color(s)?),
        key::PC_POLLING => ConfigValue::PcPolling(match s {
            "125us" | "125" | "8khz" => Polling::Us125,
            "1ms" | "1000" | "1khz" => Polling::Ms1,
            _ => return Err(fl!("parse-one-of", value = s, choices = "125us, 1ms")),
        }),
        key::POWER_CYCLE_ON_SWITCH => ConfigValue::PowerCycleOnSwitch(parse_bool(s)?),
        k if let Some(d) = key::mode_of(k).and_then(|d| Device::from_index(d as usize)) => {
            ConfigValue::Mode(d, parse_mode(d, s)?)
        }
        k if let Some(d) = key::route_of(k).and_then(|d| Device::from_index(d as usize)) => {
            ConfigValue::Route(d, parse_route(s)?)
        }
        key::SW_SWITCH_ALLOWED => ConfigValue::SwSwitchAllowed(parse_bool(s)?),
        key::SW_CONFIG_ALLOWED => ConfigValue::SwConfigAllowed(parse_bool(s)?),
        key::VIAL => ConfigValue::Vial(
            VialMode::from_name(s).ok_or_else(|| fl!("parse-one-of", value = s, choices = "off, on, auto"))?,
        ),
        key::GAMEPAD => ConfigValue::Gamepad(
            [GamepadOut::Off, GamepadOut::Hid, GamepadOut::XInput]
                .into_iter()
                .find(|g| gamepad_name(*g) == s)
                .ok_or_else(|| fl!("parse-one-of", value = s, choices = "off, hid, xinput"))?,
        ),
        key::AUTO_SELECT => ConfigValue::AutoSelect(parse_bool(s)?),
        key::STANDBY_WAKE => ConfigValue::StandbyWake(parse_bool(s)?),
        key::STANDBY_GLOW => ConfigValue::StandbyGlow(parse_bool(s)?),
        key::KEY_TAP | key::KEY_LONG => {
            let names = KeyAction::ALL.iter().map(|a| a.name()).collect::<Vec<_>>().join(", ");
            let a = KeyAction::from_name(s).ok_or_else(|| fl!("parse-one-of", value = s, choices = names.clone()))?;
            if k == key::KEY_TAP { ConfigValue::KeyTap(a) } else { ConfigValue::KeyLong(a) }
        }
        key::PROFILE => ConfigValue::Profile(num(MAX_PROFILES as u32 - 1)? as u8),
        key::PROFILE_COUNT => ConfigValue::ProfileCount(num(MAX_PROFILES as u32)? as u8),
        k if let Some(i) = key::profile_of(k, key::PROFILE_MODES_BASE) => {
            ConfigValue::ProfileModes(i, parse_profile_modes(s)?)
        }
        k if let Some(i) = key::profile_of(k, key::PROFILE_ROUTES_BASE) => {
            ConfigValue::ProfileRoutes(i, parse_profile_routes(s)?)
        }
        k if (key::PROFILE_NAME_BASE..key::PROFILE_NAME_BASE + MAX_PROFILES as u16).contains(&k) => {
            ConfigValue::ProfileName((k - key::PROFILE_NAME_BASE) as u8, parse_profile_name(s)?)
        }
        key::ABS_POINTER => ConfigValue::AbsPointer(parse_bool(s)?),
        key::HOP_ACCEL => ConfigValue::HopAccel(
            Accel::from_name(s.trim())
                .ok_or_else(|| fl!("parse-one-of", value = s, choices = "flat, precision, adaptive"))?,
        ),
        key::HOP_SPEED => {
            ConfigValue::HopSpeed(s.trim().parse::<i8>().ok().filter(|v| HOP_SPEED.contains(v)).ok_or_else(|| {
                let (min, max) = (i32::from(*HOP_SPEED.start()), i32::from(*HOP_SPEED.end()));
                fl!("parse-range", value = s, min = min, max = max)
            })?)
        }
        key::HOP_RESISTANCE => ConfigValue::HopResistance(num(HOP_RESISTANCE_MAX as u32)? as u16),
        key::HOP_CORNER => ConfigValue::HopCorner(num(HOP_CORNER_MAX as u32)? as u16),
        k if let Some(h) = Host::ALL.iter().copied().find(|h| key::hop_mapping(*h) == k) => ConfigValue::HopMapping(
            h,
            Mapping::from_name(s.trim())
                .ok_or_else(|| fl!("parse-one-of", value = s, choices = "desktop, main-screen"))?,
        ),
        k if let Some(h) = Host::ALL.iter().copied().find(|h| key::hop_units(*h) == k) => ConfigValue::HopUnits(
            h,
            Units::from_name(s.trim()).ok_or_else(|| fl!("parse-one-of", value = s, choices = "physical, logical"))?,
        ),
        k if (key::BT_POINTER_BASE..key::BT_POINTER_BASE + BT_HOSTS as u16).contains(&k) => ConfigValue::BtPointer(
            (k - key::BT_POINTER_BASE) as u8,
            BtPointer::from_name(s.trim())
                .ok_or_else(|| fl!("parse-one-of", value = s, choices = "absolute, relative"))?,
        ),
        k if let Some(i) = key::profile_of(k, key::PROFILE_KEYMAP_BASE) => {
            ConfigValue::ProfileKeymap(i, parse_arrangement(s)?)
        }
        k if let Some(i) = key::profile_of(k, key::PROFILE_FALLBACK_BASE) => {
            ConfigValue::ProfileFallback(i, parse_arrangement(s)?)
        }
        k if (key::LAYOUT_BASE..key::LAYOUT_BASE + kvm_proto::ctrl::HOSTS as u16).contains(&k) => {
            let h = Host::from_index((k - key::LAYOUT_BASE) as usize).unwrap_or(Host::Pc1);
            ConfigValue::HostLayout(h, parse_layout(s)? as u8)
        }
        k if let Some(i) = key::profile_of(k, key::PROFILE_ARRANGEMENT_BASE) => {
            ConfigValue::ProfileArrangement(i, parse_arrangement(s)?)
        }
        k if (key::ARRANGEMENT_NAME_BASE..key::ARRANGEMENT_NAME_BASE + MAX_ARRANGEMENTS as u16).contains(&k) => {
            let s = s.trim();
            if s.len() > PROFILE_NAME_MAX {
                return Err(fl!("parse-host-name", value = s, max = PROFILE_NAME_MAX));
            }
            ConfigValue::ArrangementName((k - key::ARRANGEMENT_NAME_BASE) as u8, ProfileName::new(s))
        }
        k if let Some(a) = key::arrangement_of(k) => {
            ConfigValue::Screen(a, (k - key::screen(a, 0)) as u8, parse_screen(s)?)
        }
        _ => return Err(fl!("parse-unknown-setting")),
    })
}

/// A profile's arrangement: `none` (screen hop off), or its number 0–3.
/// The virtual gamepad setting's name (`config set gamepad`).
pub const fn gamepad_name(g: GamepadOut) -> &'static str {
    match g {
        GamepadOut::Off => "off",
        GamepadOut::Hid => "hid",
        GamepadOut::XInput => "xinput",
    }
}

/// An arrangement or keymap: `none` or 0–3.
/// A host keyboard layout by its name (`us`, `de`, …).
pub fn parse_layout(s: &str) -> Result<Layout, String> {
    Layout::from_name(s.trim()).ok_or_else(|| {
        let names = Layout::ALL.iter().map(|l| l.name()).collect::<Vec<_>>().join(", ");
        fl!("parse-one-of", value = s, choices = names)
    })
}

pub fn parse_arrangement(s: &str) -> Result<Option<u8>, String> {
    match s.trim() {
        "none" => Ok(None),
        n => n
            .parse::<u8>()
            .ok()
            .filter(|a| (*a as usize) < MAX_ARRANGEMENTS)
            .map(Some)
            .ok_or_else(|| fl!("parse-one-of", value = s, choices = "none, 0, 1, 2, 3")),
    }
}

/// A mode for `d`: one it can have (a Bluetooth device takes `auto`, `normalized` and `off`).
pub fn parse_mode(d: Device, s: &str) -> Result<PortMode, String> {
    let names = || d.modes().iter().map(|m| m.name()).collect::<Vec<_>>().join(", ");
    PortMode::from_name(s.trim())
        .filter(|m| d.takes(*m))
        .ok_or_else(|| fl!("parse-mode", value = s.trim(), modes = names()))
}

/// Every device's mode, in the order Keyboard, Mouse, Aux 1, Aux 2, wireless 1–4:
/// `auto,auto,mirrored,passthrough,auto,auto,off,auto`.
pub fn parse_profile_modes(s: &str) -> Result<[PortMode; DEVICES], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != DEVICES {
        return Err(fl!("parse-profile-modes", value = s));
    }
    let mut out = [PortMode::Auto; DEVICES];
    for (i, (m, d)) in parts.iter().zip(Device::ALL).enumerate() {
        out[i] = parse_mode(*d, m)?;
    }
    Ok(out)
}

/// Every device's routing, in the same order: `follow,follow,follow,pc2,host1,follow,follow,follow`.
pub fn parse_profile_routes(s: &str) -> Result<[Routing; DEVICES], String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != DEVICES {
        return Err(fl!("parse-profile-routes", value = s));
    }
    let mut out = [Routing::Follow; DEVICES];
    for (i, r) in parts.iter().enumerate() {
        out[i] = parse_route(r)?;
    }
    Ok(out)
}

pub fn parse_profile_name(s: &str) -> Result<ProfileName, String> {
    let s = s.trim();
    if s.is_empty() || s.len() > PROFILE_NAME_MAX {
        return Err(fl!("parse-profile-name", value = s, max = PROFILE_NAME_MAX));
    }
    Ok(ProfileName::new(s))
}

pub fn mode_names() -> String {
    PortMode::ALL.iter().map(|m| m.name()).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_power_line_says_what_is_measured_and_warns_only_when_the_5_v_sags() {
        let mut c = crate::Client::new(Box::new(crate::sim::SimTransport::with_bluetooth(false)));
        let mut st = c.status().unwrap();
        assert_eq!(st.supply, Supply::Good);
        st = Status { supply_mv: 4950, supply_lowest_mv: 4870, budget_ma: 300, declared_ma: 200, ..st };
        assert_eq!(power_line(&st), "From PC USB: 4.95 V, lowest 4.87 V in the last minute");
        assert_eq!((power_warning(&st, false), power_note(&st)), (None, None));
        // More declared than the estimate, but the 5 V holds: a note why that is fine, no warning.
        st.declared_ma = 700;
        assert!(power_note(&st).is_some_and(|n| n.contains("700 mA")));
        assert_eq!(power_warning(&st, false), None);
        st = Status { supply: Supply::Sags, flags: st.flags | kvm_proto::ctrl::status_flags::SUPPLY_SAGS, ..st };
        assert_eq!(power_warning(&st, true), Some(("5 V sags".into(), false)));
        assert_eq!(power_note(&st), None);
        st.supply = Supply::Low;
        assert!(power_warning(&st, false).is_some_and(|(_, bad)| bad));
        // Before the first reading: the estimate, and no warning.
        st = Status { supply: Supply::Unknown, flags: 0, ..st };
        assert_eq!(power_line(&st), "From PC USB, about 300 mA left for the front ports");
        assert_eq!(power_warning(&st, false), None);
    }

    #[test]
    fn a_learned_hotkey_warns_of_plain_keys_and_twins() {
        let others = [("hotkey-toggle", Hotkey::double_tap(0x47, true)), ("hotkey-pc2", Hotkey::NONE)];
        assert_eq!(hotkey_warning(&Hotkey::combo(0x05, 0x0E), &others), None);
        assert_eq!(hotkey_warning(&Hotkey::combo(0, 0x68), &others), None, "f13 alone is spare");
        assert_eq!(
            hotkey_warning(&Hotkey::combo(0, 0x3E), &others).as_deref(),
            Some("F5 by itself would switch, and the PCs would no longer get it.")
        );
        let twin = hotkey_warning(&Hotkey::double_tap(0x47, true), &others).unwrap();
        assert!(twin.contains("hotkey-toggle"), "{twin}");
        assert_eq!(hotkey_warning(&Hotkey::NONE, &others), None, "no hotkey twins another unset one");
    }

    #[test]
    fn every_setting_formats_and_parses_back() {
        let cfg = kvm_core::config::Config::default();
        for v in cfg.all() {
            let text = format_value(&v);
            assert_eq!(parse_value(v.key(), &text), Ok(v), "{}", config_name(v.key()));
        }
        assert!(parse_value(key::COLOR_PC1, "#12345").is_err());
        assert_eq!(config_key("mode-mouse"), Some(key::MODE_BASE + 1));
        assert_eq!(config_key("route-wireless3"), Some(key::ROUTE_BASE + 6));
        assert_eq!(config_key("mode-aux1"), Some(key::MODE_BASE + 2));
        assert_eq!(config_key("profile7-routes"), Some(key::PROFILE_ROUTES_BASE + 7));
        assert_eq!(
            parse_value(key::MODE_BASE + 4, "passthrough").map(|_| ()),
            Err(fl!("parse-mode", value = "passthrough", modes = "auto, normalized, off"))
        );
        assert_eq!(
            parse_value(key::MODE_BASE + 2, "passthrough").map(|_| ()),
            Err(fl!("parse-mode", value = "passthrough", modes = "auto, normalized, mirrored, identity, off"))
        );
        assert_eq!(
            parse_value(key::ROUTE_BASE, "host2"),
            Ok(ConfigValue::Route(Device::Keyboard, Routing::Pin(Host::Bt2)))
        );
        let routes = parse_profile_routes("follow,pc1,follow,pc2,host1,follow,follow,host4").unwrap();
        assert_eq!(
            format_value(&ConfigValue::ProfileRoutes(0, routes)),
            "follow,pc1,follow,pc2,host1,follow,follow,host4"
        );
        assert!(parse_profile_modes("auto,auto,passthrough").is_err(), "all eight");
        assert!(parse_profile_modes("auto,auto,passthrough,auto,auto,auto,auto,auto").is_err(), "not on Aux 1");
        assert!(parse_profile_modes("auto,auto,mirrored,passthrough,auto,auto,off,auto").is_ok());
        // An entry of the hotkey list: an action and a hotkey, or none.
        let k = config_key("hotkey3").unwrap();
        let v = parse_value(k, "select-host3 ctrl+alt+3").unwrap();
        let host3 = HotkeyBinding::new(HotkeyAction::SelectHost(Host::Bt3), Hotkey::combo(0x05, 0x20));
        assert_eq!(v, ConfigValue::Hotkey(2, host3));
        assert_eq!((format_value(&v), v.key()), ("select-host3 ctrl+alt+3".to_string(), k));
        assert_eq!(parse_value(k, "none"), Ok(ConfigValue::Hotkey(2, HotkeyBinding::NONE)));
        let pc1 = HotkeyBinding::new(HotkeyAction::SelectHost(Host::Pc1), Hotkey::NONE);
        assert_eq!(parse_binding("select-pc1"), Ok(pc1), "an entry with no hotkey yet");
        assert!(parse_binding("pc1").is_err());
        assert!(parse_binding("dance ctrl+alt+3").is_err());
        // Every key has a name, and the names are distinct.
        for k in key::ALL {
            assert_ne!(config_name(*k), "?", "{k:#06x}");
        }
        let mut names: Vec<_> = CONFIG_NAMES.iter().map(|(n, _)| *n).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), CONFIG_NAMES.len());
    }

    #[test]
    fn arrangement_settings() {
        assert_eq!(config_key("arrangement3-screen7"), Some(0x00CF));
        assert_eq!((config_key("screen5"), config_key("bt4-pointer")), (None, None));
        let k = config_key("profile1-arrangement").unwrap();
        assert!(parse_value(k, "on").is_err());
        assert_eq!(parse_value(k, "none"), Ok(ConfigValue::ProfileArrangement(1, None)));
        assert_eq!(parse_value(k, "2"), Ok(ConfigValue::ProfileArrangement(1, Some(2))));
        assert!(parse_value(k, "4").is_err());
        let p = config_key("host1-pointer").unwrap();
        assert_eq!(parse_value(p, "relative"), Ok(ConfigValue::BtPointer(0, BtPointer::Relative)));
    }

    #[test]
    fn screens_format_and_parse() {
        let s = Screen { scale: 150, desk: [-3840, 0], ..Screen::new(Host::Pc2, 3840, 2160, [-2560, 120]) };
        assert_eq!(format_screen(&s), "pc2 3840x2160 150% at -2560,120 desk -3840,0");
        assert_eq!(parse_screen(&format_screen(&s)), Ok(s));
        assert_eq!(parse_screen("pc1 1920x1080"), Ok(Screen::new(Host::Pc1, 1920, 1080, [0, 0])));
        // A wireless host's: hostN, as in `switch` (btN too).
        let tablet = Screen::new(Host::Bt3, 2388, 1668, [1920, 0]);
        assert_eq!(format_screen(&tablet), "host3 2388x1668 100% at 1920,0 desk 0,0");
        assert_eq!(parse_screen("bt3 2388x1668 at 1920,0"), Ok(tablet));
        assert_eq!(parse_screen("off"), Ok(Screen::NONE));
        for bad in ["pc3 1920x1080", "pc1 1920", "pc1 10x10", "pc1 1920x1080 at 5", "pc1 1920x1080 50%"] {
            assert!(parse_screen(bad).is_err(), "{bad}");
        }
        let off = Screen { off: true, fixed: true, ..s };
        assert_eq!(format_screen(&off), "pc2 3840x2160 150% at -2560,120 desk -3840,0 no-hop fixed");
        assert_eq!(parse_screen(&format_screen(&off)), Ok(off));
        assert_eq!(
            parse_screen("pc1 1920x1080 no-hop"),
            Ok(Screen { off: true, ..Screen::new(Host::Pc1, 1920, 1080, [0, 0]) })
        );
        assert_eq!(
            parse_value(key::HOP_UNITS_BASE + 1, "logical"),
            Ok(ConfigValue::HopUnits(Host::Pc2, Units::Logical))
        );
        assert_eq!(
            parse_value(config_key("hop-mapping-host2").unwrap(), "main-screen"),
            Ok(ConfigValue::HopMapping(Host::Bt2, Mapping::MainScreen))
        );
        assert!(parse_value(key::HOP_UNITS_BASE, "metric").is_err());
        assert!(parse_value(key::HOP_SPEED, "11").is_err());
        assert_eq!(parse_value(key::HOP_SPEED, "-4"), Ok(ConfigValue::HopSpeed(-4)));
    }
}
