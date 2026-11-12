//! Monitor inputs that follow the KVM (software.md §9): DDC/CI, MCCS VCP code 0x60 "Input Source".
//!
//! Each PC's software sets the monitors it is bound to whenever the KVM reports a switch or a change of profile. The KVM
//! reports both (key, hotkey or software) to both PCs, so this also works with the software on one PC only, as long as
//! the monitor takes DDC/CI commands on an input it is not showing (most do; some only listen on the active input).

pub mod mccs;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as imp;

use crate::local::DisplayBinding;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    /// Identifies the monitor on this computer (Linux: from its EDID; Windows: its device interface).
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
    Unsupported,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound(id) => f.write_str(&crate::fl!("ddc-not-connected", id = id.as_str())),
            Error::Permission(e) | Error::Io(e) => f.write_str(e),
            Error::Ddc(e) => e.fmt(f),
            Error::Unsupported => f.write_str(&crate::fl!("ddc-unsupported-os")),
        }
    }
}

pub const fn supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// The connected monitors; with `capabilities`, also the inputs each lists (slow: about a second per monitor).
pub fn displays(capabilities: bool) -> Result<Vec<Display>, Error> {
    #[cfg(any(windows, target_os = "linux"))]
    return imp::displays(capabilities);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = capabilities;
        Err(Error::Unsupported)
    }
}

pub fn current_input(id: &str) -> Result<u16, Error> {
    #[cfg(any(windows, target_os = "linux"))]
    return imp::current_input(id);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = id;
        Err(Error::Unsupported)
    }
}

pub fn set_input(id: &str, value: u16) -> Result<(), Error> {
    #[cfg(any(windows, target_os = "linux"))]
    return imp::set_input(id, value);
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = (id, value);
        Err(Error::Unsupported)
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

    use crate::local::DisplayMode::{self, Follow, Off, Pc1, Pc2};

    fn b(id: &str, inputs: [Option<u16>; 2], profiles: [DisplayMode; 4]) -> DisplayBinding {
        DisplayBinding { id: id.into(), name: id.into(), inputs, profiles, bt_inputs: Default::default() }
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
