//! DDC/CI on macOS, without a driver or administrator rights (software.md §9).
//!
//! - Apple silicon: the display's AV service (`IOAVService`), private but there since macOS 11 and what every DDC tool
//!   for these Macs uses (m1ddc's way of finding it).
//! - Intel Macs: the I²C buses of the framebuffer the display hangs off (`IOFBCopyI2CInterfaceForBus`, IOGraphics'
//!   public interface).
//!
//! The monitors are Quartz's online displays other than built-in panels. Each one's registry entry comes from the
//! `IODisplayLocation` in CoreDisplay's info dictionary; displays without one (AirPlay, Sidecar) are left out. The id is
//! the EDID's `<maker>-<product>-<serial>`, as on Linux. The private functions are looked up when first needed, so a
//! macOS without them loses DDC/CI only, not the program.
//!
//! Built and checked from Linux only, for both Apple targets; not yet run on a Mac.

use core::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;

use super::mccs::{self, EdidInfo, INPUT_SOURCE};
use super::{Channel, Display, Error};

type CFTypeRef = *const c_void;
/// `io_object_t` (a Mach port name); 0 is none.
type IoObject = u32;

/// `kCFStringEncodingUTF8`.
const UTF8: u32 = 0x0800_0100;
/// `kCFNumberSInt64Type`.
#[cfg(target_arch = "x86_64")]
const SINT64: isize = 4;
/// `kIORegistryIterateRecursively`.
#[cfg(target_arch = "aarch64")]
const RECURSIVELY: u32 = 1;
/// `kIOMainPortDefault`: the default port is MACH_PORT_NULL.
const MAIN_PORT: u32 = 0;
const SERVICE_PLANE: &CStr = c"IOService";

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    #[cfg(target_arch = "x86_64")]
    fn CFNumberGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        external: u8,
    ) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFDictionaryGetCount(d: CFTypeRef) -> isize;
    fn CFDictionaryGetKeysAndValues(d: CFTypeRef, keys: *mut CFTypeRef, values: *mut CFTypeRef);
    fn CFDataGetLength(d: CFTypeRef) -> isize;
    fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    #[cfg(target_arch = "x86_64")]
    fn CFNumberGetValue(n: CFTypeRef, kind: isize, value: *mut c_void) -> u8;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGGetOnlineDisplayList(max: u32, displays: *mut u32, count: *mut u32) -> i32;
    fn CGDisplayIsBuiltin(display: u32) -> u32;
    fn CGDisplayVendorNumber(display: u32) -> u32;
    fn CGDisplayModelNumber(display: u32) -> u32;
    fn CGDisplaySerialNumber(display: u32) -> u32;
    fn CGDisplayUnitNumber(display: u32) -> u32;
}

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOObjectRelease(o: IoObject) -> i32;
    fn IOObjectConformsTo(o: IoObject, class: *const c_char) -> u32;
    fn IORegistryEntryCopyFromPath(main: u32, path: CFTypeRef) -> IoObject;
    fn IORegistryEntryGetParentEntry(e: IoObject, plane: *const c_char, parent: *mut IoObject) -> i32;
    fn IORegistryEntryCreateCFProperty(e: IoObject, key: CFTypeRef, alloc: CFTypeRef, options: u32) -> CFTypeRef;
}

// libSystem's dynamic loader, for the private functions.
unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
}
const RTLD_LAZY: i32 = 1;

/// A function of a system framework, by name (None if the framework or the function isn't there).
fn symbol(framework: &CStr, name: &CStr) -> Option<*mut c_void> {
    // SAFETY: plain calls with NUL-terminated names; a system framework is never unloaded.
    unsafe {
        let h = dlopen(framework.as_ptr(), RTLD_LAZY);
        if h.is_null() {
            return None;
        }
        let f = dlsym(h, name.as_ptr());
        (!f.is_null()).then_some(f)
    }
}

