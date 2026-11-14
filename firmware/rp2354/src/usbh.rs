//! Host-mode driver for the RP2350's native USB controller (RP2350 datasheet §12.7), implementing the
//! `embassy-usb-driver` host traits so that `embassy-usb-host` does enumeration and the hub (software.md §4.2).
//!
//! - **EPX** (the one software-scheduled endpoint) carries every control transfer, one packet per transaction:
//!   SETUP, each data packet, then the status packet. Control pipes share it through [`EPX_LOCK`].
//! - **Interrupt pipes** each own one of the 15 hardware-polled slots (`ADDR_ENDP1..15`, `INT_EP_CTRL`).
//! - **The Bluetooth module on a front port** (software.md §4.13): its bulk endpoints are polled by interrupt slots too,
//!   every millisecond (a device answers IN and OUT tokens the same whatever the host's schedule), so that EPX carries
//!   control transfers only. Bulk over EPX, which stops on the device's NAK (`NAK_POLL.STOP_EPX_ON_NAK`), left control
//!   transfers failing after it on the first dongle (2026-11-14).
//! - **Low-speed devices behind Hub-0** get the `PRE` preamble (`SIE_CTRL.PREAMBLE_EN`, `ADDR_ENDPn.INTEP_PREAMBLE`).
//! - **D+/D− polarity** (hardware/errata.md E1): boards 0.8–0.12 cross the pair between the RP2354B and Hub-0, so the
//!   PHY swaps it back (`USB_MUXING.SWAP_DPDM`). The root port works out which board it is on: see [`CROSSED_AT_START`].
//!
//! Host-mode DPRAM layout: setup packet 0x000, slot n control 0x008 + 8(n−1), EPX buffer control 0x080, slot n buffer
//! control 0x080 + 8n, EPX control 0x100, EPX data 0x180, slot n data 0x200 + 64(n−1).
//!
//! Not yet exercised on hardware (milestone M1).

use core::future::poll_fn;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, Ordering, compiler_fence};
use core::task::Poll;

use embassy_rp::pac;
use embassy_rp::pac::usb::regs::{AddrEndp, AddrEndpX, BuffStatus, Int, SieCtrl, SieStatus};
use embassy_rp::pac::usb_dpram::regs::{EpBufferControl, EpControl};
use embassy_rp::pac::usb_dpram::vals::EpControlEndpointType as EpType;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb_driver::host::{
    DeviceEvent, HostError, PipeError, SplitInfo, SplitSpeed, TimeoutConfig, UsbHostAllocator, UsbHostController,
    UsbPipe, pipe,
};
use embassy_usb_driver::{EndpointInfo, EndpointType, Speed};

use crate::uartlog::uart_log;

const DPRAM: usize = 0x5010_0000;
const SETUP_PACKET: usize = DPRAM;
const EPX_BUF_CTRL: usize = DPRAM + 0x80;
const EPX_CTRL: usize = DPRAM + 0x100;
const EPX_DATA_OFFSET: u16 = 0x180;
const SLOTS: usize = 15;

static EPX_WAKER: AtomicWaker = AtomicWaker::new();
/// SIE_STATUS event bits seen by the interrupt handler since the current EPX transaction started.
static EPX_EVENTS: AtomicU32 = AtomicU32::new(0);
static SLOT_WAKERS: [AtomicWaker; SLOTS] = [const { AtomicWaker::new() }; SLOTS];
static CONN_WAKER: AtomicWaker = AtomicWaker::new();
/// Bit n = interrupt slot n in use.
static SLOTS_USED: AtomicU16 = AtomicU16::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

/// Serialises EPX transfers between all control pipes.
pub static EPX_LOCK: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

fn slot_ctrl(n: usize) -> usize {
    DPRAM + 0x08 + 8 * (n - 1)
}

fn slot_buf_ctrl(n: usize) -> usize {
    DPRAM + 0x80 + 8 * n
}

fn slot_data_offset(n: usize) -> u16 {
    0x200 + 64 * (n as u16 - 1)
}

