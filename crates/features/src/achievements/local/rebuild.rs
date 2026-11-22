//! Rebuild the achievement payload a client asks for from local data.
//!
//! Steam splits achievements over two payloads: the response carries the
//! *state* (unlock bits and times), while the *definitions* (names, icons,
//! descriptions, groups) come from the schema bytes the client already holds.
//! When the client has no usable schema for an app - a controlled app whose
//! schema was never fetched, a store page that cannot resolve the definition
//! table, a schema whose layout this build cannot read - the bits have nothing
//! to render: the page comes back blank even though the account has achievements.
//! Newer layouts make that worse by nesting definitions under groups, which a
//! flat reader reports as an empty schema.
//!
//! The rebuild answers that request from the local set instead: one entry per
//! achievement carrying its display attributes, every group membership, and the
//! unlock state applied, plus totals that agree with the emitted list. The wire
//! form of that list lives in [`super::wire`].
//!
//! Precedence, in order:
//!
//! 1. a schema that parses and declares achievements wins: it owns the wire ids
//!    and the definition list, and local data only fills in what a schema cannot
//!    carry (display text, groups);
//! 2. a missing, unreadable or achievement-less schema - which includes grouped
//!    layouts a flat reader sees as empty - falls back to the local set as the
//!    source of truth, and says so in the log.
//!
//! The output is a pure function of the inputs: same definitions, same unlock
//! state, same schema, same bytes.

use std::sync::Mutex;

use tracing::{debug, info};

use super::definitions::{
    build_achievements, local_definitions, report_unknown_unlocks, schema_definitions,
    schema_mappings, unlock_map,
};
use super::groups::build_groups;
use super::model::{AchievementDisplay, AchievementUnlockState, LocalAchievementSet};

// ---------------------------------------------------------------------------
// Feature gate
// ---------------------------------------------------------------------------

/// The settings the local rebuild reads.
///
/// Deliberately separate from [`vapor_forge_config::RuntimeConfig`]: the config
/// crate owns the user-facing flag, and this module must work (and be tested)
/// without it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LocalRebuildSettings {
    /// Off by default.
    ///
    /// Not because the rebuild is dangerous, but because today the only source of
    /// "local" definitions is the schema cache itself (`hooks::achievement_local`
    /// derives them from it), so with the flag on and a schema present the host
    /// re-derives the same definitions it was given, and with no schema there is
    /// nothing to derive. The flag exists for a host that supplies an independent
    /// local definition set; until one does, leaving it off changes nothing.
    pub local_rebuild: bool,
}

static SETTINGS: Mutex<LocalRebuildSettings> = Mutex::new(LocalRebuildSettings {
    local_rebuild: false,
});

/// The settings currently in force.
pub fn local_rebuild_settings() -> LocalRebuildSettings {
    *SETTINGS.lock().unwrap_or_else(|error| error.into_inner())
}

/// Turn the local achievement rebuild on or off.
///
/// Wired by the host config later: `crates/config` owns the user-facing setting,
/// so this setter is the seam that config calls once it grows one. Until then the
/// rebuild stays off, which is what "no stealth behaviour changes" requires.
pub fn set_local_rebuild(enabled: bool) {
    let mut settings = SETTINGS.lock().unwrap_or_else(|error| error.into_inner());
    if settings.local_rebuild == enabled {
        return;
    }
    settings.local_rebuild = enabled;
    info!(
        local_rebuild = enabled,
        "achievements: local achievement rebuild toggled"
    );
}

/// Whether the rebuild is enabled right now.
pub fn local_rebuild_enabled() -> bool {
    local_rebuild_settings().local_rebuild
}

// ---------------------------------------------------------------------------
// Rebuilt list
// ---------------------------------------------------------------------------

/// Where the rebuilt definitions came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RebuildSource {
    /// No usable schema: the local set is the source of truth.
    Local,
    /// A schema declared the achievements: it owns the wire ids and the list,
    /// local data supplies display text and groups.
    Schema,
}

