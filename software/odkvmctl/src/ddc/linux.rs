//! DDC/CI on Linux through `/dev/i2c-*` (the `i2c-dev` module).
//!
//! Only buses that belong to a connected display output are touched: the DRM connectors in `/sys/class/drm` name
//! their DDC bus (the DisplayPort AUX bus inside the connector's directory, else its `ddc` link), and give the EDID
//! without any I²C traffic. Nothing is ever sent to address 0x50 or to the mainboard's SMBus (memory SPD, sensors).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::mccs::{self, INPUT_SOURCE, ReplyError};
use super::{Display, Error};

const I2C_SLAVE: libc::c_ulong = 0x0703;
/// DDC/CI 1.1: wait at least 40 ms before reading a reply, 50 ms after a write without one.
const REPLY_DELAY: Duration = Duration::from_millis(50);
const RETRIES: usize = 3;

struct Connector {
    name: String,
    edid: Vec<u8>,
    bus: PathBuf,
}

fn connectors() -> Vec<Connector> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir("/sys/class/drm") else { return out };
    for entry in dir.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("card") || !name.contains('-') {
            continue;
        }
        if fs::read_to_string(path.join("status")).map(|s| s.trim() != "connected").unwrap_or(true) {
            continue;
        }
        let Ok(edid) = fs::read(path.join("edid")) else { continue };
        // DisplayPort: the AUX channel's I²C bus is a subdirectory; HDMI, DVI, VGA: the `ddc` link.
        let aux = fs::read_dir(&path).ok().and_then(|d| {
            d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).find(|n| n.starts_with("i2c-"))
        });
        let ddc =
            fs::read_link(path.join("ddc")).ok().and_then(|l| l.file_name().map(|n| n.to_string_lossy().into_owned()));
        if let Some(bus) = aux.or(ddc) {
            out.push(Connector { name, edid, bus: Path::new("/dev").join(bus) });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

struct Bus(File);

impl Bus {
    fn open(path: &Path) -> Result<Bus, Error> {
        let f = OpenOptions::new().read(true).write(true).open(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                Error::Permission(crate::fl!("ddc-no-access", path = path.display().to_string()))
            } else if e.kind() == std::io::ErrorKind::NotFound {
                Error::Permission(crate::fl!("ddc-no-i2c-dev", path = path.display().to_string()))
            } else {
                Error::Io(format!("{}: {e}", path.display()))
            }
        })?;
        // SAFETY: I2C_SLAVE takes the 7-bit address as its argument.
        if unsafe { libc::ioctl(f.as_raw_fd(), I2C_SLAVE, mccs::DISPLAY_ADDR as libc::c_ulong) } < 0 {
            return Err(Error::Io(format!("{}: {}", path.display(), std::io::Error::last_os_error())));
        }
        Ok(Bus(f))
    }

    fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> std::io::Result<usize> {
        self.0.write_all(request)?;
        std::thread::sleep(REPLY_DELAY);
        if reply.is_empty() { Ok(0) } else { self.0.read(reply) }
    }

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
                Err(e) => last = Error::Io(e.to_string()),
            }
            std::thread::sleep(REPLY_DELAY);
        }
        Err(last)
    }

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

fn describe(c: &Connector) -> (String, String) {
    let connector = c.name.split_once('-').map_or(c.name.as_str(), |(_, n)| n).to_string();
    match mccs::parse_edid(&c.edid) {
        Some(e) => {
            let mut id = e.id();
            if e.serial == 0 && e.serial_text.is_none() {
                // Two identical monitors without serial numbers: tell them apart by connector.
                id = format!("{id}@{connector}");
            }
            (id, format!("{} ({connector})", e.display_name()))
        }
        None => (format!("@{connector}"), connector),
    }
}

pub fn displays(capabilities: bool) -> Result<Vec<Display>, Error> {
    let mut out = Vec::new();
    for c in connectors() {
        let (id, name) = describe(&c);
        let mut d = Display { id, name, inputs: Vec::new(), current: None, error: None };
        match Bus::open(&c.bus) {
            Ok(mut bus) => {
                match bus.get_vcp(INPUT_SOURCE) {
                    Ok((cur, _)) => d.current = Some(cur),
                    Err(e) => d.error = Some(e.to_string()),
                }
                if capabilities && d.current.is_some() {
                    let caps = bus.capabilities().map(|s| mccs::parse_capabilities(&s)).unwrap_or_default();
                    d.inputs = caps.inputs;
                }
            }
            Err(e) => d.error = Some(e.to_string()),
        }
        out.push(d);
    }
    Ok(out)
}

fn bus_of(id: &str) -> Result<Bus, Error> {
    let c = connectors().into_iter().find(|c| describe(c).0 == id).ok_or_else(|| Error::NotFound(id.to_string()))?;
    Bus::open(&c.bus)
}

pub fn current_input(id: &str) -> Result<u16, Error> {
    bus_of(id)?.get_vcp(INPUT_SOURCE).map(|(cur, _)| cur)
}

pub fn set_input(id: &str, value: u16) -> Result<(), Error> {
    let mut bus = bus_of(id)?;
    bus.exchange(&mccs::set_vcp_request(INPUT_SOURCE, value), &mut []).map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}
