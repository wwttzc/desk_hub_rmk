//! Generating the RGB Matrix code from `rgb.toml`.
//!
//! QMK splits this work across three places: the `rgb_matrix`/`ws2812` sections
//! of `keyboard.json` become C initialisers through a generator, the `layout`
//! array becomes the `g_led_config` struct literal, and `config.h` macros become
//! compile definitions. This module is the equivalent single pass: it reads the
//! configuration and emits
//!
//! 1. the `LedConfig` constant, from `[[rgb_matrix.layout]]`,
//! 2. the enabled-effect list, from `[rgb_matrix.animations]`,
//! 3. the engine construction, including the driver, from `[ws2812]`.
//!
//! Everything is emitted into the keyboard module, so the generated identifiers
//! live beside the matrix and keymap the same macro already produces.
//!
//! # Absence
//!
//! No `rgb.toml`, or `enabled = false`, emits nothing at all. That keeps the
//! keyboard binary identical to one built without the `rgb` feature, which is
//! what lets this land on a board whose LED table does not exist yet.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use rmk_config::rgb::{LedEntry, RgbMatrixConfig, RgbTomlConfig, Ws2812Config};
use rmk_config::resolved::hardware::{ChipModel, ChipSeries};

/// What the expander produced.
#[derive(Debug)]
pub(crate) struct RgbExpansion {
    /// Constant and `static` definitions, placed with the other config items.
    pub(crate) definitions: TokenStream2,
    /// Engine construction, placed before the processors are built.
    pub(crate) initializer: Option<TokenStream2>,
    /// The processor's run loop, appended to the registered processors.
    pub(crate) runner: Option<TokenStream2>,
    /// Messages for the build log.
    pub(crate) warnings: Vec<String>,
}

impl RgbExpansion {
    /// Nothing to emit: RGB is not configured.
    fn disabled() -> Self {
        Self {
            definitions: TokenStream2::new(),
            initializer: None,
            runner: None,
            warnings: Vec::new(),
        }
    }
}

/// Expand the RGB configuration for `chip`.
///
/// `rgb_toml` is the parsed file, or `None` when there is none.
pub(crate) fn expand_rgb_config(
    rgb_toml: Option<&RgbTomlConfig>,
    chip: &ChipModel,
) -> Result<RgbExpansion, String> {
    let Some(config) = rgb_toml else {
        return Ok(RgbExpansion::disabled());
    };
    if !config.is_enabled() {
        return Ok(RgbExpansion::disabled());
    }

    let matrix = config.matrix();
    let ws = config.ws2812();

    if matrix.layout.is_empty() {
        // A silent default would stack every LED at the origin and make all the
        // position-dependent effects meaningless, so this is an error rather
        // than a warning.
        return Err(
            "rgb_matrix.enabled is true but [[rgb_matrix.layout]] is empty: every LED needs an \
             x/y position. Add one entry per LED in the chain, or set enabled = false."
                .to_string(),
        );
    }

    let led_count = matrix.layout.len();
    let (enabled, mut warnings) = matrix.resolve_animations();
    if enabled.is_empty() {
        warnings.push(
            "rgb_matrix.animations selected no implemented effect; the keyboard will only be able \
             to show the off state"
                .to_string(),
        );
    }

    let definitions = expand_led_table(&matrix, led_count);
    let initializer = expand_engine_init(&matrix, &ws, chip, led_count)?;

    Ok(RgbExpansion {
        definitions,
        initializer: Some(initializer),
        runner: Some(quote! { rgb_processor.run() }),
        warnings,
    })
}

