//! `odkvmctl`: command-line control of the OpenDynKVM (software.md §9). Its texts, help included, come from the
//! translations (`odkvmctl::i18n`); clap's own words (Usage, Options, …) stay English.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kvm_core::hop::main_screen;
use kvm_core::hotkey::format_spec;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::ctrl::{
    BT_DEVICES, BtDeviceInfo, BtFound, ConfigValue, Device, DevicePairing, ErrorCode, Event, FwState, Host, Hotkey,
    HotkeyAction, HotkeyBinding, LearnEnd, MAX_ARRANGEMENTS, MAX_HOTKEYS, MAX_PROFILES, Mapping, Pc, Port, PortMode,
    RebootTarget, Routing, Target, diag, key, selftest, status_flags,
};
use odkvmctl::bootsel::{self, Board};
use odkvmctl::client::{BtHostView, DeviceSlot, HopView, ProfileView, Snapshot};
use odkvmctl::ddc::{self, mccs};
use odkvmctl::transport::Link;
use odkvmctl::{Client, firmware, fl, image, keycodes, local, names};

#[derive(Parser)]
#[command(name = "odkvmctl", version, about = fl!("cli-about"))]
struct Cli {
    #[arg(long, global = true, help = fl!("cli-sim"))]
    sim: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    #[command(about = fl!("cli-status"))]
    Status,
    #[command(about = fl!("cli-diag"))]
    Diag,
    #[command(about = fl!("cli-list"))]
    List,
    #[command(about = fl!("cli-watch"))]
    Watch,
    #[command(about = fl!("cli-switch"))]
    Switch {
        #[arg(value_enum)]
        target: TargetArg,
    },
    /// The wireless hosts (the Bluetooth module).
    #[command(about = fl!("cli-wireless"))]
    Wireless {
        #[command(subcommand)]
        action: Option<HostAction>,
    },
    #[command(about = fl!("cli-port"))]
    Port {
        #[arg(value_enum)]
        port: PortArg,
        #[command(subcommand)]
        action: Option<PortAction>,
    },
    /// The wireless devices (keyboards, mice) paired with the Bluetooth module.
    #[command(about = fl!("cli-device"))]
    Device {
        #[command(subcommand)]
        action: Option<DeviceAction>,
    },
    #[command(about = fl!("cli-keymap"))]
    Keymap {
        #[command(subcommand)]
        action: Option<KeymapAction>,
    },
    #[command(about = fl!("cli-calibrate"))]
    Calibrate {
        #[arg(help = fl!("cli-calibrate-device"))]
        device: String,
        #[command(subcommand)]
        action: Option<CalAction>,
    },
    #[command(about = fl!("cli-monitor"))]
    Monitor {
        #[arg(help = fl!("cli-calibrate-device"))]
        device: String,
        #[arg(default_value_t = 20)]
        seconds: u32,
    },
    #[command(about = fl!("cli-field"))]
    Field {
        #[arg(help = fl!("cli-calibrate-device"))]
        device: String,
        #[command(subcommand)]
        action: Option<FieldAction>,
    },
    #[command(about = fl!("cli-outputs"))]
    Outputs,
    #[command(about = fl!("cli-layout"))]
    Layout {
        #[arg(help = fl!("cli-layout-host"))]
        host: Option<String>,
        #[arg(help = fl!("cli-layout-name"))]
        layout: Option<String>,
    },
    #[command(about = fl!("cli-profile"))]
    Profile {
        #[arg(help = fl!("cli-profile-which"))]
        which: Option<String>,
        #[arg(help = fl!("cli-profile-setting"))]
        setting: Option<String>,
        value: Option<String>,
        #[arg(help = fl!("cli-profile-route"))]
        route: Option<String>,
    },
    #[command(about = fl!("cli-hotkey"))]
    Hotkey {
        #[command(subcommand)]
        action: Option<HotkeyCmd>,
    },
    #[command(about = fl!("cli-config"))]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    #[command(about = fl!("cli-identify"))]
    Identify {
        #[arg(default_value_t = 5)]
        seconds: u8,
    },
    #[command(about = fl!("cli-update"))]
    Update {
        #[arg(help = fl!("cli-update-file"))]
        file: Option<PathBuf>,
        #[arg(long, help = fl!("cli-update-module"))]
        module: bool,
    },
    #[command(about = fl!("cli-reboot"))]
    Reboot {
        #[arg(value_enum, default_value_t = RebootArg::Normal)]
        target: RebootArg,
    },
    #[command(about = fl!("cli-display"))]
    Display {
        #[command(subcommand)]
        action: Option<DisplayAction>,
    },
    #[command(about = fl!("cli-screens"))]
    Screens {
        /// The arrangement, by number (0–3) or name; the default is the active profile's.
        #[arg(long, short, help = fl!("cli-screens-arrangement"))]
        arrangement: Option<String>,
        #[command(subcommand)]
        action: Option<ScreensAction>,
    },
}

#[derive(Subcommand)]
enum ScreensAction {
    #[command(about = fl!("cli-screens-list"))]
    List,
    #[command(about = fl!("cli-screens-detect"))]
    Detect,
}

#[derive(Subcommand)]
enum DisplayAction {
    #[command(about = fl!("cli-display-list"))]
    List,
    #[command(about = fl!("cli-display-input"))]
    Input { id: String, value: String },
}

#[derive(Subcommand)]
enum HostAction {
    #[command(about = fl!("cli-hosts-list"))]
    List,
    #[command(about = fl!("cli-hosts-pair"))]
    Pair {
        #[arg(help = fl!("cli-hosts-pair-stop"))]
        stop: Option<String>,
    },
    #[command(about = fl!("cli-hosts-passkey"))]
    Passkey { digits: String },
    #[command(about = fl!("cli-hosts-forget"))]
    Forget { slot: String },
    #[command(about = fl!("cli-hosts-name"))]
    Name { slot: u8, name: Option<String> },
    #[command(about = fl!("cli-hosts-policy"))]
    Policy { slot: u8, policy: String },
    #[command(about = fl!("cli-wireless-erase"))]
    Erase,
    #[command(about = fl!("cli-wireless-adopt"))]
    Adopt {
        #[arg(help = fl!("cli-wireless-adopt-port"))]
        port: PortArg,
        #[arg(long, help = fl!("cli-wireless-adopt-firmware"))]
        firmware: Option<PathBuf>,
    },
    #[command(about = fl!("cli-hosts-allow"))]
    Allow {
        #[arg(help = fl!("cli-hosts-allow-slot"))]
        slot: Option<u8>,
    },
    #[command(about = fl!("cli-hosts-disallow"))]
    Disallow { slot: u8 },
}

#[derive(Subcommand)]
enum PortAction {
    #[command(about = "auto, normalized, mirrored, identity, passthrough, off")]
    Mode { mode: String },
    #[command(about = fl!("cli-route"))]
    Route { route: String },
    #[command(about = fl!("cli-port-redetect"))]
    Redetect,
}

#[derive(Subcommand)]
enum DeviceAction {
    #[command(about = fl!("cli-device-list"))]
    List,
    #[command(about = fl!("cli-device-scan"))]
    Scan {
        #[arg(help = fl!("cli-device-scan-stop"))]
        stop: Option<String>,
    },
    #[command(about = fl!("cli-device-pair"))]
    Pair { device: String },
    #[command(about = fl!("cli-device-forget"))]
    Forget { slot: String },
    #[command(about = "auto, normalized, off")]
    Mode { slot: u8, mode: String },
    #[command(about = fl!("cli-route"))]
    Route { slot: u8, route: String },
    #[command(about = fl!("cli-device-map"))]
    Map {
        #[arg(help = fl!("cli-device-map-which"))]
        device: String,
    },
    #[command(about = fl!("cli-device-log"))]
    Log,
}

#[derive(Subcommand)]
enum KeymapAction {
    #[command(about = fl!("cli-keymap-show"))]
    Show { keymap: Option<u8> },
    #[command(about = fl!("cli-keymap-set"))]
    Set { keymap: u8, layer: u8, control: String, keycode: String },
    #[command(about = fl!("cli-keymap-axis"))]
    Axis {
        keymap: u8,
        layer: u8,
        #[arg(help = fl!("cli-keymap-axis-input"))]
        input: String,
        #[arg(help = fl!("cli-keymap-axis-target"))]
        target: String,
        #[arg(long, default_value_t = 1.0, allow_negative_numbers = true)]
        scale: f32,
        #[arg(long)]
        invert: bool,
        #[arg(long, default_value_t = 0, help = fl!("cli-keymap-axis-deadzone"))]
        deadzone: u8,
        #[arg(long, default_value = "linear", help = "linear, quadratic, cubic")]
        curve: String,
        #[arg(long, help = fl!("cli-keymap-axis-param"))]
        param: Option<u8>,
    },
    #[command(about = fl!("cli-keymap-learn"))]
    Learn {
        #[arg(default_value_t = 20)]
        seconds: u32,
    },
    #[command(about = fl!("cli-keymap-use"))]
    Use { profile: u8, keymap: String },
    #[command(about = fl!("cli-keymap-fallback"))]
    Fallback { profile: u8, keymap: String },
    #[command(about = fl!("cli-keymap-macro"))]
    Macro {
        n: Option<u8>,
        #[arg(help = fl!("cli-keymap-macro-steps"), allow_hyphen_values = true)]
        steps: Vec<String>,
    },
    #[command(about = fl!("cli-keymap-dance"))]
    Dance {
        n: Option<u8>,
        tap: Option<String>,
        hold: Option<String>,
        double: Option<String>,
        tap_hold: Option<String>,
        #[arg(long, help = fl!("cli-keymap-dance-term"))]
        term: Option<u16>,
    },
    #[command(about = fl!("cli-keymap-combo"))]
    Combo {
        n: Option<u8>,
        #[arg(help = fl!("cli-keymap-combo-keys"))]
        keys: Option<String>,
        output: Option<String>,
    },
    #[command(about = fl!("cli-keymap-override"))]
    Override {
        n: Option<u8>,
        trigger: Option<String>,
        replacement: Option<String>,
        #[arg(long, help = fl!("cli-keymap-override-mods"))]
        mods: Option<String>,
        #[arg(long, help = fl!("cli-keymap-override-negative"))]
        negative: Option<String>,
        #[arg(long, help = fl!("cli-keymap-override-suppress"))]
        suppress: Option<String>,
        #[arg(long, help = fl!("cli-keymap-override-layers"))]
        layers: Option<String>,
        #[arg(long, help = fl!("cli-keymap-off"))]
        off: bool,
    },
    #[command(about = fl!("cli-keymap-repeat"))]
    Repeat {
        n: Option<u8>,
        keycode: Option<String>,
        alt: Option<String>,
        #[arg(long, help = fl!("cli-keymap-repeat-mods"))]
        mods: Option<String>,
        #[arg(long, help = fl!("cli-keymap-off"))]
        off: bool,
    },
    #[command(about = fl!("cli-keymap-setting"))]
    Setting { name: Option<String>, value: Option<String> },
}

#[derive(Subcommand)]
enum FieldAction {
    #[command(about = fl!("cli-field-list"))]
    List,
    #[command(about = fl!("cli-field-set"))]
    Set {
        n: u8,
        iface: u8,
        report_id: u8,
        bit: u16,
        bits: u8,
        #[arg(help = fl!("cli-field-target"))]
        target: String,
        #[arg(long)]
        signed: bool,
        #[arg(long)]
        invert: bool,
    },
    #[command(about = fl!("cli-field-remove"))]
    Remove { n: u8 },
}

#[derive(Subcommand)]
enum CalAction {
    #[command(about = fl!("cli-calibrate-show"))]
    Show,
    #[command(about = fl!("cli-calibrate-set"))]
    Set { axis: String, min: i16, centre: i16, max: i16, deadzone: Option<u8> },
    #[command(about = fl!("cli-calibrate-measure"))]
    Measure {
        #[arg(default_value_t = 10)]
        seconds: u32,
    },
    #[command(about = fl!("cli-calibrate-reset"))]
    Reset,
}

#[derive(Subcommand)]
enum ConfigAction {
    #[command(about = fl!("cli-config-get"))]
    Get { name: Option<String> },
    #[command(about = fl!("cli-config-set"))]
    Set { name: String, value: String },
    #[command(about = fl!("cli-config-save"))]
    Save,
    #[command(about = fl!("cli-config-reset"))]
    Reset,
}

