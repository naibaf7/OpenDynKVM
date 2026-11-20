//! The four host slots and their bonds, in the last two sectors of the module's 4 MB flash.
//!
//! The RP2354B reprograms the module from offset 0 up to its image's end (bootloader, partition table, application:
//! well under 1 MB), so the bonds there survive every firmware update. The two sectors alternate: a write goes to the
//! older one with a higher sequence number, and a record whose CRC fails is ignored, so a power loss while writing
//! leaves the previous bonds in force.
//!
//! **Wear:** a save erases one of the two sectors (70 ms typical, 500 ms at most, with the CPU stalled), and the flash
//! is rated for 100 000 erase cycles, so 200 000 saves in all. Saves come only from [`Bonds::set`] and
//! [`Bonds::clear`], and only when a slot really changes: a host paired or re-paired with new keys, a host that forgot
//! the KVM, a slot forgotten here (several at once in one save). A bonded host reconnecting changes nothing and writes
//! nothing, whatever the Bluetooth stack reports. Twenty such changes a day would last 27 years.
//!
//! A host's notification settings ([`Slot::notify`], 0.1.22 on) are saved when they change: once after pairing for a
//! real host, and never on a reconnect (the host writes the same values, or none). A host that kept switching them
//! would save every few seconds, so at most 16 such saves go to flash in one run of the module; after that the module
//! holds them in RAM ([`Bonds::hold`]), they reach flash with the next save there is anyway, and the RP2354B hears of
//! it (`EventKind::SettingsHeld`: a fault in the hosts' software, a warning in the diagnostics).
//!
//! Record: `magic u32 | seq u32 | 4 × slot (48 bytes) | crc32 u32`; a slot is `used u8 | security u8 | address 7 |
//! irk 16 | ltk 16 | notify u8 | pad 6` (`notify` 0: not known yet).
//!
//! The four device slots ([`DeviceBonds`]) live the same way in the two sectors before those, with what a device needs
//! to report at once when it reconnects: its keys, its report map and its reports' handles. A device's save never
//! touches the hosts' sectors, nor theirs the devices'. Saves come only from pairing a device, forgetting one, and a
//! device that lost its bond.

use esp_storage::FlashStorage;
use kvm_proto::link::bt::{MAP_MAX, MAX_DEVICES, MAX_HOSTS, NAME_MAX};
use trouble_host::prelude::*;

const SECTOR: u32 = 4096;
const BASE: u32 = 4 * 1024 * 1024 - 2 * SECTOR;
const DEVICE_BASE: u32 = BASE - 2 * SECTOR;
const MAGIC: u32 = 0x4B56_4D42; // "KVMB"
const SLOT_LEN: usize = 48;
const RECORD_LEN: usize = 8 + MAX_HOSTS * SLOT_LEN + 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub address: [u8; 7],
    pub irk: Option<u128>,
    pub ltk: u128,
    pub authenticated: bool,
    /// The host's notification settings ([`notify`]): a GATT server keeps them for a bonded client, which doesn't
    /// write them again when it reconnects.
    pub notify: u8,
}

/// [`Slot::notify`]: one bit per characteristic that notifies (`ble::cccds`), and whether they are known.
pub mod notify {
    pub const KEYBOARD: u8 = 1 << 0;
    pub const MOUSE: u8 = 1 << 1;
    pub const CONSUMER: u8 = 1 << 2;
    pub const POINTER: u8 = 1 << 3;
    // Bit 4: the battery level.
    /// The bits below are the host's: set once it was seen. Until then the input reports count as on, which is what
    /// every HID host sets.
    pub const KNOWN: u8 = 1 << 7;
    pub const REPORTS: u8 = KEYBOARD | MOUSE | CONSUMER | POINTER;
}

