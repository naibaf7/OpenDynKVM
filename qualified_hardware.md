# Qualified hardware

Peripherals and hosts that have been tried with the KVM, by hardware type. "Passthrough" means the device's own
USB descriptors reach the host; "normalized" means the KVM re-presents it as a standard device
([software.md](software.md)). Entries are what was observed on the bench; anything not listed is untested, not
unsupported.

Bluetooth devices are always normalized. The two module variants differ: the ESP32 module speaks Bluetooth
Classic and BLE, the ESP32-C3 module BLE only.

## Gamepads

| Device | Connection | Passthrough | Normalized | Notes |
|---|---|---|---|---|
| Xbox 360 wireless receiver + gamepad | USB receiver | works | works | One controller at most |
| PS5 controller (DualSense) | USB cable | works | works | Rumble not tested in normalized mode |
| PS5 controller (DualSense) | Bluetooth | n/a | works | ESP32 module (Bluetooth Classic) only, not the ESP32-C3 (BLE) variant. No rumble, no audio |

## Mice

| Device | Connection | Passthrough | Normalized | Notes |
|---|---|---|---|---|
| MX518 | USB cable | works | works | |
| G502X Lightspeed | USB receiver | works | works | |
| Pebble M350s | Bluetooth (BLE) | n/a | works | |

## Keyboards

| Device | Connection | Passthrough | Normalized | Notes |
|---|---|---|---|---|
| K380s | Bluetooth (BLE) | n/a | works | |

## Hosts over Bluetooth

| Host | Result |
|---|---|
| Galaxy S25 | works |
| Legion Go | works |
