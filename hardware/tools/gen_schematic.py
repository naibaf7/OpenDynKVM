#!/usr/bin/env python3
"""
OpenDynKVM -- schematic generator (KiCad 10).  Adapted from the OpenRower generator.

The circuit is defined block by block below: one entry per symbol with its pin -> net map.
Every pin gets a short wire stub plus a net label (or a power symbol), so the whole netlist is
explicit and reviewable in this file.  Reference designators are numbered per block (block 3
uses R301, C301, ...) so adding a part only renumbers its own block; symbol UUIDs derive from the
reference, which keeps the PCB linked.  Symbols are laid out automatically, block by block.

Run from hardware/:   python3 tools/gen_schematic.py
Overwrites kvm.kicad_sch.  Hand edits made in eeschema are lost on re-run.
"""

import copy
import math
import os
import re
import sys
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from sexpr import Q, dump, find, first, parse, prop  # noqa: E402

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(HERE, 'kvm.kicad_sch')
SYSLIB = '/usr/share/kicad/symbols'
PRJLIB = os.path.join(HERE, 'KVM.kicad_sym')
PROJECT = 'kvm'
ROOT_UUID = 'b6b3d0a4-6a51-4c1e-9f0e-6b766d6b766d'
NS = uuid.UUID('6b766d00-dd4d-4b56-4d2d-646d6b766d00')  # fixed namespace for stable UUIDs

STUB = 2.54
GRID = 2.54

# ── footprints / parts ─────────────────────────────────────────────────────────
R0402 = 'Resistor_SMD:R_0402_1005Metric'
C0402 = 'Capacitor_SMD:C_0402_1005Metric'
C0603 = 'Capacitor_SMD:C_0603_1608Metric'
C0805 = 'Capacitor_SMD:C_0805_2012Metric'
C1206 = 'Capacitor_SMD:C_1206_3216Metric'
SOT23 = 'Package_TO_SOT_SMD:SOT-23'
SOT235 = 'Package_TO_SOT_SMD:SOT-23-5'
SOT236 = 'Package_TO_SOT_SMD:SOT-23-6'
XTAL = 'Crystal:Crystal_SMD_3225-4Pin_3.2x2.5mm'
HRO = 'CustomFootprints:USB_C_Receptacle_HRO_TYPE-C-31-M-12_Edge'

# value -> (LCSC, MPN); JLC library checked 2026-10-20 (all Basic unless noted)
RES = {
    '27R': ('C25100', '0402WGF270JTCE'),       # Extended
    '33R': ('C25105', '0402WGF330JTCE'),
    '1k': ('C11702', '0402WGF1001TCE'), '4.7k': ('C25900', '0402WGF4701TCE'),
    '5.1k': ('C25905', '0402WGF5101TCE'), '10k': ('C25744', '0402WGF1002TCE'),
    '12k': ('C25752', '0402WGF1202TCE'), '22k': ('C25768', '0402WGF2202TCE'),
    '33k': ('C25779', '0402WGF3302TCE'),
    '47k': ('C25792', '0402WGF4702TCE'), '100k': ('C25741', '0402WGF1003TCE'),
}
CAP = {
    '15pF': ('C1548', '0402CG150J500NT', C0402),
    '18pF': ('C1549', '0402CG180J500NT', C0402),
    '10nF': ('C15195', 'CL05B103KB5NNNC', C0402),
    '100nF': ('C1525', 'CL05B104KO5NNNC', C0402),
    '1uF': ('C52923', 'CL05A105KA5NQNC', C0402),
    '4.7uF': ('C23733', 'CL05A475MP5NRNC', C0402),
    '10uF': ('C19702', 'CL10A106KP8NNNC', C0603),
    '22uF': ('C45783', 'CL21A226MAQNNNE', C0805),
}

PARTS = []   # filled by the helpers below
_counters = {}
_block = [0]


def block(n):
    """Start reference block n: designators n01, n02, ... per prefix."""
    _block[0] = n


def ref(prefix):
    key = (prefix, _block[0])
    _counters[key] = _counters.get(key, 0) + 1
    return f'{prefix}{_block[0] * 100 + _counters[key]}'


def part(prefix, lib_id, value, fp, pins, rot=0, unit=1, lcsc='', mpn='', in_bom=True, refdes=None):
    r = refdes or ref(prefix)
    PARTS.append(dict(ref=r, lib_id=lib_id, value=value, fp=fp, rot=rot, unit=unit, pins=pins,
                      lcsc=lcsc, mpn=mpn, in_bom=in_bom, block=_block[0]))
    return r


def R(value, n1, n2):
    lcsc, mpn = RES[value]
    return part('R', 'Device:R', value, R0402, {'1': n1, '2': n2}, lcsc=lcsc, mpn=mpn)


def C(value, n1, n2='GND'):
    lcsc, mpn, fp = CAP[value]
    return part('C', 'Device:C', value, fp, {'1': n1, '2': n2}, lcsc=lcsc, mpn=mpn)


def CP(n_plus, n_minus='GND'):
    """220 uF 10 V polymer aluminium, 28 mOhm (Extended, checked 2026-10-21): VBUS / +5 V bulk."""
    return part('C', 'Device:C_Polarized', '220uF', 'Capacitor_SMD:CP_Elec_6.3x7.7', {'1': n_plus, '2': n_minus},
                lcsc='C46550458', mpn='MA10V220M6X8')


def vbus_tvs(vbus):
    # SMF5.0A: 5 V stand-off unidirectional TVS at the connector (Extended, checked 2026-10-21)
    return part('D', 'Device:D_Zener', 'SMF5.0A', 'Diode_SMD:D_SOD-123F', {'1': vbus, '2': 'GND'},
                lcsc='C193402', mpn='SMF5.0A')


def decouple(net, *values):
    for v in values:
        C(v, net)


def usblc6(dm, dp, vbus, swap=False):
    # USBLC6-2SC6: 1/6 = I/O1, 3/4 = I/O2 (flow-through), 5 = VBUS, 2 = GND.  The two channels are the same part inside,
    # so which one carries D+ is a free choice: swap=True puts D+ on channel 1 (pads 1/6) and D- on channel 2 (3/4).
    # The service port's ESD must *not* swap, and that is measured, not assumed (rev 8): this part is straight-through,
    # the connector's D+ pad is west of its D- pad and the service switch's D+ pin is east of its D- pin, so one of the
    # two pairs on this part must cross.  Swapping the ESD's channels only moves the crossover from the switch's pair to
    # the connector's, and the connector's pair has nowhere to put it (its fan-out is 0.5 mm wide, measured "lines only
    # -0.090 mm apart at (160.38,109.06)").  The switch's pair does have room - at its own fan-out, on the west channel
    # (gen_schematic.py block 6 and tools/route.py pair_links).
    a, b = (dm, dp) if swap else (dp, dm)
    return part('U', 'Power_Protection:USBLC6-2SC6', 'USBLC6-2SC6', SOT236,
                {'1': b, '6': b, '3': a, '4': a, '5': vbus, '2': 'GND'}, lcsc='C7519', mpn='USBLC6-2SC6')


