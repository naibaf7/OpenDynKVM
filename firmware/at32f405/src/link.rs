//! SPI1 slave link to the RP2354B (software.md §5.3, §6).
//!
//! DMA1 channel 1 receives and channel 2 transmits exactly one 72-byte frame. CS (PA4) rising ends the frame: the
//! EXINT4 handler runs the ARQ's receive half, builds the next frame and re-arms both channels. The RP2354B keeps at
//! least 10 µs between frames for this.

use core::cell::{RefCell, UnsafeCell};
use core::sync::atomic::{AtomicU32, Ordering};

use at32f4xx_pac::at32f405::{self as pac, interrupt};
use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use kvm_proto::link::{Arq, FRAME_LEN, LinkMsg, MAX_MSG, Msg};

use crate::board;

const QUEUE: usize = 16;
/// DMAMUX request lines (RM v2.02 table 9-3).
const REQ_SPI1_RX: u8 = 10;
const REQ_SPI1_TX: u8 = 11;

struct Buffers {
    rx: UnsafeCell<[u8; FRAME_LEN]>,
    tx: UnsafeCell<[u8; FRAME_LEN]>,
}

// SAFETY: the buffers are touched by the DMA and by the EXINT4 handler only while both DMA channels are disabled.
unsafe impl Sync for Buffers {}

static BUF: Buffers = Buffers { rx: UnsafeCell::new([0; FRAME_LEN]), tx: UnsafeCell::new([0; FRAME_LEN]) };
static ARQ: Mutex<RefCell<Arq<QUEUE>>> = Mutex::new(RefCell::new(Arq::new(0)));

/// Messages from the RP2354B, in order.
pub static INBOX: Channel<CriticalSectionRawMutex, Msg, 24> = Channel::new();
pub static SHORT_FRAMES: AtomicU32 = AtomicU32::new(0);
pub static INBOX_STALLS: AtomicU32 = AtomicU32::new(0);

fn spi() -> &'static pac::spi1::RegisterBlock {
    unsafe { &*pac::SPI1::ptr() }
}

fn dma() -> &'static pac::dma1::RegisterBlock {
    unsafe { &*pac::DMA1::ptr() }
}

/// Re-arms SPI1 and both DMA channels for the next frame. Called with CS high.
fn arm() {
    let s = spi();
    let d = dma();
    s.ctrl1().modify(|_, w| w.spien().clear_bit());
    d.channel1().ctrl().modify(|_, w| w.chen().clear_bit());
    d.channel2().ctrl().modify(|_, w| w.chen().clear_bit());
    d.clr().write(|w| w.gfc1().set_bit().gfc2().set_bit());
    // Drop a stale received byte, if any.
    let _ = s.dt().read().bits();
    d.channel1().dtcnt().write(|w| unsafe { w.bits(FRAME_LEN as u16) });
    d.channel2().dtcnt().write(|w| unsafe { w.bits(FRAME_LEN as u16) });
    d.channel1().ctrl().modify(|_, w| w.chen().set_bit());
    d.channel2().ctrl().modify(|_, w| w.chen().set_bit());
    // Enabling SPI raises the TX DMA request, which preloads the first byte before CS falls.
    s.ctrl1().modify(|_, w| w.spien().set_bit());
}

