//! HID for the OpenDynKVM (design.md §4, software.md §7.1).
//!
//! - [`parser`]: report descriptor → [`Layout`] of typed fields.
//! - [`translate`]: a device's input reports → normalized keyboard / mouse / consumer / system input, plus the verdict
//!   whether everything the device reports fits the normalized interfaces (the Auto policy's main input).
//! - [`neutral`]: "release all" reports for any layout (switching away from a PC).
//! - [`normalized`]: the report descriptors and encoders of the KVM's own, always-present interfaces.
//! - [`keyboard`], [`mouse`]: normalized state types. [`keys`]: key names for UIs.
#![cfg_attr(not(test), no_std)]

pub mod keyboard;
pub mod keys;
pub mod mouse;
pub mod neutral;
pub mod normalized;
pub mod parser;
pub mod translate;
pub mod usage;

pub use keyboard::KeyState;
pub use mouse::MouseInput;
pub use parser::{Field, Layout, ParseError, ReportType};
pub use translate::{Translation, Translator};
