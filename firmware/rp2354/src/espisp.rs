//! Programs the Bluetooth module (ESP32-C3-MINI-1) through its ROM serial bootloader (software.md §4.13), as
//! `at32isp.rs` does the AT32F405s: the RP2354B carries the module's image and writes it whenever the module runs
//! another version or none.
//!
//! The protocol is Espressif's serial bootloader protocol (esptool's "serial protocol" page; the framing and sizes
//! here follow espflash 4.7, `src/command.rs` and `src/connection/mod.rs`):
//! - SLIP frames (0xC0 … 0xC0, 0xC0 → DB DC, 0xDB → DB DD);
//! - a command is `0x00 | op | len u16 | checksum u32 | data`, its response `0x01 | op | len u16 | value u32 | data`,
//!   whose last 4 bytes are the ROM's status bytes (status, error, 0, 0; the stub loader has 2);
//! - the image goes over compressed (FLASH_DEFL_*): zlib data in blocks of 0x400 bytes, which the ROM inflates and
//!   writes; the checksum of a data block is the XOR of its bytes, seeded 0xEF;
//! - SPI_FLASH_MD5 then reads back the written region: the ROM answers 32 hex digits, which must match the image's
//!   MD5 computed at build time. Only that counts as success.
//!
//! Entry: IO9 (BOOT) low at a reset (the module's strapping, datasheet section 4; IO2 and IO8 are pulled up on the
//! board). The board has two wires to the module, and IO9 is on the RP2354B's TX line (`ESP_RXD`), so:
//! - a running module restarts into its ROM by itself on [`kvm_proto::link::BtMsg::EnterRom`];
//! - anything else (blank, or a firmware that doesn't answer) resets over and over: the ROM finds no image, or a
//!   watchdog bites. [`break_into_rom`] holds TX low (a UART break) across such resets.
//!
//! Exit: FLASH_BEGIN (size 0) and FLASH_END "reboot" restart the module, and with TX idle (high) IO9 boots its
//! firmware. The same gets a module out of the ROM that landed there by chance (TX low at an unplanned reset).

use defmt::{debug, warn};
use embassy_rp::uart::BufferedUart;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{Read, Write};

/// The ROM's own speed; the transfer runs at [`FAST_BAUD`] once the ROM agreed to change.
pub const ROM_BAUD: u32 = 115_200;
const FAST_BAUD: u32 = 921_600;
const BLOCK: usize = 0x400;
const FLASH_SIZE: u32 = 4 * 1024 * 1024;

mod op {
    pub const SPI_SET_PARAMS: u8 = 0x0B;
    pub const SPI_ATTACH: u8 = 0x0D;
    pub const SYNC: u8 = 0x08;
    pub const CHANGE_BAUDRATE: u8 = 0x0F;
    pub const FLASH_DEFL_BEGIN: u8 = 0x10;
    pub const FLASH_DEFL_DATA: u8 = 0x11;
    pub const FLASH_DEFL_END: u8 = 0x12;
    pub const SPI_FLASH_MD5: u8 = 0x13;
    pub const FLASH_BEGIN: u8 = 0x02;
    pub const FLASH_END: u8 = 0x04;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum Error {
    /// The ROM never answered SYNC: no module, no download mode, or a dead line.
    NoSync,
    /// A command timed out.
    Timeout(u8),
    /// A command failed: op, ROM status, ROM error code.
    Failed(u8, u8, u8),
    /// The flash read back differs from the image.
    Verify,
    Uart,
}

/// What the RP2354B holds of the module's image (`build.rs`).
pub struct Image {
    /// zlib (deflate) data of the whole flash image from offset 0: bootloader, partition table, application.
    pub zlib: &'static [u8],
    /// Its size uncompressed.
    pub len: u32,
    /// MD5 of the uncompressed image, as the ROM reports it (lowercase hex).
    pub md5: &'static [u8; 32],
}

struct Rom<'a> {
    uart: &'a mut BufferedUart,
    rx: [u8; 96],
}

fn checksum(data: &[u8]) -> u32 {
    data.iter().fold(0xEFu8, |c, b| c ^ b) as u32
}