/// A Core Foundation object of ours (from a Create or Copy call), released on drop; null for none.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own one reference to it.
            unsafe { CFRelease(self.0) };
        }
    }
}

impl Cf {
    fn string(s: &str) -> Cf {
        // SAFETY: the bytes are valid UTF-8 for the call.
        Cf(unsafe { CFStringCreateWithBytes(core::ptr::null(), s.as_ptr(), s.len() as isize, UTF8, 0) })
    }

    /// A registry entry's own property (null if it has none).
    fn property(entry: IoObject, key: &str) -> Cf {
        let key = Cf::string(key);
        // SAFETY: a valid entry and key; the result follows the Create rule.
        Cf(unsafe { IORegistryEntryCreateCFProperty(entry, key.0, core::ptr::null(), 0) })
    }
}

/// An I/O Kit object of ours, released on drop; 0 for none.
struct Io(IoObject);

impl Drop for Io {
    fn drop(&mut self) {
        if self.0 != 0 {
            // SAFETY: we own this reference.
            unsafe { IOObjectRelease(self.0) };
        }
    }
}

impl Io {
    fn conforms_to(&self, class: &CStr) -> bool {
        // SAFETY: a valid object and a NUL-terminated class name.
        self.0 != 0 && unsafe { IOObjectConformsTo(self.0, class.as_ptr()) } != 0
    }

    /// Its parent in the service plane (0 if none).
    fn parent(&self) -> Io {
        let mut p = 0;
        // SAFETY: a valid entry; the parent comes with a reference of ours.
        if self.0 == 0 || unsafe { IORegistryEntryGetParentEntry(self.0, SERVICE_PLANE.as_ptr(), &mut p) } != 0 {
            return Io(0);
        }
        Io(p)
    }
}

/// The value of `key` in the dictionary `d` (borrowed; null if `d` isn't a dictionary or lacks it).
///
/// # Safety
/// `d` is null or a live Core Foundation object.
unsafe fn get(d: CFTypeRef, key: &str) -> CFTypeRef {
    // SAFETY: type-checked before the lookup.
    unsafe {
        if d.is_null() || CFGetTypeID(d) != CFDictionaryGetTypeID() {
            return core::ptr::null();
        }
        let key = Cf::string(key);
        CFDictionaryGetValue(d, key.0)
    }
}

