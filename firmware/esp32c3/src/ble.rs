//! The module as a Bluetooth LE keyboard and mouse (HID over GATT) towards up to four bonded hosts, all connected at
//! once (software.md §4.13).
//!
//! - **Slots:** each bonded host has a slot 1–4 (`bonds`). The RP2354B selects one ([`BtMsg::Select`]); only that
//!   one gets the input reports it sends. A host whose policy is `ConnectWhenSelected` is let go while not selected
//!   and reconnects when it is.
//! - **Advertising:** connectable, and discoverable only while pairing is open; otherwise only the bonded hosts that
//!   should be connected and aren't are let in (an unknown host that connects is dropped).
//! - **Pairing:** LE Secure Connections, IO capability KeyboardOnly: the host shows six digits, the user types them on
//!   the KVM's keyboard, the RP2354B passes them here ([`BtMsg::Passkey`]). No Just Works, so a host in range can't
//!   pair as the KVM's keyboard target unseen. A new host pairs into a slot that no bond and no connection holds; a
//!   bonded host that forgot the KVM pairs anew into its own slot, and both only while pairing is open.
//! - **Connection interval:** the selected host is asked for 7.5–15 ms, the others for 30–50 ms with latency 4, so
//!   their connection events take little of the radio's time (they may refuse; some hosts only accept multiples of 15 ms).
//! - **TX power:** +9 dBm at most: 190 mA peaks (datasheet table 6-5) instead of 340 mA at +20 dBm, for the board's
//!   power budget (design.md §6.1); a desk is a few metres.
//! - **Pinned hosts:** a host devices are pinned to ([`BtMsg::Select`]'s `pinned`) takes input as the selected one
//!   does, and stays connected and asked for the short interval whatever its policy.
//! - **Devices:** the same stack is the central of the Bluetooth devices ([`crate::devices`]).

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_futures::select::{Either, Either3, Either4, select, select3, select4};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_radio::ble::controller::BleConnector;
use kvm_proto::link::bt::{BtMsg, EventKind, HostState, MAX_HOSTS, Motion, Policy, host_flags, report};
use static_cell::StaticCell;
use trouble_host::prelude::*;

use crate::bonds::{Bonds, DeviceBonds, Slot, notify};
use crate::{devices, link, log};

/// Hosts at once: the four slots, and one more while a new host pairs into a free slot.
const HOST_CONNS: usize = MAX_HOSTS + 1;
/// Connections in all: the hosts', and one per device slot (a device pairs only into a free slot).
const CONNS: usize = HOST_CONNS + kvm_proto::link::bt::MAX_DEVICES;
const L2CAP: usize = CONNS * 2;
/// Keys the Bluetooth stack can hold: the host and device slots', and one pairing's under way per connection
/// ([`tidy_keys`]).
const KEYS: usize = MAX_HOSTS + kvm_proto::link::bt::MAX_DEVICES + CONNS;
/// The controller's activities at once (connections, advertising, scanning): its most, 10. Five host connections,
/// four devices, advertising and a scan would be 11, which takes every slot of both kinds filled and a host pairing
/// and a device sought at once: the one that doesn't get its activity tries again later.
const ACTIVITIES: u8 = 10;
const NAME: &str = "OpenDynKVM";

const MAP_LEN: usize = report::MAP.len();
const MAP: [u8; MAP_LEN] = {
    let mut a = [0u8; MAP_LEN];
    let mut i = 0;
    while i < MAP_LEN {
        a[i] = report::MAP[i];
        i += 1;
    }
    a
};
/// HID Information: bcdHID 1.11, no country, normally connectable.
const HID_INFO: [u8; 4] = [0x11, 0x01, 0x00, 0x02];
/// PnP ID: vendor ID source 2 (USB), the KVM's USB VID:PID (pid.codes test PID until one is allocated), version 1.0.
const PNP_ID: [u8; 7] = [0x02, 0x09, 0x12, 0x01, 0x00, 0x00, 0x01];
const MAKER: [u8; 10] = *b"OpenDynKVM";

#[gatt_server(connections_max = HOST_CONNS, attribute_table_size = 64)]
#[allow(dead_code)] // the Device Information and Battery services are only read by the hosts
struct Server {
    hid: HidService,
    dis: DeviceInformation,
    bas: BatteryService,
}

#[gatt_service(uuid = service::HUMAN_INTERFACE_DEVICE)]
struct HidService {
    #[characteristic(uuid = characteristic::HID_INFORMATION, read, value = HID_INFO)]
    info: [u8; 4],
    #[characteristic(uuid = characteristic::REPORT_MAP, read, value = MAP)]
    report_map: [u8; MAP_LEN],
    #[characteristic(uuid = characteristic::HID_CONTROL_POINT, write_without_response, value = 0)]
    control_point: u8,
    #[characteristic(uuid = characteristic::PROTOCOL_MODE, read, write_without_response, value = 1)]
    protocol_mode: u8,
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [report::KEYBOARD, 1])]
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0; report::KEYBOARD_LEN])]
    keyboard: [u8; report::KEYBOARD_LEN],
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [report::KEYBOARD, 2])]
    #[characteristic(uuid = characteristic::REPORT, read, write, write_without_response, value = [0; report::LEDS_LEN])]
    leds: [u8; report::LEDS_LEN],
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [report::MOUSE, 1])]
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0; report::MOUSE_LEN])]
    mouse: [u8; report::MOUSE_LEN],
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [report::CONSUMER, 1])]
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0; report::CONSUMER_LEN])]
    consumer: [u8; report::CONSUMER_LEN],
    /// Screen hop's absolute pointer (report 4), for a host set to take it.
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [report::POINTER, 1])]
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0; report::POINTER_LEN])]
    pointer: [u8; report::POINTER_LEN],
}

