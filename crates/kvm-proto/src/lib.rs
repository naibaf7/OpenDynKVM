//! Wire formats shared by both firmwares and the host software (software.md §6–§8).
//!
//! - [`link`]: the 72-byte SPI frame between the RP2354B and each AT32F405, its go-back-N ARQ and messages.
//! - [`descset`]: the descriptor set the RP2354B sends to an AT32F405 (what a PC enumerates).
//! - [`ctrl`]: the control protocol spoken over the vendor HID interface by `odkvmctl`.
//! - [`rpimage`]: the RP2350 image metadata an RP2354B update image must carry.
#![cfg_attr(not(test), no_std)]

pub mod crc;
pub mod ctrl;
pub mod descset;
pub mod link;
pub mod rpimage;
pub mod wire;

/// A version as carried in `Hello` and `Info`: `major << 16 | minor << 8 | patch`.
pub const fn version_u32(major: u8, minor: u8, patch: u8) -> u32 {
    (major as u32) << 16 | (minor as u32) << 8 | patch as u32
}

/// The firmware's version, the same on the RP2354B and both AT32F405s (DEVELOPMENT.md "Versions"): the host software's version
/// of the release that last changed the firmware. It is never ahead of the host software and doesn't move with every
/// host release.
pub const FIRMWARE_VERSION: (u8, u8, u8) = (0, 1, 19);

/// The board the firmware is built for (DEVELOPMENT.md "Versions"). A new major version is a different board with its own
/// firmware image; minor versions are revisions of one board, which one image serves. The board carries no ID of its
/// own yet, so this is the version `Info` reports.
pub const HARDWARE_VERSION: (u8, u8, u8) = (0, 12, 0);

#[cfg(test)]
mod tests {
    #[test]
    fn the_firmware_is_never_ahead_of_the_host_software() {
        // This crate's version is the workspace's, the host software's.
        let host: std::vec::Vec<u8> = env!("CARGO_PKG_VERSION").split('.').map(|p| p.parse().unwrap()).collect();
        let (a, b, c) = super::FIRMWARE_VERSION;
        assert!([a, b, c] <= [host[0], host[1], host[2]], "firmware {a}.{b}.{c}, host {host:?}");
    }
}
