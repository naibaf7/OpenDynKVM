# OpenDynKVM: specification

The requirements the design answers, then what the design delivers. How and why: [design.md](design.md) (hardware)
and [software.md](software.md) (firmware and host software).

## Requirements

- **Ports:** three USB-C ports at the back (PC1, PC2 and power) and three USB-A ports at the front (keyboard, mouse,
  one other peripheral).
- **Power:** the power port is optional. Without it, the KVM takes its 5 V from PC1.
- **USB:** USB 2.0, with gaming peripherals at up to 1000 Hz.
- **Persistent emulation:** both PCs see the keyboard, the mouse and the other peripheral as permanently connected, so
  switching adds no latency and nothing re-enumerates.
- **Switching:** a tactile key on the device, or a selectable key or key combination on the keyboard, which the KVM
  intercepts.

## What the design delivers

| | |
|---|---|
| **PC ports** | 2 × USB-C, USB 2.0 High-Speed. Each PC sees a 4-port hub with the KVM's own composite device on it, always present ([design.md §1](design.md)) |
| **Power** | USB-C PWR/SERVICE (optional, takes priority), otherwise **either** PC's port, through ideal diodes. This deviates from the requirement on purpose: with power from PC1 only, PC2 would lose its keyboard and mouse whenever PC1 is off ([design.md §6](design.md)) |
| **Power draw** | about 0.5 A from 5 V for the electronics, plus the peripherals. A PC's USB 2.0 port (500 mA) is not enough; a USB 3 port leaves about 0.4 A for the peripherals, a 3 A USB-C source about 2.5 A |
| **Front ports** | 3 × USB-A (Keyboard, Mouse, Other), each with its own current-limited 5 V switch (about 1 A) and a 1:3 analog USB switch: to the KVM's host, or straight to PC1's or PC2's hub |
| **Port modes** | Normalized (the KVM's own keyboard and mouse), Mirrored (the device's HID interfaces, copied), Mirrored + identity (also its VID/PID and name), Passthrough (the real device, on one PC) and Auto, which picks one per device ([design.md §4](design.md)) |
| **Switching time** | under 1 ms in the emulated modes; about 1 s in passthrough, where the device re-enumerates |
| **Added latency** | about 0.1 ms on average (≈ 36 µs in the KVM + 0–125 µs until the PC's next High-Speed poll) ([software.md §3](software.md)) |
| **Report rate** | the device's own rate up to 1000 Hz in the emulated modes (the KVM's host runs at full speed, so an 8 kHz High-Speed mouse runs at 1000 Hz there); the device's full rate in passthrough |
| **Switching from** | the mechanical key (tap: switch PC; long press: next profile), a hotkey (default: double-tap Scroll Lock, which neither PC sees), the host software or its command line |
| **Profiles** | 1 to 4, each setting the mode of all three ports; out of the box *work* (all ports Auto) and *gaming* (all Passthrough) |
| **Keymap** | remapping on the KVM with Vial: 8 layers, mod-tap and layer-tap, one-shot modifiers, media and mouse keys, 16 macros ([software.md §4.11](software.md)) |
| **Monitors** | switch along over DDC/CI, sent by the host software from the PCs; no video passes through the KVM |
| **Screen hop** | one mouse across both PCs' screens: the KVM moves an absolute pointer and switches PCs at a screen edge, no software needed on the PCs ([software.md §4.14](software.md)) |
| **Wireless** (optional, boards from 0.16.0) | an ESP32-C3 module that connects the keyboard and mouse to up to 4 Bluetooth LE hosts besides the two PCs ([design.md §16](design.md)) |
| **Standby** | dark LEDs and front ports off when no PC uses the KVM; a key press wakes the sleeping active PC |
| **Indicators** | an RGB LED per PC and per front port, and in the key |
| **Firmware updates** | over USB from either PC, A/B with automatic rollback; first flash and recovery over PWR/SERVICE with only a cable ([design.md §10](design.md)) |
| **Host software** | Windows 10 and 11; Linux (Debian 12, Ubuntu 22.04 and later, Fedora, openSUSE). In English, French, German, Norwegian Bokmål and Simplified Chinese |
| **Main parts** | RP2354B (USB host, switching logic), 2 × AT32F405 (one High-Speed USB device per PC), 3 × USB2514B hub, 3 × TS3USB3031 1:3 switch, TS3USB30E service switch ([design.md §5](design.md)) |
| **Board** | 120 × 80 mm, 4 layers, assembled on both sides by JLCPCB; only the key switch is plugged in by hand ([hardware/jlcpcb.md](hardware/jlcpcb.md)) |
