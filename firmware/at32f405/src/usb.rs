//! OTGHS device with the on-chip HS PHY (software.md §5.2), driven by `embassy-usb-synopsys-otg`.
//!
//! The driver is HAL-independent; what Artery needs on top is here: the OTGHS clock, the PHY power-up through
//! `GCCFG` and the UTMI width for `TRDT`. `GCCFG` is written directly because its bits differ from ST's, so the
//! driver's `config_v*` helpers must not be used (ST bit 21 = VBUS detection enable, Artery bit 21 = VBUSIG).

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};

use at32f4xx_pac::at32f405::{self as pac, interrupt};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb_driver::{EndpointAddress, EndpointAllocError, EndpointType, Event, Unsupported};
use embassy_usb_synopsys_otg::otg_v1::{Otg, vals::Dspd};
use embassy_usb_synopsys_otg::{self as otg, OtgInstance, PhyType, StateStorage};
use heapless::Vec;
use kvm_proto::link::LinkMsg;
use kvm_proto::link::msg::{CTRL_REQUEST_CHUNK, SET_REPORT_MAX, UsbState, fragments};
use kvm_usbdev::Cmd;

use crate::link;

const OTGHS_BASE: usize = 0x4004_0000;
const GCCFG: usize = OTGHS_BASE + 0x38;
const GCCFG_PWRDOWN: u32 = 1 << 16; // 1 = transceiver on (RM v2.02 §21.6.3.12)
const GCCFG_VBUSIG: u32 = 1 << 21; // VBUS is measured by the ADC instead
/// 4 KB dedicated FIFO RAM.
const FIFO_DEPTH_WORDS: u16 = 1024;
const EXTRA_RX_FIFO_WORDS: u16 = 30;
const ENDPOINTS: usize = 8;
const FORWARD_TIMEOUT: Duration = Duration::from_millis(500);

static STATE: StateStorage<ENDPOINTS> = StateStorage::new();
static TRDT_HS: AtomicU8 = AtomicU8::new(9);

pub type Report = Vec<u8, 64>;

/// Reports from the RP2354B for IN endpoints 1–7.
pub static IN_QUEUES: [Channel<CriticalSectionRawMutex, Report, 4>; 7] = [const { Channel::new() }; 7];
/// Commands for the USB core (descriptor set committed, attach, detach, wakeup).
pub static COMMANDS: Channel<CriticalSectionRawMutex, Cmd, 4> = Channel::new();
/// The RP2354B's answer to a forwarded control request.
pub static CTRL_REPLY: Signal<CriticalSectionRawMutex, CtrlReply> = Signal::new();
pub static USB_STATE: AtomicU8 = AtomicU8::new(UsbState::Detached as u8);
pub static RESETS: AtomicU16 = AtomicU16::new(0);
/// The PC enabled remote wakeup.
pub static WAKE_ENABLED: AtomicBool = AtomicBool::new(false);
/// The USB core runs (it needs the crystal); without it nothing here touches the OTGHS registers.
static RUNNING: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

pub fn set_running() {
    RUNNING.store(true, Ordering::Relaxed);
}

/// Without USB: takes the RP2354B's commands (descriptor sets, attach) and drops them, so that the service task never
/// waits on a full queue.
#[embassy_executor::task]
pub async fn sink() -> ! {
    loop {
        let _ = COMMANDS.receive().await;
    }
}
/// CRC-32 of the descriptor set the core serves.
pub static ACTIVE_CRC: AtomicU32 = AtomicU32::new(0);
pub static STATUS_DIRTY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub struct CtrlReply {
    pub tag: u8,
    pub stall: bool,
    pub data: Vec<u8, 1024>,
}

fn regs() -> Otg {
    // SAFETY: the OTGHS register block.
    unsafe { Otg::from_ptr(OTGHS_BASE as *mut ()) }
}

#[interrupt]
fn OTGHS() {
    // SAFETY: the driver's own interrupt handler, with its state.
    unsafe { otg::on_interrupt(regs(), &STATE.as_state()) }
}

fn trdt(speed: Dspd) -> u8 {
    match speed {
        Dspd::HIGH_SPEED => TRDT_HS.load(Ordering::Relaxed),
        _ => 6, // full speed with AHB ≥ 32 MHz
    }
}