def cc_tvs(cc1, cc2):
    # PESD5V0S2BT: two bidirectional TVS from pins 1/2 to the common pin 3
    return part('D', 'Device:D_TVS_Dual_AAC', 'PESD5V0S2BT', SOT23, {'1': cc1, '2': cc2, '3': 'GND'},
                lcsc='C49338', mpn='PESD5V0S2BT,215')


def usb_c(vbus, cc1, cc2, dp, dm):
    return part('J', 'Connector:USB_C_Receptacle_USB2.0_16P', 'USB-C', HRO,
                {'A1': 'GND', 'A12': 'GND', 'B1': 'GND', 'B12': 'GND', 'SH': 'GND',
                 'A4': vbus, 'A9': vbus, 'B4': vbus, 'B9': vbus, 'A5': cc1, 'B5': cc2,
                 'A6': dp, 'B6': dp, 'A7': dm, 'B7': dm, 'A8': None, 'B8': None},
                lcsc='C165948', mpn='TYPE-C-31-M-12')


def crystal(n1, n3, freq):
    if freq == '12MHz':   # Raspberry Pi's recommended part, used for all three 12 MHz clocks
        lcsc, mpn = 'C20625731', 'ABM8-272-T3'
    else:
        lcsc, mpn = 'C70590', 'X322524MOB4SI'
    part('Y', 'Device:Crystal_GND24', freq, XTAL, {'1': n1, '3': n3, '2': 'GND', '4': 'GND'}, lcsc=lcsc, mpn=mpn)
    cl = '15pF' if freq == '12MHz' else '18pF'          # 2 x CL - stray: ABM8 CL 10 pF, X322524 CL 12 pF
    C(cl, n1)
    C(cl, n3)


# ══ 1. Rear USB-C: PC1, PC2 (device ports) and PWR/SERVICE ═════════════════════
for n in (1, 2):
    block(n)
    usb_c(f'VBUS_PC{n}', f'CC1_PC{n}', f'CC2_PC{n}', f'PC{n}_DP', f'PC{n}_DM')
    R('5.1k', f'CC1_PC{n}', 'GND')                    # Rd: sink
    R('5.1k', f'CC2_PC{n}', 'GND')
    cc_tvs(f'CC1_PC{n}', f'CC2_PC{n}')
    usblc6(f'PC{n}_DM', f'PC{n}_DP', f'VBUS_PC{n}')
    C('1uF', f'VBUS_PC{n}')
    R('100k', f'VBUS_PC{n}', f'VDET_PC{n}')           # 2:1 -> hub VBUS_DET and AT32 ADC
    R('100k', f'VDET_PC{n}', 'GND')
    vbus_tvs(f'VBUS_PC{n}')

block(3)
usb_c('VBUS_PWR', 'CC1_PWR', 'CC2_PWR', 'SVC_DP', 'SVC_DM')
R('5.1k', 'CC1_PWR', 'GND')
R('5.1k', 'CC2_PWR', 'GND')
cc_tvs('CC1_PWR', 'CC2_PWR')
usblc6('SVC_DM', 'SVC_DP', 'VBUS_PWR')               # no swap: see usblc6's note (rev 8, the two pairs on U301)
C('1uF', 'VBUS_PWR')
R('100k', 'VBUS_PWR', 'VSNS_PWR')                     # 5 V -> 1.6 V for the RP2354B ADC
R('47k', 'VSNS_PWR', 'GND')
vbus_tvs('VBUS_PWR')

# ══ 4. Power path, 5 V and 3.3 V / 1.1 V rails ═════════════════════════════════
block(4)
part('U', 'KVM:LM66200', 'LM66200', 'Package_TO_SOT_SMD:SOT-583-8',
     {'3': 'VBUS_PC1', '6': 'VBUS_PC2', '2': 'VBUS_PCS', '7': 'VBUS_PCS', '4': 'GND', '8': None, '1': 'GND', '5': 'GND'},
     lcsc='C3235556', mpn='LM66200DRLR')
C('1uF', 'VBUS_PCS')
# rev 9, 2026-10-26: one input capacitor *at each mux input*, which the datasheets ask for and rev 8 did not have -
# the only 1 uF on VBUS_PC1/VBUS_PC2 sat at the port connector (measured 63.0 / 25.8 mm from the pins), and VBUS_PWR's
# was at the service connector 19.7 mm away.  LM66200: "using an input capacitance (CIN) of 1 uF is sufficient";
# TPS2121 reference schematics show 1 uF per input.  gen_pcb.py puts them beside their pin.
C('1uF', 'VBUS_PC1')                                  # LM66200 IN1 (pin 3)
C('1uF', 'VBUS_PC2')                                  # LM66200 IN2 (pin 6)
part('U', 'KVM:TPS2121RUX', 'TPS2121', 'CustomFootprints:TI_RUX0012A_VQFN-HR-12_2x2.5mm_P0.5mm',
     {'7': 'VBUS_PWR', '2': 'VBUS_PCS', '1': '+5V', '8': '+5V', '6': 'PR1', '3': 'CP2', '5': 'OV1', '4': 'OV2',
      '9': 'PWR_ST', '10': 'ILM', '11': 'SS', '12': 'GND'}, lcsc='C485916', mpn='TPS2121RUXR')
C('1uF', 'VBUS_PWR')                                  # TPS2121 IN1 (pin 7) input cap, at its pin: rev 9, was 19.7 mm
C('1uF', 'VBUS_PCS')                                  # TPS2121 IN2 (pin 2) input cap, at its pin: was 6.8 mm
# XCOMP priority (datasheet table 9-3): with a PC present CP2 > VREF, and OUT follows IN1 (PWR) while
# PR1 > CP2, i.e. above VBUS_PWR = 4.0-4.5 V for a PC at 4.75-5.25 V, with the 5 us fast switchover.  Without a PC,
# CP2 < VREF and PR1 alone selects IN1 above ~3.4 V.
R('22k', 'VBUS_PWR', 'PR1')                           # PR1 = VBUS_PWR x 10/32
R('10k', 'PR1', 'GND')
R('100k', '+3V3', 'PWR_ST')
R('47k', 'ILM', 'GND')                                # 65.2 / 47^0.861 = 2.4 A: inside the LM66200's 2.5 A rating
C('100nF', 'SS')                                      # ~0.8 V/ms: gentle inrush from a PC port
C('22uF', '+5V')
C('22uF', '+5V')
C('100nF', '+5V')
part('U', 'Regulator_Switching:TLV62569DBV', 'TLV62569', SOT235,
     {'4': '+5V', '1': '+5V', '2': 'GND', '3': 'SW_3V3', '5': 'FB_3V3'}, lcsc='C141836', mpn='TLV62569DBVR')
