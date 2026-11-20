//! The module as the central of up to four Bluetooth devices (software.md §4.13, "Bluetooth devices"): keyboards, mice
//! and other HID devices over GATT (HOGP), while it is a keyboard and mouse to its hosts ([`crate::ble`]).
//!
//! - **Slots:** each bonded device has a slot 1–4 ([`DeviceBonds`]), with its keys, its report map and its reports'
//!   handles, so that a device that wakes up and reconnects reports at once, without being read anew.
//! - **Looking for devices** ([`BtMsg::Scan`]): an active scan. A HID device that advertises as discoverable goes to
//!   the RP2354B ([`BtMsg::Found`]), again at most once a second.
//! - **Pairing** ([`BtMsg::PairDevice`]): connect, pair (LE Secure Connections, or legacy pairing for the many devices
//!   that only have that; IO capability DisplayOnly: a keyboard that asks for a passkey gets one to type, a mouse pairs
//!   without), read the HID service, store it all, and switch the reports on. Host pairing is closed meanwhile (the
//!   RP2354B sees to it): the stack has one set of IO capabilities, and hosts keep KeyboardOnly and Secure Connections.
//! - **Reconnecting:** while a bonded device isn't connected, a passive scan watches for its advertising (its address,
//!   or one its IRK resolves) and connects at once.
//! - **Reports:** each input report's notification goes to the RP2354B with its report ID ([`BtMsg::DeviceInput`]);
//!   output reports (a keyboard's LEDs) come back the other way ([`BtMsg::DeviceOutput`]).

use core::cell::RefCell;
use core::ops::ControlFlow;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use kvm_proto::crc::crc32;
use kvm_proto::ctrl::DevicePairing;
use kvm_proto::link::bt::{
    BtMsg, DeviceState, EventKind, MAP_CHUNK, MAP_MAX, MAX_DEVICES, NAME_MAX, appearance, device_flags,
};
use trouble_host::prelude::*;
use trouble_host::scan::Scanner;

use crate::ble::{Controller, St};
use crate::bonds::{Device, DeviceBonds, Report};
use crate::{link, log};

const HID_SERVICE: u16 = 0x1812;
const BATTERY_SERVICE: u16 = 0x180F;
const GAP_SERVICE: u16 = 0x1800;
const REPORT_MAP: u16 = 0x2A4B;
const REPORT: u16 = 0x2A4D;
const PROTOCOL_MODE: u16 = 0x2A4E;
const BATTERY_LEVEL: u16 = 0x2A19;
const DEVICE_NAME: u16 = 0x2A00;
const REPORT_REFERENCE: u16 = 0x2908;

/// How long connecting to a device picked for pairing may take before it counts as out of reach.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long pairing may stall (a passkey not typed): SMP's own limit is 30 s.
const SECURE_TIMEOUT: Duration = Duration::from_secs(40);
/// A bonded device's link not encrypted with its key this long after it connected: the connection goes, so that the
/// device's next advertising gets a fresh one.
const ENCRYPT_GIVE_UP: Duration = Duration::from_secs(4);
/// A reconnection that failed holds off the next one this long, doubling up to 4 s while they keep failing: a device
/// isn't hammered, and still caught while it advertises.
const RETRY_FIRST: Duration = Duration::from_millis(500);
/// Setting a connection up once it is encrypted (the GATT client, the reports switched on, the battery) may take this
/// long; then the connection goes too, rather than wait out ATT's 30 s for a device that doesn't answer.
const SETUP_TIMEOUT: Duration = Duration::from_secs(8);
/// A device found is reported again at most this often (with its signal now).
const FOUND_EVERY: Duration = Duration::from_secs(1);

/// What the Bluetooth side knows of the devices (one executor: the mutex only guards against interrupts).
struct State {
    bonds: &'static mut DeviceBonds,
    devices: [DeviceState; MAX_DEVICES],
    /// A connection holds the slot (its task runs).
    live: [bool; MAX_DEVICES],
    conn_gen: [u32; MAX_DEVICES],
    /// Reconnections that failed in a row, and when the next may start ([`RETRY_FIRST`]).
    failures: [u8; MAX_DEVICES],
    retry_at: [Option<Instant>; MAX_DEVICES],
    scan_until: Option<Instant>,
    /// A device to pair ([`BtMsg::PairDevice`]), for the central task.
    to_pair: Option<[u8; 7]>,
    /// The slot a device is being paired into, and its peer: the keys its pairing gives the stack are kept for it.
    pairing: Option<(usize, Identity)>,
    /// Devices seen while looking, so that an advertisement and its scan response make one report.
    seen: heapless::Vec<Seen, 16>,
}

struct Seen {
    addr: [u8; 7],
    name: heapless::String<NAME_MAX>,
    appearance: u16,
    hid: bool,
    discoverable: bool,
    reported: Option<Instant>,
}

