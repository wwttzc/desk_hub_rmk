//! The effect registry: which effects exist, what id each has, and how a batch
//! is dispatched to one.
//!
//! # One numbering, and it is VialRGB's
//!
//! QMK has two effect numberings, and they do not agree: `enum
//! rgb_matrix_effects`, whose order is `rgb_matrix_effects.inc` and which
//! `rgb_config_t::mode` stores, and VialRGB's own list, whose ids travel over
//! the wire. Mapping between them is not arithmetic.
//!
//! This firmware carries only the effects both sides have, so it keeps only one
//! table: [`EFFECTS`] is VialRGB's list verbatim. A stored id and a wire id are
//! the same number, and there is no cross table to transpose — which is what
//! made "Vial picks effect A, the keyboard lights effect B" impossible.
//!
//! - `2..=44` are exactly VialRGB's ids, in VialRGB's order.
//! - `45` is one past the last of them, QMK's `RGB_MATRIX_EFFECT_MAX` role.
//!
//! Effects that only QMK has — `FLOWER_BLOOMING`, `PIXEL_FLOW`, the four
//! `STARLIGHT`s and `RIVERFLOW` — are deliberately absent. Vial cannot select
//! them, so implementing them would add code no host could reach.
//!
//! `OFF` (0) and `DIRECT` (1) stay reserved and are not registry entries: the
//! first means "disabled", the second is the host driving per-LED colours
//! directly rather than an animation.

use crate::rgb::effects::impls;
use crate::rgb::fx::random8::Rng;
use crate::rgb::hit_tracker::HitTracker;
use crate::rgb::led_config::LED_FLAG_ALL;
use crate::rgb::runner::{EffectParams, FrameAll, FrameChain, LedFrame, RenderContext};

/// VialRGB's `VIALRGB_EFFECT_OFF`: the LEDs are off, not animating.
pub const EFFECT_OFF: u16 = 0;
/// VialRGB's `VIALRGB_EFFECT_DIRECT`: the host sets each LED itself.
pub const EFFECT_DIRECT: u16 = 1;
/// The first effect id.
pub const EFFECT_FIRST: u16 = 2;
/// The last id VialRGB knows. Ids above it exist only in this firmware.
pub const VIALRGB_LAST: u16 = 44;
/// One past the last effect id, QMK's `RGB_MATRIX_EFFECT_MAX` role.
pub const EFFECT_MAX: u16 = 45;

/// One registry row.
#[derive(Clone, Copy, Debug)]
pub struct EffectEntry {
    /// The effect id: a VialRGB id for `2..=44`, an extension above that.
    ///
    /// This is what `rgb_config_t`-equivalent state stores and what the wire
    /// carries, so no translation happens on either path.
    pub id: u16,
    /// QMK's enum name, e.g. `"SOLID_COLOR"`.
    pub name: &'static str,
    /// Lowercase key for the `animations` config section, as QMK spells it.
    pub key: &'static str,
    /// True once the effect has a Rust implementation in
    /// [`crate::rgb::effects::impls`].
    pub implemented: bool,
}

/// `TYPING_HEATMAP`'s id.
///
/// The table below is still the single source of truth for ids; this just gives
/// the one effect that is selected *by id* outside the dispatch path — the
/// keyboard task warms its heat map only while it is active — a readable name.
pub const TYPING_HEATMAP: u16 = 29;

