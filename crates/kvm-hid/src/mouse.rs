//! Normalized mouse input.

/// Buttons are a state; the axes are deltas since the previous report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MouseInput {
    /// Bit *n* = button *n + 1* (Button page usage).
    pub buttons: u16,
    pub x: i32,
    pub y: i32,
    /// Detents (or hi-res units of the source, see `normalized::WHEEL_MULTIPLIER`).
    pub wheel: i32,
    pub pan: i32,
}

impl MouseInput {
    pub fn has_motion(&self) -> bool {
        self.x != 0 || self.y != 0 || self.wheel != 0 || self.pan != 0
    }

    pub fn released(&self) -> MouseInput {
        MouseInput::default()
    }
}

pub fn clamp_i16(v: i32) -> i16 {
    v.clamp(-32767, 32767) as i16
}

pub fn clamp_i8(v: i32) -> i8 {
    v.clamp(-127, 127) as i8
}