static STATE: Mutex<CriticalSectionRawMutex, RefCell<Option<State>>> = Mutex::new(RefCell::new(None));

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.lock(|s| f(s.borrow_mut().as_mut().expect("device state")))
}

/// Something changed for the central task: looking started or stopped, a device to pair, a device left.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Advertising reports, from the stack's runner ([`Handler`]).
static ADV: Channel<CriticalSectionRawMutex, Adv, 16> = Channel::new();
/// Output reports for each slot's connection: (report ID, data, length).
/// An output report on its way: its ID, its bytes, and how many of them count.
type Queued = (u8, [u8; 8], u8);

static OUTPUT: [Channel<CriticalSectionRawMutex, Queued, 4>; MAX_DEVICES] = [const { Channel::new() }; MAX_DEVICES];
/// Each slot's connection task: let the device go (forgotten).
static DROP: [Signal<CriticalSectionRawMutex, ()>; MAX_DEVICES] = [const { Signal::new() }; MAX_DEVICES];

#[derive(Clone, Copy)]
struct Adv {
    addr: Address,
    rssi: i8,
    connectable: bool,
    scan_response: bool,
    data: [u8; 31],
    len: u8,
}

/// Takes the stack's advertising reports to the central task.
pub struct Handler;

impl EventHandler for Handler {
    fn on_adv_reports(&self, reports: LeAdvReportsIter) {
        for r in reports.flatten() {
            let mut data = [0u8; 31];
            let n = r.data.len().min(31);
            data[..n].copy_from_slice(&r.data[..n]);
            // ADV_IND 0, ADV_DIRECT_IND 1, ADV_SCAN_IND 2, ADV_NONCONN_IND 3, SCAN_RSP 4.
            let kind = r.event_kind as u8;
            let adv = Adv {
                addr: Address::new(r.addr_kind, r.addr),
                rssi: r.rssi,
                connectable: kind <= 1,
                scan_response: kind == 4,
                data,
                len: n as u8,
            };
            let _ = ADV.try_send(adv);
        }
    }
}

/// The device slots' bonds, for the stack's keys ([`crate::ble`]'s `tidy_keys`): their long-term keys, and the peer of
/// a pairing under way.
pub fn kept_keys() -> ([Option<u128>; MAX_DEVICES], Option<Identity>) {
    with(|s| (core::array::from_fn(|i| s.bonds.slots[i].as_ref().map(|d| d.ltk)), s.pairing.map(|(_, p)| p)))
}

fn event(kind: EventKind, dev: u8, value: u32) {
    link::send(&BtMsg::Event { kind, host: dev, value });
}

fn stage(p: DevicePairing, slot: Option<usize>) {
    event(EventKind::DeviceStage, slot.map_or(0, |s| s as u8 + 1), p as u32);
}

/// Sends slot `slot`'s state to the RP2354B.
fn report(slot: usize) {
    let mut name = heapless::String::<NAME_MAX>::new();
    let state = with(|s| {
        let mut st = s.devices[slot];
        match &s.bonds.slots[slot] {
            Some(d) => {
                st.flags |= device_flags::BONDED;
                st.addr = d.address;
                st.appearance = d.appearance;
                st.map_len = d.map.len() as u16;
                st.map_crc = crc32(&d.map);
                name = d.name.clone();
            }
            None => {
                st.flags &= !device_flags::BONDED;
                (st.map_len, st.map_crc) = (0, 0);
            }
        }
        st
    });
    link::send(&BtMsg::Device { state, name: &name });
}

/// Every slot's state (the RP2354B asked, or restarted).
pub fn report_all() {
    for slot in 0..MAX_DEVICES {
        report(slot);
    }
}

fn set_flags(slot: usize, f: impl FnOnce(&mut DeviceState)) {
    let changed = with(|s| {
        let before = s.devices[slot];
        f(&mut s.devices[slot]);
        before != s.devices[slot]
    });
    if changed {
        report(slot);
    }
}

/// Sets up the slots from the bonds in flash, and gives the stack their keys.
pub fn start(bonds: &'static mut DeviceBonds, stack: &St) {
    let mut devices = [DeviceState { battery: 0xFF, ..DeviceState::default() }; MAX_DEVICES];
    for (i, d) in devices.iter_mut().enumerate() {
        d.dev = i as u8 + 1;
        if let Some(b) = &bonds.slots[i] {
            let _ = stack.add_bond_information(b.bond());
        }
    }
    let bonded = bonds.slots.iter().flatten().count();
    STATE.lock(|s| {
        *s.borrow_mut() = Some(State {
            bonds,
            devices,
            live: [false; MAX_DEVICES],
            conn_gen: [0; MAX_DEVICES],
            failures: [0; MAX_DEVICES],
            retry_at: [None; MAX_DEVICES],
            scan_until: None,
            to_pair: None,
            pairing: None,
            seen: heapless::Vec::new(),
        })
    });
    log::line(format_args!("Bluetooth: {bonded} bonded devices"));
}

