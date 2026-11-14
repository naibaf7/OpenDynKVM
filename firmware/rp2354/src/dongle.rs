//! The Bluetooth module as an ESP32-C3 board on a front port (software.md §4.13): the chip's built-in USB serial port
//! (USB-Serial-JTAG, a CDC-ACM port in silicon) as the module's line, for the link and for its ROM bootloader.
//!
//! - **Data:** the CDC data interface's bulk endpoints, polled by interrupt slots every millisecond
//!   ([`crate::usbh`]): the controller polls them by itself, EPX stays for control transfers, and a reader that gives up
//!   leaves the slot armed for the next read (nothing is lost).
//! - **Reset and boot:** the chip's DTR/RTS lines, set with SET_CONTROL_LINE_STATE, restart it (RTS) and choose its ROM
//!   bootloader for that restart (DTR), as espflash 4.7 does it (`connection/reset.rs`: `UsbJtagSerialReset`,
//!   `reset_after_flash`). That works on any firmware and on a blank chip: no EnterRom, no strap wiring.

use core::sync::atomic::Ordering;

use embassy_time::{Duration, Instant, Timer, with_timeout};
use embassy_usb_driver::host::{PipeError, UsbPipe, pipe};
use heapless::String;
use kvm_proto::ctrl::Port;

use crate::espisp::Line;
use crate::usbh::{self, RpPipe};

type Ctl = RpPipe<pipe::Control, pipe::InOut>;
pub type DataIn = RpPipe<pipe::Interrupt, pipe::In>;
pub type DataOut = RpPipe<pipe::Interrupt, pipe::Out>;
/// How long a write waits for the chip to take a packet, unless it is being programmed.
const PATIENCE: Duration = Duration::from_millis(20);
/// CDC SET_CONTROL_LINE_STATE: DTR bit 0, RTS bit 1.
const SET_CONTROL_LINE_STATE: u8 = 0x22;

/// A front port's ESP32-C3 board, handed from its port task to the module's (`bt::dongle_task`).
pub struct Dongle {
    pub port: Port,
    pub line: UsbLine,
    /// The chip's USB serial number (its MAC address): the module's again after a restart.
    pub serial: String<24>,
}

pub struct UsbLine {
    ctl: Ctl,
    rx: DataIn,
    tx: DataOut,
    comm: u8,
    /// How long a write waits for the chip to take a packet: short while the KVM finds out what it is (a chip whose
    /// ROM finds no firmware reads nothing, and restarts every 2.5 s or so), long while its ROM writes the flash.
    patience: Duration,
    pkt: [u8; 64],
    pos: usize,
    len: usize,
}

impl UsbLine {
    pub fn new(ctl: Ctl, rx: DataIn, tx: DataOut, comm: u8) -> Self {
        UsbLine { ctl, rx, tx, comm, patience: PATIENCE, pkt: [0; 64], pos: 0, len: 0 }
    }

    /// Writes wait up to 1 s for the chip (its ROM writing the flash), or [`PATIENCE`] again.
    pub fn programming(&mut self, on: bool) {
        self.patience = if on { Duration::from_secs(1) } else { PATIENCE };
    }

    fn take(&mut self, buf: &mut [u8]) -> usize {
        let n = (self.len - self.pos).min(buf.len());
        buf[..n].copy_from_slice(&self.pkt[self.pos..self.pos + n]);
        self.pos += n;
        n
    }

