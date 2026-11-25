//! Screen hop's cursor placed by this computer (software.md §4.14 "Placed by the PC", protocol 10). Optional: the KVM
//! does without it, with its absolute pointer and, on the screens a PC maps nothing onto (Windows' other screens),
//! relative motion. While this computer holds a lease (`Request::PlaceCursor`, asked for again every
//! [`PLACE_RENEW_MS`]), the KVM sends it where the cursor goes on the screens [`screens::PLACEMENT`] names
//! (`Event::PlaceCursor`) instead of motion, and [`screens::place_cursor`] puts it there: exactly, whatever the
//! system's acceleration does with motion. When the system won't let it (Windows' sign-in, lock screen or administrator
//! prompt; macOS until it is allowed under Accessibility), it gives the lease back at once, and the KVM goes on its own
//! way until it can again.
//!
//! On a thread of its own, with a handle of its own on the KVM's control interface: the window's worker reads the
//! KVM's whole state once a second, which would hold the cursor up meanwhile. Every program with the interface open
//! reads every report, so the requests here use id 0, which `Client` never does, and `Client` drops the placements.
//! (In a browser, the web build, there is no placer: only its state, for the window.)
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kvm_proto::ctrl::{DeviceMsg, Event, PLACE_RENEW_MS, PROTOCOL_VERSION, Placement, Request, Response, req};

use crate::screens::{self, Unplaceable};
use crate::time::{Duration, Instant};
#[cfg(not(target_arch = "wasm32"))]
use crate::transport::HidTransport;
use crate::transport::Transport;

/// What placing the cursor does now, as the window says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// Turned off on this computer, or not offered on its system ([`screens::PLACEMENT`] is `Off`: Linux).
    #[default]
    Off,
    /// No KVM answers.
    NoKvm,
    /// The KVM's firmware speaks another protocol than this program.
    Mismatch,
    /// The KVM takes this computer's placements: it sends them while screen hop puts the pointer on its screens.
    Placing,
    /// The system doesn't let it now.
    Blocked(Unplaceable),
}

/// Between the placer and the window: whether it is on (this computer's setting), and what it does.
#[derive(Debug, Default)]
pub struct Shared {
    pub enabled: AtomicBool,
    pub state: Mutex<State>,
}

/// After the system refused a placement (an administrator's window in front, say), the KVM moves the pointer its own
/// way this long before this computer asks to place it again.
const RETRY_AFTER_REFUSAL: Duration = Duration::from_secs(2);
const RECONNECT: Duration = Duration::from_secs(2);
/// Firmware of another protocol is asked again this rarely (it may be updated meanwhile).
const RETRY_MISMATCH: Duration = Duration::from_secs(30);

/// Why a session with the KVM ended.
enum End {
    /// Turned off: the lease was given back.
    Disabled,
    Lost,
    Mismatch,
}

/// This computer's cursor as the placer uses it: the system's ([`System`]), or a test's.
trait Cursor {
    /// The screens to ask for ([`screens::PLACEMENT`]).
    fn placement(&self) -> Placement;
    fn can_place(&self) -> Result<(), Unplaceable>;
    fn place(&mut self, at: [i32; 2]) -> Result<(), Unplaceable>;
}

struct System;

impl Cursor for System {
    fn placement(&self) -> Placement {
        screens::PLACEMENT
    }

    fn can_place(&self) -> Result<(), Unplaceable> {
        screens::can_place_cursor()
    }

    fn place(&mut self, at: [i32; 2]) -> Result<(), Unplaceable> {
        screens::place_cursor(at)
    }
}

/// Starts placing the cursor on a thread of its own, for as long as the program runs, when [`Shared::enabled`] is on.
/// `wake`: the window redraws (the state changed). Nothing where the system has no way to place it.
pub fn spawn(shared: Arc<Shared>, wake: impl Fn() + Send + 'static) {
    if screens::PLACEMENT == Placement::Off {
        return;
    }
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || run(&shared, &wake));
    #[cfg(target_arch = "wasm32")]
    let _ = (shared, wake);
}