impl Slot {
    /// The host's identity. `address` is as [`Address::to_bytes`] gives it: the type, then the address most significant
    /// byte first, the reverse of [`BdAddr`]'s order. Until 0.1.19 it was taken as stored, so the address came out
    /// reversed: a host that connects with its public address (a PC) was a stranger to the module and to the Bluetooth
    /// stack after every restart of the module, and forgetting it could leave its keys in the stack (2026-11-15).
    pub fn identity(&self) -> Identity {
        // Types 2 and 3 are how the controller reports a resolved public and random identity address.
        let kind = if self.address[0] & 1 == 0 { AddrKind::PUBLIC } else { AddrKind::RANDOM };
        let mut a = [0u8; 6];
        a.copy_from_slice(&self.address[1..]);
        a.reverse();
        Identity {
            addr: Address::new(kind, BdAddr::new(a)),
            irk: self.irk.and_then(core::num::NonZeroU128::new).map(IdentityResolvingKey),
        }
    }

    pub fn bond(&self) -> BondInformation {
        let level = if self.authenticated { SecurityLevel::EncryptedAuthenticated } else { SecurityLevel::Encrypted };
        BondInformation::new(self.identity(), LongTermKey(self.ltk), level, true)
    }

    pub fn from_bond(b: &BondInformation) -> Slot {
        Slot {
            address: b.identity.addr.to_bytes(),
            irk: b.identity.irk.map(|k| k.0.get()),
            ltk: b.ltk.0,
            authenticated: b.security_level == SecurityLevel::EncryptedAuthenticated,
            notify: 0,
        }
    }

    /// The notifications to restore for this host when it reconnects.
    pub fn notifications(&self) -> u8 {
        if self.notify & notify::KNOWN != 0 { self.notify } else { notify::REPORTS }
    }
}

pub struct Bonds {
    flash: FlashStorage<'static>,
    seq: u32,
    pub slots: [Option<Slot>; MAX_HOSTS],
}

fn crc32(data: &[u8]) -> u32 {
    kvm_proto::crc::crc32(data)
}

impl Bonds {
    /// The newest valid record of the two sectors (none: four free slots).
    pub fn load(mut flash: FlashStorage<'static>) -> Bonds {
        let mut best: Option<(u32, [Option<Slot>; MAX_HOSTS])> = None;
        for k in 0..2 {
            let mut r = [0u8; RECORD_LEN];
            if flash.read(BASE + k * SECTOR, &mut r).is_err() {
                continue;
            }
            let word = |at: usize| u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]]);
            if word(0) != MAGIC || word(RECORD_LEN - 4) != crc32(&r[..RECORD_LEN - 4]) {
                continue;
            }
            let seq = word(4);
            if best.as_ref().is_some_and(|(s, _)| *s >= seq) {
                continue;
            }
            let mut slots = [None; MAX_HOSTS];
            for (i, slot) in slots.iter_mut().enumerate() {
                let s = &r[8 + i * SLOT_LEN..8 + (i + 1) * SLOT_LEN];
                if s[0] != 1 {
                    continue;
                }
                let mut address = [0u8; 7];
                address.copy_from_slice(&s[2..9]);
                let irk = u128::from_le_bytes(s[9..25].try_into().unwrap_or_default());
                let ltk = u128::from_le_bytes(s[25..41].try_into().unwrap_or_default());
                *slot = Some(Slot {
                    address,
                    irk: (irk != 0).then_some(irk),
                    ltk,
                    authenticated: s[1] == 2,
                    notify: s[41],
                });
            }
            best = Some((seq, slots));
        }
        let (seq, slots) = best.unwrap_or((0, [None; MAX_HOSTS]));
        Bonds { flash, seq, slots }
    }

    /// Puts `value` in `slot` without saving: it reaches flash with the next save, whichever slot that is for (the wear
    /// guard on notification settings, `ble::store_notifications`).
    pub fn hold(&mut self, slot: usize, value: Option<Slot>) {
        self.slots[slot] = value;
    }

    /// Puts `value` in `slot`, saving only if that changes the slot.
    pub fn set(&mut self, slot: usize, value: Option<Slot>) {
        if self.slots[slot] != value {
            self.slots[slot] = value;
            self.save();
        }
    }

    /// Empties the slots `which` picks, with one save for all of them, and none if they were all free already.
    /// Returns the bonds it removed, by slot.
    pub fn clear(&mut self, which: impl Fn(usize) -> bool) -> [Option<Slot>; MAX_HOSTS] {
        let mut removed = [None; MAX_HOSTS];
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if which(i) {
                removed[i] = slot.take();
            }
        }
        if removed.iter().any(Option::is_some) {
            self.save();
        }
        removed
    }

    /// Writes the slots to the older sector.
    fn save(&mut self) {
        let mut r = [0xFFu8; RECORD_LEN];
        self.seq = self.seq.wrapping_add(1);
        r[..4].copy_from_slice(&MAGIC.to_le_bytes());
        r[4..8].copy_from_slice(&self.seq.to_le_bytes());
        for (i, slot) in self.slots.iter().enumerate() {
            let s = &mut r[8 + i * SLOT_LEN..8 + (i + 1) * SLOT_LEN];
            s.fill(0);
            if let Some(slot) = slot {
                s[0] = 1;
                s[1] = if slot.authenticated { 2 } else { 1 };
                s[2..9].copy_from_slice(&slot.address);
                s[9..25].copy_from_slice(&slot.irk.unwrap_or(0).to_le_bytes());
                s[25..41].copy_from_slice(&slot.ltk.to_le_bytes());
                s[41] = slot.notify;
            }
        }
        let crc = crc32(&r[..RECORD_LEN - 4]);
        r[RECORD_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        // FlashStorage::write erases the sector and writes it back with the record in it.
        let _ = self.flash.write(BASE + (self.seq % 2) * SECTOR, &r);
    }

    /// The slot of a bond with this identity.
    pub fn slot_of(&self, id: &Identity) -> Option<usize> {
        self.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.identity().match_identity(id)))
    }
}

