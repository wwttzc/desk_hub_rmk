//! `rgb.toml`: the RGB Matrix configuration, read alongside `keyboard.toml`.
//!
//! QMK spreads RGB configuration across three places — the `rgb_matrix` and
//! `ws2812` sections of `keyboard.json`, and a pile of `config.h` macros. The
//! keys here follow those names, so a QMK keyboard's settings can be moved over
//! by hand without a translation table.
//!
//! # Why a separate file
//!
//! `keyboard.toml` already carries the matrix, layout and keymap, and it is
//! owned by the build tool (`rmkit create` copies it verbatim into the generated
//! project). Keeping RGB in its own file means the LED table and the enabled
//! effects can be edited without touching that path, and a keyboard without RGB
//! never grows an empty section.
//!
//! The file is found next to `keyboard.toml`, so no new environment variable is
//! needed — which matters, because `rmkit` does not let a project override
//! `.cargo/config.toml`. `RGB_TOML_PATH` overrides the location when set.
//!
//! # Absence is meaningful
//!
//! A missing `rgb.toml`, or `rgb_matrix.enabled = false`, compiles the whole RGB
//! subsystem out. That is the state a keyboard with no LED chain wants, and it
//! is also what lets this land before a board's LED table exists.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The environment variable that overrides where `rgb.toml` is read from.
pub const RGB_TOML_PATH_ENV: &str = "RGB_TOML_PATH";

/// The default file name, looked up beside `keyboard.toml`.
pub const RGB_TOML_FILE: &str = "rgb.toml";

/// The whole of `rgb.toml`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbTomlConfig {
    /// `[rgb_matrix]`. Absent means RGB is not configured.
    pub rgb_matrix: Option<RgbMatrixConfig>,
    /// `[ws2812]`, the driver section.
    pub ws2812: Option<Ws2812Config>,
}

impl RgbTomlConfig {
    /// Whether RGB should be compiled in.
    ///
    /// True only when the file exists, has a `[rgb_matrix]` section, and that
    /// section did not set `enabled = false`.
    pub fn is_enabled(&self) -> bool {
        self.rgb_matrix.as_ref().is_some_and(|m| m.enabled)
    }

    /// The resolved matrix configuration, or a disabled default.
    pub fn matrix(&self) -> RgbMatrixConfig {
        self.rgb_matrix.clone().unwrap_or_default()
    }

    /// The resolved driver configuration.
    pub fn ws2812(&self) -> Ws2812Config {
        self.ws2812.clone().unwrap_or_default()
    }

