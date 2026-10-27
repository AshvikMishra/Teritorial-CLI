//! A tiny, seeded, serializable pseudo-random number generator (SplitMix64).
//!
//! The core deliberately ships its own generator instead of depending on the
//! `rand` crate, so the sequence of numbers for a seed can never change under a
//! dependency upgrade. That keeps saves and replays deterministic forever.

use serde::{Deserialize, Serialize};

/// SplitMix64 generator. Cheap to clone, and serializes as its 64-bit state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    state: u64,
}

impl Rng {
    /// Creates a generator from a seed. Every seed (including 0) is valid.
    pub fn new(seed: u64) -> Rng {
        Rng { state: seed }
    }

    /// Returns the next 64 uniformly distributed bits.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Returns a value in `0..n` (exclusive). Returns 0 when `n == 0`.
    ///
    /// Uses the widening-multiply technique, whose bias is negligible for the
    /// small ranges the game uses.
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        let wide = (self.next_u64() as u128) * (n as u128);
        (wide >> 64) as u64
    }

    /// Returns a value in `lo..=hi` (inclusive). Returns `lo` when `hi <= lo`.
    pub fn range_u64(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        let span = hi - lo;
        if span == u64::MAX {
            return self.next_u64();
        }
        lo + self.below(span + 1)
    }

    /// Returns `true` with probability `p / 1000`. `p >= 1000` is always true.
    pub fn chance_permille(&mut self, p: u32) -> bool {
        if p == 0 {
            return false;
        }
        if p >= 1000 {
            return true;
        }
        self.below(1000) < p as u64
    }

    /// Shuffles a slice in place (Fisher-Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        let n = items.len();
        if n < 2 {
            return;
        }
        let mut i = n - 1;
        while i > 0 {
            let j = self.below(i as u64 + 1) as usize;
            items.swap(i, j);
            i -= 1;
        }
    }
}