/// One achievement in the rebuilt list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuiltAchievement {
    /// The achievement key, as the schema names it.
    pub key: String,
    pub stat_id: u32,
    pub bit: u32,
    pub display: AchievementDisplay,
    /// Group keys, sorted and de-duplicated. Empty when the layout is flat.
    pub groups: Vec<String>,
    pub unlocked: bool,
    /// The time to put on the wire. Present exactly when `unlocked`; Steam's
    /// "unlocked, time unknown" sentinel when no usable time was supplied.
    pub unlock_time: Option<u32>,
}

/// One group in the rebuilt list, with its surviving members.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuiltAchievementGroup {
    pub key: String,
    pub display_name: String,
    /// Keys of the achievements listed in this group, sorted.
    pub members: Vec<String>,
}

/// Totals over a rebuilt list, so callers never recount by hand.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AchievementCounts {
    pub unlocked: u32,
    pub total: u32,
}

/// A coherent achievement list rebuilt from local state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebuiltAchievements {
    pub app_id: u32,
    pub source: RebuildSource,
    /// Sorted by wire id (`stat_id`, `bit`), then key: rebuild order is output
    /// order, so equal input always encodes to equal bytes.
    pub achievements: Vec<RebuiltAchievement>,
    /// Sorted by group key, each with sorted members. A group with no emitted
    /// achievement is not listed: it could not be rendered anyway.
    pub groups: Vec<RebuiltAchievementGroup>,
    pub counts: AchievementCounts,
}

/// Rebuild `app_id`'s achievement list from local definitions and unlock state.
///
/// `schema` is the app's stats schema, when the caller has one. A schema that
/// parses and declares achievements wins; anything else falls back to `local`.
///
/// Degrades rather than fails: a malformed local definition is skipped with a
/// warning, an unlock state for an unknown key is ignored, and a schema the
/// parser cannot read is treated as absent. Nothing here panics on bad input.
pub fn rebuild_local_achievements(
    app_id: u32,
    local: &LocalAchievementSet,
    unlocks: &[AchievementUnlockState],
    schema: Option<&[u8]>,
) -> RebuiltAchievements {
    let unlock_map = unlock_map(unlocks);
    let (source, definitions) = match schema_mappings(schema, app_id) {
        Some(mappings) => (
            RebuildSource::Schema,
            schema_definitions(local, mappings, app_id),
        ),
        None => (RebuildSource::Local, local_definitions(local)),
    };
    let achievements = build_achievements(definitions, &unlock_map);
    let groups = build_groups(local, &achievements);
    let unlocked = achievements
        .iter()
        .filter(|achievement| achievement.unlocked)
        .count();
    let counts = AchievementCounts {
        unlocked: u32::try_from(unlocked).unwrap_or(u32::MAX),
        total: u32::try_from(achievements.len()).unwrap_or(u32::MAX),
    };
    report_unknown_unlocks(&achievements, &unlock_map);
    debug!(
        app_id,
        source = ?source,
        total = counts.total,
        unlocked = counts.unlocked,
        groups = groups.len(),
        "achievements: rebuilt local payload"
    );
    RebuiltAchievements {
        app_id,
        source,
        achievements,
        groups,
        counts,
    }
}

/// [`rebuild_local_achievements`], but only while the flag is on.
///
/// `None` means "behave as before": the caller leaves the request on the schema
/// path. This is the entry point a host should use, so the flag can never be
/// bypassed by accident.
pub fn rebuild_local_achievements_if_enabled(
    app_id: u32,
    local: &LocalAchievementSet,
    unlocks: &[AchievementUnlockState],
    schema: Option<&[u8]>,
) -> Option<RebuiltAchievements> {
    if !local_rebuild_enabled() {
        debug!(
            app_id,
            "achievements: local rebuild disabled, leaving the request on the schema path"
        );
        return None;
    }
    // Nothing to rebuild from: an app with no local definition set has no
    // opinion, and rewriting its reply would only replace Steam's answer with an
    // empty one.
    if local.achievements.is_empty() {
        debug!(
            app_id,
            "achievements: no local definitions, leaving the request on the schema path"
        );
        return None;
    }
    Some(rebuild_local_achievements(app_id, local, unlocks, schema))
}
