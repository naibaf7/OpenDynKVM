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

use crate::bonds::{Bonds, Slot};
use crate::{link, log};

/// Hosts at once: the four slots, and one more while a new host pairs into a free slot.
const CONNS: usize = MAX_HOSTS + 1;
const L2CAP: usize = CONNS * 2;
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

#[gatt_server(connections_max = CONNS, attribute_table_size = 64)]
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
static REPORTS: [Channel<CriticalSectionRawMutex, (u8, [u8; 8], u8), 16>; MAX_HOSTS] =
    [const { Channel::new() }; MAX_HOSTS];
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
/// The passkey typed for the slot that asked (None: cancel).
static PASSKEY: Signal<CriticalSectionRawMutex, Option<u32>> = Signal::new();
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

/// The hosts the module should be connected to now: bonded, and either always connected or the one selected.
fn wanted(s: &State, slot: usize) -> bool {
    s.hosts[slot].bonded()
        && (s.policies[slot] == Policy::StayConnected || s.selected as usize == slot + 1)
}

type Controller = ExternalController<BleConnector<'static>, 20>;
type St = Stack<'static, Controller, DefaultPacketPool>;

static RESOURCES: StaticCell<HostResources<DefaultPacketPool, CONNS, L2CAP, 1, MAX_HOSTS>> = StaticCell::new();
static STACK: StaticCell<St> = StaticCell::new();
static SERVER: StaticCell<Server<'static>> = StaticCell::new();
static BONDS: StaticCell<Bonds> = StaticCell::new();

