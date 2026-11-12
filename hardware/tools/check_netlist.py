#!/usr/bin/env python3
"""Compare the KiCad-exported netlist against the intended pin->net map in gen_schematic.py."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import parse, find, first
import gen_schematic as g

net = parse(open(sys.argv[1]).read())
actual = {}
for n in find(first(net, 'nets'), 'net'):
    name = first(n, 'name')[1].lstrip('/')
    for nd in find(n, 'node'):
        actual[(first(nd, 'ref')[1], first(nd, 'pin')[1])] = name
want = {}
for p in g.PARTS:
    for pin, nn in p['pins'].items():
        want[(p['ref'], pin)] = nn
errs = 0
for k, nn in sorted(want.items()):
    a = actual.get(k)
    if nn is None:
        if a and not a.startswith('unconnected-'):
            print('NC pin connected:', k, a); errs += 1
    elif a != nn:
        print('MISMATCH', k, 'want', nn, 'got', a); errs += 1
for k, a in actual.items():
    if k not in want and not k[0].startswith('#'):
        print('EXTRA', k, a); errs += 1
nets = {}
for k, a in actual.items():
    nets.setdefault(a, []).append(k)
single = [n for n, v in nets.items() if len(v) == 1 and not n.startswith('unconnected-')]
if single:
    print('single-pin nets:', single); errs += len(single)

# ── USB D+/D- polarity, end to end ───────────────────────────────────────────────────────────────────────────────
# The pin-map comparison above confirms whatever the generator says, so it cannot see a pair that arrives crossed
# (errata.md E1).  This checks the netlist against the parts themselves: every net's USB pins (connectors, hubs,
# RP2354B, AT32F405 - read from the pin names) must be all D+ or all D-, and through each TS3USB3031 the common pair
# (D+ 11, D- 10) must meet each used channel (USB1 3/4, USB2 5/6, MHL 7/8, + first) with the same polarity -
# datasheet Table 6-1: D+ switches to USB1+/USB2+/MHL+, D- to the minus pins.  ESD chips carry no polarity (a channel
# is one line on two pads), and a switch side with no USB pin of its own is followed through series resistors.
import re
# (switch ref, channel) -> why a crossing is accepted, e.g. a fault on an ordered board that firmware works around.
# Empty since errata.md E1 was fixed (0.13.0, then 0.14.0; boards 0.8-0.12 had ('U601', 'USB1') crossed).
KNOWN_CROSSED = {}
SWITCH_CHANNELS = {'USB1': ('3', '4'), 'USB2': ('5', '6'), 'MHL': ('7', '8')}
P_PIN = re.compile(r'(^|[^A-Z0-9])D\+|USB_?DP')
N_PIN = re.compile(r'(^|[^A-Z0-9])D-|USB_?DM')
values, pin_name, pins_of = {}, {}, {}
for c in find(first(net, 'components'), 'comp'):
    values[first(c, 'ref')[1]] = first(c, 'value')[1]
for n in find(first(net, 'nets'), 'net'):
    for nd in find(n, 'node'):
        ref, pin, fn = first(nd, 'ref')[1], first(nd, 'pin')[1], first(nd, 'pinfunction')
        name = fn[1] if fn else ''
        pin_name[(ref, pin)] = name[:-len(pin) - 1] if name.endswith('_' + pin) else name
        pins_of.setdefault(ref, []).append(pin)
switches = sorted(r for r, v in values.items() if v == 'TS3USB3031')


def marks(netname):
    """The polarities of the USB pins on a net (switches excluded: they only pass a line on)."""
    m = set()
    for ref, pin in nets.get(netname, []):
        if ref in switches:
            continue
        name = pin_name.get((ref, pin), '')
        m |= {'D+'} if P_PIN.search(name) else {'D-'} if N_PIN.search(name) else set()
    return m


def polarity(netname, hops=2):
    m = marks(netname)
    if m or hops == 0:
        return m
    for ref, pin in nets.get(netname, []):
        if ref.startswith('R') and len(pins_of[ref]) == 2:
            other = actual[(ref, next(p for p in pins_of[ref] if p != pin))]
            if not other.startswith(('+', 'GND')) and polarity(other, hops - 1):
                return polarity(other, hops - 1)
    return set()


usb_nets = 0
for n in nets:
    m = marks(n)
    usb_nets += bool(m)
    if len(m) > 1:
        print('USB polarity: D+ and D- pins on one net:', n); errs += 1
crossed, channels = set(), 0
for sw in switches:
    cp, cn = polarity(actual[(sw, '11')]), polarity(actual[(sw, '10')])
    for ch, (p, n) in SWITCH_CHANNELS.items():
        tp, tn = actual.get((sw, p), ''), actual.get((sw, n), '')
        if tp.startswith('unconnected-') and tn.startswith('unconnected-'):
            continue
        channels += 1
        tp, tn = polarity(tp), polarity(tn)
        if len(cp | cn | tp | tn) < 2 or len(cp) != 1 or len(tp) != 1 or cp == cn or tp == tn:
            print(f'USB polarity: cannot tell {sw} {ch} (common {cp}/{cn}, channel {tp}/{tn})'); errs += 1
        elif cp != tp:
            crossed.add((sw, ch))
            if (sw, ch) in KNOWN_CROSSED:
                print(f'USB polarity: {sw} {ch} crosses D+/D- - known, {KNOWN_CROSSED[(sw, ch)]}')
            else:
                print(f'USB polarity: {sw} {ch} crosses D+/D- (common D+ pin 11 is {"/".join(cp)}, '
                      f'{ch}+ pin {p} is {"/".join(tp)})'); errs += 1
for k in sorted(set(KNOWN_CROSSED) - crossed):
    print(f'USB polarity: {k[0]} {k[1]} no longer crosses: remove it from KNOWN_CROSSED and mark '
          f'errata.md E1 fixed'); errs += 1
print(f'USB polarity: {usb_nets} nets with USB pins, {channels} switch channels, {len(crossed)} crossed')
print('OK' if not errs else f'{errs} problems', f'({len(want)} pins, {len(nets)} nets)')