part('L', 'Device:L', '2.2uH', 'Inductor_SMD:L_Sunlord_SWPA3015S', {'1': 'SW_3V3', '2': '+3V3'},
     lcsc='C43389', mpn='SWPA3015S2R2MT')
R('100k', '+3V3', 'FB_3V3')                           # 0.6 V * (1 + 100k/22k) = 3.33 V
R('22k', 'FB_3V3', 'GND')
C('10uF', '+5V')
C('22uF', '+3V3')
C('22uF', '+3V3')
part('U', 'Regulator_Linear:TLV73311PDBV', 'TLV73311', SOT235,
     {'1': '+3V3', '3': '+3V3', '2': 'GND', '4': None, '5': '+1V1'}, lcsc='C2865431', mpn='TLV73311PDBVR')
C('1uF', '+3V3')
C('1uF', '+1V1')
R('33k', 'VBUS_PCS', 'CP2')                           # CP2 = VBUS_PCS x 12/45: 1.25-1.4 V with a PC (VREF <= 1.1 V)
R('12k', 'CP2', 'GND')
R('47k', 'VBUS_PWR', 'OV1')                           # OV cut-off at 1.06 V x 57/10 = 6.0 V on each input
R('10k', 'OV1', 'GND')
R('47k', 'VBUS_PCS', 'OV2')
R('10k', 'OV2', 'GND')
CP('+5V')                                             # +5 V hold-up through the switchover

# ══ 5. RP2354B: host, switching logic ══════════════════════════════════════════
block(5)
GPIO = {
    # SPI0 -> AT32F405 #1 (left), SPI1 -> AT32F405 #2 (right)
    16: 'AT1_MISO', 17: 'AT1_CS', 18: 'AT1_SCK', 19: 'AT1_MOSI', 12: 'AT1_IRQ', 13: 'AT1_NRST', 14: 'AT1_BOOT0',
    44: 'AT2_MISO', 45: 'AT2_CS', 46: 'AT2_SCK', 47: 'AT2_MOSI', 38: 'AT2_IRQ', 39: 'AT2_NRST', 43: 'AT2_BOOT0',
    # front-port routers and power switches, grouped by where their router lies on the board (left of each
    # USB-A port): keyboard and mouse on the west half of the south pins; other on the south-east corner and the
    # bottom of the east side (pins 40/42/43/46), so its lines leave east of the crystal / SWD lines instead of
    # crossing them (EN and FAULT nearest the corner: the power switch lies west of the router)
    20: 'KB_SEL0', 21: 'KB_SEL1', 22: 'KB_EN', 23: 'KB_FLT',
    24: 'MS_SEL0', 25: 'MS_SEL1', 26: 'MS_EN', 27: 'MS_FLT',
    32: 'OT_EN', 33: 'OT_FLT', 34: 'OT_SEL0', 37: 'OT_SEL1',
    # Hub-0 reset and VBUS_DET on GPIO2/3 (pins 79/80, the corner facing Hub-0); same pad type and reset state
    # (Digital IO (FT), pull-down) as any other bank 0 GPIO
    2: 'HUB0_RST', 3: 'HUB0_VBUS',
    # PC hub resets, shared open-drain with the AT32s: the RP2354B holds a hub in reset while it flashes its AT32
    6: 'HUB1_RST', 35: 'HUB2_RST',
    # user interface, service port, power status; CC ADC inputs behind 10 k series resistors (not 5 V tolerant)
    15: 'LED_DATA', 11: 'KEY', 0: 'SVC_SEL', 1: 'PWR_ST',
    40: 'VSNS_PWR', 41: 'RP_CC1', 42: 'RP_CC2',
    36: 'DBG_TX',                                              # UART1 TX -> spare pin of the SWD pads
}
rp = {'81': 'GND'}
rp_pins = {  # QFN-80 pin -> GPIO number
    1: 4, 2: 5, 3: 6, 4: 7, 6: 8, 7: 9, 8: 10, 9: 11, 11: 12, 12: 13, 13: 14, 14: 15, 16: 16, 17: 17, 18: 18,
    19: 19, 20: 20, 21: 21, 22: 22, 23: 23, 25: 24, 26: 25, 27: 26, 28: 27, 36: 28, 37: 29, 38: 30, 39: 31,
    40: 32, 42: 33, 43: 34, 44: 35, 45: 36, 46: 37, 47: 38, 48: 39, 49: 40, 52: 41, 53: 42, 54: 43, 55: 44,
    56: 45, 57: 46, 58: 47, 77: 0, 78: 1, 79: 2, 80: 3}
for pin, g in rp_pins.items():
    rp[str(pin)] = GPIO.get(g)
for pin in (5, 15, 24, 29, 41, 50, 60, 76, 59, 68, 69):        # IOVDD, ADC_AVDD, USB_OTP_VDD, QSPI_IOVDD
    rp[str(pin)] = '+3V3'
for pin in (10, 32, 51):                                       # DVDD from the external LDO
    rp[str(pin)] = '+1V1'
rp.update({'61': 'RP_VREG', '64': 'RP_VREG', '65': 'GND', '62': 'GND', '63': None,   # regulator disabled (DS fig. 24)
           '30': 'RP_XIN', '31': 'RP_XOUT', '33': 'RP_SWCLK', '34': 'RP_SWDIO', '35': 'RP_RUN',
           '66': 'RP_USB_DM_PIN', '67': 'RP_USB_DP_PIN', '75': 'QSPI_SS',
           '70': None, '71': None, '72': None, '73': None, '74': None})      # QSPI: in-package flash
part('U', 'MCU_RaspberryPi:RP2354B', 'RP2354B', 'Package_DFN_QFN:QFN-80-1EP_10x10mm_P0.4mm_EP3.4x3.4mm', rp,
     lcsc='C39843328', mpn='RP2354B')
