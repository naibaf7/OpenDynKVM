# Live-view snapshot: 0.19.0, four front ports, ESP32-MINI-1, rear extension connector (2026-11-25)

A static copy of the live view of the full run that routed 0.19.0 from scratch: Keyboard, Mouse, Aux 1 and Aux 2 at a
26 mm pitch (Aux 1 new on Hub-0's port 4 without a router, Aux 2 the Other port up to 0.18.0), all six status LEDs on
the bottom, the ESP32-MINI-1 instead of the ESP32-C3-MINI-1, and the JST SH extension connector at the rear.

```sh
uv run tools/gen_pcb.py --force
uv run python -u tools/route.py --live
```

`route.log` (local, not in git) is that run's log: 844 s, 0 unrouted nets, 3369 segments, 1085 vias.

**Checks on the board it wrote:** DRC with schematic parity 0 violations, 0 unconnected items, 0 parity issues;
`check_copper.py` pass; `check_spacing.py` and `check_aux.py` clean; `check_usb_si.py` 21 links, 0 hard failures
(worst skew H2_AT 0.99 mm as on 0.18.0; Hub-2 -> Keyboard 137.7 mm, Hub-1 -> Aux 2 105.0 mm, both 0.00 mm of skew).
+5V (`dc_drop.py +5V U402.1,U402.8 U1203=0.5 U1303=0.5 U1702=0.5 U1403=0.5 U403=0.5`): 122 / 113 / 87 / 77 mV at the
Keyboard / Mouse / Aux 1 / Aux 2 switches (0.18.0: 91 mV worst, three ports).

**The USB lanes** were re-planned by hand for the new front (`tools/route.py` LANES / H0_LANES, the "0.19.0: the front
was re-laid" comment): 28 of 28 links route as pairs at the first planned attempt after one fix (the Aux 1 pair's first
waypoint sat behind its ESD's own escape point).

**What the runs before this one showed** (all from scratch, the same placement unless noted):

| Run | Result | Fixed by |
|---|---|---|
| 1 | Stopped: the macros packed too tightly (user, from the live view) | macro parts spread out (gen_pcb.py COLUMN_PARTS, the extension block) |
| 2 | `EXT_3V3` / `EXT_SDA` / `EXT_SCL` unrouted (the JST's whole courtyard was a no-route body over its own pads), `A1_FLT` unrouted; +5V fed through the LEDs' 0.3 mm branch: 280-320 mV, ~190 K on In2.Cu | the JST body box ends at its pad row; RAIL_LANES; heavy loads join only the heavy part of a rail's tree |
| 3 | 0 unrouted, +5V 83-124 mV, but parts of the first planned feed left as dead ends | RAIL_LANES = the full-width path the router found |
| 4 | the lane's added via on R412's GND pad (short), In2 piece 0.22 mm from J501's guide hole | via spots moved; rail lanes now checked against pads (items_legal) before they are laid |
| 5 | four port LEDs' GND pads with no via ("no via spot": under the USB-A bodies), two of them on B.Cu GND islands cut off by the LED chain's +5V track; two +5V vias 0.4 mm apart | GND vias allowed under USB-A bodies (Board.usba_body) |
| 6 | this snapshot: every check above clean | |
