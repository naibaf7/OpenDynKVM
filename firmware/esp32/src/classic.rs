//! The ESP32's Bluetooth Classic host (software.md §4.13): gamepads that pair only over Classic (the 5 series of the
//! symmetric-stick pads), in the same device slots as the LE devices (`devices::classic`), their reports to the
//! RP2354B as an LE device's are.
//!
//! - **Its share of the controller:** [`crate::radio::Split`] hands it Classic's events and ACL data ([`take`]); it
//!   sends its commands and data through the same controller. It starts once the LE stack has set the controller up,
//!   sets the event mask again with Classic's events added, and turns on Secure Simple Pairing, inquiry with extended
//!   results, role switches, its class (a computer), its name, and page scan: a bonded pad reconnects by paging it.
//! - **Finding:** while the RP2354B looks for devices (`Scan`), inquiries run, and the HID peripherals that answer go
//!   to it as `Found`, their address with the Classic type (`ADDR_CLASSIC`).
//! - **Pairing** (`PairDevice` with such an address): connect, authenticate (Secure Simple Pairing, no input and no
//!   output: Just Works, as a pad has neither), the link key into a free device slot, encryption, then the HID
//!   channels (control, then interrupt).
//! - **Reconnecting:** a bonded pad pages the module (its home button): accepted, its link key given, and it opens the
//!   HID channels itself; one that hasn't started encryption within [`ENCRYPT_WAIT`] is asked to, and the module opens
//!   the channels itself if they haven't come within [`CHANNELS_WAIT`].
//! - **The pad:** once its interrupt channel is open, the module reads its feature report 5 (which switches it to its
//!   full reports) and lights its middle player light. Its reports go to the RP2354B in their USB form
//!   (`kvm_classic::pad`).

// bt-hci's `cmd!` checks a `defmt` feature of the crate it is used in.
#![allow(unexpected_cfgs)]

use core::cell::{Cell, RefCell};
use core::sync::atomic::{AtomicU32, Ordering};

use bt_hci::ControllerToHostPacket;
use bt_hci::cmd::SyncCmd;
use bt_hci::cmd::controller_baseband::SetEventMask;
use bt_hci::cmd::le::LeReadBufferSize;
use bt_hci::cmd::link_control::{
    AcceptConnectionRequest, AuthenticationRequested, Disconnect, Inquiry, InquiryCancel, IoCapabilityRequestReply,
    LinkKeyRequestNegativeReply, LinkKeyRequestReply, RejectConnectionRequest, SetConnectionEncryption,
    UserConfirmationRequestNegativeReply, UserConfirmationRequestReply,
};
use bt_hci::controller::Controller as _;
use bt_hci::data::{AclBroadcastFlag, AclPacket, AclPacketBoundary};
use bt_hci::param::{
    AuthenticationRequirements, BdAddr as HciAddr, ConnHandle, DisconnectReason, EventMask, IoCapability,
    OobDataPresent, RejectReason, Role,
};
use embassy_futures::select::{Either4, select4};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use heapless::{Deque, Vec};
use kvm_classic::hci::{self, BdAddr, Event, Route};
use kvm_classic::l2cap::{self, Commands, Happened, PSM_HID_CONTROL, PSM_HID_INTERRUPT, PSM_SDP, SIGNALING, Signaling};
use kvm_classic::{hid, pad, sdp};
use kvm_proto::ctrl::DevicePairing;
use kvm_proto::link::bt::{ADDR_CLASSIC, NAME_MAX, device_flags};

use crate::devices::classic as slots;
use crate::log;
use crate::radio::Hci;

/// Classic links at once: one per device slot, whatever the others hold (the controller's `bt_max_acl_conn`).
const LINKS: usize = kvm_proto::link::bt::MAX_DEVICES;
/// A pairing's whole time: Secure Simple Pairing waits for nobody, and a pad pairs in seconds.
const PAIR_TIMEOUT: Duration = Duration::from_secs(30);
/// A pad that reconnected and hasn't started encryption by then is asked to.
const ENCRYPT_WAIT: Duration = Duration::from_secs(2);
/// A pad whose HID channels haven't come by then (after encryption) gets them opened by the module.
const CHANNELS_WAIT: Duration = Duration::from_secs(2);
/// An inquiry's length, in 1.28 s: about 5 s, then [`INQUIRY_GAP`] without one, and so on while the RP2354B looks.
const INQUIRY_LEN: u8 = 4;
/// The radio left to the LE scan between two inquiries: the ESP32's controller gives an inquiry all of it, and an LE
/// device's advertising went unseen while back-to-back inquiries ran (2026-11-25). A pad in pairing mode stays
/// discoverable for minutes, so half the time finds it as well.
const INQUIRY_GAP: Duration = Duration::from_secs(5);
/// The General Inquiry Access Code's LAP (0x9E8B33).
const GIAC: [u8; 3] = [0x33, 0x8B, 0x9E];
/// ACL packets the module has in the controller at once on Classic links: its few (signaling, a request, the lights)
/// stay far below the controller's buffers, which the LE stack's credits don't count.
const IN_FLIGHT: u8 = 4;
const NONE: u16 = 0xFFFF;
/// What the module's SDP server says of it (`kvm_classic::sdp`): the KVM's USB vendor and product (pid.codes,
/// `kvm_core::presentation::KVM_VID`) and version.
const SDP: sdp::Server = sdp::Server { vendor: 0x1209, product: 0x0001, version: 0x0100 };
/// The biggest L2CAP frame the module sends: an SDP answer with its header.
const FRAME: usize = 4 + sdp::MAX_RESPONSE;

