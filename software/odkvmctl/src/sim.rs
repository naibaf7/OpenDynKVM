//! A simulated KVM: the firmware's own `kvm_core::Engine`, on a 0.19.0 board with a fake device on each of its four
//! front ports, and both PCs connected. This computer plays PC1. With `OPENDYN_SIM_BOARD=0.12` it is a board before
//! 0.19.0: three front ports, no Aux 1.
//!
//! The mouse streams 1 kHz reports through the engine (so report rates and normalization are real); re-detection,
//! firmware updates and descriptor-set changes are acted out with plausible delays. Someone presses the KVM's key when
//! it waits for it. Its keyboard types what is sent to [`SimTransport::simulated_keyboard`]: the GUI sends the keys
//! pressed in its window while a hotkey is learned.
//!
//! With `OPENDYN_SIM_BT=1` the board has the Bluetooth module: a tablet in slot 1, connected, and a phone
//! in slot 2, bonded and away (connect-when-selected). Pairing finds a "Laptop" that asks for a passkey (any six digits
//! do), selecting the phone connects it after a moment. With `OPENDYN_SIM_BT=needs-update` the module is fitted, but
//! runs another firmware; streaming it the module's (`update --module`, the GUI's *Update the wireless module*) acts
//! out programming it, and then it is up as above. With `OPENDYN_SIM_BT=dongle` the board has no module, and an ESP32-C3 board on the Aux 2 port is
//! it (software.md §4.13), with the same hosts. Without any of these there is no module, as on most boards.
//!
//! With `OPENDYN_SIM_HOP=1` screen hop is set up and on in the first profile: the absolute pointer, PC1 with a
//! 2560×1440 main screen and a 1920×1080 one left of it, PC2 with a 3840×2160 screen at 150 % to the right. That
//! monitor is cabled to PC1 too: PC1's is off for hop and lies on PC2's.

use crate::time::{Duration, Instant};
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};

use heapless::{String as HString, Vec as HVec};
use kvm_core::bt::BtOut;
use kvm_core::config::Config;
use kvm_core::diag::BoardDiag;
use kvm_core::engine::{Asker, Engine, FwOp, KeyEvent, Output, PowerInfo, Probed};
use kvm_core::usbdesc::DeviceDesc;
use kvm_proto::ctrl::DevicePairing;
use kvm_proto::ctrl::diag::BtModule;
use kvm_proto::ctrl::{
    Advertised, DeviceMsg, ErrorCode, FwState, FwTarget, Info, Pc, Port, PowerSource, REPORT_LEN, Response, Speed, caps,
};
use kvm_proto::link::bt::{
    DeviceState, EventKind as BtEvent, HostState, MAP_CHUNK, appearance, device_flags, host_flags,
};
use kvm_proto::link::msg::{Status, UsbSpeed, UsbState};
use kvm_proto::version_u32;

use crate::transport::{Link, Report, Transport};

/// The simulated KVM runs the current firmware on the current board.
pub const FW_VERSION: u32 = {
    let (major, minor, patch) = kvm_proto::FIRMWARE_VERSION;
    version_u32(major, minor, patch)
};

/// The simulated board has the Aux 1 port (0.19.0, the default), or is one before it (`OPENDYN_SIM_BOARD=0.12`).
fn sim_aux1() -> bool {
    crate::env_var("OPENDYN_SIM_BOARD").as_deref() != Some("0.12")
}

#[rustfmt::skip]
const BOOT_KEYBOARD: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01,
    0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01,
    0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x25, 0x65,
    0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x81, 0x00, 0xC0,
];

#[rustfmt::skip]
const MEDIA_KEYS: &[u8] = &[
    0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x03, 0x75, 0x10, 0x95, 0x02, 0x15, 0x01, 0x26, 0xFF, 0x02, 0x19,
    0x01, 0x2A, 0xFF, 0x02, 0x81, 0x00, 0xC0,
];

/// 16 buttons, 16-bit X/Y, wheel, AC Pan; report ID 2.
#[rustfmt::skip]
const GAMING_MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x02, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x10,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x10, 0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x16, 0x01, 0x80, 0x26, 0xFF, 0x7F,
    0x75, 0x10, 0x95, 0x02, 0x09, 0x30, 0x09, 0x31, 0x81, 0x06, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01,
    0x09, 0x38, 0x81, 0x06, 0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06, 0xC0, 0xC0,
];

/// A vendor command channel in the style of HID++ (report ID 0x10 on a vendor page).
#[rustfmt::skip]
const HIDPP: &[u8] = &[
    0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x85, 0x10, 0x75, 0x08, 0x95, 0x06, 0x15, 0x00, 0x26, 0xFF, 0x00,
    0x09, 0x01, 0x81, 0x00, 0x09, 0x01, 0x91, 0x00, 0xC0,
];

/// One interface of a fake configuration: class triple, HID report length, endpoints (address, size, interval).
struct Iface<'a> {
    class: (u8, u8, u8),
    report_len: Option<usize>,
    eps: &'a [(u8, u16, u8)],
    alt_with_iso: bool,
}

fn config_descriptor(ifaces: &[Iface], max_power_ma: u16) -> HVec<u8, { kvm_core::engine::MAX_CONFIG_DESC }> {
    let mut c: Vec<u8> = vec![9, 2, 0, 0, ifaces.len() as u8, 1, 0, 0xA0, (max_power_ma / 2) as u8];
    for (n, i) in ifaces.iter().enumerate() {
        c.extend([9, 4, n as u8, 0, i.eps.len() as u8, i.class.0, i.class.1, i.class.2, 0]);
        if let Some(len) = i.report_len {
            c.extend([9, 0x21, 0x11, 0x01, 0, 1, 0x22, len as u8, (len >> 8) as u8]);
        }
        for (addr, mps, interval) in i.eps {
            c.extend([7, 5, *addr, 3, *mps as u8, (*mps >> 8) as u8, *interval]);
        }
        if i.alt_with_iso {
            c.extend([9, 4, n as u8, 1, 1, i.class.0, i.class.1, i.class.2, 0]);
            c.extend([7, 5, 0x81 + n as u8, 5, 0x00, 0x14, 1]);
        }
    }
    let total = c.len() as u16;
    c[2..4].copy_from_slice(&total.to_le_bytes());
    HVec::from_slice(&c).unwrap_or_default()
}

fn hs<const N: usize>(s: &str) -> HString<N> {
    HString::try_from(s).unwrap_or_default()
}

fn probed(
    vid: u16,
    pid: u16,
    manufacturer: &str,
    product: &str,
    config: HVec<u8, 512>,
    reports: &[(u8, &[u8])],
) -> Probed {
    let mut report_descs = HVec::new();
    for (iface, rd) in reports {
        let _ = report_descs.push((*iface, HVec::from_slice(rd).unwrap_or_default()));
    }
    Probed {
        device: DeviceDesc {
            usb: 0x0200,
            max_packet0: 64,
            vid,
            pid,
            bcd: 0x0100,
            configurations: 1,
            ..Default::default()
        },
        speed: Some(Speed::Full),
        high_speed: false,
        config,
        report_descs,
        manufacturer: hs(manufacturer),
        product: hs(product),
        serial: hs(""),
    }
}