/// What the RP2354B asks about the devices.
pub fn handle(msg: &BtMsg, stack: &'static St) {
    match *msg {
        BtMsg::Scan { on, seconds } => {
            with(|s| {
                s.scan_until = on.then(|| Instant::now() + Duration::from_secs(seconds as u64));
                s.seen.clear();
            });
            event(if on { EventKind::ScanOpen } else { EventKind::ScanClosed }, 0, seconds as u32);
            WAKE.signal(());
        }
        BtMsg::PairDevice { addr } => {
            let was_looking = with(|s| {
                s.to_pair = Some(addr);
                s.scan_until.take().is_some()
            });
            if was_looking {
                event(EventKind::ScanClosed, 0, 0);
            }
            WAKE.signal(());
        }
        BtMsg::ForgetDevice { dev } => {
            let picked = |slot: usize| dev == 0xFF || dev as usize == slot + 1;
            // One flash save for all of them, none for slots already free.
            with(|s| s.bonds.clear(picked));
            // Out of the stack's keys too, or the device would get back in encrypted without pairing.
            crate::ble::tidy_keys(stack);
            for slot in (0..MAX_DEVICES).filter(|&i| picked(i)) {
                DROP[slot].signal(());
                set_flags(slot, |d| d.flags &= !(device_flags::BONDED | device_flags::READY));
                report(slot);
            }
            WAKE.signal(());
        }
        BtMsg::RequestMap { dev } => send_map(dev),
        BtMsg::DeviceOutput { dev, id, data } => {
            if let Some(slot) = (dev as usize).checked_sub(1).filter(|i| *i < MAX_DEVICES) {
                let mut d = [0u8; 8];
                let n = data.len().min(8);
                d[..n].copy_from_slice(&data[..n]);
                let _ = OUTPUT[slot].try_send((id, d, n as u8));
            }
        }
        _ => {}
    }
}

/// Every device slot empty, in both of their flash sectors (`BtMsg::EraseAll`: the module restarts next).
pub fn erase_all() {
    with(|s| s.bonds.erase_all());
}

/// Slot `dev`'s report map, in pieces.
fn send_map(dev: u8) {
    let Some(slot) = (dev as usize).checked_sub(1).filter(|i| *i < MAX_DEVICES) else { return };
    let map: heapless::Vec<u8, MAP_MAX> =
        with(|s| s.bonds.slots[slot].as_ref().map(|d| d.map.clone())).unwrap_or_default();
    let total = map.len() as u16;
    for (i, piece) in map.chunks(MAP_CHUNK).enumerate() {
        link::send(&BtMsg::DeviceMap { dev, offset: (i * MAP_CHUNK) as u16, total, data: piece });
    }
}

/// A slot no bond and no connection holds.
fn free_slot(s: &State) -> Option<usize> {
    (0..MAX_DEVICES).find(|&i| s.bonds.slots[i].is_none() && !s.live[i])
}

/// The bonded device, not connected, that advertises from `addr`.
fn bonded_at(s: &State, addr: &Address) -> Option<usize> {
    let now = Instant::now();
    (0..MAX_DEVICES).find(|&i| {
        !s.live[i]
            && s.retry_at[i].is_none_or(|t| now >= t)
            && s.bonds.slots[i].as_ref().is_some_and(|d| d.identity().match_address(addr))
    })
}

/// A device with a bond that isn't connected: the central task watches for it.
fn missing(s: &State) -> bool {
    (0..MAX_DEVICES).any(|i| s.bonds.slots[i].is_some() && !s.live[i])
}

fn looking(s: &State) -> bool {
    s.scan_until.is_some_and(|t| Instant::now() < t)
}

/// What an advertisement says: discoverable (LE Limited or General), a HID device (its service or appearance), its
/// name and appearance.
struct Ad<'a> {
    discoverable: Option<bool>,
    hid: bool,
    name: Option<&'a str>,
    appearance: Option<u16>,
}

fn parse_ad(data: &[u8]) -> Ad<'_> {
    let mut ad = Ad { discoverable: None, hid: false, name: None, appearance: None };
    let mut i = 0;
    while i + 1 < data.len() {
        let len = data[i] as usize;
        if len == 0 || i + 1 + len > data.len() {
            break;
        }
        let (ty, v) = (data[i + 1], &data[i + 2..i + 1 + len]);
        match ty {
            0x01 => ad.discoverable = v.first().map(|f| f & 0x03 != 0),
            0x02 | 0x03 => ad.hid |= v.as_chunks::<2>().0.iter().any(|u| u16::from_le_bytes(*u) == HID_SERVICE),
            0x08 | 0x09 => ad.name = core::str::from_utf8(v).ok().or(ad.name),
            0x19 if v.len() >= 2 => ad.appearance = Some(u16::from_le_bytes([v[0], v[1]])),
            _ => {}
        }
        i += 1 + len;
    }
    if ad.appearance.is_some_and(appearance::is_hid) {
        ad.hid = true;
    }
    ad
}

