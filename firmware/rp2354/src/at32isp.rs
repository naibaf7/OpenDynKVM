//! Programs an AT32F405 through its ROM bootloader's SPI interface on the link pins (design.md §10, step 1.3).
//!
//! **Unverified protocol.** Artery's ISP protocol note is still missing (design.md §15). This follows ST's SPI
//! bootloader protocol (AN4286, byte timing from AN2606 Table 231), whose USART variant Artery's ROMs share. The risk
//! stays small:
//! - Every chunk is read back, so a mismatch shows up as a failed update, never as a silently wrong image.
//! - The ROM bootloader cannot be overwritten, and user system data (nBOOT1, analysis.md finding 11) is never written,
//!   so the SPI and USB DFU recovery paths stay open whatever happens here.
//!
//! The PC hub stays in reset meanwhile, so the PC never sees the ROM's DFU device (analysis.md finding 11).

use defmt::{Format, info};
use embassy_rp::gpio::Output;
use embassy_rp::spi::{self, Async, Spi};
use embassy_time::{Duration, Instant, Timer};

use crate::board::At32Lines;

const SOF: u8 = 0x5A;
const ACK: u8 = 0x79;
const NACK: u8 = 0x1F;
/// The ROM polls its SPI in software: slow clock, and a gap after every byte (AN2606: 8 µs).
pub const ISP_HZ: u32 = 1_000_000;
const BYTE_GAP: Duration = Duration::from_micros(10);
const FLASH_BASE: u32 = 0x0800_0000;
const CHUNK: usize = 256;

mod cmd {
    pub const GET: u8 = 0x00;
    pub const GET_ID: u8 = 0x02;
    pub const READ: u8 = 0x11;
    pub const WRITE: u8 = 0x31;
    pub const ERASE: u8 = 0x43;
    pub const EXTENDED_ERASE: u8 = 0x44;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Format)]
pub enum Error {
    /// No ACK to the synchronisation byte: no ROM bootloader listening on SPI.
    NoSync,
    Nack(u8),
    Timeout(u8),
    /// A reply that doesn't fit the protocol as implemented here.
    BadReply(u8),
    /// Read-back differs at this address.
    Verify(u32),
}

fn xor(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |a, b| a ^ b)
}

struct Isp<'a, T: spi::Instance + 'static> {
    spi: &'a mut Spi<'static, T, Async>,
    /// Whether the ROM sends a dummy byte before the data of a reply (AN4286 does; detected from `Get`).
    dummy_first: bool,
}

impl<T: spi::Instance + 'static> Isp<'_, T> {
    async fn byte(&mut self, b: u8) -> u8 {
        let mut rx = [0u8];
        let _ = self.spi.transfer(&mut rx, &[b]).await;
        Timer::after(BYTE_GAP).await;
        rx[0]
    }

