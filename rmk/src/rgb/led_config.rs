//! The LED layout: what each LED's physical position is, what it represents,
//! and which key (if any) it sits under.
//!
//! This is a port of QMK's `g_led_config` (`led_config_t` in
//! `quantum/rgb_matrix/rgb_matrix_types.h`). QMK stores three parallel arrays
//! keyed by LED index — a `matrix_co[row][col] -> index` lookup, a
//! `point[index] -> (x, y)` table, and a `flags[index]` bitmap.
//!
//! We keep only the per-LED form (`matrix_pos`, `points`, `flags`) and answer
//! the row/column lookup with a scan. QMK already scans for the reverse
//! direction (`get_matrix_pos_for_led`), and a keyboard has tens of LEDs, so
//! the second table would only be a cache we would have to keep in sync.

use serde::{Deserialize, Serialize};

/// The `NO_LED` sentinel: no LED occupies this position.
pub const NO_LED: u8 = 255;

/// `LED_FLAG_NONE` — the LED does not belong to a recognised group.
pub const LED_FLAG_NONE: u8 = 0x00;
/// `LED_FLAG_MODIFIER` — the LED sits on a modifier key.
pub const LED_FLAG_MODIFIER: u8 = 0x01;
/// `LED_FLAG_UNDERGLOW` — the LED is case/underglow rather than a key.
pub const LED_FLAG_UNDERGLOW: u8 = 0x02;
/// `LED_FLAG_KEYLIGHT` — the LED backlights a key.
pub const LED_FLAG_KEYLIGHT: u8 = 0x04;
/// `LED_FLAG_INDICATOR` — the LED is a status indicator.
pub const LED_FLAG_INDICATOR: u8 = 0x08;
/// `LED_FLAG_ALL` — every group at once, the default.
pub const LED_FLAG_ALL: u8 = 0xFF;

/// `HAS_FLAGS`: true when *every* bit in `flags` is set in `bits`.
#[inline]
pub const fn has_flags(bits: u8, flags: u8) -> bool {
    bits & flags == flags
}

/// `HAS_ANY_FLAGS`: true when *any* bit in `flags` is set in `bits`.
#[inline]
pub const fn has_any_flags(bits: u8, flags: u8) -> bool {
    bits & flags != 0
}

/// A physical LED position in QMK's coordinate space.
///
/// `x` runs `0..=224` and `y` `0..=64`; both may exceed those on keyboards
/// that set a custom centre, since the type is a byte. The ranges are a
/// convention so that centre-based effects have room to run off the edge
/// before wrapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct LedPoint {
    pub x: u8,
    pub y: u8,
}

impl LedPoint {
    pub const fn new(x: u8, y: u8) -> Self {
        Self { x, y }
    }
}

/// QMK's default keyboard centre, `k_rgb_matrix_center`.
pub const DEFAULT_CENTER: LedPoint = LedPoint::new(112, 32);

/// The LED table: one entry per LED in the chain.
///
/// `LED_COUNT` is the length of the physical chain, and the index used
/// everywhere else in the engine is a position in these arrays.
#[derive(Clone, Copy, Debug)]
pub struct LedConfig<const LED_COUNT: usize> {
    points: [LedPoint; LED_COUNT],
    flags: [u8; LED_COUNT],
    /// Electrical `(row, col)` per LED, `(NO_LED, NO_LED)` when the LED is not
    /// under a key (underglow, or a bare indicator).
    matrix_pos: [(u8, u8); LED_COUNT],
}

impl<const LED_COUNT: usize> LedConfig<LED_COUNT> {
    /// Build a table. Use [`LedConfig::unmapped`] when the LED sits under no
    /// key, and see [`LedConfig::from_matrix`] for the common all-key case.
    pub const fn new(points: [LedPoint; LED_COUNT], flags: [u8; LED_COUNT], matrix_pos: [(u8, u8); LED_COUNT]) -> Self {
        Self {
            points,
            flags,
            matrix_pos,
        }
    }

    /// A table where LED `index` sits at `(row, col)`.
    ///
    /// Most keyboards map every LED to a key, which is what this spells.
    pub const fn from_matrix(points: [LedPoint; LED_COUNT], matrix_pos: [(u8, u8); LED_COUNT]) -> Self {
        Self::new(points, [LED_FLAG_KEYLIGHT; LED_COUNT], matrix_pos)
    }

    /// The `(row, col)` marker for an LED that no key sits under.
    pub const UNMAPPED: (u8, u8) = (NO_LED, NO_LED);

    /// Number of LEDs in the chain.
    #[inline]
    pub const fn len(&self) -> usize {
        LED_COUNT
    }

    /// True when the chain is empty, which no real keyboard is.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        LED_COUNT == 0
    }

    /// Physical position of LED `index`.
    ///
    /// Panics if `index >= LED_COUNT`; effect code only ever indexes within
    /// the batch it was handed.
    #[inline]
    pub const fn point(&self, index: usize) -> LedPoint {
        self.points[index]
    }

    /// `LED_FLAG_*` bitmap of LED `index`.
    #[inline]
    pub const fn flags(&self, index: usize) -> u8 {
        self.flags[index]
    }

    /// Electrical position of LED `index`, or `None` when no key sits under it.
    ///
    /// This is QMK's `get_matrix_pos_for_led`, including its `0xFF` sentinel
    /// for "not part of the key matrix".
    #[inline]
    pub const fn matrix_pos(&self, index: usize) -> Option<(u8, u8)> {
        let pos = self.matrix_pos[index];
        if pos.0 == NO_LED && pos.1 == NO_LED {
            None
        } else {
            Some(pos)
        }
    }

    /// All LEDs under the electrical position `(row, col)`.
    ///
    /// This is QMK's `rgb_matrix_map_row_column_to_led`. It returns into a
    /// caller-supplied buffer and yields the count, because a position can
    /// carry more than one LED (a key lit from both sides, say). With 19-odd
    /// LEDs the scan is cheaper than maintaining the inverse table.
    pub fn leds_at(&self, row: u8, col: u8, out: &mut [u8]) -> usize {
        let mut count = 0;
        for (index, pos) in self.matrix_pos.iter().enumerate() {
            if pos.0 == row && pos.1 == col {
                if count == out.len() {
                    break;
                }
                out[count] = index as u8;
                count += 1;
            }
        }
        count
    }

    /// First LED under `(row, col)`, the common single-LED-per-key case.
    pub fn led_at(&self, row: u8, col: u8) -> Option<u8> {
        let mut out = [0u8; 1];
        match self.leds_at(row, col, &mut out) {
            0 => None,
            _ => Some(out[0]),
        }
    }
}

