//! Monitor inputs that follow the KVM (software.md §9): DDC/CI, MCCS VCP code 0x60 "Input Source".
//!
//! Each PC's software sets the monitors it is bound to whenever the KVM reports a switch or a change of profile. The KVM
//! reports both (key, hotkey or software) to both PCs, so this also works with the software on one PC only, as long as
//! the monitor takes DDC/CI commands on an input it is not showing (most do; some only listen on the active input).
//!
//! Linux and macOS speak DDC/CI themselves over the display's I²C channel ([`Channel`]); Windows' monitor API does it.

pub mod mccs;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

#[cfg(any(target_os = "linux", target_os = "macos", test))]
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "macos", test))]
use mccs::ReplyError;

use crate::local::DisplayBinding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// Identifies the monitor on this computer (Linux, macOS: from its EDID; Windows: its device interface).
    pub id: String,
    pub name: String,
    /// The input values the monitor lists in its capabilities (empty if not read).
    pub inputs: Vec<u16>,
    /// The input it shows now (the one this computer is on, if it is showing this computer).
    pub current: Option<u16>,
    /// Why it could not be read.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    NotFound(String),
    Permission(String),
    Io(String),
    Ddc(mccs::ReplyError),
    /// The system gives no DDC/CI channel to this display (macOS: no I²C behind its connection).
    NoChannel,
    Unsupported,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => f.write_str(&crate::fl!("ddc-not-connected", id = id.as_str())),
            Error::Permission(e) | Error::Io(e) => f.write_str(e),
            Error::Ddc(e) => e.fmt(f),
            Error::NoChannel => f.write_str(&crate::fl!("ddc-no-channel")),
            Error::Unsupported => f.write_str(&crate::fl!("ddc-unsupported-os")),
        }
    }
}

pub const fn supported() -> bool {
    cfg!(any(windows, target_os = "linux", target_os = "macos"))
}

/// The connected monitors; with `capabilities`, also the inputs each lists (slow: about a second per monitor).
pub fn displays(capabilities: bool) -> Result<Vec<Display>, Error> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    return imp::displays(capabilities);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = capabilities;
        Err(Error::Unsupported)
    }
}

pub fn current_input(id: &str) -> Result<u16, Error> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    return imp::current_input(id);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = id;
        Err(Error::Unsupported)
    }
}

pub fn set_input(id: &str, value: u16) -> Result<(), Error> {
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    return imp::set_input(id, value);
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (id, value);
        Err(Error::Unsupported)
    }
}

/// DDC/CI 1.1: wait at least 40 ms before reading a reply, 50 ms after a write without one.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
const REPLY_DELAY: Duration = Duration::from_millis(50);
#[cfg(any(target_os = "linux", target_os = "macos", test))]
const RETRIES: usize = 3;

/// A display's DDC/CI link, where this program builds the packets itself (Linux, macOS).
#[cfg(any(target_os = "linux", target_os = "macos", test))]
trait Channel {
    /// Writes `request` (from the source address 0x51 on, as [`mccs`] builds it); then, after [`REPLY_DELAY`], reads
    /// the reply into `reply` unless it is empty (from the display's address 0x6E on). Returns the bytes read.
    fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> Result<usize, Error>;

    /// A VCP code's (current value, maximum).
    fn get_vcp(&mut self, code: u8) -> Result<(u16, u16), Error> {
        let mut last = Error::Ddc(ReplyError::Empty);
        for _ in 0..RETRIES {
            let mut buf = [0u8; 12];
            match self.exchange(&mccs::get_vcp_request(code), &mut buf) {
                Ok(n) => match mccs::parse_vcp_reply(&buf[..n], code) {
                    Ok(v) => return Ok(v),
                    Err(e @ ReplyError::Unsupported) => return Err(Error::Ddc(e)),
                    Err(e) => last = Error::Ddc(e),
                },
                Err(e) => last = e,
            }
            std::thread::sleep(REPLY_DELAY);
        }
        Err(last)
    }

    fn set_vcp(&mut self, code: u8, value: u16) -> Result<(), Error> {
        self.exchange(&mccs::set_vcp_request(code, value), &mut []).map(|_| ())
    }

    /// The capability string, fragment by fragment (None if it breaks off).
    fn capabilities(&mut self) -> Option<String> {
        let mut text = Vec::new();
        'fragments: while text.len() < 4096 {
            for _ in 0..RETRIES {
                let mut buf = [0u8; 40];
                let Ok(n) = self.exchange(&mccs::capabilities_request(text.len() as u16), &mut buf) else { continue };
                if let Ok((offset, data)) = mccs::parse_capabilities_reply(&buf[..n]) {
                    if offset as usize != text.len() {
                        continue;
                    }
                    if data.is_empty() {
                        break 'fragments;
                    }
                    text.extend_from_slice(data);
                    continue 'fragments;
                }
            }
            return None;
        }
        Some(String::from_utf8_lossy(&text).trim_end_matches('\0').to_string())
    }
}