fn wr(addr: usize, v: u32) {
    // SAFETY: DPRAM register or buffer word owned by this driver.
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

fn rd(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

fn copy_to_dpram(offset: u16, data: &[u8]) {
    compiler_fence(Ordering::SeqCst);
    // SAFETY: the DPRAM buffer at `offset` is at least 64 bytes and owned by the caller's endpoint.
    let dst = unsafe { core::slice::from_raw_parts_mut((DPRAM + offset as usize) as *mut u8, data.len()) };
    dst.copy_from_slice(data);
    compiler_fence(Ordering::SeqCst);
}

fn copy_from_dpram(offset: u16, out: &mut [u8]) {
    compiler_fence(Ordering::SeqCst);
    let src = unsafe { core::slice::from_raw_parts((DPRAM + offset as usize) as *const u8, out.len()) };
    out.copy_from_slice(src);
    compiler_fence(Ordering::SeqCst);
}

/// The buffer control needs its AVAILABLE bit written after the rest settled (RP2350 datasheet §12.7.3.3).
fn arm_buffer(addr: usize, mut v: EpBufferControl) {
    v.set_available(0, false);
    wr(addr, v.0);
    cortex_m::asm::delay(12);
    v.set_available(0, true);
    wr(addr, v.0);
}

fn base_sie_ctrl() -> SieCtrl {
    let mut c = SieCtrl(0);
    c.set_pulldown_en(true);
    c.set_vbus_en(true);
    c.set_keep_alive_en(true);
    c.set_sof_en(true);
    c
}

/// SIE_STATUS bits that end an EPX transaction.
fn epx_event_mask() -> u32 {
    let mut s = SieStatus(0);
    s.set_trans_complete(true);
    s.set_stall_rec(true);
    s.set_data_seq_error(true);
    s.set_rx_timeout(true);
    s.set_rx_overflow(true);
    s.set_bit_stuff_error(true);
    s.set_crc_error(true);
    s.0
}

fn epx_inte() -> Int {
    let mut i = Int(0);
    i.set_trans_complete(true);
    i.set_stall(true);
    i.set_error_data_seq(true);
    i.set_error_rx_timeout(true);
    i.set_error_rx_overflow(true);
    i.set_error_bit_stuff(true);
    i.set_error_crc(true);
    i
}

/// USBCTRL_IRQ handler.
pub fn on_interrupt() {
    let r = pac::USB;
    let ints = r.ints().read();
    if ints.host_conn_dis() {
        // Writing the speed bits clears the connect/disconnect interrupt.
        r.sie_status().write(|w| w.set_speed(0b11));
        CONN_WAKER.wake();
    }
    if ints.buff_status() {
        let bs = r.buff_status().read().0;
        r.buff_status().write_value(BuffStatus(bs));
        for n in 1..=SLOTS {
            if bs & (0b11 << (2 * n)) != 0 {
                SLOT_WAKERS[n - 1].wake();
            }
        }
    }
    let st = r.sie_status().read().0 & epx_event_mask();
    if st != 0 {
        r.sie_status().write_value(SieStatus(st));
        EPX_EVENTS.fetch_or(st, Ordering::Release);
        EPX_WAKER.wake();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Setup,
    In,
    Out,
}

/// Stops an EPX transaction that is abandoned (timeout or a dropped future).
struct AbortOnDrop;

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        let mut c = base_sie_ctrl();
        c.set_stop_trans(true);
        pac::USB.sie_ctrl().write_value(c);
        // Until the controller has stopped (TinyUSB's `sie_stop_xfer` waits the same way): a transaction started
        // before that goes wrong.
        for _ in 0..10_000 {
            if !pac::USB.sie_ctrl().read().stop_trans() {
                break;
            }
        }
        wr(EPX_CTRL, 0);
    }
}

struct Target {
    addr: u8,
    ep: u8,
    preamble: bool,
    ty: EpType,
    mps: u16,
}

/// One EPX transaction. Returns the number of bytes received (IN) or 0. A NAK is retried by the controller.
async fn epx(
    t: &Target,
    stage: Stage,
    data1: bool,
    out: &[u8],
    inbuf: &mut [u8],
    timeout: Duration,
) -> Result<usize, PipeError> {
    let r = pac::USB;
    let mut ctrl = EpControl(0);
    ctrl.set_enable(true);
    ctrl.set_interrupt_per_buff(true);
    ctrl.set_endpoint_type(t.ty);
    ctrl.set_buffer_address(EPX_DATA_OFFSET);
    wr(EPX_CTRL, ctrl.0);

    let mut bc = EpBufferControl(0);
    bc.set_last(0, true);
    bc.set_pid(0, data1);
    match stage {
        Stage::Setup => {
            let mut s = [0u8; 8];
            s.copy_from_slice(&out[..8]);
            wr(SETUP_PACKET, u32::from_le_bytes([s[0], s[1], s[2], s[3]]));
            wr(SETUP_PACKET + 4, u32::from_le_bytes([s[4], s[5], s[6], s[7]]));
            wr(EPX_BUF_CTRL, 0);
        }
        Stage::Out => {
            copy_to_dpram(EPX_DATA_OFFSET, out);
            bc.set_length(0, out.len() as u16);
            bc.set_full(0, true);
            arm_buffer(EPX_BUF_CTRL, bc);
        }
        Stage::In => {
            bc.set_length(0, t.mps);
            arm_buffer(EPX_BUF_CTRL, bc);
        }
    }
    r.addr_endp().write_value({
        let mut a = AddrEndp(0);
        a.set_address(t.addr);
        a.set_endpoint(t.ep);
        a
    });
    EPX_EVENTS.store(0, Ordering::Release);
    r.sie_status().write_value(SieStatus(epx_event_mask()));
    r.inte().modify(|w| w.0 |= epx_inte().0);

    let mut sc = base_sie_ctrl();
    sc.set_preamble_en(t.preamble);
    match stage {
        Stage::Setup => sc.set_send_setup(true),
        Stage::In => sc.set_receive_data(true),
        Stage::Out => sc.set_send_data(true),
    }
    r.sie_ctrl().write_value(sc);
    cortex_m::asm::delay(12);
    sc.set_start_trans(true);
    r.sie_ctrl().write_value(sc);

    let guard = AbortOnDrop;
    let events = with_timeout(
        timeout,
        poll_fn(|cx| {
            EPX_WAKER.register(cx.waker());
            match EPX_EVENTS.swap(0, Ordering::AcqRel) {
                0 => Poll::Pending,
                e => Poll::Ready(SieStatus(e)),
            }
        }),
    )
    .await;
    let Ok(st) = events else {
        drop(guard);
        return Err(PipeError::Timeout);
    };
    core::mem::forget(guard);

    if st.stall_rec() {
        return Err(PipeError::Stall);
    }
    if st.data_seq_error() {
        return Err(PipeError::DataToggleError);
    }
    if st.rx_overflow() {
        return Err(PipeError::Babble);
    }
    if st.rx_timeout() {
        return Err(PipeError::Timeout);
    }
    if st.crc_error() || st.bit_stuff_error() {
        return Err(PipeError::BadResponse);
    }
    if stage != Stage::In {
        return Ok(0);
    }
    let done = EpBufferControl(rd(EPX_BUF_CTRL));
    let n = (done.length(0) as usize).min(inbuf.len());
    copy_from_dpram(EPX_DATA_OFFSET, &mut inbuf[..n]);
    Ok(n)
}

/// Errata E1 (hardware/errata.md): on boards 0.8–0.12 the service switch U601 crosses D+ and D− between the RP2354B
/// and Hub-0, while its path to the PWR/SERVICE port is straight. So host mode starts with the PHY swapping the pair
/// (`USB_MUXING.SWAP_DPDM`), and the boot ROM's UF2 mode on the service port, which starts with it cleared, needs
/// nothing. A board with the crossing fixed is recognised at run time, because the board has no ID to tell: only Hub-0
/// is ever on the root port and a hub is never low speed, so a low-speed attach means its D+ pull-up arrives on our D−.
/// The root port then flips the swap once and looks again ([`RpHost::wait_for_device_event`]).
const CROSSED_AT_START: bool = true;

/// Resets the USB controller and starts it in host mode, with D+/D− swapped in the PHY if `swap`.
fn start_controller(swap: bool) {
    cortex_m::peripheral::NVIC::mask(pac::Interrupt::USBCTRL_IRQ);
    let resets = pac::RESETS;
    resets.reset().modify(|w| w.set_usbctrl(true));
    resets.reset().modify(|w| w.set_usbctrl(false));
    while !resets.reset_done().read().usbctrl() {}
    // Clear DPRAM control words (registers and buffers we use).
    for off in (0..0x600).step_by(4) {
        wr(DPRAM + off, 0);
    }
    let r = pac::USB;
    r.usb_muxing().write(|w| {
        w.set_to_phy(true);
        w.set_softcon(true);
        w.set_swap_dpdm(swap);
    });
    crate::diag::PHY_SWAP.store(swap, Ordering::Relaxed);
    r.usb_pwr().write(|w| {
        w.set_vbus_detect(true);
        w.set_vbus_detect_override_en(true);
    });
    // PHY_ISO stays 0: the PHY is connected (RP2350 adds this isolation bit).
    r.main_ctrl().write(|w| {
        w.set_host_ndevice(true);
        w.set_controller_en(true);
    });
    r.sie_ctrl().write_value(base_sie_ctrl());
    r.inte().write(|w| {
        w.set_host_conn_dis(true);
        w.set_buff_status(true);
    });
    // SAFETY: the handler only touches this driver's state.
    unsafe {
        cortex_m::peripheral::NVIC::unpend(pac::Interrupt::USBCTRL_IRQ);
        cortex_m::peripheral::NVIC::unmask(pac::Interrupt::USBCTRL_IRQ);
    }
}

/// The root port: Hub-0 on the full-speed bus.
pub struct RpHost {
    connected: bool,
    /// D+/D− swapped in the PHY ([`CROSSED_AT_START`]).
    swapped: bool,
    /// The swap was flipped after a low-speed attach; it is flipped at most once.
    flipped: bool,
}

impl RpHost {
    /// Takes the USB controller into host mode. `_usb` proves ownership of the peripheral.
    pub fn new(_usb: embassy_rp::Peri<'static, embassy_rp::peripherals::USB>) -> Self {
        if STARTED.swap(true, Ordering::AcqRel) {
            panic!("usb host started twice");
        }
        start_controller(CROSSED_AT_START);
        Self { connected: false, swapped: CROSSED_AT_START, flipped: false }
    }

    fn speed() -> u8 {
        pac::USB.sie_status().read().speed()
    }
}

#[derive(Clone, Copy)]
pub struct RpAlloc;

impl<'d> UsbHostController<'d> for RpHost {
    type Allocator = RpAlloc;

    fn allocator(&self) -> RpAlloc {
        RpAlloc
    }

    async fn wait_for_device_event(&mut self) -> DeviceEvent {
        loop {
            match (self.connected, Self::speed()) {
                (false, s @ (1 | 2)) => {
                    // USB 2.0 §7.1.7.3: let the connection settle before the reset.
                    Timer::after_millis(100).await;
                    if Self::speed() != s {
                        continue;
                    }
                    if s == 1 && !self.flipped {
                        // Hub-0 as low speed: the pair arrives crossed for this swap setting (CROSSED_AT_START).
                        // Nothing is enumerated yet, so the controller simply starts over with the other setting.
                        self.flipped = true;
                        self.swapped = !self.swapped;
                        crate::diag::PHY_SWAP_FOUND.store(true, Ordering::Relaxed);
                        defmt::warn!("Hub-0 attached as low speed: D+/D- crossed, PHY swap now {}", self.swapped);
                        uart_log!("USB: Hub-0 seen as low speed, D+/D- swap now {} (errata E1)", self.swapped);
                        start_controller(self.swapped);
                        continue;
                    }
                    self.bus_reset().await;
                    self.connected = true;
                    crate::diag::ROOT_SPEED.store(s, Ordering::Relaxed);
                    return DeviceEvent::Connected(if s == 1 { Speed::Low } else { Speed::Full });
                }
                (true, 0) => {
                    self.connected = false;
                    crate::diag::ROOT_SPEED.store(0, Ordering::Relaxed);
                    return DeviceEvent::Disconnected;
                }
                _ => {}
            }
            poll_fn(|cx| {
                CONN_WAKER.register(cx.waker());
                let now = Self::speed() != 0;
                if now != self.connected { Poll::Ready(()) } else { Poll::Pending }
            })
            .await;
        }
    }

    async fn bus_reset(&mut self) {
        let r = pac::USB;
        let mut c = base_sie_ctrl();
        c.set_reset_bus(true);
        r.sie_ctrl().write_value(c);
        Timer::after_millis(50).await;
        r.sie_ctrl().write_value(base_sie_ctrl());
        // Reset recovery (USB 2.0 §9.2.6.2).
        Timer::after_millis(10).await;
    }
}

pub struct RpPipe<T: pipe::Type, D: pipe::Direction> {
    target: Target,
    dir_in: bool,
    /// Interrupt slot 1–15, or 0 for EPX.
    slot: usize,
    toggle: bool,
    timeout: TimeoutConfig,
    _p: PhantomData<(T, D)>,
}

impl<'d> UsbHostAllocator<'d> for RpAlloc {
    type Pipe<T: pipe::Type, D: pipe::Direction> = RpPipe<T, D>;

    fn alloc_pipe<T: pipe::Type, D: pipe::Direction>(
        &self,
        addr: u8,
        endpoint: &EndpointInfo,
        split: Option<SplitInfo>,
    ) -> Result<Self::Pipe<T, D>, HostError> {
        let preamble = split.is_some_and(|s| s.device_speed() == SplitSpeed::Low);
        let kind = T::ep_type();
        let ty = match kind {
            EndpointType::Control => EpType::CONTROL,
            EndpointType::Bulk => EpType::BULK,
            EndpointType::Interrupt => EpType::INTERRUPT,
            EndpointType::Isochronous => return Err(HostError::Other("isochronous pipes are not supported")),
        };
        let dir_in = D::is_in();
        let mut p = RpPipe {
            target: Target {
                addr,
                ep: endpoint.addr.index() as u8,
                preamble,
                ty,
                mps: endpoint.max_packet_size.min(64),
            },
            dir_in,
            slot: 0,
            toggle: false,
            timeout: TimeoutConfig::default(),
            _p: PhantomData,
        };
        if kind == EndpointType::Interrupt {
            p.slot = alloc_slot().ok_or(HostError::OutOfPipes)?;
            p.start_slot(endpoint.interval_ms);
        }
        Ok(p)
    }
}

fn alloc_slot() -> Option<usize> {
    critical_section::with(|_| {
        let used = SLOTS_USED.load(Ordering::Relaxed);
        let n = (1..=SLOTS).find(|n| used & (1 << n) == 0)?;
        SLOTS_USED.store(used | 1 << n, Ordering::Relaxed);
        Some(n)
    })
}

impl<T: pipe::Type, D: pipe::Direction> RpPipe<T, D> {
    fn start_slot(&mut self, interval_ms: u8) {
        let n = self.slot;
        let r = pac::USB;
        r.addr_endp_x(n - 1).write_value({
            let mut a = AddrEndpX(0);
            a.set_address(self.target.addr);
            a.set_endpoint(self.target.ep);
            a.set_intep_dir(!self.dir_in);
            a.set_intep_preamble(self.target.preamble);
            a
        });
        let mut c = EpControl(0);
        c.set_enable(true);
        c.set_interrupt_per_buff(true);
        c.set_endpoint_type(EpType::INTERRUPT);
        c.set_buffer_address(slot_data_offset(n));
        // Host poll interval in ms − 1 lives in bits 25:16 (not modelled by the PAC).
        c.0 |= ((interval_ms.max(1) as u32 - 1) & 0x3FF) << 16;
        wr(slot_ctrl(n), c.0);
        self.toggle = false;
        if self.dir_in {
            self.rearm_in();
        } else {
            wr(slot_buf_ctrl(n), 0);
        }
        r.int_ep_ctrl().modify(|w| w.0 |= 1 << n);
    }

    fn rearm_in(&self) {
        let mut bc = EpBufferControl(0);
        bc.set_length(0, self.target.mps);
        bc.set_pid(0, self.toggle);
        bc.set_last(0, true);
        arm_buffer(slot_buf_ctrl(self.slot), bc);
    }

    async fn slot_in(&mut self, buf: &mut [u8]) -> Result<usize, PipeError> {
        let n = self.slot;
        let len = poll_fn(|cx| {
            SLOT_WAKERS[n - 1].register(cx.waker());
            let v = EpBufferControl(rd(slot_buf_ctrl(n)));
            // RP2040-E4: in single-buffered host mode the status may land in buffer 1's half.
            let len = if v.full(0) {
                v.length(0)
            } else if v.full(1) {
                v.length(1)
            } else {
                return Poll::Pending;
            };
            Poll::Ready(len as usize)
        })
        .await;
        let n_copy = len.min(buf.len());
        copy_from_dpram(slot_data_offset(n), &mut buf[..n_copy]);
        self.toggle = !self.toggle;
        self.rearm_in();
        if len > buf.len() { Err(PipeError::BufferOverflow) } else { Ok(len) }
    }

    async fn slot_out(&mut self, data: &[u8]) -> Result<(), PipeError> {
        let n = self.slot;
        let data = &data[..data.len().min(self.target.mps as usize)];
        copy_to_dpram(slot_data_offset(n), data);
        let mut bc = EpBufferControl(0);
        bc.set_length(0, data.len() as u16);
        bc.set_pid(0, self.toggle);
        bc.set_last(0, true);
        bc.set_full(0, true);
        arm_buffer(slot_buf_ctrl(n), bc);
        with_timeout(
            Duration::from_millis(100),
            poll_fn(|cx| {
                SLOT_WAKERS[n - 1].register(cx.waker());
                let v = EpBufferControl(rd(slot_buf_ctrl(n)));
                if v.available(0) { Poll::Pending } else { Poll::Ready(()) }
            }),
        )
        .await
        .map_err(|_| PipeError::Timeout)?;
        self.toggle = !self.toggle;
        Ok(())
    }

    fn data_timeout(&self) -> Duration {
        Duration::from_micros(self.timeout.data_timeout.as_micros() as u64)
    }

    async fn control(
        &mut self,
        setup: &[u8; 8],
        out: &[u8],
        inbuf: &mut [u8],
        device_to_host: bool,
    ) -> Result<usize, PipeError> {
        let _epx = EPX_LOCK.lock().await;
        let timeout = self.data_timeout();
        let mut none = [0u8; 0];
        epx(&self.target, Stage::Setup, false, setup, &mut none, timeout).await?;
        let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
        let mps = self.target.mps as usize;
        let mut done = 0;
        let mut data1 = true;
        if device_to_host {
            let want = length.min(inbuf.len());
            while done < want {
                let n = epx(&self.target, Stage::In, data1, &[], &mut inbuf[done..want], timeout).await?;
                done += n;
                data1 = !data1;
                if n < mps {
                    break;
                }
            }
            epx(&self.target, Stage::Out, true, &[], &mut none, timeout).await?;
        } else {
            for chunk in out[..length.min(out.len())].chunks(mps) {
                epx(&self.target, Stage::Out, data1, chunk, &mut none, timeout).await?;
                data1 = !data1;
                done += chunk.len();
            }
            epx(&self.target, Stage::In, true, &[], &mut none, timeout).await?;
        }
        Ok(done)
    }
}

/// Bulk transactions since start (the module as a board on a front port; diagnostics): packets and NAKs each way,
/// failed transactions, and the last failure ([`error_code`]).
pub static BULK_IN: AtomicU32 = AtomicU32::new(0);
pub static BULK_IN_NAKS: AtomicU32 = AtomicU32::new(0);
pub static BULK_OUT: AtomicU32 = AtomicU32::new(0);
pub static BULK_OUT_NAKS: AtomicU32 = AtomicU32::new(0);
pub static BULK_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static LAST_ERROR: AtomicU32 = AtomicU32::new(0);

/// Where a transfer failed (1 bulk IN, 2 bulk OUT, 3 control) and how, as one number: `where << 8 | kind`, kind 1
/// buffer overflow, 2 bad response, 3 babble, 4 data toggle, 5 canceled, 6 stall, 7 timeout, 8 disconnected.
pub fn error_code(place: u32, e: PipeError) -> u32 {
    let kind = match e {
        PipeError::BufferOverflow => 1,
        PipeError::BadResponse => 2,
        PipeError::Babble => 3,
        PipeError::DataToggleError => 4,
        PipeError::Canceled => 5,
        PipeError::Stall => 6,
        PipeError::Timeout => 7,
        PipeError::Disconnected => 8,
        #[allow(unreachable_patterns)]
        _ => 9,
    };
    place << 8 | kind
}

impl RpPipe<pipe::Interrupt, pipe::Out> {
    pub fn max_packet(&self) -> usize {
        self.target.mps as usize
    }

    /// One OUT packet on an interrupt slot, which the controller sends at the next poll and again while the device
    /// NAKs: `Ok(false)` if it wasn't taken within `timeout`, and then it is taken back (not sent later).
    pub async fn out_within(&mut self, data: &[u8], timeout: Duration) -> Result<bool, PipeError> {
        let n = self.slot;
        let data = &data[..data.len().min(self.target.mps as usize)];
        copy_to_dpram(slot_data_offset(n), data);
        let mut bc = EpBufferControl(0);
        bc.set_length(0, data.len() as u16);
        bc.set_pid(0, self.toggle);
        bc.set_last(0, true);
        bc.set_full(0, true);
        arm_buffer(slot_buf_ctrl(n), bc);
        let sent = poll_fn(|cx| {
            SLOT_WAKERS[n - 1].register(cx.waker());
            let v = EpBufferControl(rd(slot_buf_ctrl(n)));
            if v.available(0) { Poll::Pending } else { Poll::Ready(()) }
        });
        if with_timeout(timeout, sent).await.is_err() {
            // Taken back. Had the controller sent it just now, the next try repeats its data toggle, and the device
            // takes that for a retry: it acknowledges and drops it (USB 2.0 §8.6.2), so nothing arrives twice.
            wr(slot_buf_ctrl(n), 0);
            return Ok(false);
        }
        self.toggle = !self.toggle;
        Ok(true)
    }
}

impl<T: pipe::Type, D: pipe::Direction> Drop for RpPipe<T, D> {
    fn drop(&mut self) {
        if self.slot != 0 {
            let n = self.slot;
            pac::USB.int_ep_ctrl().modify(|w| w.0 &= !(1 << n));
            wr(slot_ctrl(n), 0);
            wr(slot_buf_ctrl(n), 0);
            critical_section::with(|_| {
                SLOTS_USED.fetch_and(!(1 << n), Ordering::Relaxed);
            });
        }
    }
}

impl<T: pipe::Type, D: pipe::Direction> UsbPipe<T, D> for RpPipe<T, D> {
    async fn control_in(&mut self, setup: &[u8; 8], buf: &mut [u8]) -> Result<usize, PipeError>
    where
        T: pipe::IsControl,
        D: pipe::IsIn,
    {
        self.control(setup, &[], buf, true).await
    }

    async fn control_out(&mut self, setup: &[u8; 8], buf: &[u8]) -> Result<(), PipeError>
    where
        T: pipe::IsControl,
        D: pipe::IsOut,
    {
        self.control(setup, buf, &mut [], false).await.map(|_| ())
    }

    async fn request_in(&mut self, buf: &mut [u8]) -> Result<usize, PipeError>
    where
        D: pipe::IsIn,
    {
        if self.slot != 0 {
            return self.slot_in(buf).await;
        }
        // Bulk IN over EPX: packets until a short one (NAKs are retried by the controller).
        let _epx = EPX_LOCK.lock().await;
        let mps = self.target.mps as usize;
        let mut done = 0;
        while done < buf.len() {
            let end = (done + mps).min(buf.len());
            let n =
                epx(&self.target, Stage::In, self.toggle, &[], &mut buf[done..end], Duration::from_secs(3600)).await?;
            self.toggle = !self.toggle;
            done += n;
            if n < mps {
                break;
            }
        }
        Ok(done)
    }

    async fn request_out(&mut self, buf: &[u8], ensure_transaction_end: bool) -> Result<(), PipeError>
    where
        D: pipe::IsOut,
    {
        if self.slot != 0 {
            return self.slot_out(buf).await;
        }
        let _epx = EPX_LOCK.lock().await;
        let mps = self.target.mps as usize;
        let mut none = [0u8; 0];
        for chunk in buf.chunks(mps) {
            epx(&self.target, Stage::Out, self.toggle, chunk, &mut none, Duration::from_secs(3600)).await?;
            self.toggle = !self.toggle;
        }
        if ensure_transaction_end && (buf.is_empty() || buf.len().is_multiple_of(mps)) {
            epx(&self.target, Stage::Out, self.toggle, &[], &mut none, Duration::from_secs(3600)).await?;
            self.toggle = !self.toggle;
        }
        Ok(())
    }

    fn set_timeout(&mut self, timeout: TimeoutConfig)
    where
        T: pipe::IsControl,
    {
        self.timeout = timeout;
    }

    fn reset_data_toggle(&mut self)
    where
        T: pipe::IsBulkOrInterrupt,
    {
        self.toggle = false;
        if self.slot != 0 && self.dir_in {
            self.rearm_in();
        }
    }
}
