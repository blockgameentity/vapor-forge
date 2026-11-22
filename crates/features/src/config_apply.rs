//! Push the user-facing config into the feature modules' settings seams.
//!
//! The feature modules are deliberately config-free: each owns a small settings
//! value and a setter, so it can be tested without the config crate. This module
//! is the one place that reads `RuntimeConfig` and applies it, which means a
//! reviewer can see every user-visible flag in a single function.
//!
//! Called from the runtime's config (re)load path on Linux
//! (`ensure_runtime_services_for_config`) and from the Windows host worker, so a
//! hot-reloaded config reaches the features without a restart.

use std::sync::Mutex;

use vapor_forge_config::RuntimeConfig;

use crate::achievements;
use crate::family::FamilySettings;

/// Family settings currently in force.
///
/// `features::family` takes its settings as an argument (its decisions are pure
/// functions), so the resolved value lives here for callers that have no config
/// handle — the hook bodies, which run deep inside Steam.
static FAMILY: Mutex<FamilySettings> = Mutex::new(FamilySettings::fixed());

/// What the last [`apply`] turned on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppliedFeatures {
    pub family: FamilySettings,
    pub achievements_local_rebuild: bool,
}

/// Apply `config` to every feature that keeps process-wide settings.
pub fn apply(config: &RuntimeConfig) -> AppliedFeatures {
    let shared = &config.apps.shared;
    let family = FamilySettings::fixed()
        .with_lender_validation(shared.validate_lender)
        .with_borrower_marker_cleanup(shared.clean_borrower_markers);

    {
        let mut current = FAMILY.lock().unwrap_or_else(|error| error.into_inner());
        if *current != family {
            tracing::info!(
                validate_lender = family.validate_lender,
                clean_borrower_markers = family.clean_borrower_markers,
                "family-share settings applied from config"
            );
            *current = family;
        }
    }

    let achievements_local_rebuild = config.achievements.local_rebuild;
    achievements::set_local_rebuild(achievements_local_rebuild);

    AppliedFeatures {
        family,
        achievements_local_rebuild,
    }
}

/// Family settings resolved from the last applied config.
pub fn family_settings() -> FamilySettings {
    *FAMILY.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(validate_lender: bool, clean_markers: bool, rebuild: bool) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.apps.shared.validate_lender = validate_lender;
        config.apps.shared.clean_borrower_markers = clean_markers;
        config.achievements.local_rebuild = rebuild;
        config
    }

    /// One test on purpose: `FAMILY` is process-wide and these are the only
    /// writers, so serialising them inside one test is enough. The achievements
    /// flag is deliberately asserted through the returned value only — its
    /// global is also mutated by that module's own tests, which run in parallel,
    /// and the setter itself is covered there.
    #[test]
    fn config_reaches_every_feature_seam() {
        let applied = apply(&config_with(false, true, false));
        assert!(!applied.family.validate_lender);
        assert!(applied.family.clean_borrower_markers);
        assert_eq!(family_settings(), applied.family, "the mirror must match");
        assert!(!applied.achievements_local_rebuild);

        let applied = apply(&config_with(true, true, true));
        assert!(applied.family.validate_lender);
        assert!(applied.achievements_local_rebuild);
        assert_eq!(family_settings(), applied.family);

        let defaults = apply(&RuntimeConfig::default());
        assert!(
            defaults.family.validate_lender,
            "an app we ingested must never read back as borrowed"
        );
        assert!(defaults.family.clean_borrower_markers);
        assert!(
            defaults.achievements_local_rebuild,
            "the shipped default is on, and inert without a Lua override"
        );
    }
}