decouple('+3V3', *['100nF'] * 11, '10uF')                      # 8x IOVDD, ADC_AVDD, USB_OTP_VDD, QSPI_IOVDD
decouple('+1V1', '100nF', '100nF', '100nF')
R('33R', '+3V3', 'RP_VREG')                                    # keeps POR / BOD supplies quiet
C('4.7uF', 'RP_VREG')
R('1k', 'RP_XOUT', 'RP_XO_R')                                  # Raspberry Pi crystal circuit
crystal('RP_XIN', 'RP_XO_R', '12MHz')
R('27R', 'RP_USB_DP_PIN', 'RP_DP')
R('27R', 'RP_USB_DM_PIN', 'RP_DM')
R('10k', '+3V3', 'RP_RUN')
C('100nF', 'RP_RUN')
R('1k', 'QSPI_SS', 'BOOTSEL')
part('SW', 'Switch:SW_Push', 'BOOTSEL', 'Button_Switch_SMD:SW_Push_1P1T_XKB_TS-1187A', {'1': 'GND', '2': 'BOOTSEL'},
     lcsc='C318884', mpn='TS-1187A-B-A-B')
part('J', 'Connector:Conn_ARM_SWD_TagConnect_TC2030-NL', 'SWD RP2354B',
     'Connector:Tag-Connect_TC2030-IDC-NL_2x03_P1.27mm_Vertical',
     {'1': '+3V3', '2': 'RP_SWDIO', '3': 'RP_RUN', '4': 'RP_SWCLK', '5': 'GND', '6': 'DBG_TX'}, in_bom=False)
for net in ('AT1_BOOT0', 'AT2_BOOT0'):                         # AT32 boots from flash unless the RP2354B says so
    R('10k', net, 'GND')
R('100k', 'HUB0_VBUS', 'GND')
R('100k', 'SVC_SEL', '+3V3')                                  # blank RP2354B: USB goes to the service port (SEL = 01 =
#                                                             USB2, which carries the service connector since rev 8)
R('10k', 'CC1_PWR', 'RP_CC1')                                  # CC can see 5 V (bent contact, bad cable)
R('10k', 'CC2_PWR', 'RP_CC2')

# ══ 6. Service switch: RP2354B USB -> PWR/SERVICE port (SEL=00) or Hub-0 (SEL=01) ════
block(6)
# Rev 8: the same 1:3 part as the three front-port switches.  One switch type in the BOM - and the reason it matters,
# the TS3USB3031 puts *every* differential pair on adjacent pins (D+ 11 / D- 10, USB1 3/4, USB2 5/6, MHL 7/8), where the
# TS3USB30E it replaces puts each pair on opposite rows of its TSSOP (D1 2/8, D2 3/7, D 4/6, 4.3 mm apart).  That
# straddling pinout is what forced the router's hand-built svc_breakout() (a via beside each row, both lines converging
# under the body); with this part every USB pair here is an ordinary pad-to-pad pair.
# Channel assignment (rev 8, third pass - measured, see tools/route.py pair_links): the service connector on USB2, Hub-0's
# full-speed upstream on USB1, **MHL left unused** (on purpose: only it is specified at 1.5-3.3 V
# signal levels, and a hub upstream does not belong there while a proper USB channel is free).
# Both of those USB channels are in use, so the pair *polarity* inside the service channel is what makes the link
# routable - and that is free to choose, because this switch is a passive pass-through: D+ and D- are interchangeable on
# its pins.  The service pair needs the two lines to keep their side all the way (its far end, the service ESD U301, has
# D+ on the pad west of D-).  With D+ on pin 5 - the eastern pin, like every other channel of this part - the pair would
# have to cross over, and there is nowhere for that: the neighbouring USB1 terminal's reserved corridor is 0.8 mm away
# and a crossover via needs 0.65 mm beside the pair, so the checked zone is refused and the unchecked fallback comes out
# crossed (measured: "no pair path ... (0,380,1200)", then "lines only -0.190 mm apart").  With D+ on pin 6 instead the
# two ends agree, no crossover is needed, and both channel pairs route.
part('U', 'KVM:TS3USB3031', 'TS3USB3031', 'Package_DFN_QFN:Texas_RMG0012A_WQFN-12_1.8x1.8mm_P0.4mm',
     {'12': '+3V3', '9': 'GND', '1': 'SVC_SEL', '2': 'SVC_SEL1',
      '11': 'RP_DM', '10': 'RP_DP',                    # common port: the RP2354B, behind its 27R series resistors.
      # RP_DP on pin 10 rather than 11 for the same reason as the service channel's D+: the switch is passive, so the
      # two lines of the pair are interchangeable on its pins, and this way the pair keeps its side all the way from the
      # RP2354B (whose own D+ pin 67 is west of its D- pin 66) to the switch - no crossover, which is what the router
      # could not place here (its A end is the 27R resistors, its B end is 1.6 mm west of the port's escape).
      '3': 'H0_UP_DP', '4': 'H0_UP_DM',                # USB1: Hub-0's upstream port (full speed)
      '5': 'SVC_DM', '6': 'SVC_DP',                    # USB2: the service connector, D+ on pin 6 - see below
      '7': None, '8': None},                           # MHL: unused (it carries no HS channel, so it stays free)
     lcsc='C2652459', mpn='TS3USB3031RMGR')
R('100k', 'SVC_SEL1', 'GND')                           # SEL1 low: SEL0 alone picks USB1 (00) or USB2 (01)
C('100nF', '+3V3')


# ══ 7-9. Hubs: Hub-0 (KVM host side, runs at FS), Hub-1 (PC1), Hub-2 (PC2) ════
def hub(h, up, ds, vbus_det, rst, non_rem_port1):
    """USB2514B, strap-configured (CFG_SEL = 00): self-powered, straps enabled."""
    pins = {'30': up[1], '31': up[0], '37': 'GND', '11': 'GND',
            '14': f'H{h}_CRFILT', '34': f'H{h}_PLLFILT', '35': f'H{h}_RBIAS',
            '32': f'H{h}_XO', '33': f'H{h}_XI', '26': rst, '27': vbus_det,
            '24': f'H{h}_CFG0', '25': f'H{h}_CFG1', '22': f'H{h}_NR1', '28': f'H{h}_NR0',
            '12': None, '16': None, '18': None, '20': None,          # PRTPWR / BC_EN: internal pull-down
            '13': None, '17': None, '19': None, '21': None}          # OCS_N: internal pull-up
    for k, (dm_pin, dp_pin) in enumerate([('1', '2'), ('3', '4'), ('6', '7'), ('8', '9')]):
        pins[dm_pin], pins[dp_pin] = ds[k][1], ds[k][0]
    for p in ('5', '10', '29', '36', '15', '23'):
        pins[p] = '+3V3'
    part('U', 'Interface_USB:USB2514B_Bi', 'USB2514B', 'Package_DFN_QFN:QFN-36-1EP_6x6mm_P0.5mm_EP3.7x3.7mm', pins,
         lcsc='C16251', mpn='USB2514B-AEZC-TR')
    decouple('+3V3', *['100nF'] * 6, '4.7uF')
    C('100nF', f'H{h}_CRFILT')
    C('100nF', f'H{h}_PLLFILT')
    R('12k', f'H{h}_RBIAS', 'GND')
    crystal(f'H{h}_XI', f'H{h}_XO', '24MHz')
    R('10k', f'H{h}_CFG0', 'GND')                     # CFG_SEL[1:0] = 00
    R('10k', f'H{h}_CFG1', 'GND')
    R('100k', f'H{h}_NR1', 'GND')                     # NON_REM1 = 0
    R('100k', f'H{h}_NR0', '+3V3' if non_rem_port1 else 'GND')   # NON_REM0: 1 = port 1 non-removable
    R('10k', '+3V3', rst)                             # RESET_N: RC power-on reset, MCU may pull low
    C('100nF', rst)


