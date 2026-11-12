//! RP2354B ↔ AT32F405 links over SPI (software.md §6), and the Bluetooth coprocessor link over UART ([`bt`]).
//!
//! Every SPI transaction exchanges one [`frame`] in each direction. [`arq::Arq`] turns that lockstep exchange into an
//! exactly-once, in-order message stream; [`msg`] defines the messages.

pub mod arq;
pub mod bt;
pub mod frame;
pub mod msg;

pub use arq::{Arq, Msg, Prio};
pub use bt::BtMsg;
pub use frame::{FRAME_LEN, Frame, FrameError, MAX_MSG};
pub use msg::LinkMsg;

/// Link protocol version carried in `Hello`.
pub const PROTOCOL_VERSION: u8 = 1;
