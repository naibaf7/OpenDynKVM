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
//! - **Control:** a service of its own carries the control protocol for a bonded host's software (software.md §8.1,
//!   "Over Bluetooth"; [`kvm_proto::link::bt::control`]): a request written to it goes to the RP2354B, which decides
//!   whether that host may ask; the answer, or an event, comes back to be notified. Its attributes need the bond's
//!   authenticated link, its notifications are never kept with the bond (each session subscribes anew), and while
//!   requests come the host is asked for the selected host's short interval.

use core::cell::RefCell;

use embassy_executor::Spawner;
use embassy_futures::select::{Either3, Either4, select3, select4};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use kvm_proto::link::bt::{BtMsg, EventKind, HostState, MAX_HOSTS, Motion, Policy, control, host_flags, report};
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

#[gatt_server(connections_max = HOST_CONNS, attribute_table_size = 84)]
#[allow(dead_code)] // the Device Information and Battery services are only read by the hosts
struct Server {
    hid: HidService,
    dis: DeviceInformation,
    bas: BatteryService,
    ctl: ControlService,
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
    /// The virtual gamepad (report 5), for a host that subscribes to it (hid_mapping.md §6.1).
    #[descriptor(uuid = descriptors::REPORT_REFERENCE, read, value = [report::GAMEPAD, 1])]
    #[characteristic(uuid = characteristic::REPORT, read, notify, value = [0; report::GAMEPAD_LEN])]
    gamepad: [u8; report::GAMEPAD_LEN],
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

/// The control protocol for a bonded host's software ([`control`]: the same UUIDs, which the macro takes only as
/// literals). Only over the bond's authenticated link.
#[gatt_service(uuid = "98141409-422d-48a6-aba2-21d691a393cc")]
struct ControlService {
    /// Requests, one 64-byte report each ([`control::REQUEST`]).
    #[characteristic(
        uuid = "98141410-422d-48a6-aba2-21d691a393cc",
        write,
        write_without_response,
        permissions(authenticated),
        value = [0; control::REPORT_LEN]
    )]
    request: [u8; control::REPORT_LEN],
    /// Answers and events ([`control::REPLY`]).
    #[characteristic(
        uuid = "98141411-422d-48a6-aba2-21d691a393cc",
        notify,
        permissions(authenticated),
        value = [0; control::REPORT_LEN]
    )]
    reply: [u8; control::REPORT_LEN],
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

/// A report on its way to a slot's connection task: its ID, its bytes, and how many of them count.
type Queued = (u8, [u8; report::INPUT_MAX], u8);

fn queued(id: u8, data: &[u8]) -> Queued {
    let mut d = [0u8; report::INPUT_MAX];
    let n = data.len().min(report::INPUT_MAX);
    d[..n].copy_from_slice(&data[..n]);
    (id, d, n as u8)
}

static REPORTS: [Channel<CriticalSectionRawMutex, Queued, 16>; MAX_HOSTS] = [const { Channel::new() }; MAX_HOSTS];
/// Each slot's motion, coalesced until the host takes it at a connection event ([`Motion`]): a mouse reports far more
/// often than a host takes notifications, and a queue of them was late and then dropped motion (first dongle,
/// 2026-11-14: the pointer crawled).
static MOTION: [Mutex<CriticalSectionRawMutex, RefCell<Motion>>; MAX_HOSTS] =
    [const { Mutex::new(RefCell::new(Motion::new())) }; MAX_HOSTS];
static MOTION_READY: [Signal<CriticalSectionRawMutex, ()>; MAX_HOSTS] = [const { Signal::new() }; MAX_HOSTS];

