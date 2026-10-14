//! Persisting the RGB configuration, the port of QMK's
//! `eeconfig_force_flush_rgb_matrix`.
//!
//! Only Vial's `CustomSave` (`0x09`) writes flash. Every other setter takes the
//! `_noeeprom` path, exactly as QMK's do: while a user drags the brightness
//! slider Vial sends a report per step, and committing each one would wear the
//! flash for no benefit.
//!
//! # Boot order
//!
//! [`restore`] runs during storage initialisation, before the engine task
//! starts, so the first frame already reflects what the user last saved. If
//! nothing was ever stored, the compiled-in defaults stand and are not written
//! back — an unwritten value is the default, and writing it would only consume a
//! flash page.

use super::state::RgbConfig;

/// Save the current runtime configuration.
///
/// Returns `Err(())` when the storage task refused the write. The unit error is
/// deliberate: it matches `keyboard::macros::persist` and the rest of this
/// crate's persistence helpers, which only ever signal success or failure to a
/// caller that logs and moves on.
#[cfg(feature = "storage")]
#[allow(clippy::result_unit_err)]
pub async fn save(config: RgbConfig) -> Result<(), ()> {
    crate::storage::store(crate::storage::StorageItem::RgbConfig(config))
        .await
        .map(|_| ())
}

/// Read the stored configuration, if any.
///
/// `None` means nothing was ever saved, or the stored value did not decode;
/// either way the caller keeps the compiled-in defaults.
#[cfg(feature = "storage")]
pub async fn load() -> Option<RgbConfig> {
    match crate::storage::read(crate::storage::StorageKey::RgbConfig).await {
        Ok(Some(crate::storage::StorageValue::RgbConfig(config))) => Some(config),
        _ => None,
    }
}

/// Install a restored configuration into the shared state.
///
/// Split from [`load`] so the storage task can read while this runs outside it.
pub fn apply(config: RgbConfig) {
    super::via::restore_config(config);
}

/// Restore from storage and install, leaving the defaults in place on a miss.
///
/// Called during storage initialisation.
#[cfg(feature = "storage")]
pub async fn restore() {
    if let Some(config) = load().await {
        apply(config);
    }
}

/// Persist the current configuration if the protocol marked it dirty.
///
/// Returns whether a write happened, so the caller can log one line rather than
/// one per report.
#[cfg(feature = "storage")]
pub async fn flush_if_dirty() -> bool {
    if !super::shared::is_dirty() {
        return false;
    }
    let config = super::via::config_for_storage();
    if save(config).await.is_ok() {
        super::shared::take_dirty();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rgb::registry::EFFECT_MAX;

    #[test]
    fn apply_installs_the_configuration_into_the_shared_state() {
        // The shared state is process-wide, so save and restore around it.
        let before = super::super::via::config_for_storage();

        let mut config = RgbConfig::new_const();
        config.enable();
        config.set_mode(24, EFFECT_MAX);
        config.set_speed(200);
        apply(config);

        let after = super::super::via::config_for_storage();
        assert_eq!(after.mode(), 24);
        assert_eq!(after.speed(), 200);
        assert!(after.is_enabled());

        apply(before);
    }

    #[cfg(feature = "storage")]
    #[test]
    fn the_dirty_flag_gates_flushing() {
        use crate::rgb::shared;

        // The flag is process-global, so this test only observes it. It does
        // not assert on a clean starting state, because another test in the same
        // binary may have set it; `take_dirty` is idempotent, so consuming
        // whatever is there first makes the assertions order-independent.
        shared::take_dirty();
        assert!(!shared::is_dirty(), "consuming the flag clears it");

        shared::mark_dirty();
        assert!(shared::is_dirty());
        // `take_dirty` consumes it, so a second flush writes nothing.
        assert!(shared::take_dirty());
        assert!(!shared::is_dirty());
        assert!(!shared::take_dirty());
    }

    #[test]
    fn a_config_round_trips_through_serde() {
        // The storage layer serializes with postcard; the shape being stable is
        // what makes a stored value readable by the next boot.
        let mut config = RgbConfig::new_const();
        config.set_mode(13, EFFECT_MAX);
        config.set_hsv(1, 2, 3, 255);
        config.set_speed(77);

        // A fixed buffer, since this crate builds without an allocator.
        let mut buf = [0u8; 32];
        let used = postcard::to_slice(&config, &mut buf).expect("serialize");
        let back: RgbConfig = postcard::from_bytes(used).expect("deserialize");
        assert_eq!(back, config);
    }
}