#[derive(Clone, Copy, ValueEnum)]
enum TargetArg {
    Pc1,
    Pc2,
    Toggle,
    #[value(help = fl!("cli-target-next"))]
    Next,
    // The wireless hosts' slots.
    Host1,
    Host2,
    Host3,
    Host4,
}

#[derive(Clone, Copy, ValueEnum)]
enum PortArg {
    #[value(alias = "kb")]
    Keyboard,
    #[value(alias = "ms")]
    Mouse,
    #[value(alias = "a1")]
    Aux1,
    #[value(alias = "a2")]
    Aux2,
}

#[derive(Subcommand)]
enum HotkeyCmd {
    #[command(about = fl!("cli-hotkey-list"))]
    List,
    #[command(about = fl!("cli-hotkey-add"))]
    Add {
        #[arg(help = fl!("cli-hotkey-action"))]
        action: String,
        #[arg(help = fl!("cli-hotkey-spec"))]
        hotkey: String,
    },
    #[command(about = fl!("cli-hotkey-set"))]
    Set {
        #[arg(help = fl!("cli-hotkey-entry"))]
        entry: u8,
        #[arg(help = fl!("cli-hotkey-spec"))]
        hotkey: String,
        #[arg(long, help = fl!("cli-hotkey-new-action"))]
        action: Option<String>,
    },
    #[command(about = fl!("cli-hotkey-remove"))]
    Remove {
        #[arg(help = fl!("cli-hotkey-entry"))]
        entry: u8,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum RebootArg {
    Normal,
    Bootloader,
    At32Dfu1,
    At32Dfu2,
    #[value(help = fl!("cli-reboot-at32-retry"))]
    At32Retry,
}

impl From<PortArg> for Port {
    fn from(p: PortArg) -> Self {
        match p {
            PortArg::Keyboard => Port::Keyboard,
            PortArg::Mouse => Port::Mouse,
            PortArg::Aux1 => Port::Aux1,
            PortArg::Aux2 => Port::Aux2,
        }
    }
}

/// Where a device's input goes now, and its routing when it is pinned (" → PC2, pinned").
fn target_line(s: &Snapshot, d: Device, target: Option<Host>) -> String {
    let Some(h) = target else { return String::new() };
    let pinned = s.profiles().and_then(|(a, _, list)| list.get(a as usize).map(|p| p.routes[d.index()]));
    match pinned {
        Some(Routing::Pin(_)) => format!(" → {} ({})", host_label(s, h), fl!("cli-pinned")),
        _ => format!(" → {}", host_label(s, h)),
    }
}

fn print_port(s: &Snapshot, p: Port) {
    let i = &s.ports[p.index()];
    let product = &s.products[p.index()];
    print!("  {:<9} {:<12}", Device::of_port(p).name(), names::port_state(i.state));
    if i.vid != 0 || i.pid != 0 {
        print!(" {:04x}:{:04x} {:<11}", i.vid, i.pid, names::speed(i.speed));
    }
    if !product.is_empty() {
        print!(" \"{product}\"");
    }
    println!();
    if i.state == kvm_proto::ctrl::PortState::Module {
        // The wireless module: not a device for the PCs (software.md §4.13).
        println!("            {}", fl!("port-module-summary"));
        return;
    }
    print!("            {}", names::mode(i.effective));
    if i.mode == PortMode::Auto {
        print!(" ({})", fl!("cli-port-auto", why = names::reason(i.reason)));
    } else if i.mode != i.effective {
        print!(" ({})", fl!("cli-port-set-to", mode = names::mode(i.mode), why = names::reason(i.reason)));
    }
    println!("{}", target_line(s, Device::of_port(p), i.target));
    if i.state != kvm_proto::ctrl::PortState::Empty {
        print!("            {}", names::classes(i.classes));
        if i.report_rate_hz > 0 {
            print!(", {} Hz", i.report_rate_hz);
        }
        if i.max_power_ma > 0 {
            print!(", {}", fl!("up-to-ma", milliamps = i.max_power_ma));
        }
        if i.faults > 0 {
            print!(", {}", fl!("over-current-faults", count = i.faults));
        }
        println!();
    }
}

fn print_status(c: &Client, s: &Snapshot) {
    let st = &s.status;
    println!(
        "{}",
        fl!(
            "cli-status-head",
            device = c.describe(),
            firmware = names::version(s.info.rp_fw),
            at1 = names::version(s.info.at_fw[0]),
            at2 = names::version(s.info.at_fw[1]),
            hardware = names::version(s.info.hw),
            serial = String::from_utf8_lossy(&s.info.serial).trim_end_matches('\0').to_string()
        )
    );
    // The wireless module only where there is one: on the board (0.16.0 on), or an ESP32-C3 board on a front port.
    if let Some(p) = s.module_port() {
        println!("{}", fl!("cli-module-port", port = names::port_title(p)));
    }
    match st.bt_module {
        diag::BtModule::Absent => {}
        diag::BtModule::Up => println!("{}", fl!("cli-module-firmware", version = names::version(s.info.bt_fw))),
        m => println!("{}", fl!("cli-module-state", state = names::bt_module(m))),
    }
    let this =
        st.asking_pc().map_or(String::new(), |p| format!("   ({})", fl!("cli-this-computer", pc = p.name().to_uppercase())));
    match s.bt.iter().find(|h| h.slot == st.bt_active).filter(|_| st.bt_active != 0) {
        Some(h) => println!(
            "{}{this}",
            fl!("cli-active-host", slot = h.slot, host = h.name.as_str(), pc = st.active.name().to_uppercase())
        ),
        None => println!("{}{this}", fl!("cli-active", target = st.active.name().to_uppercase())),
    }
    if let Some((active, _, list)) = s.profiles() {
        println!("{}", fl!("cli-profile-line", n = active, name = list[active as usize].name.as_str()));
    }
    for pc in [Pc::Pc1, Pc::Pc2] {
        let k = pc.index();
        println!(
            "  {}  {:<14} {}",
            pc.name().to_uppercase(),
            names::link(st.links[k]),
            fl!(
                "pc-vbus",
                volts = odkvmctl::i18n::decimal(st.vbus_mv[k + 1] as f64 / 1000.0, 2),
                offers = names::advertised(st.adv_pc[k])
            ),
        );
    }
    // Wireless hosts only where there are any: a KVM without the module shows the two PCs alone.
    for h in s.bt_bonded() {
        println!("  {}  {:<14} {}", fl!("cli-host-slot", slot = h.slot), host_state(h), h.name);
    }
    if st.bt_pairing != 0 {
        println!("  {}", fl!("cli-pairing-open", seconds = st.bt_pairing));
    }
    if st.bt_passkey != 0 {
        println!("  {}", fl!("cli-passkey-waits"));
    }
    for d in s.devices.iter().filter(|d| d.bonded()) {
        println!("  {}  {:<14} {}", fl!("cli-device-slot", slot = d.slot), device_state(d), device_label(d));
    }
    if st.bt_scan != 0 {
        println!("  {}", fl!("cli-scan-open", seconds = st.bt_scan, found = st.bt_found));
    }
    let mut power = names::power_line(st);
    if let Some((short, _)) = names::power_warning(st, true) {
        power += &format!("  ({short})");
    }
    println!("{}", fl!("cli-power", power = power));
    println!("{}", fl!("hop-line", state = names::hop_state(st.hop)));
    println!("{}", fl!("cli-selftest", result = names::selftest(st.selftest)));
    if st.selftest & (selftest::AT32_GAVE_UP1 | selftest::AT32_GAVE_UP2) != 0 {
        println!("  {}", fl!("cli-try-again", command = "odkvmctl reboot at32-retry"));
    }
    if st.selftest & selftest::BT_FW != 0 {
        println!("  {}", fl!("cli-module-update-hint", command = "odkvmctl update --module"));
    }
    println!("{}", fl!("cli-ports"));
    for p in s.front_ports() {
        print_port(s, p);
    }
    for d in s.devices.iter().filter(|d| d.bonded()) {
        print_device(s, d);
    }
    let mut locks = vec![];
    if st.flags & status_flags::SW_SWITCH_ALLOWED == 0 {
        locks.push(fl!("cli-switching-disabled"));
    }
    if st.flags & status_flags::SW_CONFIG_ALLOWED == 0 {
        locks.push(fl!("cli-config-disabled"));
    }
    if !locks.is_empty() {
        println!("{}", fl!("cli-locked", what = locks.join(", ")));
    }
}

/// A host as the CLI shows it: PC1, PC2, or a wireless host's name.
fn host_label(s: &Snapshot, h: Host) -> String {
    match h.bt_slot() {
        None => h.name().to_uppercase(),
        Some(n) => s.bt.iter().find(|b| b.slot == n).map_or_else(|| fl!("host-n", n = n), |b| b.name.clone()),
    }
}

/// An arrangement by number (0–3) or (case-insensitive) name.
fn find_arrangement(which: &str, hop: &HopView) -> Result<u8> {
    if let Ok(a) = which.parse::<u8>()
        && (a as usize) < MAX_ARRANGEMENTS
    {
        return Ok(a);
    }
    match hop.arrangements.iter().enumerate().position(|(a, v)| v.title(a as u8).eq_ignore_ascii_case(which)) {
        Some(a) => Ok(a as u8),
        None => bail!(fl!("cli-no-arrangement", which = which)),
    }
}

/// Screen hop: what it does now, the arrangements (those with screens or a profile using them, or `only`), slot by slot,
/// each host's mapping and the pointer settings.
fn print_screens(s: &Snapshot, only: Option<u8>) {
    let hop = s.hop();
    println!("{}", fl!("hop-line", state = names::hop_state(s.status.hop)));
    let profiles: Vec<ProfileView> =
        s.profiles().map(|(_, count, list)| list.into_iter().take(count as usize).collect()).unwrap_or_default();
    for (a, arr) in hop.arrangements.iter().enumerate() {
        let a = a as u8;
        let users: Vec<&str> = profiles.iter().filter(|p| p.arrangement == Some(a)).map(|p| p.name.as_str()).collect();
        if only.is_some_and(|o| o != a) || (only.is_none() && arr.is_empty() && users.is_empty()) {
            continue;
        }
        let used = if users.is_empty() {
            fl!("cli-arrangement-unused")
        } else {
            fl!("cli-arrangement-used", profiles = users.join(", "))
        };
        let mark = if hop.in_use == Some(a) { '*' } else { ' ' };
        println!("{mark} {a}  {}  ({used})", arr.title(a));
        // The main screen matters (and is marked) only on a host that maps the pointer onto it alone.
        for (i, sc) in arr.screens.iter().enumerate().filter(|(_, sc)| sc.host.is_some()) {
            let main = sc
                .host
                .is_some_and(|h| hop.mapping(h) == Mapping::MainScreen && main_screen(&arr.screens, h) == Some(i));
            let mark = if main { format!("  ({})", fl!("hop-main-screen")) } else { String::new() };
            println!("     screen{i}  {}{mark}", names::format_screen(sc));
        }
        if arr.is_empty() {
            println!("     {}", fl!("hop-no-screens"));
        }
    }
    // PC1 and PC2, and the wireless hosts that have a screen somewhere.
    let in_any = |h: Host| hop.arrangements.iter().any(|a| a.screens.iter().any(|sc| sc.host == Some(h)));
    for h in Host::ALL.iter().copied().filter(|h| h.as_pc().is_some() || in_any(*h)) {
        let label = host_label(s, h);
        println!("{}", fl!("cli-hop-mapping", pc = label.as_str(), mapping = names::mapping(hop.mapping(h))));
        println!("  {}", fl!("cli-hop-units", pc = label.as_str(), units = names::units(hop.units(h))));
        if let Some(n) = h.bt_slot() {
            let pointer = names::bt_pointer(hop.bt_pointer[n as usize - 1]);
            println!("  {}", fl!("cli-hop-bt-pointer", host = label.as_str(), pointer = pointer));
        }
    }
    println!(
        "{}",
        fl!(
            "cli-hop-pointer",
            accel = names::accel(hop.accel),
            speed = i32::from(hop.speed),
            resistance = hop.resistance,
            corner = hop.corner
        )
    );
}

/// Whether the KVM's copies of this computer's screens and units are what its display settings say (only when the KVM
/// says which PC this is): a warning naming the arrangements that are not.
fn check_screens(s: &Snapshot) {
    let Some(pc) = s.this_pc() else { return };
    let hop = s.hop();
    let host = Host::pc(pc);
    // The simulator's screens aren't this computer's.
    if hop.simulated {
        return;
    }
    let Ok(now) = odkvmctl::screens::detect() else { return };
    let stale: Vec<String> = hop
        .arrangements
        .iter()
        .enumerate()
        .filter(|(_, a)| a.screens.iter().any(|x| x.host == Some(host)))
        .filter(|(_, a)| odkvmctl::screens::differs(&a.screens, host, hop.units(host), &now))
        .map(|(i, a)| a.title(i as u8))
        .collect();
    if !stale.is_empty() {
        println!("{} ({})", fl!("cli-hop-out-of-date"), stale.join(", "));
    }
}

fn host_state(h: &BtHostView) -> String {
    if h.ready() {
        fl!("cli-host-ready", ms = odkvmctl::i18n::decimal(h.info.interval as f64 * 1.25, 2))
    } else if h.connected() {
        fl!("link-enumerating")
    } else {
        fl!("link-detached")
    }
}

fn print_hosts(s: &Snapshot) {
    for h in &s.bt {
        let mark = if s.status.bt_active == h.slot { '*' } else { ' ' };
        if !h.bonded() {
            println!("{mark} {}  ({})", h.slot, fl!("slot-free"));
            continue;
        }
        let control = match (h.may_control, h.in_session()) {
            (true, true) => format!(", {}", fl!("host-controls-now")),
            (true, false) => format!(", {}", fl!("host-may-control")),
            (false, _) => String::new(),
        };
        let this = if s.status.asking_bt() == Some(h.slot) { format!("   ({})", fl!("cli-this-computer-host")) } else { String::new() };
        println!(
            "{mark} {}  {:<12}  {:<22} {}  {}{}{control}{this}",
            h.slot,
            h.name,
            host_state(h),
            h.policy.name(),
            h.address(),
            if h.info.rssi != 0 { format!(", {} dBm", h.info.rssi) } else { String::new() }
        );
    }
}

/// A wireless device by its own name, and what it says it is.
fn device_label(d: &BtDeviceInfo) -> String {
    let kind = names::device_kind(d.appearance);
    if d.name.is_empty() { kind } else { format!("{} ({kind})", d.name) }
}

/// A device's report map as hex, 16 bytes a line, and what the KVM's parser makes of it (the same parser as the
/// KVM's, run here): to send along when a device shows as not supported.
fn print_device_map(map: &[u8]) {
    if map.is_empty() {
        println!("{}", fl!("cli-device-map-none"));
        return;
    }
    let bytes = map.len();
    println!("{}", fl!("cli-device-map-bytes", bytes = bytes));
    for line in map.chunks(16) {
        let hex: Vec<String> = line.iter().map(|b| format!("{b:02x}")).collect();
        println!("{}", hex.join(" "));
    }
    match kvm_hid::Layout::parse(map) {
        Ok(l) => {
            let fields = l.fields.len();
            let fits = kvm_hid::Translator::new(l.clone()).verdict();
            if l.incomplete {
                println!("{}", fl!("cli-device-map-incomplete", fields = fields));
            } else {
                println!("{}", fl!("cli-device-map-ok", fields = fields));
            }
            println!("caps {:#06x}, misfit {:#06x}", fits.caps, fits.misfit);
        }
        Err(e) => println!("{}", fl!("cli-device-map-error", error = format!("{e:?}"))),
    }
}

fn device_state(d: &BtDeviceInfo) -> String {
    if d.unreadable() {
        fl!("device-unsupported")
    } else if d.ready() {
        fl!("cli-device-ready")
    } else if d.connected() {
        fl!("link-enumerating")
    } else {
        fl!("link-detached")
    }
}

/// A Bluetooth address as devices show it ("C4:21:3B:6E:5A:01").
fn bt_address(a: &[u8; 7]) -> String {
    a[1..].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// A wireless device as `status` and `device list` show it, as a front port is.
fn print_device(s: &Snapshot, d: &BtDeviceInfo) {
    let dev = Device::wireless_slot(d.slot).unwrap_or(Device::Wireless1);
    print!("  {:<9} {:<12} {}", dev.name(), device_state(d), device_label(d));
    if d.battery <= 100 {
        print!(", {}", fl!("cli-battery", percent = d.battery));
    }
    if d.rssi != 0 {
        print!(", {} dBm", d.rssi);
    }
    println!();
    let mode = s.profiles().map_or(PortMode::Auto, |(a, _, list)| list[a as usize].modes[dev.index()]);
    let target = (d.ready() && mode != PortMode::Off).then(|| route_target(s, dev)).flatten();
    // Over Bluetooth, Auto is Normalized.
    let shown = match mode {
        PortMode::Auto => format!("{} ({})", names::mode(PortMode::Normalized), names::mode(PortMode::Auto)),
        m => names::mode(m),
    };
    println!("            {shown}{}", target_line(s, dev, target));
}

/// Where `d`'s input goes now: its pin, or the active host.
fn route_target(s: &Snapshot, d: Device) -> Option<Host> {
    let (a, _, list) = s.profiles()?;
    Some(match list[a as usize].routes[d.index()] {
        Routing::Pin(h) => h,
        Routing::Follow => match s.status.bt_active {
            0 => Host::pc(s.status.active),
            n => Host::bt(n)?,
        },
    })
}

fn print_devices(s: &Snapshot) {
    for d in &s.devices {
        if !d.bonded() {
            println!("  {}  ({})", d.slot, fl!("slot-free"));
            continue;
        }
        println!("  {}  {:<14} {}  {}", d.slot, device_state(d), device_label(d), bt_address(&d.addr));
    }
    let st = &s.status;
    if st.bt_scan != 0 {
        println!("{}", fl!("cli-scan-open", seconds = st.bt_scan, found = st.bt_found));
    }
    print_found(&s.found);
    print_device_pairing(st.bt_device_pairing, st.bt_device_passkey);
}

fn print_found(found: &[BtFound]) {
    for (i, f) in found.iter().enumerate() {
        let name = if f.name.is_empty() { names::device_kind(f.appearance) } else { f.name.to_string() };
        println!(
            "  [{}] {:<24} {:<14} {} dBm  {}",
            i + 1,
            name,
            names::device_kind(f.appearance),
            f.rssi,
            bt_address(&f.addr)
        );
    }
}

fn print_device_pairing(p: DevicePairing, passkey: Option<u32>) {
    if p != DevicePairing::Idle {
        println!("{}", names::device_pairing(p));
    }
    if let Some(v) = passkey.filter(|_| p == DevicePairing::Pairing) {
        println!("{}", fl!("device-passkey", passkey = format!("{v:06}")));
    }
}

fn print_profiles(s: &Snapshot, active: u8, count: u8, list: &[ProfileView], hop: &HopView) {
    let devices = s.routed_devices();
    for (i, p) in list.iter().enumerate().take(count as usize) {
        let mark = if i == active as usize { '*' } else { ' ' };
        let modes: Vec<String> = devices
            .iter()
            .map(|d| {
                let route = match p.routes[d.index()] {
                    Routing::Follow => String::new(),
                    r => format!(" → {}", names::route_name(r)),
                };
                format!("{} {}{route}", d.name(), p.modes[d.index()].name())
            })
            .collect();
        // Screen hop: the arrangement the profile uses.
        let hop = match p.arrangement {
            None => format!("; {}", fl!("cli-profile-no-hop")),
            Some(a) => format!("; {}", fl!("cli-profile-hop", name = hop.arrangements[a as usize].title(a))),
        };
        println!("{mark} {i}  {:<12}  {}{hop}", p.name, modes.join(", "));
    }
    if (count as usize) < MAX_PROFILES {
        println!("  ({})", fl!("cli-profiles-in-use", count = count, max = MAX_PROFILES));
    }
}

/// Looks for wireless devices to pair, and lists them as they show up, until the KVM stops looking (Ctrl-C leaves it
/// looking: `device pair` picks from the list meanwhile).
fn scan(c: &mut Client) -> Result<()> {
    c.bt_scan(true)?;
    println!("{}", fl!("cli-scan-started", seconds = kvm_proto::ctrl::BT_PAIR_SECONDS));
    let mut shown = 0;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let st = c.status()?;
        let found = c.bt_found(st.bt_found)?;
        if found.len() > shown {
            print_found(&found[shown..]);
            shown = found.len();
        }
        if st.bt_scan == 0 {
            break;
        }
    }
    println!("{}", fl!("cli-scan-ended"));
    Ok(())
}

/// Pairs the device `which` names (its number in the list `device scan` prints, or its address), and follows the
/// pairing to its end, with the passkey to type on a keyboard that asks for one.
fn pair_device(c: &mut Client, s: &Snapshot, which: &str) -> Result<()> {
    let found = &s.found;
    let pick = match which.parse::<usize>() {
        Ok(n) if (1..=found.len()).contains(&n) => found.get(n - 1),
        _ => found.iter().find(|f| bt_address(&f.addr).eq_ignore_ascii_case(which)),
    };
    let Some(f) = pick else { bail!(fl!("cli-device-not-found", which = which)) };
    c.bt_pair_device(f.addr)?;
    let mut last = (DevicePairing::Idle, None);
    let until = std::time::Instant::now() + Duration::from_secs(90);
    while std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(300));
        let st = c.status()?;
        let now = (st.bt_device_pairing, st.bt_device_passkey);
        if now != last {
            print_device_pairing(now.0, now.1);
            last = now;
        }
        match now.0 {
            DevicePairing::Paired => return Ok(()),
            DevicePairing::Failed => std::process::exit(1),
            _ => {}
        }
    }
    bail!(fl!("error-timeout"))
}