/// How evenly pointer motion flows to a host, for the log every [`FLOW_WINDOW`] while it moves: what comes from the
/// RP2354B and what goes to the host, each with its longest gap (shorter than [`FLOW_PAUSE`]: longer ones are the
/// hand at rest), and the most packets the controller held for all connections. A phone host's pointer jittered on
/// the ESP32 more than on the ESP32-C3 (2026-11-25): late input, late connection events, or a queue that fills.
#[derive(Clone, Copy)]
struct Flow {
    since: Option<Instant>,
    inputs: u32,
    last_input: Option<Instant>,
    input_gap: u64,
    sent: u32,
    last_sent: Option<Instant>,
    sent_gap: u64,
    queued: usize,
}
const FLOW_WINDOW: Duration = Duration::from_secs(5);

/// Packets in the controller from which a host's motion waits (on the ESP32), and how long at most.
const MOTION_QUEUE: usize = 2;
const MOTION_WAIT: Duration = Duration::from_millis(30);
const FLOW_PAUSE: Duration = Duration::from_millis(500);
static FLOW: Mutex<CriticalSectionRawMutex, RefCell<[Flow; MAX_HOSTS]>> = Mutex::new(RefCell::new(
    [Flow { since: None, inputs: 0, last_input: None, input_gap: 0, sent: 0, last_sent: None, sent_gap: 0, queued: 0 };
        MAX_HOSTS],
));

/// The gap since `last`, if it isn't a pause, into `max`; `last` becomes now.
fn flow_gap(last: &mut Option<Instant>, max: &mut u64) {
    let now = Instant::now();
    if let Some(t) = *last {
        let gap = now.duration_since(t);
        if gap < FLOW_PAUSE {
            *max = (*max).max(gap.as_millis());
        }
    }
    *last = Some(now);
}

/// Motion for host slot `slot` came from the RP2354B.
fn flow_input(slot: usize) {
    FLOW.lock(|f| {
        let f = &mut f.borrow_mut()[slot];
        f.since.get_or_insert_with(Instant::now);
        f.inputs += 1;
        flow_gap(&mut f.last_input, &mut f.input_gap);
    });
}

/// Motion went to host `host` (slot `slot`), `queued` packets in the controller: the window's line, when it's over.
fn flow_sent(host: u8, slot: usize, queued: usize) {
    let done = FLOW.lock(|f| {
        let f = &mut f.borrow_mut()[slot];
        f.sent += 1;
        flow_gap(&mut f.last_sent, &mut f.sent_gap);
        f.queued = f.queued.max(queued);
        let over = f.since.is_some_and(|t| t.elapsed() >= FLOW_WINDOW);
        over.then(|| core::mem::replace(f, Flow { since: None, inputs: 0, input_gap: 0, sent: 0, sent_gap: 0, queued: 0, ..*f }))
    });
    if let Some(f) = done {
        log::line(format_args!(
            "host {host}: motion 5 s: {} in (gap up to {} ms), {} out (gap up to {} ms), up to {} queued",
            f.inputs, f.input_gap, f.sent, f.sent_gap, f.queued
        ));
    }
}

fn take_motion(slot: usize) -> Option<Queued> {
    MOTION[slot].lock(|m| m.borrow_mut().take()).map(|(_, id, d)| queued(id, &d))
}
/// Each slot's control-protocol reports from the RP2354B, to notify: [`control::WINDOW`] answers, and the events
/// between them. A switch alone brings a burst of events (the switch, the active host, each port, each host whose
/// interval changes); until 0.1.53 there was room for 8, the answers to a refresh under way were dropped, and the
/// host's software took the KVM for gone (a Windows host, 2026-11-24: its window dropped the KVM on every switch).
static CONTROL_OUT: [Channel<CriticalSectionRawMutex, [u8; control::REPORT_LEN], CONTROL_QUEUE>; MAX_HOSTS] =
    [const { Channel::new() }; MAX_HOSTS];
const CONTROL_QUEUE: usize = 24;
/// How long after a host's last control request it keeps the short connection interval: its software asks in bursts
/// (a window's refresh), and each burst would wait for the slow interval's events otherwise.
const CONTROL_FAST: Duration = Duration::from_secs(5);
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

/// The chip's controller as the stack's HCI (`radio.rs` of each firmware: the ESP32's also carries its Classic host).
pub use crate::radio::Controller;
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