fn device_for(port: Port) -> Probed {
    match port {
        Port::Keyboard => probed(
            0x04D9,
            0x0024,
            "Simulated",
            "Mechanical keyboard",
            config_descriptor(
                &[
                    Iface {
                        class: (3, 1, 1),
                        report_len: Some(BOOT_KEYBOARD.len()),
                        eps: &[(0x81, 8, 1)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (3, 0, 0),
                        report_len: Some(MEDIA_KEYS.len()),
                        eps: &[(0x82, 8, 1)],
                        alt_with_iso: false,
                    },
                ],
                200,
            ),
            &[(0, BOOT_KEYBOARD), (1, MEDIA_KEYS)],
        ),
        Port::Mouse => probed(
            0x046D,
            0xC08B,
            "Simulated",
            "Gaming mouse (simulated)",
            config_descriptor(
                &[
                    Iface {
                        class: (3, 1, 2),
                        report_len: Some(GAMING_MOUSE.len()),
                        eps: &[(0x81, 8, 1)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (3, 0, 0),
                        report_len: Some(HIDPP.len()),
                        eps: &[(0x82, 20, 1)],
                        alt_with_iso: false,
                    },
                ],
                98,
            ),
            &[(0, GAMING_MOUSE), (1, HIDPP)],
        ),
        // A media dial: consumer keys only, mirrored there as on Aux 2.
        Port::Aux1 => probed(
            0x1209,
            0x0002,
            "Simulated",
            "Media dial (simulated)",
            config_descriptor(
                &[Iface {
                    class: (3, 0, 0),
                    report_len: Some(MEDIA_KEYS.len()),
                    eps: &[(0x81, 8, 2)],
                    alt_with_iso: false,
                }],
                50,
            ),
            &[(0, MEDIA_KEYS)],
        ),
        Port::Aux2 if sim_pad() == Some(SimPad::Receiver) => probed(
            0x045E,
            0x0719,
            "Simulated",
            "Wireless receiver (simulated)",
            // Four pads' interfaces and their headsets', interleaved.
            config_descriptor(
                &[
                    Iface {
                        class: (0xFF, 0x5D, 0x81),
                        report_len: None,
                        eps: &[(0x81, 32, 1), (0x01, 32, 8)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x82),
                        report_len: None,
                        eps: &[(0x82, 32, 2), (0x02, 32, 4)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x81),
                        report_len: None,
                        eps: &[(0x83, 32, 1), (0x03, 32, 8)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x82),
                        report_len: None,
                        eps: &[(0x84, 32, 2), (0x04, 32, 4)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x81),
                        report_len: None,
                        eps: &[(0x85, 32, 1), (0x05, 32, 8)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x82),
                        report_len: None,
                        eps: &[(0x86, 32, 2), (0x06, 32, 4)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x81),
                        report_len: None,
                        eps: &[(0x87, 32, 1), (0x07, 32, 8)],
                        alt_with_iso: false,
                    },
                    Iface {
                        class: (0xFF, 0x5D, 0x82),
                        report_len: None,
                        eps: &[(0x88, 32, 2), (0x08, 32, 4)],
                        alt_with_iso: false,
                    },
                ],
                260,
            ),
            &[],
        ),
        Port::Aux2 if sim_pad() == Some(SimPad::Symmetric) => probed(
            0x054C,
            0x0CE6,
            "Simulated",
            "Gamepad with touch pad (simulated)",
            config_descriptor(
                &[Iface {
                    class: (3, 0, 0),
                    report_len: Some(SYMMETRIC_PAD.len()),
                    eps: &[(0x84, 64, 4), (0x03, 64, 4)],
                    alt_with_iso: false,
                }],
                500,
            ),
            &[(0, SYMMETRIC_PAD)],
        ),
        Port::Aux2 if sim_pad().is_some() => probed(
            0x0F0D,
            0x00C1,
            "Simulated",
            "Gamepad (simulated)",
            config_descriptor(
                &[Iface {
                    class: (3, 0, 0),
                    report_len: Some(GAMEPAD.len()),
                    eps: &[(0x81, 8, 1)],
                    alt_with_iso: false,
                }],
                100,
            ),
            &[(0, GAMEPAD)],
        ),
        // A high-speed device: it answers the device qualifier on Hub-0.
        Port::Aux2 => Probed {
            high_speed: true,
            ..probed(
                0x046D,
                0x085E,
                "Simulated",
                "Webcam",
                config_descriptor(
                    &[
                        Iface { class: (0x0E, 1, 0), report_len: None, eps: &[(0x83, 16, 8)], alt_with_iso: false },
                        Iface { class: (0x0E, 2, 0), report_len: None, eps: &[], alt_with_iso: true },
                        Iface { class: (0x01, 1, 0), report_len: None, eps: &[], alt_with_iso: false },
                    ],
                    500,
                ),
                &[],
            )
        },
    }
}

fn pc_status(cc_mv: u16, vbus_mv: u16, state: UsbState) -> Status {
    Status {
        usb_state: state,
        speed: UsbSpeed::High,
        vbus_mv,
        cc1_mv: cc_mv,
        cc2_mv: 0,
        frame: 0,
        resets: 1,
        desc_crc: 0,
        // As Windows and Linux set it for a keyboard: a key press may wake a sleeping PC.
        wake_enabled: true,
        vdda_mv: Some(3290),
        temp_dc: Some(380),
        vbus_min_mv: vbus_mv,
    }
}

pub struct SimTransport {
    /// On the heap: a fifth of a megabyte, which copied about on the stack overflowed a browser's 1 MB one.
    engine: Box<Engine>,
    start: Instant,
    last_ms: u32,
    inbox: VecDeque<Report>,
    /// (when, what) for delayed effects.
    timeline: Vec<(u32, Delayed)>,
    /// What the keyboard types.
    keys: (Sender<[u8; 8]>, Receiver<[u8; 8]>),
    /// The simulated Bluetooth module's host slots, when it is fitted (`OPENDYN_SIM_BT`).
    bt: Option<[HostState; 4]>,
    /// The module is an ESP32-C3 board on the Aux 2 port (`OPENDYN_SIM_BT=dongle`).
    dongle: bool,
    /// The protocol `GetInfo` says, if not this program's ([`SimTransport::speaking`]).
    proto: Option<u8>,
    /// The simulated module's Bluetooth device slots.
    devices: [Option<SimDevice>; 4],
    /// The module's firmware being streamed (`FwTarget::Module`): its size.
    module_stream: Option<u32>,
    /// Who this program is to the simulated KVM: PC1, or wireless host 1 over the module's control service
    /// (`OPENDYN_SIM_BT=wireless`).
    asker: Asker,
}

/// An ESP32-C3 board's USB serial port (303A:1001: IAD, CDC-ACM, CDC data with bulk endpoints, the JTAG interface).
fn esp_board() -> Probed {
    #[rustfmt::skip]
    const CONFIG: &[u8] = &[
        9, 2, 98, 0, 3, 1, 0, 0xC0, 250,
        8, 0x0B, 0, 2, 0x02, 0x02, 0, 0,
        9, 4, 0, 0, 1, 0x02, 0x02, 0, 0,
        5, 0x24, 0x00, 0x10, 0x01,
        5, 0x24, 0x01, 0x03, 0x01,
        4, 0x24, 0x02, 0x06,
        5, 0x24, 0x06, 0x00, 0x01,
        7, 5, 0x82, 3, 64, 0, 1,
        9, 4, 1, 0, 2, 0x0A, 0, 0, 0,
        7, 5, 0x01, 2, 64, 0, 0,
        7, 5, 0x81, 2, 64, 0, 0,
        9, 4, 2, 0, 2, 0xFF, 0xFF, 0x01, 0,
        7, 5, 0x02, 2, 64, 0, 0,
        7, 5, 0x83, 2, 64, 0, 0,
    ];
    let mut p = probed(
        kvm_core::bt::DONGLE_VID,
        kvm_core::bt::DONGLE_PID,
        "Simulated",
        "ESP32-C3 board",
        HVec::from_slice(CONFIG).unwrap_or_default(),
        &[],
    );
    p.serial = "58:CF:79:00:11:22".try_into().unwrap_or_default();
    p
}

const BT_READY: u8 = host_flags::BONDED | host_flags::CONNECTED | host_flags::ENCRYPTED | host_flags::SUBSCRIBED;
const DEVICE_READY: u8 = device_flags::BONDED | device_flags::CONNECTED | device_flags::ENCRYPTED | device_flags::READY;

/// A generic gamepad (`OPENDYN_SIM_PAD=1`, on the Aux 2 port): 14 buttons, a hat with a null state, X, Y, Z, Rz.
#[rustfmt::skip]
const GAMEPAD: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x15, 0x00, 0x25, 0x01, 0x35, 0x00, 0x45, 0x01, 0x75, 0x01, 0x95, 0x0E, 0x05,
    0x09, 0x19, 0x01, 0x29, 0x0E, 0x81, 0x02, 0x95, 0x02, 0x81, 0x01, 0x05, 0x01, 0x25, 0x07, 0x46, 0x3B, 0x01, 0x75,
    0x04, 0x95, 0x01, 0x65, 0x14, 0x09, 0x39, 0x81, 0x42, 0x65, 0x00, 0x95, 0x01, 0x81, 0x01, 0x26, 0xFF, 0x00, 0x46,
    0xFF, 0x00, 0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x75, 0x08, 0x95, 0x04, 0x81, 0x02, 0xC0,
];

/// A symmetric-stick pad with a touch pad (`OPENDYN_SIM_PAD=symmetric`), report 1 as the 5 series lays it out: X Y Z
/// Rz Rx Ry, a vendor byte, the hat and 15 buttons, vendor data (the touch pad among it).
#[rustfmt::skip]
const SYMMETRIC_PAD: &[u8] = &[
    0x05, 0x01, 0x09, 0x05, 0xA1, 0x01, 0x85, 0x01,
    0x09, 0x30, 0x09, 0x31, 0x09, 0x32, 0x09, 0x35, 0x09, 0x33, 0x09, 0x34, 0x15, 0x00, 0x26, 0xFF, 0x00,
    0x75, 0x08, 0x95, 0x06, 0x81, 0x02,
    0x06, 0x00, 0xFF, 0x09, 0x20, 0x95, 0x01, 0x81, 0x02,
    0x05, 0x01, 0x09, 0x39, 0x15, 0x00, 0x25, 0x07, 0x35, 0x00, 0x46, 0x3B, 0x01, 0x65, 0x14, 0x75, 0x04,
    0x95, 0x01, 0x81, 0x42,
    0x65, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x0F, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x0F, 0x81, 0x02,
    0x06, 0x00, 0xFF, 0x09, 0x21, 0x95, 0x0D, 0x81, 0x02,
    0x06, 0x00, 0xFF, 0x09, 0x22, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x95, 0x34, 0x81, 0x02,
    0xC0,
];

/// The gamepad the simulated KVM has on the Aux 2 port instead of the webcam (`OPENDYN_SIM_PAD`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SimPad {
    /// `1`: a generic HID gamepad.
    Generic,
    /// `receiver`: an XInput wireless receiver with a pad linked.
    Receiver,
    /// `symmetric`: a symmetric-stick pad with a touch pad.
    Symmetric,
}