block(7)
hub(0, ('H0_UP_DP', 'H0_UP_DM'),
    [('H0_KB_DP', 'H0_KB_DM'), ('H0_MS_DP', 'H0_MS_DM'), ('H0_OT_DP', 'H0_OT_DM'), ('H0_D4_DP', 'H0_D4_DM')],
    'HUB0_VBUS', 'HUB0_RST', False)
R('10k', 'H0_D4_DP', '+3V3')                          # port 4 unused: disabled by strap
R('10k', 'H0_D4_DM', '+3V3')
# Hub ports follow where the lanes leave each hub on the board (all hubs sit identically under their USB-C,
# ports facing south).  AT32 #1 sits right of Hub-1 -> port 4 (east end); AT32 #2 sits left of Hub-2 -> port 1
# (west end, non-removable).  The strap can only mark port 1; the flag only affects the hub descriptor.
for n, at_port in ((1, 4), (2, 1)):
    block(7 + n)
    fronts = [(f'H{n}_{p}_DP', f'H{n}_{p}_DM') for p in ('KB', 'MS', 'OT')]
    at = (f'H{n}_AT_DP', f'H{n}_AT_DM')
    hub(n, (f'PC{n}_DP', f'PC{n}_DM'), fronts + [at] if at_port == 4 else [at] + fronts,
        f'VDET_PC{n}', f'HUB{n}_RST', at_port == 1)

# ══ 10-11. AT32F405 #1 / #2: the HS devices each PC sees ═══════════════════════
for n in (1, 2):
    block(9 + n)
    at = {str(p): None for p in range(1, 49)}
    at.update({'1': '+3V3', '28': '+3V3', '48': '+3V3', '9': '+3V3', '8': 'GND', '23': 'GND', '47': 'GND',
               '5': f'AT{n}_XI', '6': f'AT{n}_XO', '7': f'AT{n}_NRST', '44': f'AT{n}_BOOT0',
               '25': f'AT{n}_RREF', '26': f'H{n}_AT_DM', '27': f'H{n}_AT_DP',
               '14': f'AT{n}_CS', '15': f'AT{n}_SCK', '16': f'AT{n}_MISO', '17': f'AT{n}_MOSI',
               '22': f'AT{n}_IRQ', '19': f'HUB{n}_RST',
               '10': f'VDET_PC{n}', '11': f'AT{n}_CC1', '18': f'AT{n}_CC2',
               '34': f'AT{n}_SWDIO', '37': f'AT{n}_SWCLK', '2': f'AT{n}_ROLE'})
    part('U', 'KVM:AT32F405CCT7', 'AT32F405CCT7', 'Package_QFP:LQFP-48_7x7mm_P0.5mm', at,
         lcsc='C47115471', mpn='AT32F405CCT7')
    decouple('+3V3', '100nF', '100nF', '100nF', '100nF', '4.7uF', '1uF')   # 3x VDD, VDDA (+1 uF)
    R('10k', f'AT{n}_ROLE', 'GND' if n == 1 else '+3V3')   # PC13 role strap: low = PC1, high = PC2
    R('12k', f'AT{n}_RREF', 'GND')                    # OTGHS1_R: 12 kOhm 1 %
    crystal(f'AT{n}_XI', f'AT{n}_XO', '12MHz')        # OTGHS PHY needs exactly 12 MHz
    C('100nF', f'AT{n}_NRST')
    part('J', 'Connector:Conn_ARM_SWD_TagConnect_TC2030-NL', f'SWD AT32 #{n}',
         'Connector:Tag-Connect_TC2030-IDC-NL_2x03_P1.27mm_Vertical',
         {'1': '+3V3', '2': f'AT{n}_SWDIO', '3': f'AT{n}_NRST', '4': f'AT{n}_SWCLK', '5': 'GND', '6': None}, in_bom=False)
    R('10k', '+3V3', f'AT{n}_NRST')                   # defined high while the RP2354B's pads still pull down
    R('10k', f'CC1_PC{n}', f'AT{n}_CC1')              # CC ADC inputs: FTa pins are not 5 V tolerant in analog mode
    R('10k', f'CC2_PC{n}', f'AT{n}_CC2')

# ══ 12-14. Front ports: USB-A, ESD, 1:3 router, current-limited VBUS ═══════════
for k, (p, label) in enumerate([('KB', 'Keyboard'), ('MS', 'Mouse'), ('OT', 'Other')]):
    block(12 + k)
    part('J', 'Connector:USB_A', f'USB-A {label}', 'Connector_USB:USB_A_Molex_67643_Horizontal',
         {'1': f'VBUS_{p}', '2': f'{p}_DM', '3': f'{p}_DP', '4': 'GND', 'SH': 'GND'}, lcsc='C2798029', mpn='676430910')
    usblc6(f'{p}_DM', f'{p}_DP', f'VBUS_{p}')
    part('U', 'KVM:TS3USB3031', 'TS3USB3031', 'Package_DFN_QFN:Texas_RMG0012A_WQFN-12_1.8x1.8mm_P0.4mm',
         # paths chosen by where the pins face on the board: MHL (west) -> Hub-0, USB1 (north-east) -> Hub-2,
         # USB2 (north-west) -> Hub-1.  Hub-0 is on MHL so that both PCs keep the datasheet-specified USB1/USB2
         # channels: only the MHL channel is specified at 1.5-3.3 V signal levels, so the MHL fallback belongs on
         # the hub whose data path is full speed anyway (design.md §7, report.md F4).  The PC-facing hubs therefore
         # own the HS channels, and the router-side lanes are planned for these pin ends (report.md F4).
         {'12': '+3V3', '9': 'GND', '1': f'{p}_SEL0', '2': f'{p}_SEL1', '11': f'{p}_DP', '10': f'{p}_DM',
          '3': f'H2_{p}_DP', '4': f'H2_{p}_DM', '5': f'H1_{p}_DP', '6': f'H1_{p}_DM',
          '7': f'H0_{p}_DP', '8': f'H0_{p}_DM'}, lcsc='C2652459', mpn='TS3USB3031RMGR')
    C('100nF', '+3V3')
    R('100k', f'{p}_SEL0', 'GND')                     # power-up default SEL = 10: MHL = Hub-0, the KVM path, so a
    R('10k', '+3V3', f'{p}_SEL1')                     # strapped board still works without the MCU; 10 k beats the
                                                      # RP2354B's reset pull-down (>= 2.58 V)
    part('U', 'KVM:TPS2553DBV', 'TPS2553', SOT236,
         {'1': '+5V', '3': f'{p}_EN', '6': f'VBUS_{p}', '4': f'{p}_FLT', '5': f'{p}_ILIM', '2': 'GND'},
         lcsc='C55266', mpn='TPS2553DBVR')
    C('1uF', '+5V')
    R('22k', f'{p}_ILIM', 'GND')                      # ~1.18 A
    R('10k', '+3V3', f'{p}_FLT')
    R('100k', f'{p}_EN', 'GND')                       # port unpowered until firmware enables it
    CP(f'VBUS_{p}')                                   # >= 120 uF per port (USB 2.0 7.2.4.1), 10 V polymer