/// `cf` as text, if it is a string.
///
/// # Safety
/// `cf` is null or a live Core Foundation object.
unsafe fn text(cf: CFTypeRef) -> Option<String> {
    // SAFETY: type-checked; the buffer's size is passed along.
    unsafe {
        if cf.is_null() || CFGetTypeID(cf) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0 as c_char; 512];
        if CFStringGetCString(cf, buf.as_mut_ptr(), buf.len() as isize, UTF8) == 0 {
            return None;
        }
        Some(CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

/// `cf`'s bytes, if it is data.
///
/// # Safety
/// `cf` is null or a live Core Foundation object.
unsafe fn bytes(cf: CFTypeRef) -> Option<Vec<u8>> {
    // SAFETY: type-checked; the pointer and length are the data's own.
    unsafe {
        if cf.is_null() || CFGetTypeID(cf) != CFDataGetTypeID() {
            return None;
        }
        let (p, n) = (CFDataGetBytePtr(cf), CFDataGetLength(cf));
        (!p.is_null() && n > 0).then(|| std::slice::from_raw_parts(p, n as usize).to_vec())
    }
}

/// `cf` as a number, if it is one.
///
/// # Safety
/// `cf` is null or a live Core Foundation object.
#[cfg(target_arch = "x86_64")]
unsafe fn number(cf: CFTypeRef) -> Option<i64> {
    // SAFETY: type-checked; the value is written as a 64-bit integer.
    unsafe {
        if cf.is_null() || CFGetTypeID(cf) != CFNumberGetTypeID() {
            return None;
        }
        let mut v = 0i64;
        (CFNumberGetValue(cf, SINT64, (&mut v as *mut i64).cast()) != 0).then_some(v)
    }
}

/// A name in a dictionary of names by locale (`DisplayProductName`): the US English one, else any.
///
/// # Safety
/// `cf` is null or a live Core Foundation object.
unsafe fn localized(cf: CFTypeRef) -> Option<String> {
    // SAFETY: type-checked (by `get` and `text`); the arrays hold the dictionary's count.
    unsafe {
        if let Some(t) = text(cf).or_else(|| text(get(cf, "en_US"))) {
            return Some(t);
        }
        if cf.is_null() || CFGetTypeID(cf) != CFDictionaryGetTypeID() {
            return None;
        }
        let n = CFDictionaryGetCount(cf).max(0) as usize;
        let (mut keys, mut values) = (vec![core::ptr::null(); n], vec![core::ptr::null(); n]);
        CFDictionaryGetKeysAndValues(cf, keys.as_mut_ptr(), values.as_mut_ptr());
        values.into_iter().find_map(|v| text(v))
    }
}

/// CoreDisplay's info dictionary of a display (private: `CoreDisplay_DisplayCreateInfoDictionary`).
fn info_dictionary(display: u32) -> Option<Cf> {
    type Create = unsafe extern "C" fn(u32) -> CFTypeRef;
    static CREATE: OnceLock<Option<Create>> = OnceLock::new();
    let create = CREATE.get_or_init(|| {
        let f = symbol(
            c"/System/Library/Frameworks/CoreDisplay.framework/CoreDisplay",
            c"CoreDisplay_DisplayCreateInfoDictionary",
        )?;
        // SAFETY: the function's C signature: CFDictionaryRef (CGDirectDisplayID).
        Some(unsafe { std::mem::transmute::<*mut c_void, Create>(f) })
    });
    // SAFETY: a display id Quartz listed; the dictionary follows the Create rule.
    let d = unsafe { (*create)?(display) };
    (!d.is_null()).then_some(Cf(d))
}

/// A monitor as found: its id and name, and its registry path.
struct Monitor {
    id: String,
    name: String,
    location: String,
}

fn monitors() -> Vec<Monitor> {
    let mut list = [0u32; 16];
    let mut n = 0u32;
    // SAFETY: the list holds `list.len()` entries; `n` says how many were written.
    if unsafe { CGGetOnlineDisplayList(list.len() as u32, list.as_mut_ptr(), &mut n) } != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &d in &list[..(n as usize).min(list.len())] {
        // SAFETY: plain calls on a display Quartz just listed.
        if unsafe { CGDisplayIsBuiltin(d) } != 0 {
            continue;
        }
        let Some(info) = info_dictionary(d) else { continue };
        // SAFETY: values borrowed from `info`, which outlives them.
        let (location, edid, product_name) = unsafe {
            (
                text(get(info.0, "IODisplayLocation")),
                bytes(get(info.0, "IODisplayEDID")),
                localized(get(info.0, "DisplayProductName")),
            )
        };
        let Some(location) = location else { continue };
        // The EDID if the dictionary has it; else what Quartz read of it.
        let e = edid.as_deref().and_then(mccs::parse_edid).unwrap_or_else(|| {
            // SAFETY: plain calls on a listed display.
            let (vendor, model, serial) =
                unsafe { (CGDisplayVendorNumber(d), CGDisplayModelNumber(d), CGDisplaySerialNumber(d)) };
            EdidInfo {
                manufacturer: mccs::pnp_id(vendor as u16),
                product: model as u16,
                serial,
                name: None,
                serial_text: None,
            }
        });
        let mut id = e.id();
        if e.serial == 0 && e.serial_text.is_none() {
            // Two identical monitors without serial numbers: tell them apart by Quartz's unit (the framebuffer).
            // SAFETY: a plain call on a listed display.
            id = format!("{id}@{}", unsafe { CGDisplayUnitNumber(d) });
        }
        let name = e.name.clone().or(product_name).unwrap_or_else(|| e.display_name());
        out.push(Monitor { id, name, location });
    }
    out
}

pub fn displays(capabilities: bool) -> Result<Vec<Display>, Error> {
    Ok(monitors()
        .into_iter()
        .map(|m| {
            let mut d = Display { id: m.id, name: m.name, inputs: Vec::new(), current: None, error: None };
            super::read_display(&mut d, link::open(&m.location), capabilities);
            d
        })
        .collect())
}

fn find(id: &str) -> Result<link::Link, Error> {
    let m = monitors().into_iter().find(|m| m.id == id).ok_or_else(|| Error::NotFound(id.to_string()))?;
    link::open(&m.location)
}

pub fn current_input(id: &str) -> Result<u16, Error> {
    find(id)?.get_vcp(INPUT_SOURCE).map(|(cur, _)| cur)
}

pub fn set_input(id: &str, value: u16) -> Result<(), Error> {
    find(id)?.set_vcp(INPUT_SOURCE, value)
}

#[cfg(target_arch = "aarch64")]
mod link {
    //! Apple silicon: the display's framebuffer (the entry at its `IODisplayLocation`) and, after it in the registry,
    //! the AV service proxy (`DCPAVServiceProxy`) whose `Location` is "External". The HDMI port of some models has an
    //! MCDP29xx converter behind it, reached at chip address 0xB7 instead of 0x37.
    //!
    //! Each request is written twice, 10 ms apart, as m1ddc and MonitorControl do: one write alone is sometimes lost.
    //! The reply is read from the same sub-address (0x51).

    use core::ffi::{c_char, c_void};
    use std::sync::OnceLock;
    use std::time::Duration;

    use super::super::mccs::{self, ReplyError};
    use super::super::{Channel, Error, REPLY_DELAY};
    use super::{
        CFTypeRef, Cf, IORegistryEntryCopyFromPath, Io, IoObject, MAIN_PORT, RECURSIVELY, SERVICE_PLANE, symbol, text,
    };

    const WRITES: usize = 2;
    const WRITE_GAP: Duration = Duration::from_millis(10);
    const MCDP29XX_CHIP: u32 = 0xB7;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryGetRootEntry(main: u32) -> IoObject;
        fn IORegistryEntryCreateIterator(e: IoObject, plane: *const c_char, options: u32, it: *mut IoObject) -> i32;
        fn IOIteratorNext(it: IoObject) -> IoObject;
        fn IORegistryEntryGetName(e: IoObject, name: *mut c_char) -> i32;
        fn IORegistryEntryGetRegistryEntryID(e: IoObject, id: *mut u64) -> i32;
        fn IORegistryEntrySearchCFProperty(
            e: IoObject,
            plane: *const c_char,
            key: CFTypeRef,
            alloc: CFTypeRef,
            options: u32,
        ) -> CFTypeRef;
    }

    type Create = unsafe extern "C" fn(CFTypeRef, IoObject) -> CFTypeRef;
    /// `IOAVServiceReadI2C` / `IOAVServiceWriteI2C`: (service, chip address, sub-address, buffer, size) → IOReturn.
    type Transfer = unsafe extern "C" fn(CFTypeRef, u32, u32, *mut c_void, u32) -> i32;

    struct Api {
        create: Create,
        read: Transfer,
        write: Transfer,
    }

    fn api() -> Option<&'static Api> {
        static API: OnceLock<Option<Api>> = OnceLock::new();
        API.get_or_init(|| {
            let iokit = c"/System/Library/Frameworks/IOKit.framework/IOKit";
            let (create, read, write) = (
                symbol(iokit, c"IOAVServiceCreateWithService")?,
                symbol(iokit, c"IOAVServiceReadI2C")?,
                symbol(iokit, c"IOAVServiceWriteI2C")?,
            );
            // SAFETY: the functions' C signatures, as above.
            unsafe {
                Some(Api {
                    create: std::mem::transmute::<*mut c_void, Create>(create),
                    read: std::mem::transmute::<*mut c_void, Transfer>(read),
                    write: std::mem::transmute::<*mut c_void, Transfer>(write),
                })
            }
        })
        .as_ref()
    }

    fn entry_id(e: &Io) -> Option<u64> {
        let mut id = 0;
        // SAFETY: a valid entry.
        (unsafe { IORegistryEntryGetRegistryEntryID(e.0, &mut id) } == 0).then_some(id)
    }

    fn name(e: &Io) -> Option<String> {
        let mut buf = [0 as c_char; 128];
        // SAFETY: `io_name_t` is 128 bytes.
        if unsafe { IORegistryEntryGetName(e.0, buf.as_mut_ptr()) } != 0 {
            return None;
        }
        // SAFETY: NUL-terminated by the call.
        Some(unsafe { core::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().into_owned())
    }

    /// The proxy's parent is an MCDP29xx's (an HDMI port behind that converter).
    fn behind_mcdp29xx(proxy: &Io) -> bool {
        let p = Cf::property(proxy.parent().0, "EPICProviderClass");
        // SAFETY: a property of ours, or null.
        unsafe { text(p.0) }.as_deref() == Some("AppleDCPMCDP29XX")
    }

    pub struct Link {
        service: Cf,
        chip: u32,
        api: &'static Api,
    }

    pub fn open(location: &str) -> Result<Link, Error> {
        let api = api().ok_or(Error::NoChannel)?;
        let path = Cf::string(location);
        // SAFETY: a CFString path; the entry comes with a reference of ours.
        let framebuffer = Io(unsafe { IORegistryEntryCopyFromPath(MAIN_PORT, path.0) });
        let want = (framebuffer.0 != 0).then(|| entry_id(&framebuffer)).flatten().ok_or(Error::NoChannel)?;
        // SAFETY: a plain call; the root comes with a reference of ours.
        let root = Io(unsafe { IORegistryGetRootEntry(MAIN_PORT) });
        let mut it = 0;
        // SAFETY: a valid root entry; the iterator is ours.
        if unsafe { IORegistryEntryCreateIterator(root.0, SERVICE_PLANE.as_ptr(), RECURSIVELY, &mut it) } != 0 {
            return Err(Error::NoChannel);
        }
        let it = Io(it);
        let location_key = Cf::string("Location");
        let mut after_ours = false;
        loop {
            // SAFETY: a valid iterator; each entry comes with a reference of ours.
            let e = Io(unsafe { IOIteratorNext(it.0) });
            if e.0 == 0 {
                return Err(Error::NoChannel);
            }
            if e.conforms_to(c"IOMobileFramebuffer") {
                after_ours = entry_id(&e) == Some(want);
                continue;
            }
            if !after_ours || name(&e).as_deref() != Some("DCPAVServiceProxy") {
                continue;
            }
            // SAFETY: a valid entry and key; the result follows the Create rule.
            let place = Cf(unsafe {
                IORegistryEntrySearchCFProperty(
                    e.0,
                    SERVICE_PLANE.as_ptr(),
                    location_key.0,
                    core::ptr::null(),
                    RECURSIVELY,
                )
            });
            // SAFETY: a property of ours, or null.
            if unsafe { text(place.0) }.as_deref() != Some("External") {
                continue;
            }
            // SAFETY: a valid DCPAVServiceProxy entry; the service follows the Create rule.
            let service = Cf(unsafe { (api.create)(core::ptr::null(), e.0) });
            if service.0.is_null() {
                continue;
            }
            let chip = if behind_mcdp29xx(&e) { MCDP29XX_CHIP } else { u32::from(mccs::DISPLAY_ADDR) };
            return Ok(Link { service, chip, api });
        }
    }

    impl Channel for Link {
        fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> Result<usize, Error> {
            // The first byte (the source address 0x51) goes as the sub-address.
            let (&sub, rest) = request.split_first().ok_or(Error::Ddc(ReplyError::Malformed))?;
            let mut data = rest.to_vec();
            for _ in 0..WRITES {
                std::thread::sleep(WRITE_GAP);
                // SAFETY: a live service; the buffer holds `data.len()` bytes.
                let r = unsafe {
                    (self.api.write)(self.service.0, self.chip, sub.into(), data.as_mut_ptr().cast(), data.len() as u32)
                };
                if r != 0 {
                    return Err(Error::Ddc(ReplyError::Empty));
                }
            }
            std::thread::sleep(REPLY_DELAY);
            if reply.is_empty() {
                return Ok(0);
            }
            // SAFETY: a live service; the buffer holds `reply.len()` bytes.
            let r = unsafe {
                (self.api.read)(self.service.0, self.chip, sub.into(), reply.as_mut_ptr().cast(), reply.len() as u32)
            };
            if r != 0 {
                return Err(Error::Ddc(ReplyError::Empty));
            }
            Ok(reply.len())
        }
    }
}

#[cfg(target_arch = "x86_64")]
mod link {
    //! Intel Macs: the framebuffer the display hangs off (up from the entry at its `IODisplayLocation`) and its I²C
    //! buses (IOGraphics' `IOI2CInterface`), tried in turn. One request is the write and, if a reply is wanted, the
    //! read: the DDC/CI reply transaction where the bus offers it (it reads as long as the reply says), else a plain
    //! read. The driver waits for the reply; AMD's need 30 ms for it (as ddcctl found), the others answer without.

    use core::ffi::{c_char, c_void};

    use super::super::mccs::{self, ReplyError};
    use super::super::{Channel, Error, REPLY_DELAY};
    use super::{Cf, IORegistryEntryCopyFromPath, Io, IoObject, MAIN_PORT, SERVICE_PLANE, number};

    const NO_TRANSACTION: u32 = 0;
    const SIMPLE: u32 = 1;
    const DDC_CI_REPLY: u32 = 2;
    const AMD_REPLY_DELAY_NS: u64 = 30_000_000;
    const REPLY_DELAY_NS: u64 = 10;

    /// `IOI2CRequest` (IOKit/i2c/IOI2CInterface.h, 64-bit layout, `#pragma pack(4)`).
    #[repr(C, packed(4))]
    #[allow(dead_code, reason = "read by the kernel")]
    struct I2cRequest {
        send_transaction_type: u32,
        reply_transaction_type: u32,
        send_address: u32,
        reply_address: u32,
        send_sub_address: u8,
        reply_sub_address: u8,
        reserved_a: [u8; 2],
        min_reply_delay: u64,
        result: i32,
        comm_flags: u32,
        pad_a: u32,
        send_bytes: u32,
        reserved_b: [u32; 2],
        pad_b: u32,
        reply_bytes: u32,
        completion: usize,
        send_buffer: usize,
        reply_buffer: usize,
        reserved_c: [u32; 10],
    }
    const _: () = assert!(core::mem::size_of::<I2cRequest>() == 124);

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IORegistryEntryGetPath(e: IoObject, plane: *const c_char, path: *mut c_char) -> i32;
        fn IOFBGetI2CInterfaceCount(framebuffer: IoObject, count: *mut u32) -> i32;
        fn IOFBCopyI2CInterfaceForBus(framebuffer: IoObject, bus: u32, interface: *mut IoObject) -> i32;
        fn IOI2CInterfaceOpen(interface: IoObject, options: u32, connect: *mut *mut c_void) -> i32;
        fn IOI2CInterfaceClose(connect: *mut c_void, options: u32) -> i32;
        fn IOI2CSendRequest(connect: *mut c_void, options: u32, request: *mut I2cRequest) -> i32;
    }

    pub struct Link {
        framebuffer: Io,
        reply_delay_ns: u64,
    }

    fn buses(framebuffer: &Io) -> u32 {
        let mut n = 0;
        // SAFETY: a valid framebuffer.
        if unsafe { IOFBGetI2CInterfaceCount(framebuffer.0, &mut n) } != 0 { 0 } else { n }
    }

    pub fn open(location: &str) -> Result<Link, Error> {
        let path = Cf::string(location);
        // SAFETY: a CFString path; the entry comes with a reference of ours.
        let mut e = Io(unsafe { IORegistryEntryCopyFromPath(MAIN_PORT, path.0) });
        // The display's entry sits a level or two below its framebuffer's.
        for _ in 0..8 {
            if e.0 == 0 {
                break;
            }
            if e.conforms_to(c"IOFramebuffer") {
                if buses(&e) == 0 {
                    return Err(Error::NoChannel);
                }
                let mut p = [0 as c_char; 512];
                // SAFETY: `io_string_t` is 512 bytes, NUL-terminated by the call.
                let amd = unsafe { IORegistryEntryGetPath(e.0, SERVICE_PLANE.as_ptr(), p.as_mut_ptr()) } == 0
                    && unsafe { core::ffi::CStr::from_ptr(p.as_ptr()) }.to_string_lossy().contains("/AMD");
                let reply_delay_ns = if amd { AMD_REPLY_DELAY_NS } else { REPLY_DELAY_NS };
                return Ok(Link { framebuffer: e, reply_delay_ns });
            }
            e = e.parent();
        }
        Err(Error::NoChannel)
    }

    /// The reply transaction to use on a bus (None: it has no way to read).
    fn reply_type(interface: &Io) -> Option<u32> {
        let p = Cf::property(interface.0, "IOI2CTransactionTypes");
        // A bus that doesn't say: a plain read.
        // SAFETY: a property of ours, or null.
        let types = unsafe { number(p.0) }.unwrap_or(1 << SIMPLE);
        [DDC_CI_REPLY, SIMPLE].into_iter().find(|t| types & (1 << t) != 0)
    }

    impl Channel for Link {
        fn exchange(&mut self, request: &[u8], reply: &mut [u8]) -> Result<usize, Error> {
            let display = u32::from(mccs::DISPLAY_ADDR) << 1;
            for bus in 0..buses(&self.framebuffer) {
                let mut interface = 0;
                // SAFETY: a valid framebuffer; the interface comes with a reference of ours.
                if unsafe { IOFBCopyI2CInterfaceForBus(self.framebuffer.0, bus, &mut interface) } != 0 {
                    continue;
                }
                let interface = Io(interface);
                // SAFETY: all zeroes is a valid (empty) request.
                let mut req: I2cRequest = unsafe { core::mem::zeroed() };
                req.send_transaction_type = SIMPLE;
                req.send_address = display;
                req.send_buffer = request.as_ptr() as usize;
                req.send_bytes = request.len() as u32;
                req.reply_transaction_type = NO_TRANSACTION;
                if !reply.is_empty() {
                    let Some(t) = reply_type(&interface) else { continue };
                    req.reply_transaction_type = t;
                    req.reply_address = display | 1;
                    req.reply_sub_address = request[0];
                    req.reply_buffer = reply.as_mut_ptr() as usize;
                    req.reply_bytes = reply.len() as u32;
                    req.min_reply_delay = self.reply_delay_ns;
                }
                let mut connect = core::ptr::null_mut();
                // SAFETY: a valid interface; the connection is closed again below.
                if unsafe { IOI2CInterfaceOpen(interface.0, 0, &mut connect) } != 0 {
                    continue;
                }
                // SAFETY: an open connection; the request's buffers outlive the (synchronous) call.
                let sent = unsafe { IOI2CSendRequest(connect, 0, &mut req) };
                // SAFETY: closed once.
                unsafe { IOI2CInterfaceClose(connect, 0) };
                if sent == 0 && { req.result } == 0 {
                    if reply.is_empty() {
                        std::thread::sleep(REPLY_DELAY);
                    }
                    return Ok(reply.len());
                }
            }
            Err(Error::Ddc(ReplyError::Empty))
        }
    }
}
