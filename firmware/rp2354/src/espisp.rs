//! Programs the Bluetooth module (ESP32-C3) through its ROM serial bootloader (software.md §4.13), as `at32isp.rs`
//! does the AT32F405s: the RP2354B carries the module's image and writes it whenever the module runs another version
//! or none. The module is on the board's UART, or an ESP32-C3 board on a front port (its USB serial port,
//! `dongle.rs`): the protocol is the same over both ([`Line`]), only the way in and out of the ROM differs.
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
//! The first flash sector is erased first and written last ([`Image`]): a module whose programming broke off (power,
//! unplugged) has it erased, which is what [`blank_c3`] looks for before it lets a dongle that doesn't answer as the
//! module be programmed. Any other firmware has its bootloader there.
//!
//! On the board, the module's UART: entry is IO9 (BOOT) low at a reset (the module's strapping, datasheet section 4;
//! IO2 and IO8 are pulled up on the board). The board has two wires to the module, and IO9 is on the RP2354B's TX
//! line (`ESP_RXD`), so:
//! - a running module restarts into its ROM by itself on [`kvm_proto::link::BtMsg::EnterRom`];
//! - anything else (blank, or a firmware that doesn't answer) resets over and over: the ROM finds no image, or a
//!   watchdog bites. [`break_into_rom`] holds TX low (a UART break) across such resets.
//!
//! Exit, on both lines: a restart through the RTC watchdog, FORCE_DOWNLOAD_BOOT cleared first (espflash's
//! `WatchdogReset`); with TX idle (high) IO9 boots the firmware. The ROM's own reboot (FLASH_END) keeps that flag,
//! which `EnterRom` sets, and a dongle restarted by RTS from its ROM came back in it (first dongle, 2026-11-14). The
//! same gets a module out of the ROM that landed there by chance (TX low at an unplanned reset).

use defmt::{debug, warn};
use embassy_rp::uart::BufferedUart;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_io_async::{Read, Write};

/// The ROM's own speed; the transfer runs at [`FAST_BAUD`] once the ROM agreed to change.
pub const ROM_BAUD: u32 = 115_200;
const FAST_BAUD: u32 = 921_600;
const BLOCK: usize = 0x400;
const FLASH_SIZE: u32 = 4 * 1024 * 1024;
/// `GET_SECURITY_INFO`'s chip ID of the ESP32-C3; ROMs without one there (ECO2 and older) are told by the magic value
/// at [`CHIP_DETECT_MAGIC_REG`] (espflash 4.7, `target/mod.rs`).
const ESP32C3_CHIP_ID: u32 = 5;
const CHIP_DETECT_MAGIC_REG: u32 = 0x4000_1000;

/// The ESP32-C3's RTC watchdog and boot option (RTC_CNTL at 0x6000_8000; espflash 4.7 `target/mod.rs`, the
/// `esp32c3` PAC): write protection and its key, configuration 0 and 1, and OPTION1, whose bit 0 is
/// FORCE_DOWNLOAD_BOOT (kept over every restart but a power-up).
mod rtc {
    pub const WDT_WPROTECT: u32 = 0x6000_80A8;
    pub const WDT_WKEY: u32 = 0x50D8_3AA1;
    pub const WDT_CONFIG0: u32 = 0x6000_8090;
    pub const WDT_CONFIG1: u32 = 0x6000_8094;
    pub const OPTION1: u32 = 0x6000_80F4;
    pub const FORCE_DOWNLOAD_BOOT: u32 = 1;
    /// Enabled, stage 0 resets the whole chip (RTC included), chip reset on, its width: espflash's `rtc_wdt_reset`.
    pub const WDT_RESET: u32 = 1 << 31 | 5 << 28 | 1 << 8 | 1 << 2;
}
const ESP32C3_MAGIC: [u32; 4] = [0x6921_506f, 0x1b31_506f, 0x4881_606f, 0x4361_606f];

