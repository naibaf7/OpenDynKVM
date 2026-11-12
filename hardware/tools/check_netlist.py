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
print('OK' if not errs else f'{errs} problems', f'({len(want)} pins, {len(nets)} nets)')
