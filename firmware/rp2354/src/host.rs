//! USB host side (software.md §4.3): Hub-0 on the root port, one task per front port.
//!
//! The hub task enumerates whatever appears on Hub-0 ports 1–3 (Keyboard, Mouse, Other) and hands it to that port's
//! task, which reads every descriptor the engine needs, then streams the device's interrupt reports into the engine
//! and executes the engine's requests for the device (LED reports, forwarded control requests, OUT data). An ESP32-C3
//! board the engine wants as the Bluetooth module goes to `bt::dongle_task` instead (software.md §4.13).

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::select::{Either, Either3, select, select_slice, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use embassy_usb_driver::host::{DeviceEvent, PipeError, UsbHostAllocator, UsbHostController as _, UsbPipe, pipe};
use embassy_usb_driver::{Direction, EndpointAddress, EndpointInfo, EndpointType};
use embassy_usb_host::class::hub::{HubEvent, HubHandler};
use embassy_usb_host::handler::{EnumerationInfo, HandlerEvent};
use embassy_usb_host::{BusController, BusHandle, BusRoute, BusState, bus};
use heapless::Vec;
use kvm_core::engine::{CtrlData, MAX_CONFIG_DESC, MAX_REPORT_DESC, Probed};
use kvm_core::output::Report;
use kvm_core::usbdesc::{DeviceDesc, decode_string, dt, parse_config};
use kvm_proto::ctrl::{Pc, PortState, Speed};

use crate::dongle::{Dongle, UsbLine};
use crate::uartlog::uart_log;
use crate::usbh::{RpAlloc, RpHost};
use crate::{bt, kvm};

pub type Handle = BusHandle<'static, RpAlloc>;
type ControlPipe = <RpAlloc as UsbHostAllocator<'static>>::Pipe<pipe::Control, pipe::InOut>;
type InPipe = <RpAlloc as UsbHostAllocator<'static>>::Pipe<pipe::Interrupt, pipe::In>;
type OutPipe = <RpAlloc as UsbHostAllocator<'static>>::Pipe<pipe::Interrupt, pipe::Out>;

pub static BUS_STATE: BusState = BusState::new();
/// Hub-0 is enumerated and registered (for the self-test).
pub static HUB0_UP: AtomicBool = AtomicBool::new(false);

/// Something the engine wants done on a front-port device.
pub enum PortCmd {
    SetReport { iface: u8, report_type: u8, report_id: u8, data: Report },
    Out { iface: u8, data: Report },
    Control { pc: Pc, tag: u8, setup: [u8; 8], data: CtrlData },
}

pub static PORT_CMD: [Channel<CriticalSectionRawMutex, PortCmd, 8>; 3] = [const { Channel::new() }; 3];
static ATTACH: [Signal<CriticalSectionRawMutex, EnumerationInfo>; 3] = [const { Signal::new() }; 3];
static DETACH: [Signal<CriticalSectionRawMutex, ()>; 3] = [const { Signal::new() }; 3];

fn port_of(i: usize) -> kvm_proto::ctrl::Port {
    kvm_proto::ctrl::Port::from_index(i).unwrap_or(kvm_proto::ctrl::Port::Other)
}

pub type Root = BusController<'static, RpHost>;

/// Creates the bus: the root controller for [`hub_task`], handles for everyone else.
pub fn start(ctrl: RpHost) -> (Root, Handle) {
    bus(ctrl, &BUS_STATE)
}

#[embassy_executor::task]
pub async fn hub_task(mut root: Root, handle: Handle) -> ! {
    loop {
        let speed = root.wait_for_connection().await;
        let mut buf = [0u8; 256];
        let info = match handle.enumerate(BusRoute::Direct(speed), &mut buf).await {
            Ok((info, _)) => info,
            Err(e) => {
                warn!("Hub-0 enumeration failed: {}", defmt::Debug2Format(&e));
                // Hardware reset of Hub-0 (RESET_N low ≥ 1 µs, USB2514B datasheet), then try again; not while deep
                // standby holds it in reset.
                if !crate::power::HUB0_HELD.load(Ordering::Relaxed) {
                    kvm::with_board(|b| b.hub0_rst.set_low());
                    Timer::after_millis(10).await;
                    if !crate::power::HUB0_HELD.load(Ordering::Relaxed) {
                        kvm::with_board(|b| b.hub0_rst.set_high());
                    }
                }
                Timer::after_millis(500).await;
                root.controller_mut().bus_reset().await;
                continue;
            }
        };
        let mut hub = match HubHandler::<_, 4>::try_register(&handle, &info).await {
            Ok(h) => h,
            Err(e) => {
                warn!("Hub-0 is not a hub? {}", defmt::Debug2Format(&e));
                handle.free_address(info.device_address);
                continue;
            }
        };
        info!("Hub-0 up at address {}", info.device_address);
        HUB0_UP.store(true, Ordering::Relaxed);
        // The address each front-port device got, so that it is freed however the device goes.
        let mut addr = [0u8; 3];
        loop {
            match select(hub.wait_for_event(), root.wait_for_device_event()).await {
                Either::First(Ok(HandlerEvent::HandlerEvent(HubEvent::DeviceDetected { port, speed }))) => {
                    let i = port as usize;
                    if i >= 3 {
                        continue; // port 4 is strapped off
                    }
                    // A device that left and came back between two polls of Hub-0 is only reported as detected, never
                    // as removed: end the session still running on the port (its task waits for that, not for the
                    // new attach, and left the port probing for good: first board, 2026-11-12, a receiver during a PC
                    // switch), and free the address the device had.
                    DETACH[i].signal(());
                    if addr[i] != 0 {
                        handle.free_address(addr[i]);
                        addr[i] = 0;
                    }
                    kvm::with(|e| e.port_probing(port_of(i)));
                    let mut cfg = [0u8; MAX_CONFIG_DESC];
                    match hub.enumerate_port(&mut cfg, port, speed).await {
                        Ok((info, _)) => {
                            addr[i] = info.device_address;
                            ATTACH[i].signal(info);
                        }
                        Err(e) => {
                            warn!("port {}: enumeration failed: {}", i, defmt::Debug2Format(&e));
                            kvm::with(|e| e.port_detached(port_of(i)));
                            failed(i, "enumeration");
                        }
                    }
                }
                Either::First(Ok(HandlerEvent::HandlerEvent(HubEvent::DeviceRemoved { address, port }))) => {
                    let known = Some(port as usize).filter(|i| *i < 3).map(|i| {
                        DETACH[i].signal(());
                        core::mem::take(&mut addr[i])
                    });
                    if let Some(a) = address.map(|a| a.get()).or(known.filter(|&a| a != 0)) {
                        handle.free_address(a);
                    }
                }
                Either::First(Ok(_)) => {}
                Either::First(Err(e)) => {
                    // A status change the hub class doesn't handle (it handles connect and reset). It returns an error
                    // and leaves the change set, so the hub reports it on every poll and the ports after it in the
                    // bitmap are never looked at: devices coming back from passthrough stayed probing (first board,
                    // 2026-11-12). Clear what it leaves.
                    HUB0_ERRORS.fetch_add(1, Ordering::Relaxed);
                    warn!("Hub-0 event error: {}", defmt::Debug2Format(&e));
                    uart_log!("Hub-0: status change not handled ({:?}), clearing", e);
                    clear_hub_changes(&handle, &info).await;
                    Timer::after_millis(10).await;
                }
                Either::Second(DeviceEvent::Disconnected) => {
                    warn!("Hub-0 disconnected");
                    HUB0_UP.store(false, Ordering::Relaxed);
                    for d in &DETACH {
                        d.signal(());
                    }
                    for a in addr.into_iter().filter(|&a| a != 0) {
                        handle.free_address(a);
                    }
                    handle.free_address(info.device_address);
                    break;
                }
                Either::Second(_) => {}
            }
        }
    }
}

/// Enumerations or probes in a row that failed on each port; a good probe or a new attempt (`Output::Reprobe`) resets it.
pub static PROBE_FAILS: [AtomicU8; 3] = [const { AtomicU8::new(0) }; 3];
/// A failed port tries again this often before it is left empty: replugged on the data lines first, then once with
/// VBUS off for a second, for a device that needs a real power-off.
const PROBE_TRIES: u8 = 3;
/// For the diagnostics (`diag::id::HUB0_ERRORS`, `ENUM_FAILS`), since start.
pub static HUB0_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static ENUM_FAILS: AtomicU32 = AtomicU32::new(0);

/// A front-port device couldn't be enumerated or probed. The port is empty to the engine by now; plug it in anew so
/// that it attaches again, up to [`PROBE_TRIES`] times in a row.
fn failed(index: usize, what: &str) {
    ENUM_FAILS.fetch_add(1, Ordering::Relaxed);
    let fails = PROBE_FAILS[index].load(Ordering::Relaxed).saturating_add(1);
    PROBE_FAILS[index].store(fails, Ordering::Relaxed);
    uart_log!("port {}: {} failed ({} in a row)", index, what, fails);
    if fails + 1 < PROBE_TRIES {
        kvm::REPLUG[index].signal(());
    } else if fails < PROBE_TRIES {
        kvm::POWER_CYCLE[index].signal(());
    }
}

/// Hub class requests (USB 2.0 §11.24.2) and the change features the hub class leaves set.
const HUB_GET_STATUS: u8 = 0;
const HUB_CLEAR_FEATURE: u8 = 1;
const C_HUB_LOCAL_POWER: u16 = 0;
const C_HUB_OVER_CURRENT: u16 = 1;
const C_PORT_ENABLE: u16 = 17;
const C_PORT_SUSPEND: u16 = 18;
const C_PORT_OVER_CURRENT: u16 = 19;

/// After a status change the hub class couldn't handle: clears every hub and port change it leaves set (connect and
/// reset changes stay for it), and replugs a front port whose device is still connected but was disabled by the hub,
/// so that it attaches anew.
async fn clear_hub_changes(handle: &Handle, hub: &EnumerationInfo) {
    let ep0 = EndpointInfo {
        addr: EndpointAddress::from_parts(0, Direction::In),
        ep_type: EndpointType::Control,
        max_packet_size: hub.device_desc.max_packet_size0 as u16,
        interval_ms: 0,
    };
    let Ok(mut ctl) = handle.alloc_pipe::<pipe::Control, pipe::InOut>(hub.device_address, &ep0, hub.split()) else {
        return;
    };
    for f in [C_HUB_LOCAL_POWER, C_HUB_OVER_CURRENT] {
        let _ = ctl.control_out(&setup(0x20, HUB_CLEAR_FEATURE, f, 0, 0), &[]).await;
    }
    for port in 1..=4u16 {
        let mut st = [0u8; 4];
        let Ok(4) = ctl.control_in(&setup(0xA3, HUB_GET_STATUS, 0, port, 4), &mut st).await else { continue };
        let status = u16::from_le_bytes([st[0], st[1]]);
        let change = u16::from_le_bytes([st[2], st[3]]);
        for (bit, f) in [(1u16 << 1, C_PORT_ENABLE), (1 << 2, C_PORT_SUSPEND), (1 << 3, C_PORT_OVER_CURRENT)] {
            if change & bit != 0 {
                let _ = ctl.control_out(&setup(0x23, HUB_CLEAR_FEATURE, f, port, 0), &[]).await;
            }
        }
        // Connected, not enabled, and no connect change waiting for the hub class: nothing would ever enumerate it.
        let i = port as usize - 1;
        if i < 3 && status & 1 != 0 && status & 2 == 0 && change & 1 == 0 {
            uart_log!("Hub-0 port {}: connected but disabled, replugging it", port);
            kvm::REPLUG[i].signal(());
        }
    }
}

/// Probing reads into these per-port buffers (too large for task stacks).
static PROBED: [embassy_sync::mutex::Mutex<CriticalSectionRawMutex, Probed>; 3] =
    [const { embassy_sync::mutex::Mutex::new(Probed::EMPTY) }; 3];

fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
    let [v0, v1] = value.to_le_bytes();
    let [i0, i1] = index.to_le_bytes();
    let [l0, l1] = length.to_le_bytes();
    [request_type, request, v0, v1, i0, i1, l0, l1]
}

