//! 64-byte report transports: the KVM's vendor HID interface, its control service over Bluetooth LE ([`crate::ble`]),
//! the browser's WebHID and Web Bluetooth ([`crate::webio`]), or the built-in simulator.
//!
//! One trait for all of them, blocking on a computer and async in a browser (software.md §9 "On the web"), where
//! WebHID and Web Bluetooth answer only once the page's event loop has run: `maybe_async` writes the one from the other.

#[cfg(not(target_arch = "wasm32"))]
use std::ffi::CString;
use std::time::Duration;

use kvm_proto::ctrl::REPORT_LEN;
#[cfg(not(target_arch = "wasm32"))]
use kvm_proto::ctrl::{USAGE, USAGE_PAGE};

pub type Report = [u8; REPORT_LEN];

/// `Send` where the client runs on a thread of its own; nothing in a browser, whose transports hold the page's objects.
#[cfg(not(target_arch = "wasm32"))]
pub trait MaybeSend: Send {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send> MaybeSend for T {}
#[cfg(target_arch = "wasm32")]
pub trait MaybeSend {}
#[cfg(target_arch = "wasm32")]
impl<T> MaybeSend for T {}

#[maybe_async::maybe_async(?Send)]
pub trait Transport: MaybeSend {
    async fn write(&mut self, report: &Report) -> Result<(), String>;
    /// One report, or `None` after `timeout`.
    async fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String>;
    fn describe(&self) -> String;
    /// How the KVM is reached.
    fn link(&self) -> Link {
        Link::Usb
    }
    /// Requests the client may have under way at once ([`crate::Client`]'s batches): one over USB, where each answer
    /// comes within a millisecond, more over Bluetooth, where each takes two connection events.
    fn window(&self) -> usize {
        1
    }
    /// The simulator's keyboard: boot-protocol reports (modifiers, 0, six keys) sent here are typed on it.
    fn simulated_keyboard(&self) -> Option<std::sync::mpsc::Sender<[u8; 8]>> {
        None
    }
}

/// How a KVM's control protocol reaches this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    /// A PC port's control interface (vendor HID; in a browser, WebHID).
    Usb,
    /// The Bluetooth module's control service, for a computer paired as one of its hosts (in a browser, Web
    /// Bluetooth).
    Bluetooth,
    /// The built-in simulator.
    Simulated,
}

/// A KVM control interface seen by the OS.
#[derive(Debug, Clone)]
#[cfg(not(target_arch = "wasm32"))]
pub struct Found {
    pub link: Link,
    /// The HID device's path, or the Bluetooth device's (BlueZ's object path of the control service, the system's
    /// device id elsewhere).
    pub path: CString,
    pub vid: u16,
    pub pid: u16,
    pub product: String,
    pub serial: String,
}

/// Every KVM control interface: the HID ones, matched by usage page and usage (so identity mode, with a borrowed
/// VID/PID, is found too), then the KVMs whose Bluetooth module this computer is connected to as a host. (None in a
/// browser: there the page asks the browser for one, [`crate::webio`].)
#[cfg(not(target_arch = "wasm32"))]
pub fn find() -> Result<Vec<Found>, String> {
    let mut found = find_hid()?;
    found.extend(crate::ble::find());
    Ok(found)
}

/// The KVM control interfaces on USB.
#[cfg(not(target_arch = "wasm32"))]
pub fn find_hid() -> Result<Vec<Found>, String> {
    let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
    Ok(api
        .device_list()
        .filter(|d| d.usage_page() == USAGE_PAGE && d.usage() == USAGE)
        .map(|d| Found {
            link: Link::Usb,
            path: d.path().to_owned(),
            vid: d.vendor_id(),
            pid: d.product_id(),
            product: d.product_string().unwrap_or_default().to_string(),
            serial: d.serial_number().unwrap_or_default().to_string(),
        })
        .collect())
}

/// Opens what [`find`] found.
#[cfg(not(target_arch = "wasm32"))]
pub fn open(f: &Found) -> Result<Box<dyn Transport>, String> {
    Ok(match f.link {
        Link::Usb => Box::new(HidTransport::open(f)?),
        Link::Bluetooth => Box::new(crate::ble::BleTransport::open(f)?),
        Link::Simulated => Box::new(crate::sim::SimTransport::new()),
    })
}

/// The first KVM found: over USB if this computer has one there, else over Bluetooth.
#[cfg(not(target_arch = "wasm32"))]
pub fn open_first() -> Result<Box<dyn Transport>, String> {
    if let Ok(t) = HidTransport::open_first() {
        return Ok(Box::new(t));
    }
    let found = crate::ble::find();
    let f = found.first().ok_or_else(|| crate::fl!("no-kvm-found"))?;
    Ok(Box::new(crate::ble::BleTransport::open(f)?))
}

#[cfg(not(target_arch = "wasm32"))]
pub struct HidTransport {
    dev: hidapi::HidDevice,
    desc: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl HidTransport {
    pub fn open(f: &Found) -> Result<Self, String> {
        let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
        let dev = api.open_path(&f.path).map_err(|e| {
            if cfg!(target_os = "linux") { crate::fl!("hid-open-linux", error = e.to_string()) } else { e.to_string() }
        })?;
        Ok(Self { dev, desc: format!("{} {:04x}:{:04x} {}", f.product, f.vid, f.pid, f.path.to_string_lossy()) })
    }

    /// The first control interface on USB (the cursor placer's: it needs a PC's).
    pub fn open_first() -> Result<Self, String> {
        let found = find_hid()?;
        let f = found.first().ok_or_else(|| crate::fl!("no-kvm-found"))?;
        Self::open(f)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Transport for HidTransport {
    fn write(&mut self, report: &Report) -> Result<(), String> {
        // hidapi wants the report ID first; the control interface uses none.
        let mut buf = [0u8; REPORT_LEN + 1];
        buf[1..].copy_from_slice(report);
        self.dev.write(&buf).map(|_| ()).map_err(|e| e.to_string())
    }

    fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String> {
        let mut buf = [0u8; REPORT_LEN];
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        match self.dev.read_timeout(&mut buf, ms).map_err(|e| e.to_string())? {
            0 => Ok(None),
            _ => Ok(Some(buf)),
        }
    }

    fn describe(&self) -> String {
        self.desc.clone()
    }
}
