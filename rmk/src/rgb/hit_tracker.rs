//! Recently-hit LEDs, the input side of QMK's reactive effects.
//!
//! Port of `last_hit_t` and the parts of `rgb_matrix.c` that maintain it:
//! `rgb_matrix_handle_key_event` fills the buffer, `rgb_task_timers` ages it.
//!
//! A reactive effect asks "when was this LED last hit, relative to now", so the
//! buffer holds a small ring of recent hits rather than a per-LED timestamp.
//! QMK keeps `LED_HITS_TO_REMEMBER` of them and shifts older entries out when
//! it needs room.

/// `LED_HITS_TO_REMEMBER`: how many recent hits the buffer holds.
pub const LED_HITS_TO_REMEMBER: usize = 8;

/// One remembered key hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Hit {
    /// Physical position of the hit LED, so splash effects can measure distance.
    pub x: u8,
    pub y: u8,
    /// LED index, so per-LED effects can match their own LED.
    pub index: u8,
    /// Milliseconds since the hit. Starts at zero and grows; `u16::MAX` means
    /// "empty slot", which is also what the initial fill uses.
    pub tick: u16,
}

/// `last_hit_t`: a bounded, oldest-first list of recent hits.
///
/// Index `0` is the oldest entry and `count - 1` the newest, which is the
/// order `record` maintains and the order reactive effects search backwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitTracker {
    count: u8,
    x: [u8; LED_HITS_TO_REMEMBER],
    y: [u8; LED_HITS_TO_REMEMBER],
    index: [u8; LED_HITS_TO_REMEMBER],
    tick: [u16; LED_HITS_TO_REMEMBER],
}

impl Default for HitTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HitTracker {
    /// `rgb_matrix_init`'s initialisation: no hits, every tick saturated so a
    /// stale slot never looks recent.
    pub const fn new() -> Self {
        Self {
            count: 0,
            x: [0; LED_HITS_TO_REMEMBER],
            y: [0; LED_HITS_TO_REMEMBER],
            index: [0; LED_HITS_TO_REMEMBER],
            tick: [u16::MAX; LED_HITS_TO_REMEMBER],
        }
    }

    /// Number of remembered hits.
    #[inline]
    pub const fn count(&self) -> u8 {
        self.count
    }

    /// True when nothing has been hit yet.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The hit at `index`, oldest first. `None` outside the populated range, so
    /// a caller cannot read an uninitialised slot.
    pub fn hit(&self, index: u8) -> Option<Hit> {
        if index >= self.count {
            return None;
        }
        let i = index as usize;
        Some(Hit {
            x: self.x[i],
            y: self.y[i],
            index: self.index[i],
            tick: self.tick[i],
        })
    }

    /// Forget every hit, as a fresh boot does.
    pub fn clear(&mut self) {
        self.count = 0;
    }

    /// `rgb_matrix_handle_key_event`'s bookkeeping: remember that the LEDs
    /// under electrical position `(row, col)` were just hit.
    ///
    /// Does nothing when no LED sits under that position, which is the case
    /// QMK reaches through `led_count == 0` (its shift is a no-op and its
    /// append loop does not run).
    pub fn record<const LED_COUNT: usize>(&mut self, leds: &super::led_config::LedConfig<LED_COUNT>, row: u8, col: u8) {
        let mut led = [0u8; LED_HITS_TO_REMEMBER];
        let led_count = leds.leds_at(row, col, &mut led);
        if led_count == 0 {
            return;
        }

        // Make room by dropping the oldest entries, exactly as QMK's
        // `memmove(&buf[0], &buf[led_count], N - led_count)` does.
        if self.count as usize + led_count > LED_HITS_TO_REMEMBER {
            self.x.copy_within(led_count.., 0);
            self.y.copy_within(led_count.., 0);
            self.tick.copy_within(led_count.., 0);
            self.index.copy_within(led_count.., 0);
            self.count = (LED_HITS_TO_REMEMBER - led_count) as u8;
        }

        for &led_index in &led[..led_count] {
            let slot = self.count as usize;
            if slot >= LED_HITS_TO_REMEMBER {
                break;
            }
            let point = leds.point(led_index as usize);
            self.x[slot] = point.x;
            self.y[slot] = point.y;
            self.index[slot] = led_index;
            self.tick[slot] = 0;
            self.count += 1;
        }
    }

