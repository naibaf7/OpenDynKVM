//! Go-back-N ARQ over the lockstep frame exchange (software.md §6.2).
//!
//! Both ends call, per SPI transaction *t*: [`Arq::next_frame`] before it, and [`Arq::receive`] with the peer's frame
//! after it. A frame sent in transaction *t* is reflected in the `ack` of the peer's frame in *t + 1*, because each end
//! builds its next frame between transactions. A message still unacknowledged one transaction after it was sent is
//! therefore lost (or the peer was slow), and the sender goes back to the oldest unacknowledged message.

use heapless::{Deque, Vec};

use super::frame::{FRAME_LEN, Frame, FrameError, MAX_MSG, flags};

pub type Msg = Vec<u8, MAX_MSG>;

/// Messages in flight at most.
pub const WINDOW: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Prio {
    /// HID reports: always sent before anything queued at `Low`.
    High,
    /// Control traffic, descriptor sets, status.
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct QueueFull;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Stats {
    pub frames: u32,
    pub crc_errors: u32,
    pub magic_errors: u32,
    pub length_errors: u32,
    /// Data frames dropped because they were out of order or duplicates.
    pub dropped: u32,
    pub retransmits: u32,
    pub delivered: u32,
    pub peer_restarts: u32,
}

impl Stats {
    pub fn errors(&self) -> u32 {
        self.crc_errors + self.magic_errors + self.length_errors
    }
}

pub struct Arq<const Q: usize> {
    // Transmit side.
    next_seq: u8,
    base: u8,
    send_ptr: u8,
    window: [Msg; WINDOW as usize],
    sent_at: [u32; WINDOW as usize],
    syn: bool,
    tick: u32,
    hi: Deque<Msg, Q>,
    lo: Deque<Msg, Q>,
    // Receive side.
    expected: Option<u8>,
    peer_syn: bool,
    pub stats: Stats,
}

/// Distance from `a` forward to `b`, modulo 256.
fn dist(a: u8, b: u8) -> u8 {
    b.wrapping_sub(a)
}

impl<const Q: usize> Arq<Q> {
    /// `initial_seq` should differ between boots (any cheap entropy will do); it makes a stale acknowledgement
    /// from before a restart unlikely to fall inside the new window.
    pub const fn new(initial_seq: u8) -> Self {
        Self {
            next_seq: initial_seq,
            base: initial_seq,
            send_ptr: initial_seq,
            window: [const { Vec::new() }; WINDOW as usize],
            sent_at: [0; WINDOW as usize],
            syn: true,
            tick: 0,
            hi: Deque::new(),
            lo: Deque::new(),
            expected: None,
            peer_syn: false,
            stats: Stats {
                frames: 0,
                crc_errors: 0,
                magic_errors: 0,
                length_errors: 0,
                dropped: 0,
                retransmits: 0,
                delivered: 0,
                peer_restarts: 0,
            },
        }
    }

    pub fn push(&mut self, msg: &[u8], prio: Prio) -> Result<(), QueueFull> {
        let m = Msg::from_slice(msg).map_err(|_| QueueFull)?;
        let q = match prio {
            Prio::High => &mut self.hi,
            Prio::Low => &mut self.lo,
        };
        q.push_back(m).map_err(|_| QueueFull)
    }

    pub fn free(&self, prio: Prio) -> usize {
        match prio {
            Prio::High => Q - self.hi.len(),
            Prio::Low => Q - self.lo.len(),
        }
    }

    fn outstanding(&self) -> u8 {
        dist(self.base, self.next_seq)
    }

    /// Something is queued, unsent or unacknowledged: the master should run another transaction soon, and the slave
    /// holds its IRQ line high.
    pub fn has_pending(&self) -> bool {
        !self.hi.is_empty() || !self.lo.is_empty() || self.outstanding() != 0 || self.syn
    }

    /// The peer has acknowledged our restart (or we never restarted since it did).
    pub fn is_synced(&self) -> bool {
        !self.syn && self.expected.is_some()
    }

    /// Frame to send in the coming transaction.
    pub fn next_frame(&mut self) -> Frame {
        let mut f = Frame::idle();
        if self.syn {
            f.flags |= flags::SYN;
        }
        if let Some(e) = self.expected {
            f.flags |= flags::ACKV;
            f.ack = e;
        }
        if self.peer_syn {
            f.flags |= flags::SYNACK;
        }

        let seq = if self.send_ptr != self.next_seq {
            self.stats.retransmits += 1;
            Some(self.send_ptr)
        } else if self.outstanding() < WINDOW {
            match self.hi.pop_front().or_else(|| self.lo.pop_front()) {
                Some(m) => {
                    let s = self.next_seq;
                    self.window[(s % WINDOW) as usize] = m;
                    self.next_seq = s.wrapping_add(1);
                    Some(s)
                }
                None => None,
            }
        } else {
            None
        };

        match seq {
            Some(s) => {
                let i = (s % WINDOW) as usize;
                let m = &self.window[i];
                f.flags |= flags::DATA;
                f.seq = s;
                f.len = m.len() as u8;
                f.msg[..m.len()].copy_from_slice(m);
                self.sent_at[i] = self.tick;
                self.send_ptr = s.wrapping_add(1);
            }
            None => f.seq = self.next_seq,
        }
        if self.has_pending() {
            f.flags |= flags::MORE;
        }
        f
    }

    /// Process the peer's frame from the transaction that just ended. Returns the message it delivered, if any.
    pub fn receive(&mut self, raw: &[u8; FRAME_LEN]) -> Option<Msg> {
        self.stats.frames += 1;
        let now = self.tick;
        self.tick = self.tick.wrapping_add(1);

        let f = match Frame::decode(raw) {
            Ok(f) => f,
            Err(e) => {
                match e {
                    FrameError::BadCrc => self.stats.crc_errors += 1,
                    FrameError::BadMagic => self.stats.magic_errors += 1,
                    FrameError::BadLength => self.stats.length_errors += 1,
                }
                return None;
            }
        };

        // The peer restarted: forget where its numbering was.
        if f.has(flags::SYN) {
            if !self.peer_syn {
                self.peer_syn = true;
                if self.expected.take().is_some() {
                    self.stats.peer_restarts += 1;
                }
            }
        } else {
            self.peer_syn = false;
        }

        // While we are restarting, only an ack built after the peer saw our SYN refers to our numbering.
        let ack_usable = f.has(flags::ACKV) && (!self.syn || f.has(flags::SYNACK));
        if ack_usable {
            self.syn = false;
            if dist(self.base, f.ack) <= self.outstanding() {
                self.base = f.ack;
                if dist(self.base, self.send_ptr) > self.outstanding() {
                    self.send_ptr = self.base;
                }
            }
        }
        // The oldest message went out in an earlier transaction and is still unacknowledged: go back. While we are
        // restarting and no usable ack has arrived yet, the same rule keeps re-offering our first messages.
        if (ack_usable || self.syn) && self.outstanding() != 0 {
            let sent = self.sent_at[(self.base % WINDOW) as usize];
            if now.wrapping_sub(sent) >= 1 {
                self.send_ptr = self.base;
            }
        }

        let expected = *self.expected.get_or_insert(f.seq);
        if f.has(flags::DATA) {
            if f.seq == expected {
                self.expected = Some(expected.wrapping_add(1));
                self.stats.delivered += 1;
                return Some(Msg::from_slice(f.message()).unwrap_or_default());
            }
            self.stats.dropped += 1;
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec as StdVec;

    struct XorShift(u32);
    impl XorShift {
        fn next(&mut self) -> u32 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 17;
            self.0 ^= self.0 << 5;
            self.0
        }
        fn chance(&mut self, percent: u32) -> bool {
            self.next() % 100 < percent
        }
    }

    type Ep = Arq<8>;

    fn msg(side: u8, n: u32) -> StdVec<u8> {
        let len = 1 + (n as usize * 7) % (MAX_MSG - 1);
        let mut v = StdVec::with_capacity(len);
        v.push(side);
        v.extend((0..len - 1).map(|i| (n as usize + i) as u8));
        v
    }

    /// One lockstep transaction with independent corruption in each direction.
    fn exchange(a: &mut Ep, b: &mut Ep, rng: &mut XorShift, loss: u32) -> (Option<Msg>, Option<Msg>) {
        let (fa, fb) = (a.next_frame(), b.next_frame());
        let (mut ra, mut rb) = ([0u8; FRAME_LEN], [0u8; FRAME_LEN]);
        fa.encode(&mut ra);
        fb.encode(&mut rb);
        if rng.chance(loss) {
            ra[(rng.next() as usize) % FRAME_LEN] ^= 1 << (rng.next() % 8);
        }
        if rng.chance(loss) {
            rb[(rng.next() as usize) % FRAME_LEN] ^= 1 << (rng.next() % 8);
        }
        (a.receive(&rb), b.receive(&ra))
    }

    fn run(loss: u32, seed: u32, count: u32) -> (Stats, Stats) {
        let mut rng = XorShift(seed);
        let (mut a, mut b) = (Ep::new(17), Ep::new(250));
        let (mut sent_a, mut sent_b) = (0u32, 0u32);
        let (mut got_a, mut got_b) = (StdVec::new(), StdVec::new());
        for _ in 0..200_000 {
            // Priorities reorder the queue, so each side uses one class to keep the expected order simple.
            while sent_a < count && a.free(Prio::Low) > 0 {
                a.push(&msg(b'a', sent_a), Prio::Low).unwrap();
                sent_a += 1;
            }
            while sent_b < count && b.free(Prio::High) > 0 {
                b.push(&msg(b'b', sent_b), Prio::High).unwrap();
                sent_b += 1;
            }
            let (ra, rb) = exchange(&mut a, &mut b, &mut rng, loss);
            got_a.extend(ra);
            got_b.extend(rb);
            if got_a.len() as u32 == count && got_b.len() as u32 == count && !a.has_pending() && !b.has_pending() {
                break;
            }
        }
        let want_b: StdVec<_> = (0..count).map(|n| msg(b'a', n)).collect();
        let want_a: StdVec<_> = (0..count).map(|n| msg(b'b', n)).collect();
        assert_eq!(got_b.iter().map(|m| m.to_vec()).collect::<StdVec<_>>(), want_b, "a -> b, loss {loss}%");
        assert_eq!(got_a.iter().map(|m| m.to_vec()).collect::<StdVec<_>>(), want_a, "b -> a, loss {loss}%");
        (a.stats, b.stats)
    }

    #[test]
    fn clean_link_is_lossless_and_fast() {
        let (a, b) = run(0, 1, 500);
        assert_eq!((a.retransmits, b.retransmits), (0, 0));
        assert_eq!((a.delivered, b.delivered), (500, 500));
        // About one message per transaction once the window is open.
        assert!(a.frames < 520, "took {} frames", a.frames);
    }

    #[test]
    fn exactly_once_in_order_under_corruption() {
        for (loss, seed) in [(1, 7), (5, 99), (20, 1234), (50, 4321)] {
            let (a, b) = run(loss, seed, 400);
            assert!(a.errors() + b.errors() > 0 || loss == 0);
        }
    }

    #[test]
    fn priority_overtakes_queued_control_traffic() {
        let mut a = Ep::new(0);
        let mut b = Ep::new(0);
        let mut rng = XorShift(5);
        for n in 0..6 {
            a.push(&[0x20, n], Prio::Low).unwrap();
        }
        a.push(&[0x11, 0xAA], Prio::High).unwrap();
        let mut got = StdVec::new();
        for _ in 0..20 {
            got.extend(exchange(&mut a, &mut b, &mut rng, 0).1);
        }
        assert_eq!(got[0].as_slice(), &[0x11, 0xAA]);
        assert_eq!(got.len(), 7);
    }

    #[test]
    fn restart_resynchronises_without_duplicates() {
        let mut rng = XorShift(77);
        let (mut a, mut b) = (Ep::new(3), Ep::new(9));
        let mut got_b = StdVec::new();
        for n in 0..20 {
            a.push(&[1, n], Prio::Low).unwrap();
            got_b.extend(exchange(&mut a, &mut b, &mut rng, 0).1);
        }
        // b reboots in the middle of a stream; a keeps going.
        b = Ep::new(200);
        got_b.clear();
        let mut got_a = StdVec::new();
        b.push(&[2, 0xEE], Prio::Low).unwrap(); // b's Hello after its restart
        for n in 20..60u8 {
            a.push(&[1, n], Prio::Low).unwrap();
            let (ra, rb) = exchange(&mut a, &mut b, &mut rng, 0);
            got_a.extend(ra);
            got_b.extend(rb);
        }
        for _ in 0..10 {
            let (ra, rb) = exchange(&mut a, &mut b, &mut rng, 0);
            got_a.extend(ra);
            got_b.extend(rb);
        }
        assert_eq!(got_a.len(), 1);
        assert_eq!(got_a[0].as_slice(), &[2, 0xEE]);
        // Everything pushed after the restart arrives once and in order; messages in flight at the restart may be lost.
        let nums: StdVec<u8> = got_b.iter().map(|m| m[1]).collect();
        assert!(nums.windows(2).all(|w| w[1] == w[0] + 1), "{nums:?}");
        assert_eq!(*nums.last().unwrap(), 59);
        assert!(nums[0] <= 20);
        assert_eq!(a.stats.peer_restarts, 1);
        assert!(!a.has_pending() && !b.has_pending());
    }

    #[test]
    fn restart_under_corruption() {
        for seed in 1..30 {
            let mut rng = XorShift(seed * 7919);
            let (mut a, mut b) = (Ep::new(seed as u8), Ep::new(100));
            for n in 0..10 {
                a.push(&[1, n], Prio::Low).unwrap();
                exchange(&mut a, &mut b, &mut rng, 10);
            }
            b = Ep::new(seed as u8 ^ 0x5A);
            let mut got_b = StdVec::new();
            for n in 10..80u8 {
                if a.free(Prio::Low) > 0 {
                    a.push(&[1, n], Prio::Low).unwrap();
                }
                got_b.extend(exchange(&mut a, &mut b, &mut rng, 10).1);
            }
            for _ in 0..200 {
                got_b.extend(exchange(&mut a, &mut b, &mut rng, 10).1);
            }
            let nums: StdVec<u8> = got_b.iter().map(|m| m[1]).collect();
            assert!(nums.windows(2).all(|w| w[1] > w[0]), "seed {seed}: {nums:?}");
            assert!(!a.has_pending(), "seed {seed}");
        }
    }
}
