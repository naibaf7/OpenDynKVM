//! One `opendyn-kvm` per user: a second start (the menu entry while the tray copy from login runs) asks the running one
//! to open its window, then exits.
//!
//! - **Linux:** a Unix socket in `$XDG_RUNTIME_DIR`. The second start sends `open <token>`, with its XDG activation
//!   token, so the running copy's window may come to the front on Wayland.
//! - **Windows:** a named event in the session; the second start lets the first take the foreground, then sets it.

use std::sync::mpsc::Sender;

use crate::tray::TrayAction;
use crate::worker::Wake;

pub enum Claim {
    /// This is the only copy; hand the listener to [`listen`] (None: no way to listen, run without).
    First(Option<imp::Listener>),
    /// Another copy runs and was asked to open its window.
    Running,
}

/// Claims this user's copy. `sim` keeps the simulator separate; `open` asks a running copy to show its window.
pub fn claim(sim: bool, open: bool) -> Claim {
    imp::claim(if sim { "opendyn-kvm-sim" } else { "opendyn-kvm" }, open)
}

/// Turns later starts into [`TrayAction::Open`].
pub fn listen(l: imp::Listener, tx: Sender<TrayAction>, wake: Wake) {
    imp::listen(l, tx, wake);
}

#[cfg(unix)]
pub mod imp {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};

    use super::*;

    pub struct Listener(UnixListener);

    fn socket(name: &str) -> PathBuf {
        match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
            Some(d) => PathBuf::from(d).join(format!("{name}.sock")),
            None => {
                let user = std::env::var("USER").unwrap_or_default();
                std::env::temp_dir().join(format!("{name}-{user}.sock"))
            }
        }
    }

    pub fn claim(name: &str, open: bool) -> Claim {
        claim_at(&socket(name), open, std::env::var("XDG_ACTIVATION_TOKEN").ok())
    }

    pub fn claim_at(path: &Path, open: bool, token: Option<String>) -> Claim {
        if let Ok(mut s) = UnixStream::connect(path) {
            if open {
                let _ = writeln!(s, "open {}", token.unwrap_or_default());
            }
            return Claim::Running;
        }
        // Nobody answers: whatever is there is left over from a crash.
        let _ = std::fs::remove_file(path);
        Claim::First(UnixListener::bind(path).ok().map(Listener))
    }

    pub fn listen(l: Listener, tx: Sender<TrayAction>, wake: Wake) {
        std::thread::spawn(move || {
            for stream in l.0.incoming().flatten() {
                let mut line = String::new();
                let _ = BufReader::new(stream).read_line(&mut line);
                if let Some(token) = line.trim_end().strip_prefix("open") {
                    let token = token.trim();
                    let _ = tx.send(TrayAction::Open((!token.is_empty()).then(|| token.to_string())));
                    wake();
                }
            }
        });
    }
}

#[cfg(windows)]
pub mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForSingleObject};
    use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};

    use super::*;

    pub struct Listener(HANDLE);

    // SAFETY: an event handle may be waited on from any thread.
    unsafe impl Send for Listener {}

    pub fn claim(name: &str, open: bool) -> Claim {
        let wide: Vec<u16> = format!("Local\\{name}.open").encode_utf16().chain([0]).collect();
        // SAFETY: a valid NUL-terminated name; auto-reset event, initially unset.
        let h = unsafe { CreateEventW(core::ptr::null(), 0, 0, wide.as_ptr()) };
        if h.is_null() {
            return Claim::First(None);
        }
        // SAFETY: plain calls on the handle just created.
        unsafe {
            if GetLastError() == ERROR_ALREADY_EXISTS {
                if open {
                    AllowSetForegroundWindow(ASFW_ANY);
                    SetEvent(h);
                }
                CloseHandle(h);
                return Claim::Running;
            }
        }
        Claim::First(Some(Listener(h)))
    }

    pub fn listen(l: Listener, tx: Sender<TrayAction>, wake: Wake) {
        std::thread::spawn(move || {
            let l = l;
            // SAFETY: the handle stays open for the life of the program.
            while unsafe { WaitForSingleObject(l.0, INFINITE) } == WAIT_OBJECT_0 {
                let _ = tx.send(TrayAction::Open(None));
                wake();
            }
        });
    }
}

#[cfg(not(any(unix, windows)))]
pub mod imp {
    use super::*;

    pub struct Listener;

    pub fn claim(_name: &str, _open: bool) -> Claim {
        Claim::First(None)
    }

    pub fn listen(_l: Listener, _tx: Sender<TrayAction>, _wake: Wake) {}
}

#[cfg(all(test, unix))]
mod tests {
    use std::sync::Arc;
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn second_start_opens_the_first_copy_with_its_token() {
        let path = std::env::temp_dir().join(format!("odkvmctl-single-{}.sock", std::process::id()));
        let Claim::First(Some(l)) = imp::claim_at(&path, true, None) else { panic!("first claim") };
        let (tx, rx) = channel();
        listen(l, tx, Arc::new(|| {}));
        assert!(matches!(imp::claim_at(&path, true, Some("tok-1".into())), Claim::Running));
        let got = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        assert_eq!(got, TrayAction::Open(Some("tok-1".into())));
        // A start from autostart while a copy runs just leaves.
        assert!(matches!(imp::claim_at(&path, false, None), Claim::Running));
        assert!(rx.recv_timeout(std::time::Duration::from_millis(200)).is_err());
        let _ = std::fs::remove_file(&path);
    }
}