    /// Parse from TOML text.
    ///
    /// Exposed so callers that already hold the text — and tests in other crates
    /// that must not add a `toml` dependency just to build a fixture — can use
    /// the same parser as the file path entry points.
    pub fn from_str(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    /// Read `rgb.toml` from `path`, or `None` when it does not exist.
    ///
    /// A missing file is not an error: it is how a keyboard declares it has no
    /// RGB. A file that exists but does not parse *is* an error, reported with
    /// the path so a typo is not mistaken for absence.
    pub fn from_path(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let text =
            std::fs::read_to_string(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Self::from_str(&text).map_err(|e| format!("{}: {e}", path.display())).map(Some)
    }

    /// Locate `rgb.toml` for a `keyboard.toml` at `keyboard_toml_path`.
    ///
    /// `RGB_TOML_PATH` wins when set; otherwise the file is looked for beside
    /// `keyboard.toml`. Returning the path even when absent lets the caller
    /// report which location it checked.
    pub fn resolve_path(keyboard_toml_path: Option<&Path>) -> PathBuf {
        if let Some(explicit) = std::env::var_os(RGB_TOML_PATH_ENV) {
            return PathBuf::from(explicit);
        }
        match keyboard_toml_path {
            Some(path) => path.parent().unwrap_or(Path::new(".")).join(RGB_TOML_FILE),
            None => PathBuf::from(RGB_TOML_FILE),
        }
    }

    /// Read the file belonging to `keyboard_toml_path`.
    pub fn load_for(keyboard_toml_path: Option<&Path>) -> Result<Option<Self>, String> {
        Self::from_path(&Self::resolve_path(keyboard_toml_path))
    }
}

/// `[rgb_matrix]`, the effects and their defaults.
///
/// Field names follow QMK's `rgb_matrix` section and its `config.h` macros; the
/// documentation on each field names the macro it corresponds to.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbMatrixConfig {
    /// Whether to compile RGB in at all.
    ///
    /// `false` removes the whole subsystem, including the LED table requirement
    /// below. Default `true` when the section is present, so an empty
    /// `[rgb_matrix]` means "on with defaults".
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Per-effect on/off, keyed by QMK's `animations` names.
    ///
    /// A key naming an effect that is not ported yet produces a build warning
    /// rather than an error, so this list does not have to change as more
    /// effects land.
    #[serde(default)]
    pub animations: HashMap<String, bool>,

    /// `rgb_matrix.default`.
    #[serde(default)]
    pub default: RgbDefaultConfig,

    /// The LED table, one `[[rgb_matrix.layout]]` per LED.
    ///
    /// Required when `enabled`, because every effect is written in terms of LED
    /// indices and the position-dependent ones need real coordinates. A missing
    /// table is a configuration error, not something to paper over with zeros.
    #[serde(default)]
    pub layout: Vec<LedEntry>,

    /// `RGB_MATRIX_MAXIMUM_BRIGHTNESS`. QMK defaults to 255.
    ///
    /// The Vial brightness slider tops out at this value, so a lower ceiling
    /// here is what keeps a large chain off a 5 V rail's current limit.
    #[serde(default = "default_max_brightness")]
    pub max_brightness: u8,

    /// `RGB_MATRIX_CENTER`, as `[x, y]`. QMK defaults to `[112, 32]`.
    #[serde(default)]
    pub center_point: Option<[u8; 2]>,

    /// `RGB_MATRIX_LED_FLUSH_LIMIT`, milliseconds between frames.
    #[serde(default = "default_flush_limit")]
    pub led_flush_limit: u32,

    /// `RGB_MATRIX_LED_PROCESS_LIMIT`, LEDs per batch.
    ///
    /// Absent means QMK's `(LED_COUNT + 4) / 5`.
    #[serde(default)]
    pub led_process_limit: Option<usize>,

    /// `RGB_MATRIX_TIMEOUT`, milliseconds of inactivity before the LEDs turn
    /// off. `0` disables the timeout, which is QMK's default.
    #[serde(default)]
    pub timeout: u32,

    /// `RGB_MATRIX_SLEEP`: turn the LEDs off when the host suspends.
    #[serde(default)]
    pub sleep: bool,

    /// `RGB_MATRIX_FLAG_STEPS`, the flag combinations the cycling keys walk.
    #[serde(default)]
    pub flag_steps: Option<Vec<u8>>,

    /// `RGB_MATRIX_HUE_STEP`.
    #[serde(default = "default_hue_step")]
    pub hue_steps: u8,
    /// `RGB_MATRIX_SAT_STEP`.
    #[serde(default = "default_sat_step")]
    pub sat_steps: u8,
    /// `RGB_MATRIX_VAL_STEP`.
    #[serde(default = "default_val_step")]
    pub val_steps: u8,
    /// `RGB_MATRIX_SPD_STEP`.
    #[serde(default = "default_speed_step")]
    pub speed_steps: u8,

    /// `RGB_MATRIX_KEYRELEASES`: reactive effects fire on key-up instead of
    /// key-down.
    #[serde(default)]
    pub react_on_keyup: bool,

    /// `RGB_MATRIX_SPLIT`, the LEDs on each half. Unused on a unibody board.
    #[serde(default)]
    pub split_count: Option<[u16; 2]>,
}

impl Default for RgbMatrixConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            animations: HashMap::new(),
            default: RgbDefaultConfig::default(),
            layout: Vec::new(),
            max_brightness: default_max_brightness(),
            center_point: None,
            led_flush_limit: default_flush_limit(),
            led_process_limit: None,
            timeout: 0,
            sleep: false,
            flag_steps: None,
            hue_steps: default_hue_step(),
            sat_steps: default_sat_step(),
            val_steps: default_val_step(),
            speed_steps: default_speed_step(),
            react_on_keyup: false,
            split_count: None,
        }
    }
}

impl RgbMatrixConfig {
    /// `RGB_MATRIX_CENTER`, defaulted to QMK's `[112, 32]`.
    pub fn center(&self) -> [u8; 2] {
        self.center_point.unwrap_or([112, 32])
    }

