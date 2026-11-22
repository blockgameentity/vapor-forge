//! The local achievement rebuild flag, surfaced (and applied) by the host.
//!
//! `vapor_forge_features::achievements` rebuilds the achievement payload from
//! locally known definitions when the schema path cannot describe it. The reply is
//! composed in a fixed order — schema definitions, then Steam's own unlock map,
//! then the user's Lua `setachievement` intent — and only a composition that
//! differs from Steam's bytes is ever swapped in. That is why
//! `[achievements] local_rebuild` defaults to true: a host with no Lua override
//! produces byte-identical replies and touches nothing.
//!
//! [`apply`] is the only place the host writes the feature's process-wide switch,
//! and only when the `achievements` capability is enabled.
//!
//! [`rebuild`] is the other half: the decision that would answer a stats reply with
//! a locally rebuilt payload. Both win64 stats-job sites are anchored
//! (`CAPIJobRequestUserStats::BInit` and `::YieldingMergeStats`), the merge now
//! carries the body ([`crate::hooks::achievements`]) that forwards the call and
//! rewrites the reply body from a rebuilt payload. Whether it can rewrite depends on
//! one corpus decode (`patterns::job_fields`, wired through
//! [`crate::hooks::achievements::geometry`]); until that lands the body records the
//! gap and forwards Steam's reply unchanged.

use crate::features::report::FeatureState;
use vapor_forge_features::achievements::{local_rebuild_enabled, set_local_rebuild};

pub mod rebuild;

pub use rebuild::{
    counts as rebuild_counts, decide as decide_rebuild, RebuildPlan, RebuildRequest,
};

/// The flag the host resolved from config.
///
/// [`Default`] is the *unresolved* value (`false`), not the user-facing default:
/// `[achievements] local_rebuild` is on in the shipped config, and the worker
/// copies the applied value onto this struct before the state is published. A
/// state built without it therefore refuses rather than serving something no
/// config asked for.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AchievementSettings {
    /// Whether the reply rewrite may run at all.
    pub local_rebuild: bool,
}

/// What the flag means for the report. Pure: does not touch the switch.
pub fn state(settings: &AchievementSettings) -> FeatureState {
    if settings.local_rebuild {
        FeatureState::active(
            "reply rewrite is on; a Lua setachievement override is what makes a reply differ",
        )
    } else {
        FeatureState::inactive(
            "reply rewrite is off ([achievements] local_rebuild = false); \
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