/// A profile by number or (case-insensitive) name.
fn find_profile(which: &str, list: &[ProfileView]) -> Result<u8> {
    if let Ok(i) = which.parse::<u8>()
        && (i as usize) < list.len()
    {
        return Ok(i);
    }
    match list.iter().position(|p| p.name.eq_ignore_ascii_case(which)) {
        Some(i) => Ok(i as u8),
        None => {
            let last: usize = list.len() - 1;
            bail!(fl!("cli-no-profile", which = which, last = last))
        }
    }
}

/// The hotkey list as the KVM has it, entry by entry.
fn hotkey_list(c: &mut Client) -> Result<Vec<HotkeyBinding>> {
    (0..MAX_HOTKEYS as u8)
        .map(|i| match c.get_config(key::hotkey(i))? {
            ConfigValue::Hotkey(_, b) => Ok(b),
            v => bail!("{v:?}"),
        })
        .collect()
}

/// The entry `n` (1–16) names, as an index.
fn hotkey_entry(n: u8) -> Result<u8> {
    match n {
        1..=16 => Ok(n - 1),
        _ => bail!(fl!("cli-hotkey-no-entry", n = n, max = MAX_HOTKEYS)),
    }
}

/// Prints entry `i` of the list as `hotkey list` does.
fn print_hotkey(i: u8, b: &HotkeyBinding) {
    println!("{:>2}  {:<16} {}", i + 1, b.action.name(), format_spec(&b.hotkey));
}

/// The hotkey for entry `i` (doing `action`): learned from the KVM's keyboard when `spec` is `learn`, else read from
/// it. Learning warns of a plain key or one another entry has, and on a terminal asks before it is used (None: not
/// used).
fn hotkey_for(
    c: &mut Client,
    list: &[HotkeyBinding],
    i: u8,
    action: HotkeyAction,
    spec: &str,
) -> Result<Option<Hotkey>> {
    use std::io::IsTerminal as _;
    if spec != "learn" {
        return Ok(Some(
            kvm_core::hotkey::parse_spec(spec)
                .map_err(|e| anyhow::anyhow!(fl!("parse-hotkey", value = spec, error = format!("{e:?}"))))?,
        ));
    }
    if c.simulated_keyboard().is_some() {
        bail!(fl!("cli-learn-sim"));
    }
    let learned = c.learn_hotkey(|| {
        let seconds: u32 = kvm_core::engine::LEARN_MS / 1000;
        eprintln!("{}", fl!("cli-learn-press", seconds = seconds))
    });
    let (end, h) = learned?;
    if end != LearnEnd::Learned {
        bail!(fl!("cli-nothing-changed-why", why = names::learn_end(end)));
    }
    eprintln!("{}", fl!("ev-hotkey-learned", hotkey = format_spec(&h).to_string()));
    let others: Vec<(&str, Hotkey)> = list
        .iter()
        .enumerate()
        .filter(|(j, b)| *j != i as usize && b.is_used())
        .map(|(_, b)| (b.action.name(), b.hotkey))
        .collect();
    if let Some(w) = names::hotkey_warning(&h, &others) {
        eprintln!("{w}");
    }
    if !std::io::stdin().is_terminal() {
        return Ok(Some(h));
    }
    eprint!("{} ", fl!("cli-use-it-for", name = action.name()));
    Ok(yes()?.then_some(h))
}

