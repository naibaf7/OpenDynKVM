//! Human-readable names and text forms of protocol values, shared by the CLI and the GUI.

use kvm_core::hotkey::{format_spec, parse_spec};

use crate::{fl, i18n};
use kvm_proto::ctrl::{
    Accel, Advertised, AutoReason, BT_HOSTS, BtPolicy, ConfigValue, ErrorCode, FaultKind, FwState, HOP_CORNER_MAX,
    HOP_RESISTANCE_MAX, HOP_SPEED, HopState, Hotkey, HotkeyKind, KeyAction, LearnEnd, LinkState, MAX_PROFILES,
    MAX_SCREENS, Mapping, PROFILE_NAME_MAX, Pc, Polling, Port, PortMode, PortState, PowerSource, ProfileName, Rgb,
    Screen, Speed, SwitchSource, VialMode, class, diag, key,
};

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
        Port::Other => fl!("port-other"),
    }
}

/// The port's name in a column of port modes.
pub fn port_short(p: Port) -> String {
    match p {
        Port::Keyboard => fl!("port-short-keyboard"),
        Port::Mouse => fl!("port-short-mouse"),
        Port::Other => fl!("port-short-other"),
    }
}

pub fn fault_kind(f: FaultKind) -> String {
    match f {
        FaultKind::OverCurrent => fl!("fault-over-current"),
        FaultKind::EnumerationFailed => fl!("fault-enumeration"),
        FaultKind::LinkDown => fl!("fault-link-down"),
        FaultKind::PowerBudget => fl!("fault-power-budget"),
        FaultKind::HubError => fl!("fault-hub"),
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
    }
}

pub fn mode(m: PortMode) -> String {
    match m {
        PortMode::Auto => fl!("mode-auto"),
        PortMode::Normalized => fl!("mode-normalized"),
        PortMode::Mirrored => fl!("mode-mirrored"),
        PortMode::MirroredIdentity => fl!("mode-mirrored-identity"),
        PortMode::Passthrough => fl!("mode-passthrough"),
        PortMode::PassthroughPc1 => fl!("mode-passthrough-pc1"),
        PortMode::PassthroughPc2 => fl!("mode-passthrough-pc2"),
    }
}

/// The short form, for the profile overview.
pub fn mode_short(m: PortMode) -> String {
    match m {
        PortMode::Auto => fl!("mode-short-auto"),
        PortMode::Normalized => fl!("mode-short-normalized"),
        PortMode::Mirrored => fl!("mode-short-mirrored"),
        PortMode::MirroredIdentity => fl!("mode-short-identity"),
        PortMode::Passthrough => fl!("mode-short-passthrough"),
        PortMode::PassthroughPc1 => fl!("mode-short-pinned-pc1"),
        PortMode::PassthroughPc2 => fl!("mode-short-pinned-pc2"),
    }
}

pub fn key_action(a: KeyAction) -> String {
    match a {
        KeyAction::None => fl!("key-action-none"),
        KeyAction::TogglePc => fl!("key-action-toggle-pc"),
        KeyAction::NextProfile => fl!("key-action-next-profile"),
        KeyAction::OtherPortMode => fl!("key-action-other-port"),
        KeyAction::NextTarget => fl!("key-action-next-target"),
    }
}