# ══ 15. MX key, status LEDs ═══════════════════════════════════════════════════
block(15)
# assembled by JLC with everything else (the board is double-sided assembly anyway, since 2026-10-27): the hot-swap
# socket for the MX switch (bottom side; the switch itself is plugged in afterwards) and the reverse-mount RGB LED
# that shines up into the switch (bottom side, through a cut-out).  Socket: HanElectricity CPG151101S11-2, the
# same-footprint equivalent of the Kailh part, which had no stock (design.md, rev 2).
part('SW', 'Switch:SW_Push', 'MX key', 'CustomFootprints:SW_MX_Hotswap_Kailh_1u', {'1': 'KEY', '2': 'GND'},
     lcsc='C49352235', mpn='CPG151101S11-2')
C('100nF', 'KEY')                                     # contact bounce/ESD filter with the RP2354B pull-up
part('U', '74xGxx:74AHCT1G125', '74AHCT1G125', SOT235,
     {'5': '+5V', '3': 'GND', '1': 'GND', '2': 'LED_DATA', '4': 'LED_BUF'}, lcsc='C7484', mpn='SN74AHCT1G125DBVR')
R('33R', 'LED_BUF', 'LED_D0')                         # damps ringing on the long run into the chain
C('100nF', '+5V')
# chain: front edge left to right (PC1, KB, MS, OT, PC2, side-emitting), then the key LED under the MX switch.
# The key LED comes last, so the status LEDs work with it missing or damaged.
part('D', 'LED:SK6812MINI-E', 'SK6812MINI-E', 'LED_SMD:LED_SK6812MINI-E_3.2x2.8mm_P1.5mm_ReverseMount',
     {'3': '+5V', '1': 'GND', '2': 'LED_D5', '4': None}, lcsc='C5149201', mpn='SK6812MINI-E')
C('100nF', '+5V')
for k, name in enumerate(['PC1', 'KB', 'MS', 'OT', 'PC2']):
    part('D', 'KVM:SK6812SIDE-A', f'LED {name}', 'CustomFootprints:LED_SK6812SIDE-A_4.0x1.6mm',
         {'2': '+5V', '4': 'GND', '1': f'LED_D{k}', '3': f'LED_D{k + 1}'}, lcsc='C5378721', mpn='SK6812SIDE-A')
    C('100nF', '+5V')
for k in range(4):
    part('H', 'Mechanical:MountingHole', 'M3', 'MountingHole:MountingHole_3.2mm_M3', {}, in_bom=False)

# power flags: rails fed through passives (inductor, resistor) or from connectors
FLAGS = ['GND', '+3V3', 'RP_VREG', 'VBUS_PC1', 'VBUS_PC2', 'VBUS_PWR']

BLOCK_TITLES = {
    1: '1. PC1 USB-C (upstream to Hub-1)', 2: '2. PC2 USB-C (upstream to Hub-2)',
    3: '3. PWR / SERVICE USB-C', 4: '4. Power path (TPS2121 priority, LM66200 PC OR), 3.3 V buck, 1.1 V LDO',
    5: '5. RP2354B: USB host + switching logic (core 1.1 V from external LDO, VREG disabled)',
    6: '6. Service switch: RP2354B USB -> SERVICE port (S=0, default) / Hub-0 (S=1)',
    7: '7. Hub-0 (KVM host side, FS)', 8: '8. Hub-1 (PC1)', 9: '9. Hub-2 (PC2)',
    10: '10. AT32F405 #1 (HS device to PC1)', 11: '11. AT32F405 #2 (HS device to PC2)',
    12: '12. Front port: Keyboard', 13: '13. Front port: Mouse', 14: '14. Front port: Other',
    15: '15. MX key + under-key LED (hand-assembled), status LEDs, mounting holes',
}
# block -> (column x, top y) on the A0 sheet; blocks flow downward within a column
LAYOUT = {1: (20, 30), 2: (20, 120), 3: (20, 210), 4: (20, 300),
          5: (250, 30), 6: (250, 300),
          7: (480, 30), 8: (480, 190), 9: (480, 350),
          10: (710, 30), 11: (710, 230),
          12: (940, 30), 13: (940, 190), 14: (940, 350), 15: (710, 440)}
COLUMN_W = 230
NOTES = [
    'Generated by tools/gen_schematic.py from the netlist in that file - edit the generator, not this sheet.',
    'Design rationale: ../design.md.  Pin numbers of custom symbols: tools/gen_symbols.py.',
]


# ── symbol library handling ───────────────────────────────────────────────────
_libcache = {}


def _lib(libname):
    path = PRJLIB if libname == 'KVM' else f'{SYSLIB}/{libname}.kicad_sym'
    if path not in _libcache:
        _libcache[path] = {s[1]: s for s in find(parse(open(path).read()), 'symbol')}
    return _libcache[path]


