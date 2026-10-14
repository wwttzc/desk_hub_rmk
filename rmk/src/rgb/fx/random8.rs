//! Random number generation, ported from QMK's `lib/lib8tion/random8.h`.
//!
//! QMK keeps one global `uint16_t rand16seed`; we keep the same 16-bit LCG but
//! store the seed in a value the RGB engine owns and hands to each effect. The
//! *sequence* is identical for a given seed — only the storage class differs,
//! which avoids a global mutable on `no_std`.

/// QMK's linear congruential generator: `x → 2053x + 13849 (mod 2^16)`.
#[derive(Debug, Clone)]
pub struct Rng {
    seed: u16,
}

impl Rng {
    /// QMK's `rand16seed` initial value, from `lib/lib8tion/lib8tion.c`.
    pub const DEFAULT_SEED: u16 = 1337;

    /// A generator starting from QMK's default seed.
    #[inline]
    pub const fn new() -> Self {
        Self::with_seed(Self::DEFAULT_SEED)
    }

    /// A generator starting from an explicit seed.
    #[inline]
    pub const fn with_seed(seed: u16) -> Self {
        Self { seed }
    }

    /// `random16_set_seed`.
    #[inline]
    pub fn set_seed(&mut self, seed: u16) {
        self.seed = seed;
    }

    /// `random16_get_seed`.
    #[inline]
    pub const fn seed(&self) -> u16 {
        self.seed
    }

    /// `random16_add_entropy`.
    #[inline]
    pub fn add_entropy(&mut self, entropy: u16) {
        self.seed = self.seed.wrapping_add(entropy);
    }

    /// One LCG step, returning the new state.
    #[inline]
    fn step(&mut self) -> u16 {
        self.seed = self.seed.wrapping_mul(2053).wrapping_add(13849);
        self.seed
    }

    /// `random16`.
    #[inline]
    pub fn next16(&mut self) -> u16 {
        self.step()
    }

    /// `random8`: the high and low bytes summed, for better mixing.
    #[inline]
    pub fn next8(&mut self) -> u8 {
        let s = self.step();
        (s as u8).wrapping_add((s >> 8) as u8)
    }

    /// `random8_max`: uniform-ish `0..lim`.
    #[inline]
    pub fn next8_max(&mut self, lim: u8) -> u8 {
        let r = self.next8();
        ((r as u16 * lim as u16) >> 8) as u8
    }

    /// `random8_min_max`: `min..lim`.
    #[inline]
    pub fn next8_min_max(&mut self, min: u8, lim: u8) -> u8 {
        let delta = lim.wrapping_sub(min);
        self.next8_max(delta).wrapping_add(min)
    }

    /// `random16_max`: `0..lim`.
    #[inline]
    pub fn next16_max(&mut self, lim: u16) -> u16 {
        let r = self.next16();
        ((lim as u32 * r as u32) >> 16) as u16
    }

    /// `random16_min_max`: `min..lim`.
    #[inline]
    pub fn next16_min_max(&mut self, min: u16, lim: u16) -> u16 {
        let delta = lim.wrapping_sub(min);
        self.next16_max(delta).wrapping_add(min)
    }
}

impl Default for Rng {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LCG constants and the two-byte mixing are the whole contract; a
    /// friendly "nicer" RNG here would silently change every random effect.
    #[test]
    fn sequence_is_the_qmk_lcg() {
        let mut rng = Rng::with_seed(0);
        let mut seed = 0u16;
        for _ in 0..64 {
            seed = seed.wrapping_mul(2053).wrapping_add(13849);
            assert_eq!(rng.next16(), seed);
        }
    }

    #[test]
    fn next8_sums_the_state_bytes() {
        let mut a = Rng::with_seed(1337);
        let mut b = Rng::with_seed(1337);
        for _ in 0..64 {
            let s = b.next16();
            assert_eq!(a.next8(), (s as u8).wrapping_add((s >> 8) as u8));
        }
    }

    #[test]
    fn bounded_helpers_stay_in_range() {
        let mut rng = Rng::new();
        for _ in 0..1000 {
            assert!(rng.next8_max(10) < 10);
            assert!((5..20).contains(&rng.next8_min_max(5, 20)));
            assert!(rng.next16_max(1000) < 1000);
            assert!((100..5000).contains(&rng.next16_min_max(100, 5000)));
        }
    }

    #[test]
    fn entropy_and_seeding_round_trip() {
        let mut rng = Rng::new();
        rng.set_seed(0);
        assert_eq!(rng.seed(), 0);
        rng.add_entropy(5);
        assert_eq!(rng.seed(), 5);
    }

    /// `next8_max(255)` is `floor(next8() * 255 / 256)`, i.e. the next byte
    /// minus one part in 256 — never the full byte.
    #[test]
    fn saturated_limit_truncates_by_one_step() {
        let mut rng = Rng::with_seed(1337);
        for _ in 0..64 {
            // Two generators marching in lockstep: one raw, one bounded.
            let mut raw_rng = rng.clone();
            let raw = raw_rng.next8();
            assert_eq!(rng.next8_max(255), ((raw as u16 * 255) >> 8) as u8);
        }
    }
}