/// The stack's resources (23 KB), never built on the stack. On the ESP32 in the DRAM after the ROM's data, next to the
/// heap (`.dram2_uninit`): with Bluetooth on, its main DRAM is 128 KB, and its stack was 13 KB until they moved
/// (`firmware/esp32/src/main.rs`).
#[cfg_attr(target_arch = "xtensa", unsafe(link_section = ".dram2_uninit"))]
static mut RESOURCES: core::mem::MaybeUninit<HostResources<DefaultPacketPool, CONNS, L2CAP, 1, KEYS>> =
    core::mem::MaybeUninit::uninit();
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
    let controller: Controller = crate::radio::controller(spawner, bt, ACTIVITIES);
    // SAFETY: taken once, here (`run` never returns). Every field of `HostResources` is a `MaybeUninit`
    // (`HostResources::new` leaves them all uninitialized), so its uninitialized memory is a valid value of it.
    let resources = unsafe { (*core::ptr::addr_of_mut!(RESOURCES)).assume_init_mut() };
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
    // The stack's runner and the advertising, tasks of their own: their futures are made in their tasks' storage. As
    // one `select` here they were made on the stack and copied, which took this function's frame to 33 KB, and with
    // `main`'s above it more than the ESP32's stack (2026-11-24).
    spawner.spawn(host_runner(stack).expect("runner task"));
    spawner.spawn(advertiser(spawner, stack, server).expect("advertising task"));
    core::future::pending().await
}

#[embassy_executor::task]
async fn host_runner(stack: &'static St) {
    let mut runner = stack.runner();
    let _ = runner.run_with_handler(&devices::Handler).await;
    panic!("Bluetooth host stopped");
}