bt_hci::cmd! {
    /// Write Scan Enable: 2, page scan only (a bonded pad pages the module; nothing needs to find it).
    WriteScanEnable(CONTROL_BASEBAND, 0x001A) {
        Params = u8;
        Return = ();
    }
}

bt_hci::cmd! {
    /// Write Class of Device (3 bytes, least significant first).
    WriteClassOfDevice(CONTROL_BASEBAND, 0x0024) {
        Params = [u8; 3];
        Return = ();
    }
}

bt_hci::cmd! {
    /// Write Local Name (248 bytes, NUL-padded).
    WriteLocalName(CONTROL_BASEBAND, 0x0013) {
        Params = [u8; 248];
        Return = ();
    }
}

bt_hci::cmd! {
    /// Write Simple Pairing Mode: 1, on.
    WriteSimplePairingMode(CONTROL_BASEBAND, 0x0056) {
        Params = u8;
        Return = ();
    }
}

bt_hci::cmd! {
    /// Write Inquiry Mode: 2, results with RSSI or extended (with the device's name).
    WriteInquiryMode(CONTROL_BASEBAND, 0x0045) {
        Params = u8;
        Return = ();
    }
}

bt_hci::cmd! {
    /// Read Buffer Size: the controller's BR/EDR ACL buffers (LE's too on one without LE buffers of its own).
    ReadBufferSize(INFO_PARAMS, 0x0005) {
        Params = ();
        ReadBufferSizeReturn {
            acl_data_packet_length: u16,
            synchronous_data_packet_length: u8,
            total_num_acl_data_packets: u16,
            total_num_synchronous_data_packets: u16,
        }
    }
}

bt_hci::cmd! {
    /// Write Default Link Policy Settings: bit 0, role switches allowed.
    WriteDefaultLinkPolicySettings(LINK_POLICY, 0x000F) {
        Params = u16;
        Return = ();
    }
}

bt_hci::cmd! {
    /// Create Connection, its parameters as they go: the address, packet types, page scan repetition mode, a reserved
    /// byte, the clock offset, role switch allowed.
    CreateConnection(LINK_CONTROL, 0x0005) {
        Params = [u8; 13];
        Return = ();
    }
}

/// What the RP2354B asks of the Classic host (through `devices.rs`).
pub enum Ask {
    /// Look for devices (inquiries), or stop.
    Scan(bool),
    /// Pair the device at this address (type `ADDR_CLASSIC`).
    Pair([u8; 7]),
    /// Slot `n` was forgotten: its link goes.
    Drop(usize),
}

static ASK: Channel<CriticalSectionRawMutex, Ask, 4> = Channel::new();
static LE_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// One packet [`take`] took: an event (its code), or ACL data of a handle (`start`: the first piece of an L2CAP PDU).
struct Packet {
    event: Option<u8>,
    handle: u16,
    start: bool,
    data: Vec<u8, 255>,
}

static RX: Channel<CriticalSectionRawMutex, Packet, 12> = Channel::new();
/// The newest input report of each Classic link (its handle and L2CAP frame), which [`take`] keeps in place of
/// queueing it: a pad sends hundreds a second, only the newest counts, and while the executor was busy they filled
/// [`RX`] and pushed out the events (2026-11-25).
static INPUT: Mutex<CriticalSectionRawMutex, RefCell<[Option<(u16, Vec<u8, 255>)>; LINKS]>> =
    Mutex::new(RefCell::new([const { None }; LINKS]));
/// The packets the controller sent on Classic links (handle, count) since the task last looked: Number of Completed
/// Packets, counted by [`take`] as it comes, so that none is lost to a full [`RX`] (a lost one is a link that can't
/// send again).
static FREED: Mutex<CriticalSectionRawMutex, Cell<[(u16, u16); LINKS]>> = Mutex::new(Cell::new([(NONE, 0); LINKS]));
/// [`INPUT`] or [`FREED`] has something.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Packets [`RX`] had no room for, and when that was last logged (once a second at most).
static DROPPED: AtomicU32 = AtomicU32::new(0);
static DROP_LOGGED: Mutex<CriticalSectionRawMutex, Cell<Option<Instant>>> = Mutex::new(Cell::new(None));
/// Classic's connection handles, for [`take`] (which runs in the LE stack's reading).
static HANDLES: Mutex<CriticalSectionRawMutex, Cell<[u16; LINKS]>> = Mutex::new(Cell::new([NONE; LINKS]));

pub fn ask(a: Ask) {
    let _ = ASK.try_send(a);
}

/// The LE stack has set the controller up (`radio::Split`).
pub fn le_ready() {
    LE_READY.signal(());
}

fn classic_handle(h: u16) -> bool {
    HANDLES.lock(|c| c.get().contains(&h))
}

fn set_handle(from: u16, to: u16) {
    HANDLES.lock(|c| {
        let mut a = c.get();
        if let Some(x) = a.iter_mut().find(|x| **x == from) {
            *x = to;
        }
        c.set(a);
    });
}

