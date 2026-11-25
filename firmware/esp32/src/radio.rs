//! The ESP32's controller (esp-radio's, dual mode) shared by two hosts: trouble-host for Bluetooth LE, which takes it
//! as its HCI through [`Split`], and the Classic host ([`crate::classic`]), which gets what [`Split`] takes out on the
//! way ([`crate::classic::take`]): Classic's events, its connections' ACL data, and its share of Number of Completed
//! Packets. Both send their commands and data through the one controller: bt-hci's `ExternalController` matches each
//! command with its answer, whichever host sent it, and the answers are read while trouble-host reads.

use bt_hci::cmd::controller_baseband::SetEventMask;
use bt_hci::cmd::{self, AsyncCmd, Cmd, SyncCmd};
use bt_hci::controller::{Controller as HciController, ControllerCmdAsync, ControllerCmdSync};
use bt_hci::{ControllerToHostPacket, data};
use embassy_executor::Spawner;
use embedded_io::ErrorType;
use esp_radio::ble::controller::BleConnector;
use static_cell::StaticCell;
use trouble_host::prelude::ExternalController;

pub type Hci = ExternalController<BleConnector<'static>, 20>;
pub type Controller = Split;

static HCI: StaticCell<Hci> = StaticCell::new();

/// The controller as the LE stack sees it: Classic's traffic taken out.
pub struct Split {
    hci: &'static Hci,
}

/// The controller's task's stack. ESP-IDF gives it 3.5 KB and runs interrupts on a stack of their own; under esp-rtos
/// every interrupt runs on the stack of the task it interrupts, so this one also holds the controller's interrupt
/// (its ROM's handlers, at an ESP-IDF interrupt stack's 1.5 KB) and the link's executor (`link::run`, 1.5 KB measured
/// with `-Zemit-stack-sizes`), each with its trap frame. esp-radio's 4 KB overran into the heap block below it once a
/// connection came up, LE or Classic (2026-11-24: a jump to an address made of a packet's bytes).
const CONTROLLER_STACK: u16 = 12 * 1024;

/// The controller, for `activities` LE activities at once (the ESP32's controller takes 9 at most), and the Classic
/// host started on it.
pub fn controller(spawner: Spawner, bt: esp_hal::peripherals::BT<'static>, activities: u8) -> Controller {
    let config = esp_radio::ble::Config::default()
        .with_max_connections(activities.min(9))
        .with_task_stack_size(CONTROLLER_STACK);
    let hci: &'static Hci =
        HCI.init(ExternalController::new(BleConnector::new(bt, config).expect("Bluetooth controller")));
    #[cfg(feature = "classic")]
    spawner.spawn(crate::classic::run(hci).expect("Classic task"));
    #[cfg(not(feature = "classic"))]
    let _ = spawner;
    Split { hci }
}

impl ErrorType for Split {
    type Error = <Hci as ErrorType>::Error;
}

impl HciController for Split {
    type Buffer<'a> = <Hci as HciController>::Buffer<'a>;

    fn alloc_buf(&self) -> Result<Self::Buffer<'_>, Self::Error> {
        self.hci.alloc_buf()
    }

    async fn write_acl_data(&self, packet: &data::AclPacket<'_>) -> Result<(), Self::Error> {
        self.hci.write_acl_data(packet).await
    }

    async fn write_sync_data(&self, packet: &data::SyncPacket<'_>) -> Result<(), Self::Error> {
        self.hci.write_sync_data(packet).await
    }

    async fn write_iso_data(&self, packet: &data::IsoPacket<'_>) -> Result<(), Self::Error> {
        self.hci.write_iso_data(packet).await
    }

    async fn read<'a>(&self, buf: &'a mut Self::Buffer<'_>) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        let buf: *mut Self::Buffer<'_> = buf;
        loop {
            // SAFETY: one borrow at a time: a packet the Classic host takes is copied and dropped before the next read,
            // and only the packet returned outlives the loop.
            let p = self.hci.read(unsafe { &mut *buf }).await?;
            if !(cfg!(feature = "classic") && crate::classic::take(&p)) {
                return Ok(p);
            }
        }
    }
}

impl<C: SyncCmd + ?Sized> ControllerCmdSync<C> for Split
where
    Hci: ControllerCmdSync<C>,
{
    async fn exec(&self, c: &C) -> Result<C::Return, cmd::Error<Self::Error>> {
        let r = <Hci as ControllerCmdSync<C>>::exec(self.hci, c).await;
        // The LE stack set the controller up (its Reset comes before this): the Classic host's turn.
        if C::OPCODE == <SetEventMask as Cmd>::OPCODE {
            crate::classic::le_ready();
        }
        r
    }
}

impl<C: AsyncCmd + ?Sized> ControllerCmdAsync<C> for Split
where
    Hci: ControllerCmdAsync<C>,
{
    async fn exec(&self, c: &C) -> Result<(), cmd::Error<Self::Error>> {
        <Hci as ControllerCmdAsync<C>>::exec(self.hci, c).await
    }
}