async fn get_descriptor(
    ctl: &mut ControlPipe,
    ty: u8,
    index: u8,
    lang: u16,
    buf: &mut [u8],
) -> Result<usize, PipeError> {
    let s = setup(0x80, 6, (ty as u16) << 8 | index as u16, lang, buf.len() as u16);
    ctl.control_in(&s, buf).await
}

async fn read_string(ctl: &mut ControlPipe, index: u8, lang: u16) -> heapless::String<64> {
    if index == 0 {
        return heapless::String::new();
    }
    let mut buf = [0u8; 255];
    match get_descriptor(ctl, 3, index, lang, &mut buf).await {
        Ok(n) => decode_string(&buf[..n]),
        Err(_) => heapless::String::new(),
    }
}

async fn probe(ctl: &mut ControlPipe, info: &EnumerationInfo, out: &mut Probed) -> Result<(), PipeError> {
    *out = Probed::EMPTY;
    let mut dev = [0u8; 18];
    get_descriptor(ctl, 1, 0, 0, &mut dev).await?;
    out.device = DeviceDesc::parse(&dev).unwrap_or_default();
    out.speed = Some(match info.speed() {
        embassy_usb_driver::Speed::Low => Speed::Low,
        embassy_usb_driver::Speed::Full => Speed::Full,
        embassy_usb_driver::Speed::High => Speed::High,
    });

    let mut cfg = [0u8; MAX_CONFIG_DESC];
    get_descriptor(ctl, 2, 0, 0, &mut cfg[..9]).await?;
    let total = (u16::from_le_bytes([cfg[2], cfg[3]]) as usize).min(MAX_CONFIG_DESC);
    let n = get_descriptor(ctl, 2, 0, 0, &mut cfg[..total]).await?;
    let _ = out.config.extend_from_slice(&cfg[..n]);

    let mut langs = [0u8; 8];
    let lang = match get_descriptor(ctl, 3, 0, 0, &mut langs).await {
        Ok(n) if n >= 4 => u16::from_le_bytes([langs[2], langs[3]]),
        _ => 0x0409,
    };
    out.manufacturer = read_string(ctl, out.device.i_manufacturer, lang).await;
    out.product = read_string(ctl, out.device.i_product, lang).await;
    out.serial = read_string(ctl, out.device.i_serial, lang).await;

    // Hub-0 runs at full speed, so a high-speed device is probed at full speed. Only a high-speed device has a device
    // qualifier; a full-speed one stalls (USB 2.0 §9.6.2). Linux and Windows ask the same, to say "could run faster".
    if out.speed == Some(Speed::Full) && out.device.usb >= 0x0200 {
        let mut q = [0u8; 10];
        let got = get_descriptor(ctl, dt::DEVICE_QUALIFIER, 0, 0, &mut q).await;
        out.high_speed = matches!(got, Ok(10) if q[1] == dt::DEVICE_QUALIFIER);
    }

    let Some(parsed) = parse_config(&cfg[..n]) else { return Ok(()) };
    for iface in parsed.interfaces.iter().filter(|i| i.is_hid()) {
        // SET_IDLE(0): report only on change (keyboards otherwise repeat at the idle rate).
        let _ = ctl.control_out(&setup(0x21, 0x0A, 0, iface.number as u16, 0), &[]).await;
        let len = iface.hid_report_len.unwrap_or(0) as usize;
        if len == 0 || len > MAX_REPORT_DESC {
            continue;
        }
        let mut rd = [0u8; MAX_REPORT_DESC];
        let s = setup(0x81, 6, 0x2200, iface.number as u16, len as u16);
        if let Ok(got) = ctl.control_in(&s, &mut rd[..len]).await {
            let _ = out.report_descs.push((iface.number, Vec::from_slice(&rd[..got]).unwrap_or_default()));
        }
    }
    Ok(())
}