fn queue(event: Option<u8>, handle: u16, start: bool, data: &[u8]) {
    let Ok(data) = Vec::from_slice(data) else { return };
    if RX.try_send(Packet { event, handle, start, data }).is_err() {
        let n = DROPPED.fetch_add(1, Ordering::Relaxed) + 1;
        let now = Instant::now();
        let due = DROP_LOGGED.lock(|c| {
            let due = c.get().is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
            if due {
                c.set(Some(now));
            }
            due
        });
        if due {
            log::line(format_args!("classic: busy, {n} packets dropped so far"));
        }
    }
}

/// A whole input report on one of a link's channels (DATA | INPUT), in one piece: kept as the link's newest
/// ([`INPUT`]).
fn keep_input(handle: u16, data: &[u8]) -> bool {
    let Some((cid, payload)) = l2cap::frame(data) else { return false };
    let whole = data.len() == 4 + u16::from_le_bytes([data[0], data[1]]) as usize;
    if !whole || cid < 0x0040 || payload.first() != Some(&hid::DATA_INPUT) {
        return false;
    }
    let Ok(frame) = Vec::from_slice(data) else { return false };
    let kept = INPUT.lock(|c| {
        let mut slots = c.borrow_mut();
        let at = slots.iter().position(|s| s.as_ref().is_some_and(|s| s.0 == handle));
        let Some(at) = at.or_else(|| slots.iter().position(Option::is_none)) else { return false };
        slots[at] = Some((handle, frame));
        true
    });
    if kept {
        WAKE.signal(());
    }
    kept
}

/// Counts a Number of Completed Packets event's Classic packets into [`FREED`].
fn count_freed(data: &[u8]) {
    let mut any = false;
    for (h, n) in hci::completed(data).filter(|&(h, _)| classic_handle(h)) {
        FREED.lock(|c| {
            let mut a = c.get();
            let at = a.iter().position(|e| e.0 == h).or_else(|| a.iter().position(|e| e.0 == NONE));
            if let Some(at) = at {
                a[at] = (h, a[at].1.saturating_add(n));
            }
            c.set(a);
        });
        any = true;
    }
    if any {
        WAKE.signal(());
    }
}

/// A packet from the controller: whether it is Classic's alone (then the LE stack doesn't get it). Classic's copy
/// goes to the Classic task.
pub fn take(p: &ControllerToHostPacket) -> bool {
    match p {
        ControllerToHostPacket::Event(e) => {
            let code = e.kind.0;
            let route = hci::route_event(code, e.data, classic_handle);
            if route == Route::Le {
                return false;
            }
            if code == hci::code::NUMBER_OF_COMPLETED_PACKETS {
                count_freed(e.data);
                return route == Route::Classic;
            }
            // A link's handle is Classic's from its Connection Complete on (its data may come before the task reads
            // that), and the LE stack's again from its Disconnection Complete on (the controller may give it to an LE
            // connection next).
            if let Some(Event::ConnectionComplete { status: 0, handle, link_type: 1, .. }) = hci::parse(code, e.data) {
                set_handle(NONE, handle);
            }
            if let Some(Event::Disconnected { handle, .. }) = hci::parse(code, e.data)
                && route == Route::Classic
            {
                set_handle(handle, NONE);
            }
            queue(Some(code), 0, false, e.data);
            route == Route::Classic
        }
        ControllerToHostPacket::Acl(a) => {
            let h = a.handle().raw();
            if !classic_handle(h) {
                return false;
            }
            let start = !matches!(a.boundary_flag(), AclPacketBoundary::Continuing);
            if !(start && keep_input(h, a.data())) {
                queue(None, h, start, a.data());
            }
            true
        }
        _ => false,
    }
}

/// `a` (HCI's order) as the RP2354B has addresses: the type, then the bytes most significant first.
fn wire(a: BdAddr) -> [u8; 7] {
    let mut w = [ADDR_CLASSIC, 0, 0, 0, 0, 0, 0];
    for i in 0..6 {
        w[1 + i] = a[5 - i];
    }
    w
}

fn bd(w: [u8; 7]) -> BdAddr {
    core::array::from_fn(|i| w[6 - i])
}

struct Link {
    handle: u16,
    addr: BdAddr,
    slot: usize,
    /// Being paired (the module connected to it), or reconnected (it paged the module).
    pairing: bool,
    sig: Signaling,
    rx: Vec<u8, 512>,
    encrypted: bool,
    ready: bool,
    /// When it connected, or encryption came on.
    since: Instant,
    auth_asked: bool,
    /// Since when [`IN_FLIGHT`] packets wait for the controller's confirmation, and whether that went to the log.
    full_since: Option<Instant>,
    full_logged: bool,
    /// The module opened (or asked to open) the HID channels.
    opened: bool,
    out_seq: u8,
    in_flight: u8,
}

/// A device that answered an inquiry: what connecting to it needs, and what it said.
struct Seen {
    addr: BdAddr,
    scan_mode: u8,
    clock_offset: u16,
    class: u32,
    name: heapless::String<NAME_MAX>,
}