/// Asks before a software lock goes on; without a terminal to ask on (a script), the warning alone.
fn confirm_lock(switching: bool) -> Result<bool> {
    use std::io::IsTerminal as _;
    eprintln!("{}\n", names::lock_warning(switching));
    if !std::io::stdin().is_terminal() {
        return Ok(true);
    }
    eprint!("{} ", fl!("cli-turn-it-off"));
    yes()
}

/// A y/N answer from the terminal: y, yes, or the translation's own word for yes (`cli-yes-words`).
fn yes() -> Result<bool> {
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let a = answer.trim().to_lowercase();
    Ok(!a.is_empty() && (a == "y" || a == "yes" || fl!("cli-yes-words").split(',').any(|w| w.trim() == a)))
}

/// The diagnostics, grouped, with a mark per grade (coloured on a terminal). Returns whether anything is bad.
fn print_diag(items: &[diag::Item]) -> bool {
    use std::io::IsTerminal as _;
    let colour = std::io::stdout().is_terminal();
    let mark = |g: diag::Grade| {
        let (m, c) = match g {
            diag::Grade::Info => (" ", ""),
            diag::Grade::Ok => ("✓", "\x1b[32m"),
            diag::Grade::Warn => ("!", "\x1b[33m"),
            diag::Grade::Bad => ("✗", "\x1b[31m"),
        };
        if colour && !c.is_empty() { format!("{c}{m}\x1b[0m") } else { m.to_string() }
    };
    let width = items.iter().map(|i| names::diag_name(i.id).chars().count()).max().unwrap_or(0);
    for (group, list) in names::diag_groups(items) {
        println!("{}", names::diag_group(group));
        for i in list {
            println!("  {} {:<width$}  {}", mark(i.grade), names::diag_name(i.id), names::diag_value(i));
        }
    }
    let count = |g| items.iter().filter(|i| i.grade == g).count();
    let (warn, bad) = (count(diag::Grade::Warn), count(diag::Grade::Bad));
    let verdict = match (warn, bad) {
        (0, 0) => fl!("diag-all-good"),
        (w, 0) => fl!("diag-warnings", count = w),
        (0, b) => fl!("diag-bad", count = b),
        (w, b) => format!("{}, {}", fl!("diag-bad", count = b), fl!("diag-warnings", count = w)),
    };
    println!("{}", fl!("cli-readings", count = items.len(), verdict = verdict));
    bad != 0
}

fn print_event(e: &Event) {
    match e {
        Event::Switched(pc, src) => {
            println!("{}", fl!("ev-switched", target = pc.name().to_uppercase(), source = names::switch_source(*src)))
        }
        Event::PortChanged(p) => println!("{}", fl!("ev-port-changed", port = names::port_title(*p))),
        Event::Fault(kind, Some(p)) => {
            println!("{}", fl!("ev-fault-port", port = names::port_title(*p), fault = names::fault_kind(*kind)))
        }
        Event::Fault(kind, None) => println!("{}", fl!("ev-fault", fault = names::fault_kind(*kind))),
        Event::ConfigChanged(k) => println!("{}", fl!("ev-setting-changed", name = names::config_name(*k))),
        Event::HotkeyLearned(LearnEnd::Learned, h) => {
            println!("{}", fl!("ev-hotkey-learned", hotkey = format_spec(h).to_string()))
        }
        Event::HotkeyLearned(end, _) => println!("{}", fl!("ev-hotkey-not-learned", why = names::learn_end(*end))),
        Event::BtActive(0) => println!("{}", fl!("cli-ev-back-on-pc")),
        Event::BtActive(n) => println!("{}", fl!("cli-ev-host-active", slot = u32::from(*n))),
        Event::BtChanged(0) => println!("{}", fl!("cli-ev-pairing-changed")),
        Event::BtChanged(n) => println!("{}", fl!("cli-ev-host-changed", slot = u32::from(*n))),
        Event::BtDeviceChanged(0) => println!("{}", fl!("cli-ev-devices-changed")),
        Event::BtDeviceChanged(n) => println!("{}", fl!("cli-ev-device-changed", slot = u32::from(*n))),
        // Screen hop's placements, for the window's placer: `Client` keeps none.
        Event::PlaceCursor(_) => {}
        Event::Control(p, down) => {
            let state = if *down { fl!("cli-control-down") } else { fl!("cli-control-up") };
            println!("{}", fl!("cli-ev-control", control = keycodes::control_name(*p), state = state))
        }
        Event::Axis(i, v) => {
            let name = keycodes::AXES.get(*i as usize).copied().unwrap_or("?");
            println!("{}", fl!("cli-ev-axis", axis = name, value = i32::from(*v)))
        }
        // The monitor's pieces, which `odkvmctl monitor` puts together.
        Event::Report { .. } => {}
    }
}

/// How often a watch of the controls is asked again: the KVM ends one after 10 s (`WATCH_MS`).
const WATCH_RENEW: Duration = Duration::from_secs(4);