#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DeviceInformation {
    #[characteristic(uuid = characteristic::MANUFACTURER_NAME_STRING, read, value = MAKER)]
    maker: [u8; 10],
    #[characteristic(uuid = characteristic::PNP_ID, read, value = PNP_ID)]
    pnp: [u8; 7],
}

#[gatt_service(uuid = service::BATTERY)]
struct BatteryService {
    #[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 100)]
    level: u8,
}

/// What the Bluetooth side knows, shared by its tasks (one executor: the mutex only guards against interrupts).
struct State {
    bonds: &'static mut Bonds,
    hosts: [HostState; MAX_HOSTS],
    /// The slot that gets input (1–4), 0 = none.
    selected: u8,
    /// Bit n−1: devices are pinned to slot n (it takes input too, and stays connected).
    pinned: u8,
    policies: [Policy; MAX_HOSTS],
    pairing_until: Option<Instant>,
    /// Each slot's current connection, numbered when it is let in. A bonded host that connects anew before the module
    /// has seen its old connection end gets a task of its own, and the old task's end must not take the slot's state
    /// with it: it cleared the new connection's flags, and the slot showed "not connected" with the host connected
    /// (first dongle, 2026-11-14).
    conn_gen: [u32; MAX_HOSTS],
    /// A connection holds the slot (from being let in to its task's end): a new host pairs only into a slot that no
    /// bond and no connection holds. Before, a second host let in while one paired got the same free slot and took it
    /// over, and the pairing broke off after its passkey (first dongle, 2026-11-14).
    live: [bool; MAX_HOSTS],
    /// The peer of a slot's connection while it may pair (a new host, or a bonded one that forgot the KVM), until it
    /// bonds or leaves: the keys its pairing gives the stack are kept for it ([`tidy_keys`]).
    pairing_peer: [Option<Identity>; MAX_HOSTS],
    /// A slot's pairing is under way (its passkey was asked for) and hasn't ended: pairing closing doesn't cut it off
    /// ([`drop_unpaired`]); trouble-host ends one that stalls (SMP's 30 s).
    pairing_now: [bool; MAX_HOSTS],
    /// Notification settings written to flash in this run ([`NOTIFY_SAVES`]).
    notify_saves: u8,
    /// Bit n: slot n's notification settings are held in RAM, the run's [`NOTIFY_SAVES`] spent.
    settings_held: u8,
}

/// A slot a new host may pair into: no bond, no connection.
fn free_slot(s: &State) -> Option<usize> {
    (0..MAX_HOSTS).find(|&i| s.bonds.slots[i].is_none() && !s.live[i])
}

fn pairing_open(s: &State) -> bool {
    s.pairing_until.is_some_and(|t| Instant::now() < t)
}

static STATE: Mutex<CriticalSectionRawMutex, RefCell<Option<State>>> = Mutex::new(RefCell::new(None));

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.lock(|s| f(s.borrow_mut().as_mut().expect("Bluetooth state")))
}

/// Input reports for each slot's connection task: (report id, data, length).
/// A report on its way: its ID, its bytes, and how many of them count.
type Queued = (u8, [u8; 8], u8);

static REPORTS: [Channel<CriticalSectionRawMutex, Queued, 16>; MAX_HOSTS] = [const { Channel::new() }; MAX_HOSTS];
/// Each slot's motion, coalesced until the host takes it at a connection event ([`Motion`]): a mouse reports far more
/// often than a host takes notifications, and a queue of them was late and then dropped motion (first dongle,
/// 2026-11-14: the pointer crawled).
static MOTION: [Mutex<CriticalSectionRawMutex, RefCell<Motion>>; MAX_HOSTS] =
    [const { Mutex::new(RefCell::new(Motion::new())) }; MAX_HOSTS];
static MOTION_READY: [Signal<CriticalSectionRawMutex, ()>; MAX_HOSTS] = [const { Signal::new() }; MAX_HOSTS];

fn take_motion(slot: usize) -> Option<(u8, [u8; 8], u8)> {
    MOTION[slot].lock(|m| m.borrow_mut().take()).map(|(_, id, d)| (id, d, 8))
}
/// Each slot's connection task: let the host go (forgotten, or not selected under `ConnectWhenSelected`), or ask it
/// for other connection parameters (selection changed).
static CTL: [Signal<CriticalSectionRawMutex, Ctl>; MAX_HOSTS] = [const { Signal::new() }; MAX_HOSTS];
/// The passkey typed for each slot's pairing (None: cancel). Until 0.1.19 one signal served all, and the digits went to
/// whichever connection waited, which needn't be the host that showed them.
static PASSKEY: [Signal<CriticalSectionRawMutex, Option<u32>>; MAX_HOSTS] = [const { Signal::new() }; MAX_HOSTS];
/// Something changed that may start or stop advertising.
static ADVERTISE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