fn sim_pad() -> Option<SimPad> {
    match crate::env_var("OPENDYN_SIM_PAD").as_deref() {
        Some("1") => Some(SimPad::Generic),
        Some("receiver") => Some(SimPad::Receiver),
        Some("symmetric") => Some(SimPad::Symmetric),
        _ => None,
    }
}

/// A 3-button mouse with X, Y and a wheel (no report IDs), as a simulated Bluetooth mouse's report map.
#[rustfmt::skip]
const BT_MOUSE: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x03, 0x15, 0x00, 0x25,
    0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, 0x95, 0x01, 0x75, 0x05, 0x81, 0x01, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
    0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06, 0xC0, 0xC0,
];

/// A Bluetooth device the simulated module knows: paired from the start, or one to find while looking.
#[derive(Clone, Copy)]
struct SimDevice {
    addr: [u8; 7],
    name: &'static str,
    appearance: u16,
    map: &'static [u8],
    rssi: i8,
}

/// The keyboard paired from the start, and the two that show up while looking (a keyboard asks for a passkey).
const SIM_DEVICES: [SimDevice; 3] = [
    SimDevice {
        addr: [1, 0xC2, 0x11, 0x22, 0x33, 0x44, 0x01],
        name: "Desk keyboard",
        appearance: appearance::KEYBOARD,
        map: BOOT_KEYBOARD,
        rssi: -48,
    },
    SimDevice {
        addr: [1, 0xC2, 0x11, 0x22, 0x33, 0x44, 0x02],
        name: "Travel mouse",
        appearance: appearance::MOUSE,
        map: BT_MOUSE,
        rssi: -61,
    },
    SimDevice {
        addr: [1, 0xC2, 0x11, 0x22, 0x33, 0x44, 0x03],
        name: "Compact keyboard",
        appearance: appearance::KEYBOARD,
        map: BOOT_KEYBOARD,
        rssi: -74,
    },
];

fn device_state(slot: u8, d: &SimDevice, flags: u8) -> DeviceState {
    DeviceState {
        dev: slot,
        flags,
        rssi: if flags & device_flags::CONNECTED != 0 { d.rssi } else { 0 },
        battery: 70,
        appearance: d.appearance,
        addr: d.addr,
        map_len: d.map.len() as u16,
        map_crc: kvm_proto::crc::crc32(d.map),
        vid: 0,
        pid: 0,
    }
}

fn sim_host(slot: u8, flags: u8) -> HostState {
    use kvm_proto::link::bt::reports;
    let interval = if flags & host_flags::CONNECTED != 0 { 12 } else { 0 };
    // The tablet on host 1 takes no gamepad (its fallback keymap shows), any other host every report.
    let taken = if slot == 1 { reports::ALL & !reports::GAMEPAD } else { reports::ALL };
    let reports = if flags & host_flags::CONNECTED != 0 { reports::KNOWN | taken } else { 0 };
    HostState {
        host: slot,
        flags,
        interval,
        latency: 0,
        addr: [1, 0x5A, 0x6E, 0x3B, 0x21, 0xC4, slot],
        rssi: -52,
        reports,
    }
}

enum Delayed {
    Attach(Port),
    /// The simulated Bluetooth module: a host shows up while pairing, connects, or bonds.
    BtFound,
    BtHost(HostState),
    /// Looking for devices: one shows up.
    DeviceFound(SimDevice),
    /// Pairing a device moves on.
    DeviceStage(DevicePairing, SimDevice),
    PcsConfigured,
    KeyPress,
    FirmwareDone,
    /// The module's stream written and checked (its size), then the module saying hello.
    ModuleWritten(u32),
    BtUp,
}

impl Default for SimTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl SimTransport {
    /// The simulated KVM, with the Bluetooth module if `OPENDYN_SIM_BT=1`, or one that needs an update with
    /// `needs-update`.
    pub fn new() -> Self {
        match crate::env_var("OPENDYN_SIM_BT").as_deref() {
            Some("1") => Self::with_module(BtModule::Up),
            Some("wireless") => Self::over_wireless(),
            Some("needs-update") => Self::with_module(BtModule::NeedsUpdate),
            Some("dongle") => Self::with_dongle(),
            _ => Self::with_module(BtModule::Absent),
        }
    }

    /// The simulated KVM with its module, reached as wireless host 1 over the module's control service: it may not
    /// control the KVM until it asks and the (simulated) key is pressed.
    pub fn over_wireless() -> Self {
        let mut sim = Self::with_module(BtModule::Up);
        sim.asker = Asker::Bt(1);
        if let Some(hosts) = sim.bt.as_mut() {
            hosts[0].flags |= host_flags::CONTROL;
            sim.engine.bt_host(hosts[0]);
        }
        while sim.engine.poll_output().is_some() {}
        sim
    }

    /// An answer of the simulated KVM, for this program to read.
    fn answer(&mut self, mut resp: Report) -> Result<(), String> {
        if let (Some(proto), Ok(DeviceMsg::Response { id, request, response: Response::Info(i) })) =
            (self.proto, DeviceMsg::decode(&resp))
        {
            resp = Response::Info(Info { proto, ..i }).encode(request, id).map_err(|e| format!("{e:?}"))?;
        }
        // Someone "presses the key" on the simulated KVM when it asks for it.
        if let Ok(DeviceMsg::Response { response: Response::Error(ErrorCode::NeedsKeyPress), .. }) =
            DeviceMsg::decode(&resp)
        {
            self.later(2500, Delayed::KeyPress);
        }
        self.inbox.push_back(resp);
        Ok(())
    }