/// The effect table, ordered by id.
///
/// The `2..=44` block is `vialrgb_effects.inc` verbatim, so it can be diffed
/// against upstream by eye. The block above it holds QMK effects VialRGB cannot
/// select.
pub const EFFECTS: [EffectEntry; 43] = [
    // ---- VialRGB ids, in `vialrgb_effects.inc` order ----
    entry(2, "SOLID_COLOR", true),
    entry(3, "ALPHAS_MODS", true),
    entry(4, "GRADIENT_UP_DOWN", true),
    entry(5, "GRADIENT_LEFT_RIGHT", true),
    entry(6, "BREATHING", true),
    entry(7, "BAND_SAT", true),
    entry(8, "BAND_VAL", true),
    entry(9, "BAND_PINWHEEL_SAT", true),
    entry(10, "BAND_PINWHEEL_VAL", true),
    entry(11, "BAND_SPIRAL_SAT", true),
    entry(12, "BAND_SPIRAL_VAL", true),
    entry(13, "CYCLE_ALL", true),
    entry(14, "CYCLE_LEFT_RIGHT", true),
    entry(15, "CYCLE_UP_DOWN", true),
    entry(16, "RAINBOW_MOVING_CHEVRON", true),
    entry(17, "CYCLE_OUT_IN", true),
    entry(18, "CYCLE_OUT_IN_DUAL", true),
    entry(19, "CYCLE_PINWHEEL", true),
    entry(20, "CYCLE_SPIRAL", true),
    entry(21, "DUAL_BEACON", true),
    entry(22, "RAINBOW_BEACON", true),
    entry(23, "RAINBOW_PINWHEELS", true),
    entry(24, "RAINDROPS", true),
    entry(25, "JELLYBEAN_RAINDROPS", true),
    entry(26, "HUE_BREATHING", true),
    entry(27, "HUE_PENDULUM", true),
    entry(28, "HUE_WAVE", true),
    entry(29, "TYPING_HEATMAP", true),
    entry(30, "DIGITAL_RAIN", true),
    entry(31, "SOLID_REACTIVE_SIMPLE", true),
    entry(32, "SOLID_REACTIVE", true),
    entry(33, "SOLID_REACTIVE_WIDE", true),
    entry(34, "SOLID_REACTIVE_MULTIWIDE", true),
    entry(35, "SOLID_REACTIVE_CROSS", true),
    entry(36, "SOLID_REACTIVE_MULTICROSS", true),
    entry(37, "SOLID_REACTIVE_NEXUS", true),
    entry(38, "SOLID_REACTIVE_MULTINEXUS", true),
    entry(39, "SPLASH", true),
    entry(40, "MULTISPLASH", true),
    entry(41, "SOLID_SPLASH", true),
    entry(42, "SOLID_MULTISPLASH", true),
    entry(43, "PIXEL_RAIN", true),
    entry(44, "PIXEL_FRACTAL", true),
];

/// A table row, with the config key derived from the id.
const fn entry(id: u16, name: &'static str, implemented: bool) -> EffectEntry {
    EffectEntry {
        id,
        name,
        key: key_for(id),
        implemented,
    }
}

/// The `animations` key for an effect id.
///
/// An explicit match rather than a runtime `to_lowercase()` so the table stays
/// `const` and a typo becomes a compile error. The spellings follow QMK's
/// `rgb_matrix.animations` keys, which mostly lowercase the enum name but not
/// always: `ALPHAS_MODS` is `alpha_mods`, and the `BAND_*` ids are `band_*`
/// although their files are named `colorband_*`.
const fn key_for(id: u16) -> &'static str {
    match id {
        2 => "solid_color",
        3 => "alpha_mods",
        4 => "gradient_up_down",
        5 => "gradient_left_right",
        6 => "breathing",
        7 => "band_sat",
        8 => "band_val",
        9 => "band_pinwheel_sat",
        10 => "band_pinwheel_val",
        11 => "band_spiral_sat",
        12 => "band_spiral_val",
        13 => "cycle_all",
        14 => "cycle_left_right",
        15 => "cycle_up_down",
        16 => "rainbow_moving_chevron",
        17 => "cycle_out_in",
        18 => "cycle_out_in_dual",
        19 => "cycle_pinwheel",
        20 => "cycle_spiral",
        21 => "dual_beacon",
        22 => "rainbow_beacon",
        23 => "rainbow_pinwheels",
        24 => "raindrops",
        25 => "jellybean_raindrops",
        26 => "hue_breathing",
        27 => "hue_pendulum",
        28 => "hue_wave",
        29 => "typing_heatmap",
        30 => "digital_rain",
        31 => "solid_reactive_simple",
        32 => "solid_reactive",
        33 => "solid_reactive_wide",
        34 => "solid_reactive_multiwide",
        35 => "solid_reactive_cross",
        36 => "solid_reactive_multicross",
        37 => "solid_reactive_nexus",
        38 => "solid_reactive_multinexus",
        39 => "splash",
        40 => "multisplash",
        41 => "solid_splash",
        42 => "solid_multisplash",
        43 => "pixel_rain",
        44 => "pixel_fractal",
        _ => "unknown",
    }
}

