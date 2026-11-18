//! DDC/CI on Linux through `/dev/i2c-*` (the `i2c-dev` module).
//!
//! Only buses that belong to a connected display output are touched: the DRM connectors in `/sys/class/drm` name
//! their DDC bus (the DisplayPort AUX bus inside the connector's directory, else its `ddc` link), and give the EDID
//! without any I²C traffic. Nothing is ever sent to address 0x50 or to the mainboard's SMBus (memory SPD, sensors).

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

use super::mccs::{self, INPUT_SOURCE};
use super::{Channel, Display, Error, REPLY_DELAY};

const I2C_SLAVE: libc::c_ulong = 0x0703;

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
}

impl Channel for Bus {
    fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> Result<usize, Error> {
        let io = |e: std::io::Error| Error::Io(e.to_string());
        self.0.write_all(request).map_err(io)?;
        std::thread::sleep(REPLY_DELAY);
        if reply.is_empty() { Ok(0) } else { self.0.read(reply).map_err(io) }
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
        super::read_display(&mut d, Bus::open(&c.bus), capabilities);
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
    bus_of(id)?.set_vcp(INPUT_SOURCE, value)
}