/// The Synopsys driver plus Artery's bring-up on the first poll.
pub struct At32Driver(otg::Driver<'static>);

pub struct At32Bus {
    inner: otg::Bus<'static>,
    inited: bool,
}

impl At32Driver {
    pub fn new(ep_out_buffer: &'static mut [u8]) -> Self {
        let crm = unsafe { &*pac::CRM::ptr() };
        crm.ahben1().modify(|_, w| w.otghs().set_bit());
        let instance = OtgInstance {
            regs: regs(),
            state: STATE.as_state(),
            fifo_depth_words: FIFO_DEPTH_WORDS,
            phy_type: PhyType::InternalHighSpeed,
            extra_rx_fifo_words: EXTRA_RX_FIFO_WORDS,
            calculate_trdt_fn: trdt,
        };
        Self(otg::Driver::new(ep_out_buffer, instance, otg::Config::default()))
    }
}

impl embassy_usb_driver::Driver<'static> for At32Driver {
    type EndpointOut = otg::Endpoint<'static, otg::Out>;
    type EndpointIn = otg::Endpoint<'static, otg::In>;
    type ControlPipe = otg::ControlPipe<'static>;
    type Bus = At32Bus;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointOut, EndpointAllocError> {
        self.0.alloc_endpoint_out(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointIn, EndpointAllocError> {
        self.0.alloc_endpoint_in(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe) {
        let (inner, cp) = self.0.start(control_max_packet_size);
        (At32Bus { inner, inited: false }, cp)
    }
}

impl At32Bus {
    /// PHY on, core reset, device mode, UTMI width (RM v2.02 §21.3.2: wait 1 ms after PWRDOWN).
    fn init(&mut self) {
        // SAFETY: OTGHS_GCCFG; the HEXT that feeds the PHY has been running since clock init.
        unsafe { core::ptr::write_volatile(GCCFG as *mut u32, GCCFG_PWRDOWN | GCCFG_VBUSIG) };
        cortex_m::asm::delay(crate::clocks::SYSCLK_HZ / 1000);
        self.inner.core_soft_reset();
        self.inner.configure_as_device();
        // 16-bit UTMI at 30 MHz needs TRDT 5, 8-bit at 60 MHz needs 9.
        let wide = regs().hwcfg4().read().utmi_phy_data_width() == 1;
        TRDT_HS.store(if wide { 5 } else { 9 }, Ordering::Relaxed);
        unsafe {
            let mut nvic = cortex_m::Peripherals::steal().NVIC;
            nvic.set_priority(pac::Interrupt::OTGHS, 1 << 4);
            cortex_m::peripheral::NVIC::unmask(pac::Interrupt::OTGHS);
        }
    }
}

impl embassy_usb_driver::Bus for At32Bus {
    async fn enable(&mut self) {
        self.inner.enable().await
    }

    async fn disable(&mut self) {
        self.inner.disable().await
    }

    async fn poll(&mut self) -> Event {
        if !self.inited {
            self.init();
            self.inited = true;
        }
        let ev = self.inner.poll().await;
        if ev == Event::Reset {
            RESETS.fetch_add(1, Ordering::Relaxed);
        }
        ev
    }

    fn endpoint_set_enabled(&mut self, ep_addr: EndpointAddress, enabled: bool) {
        self.inner.endpoint_set_enabled(ep_addr, enabled)
    }

    fn endpoint_set_stalled(&mut self, ep_addr: EndpointAddress, stalled: bool) {
        self.inner.endpoint_set_stalled(ep_addr, stalled)
    }

    fn endpoint_is_stalled(&mut self, ep_addr: EndpointAddress) -> bool {
        self.inner.endpoint_is_stalled(ep_addr)
    }

    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        self.inner.remote_wakeup().await
    }
}

pub fn is_high_speed() -> bool {
    regs().dsts().read().enumspd() == Dspd::HIGH_SPEED
}

pub fn frame_number() -> u16 {
    regs().dsts().read().fnsof()
}

/// Glue between the device core and the link.
pub struct Host {
    next_tag: u8,
}

impl Host {
    pub const fn new() -> Self {
        Self { next_tag: 1 }
    }
}

impl kvm_usbdev::Host for Host {
    async fn command(&mut self) -> Cmd {
        COMMANDS.receive().await
    }

    fn load_set(&mut self, buf: &mut [u8]) -> usize {
        let n = crate::service::committed_set(buf);
        ACTIVE_CRC.store(kvm_proto::crc::crc32(&buf[..n]), Ordering::Relaxed);
        STATUS_DIRTY.signal(());
        n
    }

    fn soft_connect(&mut self, connected: bool) {
        regs().dctl().modify(|w| w.set_sdis(!connected));
    }

    fn is_high_speed(&self) -> bool {
        is_high_speed()
    }

    async fn delay_ms(&mut self, ms: u32) {
        Timer::after_millis(ms as u64).await
    }

    async fn forward(&mut self, setup: [u8; 8], data: &[u8], reply: &mut [u8]) -> Option<usize> {
        let tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1).max(1);
        CTRL_REPLY.reset();
        for (offset, chunk) in fragments(data, CTRL_REQUEST_CHUNK) {
            link::send(&LinkMsg::CtrlRequest { tag, setup, offset, data: chunk }).await;
        }
        loop {
            let r = with_timeout(FORWARD_TIMEOUT, CTRL_REPLY.wait()).await.ok()?;
            if r.tag != tag {
                continue; // a late answer to an earlier request
            }
            if r.stall {
                return None;
            }
            let n = r.data.len().min(reply.len());
            reply[..n].copy_from_slice(&r.data[..n]);
            return Some(n);
        }
    }

    fn set_report(&mut self, iface: u8, report_type: u8, report_id: u8, data: &[u8]) {
        let data = &data[..data.len().min(SET_REPORT_MAX)];
        let _ = link::try_send(&LinkMsg::SetReport { iface, report_type, report_id, data });
    }

    fn protocol_changed(&mut self, iface: u8, protocol: u8) {
        let _ = link::try_send(&LinkMsg::ProtocolChanged { iface, protocol });
    }

    fn state_changed(&mut self, state: UsbState) {
        USB_STATE.store(state as u8, Ordering::Relaxed);
        STATUS_DIRTY.signal(());
    }

    fn wake_enabled_changed(&mut self, enabled: bool) {
        WAKE_ENABLED.store(enabled, Ordering::Relaxed);
        STATUS_DIRTY.signal(());
    }
}

/// Queues a report for IN endpoint `ep` (1–7); a full queue drops its oldest report.
pub fn queue_report(ep: u8, data: &[u8]) {
    let Some(q) = IN_QUEUES.get(ep.wrapping_sub(1) as usize) else { return };
    let Ok(r) = Report::from_slice(&data[..data.len().min(64)]) else { return };
    if let Err(embassy_sync::channel::TrySendError::Full(r)) = q.try_send(r) {
        let _ = q.try_receive();
        let _ = q.try_send(r);
    }
}