#[derive(Clone, Copy)]
enum Ctl {
    Disconnect,
    Params,
}

fn report_host(slot: usize) {
    let h = with(|s| s.hosts[slot]);
    link::send(&BtMsg::Host(h));
}

fn event(kind: EventKind, host: u8, value: u32) {
    link::send(&BtMsg::Event { kind, host, value });
}

fn set_flags(slot: usize, f: impl FnOnce(&mut HostState)) {
    let changed = with(|s| {
        let before = s.hosts[slot];
        f(&mut s.hosts[slot]);
        before != s.hosts[slot]
    });
    if changed {
        report_host(slot);
    }
}

/// A host in use: the selected one, or one devices are pinned to.
fn in_use(s: &State, slot: usize) -> bool {
    s.selected as usize == slot + 1 || s.pinned & (1 << slot) != 0
}

/// The hosts the module should be connected to now: bonded, and either always connected or in use.
fn wanted(s: &State, slot: usize) -> bool {
    s.hosts[slot].bonded() && (s.policies[slot] == Policy::StayConnected || in_use(s, slot))
}

/// Pairing closed (ran out, closed by the RP2354B, or a host bonded): a connection without a bond that hasn't started
/// pairing came in to pair, and goes. Until 0.1.19 one stayed, holding its slot: a host that got in on keys the module
/// no longer kept, or one that connected and never paired. One whose passkey was asked for finishes: Windows takes long
/// enough over adding a device that the window may close in the middle.
fn drop_unpaired() {
    for (slot, ctl) in CTL.iter().enumerate() {
        if with(|s| s.live[slot] && !s.hosts[slot].bonded() && !s.pairing_now[slot]) {
            ctl.signal(Ctl::Disconnect);
        }
    }
}

pub type Controller = ExternalController<BleConnector<'static>, 20>;
pub type St = Stack<'static, Controller, DefaultPacketPool>;

/// Keeps the Bluetooth stack's keys to the slots' bonds and the pairings under way. The stack stores a pairing's keys
/// as soon as it encrypts, whether or not the module keeps the host, and never lets them go by itself: keys left from
/// a pairing that broke off, or from a host forgotten here that didn't match its identity (`bonds::Slot::identity`),
/// filled the four places it had, and from then on every pairing failed just after its passkey, on keys it had no room
/// to store (first dongle, 2026-11-15: "it accepts the code, connects for a moment and drops off").
pub fn tidy_keys(stack: &St) {
    let (ltks, peers) = with(|s| (s.bonds.slots.map(|b| b.map(|b| b.ltk)), s.pairing_peer));
    let (device_ltks, device_peer) = devices::kept_keys();
    let keep = |b: &BondInformation| {
        ltks.iter().chain(device_ltks.iter()).flatten().any(|k| *k == b.ltk.0)
            || peers
                .iter()
                .chain(core::iter::once(&device_peer))
                .flatten()
                .any(|p| p.match_identity(&b.identity) || b.identity.match_identity(p))
    };
    let (kept, stray) = stack.with_bond_information(|keys| {
        let kept: heapless::Vec<BondInformation, KEYS> = keys.iter().filter(|b| keep(b)).cloned().collect();
        let stray = keys.len() - kept.len();
        (kept, stray)
    });
    if stray == 0 {
        return;
    }
    // Removing by identity takes the first key that matches it, which may not be the one meant: all go, the kept ones
    // come back.
    while let Some(id) = stack.with_bond_information(|keys| keys.first().map(|b| b.identity)) {
        if stack.remove_bond_information(id).is_err() {
            break;
        }
    }
    for b in kept {
        let _ = stack.add_bond_information(b);
    }
    log::line(format_args!("Bluetooth: {stray} stray keys dropped"));
}

static RESOURCES: StaticCell<HostResources<DefaultPacketPool, CONNS, L2CAP, 1, KEYS>> = StaticCell::new();
static STACK: StaticCell<St> = StaticCell::new();
static SERVER: StaticCell<Server<'static>> = StaticCell::new();
static BONDS: StaticCell<Bonds> = StaticCell::new();

static DEVICE_BONDS: StaticCell<DeviceBonds> = StaticCell::new();