mod op {
    pub const SPI_SET_PARAMS: u8 = 0x0B;
    pub const SPI_ATTACH: u8 = 0x0D;
    pub const SYNC: u8 = 0x08;
    pub const WRITE_REG: u8 = 0x09;
    pub const READ_REG: u8 = 0x0A;
    pub const CHANGE_BAUDRATE: u8 = 0x0F;
    pub const FLASH_DEFL_BEGIN: u8 = 0x10;
    pub const FLASH_DEFL_DATA: u8 = 0x11;
    pub const FLASH_DEFL_END: u8 = 0x12;
    pub const SPI_FLASH_MD5: u8 = 0x13;
    pub const GET_SECURITY_INFO: u8 = 0x14;
    pub const FLASH_BEGIN: u8 = 0x02;
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
    /// Not an ESP32-C3 (its chip ID), so the image is not for it.
    Chip(u32),
    Line,
}

/// The ROM's last error since start, as [`code`] (diagnostics: `BOARD_ROM_ERROR`).
pub static LAST_ERROR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// An error as one number: 1 no SYNC, 0x1_00oo timeout of op oo, 0x2_ooee op oo failed with ROM error ee, 3 verify,
/// 0x4_iiii another chip (its ID or magic, low 16 bits), 5 the line.
pub fn code(e: Error) -> u32 {
    match e {
        Error::NoSync => 1,
        Error::Timeout(op) => 0x1_0000 | op as u32,
        Error::Failed(op, _, err) => 0x2_0000 | (op as u32) << 8 | err as u32,
        Error::Verify => 3,
        Error::Chip(id) => 0x4_0000 | (id & 0xFFFF),
        Error::Line => 5,
    }
}

fn noted(e: Error) -> Error {
    LAST_ERROR.store(code(e), core::sync::atomic::Ordering::Relaxed);
    e
}

/// The line to the module's ROM: the board's UART, or a dongle's USB serial port.
#[allow(async_fn_in_trait)]
pub trait Line: Read + Write {
    /// A UART: the ROM is asked for a faster speed for the transfer, and restarts the module itself at the end (IO9
    /// on the idle TX line boots its firmware). A USB serial port has no speed, and its DTR/RTS restart the chip.
    const UART: bool;
    fn set_baud(&mut self, baud: u32);
    /// Whatever arrives within `timeout`, up to `buf.len()` bytes; `Ok(0)` if nothing did. The USB serial port waits
    /// between whole USB transactions, so giving up never loses part of one.
    async fn read_within(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ()>;
}

impl Line for BufferedUart {
    const UART: bool = true;

    fn set_baud(&mut self, baud: u32) {
        self.set_baudrate(baud);
    }

