//! 64-byte report transports: the KVM's vendor HID interface, or the built-in simulator.

use std::ffi::CString;
use std::time::Duration;

use kvm_proto::ctrl::{REPORT_LEN, USAGE, USAGE_PAGE};

pub type Report = [u8; REPORT_LEN];

pub trait Transport: Send {
    fn write(&mut self, report: &Report) -> Result<(), String>;
    /// One report, or `None` after `timeout`.
    fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String>;
    fn describe(&self) -> String;
    /// The simulator's keyboard: boot-protocol reports (modifiers, 0, six keys) sent here are typed on it.
    fn simulated_keyboard(&self) -> Option<std::sync::mpsc::Sender<[u8; 8]>> {
        None
    }
}

/// A KVM control interface seen by the OS.
#[derive(Debug, Clone)]
pub struct Found {
    pub path: CString,
    pub vid: u16,
    pub pid: u16,
    pub product: String,
    pub serial: String,
}

/// Every KVM control interface, matched by usage page and usage (so identity mode, with a borrowed VID/PID, is found
/// too).
pub fn find() -> Result<Vec<Found>, String> {
    let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
    Ok(api
        .device_list()
        .filter(|d| d.usage_page() == USAGE_PAGE && d.usage() == USAGE)
        .map(|d| Found {
            path: d.path().to_owned(),
            vid: d.vendor_id(),
            pid: d.product_id(),
            product: d.product_string().unwrap_or_default().to_string(),
            serial: d.serial_number().unwrap_or_default().to_string(),
        })
        .collect())
}

pub struct HidTransport {
    dev: hidapi::HidDevice,
    desc: String,
}

impl HidTransport {
    pub fn open(f: &Found) -> Result<Self, String> {
        let api = hidapi::HidApi::new().map_err(|e| e.to_string())?;
        let dev = api.open_path(&f.path).map_err(|e| {
            if cfg!(target_os = "linux") { crate::fl!("hid-open-linux", error = e.to_string()) } else { e.to_string() }
        })?;
        Ok(Self { dev, desc: format!("{} {:04x}:{:04x} {}", f.product, f.vid, f.pid, f.path.to_string_lossy()) })
    }

    pub fn open_first() -> Result<Self, String> {
        let found = find()?;
        let f = found.first().ok_or_else(|| crate::fl!("no-kvm-found"))?;
        Self::open(f)
    }
}

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