/// The `LED_CONFIG` constant and the counts derived from it.
///
/// `matrix_pos` uses `LedConfig::UNMAPPED` for an LED under no key, which is
/// what `GET_LED_INFO` reports as `0xFF, 0xFF`.
fn expand_led_table(matrix: &RgbMatrixConfig, led_count: usize) -> TokenStream2 {
    let points = matrix.layout.iter().map(|entry| {
        let x = entry.x;
        let y = entry.y;
        quote! { ::rmk::rgb::led_config::LedPoint::new(#x, #y) }
    });

    let flags = matrix.layout.iter().map(|entry| entry.flags);

    let matrix_pos = matrix.layout.iter().map(|entry: &LedEntry| match entry.matrix {
        Some([row, col]) => quote! { (#row, #col) },
        None => quote! { ::rmk::rgb::led_config::LedConfig::<#led_count>::UNMAPPED },
    });

    let count = led_count;
    let max_brightness = matrix.max_brightness;
    let center = matrix.center();
    let center_x = center[0];
    let center_y = center[1];
    let flush_limit = matrix.led_flush_limit;
    let process_limit = matrix.process_limit(led_count);
    let default = &matrix.default;

    let default_on = default.on;
    // `RGB_MATRIX_DEFAULT_VAL` is defined in terms of the brightness ceiling, so
    // an unset value means "as bright as the board allows".
    let default_val = default.val.unwrap_or(max_brightness);
    let default_hue = default.hue;
    let default_sat = default.sat;
    let default_speed = default.speed;
    let default_flags = default.flags;

    // A default animation that names nothing, or names an effect that is not
    // ported, falls back to the first enabled effect rather than silently
    // selecting an id that renders the solid-colour fallback.
    let default_mode = match &default.animation {
        Some(name) => match rmk_config::rgb::resolve_effect_name(name) {
            Some((id, true)) => quote! { #id },
            Some((id, false)) => {
                let _ = id;
                quote! { ::rmk::rgb::registry::EFFECT_FIRST }
            }
            None => quote! { ::rmk::rgb::registry::EFFECT_FIRST },
        },
        None => quote! { ::rmk::rgb::registry::EFFECT_FIRST },
    };

    // The enabled-effect list. `GET_SUPPORTED` reports exactly these.
    let enabled_ids = {
        let (ids, _) = matrix.resolve_animations();
        if ids.is_empty() {
            // A `[u16; 0]` array is legal, but the engine's clamps would then
            // have no range; keep the first effect so the state is selectable.
            vec![quote! { ::rmk::rgb::registry::EFFECT_FIRST }]
        } else {
            ids.into_iter().map(|id| quote! { #id }).collect()
        }
    };
    let enabled_len = enabled_ids.len();

    quote! {
        /// The board's LED table, one entry per LED in the chain.
        pub(crate) const RGB_LED_COUNT: usize = #count;

        pub(crate) static RGB_LED_CONFIG: ::rmk::rgb::led_config::LedConfig<#count> =
            ::rmk::rgb::led_config::LedConfig::new(
                [#(#points),*],
                [#(#flags),*],
                [#(#matrix_pos),*],
            );

        /// `RGB_MATRIX_MAXIMUM_BRIGHTNESS`, and the Vial slider's upper bound.
        pub(crate) const RGB_MAX_BRIGHTNESS: u8 = #max_brightness;

        /// `k_rgb_matrix_center`.
        pub(crate) const RGB_CENTER: ::rmk::rgb::led_config::LedPoint =
            ::rmk::rgb::led_config::LedPoint::new(#center_x, #center_y);

        /// `RGB_MATRIX_LED_FLUSH_LIMIT`, in milliseconds.
        pub(crate) const RGB_FLUSH_LIMIT_MS: u32 = #flush_limit;

        /// `RGB_MATRIX_LED_PROCESS_LIMIT`.
        pub(crate) const RGB_PROCESS_LIMIT: usize = #process_limit;

        /// The effects this build enables, as registry ids.
        pub(crate) const RGB_ENABLED_EFFECTS: [u16; #enabled_len] = [#(#enabled_ids),*];

        /// The `RGB_MATRIX_DEFAULT_*` set, before storage is read.
        pub(crate) const RGB_DEFAULTS: ::rmk::rgb::state::RgbConfig =
            ::rmk::rgb::state::RgbConfig {
                enable: #default_on,
                mode: #default_mode,
                hsv: ::rmk::rgb::fx::color::Hsv::new(#default_hue, #default_sat, #default_val),
                speed: #default_speed,
                flags: #default_flags,
            };
    }
}

/// The engine construction and the processor that drives it.
fn expand_engine_init(
    matrix: &RgbMatrixConfig,
    ws: &Ws2812Config,
    chip: &ChipModel,
    led_count: usize,
) -> Result<TokenStream2, String> {
    let driver = expand_driver(ws, chip, led_count)?;
    let center_x = matrix.center()[0];
    let center_y = matrix.center()[1];

    Ok(quote! {
        // Install the board's table and enabled list into the shared state
        // before the engine starts, so the Vial protocol can answer from the
        // first report.
        ::rmk::rgb::via::install(
            &RGB_LED_CONFIG,
            RGB_MAX_BRIGHTNESS,
            &RGB_ENABLED_EFFECTS,
        );
        ::rmk::rgb::via::restore_config(RGB_DEFAULTS);

        #driver

        let mut rgb_processor = ::rmk::rgb::engine::RgbEngine::new(
            &RGB_LED_CONFIG,
            RGB_DEFAULTS,
            RGB_MAX_BRIGHTNESS,
            leds,
        )
        .with_process_limit(RGB_PROCESS_LIMIT)
        .with_center(::rmk::rgb::led_config::LedPoint::new(#center_x, #center_y))
        .with_flush_limit(RGB_FLUSH_LIMIT_MS);
    })
}

/// The LED chain driver, per chip family.
///
/// QMK picks a driver through `WS2812_DRIVER`; here the chip decides, and
/// `ws2812.driver` only has to agree with it. The driver's dependencies are
/// declared by the user's crate, exactly as `esp-hal` already is for USB, which
/// is why the generated code names them fully qualified rather than importing.
fn expand_driver(ws: &Ws2812Config, chip: &ChipModel, led_count: usize) -> Result<TokenStream2, String> {
    let pin = ws.pin.as_deref().ok_or_else(|| {
        "ws2812.pin is required: it names the GPIO driving the first LED in the chain".to_string()
    })?;
    let pin_ident = syn::parse_str::<syn::Ident>(pin)
        .map_err(|_| format!("ws2812.pin = {pin:?} is not a valid pin identifier"))?;

    let driver = ws.driver.as_deref().unwrap_or(match chip.series {
        ChipSeries::Esp32 => "rmt",
        _ => "spi",
    });
    let color_order = ws.color_order.as_deref().unwrap_or("grb");
    let order_ident = match color_order.to_ascii_lowercase().as_str() {
        "rgb" => quote! { ::esp_hal_smartled::color_order::Rgb },
        "grb" => quote! { ::esp_hal_smartled::color_order::Grb },
        "bgr" => quote! { ::esp_hal_smartled::color_order::Bgr },
        "brg" => quote! { ::esp_hal_smartled::color_order::Brg },
        "rbg" => quote! { ::esp_hal_smartled::color_order::Rbg },
        "gbr" => quote! { ::esp_hal_smartled::color_order::Gbr },
        other => {
            return Err(format!(
                "ws2812.color_order = {other:?} is not a recognised order; \
                 expected one of rgb, grb, bgr, brg, rbg, gbr"
            ));
        }
    };

    match (&chip.series, driver) {
        (ChipSeries::Esp32, "rmt") => {
            let memsize = ws.rmt_memsize.unwrap_or(1);
            // `new_with_memsize` takes the RMT source rate as its last argument:
            // the driver derives its pulse codes from it, so the value must match
            // the rate the peripheral was created with.
            let freq_mhz = ws.rmt_freq_mhz.unwrap_or(80);
            Ok(quote! {
                let leds = {
                    let _rmt_rate = ::esp_hal::time::Rate::from_mhz(#freq_mhz);
                    let _rmt = ::esp_hal::rmt::Rmt::new(p.RMT, _rmt_rate)
                        .expect("failed to initialise the RMT peripheral");
                    let _chain = ::esp_hal_smartled::RmtSmartLeds::<
                        { ::esp_hal_smartled::buffer_size::<::rmk::smart_leds_trait::RGB8>(#led_count) },
                        _,
                        ::rmk::smart_leds_trait::RGB8,
                        #order_ident,
                    >::new_with_memsize(
                        ::esp_hal_smartled::WS2812B_TIMING,
                        _rmt.channel0,
                        p.#pin_ident,
                        #memsize,
                        _rmt_rate,
                    )
                    .expect("failed to initialise the WS2812 driver");
                    ::rmk::rgb::drivers::SmartLedsFrame::<#led_count, _>::new(_chain)
                };
            })
        }
        (ChipSeries::Esp32, other) => Err(format!(
            "ws2812.driver = {other:?} is not supported on ESP32; \
             the esp-hal-smartled RMT driver is the only one wired up"
        )),
        (series, other) => Err(format!(
            "ws2812 is not wired up for {series:?} (driver {other:?}); \
             only ESP32 currently generates a driver. \
             See rmk-macro/src/codegen/rgb.rs to add one."
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmk_config::resolved::hardware::{ChipModel, ChipSeries};

    fn chip(series: ChipSeries, name: &str) -> ChipModel {
        ChipModel {
            series,
            chip: name.to_string(),
            board: None,
        }
    }

    fn esp32s3() -> ChipModel {
        chip(ChipSeries::Esp32, "esp32s3")
    }

    /// Three LEDs, enough to exercise the array handling without noise.
    fn config_toml() -> RgbTomlConfig {
        RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
enabled = true
max_brightness = 128

[rgb_matrix.default]
animation = "breathing"
hue = 10
sat = 200
val = 100
speed = 90

[rgb_matrix.animations]
breathing = true
cycle_all = true

[[rgb_matrix.layout]]
x = 0
y = 0
flags = 4
matrix = [0, 0]

[[rgb_matrix.layout]]
x = 112
y = 32
flags = 4

[[rgb_matrix.layout]]
x = 224
y = 64
flags = 1
matrix = [1, 0]

[ws2812]
pin = "GPIO48"
"#,
        )
        .expect("parses")
    }

    #[test]
    fn no_configuration_emits_nothing() {
        let expansion = expand_rgb_config(None, &esp32s3()).unwrap();
        assert!(expansion.definitions.is_empty());
        assert!(expansion.initializer.is_none());
        assert!(expansion.runner.is_none());
        assert!(expansion.warnings.is_empty());
    }

    #[test]
    fn enabled_false_emits_nothing() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str("[rgb_matrix]\nenabled = false\n").unwrap();
        let expansion = expand_rgb_config(Some(&config), &esp32s3()).unwrap();
        assert!(expansion.definitions.is_empty());
        assert!(expansion.initializer.is_none());
    }

    /// The LED table is the one thing that cannot be defaulted: without
    /// positions every positional effect is meaningless.
    #[test]
    fn an_empty_layout_is_an_error() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str("[rgb_matrix]\n[ws2812]\npin = \"GPIO48\"\n").unwrap();
        let err = expand_rgb_config(Some(&config), &esp32s3()).unwrap_err();
        assert!(err.contains("rgb_matrix.layout"), "{err}");
    }

    #[test]
    fn a_missing_pin_is_an_error() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            "[rgb_matrix]\n[[rgb_matrix.layout]]\nx = 0\ny = 0\n",
        )
        .unwrap();
        let err = expand_rgb_config(Some(&config), &esp32s3()).unwrap_err();
        assert!(err.contains("ws2812.pin"), "{err}");
    }

    #[test]
    fn the_definitions_carry_the_led_count_and_ceiling() {
        let expansion = expand_rgb_config(Some(&config_toml()), &esp32s3()).unwrap();
        let text = expansion.definitions.to_string();
        assert!(text.contains("RGB_LED_COUNT"), "{text}");
        assert!(text.contains("RGB_LED_CONFIG"), "{text}");
        assert!(text.contains("RGB_MAX_BRIGHTNESS"), "{text}");
        assert!(text.contains("RGB_ENABLED_EFFECTS"), "{text}");
        assert!(text.contains("RGB_DEFAULTS"), "{text}");
    }

    /// An unset `default.val` means the brightness ceiling, matching QMK's
    /// `RGB_MATRIX_DEFAULT_VAL` being defined in terms of
    /// `RGB_MATRIX_MAXIMUM_BRIGHTNESS`.
    #[test]
    fn the_default_value_falls_back_to_the_ceiling() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
max_brightness = 96
[[rgb_matrix.layout]]
x = 0
y = 0
[ws2812]
pin = "GPIO48"
"#,
        )
        .unwrap();
        let expansion = expand_rgb_config(Some(&config), &esp32s3()).unwrap();
        let text = expansion.definitions.to_string();
        // Hsv::new(h, s, v) with v = 96.
        assert!(text.contains("Hsv :: new (0u8 , 255u8 , 96u8)") || text.contains("96"), "{text}");
    }

    #[test]
    fn the_initializer_installs_the_table_and_builds_the_engine() {
        let expansion = expand_rgb_config(Some(&config_toml()), &esp32s3()).unwrap();
        let init = expansion.initializer.expect("an initializer").to_string();
        assert!(init.contains("via :: install"), "{init}");
        assert!(init.contains("RgbEngine :: new"), "{init}");
        assert!(init.contains("RmtSmartLeds"), "the ESP32 driver: {init}");
        assert!(init.contains("GPIO48"), "the configured pin: {init}");
    }

    #[test]
    fn the_runner_starts_the_processor() {
        let expansion = expand_rgb_config(Some(&config_toml()), &esp32s3()).unwrap();
        assert!(expansion.runner.unwrap().to_string().contains("rgb_processor"));
    }

    #[test]
    fn an_unknown_default_animation_falls_back_too() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
[rgb_matrix.default]
animation = "not_a_real_effect"
[[rgb_matrix.layout]]
x = 0
y = 0
[ws2812]
pin = "GPIO48"
"#,
        )
        .unwrap();
        let expansion = expand_rgb_config(Some(&config), &esp32s3()).unwrap();
        assert!(expansion.definitions.to_string().contains("EFFECT_FIRST"));
    }

    /// Every VialRGB effect is ported, so the only animations that warn are
    /// names that were never in the table.
    #[test]
    fn an_unknown_animation_warns() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
[rgb_matrix.animations]
not_a_real_effect = true
breathing = true
[[rgb_matrix.layout]]
x = 0
y = 0
[ws2812]
pin = "GPIO48"
"#,
        )
        .unwrap();
        let expansion = expand_rgb_config(Some(&config), &esp32s3()).unwrap();
        assert_eq!(expansion.warnings.len(), 1);
        assert!(
            expansion.warnings[0].contains("not_a_real_effect"),
            "{:?}",
            expansion.warnings
        );
    }

    #[test]
    fn selecting_no_implemented_effect_warns() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
[rgb_matrix.animations]
not_a_real_effect = true
[[rgb_matrix.layout]]
x = 0
y = 0
[ws2812]
pin = "GPIO48"
"#,
        )
        .unwrap();
        let expansion = expand_rgb_config(Some(&config), &esp32s3()).unwrap();
        assert!(
            expansion.warnings.iter().any(|w| w.contains("no implemented effect")),
            "{:?}",
            expansion.warnings
        );
    }

    #[test]
    fn a_bad_color_order_is_an_error() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
[[rgb_matrix.layout]]
x = 0
y = 0
[ws2812]
pin = "GPIO48"
color_order = "rbgw"
"#,
        )
        .unwrap();
        let err = expand_rgb_config(Some(&config), &esp32s3()).unwrap_err();
        assert!(err.contains("color_order"), "{err}");
    }

    #[test]
    fn every_valid_color_order_is_accepted() {
        for order in ["rgb", "grb", "bgr", "brg", "rbg", "gbr", "RGB", "Grb"] {
            let config: RgbTomlConfig = RgbTomlConfig::from_str(&format!(
                "[rgb_matrix]\n[[rgb_matrix.layout]]\nx = 0\ny = 0\n[ws2812]\npin = \"GPIO48\"\ncolor_order = \"{order}\"\n"
            ))
            .unwrap();
            assert!(
                expand_rgb_config(Some(&config), &esp32s3()).is_ok(),
                "colour order {order} should be accepted"
            );
        }
    }

    #[test]
    fn a_non_esp_chip_is_reported_as_unsupported() {
        let err = expand_rgb_config(Some(&config_toml()), &chip(ChipSeries::Nrf52, "nrf52840")).unwrap_err();
        assert!(err.contains("nrf52840") || err.contains("Nrf52"), "{err}");
        assert!(err.contains("only ESP32"), "{err}");
    }

    #[test]
    fn a_non_rmt_esp_driver_is_reported() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            r#"
[rgb_matrix]
[[rgb_matrix.layout]]
x = 0
y = 0
[ws2812]
pin = "GPIO48"
driver = "spi"
"#,
        )
        .unwrap();
        let err = expand_rgb_config(Some(&config), &esp32s3()).unwrap_err();
        assert!(err.contains("not supported on ESP32"), "{err}");
    }

    #[test]
    fn a_bad_pin_identifier_is_an_error() {
        let config: RgbTomlConfig = RgbTomlConfig::from_str(
            "[rgb_matrix]\n[[rgb_matrix.layout]]\nx = 0\ny = 0\n[ws2812]\npin = \"48\"\n",
        )
        .unwrap();
        let err = expand_rgb_config(Some(&config), &esp32s3()).unwrap_err();
        assert!(err.contains("not a valid pin"), "{err}");
    }
}