    async fn lines(&mut self, dtr: bool, rts: bool) -> Result<(), PipeError> {
        let value = dtr as u16 | (rts as u16) << 1;
        let [v0, v1] = value.to_le_bytes();
        let setup = [0x21, SET_CONTROL_LINE_STATE, v0, v1, self.comm, 0, 0, 0];
        let mut tries = 0;
        loop {
            match self.ctl.control_out(&setup, &[]).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    crate::usbh::LAST_ERROR.store(crate::usbh::error_code(3, e), core::sync::atomic::Ordering::Relaxed);
                    tries += 1;
                    if tries == 3 || e == PipeError::Disconnected {
                        return Err(e);
                    }
                    Timer::after_millis(2).await;
                }
            }
        }
    }

    /// Restarts the chip into its ROM bootloader: DTR up, then RTS through (1,1) to (0,1), then both down (espflash's
    /// `UsbJtagSerialReset`). Data that was waiting is dropped.
    pub async fn restart_into_rom(&mut self) -> Result<(), PipeError> {
        self.lines(false, false).await?;
        Timer::after_millis(100).await;
        self.lines(true, false).await?;
        Timer::after_millis(100).await;
        self.lines(true, true).await?;
        self.lines(false, true).await?;
        Timer::after_millis(100).await;
        self.lines(false, false).await?;
        self.pos = 0;
        self.len = 0;
        // The ROM's start.
        Timer::after_millis(100).await;
        Ok(())
    }

    /// Restarts the chip into its firmware: RTS with DTR down (espflash's `reset_after_flash` for this port).
    pub async fn reset(&mut self) -> Result<(), PipeError> {
        self.lines(false, false).await?;
        Timer::after_millis(100).await;
        self.lines(false, true).await?;
        Timer::after_millis(100).await;
        self.lines(false, false).await?;
        self.pos = 0;
        self.len = 0;
        Ok(())
    }
}

impl embedded_io_async::ErrorType for UsbLine {
    type Error = embedded_io_async::ErrorKind;
}

fn io_error(e: PipeError) -> embedded_io_async::ErrorKind {
    match e {
        PipeError::Disconnected => embedded_io_async::ErrorKind::NotConnected,
        PipeError::Timeout => embedded_io_async::ErrorKind::TimedOut,
        _ => embedded_io_async::ErrorKind::Other,
    }
}

impl embedded_io_async::Read for UsbLine {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            match self.read_within(buf, Duration::from_secs(3600)).await {
                Ok(0) => {}
                Ok(n) => return Ok(n),
                Err(()) => return Err(embedded_io_async::ErrorKind::NotConnected),
            }
        }
    }
}

impl embedded_io_async::Write for UsbLine {
    /// One packet's worth (up to the endpoint's size) per call; `write_all` sends the rest.
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let n = buf.len().min(self.tx.max_packet());
        match self.tx.out_within(&buf[..n], self.patience).await {
            Ok(true) => {
                usbh::BULK_OUT.fetch_add(1, Ordering::Relaxed);
                Ok(n)
            }
            // Not taken in time (a chip whose ROM reads nothing).
            Ok(false) => {
                usbh::BULK_OUT_NAKS.fetch_add(1, Ordering::Relaxed);
                Err(embedded_io_async::ErrorKind::TimedOut)
            }
            Err(e) => {
                usbh::BULK_ERRORS.fetch_add(1, Ordering::Relaxed);
                usbh::LAST_ERROR.store(usbh::error_code(2, e), Ordering::Relaxed);
                Err(io_error(e))
            }
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Line for UsbLine {
    const UART: bool = false;

    fn set_baud(&mut self, _baud: u32) {}

    async fn read_within(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, ()> {
        let deadline = Instant::now() + timeout;
        loop {
            if self.pos < self.len {
                return Ok(self.take(buf));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            // A read that gives up leaves the slot armed: what comes later is there for the next read.
            match with_timeout(left, self.rx.request_in(&mut self.pkt)).await {
                Ok(Ok(n)) => {
                    usbh::BULK_IN.fetch_add(1, Ordering::Relaxed);
                    self.pos = 0;
                    self.len = n.min(self.pkt.len());
                }
                Ok(Err(PipeError::Disconnected)) => return Err(()),
                Ok(Err(e)) => {
                    usbh::BULK_ERRORS.fetch_add(1, Ordering::Relaxed);
                    usbh::LAST_ERROR.store(usbh::error_code(1, e), Ordering::Relaxed);
                    if Instant::now() >= deadline {
                        return Ok(0);
                    }
                    Timer::after_millis(1).await;
                }
                Err(_) => return Ok(0),
            }
        }
    }
}
