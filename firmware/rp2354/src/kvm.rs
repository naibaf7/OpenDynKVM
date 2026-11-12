//! The engine (`kvm_core::Engine`) and where its outputs go. Only core-0 tasks call into it.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use defmt::warn;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, ThreadModeRawMutex};
use embassy_sync::signal::Signal;
use kvm_core::config::Config;
use kvm_core::engine::{Engine, Output};
use kvm_proto::ctrl::{FwState, Info, Pc, Port, RebootTarget};
use kvm_proto::link::LinkMsg;
use kvm_proto::link::msg::{CTRL_REPLY_CHUNK, Command, CtrlStatus, fragments};

use crate::board::Board;
use crate::host::{PORT_CMD, PortCmd};
use crate::{link, power};

/// The engine is over 100 KB: the static holds it from the start (`Engine::empty` is const) and [`init`] only fills it
/// in, so no stack ever holds a copy.
static ENGINE: Mutex<ThreadModeRawMutex, RefCell<Engine>> = Mutex::new(RefCell::new(Engine::empty()));
static BOARD: Mutex<ThreadModeRawMutex, RefCell<Option<Board>>> = Mutex::new(RefCell::new(None));

/// Something changed that belongs in flash (the storage task pulls the items from the engine).
pub static SAVE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static REBOOT: Signal<CriticalSectionRawMutex, RebootTarget> = Signal::new();
/// Port whose VBUS should be cycled (passthrough switch with `power-cycle-on-switch`, a probe's last retry).
pub static POWER_CYCLE: [Signal<CriticalSectionRawMutex, ()>; 3] = [const { Signal::new() }; 3];
/// Port to unplug on its data lines and plug into Hub-0 again (`power::port_power`): the router goes off for a moment,
/// so Hub-0 sees the device leave and come back, and resets it; VBUS stays on. Taking a device back from passthrough,
/// re-detecting it, retrying a failed probe.
pub static REPLUG: [Signal<CriticalSectionRawMutex, ()>; 3] = [const { Signal::new() }; 3];
/// Counts every `Output::Route` per port, so a replug doesn't put back Hub-0 over a route the engine set meanwhile.
pub static ROUTE_GEN: [AtomicU32; 3] = [const { AtomicU32::new(0) }; 3];
pub static DROPPED: AtomicU32 = AtomicU32::new(0);

pub fn init(config: Config, info: Info, serial: &str, board: Board) {
    BOARD.lock(|b| *b.borrow_mut() = Some(board));
    with(|e| e.start(config, info, serial));
}

/// Runs `f` on the engine, then executes every output it produced.
pub fn with<R>(f: impl FnOnce(&mut Engine) -> R) -> R {
    ENGINE.lock(|cell| {
        let mut guard = cell.borrow_mut();
        let e = &mut *guard;
        let r = f(e);
        while let Some(o) = e.poll_output() {
            route(e, o);
        }
        r
    })
}

pub fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> R {
    BOARD.lock(|b| f(b.borrow_mut().as_mut().expect("board not initialised")))
}

fn pc_index(pc: Pc) -> usize {
    pc.index()
}

fn route(e: &mut Engine, o: Output) {
    match o {
        Output::Report { pc, ep, data } => {
            if !link::send_report(pc_index(pc), ep & 0x7F, &data) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        Output::DescriptorSetChanged => {
            for s in &link::DESC_DIRTY {
                s.signal(());
            }
        }
        Output::DeviceSetReport { port, iface, report_type, report_id, data } => {
            cmd(port, PortCmd::SetReport { iface, report_type, report_id, data });
        }
        Output::DeviceOut { port, iface, data } => cmd(port, PortCmd::Out { iface, data }),
        Output::DeviceControl { pc, tag, port, setup, data } => cmd(port, PortCmd::Control { pc, tag, setup, data }),
        Output::CtrlReply { pc, tag, stall, data } => {
            let status = if stall { CtrlStatus::Stall } else { CtrlStatus::Ok };
            for (offset, chunk) in fragments(&data, CTRL_REPLY_CHUNK) {
                let m = LinkMsg::CtrlReply { tag, status, total_len: data.len() as u16, offset, data: chunk };
                if !link::send_control(pc_index(pc), &m) {
                    DROPPED.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        Output::Route { port, route, power_cycle } => {
            ROUTE_GEN[port.index()].fetch_add(1, Ordering::Relaxed);
            with_board(|b| b.ports[port.index()].route(route));
            if power_cycle {
                POWER_CYCLE[port.index()].signal(());
            }
        }
        Output::Reprobe(port) => {
            // A new attempt (mode change, `odkvmctl port … redetect`): failed probes before it don't count.
            crate::host::PROBE_FAILS[port.index()].store(0, Ordering::Relaxed);
            // Unplugged and plugged into Hub-0 on the data lines; Hub-0's port reset restarts the device. Until 0.1.6
            // this cycled VBUS for 300 ms instead, which is no power-off: the port's 220 µF has no discharge path but
            // the device (the TPS2553 doesn't discharge its output), so VBUS sagged and came back, and a receiver
            // browned out without a reset and stayed stuck until it was replugged (first board, 2026-11-12).
            REPLUG[port.index()].signal(());
        }
        Output::Save => SAVE.signal(()),
        Output::Reboot(t) => REBOOT.signal(t),
        Output::Firmware(op) => {
            if crate::update::OPS.try_send(op).is_err() {
                warn!("firmware update queue full");
                e.set_fw_state(FwState::Failed, 0);
            }
        }
        Output::Power(plan) => power::apply(plan),
        Output::RemoteWakeup(pc) => {
            if !link::send_control(pc_index(pc), &LinkMsg::Command(Command::RemoteWakeup)) {
                DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        Output::Bt(b) => crate::bt::send(b),
    }
}

fn cmd(port: Port, c: PortCmd) {
    if PORT_CMD[port.index()].try_send(c).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
}
