# OpenDynKVM: odkvmctl and opendyn-kvm, English (the fallback: every message is here, and `fl!` checks against it).

## Branding. Product and feature names are terms: change them here, and every message follows.
## -wireless is the Bluetooth feature. Until the product is listed with the Bluetooth SIG it is called "wireless";
## once listed, make it "Bluetooth" (both forms). Message ids must not repeat a term's name (Fluent keeps one of them).

-product = OpenDynKVM
-wireless = { $case ->
   *[title] Wireless
    [lower] wireless
}
## Where a phone or PC pairs a keyboard: the host's own menu, so it keeps its own name.
-host-pairing-settings = Bluetooth settings
## -hop is screen hop: the KVM moves the pointer across both PCs' screens and switches PCs at a screen edge.
-hop = { $case ->
   *[title] Screen hop
    [lower] screen hop
}

product-name = { -product }

## Words used in many places

yes = yes
no = no
on = on
off = off

## Errors the KVM answers with

err-unknown = unknown request
err-malformed = malformed request
err-locked = turned off at the KVM (turn it back on with `odkvmctl config set software-config on` and a press on the KVM's key, or hold the key while powering the KVM up)
err-busy = busy, try again
err-unsupported = not supported by this firmware
err-invalid-value = invalid value
err-needs-key-press = press the key on the KVM to confirm
err-no-device = no device on that port
err-not-active = only the active PC can do that: switch the KVM to this PC first

## Learning a hotkey

learn-learned = learned
learn-timed-out = nothing that makes a hotkey was pressed in time
learn-cancelled = stopped (the KVM switched PCs, or learning was cancelled)
hotkey-warn-twin = It is the { $name } hotkey too: one press would do both.
hotkey-warn-alone = { $key } by itself would switch, and the PCs would no longer get it.

## Turning software control off

lock-warn-switching = Neither PC will be able to switch the KVM from software: not with odkvmctl, the app or its tray icon. The key on the KVM and the keyboard hotkeys keep working.
lock-warn-config = Neither PC will be able to change any setting from software: no port modes, profiles, hotkeys, firmware updates or restarts, and Vial becomes read-only. The status stays visible.
lock-warn-back-on = Turning it back on takes the key on the KVM: a press within { $seconds } s of asking for it, or holding the key while the KVM powers up.

## Link to a PC

link-unpowered = off
link-detached = not connected
link-enumerating = connecting
link-connected = connected
link-suspended = asleep
link-down = link down

## Front ports

port-state-empty = empty
port-state-probing = detecting
port-state-hosted = active
port-state-passthrough = passthrough
port-state-unpowered = unpowered
port-state-fault = fault
port-state-unsupported = unsupported

mode-auto = Auto
mode-normalized = Normalized
mode-mirrored = Mirrored
mode-mirrored-identity = Mirrored + identity
mode-passthrough = Passthrough (follows the active PC)
mode-passthrough-pc1 = Passthrough, pinned to PC1
mode-passthrough-pc2 = Passthrough, pinned to PC2
mode-short-auto = Auto
mode-short-normalized = Normalized
mode-short-mirrored = Mirrored
mode-short-identity = Identity
mode-short-passthrough = Passthrough
mode-short-pinned-pc1 = Pinned PC1
mode-short-pinned-pc2 = Pinned PC2

reason-not-auto = set by you
reason-fits-normalized = all inputs fit the normalized interfaces
reason-extra-inputs = has inputs the normalized interfaces can't carry
reason-other-port-hid = HID device on the Other port
reason-non-hid = has non-HID interfaces
reason-xinput = XInput gamepad
reason-endpoint-budget = not enough endpoints left to mirror it
reason-identity-taken = another port already uses identity

speed-low = low-speed
speed-full = full-speed
speed-high = high-speed

class-keyboard = keyboard
class-mouse = mouse
class-consumer = media keys
class-gamepad = gamepad
class-xinput = XInput
class-vendor-hid = vendor inputs
class-vendor-channel = vendor commands
class-other-hid = other HID
class-audio = audio
class-video = video
class-storage = storage
class-hub = hub

## The KVM's key

key-action-none = Nothing
key-action-toggle-pc = Switch to the other PC
key-action-next-profile = Next profile
key-action-other-port = Cycle the Other port's mode
key-action-next-target = Next target (PCs, then { -wireless(case: "lower") } hosts)

## Power

adv-none = none
adv-default = USB default
adv-1a5 = 1.5 A
adv-3a = 3 A
power-source-pwr = PWR port
power-source-pc = PC USB

## Firmware update

fw-state-idle = idle
fw-state-receiving = uploading
fw-state-awaiting-key = press the key on the KVM to install
fw-state-writing = installing
fw-state-done = done
fw-state-failed = failed

## The wireless module

bt-module-up = up
bt-module-starting = starting
bt-module-programming = being updated
bt-module-failed = doesn't run its firmware: { -wireless(case: "lower") } off until the next power-up
bt-module-gave-up = gave up after 5 attempts: { -wireless(case: "lower") } off
bt-module-absent = not fitted

## Self-test

selftest-hub0 = Hub-0 (front ports) not found
selftest-link1 = no link to the PC1 chip
selftest-link2 = no link to the PC2 chip
selftest-hub1 = PC1 hub not working
selftest-hub2 = PC2 hub not working
selftest-port-kb = keyboard port power switch
selftest-port-ms = mouse port power switch
selftest-port-ot = other port power switch
selftest-power = PWR port readings implausible
selftest-at32-fw1 = programming the PC1 chip failed (tried again at the next power-up)
selftest-at32-fw2 = programming the PC2 chip failed (tried again at the next power-up)
selftest-at32-gave-up1 = gave up programming the PC1 chip after 5 attempts
selftest-at32-gave-up2 = gave up programming the PC2 chip after 5 attempts
selftest-at32-xtal1 = the PC1 chip's crystal doesn't run (no USB to PC1)
selftest-at32-xtal2 = the PC2 chip's crystal doesn't run (no USB to PC2)
selftest-bt-fw = the { -wireless(case: "lower") } module doesn't run its firmware (tried again at the next power-up)
selftest-bt-gave-up = gave up programming the { -wireless(case: "lower") } module after 5 attempts
selftest-running = running
selftest-passed = passed
selftest-running-so-far = running, so far: { $failed }
selftest-failed = FAILED: { $failed }

## Why the KVM switched

switch-source-key = key on the KVM
switch-source-hotkey = hotkey
switch-source-software = software
switch-source-boot = start
switch-source-power = the active PC went off

## Diagnostics

diag-group-system = System
diag-group-clocks = Clocks
diag-group-usb = USB
diag-group-power = Power
diag-group-pc-chips = PC-side chips
diag-group-wireless = { -wireless }
diag-group-other = Other

diag-uptime = Uptime
diag-reset-cause = Last reset
diag-rp-temp = RP2354B temperature
diag-selftest = Self-test
diag-dropped = Engine outputs dropped
diag-fw-updates = Firmware updates over USB
diag-xosc = Crystal (XOSC)
diag-pll-sys = System PLL
diag-pll-usb = USB PLL
diag-rosc = Ring oscillator
diag-at-clock = AT32F405 #{ $n } clock
diag-hub0-speed = Hub-0 link
diag-hub0-up = Hub-0 enumerated
diag-phy-swap = D+/D− swap (errata E1)
diag-kb-speed = Keyboard port device
diag-ms-speed = Mouse port device
diag-ot-speed = Other port device
diag-pc-speed = { $pc } USB speed
diag-pc-link = { $pc } link
diag-pc-resets = { $pc } bus resets
diag-link-errors = AT32F405 #{ $n } SPI errors
diag-hub0-errors = Hub-0 changes not handled
diag-enum-fails = Front-port enumerations failed
diag-vbus = { $port } VBUS
diag-cc = { $port } CC
diag-at-3v3 = 3.3 V at AT32F405 #{ $n }
diag-budget = Front-port budget
diag-kb-faults = Keyboard port faults
diag-ms-faults = Mouse port faults
diag-ot-faults = Other port faults
diag-at-temp = AT32F405 #{ $n } temperature
diag-at-fw = AT32F405 #{ $n } firmware
diag-bt-module = ESP32-C3 module
diag-bt-fw = ESP32-C3 firmware
diag-bt-link-errors = ESP32-C3 UART errors
diag-bt-hosts = { -wireless } hosts connected
diag-unknown = item { $id }

reset-software = restart from software
reset-power-on = power-on
reset-brown-out = brown-out (the supply dipped)
reset-glitch = supply glitch
reset-run-pin = RUN pin
reset-watchdog = watchdog (also after an update)
reset-debugger = debugger
reset-other = cause { $cause }
hz-stopped = 0 Hz (stopped)
clock-crystal = 12 MHz crystal
clock-internal = internal oscillator: no USB
swap-found = { $state }, found at run time
fw-updates-on = on, firmware in partition { $partition }
fw-updates-no-table = off: no partition table (flashed by a debug probe)
fw-updates-read-failed = off: the boot ROM didn't return the partition table ({ $detail })
fw-updates-other-table = off: another partition table ({ $detail })
fw-updates-other-layout = off: the partitions differ from this firmware's ({ $detail })
fw-updates-unknown-partition = off: unknown which partition runs ({ $detail })
state-other = state { $state }

diag-help-reset-cause = Orange after a brown-out or a supply glitch: the 5 V dipped. Restarts on purpose have no colour.
diag-help-rp-temp = Orange from 70 °C, red from 85 °C.
diag-help-selftest = Red when a part of the KVM doesn't work; orange for a front port's power switch.
diag-help-dropped = Work the firmware had no room for. Orange when not 0.
diag-help-fw-updates = Whether the KVM takes firmware updates over a PC port. Orange when it doesn't: the code in brackets says what its check read, worth a bug report. No colour on a board flashed by a debug probe.
diag-help-xosc = The 12 MHz crystal; red when it doesn't run.
diag-help-clk-sys = Counted against the crystal. Red when 1 % off, or not 10 % above clk_usb (RP2350-E12).
diag-help-clk-usb = Counted against the crystal. Red when more than 0.25 % off 48 MHz (USB full speed).
diag-help-clk-adc = Red when more than 1 % off 48 MHz.
diag-help-at-clock = Red when the chip runs without its crystal: then it has no USB to its PC.
diag-help-hub0-speed = The front ports' hub. Red as low speed: D+ and D− are crossed for the PHY's setting (errata E1).
diag-help-phy-swap = Whether the RP2354B's USB swaps D+ and D− (errata E1): on for boards 0.8 to 0.12.
diag-help-pc-speed = Orange at full speed: 125 µs polling needs high speed. Try another cable or port.
diag-help-pc-link = Red when the PC-side chip doesn't answer; orange while the PC hasn't set it up.
diag-help-link-errors = Failed exchanges with the PC-side chip since start, once it has answered for the first time.
diag-help-hub0-errors = Port changes Hub-0 reported that the KVM's USB host doesn't handle, cleared so they don't hide the other ports. A few at most; more is worth a bug report.
diag-help-enum-fails = Devices on the front ports the KVM couldn't read, each tried again a few times by switching the port off and on. Unplugging a device while it is read counts too.
diag-help-vbus = Good 4.75–5.50 V; orange 4.40–4.75 V (sagging) or up to 5.70 V; red beyond. No colour under 0.8 V: nothing supplies it.
diag-help-cc = Red above 2.3 V (5 V on CC: cable or contact). Orange below 0.2 V with 5 V there: a cable without Rp.
diag-help-3v3 = Good 3.135–3.465 V (±5 %), against a ±3.3 % reference; orange to 3.0 / 3.6 V.
diag-help-budget = Orange when the attached devices may draw more than the supply offers: use the PWR port.
diag-help-faults = Over-current trips of the port's power switch.
diag-help-at-temp = Orange from 85 °C, red from 105 °C. Off by up to 20 °C from chip to chip.
diag-help-at-fw = Orange when it differs from the RP2354B's: being reprogrammed, or programming failed.
diag-help-bt-module = Orange while the KVM programs it (after an update, 10 to 20 s). Red when it doesn't run its firmware this boot (tried again at the next power-up), or after 5 failed attempts in a row: then only holding the KVM's key at power-up or Firmware → Try again makes it try again.
diag-help-bt-fw = The module runs the firmware the KVM carries: an update of the KVM updates it too.
diag-help-bt-link-errors = Failed exchanges with the module since start, including its programming.
diag-help-bt-hosts = Paired hosts that are connected now and can take input.

## Settings typed on the command line

parse-colour = colour '{ $value }': use #rrggbb
parse-hotkey = hotkey '{ $value }': { $error } (try double:scrolllock or ctrl+alt+1)
parse-host-name = name '{ $value }': at most { $max } bytes (empty: Host n)
parse-profile-name = name '{ $value }': 1 to { $max } bytes
parse-mode = mode '{ $value }': { $modes }
parse-profile-modes = '{ $value }': three modes for keyboard, mouse and other, e.g. auto,auto,passthrough
parse-number = '{ $value }': a number up to { $max }
parse-one-of = '{ $value }': use { $choices }
parse-unknown-setting = unknown setting

## The window: sidebar and header

window-title-sim = { -product } (simulated)
simulated-kvm = simulated KVM
page-overview = Overview
page-ports = Ports
page-displays = Displays
page-keyboard = Keyboard
page-settings = Settings
page-firmware = Firmware
page-wireless = { -wireless }
light-mode = Light mode
dark-mode = Dark mode
no-kvm-connected = No KVM connected
leds = LEDs
active-pc = Active PC
profile = Profile
connected-to = Connected · { $device }
reading = Reading…
not-connected = Not connected

## Overview

chip-active = ACTIVE
chip-this-computer = this computer
pc-vbus = VBUS { $volts } V · offers { $offers }
showing = Showing
switch-here = Switch here
front-ports = Front ports
unnamed-device = (unnamed device)
host-has-input = { $host } gets the keyboard and mouse over { -wireless(case: "lower") }. Ports in passthrough stay with { $pc }.
power = Power
power-left = From { $source }, about { $milliamps } mA left for the front ports
budget-low = The attached devices may need more: use the PWR port.
selftest-line = Self-test { $result }
recent = Recent

## Ports

port-keyboard = Keyboard port
port-mouse = Mouse port
port-other = Other port
port-short-keyboard = Keyboard
port-short-mouse = Mouse
port-short-other = Other
redetect = Re-detect
mode = Mode
no-device = No device
running-mode = Running { $mode }
up-to-ma = up to { $milliamps } mA
over-current-faults = { $count ->
    [one] { $count } over-current fault
   *[other] { $count } over-current faults
}
profiles = Profiles
profiles-note = Each profile is a set of port modes, e.g. emulated devices for work and passthrough for games. A long press on the KVM's key selects the next one (Keyboard page). The ports below run the active profile.
name = Name
profiles-in-use = Profiles in use:
remember-profile = Power up in the profile last active (otherwise in profile 0, and choosing a profile writes nothing to the KVM's flash)
config-locked = Configuration from software is turned off at the KVM.
config-locked-how = To turn it back on: Settings → Software control, then press the key on the KVM.

## Keyboard

hotkeys = Hotkeys
hotkey-toggle = Toggle PC
hotkey-pc1 = Select PC1
hotkey-pc2 = Select PC2
hotkey-next = Next target
cant-learn-firmware = This KVM's firmware can't learn hotkeys: type it instead
cant-learn-not-active = Only the active PC can learn a hotkey: switch the KVM to this PC first
cant-learn-no-keyboard = No keyboard that the KVM reads is plugged in
learning-another = Learning another hotkey
learned-with-warning = { $warning } Apply uses it anyway.
learned = Learned from the keyboard. Apply uses it.
not-learned = Not learned: { $why }.
kvm-no-answer = The KVM didn't answer.
learn-hint-sim = Or press Learn, then the hotkey in this window: it plays the simulated KVM's keyboard.
learn-hint = Or press Learn, then the hotkey on the KVM's keyboard.
press-the-hotkey = Press the hotkey… { $seconds } s
apply = Apply
cancel = Cancel
learn = Learn
learn-hover = Press the hotkey on the KVM's keyboard instead of typing it
passthrough-keyboard-note = A keyboard in Passthrough goes straight to the PC: the KVM doesn't see it, so no hotkey works on it, and none is learned from it.
double-tap-window = ms double-tap window
kvm-key = The key on the KVM
tap = Tap
long-press = Long press
press-tap = tap
press-long-from = long press from { $seconds } s
held-for-long-press = Held for a long press:
kvm-key-note = Whatever is set here, a press confirms a firmware update waiting for it, and in standby the first press only wakes the KVM.
keymap-vial = Keymap (Vial)
keymap-note = Edit the keymap with the Vial app while its interface is shown. The keymap keeps working with the interface off. Changing the interface makes both PCs detect the KVM again (about 1 s).
vial-interface = Vial interface:
vial-off = Off
vial-auto = Auto (when no mirrored device needs the endpoint)
vial-always = Always
apply-keymap = Apply the keymap to keyboards the KVM normalizes

## Settings

this-computer = This computer
autostart = Start with the system, in the tray
could-not-change = Could not change it: { $error }
appearance = Appearance:
theme-system = System
theme-dark = Dark
theme-light = Light
language = Language:
language-system = System
brightness = brightness
pc-colour = { $pc } colour
pc-side = PC side
polling = Polling:
polling-125us = 125 µs (lowest latency)
polling-1ms = 1 ms (for picky BIOSes)
power-cycle = Power-cycle passthrough devices when they switch PCs
standby = When the PCs are off or asleep
standby-note = When no PC has used the KVM for 10 s, its LEDs go dark and the front ports switch off. A PC that turns on, or a press on the KVM's key, wakes it.
auto-select = Follow the PC that is on: when the active PC is switched off, the other one becomes active
standby-wake = Keep keyboard and mouse powered while a PC sleeps, so a key press wakes it
standby-glow = Let the key glow in standby (otherwise all LEDs are dark)
save-now = Save now
factory-settings = Factory settings
unsaved-changes = Unsaved changes are written in a few seconds.
software-control = Software control
software-control-note = Either PC can turn these off. Turning them back on takes the key on the KVM: a press within { $seconds } s of asking for it here, or holding the key while the KVM powers up.
allow-switching = Allow switching from software
allow-config = Allow configuration from software
not-turned-on = Not turned on: the key on the KVM wasn't pressed in time.
press-key-to-confirm = Press the key on the KVM to confirm ({ $seconds } s left). Its LED pulses white.
confirm-lock-switching = Turn off switching from software?
confirm-lock-config = Turn off configuration from software?
turn-off = Turn off

## Firmware

versions = Versions
wireless-module = { -wireless } module
hardware = Hardware
serial = Serial
uptime = Uptime
gave-up-note-chip = The KVM stopped trying after 5 failed attempts in a row, so it doesn't reprogram the chip at every power-up. It tries again with a new firmware, when its key is held while it powers up, or now:
try-again = Try again (the KVM restarts)
update = Update
image-file = Image file:
install = Install
press-key-to-install = Press the key on the KVM to install the update.
installed = Installed; the KVM restarts.
fw-file-title = Firmware file
fw-file-hint = a release's folder, kvm-rp2354.bin or a .uf2
browse = Browse…
fw-shipped = Shipped firmware
fw-shipped-note = shipped with this app
fw-file-is = Firmware { $version } for hardware { $major }.x
fw-same = The KVM runs this version already.
fw-newer = Newer than the KVM's { $kvm }.
fw-older = Older than the KVM's { $kvm }: installing it goes back.
fw-no-file = No firmware chosen: pick a file or a release's folder.
update-over-pc-note = Over this computer's port, to the KVM that runs. The new firmware starts on trial: if it doesn't come up, the KVM goes back to the old one.
pick-firmware = Choose the KVM's firmware
firmware-files = KVM firmware
boot-title = In the bootloader (PWR/SERVICE port)
boot-kvm = { -product } with firmware { $version }; an update goes into partition { $slot }.
boot-kvm-empty = { -product } without firmware; it goes into partition { $slot }.
boot-new = A new board: nothing on it yet. Setting it up writes the partition table and the firmware.
boot-new-unpartitioned = Firmware { $version }, but no partition table (flashed by a debug probe). Setting it up writes both.
boot-other = Not a { -product }: an RP2350 with another partition table or other firmware. Nothing is installed on it.
boot-unknown-note = Without it, this app can't tell a { -product } from a new board: install only on a { -product } that has firmware, and set up only a new one.
boot-drive = Drive: { $path }
boot-no-drive-note = Its drive isn't mounted: installing mounts it, or open it in the file manager first.
boot-set-up = Set up
boot-writes = Writes { $file }: firmware { $version } for hardware { $major }.x
boot-writing = writing
boot-restart = Restart without installing
boot-restart-hover = Leaves the bootloader: the board starts the firmware it has.
boot-installed = Installed: the board restarts with the new firmware. Connect it to the PCs to use it.
boot-none-title = Install over the PWR/SERVICE port
boot-none-note = For a new board, or one whose firmware doesn't start: connect this computer to the KVM's PWR/SERVICE port. A new board shows up here by itself; any other when its pinhole button is held while it is plugged in.
boot-found-hint = A { -product } waits in its bootloader on the PWR/SERVICE port.
open-firmware-page = Firmware page
identify = Identify (blink the LEDs)
diagnostics = Diagnostics
measuring = Measuring…
diag-bad = { $count } bad
diag-warnings = { $count ->
    [one] { $count } warning
   *[other] { $count } warnings
}
diag-all-good = all good
diag-readings = { $count } readings, { $seconds } s ago
copy = Copy
copy-hover = Copy the readings as text
looking-for-kvm = Looking for the KVM…
plug-in-kvm = Plug the KVM into this computer.
udev-hint = Installed from source? Then install software/odkvmctl/udev/70-opendyn-kvm.rules (DEVELOPMENT.md).

## Wireless hosts (the Bluetooth feature; its name is the -wireless term)

host-connecting = connecting…
host-hover = { -wireless }, { $state }
send = Send
cancel-pairing = Cancel pairing
host-pairing = A { -wireless(case: "lower") } host is pairing
passkey-text = The host shows six digits: type them on the KVM's keyboard and press Enter (Escape cancels), or enter them here. Until then the KVM's keyboard types nowhere else.
module-is = The module is { $state }.
module-programming-note = After a firmware update the KVM programs the module with the firmware it carries, which takes 10 to 20 s. { -wireless } hosts reconnect when it is done.
module-failed-note = { -wireless } is off until the KVM is powered up again, when it tries once more. The two PCs work as before.
gave-up-note-module = The KVM stopped trying after 5 failed attempts in a row, so it doesn't reprogram the module at every power-up. It tries again with a new firmware, when its key is held while it powers up, or now:
pair-new-host = Pair a new host
pair-note = Makes the KVM discoverable as "{ -product }" for a minute. Pair it from the PC, tablet or phone in its { -host-pairing-settings }, as a keyboard. If the host shows six digits, type them on the KVM's keyboard and press Enter.
pair = Pair
slots-full = Forget a host first: all four slots are in use
discoverable = Discoverable · { $seconds } s
stop = Stop
hosts = Hosts
hosts-note = { -wireless } hosts get the KVM's own keyboard and mouse (mirroring, identity and passthrough stay with the PCs). "Stay connected" makes switching instant; "Only while selected" suits phones and tablets that hide their on-screen keyboard while a keyboard is connected.
slot-free = free
active = active
forget = Forget
forget-hover = Removes the pairing. To pair it again, remove "{ -product }" on the host too.
link-every = Every { $ms } ms
link-may-skip = { $count ->
    [one] may skip { $count }
   *[other] may skip { $count }
}
link-signal = signal { $dbm } dBm
policy-stay = Stay connected
policy-on-select = Only while selected
a-host = a { -wireless(case: "lower") } host

## Tray

tray-not-connected = { -product }: not connected
tray-host-active = { -product }: { $host } active ({ -wireless(case: "lower") })
tray-pc-active = { -product }: { $pc } active
tray-switch-to = Switch to { $target }
tray-open = Open
tray-quit = Quit

## Recent events

ev-switched = Switched to { $target } ({ $source })
ev-port-changed = { $port } changed
ev-fault-port = { $port }: { $fault }
ev-fault = Fault: { $fault }
ev-setting-changed = Setting { $name } changed
ev-hotkey-learned = Hotkey learned: { $hotkey }
ev-hotkey-not-learned = Learning a hotkey: { $why }
ev-pairing-open = { -wireless } pairing open
ev-pairing-closed = { -wireless } pairing closed
ev-passkey-wanted = A { -wireless(case: "lower") } host asks for its passkey
ev-paired = Paired: { $host } (slot { $slot }; name it on the { -wireless } page)
ev-forgot = Forgot the { -wireless(case: "lower") } host { $host }
ev-host-connected = { $host } connected
ev-host-disconnected = { $host } disconnected
ev-switched-host = Switched to { $host } ({ -wireless(case: "lower") })
ev-monitor-not-switched = Monitor not switched: { $error }
fault-over-current = over-current
fault-enumeration = enumeration failed
fault-link-down = link down
fault-power-budget = not enough power
fault-hub = hub error
no-diagnostics = This KVM's firmware has no diagnostics yet: update it.
kvm-refused = The KVM refused: { $why }.

## Displays: monitors that follow the KVM (DDC/CI)

profile-n = Profile { $n }
display-follow = Follows the KVM
display-pc1 = Stays on PC1
display-pc2 = Stays on PC2
display-off = Not switched
input-none-pc = — not switched
input-none-host = — stays on the PC
display-switched = { $monitor } switched to { $input }
displays-title = Monitor inputs follow the KVM
displays-note = When the KVM switches PCs (or to a { -wireless(case: "lower") } host) or profiles, this computer switches the monitors below (DDC/CI): each profile can have a monitor follow the KVM, stay on one PC, or leave it alone. The KVM tells both PCs about every change, so set each monitor up on one PC if you can.
displays-two-pcs = The two PCs don't share these settings: a monitor set up on both gets every command twice, and follows neither set-up if they differ. One PC is enough if the monitor listens on an input it is not showing: "Show PC1" / "Show PC2" away from this PC and back tells. If it doesn't come back, set it up the same way on both PCs.
displays-ddc-on = DDC/CI must be on in the monitor's own menu.
look-again = Look for monitors again
asking-monitors = Asking the monitors…
this-computer-is = This computer is { $pc }
kvm-not-connected = KVM not connected
no-monitor = No monitor answered. Check that DDC/CI is on in the monitor's menu.
saved-not-connected = Saved, not connected now
input-other = other
input-other-hover = An input the monitor doesn't list, in hex (e.g. 0x0F), then Enter
use-current = Use current
use-current-hover = The input this monitor shows now
show-target = Show { $target }
monitor-shows = shows { $input }
not-answering = not answering
target-input = { $target } input
displays-hosts-note = { -wireless } hosts are optional: while one has the keyboard and mouse, a monitor that follows the KVM shows it if it has an input here, and stays on the PC otherwise.
in-each-profile = In each profile:
input-missing = A profile needs an input that isn't chosen above.
could-not-save = Could not save this computer's settings: { $error }
input-unknown = Input { $value }
ddc-no-answer = no answer (is DDC/CI on in the monitor's menu?)
ddc-garbled = garbled answer
ddc-unexpected = unexpected answer
ddc-unsupported-code = the monitor does not support it
ddc-not-connected = { $id }: not connected
ddc-unsupported-os = DDC/CI is only supported on Windows and Linux
ddc-no-access = { $path }: no access. Install the package, or the udev rule from the README, then log in again.
ddc-no-i2c-dev = { $path }: missing, the i2c-dev module is not loaded. Install the package, or load it as the README says.

## Talking to the KVM

error-timeout = the KVM did not answer
error-device = the KVM refused: { $why }
error-protocol = protocol error: { $error }
protocol-unexpected = unexpected answer { $answer }
protocol-update-ended = update ended in state { $state }
error-not-confirmed = the key on the KVM wasn't pressed in time: nothing changed
host-n = Host { $n }
hid-open-linux = { $error } (install software/odkvmctl/udev/70-opendyn-kvm.rules for access without root)
no-kvm-found = no { -product } found

## Firmware files

img-damaged-uf2 = damaged UF2 file
img-uf2-partition-table = this UF2 also holds the partition table (kvm-first.uf2); use kvm-rp2354.uf2 or kvm-rp2354.bin
img-uf2-other-chip = UF2 for another chip (family { $family })
img-uf2-out-of-range = UF2 block at { $address } out of range
img-too-big = { $kb } KB is more than a firmware partition holds ({ $max } KB)
img-not-rp2354b = not RP2354B firmware (no RP2350 image definition)
img-wrong-kind = RP2350 image of the wrong kind (flags { $flags })
img-no-fw-id = not { -product } firmware (no firmware ID)
img-other-hardware = firmware { $version } is for hardware { $major }.x, this KVM is hardware { $hw }
img-not-first = not a first-install UF2 (kvm-first.uf2: partition table and firmware)
fw-read = { $file }: { $error }
fw-not-in-folder = no { $name } in { $folder }
boot-no-picoboot = the boot ROM shows no PICOBOOT interface
boot-no-permission = no permission for the RP2350 boot ROM (USB 2e8a:000f): install the udev rule (software/odkvmctl/udev/70-opendyn-kvm.rules)
boot-no-access = can't open the RP2350 boot ROM: { $error }
boot-usb = USB: { $error }
boot-rom-ignored = the boot ROM ignored the file (not for this board's partitions)
boot-rom-locked = another program holds the boot ROM (picotool?)
boot-rom-bad-address = the boot ROM stopped: an address outside the flash
boot-rom-write-error = the boot ROM couldn't write the flash
boot-rom-reboot-failure = the boot ROM wrote the firmware but couldn't start it
boot-no-drive = the boot ROM's drive (RP2350) isn't mounted: open it in the file manager, then install again
boot-write = writing to the boot ROM's drive: { $error }
boot-no-restart = the board didn't restart after the copy (nothing for 20 s)

## odkvmctl: help

cli-about = Control the { -product } switch
cli-sim = Talk to a built-in simulated KVM instead of the real one
cli-status = Show the KVM's state (the default)
cli-diag = Measure the KVM's clocks, voltages, temperatures and USB links, each graded good, warning or bad; exits with 1 if anything is bad
cli-list = List the KVM control interfaces this computer can see
cli-watch = Print events (switches, port changes, faults) as they happen
cli-switch = Switch the keyboard, mouse and other device to a PC, or (with the { -wireless(case: "lower") } module) to a { -wireless(case: "lower") } host
cli-wireless = { -wireless } hosts (KVMs with the module): list them (the default), pair a new one, forget, name one, or set what happens to a host while another target is active
cli-port = Show a front port, set its mode, or detect its device again
cli-profile = List the profiles, make one active (profile next, profile 1, profile gaming), or change one (profile 1 name gaming, profile 1 mouse passthrough)
cli-profile-which = A profile's number or name, or next
cli-profile-setting = name, or a port: keyboard, mouse, other
cli-hotkey = Show or set a hotkey: double:scrolllock, double:rctrl:noswallow, ctrl+alt+1, none; or learn it from the next keys pressed on the KVM's keyboard
cli-config = Read and change settings
cli-identify = Blink the KVM's LEDs
cli-update = Install RP2354B firmware: onto a board waiting in its bootloader on the PWR/SERVICE port if there is one, else over a PC port (asks for a press of the key on the KVM)
cli-reboot = Restart the KVM, or put a chip into its bootloader
cli-display = Monitors whose input this computer can switch over DDC/CI (no KVM needed)
cli-display-list = The monitors, the input each shows and the inputs it offers (the default; takes a few seconds)
cli-display-input = Switch a monitor to an input, e.g. display input XYZ-1A2B-1234 0x11 (IDs from display list)
cli-hosts-list = The four host slots: name, state, policy
cli-hosts-pair = Make the KVM discoverable for 60 s as "{ -product }"; then pair it on the PC, tablet or phone. If the host shows six digits, type them on the KVM's keyboard and press Enter (or use passkey)
cli-hosts-pair-stop = stop closes pairing early
cli-hosts-passkey = The six digits a pairing host shows, when the KVM's keyboard isn't at hand
cli-hosts-forget = Forget a host's pairing: slot 1-4, or all
cli-hosts-name = Name a host slot (empty: "Host n")
cli-hosts-policy = stay: the host stays connected and switching to it is instant. on-select: connected only while selected (for phones and tablets that hide their on-screen keyboard while a keyboard is connected)
cli-port-redetect = Detect the device again (needed after swapping a device on a passthrough port)
cli-config-get = Show one setting, or all of them
cli-config-set = Change a setting (see config get for names and current values)
cli-config-save = Write the settings to the KVM's flash now (otherwise done 5 s after the last change)
cli-config-reset = Back to factory settings (the software lock settings are kept)
cli-target-next = The next target: PC1, PC2, then each { -wireless(case: "lower") } host that is connected
cli-reboot-at32-retry = Give the AT32F405s up to 5 more programming attempts, then restart
cli-reboot-wireless-retry = Give the { -wireless(case: "lower") } module up to 5 more programming attempts, then restart

## odkvmctl: output

cli-port-auto = auto: { $why }
cli-port-set-to = set to { $mode }: { $why }
cli-status-head = { -product } ({ $device }): firmware { $firmware }, AT32 { $at1 } / { $at2 }, hardware { $hardware }, serial { $serial }
cli-module-firmware = { -wireless } module: firmware { $version }
cli-module-state = { -wireless } module: { $state }
cli-active-host = Active: { -wireless(case: "lower") } host { $slot } "{ $host }" (home PC { $pc })
cli-active = Active: { $target }
cli-this-computer = this computer is { $pc }
cli-profile-line = Profile: { $n } ({ $name })
cli-host-slot = Host { $slot }
cli-pairing-open = { -wireless } pairing is open ({ $seconds } s left)
cli-passkey-waits = a pairing host waits for its passkey: type it on the KVM's keyboard, then Enter
cli-budget-low = devices may need more
cli-power = Power: { $power }
cli-selftest = Self-test: { $result }
cli-try-again = try again: { $command }, or hold the KVM's key while powering it up
cli-ports = Ports:
cli-switching-disabled = switching from software is disabled
cli-config-disabled = configuration from software is disabled
cli-locked = Locked at the device: { $what }
cli-host-ready = connected, { $ms } ms
cli-profiles-in-use = { $count } of { $max } in use: odkvmctl config set profiles <1-{ $max }>
cli-no-profile = no profile '{ $which }': a number from 0 to { $last } or a name
cli-learn-sim = the simulated KVM's keyboard can't be pressed from here: try Learn in `opendyn-kvm --sim`
cli-learn-press = Press the new hotkey on the KVM's keyboard within { $seconds } s. The keys reach neither PC meanwhile.
cli-learn-unsupported = this KVM's firmware can't learn hotkeys: give the hotkey as text, e.g. ctrl+alt+1
cli-nothing-changed-why = { $why }: nothing changed
cli-use-it-for = Use it for { $name }? [y/N]
cli-turn-it-off = Turn it off? [y/N]
## Words besides y and yes that answer a [y/N] question with yes, separated by commas.
cli-yes-words = y, yes
cli-readings = { $count } readings: { $verdict }
cli-ev-back-on-pc = input back on a PC
cli-ev-host-active = switched to { -wireless(case: "lower") } host { $slot }
cli-ev-pairing-changed = { -wireless } pairing changed
cli-ev-host-changed = { -wireless } host { $slot } changed
cli-no-monitors = no monitors found
cli-inputs = inputs: { $inputs }
cli-input-number = input '{ $value }': a number such as 0x11 or 17
cli-no-interface = no { -product } control interface found
cli-connecting = connecting to the KVM
cli-watching = watching { $device } (Ctrl+C to stop)
cli-no-module = this KVM has no { -wireless(case: "lower") } module
cli-no-host-in-slot = no host is paired in that slot
cli-no-module-boards = this KVM has no { -wireless(case: "lower") } module (boards from 0.16.0 can carry one)
cli-pair-usage = '{ $value }': use `wireless pair` or `wireless pair stop`
cli-pairing-opened = Pairing is open for { $seconds } s: pair "{ -product }" on the PC, tablet or phone.
cli-pairing-digits = If it shows six digits, type them on the KVM's keyboard and press Enter.
cli-six-digits = the six digits the host shows
cli-no-passkey-wanted = no host waits for a passkey
cli-slot-or-all = slot 1-4 or all
cli-slot = slot 1-4
cli-no-profiles = this firmware has no profiles
cli-profile-not-in-use = profile { $n } is not in use: odkvmctl config set profiles { $count }
cli-missing-value = profile { $setting }: missing the value
cli-nothing-changed = Nothing changed.
cli-unknown-setting = unknown setting '{ $name }'
cli-press-key-within = Press the key on the KVM within { $seconds } s to confirm…
cli-saved = saved
cli-factory-restored = factory settings restored
cli-reading = reading { $file }
cli-firmware-for = firmware { $version } for hardware { $major }.x
cli-update-file = The firmware: a release's folder, kvm-rp2354.bin or a .uf2 (default: the firmware shipped with this program)
cli-boot-no-drive = drive not mounted
cli-no-firmware = no firmware named, and none shipped with this program
cli-no-kvm-anywhere = no { -product } on a PC port, and none in its bootloader on the PWR/SERVICE port
cli-boot-several = several boards in their bootloader: connect one at a time
cli-boot-unknown = Without it, odkvmctl can't tell a KVM from a new board. Copy kvm-rp2354.uf2 (a KVM with firmware) or kvm-first.uf2 (a new board) onto its RP2350 drive by hand.

## Screen hop (the feature's name is the -hop term)

page-screens = { -hop }
switch-source-edge = the pointer crossed a screen edge
hop-line = { -hop }: { $state }
hop-state-off = off in this profile
hop-state-active = on
hop-state-no-absolute = needs the absolute pointer
hop-state-not-normalized = needs the Mouse port in Normalized mode
hop-state-no-screens = this PC has no screen in the arrangement
hop-state-boot-protocol = off while the PC is in its firmware setup
hop-state-wireless = off while a { -wireless(case: "lower") } host has the input
accel-flat = Flat (speed only)
accel-precision = Precision curve, as on Windows
accel-adaptive = Adaptive, as on Linux
mapping-desktop = Across all its screens (Linux)
mapping-main-screen = On its main screen only (Windows)
parse-screen = '{ $value }': write a screen as pc1 2560x1440 150% at 1920,0 desk 0,0 (scaling and positions may be left out), or off
parse-range = '{ $value }': a number from { $min } to { $max }
screens-none-found = no screens found
screens-xrandr-missing = can't run xrandr to list the screens ({ $error })
screens-xrandr-failed = xrandr couldn't list the screens: { $error }
screens-unsupported-os = finding the screens is only supported on Windows and Linux
profile-hop = Hop
profile-hop-hover = { -hop } in this profile (its own page)
hop-firmware-too-old = This KVM's firmware has no { -hop(case: "lower") }: update it.
hop-what = The KVM moves the pointer itself, across the screens arranged below. Push it across an edge onto the other PC's screen and the KVM switches to that PC, keyboard and all.
hop-turn-on-absolute = Turn on the absolute pointer
hop-mouse-normalized = Run the Mouse port Normalized in this profile
hop-in-profiles = On in these profiles
hop-profile-active = { $name } (active)
hop-absolute-pointer = Absolute pointer on the KVM's mouse
hop-absolute-pointer-note = { -hop } needs it. Turning it on or off makes both PCs reconnect the KVM's devices once.
hop-arrangement = Arrangement
hop-arrangement-note = Drag the screens to where they stand. The pointer passes where two screens meet and jumps a gap between them; an edge with nothing beside it holds the pointer.
hop-overlap = Some screens overlap: the pointer can't cross between them.
hop-add-screen = Add a { $pc } screen
hop-detect = Detect { $pc }'s screens
hop-detect-hover = This computer's screens, with their resolution, scaling and place in its display settings
hop-remove-screen = Remove
hop-mapping-of = { $pc } maps the pointer
hop-mapping-note = Windows puts an absolute pointer on its main screen only. The KVM reaches its other screens by moving the cursor there as a mouse does, and Windows accelerates that: turn off "Enhance pointer precision" on such a PC for exact edges, or keep { -product } running on it.
hop-mapping-windows = This computer runs Windows, which puts the absolute pointer on its main screen only: across all screens, the pointer lands in the wrong place and the other screens can't be reached. Choose "On its main screen only (Windows)".
hop-no-screens = No screens yet: add one for each PC.
hop-main-screen = main
hop-screen-hover = Click to edit, drag to move
hop-screen-pc = Computer
hop-screen-resolution = Resolution
hop-screen-scale = Scaling
hop-screen-desk = Position on its computer
hop-screen-at = Position here
hop-screen-desk-note = Where the screen is in its computer's display settings, in that computer's pixels. The main screen is at 0, 0.
hop-detected = { $count ->
    [one] { $pc } has 1 screen now, as this computer has it.
   *[other] { $pc } has { $count } screens now, as this computer has them.
} { $pc } maps the pointer: { $mapping }.
hop-pointer = Pointer
hop-accel = Acceleration
hop-speed = Speed
hop-resistance = Edge resistance
hop-corner = Corners
hop-pointer-note = The PCs don't accelerate an absolute pointer, so the KVM does. Edge resistance is how far the pointer must be pushed against an edge before it goes over to the other PC; corners is how far from each end of that edge it never goes over, so the corners stay easy to hit. A drag never goes over.
cli-screens = Screen hop: the arrangement of both PCs' screens and how the pointer moves
cli-screens-list = Show the arrangement and the pointer settings
cli-screens-detect = Put this computer's screens into the arrangement
cli-screens-which-pc = the KVM doesn't say which PC this computer is (the simulator or the service port): set the screens with odkvmctl config set screen0 … instead
cli-hop-mapping = { $pc } maps the pointer: { $mapping }
cli-hop-pointer = Pointer: { $accel }, speed { $speed }, edge resistance { $resistance } px, corners { $corner } px
