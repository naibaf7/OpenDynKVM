//! Start `opendyn-kvm --tray` when the user logs in (software.md §9). A setting of this computer and user, not of the
//! KVM, whose config both PCs share.
//!
//! - **Windows:** the value [`NAME`] under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, as the installer
//!   writes it. If the entry was switched off in Task Manager (`...\Explorer\StartupApproved\Run`), it counts as off,
//!   and switching it on here clears that.
//! - **Linux:** an XDG autostart entry, `$XDG_CONFIG_HOME/autostart/opendyn-kvm.desktop` (`~/.config/...`), with
//!   this executable's path. If a package update replaced the executable while this copy runs, Linux reports its path
//!   with " (deleted)" after it; the entry gets the path without, which is where the new one is.

use std::io;
use std::path::Path;

/// Name of the Run value (Windows) and of the desktop entry (Linux, plus `.desktop`).
pub const NAME: &str = "OpenDynKVM";
/// The command line option that starts in the tray, without a window.
pub const TRAY_ARG: &str = "--tray";

/// Whether this platform has autostart support here.
pub const fn supported() -> bool {
    cfg!(any(windows, target_os = "linux"))
}

/// Whether `opendyn-kvm` starts when the user logs in.
pub fn is_enabled() -> bool {
    imp::is_enabled()
}

/// Starts this `opendyn-kvm` executable at login, in the tray (`enabled`), or stops doing so.
pub fn set(enabled: bool) -> io::Result<()> {
    imp::set(enabled, &std::env::current_exe()?)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    fn dir() -> Option<std::path::PathBuf> {
        crate::local::config_home().map(|c| c.join("autostart"))
    }

    pub fn is_enabled() -> bool {
        dir().is_some_and(|d| enabled_in(&d))
    }

    pub fn set(enabled: bool, exe: &Path) -> io::Result<()> {
        let d = dir().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no HOME or XDG_CONFIG_HOME"))?;
        set_in(&d, enabled, &replaced_exe(exe))
    }
}

/// `exe` as `/proc/self/exe` gives it, without the " (deleted)" Linux adds once the file was replaced.
#[cfg(any(target_os = "linux", test))]
fn replaced_exe(exe: &Path) -> std::path::PathBuf {
    let s = exe.to_string_lossy();
    match s.strip_suffix(" (deleted)") {
        Some(path) => path.into(),
        None => exe.into(),
    }
}

#[cfg(any(target_os = "linux", test))]
fn entry_path(dir: &Path) -> std::path::PathBuf {
    dir.join("opendyn-kvm.desktop")
}

/// An argument for a desktop entry's `Exec=` key (desktop entry specification, "The Exec key").
pub fn exec_quote(arg: &str) -> String {
    if arg.chars().all(|c| c.is_ascii_alphanumeric() || "/._-+:,".contains(c)) {
        return arg.into();
    }
    let mut out = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    // Inside a desktop entry value a backslash itself is escaped once more.
    out.replace('\\', "\\\\")
}

#[cfg(any(target_os = "linux", test))]
fn entry(exe: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={NAME}\nComment=KVM switch control in the tray\n\
         Exec={} {TRAY_ARG}\nIcon=opendyn-kvm\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_quote(&exe.to_string_lossy())
    )
}

#[cfg(any(target_os = "linux", test))]
fn enabled_in(dir: &Path) -> bool {
    std::fs::read_to_string(entry_path(dir)).is_ok_and(|text| {
        let off = |line: &str| {
            let line = line.trim().replace(' ', "");
            line.eq_ignore_ascii_case("Hidden=true") || line.eq_ignore_ascii_case("X-GNOME-Autostart-enabled=false")
        };
        !text.lines().any(off)
    })
}