struct Host {
    hci: &'static Hci,
    links: [Option<Link>; LINKS],
    /// A pairing under way: the device's address, its slot, its deadline.
    pairing: Option<(BdAddr, usize, Instant)>,
    /// A bonded device's connection accepted: its address and slot, until its Connection Complete.
    accepting: Option<(BdAddr, usize)>,
    looking: bool,
    inquiring: bool,
    /// When the next inquiry starts, while looking ([`INQUIRY_GAP`]).
    next_inquiry: Option<Instant>,
    seen: Vec<Seen, 8>,
    /// ACL packets waiting for room in the controller: the handle and the packet.
    out: Deque<(u16, Vec<u8, FRAME>), 8>,
}

fn ok<T, E>(what: &str, r: Result<T, E>) -> bool {
    if r.is_err() {
        log::line(format_args!("classic: {what} failed"));
    }
    r.is_ok()
}

impl Host {
    /// Runs command `c` (answered by Command Complete, or by Command Status for those that end in an event: bt-hci's
    /// controller takes either as the answer).
    async fn sync<C: SyncCmd>(&self, what: &str, c: C) -> bool
    where
        Hci: bt_hci::controller::ControllerCmdSync<C>,
    {
        ok(what, c.exec(self.hci).await)
    }

    async fn setup(&mut self) {
        // The LE stack's events (trouble-host's start), and Classic's.
        let mask = EventMask::new()
            .enable_le_meta(true)
            .enable_conn_request(true)
            .enable_conn_complete(true)
            .enable_hardware_error(true)
            .enable_disconnection_complete(true)
            .enable_encryption_change_v1(true)
            .enable_encryption_key_refresh_complete(true)
            .enable_inquiry_complete(true)
            .enable_inquiry_result(true)
            .enable_inquiry_result_with_rssi(true)
            .enable_ext_inquiry_result(true)
            .enable_authentication_complete(true)
            .enable_remote_name_request_complete(true)
            .enable_pin_code_request(true)
            .enable_link_key_request(true)
            .enable_link_key_notification(true)
            .enable_io_capability_request(true)
            .enable_io_capability_response(true)
            .enable_user_confirmation_request(true)
            .enable_user_passkey_request(true)
            .enable_simple_pairing_complete(true);
        self.sync("event mask", SetEventMask::new(mask)).await;
        self.sync("simple pairing", WriteSimplePairingMode::new(1)).await;
        self.sync("inquiry mode", WriteInquiryMode::new(2)).await;
        self.sync("link policy", WriteDefaultLinkPolicySettings::new(0x0001)).await;
        // Major class computer (1), minor desktop (1).
        self.sync("class", WriteClassOfDevice::new([0x04, 0x01, 0x00])).await;
        let mut name = [0u8; 248];
        name[..10].copy_from_slice(b"OpenDynKVM");
        self.sync("name", WriteLocalName::new(name)).await;
        if self.sync("page scan", WriteScanEnable::new(2)).await {
            log::line(format_args!("classic: up, page scan on"));
        }
        // The controller's buffers, BR/EDR and LE, which the two hosts share (LE's 0: it has none of its own).
        if let (Ok(b), Ok(le)) =
            (ReadBufferSize::new().exec(self.hci).await, LeReadBufferSize::new().exec(self.hci).await)
        {
            let (n, len) = (b.total_num_acl_data_packets, b.acl_data_packet_length);
            let (le_n, le_len) = (le.total_num_le_acl_data_packets, le.le_acl_data_packet_length);
            log::line(format_args!(
                "classic: controller buffers: BR/EDR {n} of {len} bytes, LE {le_n} of {le_len} bytes"
            ));
        }
    }

    fn link(&mut self, handle: u16) -> Option<&mut Link> {
        self.links.iter_mut().flatten().find(|l| l.handle == handle)
    }

    fn link_at(&mut self, addr: BdAddr) -> Option<&mut Link> {
        self.links.iter_mut().flatten().find(|l| l.addr == addr)
    }

    /// An L2CAP frame for `handle`'s link, queued.
    fn l2cap(&mut self, handle: u16, cid: u16, payload: &[u8]) {
        let mut pdu = [0u8; FRAME];
        let Some(n) = l2cap::write_frame(cid, payload, &mut pdu) else { return };
        if self.out.push_back((handle, Vec::from_slice(&pdu[..n]).unwrap_or_default())).is_err() {
            log::line(format_args!("classic: send queue full"));
        }
    }

    fn signal(&mut self, handle: u16, cmds: Commands) {
        for c in cmds {
            self.l2cap(handle, SIGNALING, &c);
        }
    }

    /// Sends what waits while the controller has room for it.
    async fn flush(&mut self) {
        while let Some((handle, _)) = self.out.front() {
            let handle = *handle;
            let Some(l) = self.link(handle) else {
                self.out.pop_front();
                continue;
            };
            if l.in_flight >= IN_FLIGHT {
                // A link whose packets the controller doesn't confirm: said once a stall, after a second.
                let since = *l.full_since.get_or_insert_with(Instant::now);
                if !l.full_logged && since.elapsed() >= Duration::from_secs(1) {
                    l.full_logged = true;
                    log::line(format_args!(
                        "device {}: {IN_FLIGHT} packets unconfirmed for a second, {} waiting",
                        l.slot + 1,
                        self.out.len()
                    ));
                }
                break;
            }
            l.full_since = None;
            l.full_logged = false;
            l.in_flight += 1;
            let Some((_, pdu)) = self.out.pop_front() else { break };
            let p = AclPacket::new(
                ConnHandle::new(handle),
                AclPacketBoundary::FirstFlushable,
                AclBroadcastFlag::PointToPoint,
                &pdu,
            );
            ok("ACL", self.hci.write_acl_data(&p).await);
        }
    }