/// `odkvmctl keymap …` (hid_mapping.md).
fn keymap(c: &mut Client, action: KeymapAction) -> Result<()> {
    use kvm_map::axes::{Rule, flags};
    let check = |k: u8, l: u8| {
        if (k as usize) < kvm_map::KEYMAPS && (l as usize) < kvm_map::LAYERS {
            Ok(())
        } else {
            Err(anyhow::anyhow!(fl!("cli-keymap-range")))
        }
    };
    match action {
        KeymapAction::Show { keymap } => {
            let profiles = c.snapshot()?.profiles().map(|(_, n, list)| (n, list));
            for k in keymap.map_or(0..kvm_map::KEYMAPS as u8, |k| k..k + 1) {
                check(k, 0)?;
                let used: Vec<String> = profiles
                    .iter()
                    .flat_map(|(n, list)| {
                        list.iter().take(*n as usize).filter(|p| p.keymap == Some(k)).map(|p| p.name.clone())
                    })
                    .collect();
                let used = if used.is_empty() { fl!("cli-keymap-unused") } else { used.join(", ") };
                println!("{}", fl!("cli-keymap-title", keymap = u32::from(k), profiles = used));
                let mut any = false;
                for l in 0..kvm_map::LAYERS as u8 {
                    let codes = c.keycodes(k, l)?;
                    for (p, kc) in codes.iter().enumerate() {
                        let fresh = if l == 0 { kvm_map::keymap::identity(p) } else { kvm_map::keycode::KC_TRNS };
                        if *kc != fresh {
                            any = true;
                            let (ctl, code) = (keycodes::control_name(p as u16), keycodes::keycode_name(*kc));
                            println!(
                                "  {}",
                                fl!("cli-keymap-entry", layer = u32::from(l), control = ctl, keycode = code)
                            );
                        }
                    }
                    for (i, r) in c.axis_rules(k, l)?.iter().enumerate() {
                        if r.target != kvm_map::axes::Target::Transparent {
                            any = true;
                            let target = keycodes::target_name(r.target);
                            let scale = format!("{:.2}", r.scale as f32 / 256.0);
                            println!(
                                "  {}",
                                fl!(
                                    "cli-keymap-axis-entry",
                                    layer = u32::from(l),
                                    axis = keycodes::AXES[i],
                                    target = target,
                                    scale = scale
                                )
                            );
                        }
                    }
                }
                if !any {
                    println!("  {}", fl!("cli-keymap-fresh"));
                }
            }
        }
        KeymapAction::Set { keymap, layer, control, keycode } => {
            check(keymap, layer)?;
            let p = keycodes::parse_control(&control).map_err(anyhow::Error::msg)?;
            let kc = keycodes::parse_keycode(&keycode).map_err(anyhow::Error::msg)?;
            c.set_keycodes(keymap, layer, p, &[kc])?;
            let (ctl, code) = (keycodes::control_name(p), keycodes::keycode_name(kc));
            println!("{}", fl!("cli-keymap-entry", layer = u32::from(layer), control = ctl, keycode = code));
        }
        KeymapAction::Axis { keymap, layer, input, target, scale, invert, deadzone, curve, param } => {
            check(keymap, layer)?;
            let i = keycodes::parse_axis(&input).map_err(anyhow::Error::msg)?;
            let target = keycodes::parse_target(&target).map_err(anyhow::Error::msg)?;
            let curve = match curve.as_str() {
                "linear" => 0,
                "quadratic" => 1,
                "cubic" => 2,
                other => bail!(fl!("parse-one-of", value = other, choices = "linear, quadratic, cubic")),
            };
            let rule = Rule {
                target,
                flags: (u8::from(invert) * flags::INVERT) | (curve << flags::CURVE_SHIFT),
                scale: (scale * 256.0).round().clamp(-32768.0, 32767.0) as i16,
                deadzone: keycodes::in_256ths(deadzone as u32),
                param: param.unwrap_or(kvm_map::axes::DEFAULT_PARAM),
            };
            c.set_axis_rule(keymap, layer, i as u8, rule)?;
            let scale = format!("{:.2}", rule.scale as f32 / 256.0);
            let target = keycodes::target_name(target);
            println!(
                "{}",
                fl!(
                    "cli-keymap-axis-entry",
                    layer = u32::from(layer),
                    axis = keycodes::AXES[i],
                    target = target,
                    scale = scale
                )
            );
        }
        KeymapAction::Learn { seconds } => {
            c.watch_controls(1)?;
            println!("{}", fl!("cli-keymap-learning", seconds = seconds));
            let until = Instant::now() + Duration::from_secs(seconds as u64);
            let mut renew = Instant::now() + WATCH_RENEW;
            let r = (|| -> Result<()> {
                while Instant::now() < until {
                    if Instant::now() >= renew {
                        c.watch_controls(1)?;
                        renew = Instant::now() + WATCH_RENEW;
                    }
                    for e in c.events(Duration::from_millis(200))? {
                        if matches!(e, Event::Control(..) | Event::Axis(..)) {
                            print_event(&e);
                        }
                    }
                }
                Ok(())
            })();
            c.watch_controls(0)?;
            r?;
        }
        KeymapAction::Fallback { profile, keymap } => {
            let k = names::parse_arrangement(&keymap).map_err(anyhow::Error::msg)?;
            c.set_config(ConfigValue::ProfileFallback(profile, k))?;
            let k = k.map_or_else(|| "none".to_string(), |k| k.to_string());
            println!("{}", fl!("cli-keymap-fallback-used", profile = u32::from(profile), keymap = k));
        }
        KeymapAction::Macro { n, steps } => macro_cmd(c, n, &steps)?,
        KeymapAction::Dance { n, tap, hold, double, tap_hold, term } => {
            use kvm_map::keymap::{Dynamic, TAP_DANCES};
            let show = |c: &mut Client, i: u8| -> Result<()> {
                let b = c.map_entry(Dynamic::TapDance, i)?;
                let kc = |at: usize| keycodes::keycode_name(u16::from_le_bytes([b[at], b[at + 1]]));
                let term = u16::from_le_bytes([b[8], b[9]]);
                let line = fl!(
                    "cli-dance-entry",
                    n = u32::from(i),
                    tap = kc(0),
                    hold = kc(2),
                    double = kc(4),
                    taphold = kc(6),
                    term = u32::from(term)
                );
                println!("{line}");
                Ok(())
            };
            match (n, tap) {
                (None, _) => {
                    for i in 0..TAP_DANCES as u8 {
                        if c.map_entry(Dynamic::TapDance, i)?.iter().any(|b| *b != 0) {
                            show(c, i)?;
                        }
                    }
                }
                (Some(i), None) => show(c, i)?,
                (Some(i), Some(tap)) => {
                    let mut b = [0u8; 10];
                    for (at, kc) in [Some(tap), hold, double, tap_hold].into_iter().enumerate() {
                        let k = kc.map(|k| keycodes::parse_keycode(&k)).transpose().map_err(anyhow::Error::msg)?;
                        b[2 * at..2 * at + 2].copy_from_slice(&k.unwrap_or(0).to_le_bytes());
                    }
                    b[8..10].copy_from_slice(&term.unwrap_or(0).to_le_bytes());
                    c.set_map_entry(Dynamic::TapDance, i, &b)?;
                    show(c, i)?;
                }
            }
        }
        KeymapAction::Combo { n, keys, output } => {
            use kvm_map::keymap::{COMBOS, Dynamic};
            let show = |c: &mut Client, i: u8| -> Result<()> {
                let b = c.map_entry(Dynamic::Combo, i)?;
                let kc = |at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
                let keys: Vec<String> =
                    (0..4).map(|k| kc(2 * k)).take_while(|k| *k != 0).map(keycodes::keycode_name).collect();
                let line = fl!(
                    "cli-combo-entry",
                    n = u32::from(i),
                    keys = keys.join(","),
                    output = keycodes::keycode_name(kc(8))
                );
                println!("{line}");
                Ok(())
            };
            match (n, keys) {
                (None, _) => {
                    for i in 0..COMBOS as u8 {
                        if c.map_entry(Dynamic::Combo, i)?[..2] != [0, 0] {
                            show(c, i)?;
                        }
                    }
                }
                (Some(i), None) => show(c, i)?,
                (Some(i), Some(keys)) => {
                    let mut b = [0u8; 10];
                    if keys != "none" {
                        let ks: Vec<u16> = keys
                            .split(',')
                            .map(keycodes::parse_keycode)
                            .collect::<Result<_, _>>()
                            .map_err(anyhow::Error::msg)?;
                        if ks.is_empty() || ks.len() > 4 {
                            bail!(fl!("cli-combo-keys"));
                        }
                        for (k, kc) in ks.iter().enumerate() {
                            b[2 * k..2 * k + 2].copy_from_slice(&kc.to_le_bytes());
                        }
                        let out =
                            output.as_deref().map(keycodes::parse_keycode).transpose().map_err(anyhow::Error::msg)?;
                        b[8..10].copy_from_slice(&out.unwrap_or(0).to_le_bytes());
                    }
                    c.set_map_entry(Dynamic::Combo, i, &b)?;
                    show(c, i)?;
                }
            }
        }
        KeymapAction::Override { n, trigger, replacement, mods, negative, suppress, layers, off } => {
            use kvm_map::keymap::{Dynamic, KEY_OVERRIDES, ko};
            let show = |c: &mut Client, i: u8| -> Result<()> {
                let b = c.map_entry(Dynamic::KeyOverride, i)?;
                let kc = |at: usize| keycodes::keycode_name(u16::from_le_bytes([b[at], b[at + 1]]));
                let state = if b[9] & ko::ENABLED != 0 { fl!("cli-entry-on") } else { fl!("cli-entry-off") };
                let line = fl!(
                    "cli-override-entry",
                    n = u32::from(i),
                    mods = keycodes::mods_name_or_none(b[6]),
                    trigger = kc(0),
                    replacement = kc(2),
                    state = state
                );
                println!("{line}");
                Ok(())
            };
            match (n, trigger) {
                (None, _) => {
                    for i in 0..KEY_OVERRIDES as u8 {
                        if c.map_entry(Dynamic::KeyOverride, i)?.iter().any(|b| *b != 0) {
                            show(c, i)?;
                        }
                    }
                }
                (Some(i), None) => show(c, i)?,
                (Some(i), Some(trigger)) => {
                    let kc = |s: &str| keycodes::parse_keycode(s).map_err(anyhow::Error::msg);
                    let m = |s: Option<String>| {
                        s.map_or(Ok(0), |s| keycodes::parse_mod_names(&s).map_err(anyhow::Error::msg))
                    };
                    let layers = match layers {
                        None => 0xFFFFu16,
                        Some(l) => l
                            .split(',')
                            .try_fold(0u16, |acc, n| {
                                n.trim()
                                    .parse::<u8>()
                                    .ok()
                                    .filter(|n| (*n as usize) < kvm_map::LAYERS)
                                    .map(|n| acc | 1 << n)
                            })
                            .ok_or_else(|| anyhow::anyhow!(fl!("cli-keymap-range")))?,
                    };
                    let t = kc(&trigger)?.to_le_bytes();
                    let r = kc(replacement.as_deref().unwrap_or("none"))?.to_le_bytes();
                    let options = ko::ACTIVATION_TRIGGER_DOWN
                        | ko::ACTIVATION_REQUIRED_MOD_DOWN
                        | if off { 0 } else { ko::ENABLED };
                    let l = layers.to_le_bytes();
                    let b = [t[0], t[1], r[0], r[1], l[0], l[1], m(mods)?, m(negative)?, m(suppress)?, options];
                    c.set_map_entry(Dynamic::KeyOverride, i, &b)?;
                    show(c, i)?;
                }
            }
        }
        KeymapAction::Repeat { n, keycode, alt, mods, off } => {
            use kvm_map::keymap::{ALT_REPEATS, Dynamic, arep};
            let show = |c: &mut Client, i: u8| -> Result<()> {
                let b = c.map_entry(Dynamic::AltRepeat, i)?;
                let kc = |at: usize| keycodes::keycode_name(u16::from_le_bytes([b[at], b[at + 1]]));
                let state = if b[5] & arep::ENABLED != 0 { fl!("cli-entry-on") } else { fl!("cli-entry-off") };
                println!("{}", fl!("cli-repeat-entry", n = u32::from(i), keycode = kc(0), alt = kc(2), state = state));
                Ok(())
            };
            match (n, keycode) {
                (None, _) => {
                    for i in 0..ALT_REPEATS as u8 {
                        if c.map_entry(Dynamic::AltRepeat, i)?.iter().any(|b| *b != 0) {
                            show(c, i)?;
                        }
                    }
                }
                (Some(i), None) => show(c, i)?,
                (Some(i), Some(k)) => {
                    let kc = |s: &str| keycodes::parse_keycode(s).map_err(anyhow::Error::msg);
                    let k = kc(&k)?.to_le_bytes();
                    let a = kc(alt.as_deref().unwrap_or("none"))?.to_le_bytes();
                    let m = mods.map_or(Ok(0), |s| keycodes::parse_mod_names(&s)).map_err(anyhow::Error::msg)?;
                    let options = if off { 0 } else { arep::ENABLED };
                    c.set_map_entry(Dynamic::AltRepeat, i, &[k[0], k[1], a[0], a[1], m, options])?;
                    show(c, i)?;
                }
            }
        }
        KeymapAction::Setting { name, value } => {
            let show = |c: &mut Client, (n, q, flag): (&str, u16, bool)| -> Result<()> {
                let v = c.map_setting(q)?;
                let v = if flag { if v != 0 { "on".into() } else { "off".into() } } else { v.to_string() };
                println!("{}", fl!("cli-setting-entry", name = n, value = v));
                Ok(())
            };
            match name {
                None => {
                    for s in keycodes::SETTINGS {
                        show(c, s)?;
                    }
                }
                Some(name) => {
                    let s = keycodes::SETTINGS.iter().copied().find(|(n, ..)| *n == name).ok_or_else(|| {
                        let names = keycodes::SETTINGS.iter().map(|(n, ..)| *n).collect::<Vec<_>>().join(", ");
                        anyhow::anyhow!(fl!("parse-one-of", value = name.clone(), choices = names))
                    })?;
                    if let Some(v) = value {
                        let v = if s.2 {
                            u32::from(names::parse_bool(&v).map_err(anyhow::Error::msg)?)
                        } else {
                            v.trim()
                                .parse::<u32>()
                                .map_err(|_| anyhow::anyhow!(fl!("parse-number", value = v.clone(), max = u32::MAX)))?
                        };
                        c.set_map_setting(s.1, v)?;
                    }
                    show(c, s)?;
                }
            }
        }
        KeymapAction::Use { profile, keymap } => {
            let k = names::parse_arrangement(&keymap).map_err(anyhow::Error::msg)?;
            c.set_config(ConfigValue::ProfileKeymap(profile, k))?;
            let k = k.map_or_else(|| "none".to_string(), |k| k.to_string());
            println!("{}", fl!("cli-keymap-used", profile = u32::from(profile), keymap = k));
        }
    }
    Ok(())
}

/// A device by name (`keyboard`, `mouse`, `other`, `wireless1`–`4`), or a Bluetooth device slot's number.
fn parse_device(s: &str) -> Result<Device> {
    Device::from_name(s).or_else(|| s.parse::<u8>().ok().and_then(Device::wireless_slot)).ok_or_else(|| {
        anyhow::anyhow!(fl!("parse-one-of", value = s, choices = "keyboard, mouse, other, wireless1…wireless4"))
    })
}

/// `odkvmctl keymap macro [<n> [<step>…]]`: the shared macros (hid_mapping.md §5.4).
fn macro_cmd(c: &mut Client, n: Option<u8>, steps: &[String]) -> Result<()> {
    use kvm_map::keymap::{MACRO_BUFFER, MACRO_COUNT};
    use odkvmctl::keycodes::MacroStep;
    let buf = c.macros()?;
    let count = MACRO_COUNT as usize;
    let show = |buf: &[u8], i: usize| {
        let m = kvm_map::macros::get(buf, i);
        if m.is_empty() {
            println!("{}", fl!("cli-macro-empty", n = (i as u32)));
        } else {
            let words: Vec<String> = MacroStep::read(m).iter().map(MacroStep::name).collect();
            println!("{}", fl!("cli-macro-entry", n = (i as u32), steps = words.join("  ")));
        }
    };
    match n {
        None => (0..count).filter(|i| !kvm_map::macros::get(&buf, *i).is_empty()).for_each(|i| show(&buf, i)),
        Some(i) if i as usize >= count => {
            let max = count as u32 - 1;
            bail!(fl!("cli-macro-range", max = max))
        }
        Some(i) if steps.is_empty() => show(&buf, i as usize),
        Some(i) => {
            let m = if steps == ["none"] {
                Vec::new()
            } else {
                let parsed: Vec<MacroStep> =
                    steps.iter().map(|s| MacroStep::parse(s)).collect::<Result<_, _>>().map_err(anyhow::Error::msg)?;
                MacroStep::write(&parsed).ok_or_else(|| anyhow::anyhow!(fl!("cli-macro-too-long")))?
            };
            let after = kvm_map::macros::replace::<MACRO_BUFFER>(&buf, count, i as usize, &m)
                .ok_or_else(|| anyhow::anyhow!(fl!("cli-macro-too-long")))?;
            c.set_macros(&buf, &after)?;
            show(&after, i as usize);
        }
    }
    Ok(())
}

/// `odkvmctl monitor <device>`: its raw reports as they change, with the fields its descriptor declares (hid_mapping.md
/// §9), to find what a field override should read.
fn monitor(c: &mut Client, device: &str, seconds: u32) -> Result<()> {
    let d = parse_device(device)?;
    for (iface, map) in c.device_maps(d)? {
        let Ok(l) = kvm_hid::Layout::parse(&map) else { continue };
        println!("{}", fl!("cli-monitor-iface", iface = u32::from(iface), fields = l.fields.len()));
        for f in l.fields.iter().filter(|f| f.ty == kvm_hid::ReportType::Input) {
            let usage = f.usages.first().map_or(0, |u| u.min);
            let line = fl!(
                "cli-monitor-field",
                id = u32::from(f.report_id),
                bit = u32::from(f.bit_offset),
                bits = u32::from(f.bit_size),
                count = u32::from(f.count),
                usage = format!("{:04x}:{:04x}", usage >> 16, usage & 0xFFFF)
            );
            println!("  {line}");
        }
    }
    c.monitor(d, true)?;
    println!("{}", fl!("cli-monitor-start", seconds = seconds));
    let until = Instant::now() + Duration::from_secs(seconds as u64);
    let mut renew = Instant::now() + WATCH_RENEW;
    let mut pieces: std::collections::BTreeMap<u8, Vec<u8>> = Default::default();
    let mut last: std::collections::BTreeMap<(u8, u8), Vec<u8>> = Default::default();
    let r = (|| -> Result<()> {
        while Instant::now() < until {
            if Instant::now() >= renew {
                c.monitor(d, true)?;
                renew = Instant::now() + WATCH_RENEW;
            }
            for e in c.events(Duration::from_millis(100))? {
                let Event::Report { iface, offset, total, len, data, .. } = e else { continue };
                let buf = pieces.entry(iface).or_default();
                if offset == 0 {
                    buf.clear();
                }
                buf.extend_from_slice(&data[..len as usize]);
                if buf.len() < total as usize {
                    continue;
                }
                let report = std::mem::take(buf);
                let before = last.insert((iface, report[0]), report.clone()).unwrap_or_default();
                let hex: Vec<String> =
                    report
                        .iter()
                        .enumerate()
                        .map(|(i, b)| {
                            if before.get(i).is_some_and(|o| o != b) {
                                format!("{b:02x}*")
                            } else {
                                format!("{b:02x} ")
                            }
                        })
                        .collect();
                println!("{}", fl!("cli-monitor-report", iface = u32::from(iface), bytes = hex.concat()));
            }
        }
        Ok(())
    })();
    c.monitor(d, false)?;
    r
}

