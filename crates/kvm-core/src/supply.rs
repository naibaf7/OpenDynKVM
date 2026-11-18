//! The 5 V the KVM runs from, judged from what is measured (software.md §4.7): the PWR port's VBUS (the RP2354B's
//! ADC, every 200 ms), or on PC power the higher of the PCs' VBUS (each AT32F405's ADC: a reading every 100 ms and the
//! lowest of its millisecond samples in between), as the ideal diodes feed the board from the higher one.
//!
//! The board measures no current, and a PC port's limit can't be known without going over it. What tells whether a
//! source keeps up is its voltage under the real load: many PC ports give far more than the 500 mA they advertise and
//! hold their 5 V, while a weak one (or a long cable) sags. Until 0.1.27 the KVM compared the front devices' declared
//! maximum (`bMaxPower`) with the advertised current less an estimate of its own draw, and warned on ports that coped
//! well (first board, 2026-11-17: the whole board under 1 W). That sum is now information only.
//!
//! The readings are good to about ±3 % (the AT32F405s take their 3.3 V as the reference): a healthy port's 4.75 V or
//! more reads well clear of the thresholds.
//! - [`Supply::Sags`]: under [`SAG_MV`] for [`SAG_MS`]; [`Supply::Low`]: under [`LOW_MV`] (the least USB devices are
//!   required to work at) for [`LOW_MS`]. Back to `Good` after [`CLEAR_MS`] at [`CLEAR_MV`] or more.
//! - Counted, not judged: windows whose millisecond minimum fell under [`LOW_MV`] (a device's inrush), and a PC's 5 V
//!   gone for less than [`DROPOUT_MS`] (a port that cut out, or a PC's reboot: the two look the same from here).

use kvm_proto::ctrl::Supply;

pub const SAG_MV: u16 = 4500;
pub const SAG_MS: u32 = 2000;
pub const LOW_MV: u16 = 4400;
pub const LOW_MS: u32 = 500;
pub const CLEAR_MV: u16 = 4600;
pub const CLEAR_MS: u32 = 5000;
/// A PC's 5 V gone for less than this, then back: a dropout.
pub const DROPOUT_MS: u32 = 3000;
/// A PC's 5 V counts as there from this, as gone under [`ABSENT_MV`].
const PRESENT_MV: u16 = 4000;
const ABSENT_MV: u16 = 1000;
/// Just after a PC's 5 V comes up its minimum holds the rise: not counted for this long.
const SETTLE_MS: u32 = 500;
/// "The last minute" of the lowest reading: two windows of this, the one running and the one before.
const WINDOW_MS: u32 = 30_000;

/// One PC's 5 V over time.
#[derive(Debug, Clone, Copy, Default)]
struct Pc {
    /// The latest reading and millisecond minimum, while there.
    mv: Option<(u16, u16)>,
    /// Since when it is there; since when it is gone (after it was there).
    present_since: Option<u32>,
    gone_since: Option<u32>,
    dropouts: u16,
}

#[derive(Debug, Clone)]
pub struct Tracker {
    level: Supply,
    /// Since when the reading is under [`SAG_MV`], under [`LOW_MV`], at [`CLEAR_MV`] or more.
    sag_since: Option<u32>,
    low_since: Option<u32>,
    clear_since: Option<u32>,
    now_mv: u16,
    /// The lowest in the window running (since `window_at`) and in the one before.
    lowest: [u16; 2],
    window_at: Option<u32>,
    dips: u16,
    pcs: [Pc; 2],
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    pub const fn new() -> Self {
        Tracker {
            level: Supply::Unknown,
            sag_since: None,
            low_since: None,
            clear_since: None,
            now_mv: 0,
            lowest: [u16::MAX; 2],
            window_at: None,
            dips: 0,
            pcs: [Pc { mv: None, present_since: None, gone_since: None, dropouts: 0 }; 2],
        }
    }

    pub fn level(&self) -> Supply {
        self.level
    }

    /// The voltage now and the lowest in the last minute (0 before any reading).
    pub fn readings(&self) -> (u16, u16) {
        let lowest = self.lowest[0].min(self.lowest[1]);
        (self.now_mv, if lowest == u16::MAX { 0 } else { lowest })
    }

    pub fn dips(&self) -> u16 {
        self.dips
    }

    pub fn dropouts(&self, k: usize) -> u16 {
        self.pcs[k].dropouts
    }

    /// PC `k`'s status: its reading and the lowest millisecond sample since the last.
    pub fn pc(&mut self, now: u32, k: usize, mv: u16, min_mv: u16) {
        let p = &mut self.pcs[k];
        if mv >= PRESENT_MV {
            if let Some(gone) = p.gone_since.take()
                && now.wrapping_sub(gone) < DROPOUT_MS
            {
                p.dropouts = p.dropouts.saturating_add(1);
            }
            let since = *p.present_since.get_or_insert(now);
            // The rise itself is in the first windows' minimum.
            let settled = now.wrapping_sub(since) >= SETTLE_MS;
            p.mv = Some((mv, if settled { min_mv.min(mv) } else { mv }));
        } else {
            if mv < ABSENT_MV && p.present_since.take().is_some() {
                p.gone_since = Some(now);
            }
            p.mv = None;
        }
    }