    async fn inquire(&mut self) {
        if !self.inquiring && self.sync("inquiry", Inquiry::new(GIAC, INQUIRY_LEN, 0)).await {
            self.inquiring = true;
        }
    }

    fn fail(&mut self, why: &str) {
        let Some((addr, slot, _)) = self.pairing.take() else { return };
        log::line(format_args!("classic: pairing {:02x?} failed: {why}", wire(addr)));
        slots::release(slot);
        slots::stage(DevicePairing::Failed, Some(slot));
    }

    async fn ask(&mut self, a: Ask) {
        match a {
            Ask::Scan(on) => {
                self.looking = on;
                self.next_inquiry = None;
                if on {
                    self.seen.clear();
                    self.inquire().await;
                } else if self.inquiring {
                    self.sync("inquiry cancel", InquiryCancel::new()).await;
                    self.inquiring = false;
                }
            }
            Ask::Pair(w) => {
                if self.pairing.is_some() {
                    return;
                }
                let Some(slot) = slots::take_slot() else {
                    log::line(format_args!("classic: no slot free to pair into"));
                    slots::stage(DevicePairing::Failed, None);
                    return;
                };
                if self.inquiring {
                    self.sync("inquiry cancel", InquiryCancel::new()).await;
                    self.inquiring = false;
                }
                self.looking = false;
                self.next_inquiry = None;
                let addr = bd(w);
                self.pairing = Some((addr, slot, Instant::now() + PAIR_TIMEOUT));
                slots::stage(DevicePairing::Connecting, Some(slot));
                let (scan_mode, offset) = self
                    .seen
                    .iter()
                    .find(|s| s.addr == addr)
                    .map_or((1, 0), |s| (s.scan_mode, s.clock_offset | 0x8000));
                let mut p = [0u8; 13];
                p[..6].copy_from_slice(&addr);
                // DM1, DH1, DM3, DH3, DM5, DH5.
                p[6..8].copy_from_slice(&0xCC18u16.to_le_bytes());
                p[8] = scan_mode;
                p[10..12].copy_from_slice(&offset.to_le_bytes());
                p[12] = 1;
                if !self.sync("connect", CreateConnection::new(p)).await {
                    self.fail("connect refused");
                }
            }
            Ask::Drop(slot) => {
                let handle = self.links.iter().flatten().find(|l| l.slot == slot).map(|l| l.handle);
                if let Some(h) = handle {
                    let reason = DisconnectReason::RemoteUserTerminatedConn;
                    self.sync("disconnect", Disconnect::new(ConnHandle::new(h), reason)).await;
                }
            }
        }
    }

    /// What [`take`] kept: the links' sent packets ([`FREED`]) and their newest input reports ([`INPUT`]).
    fn woken(&mut self) {
        let freed = FREED.lock(|c| c.replace([(NONE, 0); LINKS]));
        for (h, n) in freed.into_iter().filter(|e| e.0 != NONE) {
            if let Some(l) = self.link(h) {
                l.in_flight = l.in_flight.saturating_sub(n.min(255) as u8);
            }
        }
        let input = INPUT.lock(|c| c.replace([const { None }; LINKS]));
        for (h, frame) in input.into_iter().flatten() {
            self.pdu(h, &frame);
        }
    }

