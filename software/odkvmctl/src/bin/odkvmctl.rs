//! `odkvmctl`: command-line control of the OpenDynKVM (software.md §9). Its texts, help included, come from the
//! translations (`odkvmctl::i18n`); clap's own words (Usage, Options, …) stay English.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use kvm_core::hop::main_screen;
use kvm_core::hotkey::format_spec;
use kvm_proto::ctrl::{
    BT_DEVICES, BtDeviceInfo, BtFound, ConfigValue, Device, DevicePairing, ErrorCode, Event, FwState, Host, Hotkey,
    HotkeyAction, HotkeyBinding, LearnEnd, MAX_ARRANGEMENTS, MAX_HOTKEYS, MAX_PROFILES, Mapping, Pc, Port, PortMode,
    RebootTarget, Routing, Target, diag, key, selftest, status_flags,
};
use odkvmctl::bootsel::{self, Board};
use odkvmctl::client::{BtHostView, DeviceSlot, HopView, ProfileView, Snapshot};
use odkvmctl::ddc::{self, mccs};
use odkvmctl::{Client, firmware, fl, image, local, names};

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
    Map { slot: u8 },
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
    #[value(alias = "ot")]
    Other,
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
    #[value(help = fl!("cli-reboot-wireless-retry"))]
    WirelessRetry,
}

impl From<PortArg> for Port {
    fn from(p: PortArg) -> Self {
        match p {
            PortArg::Keyboard => Port::Keyboard,
            PortArg::Mouse => Port::Mouse,
            PortArg::Other => Port::Other,
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
        st.asking.map_or(String::new(), |p| format!("   ({})", fl!("cli-this-computer", pc = p.name().to_uppercase())));
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
    if st.selftest & selftest::BT_GAVE_UP != 0 {
        println!("  {}", fl!("cli-try-again", command = "odkvmctl reboot wireless-retry"));
    }
    println!("{}", fl!("cli-ports"));
    for p in Port::ALL {
        print_port(s, *p);
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
        println!(
            "{mark} {}  {:<12}  {:<22} {}  {}{}",
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
    }
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
        Ok(Board::Other) => fl!("boot-other"),
        Err(e) => e.clone(),
    }
}

/// `odkvmctl update [FILE]`: over a PC port to the KVM that answers there; otherwise onto a board in the boot ROM's
/// bootloader (PWR/SERVICE port). Without FILE, the firmware shipped with this program.
fn update(sim: bool, file: Option<PathBuf>) -> Result<()> {
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
    let boot = if sim { Vec::new() } else { bootsel::Scanner::default().scan() };
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
    let hw = c.info()?.hw;
    let f = firmware::for_pc(&path).map_err(anyhow::Error::msg)?;
    image::check_hardware(f.version, f.hw_major, hw).map_err(anyhow::Error::msg)?;
    println!("{}", f.file.display());
    println!("{}", fl!("cli-firmware-for", version = names::version(f.version), major = u32::from(f.hw_major)));
    let mut last = None;
    c.update_firmware(&f.data, |state, done, total| {
        if state == FwState::Receiving {
            progress_line(&names::fw_state(FwState::Receiving), done, total);
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
    Ok(())
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
            println!("{:04x}:{:04x}  {:<24} {}", f.vid, f.pid, f.product, f.path.to_string_lossy());
        }
        for d in &boot {
            let drive = d.drive.as_ref().map_or_else(|| fl!("cli-boot-no-drive"), |p| p.display().to_string());
            println!("{:04x}:{:04x}  {:<24} {}", bootsel::VID, bootsel::PID, board_text(&d.board), drive);
        }
        return Ok(());
    }
    if let Some(Cmd::Update { file }) = &cli.cmd {
        return update(cli.sim, file.clone());
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
            }
        }
        Cmd::Port { port, action } => {
            let port: Port = port.into();
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
        Cmd::Device { action } => {
            let s = c.snapshot()?;
            if !s.has_bluetooth() {
                bail!(fl!("cli-no-module-boards"));
            }
            match action.unwrap_or(DeviceAction::List) {
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
                DeviceAction::Map { slot } => {
                    Device::wireless_slot(slot).context(fl!("cli-slot"))?;
                    print_device_map(&c.bt_device_map(slot)?);
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
            RebootArg::WirelessRetry => RebootTarget::BtRetry,
        })?,
    }
    Ok(())
}