/// An advertisement seen while looking for devices: a HID device that may be paired goes to the RP2354B.
fn seen(adv: &Adv) {
    let ad = parse_ad(&adv.data[..adv.len as usize]);
    let addr = adv.addr.to_bytes();
    let now = Instant::now();
    let out = with(|s| {
        // A bonded device isn't one to pair.
        if s.bonds.slot_of(&Identity { addr: adv.addr, irk: None }).is_some() || bonded_at(s, &adv.addr).is_some() {
            return None;
        }
        let i = match s.seen.iter().position(|e| e.addr == addr) {
            Some(i) => i,
            None => {
                let e = Seen {
                    addr,
                    name: heapless::String::new(),
                    appearance: 0,
                    hid: false,
                    discoverable: false,
                    reported: None,
                };
                if s.seen.push(e).is_err() {
                    // Full: the oldest makes way.
                    s.seen.remove(0);
                    let _ = s.seen.push(Seen {
                        addr,
                        name: heapless::String::new(),
                        appearance: 0,
                        hid: false,
                        discoverable: false,
                        reported: None,
                    });
                }
                s.seen.len() - 1
            }
        };
        let e = &mut s.seen[i];
        if let Some(n) = ad.name {
            e.name = kvm_proto::ctrl::bt_device_name(n);
        }
        if let Some(a) = ad.appearance {
            e.appearance = a;
        }
        e.hid |= ad.hid;
        if !adv.scan_response {
            e.discoverable = ad.discoverable.unwrap_or(false) && adv.connectable;
        }
        let due = e.reported.is_none_or(|t| now - t >= FOUND_EVERY);
        if e.hid && e.discoverable && due {
            e.reported = Some(now);
            Some((e.appearance, e.name.clone()))
        } else {
            None
        }
    });
    if let Some((appearance, name)) = out {
        link::send(&BtMsg::Found { addr, rssi: adv.rssi, appearance, name: &name });
    }
}

/// The address in `b`, as [`Address::to_bytes`] gives it.
fn address_of(b: &[u8; 7]) -> Address {
    crate::bonds::identity_of(b, None).addr
}

fn connect_params() -> RequestedConnParams {
    RequestedConnParams {
        min_connection_interval: Duration::from_micros(7_500),
        max_connection_interval: Duration::from_millis(15),
        max_latency: 0,
        supervision_timeout: Duration::from_secs(4),
        ..Default::default()
    }
}

/// Looks for devices while asked to, watches for the bonded ones that aren't connected, and connects: to a device
/// picked for pairing, or to a bonded one that advertises.
#[embassy_executor::task]
pub async fn central(spawner: Spawner, stack: &'static St) {
    let mut central = stack.central();
    loop {
        if let Some(addr) = with(|s| s.to_pair.take()) {
            pair(spawner, &mut central, stack, addr).await;
            continue;
        }
        let expired = with(|s| {
            let gone = s.scan_until.is_some_and(|t| Instant::now() >= t);
            if gone {
                s.scan_until = None;
            }
            gone
        });
        if expired {
            event(EventKind::ScanClosed, 0, 0);
        }
        let (scan, watch, until) = with(|s| (looking(s), missing(s), s.scan_until));
        if !scan && !watch {
            WAKE.wait().await;
            continue;
        }
        // Looking: active (for the scan responses with the names), two thirds of the time. Watching only: passive, a
        // quarter of the time, about 20 mA at 3.3 V while a bonded device stays off: a device that wakes advertises
        // for seconds, and is caught within 160 ms.
        let config = ScanConfig {
            active: scan,
            interval: Duration::from_millis(if scan { 60 } else { 160 }),
            window: Duration::from_millis(40),
            ..Default::default()
        };
        let found = {
            let mut scanner = Scanner::new(&mut central);
            let Ok(session) = scanner.scan(&config).await else {
                Timer::after_millis(200).await;
                continue;
            };
            while ADV.try_receive().is_ok() {}
            let deadline = until.unwrap_or(Instant::now() + Duration::from_secs(3600));
            let found = loop {
                match select3(ADV.receive(), WAKE.wait(), Timer::at(deadline)).await {
                    Either3::First(adv) => {
                        if adv.connectable
                            && let Some(slot) = with(|s| bonded_at(s, &adv.addr))
                        {
                            break Some((slot, adv.addr));
                        }
                        if with(|s| looking(s)) {
                            seen(&adv);
                        }
                    }
                    _ => break None,
                }
            };
            session.stop().await;
            found
        };
        if let Some((slot, addr)) = found {
            reconnect(spawner, &mut central, stack, slot, addr).await;
        }
    }
}

type Central = trouble_host::prelude::Central<'static, Controller, DefaultPacketPool>;
type Conn = Connection<'static, DefaultPacketPool>;

