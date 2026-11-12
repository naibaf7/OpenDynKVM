//! OpenDynKVM firmware for the RP2354B (software.md §4): USB host for the front ports, the switching engine, and the
//! SPI links to the two AT32F405s.
//!
//! Core 0 runs everything that talks to the engine; core 1 runs only the two link loops, which have the tightest
//! timing.
#![no_std]
#![no_main]

mod at32isp;
mod board;
mod bt;
mod diag;
mod espisp;
mod flash;
mod host;
mod kvm;
mod link;
mod power;
mod selftest;
mod storage;
mod system;
mod uartlog;
mod ui;
mod update;
mod usbh;

use defmt::{info, unwrap};
use embassy_executor::{Executor, Spawner};
use embassy_rp::adc::{self, Adc};
use embassy_rp::block::{self, Architecture, Block, Security};
use embassy_rp::flash::Flash;
use embassy_rp::gpio::{Flex, Input, Level, Output, Pull};
use embassy_rp::interrupt;
use embassy_rp::multicore::{Stack, spawn_core1};
use embassy_rp::peripherals::{PIO0, SPI0, SPI1, UART0};
use embassy_rp::pio::{self, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::spi::{self, Spi};
use embassy_rp::uart::{self, BufferedUart, UartTx};
use embassy_rp::{bind_interrupts, dma, pac};
use kvm_proto::ctrl::{Info, caps};
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

use crate::board::{At32Lines, Board, FrontPort};

const VERSION: (u8, u8, u8) = kvm_proto::FIRMWARE_VERSION;
pub const FW_VERSION: u32 = kvm_proto::version_u32(VERSION.0, VERSION.1, VERSION.2);
const HW: (u8, u8, u8) = kvm_proto::HARDWARE_VERSION;

/// Which firmware and board this image is: `odkvmctl update` refuses an image for another board.
#[used]
static FW_ID: kvm_proto::rpimage::FwId = kvm_proto::rpimage::FwId::new(FW_VERSION, HW.0);

/// The boot ROM's image definition (RP2350 datasheet §5.9.3), in place of embassy-rp's (feature `imagedef-none`): a
/// Secure Arm executable with a VERSION item (major, minor << 8 | patch), so `picotool info` and the ROM's A/B choice
/// see the firmware version. The update writer sets the TBYB flag on images that arrive over USB (`update`).
#[unsafe(link_section = ".start_block")]
#[used]
static IMAGE_DEF: Block<3> = Block::new([
    block::item_image_type_exe(Security::Secure, Architecture::Arm),
    block::item_generic_2bs(0, 2, block::ITEM_1BS_VERSION),
    (VERSION.0 as u32) << 16 | (VERSION.1 as u32) << 8 | VERSION.2 as u32,
]);

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    UART0_IRQ => uart::BufferedInterruptHandler<UART0>;
    ADC_IRQ_FIFO => adc::InterruptHandler;
    DMA_IRQ_0 => dma::InterruptHandler<embassy_rp::peripherals::DMA_CH0>,
        dma::InterruptHandler<embassy_rp::peripherals::DMA_CH1>,
        dma::InterruptHandler<embassy_rp::peripherals::DMA_CH2>,
        dma::InterruptHandler<embassy_rp::peripherals::DMA_CH3>,
        dma::InterruptHandler<embassy_rp::peripherals::DMA_CH4>,
        dma::InterruptHandler<embassy_rp::peripherals::DMA_CH5>;
});

#[interrupt]
fn USBCTRL_IRQ() {
    usbh::on_interrupt();
}

/// DVDD comes from the external TLV73311, so the on-chip regulator goes high-impedance (design.md §6). Its
/// low-power mode must never be used: there its output is tied to DVDD.
fn vreg_hiz() {
    const POWMAN_PASSWORD: u32 = 0x5AFE << 16;
    let mut v = pac::POWMAN.vreg().read();
    v.set_hiz(true);
    pac::POWMAN.vreg().write_value(pac::powman::regs::Vreg(POWMAN_PASSWORD | (v.0 & 0xFFFF)));
}

fn serial() -> [u8; 8] {
    // Hex of the low 32 bits of the chip ID.
    let id = embassy_rp::otp::get_chipid().unwrap_or(0) as u32;
    let mut s = [b'0'; 8];
    for (i, c) in s.iter_mut().enumerate() {
        let nibble = (id >> (28 - 4 * i)) & 0xF;
        *c = b"0123456789ABCDEF"[nibble as usize];
    }
    s
}

#[embassy_executor::task]
async fn link1(
    spi: Spi<'static, SPI0, spi::Async>,
    cs: Output<'static>,
    irq: Input<'static>,
    lines: At32Lines,
    seed: u8,
) -> ! {
    link::run(0, spi, cs, irq, lines, seed).await
}

