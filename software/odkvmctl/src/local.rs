//! Settings of this computer, not of the KVM: the window's appearance and language, and the monitors whose inputs
//! follow the KVM (software.md §9 "Local settings"). Everything the KVM itself does is in its own flash, shared by
//! both PCs; this is what differs per computer and user.
//!
//! - **Where:** one JSON file in the user's configuration directory: `$XDG_CONFIG_HOME/opendyn-kvm/settings.json`
//!   (`~/.config/...` when unset, or not an absolute path as the XDG spec requires) on Linux,
//!   `%APPDATA%\OpenDynKVM\settings.json` on Windows (the roaming profile: the theme and language follow the user,
//!   and a monitor binding only applies to the monitor with that id, so it is harmless on another computer). Start at
//!   login is kept where each system looks for it instead ([`crate::autostart`]).
//! - **Who writes:** only the window (`opendyn-kvm`), one copy per user, as each setting changes; the command line
//!   only reads the language. A change is a click, so the whole file is written each time.
//! - **How:** to a temporary file, flushed to disk, then renamed over the old one: a crash or power cut leaves either
//!   file whole, never half of one or an empty one. On Windows the rename is tried again for a moment when it fails, as
//!   it does while a virus scanner or the search indexer holds the file.
//! - **Reading:** a missing file means the defaults. Fields this version doesn't know are ignored and missing ones
//!   take their defaults, and so does a theme or monitor mode it doesn't know (one written by a newer version). A file
//!   that still doesn't parse (edited by hand, say) also gives the defaults, but is first copied to
//!   `settings.json.unreadable`, so the next save doesn't lose it.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use kvm_proto::ctrl::MAX_PROFILES;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Dark,
    Light,
    /// Follow the system's dark or light mode; also what a theme this version doesn't know reads as (last, as serde
    /// wants the catch-all).
    #[default]
    #[serde(other)]
    System,
}

/// What a monitor does while a KVM profile is active.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    /// Shows the active PC, or the Bluetooth host that has the input if it has an input of its own here.
    Follow,
    /// Stays on PC1's / PC2's input, whichever PC is active.
    Pc1,
    Pc2,
    /// Left alone; also what a mode this version doesn't know reads as (last, as serde wants the catch-all).
    #[default]
    #[serde(other)]
    Off,
}

impl DisplayMode {
    pub const ALL: [DisplayMode; 4] = [DisplayMode::Follow, DisplayMode::Pc1, DisplayMode::Pc2, DisplayMode::Off];
}

/// A monitor whose input follows the KVM.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "StoredBinding", into = "StoredBinding")]
pub struct DisplayBinding {
    /// `ddc::Display::id`.
    pub id: String,
    pub name: String,
    /// The input (MCCS VCP 0x60 value) that shows PC1 and PC2: how the monitor is wired.
    pub inputs: [Option<u16>; 2],
    /// What it does in each of the KVM's profiles.
    pub profiles: [DisplayMode; MAX_PROFILES],
    /// Optional: the input that shows a Bluetooth host (a laptop or tablet wired to the monitor too), by the host's
    /// address (`BtHostView::address`). A host without one leaves the monitor on the PC.
    pub bt_inputs: BTreeMap<String, u16>,
}

/// What the KVM is doing, as far as the monitors care: the active PC (0 or 1), the Bluetooth host that has the input
/// (its address; None: the PC has it), the active profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvmState {
    pub pc: usize,
    pub bt: Option<String>,
    pub profile: usize,
}

impl DisplayBinding {
    /// The input the monitor should show in state `s`; None: leave it.
    pub fn target(&self, s: &KvmState) -> Option<u16> {
        let host = s.bt.as_ref().and_then(|a| self.bt_inputs.get(a)).copied();
        match self.profiles.get(s.profile).copied().unwrap_or_default() {
            DisplayMode::Off => None,
            DisplayMode::Follow => host.or(self.inputs[s.pc.min(1)]),
            DisplayMode::Pc1 => self.inputs[0],
            DisplayMode::Pc2 => self.inputs[1],
        }
    }
}

/// A binding as files hold it. Before profiles there was only `enabled` (follow or not); it is still written, as
/// profile 0's, so that an older version keeps the binding.
#[derive(Serialize, Deserialize)]
struct StoredBinding {
    id: String,
    name: String,
    #[serde(default)]
    enabled: bool,
    inputs: [Option<u16>; 2],
    profiles: Option<[DisplayMode; MAX_PROFILES]>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    bt_inputs: BTreeMap<String, u16>,
}