    /// The simulated KVM without the module on its board, and an ESP32-C3 board on the Aux 2 port as the module.
    pub fn with_dongle() -> Self {
        let mut sim = Self::with_module(BtModule::Absent);
        sim.dongle = true;
        sim.engine.port_detached(Port::Aux2);
        sim.attach(Port::Aux2);
        while sim.engine.poll_output().is_some() {}
        sim
    }

    /// What the simulated device on `p` does on attaching; the dongle is found to be the module and says hello.
    fn attach(&mut self, p: Port) {
        if self.dongle && p == Port::Aux2 {
            self.engine.port_attached(p, &esp_board());
            if self.engine.dongle_wanted(p) {
                self.engine.dongle_claimed(p);
                self.bt = Some(Self::module_up(&mut self.engine));
                self.devices_up();
            }
        } else {
            self.engine.port_attached(p, &device_for(p));
        }
    }

    pub fn with_bluetooth(fitted: bool) -> Self {
        Self::with_module(if fitted { BtModule::Up } else { BtModule::Absent })
    }

    /// The simulated KVM with its Bluetooth module `module`: absent, up, or needing an update (anything else counts as
    /// up).
    pub fn with_module(module: BtModule) -> Self {
        Self::with_board(module, sim_aux1())
    }

    /// The simulated KVM with its Bluetooth module `module`, on a board with the Aux 1 port (0.19.0) or one before it.
    pub fn with_board(module: BtModule, aux1: bool) -> Self {
        let info = Info {
            proto: kvm_proto::ctrl::PROTOCOL_VERSION,
            rp_fw: FW_VERSION,
            at_fw: [0; 2],
            hw: {
                let (major, minor, patch) =
                    if aux1 { kvm_proto::HARDWARE_VERSION_AUX1 } else { kvm_proto::HARDWARE_VERSION };
                version_u32(major, minor, patch)
            },
            serial: *b"SIM00001",
            caps: caps::MIRRORED
                | caps::IDENTITY
                | caps::PASSTHROUGH
                | caps::FW_UPDATE
                | caps::BT_UPDATE
                | caps::VIAL
                | if aux1 { caps::AUX1 } else { 0 }
                | caps::SIMULATED,
            bt_fw: 0,
        };
        let mut engine = Box::new(Engine::new(Config::default(), info, "SIM00001"));
        engine.pc_hello(Pc::Pc1, FW_VERSION);
        engine.pc_hello(Pc::Pc2, FW_VERSION);
        engine.pc_status(Pc::Pc1, pc_status(940, 5050, UsbState::Configured));
        engine.pc_status(Pc::Pc2, pc_status(420, 4980, UsbState::Configured));
        engine.set_power(PowerInfo { source: PowerSource::PcBus, adv_pwr: Advertised::None, vbus_pwr_mv: 0 });
        engine.set_selftest(if module == BtModule::NeedsUpdate { kvm_proto::ctrl::selftest::BT_FW } else { 0 });
        engine.set_board_diag(BoardDiag::TYPICAL);
        if sim_pad().is_some() {
            use kvm_proto::ctrl::{ConfigValue, Device, GamepadOut, PortMode};
            let normalized = ConfigValue::Mode(Device::Aux2, PortMode::Normalized);
            let _ = engine.set_config(normalized, kvm_core::config::Origin::Device);
            let _ = engine.set_config(ConfigValue::Gamepad(GamepadOut::Hid), kvm_core::config::Origin::Device);
        }
        for p in Port::ALL.iter().filter(|p| aux1 || **p != Port::Aux1) {
            engine.port_attached(*p, &device_for(*p));
        }
        // The receiver's pad is linked.
        if sim_pad() == Some(SimPad::Receiver) {
            engine.device_report(Port::Aux2, 0, &[0x08, 0x80], 0);
        }
        if crate::env_var("OPENDYN_SIM_HOP").as_deref() == Some("1") {
            Self::hop_demo(&mut engine);
        }
        // The simulation starts with everything already enumerated: boot-time outputs are not acted out.
        let bt = match module {
            BtModule::Absent => None,
            BtModule::NeedsUpdate => {
                engine.set_bt_module(BtModule::NeedsUpdate);
                None
            }
            _ => Some(Self::module_up(&mut engine)),
        };
        let mut sim = Self {
            engine,
            start: Instant::now(),
            last_ms: 0,
            inbox: VecDeque::new(),
            timeline: Vec::new(),
            keys: channel(),
            bt,
            dongle: false,
            proto: None,
            devices: [None; 4],
            module_stream: None,
            asker: Asker::Pc(Pc::Pc1),
        };
        if sim.bt.is_some() {
            sim.devices_up();
        }
        while sim.engine.poll_output().is_some() {}
        sim
    }

    /// The module's devices: the keyboard paired from the start, connected, in slot 1.
    fn devices_up(&mut self) {
        self.devices = [Some(SIM_DEVICES[0]), None, None, None];
        for slot in 1..=4u8 {
            self.device_to_engine(slot, DEVICE_READY);
        }
        // Its report map, as the engine asks for it; nothing else is acted out (as at boot).
        while let Some(o) = self.engine.poll_output() {
            if let Output::Bt(b) = o {
                self.bt_out(b);
            }
        }
    }

    /// Slot `slot`'s state to the engine: with `flags` if a device is in it.
    fn device_to_engine(&mut self, slot: u8, flags: u8) {
        let i = slot as usize - 1;
        let (state, name) = match self.devices[i] {
            Some(d) => (device_state(slot, &d, flags), d.name),
            None => (DeviceState { dev: slot, battery: 0xFF, ..DeviceState::default() }, ""),
        };
        self.engine.bt_device(state, name);
    }

    /// The simulated KVM's firmware speaking protocol `proto` (another than this program's: the programs offer only the
    /// firmware update then).
    pub fn speaking(self, proto: u8) -> Self {
        Self { proto: Some(proto), ..self }
    }

    /// Screen hop set up (`OPENDYN_SIM_HOP=1`): the desk in arrangement 0, used by profile 0, with the tablet of
    /// wireless host 1 below PC1's main screen; arrangement 1, PC2 with the tablet beside it, used by no profile yet.
    fn hop_demo(engine: &mut Engine) {
        use kvm_proto::ctrl::{ConfigValue, Host, ProfileName, Screen};
        let pc2 = Screen { scale: 150, ..Screen::new(Host::Pc2, 3840, 2160, [2560, -120]) };
        let tablet = Screen { scale: 200, ..Screen::new(Host::Bt1, 2388, 1668, [683, 1440]) };
        for v in [
            ConfigValue::AbsPointer(true),
            ConfigValue::ProfileArrangement(0, Some(0)),
            ConfigValue::ArrangementName(0, ProfileName::new("desk")),
            ConfigValue::Screen(0, 0, Screen::new(Host::Pc1, 2560, 1440, [0, 0])),
            ConfigValue::Screen(
                0,
                1,
                Screen { desk: [-1920, 360], ..Screen::new(Host::Pc1, 1920, 1080, [-1920, 360]) },
            ),
            ConfigValue::Screen(0, 2, pc2),
            // A monitor cabled to both: PC1 has it too, off for hop, laid on PC2's screen.
            ConfigValue::Screen(
                0,
                3,
                Screen { host: Some(Host::Pc1), desk: [2560, -120], off: true, fixed: true, ..pc2 },
            ),
            ConfigValue::Screen(0, 4, tablet),
            ConfigValue::ArrangementName(1, ProfileName::new("sofa")),
            ConfigValue::Screen(1, 0, Screen { at: [0, 0], ..pc2 }),
            ConfigValue::Screen(1, 1, Screen { at: [2560, 300], ..tablet }),
        ] {
            let _ = engine.set_config(v, kvm_core::config::Origin::Device);
        }
    }

