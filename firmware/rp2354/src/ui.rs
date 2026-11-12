//! The MX key and the six LEDs (software.md §4.6).

use embassy_rp::gpio::Input;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio_programs::ws2812::PioWs2812;
use embassy_time::{Duration, Instant, Ticker, Timer};
use kvm_core::KeyEvent;
use kvm_core::config::TAP_MAX_MS;
use kvm_core::leds::{self, COUNT};
use smart_leds::RGB8;

use crate::kvm;

const TICK: Duration = Duration::from_millis(10);
/// Two equal samples 10 ms apart (the 100 nF and the pull-up already filter most bounce).
const DEBOUNCE_SAMPLES: u8 = 2;
const TAP_MAX: Duration = Duration::from_millis(TAP_MAX_MS as u64);

pub type Leds = PioWs2812<'static, PIO0, 0, COUNT, smart_leds_order::Grb>;

/// Re-exported so `main` can name the colour order without depending on the module layout.
pub mod smart_leds_order {
    pub use embassy_rp::pio_programs::ws2812::Grb;
}

/// Whether the key is held now, at power-up: low on three samples 10 ms apart (after the pull-up has settled).
pub async fn held_at_power_up(key: &Input<'static>) -> bool {
    let mut held = true;
    for _ in 0..3 {
        Timer::after(TICK).await;
        held &= key.is_low();
    }
    held
}

#[embassy_executor::task]
pub async fn run(key: Input<'static>, mut leds: Leds) -> ! {
    let mut ticker = Ticker::every(TICK);
    // A press held since power-up (the AT32F405 programming reset, `held_at_power_up`) is neither a tap nor a long
    // press: it only ends.
    let mut stable = key.is_low();
    let mut same = 0u8;
    let mut last_raw = stable;
    let mut pressed_at = Instant::now();
    let mut long_sent = stable;
    // The long press's duration is a setting; read with the LEDs, at most one tick old.
    let mut long_press = Duration::from_millis(kvm::with(|e| e.config().long_press_ms) as u64);
    loop {
        ticker.next().await;
        let raw = key.is_low();
        same = if raw == last_raw { same.saturating_add(1) } else { 0 };
        last_raw = raw;
        if same >= DEBOUNCE_SAMPLES && raw != stable {
            stable = raw;
            if stable {
                pressed_at = Instant::now();
                long_sent = false;
            } else if !long_sent && pressed_at.elapsed() < TAP_MAX {
                kvm::with(|e| e.key(KeyEvent::Tap));
            }
        }
        if stable && !long_sent && pressed_at.elapsed() >= long_press {
            long_sent = true;
            kvm::with(|e| e.key(KeyEvent::Long));
        }

        let now = Instant::now().as_millis() as u32;
        let (colors, long_ms) = kvm::with(|e| (leds::render(&e.led_inputs(), now), e.config().long_press_ms));
        long_press = Duration::from_millis(long_ms as u64);
        leds.write(&colors.map(|c| RGB8::new(c.r, c.g, c.b))).await;
    }
}
