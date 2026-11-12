//! Engine timers and reboots.

use core::sync::atomic::Ordering;

use embassy_futures::select::{Either, select};
use embassy_time::{Instant, Timer};
use kvm_proto::ctrl::RebootTarget;

use crate::link::{self, LinkCtl};
use crate::{kvm, storage};

/// Runs the engine's `tick` (hotkey replays, report rates, autosave) at least every 100 ms.
#[embassy_executor::task]
pub async fn ticker() -> ! {
    loop {
        let now = Instant::now().as_millis() as u32;
        let errors = link::LINK_ERRORS.each_ref().map(|c| c.load(Ordering::Relaxed).min(u16::MAX as u32) as u16);
        let next = kvm::with(|e| {
            e.set_link_errors(errors);
            e.tick(now);
            e.deadline()
        });
        let wait = next.map_or(100, |d| d.saturating_sub(now).clamp(1, 100));
        Timer::after_millis(wait as u64).await;
    }
}

#[embassy_executor::task]
pub async fn reboots() -> ! {
    loop {
        let t = match select(kvm::REBOOT.wait(), Timer::after_secs(3600)).await {
            Either::First(t) => t,
            Either::Second(()) => continue,
        };
        // Let the response reach the PC first.
        Timer::after_millis(50).await;
        match t {
            RebootTarget::Normal => cortex_m::peripheral::SCB::sys_reset(),
            RebootTarget::Bootloader => {
                // RP2350 ROM reboot (datasheet §5.4.8.24): REBOOT_TYPE_BOOTSEL, both interfaces enabled.
                const REBOOT_TYPE_BOOTSEL: u32 = 0x0002;
                // The UF2 drive appears on the PWR/SERVICE port: SVC_SEL high points the USB there on rev 8 (the service
                // connector is on the switch's USB2 channel). Driving it high here covers only the moment until the
                // ROM takes over; in the bootloader its pull-up alone holds it, which needs 0.17.0's 10k: on boards up
                // to 0.16.0 the pad's own pull-down wins and the drive doesn't appear (hardware/errata.md E2, tried on a
                // 0.12.0 board).
                kvm::with_board(|b| b.svc_sel.set_high());
                embassy_rp::rom_data::reboot(REBOOT_TYPE_BOOTSEL, 10, 0, 0);
            }
            // The link loop owns the AT32F405's lines; it holds BOOT0 across a reset, and the ROM then offers USB
            // DFU to that PC (software.md §4.9) until the next reset.
            RebootTarget::At32Dfu1 => link::CTL[0].signal(LinkCtl::Dfu),
            RebootTarget::At32Dfu2 => link::CTL[1].signal(LinkCtl::Dfu),
            // Both AT32F405s get up to 5 more programming attempts: the count goes to flash before the restart.
            RebootTarget::At32Retry => {
                for k in 0..2 {
                    storage::set_at32_fails(k, 0).await;
                }
                cortex_m::peripheral::SCB::sys_reset();
            }
            // The same for the Bluetooth module (software.md §4.13).
            RebootTarget::BtRetry => {
                storage::set_bt_fails(0).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
    }
}
