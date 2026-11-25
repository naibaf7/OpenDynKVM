//! The ESP32-C3's Bluetooth LE controller (esp-radio's, over VHCI) as the host stack's HCI. The ESP32's firmware has
//! its own (`firmware/esp32/src/radio.rs`), which also carries its Classic host.

use embassy_executor::Spawner;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::ExternalController;

pub type Controller = ExternalController<BleConnector<'static>, 20>;

/// The controller, for `activities` at once (connections, advertising, scanning), at +9 dBm.
pub fn controller(_spawner: Spawner, bt: esp_hal::peripherals::BT<'static>, activities: u8) -> Controller {
    let config = esp_radio::ble::Config::default()
        .with_max_connections(activities)
        .with_default_tx_power(esp_radio::ble::TxPower::P9);
    ExternalController::new(BleConnector::new(bt, config).expect("Bluetooth controller"))
}
