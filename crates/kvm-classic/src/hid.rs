//! HID over L2CAP's transaction headers (Bluetooth HID Profile 1.1, §3.1): the first byte of every message on the
//! control and interrupt channels.

/// DATA, input report (device → host, on the interrupt channel).
pub const DATA_INPUT: u8 = 0xA1;
/// DATA, output report (host → device, on the interrupt channel).
pub const DATA_OUTPUT: u8 = 0xA2;
/// DATA, feature report (the answer to a GET_REPORT of one, on the control channel).
pub const DATA_FEATURE: u8 = 0xA3;
/// GET_REPORT of a feature report (its ID follows), on the control channel.
pub const GET_FEATURE: u8 = 0x43;
/// HANDSHAKE (the low nibble its result: 0 successful), on the control channel.
pub const HANDSHAKE: u8 = 0x00;

/// What a message on the interrupt channel carries: an input report (its ID first).
pub fn input(msg: &[u8]) -> Option<&[u8]> {
    (msg.first() == Some(&DATA_INPUT)).then(|| &msg[1..])
}