pub fn reason(r: AutoReason) -> String {
    match r {
        AutoReason::NotAuto => fl!("reason-not-auto"),
        AutoReason::FitsNormalized => fl!("reason-fits-normalized"),
        AutoReason::ExtraInputs => fl!("reason-extra-inputs"),
        AutoReason::OtherPortHid => fl!("reason-other-port-hid"),
        AutoReason::NonHid => fl!("reason-non-hid"),
        AutoReason::XInput => fl!("reason-xinput"),
        AutoReason::EndpointBudget => fl!("reason-endpoint-budget"),
        AutoReason::IdentityTaken => fl!("reason-identity-taken"),
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
        diag::BtModule::Failed => fl!("bt-module-failed"),
        diag::BtModule::GaveUp => fl!("bt-module-gave-up"),
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
        (PORT_OT, fl!("selftest-port-ot")),
        (POWER, fl!("selftest-power")),
        (AT32_FW1, fl!("selftest-at32-fw1")),
        (AT32_FW2, fl!("selftest-at32-fw2")),
        (AT32_GAVE_UP1, fl!("selftest-at32-gave-up1")),
        (AT32_GAVE_UP2, fl!("selftest-at32-gave-up2")),
        (AT32_XTAL1, fl!("selftest-at32-xtal1")),
        (AT32_XTAL2, fl!("selftest-at32-xtal2")),
        (BT_FW, fl!("selftest-bt-fw")),
        (BT_GAVE_UP, fl!("selftest-bt-gave-up")),
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
        HopState::Wireless => fl!("hop-state-wireless"),
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

/// A screen slot as the CLI writes it: `pc1 2560x1440 150% at 1920,0 desk 0,0`, or `off`.
pub fn format_screen(s: &Screen) -> String {
    match s.pc {
        None => "off".into(),
        Some(pc) => format!(
            "{} {}x{} {}% at {},{} desk {},{}",
            pc.name(),
            s.w,
            s.h,
            s.scale,
            s.at[0],
            s.at[1],
            s.desk[0],
            s.desk[1]
        ),
    }
}

/// The CLI's form of a screen slot ([`format_screen`]); the scaling (100 %) and both positions (0,0) may be left out.
pub fn parse_screen(text: &str) -> Result<Screen, String> {
    let bad = || fl!("parse-screen", value = text);
    let mut words = text.split_whitespace();
    let pc = match words.next() {
        Some("off" | "none") => return Ok(Screen::NONE),
        Some("pc1" | "1") => Pc::Pc1,
        Some("pc2" | "2") => Pc::Pc2,
        _ => return Err(bad()),
    };
    let point = |w: Option<&str>| -> Result<[i16; 2], String> {
        let (x, y) = w.and_then(|w| w.split_once(',')).ok_or_else(bad)?;
        Ok([x.trim().parse().map_err(|_| bad())?, y.trim().parse().map_err(|_| bad())?])
    };
    let (w, h) = words.next().and_then(|r| r.split_once(['x', '×'])).ok_or_else(bad)?;
    let mut s = Screen::new(pc, w.parse().map_err(|_| bad())?, h.parse().map_err(|_| bad())?, [0, 0]);
    while let Some(word) = words.next() {
        match word {
            "at" => s.at = point(words.next())?,
            "desk" => s.desk = point(words.next())?,
            w if w.ends_with('%') => s.scale = w.trim_end_matches('%').parse().map_err(|_| bad())?,
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
        OT_SPEED => fl!("diag-ot-speed"),
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
        KB_FAULTS => fl!("diag-kb-faults"),
        MS_FAULTS => fl!("diag-ms-faults"),
        OT_FAULTS => fl!("diag-ot-faults"),
        AT1_TEMP => fl!("diag-at-temp", n = 1),
        AT2_TEMP => fl!("diag-at-temp", n = 2),
        AT1_FW => fl!("diag-at-fw", n = 1),
        AT2_FW => fl!("diag-at-fw", n = 2),
        BT_MODULE => fl!("diag-bt-module"),
        BT_FW => fl!("diag-bt-fw"),
        BT_LINK_ERRORS => fl!("diag-bt-link-errors"),
        BT_HOSTS => fl!("diag-bt-hosts"),
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
        KB_FAULTS | MS_FAULTS | OT_FAULTS => fl!("diag-help-faults"),
        AT1_TEMP | AT2_TEMP => fl!("diag-help-at-temp"),
        AT1_FW | AT2_FW => fl!("diag-help-at-fw"),
        BT_MODULE => fl!("diag-help-bt-module"),
        BT_FW => fl!("diag-help-bt-fw"),
        BT_LINK_ERRORS => fl!("diag-help-bt-link-errors"),
        BT_HOSTS => fl!("diag-help-bt-hosts"),
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
    ("hotkey-toggle", key::HOTKEY_TOGGLE),
    ("hotkey-pc1", key::HOTKEY_PC1),
    ("hotkey-pc2", key::HOTKEY_PC2),
    ("double-tap-ms", key::DOUBLE_TAP_MS),
    ("key-tap", key::KEY_TAP),
    ("key-long", key::KEY_LONG),
    ("long-press-ms", key::LONG_PRESS_MS),
    ("led-brightness", key::LED_BRIGHTNESS),
    ("color-pc1", key::COLOR_PC1),
    ("color-pc2", key::COLOR_PC2),
    ("polling", key::PC_POLLING),
    ("power-cycle", key::POWER_CYCLE_ON_SWITCH),
    ("mode-keyboard", key::PORT_MODE_BASE),
    ("mode-mouse", key::PORT_MODE_BASE + 1),
    ("mode-other", key::PORT_MODE_BASE + 2),
    ("software-switching", key::SW_SWITCH_ALLOWED),
    ("software-config", key::SW_CONFIG_ALLOWED),
    ("vial", key::VIAL),
    ("keymap", key::KEYMAP),
    ("auto-select", key::AUTO_SELECT),
    ("standby-wake", key::STANDBY_WAKE),
    ("standby-glow", key::STANDBY_GLOW),
    ("profiles", key::PROFILE_COUNT),
    ("profile", key::PROFILE),
    ("remember-profile", key::REMEMBER_PROFILE),
    ("profile0-modes", key::PROFILE_MODES_BASE),
    ("profile1-modes", key::PROFILE_MODES_BASE + 1),
    ("profile2-modes", key::PROFILE_MODES_BASE + 2),
    ("profile3-modes", key::PROFILE_MODES_BASE + 3),
    ("profile0-name", key::PROFILE_NAME_BASE),
    ("profile1-name", key::PROFILE_NAME_BASE + 1),
    ("profile2-name", key::PROFILE_NAME_BASE + 2),
    ("profile3-name", key::PROFILE_NAME_BASE + 3),
    ("hotkey-next", key::HOTKEY_NEXT),
    ("host1-name", key::BT_NAME_BASE),
    ("host2-name", key::BT_NAME_BASE + 1),
    ("host3-name", key::BT_NAME_BASE + 2),
    ("host4-name", key::BT_NAME_BASE + 3),
    ("host1-policy", key::BT_POLICY_BASE),
    ("host2-policy", key::BT_POLICY_BASE + 1),
    ("host3-policy", key::BT_POLICY_BASE + 2),
    ("host4-policy", key::BT_POLICY_BASE + 3),
    ("abs-pointer", key::ABS_POINTER),
    ("hop-accel", key::HOP_ACCEL),
    ("hop-speed", key::HOP_SPEED),
    ("hop-resistance", key::HOP_RESISTANCE),
    ("hop-corner", key::HOP_CORNER),
    ("hop-mapping-pc1", key::HOP_MAPPING_BASE),
    ("hop-mapping-pc2", key::HOP_MAPPING_BASE + 1),
    ("profile0-hop", key::PROFILE_HOP_BASE),
    ("profile1-hop", key::PROFILE_HOP_BASE + 1),
    ("profile2-hop", key::PROFILE_HOP_BASE + 2),
    ("profile3-hop", key::PROFILE_HOP_BASE + 3),
    ("screen0", key::SCREEN_BASE),
    ("screen1", key::SCREEN_BASE + 1),
    ("screen2", key::SCREEN_BASE + 2),
    ("screen3", key::SCREEN_BASE + 3),
    ("screen4", key::SCREEN_BASE + 4),
    ("screen5", key::SCREEN_BASE + 5),
    ("screen6", key::SCREEN_BASE + 6),
    ("screen7", key::SCREEN_BASE + 7),
];

pub fn config_name(k: u16) -> &'static str {
    CONFIG_NAMES.iter().find(|(_, v)| *v == k).map_or("?", |(n, _)| n)
}

pub fn config_key(name: &str) -> Option<u16> {
    // The wireless hosts' settings were called bt1-name … bt4-policy before.
    let name = match name.strip_prefix("bt") {
        Some(rest) if rest.starts_with(['1', '2', '3', '4']) => format!("host{rest}"),
        _ => name.to_string(),
    };
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

fn parse_bool(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "yes" | "true" | "1" => Ok(true),
        "off" | "no" | "false" | "0" => Ok(false),
        _ => Err(fl!("parse-one-of", value = s, choices = "on, off")),
    }
}

pub fn format_value(v: &ConfigValue) -> String {
    let on_off = |b: bool| if b { "on" } else { "off" }.to_string();
    match v {
        ConfigValue::HotkeyToggle(h)
        | ConfigValue::HotkeyPc1(h)
        | ConfigValue::HotkeyPc2(h)
        | ConfigValue::HotkeyNext(h) => format_spec(h).to_string(),
        ConfigValue::BtName(_, n) => n.as_str().into(),
        ConfigValue::BtPolicy(_, p) => p.name().into(),
        ConfigValue::DoubleTapMs(ms) | ConfigValue::LongPressMs(ms) => ms.to_string(),
        ConfigValue::LedBrightness(b) => b.to_string(),
        ConfigValue::ColorPc1(c) | ConfigValue::ColorPc2(c) => color(*c),
        ConfigValue::PcPolling(Polling::Us125) => "125us".into(),
        ConfigValue::PcPolling(Polling::Ms1) => "1ms".into(),
        ConfigValue::PortMode(_, m) => m.name().into(),
        ConfigValue::Vial(m) => m.name().into(),
        ConfigValue::KeyTap(a) | ConfigValue::KeyLong(a) => a.name().into(),
        ConfigValue::Profile(i) | ConfigValue::ProfileCount(i) => i.to_string(),
        ConfigValue::ProfileModes(_, m) => m.map(|m| m.name()).join(","),
        ConfigValue::ProfileName(_, n) => n.as_str().into(),
        ConfigValue::PowerCycleOnSwitch(b)
        | ConfigValue::SwSwitchAllowed(b)
        | ConfigValue::SwConfigAllowed(b)
        | ConfigValue::Keymap(b)
        | ConfigValue::AutoSelect(b)
        | ConfigValue::StandbyWake(b)
        | ConfigValue::StandbyGlow(b)
        | ConfigValue::RememberProfile(b)
        | ConfigValue::AbsPointer(b)
        | ConfigValue::ProfileHop(_, b) => on_off(*b),
        ConfigValue::HopAccel(a) => a.name().into(),
        ConfigValue::HopSpeed(v) => v.to_string(),
        ConfigValue::HopResistance(v) | ConfigValue::HopCorner(v) => v.to_string(),
        ConfigValue::HopMapping(_, m) => m.name().into(),
        ConfigValue::Screen(_, s) => format_screen(s),
    }
}

pub fn parse_value(k: u16, s: &str) -> Result<ConfigValue, String> {
    let hotkey = || parse_spec(s).map_err(|e| fl!("parse-hotkey", value = s, error = format!("{e:?}")));
    let num = |max: u32| -> Result<u32, String> {
        s.trim().parse::<u32>().ok().filter(|v| *v <= max).ok_or_else(|| fl!("parse-number", value = s, max = max))
    };
    Ok(match k {
        key::HOTKEY_TOGGLE => ConfigValue::HotkeyToggle(hotkey()?),
        key::HOTKEY_PC1 => ConfigValue::HotkeyPc1(hotkey()?),
        key::HOTKEY_PC2 => ConfigValue::HotkeyPc2(hotkey()?),
        key::HOTKEY_NEXT => ConfigValue::HotkeyNext(hotkey()?),
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
        k if (key::PORT_MODE_BASE..key::PORT_MODE_BASE + 3).contains(&k) => ConfigValue::PortMode(
            Port::from_index((k - key::PORT_MODE_BASE) as usize).unwrap_or(Port::Other),
            PortMode::from_name(s).ok_or_else(|| fl!("parse-mode", value = s, modes = mode_names()))?,
        ),
        key::SW_SWITCH_ALLOWED => ConfigValue::SwSwitchAllowed(parse_bool(s)?),
        key::SW_CONFIG_ALLOWED => ConfigValue::SwConfigAllowed(parse_bool(s)?),
        key::VIAL => ConfigValue::Vial(
            VialMode::from_name(s).ok_or_else(|| fl!("parse-one-of", value = s, choices = "off, on, auto"))?,
        ),
        key::KEYMAP => ConfigValue::Keymap(parse_bool(s)?),
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
        k if (key::PROFILE_MODES_BASE..key::PROFILE_MODES_BASE + MAX_PROFILES as u16).contains(&k) => {
            ConfigValue::ProfileModes((k - key::PROFILE_MODES_BASE) as u8, parse_profile_modes(s)?)
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
        k if (key::HOP_MAPPING_BASE..key::HOP_MAPPING_BASE + 2).contains(&k) => ConfigValue::HopMapping(
            Pc::from_index((k - key::HOP_MAPPING_BASE) as usize),
            Mapping::from_name(s.trim())
                .ok_or_else(|| fl!("parse-one-of", value = s, choices = "desktop, main-screen"))?,
        ),
        k if (key::PROFILE_HOP_BASE..key::PROFILE_HOP_BASE + MAX_PROFILES as u16).contains(&k) => {
            ConfigValue::ProfileHop((k - key::PROFILE_HOP_BASE) as u8, parse_bool(s)?)
        }
        k if (key::SCREEN_BASE..key::SCREEN_BASE + MAX_SCREENS as u16).contains(&k) => {
            ConfigValue::Screen((k - key::SCREEN_BASE) as u8, parse_screen(s)?)
        }
        _ => return Err(fl!("parse-unknown-setting")),
    })
}

/// Three modes, Keyboard, Mouse and Other: `auto,auto,passthrough`.
pub fn parse_profile_modes(s: &str) -> Result<[PortMode; 3], String> {
    let modes: Vec<PortMode> = s
        .split(',')
        .map(|m| PortMode::from_name(m.trim()).ok_or_else(|| fl!("parse-mode", value = m.trim(), modes = mode_names())))
        .collect::<Result<_, _>>()?;
    modes.try_into().map_err(|_| fl!("parse-profile-modes", value = s))
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
        assert_eq!(config_key("mode-mouse"), Some(key::PORT_MODE_BASE + 1));
    }

    #[test]
    fn screens_format_and_parse() {
        let s = Screen { scale: 150, desk: [-3840, 0], ..Screen::new(Pc::Pc2, 3840, 2160, [-2560, 120]) };
        assert_eq!(format_screen(&s), "pc2 3840x2160 150% at -2560,120 desk -3840,0");
        assert_eq!(parse_screen(&format_screen(&s)), Ok(s));
        assert_eq!(parse_screen("pc1 1920x1080"), Ok(Screen::new(Pc::Pc1, 1920, 1080, [0, 0])));
        assert_eq!(parse_screen("off"), Ok(Screen::NONE));
        for bad in ["pc3 1920x1080", "pc1 1920", "pc1 10x10", "pc1 1920x1080 at 5", "pc1 1920x1080 50%"] {
            assert!(parse_screen(bad).is_err(), "{bad}");
        }
        assert!(parse_value(key::HOP_SPEED, "11").is_err());
        assert_eq!(parse_value(key::HOP_SPEED, "-4"), Ok(ConfigValue::HopSpeed(-4)));
    }
}