    async fn event(&mut self, code: u8, data: &[u8]) {
        let Some(e) = hci::parse(code, data) else {
            // The link's other events, for the log: role change (0x12), mode change (0x14, sniff), max slots (0x1B),
            // remote features (0x0B), link supervision timeout (0x38).
            if matches!(code, 0x0B | 0x12 | 0x14 | 0x1B | 0x38) {
                log::line(format_args!("classic: event {code:#04x} {:02x?}", &data[..data.len().min(12)]));
            }
            return;
        };
        match e {
            Event::InquiryComplete => {
                self.inquiring = false;
                if self.looking && slots::looking() {
                    self.next_inquiry = Some(Instant::now() + INQUIRY_GAP);
                } else {
                    self.looking = false;
                }
            }
            Event::Found { addr, class, rssi, name, scan_mode, clock_offset } => {
                let appearance = hci::appearance(class);
                if appearance == 0 {
                    return;
                }
                let i = match self.seen.iter().position(|s| s.addr == addr) {
                    Some(i) => i,
                    None => {
                        if self.seen.is_full() {
                            self.seen.remove(0);
                        }
                        let s = Seen { addr, scan_mode, clock_offset, class, name: heapless::String::new() };
                        let _ = self.seen.push(s);
                        self.seen.len() - 1
                    }
                };
                let s = &mut self.seen[i];
                (s.scan_mode, s.clock_offset, s.class) = (scan_mode, clock_offset, class);
                if let Some(n) = name {
                    s.name = kvm_proto::ctrl::bt_device_name(n);
                }
                slots::found(wire(addr), rssi, appearance, &s.name);
            }
            Event::ConnectionRequest { addr, link_type, .. } => {
                let free = self.links.iter().any(Option::is_none);
                let bonded = slots::bonded(wire(addr)).map(|(slot, _)| slot);
                let target = self.pairing.filter(|p| p.0 == addr).map(|p| p.1);
                match bonded.or(target) {
                    Some(slot) if link_type == 1 && free && self.accepting.is_none() => {
                        log::line(format_args!("device {}: pages the module, accepted", slot + 1));
                        self.accepting = Some((addr, slot));
                        let c = AcceptConnectionRequest::new(HciAddr::new(addr), Role::Central);
                        self.sync("accept", c).await;
                    }
                    other => {
                        log::line(format_args!(
                            "classic: {:02x?} pages the module, refused (slot {other:?}, link type {link_type}, \
                             a link free {free}, accepting {})",
                            wire(addr),
                            self.accepting.is_some()
                        ));
                        let c = RejectConnectionRequest::new(HciAddr::new(addr), RejectReason::UnacceptableBdAddr);
                        self.sync("reject", c).await;
                    }
                }
            }
            Event::ConnectionComplete { status, handle, addr, .. } => {
                let pairing = self.pairing.filter(|p| p.0 == addr);
                let accepted = self.accepting.take().filter(|a| a.0 == addr);
                let Some(slot) = pairing.map(|p| p.1).or(accepted.map(|a| a.1)) else {
                    if status == 0 {
                        let reason = DisconnectReason::RemoteUserTerminatedConn;
                        self.sync("disconnect", Disconnect::new(ConnHandle::new(handle), reason)).await;
                    }
                    return;
                };
                if status != 0 {
                    if pairing.is_some() {
                        self.fail("not reached");
                    } else {
                        slots::release(slot);
                    }
                    return;
                }
                let Some(place) = self.links.iter_mut().find(|l| l.is_none()) else { return };
                *place = Some(Link {
                    handle,
                    addr,
                    slot,
                    pairing: pairing.is_some(),
                    sig: Signaling::new(),
                    rx: Vec::new(),
                    encrypted: false,
                    ready: false,
                    since: Instant::now(),
                    auth_asked: pairing.is_some(),
                    opened: false,
                    out_seq: 0,
                    in_flight: 0,
                    full_since: None,
                    full_logged: false,
                });
                slots::hold(slot);
                slots::flags(slot, device_flags::CONNECTED, 0);
                log::line(format_args!("device {}: connected over Classic", slot + 1));
                if pairing.is_some() {
                    slots::stage(DevicePairing::Pairing, Some(slot));
                    self.sync("authenticate", AuthenticationRequested::new(ConnHandle::new(handle))).await;
                }
            }
            Event::Disconnected { handle, reason } => {
                let Some(place) = self.links.iter_mut().find(|l| l.as_ref().is_some_and(|l| l.handle == handle)) else {
                    return;
                };
                let Some(l) = place.take() else { return };
                log::line(format_args!("device {}: disconnected ({reason:#04x})", l.slot + 1));
                if self.pairing.is_some_and(|p| p.0 == l.addr) {
                    self.fail("the link went");
                } else {
                    slots::release(l.slot);
                }
                self.out.retain(|(h, _)| *h != handle);
            }
            Event::LinkKeyRequest { addr } => {
                // A pairing goes without its old key: the device has none for this pairing either.
                let key =
                    slots::bonded(wire(addr)).map(|(_, k)| k).filter(|_| self.pairing.is_none_or(|p| p.0 != addr));
                log::line(format_args!(
                    "classic: {:02x?} asks for its link key: {}",
                    wire(addr),
                    if key.is_some() { "given" } else { "none" }
                ));
                match key {
                    Some(k) => self.sync("link key", LinkKeyRequestReply::new(HciAddr::new(addr), k)).await,
                    None => self.sync("no link key", LinkKeyRequestNegativeReply::new(HciAddr::new(addr))).await,
                };
            }
            Event::IoCapabilityRequest { addr } => {
                let c = IoCapabilityRequestReply::new(
                    HciAddr::new(addr),
                    IoCapability::NoInputNoOutput,
                    OobDataPresent::NotPresent,
                    AuthenticationRequirements::MitmNotRequiredGeneralBonding,
                );
                self.sync("IO capability", c).await;
            }
            Event::UserConfirmationRequest { addr, .. } => {
                // Just Works, for the device being paired only.
                log::line(format_args!(
                    "classic: {:02x?} pairs anew: {}",
                    wire(addr),
                    if self.pairing.is_some_and(|p| p.0 == addr) { "confirmed" } else { "refused (not being paired)" }
                ));
                if self.pairing.is_some_and(|p| p.0 == addr) {
                    self.sync("confirm", UserConfirmationRequestReply::new(HciAddr::new(addr))).await;
                } else {
                    self.sync("refuse", UserConfirmationRequestNegativeReply::new(HciAddr::new(addr))).await;
                }
            }
            Event::LinkKeyNotification { addr, key, kind } => {
                let Some((_, slot, _)) = self.pairing.filter(|p| p.0 == addr) else { return };
                let (name, appearance) = self
                    .seen
                    .iter()
                    .find(|s| s.addr == addr)
                    .map_or((heapless::String::new(), 0x03C4), |s| (s.name.clone(), hci::appearance(s.class)));
                let name = if name.is_empty() { "Gamepad" } else { name.as_str() };
                slots::bond(slot, wire(addr), key, kind == 5 || kind == 8, name, appearance);
                log::line(format_args!("device {}: paired over Classic", slot + 1));
            }
            Event::AuthenticationComplete { status, handle } => {
                if status == 0 {
                    let c = SetConnectionEncryption::new(ConnHandle::new(handle), true);
                    self.sync("encrypt", c).await;
                    return;
                }
                let Some((slot, pairing)) = self.link(handle).map(|l| (l.slot, l.pairing)) else { return };
                log::line(format_args!("device {}: authentication failed ({status:#04x})", slot + 1));
                if pairing {
                    self.fail("authentication");
                }
                let reason = DisconnectReason::AuthenticationFailure;
                self.sync("disconnect", Disconnect::new(ConnHandle::new(handle), reason)).await;
            }
            Event::EncryptionChange { status, handle, on } => {
                let Some(l) = self.link(handle) else { return };
                log::line(format_args!(
                    "device {}: encryption {} ({status:#04x})",
                    l.slot + 1,
                    if on { "on" } else { "off" }
                ));
                if status != 0 || !on {
                    return;
                }
                l.encrypted = true;
                l.since = Instant::now();
                let (slot, pairing) = (l.slot, l.pairing);
                slots::flags(slot, device_flags::ENCRYPTED, 0);
                if pairing {
                    slots::stage(DevicePairing::Reading, Some(slot));
                    self.open_channels(handle);
                }
            }
            Event::PinCodeRequest { addr } | Event::UserPasskeyRequest { addr } => {
                // Legacy PINs and passkeys: not for a pad.
                log::line(format_args!("classic: {:02x?} asks for a PIN or passkey: refused", wire(addr)));
                let reason = DisconnectReason::AuthenticationFailure;
                if let Some(h) = self.link_at(addr).map(|l| l.handle) {
                    self.sync("disconnect", Disconnect::new(ConnHandle::new(h), reason)).await;
                }
            }
            Event::SimplePairingComplete { status, addr } if status != 0 => {
                log::line(format_args!("classic: pairing {:02x?} failed ({status:#04x})", wire(addr)));
            }
            _ => {}
        }
    }