    /// The module says hello with its hosts: a tablet in slot 1, connected, a phone in slot 2, away.
    fn module_up(engine: &mut Engine) -> [HostState; 4] {
        let hosts = [sim_host(1, BT_READY), sim_host(2, host_flags::BONDED), sim_host(3, 0), sim_host(4, 0)];
        engine.set_bt_module(BtModule::Up);
        engine.bt_hello(FW_VERSION);
        let names = [(0, "Tablet"), (1, "Phone")];
        for (i, name) in names {
            let v = kvm_proto::ctrl::ConfigValue::BtName(i, kvm_proto::ctrl::ProfileName::new(name));
            let _ = engine.set_config(v, kvm_core::config::Origin::Device);
        }
        let policy = kvm_proto::ctrl::ConfigValue::BtPolicy(1, kvm_proto::ctrl::BtPolicy::ConnectWhenSelected);
        let _ = engine.set_config(policy, kvm_core::config::Origin::Device);
        for h in hosts {
            engine.bt_host(h);
        }
        hosts
    }

    /// The simulated module acts on what the engine asks of it.
    fn bt_out(&mut self, o: BtOut) {
        // An answer or event over the control service: this program's, when it is that host.
        if let BtOut::Control { host, data } = o {
            if self.asker == Asker::Bt(host) {
                let _ = self.answer(data);
            }
            return;
        }
        let Some(hosts) = self.bt.as_mut() else { return };
        match o {
            BtOut::Pair { on: true, seconds } => {
                self.engine.bt_event(BtEvent::PairingOpen, 0, seconds as u32);
                self.later(3000, Delayed::BtFound);
            }
            BtOut::Pair { on: false, .. } => self.engine.bt_event(BtEvent::PairingClosed, 0, 0),
            BtOut::Passkey { host, value: Some(_) } => {
                self.engine.bt_event(BtEvent::Paired, host, 0);
                self.later(600, Delayed::BtHost(sim_host(host, BT_READY)));
            }
            BtOut::Passkey { host, value: None } => {
                self.engine.bt_event(BtEvent::PairFailed, host, 0x01);
                self.engine.bt_event(BtEvent::PairingClosed, 0, 0);
            }
            BtOut::Forget(n) => {
                for h in hosts.iter_mut().filter(|h| n == 0xFF || h.host == n) {
                    *h = sim_host(h.host, 0);
                    self.engine.bt_host(*h);
                }
            }
            BtOut::Select { host: n, pinned } => {
                // A host that connects only while selected (or pinned to) connects now.
                let wanted = |h: &HostState| h.host == n || pinned & (1 << (h.host - 1)) != 0;
                if let Some(h) = hosts.iter().find(|h| wanted(h) && h.bonded() && !h.connected()) {
                    let h = sim_host(h.host, BT_READY);
                    self.later(800, Delayed::BtHost(h));
                }
            }
            BtOut::RequestState => {
                for h in *hosts {
                    self.engine.bt_host(h);
                }
                for slot in 1..=4u8 {
                    let connected = self.devices[slot as usize - 1].is_some();
                    self.device_to_engine(slot, if connected { DEVICE_READY } else { 0 });
                }
            }
            BtOut::Scan { on: true, seconds } => {
                self.engine.bt_event(BtEvent::ScanOpen, 0, seconds as u32);
                // The devices not paired yet show up, the nearest first.
                for (i, d) in SIM_DEVICES.iter().enumerate() {
                    if !self.devices.iter().flatten().any(|p| p.addr == d.addr) {
                        self.later(1200 + 900 * i as u32, Delayed::DeviceFound(*d));
                    }
                }
            }
            BtOut::Scan { on: false, .. } => {
                self.timeline.retain(|(_, d)| !matches!(d, Delayed::DeviceFound(_)));
                self.engine.bt_event(BtEvent::ScanClosed, 0, 0);
            }
            BtOut::PairDevice(addr) => {
                self.timeline.retain(|(_, d)| !matches!(d, Delayed::DeviceFound(_)));
                self.engine.bt_event(BtEvent::ScanClosed, 0, 0);
                match SIM_DEVICES.iter().find(|d| d.addr == addr) {
                    Some(d) if self.devices.iter().any(Option::is_none) => {
                        self.engine.bt_event(BtEvent::DeviceStage, 0, DevicePairing::Connecting as u32);
                        self.later(700, Delayed::DeviceStage(DevicePairing::Pairing, *d));
                    }
                    _ => self.engine.bt_event(BtEvent::DeviceStage, 0, DevicePairing::Failed as u32),
                }
            }
            BtOut::EraseAll => {
                // The module forgets every host and device and restarts: its link goes and comes back.
                for h in hosts.iter_mut() {
                    *h = sim_host(h.host, 0);
                }
                self.devices = [None; 4];
                self.engine.bt_link_down();
                self.engine.bt_hello(FW_VERSION);
            }
            BtOut::ForgetDevice(n) => {
                for slot in 1..=4u8 {
                    if n == 0xFF || n == slot {
                        self.devices[slot as usize - 1] = None;
                        self.device_to_engine(slot, 0);
                    }
                }
            }
            BtOut::RequestMap(dev) => {
                if let Some(d) = self.devices.get(dev as usize - 1).copied().flatten() {
                    let total = d.map.len() as u16;
                    for (i, piece) in d.map.chunks(MAP_CHUNK).enumerate() {
                        self.engine.bt_device_map(dev, (i * MAP_CHUNK) as u16, total, piece);
                    }
                }
            }
            BtOut::Input { .. } | BtOut::Policy { .. } | BtOut::DeviceOutput { .. } | BtOut::Control { .. } => {}
        }
    }

    /// Pairing a simulated device moves on: a keyboard asks for its passkey to be typed (on it: here nobody types,
    /// it goes on after a while), then the device is read, then it is in a slot of its own.
    fn device_stage(&mut self, stage: DevicePairing, d: SimDevice) {
        match stage {
            DevicePairing::Pairing => {
                self.engine.bt_event(BtEvent::DeviceStage, 0, DevicePairing::Pairing as u32);
                let typing = if d.appearance == appearance::KEYBOARD {
                    self.engine.bt_event(BtEvent::DevicePasskey, 0, 246_810);
                    4000
                } else {
                    600
                };
                self.later(typing, Delayed::DeviceStage(DevicePairing::Reading, d));
            }
            DevicePairing::Reading => {
                self.engine.bt_event(BtEvent::DeviceStage, 0, DevicePairing::Reading as u32);
                self.later(800, Delayed::DeviceStage(DevicePairing::Paired, d));
            }
            DevicePairing::Paired => {
                let Some(i) = self.devices.iter().position(Option::is_none) else {
                    self.engine.bt_event(BtEvent::DeviceStage, 0, DevicePairing::Failed as u32);
                    return;
                };
                self.devices[i] = Some(d);
                self.device_to_engine(i as u8 + 1, DEVICE_READY);
                self.engine.bt_event(BtEvent::DeviceStage, i as u8 + 1, DevicePairing::Paired as u32);
            }
            _ => {}
        }
    }

