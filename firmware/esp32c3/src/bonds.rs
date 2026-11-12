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
//! Record: `magic u32 | seq u32 | 4 × slot (48 bytes) | crc32 u32`; a slot is `used u8 | security u8 | address 7 |
//! irk 16 | ltk 16 | pad 7`.

use esp_storage::FlashStorage;
use kvm_proto::link::bt::MAX_HOSTS;
use trouble_host::prelude::*;

const SECTOR: u32 = 4096;
const BASE: u32 = 4 * 1024 * 1024 - 2 * SECTOR;
const MAGIC: u32 = 0x4B56_4D42; // "KVMB"
const SLOT_LEN: usize = 48;
const RECORD_LEN: usize = 8 + MAX_HOSTS * SLOT_LEN + 4;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub address: [u8; 7],
    pub irk: Option<u128>,
    pub ltk: u128,
    pub authenticated: bool,
}

impl Slot {
    pub fn identity(&self) -> Identity {
        let kind = if self.address[0] == 0 { AddrKind::PUBLIC } else { AddrKind::RANDOM };
        let mut a = [0u8; 6];
        a.copy_from_slice(&self.address[1..]);
        Identity {
            addr: Address::new(kind, BdAddr::new(a)),
            irk: self.irk.and_then(|k| core::num::NonZeroU128::new(k)).map(IdentityResolvingKey),
        }
    }

    pub fn bond(&self) -> BondInformation {
        BondInformation {
            ltk: LongTermKey(self.ltk),
            identity: self.identity(),
            is_bonded: true,
            security_level: if self.authenticated {
                SecurityLevel::EncryptedAuthenticated
            } else {
                SecurityLevel::Encrypted
            },
        }
    }

    pub fn from_bond(b: &BondInformation) -> Slot {
        Slot {
            address: b.identity.addr.to_bytes(),
            irk: b.identity.irk.map(|k| k.0.get()),
            ltk: b.ltk.0,
            authenticated: b.security_level == SecurityLevel::EncryptedAuthenticated,
        }
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
                *slot = Some(Slot { address, irk: (irk != 0).then_some(irk), ltk, authenticated: s[1] == 2 });
            }
            best = Some((seq, slots));
        }
        let (seq, slots) = best.unwrap_or((0, [None; MAX_HOSTS]));
        Bonds { flash, seq, slots }
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

    pub fn free_slot(&self) -> Option<usize> {
        self.slots.iter().position(|s| s.is_none())
    }
}
