# Live-view snapshot: the hubs' VDD33 caps on the bottom side, full run (2026-10-27)

Static copy of `tools/build/live/` from the rev-11 `python3 tools/route.py --live` run (442 s, 2923 segments, 1049
vias, `unrouted: 0 nets []`, 27/27 pairs).

Each hub's 100 nF VDD33 caps sit on B.Cu radially under their own pin (gen_pcb.py `HUB_PIN_CAPS`): the pin runs
0.45 mm on F.Cu past its tip to a via, 0.67 mm on B.Cu to the cap's +3V3 pad, and the cap's GND pad has a 0.50-0.71 mm
B.Cu stub to an exposed-pad GND via.  Hub-1 (U801) and Hub-2 (U901) have all six; Hub-0 (U701, full speed) has four -
its pins 5 and 36 keep their ring caps C701 / C706, because Hub-1's Other pair crosses under Hub-0's south pin row on
B.Cu.

Look at the three hubs (x 110-125 / 155-170 / 200-215, y 115-130) with F.Cu and In1/In2 off.  The verification is in
[analysis.md](../../../analysis.md) section 11.

Serve it and open the viewer:

```sh
python3 -m http.server 8765 --directory hardware/debug/rev11-hub-caps
```

then http://localhost:8765/ (stop the live router's server first: `fuser -k -TERM 8765/tcp`).
