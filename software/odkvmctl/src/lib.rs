//! Host side of the OpenDynKVM control protocol (software.md §8–§9), shared by the `odkvmctl` CLI and `opendyn-kvm`.

pub mod autostart;
pub mod bootsel;
pub mod client;
pub mod ddc;
pub mod firmware;
pub mod i18n;
pub mod icon;
pub mod image;
pub mod local;
pub mod names;
pub mod screens;
pub mod sim;
pub mod transport;

pub use client::{Client, Error};
