//! OpenDynKVM firmware for the AT32F405s: the USB device each PC sees (software.md §5).
//!
//! One image serves both chips; the PC13 strap says which PC this one faces.
#![no_std]
#![no_main]

mod adc;
mod board;
mod clocks;
mod link;
mod service;
mod time;
mod usb;

use core::sync::atomic::Ordering;

use defmt::{info, unwrap, warn};
use embassy_executor::Spawner;
use embassy_futures::select::select;
use embassy_time::{Duration, Instant, Ticker};
use embassy_usb_driver::{Endpoint as _, EndpointIn as _, EndpointOut as _};
use embassy_usb_synopsys_otg as otg;
use kvm_proto::link::LinkMsg;
use kvm_proto::link::msg::{Status, UsbSpeed, UsbState};
use kvm_usbdev::UsbCore;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

pub const FW_VERSION: u32 = {
    let (major, minor, patch) = kvm_proto::FIRMWARE_VERSION;
    kvm_proto::version_u32(major, minor, patch)
};

/// Marks the version in the image, where the RP2354B's build script finds it when it embeds this firmware: it
/// reflashes an AT32F405 whose `Hello` reports another version (software.md §4.9).
#[repr(C)]
struct FwId {
    magic: [u8; 8],
    version: u32,
}

#[used]
static FW_ID: FwId = FwId { magic: *b"KVMAT32\x01", version: FW_VERSION };

type Core = UsbCore<'static, usb::At32Driver>;

#[embassy_executor::task]
async fn usb_task(core: &'static mut Core) -> ! {
    let mut host = usb::Host::new();
    core.run(&mut host).await
}

#[embassy_executor::task(pool_size = 7)]
async fn ep_in(mut ep: otg::Endpoint<'static, otg::In>, index: usize) -> ! {
    loop {
        let r = usb::IN_QUEUES[index].receive().await;
        // A disabled endpoint (not configured yet) drops the report.
        let _ = ep.write(&r).await;
    }
}

#[embassy_executor::task(pool_size = 7)]
async fn ep_out(mut ep: otg::Endpoint<'static, otg::Out>, index: usize) -> ! {
    let mut buf = [0u8; 64];
    loop {
        ep.wait_enabled().await;
        if let Ok(n) = ep.read(&mut buf).await {
            link::send(&LinkMsg::Report { ep: index as u8 + 1, data: &buf[..n] }).await;
        }
    }
}

/// Without the PC's 5 V this long, the PC hub goes into reset; a short dip during a reboot doesn't count.
const HUB_HOLD_AFTER: Duration = Duration::from_secs(1);

/// Status to the RP2354B every 100 ms and whenever something changes. Also holds the PC hub in reset while the PC
/// gives no 5 V (`board::hold_hub`), and lets it go as soon as the 5 V is back.
#[embassy_executor::task]
async fn status_task() -> ! {
    let mut ticker = Ticker::every(Duration::from_millis(100));
    let mut vbus_gone_at: Option<Instant> = None;
    let mut hub_held = false;
    loop {
        select(ticker.next(), usb::STATUS_DIRTY.wait()).await;
        let port = adc::read_port();
        // Only a measured absence holds the hub: an ADC that doesn't convert must not cut the PC off.
        vbus_gone_at = if port.no_vbus() { Some(vbus_gone_at.unwrap_or_else(Instant::now)) } else { None };
        let hold = vbus_gone_at.is_some_and(|t| t.elapsed() >= HUB_HOLD_AFTER);
        if hold != hub_held {
            hub_held = hold;
            board::hold_hub(hold);
            info!("PC hub {}", if hold { "held in reset: no 5 V from the PC" } else { "released" });
        }
        let usb_state = UsbState::from_u8(usb::USB_STATE.load(Ordering::Relaxed)).unwrap_or(UsbState::Detached);
        // No VBUS from the PC means unpowered, whatever the core last saw.
        let usb_state = if port.no_vbus() { UsbState::Unpowered } else { usb_state };
        let speed = match usb_state {
            UsbState::Default | UsbState::Addressed | UsbState::Configured | UsbState::Suspended => {
                if usb::is_high_speed() {
                    UsbSpeed::High
                } else {
                    UsbSpeed::Full
                }
            }
            _ => UsbSpeed::Unknown,
        };
        let (vdda_mv, temp_dc) = adc::read_internal();
        let s = Status {
            usb_state,
            speed,
            vbus_mv: port.vbus_mv.unwrap_or(0),
            cc1_mv: port.cc1_mv,
            cc2_mv: port.cc2_mv,
            frame: if usb::running() { usb::frame_number() } else { 0 },
            resets: usb::RESETS.load(Ordering::Relaxed),
            desc_crc: usb::ACTIVE_CRC.load(Ordering::Relaxed),
            wake_enabled: usb::WAKE_ENABLED.load(Ordering::Relaxed),
            vdda_mv,
            temp_dc,
        };
        let _ = link::try_send(&LinkMsg::Status(s));
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let cp = unwrap!(cortex_m::Peripherals::take());
    let crystal = clocks::init();
    time::init(cp.SYST);
    let role = board::init();
    adc::init();
    // Any bits that differ between boots: the low byte of the HICK calibration and the ADC noise.
    let seed = (adc::read_mv(adc::CH_CC1).unwrap_or(0) as u8) ^ (cortex_m::peripheral::SYST::get_current() as u8);
    link::init(seed);
    info!("AT32F405 for PC{}, firmware {=u32:#x}", role, FW_VERSION);
    if !crystal {
        // The HS PHY needs the crystal: no USB. The link and the status still run, and Hello says why.
        warn!("12 MHz crystal not running: on the HICK, USB off");
        spawner.spawn(unwrap!(service::run(role)));
        spawner.spawn(unwrap!(status_task()));
        spawner.spawn(unwrap!(usb::sink()));
        return;
    }
    usb::set_running();

    static EP_OUT_BUF: StaticCell<[u8; 64 * 8]> = StaticCell::new();
    static CORE: StaticCell<Core> = StaticCell::new();
    let driver = usb::At32Driver::new(EP_OUT_BUF.init([0; 64 * 8]));
    let (core, eps) = unwrap!(UsbCore::new(driver).ok());
    let core = CORE.init(core);

    for (i, ep) in eps.ins.into_iter().enumerate() {
        spawner.spawn(unwrap!(ep_in(ep, i)));
    }
    for (i, ep) in eps.outs.into_iter().enumerate() {
        spawner.spawn(unwrap!(ep_out(ep, i)));
    }
    spawner.spawn(unwrap!(service::run(role)));
    spawner.spawn(unwrap!(status_task()));
    spawner.spawn(unwrap!(usb_task(core)));
}