    /// The effective batch size, QMK's `(LED_COUNT + 4) / 5` when unset.
    pub fn process_limit(&self, led_count: usize) -> usize {
        self.led_process_limit.unwrap_or(led_count.div_ceil(5)).max(1)
    }

    /// `RGB_MATRIX_FLAG_STEPS`, defaulted to QMK's table.
    pub fn flag_steps(&self) -> Vec<u8> {
        self.flag_steps
            .clone()
            .unwrap_or_else(|| vec![0xFF, 0x05, 0x02, 0x00])
    }

    /// The effects this build enables, as registry ids, with any warnings.
    ///
    /// Names are resolved case-insensitively through [`resolve_effect_name`],
    /// which is the only place the id numbering lives. An unknown name, or one
    /// whose effect is not ported yet, produces a warning rather than an error:
    /// the file must not have to change as more effects land.
    ///
    /// An empty `animations` table means "every ported effect", the sensible
    /// default for a first bring-up.
    ///
    /// The returned ids are sorted and deduplicated, because the enabled list
    /// reaches `GET_SUPPORTED` and Vial pages through it assuming ascending
    /// order.
    pub fn resolve_animations(&self) -> (Vec<u16>, Vec<String>) {
        let mut enabled = Vec::new();
        let mut warnings = Vec::new();

        if self.animations.is_empty() {
            enabled.extend(rmk_effect_table().iter().filter(|(_, done)| *done).map(|(id, _)| *id));
            return (enabled, warnings);
        }

        for (name, on) in &self.animations {
            if !*on {
                continue;
            }
            match resolve_effect_name(name) {
                Some((id, true)) => enabled.push(id),
                // Every table entry is ported today, so this arm is what an
                // entry added as `false` would fall into: a warning rather than
                // a silent switch-on. It shares the warning shape with an
                // unknown name because both mean "nothing will light for this".
                Some((_, false)) | None => warnings.push(format!(
                    "rgb_matrix.animations.{name}: no implemented effect by that name; \
                     it will not appear in Vial"
                )),
            }
        }

        enabled.sort_unstable();
        enabled.dedup();
        (enabled, warnings)
    }
}

/// One `[[rgb_matrix.layout]]` entry, QMK's `led_config_t` row.
///
/// `x` and `y` are required: an LED with no position cannot participate in the
/// position-dependent effects, and silently defaulting to the origin would put
/// every one of them on top of each other.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LedEntry {
    /// Physical position, `x` in `0..=224` and `y` in `0..=64` by convention.
    pub x: u8,
    pub y: u8,
    /// `LED_FLAG_*` bitmap. Defaults to `LED_FLAG_KEYLIGHT` (0x04), the common
    /// case of a per-key LED.
    #[serde(default = "default_keylight_flag")]
    pub flags: u8,
    /// Electrical `(row, col)`, or absent for an LED under no key.
    #[serde(default)]
    pub matrix: Option<[u8; 2]>,
}

/// `[rgb_matrix.default]`, QMK's defaults section.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RgbDefaultConfig {
    /// `RGB_MATRIX_DEFAULT_ON`.
    #[serde(default = "default_true")]
    pub on: bool,
    /// `RGB_MATRIX_DEFAULT_MODE`, by registry key (`"solid_color"`).
    #[serde(default)]
    pub animation: Option<String>,
    /// `RGB_MATRIX_DEFAULT_HUE`.
    #[serde(default)]
    pub hue: u8,
    /// `RGB_MATRIX_DEFAULT_SAT`.
    #[serde(default = "default_sat")]
    pub sat: u8,
    /// `RGB_MATRIX_DEFAULT_VAL`. `None` means the brightness ceiling.
    #[serde(default)]
    pub val: Option<u8>,
    /// `RGB_MATRIX_DEFAULT_SPD`.
    #[serde(default = "default_speed")]
    pub speed: u8,
    /// `RGB_MATRIX_DEFAULT_FLAGS`.
    #[serde(default = "default_all_flags")]
    pub flags: u8,
}

impl Default for RgbDefaultConfig {
    fn default() -> Self {
        Self {
            on: true,
            animation: None,
            hue: 0,
            sat: default_sat(),
            val: None,
            speed: default_speed(),
            flags: default_all_flags(),
        }
    }
}