def flatten(syms, name):
    s = copy.deepcopy(syms[name])
    ext = first(s, 'extends')
    if not ext:
        return s
    parent = flatten(syms, ext[1])
    child_props = {p[1]: p for p in find(s, 'property')}
    out = ['symbol', Q(name)]
    for item in parent[2:]:
        if isinstance(item, list) and item[0] == 'property':
            out.append(child_props.pop(item[1], item))
        elif isinstance(item, list) and item[0] == 'symbol':
            sub = copy.deepcopy(item)
            sub[1] = Q(name + sub[1][len(ext[1]):])
            out.append(sub)
        else:
            out.append(item)
    idx = next(i for i, it in enumerate(out) if isinstance(it, list) and it[0] == 'symbol')
    out[idx:idx] = list(child_props.values())
    return out


def lib_symbol(lib_id):
    libname, name = lib_id.split(':')
    return flatten(_lib(libname), name)


def symbol_pins(sym, unit):
    pins = {}
    for sub in find(sym, 'symbol'):
        m = re.match(r'.*_(\d+)_(\d+)$', sub[1])
        u, style = int(m.group(1)), int(m.group(2))
        if u not in (0, unit) or style not in (0, 1):
            continue
        for p in find(sub, 'pin'):
            at = first(p, 'at')
            num = first(p, 'number')[1]
            pins[num] = (float(at[1]), float(at[2]), float(at[3]))
    return pins


def body_box(sym):
    """Bounding box of the symbol graphics (symbol coordinates, y up)."""
    xs, ys = [], []
    for sub in find(sym, 'symbol'):
        for r in find(sub, 'rectangle'):
            for k in ('start', 'end'):
                e = first(r, k)
                xs.append(float(e[1]))
                ys.append(float(e[2]))
        for pl in find(sub, 'polyline'):
            for xy in find(first(pl, 'pts'), 'xy'):
                xs.append(float(xy[1]))
                ys.append(float(xy[2]))
        for c in find(sub, 'circle'):
            ctr, rad = first(c, 'center'), float(first(c, 'radius')[1])
            xs += [float(ctr[1]) - rad, float(ctr[1]) + rad]
            ys += [float(ctr[2]) - rad, float(ctr[2]) + rad]
    return (min(xs), min(ys), max(xs), max(ys)) if xs else (-1.27, -1.27, 1.27, 1.27)


def xform(pos, rot, x, y):
    r = math.radians(rot)
    xr = x * math.cos(r) - y * math.sin(r)
    yr = x * math.sin(r) + y * math.cos(r)
    return (round(pos[0] + xr, 3), round(pos[1] - yr, 3))


def step(pt, ang, dist):
    a = math.radians(ang)
    return (round(pt[0] + dist * math.cos(a), 3), round(pt[1] - dist * math.sin(a), 3))


# ── s-expression builders ─────────────────────────────────────────────────────
def uid(*key):
    return ['uuid', Q(str(uuid.uuid5(NS, ':'.join(str(k) for k in key))))]


def effects(size=1.27, justify=None, hide=False):
    e = ['effects', ['font', ['size', size, size]]]
    if justify:
        e.append(['justify'] + justify.split())
    return e


def sym_prop(name, value, at, hide=False, justify=None, ang=0):
    p = ['property', Q(name), Q(value), ['at', at[0], at[1], ang]]
    if hide:
        p.append(['hide', 'yes'])
    p += [['show_name', 'no'], ['do_not_autoplace', 'no'], effects(justify=justify)]
    return p


def wire(a, b):
    return ['wire', ['pts', ['xy', a[0], a[1]], ['xy', b[0], b[1]]],
            ['stroke', ['width', 0], ['type', 'default']], uid('wire', a, b)]


def label(net, at, ang):
    # global, not local: a local label's net is named after its sheet ("/VBUS_PC1"), the board and the router use the
    # bare name, and the DRC's --schematic-parity then reported every labelled net as a net_conflict (199 of them on
    # rev 9) - noise that would hide a real mismatch.  A global label's net carries no sheet prefix.
    just = 'left' if ang in (0, 90) else 'right'
    return ['global_label', Q(net), ['shape', 'passive'], ['at', at[0], at[1], ang], ['fields_autoplaced', 'yes'],
            effects(justify=just), uid('label', net, at),
            ['property', Q('Intersheetrefs'), Q('${INTERSHEET_REFS}'), ['at', at[0], at[1], 0], ['hide', 'yes'],
             effects(justify=just)]]


def instance(ref, unit, lib_id, value, pos, rot, props, pins, in_bom=True, key=None):
    key = key or (ref, unit)
    node = ['symbol', ['lib_id', Q(lib_id)], ['at', pos[0], pos[1], rot], ['unit', unit],
            ['body_style', 1], ['exclude_from_sim', 'no'], ['in_bom', 'yes' if in_bom else 'no'],
            ['on_board', 'yes'], ['in_pos_files', 'yes' if in_bom else 'no'], ['dnp', 'no'],
            uid('sym', *key)]
    node += props
    node += [['pin', Q(n), uid('pin', *key, n)] for n in sorted(pins)]
    node.append(['instances', ['project', Q(PROJECT),
                               ['path', Q('/' + ROOT_UUID), ['reference', Q(ref)], ['unit', unit]]]])
    return node


POWER_LIB = {'GND': 'power:GND', '+3V3': 'power:+3V3', '+5V': 'power:+5V', '+1V1': 'power:+1V1'}
_pwr_count = [0]


def power_symbol(net, at, out_ang):
    _pwr_count[0] += 1
    ref = f'#PWR{_pwr_count[0]:04d}'
    graphic = 270 if net == 'GND' else 90
    rot = (out_ang - graphic) % 360
    vpos = step(at, out_ang, 5.08 if net == 'GND' else 4.45)
    props = [sym_prop('Reference', ref, at, hide=True),
             sym_prop('Value', net, vpos),
             sym_prop('Footprint', '', at, hide=True),
             sym_prop('Datasheet', '', at, hide=True),
             sym_prop('Description', '', at, hide=True)]
    return instance(ref, 1, POWER_LIB[net], net, at, rot, props, ['1'], key=('pwr', net, at))


# ── automatic layout ─────────────────────────────────────────────────────────
def label_len(net):
    return 0 if net is None else (4.0 if net in POWER_LIB else 1.0 * len(net) + 2.0)


def extent(p, sym, pins):
    """Bounding box (relative to the symbol origin, page coordinates) including stubs + labels."""
    bx0, by0, bx1, by1 = body_box(sym)
    xs = [bx0, bx1]
    ys = [-by0, -by1]
    for num, net in p['pins'].items():
        x, y, a = pins[num]
        pt = xform((0, 0), p['rot'], x, y)
        out = (a + 180 + p['rot']) % 360
        end = step(pt, out, STUB + label_len(net) + (2.0 if net in POWER_LIB else 0))
        xs += [pt[0], end[0]]
        ys += [pt[1], end[1]]
        if out in (0, 180):
            ys += [end[1] - 1.5, end[1] + 1.5]
        else:
            xs += [end[0] - 1.5, end[0] + 1.5]
    return min(xs) - 1.27, min(ys) - 3.0, max(xs) + 1.27, max(ys) + 3.0