#[embassy_executor::task(pool_size = 3)]
pub async fn port_task(index: usize, handle: Handle) -> ! {
    let port = port_of(index);
    loop {
        let info = ATTACH[index].wait().await;
        DETACH[index].reset();
        info!("port {}: device at address {}", index, info.device_address);
        run_device(index, &handle, &info).await;
        kvm::with(|e| e.port_detached(port));
        // Requests queued for the old device are void.
        while PORT_CMD[index].try_receive().is_ok() {}
    }
}

async fn run_device(index: usize, handle: &Handle, info: &EnumerationInfo) {
    let port = port_of(index);
    let ep0 = EndpointInfo {
        addr: EndpointAddress::from_parts(0, Direction::In),
        ep_type: EndpointType::Control,
        max_packet_size: info.device_desc.max_packet_size0 as u16,
        interval_ms: 0,
    };
    let Ok(mut ctl) = handle.alloc_pipe::<pipe::Control, pipe::InOut>(info.device_address, &ep0, info.split()) else {
        return;
    };

    // Probe, then let the engine decide what the port becomes.
    let config = {
        let mut probed = PROBED[index].lock().await;
        if let Err(e) = probe(&mut ctl, info, &mut probed).await {
            // The engine has the port as probing since Hub-0 reported the device. Waiting for it to detach left a
            // device that stays plugged in probing (a yellow LED) for good: first board, 2026-11-12, a receiver
            // switched from passthrough to mirrored. So say it's gone (`port_task` does, on return), and power-cycle
            // the port so that it attaches and is probed anew, a few times.
            warn!("port {}: probing failed: {}", index, defmt::Debug2Format(&e));
            failed(index, "probing");
            return;
        }
        PROBE_FAILS[index].store(0, Ordering::Relaxed);
        kvm::with(|e| e.port_attached(port, &probed));
        if kvm::with(|e| e.dongle_wanted(port)) {
            let serial = heapless::String::try_from(probed.serial.as_str()).unwrap_or_default();
            let line = kvm_core::bt::dongle_endpoints(&probed.device, &probed.config)
                .and_then(|eps| open_dongle(handle, info, eps));
            drop(probed);
            let handed = line.is_some_and(|line| bt::DONGLE.try_send(Dongle { port, line, serial }).is_ok());
            if !handed {
                kvm::with(|e| e.dongle_declined(port));
            }
            // The module, or (declined) an ordinary device the engine routed or left: nothing more to run here. Its
            // serial port has no HID interfaces.
            DETACH[index].wait().await;
            bt::DONGLE_GONE[index].signal(());
            return;
        }
        probed.config.clone()
    };
    if kvm::with(|e| e.port_info(port).state) != PortState::Hosted {
        // Passthrough (the router moves it away) or unsupported: nothing to run here.
        DETACH[index].wait().await;
        return;
    }

    let Some(parsed) = parse_config(&config) else { return };
    let mut ins: Vec<(u8, InPipe), 4> = Vec::new();
    let mut outs: Vec<(u8, OutPipe), 4> = Vec::new();
    for iface in parsed.interfaces.iter().filter(|i| i.is_hid() || i.is_xinput()) {
        let ep_info = |e: &kvm_core::usbdesc::EndpointDesc, dir| EndpointInfo {
            addr: EndpointAddress::from_parts(e.number() as usize, dir),
            ep_type: EndpointType::Interrupt,
            max_packet_size: e.max_packet,
            interval_ms: e.interval,
        };
        if let Some(e) = iface.in_ep() {
            match handle.alloc_pipe::<pipe::Interrupt, pipe::In>(
                info.device_address,
                &ep_info(e, Direction::In),
                info.split(),
            ) {
                Ok(p) => {
                    let _ = ins.push((iface.number, p));
                }
                Err(_) => warn!("port {}: no interrupt slot left for interface {}", index, iface.number),
            }
        }
        if let Some(e) = iface.out_ep()
            && let Ok(p) = handle.alloc_pipe::<pipe::Interrupt, pipe::Out>(
                info.device_address,
                &ep_info(e, Direction::Out),
                info.split(),
            )
        {
            let _ = outs.push((iface.number, p));
        }
    }

    let mut bufs = [[0u8; 64]; 4];
    loop {
        let event = {
            let mut reads: Vec<_, 4> = ins
                .iter_mut()
                .zip(bufs.iter_mut())
                .map(|((iface, p), buf)| async move { (*iface, p.request_in(buf).await) })
                .collect();
            // SAFETY: `reads` is not moved while pinned; the futures live in it until this block ends.
            let reads = unsafe { core::pin::Pin::new_unchecked(reads.as_mut_slice()) };
            let any_read =
                async { if reads.is_empty() { core::future::pending().await } else { select_slice(reads).await } };
            match select3(any_read, PORT_CMD[index].receive(), DETACH[index].wait()).await {
                Either3::First(((iface, r), slot)) => Event::Report(iface, r, slot),
                Either3::Second(c) => Event::Cmd(c),
                Either3::Third(()) => return,
            }
        };
        match event {
            Event::Report(iface, Ok(n), slot) => {
                let data = &bufs[slot][..n.min(64)];
                let now = Instant::now().as_millis() as u32;
                kvm::with(|e| e.device_report(port, iface, data, now));
            }
            Event::Report(_, Err(PipeError::Disconnected), _) => return,
            Event::Report(iface, Err(e), _) => {
                warn!("port {} iface {}: {}", index, iface, defmt::Debug2Format(&e));
                Timer::after_millis(10).await;
            }
            Event::Cmd(PortCmd::SetReport { iface, report_type, report_id, data }) => {
                let s =
                    setup(0x21, 0x09, (report_type as u16) << 8 | report_id as u16, iface as u16, data.len() as u16);
                let _ = ctl.control_out(&s, &data).await;
            }
            Event::Cmd(PortCmd::Out { iface, data }) => {
                if let Some((_, p)) = outs.iter_mut().find(|(i, _)| *i == iface) {
                    let _ = p.request_out(&data, false).await;
                }
            }
            Event::Cmd(PortCmd::Control { pc, tag, setup, data }) => {
                let reply = if setup[0] & 0x80 != 0 {
                    let mut buf = [0u8; kvm_core::engine::MAX_CTRL_DATA];
                    let len = (u16::from_le_bytes([setup[6], setup[7]]) as usize).min(buf.len());
                    ctl.control_in(&setup, &mut buf[..len]).await.ok().map(|n| (buf, n))
                } else {
                    ctl.control_out(&setup, &data).await.ok().map(|_| ([0u8; kvm_core::engine::MAX_CTRL_DATA], 0))
                };
                kvm::with(|e| e.device_ctrl_reply(pc, tag, port, setup, reply.as_ref().map(|(b, n)| &b[..*n])));
            }
        }
    }
}