/// Connects to bonded device `slot`, which advertises from `addr`.
async fn reconnect(spawner: Spawner, central: &mut Central, stack: &'static St, slot: usize, addr: Address) {
    let list = [addr];
    let config = ConnectConfig {
        scan_config: ScanConfig { filter_accept_list: &list, ..Default::default() },
        connect_params: connect_params(),
    };
    let conn = match with_timeout(Duration::from_secs(3), central.connect(&config)).await {
        Ok(Ok(c)) => c,
        _ => return,
    };
    let mine = with(|s| {
        s.live[slot] = true;
        s.conn_gen[slot] = s.conn_gen[slot].wrapping_add(1);
        s.conn_gen[slot]
    });
    run(spawner, slot, false, mine, conn, stack);
}

/// Connects to the device at `addr` and pairs it into a free slot.
async fn pair(spawner: Spawner, central: &mut Central, stack: &'static St, addr: [u8; 7]) {
    let Some(slot) = with(|s| {
        let slot = free_slot(s)?;
        s.live[slot] = true;
        Some(slot)
    }) else {
        log::line(format_args!("devices: no slot free to pair into"));
        stage(DevicePairing::Failed, None);
        return;
    };
    stage(DevicePairing::Connecting, Some(slot));
    let list = [address_of(&addr)];
    let config = ConnectConfig {
        scan_config: ScanConfig { filter_accept_list: &list, ..Default::default() },
        connect_params: connect_params(),
    };
    let conn = match with_timeout(CONNECT_TIMEOUT, central.connect(&config)).await {
        Ok(Ok(c)) => c,
        _ => {
            log::line(format_args!("device {}: not reached", slot + 1));
            with(|s| s.live[slot] = false);
            stage(DevicePairing::Failed, Some(slot));
            return;
        }
    };
    let mine = with(|s| {
        s.conn_gen[slot] = s.conn_gen[slot].wrapping_add(1);
        s.pairing = Some((slot, conn.peer_identity()));
        s.conn_gen[slot]
    });
    run(spawner, slot, true, mine, conn, stack);
}

fn run(spawner: Spawner, slot: usize, new: bool, mine: u32, conn: Conn, stack: &'static St) {
    match connection(slot, new, mine, conn, stack) {
        Ok(t) => spawner.spawn(t),
        Err(_) => {
            log::line(format_args!("no task left for a device"));
            with(|s| {
                if s.conn_gen[slot] == mine {
                    s.live[slot] = false;
                    s.pairing = None;
                }
            });
        }
    }
}

/// The stack's IO capabilities and pairing methods for pairing a device, or (`false`) the hosts' again.
fn pairing_devices(stack: &St, on: bool) {
    if on {
        stack.set_io_capabilities(IoCapabilities::DisplayOnly);
        stack.set_secure_connections_only(false);
    } else {
        stack.set_io_capabilities(IoCapabilities::KeyboardOnly);
        stack.set_secure_connections_only(true);
    }
}

/// Waits until the link is encrypted: with the bond's key (`new` false), or after pairing a new device, whose bond it
/// returns. None: it failed, or the device left.
async fn secure(slot: usize, conn: &Conn, stack: &'static St) -> Option<BondInformation> {
    loop {
        let ev = with_timeout(SECURE_TIMEOUT, conn.next()).await.ok()?;
        match ev {
            ConnectionEvent::Disconnected { reason } => {
                log::line(format_args!("device {}: disconnected while securing ({reason:?})", slot + 1));
                return None;
            }
            ConnectionEvent::PassKeyDisplay(key) => {
                // Typed on the keyboard being paired, then Enter: the window shows it.
                event(EventKind::DevicePasskey, slot as u8 + 1, key.value());
            }
            // A device that would show digits to compare or have them typed here: the module shows nothing to compare
            // and has no keyboard of its own.
            ConnectionEvent::PassKeyConfirm(_) | ConnectionEvent::PassKeyInput => {
                let _ = conn.pass_key_cancel();
            }
            ConnectionEvent::PairingComplete { bond, .. } => return bond,
            ConnectionEvent::PairingFailed(_) => {
                log::line(format_args!("device {}: pairing failed", slot + 1));
                return None;
            }
            ConnectionEvent::RequestConnectionParams(req) => {
                let _ = req.accept(None, stack).await;
            }
            _ => {}
        }
    }
}