impl Rom<'_> {
    async fn send_slip(&mut self, parts: &[&[u8]]) -> Result<(), Error> {
        let mut buf = [0u8; 64];
        let mut n = 0;
        buf[n] = 0xC0;
        n += 1;
        for part in parts {
            for &b in *part {
                if n + 2 > buf.len() {
                    self.uart.write_all(&buf[..n]).await.map_err(|_| Error::Uart)?;
                    n = 0;
                }
                match b {
                    0xC0 => {
                        buf[n] = 0xDB;
                        buf[n + 1] = 0xDC;
                        n += 2;
                    }
                    0xDB => {
                        buf[n] = 0xDB;
                        buf[n + 1] = 0xDD;
                        n += 2;
                    }
                    _ => {
                        buf[n] = b;
                        n += 1;
                    }
                }
            }
        }
        if n + 1 > buf.len() {
            self.uart.write_all(&buf[..n]).await.map_err(|_| Error::Uart)?;
            n = 0;
        }
        buf[n] = 0xC0;
        n += 1;
        self.uart.write_all(&buf[..n]).await.map_err(|_| Error::Uart)?;
        self.uart.flush().await.map_err(|_| Error::Uart)
    }

    /// Reads one SLIP frame into `self.rx`; returns its length.
    async fn recv_slip(&mut self) -> Result<usize, Error> {
        let mut n = 0;
        let mut started = false;
        let mut esc = false;
        let mut b = [0u8; 1];
        loop {
            self.uart.read_exact(&mut b).await.map_err(|_| Error::Uart)?;
            let c = b[0];
            if !started {
                started = c == 0xC0;
                continue;
            }
            match (c, esc) {
                (0xC0, _) if n == 0 => continue,
                (0xC0, _) => return Ok(n),
                (0xDB, false) => esc = true,
                (c, true) => {
                    esc = false;
                    if n < self.rx.len() {
                        self.rx[n] = match c {
                            0xDC => 0xC0,
                            0xDD => 0xDB,
                            c => c,
                        };
                        n += 1;
                    }
                }
                (c, false) => {
                    if n < self.rx.len() {
                        self.rx[n] = c;
                        n += 1;
                    }
                }
            }
        }
    }

    /// Sends a command and waits for its response (other responses, e.g. extra SYNC answers, are skipped). Returns the
    /// response's value word and its data (without the status bytes).
    async fn command(&mut self, op: u8, data: &[&[u8]], chk: u32, timeout: Duration) -> Result<(u32, usize), Error> {
        let len: usize = data.iter().map(|d| d.len()).sum();
        let mut head = [0u8; 8];
        head[1] = op;
        head[2..4].copy_from_slice(&(len as u16).to_le_bytes());
        head[4..8].copy_from_slice(&chk.to_le_bytes());
        let mut parts: heapless::Vec<&[u8], 4> = heapless::Vec::new();
        let _ = parts.push(&head);
        for d in data {
            let _ = parts.push(d);
        }
        self.send_slip(&parts).await?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let n = with_timeout(left, self.recv_slip()).await.map_err(|_| Error::Timeout(op))??;
            if n < 8 + 4 || self.rx[0] != 0x01 || self.rx[1] != op {
                continue;
            }
            let value = u32::from_le_bytes([self.rx[4], self.rx[5], self.rx[6], self.rx[7]]);
            let status = &self.rx[n - 4..n];
            if status[0] != 0 {
                return Err(Error::Failed(op, status[0], status[1]));
            }
            return Ok((value, n - 4));
        }
    }

    async fn sync(&mut self) -> Result<(), Error> {
        let mut frame = [0x55u8; 36];
        frame[..4].copy_from_slice(&[0x07, 0x07, 0x12, 0x20]);
        for _ in 0..10 {
            if self.command(op::SYNC, &[&frame], 0, Duration::from_millis(100)).await.is_ok() {
                // The ROM answers SYNC several times: let the rest pass.
                Timer::after_millis(50).await;
                drain(self.uart).await;
                return Ok(());
            }
        }
        Err(Error::NoSync)
    }
}

/// Discards whatever the module has sent (its boot log, extra answers).
async fn drain(uart: &mut BufferedUart) {
    let mut b = [0u8; 64];
    while let Ok(Ok(n)) = with_timeout(Duration::from_millis(20), uart.read(&mut b)).await {
        if n == 0 {
            break;
        }
    }
}

/// Holds TX (and so the module's IO9) low for `ms`: a module that resets meanwhile starts in its ROM bootloader.
pub async fn break_into_rom(uart: &mut BufferedUart, ms: u32) {
    uart.set_baudrate(ROM_BAUD);
    // send_break counts bit times at the current speed.
    uart.send_break(ms * (ROM_BAUD / 1000)).await;
    Timer::after_millis(5).await;
}

/// Connects to a module whose ROM bootloader listens (after [`break_into_rom`] or `EnterRom`).
async fn connect(uart: &mut BufferedUart) -> Result<Rom<'_>, Error> {
    uart.set_baudrate(ROM_BAUD);
    drain(uart).await;
    let mut rom = Rom { uart, rx: [0; 96] };
    rom.sync().await?;
    Ok(rom)
}

