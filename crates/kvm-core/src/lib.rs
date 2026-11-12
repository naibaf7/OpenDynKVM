//! The OpenDynKVM's logic, free of I/O so that it runs (and is tested) on the PC as well as on the RP2354B.
//!
//! [`engine::Engine`] is the entry point; the other modules are its parts.
#![cfg_attr(not(test), no_std)]

pub mod at32prog;
pub mod bt;
pub mod config;
pub mod diag;
pub mod engine;
pub mod hop;
pub mod hotkey;
pub mod leds;
pub mod merge;
pub mod output;
pub mod policy;
pub mod presentation;
pub mod standby;
pub mod store;
pub mod usbdesc;

pub use engine::{Engine, KeyEvent, Output, Probed, Route};