/// `odkvmctl field <device> …`: the bits of its reports made controls (hid_mapping.md §9).
fn field(c: &mut Client, device: &str, action: FieldAction) -> Result<()> {
    use kvm_proto::ctrl::{FieldOverride, MAX_OVERRIDES, field};
    let d = parse_device(device)?;
    let target_name = |t: u8| match t.checked_sub(field::AXIS) {
        Some(a) => keycodes::AXES[kvm_map::axes::RELATIVE + a as usize].to_string(),
        None => format!("pad{}", t + 1),
    };
    let show = |c: &mut Client, n: u8| -> Result<()> {
        if let Some(o) = c.field_override(d, n)? {
            let mut flags = Vec::new();
            if o.flags & field::SIGNED != 0 {
                flags.push("signed");
            }
            if o.flags & field::INVERT != 0 {
                flags.push("invert");
            }
            let line = fl!(
                "cli-field-entry",
                n = u32::from(n),
                iface = u32::from(o.iface),
                id = u32::from(o.report_id),
                bit = u32::from(o.bit),
                bits = u32::from(o.bits),
                target = target_name(o.target),
                flags = flags.join(" ")
            );
            println!("{line}");
        }
        Ok(())
    };
    match action {
        FieldAction::List => {
            for n in 0..MAX_OVERRIDES as u8 {
                show(c, n)?;
            }
        }
        FieldAction::Set { n, iface, report_id, bit, bits, target, signed, invert } => {
            let t = match target.strip_prefix("pad").and_then(|n| n.parse::<u8>().ok()) {
                Some(b @ 1..=32) => b - 1,
                _ => {
                    let a = keycodes::AXES[kvm_map::axes::RELATIVE..]
                        .iter()
                        .position(|a| *a == target)
                        .ok_or_else(|| anyhow::anyhow!(fl!("cli-field-target-bad", value = target.clone())))?;
                    field::AXIS + a as u8
                }
            };
            let flags = if signed { field::SIGNED } else { 0 } | if invert { field::INVERT } else { 0 };
            c.set_field_override(d, n, Some(FieldOverride { iface, report_id, bit, bits, flags, target: t }))?;
            show(c, n)?;
        }
        FieldAction::Remove { n } => c.set_field_override(d, n, None)?,
    }
    Ok(())
}

/// `odkvmctl outputs`: what each host is offered and took (hid_mapping.md §6.1).
fn outputs(c: &mut Client) -> Result<()> {
    use kvm_proto::ctrl::outputs as o;
    const NAMES: [(u8, &str); 8] = [
        (o::KEYBOARD, "keyboard"),
        (o::MOUSE, "mouse"),
        (o::MEDIA, "media"),
        (o::POINTER, "pointer"),
        (o::GAMEPAD, "gamepad"),
        (o::XINPUT, "xinput"),
        (o::VIAL, "vial"),
        (o::MIRRORS, "mirrors"),
    ];
    let s = c.snapshot()?;
    let hosts: Vec<Host> = Host::ALL.iter().copied().filter(|h| h.bt_slot().is_none() || s.has_bluetooth()).collect();
    for h in hosts {
        let (offered, taken) = c.outputs(h)?;
        let list =
            |bits: u8| NAMES.iter().filter(|(b, _)| bits & b != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(", ");
        let name = names::host_name(h);
        if offered == 0 {
            println!("{}", fl!("cli-outputs-none", host = name));
        } else if taken == 0 {
            println!("{}", fl!("cli-outputs-idle", host = name, outputs = list(offered)));
        } else if offered & !taken == 0 {
            println!("{}", fl!("cli-outputs-all", host = name, outputs = list(taken)));
        } else {
            println!("{}", fl!("cli-outputs-some", host = name, taken = list(taken), left = list(offered & !taken)));
        }
    }
    Ok(())
}

/// `odkvmctl layout [<host> [<layout>]]`: each host's keyboard layout, which macro text is typed through.
fn layout_cmd(c: &mut Client, host: Option<&str>, set: Option<&str>) -> Result<()> {
    use kvm_proto::ctrl::key;
    let parse_host = |h: &str| {
        names::parse_host(h)
            .ok_or_else(|| anyhow::anyhow!(fl!("parse-one-of", value = h, choices = "pc1, pc2, host1…host4")))
    };
    let hosts: Vec<Host> = match host {
        Some(h) => vec![parse_host(h)?],
        None => Host::ALL.to_vec(),
    };
    if let (Some(h), Some(l)) = (hosts.first(), set) {
        let l = names::parse_layout(l).map_err(anyhow::Error::msg)?;
        c.set_config(ConfigValue::HostLayout(*h, l as u8))?;
    }
    for h in hosts {
        let l = match c.get_config(key::layout(h))? {
            ConfigValue::HostLayout(_, l) => kvm_hid::layouts::Layout::from_u8(l).unwrap_or_default(),
            _ => kvm_hid::layouts::Layout::Us,
        };
        println!("{}", fl!("cli-layout-entry", host = names::host_name(h), layout = l.name()));
    }
    Ok(())
}

/// `odkvmctl calibrate <device> …`: a gamepad's axes (hid_mapping.md §8).
fn calibrate(c: &mut Client, device: &str, action: CalAction) -> Result<()> {
    use kvm_hid::gamepad::{AXES, axis};
    use kvm_proto::ctrl::AxisCal;
    let d = Device::from_name(device).ok_or_else(|| {
        anyhow::anyhow!(fl!("parse-one-of", value = device, choices = "keyboard, mouse, other, wireless1…wireless4"))
    })?;
    let present = c.controls(d)?.pad_axes;
    let name = |a: usize| keycodes::AXES[kvm_map::axes::RELATIVE + a];
    match action {
        CalAction::Show => {
            for a in (0..AXES).filter(|a| present & (1 << a) != 0) {
                let cal = c.calibration(d, a as u8)?;
                println!(
                    "{}",
                    fl!(
                        "cli-calibration",
                        axis = name(a),
                        min = i32::from(cal.min),
                        centre = i32::from(cal.centre),
                        max = i32::from(cal.max),
                        deadzone = u32::from(cal.deadzone)
                    )
                );
            }
        }
        CalAction::Set { axis, min, centre, max, deadzone } => {
            let a = keycodes::parse_axis(&axis).map_err(anyhow::Error::msg)?;
            let a =
                a.checked_sub(kvm_map::axes::RELATIVE).ok_or_else(|| anyhow::anyhow!(fl!("cli-calibrate-not-pad")))?;
            c.set_calibration(d, a as u8, AxisCal { min, centre, max, deadzone: deadzone.unwrap_or(0) })?;
        }
        CalAction::Reset => {
            for a in 0..AXES {
                c.set_calibration(d, a as u8, AxisCal::NONE)?;
            }
        }
        CalAction::Measure { seconds } => {
            // Raw values: the calibration off while measuring.
            for a in 0..AXES {
                c.set_calibration(d, a as u8, AxisCal::NONE)?;
            }
            c.watch_controls(1)?;
            println!("{}", fl!("cli-calibrate-move", seconds = seconds));
            let mut seen = [(i16::MAX, i16::MIN); AXES];
            let mut last = [0i16; AXES];
            let until = Instant::now() + Duration::from_secs(seconds as u64);
            let mut renew = Instant::now() + WATCH_RENEW;
            while Instant::now() < until {
                if Instant::now() >= renew {
                    c.watch_controls(1)?;
                    renew = Instant::now() + WATCH_RENEW;
                }
                for e in c.events(Duration::from_millis(100))? {
                    if let Event::Axis(i, v) = e
                        && let Some(a) = (i as usize).checked_sub(kvm_map::axes::RELATIVE)
                    {
                        seen[a] = (seen[a].0.min(v), seen[a].1.max(v));
                        last[a] = v;
                    }
                }
            }
            println!("{}", fl!("cli-calibrate-let-go"));
            let rest = Instant::now() + Duration::from_secs(2);
            while Instant::now() < rest {
                for e in c.events(Duration::from_millis(100))? {
                    if let Event::Axis(i, v) = e
                        && let Some(a) = (i as usize).checked_sub(kvm_map::axes::RELATIVE)
                    {
                        last[a] = v;
                    }
                }
            }
            c.watch_controls(0)?;
            for a in (0..AXES).filter(|a| present & (1 << a) != 0 && seen[*a].0 < seen[*a].1) {
                let centre = if axis::is_trigger(a) { 0 } else { last[a] };
                let cal = AxisCal { min: seen[a].0, centre, max: seen[a].1, deadzone: 8 };
                c.set_calibration(d, a as u8, cal)?;
                println!(
                    "{}",
                    fl!(
                        "cli-calibration",
                        axis = name(a),
                        min = i32::from(cal.min),
                        centre = i32::from(cal.centre),
                        max = i32::from(cal.max),
                        deadzone = u32::from(cal.deadzone)
                    )
                );
            }
        }
    }
    Ok(())
}

fn display(action: Option<DisplayAction>) -> Result<()> {
    match action.unwrap_or(DisplayAction::List) {
        DisplayAction::List => {
            let list = ddc::displays(true).map_err(anyhow::Error::msg)?;
            if list.is_empty() {
                println!("{}", fl!("cli-no-monitors"));
            }
            for d in list {
                println!("{}  {}", d.id, d.name);
                match (d.current, &d.error) {
                    (Some(v), _) => println!("  {}", fl!("monitor-shows", input = mccs::input_name(v))),
                    (None, Some(e)) => println!("  {e}"),
                    (None, None) => {}
                }
                if !d.inputs.is_empty() {
                    let names: Vec<String> = d.inputs.iter().map(|v| mccs::input_name(*v)).collect();
                    println!("  {}", fl!("cli-inputs", inputs = names.join(", ")));
                }
            }
        }
        DisplayAction::Input { id, value } => {
            let v = value.trim();
            let v = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                Some(hex) => u16::from_str_radix(hex, 16),
                None => v.parse(),
            }
            .with_context(|| fl!("cli-input-number", value = value.as_str()))?;
            ddc::set_input(&id, v).map_err(anyhow::Error::msg)?;
        }
    }
    Ok(())
}

/// What a board in the boot ROM's bootloader is, in words.
fn board_text(b: &Result<Board, String>) -> String {
    match b {
        Ok(Board::Kvm { installed: Some(v), slot }) => {
            fl!("boot-kvm", version = names::version(*v), slot = if *slot == 0 { "A" } else { "B" })
        }
        Ok(Board::Kvm { installed: None, slot }) => fl!("boot-kvm-empty", slot = if *slot == 0 { "A" } else { "B" }),
        Ok(Board::New { installed: None }) => fl!("boot-new"),
        Ok(Board::New { installed: Some(v) }) => fl!("boot-new-unpartitioned", version = names::version(*v)),
        Ok(Board::Previous { installed }) => {
            fl!("boot-kvm-previous", version = installed.map_or_else(|| "-".to_string(), names::version))
        }
        Ok(Board::Other) => fl!("boot-other"),
        Err(e) => e.clone(),
    }
}