/// A bonded device reconnected, and its GATT client was made: `GattClient::new` encrypts a bonded peer's link with its
/// key the stack's own way, which never falls back to pairing anew. Waits until the link is encrypted, as the stack's
/// state says (its `Encrypted` event doesn't always come), until `ENCRYPT_GIVE_UP` after the device connected (`from`).
/// When the key doesn't work (`BondLost`) or the stack wants a passkey, the module lets the connection go without
/// telling the device that a pairing failed. Until 0.1.49 the module asked for security itself (`request_security`),
/// and when the key failed once, for whatever reason, the stack then sent the device a pairing request: a keyboard
/// took it, changed its key, and from then on every reconnection ended in a MIC failure (2026-11-20). False: not
/// encrypted, or the device left.
async fn encrypted_with_key(slot: usize, conn: &Conn, stack: &'static St, from: Instant) -> bool {
    let dev = slot + 1;
    loop {
        if conn.security_level().is_ok_and(|l| l.encrypted()) {
            let ms = from.elapsed().as_millis();
            log::line(format_args!("device {dev}: encrypted after {ms} ms"));
            return true;
        }
        if from.elapsed() >= ENCRYPT_GIVE_UP {
            log::line(format_args!("device {dev}: not encrypted with its key, letting it go"));
            return false;
        }
        if let Either::First(ev) = select(conn.next(), Timer::after_millis(50)).await {
            match ev {
                ConnectionEvent::Disconnected { reason } => {
                    log::line(format_args!("device {dev}: disconnected while securing ({reason:?})"));
                    return false;
                }
                ConnectionEvent::BondLost => {
                    log::line(format_args!("device {dev}: its key didn't work this time, letting it go"));
                    return false;
                }
                // A passkey to show, compare or enter: the stack is pairing anew, which a reconnection never does.
                ConnectionEvent::PassKeyDisplay(_)
                | ConnectionEvent::PassKeyConfirm(_)
                | ConnectionEvent::PassKeyInput => {
                    log::line(format_args!("device {dev}: the stack began pairing anew, letting it go"));
                    return false;
                }
                // Also what the stack says when the link is encrypted with the bond's key (its pairing state machine
                // ends in success either way): the encryption state, checked above, decides. From 0.1.34 to 0.1.37
                // the module let every reconnection go the moment it was encrypted (2026-11-20).
                ConnectionEvent::PairingComplete { .. } => {}
                ConnectionEvent::PairingFailed(e) => {
                    log::line(format_args!("device {dev}: security failed ({e:?})"));
                    return false;
                }
                ConnectionEvent::RequestConnectionParams(req) => {
                    let _ = req.accept(None, stack).await;
                }
                _ => {}
            }
        }
    }
}

type Client = GattClient<'static, Controller, DefaultPacketPool, 16>;

/// Tries at making a connection's GATT client, `CLIENT_RETRY` apart, before the connection goes.
const CLIENT_TRIES: u32 = 10;
const CLIENT_RETRY: Duration = Duration::from_millis(100);

/// The connection's GATT client (for a bonded device, making it also encrypts the link with the bond's key). A try
/// that fails while the device is still connected is tried again: a keyboard waking while a mouse moved lost its
/// client about 110 ms after it connected, again and again for 20 s, until the mouse was switched off (2026-11-20).
/// The first failure and the last go to the log, with the stack's packet pool as it is then.
async fn gatt_client(slot: usize, conn: &Conn, stack: &'static St) -> Option<Client> {
    let dev = slot + 1;
    for attempt in 1..=CLIENT_TRIES {
        let e = match with_timeout(SETUP_TIMEOUT, Client::new(stack, conn)).await {
            Ok(Ok(client)) => {
                if attempt > 1 {
                    log::line(format_args!("device {dev}: GATT client at try {attempt}"));
                }
                return Some(client);
            }
            Ok(Err(e)) => e,
            Err(_) => {
                log::line(format_args!("device {dev}: no GATT client in time, letting it go"));
                return None;
            }
        };
        let last = attempt == CLIENT_TRIES || !conn.is_connected();
        if attempt == 1 || last {
            let (free, all) = free_packets();
            log::line(format_args!("device {dev}: no GATT client ({e:?}; {free} of {all} packets free)"));
        }
        if last {
            log::line(format_args!("device {dev}: letting it go"));
            return None;
        }
        Timer::after(CLIENT_RETRY).await;
    }
    None
}

/// Free packets in the stack's pool, and how many it has: taken until none is left, and given back at once.
fn free_packets() -> (usize, usize) {
    let mut held: heapless::Vec<<DefaultPacketPool as PacketPool>::Packet, 64> = heapless::Vec::new();
    while let Some(p) = DefaultPacketPool::allocate() {
        if held.push(p).is_err() {
            break;
        }
    }
    (held.len(), DefaultPacketPool::capacity())
}

