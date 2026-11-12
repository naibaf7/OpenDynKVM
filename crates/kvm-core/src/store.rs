//! Config, keymap and macros in flash: a log-structured key-value store with wear levelling (software.md §4.8).
//!
//! # Why
//!
//! The store lives in NOR flash (the W25Q16JV in the RP2354B). NOR can only *program* bits from 1 to 0; getting a 1
//! back takes an *erase*, of a whole 4 KB sector at once, and each sector is rated for 100 000 erase cycles. An erase
//! also takes tens of milliseconds (45 ms typical for the W25Q…JV family), with both cores stalled, while a page
//! program takes 0.4 ms. The obvious layout, one sector per value erased and rewritten on every save, would spend one
//! erase per save and all of them on the same sector: 100 000 saves, then that value is lost.
//!
//! # How
//!
//! The data partition (17 sectors in the firmware) is a ring, used as a log:
//!
//! - **A save appends.** The new value goes, as an item, after the last item of the newest sector (the *head*). The
//!   old value stays where it is and simply stops counting: the newest copy of a key wins. Nothing in flash is ever
//!   rewritten in place, so every byte is programmed at most once between two erases of its sector.
//! - **A full head moves on** to the next sector of the ring. One sector is always kept erased as the spare; when
//!   opening the head uses it up, the *oldest* sector is collected: the items in it that are still current are copied
//!   to the head, and it is erased. That is the only erase there is.
//! - **So the erases rotate.** The sectors are taken in strict ring order whatever they hold, and collecting carries
//!   the values that never change (cold data, such as a keymap set up once) forward with the ring instead of leaving
//!   them parked. Every sector is erased once per trip of the ring, so the spread between the most and the least worn
//!   sector stays within a cycle or two (`rewriting_a_value_spreads_erases_over_every_sector`).
//! - **No fixed place is written more than another.** The store keeps no superblock, no "current sector" pointer and
//!   no valid/obsolete flags to flip: which sectors are in use, their order and the current value of each key are all
//!   worked out at mount from what the sectors hold (a header with a good CRC, the sequence numbers, the items' CRCs),
//!   and kept in RAM. The sector header is written once per erase, together with the sector's first use.
//! - **Unchanged values cost nothing:** writing the value already stored programs nothing ([`Store::write`]).
//!
//! The cost of a save is its item's size: about 0.6 KB for the config. With everything the firmware stores at its
//! largest (5.9 KB of keymap, macros and records, carried around the ring), each sector is erased once per about 90
//! config saves, which makes about 9 million saves before the first sector reaches its rated cycles
//! (`endurance_with_the_firmware_items`). How often saves come is the engine's business: it saves 5 s after the last
//! change and caps the rate of saves (`kvm_core::engine`, `SAVE_BURST`), so even a host script that changes a setting
//! every few seconds for years cannot wear the flash out.
//!
//! # Format
//!
//! - **Sector:** `"KST1" | seq u32 | erases u32 | crc32 u32`, then items. `seq` orders the sectors; `erases` counts the
//!   sector's erase cycles, for the log only (nothing is chosen by it). A free sector has no header to carry its count,
//!   so at mount it gets the highest count of the others, which the rotation keeps within a cycle or two of its own.
//! - **Item:** `key u16 | len u16 | crc32 u32 | data`, padded to 4 bytes. The CRC covers key, length and data. An item
//!   never spans two sectors.
//! - **Power loss:** the header of an item is programmed before its data, and the header of a sector before any item.
//!   A cut write leaves an item whose CRC fails: reading stops there, the value before it stays in force, and the next
//!   write starts a new sector. A cut garbage collection leaves both copies, and the newer one wins. A sector whose
//!   header is not valid is erased before it is used again (`power_cuts_never_lose_a_committed_value`).

use heapless::Vec;
use kvm_proto::crc::Crc32;

pub const SECTOR: u32 = 4096;
pub const MAX_SECTORS: usize = 32;
pub const MAX_KEYS: usize = 32;
const MAGIC: &[u8; 4] = b"KST1";
const HEADER: u32 = 16;
const ITEM_HEADER: u32 = 8;
/// Largest value one item can hold.
pub const MAX_ITEM: usize = (SECTOR - HEADER - ITEM_HEADER) as usize;
const ERASED_KEY: u16 = 0xFFFF;