impl EffectEntry {
    /// The entry for an effect id.
    pub fn by_id(id: u16) -> Option<&'static EffectEntry> {
        if id < EFFECT_FIRST {
            return None;
        }
        EFFECTS.iter().find(|e| e.id == id)
    }

    /// The entry whose `animations` key matches, case-insensitively.
    pub fn by_key(key: &str) -> Option<&'static EffectEntry> {
        EFFECTS.iter().find(|e| e.key.eq_ignore_ascii_case(key))
    }
}

/// The effect ids every enabled effect should report, ascending and above `gt`.
///
/// Mirrors `get_supported()` in `vialrgb.c`. Every id in [`EFFECTS`] is a
/// VialRGB id by construction, so nothing has to be withheld here.
///
/// Returns the number written into `out`.
pub fn supported_ids(gt: u16, enabled: &[u16], out: &mut [u16]) -> usize {
    let mut count = 0;
    for entry in EFFECTS.iter() {
        if entry.id <= gt || !is_enabled(entry.id, enabled) {
            continue;
        }
        if count == out.len() {
            break;
        }
        out[count] = entry.id;
        count += 1;
    }
    count
}

/// Whether `id` is enabled in this build.
///
/// QMK gates each effect on its own `ENABLE_RGB_MATRIX_*` macro. We gate on an
/// explicit id list; an empty list means "everything ported", which is what an
/// unconfigured build gets before the `rgb.toml` layer lands.
pub fn is_enabled(id: u16, enabled: &[u16]) -> bool {
    if enabled.is_empty() {
        return EffectEntry::by_id(id).is_some_and(|e| e.implemented);
    }
    enabled.contains(&id)
}