/// One LED's description, flattened so it can be stored outside a const-generic
/// array.
///
/// [`LedConfig`] carries a const-generic length and therefore cannot live in a
/// `static` without knowing the board's LED count. This is the plain-value form
/// the shared global keeps, and it is also what `GET_LED_INFO` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct LedInfo {
    pub point: LedPoint,
    pub flags: u8,
    /// `(row, col)`, or `None` when no key sits under the LED.
    pub matrix_pos: Option<(u8, u8)>,
}

impl LedInfo {
    /// A blank entry, for a table's unused tail.
    pub const EMPTY: Self = Self {
        point: LedPoint { x: 0, y: 0 },
        flags: LED_FLAG_NONE,
        matrix_pos: None,
    };
}

impl<const LED_COUNT: usize> LedConfig<LED_COUNT> {
    /// Flatten LED `index` into a [`LedInfo`].
    pub fn info(&self, index: usize) -> LedInfo {
        LedInfo {
            point: self.points[index],
            flags: self.flags[index],
            matrix_pos: self.matrix_pos(index),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 matrix with one LED per key, plus an unmapped underglow LED.
    const ROW: usize = 2;
    const COL: usize = 2;

    fn sample() -> LedConfig<5> {
        LedConfig::new(
            [
                LedPoint::new(0, 0),
                LedPoint::new(224, 0),
                LedPoint::new(0, 64),
                LedPoint::new(224, 64),
                LedPoint::new(112, 32),
            ],
            [
                LED_FLAG_KEYLIGHT,
                LED_FLAG_KEYLIGHT,
                LED_FLAG_KEYLIGHT,
                LED_FLAG_MODIFIER,
                LED_FLAG_UNDERGLOW,
            ],
            [(0, 0), (0, 1), (1, 0), (1, 1), LedConfig::<5>::UNMAPPED],
        )
    }

    #[test]
    fn flags_helpers_match_qmk_macros() {
        assert!(has_flags(0xFF, LED_FLAG_ALL));
        assert!(has_flags(LED_FLAG_KEYLIGHT | LED_FLAG_MODIFIER, LED_FLAG_KEYLIGHT));
        assert!(!has_flags(LED_FLAG_KEYLIGHT, LED_FLAG_KEYLIGHT | LED_FLAG_MODIFIER));
        assert!(has_any_flags(LED_FLAG_KEYLIGHT, LED_FLAG_KEYLIGHT | LED_FLAG_MODIFIER));
        assert!(!has_any_flags(LED_FLAG_NONE, LED_FLAG_KEYLIGHT));
    }

    #[test]
    fn matrix_position_round_trips() {
        let leds = sample();
        for row in 0..ROW as u8 {
            for col in 0..COL as u8 {
                let index = leds
                    .led_at(row, col)
                    .unwrap_or_else(|| panic!("no LED at ({row}, {col})"));
                assert_eq!(leds.matrix_pos(index as usize), Some((row, col)));
            }
        }
    }

    #[test]
    fn unmapped_leds_report_no_matrix_position() {
        let leds = sample();
        assert_eq!(leds.matrix_pos(4), None, "the underglow LED is not under a key");
        // It is still addressable and still has a position and flags.
        assert_eq!(leds.point(4), LedPoint::new(112, 32));
        assert_eq!(leds.flags(4), LED_FLAG_UNDERGLOW);
    }

    #[test]
    fn leds_at_collects_every_led_on_a_position() {
        // Two LEDs sharing key (0, 0), as a both-sides-lit key would have.
        let leds = LedConfig::new(
            [LedPoint::new(1, 1), LedPoint::new(2, 2)],
            [LED_FLAG_KEYLIGHT, LED_FLAG_KEYLIGHT],
            [(0, 0), (0, 0)],
        );
        let mut out = [0u8; 4];
        assert_eq!(leds.leds_at(0, 0, &mut out), 2);
        assert_eq!(&out[..2], &[0, 1]);
        assert_eq!(leds.led_at(0, 0), Some(0), "the first match wins");
        assert_eq!(leds.leds_at(1, 1, &mut out), 0);
    }

    #[test]
    fn leds_at_stops_at_the_buffer_length() {
        let leds = LedConfig::new(
            [LedPoint::new(1, 1), LedPoint::new(2, 2)],
            [LED_FLAG_KEYLIGHT, LED_FLAG_KEYLIGHT],
            [(0, 0), (0, 0)],
        );
        let mut one = [0u8; 1];
        assert_eq!(leds.leds_at(0, 0, &mut one), 1);
    }

    #[test]
    fn length_helpers_track_the_chain() {
        assert_eq!(sample().len(), 5);
        assert!(!sample().is_empty());
        assert!(LedConfig::<0>::new([], [], []).is_empty());
    }
}
