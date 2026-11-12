#!/usr/bin/env python3
"""
JLC stock and part class for every line of bom/jlcpcb_bom.csv, against what an order of N assembled boards needs.
The evidence for jlcpcb.md section 7: which parts block the order, which are thin, how many are Extended (each
Extended line costs a loading fee).  Stock is not reserved until an order is paid - re-run it right before ordering.

Queries JLC's public parts search (the same one jlcpcb.com/parts uses) once per line; needs network.

Run from hardware/:   python3 tools/jlc_stock.py [--boards N]          (default 5)
"""

import csv
import datetime
import json
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BOM = os.path.join(HERE, 'bom', 'jlcpcb_bom.csv')
URL = 'https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList'
HDRS = {'Content-Type': 'application/json', 'Accept': 'application/json', 'Origin': 'https://jlcpcb.com',
        'Referer': 'https://jlcpcb.com/parts',
        'User-Agent': 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36'}
THIN = 10                        # stock below THIN x what the order needs is flagged: it can vanish before you pay


def query(lcsc):
    body = json.dumps({'keyword': lcsc, 'currentPage': 1, 'pageSize': 10})
    cmd = ['curl', '-s', '-m', '30', '-X', 'POST', URL, '-d', body] + sum((['-H', f'{k}: {v}'] for k, v in HDRS.items()), [])
    try:
        items = json.loads(subprocess.run(cmd, capture_output=True, text=True).stdout)['data']['componentPageInfo']['list']
    except (ValueError, KeyError, TypeError):
        return None
    return next((i for i in items or [] if i['componentCode'] == lcsc), None)


def main():
    boards = int(sys.argv[sys.argv.index('--boards') + 1]) if '--boards' in sys.argv else 5
    lines = list(csv.DictReader(open(BOM)))
    rows = []
    for r in lines:
        refs = [x.strip() for x in r['Designator'].split(',')]
        it = query(r['LCSC Part #'])
        need = len(refs) * boards
        stock = it['stockCount'] if it else None
        cls = {'base': 'Basic', 'expand': 'Extended'}.get(it['componentLibraryType'], it['componentLibraryType']) if it else '?'
        state = 'NOT FOUND' if it is None else 'BLOCKS' if stock < need else 'thin' if stock < THIN * need else ''
        rows.append((state, r, refs, need, stock, cls))
        time.sleep(0.5)
    print(f'JLC parts library, {datetime.date.today()}: {len(lines)} BOM lines for {boards} assembled board(s)\n')
    order = {'NOT FOUND': 0, 'BLOCKS': 1, 'thin': 2, '': 3}
    for state, r, refs, need, stock, cls in sorted(rows, key=lambda x: (order[x[0]], x[4] or 0)):
        print(f'{state:9} {r["LCSC Part #"]:10} {r["Comment"][:22]:22} {cls:8} x{len(refs):<3} need {need:5}  stock '
              f'{stock if stock is not None else "-":>9}  {", ".join(refs[:4])}{" ..." if len(refs) > 4 else ""}')
    ext = sum(1 for *_, cls in rows if cls == 'Extended')
    bad = [r for r in rows if r[0] in ('BLOCKS', 'NOT FOUND')]
    print(f'\n{ext} of {len(rows)} lines are Extended parts; '
          + (f'{len(bad)} line(s) block the order' if bad else 'every line is in stock for this order'))
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