/// Pipes to an ESP32-C3's USB serial port: its own control pipe (DTR/RTS) and the CDC data interface's bulk endpoints.
fn open_dongle(handle: &Handle, info: &EnumerationInfo, eps: kvm_core::bt::DongleEps) -> Option<UsbLine> {
    let ep0 = EndpointInfo {
        addr: EndpointAddress::from_parts(0, Direction::In),
        ep_type: EndpointType::Control,
        max_packet_size: info.device_desc.max_packet_size0 as u16,
        interval_ms: 0,
    };
    // The bulk endpoints on interrupt slots, polled every millisecond (`usbh`: EPX stays for control transfers).
    let data = |n: u8, dir| EndpointInfo {
        addr: EndpointAddress::from_parts(n as usize, dir),
        ep_type: EndpointType::Interrupt,
        max_packet_size: eps.mps,
        interval_ms: 1,
    };
    let addr = info.device_address;
    let ctl = handle.alloc_pipe::<pipe::Control, pipe::InOut>(addr, &ep0, info.split()).ok()?;
    let rx =
        handle.alloc_pipe::<pipe::Interrupt, pipe::In>(addr, &data(eps.ep_in, Direction::In), info.split()).ok()?;
    let tx =
        handle.alloc_pipe::<pipe::Interrupt, pipe::Out>(addr, &data(eps.ep_out, Direction::Out), info.split()).ok()?;
    Some(UsbLine::new(ctl, rx, tx, eps.comm))
}

// One per loop iteration on the stack; boxing isn't available and the size is harmless.
#[allow(clippy::large_enum_variant)]
enum Event {
    /// Interface, result, index of the buffer that holds the data.
    Report(u8, Result<usize, PipeError>, usize),
    Cmd(PortCmd),
}