pub async fn run(
    spawner: Spawner,
    bt: esp_hal::peripherals::BT<'static>,
    bonds: Bonds,
    device_bonds: DeviceBonds,
) -> ! {
    let config = esp_radio::ble::Config::default()
        .with_max_connections(ACTIVITIES)
        .with_default_tx_power(esp_radio::ble::TxPower::P9);
    let connector = BleConnector::new(bt, config).expect("Bluetooth controller");
    let controller: Controller = ExternalController::new(connector);
    let resources = RESOURCES.init(HostResources::new());
    // A static random address from the chip's MAC, so the hosts' bonds stay valid across restarts and updates.
    let m = esp_hal::efuse::base_mac_address();
    let mac = m.as_bytes();
    let mut addr = [mac[5], mac[4], mac[3], mac[2], mac[1], mac[0]];
    addr[5] |= 0xC0;
    let stack: &'static St = STACK.init(
        trouble_host::new(controller, resources)
            .set_random_address(Address::random(addr))
            .set_io_capabilities(IoCapabilities::KeyboardOnly)
            // Hosts pair with LE Secure Connections only; legacy pairing is let in while a device pairs.
            .set_secure_connections_only(true)
            .build(),
    );
    let bonds = BONDS.init(bonds);
    let bonded = bonds.slots.iter().flatten().count();
    let mut hosts = [HostState::default(); MAX_HOSTS];
    for (i, h) in hosts.iter_mut().enumerate() {
        h.host = i as u8 + 1;
        if let Some(slot) = bonds.slots[i] {
            h.flags = host_flags::BONDED;
            h.addr = slot.address;
            let _ = stack.add_bond_information(slot.bond());
        }
    }
    STATE.lock(|s| {
        *s.borrow_mut() = Some(State {
            bonds,
            hosts,
            selected: 0,
            pinned: 0,
            policies: [Policy::StayConnected; MAX_HOSTS],
            pairing_until: None,
            conn_gen: [0; MAX_HOSTS],
            live: [false; MAX_HOSTS],
            pairing_peer: [None; MAX_HOSTS],
            pairing_now: [false; MAX_HOSTS],
            notify_saves: 0,
            settings_held: 0,
        })
    });
    let server = SERVER.init(
        Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
            name: NAME,
            appearance: &appearance::human_interface_device::KEYBOARD,
        }))
        .expect("GATT server"),
    );
    log::line(format_args!("Bluetooth up, {bonded} bonded hosts"));
    // Device Information and Battery: only the hosts read them.
    let _ = (&server.dis, &server.bas);
    for slot in 0..MAX_HOSTS {
        report_host(slot);
    }
    devices::start(DEVICE_BONDS.init(device_bonds), stack);
    spawner.spawn(control(stack).expect("control task"));
    spawner.spawn(devices::central(spawner, stack).expect("central task"));
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let _ =
        select(runner.run_with_handler(&devices::Handler), advertise(spawner, &mut peripheral, stack, server)).await;
    panic!("Bluetooth host stopped");
}

/// Advertises while a host may connect: pairing is open (and a slot is free), or a host that should be connected
/// isn't. Each connection gets its own task.
async fn advertise(
    spawner: Spawner,
    peripheral: &mut Peripheral<'static, Controller, DefaultPacketPool>,
    stack: &'static St,
    server: &'static Server<'static>,
) -> ! {
    loop {
        // Pairing that ran out closes here.
        let expired = with(|s| {
            let gone = s.pairing_until.is_some_and(|t| Instant::now() >= t);
            if gone {
                s.pairing_until = None;
            }
            gone
        });
        if expired {
            event(EventKind::PairingClosed, 0, 0);
            drop_unpaired();
        }
        let (pairing, reconnect) = with(|s| {
            let pairing = pairing_open(s) && free_slot(s).is_some();
            let reconnect = (0..MAX_HOSTS).any(|i| wanted(s, i) && !s.hosts[i].connected());
            (pairing, reconnect)
        });
        if !pairing && !reconnect {
            ADVERTISE.wait().await;
            continue;
        }
        let flags = if pairing { LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED } else { BR_EDR_NOT_SUPPORTED };
        let mut adv = [0u8; 31];
        let n = AdStructure::encode_slice(
            &[
                AdStructure::Flags(flags),
                AdStructure::CompleteServiceUuids16(&[service::HUMAN_INTERFACE_DEVICE.to_le_bytes()]),
                AdStructure::Unknown { ty: 0x19, data: &0x03C1u16.to_le_bytes() }, // appearance: keyboard
            ],
            &mut adv,
        )
        .unwrap_or(0);
        let mut scan = [0u8; 31];
        let m = AdStructure::encode_slice(&[AdStructure::CompleteLocalName(NAME.as_bytes())], &mut scan).unwrap_or(0);
        let params = AdvertisementParameters {
            interval_min: Duration::from_millis(30),
            interval_max: Duration::from_millis(50),
            ..Default::default()
        };
        let adv = Advertisement::ConnectableScannableUndirected { adv_data: &adv[..n], scan_data: &scan[..m] };
        let acceptor = match peripheral.advertise(&params, adv).await {
            Ok(a) => a,
            Err(_) => {
                Timer::after_millis(200).await;
                continue;
            }
        };
        // Stop and look again when the reason to advertise changes (pairing opened or closed, a host selected), or when
        // pairing runs out.
        let until = with(|s| s.pairing_until).unwrap_or(Instant::now() + Duration::from_secs(3600));
        let conn = match select3(acceptor.accept(), ADVERTISE.wait(), Timer::at(until)).await {
            Either3::First(Ok(c)) => c,
            _ => continue,
        };
        let id = conn.peer_identity();
        // The slot is picked and held in one go: it is this connection's from here on.
        let picked = with(|s| {
            let (slot, new) = match s.bonds.slot_of(&id) {
                // A bonded host that should be connected now (one whose old connection the module still has is
                // taken to have lost it: the newer one is the slot's).
                Some(i) if wanted(s, i) => (i, false),
                Some(_) => return None,
                // Not a bonded host: only while pairing is open, and into a slot of its own.
                None if pairing_open(s) => (free_slot(s)?, true),
                None => return None,
            };
            s.live[slot] = true;
            s.conn_gen[slot] = s.conn_gen[slot].wrapping_add(1);
            Some((slot, new, s.conn_gen[slot]))
        });
        let Some((slot, new, mine)) = picked else {
            conn.disconnect();
            continue;
        };
        let spawned = match conn.with_attribute_server(server) {
            Ok(gatt) => match connection(slot, new, mine, gatt, stack, server) {
                Ok(t) => {
                    spawner.spawn(t);
                    true
                }
                Err(_) => {
                    log::line(format_args!("no task left for a connection"));
                    false
                }
            },
            Err(_) => false,
        };
        if !spawned {
            with(|s| {
                if s.conn_gen[slot] == mine {
                    s.live[slot] = false;
                }
            });
        }
    }
}

