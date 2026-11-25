//! The Bluetooth module's firmware as the host software streams it (`FwTarget::Module`, `kvm_proto::modimage`;
//! software.md §4.13). The RP2354B keeps none of it: `bt.rs` puts the module into its ROM loader, and [`Incoming`]
//! hands [`crate::espisp::program`] the data as it arrives.
//!
//! - **What is taken:** only an image this firmware was built with, for the module's chip: `build.rs` puts their
//!   SHA-256 here (from `cargo xtask dist`'s `kvm-module-esp32c3.bin` and `kvm-module-esp32.bin`), and the header's
//!   chip must be the module's (`espisp::program` checks it against the ROM too). The key that installed this firmware
//!   so vouches for it, and the stream needs no key of its own. The module's first flash sector is written only once the whole file has arrived with that hash
//!   and the CRC-32 its `FwBegin` named; until then it stays erased, so anything else that came is never booted.
//! - **Flow:** the engine's operations arrive in [`STREAM`] ([`pass`]); past [`HIGH`] waiting, the engine answers
//!   `FwData` with Busy and the host retries. Nothing coming for [`STALL`] ends the stream.
//! - **State:** the engine's update state (`Receiving` with the bytes so far, then `Writing`, `Done` or `Failed`) and
//!   the module's programming progress.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, with_timeout};
use heapless::Vec;
use kvm_core::Engine;
use kvm_core::engine::FwOp;
use kvm_proto::crc::Crc32;
use kvm_proto::ctrl::FwState;
use kvm_proto::link::bt::Chip;
use kvm_proto::modimage::{HEAD, HEADER_LEN, Header};
use sha2::{Digest, Sha256};

use crate::espisp::{self, BLOCK, Error};
use crate::kvm;

mod built {
    include!(concat!(env!("OUT_DIR"), "/module_fw.rs"));
}

/// The engine's `FwTarget::Module` operations, in order.
static STREAM: Channel<CriticalSectionRawMutex, FwOp, 64> = Channel::new();
/// An update of the module began: the link lets the module go into its ROM, or a module waiting for one goes on.
pub static WANTED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Waiting operations past which `FwData` is answered with Busy.
const HIGH: usize = 40;
/// How long the stream may stop before it is given up.
const STALL: Duration = Duration::from_secs(10);

/// Whether this firmware was built with a module image (it can update a module).
pub fn available() -> bool {
    built::MODULE_SHA256_ESP32C3.is_some() || built::MODULE_SHA256_ESP32.is_some()
}

/// The SHA-256 of the image this firmware takes for `chip`.
fn wanted_sha(chip: Chip) -> Option<[u8; 32]> {
    match chip {
        Chip::Esp32C3 => built::MODULE_SHA256_ESP32C3,
        Chip::Esp32 => built::MODULE_SHA256_ESP32,
        Chip::Unknown => None,
    }
}

/// Passes an operation of the module's update on (`kvm.rs`, from the engine's outputs, with the engine at hand);
/// false if the queue is full.
pub fn pass(op: FwOp, e: &mut Engine) -> bool {
    if matches!(op, FwOp::Begin { .. }) {
        WANTED.signal(());
    }
    let ok = STREAM.try_send(op).is_ok();
    e.set_fw_busy(STREAM.len() >= HIGH);
    ok
}

/// Ends the update under way as failed, and drops what is left of its stream.
pub fn fail(why: &str) {
    defmt::warn!("module update failed: {}", why);
    crate::uartlog::uart_log!("Bluetooth module update FAILED: {}", why);
    while STREAM.try_receive().is_ok() {}
    kvm::with(|e| {
        e.set_fw_busy(false);
        e.set_fw_state(FwState::Failed, 0);
    });
}

/// The stream of one update, from its `FwBegin`.
pub struct Incoming {
    size: u32,
    crc32: u32,
    crc: Crc32,
    sha: Sha256,
    /// Bytes taken from the stream so far.
    got: u32,
    /// The chunk being taken apart, and how far.
    chunk: Vec<u8, { kvm_proto::ctrl::FW_CHUNK }>,
    at: usize,
    header: Header,
    /// Compressed bytes of the part after the first sector handed out so far.
    tail_done: u32,
    head: Vec<u8, { HEAD as usize }>,
}

