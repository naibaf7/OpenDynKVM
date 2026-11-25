//! Normalized gamepad input: buttons, a hat and the axes, by HID usage (hid_mapping.md §4).

/// Axes by usage: the generic desktop's X–Dial, then the simulation page's brake and accelerator (triggers).
pub const AXES: usize = 10;

pub mod axis {
    pub const X: usize = 0;
    pub const Y: usize = 1;
    pub const Z: usize = 2;
    pub const RX: usize = 3;
    pub const RY: usize = 4;
    pub const RZ: usize = 5;
    pub const SLIDER: usize = 6;
    pub const DIAL: usize = 7;
    pub const BRAKE: usize = 8;
    pub const ACCELERATOR: usize = 9;

    /// Whether axis `a` has no centre: its value runs from 0 up.
    pub const fn is_trigger(a: usize) -> bool {
        a == BRAKE || a == ACCELERATOR
    }
}

/// The hat at rest.
pub const HAT_CENTRE: u8 = 8;
/// The span of a normalized axis: centred ones −FULL…FULL, triggers 0…FULL.
pub const FULL: i32 = 32767;

/// What a gamepad reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct GamepadInput {
    /// Buttons 1–32: bit n = button n + 1.
    pub buttons: u32,
    /// 0–7 clockwise from up, or [`HAT_CENTRE`].
    pub hat: u8,
    pub axes: [i32; AXES],
    /// Bit per axis the device has.
    pub present: u16,
}

impl Default for GamepadInput {
    fn default() -> Self {
        GamepadInput { buttons: 0, hat: HAT_CENTRE, axes: [0; AXES], present: 0 }
    }
}

impl GamepadInput {
    /// The hat as the four d-pad directions: bits up, right, down, left.
    pub fn dpad(&self) -> u8 {
        const DIRS: [u8; 8] = [0b0001, 0b0011, 0b0010, 0b0110, 0b0100, 0b1100, 0b1000, 0b1001];
        DIRS.get(self.hat as usize).copied().unwrap_or(0)
    }

    /// The hat for d-pad bits (up, right, down, left); opposite directions cancel.
    pub fn hat_of(dpad: u8) -> u8 {
        let axis = |minus: u8, plus: u8| i32::from(dpad & plus != 0) - i32::from(dpad & minus != 0);
        match (axis(8, 2), axis(1, 4)) {
            (0, -1) => 0,
            (1, -1) => 1,
            (1, 0) => 2,
            (1, 1) => 3,
            (0, 1) => 4,
            (-1, 1) => 5,
            (-1, 0) => 6,
            (-1, -1) => 7,
            _ => HAT_CENTRE,
        }
    }
}

/// A raw value in `min…max` as a normalized axis value.
pub fn normalize(v: i32, min: i32, max: i32, trigger: bool) -> i32 {
    if max <= min {
        return 0;
    }
    let (v, min, max) = (v.clamp(min, max) as i64, min as i64, max as i64);
    if trigger {
        ((v - min) * FULL as i64 / (max - min)) as i32
    } else {
        ((v - min) * (2 * FULL as i64) / (max - min) - FULL as i64) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn axes_scale_to_the_full_span() {
        assert_eq!(normalize(0, 0, 255, false), -FULL);
        assert_eq!(normalize(255, 0, 255, false), FULL);
        assert_eq!(normalize(-32768, -32768, 32767, false), -FULL);
        assert_eq!(normalize(512, 0, 1023, true), 16399);
        assert_eq!(normalize(5, 5, 5, false), 0);
    }

    #[test]
    fn the_hat_and_the_dpad() {
        for hat in 0..=8 {
            let g = GamepadInput { hat, ..GamepadInput::default() };
            assert_eq!(GamepadInput::hat_of(g.dpad()), hat);
        }
        assert_eq!(GamepadInput::hat_of(0b0101), HAT_CENTRE, "up and down cancel");
    }
}