/// The selected host's parameters, or the others'.
fn params(selected: bool) -> RequestedConnParams {
    if selected {
        RequestedConnParams {
            min_connection_interval: Duration::from_micros(7_500),
            max_connection_interval: Duration::from_millis(15),
            max_latency: 0,
            supervision_timeout: Duration::from_secs(4),
            ..Default::default()
        }
    } else {
        RequestedConnParams {
            min_connection_interval: Duration::from_millis(30),
            max_connection_interval: Duration::from_millis(50),
            max_latency: 4,
            supervision_timeout: Duration::from_secs(6),
            ..Default::default()
        }
    }
}

type Conn = GattConnection<'static, 'static, DefaultPacketPool>;

/// The CCCDs of the characteristics that notify, in the order of [`notify`]'s bits.
fn cccds(server: &Server<'_>) -> [Option<u16>; 5] {
    [
        server.hid.keyboard.cccd_handle,
        server.hid.mouse.cccd_handle,
        server.hid.consumer.cccd_handle,
        server.hid.pointer.cccd_handle,
        server.bas.level.cccd_handle,
    ]
}

/// The notifications `conn`'s host has switched on ([`notify`] bits, with `KNOWN`).
fn notifications(server: &Server<'_>, conn: &Conn) -> u8 {
    let Some(t) = server.get_client_att_table(conn.raw()) else { return 0 };
    cccds(server).iter().enumerate().fold(notify::KNOWN, |m, (bit, h)| {
        let on = h.and_then(|h| t.get(h)).is_some_and(|v| v.first().is_some_and(|b| b & 1 != 0));
        if on { m | 1 << bit } else { m }
    })
}

/// Gives a bonded host that reconnects the notifications it switched on before. A GATT server keeps them for a bonded
/// client, which doesn't write them again; trouble-host keeps them only in RAM, and only while the module runs. Until
/// 0.1.21 a host back from sleep counted as not subscribed (the window showed it "connecting…" for good), and after a
/// restart of the module it got no input at all: Windows writes them once, after pairing (first dongle, 2026-11-15).
fn restore_notifications(server: &Server<'_>, conn: &Conn, bits: u8) {
    let Some(mut t) = server.get_client_att_table(conn.raw()) else { return };
    for (bit, h) in cccds(server).iter().enumerate() {
        if let Some(h) = h {
            let _ = t.write(*h, 0, if bits & (1 << bit) != 0 { &[1, 0] } else { &[0, 0] });
        }
    }
    server.set_client_att_table(conn.raw(), &t.view());
}

/// A host's notification settings go to flash this long after its last change: a host switches several on at once,
/// and each save erases a sector.
const NOTIFY_SAVE_AFTER: Duration = Duration::from_secs(2);

/// Saves of notification settings in one run of the module, at most (`bonds.rs`, "Wear"): a real host makes one per
/// pairing, a host that kept switching them would make one every few seconds.
const NOTIFY_SAVES: u8 = 16;

/// Writes slot `slot`'s notification settings, if it holds a bond and they changed (`Bonds::set`). Past the run's
/// [`NOTIFY_SAVES`] they are held in RAM instead, and the RP2354B is told (once per host).
fn store_notifications(slot: usize, bits: u8) {
    let first_held = with(|s| {
        let Some(b) = s.bonds.slots[slot] else { return false };
        let new = Slot { notify: bits, ..b };
        if new == b {
            false
        } else if s.notify_saves < NOTIFY_SAVES {
            s.notify_saves += 1;
            s.bonds.set(slot, Some(new));
            false
        } else {
            s.bonds.hold(slot, Some(new));
            let first = s.settings_held & (1 << slot) == 0;
            s.settings_held |= 1 << slot;
            first
        }
    });
    if first_held {
        log::line(format_args!("host {}: notification settings held in RAM (wear guard)", slot + 1));
        event(EventKind::SettingsHeld, slot as u8 + 1, 0);
    }
}

fn set_subscribed(slot: usize, bits: u8) {
    set_flags(slot, |h| {
        if bits & notify::KEYBOARD != 0 {
            h.flags |= host_flags::SUBSCRIBED;
        } else {
            h.flags &= !host_flags::SUBSCRIBED;
        }
    });
}