    async fn read_within(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ()> {
        match with_timeout(timeout, self.read(buf)).await {
            Ok(Ok(n)) => Ok(n),
            Ok(Err(_)) => Err(()),
            Err(_) => Ok(0),
        }
    }
}

/// What the RP2354B holds of the module's image (`build.rs`): zlib (deflate) data of the whole flash image from
/// offset 0 (bootloader, partition table, application), split after its first sector.
pub struct Image {
    /// The first `head_len` bytes, written last.
    pub head: &'static [u8],
    pub head_len: u32,
    /// The rest, from `head_len` on, written first.
    pub tail: &'static [u8],
    /// The size uncompressed.
    pub len: u32,
    /// MD5 of the uncompressed image, as the ROM reports it (lowercase hex).
    pub md5: &'static [u8; 32],
    /// MD5 of the first sector erased.
    pub blank_md5: &'static [u8; 32],
}

struct Rom<'a, L: Line> {
    line: &'a mut L,
    rx: [u8; 96],
}

fn checksum(data: &[u8]) -> u32 {
    data.iter().fold(0xEFu8, |c, b| c ^ b) as u32
}

impl<L: Line> Rom<'_, L> {
    async fn send_slip(&mut self, parts: &[&[u8]]) -> Result<(), Error> {
        let mut buf = [0u8; 64];
        let mut n = 0;
        buf[n] = 0xC0;
        n += 1;
        for part in parts {
            for &b in *part {
                if n + 2 > buf.len() {
                    self.line.write_all(&buf[..n]).await.map_err(|_| Error::Line)?;
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
            self.line.write_all(&buf[..n]).await.map_err(|_| Error::Line)?;
            n = 0;
        }
        buf[n] = 0xC0;
        n += 1;
        self.line.write_all(&buf[..n]).await.map_err(|_| Error::Line)?;
        self.line.flush().await.map_err(|_| Error::Line)
    }

    /// Reads one SLIP frame into `self.rx` by `deadline`; returns its length.
    async fn recv_slip(&mut self, deadline: Instant, op: u8) -> Result<usize, Error> {
        let mut n = 0;
        let mut started = false;
        let mut esc = false;
        let mut b = [0u8; 1];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.line.read_within(&mut b, left).await {
                Ok(0) if left == Duration::from_ticks(0) => return Err(Error::Timeout(op)),
                Ok(0) => continue,
                Ok(_) => {}
                Err(()) => return Err(Error::Line),
            }
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
    /// response's value word and its data's end in `rx` (without the status bytes).
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
        self.send_slip(&parts).await.map_err(noted)?;
        let deadline = Instant::now() + timeout;
        loop {
            let n = self.recv_slip(deadline, op).await.map_err(noted)?;
            if n < 8 + 4 || self.rx[0] != 0x01 || self.rx[1] != op {
                continue;
            }
            let value = u32::from_le_bytes([self.rx[4], self.rx[5], self.rx[6], self.rx[7]]);
            let status = &self.rx[n - 4..n];
            if status[0] != 0 {
                return Err(noted(Error::Failed(op, status[0], status[1])));
            }
            return Ok((value, n - 4));
        }
    }

    async fn sync(&mut self, tries: u8) -> Result<(), Error> {
        let mut frame = [0x55u8; 36];
        frame[..4].copy_from_slice(&[0x07, 0x07, 0x12, 0x20]);
        for _ in 0..tries {
            if self.command(op::SYNC, &[&frame], 0, Duration::from_millis(100)).await.is_ok() {
                // The ROM answers SYNC several times: let the rest pass.
                Timer::after_millis(50).await;
                drain(self.line).await;
                return Ok(());
            }
        }
        Err(noted(Error::NoSync))
    }

    /// WRITE_REG: `value` into the bits of `mask` at `addr`.
    async fn write_reg(&mut self, addr: u32, value: u32, mask: u32) -> Result<(), Error> {
        let mut d = [0u8; 16];
        for (k, v) in [addr, value, mask, 0].iter().enumerate() {
            d[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.command(op::WRITE_REG, &[&d], 0, Duration::from_millis(500)).await.map(|_| ())
    }

    /// Turns the RTC watchdog off: the ROM leaves it running in USB download mode, where it would restart the chip in
    /// the middle of a long write (espflash does the same without its flasher stub).
    async fn watchdog_off(&mut self) -> Result<(), Error> {
        self.write_reg(rtc::WDT_WPROTECT, rtc::WDT_WKEY, u32::MAX).await?;
        self.write_reg(rtc::WDT_CONFIG0, 0, u32::MAX).await?;
        self.write_reg(rtc::WDT_WPROTECT, 0, u32::MAX).await
    }

    /// Restarts the chip into its firmware through the RTC watchdog, as espflash's `WatchdogReset` does for an
    /// ESP32-C3 on its USB serial port: a reset by RTS from the ROM brought a board back in the ROM (first dongle,
    /// 2026-11-14), until it was powered off. FORCE_DOWNLOAD_BOOT is cleared first.
    async fn watchdog_restart(&mut self) -> Result<(), Error> {
        self.write_reg(rtc::OPTION1, 0, rtc::FORCE_DOWNLOAD_BOOT).await?;
        self.write_reg(rtc::WDT_WPROTECT, rtc::WDT_WKEY, u32::MAX).await?;
        self.write_reg(rtc::WDT_CONFIG1, 2000, u32::MAX).await?;
        self.write_reg(rtc::WDT_CONFIG0, rtc::WDT_RESET, u32::MAX).await?;
        // The chip restarts within milliseconds; the answer to this may not come.
        let _ = self.write_reg(rtc::WDT_WPROTECT, 0, u32::MAX).await;
        Ok(())
    }

    /// The chip must be an ESP32-C3: the image is for it alone.
    async fn check_chip(&mut self) -> Result<(), Error> {
        // Data: flags u32, flash_crypt_cnt u8, key_purposes [u8; 7], chip_id u32, eco_version u32 (12 bytes without the
        // chip ID).
        let info = self.command(op::GET_SECURITY_INFO, &[], 0, Duration::from_millis(500)).await;
        if let Ok((_, end)) = info
            && end >= 8 + 16
        {
            let id = u32::from_le_bytes([self.rx[20], self.rx[21], self.rx[22], self.rx[23]]);
            return if id == ESP32C3_CHIP_ID { Ok(()) } else { Err(noted(Error::Chip(id))) };
        }
        let (magic, _) =
            self.command(op::READ_REG, &[&CHIP_DETECT_MAGIC_REG.to_le_bytes()], 0, Duration::from_millis(500)).await?;
        if ESP32C3_MAGIC.contains(&magic) { Ok(()) } else { Err(noted(Error::Chip(magic))) }
    }

    /// The in-package flash attached (default pins: 0, ROM form with the extra word), and its geometry.
    async fn attach_flash(&mut self) -> Result<(), Error> {
        self.command(op::SPI_ATTACH, &[&[0u8; 8]], 0, Duration::from_secs(1)).await?;
        let mut p = [0u8; 24];
        for (k, v) in [0u32, FLASH_SIZE, 64 * 1024, 4 * 1024, 256, 0xFFFF].iter().enumerate() {
            p[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.command(op::SPI_SET_PARAMS, &[&p], 0, Duration::from_secs(1)).await?;
        Ok(())
    }

    /// MD5 of `len` bytes of flash from `offset`, compared with `want` (32 lowercase hex digits).
    async fn md5_is(&mut self, offset: u32, len: u32, want: &[u8; 32]) -> Result<bool, Error> {
        let mut md5 = [0u8; 16];
        md5[..4].copy_from_slice(&offset.to_le_bytes());
        md5[4..8].copy_from_slice(&len.to_le_bytes());
        let (_, n) = self.command(op::SPI_FLASH_MD5, &[&md5], 0, Duration::from_secs(10)).await?;
        Ok(n >= 8 + 32 && self.rx[8..8 + 32].eq_ignore_ascii_case(want))
    }

    /// One region, compressed, from `offset`: FLASH_DEFL_BEGIN (the ROM erases it first), then the data blocks.
    async fn write_region(&mut self, zlib: &[u8], len: u32, offset: u32) -> Result<(), Error> {
        let blocks = zlib.len().div_ceil(BLOCK) as u32;
        let mut begin = [0u8; 20];
        for (k, v) in [len, blocks, BLOCK as u32, offset, 0].iter().enumerate() {
            begin[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
        }
        // The ROM erases the whole region first: about 30 s per MB at worst (espflash's ERASE_REGION_TIMEOUT_PER_MB).
        let erase = Duration::from_millis(3000 + len as u64 * 30 / 1024);
        self.command(op::FLASH_DEFL_BEGIN, &[&begin], 0, erase).await?;
        for (seq, chunk) in zlib.chunks(BLOCK).enumerate() {
            let mut head = [0u8; 16];
            head[..4].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
            head[4..8].copy_from_slice(&(seq as u32).to_le_bytes());
            self.command(op::FLASH_DEFL_DATA, &[&head, chunk], checksum(chunk), Duration::from_secs(5)).await?;
            if seq % 64 == 0 {
                debug!("module: block {} of {} at {=u32:#x}", seq, blocks, offset);
            }
        }
        Ok(())
    }
}

/// Discards whatever the module has sent (its boot log, extra answers).
pub async fn drain<L: Line>(line: &mut L) {
    let mut b = [0u8; 64];
    while let Ok(n) = line.read_within(&mut b, Duration::from_millis(20)).await {
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

/// Connects to a module whose ROM bootloader listens (after [`break_into_rom`], `EnterRom` or a dongle's DTR/RTS).
async fn connect<L: Line>(line: &mut L, tries: u8) -> Result<Rom<'_, L>, Error> {
    line.set_baud(ROM_BAUD);
    drain(line).await;
    let mut rom = Rom { line, rx: [0; 96] };
    rom.sync(tries).await?;
    Ok(rom)
}

/// Whether the module's ROM bootloader listens right now (a dongle that is already in it, after a reset that made it
/// show up on USB anew).
pub async fn listening<L: Line>(line: &mut L) -> bool {
    connect(line, 3).await.is_ok()
}

/// A module that sits in its ROM bootloader (it answers SYNC) is restarted into its firmware by the RTC watchdog;
/// false if no ROM answers. The ROM's own reboot (FLASH_END) keeps FORCE_DOWNLOAD_BOOT, which `EnterRom` sets: the
/// module would come back in its ROM.
pub async fn leave_rom<L: Line>(line: &mut L) -> bool {
    let left = match connect(line, 10).await {
        Ok(mut rom) => rom.watchdog_restart().await.is_ok(),
        Err(_) => false,
    };
    line.set_baud(ROM_BAUD);
    left
}

/// A chip whose ROM bootloader listens is restarted into its firmware by the RTC watchdog (a dongle; the board's UART
/// module restarts with [`leave_rom`]).
pub async fn watchdog_restart<L: Line>(line: &mut L) -> Result<(), Error> {
    let mut rom = connect(line, 3).await?;
    rom.watchdog_restart().await
}

/// Whether the chip in its ROM bootloader is an ESP32-C3 with an erased first flash sector: blank, or the module whose
/// programming broke off ([`program`] writes that sector last). Leaves the ROM listening.
pub async fn blank_c3<L: Line>(line: &mut L, image: &Image) -> Result<bool, Error> {
    let mut rom = connect(line, 10).await?;
    rom.check_chip().await?;
    rom.attach_flash().await?;
    rom.md5_is(0, image.head_len, image.blank_md5).await
}

/// Writes `image` to the module's flash from offset 0, verifies it and restarts the module into it (by its RTC
/// watchdog). The module's ROM bootloader must be listening
/// (`EnterRom`, [`break_into_rom`], or a dongle's DTR/RTS). Leaves the UART at [`ROM_BAUD`]; the caller restores the
/// link's speed.
pub async fn program<L: Line>(line: &mut L, image: &Image) -> Result<(), Error> {
    if image.tail.is_empty() && image.head.is_empty() {
        return Err(Error::Verify);
    }
    let mut rom = connect(line, 10).await?;
    rom.check_chip().await?;
    if L::UART {
        // Faster, if the ROM agrees (new baud, old baud: 0 for the ROM loader).
        let mut args = [0u8; 8];
        args[..4].copy_from_slice(&FAST_BAUD.to_le_bytes());
        if rom.command(op::CHANGE_BAUDRATE, &[&args], 0, Duration::from_millis(300)).await.is_ok() {
            Timer::after_millis(20).await;
            rom.line.set_baud(FAST_BAUD);
            Timer::after_millis(20).await;
            drain(rom.line).await;
        }
    }
    rom.attach_flash().await?;
    rom.watchdog_off().await?;
    // The first sector goes first (erased, nothing written) and comes back last.
    let mut erase = [0u8; 20];
    for (k, v) in [image.head_len, 0, BLOCK as u32, 0, 0].iter().enumerate() {
        erase[4 * k..4 * k + 4].copy_from_slice(&v.to_le_bytes());
    }
    rom.command(op::FLASH_BEGIN, &[&erase], 0, Duration::from_secs(3)).await?;
    if !image.tail.is_empty() {
        rom.write_region(image.tail, image.len - image.head_len, image.head_len).await?;
    }
    rom.write_region(image.head, image.head_len, 0).await?;
    // Stay in the loader (1) for the check below.
    rom.command(op::FLASH_DEFL_END, &[&1u32.to_le_bytes()], 0, Duration::from_secs(2)).await?;
    if !rom.md5_is(0, image.len, image.md5).await? {
        rom.line.set_baud(ROM_BAUD);
        warn!("module: flash differs from the image after writing it");
        return Err(Error::Verify);
    }
    // Out through the RTC watchdog, FORCE_DOWNLOAD_BOOT cleared: the ROM's own reboot keeps that flag, which
    // `EnterRom` sets, and a reset by RTS from the ROM brought a dongle back in its ROM (first dongle, 2026-11-14).
    let restarted = rom.watchdog_restart().await;
    rom.line.set_baud(ROM_BAUD);
    restarted
}
