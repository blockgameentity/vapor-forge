//! The local achievement rebuild flag, surfaced (and applied) by the host.
//!
//! `vapor_forge_features::achievements` rebuilds the achievement payload from
//! locally known definitions when the schema path cannot describe it. The whole
//! path is behind a process-wide switch that is off by default, so with the flag
//! off the stats response path behaves exactly as it did before the rebuild
//! existed - no stealth behaviour change.
//!
//! Before this module the Windows host never touched that switch, so the feature
//! was unreachable: the flag stayed off no matter what the user configured.
//! [`apply`] is now the only place the host writes it, and only when the
//! `achievements` capability is enabled.

use crate::features::report::FeatureState;
use vapor_forge_features::achievements::{local_rebuild_enabled, set_local_rebuild};

/// The flag the host resolved from config.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AchievementSettings {
    /// Off by default, like the feature itself.
    pub local_rebuild: bool,
}

/// What the flag means for the report. Pure: does not touch the switch.
pub fn state(settings: &AchievementSettings) -> FeatureState {
    if settings.local_rebuild {
        FeatureState::active("local achievement rebuild is on")
    } else {
        FeatureState::inactive(
            "local achievement rebuild is off ([achievements] local_rebuild = false); \
             the schema path is unchanged",
        )
    }
}

/// Apply the flag to the process-wide switch and report the resulting state.
///
/// Idempotent: the feature crate's setter returns early when the value does not
/// change. Called only when the `achievements` capability is enabled.
pub fn apply(settings: &AchievementSettings) -> FeatureState {
    set_local_rebuild(settings.local_rebuild);
    let state = state(settings);
    tracing::info!(
        local_rebuild = settings.local_rebuild,
        enabled = local_rebuild_enabled(),
        "achievements: local rebuild flag applied"
    );
    state
}
