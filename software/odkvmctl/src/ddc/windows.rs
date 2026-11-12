//! DDC/CI on Windows through the monitor configuration API (dxva2): no driver or administrator rights needed.
//!
//! A display's ID is its monitor device interface without the class GUID, e.g. `DISPLAY#ODK41A8#5&3a0bbd8&0&UID4352`:
//! the same for that monitor on that output across restarts.

use windows_sys::Win32::Devices::Display::{
    CapabilitiesRequestAndCapabilitiesReply, DestroyPhysicalMonitors, GetCapabilitiesStringLength,
    GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, GetVCPFeatureAndVCPFeatureReply,
    PHYSICAL_MONITOR, SetVCPFeature,
};
use windows_sys::Win32::Foundation::{LPARAM, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
use windows_sys::core::BOOL;

use super::mccs::{self, INPUT_SOURCE, ReplyError};
use super::{Display, Error};

fn text(w: &[u16]) -> String {
    String::from_utf16_lossy(&w[..w.iter().position(|c| *c == 0).unwrap_or(w.len())])
}

/// One physical monitor, released on drop.
struct Monitor {
    id: String,
    name: String,
    handle: PHYSICAL_MONITOR,
}

impl Drop for Monitor {
    fn drop(&mut self) {
        // SAFETY: a handle from GetPhysicalMonitorsFromHMONITOR, released once.
        unsafe { DestroyPhysicalMonitors(1, &self.handle) };
    }
}

unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
    // SAFETY: `list` is the Vec passed to EnumDisplayMonitors below, alive for the call.
    unsafe { (*(list as *mut Vec<HMONITOR>)).push(h) };
    1
}

fn monitors() -> Vec<Monitor> {
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only pushes into `handles`.
    unsafe {
        EnumDisplayMonitors(
            core::ptr::null_mut(),
            core::ptr::null(),
            Some(collect),
            &mut handles as *mut Vec<HMONITOR> as LPARAM,
        )
    };
    let mut out = Vec::new();
    for h in handles {
        // SAFETY: plain calls with correctly sized and initialised structures.
        unsafe {
            let mut info: MONITORINFOEXW = core::mem::zeroed();
            info.monitorInfo.cbSize = core::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(h, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) == 0 {
                continue;
            }
            let mut dev: DISPLAY_DEVICEW = core::mem::zeroed();
            dev.cb = core::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            let (interface, friendly) =
                if EnumDisplayDevicesW(info.szDevice.as_ptr(), 0, &mut dev, EDD_GET_DEVICE_INTERFACE_NAME) != 0 {
                    (text(&dev.DeviceID), text(&dev.DeviceString))
                } else {
                    (text(&info.szDevice), String::new())
                };
            // "\\?\DISPLAY#ODK41A8#5&3a0bbd8&0&UID4352#{e6f07b5f-…}" → "DISPLAY#ODK41A8#5&3a0bbd8&0&UID4352"
            let base = interface.trim_start_matches(r"\\?\").split("#{").next().unwrap_or("").to_string();
            let mut n = 0u32;
            if GetNumberOfPhysicalMonitorsFromHMONITOR(h, &mut n) == 0 || n == 0 {
                continue;
            }
            let mut phys = vec![core::mem::zeroed::<PHYSICAL_MONITOR>(); n as usize];
            if GetPhysicalMonitorsFromHMONITOR(h, n, phys.as_mut_ptr()) == 0 {
                continue;
            }
            for (k, p) in phys.into_iter().enumerate() {
                let id = if n > 1 { format!("{base}#{k}") } else { base.clone() };
                // PHYSICAL_MONITOR is packed: copy the field before borrowing it.
                let raw = p.szPhysicalMonitorDescription;
                let desc = text(&raw);
                let name =
                    if friendly.is_empty() || friendly == "Generic PnP Monitor" { desc } else { friendly.clone() };
                out.push(Monitor { id, name, handle: p });
            }
        }
    }
    out
}

impl Monitor {
    fn input(&self) -> Result<u16, Error> {
        let (mut cur, mut max) = (0u32, 0u32);
        // SAFETY: a valid physical monitor handle; the code type is not wanted.
        let ok = unsafe {
            GetVCPFeatureAndVCPFeatureReply(
                self.handle.hPhysicalMonitor,
                INPUT_SOURCE,
                core::ptr::null_mut(),
                &mut cur,
                &mut max,
            )
        };
        if ok == 0 { Err(Error::Ddc(ReplyError::Empty)) } else { Ok(cur as u16) }
    }

    fn capabilities(&self) -> Option<String> {
        let mut len = 0u32;
        // SAFETY: a valid handle; the buffer holds `len` bytes.
        unsafe {
            if GetCapabilitiesStringLength(self.handle.hPhysicalMonitor, &mut len) == 0 || len == 0 {
                return None;
            }
            let mut buf = vec![0u8; len as usize];
            if CapabilitiesRequestAndCapabilitiesReply(self.handle.hPhysicalMonitor, buf.as_mut_ptr(), len) == 0 {
                return None;
            }
            Some(String::from_utf8_lossy(&buf).trim_end_matches('\0').to_string())
        }
    }
}

pub fn displays(capabilities: bool) -> Result<Vec<Display>, Error> {
    Ok(monitors()
        .iter()
        .map(|m| {
            let mut d =
                Display { id: m.id.clone(), name: m.name.clone(), inputs: Vec::new(), current: None, error: None };
            match m.input() {
                Ok(v) => d.current = Some(v),
                Err(e) => d.error = Some(e.to_string()),
            }
            if capabilities && d.current.is_some() {
                let caps = m.capabilities().map(|s| mccs::parse_capabilities(&s)).unwrap_or_default();
                if let Some(model) = caps.model {
                    d.name = model;
                }
                d.inputs = caps.inputs;
            }
            d
        })
        .collect())
}

fn find(id: &str) -> Result<Monitor, Error> {
    monitors().into_iter().find(|m| m.id == id).ok_or_else(|| Error::NotFound(id.to_string()))
}

pub fn current_input(id: &str) -> Result<u16, Error> {
    find(id)?.input()
}

pub fn set_input(id: &str, value: u16) -> Result<(), Error> {
    let m = find(id)?;
    // SAFETY: a valid physical monitor handle.
    if unsafe { SetVCPFeature(m.handle.hPhysicalMonitor, INPUT_SOURCE, value as u32) } == 0 {
        return Err(Error::Ddc(ReplyError::Empty));
    }
    Ok(())
}