#[embassy_executor::task(pool_size = HOST_CONNS)]
async fn connection(
    slot: usize,
    new: bool,
    mine: u32,
    conn: GattConnection<'static, 'static, DefaultPacketPool>,
    stack: &'static St,
    server: &'static Server<'static>,
) {
    let host = slot as u8 + 1;
    log::line(format_args!("host {host}: connected{}", if new { " (pairing)" } else { "" }));
    // This connection is the slot's (`advertise`); an older task still running for it stops at its next event.
    let current = || with(|s| s.conn_gen[slot] == mine);
    set_flags(slot, |h| {
        h.flags |= host_flags::CONNECTED;
        h.addr = conn.raw().peer_address().to_bytes();
    });
    with(|s| s.pairing_peer[slot] = new.then(|| conn.raw().peer_identity()));
    while REPORTS[slot].try_receive().is_ok() {}
    MOTION[slot].lock(|m| *m.borrow_mut() = Motion::new());
    MOTION_READY[slot].reset();
    CTL[slot].reset();
    PASSKEY[slot].reset();
    // Bonded: encrypt with the stored key. New: pair and bond, the host asking for the passkey it shows. Bondable either
    // way (trouble-host's connections start not bondable): a bonded host that forgot the KVM pairs anew on this
    // connection, and trouble-host answers its pairing request before this task hears of it (`BondLost`). Not bondable,
    // that pairing took the passkey and gave no bond (first dongle, 2026-11-14: "it accepts the code, then nothing").
    let _ = conn.raw().set_bondable(true);
    let _ = conn.raw().request_security();
    if !new {
        let bits = with(|s| s.bonds.slots[slot].map(|b| b.notifications())).unwrap_or(notify::REPORTS);
        restore_notifications(server, &conn, bits);
        set_subscribed(slot, bits);
    }
    let mut waiting_passkey = false;
    // Notification settings changed by the host, and when they go to flash.
    let mut unsaved: Option<(u8, Instant)> = None;
    loop {
        let passkey =
            async { if waiting_passkey { PASSKEY[slot].wait().await } else { core::future::pending().await } };
        let save = async {
            match unsaved {
                Some((_, at)) => Timer::at(at).await,
                None => core::future::pending().await,
            }
        };
        let input = select(REPORTS[slot].receive(), MOTION_READY[slot].wait());
        let woke = select4(conn.next(), input, CTL[slot].wait(), select(passkey, save)).await;
        // Superseded by a newer connection of this slot: nothing of this one counts any more.
        if !current() {
            log::line(format_args!("host {host}: an older connection, superseded"));
            conn.raw().disconnect();
            break;
        }
        match woke {
            Either4::First(ev) => match ev {
                GattConnectionEvent::Disconnected { reason } => {
                    log::line(format_args!("host {host}: disconnected ({reason:?})"));
                    break;
                }
                GattConnectionEvent::ConnectionParamsUpdated { conn_interval, peripheral_latency, .. } => {
                    set_flags(slot, |h| {
                        h.interval = (conn_interval.as_micros() / 1250) as u16;
                        h.latency = peripheral_latency;
                    });
                }
                GattConnectionEvent::PassKeyInput => {
                    waiting_passkey = true;
                    PASSKEY[slot].reset();
                    with(|s| s.pairing_now[slot] = true);
                    event(EventKind::PasskeyRequest, host, 0);
                }
                GattConnectionEvent::PairingComplete { bond, .. } => {
                    waiting_passkey = false;
                    if let Some(b) = bond {
                        // With the notifications the host may have switched on before it bonded.
                        let s = Slot { notify: notifications(server, &conn), ..Slot::from_bond(&b) };
                        unsaved = None;
                        // Written only if the keys are new (bonds.rs, "Wear"). The same host in another slot goes
                        // (it paired anew: one host in two slots polluted the module, 2026-11-20).
                        let twins = with(|st| {
                            let twins = st.bonds.set_new(slot, s);
                            st.pairing_until = None;
                            st.pairing_peer[slot] = None;
                            st.pairing_now[slot] = false;
                            twins
                        });
                        set_flags(slot, |h| h.flags |= host_flags::BONDED | host_flags::ENCRYPTED);
                        log::line(format_args!("host {host}: bonded"));
                        for (i, ctl) in CTL.iter().enumerate().filter(|(i, _)| twins & (1 << i) != 0) {
                            log::line(format_args!(
                                "host {}: the same host as host {host}, its old pairing goes",
                                i + 1
                            ));
                            set_flags(i, |h| h.flags &= !host_flags::BONDED);
                            ctl.signal(Ctl::Disconnect);
                        }
                        event(EventKind::Paired, host, 0);
                        // Pairing is closed now: other hosts let in to pair go, and keys left from earlier attempts.
                        drop_unpaired();
                        tidy_keys(stack);
                        let selected = with(|s| in_use(s, slot));
                        let _ = conn.raw().update_connection_params(stack, &params(selected)).await;
                    } else if !with(|s| s.hosts[slot].bonded()) {
                        // Paired without a bond (the host didn't ask for one): nothing to reconnect with, so not a host.
                        log::line(format_args!("host {host}: paired without a bond"));
                        event(EventKind::PairFailed, host, 0);
                        conn.raw().disconnect();
                    }
                }
                GattConnectionEvent::PairingFailed(_) => {
                    waiting_passkey = false;
                    with(|s| s.pairing_now[slot] = false);
                    event(EventKind::PairFailed, host, 0);
                    if !with(|s| s.hosts[slot].bonded()) {
                        conn.raw().disconnect();
                    }
                }
                GattConnectionEvent::BondLost => {
                    // The host forgot the KVM and pairs anew: its old keys are no good any more.
                    let id = with(|s| s.bonds.slots[slot].map(|b| b.identity()));
                    if let Some(id) = id {
                        let _ = stack.remove_bond_information(id);
                    }
                    with(|s| s.bonds.set(slot, None));
                    set_flags(slot, |h| h.flags &= !host_flags::BONDED);
                    event(EventKind::BondLost, host, 0);
                    if with(|s| pairing_open(s)) {
                        // Into the same slot (this connection holds it), with a passkey as any new host.
                        with(|s| s.pairing_peer[slot] = Some(conn.raw().peer_identity()));
                        log::line(format_args!("host {host}: pairing anew"));
                    } else {
                        // Only while pairing is open, as for any host the module doesn't know.
                        log::line(format_args!("host {host}: forgot the KVM, pairing is closed"));
                        conn.raw().disconnect();
                    }
                }
                GattConnectionEvent::Encrypted { .. } => {
                    set_flags(slot, |h| h.flags |= host_flags::ENCRYPTED);
                    // A pairing exchanges its keys from here on: the parameters wait for its end (`PairingComplete`),
                    // so that this task keeps taking the stack's events meanwhile.
                    if with(|s| s.pairing_peer[slot].is_none()) {
                        let selected = with(|s| in_use(s, slot));
                        let _ = conn.raw().update_connection_params(stack, &params(selected)).await;
                    }
                }
                GattConnectionEvent::Gatt { event: e } => {
                    let mut cccd = false;
                    if let GattEvent::Write(w) = &e {
                        if cccds(server).contains(&Some(w.handle())) {
                            cccd = true;
                        } else if w.handle() == server.hid.leds.handle {
                            let leds = w.with_data(|_, d| d.first().copied().unwrap_or(0));
                            link::send(&BtMsg::Output { host, id: report::KEYBOARD, data: &[leds] });
                        }
                    }
                    if let Ok(reply) = e.accept() {
                        reply.send().await;
                    }
                    // Accepted, the write is in the table: what the host has on now, kept with its bond.
                    if cccd {
                        let bits = notifications(server, &conn);
                        set_subscribed(slot, bits);
                        unsaved = Some((bits, Instant::now() + NOTIFY_SAVE_AFTER));
                    }
                }
                _ => {}
            },
            Either4::Second(input) => {
                // Queued reports first (motion before them was moved into the queue), then the motion so far: what
                // comes while it goes out waits for the next one.
                let Some((id, data, len)) = (match input {
                    Either::First(r) => Some(r),
                    Either::Second(()) => take_motion(slot),
                }) else {
                    continue;
                };
                if !MOTION[slot].lock(|m| m.borrow().is_empty()) {
                    MOTION_READY[slot].signal(());
                }
                // The link's own state: an `Encrypted` event can be missed (a full event queue, before 0.1.21), and the
                // host then got nothing while it showed "connecting…".
                let encrypted = conn.raw().security_level().is_ok_and(|l| l.encrypted());
                if !encrypted {
                    continue;
                }
                if with(|s| s.hosts[slot].flags & host_flags::ENCRYPTED == 0) {
                    set_flags(slot, |h| h.flags |= host_flags::ENCRYPTED);
                }
                let d = &data[..len as usize];
                let _ = match id {
                    report::KEYBOARD => {
                        let mut v = [0u8; report::KEYBOARD_LEN];
                        v[..d.len().min(8)].copy_from_slice(&d[..d.len().min(8)]);
                        server.hid.keyboard.notify(&conn, &v, false).await
                    }
                    report::MOUSE => {
                        let mut v = [0u8; report::MOUSE_LEN];
                        v[..d.len().min(8)].copy_from_slice(&d[..d.len().min(8)]);
                        server.hid.mouse.notify(&conn, &v, false).await
                    }
                    report::CONSUMER => {
                        let mut v = [0u8; report::CONSUMER_LEN];
                        v[..d.len().min(2)].copy_from_slice(&d[..d.len().min(2)]);
                        server.hid.consumer.notify(&conn, &v, false).await
                    }
                    report::POINTER => {
                        let mut v = [0u8; report::POINTER_LEN];
                        v[..d.len().min(8)].copy_from_slice(&d[..d.len().min(8)]);
                        server.hid.pointer.notify(&conn, &v, false).await
                    }
                    _ => Ok(()),
                };
            }
            Either4::Third(Ctl::Disconnect) => conn.raw().disconnect(),
            Either4::Third(Ctl::Params) => {
                let selected = with(|s| in_use(s, slot));
                let _ = conn.raw().update_connection_params(stack, &params(selected)).await;
            }
            Either4::Fourth(Either::Second(())) => {
                if let Some((bits, _)) = unsaved.take() {
                    store_notifications(slot, bits);
                }
            }
            Either4::Fourth(Either::First(value)) => {
                waiting_passkey = false;
                let _ = match value {
                    Some(v) => conn.raw().pass_key_input(v),
                    None => conn.raw().pass_key_cancel(),
                };
            }
        }
    }
    // Settings the host changed just before it left (an older connection's are no longer the slot's).
    if let Some((bits, _)) = unsaved
        && current()
    {
        store_notifications(slot, bits);
    }
    // Only the slot's current connection takes its state with it, and lets the slot go.
    if current() {
        with(|s| {
            s.live[slot] = false;
            s.pairing_peer[slot] = None;
            s.pairing_now[slot] = false;
        });
        set_flags(slot, |h| {
            h.flags &= !(host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED);
            h.interval = 0;
            h.latency = 0;
        });
    }
    // A pairing that didn't end in a bond leaves its keys in the stack.
    tidy_keys(stack);
    ADVERTISE.signal(());
}