    /// `rgb_task_timers`' ageing: add `delta_ms` to every remembered hit.
    ///
    /// Ported as-is, including its quirk: the loop bound is a snapshot of
    /// `count` taken before the loop, while the body decrements `count` for
    /// each saturated entry. So a saturated hit *drops the newest slot* rather
    /// than itself. That is QMK's behaviour; a "fixed" version would make
    /// long-running reactive effects fade differently from upstream.
    pub fn age(&mut self, delta_ms: u32) {
        let snapshot = self.count as usize;
        for i in 0..snapshot {
            // The C comparison promotes both sides to `uint32_t`.
            if (u16::MAX as u32).wrapping_sub(delta_ms) < self.tick[i] as u32 {
                self.count = self.count.saturating_sub(1);
                continue;
            }
            self.tick[i] = self.tick[i].wrapping_add(delta_ms as u16);
        }
    }

    /// The reverse search `effect_runner_reactive` performs: the *smallest*
    /// tick among hits on `led_index` that is below `ceiling`, else `ceiling`.
    ///
    /// Searching backwards from the newest hit and stopping at the first match
    /// is what makes the most recent hit win when an LED was hit repeatedly.
    pub fn most_recent_tick(&self, led_index: u8, ceiling: u16) -> u16 {
        let mut tick = ceiling;
        let mut j = self.count;
        while j > 0 {
            j -= 1;
            let i = j as usize;
            if self.index[i] == led_index && self.tick[i] < tick {
                tick = self.tick[i];
                break;
            }
        }
        tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::led_config::{LedConfig, LedPoint};

    /// Four keys in a row, one LED each, at increasing x.
    fn leds() -> LedConfig<4> {
        LedConfig::from_matrix(
            [
                LedPoint::new(0, 0),
                LedPoint::new(75, 0),
                LedPoint::new(150, 0),
                LedPoint::new(224, 0),
            ],
            [(0, 0), (0, 1), (0, 2), (0, 3)],
        )
    }

    #[test]
    fn starts_empty_with_saturated_ticks() {
        let tracker = HitTracker::new();
        assert_eq!(tracker.count(), 0);
        assert!(tracker.is_empty());
        assert_eq!(tracker.hit(0), None, "an empty slot is not readable");
        // The initial fill is what keeps a stale slot from looking recent.
        assert_eq!(tracker.most_recent_tick(0, u16::MAX), u16::MAX);
    }

    #[test]
    fn recording_a_key_remembers_its_led() {
        let leds = leds();
        let mut tracker = HitTracker::new();

        tracker.record(&leds, 0, 2);
        assert_eq!(tracker.count(), 1);
        assert_eq!(
            tracker.hit(0),
            Some(Hit {
                x: 150,
                y: 0,
                index: 2,
                tick: 0
            })
        );
    }

    #[test]
    fn recording_a_position_without_an_led_is_a_no_op() {
        let leds = leds();
        let mut tracker = HitTracker::new();
        tracker.record(&leds, 5, 5);
        assert_eq!(tracker.count(), 0);
    }

    #[test]
    fn the_buffer_keeps_the_newest_hits_and_drops_the_oldest() {
        let leds = leds();
        let mut tracker = HitTracker::new();

        // Fill past capacity: LED_HITS_TO_REMEMBER + 2 = 10 hits.
        for n in 0..(LED_HITS_TO_REMEMBER + 2) {
            tracker.record(&leds, 0, (n % 4) as u8);
        }
        assert_eq!(tracker.count() as usize, LED_HITS_TO_REMEMBER);

        // The two oldest were shifted out; the newest is the last recorded.
        assert_eq!(tracker.hit(tracker.count() - 1).unwrap().index, 1);
    }

    #[test]
    fn ageing_adds_to_every_remembered_tick() {
        let leds = leds();
        let mut tracker = HitTracker::new();
        tracker.record(&leds, 0, 0);
        tracker.record(&leds, 0, 1);

        tracker.age(10);
        assert_eq!(tracker.hit(0).unwrap().tick, 10);
        assert_eq!(tracker.hit(1).unwrap().tick, 10);

        tracker.age(5);
        assert_eq!(tracker.hit(0).unwrap().tick, 15);
    }

    /// The quirk documented on `age`: a hit that would overflow saturates the
    /// buffer by shrinking `count`, which discards slots from the *newest* end.
    ///
    /// Reaching the branch needs a `delta` that does not itself overflow the
    /// subtraction; see the sibling test for why.
    #[test]
    fn ageing_saturates_by_shrinking_the_count() {
        let leds = leds();
        let mut tracker = HitTracker::new();
        tracker.record(&leds, 0, 0);
        tracker.record(&leds, 0, 1);

        // 65535 - 65535 == 0, and 0 < 0 is false, so this step is still
        // additive and lands both ticks exactly on the ceiling.
        tracker.age(65535);
        assert_eq!(tracker.hit(0).unwrap().tick, 65535);
        assert_eq!(tracker.count(), 2);

        // Now one more millisecond would overflow: the guard fires, and each
        // saturated entry decrements `count` instead of being removed.
        tracker.age(1);
        assert_eq!(tracker.count(), 0, "both entries saturated");
    }

    /// A `delta` larger than `u16::MAX` wraps inside the guard's own
    /// subtraction (`UINT16_MAX - deltaTime` promotes to `uint32_t`), so the
    /// comparison is false and the ticks take a wrapped addition instead of
    /// saturating. Counter-intuitive, but it is what the C computes.
    #[test]
    fn an_oversized_delta_wraps_instead_of_saturating() {
        let leds = leds();
        let mut tracker = HitTracker::new();
        tracker.record(&leds, 0, 0);

        tracker.age(100_000);
        // 100_000 as u16 == 34_464, added to a tick of 0.
        assert_eq!(tracker.hit(0).unwrap().tick, (100_000u32 % 65_536) as u16);
        assert_eq!(tracker.count(), 1, "nothing saturated");
    }

    #[test]
    fn most_recent_tick_prefers_the_latest_hit_on_that_led() {
        let leds = leds();
        let mut tracker = HitTracker::new();
        tracker.record(&leds, 0, 2);
        tracker.age(100);
        // Hit the same LED again; its newest tick is 0.
        tracker.record(&leds, 0, 2);

        assert_eq!(
            tracker.most_recent_tick(2, u16::MAX),
            0,
            "the newest hit on LED 2 wins over the older one at tick 100"
        );
        assert_eq!(
            tracker.most_recent_tick(1, u16::MAX),
            u16::MAX,
            "a LED that was never hit keeps the ceiling"
        );
    }

    #[test]
    fn most_recent_tick_respects_the_ceiling() {
        let leds = leds();
        let mut tracker = HitTracker::new();
        tracker.record(&leds, 0, 1);
        tracker.age(50);

        // A ceiling below the stored tick means "nothing recent enough".
        assert_eq!(tracker.most_recent_tick(1, 10), 10);
        assert_eq!(tracker.most_recent_tick(1, 60), 50);
    }

    #[test]
    fn a_shared_matrix_position_records_every_led() {
        // Two LEDs under one key, as a both-sides-lit key would have.
        let shared = LedConfig::from_matrix(
            [LedPoint::new(100, 10), LedPoint::new(100, 20)],
            [(0, 0), (0, 0)],
        );
        let mut tracker = HitTracker::new();
        tracker.record(&shared, 0, 0);
        assert_eq!(tracker.count(), 2);
        assert_eq!(tracker.hit(0).unwrap().y, 10);
        assert_eq!(tracker.hit(1).unwrap().y, 20);
    }
}