pub async fn run(spawner: Spawner, bt: esp_hal::peripherals::BT<'static>, bonds: Bonds) -> ! {
    let config = esp_radio::ble::Config::default()
        .with_max_connections(CONNS as u8)
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
            policies: [Policy::StayConnected; MAX_HOSTS],
            pairing_until: None,
            conn_gen: [0; MAX_HOSTS],
            live: [false; MAX_HOSTS],
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
    spawner.spawn(control(stack).expect("control task"));
    let mut runner = stack.runner();
    let mut peripheral = stack.peripheral();
    let _ = select(runner.run(), advertise(spawner, &mut peripheral, stack, server)).await;
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

#[embassy_executor::task(pool_size = CONNS)]
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
    while REPORTS[slot].try_receive().is_ok() {}
    MOTION[slot].lock(|m| *m.borrow_mut() = Motion::new());
    MOTION_READY[slot].reset();
    CTL[slot].reset();
    // Bonded: encrypt with the stored key. New: pair and bond, the host asking for the passkey it shows. Bondable either
    // way (trouble-host's connections start not bondable): a bonded host that forgot the KVM pairs anew on this
    // connection, and trouble-host answers its pairing request before this task hears of it (`BondLost`). Not bondable,
    // that pairing took the passkey and gave no bond (first dongle, 2026-11-14: "it accepts the code, then nothing").
    let _ = conn.raw().set_bondable(true);
    let _ = conn.raw().request_security();
    let mut waiting_passkey = false;
    let kb_cccd = server.hid.keyboard.cccd_handle;
    loop {
        let passkey = async {
            if waiting_passkey { PASSKEY.wait().await } else { core::future::pending().await }
        };
        let input = select(REPORTS[slot].receive(), MOTION_READY[slot].wait());
        let woke = select4(conn.next(), input, CTL[slot].wait(), passkey).await;
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
                    PASSKEY.reset();
                    event(EventKind::PasskeyRequest, host, 0);
                }
                GattConnectionEvent::PairingComplete { bond, .. } => {
                    waiting_passkey = false;
                    if let Some(b) = bond {
                        let s = Slot::from_bond(&b);
                        // Written only if the keys are new (bonds.rs, "Wear").
                        with(|st| {
                            st.bonds.set(slot, Some(s));
                            st.pairing_until = None;
                        });
                        set_flags(slot, |h| h.flags |= host_flags::BONDED | host_flags::ENCRYPTED);
                        log::line(format_args!("host {host}: bonded"));
                        event(EventKind::Paired, host, 0);
                    } else if !with(|s| s.hosts[slot].bonded()) {
                        // Paired without a bond (the host didn't ask for one): nothing to reconnect with, so not a host.
                        log::line(format_args!("host {host}: paired without a bond"));
                        event(EventKind::PairFailed, host, 0);
                        conn.raw().disconnect();
                    }
                }
                GattConnectionEvent::PairingFailed(_) => {
                    waiting_passkey = false;
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
                        log::line(format_args!("host {host}: pairing anew"));
                    } else {
                        // Only while pairing is open, as for any host the module doesn't know.
                        log::line(format_args!("host {host}: forgot the KVM, pairing is closed"));
                        conn.raw().disconnect();
                    }
                }
                GattConnectionEvent::Encrypted { .. } => {
                    set_flags(slot, |h| h.flags |= host_flags::ENCRYPTED);
                    let selected = with(|s| s.selected == host);
                    let _ = conn.raw().update_connection_params(stack, &params(selected)).await;
                }
                GattConnectionEvent::Gatt { event: e } => {
                    if let GattEvent::Write(w) = &e {
                        if Some(w.handle()) == kb_cccd {
                            let on = w.with_data(|_, d| d.first().is_some_and(|b| b & 1 != 0));
                            set_flags(slot, |h| {
                                if on {
                                    h.flags |= host_flags::SUBSCRIBED;
                                } else {
                                    h.flags &= !host_flags::SUBSCRIBED;
                                }
                            });
                        } else if w.handle() == server.hid.leds.handle {
                            let leds = w.with_data(|_, d| d.first().copied().unwrap_or(0));
                            link::send(&BtMsg::Output { host, id: report::KEYBOARD, data: &[leds] });
                        }
                    }
                    if let Ok(reply) = e.accept() {
                        reply.send().await;
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
                let encrypted = with(|s| s.hosts[slot].flags & host_flags::ENCRYPTED != 0);
                if !encrypted {
                    continue;
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
                let selected = with(|s| s.selected == host);
                let _ = conn.raw().update_connection_params(stack, &params(selected)).await;
            }
            Either4::Fourth(value) => {
                waiting_passkey = false;
                let _ = match value {
                    Some(v) => conn.raw().pass_key_input(v),
                    None => conn.raw().pass_key_cancel(),
                };
            }
        }
    }
    // Only the slot's current connection takes its state with it, and lets the slot go.
    if current() {
        with(|s| s.live[slot] = false);
        set_flags(slot, |h| {
            h.flags &= !(host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED);
            h.interval = 0;
            h.latency = 0;
        });
    }
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
                if (1..=MAX_HOSTS).contains(&slot) && with(|s| s.selected == host) {
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
            BtMsg::Select { host } => {
                let old = with(|s| core::mem::replace(&mut s.selected, host));
                for slot in 0..MAX_HOSTS {
                    let n = slot as u8 + 1;
                    if n != old && n != host {
                        continue;
                    }
                    let (connected, keep) = with(|s| (s.hosts[slot].connected(), wanted(s, slot)));
                    if connected && !keep {
                        CTL[slot].signal(Ctl::Disconnect);
                    } else if connected {
                        CTL[slot].signal(Ctl::Params);
                    }
                }
                ADVERTISE.signal(());
            }
            BtMsg::Pair { on, seconds } => {
                with(|s| s.pairing_until = on.then(|| Instant::now() + Duration::from_secs(seconds as u64)));
                event(if on { EventKind::PairingOpen } else { EventKind::PairingClosed }, 0, seconds as u32);
                // The advertising loop closes it when the time is up.
                ADVERTISE.signal(());
            }
            BtMsg::Forget { host } => {
                let picked = |slot: usize| host == 0xFF || host as usize == slot + 1;
                // One flash save for all of them, none for slots already free (bonds.rs, "Wear").
                let removed = with(|s| s.bonds.clear(picked));
                // Out of the stack's keys too: otherwise the forgotten host, which still has its keys, would get back in
                // encrypted, without a passkey, the next time pairing is open (until the module restarts).
                for b in removed.iter().flatten() {
                    let _ = stack.remove_bond_information(b.identity());
                }
                for slot in (0..MAX_HOSTS).filter(|&i| picked(i)) {
                    set_flags(slot, |h| h.flags &= !host_flags::BONDED);
                    CTL[slot].signal(Ctl::Disconnect);
                }
                ADVERTISE.signal(());
            }
            BtMsg::Passkey { value, .. } => PASSKEY.signal(value),
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
            }
            _ => {}
        }
    }
}