    /// The module opens the HID channels (control first, interrupt once that is open).
    fn open_channels(&mut self, handle: u16) {
        let Some(l) = self.link(handle) else { return };
        if l.opened {
            return;
        }
        l.opened = true;
        let req = l.sig.connect(PSM_HID_CONTROL);
        if let Some(c) = req {
            self.l2cap(handle, SIGNALING, &c);
        }
    }

    /// An L2CAP PDU of `handle`'s link, whole.
    fn pdu(&mut self, handle: u16, pdu: &[u8]) {
        let Some((cid, payload)) = l2cap::frame(pdu) else { return };
        let Some(l) = self.link(handle) else { return };
        if cid == SIGNALING {
            // Its channel requests (code 2: PSM, source CID), for the log.
            if payload.len() >= 6 && payload[0] == 0x02 {
                let psm = u16::from_le_bytes([payload[4], payload[5]]);
                log::line(format_args!(
                    "device {}: opens PSM {psm:#06x}{}",
                    l.slot + 1,
                    if l.encrypted { "" } else { " (not encrypted yet)" }
                ));
            }
            let mut out = Commands::new();
            let happened = l.sig.receive(payload, &mut out);
            self.signal(handle, out);
            for h in happened {
                self.channels(handle, h);
            }
            return;
        }
        let slot = l.slot;
        // SDP, which a pad that reconnects asks first: answered on its channel.
        if let Some(sdp) = l.sig.channel(PSM_SDP).filter(|c| c.local == cid).copied() {
            let mut answer = [0u8; sdp::MAX_RESPONSE];
            if let Some(n) = SDP.respond(payload, &mut answer) {
                log::line(format_args!(
                    "device {}: SDP request {:#04x}, answer {:#04x} ({n} bytes)",
                    slot + 1,
                    payload.first().copied().unwrap_or(0),
                    answer[0]
                ));
                self.l2cap(handle, sdp.remote, &answer[..n]);
            }
            return;
        }
        let interrupt = l.sig.channel(PSM_HID_INTERRUPT).filter(|c| c.open()).map(|c| c.local);
        if Some(cid) == interrupt
            && let Some(usb) = hid::input(payload).and_then(pad::input)
        {
            slots::input(slot, 1, &usb);
        }
    }

