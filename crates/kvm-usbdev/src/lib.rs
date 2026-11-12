//! The AT32F405's USB device core (software.md §5.2): presents whatever descriptor set the RP2354B sent.
//!
//! - [`requests`]: the control-request dispatcher, a pure function of (setup packet, descriptor set, state).
//! - [`device`]: the async loop that runs it over any `embassy-usb-driver` implementation, plus endpoint allocation.
//!
//! The firmware supplies a [`device::Host`]: link to the RP2354B, soft connect, speed.
#![cfg_attr(not(test), no_std)]
#![allow(async_fn_in_trait)]

pub mod device;
pub mod requests;

pub use device::{Cmd, Endpoints, Host, UsbCore};
pub use requests::{Action, DeviceState, Setup};