#[embassy_executor::task]
async fn advertiser(spawner: Spawner, stack: &'static St, server: &'static Server<'static>) {
    let mut peripheral = stack.peripheral();
    advertise(spawner, &mut peripheral, stack, server).await
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
        // The control service too (30 of the 31 bytes), so that a host's software can find the KVM by it.
        let m = AdStructure::encode_slice(
            &[
                AdStructure::CompleteLocalName(NAME.as_bytes()),
                AdStructure::CompleteServiceUuids128(&[control::SERVICE_LE]),
            ],
            &mut scan,
        )
        .unwrap_or(0);
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
fn cccds(server: &Server<'_>) -> [Option<u16>; 6] {
    [
        server.hid.keyboard.cccd_handle,
        server.hid.mouse.cccd_handle,
        server.hid.consumer.cccd_handle,
        server.hid.pointer.cccd_handle,
        server.bas.level.cccd_handle,
        server.hid.gamepad.cccd_handle,
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

/// The host's software has the control service's answers switched on: a control session ([`host_flags::CONTROL`]).
fn control_session(server: &Server<'_>, conn: &Conn) -> bool {
    let (Some(h), Some(t)) = (server.ctl.reply.cccd_handle, server.get_client_att_table(conn.raw())) else {
        return false;
    };
    t.get(h).is_some_and(|v| v.first().is_some_and(|b| b & 1 != 0))
}

/// A connection starts without a control session, whatever the stack kept from the last one of a bonded host: the
/// software that had it may be gone, and each session subscribes anew.
fn end_control_session(server: &Server<'_>, conn: &Conn) {
    let (Some(h), Some(mut t)) = (server.ctl.reply.cccd_handle, server.get_client_att_table(conn.raw())) else {
        return;
    };
    let _ = t.write(h, 0, &[0, 0]);
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
        // The RP2354B sees what the host took (its outputs view, the gamepad's fallback).
        h.reports = bits;
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
        h.flags &= !host_flags::CONTROL;
        h.addr = conn.raw().peer_address().to_bytes();
    });
    with(|s| s.pairing_peer[slot] = new.then(|| conn.raw().peer_identity()));
    while REPORTS[slot].try_receive().is_ok() {}
    while CONTROL_OUT[slot].try_receive().is_ok() {}
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
    end_control_session(server, &conn);
    let mut waiting_passkey = false;
    // Notification settings changed by the host, and when they go to flash.
    let mut unsaved: Option<(u8, Instant)> = None;
    // The short interval for the host's software, until this long after its last request.
    let mut fast_until: Option<Instant> = None;
    loop {
        let passkey =
            async { if waiting_passkey { PASSKEY[slot].wait().await } else { core::future::pending().await } };
        let save = async {
            match unsaved {
                Some((_, at)) => Timer::at(at).await,
                None => core::future::pending().await,
            }
        };
        let slow_down = async {
            match fast_until {
                Some(at) => Timer::at(at).await,
                None => core::future::pending().await,
            }
        };
        let input = select3(REPORTS[slot].receive(), MOTION_READY[slot].wait(), CONTROL_OUT[slot].receive());
        let woke = select4(conn.next(), input, CTL[slot].wait(), select3(passkey, save, slow_down)).await;
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
                    log::line(format_args!(
                        "host {host}: interval {}.{:02} ms, latency {peripheral_latency}",
                        conn_interval.as_micros() / 1000,
                        conn_interval.as_micros() % 1000 / 10
                    ));
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
                    let mut session = false;
                    let mut requested = false;
                    if let GattEvent::Write(w) = &e {
                        if cccds(server).contains(&Some(w.handle())) {
                            cccd = true;
                        } else if Some(w.handle()) == server.ctl.reply.cccd_handle {
                            session = true;
                        } else if w.handle() == server.ctl.request.handle {
                            // Only a bonded host's (the attribute already wants an authenticated link): whether it may
                            // ask is the RP2354B's to say.
                            if with(|s| s.hosts[slot].bonded() && s.pairing_peer[slot].is_none()) {
                                w.with_data(|_, d| link::send(&BtMsg::ControlRequest { host, data: d }));
                                requested = true;
                            }
                        } else if w.handle() == server.hid.leds.handle {
                            let leds = w.with_data(|_, d| d.first().copied().unwrap_or(0));
                            link::send(&BtMsg::Output { host, id: report::KEYBOARD, data: &[leds] });
                        }
                    }
                    if let Ok(reply) = e.accept() {
                        crate::stall::watched(stack, host, reply.send()).await;
                    }
                    // Accepted, the write is in the table: what the host has on now, kept with its bond.
                    if cccd {
                        let bits = notifications(server, &conn);
                        set_subscribed(slot, bits);
                        unsaved = Some((bits, Instant::now() + NOTIFY_SAVE_AFTER));
                    }
                    // A control session starts or ends: only in RAM, never with the bond.
                    if session {
                        let on = control_session(server, &conn);
                        let mtu = conn.raw().att_mtu();
                        if on && mtu < control::MIN_MTU {
                            log::line(format_args!("host {host}: ATT MTU {mtu}, too small for the control service"));
                        }
                        log::line(format_args!("host {host}: control session {}", if on { "open" } else { "closed" }));
                        set_flags(slot, |h| {
                            if on {
                                h.flags |= host_flags::CONTROL;
                            } else {
                                h.flags &= !host_flags::CONTROL;
                            }
                        });
                    }
                    // The software asks in bursts: the short interval while they come.
                    if requested {
                        let was_fast = fast_until.is_some() || with(|s| in_use(s, slot));
                        fast_until = Some(Instant::now() + CONTROL_FAST);
                        if !was_fast {
                            let _ = conn.raw().update_connection_params(stack, &params(true)).await;
                        }
                    }
                }
                _ => {}
            },
            Either4::Second(Either3::Third(reply)) => {
                // The answer to a request of this host's software, or an event for it. A session it ended gets none.
                let encrypted = conn.raw().security_level().is_ok_and(|l| l.encrypted());
                if encrypted && with(|s| s.hosts[slot].flags & host_flags::CONTROL != 0) {
                    let _ = crate::stall::watched(stack, host, server.ctl.reply.notify(&conn, &reply, false)).await;
                }
            }
            Either4::Second(input) => {
                // Queued reports first (motion before them was moved into the queue), then the motion so far: what
                // comes while it goes out waits for the next one.
                // On the ESP32, motion waits while the controller still holds [`MOTION_QUEUE`] packets, and adds up
                // meanwhile: queued as it came, it went out late and in bursts whenever connection events were late
                // (its controller serves them less evenly than the ESP32-C3's), and the pointer jumped.
                if matches!(input, Either3::Second(())) && cfg!(target_arch = "xtensa") {
                    let (free, all) = stack.link_credits();
                    if all.saturating_sub(free) >= MOTION_QUEUE {
                        crate::stall::SENT.reset();
                        let _ = embassy_time::with_timeout(MOTION_WAIT, crate::stall::SENT.wait()).await;
                    }
                }
                let Some((id, data, len)) = (match input {
                    Either3::First(r) => Some(r),
                    Either3::Second(()) => take_motion(slot),
                    Either3::Third(_) => None,
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
                let send = async {
                    match id {
                        report::KEYBOARD => {
                            let mut v = [0u8; report::KEYBOARD_LEN];
                            v[..d.len().min(8)].copy_from_slice(&d[..d.len().min(8)]);
                            server.hid.keyboard.notify(&conn, &v, false).await
                        }
                        report::MOUSE => {
                            let mut v = [0u8; report::MOUSE_LEN];
                            v[..d.len().min(8)].copy_from_slice(&d[..d.len().min(8)]);
                            let r = server.hid.mouse.notify(&conn, &v, false).await;
                            let (free, all) = stack.link_credits();
                            flow_sent(host, slot, all.saturating_sub(free));
                            r
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
                        report::GAMEPAD => {
                            let mut v = [0u8; report::GAMEPAD_LEN];
                            let n = d.len().min(report::GAMEPAD_LEN);
                            v[..n].copy_from_slice(&d[..n]);
                            server.hid.gamepad.notify(&conn, &v, false).await
                        }
                        _ => Ok(()),
                    }
                };
                // A report that can't go out for long: the stack is stuck (`stall`).
                let _ = crate::stall::watched(stack, host, send).await;
            }
            Either4::Third(Ctl::Disconnect) => conn.raw().disconnect(),
            Either4::Third(Ctl::Params) => {
                let selected = with(|s| in_use(s, slot));
                let _ = conn.raw().update_connection_params(stack, &params(selected || fast_until.is_some())).await;
            }
            Either4::Fourth(Either3::Second(())) => {
                if let Some((bits, _)) = unsaved.take() {
                    store_notifications(slot, bits);
                }
            }
            Either4::Fourth(Either3::Third(())) => {
                // The host's software went quiet: back to the interval its place says.
                fast_until = None;
                if !with(|s| in_use(s, slot)) {
                    let _ = conn.raw().update_connection_params(stack, &params(false)).await;
                }
            }
            Either4::Fourth(Either3::First(value)) => {
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
            h.flags &= !(host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED | host_flags::CONTROL);
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
                        flow_input(i);
                        if let Some((_, id, d)) = MOTION[i].lock(|m| m.borrow_mut().add(host, id, data)) {
                            let _ = REPORTS[i].try_send(queued(id, &d));
                        }
                        MOTION_READY[i].signal(());
                    } else {
                        // After the motion before it.
                        while let Some(r) = take_motion(i) {
                            let _ = REPORTS[i].try_send(r);
                        }
                        let _ = REPORTS[i].try_send(queued(id, data));
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
            BtMsg::ControlReply { host, data } => {
                // Dropped if the host's software is that far behind: it times out and asks again.
                if let Some(slot) = (host as usize).checked_sub(1).filter(|i| *i < MAX_HOSTS) {
                    let _ = CONTROL_OUT[slot].try_send(control::pad(data));
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