/// `odkvmctl update [FILE] [--module]`: over a PC port to the KVM that answers there, the RP2354B's firmware and then
/// its module's (the release's `kvm-module-<chip>.bin`, once the KVM is back); otherwise onto a board in the boot ROM's
/// bootloader (PWR/SERVICE port). `--module`: only the module's. Without FILE, the firmware shipped with this program.
fn update(sim: bool, file: Option<PathBuf>, module_only: bool) -> Result<()> {
    let path = match file.or_else(firmware::bundled) {
        Some(p) => p,
        None => bail!(fl!("cli-no-firmware")),
    };
    let progress_line = |what: &str, done: usize, total: usize| {
        print!("\r{what} {:3}%", done * 100 / total.max(1));
        let _ = std::io::stdout().flush();
    };
    // A board waiting in its bootloader on PWR/SERVICE comes first: it can do nothing else, and the KVM on a PC port
    // may be that same board, whose AT32F405 stays on the PC's bus while the RP2354B is in the boot ROM and so never
    // answers (first board, 2026-11-12: `update` picked it and timed out every time).
    let boot = if sim || module_only { Vec::new() } else { bootsel::Scanner::default().scan() };
    if !boot.is_empty() {
        let d = match boot.as_slice() {
            [d] => d,
            _ => bail!(fl!("cli-boot-several")),
        };
        let board = match &d.board {
            Ok(Board::Other) => bail!(fl!("boot-other")),
            Ok(b) => *b,
            Err(why) => bail!("{why}\n{}", fl!("cli-boot-unknown")),
        };
        println!("{}", board_text(&d.board));
        let f = firmware::for_bootloader(&path, board.takes_first()).map_err(anyhow::Error::msg)?;
        println!("{}", f.file.display());
        println!("{}", fl!("cli-firmware-for", version = names::version(f.version), major = u32::from(f.hw_major)));
        bootsel::install(d, &f.data, |done, total| progress_line(&fl!("boot-writing"), done, total))
            .map_err(anyhow::Error::msg)?;
        println!();
        println!("{}", fl!("boot-installed"));
        return Ok(());
    }
    let mut c = Client::connect(sim).map_err(|e| anyhow::Error::new(e).context(fl!("cli-no-kvm-anywhere")))?;
    let info = c.info()?;
    // The image for the module's chip (an ESP32-C3 if the KVM doesn't know it yet: it is looked up again below).
    let module = firmware::module_for(&path, c.status()?.bt_chip).map_err(anyhow::Error::msg)?;
    let mismatch = |m: &firmware::Module, v: u32| {
        anyhow::anyhow!(fl!(
            "fw-module-mismatch",
            file = m.file.display().to_string(),
            module = names::version(m.version),
            version = names::version(v)
        ))
    };
    if module_only {
        // The KVM's firmware takes only its own release's.
        let m = module.ok_or_else(|| anyhow::anyhow!(fl!("fw-no-module", folder = path.display().to_string())))?;
        if m.version != info.rp_fw {
            return Err(mismatch(&m, info.rp_fw));
        }
        return update_module(&mut c, &m, true);
    }
    let f = firmware::for_pc(&path).map_err(anyhow::Error::msg)?;
    image::check_hardware(f.version, f.hw_major, info.hw).map_err(anyhow::Error::msg)?;
    image::check_layout(info.rp_fw, f.version).map_err(anyhow::Error::msg)?;
    if let Some(m) = &module
        && m.version != f.version
    {
        return Err(mismatch(m, f.version));
    }
    println!("{}", f.file.display());
    println!("{}", fl!("cli-firmware-for", version = names::version(f.version), major = u32::from(f.hw_major)));
    let mut last = None;
    c.update_firmware(&f.data, |state, done, total| {
        if state == FwState::Receiving {
            progress_line(&format!("RP2354B: {}", names::fw_state(FwState::Receiving)), done, total);
        } else if last != Some(state) {
            // End the progress line once, then one line per state.
            if last == Some(FwState::Receiving) {
                println!();
            }
            println!("{}", names::fw_state(state));
        }
        last = Some(state);
    })?;
    println!("{}", fl!("installed"));
    let Some(m) = module else { return Ok(()) };
    // The KVM restarts into the new firmware; then the module's turn.
    println!("{}", fl!("cli-waiting-for-kvm"));
    drop(c);
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut c = loop {
        if let Ok(mut c) = Client::connect(sim)
            && c.info().is_ok_and(|i| i.rp_fw == f.version)
        {
            break c;
        }
        if Instant::now() > deadline {
            bail!(fl!("cli-kvm-not-back"));
        }
        std::thread::sleep(Duration::from_secs(1));
    };
    // The new firmware may know the module's chip now (its `Hello`, or its ROM).
    let chip = c.status()?.bt_chip;
    let m = firmware::module_for(&path, chip).map_err(anyhow::Error::msg)?.unwrap_or(m);
    update_module(&mut c, &m, false)
}