/// The identity in `address`, as [`Address::to_bytes`] gives it (the type, then the address most significant byte
/// first).
pub fn identity_of(address: &[u8; 7], irk: Option<u128>) -> Identity {
    let kind = if address[0] & 1 == 0 { AddrKind::PUBLIC } else { AddrKind::RANDOM };
    let mut a = [0u8; 6];
    a.copy_from_slice(&address[1..]);
    a.reverse();
    Identity {
        addr: Address::new(kind, BdAddr::new(a)),
        irk: irk.and_then(core::num::NonZeroU128::new).map(IdentityResolvingKey),
    }
}

/// Reports a device's HID service has, at most (input, output and feature reports).
pub const MAX_REPORTS: usize = 12;

/// One of a device's reports: its characteristic's value handle, its notification switch (0: none), its report ID and
/// type (1 input, 2 output, 3 feature), and whether it takes writes without response.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Report {
    pub handle: u16,
    pub cccd: u16,
    pub id: u8,
    pub kind: u8,
    pub write_without_response: bool,
}

impl Report {
    pub const INPUT: u8 = 1;
    pub const OUTPUT: u8 = 2;
}

/// A bonded device: its keys (legacy pairing's EDIV and Rand too), what it is, and what reading its HID service found.
#[derive(Clone, PartialEq, Eq)]
pub struct Device {
    pub address: [u8; 7],
    pub irk: Option<u128>,
    pub ltk: u128,
    pub authenticated: bool,
    pub ediv: u16,
    pub rand: [u8; 8],
    pub key_len: u8,
    pub appearance: u16,
    pub name: heapless::String<NAME_MAX>,
    /// Its battery level's value handle and notification switch (0: none).
    pub battery: (u16, u16),
    pub reports: heapless::Vec<Report, MAX_REPORTS>,
    pub map: heapless::Vec<u8, MAP_MAX>,
}

impl Device {
    pub fn identity(&self) -> Identity {
        identity_of(&self.address, self.irk)
    }