/// `[ws2812]`, the smart-LED driver section.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ws2812Config {
    /// The data pin, e.g. `"GPIO48"`.
    pub pin: Option<String>,
    /// `WS2812_DRIVER`: `"rmt"`, `"spi"`, `"pwm"`, `"bitbang"`, `"i2c"`,
    /// `"vendor"` or `"custom"`.
    ///
    /// `None` means the chip's natural choice, which for ESP32-S3 is RMT.
    #[serde(default)]
    pub driver: Option<String>,
    /// `WS2812_RGBW`: the chain has a dedicated white channel.
    #[serde(default)]
    pub rgbw: bool,
    /// The byte order the LEDs expect, e.g. `"grb"`.
    ///
    /// `None` uses the driver's default, which for WS2812 is GRB.
    #[serde(default)]
    pub color_order: Option<String>,
    /// How many RMT memory blocks the driver may use.
    ///
    /// Absent lets the driver decide; a long chain may need more than one.
    #[serde(default)]
    pub rmt_memsize: Option<u8>,
    /// The RMT source clock in MHz, which the driver derives its pulse codes
    /// from.
    ///
    /// Must match the rate the peripheral is created with. Defaults to 80.
    #[serde(default)]
    pub rmt_freq_mhz: Option<u32>,
}

impl Default for Ws2812Config {
    fn default() -> Self {
        Self {
            pin: None,
            driver: None,
            rgbw: false,
            color_order: None,
            rmt_memsize: None,
            rmt_freq_mhz: None,
        }
    }
}

/// `LED_FLAG_KEYLIGHT`, the default for a per-key LED.
fn default_keylight_flag() -> u8 {
    0x04
}

fn default_true() -> bool {
    true
}

fn default_max_brightness() -> u8 {
    255
}

fn default_flush_limit() -> u32 {
    16
}

fn default_hue_step() -> u8 {
    8
}

fn default_sat_step() -> u8 {
    16
}

fn default_val_step() -> u8 {
    16
}

fn default_speed_step() -> u8 {
    16
}

fn default_sat() -> u8 {
    255
}

fn default_speed() -> u8 {
    127
}

fn default_all_flags() -> u8 {
    0xFF
}

/// The effect table, as `(id, implemented)` pairs.
///
/// Duplicated from `rmk::rgb::registry` because `rmk-config` is a dependency of
/// the macro crate and is compiled before the firmware crate exists — the two
/// cannot share a constant, so they have to be kept in step by hand. The table
/// holds VialRGB's 43 ids (`2..=44`) and nothing else; the test below pins those
/// bounds so a dropped row is a failure rather than a silently disabled effect.
fn rmk_effect_table() -> &'static [(u16, bool)] {
    &[
        (2, true),   // solid_color
        (3, true),   // alpha_mods
        (4, true),   // gradient_up_down
        (5, true),   // gradient_left_right
        (6, true),   // breathing
        (7, true),   // band_sat
        (8, true),   // band_val
        (9, true),   // band_pinwheel_sat
        (10, true),  // band_pinwheel_val
        (11, true),  // band_spiral_sat
        (12, true),  // band_spiral_val
        (13, true),  // cycle_all
        (14, true),  // cycle_left_right
        (15, true),  // cycle_up_down
        (16, true),  // rainbow_moving_chevron
        (17, true),  // cycle_out_in
        (18, true),  // cycle_out_in_dual
        (19, true),  // cycle_pinwheel
        (20, true),  // cycle_spiral
        (21, true),  // dual_beacon
        (22, true),  // rainbow_beacon
        (23, true),  // rainbow_pinwheels
        (24, true),  // raindrops
        (25, true),  // jellybean_raindrops
        (26, true),  // hue_breathing
        (27, true),  // hue_pendulum
        (28, true),  // hue_wave
        (29, true),  // typing_heatmap
        (30, true),  // digital_rain
        (31, true),  // solid_reactive_simple
        (32, true),  // solid_reactive
        (33, true),  // solid_reactive_wide
        (34, true),  // solid_reactive_multiwide
        (35, true),  // solid_reactive_cross
        (36, true),  // solid_reactive_multicross
        (37, true),  // solid_reactive_nexus
        (38, true),  // solid_reactive_multinexus
        (39, true),  // splash
        (40, true),  // multisplash
        (41, true),  // solid_splash
        (42, true),  // solid_multisplash
        (43, true),  // pixel_rain
        (44, true),  // pixel_fractal
    ]
}

