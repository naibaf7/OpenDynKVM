//! The RP2354B's partition table as a boot ROM block (RP2350 datasheet §5.9.4), encoded the way
//! `picotool partition create` does it (checked against picotool's output in the test below), and as picotool JSON.

use sha2::{Digest, Sha256};

const BLOCK_START: u32 = 0xffff_ded3;
const BLOCK_END: u32 = 0xab12_3579;
const ITEM_PARTITION_TABLE: u32 = 0x0a;
const ITEM_VERSION: u32 = 0x48;
const ITEM_HASH_DEF: u32 = 0x47;
const ITEM_HASH_VALUE: u32 = 0x4b;
const ITEM_LAST: u32 = 0xff;
const HASH_SHA256: u32 = 1;

/// Read and write for Secure, Non-secure and bootloader code (Table 500).
pub const PERMISSIONS_ALL: u32 = 0xfc00_0000;
pub const ACCEPTS_ABSOLUTE: u32 = 1 << 15;
pub const ACCEPTS_RP2350_ARM_S: u32 = 1 << 17;
pub const ACCEPTS_RP2350_RISCV: u32 = 1 << 18;
/// Never booted on that architecture (a data partition; datasheet Table 500: also saves boot time).
pub const NOT_BOOTABLE_ARM: u32 = 1 << 9;
pub const NOT_BOOTABLE_RISCV: u32 = 1 << 10;
const HAS_ID: u32 = 1 << 0;
const LINK_A_PARTITION: u32 = 1 << 1;
const HAS_NAME: u32 = 1 << 12;

pub struct Partition {
    pub name: &'static str,
    pub id: u64,
    pub first_sector: u32,
    pub last_sector: u32,
    /// `ACCEPTS_*` UF2 families and `NOT_BOOTABLE_*` bits.
    pub flags: u32,
    /// For a B partition: the index of its A partition.
    pub link_a: Option<u32>,
}

/// One item header: 1-byte size for sizes below 256 words, as picotool writes them.
fn item(ty: u32, size_words: u32, extra: u32) -> u32 {
    extra << 24 | size_words << 8 | ty
}

pub fn block(unpartitioned_flags: u32, partitions: &[Partition], version: (u16, u16)) -> Vec<u32> {
    let mut w = vec![BLOCK_START];
    let header = w.len();
    w.push(0);
    w.push(PERMISSIONS_ALL | unpartitioned_flags);
    for p in partitions {
        w.push(PERMISSIONS_ALL | p.last_sector << 13 | p.first_sector);
        let link = p.link_a.map_or(0, |a| LINK_A_PARTITION | a << 3);
        w.push(PERMISSIONS_ALL | HAS_ID | HAS_NAME | p.flags | link);
        w.push(p.id as u32);
        w.push((p.id >> 32) as u32);
        // Name: length byte, then the bytes, padded to whole words.
        let mut name = vec![p.name.len() as u8];
        name.extend_from_slice(p.name.as_bytes());
        name.resize(name.len().next_multiple_of(4), 0);
        w.extend(name.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
    }
    w[header] = item(ITEM_PARTITION_TABLE, (w.len() - header) as u32, partitions.len() as u32);
    w.push(item(ITEM_VERSION, 2, 0));
    w.push((version.0 as u32) << 16 | version.1 as u32);
    // The hash covers everything so far, the HASH_DEF item included.
    w.push(item(ITEM_HASH_DEF, 2, HASH_SHA256));
    w.push(w.len() as u32 + 1);
    let bytes: Vec<u8> = w.iter().flat_map(|x| x.to_le_bytes()).collect();
    let hash = Sha256::digest(&bytes);
    w.push(item(ITEM_HASH_VALUE, 9, 0));
    w.extend(hash.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])));
    w.push(item(ITEM_LAST, (w.len() - 1) as u32, 0));
    w.push(0); // next block: this one (a loop of one)
    w.push(BLOCK_END);
    w
}

fn families_json(f: u32) -> String {
    let mut v = Vec::new();
    if f & ACCEPTS_ABSOLUTE != 0 {
        v.push("\"absolute\"");
    }
    if f & ACCEPTS_RP2350_ARM_S != 0 {
        v.push("\"rp2350-arm-s\"");
    }
    if f & ACCEPTS_RP2350_RISCV != 0 {
        v.push("\"rp2350-riscv\"");
    }
    format!("[{}]", v.join(", "))
}

/// The same table as `picotool partition create` JSON (datasheet §5.10.4).
pub fn json(unpartitioned_flags: u32, partitions: &[Partition], version: (u16, u16)) -> String {
    let perms = r#"{ "secure": "rw", "nonsecure": "rw", "bootloader": "rw" }"#;
    let parts: Vec<String> = partitions
        .iter()
        .map(|p| {
            let mut extra = p.link_a.map_or(String::new(), |a| format!(",\n      \"link\": [\"a\", {a}]"));
            if p.flags & NOT_BOOTABLE_ARM != 0 {
                extra += ",\n      \"ignored_during_arm_boot\": true";
            }
            if p.flags & NOT_BOOTABLE_RISCV != 0 {
                extra += ",\n      \"ignored_during_riscv_boot\": true";
            }
            format!(
                "    {{\n      \"name\": \"{}\",\n      \"id\": {},\n      \"start\": \"{}K\",\n      \"size\": \"{}K\",\n      \
                 \"families\": {},\n      \"permissions\": {perms}{extra}\n    }}",
                p.name,
                p.id,
                p.first_sector * 4,
                (p.last_sector + 1 - p.first_sector) * 4,
                families_json(p.flags),
            )
        })
        .collect();
    format!(
        "{{\n  \"version\": [{}, {}],\n  \"unpartitioned\": {{\n    \"families\": {},\n    \"permissions\": {perms}\n  }},\n  \
         \"partitions\": [\n{}\n  ]\n}}\n",
        version.0,
        version.1,
        families_json(unpartitioned_flags),
        parts.join(",\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// embassy-rp's `make_hashed_partition_table` test vector, which its author took from `picotool partition create`
    /// (2044K A/B partitions accepting Arm-S and RISC-V images, absolute family in unpartitioned space).
    #[test]
    fn matches_picotool() {
        let fam = ACCEPTS_RP2350_ARM_S | ACCEPTS_RP2350_RISCV;
        let parts = [
            Partition { name: "A", id: 0, first_sector: 2, last_sector: 512, flags: fam, link_a: None },
            Partition { name: "B", id: 1, first_sector: 513, last_sector: 1023, flags: fam, link_a: Some(0) },
        ];
        let expected = [
            0xffffded3, 0x02000c0a, 0xfc008000, 0xfc400002, 0xfc061001, 0x00000000, 0x00000000, 0x00004101, 0xfc7fe201,
            0xfc061003, 0x00000001, 0x00000000, 0x00004201, 0x00000248, 0x00010000, 0x01000247, 0x00000011, 0x0000094b,
            0x1945cdad, 0x6b5f9773, 0xe2bf39bd, 0xb243e599, 0xab2f0e9a, 0x4d5d6d0b, 0xf973050f, 0x5ab6dadb, 0x000019ff,
            0x00000000, 0xab123579,
        ];
        assert_eq!(block(ACCEPTS_ABSOLUTE, &parts, (1, 0)), expected);
    }
}