    pub fn bond(&self) -> BondInformation {
        let level = if self.authenticated { SecurityLevel::EncryptedAuthenticated } else { SecurityLevel::Encrypted };
        BondInformation {
            ediv: self.ediv,
            rand: self.rand,
            encryption_key_len: self.key_len,
            ..BondInformation::new(self.identity(), LongTermKey(self.ltk), level, true)
        }
    }

    /// A device just paired, before its HID service is read.
    pub fn from_bond(b: &BondInformation) -> Device {
        Device {
            address: b.identity.addr.to_bytes(),
            irk: b.identity.irk.map(|k| k.0.get()),
            ltk: b.ltk.0,
            authenticated: b.security_level == SecurityLevel::EncryptedAuthenticated,
            ediv: b.ediv,
            rand: b.rand,
            key_len: b.encryption_key_len,
            appearance: 0,
            name: heapless::String::new(),
            battery: (0, 0),
            reports: heapless::Vec::new(),
            map: heapless::Vec::new(),
        }
    }
}

/// Device record: `magic u32 | seq u32 | 4 × slot | crc32 u32`. A slot: `used u8 | authenticated u8 | key length u8 |
/// address 7 | irk 16 | ltk 16 | ediv u16 | rand 8 | appearance u16 | name (length u8, 29) | battery handle u16, cccd
/// u16 | reports (count u8, 12 × handle u16, cccd u16, id u8, type u8, flags u8) | map (length u16, 512)`.
const DEVICE_MAGIC: u32 = 0x4B56_4D44; // "KVMD"
const DEVICE_SLOT_LEN: usize = 688;
const DEVICE_RECORD_LEN: usize = 8 + MAX_DEVICES * DEVICE_SLOT_LEN + 4;

pub struct DeviceBonds {
    flash: FlashStorage<'static>,
    seq: u32,
    pub slots: [Option<Device>; MAX_DEVICES],
    /// The record as it goes to flash (kept here rather than on a task's stack).
    buf: [u8; DEVICE_RECORD_LEN],
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

impl DeviceBonds {
    /// The newest valid record of the two device sectors. `flash` is a second handle on the flash the hosts' bonds use:
    /// both are used only on the Bluetooth side's executor, one at a time.
    pub fn load(flash: FlashStorage<'static>) -> DeviceBonds {
        let mut bonds = DeviceBonds { flash, seq: 0, slots: Default::default(), buf: [0; DEVICE_RECORD_LEN] };
        let mut best: Option<u32> = None;
        for k in 0..2 {
            let r = &mut bonds.buf;
            if bonds.flash.read(DEVICE_BASE + k * SECTOR, r).is_err() {
                continue;
            }
            let word = |at: usize| u32::from_le_bytes([r[at], r[at + 1], r[at + 2], r[at + 3]]);
            if word(0) != DEVICE_MAGIC || word(DEVICE_RECORD_LEN - 4) != crc32(&r[..DEVICE_RECORD_LEN - 4]) {
                continue;
            }
            let seq = word(4);
            if best.is_some_and(|s| s >= seq) {
                continue;
            }
            best = Some(seq);
            bonds.seq = seq;
            for (i, slot) in bonds.slots.iter_mut().enumerate() {
                *slot = decode_device(&r[8 + i * DEVICE_SLOT_LEN..8 + (i + 1) * DEVICE_SLOT_LEN]);
            }
        }
        bonds
    }

    /// Puts `value` in `slot`, saving only if that changes the slot.
    pub fn set(&mut self, slot: usize, value: Option<Device>) {
        if self.slots[slot] != value {
            self.slots[slot] = value;
            self.save();
        }
    }

    /// Empties the slots `which` picks, with one save for all of them, and none if they were all free already.
    pub fn clear(&mut self, which: impl Fn(usize) -> bool) {
        let mut any = false;
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if which(i) && slot.take().is_some() {
                any = true;
            }
        }
        if any {
            self.save();
        }
    }