/// What the RP2354B asks for.
#[embassy_executor::task]
async fn control(stack: &'static St) {
    loop {
        let m = link::FROM_RP.receive().await;
        let Ok(msg) = BtMsg::decode(&m) else { continue };
        match msg {
            BtMsg::Input { host, id, data } => {
                let slot = host as usize;
                if (1..=MAX_HOSTS).contains(&slot) && with(|s| in_use(s, slot - 1)) {
                    let i = slot - 1;
                    if Motion::is_motion(id) {
                        if let Some((_, id, d)) = MOTION[i].lock(|m| m.borrow_mut().add(host, id, data)) {
                            let _ = REPORTS[i].try_send((id, d, 8));
                        }
                        MOTION_READY[i].signal(());
                    } else {
                        // After the motion before it.
                        while let Some(r) = take_motion(i) {
                            let _ = REPORTS[i].try_send(r);
                        }
                        let mut d = [0u8; 8];
                        let n = data.len().min(8);
                        d[..n].copy_from_slice(&data[..n]);
                        let _ = REPORTS[i].try_send((id, d, n as u8));
                    }
                }
            }
            BtMsg::Select { host, pinned } => {
                let (old, old_pinned) =
                    with(|s| (core::mem::replace(&mut s.selected, host), core::mem::replace(&mut s.pinned, pinned)));
                for (slot, ctl) in CTL.iter().enumerate() {
                    let n = slot as u8 + 1;
                    let bit = 1 << slot;
                    if n != old && n != host && (old_pinned ^ pinned) & bit == 0 {
                        continue;
                    }
                    let (connected, keep) = with(|s| (s.hosts[slot].connected(), wanted(s, slot)));
                    if connected && !keep {
                        ctl.signal(Ctl::Disconnect);
                    } else if connected {
                        ctl.signal(Ctl::Params);
                    }
                }
                ADVERTISE.signal(());
            }
            BtMsg::Pair { on, seconds } => {
                with(|s| s.pairing_until = on.then(|| Instant::now() + Duration::from_secs(seconds as u64)));
                event(if on { EventKind::PairingOpen } else { EventKind::PairingClosed }, 0, seconds as u32);
                if !on {
                    drop_unpaired();
                }
                // The advertising loop closes it when the time is up.
                ADVERTISE.signal(());
            }
            BtMsg::Forget { host } => {
                let picked = |slot: usize| host == 0xFF || host as usize == slot + 1;
                // One flash save for all of them, none for slots already free (bonds.rs, "Wear").
                with(|s| {
                    s.bonds.clear(picked);
                    for (i, p) in s.pairing_peer.iter_mut().enumerate() {
                        if picked(i) {
                            *p = None;
                        }
                    }
                });
                // Out of the stack's keys too: otherwise the forgotten host, which still has its keys, would get back in
                // encrypted, without a passkey, the next time pairing is open (until the module restarts). The host
                // keeps the KVM in its own list: Bluetooth LE has no way to make it let go, so it is removed there.
                tidy_keys(stack);
                for slot in (0..MAX_HOSTS).filter(|&i| picked(i)) {
                    set_flags(slot, |h| h.flags &= !host_flags::BONDED);
                    CTL[slot].signal(Ctl::Disconnect);
                }
                ADVERTISE.signal(());
            }
            BtMsg::Passkey { host, value } => {
                if let Some(slot) = (host as usize).checked_sub(1).filter(|i| *i < MAX_HOSTS) {
                    PASSKEY[slot].signal(value);
                }
            }
            BtMsg::Policy { host, policy } => {
                if let Some(slot) = (host as usize).checked_sub(1).filter(|i| *i < MAX_HOSTS) {
                    with(|s| s.policies[slot] = policy);
                    let (connected, keep) = with(|s| (s.hosts[slot].connected(), wanted(s, slot)));
                    if connected && !keep {
                        CTL[slot].signal(Ctl::Disconnect);
                    }
                    ADVERTISE.signal(());
                }
            }
            BtMsg::RequestState | BtMsg::Hello { .. } => {
                for slot in 0..MAX_HOSTS {
                    report_host(slot);
                }
                devices::report_all();
                // The RP2354B may have restarted and forgotten.
                let held = with(|s| s.settings_held);
                for slot in (0..MAX_HOSTS).filter(|i| held & (1 << i) != 0) {
                    event(EventKind::SettingsHeld, slot as u8 + 1, 0);
                }
            }
            BtMsg::EraseAll => {
                log::line(format_args!("erasing every pairing, then restarting"));
                with(|s| s.bonds.erase_all());
                devices::erase_all();
                while let Some(id) = stack.with_bond_information(|keys| keys.first().map(|b| b.identity)) {
                    if stack.remove_bond_information(id).is_err() {
                        break;
                    }
                }
                // The log line and the acknowledgement out first; the restart empties the controller's resolving
                // list and every connection.
                Timer::after_millis(300).await;
                esp_hal::system::software_reset();
            }
            BtMsg::Scan { .. }
            | BtMsg::PairDevice { .. }
            | BtMsg::ForgetDevice { .. }
            | BtMsg::RequestMap { .. }
            | BtMsg::DeviceOutput { .. } => devices::handle(&msg, stack),
            _ => {}
        }
    }
}
