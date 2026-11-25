//! Bluetooth Classic (BR/EDR) for the ESP32 module's gamepad host (`firmware/esp32`, software.md §4.13): what doesn't
//! need the radio, so that it is tested here.
//!
//! - [`hci`]: the Classic HCI events the host reads, and which of the controller's events and data are Classic's
//!   ([`hci::route_event`]): the LE stack shares the controller and never sees them.
//! - [`l2cap`]: L2CAP's signaling channel for the two HID channels (control and interrupt), whichever side opens them,
//!   and SDP's.
//! - [`sdp`]: SDP's server side, which a pad that reconnects asks before anything else.
//! - [`hid`]: HID's transaction headers on those channels.
//! - [`pad`]: the 5 series of the symmetric-stick pads (vendor 0x054C) over Bluetooth: its full input report put in
//!   the USB form of its report 1, which [`pad::DESCRIPTOR`] declares and `kvm_core::pads` knows, and its output
//!   report (the player light).

#![no_std]

#[cfg(test)]
extern crate std;

pub mod hci;
pub mod hid;
pub mod l2cap;
pub mod pad;
pub mod sdp;