    fn save(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        let r = &mut self.buf;
        r.fill(0);
        r[..4].copy_from_slice(&DEVICE_MAGIC.to_le_bytes());
        r[4..8].copy_from_slice(&self.seq.to_le_bytes());
        for (i, slot) in self.slots.iter().enumerate() {
            if let Some(d) = slot {
                encode_device(d, &mut r[8 + i * DEVICE_SLOT_LEN..8 + (i + 1) * DEVICE_SLOT_LEN]);
            }
        }
        let crc = crc32(&r[..DEVICE_RECORD_LEN - 4]);
        r[DEVICE_RECORD_LEN - 4..].copy_from_slice(&crc.to_le_bytes());
        let _ = self.flash.write(DEVICE_BASE + (self.seq % 2) * SECTOR, &self.buf);
    }

    /// The slot of a bond with this identity.
    pub fn slot_of(&self, id: &Identity) -> Option<usize> {
        self.slots.iter().position(|s| s.as_ref().is_some_and(|s| s.identity().match_identity(id)))
    }
}

fn encode_device(d: &Device, s: &mut [u8]) {
    s[0] = 1;
    s[1] = d.authenticated as u8;
    s[2] = d.key_len;
    s[3..10].copy_from_slice(&d.address);
    s[10..26].copy_from_slice(&d.irk.unwrap_or(0).to_le_bytes());
    s[26..42].copy_from_slice(&d.ltk.to_le_bytes());
    s[42..44].copy_from_slice(&d.ediv.to_le_bytes());
    s[44..52].copy_from_slice(&d.rand);
    s[52..54].copy_from_slice(&d.appearance.to_le_bytes());
    s[54] = d.name.len() as u8;
    s[55..55 + d.name.len()].copy_from_slice(d.name.as_bytes());
    s[84..86].copy_from_slice(&d.battery.0.to_le_bytes());
    s[86..88].copy_from_slice(&d.battery.1.to_le_bytes());
    s[88] = d.reports.len() as u8;
    for (i, r) in d.reports.iter().enumerate() {
        let at = 89 + i * 7;
        s[at..at + 2].copy_from_slice(&r.handle.to_le_bytes());
        s[at + 2..at + 4].copy_from_slice(&r.cccd.to_le_bytes());
        s[at + 4] = r.id;
        s[at + 5] = r.kind;
        s[at + 6] = r.write_without_response as u8;
    }
    s[173..175].copy_from_slice(&(d.map.len() as u16).to_le_bytes());
    s[175..175 + d.map.len()].copy_from_slice(&d.map);
}

fn decode_device(s: &[u8]) -> Option<Device> {
    if s[0] != 1 {
        return None;
    }
    let mut address = [0u8; 7];
    address.copy_from_slice(&s[3..10]);
    let irk = u128::from_le_bytes(s[10..26].try_into().ok()?);
    let name_len = (s[54] as usize).min(NAME_MAX);
    let name = core::str::from_utf8(&s[55..55 + name_len]).ok().and_then(|n| heapless::String::try_from(n).ok())?;
    let mut reports = heapless::Vec::new();
    for i in 0..(s[88] as usize).min(MAX_REPORTS) {
        let at = 89 + i * 7;
        let _ = reports.push(Report {
            handle: u16_at(s, at),
            cccd: u16_at(s, at + 2),
            id: s[at + 4],
            kind: s[at + 5],
            write_without_response: s[at + 6] != 0,
        });
    }
    let map_len = (u16_at(s, 173) as usize).min(MAP_MAX);
    Some(Device {
        address,
        irk: (irk != 0).then_some(irk),
        ltk: u128::from_le_bytes(s[26..42].try_into().ok()?),
        authenticated: s[1] != 0,
        ediv: u16_at(s, 42),
        rand: s[44..52].try_into().ok()?,
        key_len: s[2],
        appearance: u16_at(s, 52),
        name,
        battery: (u16_at(s, 84), u16_at(s, 86)),
        reports,
        map: heapless::Vec::from_slice(&s[175..175 + map_len]).ok()?,
    })
}