#[cfg(any(target_os = "linux", test))]
fn set_in(dir: &Path, enabled: bool, exe: &Path) -> io::Result<()> {
    if enabled {
        std::fs::create_dir_all(dir)?;
        std::fs::write(entry_path(dir), entry(exe))
    } else {
        match std::fs::remove_file(entry_path(dir)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    use super::*;

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain([0]).collect()
    }

    /// The command the Run value holds (the installer writes the same).
    pub fn command(exe: &Path) -> String {
        format!("\"{}\" {TRAY_ARG}", exe.display())
    }

    pub fn is_enabled() -> bool {
        is_enabled_named(NAME)
    }

    pub fn set(enabled: bool, exe: &Path) -> io::Result<()> {
        set_named(NAME, enabled, exe)
    }

    /// [`is_enabled`] for the Run value `name` (the tests use one of their own, and leave the user's alone).
    pub(super) fn is_enabled_named(name: &str) -> bool {
        let (run, approved, name) = (wide(RUN), wide(APPROVED), wide(name));
        let mut len = 0u32;
        // SAFETY: valid NUL-terminated strings; no data buffer, so only the size is written.
        let present = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                run.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                &mut len,
            )
        } == ERROR_SUCCESS;
        // Task Manager's switch: first byte 2 = enabled, 3 = disabled.
        let mut flags = [0u8; 12];
        let mut flen = flags.len() as u32;
        // SAFETY: `flags` holds `flen` bytes.
        let read = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                approved.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_BINARY,
                core::ptr::null_mut(),
                flags.as_mut_ptr() as *mut c_void,
                &mut flen,
            )
        } == ERROR_SUCCESS;
        present && !(read && flen > 0 && flags[0] & 1 == 1)
    }

    pub(super) fn set_named(name: &str, enabled: bool, exe: &Path) -> io::Result<()> {
        let (run, approved, name) = (wide(RUN), wide(APPROVED), wide(name));
        let err = |e: u32| if e == ERROR_SUCCESS { Ok(()) } else { Err(io::Error::from_raw_os_error(e as i32)) };
        if enabled {
            let cmd = wide(&command(exe));
            // SAFETY: `cmd` is a NUL-terminated UTF-16 string of `cmd.len()` units; the key is created if missing.
            err(unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    run.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    cmd.as_ptr() as *const c_void,
                    (cmd.len() * 2) as u32,
                )
            })?;
            // A "disabled" left behind by Task Manager would otherwise keep it off.
            // SAFETY: valid NUL-terminated strings.
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, approved.as_ptr(), name.as_ptr()) };
            Ok(())
        } else {
            // SAFETY: valid NUL-terminated strings.
            match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, run.as_ptr(), name.as_ptr()) } {
                ERROR_FILE_NOT_FOUND => Ok(()),
                e => err(e),
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::*;

    pub fn is_enabled() -> bool {
        false
    }

    pub fn set(_enabled: bool, _exe: &Path) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "autostart is only set up on Windows and Linux"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_entry_round_trip() {
        let dir = std::env::temp_dir().join(format!("odkvmctl-autostart-{}", std::process::id()));
        let exe = Path::new("/opt/Open Dyn/opendyn-kvm");
        assert!(!enabled_in(&dir));
        set_in(&dir, true, exe).unwrap();
        assert!(enabled_in(&dir));
        let text = std::fs::read_to_string(entry_path(&dir)).unwrap();
        assert!(text.contains("Exec=\"/opt/Open Dyn/opendyn-kvm\" --tray\n"), "{text}");
        // Switched off by the desktop's own settings: counts as off.
        std::fs::write(entry_path(&dir), text + "Hidden=true\n").unwrap();
        assert!(!enabled_in(&dir));
        set_in(&dir, false, exe).unwrap();
        set_in(&dir, false, exe).unwrap();
        assert!(!entry_path(&dir).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exec_arguments_are_quoted() {
        assert_eq!(exec_quote("/usr/bin/opendyn-kvm"), "/usr/bin/opendyn-kvm");
        assert_eq!(exec_quote("/a b/$x"), "\"/a b/\\\\$x\"");
    }

    #[test]
    fn a_replaced_executable_is_started_from_its_path() {
        let gone = Path::new("/usr/bin/opendyn-kvm (deleted)");
        assert_eq!(replaced_exe(gone), Path::new("/usr/bin/opendyn-kvm"));
        assert_eq!(replaced_exe(Path::new("/opt/x/opendyn-kvm")), Path::new("/opt/x/opendyn-kvm"));
    }

    /// Under a Run value of its own: the user's own setting stays as it is.
    #[cfg(windows)]
    #[test]
    fn run_value_round_trip() {
        let name = format!("{NAME}-test-{}", std::process::id());
        let exe = Path::new(r"C:\Program Files\OpenDynKVM\opendyn-kvm.exe");
        let before = is_enabled();
        imp::set_named(&name, true, exe).unwrap();
        assert!(imp::is_enabled_named(&name));
        imp::set_named(&name, false, exe).unwrap();
        assert!(!imp::is_enabled_named(&name));
        imp::set_named(&name, false, exe).unwrap();
        assert_eq!(is_enabled(), before);
    }
}