    fn channels(&mut self, handle: u16, h: Happened) {
        let Some(l) = self.link(handle) else { return };
        let slot = l.slot;
        match h {
            // SDP's channel: nothing to do with the pad's readiness.
            Happened::Opened(PSM_SDP) | Happened::Closed(PSM_SDP) | Happened::Refused(PSM_SDP) => {}
            Happened::Opened(PSM_HID_CONTROL) => {
                // The module opened control: interrupt next.
                if l.opened
                    && l.sig.channel(PSM_HID_INTERRUPT).is_none()
                    && let Some(c) = l.sig.connect(PSM_HID_INTERRUPT)
                {
                    self.l2cap(handle, SIGNALING, &c);
                }
            }
            Happened::Opened(_) => {
                // Interrupt open: the pad is ready. Its full reports, its lights.
                l.ready = true;
                let pairing = core::mem::replace(&mut l.pairing, false);
                let control = l.sig.channel(PSM_HID_CONTROL).map(|c| c.remote);
                let interrupt = l.sig.channel(PSM_HID_INTERRUPT).map(|c| c.remote).unwrap_or(0);
                let seq = l.out_seq;
                l.out_seq = l.out_seq.wrapping_add(2);
                if let Some(c) = control {
                    self.l2cap(handle, c, &pad::ASK_FULL);
                }
                for (i, o) in [
                    pad::Output { fade_out: true, ..pad::Output::default() },
                    pad::Output { player: 0x04, rgb: [0, 0, 40], fade_out: false },
                ]
                .iter()
                .enumerate()
                {
                    let mut msg = [0u8; 1 + pad::BT_REPORT_LEN];
                    msg[0] = hid::DATA_OUTPUT;
                    msg[1..].copy_from_slice(&pad::output(seq.wrapping_add(i as u8), o));
                    self.l2cap(handle, interrupt, &msg);
                }
                slots::flags(slot, device_flags::READY, 0);
                if pairing {
                    self.pairing = None;
                    slots::stage(DevicePairing::Paired, Some(slot));
                }
                log::line(format_args!("device {}: ready", slot + 1));
            }
            Happened::Closed(psm) | Happened::Refused(psm) => {
                log::line(format_args!(
                    "device {}: PSM {psm:#06x} {}",
                    slot + 1,
                    if matches!(h, Happened::Refused(_)) { "refused" } else { "closed" }
                ));
                l.ready = false;
                slots::flags(slot, 0, device_flags::READY);
                if matches!(h, Happened::Refused(_)) && self.pairing.is_some_and(|p| p.1 == slot) {
                    self.fail("its HID channels refused");
                }
            }
        }
    }

    /// ACL data of `handle`, put together into L2CAP PDUs.
    fn acl(&mut self, handle: u16, start: bool, data: &[u8]) {
        let Some(l) = self.link(handle) else { return };
        if start {
            l.rx.clear();
        }
        if l.rx.extend_from_slice(data).is_err() {
            l.rx.clear();
            return;
        }
        let want = l.rx.get(..2).map_or(usize::MAX, |b| 4 + u16::from_le_bytes([b[0], b[1]]) as usize);
        if l.rx.len() >= want {
            let pdu = core::mem::take(&mut l.rx);
            self.pdu(handle, &pdu[..want]);
        }
    }

    /// Timeouts: a pairing that takes too long; a reconnected pad that doesn't encrypt or open its channels. And the
    /// next inquiry while looking.
    async fn tick(&mut self) {
        if self.next_inquiry.is_some_and(|t| Instant::now() >= t) {
            self.next_inquiry = None;
            if self.looking && slots::looking() {
                self.inquire().await;
            } else {
                self.looking = false;
            }
        }
        if self.pairing.is_some_and(|p| Instant::now() >= p.2) {
            let addr = self.pairing.map(|p| p.0);
            self.fail("it took too long");
            if let Some(h) = addr.and_then(|a| self.link_at(a)).map(|l| l.handle) {
                let reason = DisconnectReason::RemoteUserTerminatedConn;
                self.sync("disconnect", Disconnect::new(ConnHandle::new(h), reason)).await;
            }
        }
        let mut auth = None;
        let mut open = None;
        for l in self.links.iter_mut().flatten().filter(|l| !l.pairing) {
            let waited = l.since.elapsed();
            if !l.encrypted && !l.auth_asked && waited >= ENCRYPT_WAIT {
                l.auth_asked = true;
                auth = Some(l.handle);
            } else if l.encrypted && !l.ready && !l.opened && waited >= CHANNELS_WAIT {
                open = Some(l.handle);
            }
        }
        if let Some(h) = auth {
            self.sync("authenticate", AuthenticationRequested::new(ConnHandle::new(h))).await;
        }
        if let Some(h) = open {
            self.open_channels(h);
        }
    }

    /// Something waits for time: a pairing, a link that isn't ready, the next inquiry.
    fn waiting(&self) -> bool {
        self.pairing.is_some() || self.next_inquiry.is_some() || self.links.iter().flatten().any(|l| !l.ready)
    }
}

#[embassy_executor::task]
pub async fn run(hci: &'static Hci) {
    LE_READY.wait().await;
    let mut host = Host {
        hci,
        links: [const { None }; LINKS],
        pairing: None,
        accepting: None,
        looking: false,
        inquiring: false,
        next_inquiry: None,
        seen: Vec::new(),
        out: Deque::new(),
    };
    host.setup().await;
    loop {
        let waiting = host.waiting();
        let tick = async { if waiting { Timer::after_millis(250).await } else { core::future::pending().await } };
        match select4(RX.receive(), WAKE.wait(), ASK.receive(), tick).await {
            Either4::First(p) => match p.event {
                Some(code) => host.event(code, &p.data).await,
                None => host.acl(p.handle, p.start, &p.data),
            },
            Either4::Second(()) => host.woken(),
            Either4::Third(a) => host.ask(a).await,
            Either4::Fourth(()) => host.tick().await,
        }
        host.flush().await;
    }
}