/// Streams the module's firmware to the module if it needs it (`force`: whatever it runs), then waits until it is up.
fn update_module(c: &mut Client, m: &firmware::Module, force: bool) -> Result<()> {
    // After a restart the module first says which firmware it runs.
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut st = c.status()?;
    while st.bt_module == BtModule::Starting && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        st = c.status()?;
    }
    match st.bt_module {
        BtModule::Absent => {
            println!("{}", fl!("cli-update-no-module"));
            return Ok(());
        }
        BtModule::Up if !force => {
            println!("{}", fl!("cli-module-current"));
            return Ok(());
        }
        _ => {}
    }
    println!("{}", m.file.display());
    let what = fl!("wireless-module");
    let mut last = None;
    c.update_module(&m.data, |state, done, total| {
        if state == FwState::Receiving {
            print!("\r{what}: {} {:3}%", names::fw_state(state), done * 100 / total.max(1));
            let _ = std::io::stdout().flush();
        } else if last != Some(state) {
            if last == Some(FwState::Receiving) {
                println!();
            }
            println!("{what}: {}", names::fw_state(state));
        }
        last = Some(state);
    })?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = c.status()?;
        if st.bt_module == BtModule::Up {
            println!("{}", fl!("cli-module-up", version = names::version(c.info()?.bt_fw)));
            return Ok(());
        }
        if Instant::now() > deadline {
            bail!(fl!("cli-module-not-up", state = names::bt_module(st.bt_module)));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn main() -> Result<()> {
    // The language set in the window (Settings), or the system's; before the arguments, for the help texts.
    odkvmctl::i18n::init(local::load().language.as_deref());
    let cli = Cli::parse();
    if let Some(Cmd::Display { action }) = cli.cmd {
        return display(action);
    }
    if let Some(Cmd::List) = cli.cmd {
        let found = odkvmctl::transport::find().map_err(anyhow::Error::msg)?;
        let boot = bootsel::Scanner::default().scan();
        if found.is_empty() && boot.is_empty() {
            println!("{}", fl!("cli-no-interface"));
        }
        for f in found {
            match f.link {
                Link::Bluetooth => println!("{:<9}  {:<24} {}", fl!("cli-list-wireless"), f.product, f.serial),
                _ => println!("{:04x}:{:04x}  {:<24} {}", f.vid, f.pid, f.product, f.path.to_string_lossy()),
            }
        }
        for d in &boot {
            let drive = d.drive.as_ref().map_or_else(|| fl!("cli-boot-no-drive"), |p| p.display().to_string());
            println!("{:04x}:{:04x}  {:<24} {}", bootsel::VID, bootsel::PID, board_text(&d.board), drive);
        }
        return Ok(());
    }
    if let Some(Cmd::Update { file, module }) = &cli.cmd {
        return update(cli.sim, file.clone(), *module);
    }
    let mut c = Client::connect(cli.sim).context(fl!("cli-connecting"))?;
    // Every command but `update` needs firmware that speaks this program's protocol.
    c.info_checked()?;

    match cli.cmd.unwrap_or(Cmd::Status) {
        Cmd::Status => {
            let s = c.snapshot()?;
            print_status(&c, &s);
        }
        Cmd::List | Cmd::Display { .. } | Cmd::Update { .. } => unreachable!(),
        Cmd::Screens { arrangement, action } => {
            let s = c.snapshot()?;
            let hop = s.hop();
            let named = arrangement.as_deref().map(|w| find_arrangement(w, &hop)).transpose()?;
            match action {
                Some(ScreensAction::Detect) => {
                    let Some(pc) = s.this_pc() else { bail!(fl!("cli-screens-which-pc")) };
                    let host = Host::pc(pc);
                    let a = named.unwrap_or(hop.current());
                    let found = odkvmctl::screens::detect().map_err(anyhow::Error::msg)?;
                    let screens = &hop.arrangements[a as usize].screens;
                    let next = odkvmctl::screens::merge(screens, host, &found);
                    for (i, (was, now)) in screens.iter().zip(next.iter()).enumerate() {
                        if was != now {
                            c.set_config(ConfigValue::Screen(a, i as u8, *now))?;
                        }
                    }
                    c.set_config(ConfigValue::HopMapping(host, odkvmctl::screens::MAPPING))?;
                    c.set_config(ConfigValue::HopUnits(host, found.units))?;
                    println!(
                        "{}",
                        fl!(
                            "hop-detected",
                            count = found.screens.len(),
                            pc = pc.name().to_uppercase(),
                            mapping = names::mapping(odkvmctl::screens::MAPPING)
                        )
                    );
                }
                Some(ScreensAction::List) | None => {}
            }
            let s = c.snapshot()?;
            print_screens(&s, named);
            check_screens(&s);
        }
        Cmd::Diag => {
            let items = c.diag()?;
            if print_diag(&items) {
                std::process::exit(1);
            }
        }
        Cmd::Watch => {
            println!("{}", fl!("cli-watching", device = c.describe()));
            loop {
                for e in c.events(Duration::from_millis(500))? {
                    print_event(&e);
                }
            }
        }
        Cmd::Switch { target } => {
            let t = match target {
                TargetArg::Pc1 => Target::Pc1,
                TargetArg::Pc2 => Target::Pc2,
                TargetArg::Toggle => Target::Toggle,
                TargetArg::Next => Target::Next,
                TargetArg::Host1 => Target::Bt1,
                TargetArg::Host2 => Target::Bt2,
                TargetArg::Host3 => Target::Bt3,
                TargetArg::Host4 => Target::Bt4,
            };
            match c.switch(t) {
                Err(odkvmctl::Error::Device(ErrorCode::Unsupported)) => bail!(fl!("cli-no-module")),
                Err(odkvmctl::Error::Device(ErrorCode::NoDevice)) => bail!(fl!("cli-no-host-in-slot")),
                r => r?,
            }
            let st = c.status()?;
            if st.bt_active != 0 {
                println!("{}", fl!("cli-active", target = fl!("cli-host-slot", slot = st.bt_active)));
            } else {
                println!("{}", fl!("cli-active", target = st.active.name().to_uppercase()));
            }
        }
        // From a computer that reaches the KVM over its module: before anything else, which it may not ask yet.
        Cmd::Wireless { action: Some(HostAction::Allow { slot: None }) } => {
            if c.link() != Link::Bluetooth {
                bail!(fl!("cli-allow-needs-slot"));
            }
            let seconds = kvm_core::engine::UNLOCK_WAIT_MS / 1000;
            c.ask_control(|| println!("{}", fl!("cli-press-key-within", seconds = seconds)))?;
            println!("{}", fl!("cli-allowed-here"));
        }
        // For a KVM without a module: before the check that it has one.
        Cmd::Wireless { action: Some(HostAction::Adopt { port, firmware }) } => {
            let port: Port = port.into();
            let name = names::port_title(port);
            match c.adopt_module(port) {
                Err(odkvmctl::Error::Device(ErrorCode::Busy)) => bail!(fl!("cli-adopt-busy")),
                Err(odkvmctl::Error::Device(ErrorCode::NoDevice)) => bail!(fl!("cli-adopt-empty", port = name)),
                r => r?,
            }
            println!("{}", fl!("cli-adopting", port = name.clone()));
            // In its ROM, its chip known: then its firmware.
            let deadline = Instant::now() + Duration::from_secs(60);
            let st = loop {
                let st = c.status()?;
                if st.bt_module == BtModule::NeedsUpdate && st.bt_chip != kvm_proto::link::bt::Chip::Unknown {
                    break st;
                }
                if Instant::now() > deadline {
                    bail!(fl!("cli-adopt-not-taken", port = name, state = names::bt_module(st.bt_module)));
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            let folder = firmware.or_else(firmware::bundled).context(fl!("fw-no-module", folder = "-"))?;
            let m = firmware::module_for(&folder, st.bt_chip)
                .map_err(anyhow::Error::msg)?
                .ok_or_else(|| anyhow::anyhow!(fl!("fw-no-module", folder = folder.display().to_string())))?;
            update_module(&mut c, &m, true)?;
        }
        Cmd::Wireless { action } => {
            let s = c.snapshot()?;
            if !s.has_bluetooth() {
                bail!(fl!("cli-no-module-boards"));
            }
            match action.unwrap_or(HostAction::List) {
                HostAction::List => print_hosts(&s),
                HostAction::Pair { stop } => match stop.as_deref() {
                    Some("stop") => c.bt_pair(false)?,
                    Some(other) => bail!(fl!("cli-pair-usage", value = other)),
                    None => {
                        c.bt_pair(true)?;
                        println!("{}", fl!("cli-pairing-opened", seconds = kvm_proto::ctrl::BT_PAIR_SECONDS));
                        println!("{}", fl!("cli-pairing-digits"));
                    }
                },
                HostAction::Passkey { digits } => {
                    let v: u32 = digits
                        .trim()
                        .parse()
                        .ok()
                        .filter(|v| *v <= 999_999 && digits.trim().len() == 6)
                        .context(fl!("cli-six-digits"))?;
                    match c.bt_passkey(v) {
                        Err(odkvmctl::Error::Device(ErrorCode::InvalidValue)) => bail!(fl!("cli-no-passkey-wanted")),
                        r => r?,
                    }
                }
                HostAction::Forget { slot } => {
                    let n = if slot == "all" {
                        0xFF
                    } else {
                        slot.parse::<u8>().ok().filter(|n| (1..=4).contains(n)).context(fl!("cli-slot-or-all"))?
                    };
                    c.bt_forget(n)?;
                }
                HostAction::Name { slot, name } => {
                    let k = names::config_key(&format!("host{slot}-name")).context(fl!("cli-slot"))?;
                    c.set_config(names::parse_value(k, name.as_deref().unwrap_or("")).map_err(anyhow::Error::msg)?)?;
                }
                HostAction::Policy { slot, policy } => {
                    let k = names::config_key(&format!("host{slot}-policy")).context(fl!("cli-slot"))?;
                    c.set_config(names::parse_value(k, &policy).map_err(anyhow::Error::msg)?)?;
                }
                HostAction::Allow { slot: Some(slot) } => {
                    let seconds = kvm_core::engine::UNLOCK_WAIT_MS / 1000;
                    c.allow_host(slot, true, || println!("{}", fl!("cli-press-key-within", seconds = seconds)))?;
                    println!("{}", fl!("cli-host-allowed", slot = slot));
                }
                HostAction::Allow { slot: None } => unreachable!(),
                HostAction::Disallow { slot } => c.allow_host(slot, false, || {})?,
                HostAction::Erase => {
                    c.bt_erase_all()?;
                    println!("{}", fl!("cli-wireless-erased"));
                }
                HostAction::Adopt { .. } => unreachable!(),
            }
        }
        Cmd::Port { port, action } => {
            let port: Port = port.into();
            if !c.snapshot()?.has_port(port) {
                bail!(fl!("cli-no-aux1"));
            }
            match action {
                None => {}
                Some(PortAction::Mode { mode }) => {
                    let m = names::parse_mode(port.into(), &mode).map_err(anyhow::Error::msg)?;
                    c.set_mode(port.into(), m)?;
                    // Re-detection and re-enumeration take a moment.
                    std::thread::sleep(Duration::from_millis(300));
                }
                Some(PortAction::Route { route }) => {
                    c.set_route(port.into(), names::parse_route(&route).map_err(anyhow::Error::msg)?)?;
                    std::thread::sleep(Duration::from_millis(300));
                }
                Some(PortAction::Redetect) => {
                    c.redetect(port)?;
                    std::thread::sleep(Duration::from_millis(1000));
                }
            }
            print_port(&c.snapshot()?, port);
        }
        Cmd::Keymap { action } => keymap(&mut c, action.unwrap_or(KeymapAction::Show { keymap: None }))?,
        Cmd::Calibrate { device, action } => calibrate(&mut c, &device, action.unwrap_or(CalAction::Show))?,
        Cmd::Monitor { device, seconds } => monitor(&mut c, &device, seconds)?,
        Cmd::Field { device, action } => field(&mut c, &device, action.unwrap_or(FieldAction::List))?,
        Cmd::Outputs => outputs(&mut c)?,
        Cmd::Layout { host, layout } => layout_cmd(&mut c, host.as_deref(), layout.as_deref())?,
        Cmd::Device { action } => {
            let s = c.snapshot()?;
            let action = action.unwrap_or(DeviceAction::List);
            // A front-port device's descriptor needs no module, nor the module's log (the KVM keeps it, and adds its own
            // lines: why programming a module failed, for one).
            if !s.has_bluetooth()
                && !matches!(&action, DeviceAction::Log)
                && !matches!(&action, DeviceAction::Map { device } if parse_device(device)?.port().is_some())
            {
                bail!(fl!("cli-no-module-boards"));
            }
            match action {
                DeviceAction::List => print_devices(&s),
                DeviceAction::Scan { stop } => match stop.as_deref() {
                    Some("stop") => c.bt_scan(false)?,
                    Some(other) => bail!(fl!("cli-scan-usage", value = other)),
                    None => scan(&mut c)?,
                },
                DeviceAction::Pair { device } => pair_device(&mut c, &s, &device)?,
                DeviceAction::Forget { slot } => {
                    let n = if slot == "all" {
                        0xFF
                    } else {
                        slot.parse::<u8>()
                            .ok()
                            .filter(|n| (1..=BT_DEVICES as u8).contains(n))
                            .context(fl!("cli-slot-or-all"))?
                    };
                    c.bt_forget_device(n)?;
                }
                DeviceAction::Mode { slot, mode } => {
                    let d = Device::wireless_slot(slot).context(fl!("cli-slot"))?;
                    c.set_mode(d, names::parse_mode(d, &mode).map_err(anyhow::Error::msg)?)?;
                }
                DeviceAction::Route { slot, route } => {
                    let d = Device::wireless_slot(slot).context(fl!("cli-slot"))?;
                    c.set_route(d, names::parse_route(&route).map_err(anyhow::Error::msg)?)?;
                }
                DeviceAction::Log => {
                    let log = c.module_log()?;
                    if log.is_empty() {
                        println!("{}", fl!("cli-device-log-empty"));
                    }
                    print!("{log}");
                }
                DeviceAction::Map { device } => {
                    let d = parse_device(&device)?;
                    for (iface, map) in c.device_maps(d)? {
                        println!("{}", fl!("cli-device-map-iface", iface = u32::from(iface)));
                        print_device_map(&map);
                    }
                }
            }
        }
        Cmd::Profile { which, setting, value, route } => {
            let s = c.snapshot()?;
            let Some((active, count, list)) = s.profiles() else { bail!(fl!("cli-no-profiles")) };
            match (which, setting, value) {
                (None, ..) => {}
                (Some(w), None, _) => {
                    let i = if w == "next" { (active + 1) % count } else { find_profile(&w, &list)? };
                    if i >= count {
                        let count: u8 = i + 1;
                        bail!(fl!("cli-profile-not-in-use", n = i, count = count));
                    }
                    c.set_config(ConfigValue::Profile(i))?;
                }
                (Some(w), Some(setting), Some(value)) => {
                    let i = find_profile(&w, &list)?;
                    let v = if setting == "name" {
                        ConfigValue::ProfileName(i, names::parse_profile_name(&value).map_err(anyhow::Error::msg)?)
                    } else if setting == "arrangement" {
                        let hop = s.hop();
                        let a = match value.as_str() {
                            "none" => None,
                            v => Some(find_arrangement(v, &hop)?),
                        };
                        ConfigValue::ProfileArrangement(i, a)
                    } else {
                        let d = Device::from_name(&setting).with_context(|| {
                            let choices = "name, arrangement, keyboard, mouse, other, wireless1–wireless4";
                            fl!("parse-one-of", value = setting.as_str(), choices = choices)
                        })?;
                        // The mode (`-` keeps it), then the routing if given.
                        if let Some(r) = &route {
                            let mut routes = list[i as usize].routes;
                            routes[d.index()] = names::parse_route(r).map_err(anyhow::Error::msg)?;
                            c.set_config(ConfigValue::ProfileRoutes(i, routes))?;
                        }
                        let mut modes = list[i as usize].modes;
                        if value != "-" {
                            modes[d.index()] = names::parse_mode(d, &value).map_err(anyhow::Error::msg)?;
                        }
                        ConfigValue::ProfileModes(i, modes)
                    };
                    c.set_config(v)?;
                }
                (Some(_), Some(setting), None) => bail!(fl!("cli-missing-value", setting = setting)),
            }
            let s = c.snapshot()?;
            let (active, count, list) = s.profiles().context("profiles")?;
            print_profiles(&s, active, count, &list, &s.hop());
        }
        Cmd::Hotkey { action } => {
            let list = hotkey_list(&mut c)?;
            let (i, binding) = match action.unwrap_or(HotkeyCmd::List) {
                HotkeyCmd::List => {
                    let used: Vec<_> = list.iter().enumerate().filter(|(_, b)| b.is_used()).collect();
                    if used.is_empty() {
                        println!("{}", fl!("cli-hotkeys-none"));
                    }
                    for (i, b) in used {
                        print_hotkey(i as u8, b);
                    }
                    return Ok(());
                }
                HotkeyCmd::Add { action, hotkey } => {
                    let action = names::parse_action(&action).map_err(anyhow::Error::msg)?;
                    // After the last entry in use, else in the first free one.
                    let after = list.iter().rposition(HotkeyBinding::is_used).map_or(0, |j| j + 1);
                    let free = (after..MAX_HOTKEYS).chain(0..after).find(|j| !list[*j].is_used());
                    let Some(i) = free.map(|j| j as u8) else { bail!(fl!("cli-hotkeys-full", max = MAX_HOTKEYS)) };
                    match hotkey_for(&mut c, &list, i, action, &hotkey)? {
                        Some(h) => (i, HotkeyBinding::new(action, h)),
                        None => {
                            eprintln!("{}", fl!("cli-nothing-changed"));
                            return Ok(());
                        }
                    }
                }
                HotkeyCmd::Set { entry, hotkey, action } => {
                    let i = hotkey_entry(entry)?;
                    let action = match action {
                        Some(a) => names::parse_action(&a).map_err(anyhow::Error::msg)?,
                        None if list[i as usize].is_used() => list[i as usize].action,
                        None => bail!(fl!("cli-hotkey-unused", n = entry)),
                    };
                    match hotkey_for(&mut c, &list, i, action, &hotkey)? {
                        Some(h) => (i, HotkeyBinding::new(action, h)),
                        None => {
                            eprintln!("{}", fl!("cli-nothing-changed"));
                            return Ok(());
                        }
                    }
                }
                HotkeyCmd::Remove { entry } => (hotkey_entry(entry)?, HotkeyBinding::NONE),
            };
            let n: u32 = i as u32 + 1;
            match c.set_config(ConfigValue::Hotkey(i, binding))? {
                ConfigValue::Hotkey(i, b) if b.is_used() => print_hotkey(i, &b),
                _ => println!("{}", fl!("cli-hotkey-removed", n = n)),
            }
        }
        Cmd::Config { action } => match action {
            ConfigAction::Get { name: None } => {
                for v in c.all_config()? {
                    println!("{:<20} {}", names::config_name(v.key()), names::format_value(&v));
                }
            }
            ConfigAction::Get { name: Some(n) } => {
                let k = names::config_key(&n).with_context(|| fl!("cli-unknown-setting", name = n.as_str()))?;
                println!("{}", names::format_value(&c.get_config(k)?));
            }
            ConfigAction::Set { name, value } => {
                let k = names::config_key(&name).with_context(|| fl!("cli-unknown-setting", name = name.as_str()))?;
                let v = names::parse_value(k, &value).map_err(anyhow::Error::msg)?;
                let v = match v {
                    ConfigValue::SwSwitchAllowed(true) | ConfigValue::SwConfigAllowed(true) => {
                        let seconds = kvm_core::engine::UNLOCK_WAIT_MS / 1000;
                        c.allow_software(v, || println!("{}", fl!("cli-press-key-within", seconds = seconds)))?;
                        v
                    }
                    ConfigValue::BtControl(i, true) => {
                        let seconds = kvm_core::engine::UNLOCK_WAIT_MS / 1000;
                        c.allow_host(i + 1, true, || println!("{}", fl!("cli-press-key-within", seconds = seconds)))?;
                        v
                    }
                    ConfigValue::SwSwitchAllowed(false) | ConfigValue::SwConfigAllowed(false) => {
                        if !confirm_lock(matches!(v, ConfigValue::SwSwitchAllowed(_)))? {
                            bail!(fl!("cli-nothing-changed"));
                        }
                        c.set_config(v)?
                    }
                    v => c.set_config(v)?,
                };
                // Under its name now, also when an earlier one was typed (screen0, profile1-hop).
                println!("{} = {}", names::config_name(k), names::format_value(&v));
            }
            ConfigAction::Save => {
                c.save()?;
                println!("{}", fl!("cli-saved"));
            }
            ConfigAction::Reset => {
                c.factory_reset()?;
                println!("{}", fl!("cli-factory-restored"));
            }
        },
        Cmd::Identify { seconds } => c.identify(seconds)?,
        Cmd::Reboot { target } => c.reboot(match target {
            RebootArg::Normal => RebootTarget::Normal,
            RebootArg::Bootloader => RebootTarget::Bootloader,
            RebootArg::At32Dfu1 => RebootTarget::At32Dfu1,
            RebootArg::At32Dfu2 => RebootTarget::At32Dfu2,
            RebootArg::At32Retry => RebootTarget::At32Retry,
        })?,
    }
    Ok(())
}