impl Rom<'_> {
    /// FLASH_BEGIN of nothing, then FLASH_END "reboot": the ROM restarts the module (esptool's soft reset for the ROM
    /// loader). With TX idle, IO9 reads high and the module boots its firmware.
    async fn reboot(&mut self) -> Result<(), Error> {
        let mut begin = [0u8; 20];
        begin[8..12].copy_from_slice(&(BLOCK as u32).to_le_bytes());
        self.command(op::FLASH_BEGIN, &[&begin], 0, Duration::from_secs(2)).await?;
        // 0 = reboot (espflash FlashEnd { reboot: true } sends !reboot).
        let _ = self.command(op::FLASH_END, &[&0u32.to_le_bytes()], 0, Duration::from_millis(300)).await;
        self.uart.set_baudrate(ROM_BAUD);
        Ok(())
    }
}

/// A module that sits in its ROM bootloader (it answers SYNC) is restarted into its firmware; false if no ROM answers.
pub async fn leave_rom(uart: &mut BufferedUart) -> bool {
    match connect(uart).await {
        Ok(mut rom) => rom.reboot().await.is_ok(),
        Err(_) => false,
    }
}

/// Writes `image` to the module's flash from offset 0, verifies it and restarts the module into it. The module's ROM
/// bootloader must be listening (`EnterRom`, or [`break_into_rom`]). Leaves the UART at [`ROM_BAUD`]; the caller
/// restores the link's speed.
pub async fn program(uart: &mut BufferedUart, image: &Image) -> Result<(), Error> {
    if image.zlib.is_empty() {
        return Err(Error::Verify);
    }
    let mut rom = connect(uart).await?;
    // Faster, if the ROM agrees (new baud, old baud: 0 for the ROM loader).
    let mut args = [0u8; 8];
    args[..4].copy_from_slice(&FAST_BAUD.to_le_bytes());
    if rom.command(op::CHANGE_BAUDRATE, &[&args], 0, Duration::from_millis(300)).await.is_ok() {
        Timer::after_millis(20).await;
        rom.uart.set_baudrate(FAST_BAUD);
        Timer::after_millis(20).await;
        drain(rom.uart).await;
    }
    // Attach the in-package flash (default pins: 0), ROM form with the extra word; then its geometry.
    rom.command(op::SPI_ATTACH, &[&[0u8; 8]], 0, Duration::from_secs(1)).await?;
    let mut p = [0u8; 24];
    for (k, v) in [0u32, FLASH_SIZE, 64 * 1024, 4 * 1024, 256, 0xFFFF].iter().enumerate() {
        p[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
    }
    rom.command(op::SPI_SET_PARAMS, &[&p], 0, Duration::from_secs(1)).await?;

    let blocks = image.zlib.len().div_ceil(BLOCK) as u32;
    let mut begin = [0u8; 20];
    for (k, v) in [image.len, blocks, BLOCK as u32, 0, 0].iter().enumerate() {
        begin[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
    }
    // The ROM erases the whole region first: about 30 s per MB at worst (espflash's ERASE_REGION_TIMEOUT_PER_MB).
    let erase = Duration::from_millis(3000 + image.len as u64 * 30 / 1024);
    rom.command(op::FLASH_DEFL_BEGIN, &[&begin], 0, erase).await?;
    for (seq, chunk) in image.zlib.chunks(BLOCK).enumerate() {
        let mut head = [0u8; 16];
        head[..4].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        head[4..8].copy_from_slice(&(seq as u32).to_le_bytes());
        rom.command(op::FLASH_DEFL_DATA, &[&head, chunk], checksum(chunk), Duration::from_secs(5)).await?;
        if seq % 64 == 0 {
            debug!("module: block {} of {}", seq, blocks);
        }
    }
    // Stay in the loader (1) for the check below.
    rom.command(op::FLASH_DEFL_END, &[&1u32.to_le_bytes()], 0, Duration::from_secs(2)).await?;
    let mut md5 = [0u8; 16];
    md5[4..8].copy_from_slice(&image.len.to_le_bytes());
    let (_, n) = rom.command(op::SPI_FLASH_MD5, &[&md5], 0, Duration::from_secs(10)).await?;
    let ok = n >= 8 + 32 && rom.rx[8..8 + 32].eq_ignore_ascii_case(image.md5);
    if !ok {
        rom.uart.set_baudrate(ROM_BAUD);
        warn!("module: flash differs from the image after writing it");
        return Err(Error::Verify);
    }
    rom.reboot().await
}