#[cfg(not(target_arch = "wasm32"))]
fn run(shared: &Shared, wake: &dyn Fn()) {
    screens::prepare_to_place();
    let set = |s: State| {
        let mut st = shared.state.lock().unwrap();
        if *st != s {
            *st = s;
            drop(st);
            wake();
        }
    };
    loop {
        if !shared.enabled.load(Ordering::Relaxed) {
            set(State::Off);
            std::thread::sleep(Duration::from_millis(PLACE_RENEW_MS as u64));
            continue;
        }
        let mut t = match HidTransport::open_first() {
            Ok(t) => t,
            Err(_) => {
                set(State::NoKvm);
                std::thread::sleep(RECONNECT);
                continue;
            }
        };
        let Err(end) = session(&mut t, shared, &set, &mut System);
        match end {
            End::Disabled => {}
            End::Lost => {
                set(State::NoKvm);
                std::thread::sleep(RECONNECT);
            }
            End::Mismatch => {
                set(State::Mismatch);
                std::thread::sleep(RETRY_MISMATCH);
            }
        }
    }
}

// The KVM's control interface on this computer's USB: a placer has nothing to do in a browser.
#[cfg(not(target_arch = "wasm32"))]
fn send(t: &mut dyn Transport, p: Placement) -> Result<(), End> {
    let raw = Request::PlaceCursor(p).encode(0).map_err(|_| End::Lost)?;
    t.write(&raw).map_err(|_| End::Lost)
}

/// Whether the KVM's firmware speaks this program's protocol (it places the cursor only then).
#[cfg(not(target_arch = "wasm32"))]
fn check_protocol(t: &mut dyn Transport) -> Result<(), End> {
    let raw = Request::GetInfo.encode(0).map_err(|_| End::Lost)?;
    t.write(&raw).map_err(|_| End::Lost)?;
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        let Some(r) = t.read(Duration::from_millis(50)).map_err(|_| End::Lost)? else { continue };
        if let Ok(DeviceMsg::Response { id: 0, request: req::GET_INFO, response: Response::Info(i) }) =
            DeviceMsg::decode(&r)
        {
            return if i.proto == PROTOCOL_VERSION { Ok(()) } else { Err(End::Mismatch) };
        }
    }
    Err(End::Lost)
}