/// Fills in what `channel` reads of a monitor: the input it shows and, with `capabilities`, those it lists; or why not.
#[cfg(any(target_os = "linux", target_os = "macos", test))]
fn read_display(d: &mut Display, channel: Result<impl Channel, Error>, capabilities: bool) {
    let mut channel = match channel {
        Ok(c) => c,
        Err(e) => {
            d.error = Some(e.to_string());
            return;
        }
    };
    match channel.get_vcp(mccs::INPUT_SOURCE) {
        Ok((cur, _)) => d.current = Some(cur),
        Err(e) => d.error = Some(e.to_string()),
    }
    if capabilities && d.current.is_some() {
        d.inputs = channel.capabilities().map(|s| mccs::parse_capabilities(&s)).unwrap_or_default().inputs;
    }
}

pub use crate::local::KvmState;

/// The KVM went from `from` to `to` (a switch to a PC or a Bluetooth host, or a change of profile): sets each monitor
/// whose input should change to it. One pinned to a PC isn't sent its input again on every switch (some monitors blank for a moment even then).
/// Returns a line per monitor that failed.
pub fn follow(from: &KvmState, to: &KvmState, bindings: &[DisplayBinding]) -> Vec<String> {
    follow_with(from, to, bindings, set_input)
}

fn follow_with(
    from: &KvmState,
    to: &KvmState,
    bindings: &[DisplayBinding],
    mut set: impl FnMut(&str, u16) -> Result<(), Error>,
) -> Vec<String> {
    let mut failed = Vec::new();
    for b in bindings {
        if let Some(input) = b.target(to)
            && b.target(from) != Some(input)
            && let Err(e) = set(&b.id, input)
        {
            failed.push(format!("{}: {e}", b.name));
        }
    }
    failed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A display that answers with `replies` in turn (then nothing), noting what it was sent.
    #[derive(Default)]
    struct Scripted {
        replies: std::collections::VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }

    impl Channel for Scripted {
        fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> Result<usize, Error> {
            self.sent.push(request.to_vec());
            if reply.is_empty() {
                return Ok(0);
            }
            let r = self.replies.pop_front().unwrap_or_default();
            let n = r.len().min(reply.len());
            reply[..n].copy_from_slice(&r[..n]);
            Ok(n)
        }
    }

    /// A reply from the display: its address, the length, `payload` and the checksum.
    fn from_display(payload: &[u8]) -> Vec<u8> {
        let mut b = vec![0x6E, 0x80 | payload.len() as u8];
        b.extend_from_slice(payload);
        b.push(b.iter().fold(0x50, |a, x| a ^ x));
        b
    }

    #[test]
    fn a_channel_retries_a_busy_display_and_reads_the_capabilities_fragment_by_fragment() {
        let mut d = Scripted::default();
        // Busy (the null message) first, then the input: DisplayPort 1 of a maximum 0x12.
        d.replies.push_back(from_display(&[]));
        d.replies.push_back(from_display(&[0x02, 0x00, 0x60, 0x00, 0x00, 0x12, 0x00, 0x0F]));
        assert_eq!(d.get_vcp(mccs::INPUT_SOURCE), Ok((0x0F, 0x12)));
        assert_eq!(d.sent, [mccs::get_vcp_request(0x60), mccs::get_vcp_request(0x60)]);
        // A display without the code says so at once: no retry.
        d.replies.push_back(from_display(&[0x02, 0x01, 0x60, 0, 0, 0, 0, 0]));
        assert_eq!(d.get_vcp(mccs::INPUT_SOURCE), Err(Error::Ddc(ReplyError::Unsupported)));
        assert_eq!(d.sent.len(), 3);

        let caps = b"(vcp(60(0F 11)))";
        let mut d = Scripted::default();
        d.replies.push_back(from_display(&[[0xE3, 0, 0].as_slice(), &caps[..8]].concat()));
        // A fragment for the wrong offset (a repeated reply) is asked again.
        d.replies.push_back(from_display(&[[0xE3, 0, 0].as_slice(), &caps[..8]].concat()));
        d.replies.push_back(from_display(&[[0xE3, 0, 8].as_slice(), &caps[8..]].concat()));
        d.replies.push_back(from_display(&[0xE3, 0, 16]));
        assert_eq!(d.capabilities().as_deref(), Some("(vcp(60(0F 11)))"));
        let offsets: Vec<u8> = d.sent.iter().map(|r| r[4]).collect();
        assert_eq!(offsets, [0, 8, 8, 16]);

        let mut shown = Display { id: "m".into(), name: "m".into(), inputs: vec![], current: None, error: None };
        let mut d = Scripted::default();
        d.replies.push_back(from_display(&[0x02, 0x00, 0x60, 0x00, 0x00, 0x12, 0x00, 0x11]));
        d.replies.push_back(from_display(&[[0xE3, 0, 0].as_slice(), caps].concat()));
        d.replies.push_back(from_display(&[0xE3, 0, caps.len() as u8]));
        read_display(&mut shown, Ok(d), true);
        assert_eq!(
            (shown.current, shown.inputs.as_slice(), shown.error.clone()),
            (Some(0x11), [0x0F, 0x11].as_slice(), None)
        );
        read_display(&mut shown, Err::<Scripted, _>(Error::NoChannel), true);
        assert!(shown.error.is_some());

        let mut d = Scripted::default();
        assert_eq!(d.set_vcp(mccs::INPUT_SOURCE, 0x1B), Ok(()));
        assert_eq!(d.sent, [mccs::set_vcp_request(0x60, 0x1B)]);
    }

    use crate::local::DisplayMode::{self, Follow, Off, Pc1, Pc2};

    fn b(id: &str, inputs: [Option<u16>; 2], profiles: [DisplayMode; 4]) -> DisplayBinding {
        DisplayBinding {
            id: id.into(),
            name: id.into(),
            inputs,
            profiles: profiles.to_vec(),
            bt_inputs: Default::default(),
        }
    }

    /// PC `pc` (0 or 1) active in `profile`.
    fn at(pc: usize, profile: usize) -> KvmState {
        KvmState { pc, bt: None, profile }
    }

    /// The monitors `follow_with` sets, going `from` → `to`.
    fn sets(from: (usize, usize), to: (usize, usize), bindings: &[DisplayBinding]) -> Vec<(String, u16)> {
        sets_s(&at(from.0, from.1), &at(to.0, to.1), bindings)
    }

    fn sets_s(from: &KvmState, to: &KvmState, bindings: &[DisplayBinding]) -> Vec<(String, u16)> {
        let mut calls = Vec::new();
        follow_with(from, to, bindings, |id, v| {
            calls.push((id.to_string(), v));
            Ok(())
        });
        calls
    }

    #[test]
    fn a_switch_sets_the_following_monitors_to_the_pcs_input() {
        let bindings = [
            b("left", [Some(0x0F), Some(0x11)], [Follow; 4]),
            b("right", [Some(0x12), None], [Follow; 4]),
            b("off", [Some(0x01), Some(0x02)], [Off; 4]),
        ];
        assert_eq!(sets((0, 0), (1, 0), &bindings), [("left".to_string(), 0x11)], "right has no PC2 input");
        let failed = follow_with(&at(1, 0), &at(0, 0), &bindings, |id, _| Err(Error::NotFound(id.into())));
        assert_eq!(failed, ["left: left: not connected", "right: right: not connected"]);
    }

    #[test]
    fn each_profile_routes_the_monitors_its_own_way() {
        // Work (profile 0): both follow. Gaming (profile 1): the left one stays on PC2, the right one is left alone.
        let bindings = [
            b("left", [Some(0x0F), Some(0x11)], [Follow, Pc2, Follow, Follow]),
            b("right", [Some(0x0F), Some(0x11)], [Follow, Off, Follow, Follow]),
            b("pinned", [Some(0x0F), Some(0x11)], [Pc1; 4]),
        ];
        // Into gaming on PC1: only the left one moves (to PC2); the right one stays where it is.
        assert_eq!(sets((0, 0), (0, 1), &bindings), [("left".to_string(), 0x11)]);
        // A switch in gaming: nothing moves.
        assert!(sets((0, 1), (1, 1), &bindings).is_empty());
        // Back to work with PC2 active: the right one comes to PC2; left is there already, pinned never moves.
        assert_eq!(sets((1, 1), (1, 0), &bindings), [("right".to_string(), 0x11)]);
    }

    #[test]
    fn a_monitor_with_an_input_for_a_bluetooth_host_follows_it_there_and_back() {
        let mut both = b("both", [Some(0x0F), Some(0x11)], [Follow; 4]);
        both.bt_inputs.insert("AA".into(), 0x12);
        let bindings = [both, b("pcs", [Some(0x0F), Some(0x11)], [Follow; 4])];
        let host = KvmState { pc: 0, bt: Some("AA".into()), profile: 0 };
        let other = KvmState { bt: Some("BB".into()), ..host.clone() };
        // PC1 → the host: only the monitor wired to it; the other stays on PC1 (it shows the home PC).
        assert_eq!(sets_s(&at(0, 0), &host, &bindings), [("both".to_string(), 0x12)]);
        // The host → PC2: both move; the host → a host without an input here: back to the home PC.
        assert_eq!(sets_s(&host, &at(1, 0), &bindings), [("both".to_string(), 0x11), ("pcs".to_string(), 0x11)]);
        assert_eq!(sets_s(&host, &other, &bindings), [("both".to_string(), 0x0F)]);
    }
}