/// The flash under the store; offsets are relative to the start of the data partition.
pub trait Nor {
    fn read(&mut self, at: u32, buf: &mut [u8]);
    /// Erases the sector starting at `at`.
    fn erase(&mut self, at: u32) -> bool;
    /// Programs `data` at `at` (bits only go from 1 to 0).
    fn program(&mut self, at: u32, data: &[u8]) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum StoreError {
    TooLarge,
    /// The live items no longer fit next to one spare sector.
    Full,
    /// Erase or program reported a failure.
    Flash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sector {
    Free,
    /// Not erased and no valid header: erase before use.
    Dirty,
    Used(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Entry {
    key: u16,
    sector: u8,
    offset: u16,
    len: u16,
}

pub struct Store {
    n: usize,
    sectors: [Sector; MAX_SECTORS],
    erases: [u32; MAX_SECTORS],
    /// Newest sector and where its next item goes (`SECTOR` = full).
    head: Option<(usize, u32)>,
    index: Vec<Entry, MAX_KEYS>,
}

fn pad4(n: u32) -> u32 {
    (n + 3) & !3
}

fn item_crc(key: u16, len: u16, data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(&key.to_le_bytes());
    c.update(&len.to_le_bytes());
    c.update(data);
    c.finish()
}

fn erased(f: &mut impl Nor, at: u32, len: u32) -> bool {
    let mut buf = [0u8; 64];
    (0..len).step_by(buf.len()).all(|o| {
        let n = (len - o).min(buf.len() as u32) as usize;
        f.read(at + o, &mut buf[..n]);
        buf[..n].iter().all(|b| *b == 0xFF)
    })
}

/// One item as found in flash.
#[derive(Clone, Copy)]
struct Found {
    key: u16,
    offset: u32,
    len: u16,
    crc: u32,
}

impl Store {
    /// Reads the partition of `sectors` sectors and rebuilds the index of current values.
    pub fn mount(f: &mut impl Nor, sectors: usize) -> Result<Self, StoreError> {
        let mut s = Store {
            n: sectors.clamp(3, MAX_SECTORS),
            sectors: [Sector::Free; MAX_SECTORS],
            erases: [0; MAX_SECTORS],
            head: None,
            index: Vec::new(),
        };
        for i in 0..s.n {
            let mut h = [0u8; HEADER as usize];
            f.read(i as u32 * SECTOR, &mut h);
            let word = |k: usize| u32::from_le_bytes([h[k], h[k + 1], h[k + 2], h[k + 3]]);
            let mut c = Crc32::new();
            c.update(&h[..12]);
            s.sectors[i] = if &h[..4] == MAGIC && c.finish() == word(12) {
                s.erases[i] = word(8);
                Sector::Used(word(4))
            } else if erased(f, i as u32 * SECTOR, SECTOR) {
                Sector::Free
            } else {
                Sector::Dirty
            };
        }
        // A sector without a header lost its count with its last erase; see the module documentation.
        let seen = s.erases[..s.n].iter().copied().max().unwrap_or(0);
        for i in 0..s.n {
            if !matches!(s.sectors[i], Sector::Used(_)) {
                s.erases[i] = seen;
            }
        }
        // Oldest first, so that newer copies of a key override older ones.
        let mut order: Vec<(u32, usize), MAX_SECTORS> = Vec::new();
        for i in 0..s.n {
            if let Sector::Used(seq) = s.sectors[i] {
                let _ = order.push((seq, i));
            }
        }
        order.sort_unstable();
        for &(_, i) in &order {
            let end = s.scan(f, i, |s, it| s.set_index(it.key, i, it.offset, it.len));
            s.head = Some((i, end));
        }
        // A garbage collection cut short leaves no spare sector: finish it. (If even that fails, writes report Full.)
        if order.len() == s.n {
            let _ = s.collect(f, order[0].1);
        }
        Ok(s)
    }

    /// Walks the valid items of sector `i`; returns where the next item would go (`SECTOR` if the sector is full or a
    /// write into it was cut short).
    fn scan(&mut self, f: &mut impl Nor, i: usize, mut each: impl FnMut(&mut Self, &Found)) -> u32 {
        let base = i as u32 * SECTOR;
        let mut pos = HEADER;
        while pos + ITEM_HEADER <= SECTOR {
            let mut h = [0u8; ITEM_HEADER as usize];
            f.read(base + pos, &mut h);
            let key = u16::from_le_bytes([h[0], h[1]]);
            let len = u16::from_le_bytes([h[2], h[3]]);
            let crc = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
            if key == ERASED_KEY && len == 0xFFFF && crc == u32::MAX {
                // End of the log, provided nothing was programmed after it.
                return if erased(f, base + pos, SECTOR - pos) { pos } else { SECTOR };
            }
            let size = ITEM_HEADER + pad4(len as u32);
            if key == ERASED_KEY || pos + size > SECTOR {
                return SECTOR;
            }
            let mut c = Crc32::new();
            c.update(&key.to_le_bytes());
            c.update(&len.to_le_bytes());
            let mut buf = [0u8; 64];
            let mut done = 0u32;
            while done < len as u32 {
                let k = (len as u32 - done).min(buf.len() as u32) as usize;
                f.read(base + pos + ITEM_HEADER + done, &mut buf[..k]);
                c.update(&buf[..k]);
                done += k as u32;
            }
            if c.finish() != crc {
                return SECTOR;
            }
            each(self, &Found { key, offset: pos, len, crc });
            pos += size;
        }
        SECTOR
    }

    fn set_index(&mut self, key: u16, sector: usize, offset: u32, len: u16) {
        let e = Entry { key, sector: sector as u8, offset: offset as u16, len };
        match self.index.iter_mut().find(|e| e.key == key) {
            Some(slot) => *slot = e,
            None => {
                let _ = self.index.push(e);
            }
        }
    }

    /// Length of the current value of `key`.
    pub fn len(&self, key: u16) -> Option<usize> {
        self.index.iter().find(|e| e.key == key).map(|e| e.len as usize)
    }

    /// Reads the current value of `key` into `buf`; returns its length (the value is cut to `buf`).
    pub fn read(&self, f: &mut impl Nor, key: u16, buf: &mut [u8]) -> Option<usize> {
        let e = self.index.iter().find(|e| e.key == key)?;
        let n = (e.len as usize).min(buf.len());
        f.read(e.sector as u32 * SECTOR + e.offset as u32 + ITEM_HEADER, &mut buf[..n]);
        Some(e.len as usize)
    }

    /// Stores `data` as the new value of `key`. Writing the value already stored costs nothing.
    pub fn write(&mut self, f: &mut impl Nor, key: u16, data: &[u8]) -> Result<(), StoreError> {
        if data.len() > MAX_ITEM || key == ERASED_KEY {
            return Err(StoreError::TooLarge);
        }
        if self.same(f, key, data) {
            return Ok(());
        }
        if self.index.is_full() && !self.index.iter().any(|e| e.key == key) {
            return Err(StoreError::Full);
        }
        let size = ITEM_HEADER + pad4(data.len() as u32);
        for _ in 0..=self.n {
            match self.head {
                Some((i, pos)) if pos + size <= SECTOR => {
                    let at = i as u32 * SECTOR + pos;
                    let len = data.len() as u16;
                    let mut h = [0u8; ITEM_HEADER as usize];
                    h[..2].copy_from_slice(&key.to_le_bytes());
                    h[2..4].copy_from_slice(&len.to_le_bytes());
                    h[4..].copy_from_slice(&item_crc(key, len, data).to_le_bytes());
                    // Whatever happens next, this sector takes no further items until the write has succeeded.
                    self.head = Some((i, SECTOR));
                    if !f.program(at, &h) || !(data.is_empty() || f.program(at + ITEM_HEADER, data)) {
                        return Err(StoreError::Flash);
                    }
                    self.head = Some((i, pos + size));
                    self.set_index(key, i, pos, len);
                    return Ok(());
                }
                _ => self.open_next(f)?,
            }
        }
        Err(StoreError::Full)
    }

    fn same(&self, f: &mut impl Nor, key: u16, data: &[u8]) -> bool {
        let Some(e) = self.index.iter().find(|e| e.key == key) else { return false };
        if e.len as usize != data.len() {
            return false;
        }
        let base = e.sector as u32 * SECTOR + e.offset as u32 + ITEM_HEADER;
        let mut buf = [0u8; 64];
        data.chunks(buf.len()).enumerate().all(|(k, chunk)| {
            f.read(base + (k * buf.len()) as u32, &mut buf[..chunk.len()]);
            buf[..chunk.len()] == *chunk
        })
    }

    /// Starts a new head sector after the current one; collects the oldest sector if no spare would be left.
    ///
    /// The next sector in ring order, never the least worn or the emptiest: the strict rotation is what spreads the
    /// erases evenly, without the store having to know any counts.
    fn open_next(&mut self, f: &mut impl Nor) -> Result<(), StoreError> {
        let after = self.head.map_or(0, |(i, _)| i + 1);
        let next = (0..self.n)
            .map(|k| (after + k) % self.n)
            .find(|&i| !matches!(self.sectors[i], Sector::Used(_)))
            .ok_or(StoreError::Full)?;
        let base = next as u32 * SECTOR;
        if self.sectors[next] == Sector::Dirty || !erased(f, base, SECTOR) {
            if !f.erase(base) {
                return Err(StoreError::Flash);
            }
            self.erases[next] = self.erases[next].wrapping_add(1);
        }
        let seq = self.newest_seq().wrapping_add(1);
        let mut h = [0u8; HEADER as usize];
        h[..4].copy_from_slice(MAGIC);
        h[4..8].copy_from_slice(&seq.to_le_bytes());
        h[8..12].copy_from_slice(&self.erases[next].to_le_bytes());
        let mut c = Crc32::new();
        c.update(&h[..12]);
        h[12..].copy_from_slice(&c.finish().to_le_bytes());
        self.sectors[next] = Sector::Dirty;
        if !f.program(base, &h) {
            return Err(StoreError::Flash);
        }
        self.sectors[next] = Sector::Used(seq);
        self.head = Some((next, HEADER));
        if self.sectors[..self.n].iter().all(|s| matches!(s, Sector::Used(_))) {
            let oldest = (0..self.n)
                .filter(|&i| i != next)
                .filter_map(|i| if let Sector::Used(q) = self.sectors[i] { Some((q, i)) } else { None })
                .min();
            if let Some((_, oldest)) = oldest {
                self.collect(f, oldest)?;
            }
        }
        Ok(())
    }

    fn newest_seq(&self) -> u32 {
        self.sectors[..self.n]
            .iter()
            .filter_map(|s| if let Sector::Used(q) = s { Some(*q) } else { None })
            .max()
            .unwrap_or(0)
    }

    /// Copies the items of sector `i` that are still current to the head, then erases `i`. Called with a freshly opened
    /// head, which always has room for them: they came from one sector.
    fn collect(&mut self, f: &mut impl Nor, i: usize) -> Result<(), StoreError> {
        let mut live: Vec<Found, MAX_KEYS> = Vec::new();
        self.scan(f, i, |s, it| {
            let current =
                s.index.iter().any(|e| e.key == it.key && e.sector as usize == i && e.offset as u32 == it.offset);
            if current {
                let _ = live.push(*it);
            }
        });
        for it in &live {
            let size = ITEM_HEADER + pad4(it.len as u32);
            let Some((h, pos)) = self.head.filter(|(h, pos)| *h != i && pos + size <= SECTOR) else {
                return Err(StoreError::Full);
            };
            // Copied as stored: header (with its CRC) first, then the data in pieces.
            let src = i as u32 * SECTOR + it.offset;
            let dst = h as u32 * SECTOR + pos;
            let mut hdr = [0u8; ITEM_HEADER as usize];
            hdr[..2].copy_from_slice(&it.key.to_le_bytes());
            hdr[2..4].copy_from_slice(&it.len.to_le_bytes());
            hdr[4..].copy_from_slice(&it.crc.to_le_bytes());
            self.head = Some((h, SECTOR));
            if !f.program(dst, &hdr) {
                return Err(StoreError::Flash);
            }
            let mut buf = [0u8; 256];
            let mut done = 0u32;
            while done < it.len as u32 {
                let k = (it.len as u32 - done).min(buf.len() as u32) as usize;
                f.read(src + ITEM_HEADER + done, &mut buf[..k]);
                if !f.program(dst + ITEM_HEADER + done, &buf[..k]) {
                    return Err(StoreError::Flash);
                }
                done += k as u32;
            }
            self.head = Some((h, pos + size));
            self.set_index(it.key, h, pos, it.len);
        }
        self.sectors[i] = Sector::Dirty;
        if !f.erase(i as u32 * SECTOR) {
            return Err(StoreError::Flash);
        }
        self.erases[i] = self.erases[i].wrapping_add(1);
        self.sectors[i] = Sector::Free;
        Ok(())
    }

    /// The most erase cycles any sector has seen (the W25Q16JV is rated for 100 000).
    pub fn max_erases(&self) -> u32 {
        self.erases[..self.n].iter().copied().max().unwrap_or(0)
    }

    /// Bytes the current values take, headers included.
    pub fn live_bytes(&self) -> usize {
        self.index.iter().map(|e| (ITEM_HEADER + pad4(e.len as u32)) as usize).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// NOR flash in RAM. `budget` counts bytes programmed (and sectors erased) until a simulated power cut, after
    /// which every operation fails.
    struct Ram {
        mem: std::vec::Vec<u8>,
        budget: Option<usize>,
        erases: std::vec::Vec<u32>,
    }

    impl Ram {
        fn new(sectors: usize) -> Self {
            Ram { mem: std::vec![0xFF; sectors * SECTOR as usize], budget: None, erases: std::vec![0; sectors] }
        }
    }

    impl Nor for Ram {
        fn read(&mut self, at: u32, buf: &mut [u8]) {
            buf.copy_from_slice(&self.mem[at as usize..at as usize + buf.len()]);
        }

        fn erase(&mut self, at: u32) -> bool {
            assert_eq!(at % SECTOR, 0);
            if let Some(b) = self.budget.as_mut() {
                if *b == 0 {
                    // Cut mid-erase: the first half is erased, the rest keeps its contents.
                    self.mem[at as usize..at as usize + SECTOR as usize / 2].fill(0xFF);
                    return false;
                }
                *b -= 1;
            }
            self.mem[at as usize..(at + SECTOR) as usize].fill(0xFF);
            self.erases[(at / SECTOR) as usize] += 1;
            true
        }

        fn program(&mut self, at: u32, data: &[u8]) -> bool {
            for (k, b) in data.iter().enumerate() {
                if let Some(budget) = self.budget.as_mut() {
                    if *budget == 0 {
                        return false;
                    }
                    *budget -= 1;
                }
                // NOR: programming only clears bits.
                self.mem[at as usize + k] &= *b;
            }
            true
        }
    }

    fn get(s: &Store, f: &mut Ram, key: u16) -> Option<std::vec::Vec<u8>> {
        let mut buf = [0u8; MAX_ITEM];
        let n = s.read(f, key, &mut buf)?;
        Some(buf[..n].to_vec())
    }

    #[test]
    fn values_survive_a_remount() {
        let mut f = Ram::new(4);
        let mut s = Store::mount(&mut f, 4).unwrap();
        assert_eq!(get(&s, &mut f, 1), None);
        s.write(&mut f, 1, b"config").unwrap();
        s.write(&mut f, 0x100, &[7; 464]).unwrap();
        s.write(&mut f, 1, b"config v2").unwrap();
        s.write(&mut f, 2, b"").unwrap();
        let mut s = Store::mount(&mut f, 4).unwrap();
        assert_eq!(get(&s, &mut f, 1).unwrap(), b"config v2");
        assert_eq!(get(&s, &mut f, 0x100).unwrap(), [7; 464]);
        assert_eq!(get(&s, &mut f, 2).unwrap(), b"");
        assert_eq!(s.write(&mut Ram::new(4), 3, &[0; MAX_ITEM + 1]), Err(StoreError::TooLarge));
    }

    #[test]
    fn rewriting_a_value_spreads_erases_over_every_sector() {
        let mut f = Ram::new(8);
        let mut s = Store::mount(&mut f, 8).unwrap();
        s.write(&mut f, 9, &[1; 2000]).unwrap(); // a value that never changes
        for k in 0..4000u32 {
            let layer = (k % 8) as u16;
            s.write(&mut f, 0x100 + layer, &[(k / 8) as u8; 464]).unwrap();
        }
        let s = Store::mount(&mut f, 8).unwrap();
        assert_eq!(get(&s, &mut f, 9).unwrap(), [1; 2000]);
        for layer in 0..8u16 {
            let last = (0..4000u32).rev().find(|k| (k % 8) as u16 == layer).unwrap();
            assert_eq!(get(&s, &mut f, 0x100 + layer).unwrap(), [(last / 8) as u8; 464]);
        }
        let (min, max) = (f.erases.iter().min().unwrap(), f.erases.iter().max().unwrap());
        // 4000 × 472 bytes ≈ 1.9 MB through a 32 KB ring: about 60 erases per sector, evenly spread.
        assert!(*min >= 40 && max - min <= 2, "erases per sector: {:?}", f.erases);
        assert_eq!(s.max_erases(), *max);
    }

    #[test]
    fn same_value_is_not_written_again() {
        let mut f = Ram::new(4);
        let mut s = Store::mount(&mut f, 4).unwrap();
        s.write(&mut f, 1, b"abc").unwrap();
        let before = f.mem.clone();
        s.write(&mut f, 1, b"abc").unwrap();
        assert!(f.mem == before);
    }

    #[test]
    fn full_store_is_reported() {
        let mut f = Ram::new(3);
        let mut s = Store::mount(&mut f, 3).unwrap();
        // Three sectors leave room for two sectors of values next to the spare.
        s.write(&mut f, 1, &[1; 4000]).unwrap();
        s.write(&mut f, 2, &[2; 4000]).unwrap();
        assert_eq!(s.write(&mut f, 3, &[3; 4000]), Err(StoreError::Full));
    }

    /// The firmware's data partition, in sectors (`firmware/rp2354/src/flash.rs`, `DATA_SECTORS`).
    const FIRMWARE_SECTORS: usize = 17;
    /// Rated erase cycles per sector of the W25Q16JV.
    const RATED_ERASES: u64 = 100_000;

    /// Stores everything the firmware keeps, at its largest: 8 keymap layers, a full macro buffer, the keymap
    /// settings, both programming records. Returns the bytes it takes in flash.
    fn firmware_items(s: &mut Store, f: &mut Ram) -> usize {
        use kvm_vial::keymap::{LAYER_ITEM_LEN, MACRO_BUFFER};
        for l in 0..kvm_vial::LAYERS as u16 {
            s.write(f, 0x0110 + l, &[l as u8 + 1; LAYER_ITEM_LEN]).unwrap();
        }
        s.write(f, 0x0101, &[0x55; MACRO_BUFFER]).unwrap();
        s.write(f, 0x0100, &[1; 8]).unwrap();
        s.write(f, crate::at32prog::ITEM, &[2; crate::at32prog::Record::LEN]).unwrap();
        s.write(f, crate::at32prog::BT_ITEM, &[3; crate::at32prog::Record::LEN]).unwrap();
        s.live_bytes()
    }

    /// How long the flash lasts at the config's largest size, with the keymap carried around the ring: the number the
    /// module documentation and software.md §4.8 quote.
    #[test]
    fn endurance_with_the_firmware_items() {
        let mut f = Ram::new(FIRMWARE_SECTORS);
        let mut s = Store::mount(&mut f, FIRMWARE_SECTORS).unwrap();
        let cold = firmware_items(&mut s, &mut f);
        assert!((5_500..6_500).contains(&cold), "{cold} bytes besides the config");
        let saves = 20_000u32;
        for k in 0..saves {
            // A different config every time, so each one is written.
            s.write(&mut f, crate::config::ITEM, &[(k % 251) as u8; crate::config::ITEM_MAX]).unwrap();
        }
        let (min, max) = (*f.erases.iter().min().unwrap(), *f.erases.iter().max().unwrap());
        assert!(max - min <= 2, "erases per sector: {:?}", f.erases);
        let per_erase = saves / max;
        let lifetime = u64::from(saves) * RATED_ERASES / u64::from(max);
        assert!((85..=100).contains(&per_erase), "{per_erase} config saves per erase of a sector");
        assert!(lifetime >= 8_500_000, "{lifetime} config saves before a sector reaches its rated erases");
        // The cold items went round the ring with it.
        let s = Store::mount(&mut f, FIRMWARE_SECTORS).unwrap();
        assert_eq!(get(&s, &mut f, 0x0117).unwrap(), [8; kvm_vial::keymap::LAYER_ITEM_LEN]);
        assert_eq!(get(&s, &mut f, 0x0101).unwrap(), [0x55; kvm_vial::keymap::MACRO_BUFFER]);
    }

    /// The erase counts the headers carry stay right across power cycles, though the spare sector has no header.
    #[test]
    fn erase_counts_survive_remounts() {
        let mut f = Ram::new(FIRMWARE_SECTORS);
        let mut s = Store::mount(&mut f, FIRMWARE_SECTORS).unwrap();
        firmware_items(&mut s, &mut f);
        for k in 0..6_000u32 {
            s.write(&mut f, crate::config::ITEM, &[(k % 251) as u8; 300]).unwrap();
            if k % 37 == 0 {
                s = Store::mount(&mut f, FIRMWARE_SECTORS).unwrap();
            }
        }
        let real = *f.erases.iter().max().unwrap();
        assert!(real >= 20, "{real}");
        let told = Store::mount(&mut f, FIRMWARE_SECTORS).unwrap().max_erases();
        assert!(told.abs_diff(real) <= 2, "reported {told}, real {real}: {:?}", f.erases);
    }

    /// Cuts the power after every possible number of programmed bytes and erases during a run of writes, remounts,
    /// and checks that every key holds either its value before the interrupted write or the new one.
    #[test]
    fn power_cuts_never_lose_a_committed_value() {
        let writes: std::vec::Vec<(u16, std::vec::Vec<u8>)> =
            (0..30u32).map(|k| ((k % 3) as u16, std::vec![k as u8; 300 + (k as usize * 37) % 900])).collect();
        let total = {
            let mut f = Ram::new(4);
            let mut s = Store::mount(&mut f, 4).unwrap();
            f.budget = Some(usize::MAX);
            for (k, v) in &writes {
                s.write(&mut f, *k, v).unwrap();
            }
            // The run must reach garbage collection, or the cuts below would not cover it.
            assert!(f.erases.iter().sum::<u32>() >= 2, "{:?}", f.erases);
            usize::MAX - f.budget.unwrap()
        };
        for cut in 0..total {
            let mut f = Ram::new(4);
            let mut s = Store::mount(&mut f, 4).unwrap();
            f.budget = Some(cut);
            let mut committed: [Option<&[u8]>; 3] = [None; 3];
            let mut interrupted = None;
            for (k, v) in &writes {
                if s.write(&mut f, *k, v).is_ok() {
                    committed[*k as usize] = Some(v);
                } else {
                    interrupted = Some((*k, v));
                    break;
                }
            }
            f.budget = None;
            let mut s = Store::mount(&mut f, 4).unwrap();
            for key in 0..3u16 {
                let got = get(&s, &mut f, key);
                let ok = got.as_deref() == committed[key as usize]
                    || interrupted.is_some_and(|(k, v)| k == key && got.as_deref() == Some(v.as_slice()));
                assert!(ok, "cut at {cut}: key {key} lost its value");
            }
            // And the store keeps working afterwards.
            for (k, v) in writes.iter().take(12) {
                s.write(&mut f, *k, v).unwrap();
            }
            let s = Store::mount(&mut f, 4).unwrap();
            for (k, v) in writes.iter().take(12).rev().take(3) {
                assert_eq!(get(&s, &mut f, *k).as_deref(), Some(v.as_slice()), "cut at {cut}");
            }
        }
    }
}