/// `seed` makes the ARQ's first sequence number differ between boots.
pub fn init(seed: u8) {
    let crm = unsafe { &*pac::CRM::ptr() };
    crm.apb2en().modify(|_, w| w.spi1().set_bit().scfg().set_bit());
    crm.ahben1().modify(|_, w| w.dma1().set_bit());

    critical_section::with(|cs| *ARQ.borrow_ref_mut(cs) = Arq::new(seed));

    let d = dma();
    d.dma_muxsel().write(|w| w.tbl_sel().set_bit());
    d.muxc1ctrl().write(|w| unsafe { w.reqsel().bits(REQ_SPI1_RX) });
    d.muxc2ctrl().write(|w| unsafe { w.reqsel().bits(REQ_SPI1_TX) });
    let dt = spi().dt().as_ptr() as u32;
    d.channel1().paddr().write(|w| unsafe { w.bits(dt) });
    d.channel1().maddr().write(|w| unsafe { w.bits(BUF.rx.get() as u32) });
    d.channel1().ctrl().write(|w| unsafe { w.dtd().clear_bit().mincm().set_bit().chpl().bits(0b11) });
    d.channel2().paddr().write(|w| unsafe { w.bits(dt) });
    d.channel2().maddr().write(|w| unsafe { w.bits(BUF.tx.get() as u32) });
    d.channel2().ctrl().write(|w| unsafe { w.dtd().set_bit().mincm().set_bit().chpl().bits(0b11) });

    // Slave, mode 0, 8 bit, MSB first, hardware CS; both DMA requests on.
    spi().ctrl1().write(|w| w.msten().clear_bit().clkpol().clear_bit().clkpha().clear_bit().swcsen().clear_bit());
    spi().ctrl2().write(|w| w.dmaren().set_bit().dmaten().set_bit());

    critical_section::with(|cs| {
        let f = ARQ.borrow_ref_mut(cs).next_frame();
        // SAFETY: DMA not armed yet.
        f.encode(unsafe { &mut *BUF.tx.get() });
    });
    arm();

    // CS rising edge on PA4 → EXINT line 4.
    let scfg = unsafe { &*pac::SCFG::ptr() };
    let exint = unsafe { &*pac::EXINT::ptr() };
    scfg.exintc2().modify(|_, w| unsafe { w.exint4().bits(0) });
    exint.polcfg1().modify(|_, w| w.rp4().set_bit());
    exint.inten().modify(|_, w| w.inten4().set_bit());
    unsafe {
        let mut nvic = cortex_m::Peripherals::steal().NVIC;
        nvic.set_priority(pac::Interrupt::EXINT4, 0);
        cortex_m::peripheral::NVIC::unmask(pac::Interrupt::EXINT4);
    }
}

#[interrupt]
fn EXINT4() {
    let exint = unsafe { &*pac::EXINT::ptr() };
    exint.intsts().write(|w| w.line4().clear_bit_by_one());

    let received = FRAME_LEN - dma().channel1().dtcnt().read().bits() as usize;
    spi().ctrl1().modify(|_, w| w.spien().clear_bit());
    dma().channel1().ctrl().modify(|_, w| w.chen().clear_bit());
    dma().channel2().ctrl().modify(|_, w| w.chen().clear_bit());

    critical_section::with(|cs| {
        let mut arq = ARQ.borrow_ref_mut(cs);
        // SAFETY: both channels are disabled.
        let rx = unsafe { &*BUF.rx.get() };
        // A short frame, or no room to keep a message, is handled as a lost frame: the RP2354B sends it again.
        let usable = received == FRAME_LEN && !INBOX.is_full();
        if received != FRAME_LEN {
            SHORT_FRAMES.fetch_add(1, Ordering::Relaxed);
        } else if INBOX.is_full() {
            INBOX_STALLS.fetch_add(1, Ordering::Relaxed);
        }
        let got = if usable { arq.receive(rx) } else { arq.receive(&[0; FRAME_LEN]) };
        if let Some(m) = got {
            let _ = INBOX.try_send(m);
        }
        let f = arq.next_frame();
        f.encode(unsafe { &mut *BUF.tx.get() });
        board::set_irq(arq.has_pending());
    });
    arm();
}

/// Queues a message for the RP2354B; `false` if the queue is full.
pub fn try_send(m: &LinkMsg) -> bool {
    let mut buf = [0u8; MAX_MSG];
    let Ok(n) = m.encode(&mut buf) else { return false };
    critical_section::with(|cs| {
        let mut arq = ARQ.borrow_ref_mut(cs);
        let ok = arq.push(&buf[..n], m.prio()).is_ok();
        board::set_irq(arq.has_pending());
        ok
    })
}

/// Queues a message, waiting for room.
pub async fn send(m: &LinkMsg<'_>) {
    while !try_send(m) {
        Timer::after_millis(1).await;
    }
}