/// One connection: the lease while placing may and is allowed, the placements, until it is turned off or the KVM
/// goes away.
#[cfg(not(target_arch = "wasm32"))]
fn session(
    t: &mut dyn Transport,
    shared: &Shared,
    set: &dyn Fn(State),
    cursor: &mut dyn Cursor,
) -> Result<Infallible, End> {
    check_protocol(t)?;
    let renew = Duration::from_millis(PLACE_RENEW_MS as u64);
    let mut next = Instant::now();
    // What the KVM was asked for last, unless that was `Off`.
    let mut leased = false;
    let mut refused_until: Option<Instant> = None;
    loop {
        let now = Instant::now();
        if now >= next {
            next = now + renew;
            if !shared.enabled.load(Ordering::Relaxed) {
                if leased {
                    send(t, Placement::Off)?;
                }
                return Err(End::Disabled);
            }
            let may = if refused_until.is_some_and(|u| now < u) {
                Err(Unplaceable::Refused)
            } else {
                refused_until = None;
                cursor.can_place()
            };
            match may {
                Ok(()) => {
                    send(t, cursor.placement())?;
                    leased = true;
                }
                Err(why) => {
                    if leased {
                        send(t, Placement::Off)?;
                        leased = false;
                    }
                    set(State::Blocked(why));
                }
            }
        }
        let wait = next.saturating_duration_since(Instant::now()).min(Duration::from_millis(50));
        let Some(r) = t.read(wait).map_err(|_| End::Lost)? else { continue };
        match DeviceMsg::decode(&r) {
            Ok(DeviceMsg::Event(Event::PlaceCursor(at))) if leased => {
                if let Err(why) = cursor.place([at[0] as i32, at[1] as i32]) {
                    // The pointer goes back to the KVM's own way at once, and placing is asked for again later.
                    send(t, Placement::Off)?;
                    leased = false;
                    refused_until = Some(Instant::now() + RETRY_AFTER_REFUSAL);
                    set(State::Blocked(why));
                }
            }
            Ok(DeviceMsg::Response { id: 0, request: req::PLACE_CURSOR, response }) => match response {
                Response::Ok if leased => set(State::Placing),
                _ => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kvm_proto::ctrl::Info;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use crate::transport::Report;

    /// The KVM's side, scripted: what it sends, in order (then it is gone), and what it was sent.
    #[derive(Default)]
    struct Kvm {
        sends: VecDeque<Report>,
        got: Vec<Request>,
    }

    impl Transport for Kvm {
        fn write(&mut self, report: &Report) -> Result<(), String> {
            self.got.push(Request::decode(report).map_err(|e| format!("{e:?}"))?.1);
            Ok(())
        }

        fn read(&mut self, _: Duration) -> Result<Option<Report>, String> {
            self.sends.pop_front().map(Some).ok_or_else(|| "gone".into())
        }

        fn describe(&self) -> String {
            "scripted".into()
        }
    }

    /// The cursor, refusing from the `refuse_from`th placement on.
    struct Fake {
        placed: Vec<[i32; 2]>,
        refuse_from: usize,
    }

    impl Cursor for Fake {
        fn placement(&self) -> Placement {
            Placement::Others
        }

        fn can_place(&self) -> Result<(), Unplaceable> {
            Ok(())
        }

        fn place(&mut self, at: [i32; 2]) -> Result<(), Unplaceable> {
            if self.placed.len() >= self.refuse_from {
                return Err(Unplaceable::Refused);
            }
            self.placed.push(at);
            Ok(())
        }
    }

    fn event(e: Event) -> Report {
        e.encode().unwrap()
    }

    fn answer(r: Response) -> Report {
        r.encode(req::PLACE_CURSOR, 0).unwrap()
    }

    /// The KVM's answer to the protocol check.
    fn info(proto: u8) -> Report {
        let i = Info {
            proto,
            rp_fw: 0x00011D,
            at_fw: [0x00011D; 2],
            hw: 0x000C00,
            serial: *b"TEST0001",
            caps: 0,
            bt_fw: 0,
        };
        Response::Info(i).encode(req::GET_INFO, 0).unwrap()
    }

    fn run_session(kvm: &mut Kvm, cursor: &mut Fake) -> (End, Vec<State>) {
        let shared = Shared { enabled: AtomicBool::new(true), ..Shared::default() };
        let states = RefCell::new(Vec::new());
        let Err(end) = session(kvm, &shared, &|s| states.borrow_mut().push(s), cursor);
        (end, states.into_inner())
    }

    #[test]
    fn it_asks_for_the_lease_and_places_the_cursor_where_the_kvm_says() {
        let mut kvm = Kvm::default();
        kvm.sends.extend([
            info(PROTOCOL_VERSION),
            answer(Response::Ok),
            event(Event::PlaceCursor([-140, 540])),
            // Another program's answer, and the other events, are left alone.
            Response::Ok.encode(req::GET_STATUS, 9).unwrap(),
            event(Event::ConfigChanged(1)),
            event(Event::PlaceCursor([-240, 541])),
        ]);
        let mut cursor = Fake { placed: Vec::new(), refuse_from: usize::MAX };
        let (end, states) = run_session(&mut kvm, &mut cursor);
        assert!(matches!(end, End::Lost));
        assert_eq!(kvm.got, [Request::GetInfo, Request::PlaceCursor(Placement::Others)]);
        assert_eq!(states, [State::Placing]);
        assert_eq!(cursor.placed, [[-140, 540], [-240, 541]]);
    }

    #[test]
    fn a_refusal_gives_the_lease_back_at_once() {
        let mut kvm = Kvm::default();
        kvm.sends.extend([
            info(PROTOCOL_VERSION),
            answer(Response::Ok),
            event(Event::PlaceCursor([1, 2])),
            event(Event::PlaceCursor([3, 4])),
            // Still on the way: not placed any more.
            event(Event::PlaceCursor([5, 6])),
        ]);
        let mut cursor = Fake { placed: Vec::new(), refuse_from: 1 };
        let (_, states) = run_session(&mut kvm, &mut cursor);
        assert_eq!(
            kvm.got,
            [Request::GetInfo, Request::PlaceCursor(Placement::Others), Request::PlaceCursor(Placement::Off)]
        );
        assert_eq!(states, [State::Placing, State::Blocked(Unplaceable::Refused)]);
        assert_eq!(cursor.placed, [[1, 2]]);
    }

    #[test]
    fn firmware_of_another_protocol_is_never_asked_to_place() {
        let mut kvm = Kvm::default();
        kvm.sends.push_back(info(PROTOCOL_VERSION - 1));
        let (end, _) = run_session(&mut kvm, &mut Fake { placed: Vec::new(), refuse_from: usize::MAX });
        assert!(matches!(end, End::Mismatch));
        assert_eq!(kvm.got, [Request::GetInfo]);
    }
}