    async fn send(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.byte(b).await;
        }
    }

    /// Bytes followed by their XOR checksum.
    async fn send_checked(&mut self, bytes: &[u8]) {
        self.send(bytes).await;
        self.byte(xor(bytes)).await;
    }

    /// AN4286 "get ACK": clock dummy bytes until ACK or NACK comes back, then acknowledge it.
    async fn ack(&mut self, c: u8, timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.byte(0x00).await {
                ACK => {
                    self.byte(ACK).await;
                    return Ok(());
                }
                NACK => {
                    self.byte(ACK).await;
                    return Err(Error::Nack(c));
                }
                _ if Instant::now() > deadline => return Err(Error::Timeout(c)),
                _ => {}
            }
        }
    }

    async fn command(&mut self, c: u8) -> Result<(), Error> {
        self.send(&[SOF, c, !c]).await;
        self.ack(c, Duration::from_millis(100)).await
    }

    async fn read_reply(&mut self, out: &mut [u8]) {
        if self.dummy_first {
            self.byte(0).await;
        }
        for b in out {
            *b = self.byte(0).await;
        }
    }

    async fn sync(&mut self) -> Result<(), Error> {
        self.byte(SOF).await;
        self.ack(SOF, Duration::from_millis(500)).await.map_err(|_| Error::NoSync)
    }

    /// `Get`: the bootloader version and the commands it supports. Also settles [`Self::dummy_first`].
    async fn get(&mut self, commands: &mut [u8; 32]) -> Result<(u8, usize), Error> {
        self.command(cmd::GET).await?;
        let b0 = self.byte(0).await;
        // N, the number of commands (bytes to follow − 1), is never 0x00, 0xA5 or 0xFF; a dummy byte is.
        let n = if matches!(b0, 0x00 | 0xA5 | 0xFF) {
            self.dummy_first = true;
            self.byte(0).await as usize
        } else {
            b0 as usize
        };
        if n == 0 || n > commands.len() {
            return Err(Error::BadReply(cmd::GET));
        }
        let version = self.byte(0).await;
        for c in commands[..n].iter_mut() {
            *c = self.byte(0).await;
        }
        self.ack(cmd::GET, Duration::from_millis(100)).await?;
        Ok((version, n))
    }

    async fn get_id(&mut self) -> Result<u16, Error> {
        self.command(cmd::GET_ID).await?;
        let mut r = [0u8; 3];
        self.read_reply(&mut r).await;
        self.ack(cmd::GET_ID, Duration::from_millis(100)).await?;
        Ok(u16::from_be_bytes([r[1], r[2]]))
    }

    async fn mass_erase(&mut self, extended: bool) -> Result<(), Error> {
        let c = if extended { cmd::EXTENDED_ERASE } else { cmd::ERASE };
        self.command(c).await?;
        if extended {
            self.send_checked(&[0xFF, 0xFF]).await;
        } else {
            self.send(&[0xFF, 0x00]).await;
        }
        // 256 KB of main flash; the RM gives no mass-erase time, so be generous.
        self.ack(c, Duration::from_secs(10)).await
    }

    async fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), Error> {
        self.command(cmd::WRITE).await?;
        self.send_checked(&addr.to_be_bytes()).await;
        self.ack(cmd::WRITE, Duration::from_millis(100)).await?;
        let n = (data.len() - 1) as u8;
        self.byte(n).await;
        self.send(data).await;
        self.byte(n ^ xor(data)).await;
        self.ack(cmd::WRITE, Duration::from_secs(1)).await
    }

    async fn read(&mut self, addr: u32, out: &mut [u8]) -> Result<(), Error> {
        self.command(cmd::READ).await?;
        self.send_checked(&addr.to_be_bytes()).await;
        self.ack(cmd::READ, Duration::from_millis(100)).await?;
        let n = (out.len() - 1) as u8;
        self.send(&[n, !n]).await;
        self.ack(cmd::READ, Duration::from_millis(100)).await?;
        self.read_reply(out).await;
        Ok(())
    }
}

async fn session<T: spi::Instance>(spi: &mut Spi<'static, T, Async>, image: &[u8]) -> Result<(), Error> {
    let mut isp = Isp { spi, dummy_first: false };
    isp.sync().await?;
    let mut commands = [0u8; 32];
    let (version, n) = isp.get(&mut commands).await?;
    let commands = &commands[..n];
    let id = isp.get_id().await?;
    info!("AT32 ROM bootloader {=u8:#x}, chip ID {=u16:#x}, commands {=[u8]:#x}", version, id, commands);
    let extended = if commands.contains(&cmd::EXTENDED_ERASE) {
        true
    } else if commands.contains(&cmd::ERASE) {
        false
    } else {
        return Err(Error::BadReply(cmd::GET));
    };
    isp.mass_erase(extended).await?;
    let mut back = [0u8; CHUNK];
    for (i, chunk) in image.chunks(CHUNK).enumerate() {
        let addr = FLASH_BASE + (i * CHUNK) as u32;
        isp.write(addr, chunk).await?;
        let back = &mut back[..chunk.len()];
        isp.read(addr, back).await?;
        if back != chunk {
            return Err(Error::Verify(addr));
        }
    }
    Ok(())
}

/// Starts the AT32F405 normally (BOOT0 low across a reset).
pub async fn reset_normal(lines: &mut At32Lines) {
    lines.boot0.set_low();
    lines.set_reset(true);
    Timer::after_millis(5).await;
    lines.set_reset(false);
}

/// Starts the AT32F405 in its ROM bootloader (BOOT0 high across a reset). BOOT0 is sampled at reset only, so it is
/// released again once the ROM runs: the next reset boots from flash.
pub async fn reset_to_rom(lines: &mut At32Lines) {
    lines.boot0.set_high();
    lines.set_reset(true);
    Timer::after_millis(5).await;
    lines.set_reset(false);
    Timer::after_millis(100).await;
    lines.boot0.set_low();
}

/// Writes `image` to the AT32F405's flash at 0x0800_0000 and restarts it, whatever the outcome. The link SPI runs at
/// [`ISP_HZ`] meanwhile and is set back to `link_hz`.
pub async fn program<T: spi::Instance>(
    spi: &mut Spi<'static, T, Async>,
    cs: &mut Output<'static>,
    lines: &mut At32Lines,
    image: &[u8],
    link_hz: u32,
) -> Result<(), Error> {
    lines.hold_hub(true);
    reset_to_rom(lines).await;
    spi.set_frequency(ISP_HZ);
    cs.set_low();
    let r = session(spi, image).await;
    cs.set_high();
    spi.set_frequency(link_hz);
    reset_normal(lines).await;
    lines.hold_hub(false);
    r
}
