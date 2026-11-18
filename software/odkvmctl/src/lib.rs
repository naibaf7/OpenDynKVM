//! Host side of the OpenDynKVM control protocol (software.md §8–§9), shared by the `odkvmctl` CLI and `opendyn-kvm`.

pub mod autostart;
pub mod bootsel;
pub mod client;
pub mod ddc;
pub mod firmware;
pub mod i18n;
pub mod icon;
pub mod image;
pub mod local;
pub mod names;
pub mod placer;
pub mod screens;
pub mod sim;
pub mod transport;

pub use client::{Client, Error};

/// The clock, and waiting: std's, or in a browser (the web build, wasm32: software.md §9 "On the web") the browser's
/// clock, where std's `Instant` has none and a thread can't sleep.
pub mod time {
    pub use std::time::Duration;
    #[cfg(not(target_arch = "wasm32"))]
    pub use std::time::{Instant, SystemTime, UNIX_EPOCH};
    #[cfg(target_arch = "wasm32")]
    pub use web_time::{Instant, SystemTime, UNIX_EPOCH};

    /// Waits `d`. In a browser there is one thread: it spins, so only for the few milliseconds the simulator takes.
    pub fn sleep(d: Duration) {
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(d);
        #[cfg(target_arch = "wasm32")]
        {
            let until = Instant::now() + d;
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
    }
}

/// An environment variable; in a browser, a parameter of the page's address (`?OPENDYN_SIM_BT=1`).
pub fn env_var(name: &str) -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::var(name).ok();
    #[cfg(target_arch = "wasm32")]
    {
        let search = web_sys::window()?.location().search().ok()?;
        search.trim_start_matches('?').split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == name).then(|| v.to_string())
        })
    }
}