/// Reads a new device's HID service: its report map, its reports and their IDs, and its battery and name.
async fn discover(client: &Client, bond: &BondInformation, addr: &[u8; 7]) -> Option<Device> {
    let mut d = Device::from_bond(bond);
    // The name and appearance it advertised with.
    let (name, appearance) =
        with(|s| s.seen.iter().find(|e| e.addr == *addr).map(|e| (e.name.clone(), e.appearance))).unwrap_or_default();
    (d.name, d.appearance) = (name, appearance);
    let services = client.services().await.ok()?;
    let short = Uuid::new_short;
    let hid = services.iter().find(|s| s.uuid() == short(HID_SERVICE))?;
    let chars = client.characteristics::<24>(hid).await.ok()?;
    for c in &chars {
        if c.uuid == short(REPORT_MAP) {
            let mut buf = [0u8; MAP_MAX];
            let mut n = client.read_handle(c.handle, &mut buf).await.ok()?;
            // The stack reads on with Read Blob only after a read as long as the MTU allows; a device that answers
            // shorter would leave the map cut. One more read from where it ended: an error or nothing means the end.
            if n > 0
                && n < MAP_MAX
                && let Ok(more) = client.read_handle_blob(c.handle, n as u16, &mut buf[n..]).await
            {
                n += more;
            }
            d.map = heapless::Vec::from_slice(&buf[..n]).ok()?;
        } else if c.uuid == short(REPORT) {
            // Its Report Reference says its ID and type; without one it is input report 0.
            let reference = client
                .find_information(c.handle + 1, c.end_handle, |h, u| {
                    if u == short(REPORT_REFERENCE) { ControlFlow::Break(h) } else { ControlFlow::Continue(()) }
                })
                .await
                .ok()
                .flatten();
            let (id, kind) = match reference {
                Some(h) => {
                    let mut b = [0u8; 2];
                    match client.read_handle(h, &mut b).await {
                        Ok(2) => (b[0], b[1]),
                        _ => (0, Report::INPUT),
                    }
                }
                None => (0, Report::INPUT),
            };
            let wwr = c.props.any(&[CharacteristicProp::WriteWithoutResponse]);
            let report =
                Report { handle: c.handle, cccd: c.cccd_handle.unwrap_or(0), id, kind, write_without_response: wwr };
            let _ = d.reports.push(report);
        } else if c.uuid == short(PROTOCOL_MODE) {
            // Report Protocol: the reports the map describes, not the boot ones.
            let _ = client.write_handle_without_response(c.handle, &[1]).await;
        }
    }
    if d.map.is_empty() || !d.reports.iter().any(|r| r.kind == Report::INPUT) {
        return None;
    }
    if let Some(bas) = services.iter().find(|s| s.uuid() == short(BATTERY_SERVICE))
        && let Ok(cs) = client.characteristics::<4>(bas).await
        && let Some(c) = cs.iter().find(|c| c.uuid == short(BATTERY_LEVEL))
    {
        d.battery = (c.handle, c.cccd_handle.unwrap_or(0));
    }
    if d.name.is_empty()
        && let Some(gap) = services.iter().find(|s| s.uuid() == short(GAP_SERVICE))
        && let Ok(cs) = client.characteristics::<8>(gap).await
        && let Some(c) = cs.iter().find(|c| c.uuid == short(DEVICE_NAME))
    {
        let mut b = [0u8; NAME_MAX];
        if let Ok(n) = client.read_handle(c.handle, &mut b).await {
            d.name = kvm_proto::ctrl::bt_device_name(core::str::from_utf8(&b[..n]).unwrap_or(""));
        }
    }
    Some(d)
}