    /// The source the KVM runs from, judged now: the PWR port's reading (`pwr_mv`, Some while it powers the board), or
    /// the higher of the PCs'.
    pub fn judge(&mut self, now: u32, pwr_mv: Option<u16>) {
        let (mv, min_mv) = match pwr_mv {
            Some(v) => (v, v),
            None => {
                let there = self.pcs.iter().filter_map(|p| p.mv);
                match there.reduce(|a, b| (a.0.max(b.0), a.1.max(b.1))) {
                    Some(r) => r,
                    // Neither PC gives 5 V (the KVM runs from the one coming up, or the readings lag): nothing to judge.
                    None => return,
                }
            }
        };
        self.now_mv = mv;
        if min_mv < LOW_MV {
            self.dips = self.dips.saturating_add(1);
        }
        match self.window_at {
            Some(t) if now.wrapping_sub(t) < WINDOW_MS => {}
            Some(_) => {
                self.lowest = [u16::MAX, self.lowest[0]];
                self.window_at = Some(now);
            }
            None => self.window_at = Some(now),
        }
        self.lowest[0] = self.lowest[0].min(min_mv);
        let since = |slot: &mut Option<u32>, on: bool| {
            if on {
                Some(now.wrapping_sub(*slot.get_or_insert(now)))
            } else {
                *slot = None;
                None
            }
        };
        let sag = since(&mut self.sag_since, mv < SAG_MV);
        let low = since(&mut self.low_since, mv < LOW_MV);
        let clear = since(&mut self.clear_since, mv >= CLEAR_MV);
        self.level = if low.is_some_and(|t| t >= LOW_MS) {
            Supply::Low
        } else if sag.is_some_and(|t| t >= SAG_MS) {
            self.level.max_with(Supply::Sags)
        } else {
            match self.level {
                Supply::Unknown => Supply::Good,
                l @ (Supply::Sags | Supply::Low) if !clear.is_some_and(|t| t >= CLEAR_MS) => l,
                _ => Supply::Good,
            }
        };
    }
}

trait Worse {
    fn max_with(self, other: Supply) -> Supply;
}

impl Worse for Supply {
    /// The worse of the two (a `Low` stays `Low` while the reading only sags).
    fn max_with(self, other: Supply) -> Supply {
        if self as u8 >= other as u8 { self } else { other }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PC1 alone at `mv` (minimum `min`) every 100 ms from `from` for `ms`.
    fn run(t: &mut Tracker, from: u32, ms: u32, mv: u16, min: u16) -> u32 {
        let mut now = from;
        while now < from + ms {
            t.pc(now, 0, mv, min);
            t.judge(now, None);
            now += 100;
        }
        now
    }

    #[test]
    fn a_port_that_holds_its_5_v_is_good_whatever_the_load() {
        let mut t = Tracker::new();
        assert_eq!(t.level(), Supply::Unknown);
        let now = run(&mut t, 0, 10_000, 4950, 4900);
        assert_eq!(t.level(), Supply::Good);
        assert_eq!(t.readings(), (4950, 4900));
        // A device's inrush: one window's minimum at 4.3 V. Counted, no warning.
        t.pc(now, 0, 4950, 4300);
        t.judge(now, None);
        assert_eq!((t.level(), t.dips()), (Supply::Good, 1));
        assert_eq!(t.readings().1, 4300);
    }

    #[test]
    fn a_sustained_sag_warns_and_clears_only_once_it_is_back() {
        let mut t = Tracker::new();
        let now = run(&mut t, 0, 1000, 4950, 4950);
        // Under 4.5 V: a blip is nothing, two seconds sag.
        let now = run(&mut t, now, 1000, 4450, 4450);
        assert_eq!(t.level(), Supply::Good);
        let now = run(&mut t, now, 1200, 4450, 4450);
        assert_eq!(t.level(), Supply::Sags);
        // Under 4.4 V for half a second: low.
        let now = run(&mut t, now, 600, 4350, 4300);
        assert_eq!(t.level(), Supply::Low);
        // Back over 4.6 V: still low for a while, then good.
        let now = run(&mut t, now, 3000, 4700, 4700);
        assert_eq!(t.level(), Supply::Low);
        run(&mut t, now, 2500, 4700, 4700);
        assert_eq!(t.level(), Supply::Good);
    }

    #[test]
    fn on_pc_power_the_higher_pc_feeds_the_board() {
        let mut t = Tracker::new();
        let mut now = 0;
        while now < 5000 {
            t.pc(now, 0, 4300, 4300);
            t.pc(now, 1, 5000, 4950);
            t.judge(now, None);
            now += 100;
        }
        assert_eq!(t.level(), Supply::Good);
        assert_eq!(t.readings(), (5000, 4950));
        // On the PWR port its reading counts, however the PCs do.
        for _ in 0..30 {
            t.judge(now, Some(4380));
            now += 100;
        }
        assert_eq!(t.level(), Supply::Low);
    }

    #[test]
    fn a_short_dropout_is_counted_and_the_rise_after_it_is_no_dip() {
        let mut t = Tracker::new();
        let now = run(&mut t, 0, 2000, 5000, 5000);
        t.pc(now, 0, 0, 0);
        // Back 1.5 s later, its first window holding the rise.
        t.pc(now + 1500, 0, 5000, 600);
        t.judge(now + 1500, None);
        assert_eq!((t.dropouts(0), t.dips()), (1, 0));
        // Gone longer (switched off, a slow reboot): not a dropout.
        t.pc(now + 3000, 0, 0, 0);
        t.pc(now + 9000, 0, 5000, 5000);
        assert_eq!(t.dropouts(0), 1);
    }

    #[test]
    fn the_lowest_reading_is_the_last_minutes() {
        let mut t = Tracker::new();
        let now = run(&mut t, 0, 1000, 5000, 4600);
        let now = run(&mut t, now, 40_000, 5000, 4950);
        assert_eq!(t.readings().1, 4600, "within the window before");
        run(&mut t, now, 30_000, 5000, 4950);
        assert_eq!(t.readings().1, 4950);
    }
}
