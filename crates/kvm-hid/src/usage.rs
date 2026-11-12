//! Usage pages and usages (HID Usage Tables 1.6). A full usage is `page << 16 | id`.

pub const fn usage(page: u16, id: u16) -> u32 {
    (page as u32) << 16 | id as u32
}

pub const fn page_of(usage: u32) -> u16 {
    (usage >> 16) as u16
}

pub const fn id_of(usage: u32) -> u16 {
    usage as u16
}

pub mod page {
    pub const GENERIC_DESKTOP: u16 = 0x01;
    pub const SIMULATION: u16 = 0x02;
    pub const GAME: u16 = 0x05;
    pub const GENERIC_DEVICE: u16 = 0x06;
    pub const KEYBOARD: u16 = 0x07;
    pub const LED: u16 = 0x08;
    pub const BUTTON: u16 = 0x09;
    pub const ORDINAL: u16 = 0x0A;
    pub const CONSUMER: u16 = 0x0C;
    pub const DIGITIZER: u16 = 0x0D;
    pub const VENDOR_MIN: u16 = 0xFF00;

    pub const fn is_vendor(page: u16) -> bool {
        page >= VENDOR_MIN
    }
}

/// Generic Desktop page.
pub mod gd {
    pub const POINTER: u16 = 0x01;
    pub const MOUSE: u16 = 0x02;
    pub const JOYSTICK: u16 = 0x04;
    pub const GAMEPAD: u16 = 0x05;
    pub const KEYBOARD: u16 = 0x06;
    pub const KEYPAD: u16 = 0x07;
    pub const MULTI_AXIS: u16 = 0x08;
    pub const X: u16 = 0x30;
    pub const Y: u16 = 0x31;
    pub const Z: u16 = 0x32;
    pub const RX: u16 = 0x33;
    pub const RY: u16 = 0x34;
    pub const RZ: u16 = 0x35;
    pub const SLIDER: u16 = 0x36;
    pub const DIAL: u16 = 0x37;
    pub const WHEEL: u16 = 0x38;
    pub const HAT_SWITCH: u16 = 0x39;
    pub const RESOLUTION_MULTIPLIER: u16 = 0x48;
    pub const SYSTEM_CONTROL: u16 = 0x80;
    pub const SYSTEM_POWER_DOWN: u16 = 0x81;
    pub const SYSTEM_SLEEP: u16 = 0x82;
    pub const SYSTEM_WAKE_UP: u16 = 0x83;
}

/// Consumer page.
pub mod consumer {
    pub const CONSUMER_CONTROL: u16 = 0x01;
    pub const AC_PAN: u16 = 0x238;
}

/// Keyboard page.
pub mod key {
    pub const ERROR_ROLL_OVER: u8 = 0x01;
    pub const CAPS_LOCK: u8 = 0x39;
    pub const SCROLL_LOCK: u8 = 0x47;
    pub const NUM_LOCK: u8 = 0x53;
    pub const LEFT_CTRL: u8 = 0xE0;
    pub const RIGHT_GUI: u8 = 0xE7;
}

/// Is this Generic Desktop usage an axis whose rest position is its centre when absolute?
pub const fn is_centred_axis(usage: u32) -> bool {
    page_of(usage) == page::GENERIC_DESKTOP
        && matches!(id_of(usage), gd::X | gd::Y | gd::Z | gd::RX | gd::RY | gd::RZ | gd::SLIDER | gd::DIAL | gd::WHEEL)
}