#[embassy_executor::task(pool_size = MAX_DEVICES)]
async fn connection(slot: usize, new: bool, mine: u32, conn: Conn, stack: &'static St) {
    let dev = slot as u8 + 1;
    log::line(format_args!("device {dev}: connected{}", if new { " (pairing)" } else { "" }));
    let current = || with(|s| s.conn_gen[slot] == mine);
    let addr = conn.peer_address().to_bytes();
    set_flags(slot, |d| {
        d.flags |= device_flags::CONNECTED;
        d.flags &= !(device_flags::ENCRYPTED | device_flags::READY);
    });
    while OUTPUT[slot].try_receive().is_ok() {}
    DROP[slot].reset();
    if new {
        stage(DevicePairing::Pairing, Some(slot));
        pairing_devices(stack, true);
    }
    let _ = conn.set_bondable(new);
    let from = Instant::now();
    // Pairing: asked for at once. A bonded device's link is encrypted by `Client::new` (`encrypted_with_key`).
    let bond = if new {
        let _ = conn.request_security();
        let secured = select(secure(slot, &conn, stack), DROP[slot].wait()).await;
        pairing_devices(stack, false);
        match secured {
            Either::First(Some(b)) => Some(b),
            _ => {
                end(slot, new, mine, &conn, stack, current());
                return;
            }
        }
    } else {
        None
    };
    let Some(client) = gatt_client(slot, &conn, stack).await else {
        end(slot, new, mine, &conn, stack, current());
        return;
    };
    if !new
        && !matches!(select(encrypted_with_key(slot, &conn, stack, from), DROP[slot].wait()).await, Either::First(true))
    {
        end(slot, new, mine, &conn, stack, current());
        return;
    }
    set_flags(slot, |d| d.flags |= device_flags::ENCRYPTED);
    let work = async {
        if let Some(b) = &bond {
            stage(DevicePairing::Reading, Some(slot));
            match discover(&client, b, &addr).await {
                Some(d) => {
                    log::line(format_args!(
                        "device {dev}: paired, {} reports, map {} bytes",
                        d.reports.len(),
                        d.map.len()
                    ));
                    // Written once, with everything read; the same device in another slot goes.
                    let twins = with(|s| {
                        s.pairing = None;
                        s.bonds.set_new(slot, d)
                    });
                    crate::ble::tidy_keys(stack);
                    report(slot);
                    for i in (0..MAX_DEVICES).filter(|i| twins & (1 << i) != 0) {
                        log::line(format_args!(
                            "device {}: the same device as device {dev}, its old pairing goes",
                            i + 1
                        ));
                        DROP[i].signal(());
                        report(i);
                    }
                    stage(DevicePairing::Paired, Some(slot));
                }
                None => {
                    log::line(format_args!("device {dev}: no HID service to use"));
                    stage(DevicePairing::Failed, Some(slot));
                    return;
                }
            }
        }
        let Some(d) = with(|s| s.bonds.slots[slot].clone()) else { return };
        // The reports on, and the battery level (a bonded device usually keeps them on; writing again costs nothing).
        let setup = async {
            for r in d.reports.iter().filter(|r| r.kind == Report::INPUT && r.cccd != 0) {
                let _ = client.write_handle(r.cccd, &[1, 0]).await;
            }
            if d.battery.0 != 0 {
                let mut b = [0u8; 1];
                if client.read_handle(d.battery.0, &mut b).await.is_ok() {
                    set_flags(slot, |s| s.battery = b[0].min(100));
                }
                if d.battery.1 != 0 {
                    let _ = client.write_handle(d.battery.1, &[1, 0]).await;
                }
            }
        };
        if with_timeout(SETUP_TIMEOUT, setup).await.is_err() {
            log::line(format_args!("device {dev}: its reports didn't answer, letting it go"));
            return;
        }
        let Ok(mut listener) = client.listen_all() else { return };
        set_flags(slot, |s| s.flags |= device_flags::READY);
        loop {
            match select3(listener.next(), OUTPUT[slot].receive(), DROP[slot].wait()).await {
                Either3::First(n) => {
                    let h = n.handle();
                    if let Some(r) = d.reports.iter().find(|r| r.handle == h && r.kind == Report::INPUT) {
                        link::send(&BtMsg::DeviceInput { dev, id: r.id, data: n.as_ref() });
                    } else if h == d.battery.0 && !n.as_ref().is_empty() {
                        let level = n.as_ref()[0].min(100);
                        set_flags(slot, |s| s.battery = level);
                    }
                }
                Either3::Second((id, data, len)) => {
                    let data = &data[..len as usize];
                    if let Some(r) = d.reports.iter().find(|r| r.kind == Report::OUTPUT && r.id == id) {
                        let _ = if r.write_without_response {
                            client.write_handle_without_response(r.handle, data).await
                        } else {
                            client.write_handle(r.handle, data).await
                        };
                    }
                }
                Either3::Third(()) => return,
            }
        }
    };
    let events = async {
        loop {
            match conn.next().await {
                ConnectionEvent::Disconnected { reason } => {
                    log::line(format_args!("device {dev}: disconnected ({reason:?})"));
                    return;
                }
                ConnectionEvent::RequestConnectionParams(req) => {
                    let _ = req.accept(None, stack).await;
                }
                _ => {}
            }
        }
    };
    let _ = select3(client.task(), work, events).await;
    end(slot, new, mine, &conn, stack, current());
}

/// A device's connection ended (or its pairing did): the slot lets go of it, a pairing that gave no bond goes.
fn end(slot: usize, new: bool, mine: u32, conn: &Conn, stack: &'static St, current: bool) {
    conn.disconnect();
    if !current {
        return;
    }
    let unpaired = with(|s| {
        s.live[slot] = false;
        if !new {
            // A connection that never got ready holds off the next one; one that did starts the count again.
            if s.devices[slot].flags & device_flags::READY != 0 {
                s.failures[slot] = 0;
                s.retry_at[slot] = None;
            } else {
                s.failures[slot] = s.failures[slot].saturating_add(1);
                let wait = RETRY_FIRST * (1u32 << (s.failures[slot] - 1).min(3));
                s.retry_at[slot] = Some(Instant::now() + wait);
            }
        }
        let unpaired = new && s.bonds.slots[slot].is_none();
        if s.pairing.is_some_and(|(p, _)| p == slot) {
            s.pairing = None;
        }
        let _ = mine;
        unpaired
    });
    if unpaired {
        stage(DevicePairing::Failed, Some(slot));
    }
    set_flags(slot, |d| d.flags &= !(device_flags::CONNECTED | device_flags::ENCRYPTED | device_flags::READY));
    // A pairing that didn't end in a bond leaves its keys in the stack.
    crate::ble::tidy_keys(stack);
    WAKE.signal(());
}