/// Dispatch one batch of `id` into `frame`.
///
/// The per-effect state QMK keeps in `static` locals is threaded through
/// explicitly: `rng` for the random effects, `hits` for the reactive ones, and
/// `raindrops_index` for `RAINDROPS`'s target LED. Effects that need none ignore
/// them, which keeps one signature instead of a state object per effect.
///
/// `EFFECT_OFF` clears the frame — `rgb_matrix_none`'s job. An id with no
/// implementation falls back to [`impls::solid_color`] so the keyboard stays lit
/// and the state a host reads back stays coherent; the build-time check is what
/// reports the gap.
#[allow(clippy::too_many_arguments)]
pub fn dispatch<const LED_COUNT: usize, F: LedFrame + FrameAll + FrameChain>(
    id: u16,
    ctx: &RenderContext<'_, LED_COUNT>,
    params: &EffectParams,
    hits: &HitTracker,
    rng: &mut Rng,
    raindrops_index: &mut u16,
    pixel_rain_index: &mut u8,
    pixel_rain_wait: &mut u32,
    framebuffer: &mut impls::FramebufferEffects,
    frame: &mut F,
) -> bool {
    use crate::rgb::fx::math8::qsub8;
    use crate::rgb::runner as r;
    use impls::*;

    // Shorthands so each arm reads like its QMK original.
    let x = |i: u8| ctx.leds.point(i as usize).x;
    let y = |i: u8| ctx.leds.point(i as usize).y;

    match id {
        // `RGB_MATRIX_NONE`, which `rgb_task_start` selects when disabled.
        0 => r::rgb_matrix_none(params, frame),

        2 => solid_color(ctx, params, frame),
        3 => alphas_mods(ctx, params, frame),
        4 => gradient_up_down(ctx, params, frame),
        5 => gradient_left_right(ctx, params, frame),

        6 => r::effect_runner_i(ctx, params, frame, breathing_math),
        7 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| band_sat_math(hsv, x(i), t)),
        8 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| band_val_math(hsv, x(i), t)),
        9 => r::effect_runner_dx_dy(ctx, params, frame, band_pinwheel_sat_math),
        10 => r::effect_runner_dx_dy(ctx, params, frame, band_pinwheel_val_math),
        11 => r::effect_runner_dx_dy_dist(ctx, params, frame, band_spiral_sat_math),
        12 => r::effect_runner_dx_dy_dist(ctx, params, frame, band_spiral_val_math),

        13 => r::effect_runner_i(ctx, params, frame, cycle_all_math),
        14 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| cycle_left_right_math(hsv, x(i), t)),
        15 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| cycle_up_down_math(hsv, y(i), t)),

        17 => r::effect_runner_dx_dy_dist(ctx, params, frame, |hsv, _dx, _dy, dist, t| {
            cycle_out_in_math(hsv, dist, t)
        }),
        19 => r::effect_runner_dx_dy(ctx, params, frame, cycle_pinwheel_math),
        20 => r::effect_runner_dx_dy_dist(ctx, params, frame, cycle_spiral_math),

        16 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| {
            let p = ctx.leds.point(i as usize);
            rainbow_moving_chevron_math(hsv, p.x, p.y, t, ctx.center.y)
        }),

        18 => r::effect_runner_dx_dy(ctx, params, frame, |hsv, dx, dy, t| {
            cycle_out_in_dual_math(hsv, dx, dy, t, ctx.center.x)
        }),

        21 => r::effect_runner_sin_cos_i(ctx, params, frame, |hsv, cos, sin, i, _t| {
            let p = ctx.leds.point(i as usize);
            dual_beacon_math(hsv, cos, sin, p.x, p.y, ctx.center.x, ctx.center.y)
        }),

        22 => r::effect_runner_sin_cos_i(ctx, params, frame, |hsv, cos, sin, i, _t| {
            let p = ctx.leds.point(i as usize);
            rainbow_beacon_math(hsv, cos, sin, p.x, p.y, ctx.center.x, ctx.center.y)
        }),

        23 => r::effect_runner_sin_cos_i(ctx, params, frame, |hsv, cos, sin, i, _t| {
            let p = ctx.leds.point(i as usize);
            rainbow_pinwheels_math(hsv, cos, sin, p.x, p.y, ctx.center.x, ctx.center.y)
        }),

        24 => raindrops(ctx, params, rng, raindrops_index, frame),

        25 => jellybean_raindrops(ctx, params, rng, raindrops_index, frame),

        26 => r::effect_runner_i(ctx, params, frame, hue_breathing_math),
        27 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| hue_pendulum_math(hsv, x(i), t)),
        28 => r::effect_runner_i(ctx, params, frame, |hsv, i, t| hue_wave_math(hsv, x(i), t)),

        // The two framebuffer effects walk the electrical matrix rather than the
        // LED chain, so they read and write `frame_buf` instead of batching over
        // LEDs. `TYPING_HEATMAP` still batches, cell by cell.
        29 => typing_heatmap(ctx, params, &mut framebuffer.heat, &mut framebuffer.heatmap, frame),
        30 => digital_rain(
            ctx,
            params,
            &mut framebuffer.heat,
            rng,
            &mut framebuffer.digital_rain_drop,
            &mut framebuffer.digital_rain_decay,
            frame,
        ),

        31 => r::effect_runner_reactive(ctx, params, hits, frame, solid_reactive_simple_math),
        32 => r::effect_runner_reactive(ctx, params, hits, frame, solid_reactive_math),

        // The splash family shares its runner and differs only in where the
        // accumulation starts: `start = count - 1` keeps a single splash from
        // the newest hit, while `0` sums every remembered hit.
        39 => {
            let start = qsub8(hits.count(), 1);
            r::effect_runner_reactive_splash(start, ctx, params, hits, frame, splash_math)
        }
        40 => r::effect_runner_reactive_splash(0, ctx, params, hits, frame, splash_math),
        41 => {
            let start = qsub8(hits.count(), 1);
            r::effect_runner_reactive_splash(start, ctx, params, hits, frame, solid_splash_math)
        }
        42 => r::effect_runner_reactive_splash(0, ctx, params, hits, frame, solid_splash_math),

        // The same runner again, with the pulse shaped by the LED's offset from
        // each hit rather than by the bare distance.
        33 => {
            let start = qsub8(hits.count(), 1);
            r::effect_runner_reactive_splash(start, ctx, params, hits, frame, solid_reactive_wide_math)
        }
        34 => r::effect_runner_reactive_splash(0, ctx, params, hits, frame, solid_reactive_wide_math),
        35 => {
            let start = qsub8(hits.count(), 1);
            r::effect_runner_reactive_splash(start, ctx, params, hits, frame, solid_reactive_cross_math)
        }
        36 => r::effect_runner_reactive_splash(0, ctx, params, hits, frame, solid_reactive_cross_math),
        37 => {
            let start = qsub8(hits.count(), 1);
            r::effect_runner_reactive_splash(start, ctx, params, hits, frame, solid_reactive_nexus_math)
        }
        38 => r::effect_runner_reactive_splash(0, ctx, params, hits, frame, solid_reactive_nexus_math),

        43 => pixel_rain(ctx, params, rng, pixel_rain_index, pixel_rain_wait, frame),

        44 => pixel_fractal(
            ctx,
            params,
            &mut framebuffer.fractal,
            rng,
            &mut framebuffer.fractal_wait,
            frame,
        ),

        // In the table but not ported yet. Keep the frame coherent; the
        // build-time check reports it.
        _ => solid_color(ctx, params, frame),
    }
}