    fn now(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn later(&mut self, ms: u32, d: Delayed) {
        let at = self.now() + ms;
        self.timeline.push((at, d));
    }

    fn mouse_report(t: u32) -> [u8; 9] {
        // A slow circle: deltas of at most 1, no buttons. Small enough that screen hop's pointer
        // (`OPENDYN_SIM_HOP=1`) circles in the middle of a screen and never pushes across to the other PC.
        let phase = (t % 2000) as f32 / 2000.0 * std::f32::consts::TAU;
        let (x, y) = ((phase.cos() * 1.5) as i16, (phase.sin() * 1.5) as i16);
        let mut r = [0u8; 9];
        r[0] = 2;
        r[3..5].copy_from_slice(&x.to_le_bytes());
        r[5..7].copy_from_slice(&y.to_le_bytes());
        r
    }

    /// Advances simulated time: mouse traffic, timers, outputs.
    fn pump(&mut self) {
        let now = self.now();
        // Catch up on reports missed while nobody asked (bounded, so a long pause doesn't stall the caller).
        let from = self.last_ms.max(now.saturating_sub(3000));
        for t in from..now {
            self.engine.device_report(Port::Mouse, 0, &Self::mouse_report(t), t);
            // Drain after every call, as the firmware does: the engine's output queue is sized for one call.
            self.drain();
        }
        self.last_ms = now;
        while let Ok(r) = self.keys.1.try_recv() {
            self.engine.device_report(Port::Keyboard, 0, &r, now);
            self.drain();
        }
        self.engine.tick(now);
        self.drain();

        let due: Vec<Delayed> = {
            let (due, keep): (Vec<_>, Vec<_>) = self.timeline.drain(..).partition(|(at, _)| *at <= now);
            self.timeline = keep;
            due.into_iter().map(|(_, d)| d).collect()
        };
        for d in due {
            match d {
                Delayed::Attach(p) => self.attach(p),
                Delayed::BtFound => {
                    if let Some(free) = self.bt.and_then(|hs| hs.iter().find(|h| !h.bonded()).map(|h| h.host)) {
                        let _ = self.engine.set_config(
                            kvm_proto::ctrl::ConfigValue::BtName(free - 1, kvm_proto::ctrl::ProfileName::new("Laptop")),
                            kvm_core::config::Origin::Device,
                        );
                        self.engine.bt_event(BtEvent::PasskeyRequest, free, 0);
                    }
                }
                Delayed::BtHost(h) => {
                    if let Some(hosts) = self.bt.as_mut() {
                        hosts[h.host as usize - 1] = h;
                    }
                    self.engine.bt_host(h);
                }
                Delayed::PcsConfigured => {
                    self.engine.pc_status(Pc::Pc1, pc_status(940, 5050, UsbState::Configured));
                    self.engine.pc_status(Pc::Pc2, pc_status(420, 4980, UsbState::Configured));
                }
                Delayed::KeyPress => self.engine.key(KeyEvent::Tap),
                Delayed::FirmwareDone => self.engine.set_fw_state(FwState::Done, 0),
                Delayed::ModuleWritten(size) => {
                    self.engine.set_fw_state(FwState::Done, size);
                    self.engine.set_bt_module(BtModule::Starting);
                    let bits = self.engine.selftest() & !kvm_proto::ctrl::selftest::BT;
                    self.engine.set_selftest(bits);
                    self.later(1500, Delayed::BtUp);
                }
                Delayed::BtUp => {
                    self.bt = Some(Self::module_up(&mut self.engine));
                    self.devices_up();
                }
                Delayed::DeviceFound(d) => self.engine.bt_found(d.addr, d.rssi, d.appearance, d.name),
                Delayed::DeviceStage(stage, d) => self.device_stage(stage, d),
            }
            self.drain();
        }
    }

    fn drain(&mut self) {
        while let Some(o) = self.engine.poll_output() {
            match o {
                Output::Report { pc: Pc::Pc1, ep, data } if ep == self.engine.presentation().control.ep_in => {
                    let mut r = [0u8; REPORT_LEN];
                    r[..data.len()].copy_from_slice(&data);
                    self.inbox.push_back(r);
                }
                Output::Reprobe(p) => {
                    if self.dongle && p == Port::Aux2 {
                        // Replugged, it leaves Hub-0 first (the real KVM sees it go).
                        self.engine.port_detached(p);
                        self.bt = None;
                    }
                    self.engine.port_probing(p);
                    self.later(700, Delayed::Attach(p));
                }
                Output::DescriptorSetChanged => {
                    // Both PCs re-enumerate the composite.
                    self.engine.pc_status(Pc::Pc1, pc_status(940, 5050, UsbState::Default));
                    self.engine.pc_status(Pc::Pc2, pc_status(420, 4980, UsbState::Default));
                    self.later(900, Delayed::PcsConfigured);
                }
                Output::Save => {
                    let mut buf = [0u8; kvm_core::engine::SAVE_ITEM_MAX];
                    while self.engine.take_save_item(&mut buf).is_some() {}
                    self.engine.config_saved();
                }
                // The module's firmware streams through to the module as it comes, and needs no key.
                Output::Firmware(FwOp::Begin { target, size, .. }) => {
                    self.module_stream = (target == FwTarget::Module).then_some(size);
                    if self.module_stream.is_some() {
                        self.engine.bt_link_down();
                        self.bt = None;
                        self.engine.set_bt_module(BtModule::Programming);
                    }
                }
                Output::Firmware(FwOp::Data { offset, data }) => {
                    if let Some(size) = self.module_stream {
                        let done = offset + data.len() as u32;
                        self.engine.set_fw_state(FwState::Receiving, done);
                        self.engine.set_bt_progress((done as u64 * 100 / size.max(1) as u64) as u8);
                    }
                }
                Output::Firmware(FwOp::Commit) => match self.module_stream.take() {
                    Some(size) => self.later(1200, Delayed::ModuleWritten(size)),
                    None => self.later(2500, Delayed::KeyPress),
                },
                Output::Firmware(FwOp::Confirm) => self.later(1500, Delayed::FirmwareDone),
                Output::Bt(b) => self.bt_out(b),
                _ => {}
            }
        }
    }
}

#[maybe_async::maybe_async(?Send)]
impl Transport for SimTransport {
    async fn write(&mut self, report: &Report) -> Result<(), String> {
        self.pump();
        match self.asker {
            // Over the module: the answer comes back through it (`bt_out`).
            Asker::Bt(n) => {
                let now = self.now();
                self.engine.bt_control(n, kvm_proto::link::bt::control::trim(report), now);
                self.drain();
            }
            asker => {
                if let Some(resp) = self.engine.control(asker, report) {
                    self.answer(resp)?;
                }
            }
        }
        self.pump();
        Ok(())
    }

    async fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if let Some(r) = self.inbox.pop_front() {
                return Ok(Some(r));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            crate::time::pause(left.min(Duration::from_millis(5))).await;
        }
    }

    fn describe(&self) -> String {
        crate::fl!("simulated-kvm")
    }

    fn link(&self) -> Link {
        match self.asker {
            Asker::Bt(_) => Link::Bluetooth,
            _ => Link::Simulated,
        }
    }

    fn window(&self) -> usize {
        match self.asker {
            Asker::Bt(_) => kvm_proto::link::bt::control::WINDOW,
            _ => 1,
        }
    }