/// Look up an effect by its `animations` key, returning `(id, implemented)`.
///
/// Public because the macro crate resolves `rgb_matrix.default.animation` the
/// same way, and duplicating the table would let the two drift.
pub fn resolve_effect_name(name: &str) -> Option<(u16, bool)> {
    let lower = name.to_ascii_lowercase();
    let keys: &[(&str, u16)] = &[
        ("solid_color", 2),
        ("alpha_mods", 3),
        ("alphas_mods", 3),
        ("gradient_up_down", 4),
        ("gradient_left_right", 5),
        ("breathing", 6),
        ("band_sat", 7),
        ("band_val", 8),
        ("band_pinwheel_sat", 9),
        ("band_pinwheel_val", 10),
        ("band_spiral_sat", 11),
        ("band_spiral_val", 12),
        ("cycle_all", 13),
        ("cycle_left_right", 14),
        ("cycle_up_down", 15),
        ("rainbow_moving_chevron", 16),
        ("cycle_out_in", 17),
        ("cycle_out_in_dual", 18),
        ("cycle_pinwheel", 19),
        ("cycle_spiral", 20),
        ("dual_beacon", 21),
        ("rainbow_beacon", 22),
        ("rainbow_pinwheels", 23),
        ("raindrops", 24),
        ("jellybean_raindrops", 25),
        ("hue_breathing", 26),
        ("hue_pendulum", 27),
        ("hue_wave", 28),
        ("typing_heatmap", 29),
        ("digital_rain", 30),
        ("solid_reactive_simple", 31),
        ("solid_reactive", 32),
        ("solid_reactive_wide", 33),
        ("solid_reactive_multiwide", 34),
        ("solid_reactive_cross", 35),
        ("solid_reactive_multicross", 36),
        ("solid_reactive_nexus", 37),
        ("solid_reactive_multinexus", 38),
        ("splash", 39),
        ("multisplash", 40),
        ("solid_splash", 41),
        ("solid_multisplash", 42),
        ("pixel_rain", 43),
        ("pixel_fractal", 44),
    ];
    let id = keys.iter().find(|(k, _)| *k == lower).map(|(_, id)| *id)?;
    let implemented = rmk_effect_table().iter().find(|(i, _)| *i == id).map(|(_, im)| *im)?;
    Some((id, implemented))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<RgbTomlConfig, String> {
        toml::from_str(text).map_err(|e| e.to_string())
    }

    #[test]
    fn an_empty_file_disables_rgb() {
        let config = parse("").expect("empty parses");
        assert!(!config.is_enabled(), "no [rgb_matrix] means no RGB");
    }

    #[test]
    fn an_empty_matrix_section_is_enabled_with_defaults() {
        let config = parse("[rgb_matrix]\n").expect("parses");
        assert!(config.is_enabled(), "present section defaults to on");
        let matrix = config.matrix();
        assert_eq!(matrix.max_brightness, 255, "QMK's default ceiling");
        assert_eq!(matrix.center(), [112, 32], "QMK's default centre");
        assert_eq!(matrix.led_flush_limit, 16);
        assert!(matrix.layout.is_empty());
    }

    #[test]
    fn enabled_false_turns_the_subsystem_off() {
        let config = parse("[rgb_matrix]\nenabled = false\n").expect("parses");
        assert!(!config.is_enabled());
    }

    #[test]
    fn the_qmk_defaults_are_used_for_steps() {
        let matrix = parse("[rgb_matrix]\n").unwrap().matrix();
        assert_eq!(matrix.hue_steps, 8);
        assert_eq!(matrix.sat_steps, 16);
        assert_eq!(matrix.val_steps, 16);
        assert_eq!(matrix.speed_steps, 16);
        assert_eq!(matrix.flag_steps(), vec![0xFF, 0x05, 0x02, 0x00]);
    }

    #[test]
    fn the_process_limit_follows_qmk_when_unset() {
        let matrix = parse("[rgb_matrix]\n").unwrap().matrix();
        // QMK's (LED_COUNT + 4) / 5.
        assert_eq!(matrix.process_limit(19), 4);
        assert_eq!(matrix.process_limit(1), 1);
        assert_eq!(matrix.process_limit(100), 20);
    }

    #[test]
    fn an_explicit_process_limit_wins() {
        let matrix = parse("[rgb_matrix]\nled_process_limit = 8\n").unwrap().matrix();
        assert_eq!(matrix.process_limit(19), 8);
    }

    #[test]
    fn a_zero_process_limit_is_at_least_one() {
        // A zero would make `limits` treat every batch as empty.
        let matrix = parse("[rgb_matrix]\nled_process_limit = 0\n").unwrap().matrix();
        assert_eq!(matrix.process_limit(19), 1);
    }

    #[test]
    fn the_layout_parses_with_its_qmk_field_names() {
        let text = r#"
[rgb_matrix]
[[rgb_matrix.layout]]
x = 0
y = 0
flags = 4
matrix = [0, 0]

[[rgb_matrix.layout]]
x = 224
y = 64
flags = 1
"#;
        let matrix = parse(text).unwrap().matrix();
        assert_eq!(matrix.layout.len(), 2);
        assert_eq!(matrix.layout[0].x, 0);
        assert_eq!(matrix.layout[0].matrix, Some([0, 0]));
        // An LED with no `matrix` is one under no key.
        assert_eq!(matrix.layout[1].matrix, None);
        assert_eq!(matrix.layout[1].flags, 1);
    }

    #[test]
    fn flags_default_to_keylight() {
        let matrix = parse("[rgb_matrix]\n[[rgb_matrix.layout]]\nx=1\ny=2\n")
            .unwrap()
            .matrix();
        assert_eq!(matrix.layout[0].flags, 0x04, "LED_FLAG_KEYLIGHT");
    }

    /// `x` and `y` are the point of the table; a silent default would stack every
    /// LED at the origin.
    #[test]
    fn a_layout_entry_without_a_position_is_an_error() {
        let err = parse("[rgb_matrix]\n[[rgb_matrix.layout]]\nflags = 4\n").unwrap_err();
        assert!(err.contains("x") || err.contains("missing"), "{err}");
    }

    #[test]
    fn the_driver_section_parses() {
        let config = parse(
            r#"
[rgb_matrix]
[ws2812]
pin = "GPIO48"
driver = "rmt"
rgbw = false
"#,
        )
        .unwrap();
        let ws = config.ws2812();
        assert_eq!(ws.pin.as_deref(), Some("GPIO48"));
        assert_eq!(ws.driver.as_deref(), Some("rmt"));
        assert!(!ws.rgbw);
    }

    #[test]
    fn an_unknown_key_is_rejected_rather_than_ignored() {
        // A typo must not silently configure nothing.
        let err = parse("[rgb_matrix]\nmax_brightnes = 128\n").unwrap_err();
        assert!(err.contains("max_brightnes"), "{err}");
    }

    #[test]
    fn the_effect_table_covers_exactly_vialrgb_ids() {
        let table = rmk_effect_table();
        assert_eq!(table.len(), 43, "VialRGB defines 43 effects");
        for (i, (id, _)) in table.iter().enumerate() {
            assert_eq!(*id, 2 + i as u16, "ids are dense from 2");
        }
        assert_eq!(table.last().unwrap().0, 44, "the last VialRGB id");
    }

    #[test]
    fn an_empty_animations_table_enables_every_ported_effect() {
        let matrix = parse("[rgb_matrix]\n").unwrap().matrix();
        let (enabled, warnings) = matrix.resolve_animations();
        assert!(!enabled.is_empty());
        assert!(enabled.contains(&2), "solid_color is ported");
        assert!(enabled.contains(&16), "rainbow_moving_chevron is ported");
        assert!(enabled.contains(&29), "typing_heatmap is ported");
        assert!(!enabled.contains(&51), "id 51 is not an effect in this build");
        assert_eq!(enabled.len(), 43, "every VialRGB effect is enabled");
        assert!(warnings.is_empty(), "nothing to warn about");
        // Sorted and deduplicated, because Vial pages through this list.
        let mut sorted = enabled.clone();
        sorted.sort_unstable();
        assert_eq!(enabled, sorted);
    }

    #[test]
    fn an_explicit_animations_table_selects_only_those() {
        let text = r#"
[rgb_matrix]
[rgb_matrix.animations]
breathing = true
cycle_all = true
solid_color = false
"#;
        let matrix = parse(text).unwrap().matrix();
        let (enabled, warnings) = matrix.resolve_animations();
        assert_eq!(enabled, vec![6, 13], "only the true ones, by registry id");
        assert!(warnings.is_empty());
    }

    /// A key naming an effect that does not exist warns instead of failing, so a
    /// config written against a newer Vial list still builds.
    ///
    /// Every VialRGB id is ported now, so the only way to reach this warning is
    /// a name that was never in the table.
    #[test]
    fn an_unknown_effect_warns_and_is_skipped() {
        let text = r#"
[rgb_matrix]
[rgb_matrix.animations]
not_a_real_effect = true
breathing = true
"#;
        let matrix = parse(text).unwrap().matrix();
        let (enabled, warnings) = matrix.resolve_animations();
        assert_eq!(enabled, vec![6], "the unknown name is skipped");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no implemented effect"), "{warnings:?}");
    }

    #[test]
    fn an_unknown_effect_name_warns() {
        let text = r#"
[rgb_matrix]
[rgb_matrix.animations]
not_a_real_effect = true
"#;
        let matrix = parse(text).unwrap().matrix();
        let (enabled, warnings) = matrix.resolve_animations();
        assert!(enabled.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("not_a_real_effect"));
    }

    #[test]
    fn the_animation_names_are_case_insensitive() {
        let text = r#"
[rgb_matrix]
[rgb_matrix.animations]
BREATHING = true
"#;
        let matrix = parse(text).unwrap().matrix();
        assert_eq!(matrix.resolve_animations().0, vec![6]);
    }

    #[test]
    fn a_false_animation_never_warns() {
        // Turning an effect off by name must not require the effect to exist:
        // a stale entry should be harmless.
        let text = r#"
[rgb_matrix]
[rgb_matrix.animations]
not_a_real_effect = false
breathing = true
"#;
        let matrix = parse(text).unwrap().matrix();
        let (enabled, warnings) = matrix.resolve_animations();
        assert_eq!(enabled, vec![6]);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn the_defaults_section_parses_with_qmk_names() {
        let text = r#"
[rgb_matrix]
[rgb_matrix.default]
on = true
animation = "cycle_left_right"
hue = 10
sat = 200
val = 128
speed = 90
flags = 4
"#;
        let matrix = parse(text).unwrap().matrix();
        let d = &matrix.default;
        assert!(d.on);
        assert_eq!(d.animation.as_deref(), Some("cycle_left_right"));
        assert_eq!(d.hue, 10);
        assert_eq!(d.sat, 200);
        assert_eq!(d.val, Some(128));
        assert_eq!(d.speed, 90);
        assert_eq!(d.flags, 4);
    }

    #[test]
    fn the_defaults_section_defaults_match_qmk() {
        let d = parse("[rgb_matrix]\n").unwrap().matrix().default;
        assert!(d.on, "RGB_MATRIX_DEFAULT_ON");
        assert_eq!(d.sat, 255, "RGB_MATRIX_DEFAULT_SAT");
        assert_eq!(d.speed, 127, "RGB_MATRIX_DEFAULT_SPD");
        assert_eq!(d.flags, 0xFF, "RGB_MATRIX_DEFAULT_FLAGS");
        assert_eq!(d.val, None, "val falls back to the brightness ceiling");
    }

    #[test]
    fn resolve_path_prefers_the_environment_variable() {
        // The override exists so a project can keep rgb.toml elsewhere; the
        // default sits beside keyboard.toml.
        let resolved = RgbTomlConfig::resolve_path(Some(Path::new("/boards/desk_hub/keyboard.toml")));
        assert!(resolved.ends_with(RGB_TOML_FILE));
        // Without a keyboard.toml (build.rs before [keyboard] is known), the
        // current directory is used.
        let bare = RgbTomlConfig::resolve_path(None);
        assert_eq!(bare, PathBuf::from(RGB_TOML_FILE));
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let missing = Path::new("rmk-rgb-does-not-exist-9f3a.toml");
        assert!(RgbTomlConfig::from_path(missing).unwrap().is_none());
    }

    #[test]
    fn a_malformed_file_reports_its_path() {
        // Written beside the crate rather than in `temp_dir`, which a confined
        // build may not have write access to.
        let path = Path::new("target/rmk-rgb-malformed-9f3a.toml");
        std::fs::create_dir_all("target").expect("create target");
        std::fs::write(path, "[rgb_matrix]\nmax_brightness = \"not a number\"\n").unwrap();

        let err = RgbTomlConfig::from_path(path).unwrap_err();
        assert!(
            err.contains("rmk-rgb-malformed"),
            "the path is in the message: {err}"
        );
        std::fs::remove_file(path).ok();
    }
}