#[embassy_executor::task]
async fn link2(
    spi: Spi<'static, SPI1, spi::Async>,
    cs: Output<'static>,
    irq: Input<'static>,
    lines: At32Lines,
    seed: u8,
) -> ! {
    link::run(1, spi, cs, irq, lines, seed).await
}

static CORE1_STACK: StaticCell<Stack<8192>> = StaticCell::new();
static BT_TX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
static BT_RX_BUF: StaticCell<[u8; 512]> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut p = embassy_rp::init(Default::default());
    vreg_hiz();
    diag::note_reset_cause();
    info!("OpenDynKVM RP2354B firmware {=u32:#x}", FW_VERSION);
    let mut uart_cfg = uart::Config::default();
    uart_cfg.baudrate = uartlog::BAUD;
    spawner.spawn(unwrap!(uartlog::run(UartTx::new(p.UART1, p.PIN_36, p.DMA_CH5, Irqs, uart_cfg))));
    let (v, hw) = (VERSION, HW);
    uartlog::uart_log!("OpenDynKVM RP2354B firmware {}.{}.{}, hardware {}.{}.{}", v.0, v.1, v.2, hw.0, hw.1, hw.2);

    // Hub-0 out of reset and told that its upstream is powered; the RP2354B's USB goes to Hub-0 (SVC_SEL low = Hub-0: the
    // service connector sits on the switch's USB2 channel since rev 8, so SEL0 is inverted - gen_schematic.py block 6).
    let mut svc_sel = Output::new(p.PIN_0, Level::High);
    let hub0_rst = Output::new(p.PIN_2, Level::High);
    let hub0_vbus = Output::new(p.PIN_3, Level::High);
    svc_sel.set_low();

    // Routers on Hub-0 (the KVM path), as the board's straps hold them at power-up: SEL1/SEL0 = 1/0 = MHL
    // (`FrontPort::route`, TS3USB3031 Table 6-1). The engine starts from that and routes a port only when it leaves it.
    // Until 0.1.5 this set 0/1, an older channel assignment: every front port went to PC1's hub and the KVM never saw
    // its device (first board, 2026-11-12). VBUS off until the port tasks switch it on.
    let ports = [
        FrontPort {
            sel0: Output::new(p.PIN_20, Level::Low),
            sel1: Output::new(p.PIN_21, Level::High),
            en: Output::new(p.PIN_22, Level::Low),
        },
        FrontPort {
            sel0: Output::new(p.PIN_24, Level::Low),
            sel1: Output::new(p.PIN_25, Level::High),
            en: Output::new(p.PIN_26, Level::Low),
        },
        FrontPort {
            sel0: Output::new(p.PIN_34, Level::Low),
            sel1: Output::new(p.PIN_37, Level::High),
            en: Output::new(p.PIN_32, Level::Low),
        },
    ];
    let board = Board { ports, hub0_rst, _hub0_vbus: hub0_vbus, svc_sel };

    flash::init(Flash::new_blocking(p.FLASH));
    let (store, config) = storage::open();
    let updates = update::init().is_some();
    let info = Info {
        proto: kvm_proto::ctrl::PROTOCOL_VERSION,
        rp_fw: FW_VERSION,
        at_fw: [0; 2],
        hw: kvm_proto::version_u32(HW.0, HW.1, HW.2),
        serial: serial(),
        caps: caps::MIRRORED
            | caps::IDENTITY
            | caps::PASSTHROUGH
            | caps::VIAL
            | if updates { caps::FW_UPDATE } else { 0 },
        bt_fw: 0,
    };
    let serial_str = core::str::from_utf8(&info.serial).unwrap_or("");
    kvm::init(config, info, serial_str, board);
    storage::restore(&store);

    // The KVM's key held at power-up: the AT32F405s and the Bluetooth module get up to 5 more programming attempts
    // (software.md §4.9, §4.13), and switching and configuration from software are allowed again (§4.6). Read before
    // the links start, which decide about programming.
    let key = Input::new(p.PIN_11, Pull::Up);
    let held = ui::held_at_power_up(&key).await;
    if held {
        kvm::with(|e| e.allow_software(true, true));
        info!("key held at power-up: programming attempts start again, software control allowed");
        uartlog::uart_log!("key held at power-up: programming attempts start again, software control allowed");
    }
    storage::load_at32_record(&store, link::image_crc(), held);

    // Core 1: the two SPI links, each with its AT32F405's reset and boot lines and its PC hub's reset.
    let at1 = At32Lines::new(Flex::new(p.PIN_13), Output::new(p.PIN_14, Level::Low), Flex::new(p.PIN_6));
    let at2 = At32Lines::new(Flex::new(p.PIN_39), Output::new(p.PIN_43, Level::Low), Flex::new(p.PIN_35));
    let mut spi_cfg = spi::Config::default();
    spi_cfg.frequency = link::SPI_HZ;
    let spi0 = Spi::new(p.SPI0, p.PIN_18, p.PIN_19, p.PIN_16, p.DMA_CH0, p.DMA_CH1, Irqs, spi_cfg.clone());
    let spi1 = Spi::new(p.SPI1, p.PIN_46, p.PIN_47, p.PIN_44, p.DMA_CH2, p.DMA_CH3, Irqs, spi_cfg);
    let cs1 = Output::new(p.PIN_17, Level::High);
    let cs2 = Output::new(p.PIN_45, Level::High);
    let irq1 = Input::new(p.PIN_12, Pull::Down);
    let irq2 = Input::new(p.PIN_38, Pull::Down);
    let seed = embassy_rp::clocks::RoscRng.next_u32();
    spawn_core1(p.CORE1, CORE1_STACK.init(Stack::new()), move || {
        let executor = EXECUTOR1.init(Executor::new());
        executor.run(|s| {
            s.spawn(unwrap!(link1(spi0, cs1, irq1, at1, seed as u8)));
            s.spawn(unwrap!(link2(spi1, cs2, irq2, at2, (seed >> 8) as u8)));
        });
    });

    // Core 0.
    // The Bluetooth module (boards 0.16.0 on, when fitted; software.md §4.13): UART0 on GPIO28/29. Fitted or not: its
    // TXD0 idles high on GPIO29, which the pad's pull-down holds low on a board without it (or before 0.16.0).
    let bt_fitted = {
        let mut rx = Flex::new(p.PIN_29.reborrow());
        rx.set_as_input();
        rx.set_pull(Pull::Down);
        embassy_time::Timer::after_millis(20).await;
        let high = rx.is_high();
        rx.set_pull(Pull::None);
        high
    };
    if bt_fitted {
        storage::load_bt_record(&store, bt::image_crc(), held);
    }
    let mut bt_cfg = uart::Config::default();
    bt_cfg.baudrate = kvm_proto::link::bt::BAUD;
    let bt_uart = BufferedUart::new(
        p.UART0,
        p.PIN_28,
        p.PIN_29,
        Irqs,
        BT_TX_BUF.init([0; 256]),
        BT_RX_BUF.init([0; 512]),
        bt_cfg,
    );
    spawner.spawn(unwrap!(bt::run(bt_uart, bt_fitted, (seed >> 16) as u8)));

    let (root, handle) = host::start(usbh::RpHost::new(p.USB));
    spawner.spawn(unwrap!(host::hub_task(root, handle.clone())));
    for i in 0..3 {
        spawner.spawn(unwrap!(host::port_task(i, handle.clone())));
    }
    for k in 0..2 {
        spawner.spawn(unwrap!(link::dispatch(k)));
        spawner.spawn(unwrap!(link::uploader(k)));
    }

    let mut pio = Pio::new(p.PIO0, Irqs);
    let program = PioWs2812Program::new(&mut pio.common);
    let leds = PioWs2812::new(&mut pio.common, pio.sm0, p.DMA_CH4, Irqs, p.PIN_15, &program);
    spawner.spawn(unwrap!(ui::run(key, leds)));

    let adc = Adc::new(p.ADC, Irqs, adc::Config::default());
    let inputs = power::Inputs {
        adc,
        vsns: adc::Channel::new_pin(p.PIN_40, Pull::None),
        cc1: adc::Channel::new_pin(p.PIN_41, Pull::None),
        cc2: adc::Channel::new_pin(p.PIN_42, Pull::None),
        temp: adc::Channel::new_temp_sensor(p.ADC_TEMP_SENSOR),
        pwr_st: Input::new(p.PIN_1, Pull::None),
    };
    spawner.spawn(unwrap!(power::monitor(inputs)));
    for (i, pin) in
        [Input::new(p.PIN_23, Pull::None), Input::new(p.PIN_27, Pull::None), Input::new(p.PIN_33, Pull::None)]
            .into_iter()
            .enumerate()
    {
        spawner.spawn(unwrap!(power::port_power(i, pin)));
    }
    spawner.spawn(unwrap!(storage::run(store)));
    spawner.spawn(unwrap!(update::run()));
    spawner.spawn(unwrap!(selftest::run()));
    spawner.spawn(unwrap!(system::ticker()));
    spawner.spawn(unwrap!(system::reboots()));
    spawner.spawn(unwrap!(diag::run()));
}