    fn simulated_keyboard(&self) -> Option<Sender<[u8; 8]>> {
        Some(self.keys.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Client;
    use crate::client::DeviceSlot;
    use kvm_proto::ctrl::{
        ConfigValue, Device, Event, Hotkey, HotkeyAction, HotkeyBinding, LearnEnd, PortMode, PortState, SwitchSource,
        Target,
    };

    #[test]
    fn a_board_before_0_19_has_three_ports_and_a_newer_one_four() {
        let mut c = Client::new(Box::new(SimTransport::with_board(BtModule::Absent, false)));
        let s = c.snapshot().unwrap();
        assert!(!s.has_port(Port::Aux1));
        assert_eq!(s.front_ports().collect::<Vec<_>>(), [Port::Keyboard, Port::Mouse, Port::Aux2]);
        assert_eq!(s.routed_devices(), [Device::Keyboard, Device::Mouse, Device::Aux2]);
        assert_eq!(s.info.hw, version_u32(0, 12, 0));
        assert_eq!(s.ports[Port::Aux1.index()].state, PortState::Empty);

        let mut c = Client::new(Box::new(SimTransport::with_board(BtModule::Absent, true)));
        let s = c.snapshot().unwrap();
        assert_eq!(s.front_ports().count(), 4);
        assert_eq!(s.info.hw, version_u32(0, 19, 0));
        // Its media dial is mirrored, as on Aux 2; it can't be passed through.
        let a1 = &s.ports[Port::Aux1.index()];
        assert_eq!((a1.state, a1.effective), (PortState::Hosted, PortMode::Mirrored));
        assert_eq!(s.products[Port::Aux1.index()], "Media dial (simulated)");
        assert!(c.set_mode(Device::Aux1, PortMode::Passthrough).is_err());
    }

    #[test]
    fn configuration_comes_back_after_the_key_is_pressed() {
        let mut c = Client::new(Box::new(SimTransport::new()));
        c.set_config(ConfigValue::SwConfigAllowed(false)).unwrap();
        assert_eq!(c.set_config(ConfigValue::LedBrightness(9)), Err(crate::Error::Device(ErrorCode::Locked)));
        let mut asked = false;
        // The simulator presses the key 2.5 s after the KVM asks for it.
        c.allow_software(ConfigValue::SwConfigAllowed(true), || asked = true).unwrap();
        assert!(asked);
        assert_eq!(c.set_config(ConfigValue::LedBrightness(9)), Ok(ConfigValue::LedBrightness(9)));
    }

    #[test]
    fn the_simulated_keyboard_types_a_hotkey_to_learn() {
        let mut c = Client::new(Box::new(SimTransport::new()));
        let keys = c.simulated_keyboard().unwrap();
        // Ctrl+Alt+K, typed once the KVM listens.
        let (end, h) = c
            .learn_hotkey(|| {
                for r in
                    [[0x05, 0, 0, 0, 0, 0, 0, 0], [0x05, 0, 0x0E, 0, 0, 0, 0, 0], [0x05, 0, 0, 0, 0, 0, 0, 0], [0; 8]]
                {
                    keys.send(r).unwrap();
                }
            })
            .unwrap();
        assert_eq!((end, kvm_core::hotkey::format_spec(&h).to_string()), (LearnEnd::Learned, "ctrl+alt+k".into()));
        // Learning changed nothing: the list still has its one default entry.
        let toggle = HotkeyBinding::new(HotkeyAction::TogglePc, Hotkey::double_tap(0x47, true));
        assert_eq!(c.get_config(kvm_proto::ctrl::key::hotkey(0)), Ok(ConfigValue::Hotkey(0, toggle)));
        assert_eq!(c.get_config(kvm_proto::ctrl::key::hotkey(1)), Ok(ConfigValue::Hotkey(1, HotkeyBinding::NONE)));
    }

    #[test]
    fn a_module_that_needs_an_update_gets_it_streamed() {
        let mut c = Client::new(Box::new(SimTransport::with_module(BtModule::NeedsUpdate)));
        let s = c.snapshot().unwrap();
        assert!(!s.has_bluetooth() && s.bt.is_empty(), "no Bluetooth while the module doesn't run");
        assert_eq!(s.status.bt_module, BtModule::NeedsUpdate);
        assert_ne!(s.status.selftest & kvm_proto::ctrl::selftest::BT_FW, 0);
        let mut seen = Vec::new();
        c.update_module(&[0x55; 3000], |state, done, _| seen.push((state, done))).unwrap();
        assert!(seen.contains(&(FwState::Receiving, 3000)) && seen.last() == Some(&(FwState::Done, 3000)), "{seen:?}");
        let deadline = Instant::now() + Duration::from_secs(10);
        while c.status().unwrap().bt_module != BtModule::Up {
            assert!(Instant::now() < deadline, "the module never came up");
            std::thread::sleep(Duration::from_millis(100));
        }
        let s = c.snapshot().unwrap();
        assert!(s.has_bluetooth());
        assert_eq!(s.info.bt_fw, FW_VERSION);
        assert_eq!(s.status.selftest & kvm_proto::ctrl::selftest::BT, 0);
        assert_eq!(s.bt_bonded().count(), 2);
    }

    #[test]
    fn bluetooth_hosts_pair_switch_and_go_away_without_the_module() {
        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(false)));
        let s = c.snapshot().unwrap();
        assert!(!s.has_bluetooth() && s.bt.is_empty());

        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(true)));
        let s = c.snapshot().unwrap();
        assert!(s.has_bluetooth());
        let names: Vec<&str> = s.bt_bonded().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["Tablet", "Phone"]);
        // Pairing finds a laptop that asks for its passkey; typed in software here.
        c.bt_pair(true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        while c.status().unwrap().bt_passkey == 0 {
            assert!(Instant::now() < deadline, "no passkey request");
            std::thread::sleep(Duration::from_millis(50));
        }
        c.bt_passkey(424242).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        let s = c.snapshot().unwrap();
        assert_eq!(s.bt_bonded().count(), 3);
        assert!(s.bt.iter().any(|h| h.name == "Laptop" && h.ready()));
        assert_eq!(s.status.bt_pairing, 0);
        // Switching to a host and back.
        c.switch(kvm_proto::ctrl::Target::Bt1).unwrap();
        assert_eq!(c.status().unwrap().bt_active, 1);
        c.switch(kvm_proto::ctrl::Target::Toggle).unwrap();
        let st = c.status().unwrap();
        assert_eq!((st.bt_active, st.active), (0, Pc::Pc1));
    }

    #[test]
    fn a_wireless_device_is_found_paired_and_listed_with_its_routing() {
        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(true)));
        let s = c.snapshot().unwrap();
        // The keyboard paired from the start, connected; a free slot is no device to set.
        assert_eq!(s.devices.iter().filter(|d| d.bonded()).count(), 1);
        assert!(s.device(1).is_some_and(|d| d.ready() && d.name.as_str() == "Desk keyboard"));
        assert_eq!(
            s.routed_devices(),
            [Device::Keyboard, Device::Mouse, Device::Aux1, Device::Aux2, Device::Wireless1]
        );
        // The keyboard's report map as the KVM holds it (several pieces), and none for a free slot.
        let (iface, interfaces, map) = c.device_map(Device::Wireless1, 0).unwrap();
        assert_eq!((iface, interfaces), (0, 1));
        assert!(map.len() > kvm_proto::ctrl::MAP_PIECE && kvm_hid::Layout::parse(&map).is_ok());
        assert!(c.device_map(Device::Wireless3, 0).unwrap().2.is_empty(), "a free slot");
        // The module's log as the KVM keeps it: here its own line about the module's hello.
        assert!(c.module_log().unwrap().contains("the module said hello, firmware"));
        c.bt_scan(true).unwrap();
        let found = loop {
            let st = c.status().unwrap();
            if st.bt_found >= 2 {
                break c.bt_found(st.bt_found).unwrap();
            }
            crate::time::sleep(Duration::from_millis(50));
        };
        assert_eq!(found[0].name.as_str(), "Travel mouse", "the device paired already isn't offered");
        c.bt_pair_device(found[0].addr).unwrap();
        let st = loop {
            let st = c.status().unwrap();
            if st.bt_device_pairing == DevicePairing::Paired {
                break st;
            }
            assert_ne!(st.bt_device_pairing, DevicePairing::Failed);
            crate::time::sleep(Duration::from_millis(50));
        };
        assert_eq!((st.bt_scan, st.bt_devices_bonded, st.bt_devices_ready), (0, 0b11, 0b11));
        // Pinned to host 1 in the active profile, as a front port's device would be.
        c.set_config(ConfigValue::Route(Device::Wireless2, kvm_proto::ctrl::Routing::Pin(kvm_proto::ctrl::Host::Bt1)))
            .unwrap();
        let s = c.snapshot().unwrap();
        let (active, _, list) = s.profiles().unwrap();
        assert_eq!(
            list[active as usize].routes[Device::Wireless2.index()],
            kvm_proto::ctrl::Routing::Pin(kvm_proto::ctrl::Host::Bt1)
        );
        assert_eq!(s.routed_devices().len(), 6);
        // Forgotten: no longer listed.
        c.bt_forget_device(2).unwrap();
        assert_eq!(c.snapshot().unwrap().routed_devices().len(), 5);
        // Every pairing erased: no host and no device left.
        c.bt_erase_all().unwrap();
        let s = c.snapshot().unwrap();
        assert!(s.devices.iter().all(|d| !d.bonded()) && s.bt.iter().all(|h| !h.bonded()));
        assert_eq!(s.routed_devices().len(), 4, "the four front ports");
    }

    #[test]
    fn an_esp32_board_on_the_other_port_is_the_module() {
        let mut c = Client::new(Box::new(SimTransport::with_dongle()));
        let s = c.snapshot().unwrap();
        assert_eq!(s.ports[Port::Aux2.index()].state, PortState::Module);
        assert_eq!(s.module_port(), Some(Port::Aux2));
        assert!(s.has_bluetooth());
        assert_eq!(s.bt_bonded().count(), 2);
        // Re-detected, it leaves and comes back as the module.
        c.redetect(Port::Aux2).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while c.port(Port::Aux2).unwrap().state != PortState::Module {
            assert!(Instant::now() < deadline, "the board didn't come back as the module");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(c.snapshot().unwrap().has_bluetooth());
        // Without a board there is no module.
        let mut c = Client::new(Box::new(SimTransport::with_bluetooth(false)));
        assert_eq!(c.snapshot().unwrap().module_port(), None);
    }

    #[test]
    fn over_the_wireless_module_the_kvm_waits_for_its_key_then_answers_in_batches() {
        let mut c = Client::new(Box::new(SimTransport::over_wireless()));
        assert_eq!(c.link(), crate::transport::Link::Bluetooth);
        // Its identity, and nothing else until the key allowed it.
        assert!(c.info_checked().is_ok());
        assert_eq!(c.snapshot().err(), Some(crate::Error::Device(ErrorCode::NotAllowed)));
        assert!(!c.request_control().unwrap(), "the KVM waits for its key");
        // The simulator presses it after a while.
        c.ask_control(|| {}).unwrap();
        let s = c.snapshot().unwrap();
        assert_eq!((s.status.asking_bt(), s.status.asking_pc()), (Some(1), None));
        assert!(s.config.contains(&ConfigValue::BtControl(0, true)));
        // A setting goes through, and its event comes back over the module.
        c.set_config(ConfigValue::DoubleTapMs(450)).unwrap();
        let ev = c.events(Duration::from_millis(50)).unwrap();
        assert!(ev.contains(&Event::ConfigChanged(kvm_proto::ctrl::key::DOUBLE_TAP_MS)));
        // What only USB may do.
        assert_eq!(c.reboot(kvm_proto::ctrl::RebootTarget::Bootloader), Err(crate::Error::Device(ErrorCode::Unsupported)));
    }

    /// Over the wireless link the module drops an answer it has no room for (a switch's events came between): a read
    /// is asked again, and one missed answer is no lost KVM; answers missing in a row are. Until 0.1.53 one was, and
    /// the window let go of the KVM on a switch (2026-11-24).
    #[test]
    fn over_the_wireless_module_a_dropped_answer_is_no_lost_kvm() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// The simulator over the wireless link, leaving out the next `drop` answers.
        struct Lossy {
            sim: SimTransport,
            drop: Arc<AtomicUsize>,
        }
        impl Transport for Lossy {
            fn write(&mut self, r: &Report) -> Result<(), String> {
                self.sim.write(r)
            }
            fn read(&mut self, timeout: Duration) -> Result<Option<Report>, String> {
                let deadline = Instant::now() + timeout;
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    let Some(r) = self.sim.read(left)? else { return Ok(None) };
                    let answer = matches!(DeviceMsg::decode(&r), Ok(DeviceMsg::Response { .. }));
                    if answer && self.drop.load(Ordering::SeqCst) > 0 {
                        self.drop.fetch_sub(1, Ordering::SeqCst);
                        continue;
                    }
                    return Ok(Some(r));
                }
            }
            fn describe(&self) -> String {
                self.sim.describe()
            }
            fn link(&self) -> Link {
                self.sim.link()
            }
            fn window(&self) -> usize {
                self.sim.window()
            }
        }

        let drop = Arc::new(AtomicUsize::new(0));
        let mut c = Client::new(Box::new(Lossy { sim: SimTransport::over_wireless(), drop: drop.clone() }));
        c.ask_control(|| {}).unwrap();
        c.timeout = Duration::from_millis(100);
        // A refresh, one of its answers gone: asked again.
        drop.store(1, Ordering::SeqCst);
        assert!(c.snapshot().is_ok());
        drop.store(1, Ordering::SeqCst);
        assert!(c.status().is_ok());
        // A setting's answer gone: it isn't asked again (it did its work), and the KVM is still there.
        drop.store(1, Ordering::SeqCst);
        let e = c.set_config(ConfigValue::DoubleTapMs(450)).unwrap_err();
        assert_eq!(e, crate::Error::Timeout);
        assert!(!c.lost(&e));
        assert!(c.status().is_ok());
        // No answers any more: gone, after a few.
        drop.store(usize::MAX / 2, Ordering::SeqCst);
        let e = c.set_config(ConfigValue::DoubleTapMs(400)).unwrap_err();
        assert!(!c.lost(&e));
        let e = c.status().unwrap_err();
        assert!(c.lost(&e), "three missed in a row");
        // Over USB nothing is lost on the way: a missed answer is a KVM gone.
        let c = Client::new(Box::new(SimTransport::new()));
        assert!(c.lost(&crate::Error::Timeout));
        assert!(!c.lost(&crate::Error::Device(ErrorCode::Busy)));
    }

    #[test]
    fn simulated_kvm_answers_like_a_real_one() {
        let mut c = Client::new(Box::new(SimTransport::new()));
        let s = c.snapshot().unwrap();
        assert_eq!(s.status.asking_pc(), Some(Pc::Pc1));
        assert_eq!(s.ports[Port::Keyboard.index()].effective, PortMode::Normalized);
        assert_eq!(s.ports[Port::Mouse.index()].effective, PortMode::Normalized, "HID++ alone doesn't force mirroring");
        assert_eq!(s.ports[Port::Aux2.index()].state, PortState::Passthrough);
        assert_eq!(s.products[Port::Mouse.index()], "Gaming mouse (simulated)");

        c.switch(Target::Pc2).unwrap();
        let ev = c.events(Duration::from_millis(50)).unwrap();
        assert!(ev.contains(&Event::Switched(Pc::Pc2, SwitchSource::Software)));

        std::thread::sleep(Duration::from_millis(1100));
        let rate = c.port(Port::Mouse).unwrap().report_rate_hz;
        assert!((800..=1100).contains(&rate), "mouse rate {rate}");

        // The simulator acts out the key press on the KVM.
        c.update_firmware(&[0x55; 3000], |_, _, _| {}).unwrap();

        let v = c.set_config(ConfigValue::Mode(Device::Mouse, PortMode::Mirrored)).unwrap();
        assert_eq!(v, ConfigValue::Mode(Device::Mouse, PortMode::Mirrored));
        assert_eq!(c.port(Port::Mouse).unwrap().effective, PortMode::Mirrored);
    }

    #[test]
    fn the_simulated_kvm_is_healthy_in_its_diagnostics() {
        use kvm_proto::ctrl::diag::{Grade, id};
        let mut c = Client::new(Box::new(SimTransport::new()));
        let items = c.diag().unwrap();
        assert!(items.len() > kvm_proto::ctrl::diag::PER_PAGE, "more than one page");
        let bad: Vec<_> = items.iter().filter(|i| matches!(i.grade, Grade::Warn | Grade::Bad)).collect();
        assert!(bad.is_empty(), "{bad:?}");
        let usb = items.iter().find(|i| i.id == id::CLK_USB).unwrap();
        assert_eq!(crate::names::diag_value(usb), "48.000 MHz");
        let t = items.iter().find(|i| i.id == id::AT1_3V3).unwrap();
        assert_eq!(crate::names::diag_value(t), "3.29 V");
    }
}
