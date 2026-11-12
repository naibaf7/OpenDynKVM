#!/usr/bin/env python3
"""
KVM project symbol library -- generator (KiCad 10).

Writes KVM.kicad_sym with the parts KiCad's stock libraries lack.  Every symbol is a plain
rectangle; pins are listed per side as (number, name, electrical type).  Pin numbers follow the
datasheets in ../datasheets/ (cited per symbol) and the EasyEDA symbols JLC assembles from.

Run from hardware/:   python3 tools/gen_symbols.py
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import Q, dump  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(HERE, 'KVM.kicad_sym')
P = 2.54

SYMBOLS = []


def sym(name, ref, footprint, datasheet, desc, left=(), right=(), top=(), bottom=(), width=None, lcsc=''):
    SYMBOLS.append(dict(name=name, ref=ref, fp=footprint, ds=datasheet, desc=desc,
                        left=list(left), right=list(right), top=list(top), bottom=list(bottom), width=width,
                        lcsc=lcsc))


# AT32F405CCT7, LQFP-48 (DS_AT32F405_402_V2.03_EN, figure 3 / table 9)
sym('AT32F405CCT7', 'U', 'Package_QFP:LQFP-48_7x7mm_P0.5mm',
    'https://www.arterychip.com/en/product/AT32F405.jsp',
    'Artery AT32F405 Cortex-M4F 216 MHz, 256 KB flash, OTGHS with on-chip HS PHY (needs 12 MHz HEXT), LQFP-48',
    left=[('1', 'VDD', 'power_in'), ('28', 'VDD', 'power_in'), ('48', 'VDD', 'power_in'), ('9', 'VDDA', 'power_in'),
          None, ('7', 'NRST', 'input'), ('44', 'PF11/BOOT0', 'input'), None,
          ('5', 'PF0/HEXT_IN', 'input'), ('6', 'PF1/HEXT_OUT', 'output'), None,
          ('27', 'OTGHS1_D+', 'bidirectional'), ('26', 'OTGHS1_D-', 'bidirectional'), ('25', 'OTGHS1_R', 'passive'),
          None, ('34', 'PA13/SWDIO', 'bidirectional'), ('37', 'PA14/SWCLK', 'input'), None,
          ('8', 'VSSA', 'power_in'), ('23', 'VSS', 'power_in'), ('47', 'VSS', 'power_in')],
    right=[(str(n), nm, 'bidirectional') for n, nm in [
        (10, 'PA0'), (11, 'PA1'), (12, 'PA2'), (13, 'PA3'), (14, 'PA4'), (15, 'PA5'), (16, 'PA6'), (17, 'PA7'),
        (29, 'PA8'), (30, 'PA9'), (31, 'PA10'), (32, 'PA11'), (33, 'PA12'), (38, 'PA15'),
        (18, 'PB0'), (19, 'PB1'), (20, 'PB2'), (39, 'PB3'), (40, 'PB4'), (41, 'PB5'), (42, 'PB6'), (43, 'PB7'),
        (45, 'PB8'), (46, 'PB9'), (21, 'PB10'), (22, 'PB12'), (24, 'PB13'),
        (2, 'PC13'), (3, 'PC14'), (4, 'PC15'), (35, 'PF6'), (36, 'PF7')]],
    width=22.86, lcsc='C47115471')

# TS3USB3031, 1:3 USB 2.0 HS switch, WQFN-12 RMG (ts3usb3031.pdf, pin functions + table 6-1)
sym('TS3USB3031', 'U', 'Package_DFN_QFN:Texas_RMG0012A_WQFN-12_1.8x1.8mm_P0.4mm',
    'https://www.ti.com/lit/ds/symlink/ts3usb3031.pdf',
    '1:3 USB 2.0 HS switch. SEL1/SEL0: 00 = USB1, 01 = USB2, 10 = MHL, 11 = all off',
    left=[('12', 'VCC', 'power_in'), None, ('11', 'D+', 'bidirectional'), ('10', 'D-', 'bidirectional'), None,
          ('1', 'SEL0', 'input'), ('2', 'SEL1', 'input'), None, ('9', 'GND', 'power_in')],
    right=[('3', 'USB1+', 'bidirectional'), ('4', 'USB1-', 'bidirectional'), None,
           ('5', 'USB2+', 'bidirectional'), ('6', 'USB2-', 'bidirectional'), None,
           ('7', 'MHL+', 'bidirectional'), ('8', 'MHL-', 'bidirectional')],
    width=15.24, lcsc='C2652459')

# TPS2121, priority power mux, VQFN-HR-12 RUX (tps2121.pdf pin functions; EasyEDA C485916 agrees)
sym('TPS2121RUX', 'U', 'CustomFootprints:TI_RUX0012A_VQFN-HR-12_2x2.5mm_P0.5mm',
    'https://www.ti.com/lit/ds/symlink/tps2121.pdf',
    '2.8-22 V 4.5 A priority power mux. PR1 > 1.06 V selects IN1; ILM sets current limit; ST = status',
    left=[('7', 'IN1', 'power_in'), ('2', 'IN2', 'power_in'), None, ('6', 'PR1', 'input'), ('3', 'CP2', 'input'),
          ('5', 'OV1', 'input'), ('4', 'OV2', 'input')],
    right=[('1', 'OUT', 'power_out'), ('8', 'OUT', 'passive'), None, ('9', 'ST', 'open_collector'),
           ('10', 'ILM', 'passive'), ('11', 'SS', 'passive'), None, ('12', 'GND', 'power_in')],
    width=15.24, lcsc='C485916')

# LM66200, dual ideal diode, SOT-583 DRL (lm66200.pdf pin functions)
sym('LM66200', 'U', 'Package_TO_SOT_SMD:SOT-583-8',
    'https://www.ti.com/lit/ds/symlink/lm66200.pdf',
    'Dual 1.6-5.5 V 2.5 A ideal diode (diode-OR). ON active low; ST high when VIN1 is used',
    left=[('3', 'VIN1', 'power_in'), ('6', 'VIN2', 'power_in'), None, ('4', '~{ON}', 'input')],
    right=[('2', 'VOUT', 'power_out'), ('7', 'VOUT', 'passive'), None, ('8', 'ST', 'open_collector')],
    bottom=[('1', 'GND', 'power_in'), ('5', 'GND', 'power_in')],
    width=12.7, lcsc='C3235556')

# TPS2553, current-limited power switch, SOT-23-6 DBV (tps2553.pdf pin functions, TPS2553 column)
sym('TPS2553DBV', 'U', 'Package_TO_SOT_SMD:SOT-23-6',
    'https://www.ti.com/lit/ds/symlink/tps2553.pdf',
    'Current-limited USB power switch, 2.5-6.5 V, EN active high, Ilim[A] ~ 25.9/Rilim[kOhm]',
    left=[('1', 'IN', 'power_in'), ('3', 'EN', 'input')],
    right=[('6', 'OUT', 'power_out'), ('4', '~{FAULT}', 'open_collector'), ('5', 'ILIM', 'passive')],
    bottom=[('2', 'GND', 'power_in')],
    width=12.7, lcsc='C55266')

# SK6812SIDE-A, side-emitting addressable RGB LED (EasyEDA C5378721 pin names)
sym('SK6812SIDE-A', 'D', 'CustomFootprints:LED_SK6812SIDE-A_4.0x1.6mm',
    'https://www.lcsc.com/datasheet/C5378721.pdf',
    'Side-emitting addressable RGB LED (SK6812 protocol), 3.5-5.5 V',
    left=[('2', 'VDD', 'power_in'), ('1', 'DIN', 'input')],
    right=[('3', 'DOUT', 'output')],
    bottom=[('4', 'GND', 'power_in')],
    width=10.16, lcsc='C5378721')


# ESP32-C3-MINI-1, Bluetooth LE coprocessor (0.16.0), 53-pad LGA (esp32-c3-mini-1 datasheet v2.2, table 3-1).  Every
# ground pad (1, 2, 11, 14, 36-48 under the antenna end, the exposed pad 49, the corner pads 50-53) is one stacked GND
# pin; the NC pads are pins too, so the netlist covers every pad of the footprint.
sym('ESP32-C3-MINI-1', 'U', 'CustomFootprints:ESP32-C3-MINI-1',
    'https://documentation.espressif.com/esp32-c3-mini-1_datasheet_en.pdf',
    'ESP32-C3 module: RISC-V 160 MHz, Bluetooth LE 5 + Wi-Fi 4, 4 MB flash, PCB antenna, 13.2 x 16.6 mm. '
    'Strapping: IO2 and IO8 high, IO9 low at reset = UART download; EN high = on',
    left=[('3', '3V3', 'power_in'), None, ('8', 'EN', 'input'), None,
          ('30', 'RXD0/IO20', 'bidirectional'), ('31', 'TXD0/IO21', 'bidirectional'), None,
          ('5', 'IO2', 'bidirectional'), ('22', 'IO8', 'bidirectional'), ('23', 'IO9', 'bidirectional'), None,
          ('26', 'IO18/USB_D-', 'bidirectional'), ('27', 'IO19/USB_D+', 'bidirectional')],
    right=[('12', 'IO0', 'bidirectional'), ('13', 'IO1', 'bidirectional'), ('6', 'IO3', 'bidirectional'),
           ('18', 'IO4/MTMS', 'bidirectional'), ('19', 'IO5/MTDI', 'bidirectional'),
           ('20', 'IO6/MTCK', 'bidirectional'), ('21', 'IO7/MTDO', 'bidirectional'),
           ('16', 'IO10', 'bidirectional'), None]
          + [(str(n), 'NC', 'no_connect') for n in (4, 7, 9, 10, 15, 17, 24, 25, 28, 29, 32, 33, 34, 35)],
    bottom=[([str(n) for n in (1, 2, 11, 14, *range(36, 54))], 'GND', 'power_in')],
    width=20.32, lcsc='C41349510')


def pin(num, name, etype, x, y, ang, hide=False):
    eff = ['effects', ['font', ['size', 1.27, 1.27]]]
    return ['pin', etype, 'line', ['at', x, y, ang], ['length', P]] + ([['hide', 'yes']] if hide else []) + \
        [['name', Q(name), eff], ['number', Q(num), eff]]


def pins_at(it, x, y, ang):
    """One pin, or a stack of pins at one spot (it[0] a list of numbers): all but the first hidden, as KiCad's own
    module symbols stack their ground pads."""
    nums = it[0] if isinstance(it[0], list) else [it[0]]
    return [pin(n, it[1], it[2], x, y, ang, hide=k > 0) for k, n in enumerate(nums)]


def prop(name, value, at, hide=False, justify=None):
    e = ['effects', ['font', ['size', 1.27, 1.27]]]
    if justify:
        e.append(['justify', justify])
    p = ['property', Q(name), Q(value), ['at', at[0], at[1], 0], ['show_name', 'no'], ['do_not_autoplace', 'no']]
    if hide:
        p.append(['hide', 'yes'])
    return p + [e]


def build(s):
    rows = max(len(s['left']), len(s['right']))
    h = max(rows, 2) * P + P
    w = s['width'] or 15.24
    hw = round(w / 2 / P) * P
    top = round((h / 2) / P) * P
    pins = []

    def side(items, x, ang):
        y = top - P
        for it in items:
            if it is not None:
                pins.extend(pins_at(it, x, y, ang))
            y -= P

    side(s['left'], -hw - P, 0)
    side(s['right'], hw + P, 180)
    bottom_y = top - (rows + 1) * P
    for k, it in enumerate(s['bottom']):
        x = (k - (len(s['bottom']) - 1) / 2) * P * 2
        pins.extend(pins_at(it, round(x / P) * P, bottom_y - P, 90))
    for k, it in enumerate(s['top']):
        x = (k - (len(s['top']) - 1) / 2) * P * 2
        pins.extend(pins_at(it, round(x / P) * P, top + P, 270))
    name = s['name']
    node = ['symbol', Q(name), ['pin_names', ['offset', 1.016]], ['exclude_from_sim', 'no'],
            ['in_bom', 'no' if s['ref'] == 'TP' else 'yes'], ['on_board', 'yes'],
            ['in_pos_files', 'no' if s['ref'] == 'TP' else 'yes'], ['duplicate_pin_numbers_are_jumpers', 'no'],
            prop('Reference', s['ref'], (-hw, top + 1.27), justify='left'),
            prop('Value', name, (0, top + 1.27), justify='left'),
            prop('Footprint', s['fp'], (0, bottom_y - 5.08), hide=True),
            prop('Datasheet', s['ds'], (0, bottom_y - 7.62), hide=True),
            prop('Description', s['desc'], (0, bottom_y - 10.16), hide=True)]
    if s['lcsc']:
        node.append(prop('LCSC', s['lcsc'], (0, bottom_y - 12.7), hide=True))
    node.append(['symbol', Q(f'{name}_0_1'),
                 ['rectangle', ['start', -hw, top], ['end', hw, bottom_y],
                  ['stroke', ['width', 0.254], ['type', 'default']], ['fill', ['type', 'background']]]])
    node.append(['symbol', Q(f'{name}_1_1')] + pins)
    node.append(['embedded_fonts', 'no'])
    return node


def main():
    lib = ['kicad_symbol_lib', ['version', '20251024'], ['generator', Q('kicad_symbol_editor')],
           ['generator_version', Q('10.0')]] + [build(s) for s in SYMBOLS]
    with open(OUT, 'w') as f:
        f.write(dump(lib) + '\n')
    print(f'wrote {OUT}: {len(SYMBOLS)} symbols')


if __name__ == '__main__':
    main()
