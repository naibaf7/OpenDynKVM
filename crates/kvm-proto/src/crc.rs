//! CRC-16/CCITT-FALSE (link frames) and CRC-32/ISO-HDLC (descriptor sets, config records, firmware images).

const fn crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn crc32_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static CRC16: [u16; 256] = crc16_table();
static CRC32: [u32; 256] = crc32_table();

/// CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF, no reflection, no final XOR.
pub fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0xFFFFu16;
    for &b in data {
        crc = (crc << 8) ^ CRC16[((crc >> 8) as u8 ^ b) as usize];
    }
    crc
}

/// Incremental CRC-32/ISO-HDLC (the zlib / Ethernet CRC).
#[derive(Clone, Copy)]
pub struct Crc32(u32);

impl Crc32 {
    pub const fn new() -> Self {
        Self(0xFFFF_FFFF)
    }

    pub fn update(&mut self, data: &[u8]) {
        let mut crc = self.0;
        for &b in data {
            crc = (crc >> 8) ^ CRC32[((crc as u8) ^ b) as usize];
        }
        self.0 = crc;
    }

    pub const fn finish(self) -> u32 {
        !self.0
    }
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

pub fn crc32(data: &[u8]) -> u32 {
    let mut c = Crc32::new();
    c.update(data);
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_values() {
        // The standard "check" values from the CRC catalogue.
        assert_eq!(crc16(b"123456789"), 0x29B1);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn incremental_crc32() {
        let mut c = Crc32::new();
        c.update(b"1234");
        c.update(b"56789");
        assert_eq!(c.finish(), 0xCBF4_3926);
    }
}
