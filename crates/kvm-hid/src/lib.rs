//! HID for the OpenDynKVM (design.md §4, software.md §7.1).
//!
//! - [`parser`]: report descriptor → [`Layout`] of typed fields.
//! - [`translate`]: a device's input reports → normalized keyboard / mouse / consumer / system input, plus the verdict
//!   whether everything the device reports fits the normalized interfaces (the Auto policy's main input).
//! - [`neutral`]: "release all" reports for any layout (switching away from a PC).
//! - [`normalized`]: the report descriptors and encoders of the KVM's own, always-present interfaces.
//! - [`xinput`]: XInput pads (wired, and a wireless receiver's), which have no report descriptor.
//! - [`keyboard`], [`mouse`], [`gamepad`]: normalized state types. [`keys`]: key names for UIs. [`layouts`]: what
//!   the keys type on a host set to a keyboard layout.
#![cfg_attr(not(test), no_std)]

pub mod gamepad;
pub mod keyboard;
pub mod keys;
pub mod layouts;
pub mod mouse;
pub mod neutral;
pub mod normalized;
pub mod parser;
pub mod translate;
pub mod usage;
pub mod xinput;

pub use gamepad::GamepadInput;
pub use keyboard::KeyState;
pub use mouse::MouseInput;
pub use parser::{Field, Layout, ParseError, ReportType};
pub use translate::{Translation, Translator};