impl From<StoredBinding> for DisplayBinding {
    fn from(s: StoredBinding) -> Self {
        let old = if s.enabled { DisplayMode::Follow } else { DisplayMode::Off };
        DisplayBinding {
            id: s.id,
            name: s.name,
            inputs: s.inputs,
            profiles: s.profiles.unwrap_or([old; MAX_PROFILES]),
            bt_inputs: s.bt_inputs,
        }
    }
}

impl From<DisplayBinding> for StoredBinding {
    fn from(b: DisplayBinding) -> Self {
        let enabled = b.profiles[0] == DisplayMode::Follow;
        StoredBinding {
            id: b.id,
            name: b.name,
            enabled,
            inputs: b.inputs,
            profiles: Some(b.profiles),
            bt_inputs: b.bt_inputs,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Local {
    pub theme: Theme,
    pub displays: Vec<DisplayBinding>,
    /// The language of the window and the command line (`i18n::LANGUAGES`); None: the system's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

impl Local {
    pub fn binding_mut(&mut self, id: &str, name: &str) -> &mut DisplayBinding {
        match self.displays.iter().position(|b| b.id == id) {
            Some(i) => &mut self.displays[i],
            None => {
                let b = DisplayBinding {
                    id: id.into(),
                    name: name.into(),
                    inputs: [None, None],
                    profiles: [DisplayMode::Off; MAX_PROFILES],
                    bt_inputs: BTreeMap::new(),
                };
                self.displays.push(b);
                self.displays.last_mut().unwrap()
            }
        }
    }
}

/// The XDG configuration directory (Linux and other Unix): `$XDG_CONFIG_HOME` if it is an absolute path, else
/// `~/.config`. Also where the autostart entry goes.
#[cfg(not(windows))]
pub fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

pub fn path() -> Option<PathBuf> {
    #[cfg(windows)]
    let dir = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("OpenDynKVM"));
    #[cfg(not(windows))]
    let dir = config_home().map(|c| c.join("opendyn-kvm"));
    dir.map(|d| d.join("settings.json"))
}

pub fn load() -> Local {
    path().map(|p| load_from(&p)).unwrap_or_default()
}

/// Written to a temporary file, flushed, and renamed over the old one (see the module documentation).
pub fn save(l: &Local) -> io::Result<()> {
    let p = path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
    save_to(&p, l)
}

/// Where a file that doesn't parse is kept.
fn unreadable_path(p: &Path) -> PathBuf {
    p.with_extension("json.unreadable")
}

fn load_from(p: &Path) -> Local {
    let bytes = match std::fs::read(p) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Local::default(),
        Err(e) => {
            eprintln!("{}: {e}; using the default settings", p.display());
            return Local::default();
        }
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        let keep = unreadable_path(p);
        if std::fs::read(&keep).ok().as_deref() != Some(&bytes[..]) {
            match std::fs::write(&keep, &bytes) {
                Ok(()) => eprintln!(
                    "{}: {e}; using the default settings, the file is kept as {}",
                    p.display(),
                    keep.display()
                ),
                Err(w) => eprintln!("{}: {e}; using the default settings ({}: {w})", p.display(), keep.display()),
            }
        }
        Local::default()
    })
}

fn save_to(p: &Path, l: &Local) -> io::Result<()> {
    use std::io::Write;
    let dir = p.parent().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = p.with_extension("json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(l).map_err(io::Error::other)?)?;
        // On disk before the rename makes it the file: otherwise a power cut can leave the new name on empty data.
        f.sync_all()?;
    }
    replace(&tmp, p)?;
    // The rename itself, on disk too (Unix; Windows has no handle on a directory to flush).
    #[cfg(unix)]
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Renames `tmp` over `p`. On Windows, a virus scanner or the search indexer that has `p` open can make the rename
/// fail for a moment: tried again for up to half a second.
fn replace(tmp: &Path, p: &Path) -> io::Result<()> {
    let mut tries = if cfg!(windows) { 10 } else { 1 };
    loop {
        match std::fs::rename(tmp, p) {
            Err(e) if tries > 1 && e.kind() != io::ErrorKind::NotFound => {
                tries -= 1;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            r => return r,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of its own under the system's temporary directory, removed afterwards.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let d = std::env::temp_dir().join(format!("odkvmctl-local-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TempDir(d)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saved_settings_load_back_and_leave_no_temporary_file() {
        let d = TempDir::new("save");
        let p = d.0.join("opendyn-kvm").join("settings.json");
        assert_eq!(load_from(&p), Local::default(), "no file yet: the defaults");
        let mut l = Local { theme: Theme::Dark, language: Some("nb".into()), ..Default::default() };
        l.binding_mut("m", "M").inputs = [Some(0x0F), None];
        save_to(&p, &l).unwrap();
        l.theme = Theme::Light;
        save_to(&p, &l).unwrap();
        assert_eq!(load_from(&p), l);
        let names: Vec<_> = std::fs::read_dir(p.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["settings.json"]);
    }

    #[test]
    fn a_file_that_does_not_parse_is_kept_before_it_can_be_overwritten() {
        let d = TempDir::new("unreadable");
        let p = d.0.join("settings.json");
        let broken = br#"{"theme": "dark", "displays": [{"id": "m", "name": "M", "inputs": [15, 17],}]}"#;
        std::fs::write(&p, broken).unwrap();
        assert_eq!(load_from(&p), Local::default());
        assert_eq!(std::fs::read(unreadable_path(&p)).unwrap(), broken);
        // The window saves its defaults over it: the original is still there.
        save_to(&p, &Local::default()).unwrap();
        assert_eq!(std::fs::read(unreadable_path(&p)).unwrap(), broken);
    }

    #[test]
    fn a_newer_versions_theme_and_monitor_modes_read_as_the_defaults() {
        let newer = r#"{"theme": "high-contrast", "window": {"width": 900},
                        "displays": [{"id": "m", "name": "M", "inputs": [15, 17],
                                      "profiles": ["follow", "pip", "pc2", "off"]}]}"#;
        let l: Local = serde_json::from_str(newer).unwrap();
        assert_eq!(l.theme, Theme::System);
        assert_eq!(l.displays[0].profiles, [DisplayMode::Follow, DisplayMode::Off, DisplayMode::Pc2, DisplayMode::Off]);
        assert_eq!(l.displays[0].inputs, [Some(15), Some(17)], "the binding itself is kept");
    }

    #[test]
    fn json_round_trip_and_defaults() {
        let mut l = Local { theme: Theme::Light, ..Default::default() };
        l.binding_mut("XYZ-1A2B-1", "Monitor 27").inputs = [Some(0x0F), Some(0x11)];
        l.binding_mut("XYZ-1A2B-1", "Monitor 27").profiles[1] = DisplayMode::Pc2;
        assert_eq!(l.displays.len(), 1);
        let text = serde_json::to_string(&l).unwrap();
        assert!(text.contains("\"theme\":\"light\""), "{text}");
        assert_eq!(serde_json::from_str::<Local>(&text).unwrap(), l);
        // Missing fields keep their defaults, so older files still load.
        assert_eq!(serde_json::from_str::<Local>("{}").unwrap(), Local::default());
    }

    #[test]
    fn a_monitor_that_followed_before_profiles_follows_in_every_profile() {
        let old = r#"{"displays":[{"id":"a","name":"A","enabled":true,"inputs":[15,17]},
                                  {"id":"b","name":"B","enabled":false,"inputs":[15,null]}]}"#;
        let l: Local = serde_json::from_str(old).unwrap();
        assert_eq!(l.displays[0].profiles, [DisplayMode::Follow; MAX_PROFILES]);
        assert_eq!(l.displays[1].profiles, [DisplayMode::Off; MAX_PROFILES]);
        let b = &l.displays[0];
        let at = |pc, profile| KvmState { pc, bt: None, profile };
        assert_eq!((b.target(&at(0, 0)), b.target(&at(1, 3))), (Some(15), Some(17)));
        assert!(b.bt_inputs.is_empty(), "files from before Bluetooth hosts have none");
    }

    #[test]
    fn a_following_monitor_shows_a_bluetooth_host_only_where_it_has_an_input() {
        let mut l = Local::default();
        let b = l.binding_mut("m", "M");
        b.inputs = [Some(0x0F), Some(0x11)];
        b.profiles = [DisplayMode::Follow, DisplayMode::Pc2, DisplayMode::Off, DisplayMode::Follow];
        b.bt_inputs.insert("C4:21:3B:6E:5A:01".into(), 0x12);
        let host = |addr: &str, profile| KvmState { pc: 0, bt: Some(addr.into()), profile };
        assert_eq!(b.target(&host("C4:21:3B:6E:5A:01", 0)), Some(0x12));
        assert_eq!(b.target(&host("C4:21:3B:6E:5A:02", 0)), Some(0x0F), "no input of its own: stays on the PC");
        assert_eq!(b.target(&host("C4:21:3B:6E:5A:01", 1)), Some(0x11), "pinned to PC2");
        assert_eq!(b.target(&host("C4:21:3B:6E:5A:01", 2)), None);
        // Saved and read back; a file without hosts doesn't mention them.
        let text = serde_json::to_string(&l).unwrap();
        assert!(text.contains("\"bt_inputs\":{\"C4:21:3B:6E:5A:01\":18}"), "{text}");
        assert_eq!(serde_json::from_str::<Local>(&text).unwrap(), l);
        l.displays[0].bt_inputs.clear();
        assert!(!serde_json::to_string(&l).unwrap().contains("bt_inputs"));
    }
}
