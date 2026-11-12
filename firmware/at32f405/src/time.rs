//! `embassy-time` driver on SysTick at 1 kHz (feature `tick-hz-1_000`). Millisecond resolution is all this firmware
//! needs: reconnect delays, the 500 ms control-forward timeout and the 100 ms status period.

use core::cell::{Cell, RefCell};
use core::task::Waker;

use cortex_m::peripheral::syst::SystClkSource;
use cortex_m_rt::exception;
use critical_section::Mutex;
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;

use crate::clocks::SYSCLK_HZ;

struct SysTickDriver {
    ticks: Mutex<Cell<u64>>,
    next: Mutex<Cell<u64>>,
    queue: Mutex<RefCell<Queue>>,
}

embassy_time_driver::time_driver_impl!(static DRIVER: SysTickDriver = SysTickDriver {
    ticks: Mutex::new(Cell::new(0)),
    next: Mutex::new(Cell::new(u64::MAX)),
    queue: Mutex::new(RefCell::new(Queue::new())),
});

impl Driver for SysTickDriver {
    fn now(&self) -> u64 {
        critical_section::with(|cs| self.ticks.borrow(cs).get())
    }

    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let mut q = self.queue.borrow(cs).borrow_mut();
            if q.schedule_wake(at, waker) {
                let now = self.ticks.borrow(cs).get();
                self.next.borrow(cs).set(q.next_expiration(now));
            }
        });
    }
}

pub fn init(mut syst: cortex_m::peripheral::SYST) {
    syst.set_clock_source(SystClkSource::Core);
    syst.set_reload(SYSCLK_HZ / 1000 - 1);
    syst.clear_current();
    syst.enable_interrupt();
    syst.enable_counter();
}

#[exception]
fn SysTick() {
    critical_section::with(|cs| {
        let t = DRIVER.ticks.borrow(cs).get() + 1;
        DRIVER.ticks.borrow(cs).set(t);
        if t >= DRIVER.next.borrow(cs).get() {
            let next = DRIVER.queue.borrow(cs).borrow_mut().next_expiration(t);
            DRIVER.next.borrow(cs).set(next);
        }
    });
}