def snap(v):
    return round(v / GRID) * GRID


def layout(parts):
    """Flow the parts of each block into rows inside its column."""
    for b in sorted({p['block'] for p in parts}):
        x0, y0 = LAYOUT[b]
        x, y, row_h = x0, y0 + 8, 0.0
        for p in [q for q in parts if q['block'] == b]:
            sym = lib_symbol(p['lib_id'])
            pins = symbol_pins(sym, p['unit'])
            ex0, ey0, ex1, ey1 = extent(p, sym, pins)
            w, h = ex1 - ex0, ey1 - ey0
            if x + w > x0 + COLUMN_W and x > x0:
                x, y, row_h = x0, y + row_h + 2.54, 0.0
            p['pos'] = (snap(x - ex0), snap(y - ey0))
            x += w + 2.54
            row_h = max(row_h, h)
        BLOCK_END[b] = y + row_h


BLOCK_END = {}


def main():
    layout(PARTS)
    used_libs = {}
    items = []
    for p in PARTS:
        sym = lib_symbol(p['lib_id'])
        used_libs[p['lib_id']] = sym
        pins = symbol_pins(sym, p['unit'])
        missing = set(p['pins']) - set(pins)
        if missing:
            sys.exit(f"{p['ref']}: pins {missing} not in {p['lib_id']} unit {p['unit']}")
        unmapped = set(pins) - set(p['pins'])
        if unmapped and p['lib_id'] != 'Mechanical:MountingHole':
            sys.exit(f"{p['ref']}: pins {sorted(unmapped)} have no net (use None for no-connect)")

        bx0, by0, bx1, by1 = body_box(sym)
        x0, y0 = p['pos']
        if p['rot'] in (90, 270):
            bx0, by0, bx1, by1 = by0, bx0, by1, bx1
        ref_at = (round(x0 + bx0, 2), round(y0 - by1 - 1.27, 2))
        val_at = (round(x0 + bx0, 2), round(y0 - by0 + 2.54, 2))
        ds = prop(sym, 'Datasheet')
        desc = prop(sym, 'Description')
        props = [sym_prop('Reference', p['ref'], ref_at, justify='left'),
                 sym_prop('Value', p['value'], val_at, justify='left'),
                 sym_prop('Footprint', p['fp'], p['pos'], hide=True),
                 sym_prop('Datasheet', ds[2] if ds else '', p['pos'], hide=True),
                 sym_prop('Description', desc[2] if desc else '', p['pos'], hide=True)]
        if p['lcsc']:
            props.append(sym_prop('LCSC', p['lcsc'], p['pos'], hide=True))
        if p['mpn']:                                  # also hand-fitted parts (no LCSC)
            props.append(sym_prop('MPN', p['mpn'], p['pos'], hide=True))
        items.append(instance(p['ref'], p['unit'], p['lib_id'], p['value'], p['pos'], p['rot'],
                              props, list(pins), in_bom=p['in_bom']))

        done = set()
        for num, net in p['pins'].items():
            x, y, a = pins[num]
            pt = xform(p['pos'], p['rot'], x, y)
            if pt in done:
                continue           # stacked pins share one stub
            done.add(pt)
            if net is None:
                items.append(['no_connect', ['at', pt[0], pt[1]], uid('nc', p['ref'], num)])
                continue
            out = (a + 180 + p['rot']) % 360
            end = step(pt, out, STUB)
            items.append(wire(pt, end))
            if net in POWER_LIB:
                items.append(power_symbol(net, end, out))
            else:
                items.append(label(net, end, int(out)))

    fx, fy = snap(1100.0), snap(760.0)
    for k, net in enumerate(FLAGS):
        at = (round(fx + 10.16 * k, 2), fy)
        b = (at[0], at[1] + 5.08)
        items.append(wire(at, b))
        _pwr_count[0] += 1
        items.append(instance(f'#FLG{_pwr_count[0]:04d}', 1, 'power:PWR_FLAG', 'PWR_FLAG', at, 0,
                              [sym_prop('Reference', f'#FLG{_pwr_count[0]:04d}', at, hide=True),
                               sym_prop('Value', 'PWR_FLAG', (at[0], at[1] - 3.81)),
                               sym_prop('Footprint', '', at, hide=True),
                               sym_prop('Datasheet', '', at, hide=True),
                               sym_prop('Description', '', at, hide=True)],
                              ['1'], key=('flag', net)))
        if net in POWER_LIB:
            items.append(power_symbol(net, b, 270))
        else:
            items.append(label(net, b, 270))

    for net in POWER_LIB:
        used_libs[POWER_LIB[net]] = lib_symbol(POWER_LIB[net])
    used_libs['power:PWR_FLAG'] = lib_symbol('power:PWR_FLAG')

    texts = [((20.32, 20.32), 3.0, 'OpenDynKVM  -  generated schematic (see design.md)')]
    texts += [((LAYOUT[b][0], LAYOUT[b][1] + 3), 2.0, t) for b, t in BLOCK_TITLES.items()]
    texts += [((1100.0, 740.0 + 3.0 * i - 20), 1.27, line) for i, line in enumerate(NOTES)]
    for (x, y), size, text in texts:
        items.append(['text', Q(text), ['exclude_from_sim', 'no'], ['at', x, y, 0],
                      effects(size=size, justify='left bottom'), uid('text', text)])

    lib_symbols = ['lib_symbols']
    for lib_id in sorted(used_libs):
        s = copy.deepcopy(used_libs[lib_id])
        s[1] = Q(lib_id)
        lib_symbols.append(s)

    sch = ['kicad_sch', ['version', '20260306'], ['generator', Q('eeschema')],
           ['generator_version', Q('10.0')], ['uuid', Q(ROOT_UUID)], ['paper', Q('A0')],
           ['title_block', ['title', Q('OpenDynKVM')], ['date', Q('2026-10-22')],
            ['rev', Q('5')], ['comment', 1, Q('Generated by tools/gen_schematic.py - edit the generator, not this file')]],
           lib_symbols] + items + [
           ['sheet_instances', ['path', Q('/'), ['page', Q('1')]]], ['embedded_fonts', 'no']]
    with open(OUT, 'w') as f:
        f.write(dump(sch) + '\n')
    over = {b: round(BLOCK_END[b], 1) for b in BLOCK_END}
    print(f'wrote {OUT}: {len(PARTS)} symbols; block bottoms: {over}')


if __name__ == '__main__':
    main()