/// The default flag filter: `LED_FLAG_ALL`.
pub const ALL_FLAGS: u8 = LED_FLAG_ALL;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn table_is_dense_and_ordered() {
        for (i, entry) in EFFECTS.iter().enumerate() {
            assert_eq!(
                entry.id,
                EFFECT_FIRST + i as u16,
                "{} is out of place at index {i}",
                entry.name
            );
        }
        assert_eq!(EFFECTS.last().unwrap().id, EFFECT_MAX - 1);
    }

    /// The `2..=44` block is an external contract: those ids travel over the wire
    /// and a stored id is the same number. Pinned by name at the boundaries and
    /// at every point where VialRGB's order is non-obvious.
    #[test]
    fn vialrgb_block_matches_the_upstream_list() {
        // Every VialRGB id and the name `vialrgb_effects.inc` gives it.
        let expected: [(u16, &str); 43] = [
            (2, "SOLID_COLOR"),
            (3, "ALPHAS_MODS"),
            (4, "GRADIENT_UP_DOWN"),
            (5, "GRADIENT_LEFT_RIGHT"),
            (6, "BREATHING"),
            (7, "BAND_SAT"),
            (8, "BAND_VAL"),
            (9, "BAND_PINWHEEL_SAT"),
            (10, "BAND_PINWHEEL_VAL"),
            (11, "BAND_SPIRAL_SAT"),
            (12, "BAND_SPIRAL_VAL"),
            (13, "CYCLE_ALL"),
            (14, "CYCLE_LEFT_RIGHT"),
            (15, "CYCLE_UP_DOWN"),
            (16, "RAINBOW_MOVING_CHEVRON"),
            (17, "CYCLE_OUT_IN"),
            (18, "CYCLE_OUT_IN_DUAL"),
            (19, "CYCLE_PINWHEEL"),
            (20, "CYCLE_SPIRAL"),
            (21, "DUAL_BEACON"),
            (22, "RAINBOW_BEACON"),
            (23, "RAINBOW_PINWHEELS"),
            (24, "RAINDROPS"),
            (25, "JELLYBEAN_RAINDROPS"),
            (26, "HUE_BREATHING"),
            (27, "HUE_PENDULUM"),
            (28, "HUE_WAVE"),
            (29, "TYPING_HEATMAP"),
            (30, "DIGITAL_RAIN"),
            (31, "SOLID_REACTIVE_SIMPLE"),
            (32, "SOLID_REACTIVE"),
            (33, "SOLID_REACTIVE_WIDE"),
            (34, "SOLID_REACTIVE_MULTIWIDE"),
            (35, "SOLID_REACTIVE_CROSS"),
            (36, "SOLID_REACTIVE_MULTICROSS"),
            (37, "SOLID_REACTIVE_NEXUS"),
            (38, "SOLID_REACTIVE_MULTINEXUS"),
            (39, "SPLASH"),
            (40, "MULTISPLASH"),
            (41, "SOLID_SPLASH"),
            (42, "SOLID_MULTISPLASH"),
            (43, "PIXEL_RAIN"),
            (44, "PIXEL_FRACTAL"),
        ];
        for (id, name) in expected {
            let entry = EffectEntry::by_id(id).unwrap_or_else(|| panic!("no entry for id {id}"));
            assert_eq!(entry.name, name, "VialRGB id {id}");
        }
        assert_eq!(expected.len(), (VIALRGB_LAST - EFFECT_FIRST + 1) as usize);
    }

    /// Every entry is a VialRGB id, so the table has no id a host cannot ask
    /// for and no gap it would trip over while paging.
    #[test]
    fn every_entry_is_a_vialrgb_id_within_the_wire_range() {
        assert!(
            EFFECTS.iter().all(|e| e.id >= EFFECT_FIRST && e.id <= VIALRGB_LAST),
            "an entry sits outside VialRGB's id range"
        );
    }

    #[test]
    fn ids_are_unique_and_lookups_round_trip() {
        let ids: HashSet<u16> = EFFECTS.iter().map(|e| e.id).collect();
        assert_eq!(ids.len(), EFFECTS.len(), "an id is duplicated");

        for entry in EFFECTS.iter() {
            assert_eq!(EffectEntry::by_id(entry.id).unwrap().name, entry.name);
            assert_eq!(EffectEntry::by_key(entry.key).unwrap().name, entry.name);
        }
    }

    #[test]
    fn off_direct_and_unknown_ids_have_no_entry() {
        assert!(EffectEntry::by_id(EFFECT_OFF).is_none(), "OFF means disabled");
        assert!(EffectEntry::by_id(EFFECT_DIRECT).is_none(), "DIRECT is host-driven");
        assert!(EffectEntry::by_id(EFFECT_MAX).is_none());
        assert!(EffectEntry::by_id(999).is_none());
        assert!(EffectEntry::by_key("nope").is_none());
    }

    #[test]
    fn keys_are_unique_and_lowercase() {
        let keys: HashSet<&str> = EFFECTS.iter().map(|e| e.key).collect();
        assert_eq!(keys.len(), EFFECTS.len(), "a config key is duplicated");
        for entry in EFFECTS.iter() {
            assert_eq!(entry.key, entry.key.to_lowercase(), "{} is not lowercase", entry.key);
        }
    }

    #[test]
    fn supported_ids_skip_gt_and_filter_by_enabled() {
        let mut out = [0u16; 64];
        let n = supported_ids(0, &[], &mut out);
        let expected: Vec<u16> = EFFECTS
            .iter()
            .filter(|e| e.implemented)
            .map(|e| e.id)
            .collect();
        assert_eq!(&out[..n], &expected[..]);
        assert!(n > 0);
        // An id with no entry at all is not reported.
        assert!(!out[..n].contains(&51));

        // `gt` drops everything at or below it.
        let mut out = [0u16; 64];
        let n = supported_ids(2, &[], &mut out);
        assert!(!out[..n].contains(&2));
        assert!(out[..n].contains(&6));
    }

    #[test]
    fn an_explicit_enabled_set_overrides_the_implemented_default() {
        // Bind the count first: reading `out` inside the assertion while the
        // slice borrow is live is what the borrow checker rejects.
        let mut out = [0u16; 64];
        let n = supported_ids(0, &[6], &mut out);
        assert_eq!(&out[..n], &[6]);

        // An enabled effect that is not implemented yet still has an id to
        // report: rejecting it is the build-time check's job, not this one's.
        let mut out = [0u16; 64];
        let n = supported_ids(0, &[25], &mut out);
        assert_eq!(&out[..n], &[25]);

        // An id with no entry at all is never reported, enabled or not.
        let mut out = [0u16; 64];
        assert_eq!(supported_ids(0, &[51], &mut out), 0, "51 is not an effect here");
    }

    #[test]
    fn supported_ids_stop_at_the_buffer_length() {
        let mut out = [0u16; 3];
        assert_eq!(supported_ids(0, &[], &mut out), 3);
        assert_eq!(out, [2, 3, 4]);
    }

    /// The end state of the port: every id Vial can ask for is implemented, so
    /// an unconfigured build reports the whole table and `supported_ids` has
    /// nothing to withhold.
    #[test]
    fn every_vialrgb_effect_is_implemented() {
        assert_eq!(EFFECTS.len(), 43, "VialRGB defines 43 effects");
        let missing: Vec<&str> = EFFECTS
            .iter()
            .filter(|e| !e.implemented)
            .map(|e| e.name)
            .collect();
        assert!(missing.is_empty(), "not ported yet: {missing:?}");

        let mut out = [0u16; 64];
        let n = supported_ids(0, &[], &mut out);
        assert_eq!(n, EFFECTS.len(), "every id is reported");
    }

    #[test]
    fn is_enabled_defaults_to_implemented() {
        // Every table entry is ported, so the default build enables all of them.
        for entry in EFFECTS.iter() {
            assert!(is_enabled(entry.id, &[]), "{} is ported", entry.name);
        }
        // An id with no entry is never on, however it is spelled.
        assert!(!is_enabled(51, &[]), "RIVERFLOW is not in this build");
        assert!(!is_enabled(0, &[]), "OFF is not an effect");

        // An explicit set still narrows the choice.
        assert!(is_enabled(6, &[6]));
        assert!(!is_enabled(2, &[6]));
    }

    /// Every id in the table must dispatch without panicking, batched or not,
    /// with and without remembered hits. This is the check that catches a
    /// dispatch arm wired to the wrong runner signature.
    #[test]
    fn every_id_dispatches_cleanly() {
        use crate::rgb::led_config::{LedConfig, LedPoint};
        use crate::rgb::runner::ArrayFrame;
        use crate::rgb::state::RgbConfig;

        const N: usize = 5;
        let mut points = [LedPoint::new(0, 32); N];
        let mut matrix = [(0u8, 0u8); N];
        for i in 0..N {
            points[i] = LedPoint::new((i as u8) * 56, if i < 3 { 0 } else { 64 });
            matrix[i] = (if i < 3 { 0 } else { 1 }, (i % 3) as u8);
        }
        let leds = LedConfig::from_matrix(points, matrix);
        let config = RgbConfig::with_defaults(true, 100, 200, 128, ALL_FLAGS, 255);

        // Cover batched and unbatched iteration, and a hit for the reactive ones.
        for process_limit in [N, 2] {
            for time in [0u32, 1234, 60_000] {
                let ctx = RenderContext {
                    leds: &leds,
                    config: &config,
                    center: RenderContext::<N>::DEFAULT_CENTER,
                    time,
                    process_limit,
                };
                let mut hits = HitTracker::new();
                hits.record(&leds, 0, 1);

                for entry in EFFECTS.iter() {
                    let mut rng = Rng::with_seed(1337);
                    let mut raindrops_index = u16::MAX;
                    let mut pixel_rain_index = 0u8;
                    let mut pixel_rain_wait = 0u32;
                    let mut frame_buf = impls::FramebufferEffects::new();
                    let mut frame = ArrayFrame::<N>::new();

                    // Walk the batches exactly as the engine will.
                    let mut iter = 0u8;
                    let mut guard = 0;
                    loop {
                        let params = EffectParams {
                            iter,
                            flags: ALL_FLAGS,
                            init: iter == 0,
                        };
                        let more = dispatch(
                            entry.id,
                            &ctx,
                            &params,
                            &hits,
                            &mut rng,
                            &mut raindrops_index,
                            &mut pixel_rain_index,
                            &mut pixel_rain_wait,
                            &mut frame_buf,
                            &mut frame,
                        );
                        guard += 1;
                        if !more {
                            break;
                        }
                        assert!(
                            guard < 64,
                            "{} did not terminate at process_limit {process_limit}",
                            entry.name
                        );
                        iter += 1;
                    }

                    // Every ported effect must leave the frame in a known state,
                    // not panic. A solid fallback counts as "known".
                    assert!(frame.pixels.len() == N, "{}", entry.name);
                }
            }
        }
    }

    /// `EFFECT_OFF` clears the frame rather than drawing something.
    #[test]
    fn off_clears_the_frame() {
        use crate::rgb::fx::color::Rgb;
        use crate::rgb::led_config::{LedConfig, LedPoint};
        use crate::rgb::runner::ArrayFrame;
        use crate::rgb::state::RgbConfig;

        let leds = LedConfig::from_matrix([LedPoint::new(0, 0), LedPoint::new(224, 64)], [(0, 0), (0, 1)]);
        let config = RgbConfig::default();
        let ctx = RenderContext::<2> {
            leds: &leds,
            config: &config,
            center: RenderContext::<2>::DEFAULT_CENTER,
            time: 0,
            process_limit: 2,
        };
        let hits = HitTracker::new();
        let mut rng = Rng::with_seed(1);
        let mut index = u16::MAX;
        let mut rain_index = 0u8;
        let mut wait = 0u32;
        let mut frame_buf = impls::FramebufferEffects::new();
        let mut frame = ArrayFrame::<2>::new();
        frame.fill(Rgb::WHITE);

        let params = EffectParams {
            iter: 0,
            flags: ALL_FLAGS,
            init: true,
        };
        assert!(!dispatch(
            0,
            &ctx,
            &params,
            &hits,
            &mut rng,
            &mut index,
            &mut rain_index,
            &mut wait,
            &mut frame_buf,
            &mut frame
        ));
        assert_eq!(frame.pixels, [Rgb::BLACK; 2]);
    }
}