impl Incoming {
    /// The stream's `FwBegin` and header: the image must be this firmware's version and fit the file `FwBegin` named.
    pub async fn open() -> Result<Incoming, Error> {
        let (size, crc32) = loop {
            match recv().await? {
                FwOp::Begin { size, crc32, .. } => break (size, crc32),
                // What is left of an earlier stream.
                _ => continue,
            }
        };
        let blank = Header {
            version: 0,
            len: 0,
            head_len: 0,
            head_stored: 0,
            tail_z: 0,
            crc32: 0,
            md5: [0; 32],
            chip: 0,
            base: 0,
        };
        let mut s = Incoming {
            size,
            crc32,
            crc: Crc32::new(),
            sha: Sha256::new(),
            got: 0,
            chunk: Vec::new(),
            at: 0,
            header: blank,
            tail_done: 0,
            head: Vec::new(),
        };
        let mut h = [0u8; HEADER_LEN];
        s.read(&mut h).await?;
        let header = Header::decode(&h).ok_or(Error::Stream)?;
        if header.version != crate::FW_VERSION || header.file_len() != size as usize {
            defmt::warn!("module image {=u32:#x}, {} bytes: not this firmware's", header.version, size);
            return Err(Error::Stream);
        }
        s.header = header;
        Ok(s)
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Fills `out` from the stream.
    async fn read(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let mut filled = 0;
        while filled < out.len() {
            if self.at == self.chunk.len() {
                let FwOp::Data { offset, data } = recv().await? else { return Err(Error::Stream) };
                if offset != self.got || self.got + data.len() as u32 > self.size {
                    defmt::warn!("module image: data out of order");
                    return Err(Error::Stream);
                }
                self.crc.update(&data);
                self.sha.update(&data);
                self.got += data.len() as u32;
                self.chunk = data;
                self.at = 0;
                let got = self.got;
                kvm::with(|e| e.set_fw_state(FwState::Receiving, got));
            }
            let n = (out.len() - filled).min(self.chunk.len() - self.at);
            out[filled..filled + n].copy_from_slice(&self.chunk[self.at..self.at + n]);
            self.at += n;
            filled += n;
        }
        Ok(())
    }
}

impl espisp::Source for Incoming {
    async fn tail(&mut self, out: &mut [u8; BLOCK]) -> Result<usize, Error> {
        let n = ((self.header.tail_z - self.tail_done) as usize).min(BLOCK);
        self.read(&mut out[..n]).await?;
        self.tail_done += n as u32;
        Ok(n)
    }

    async fn head(&mut self) -> Result<&[u8], Error> {
        let n = self.header.head_stored as usize;
        let mut head = [0u8; HEAD as usize];
        self.read(&mut head[..n]).await?;
        self.head = Vec::from_slice(&head[..n]).map_err(|_| Error::Stream)?;
        // All of it, as `FwBegin` named it and as this firmware was built with: only then is the first sector written.
        let sha: [u8; 32] = self.sha.clone().finalize().into();
        let crc = core::mem::replace(&mut self.crc, Crc32::new()).finish();
        let chip = Chip::from_u8(self.header.chip);
        if self.got != self.size || crc != self.crc32 || Some(sha) != wanted_sha(chip) {
            defmt::warn!("module image: not the one this firmware was built with (or damaged)");
            return Err(Error::Stream);
        }
        kvm::with(|e| e.set_fw_state(FwState::Writing, self.size));
        Ok(&self.head)
    }
}

/// The next operation of the stream, within [`STALL`]; the engine may take more again once fewer wait.
async fn recv() -> Result<FwOp, Error> {
    let op = with_timeout(STALL, STREAM.receive()).await.map_err(|_| Error::Stream)?;
    if STREAM.len() < HIGH / 2 {
        kvm::with(|e| e.set_fw_busy(false));
    }
    Ok(op)
}
