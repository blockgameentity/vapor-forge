//! The local achievement rebuild — the decision, and the body that applies it.
//!
//! `features::achievements::local` rebuilds a stats reply's achievement payload
//! from locally known definitions when the schema path cannot describe it. The
//! Linux host reaches it from the reply path it owns
//! (`crates/hooks/src/netpacket/stats_proxy.rs::inject_merged_stats_response` →
//! `client::achievement::local_reply`): it keeps Steam's schema when there is
//! one, and otherwise answers with the rebuilt page, encoded by
//! `RebuiltAchievements::{service_response, legacy_response}`.
//!
//! # The site
//!
//! `res/patterns/win64/VTABLE-REPORT.md` §1 replaced the old verdict ("no stats
//! symbol exists") with the real shape: a job chain. `CUserStats` slot 6 constructs
//! a `CAPIJobRequestUserStats`; the job's run method
//! [`crate::abi::achievements::BINIT_SYMBOL`] sends the request and calls the
//! response parser and then the merge; and
//! [`crate::abi::achievements::MERGE_SYMBOL`] merges the parsed reply into the
//! client's stats map. Both are anchored in both live builds, and the merge is the
//! site this host bodies: it is the completion point, and it is where the reply
//! object is reachable from `this`.
//!
//! # What the body can and cannot do
//!
//! [`crate::hooks::achievements`] forwards the merge verbatim and, when the
//! rebuild produces a payload that differs, swaps the reply body for the duration
//! of that call and restores Steam's bytes afterwards. Three inputs are needed, and
//! each has a named answer rather than a guess:
//!
//! 1. **Where the reply body is** — the geometry decoder
//!    (`patterns::job_fields::decode_job_reply`), wired through
//!    [`crate::hooks::achievements::geometry`]. Not merged yet, so
//!    [`crate::hooks::achievements::ReplyOutcome::NoGeometry`] is what a live client
//!    records today.
//! 2. **Which app the reply is about** — the reply itself, for the legacy shape
//!    (`CGameID`'s low 24 bits). A service-shaped reply names no app, and the job
//!    object's own `game_id` is a per-build offset this crate no longer keeps, so
//!    that reply is reported as [`crate::hooks::achievements::reply::RewriteBlocker::NoApp`].
//! 3. **The definitions** — the schema a *previous* reply for the same app carried,
//!    held in [`crate::hooks::achievements::cache`]. A reply that carries its own
//!    schema keeps the schema path authoritative, exactly as on Linux, and the
//!    rebuild is not consulted for it.
//!
//! and one input that is a source rather than a derivation:
//!
//! 4. **The user's intent** — `setachievement(appid, key, unlocked|time)` in a Lua
//!    script, carried on [`RebuildRequest::overrides`]. It is composed on top of
//!    Steam's unlock map by `features::achievements::compose_unlocks`, and only
//!    where the definitions name the key.
//!
//! So the rewrite fires for a schema-less legacy reply whose app this process has
//! already seen a schema for **and** whose Lua overrides change a defined
//! achievement. That second half is what makes the flag safe to leave on: with no
//! overrides the composed map is Steam's own, [`RebuildPlan::OverridesInert`] is
//! the answer, and not a byte of Steam's reply is touched. It is also the same
//! fallback order the Linux host applies, so the first reply for an app teaches
//! this host and a later one is answered.
//!
//! [`decide`] is the portable half the body consults: it runs the **real** feature
//! entry point
//! (`rebuild_local_achievements_if_enabled`), so the flag, the schema precedence
//! and the definitions all behave exactly as the Linux path does.

use std::collections::BTreeMap;

use vapor_forge_config::AppId;
use vapor_forge_core::AchievementIntent;
use vapor_forge_features::achievements::{
    compose_unlocks, local_rebuild_enabled, rebuild_local_achievements_if_enabled,
    AchievementCounts, AchievementUnlockState, LocalAchievementSet, RebuildSource,
    RebuiltAchievements,
};

use super::AchievementSettings;

/// One stats reply the host is asked about.
///
/// `unlocks` and `schema` are what the reply would carry; `local` is what the host
/// knows locally about `app_id`; `overrides` is what the user's scripts asked for.
/// Nothing here is read from the module image.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RebuildRequest {
    /// The app the reply is about. `0` means "not a known app", which the Linux
    /// decision refuses.
    pub app_id: u32,
    /// Locally known definitions, empty when the host has none.
    pub local: LocalAchievementSet,
    /// The account's unlock state per achievement key.
    pub unlocks: Vec<AchievementUnlockState>,
    /// The schema the reply carries, when it carries one.
    pub schema: Option<Vec<u8>>,
    /// The user's Lua intent for this app, keyed by achievement key. Empty for an
    /// app nothing was written for, which is every app by default.
    pub overrides: BTreeMap<String, AchievementIntent>,
}

/// What the host should answer with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RebuildPlan {
    /// The install pass disabled `achievements`: do nothing at all.
    CapabilityDisabled(String),
    /// `[achievements] local_rebuild` is off: Steam's schema path is untouched.
    FlagOff,
    /// The setting says on but the process-wide switch is off, i.e. the host never
    /// applied it (or something reset it). Refused rather than served, because the
    /// switch is the feature's own guard.
    SwitchOff,
    /// The reply names no app: there is nothing to look definitions up for.
    NoApp,
    /// The reply carries a usable schema. The schema path stays authoritative;
    /// the rebuild is not consulted, exactly as on Linux.
    SchemaAvailable,
    /// The flag is on and the reply carries no schema, but the host knows no
    /// definitions for this app, so there is nothing to build a page from.
    NoLocalDefinitions,
    /// The rebuild could run, but nothing it would say differs from Steam's own
    /// reply: no Lua override changed a defined achievement. Forwarded untouched,
    /// which is what makes the flag inert without overrides.
    OverridesInert,
    /// The payload the host would serve, and every count the log needs.
    Rebuild(RebuiltAchievements),
}

impl RebuildPlan {
    /// Whether the host would answer the reply itself.
    pub fn rebuilds(&self) -> bool {
        matches!(self, Self::Rebuild(_))
    }

    /// One line for the report or the log.
    pub fn summary(&self) -> String {
        match self {
            Self::CapabilityDisabled(reason) => {
                format!("achievements are disabled: {reason}")
            }
            Self::FlagOff => "the local achievement rebuild is off".to_owned(),
            Self::SwitchOff => {
                "the local achievement rebuild flag was not applied to this process".to_owned()
            }
            Self::NoApp => "the reply names no app".to_owned(),
            Self::SchemaAvailable => {
                "the reply carries a schema; the schema path stays authoritative".to_owned()
            }
            Self::NoLocalDefinitions => "no local achievement definitions for this app".to_owned(),
            Self::OverridesInert => {
                "no Lua override changes this reply, so Steam's bytes stand".to_owned()
            }
            Self::Rebuild(rebuilt) => format!(
                "rebuilt {} achievement(s) from {} definitions ({} unlocked)",
                rebuilt.counts.total,
                match rebuilt.source {
                    RebuildSource::Schema => "schema",
                    RebuildSource::Local => "local",
                },
                rebuilt.counts.unlocked
            ),
        }
    }
}

/// Decide what to answer `request` with.
///
/// `capability_reason` is the gate's reason for `achievements`, so a disabled
/// capability wins over every configuration reason — the same precedence
/// [`crate::features::run`] applies.
///
/// Pure with one exception, which is the point: it calls the feature's own
/// entry point, so the flag the feature enforces cannot be bypassed by a caller
/// that computed its own answer.
pub fn decide(
    capability_reason: Option<&str>,
    settings: &AchievementSettings,
    request: &RebuildRequest,
) -> RebuildPlan {
    if let Some(reason) = capability_reason {
        return RebuildPlan::CapabilityDisabled(reason.to_owned());
    }
    if !settings.local_rebuild {
        return RebuildPlan::FlagOff;
    }
    if !local_rebuild_enabled() {
        return RebuildPlan::SwitchOff;
    }
    if request.app_id == 0 {
        return RebuildPlan::NoApp;
    }
    if request
        .schema
        .as_ref()
        .is_some_and(|schema| !schema.is_empty())
    {
        return RebuildPlan::SchemaAvailable;
    }
    if request.local.achievements.is_empty() {
        return RebuildPlan::NoLocalDefinitions;
    }
    // Compose before rebuilding: the reply is Steam's unlock map with the user's
    // intent on top, and a map that says what Steam already said needs no rebuild
    // and no swap. This is the whole reason the flag can stay on by default.
    let composed = compose_unlocks(
        request.app_id,
        &request.local,
        &request.unlocks,
        Some(&request.overrides),
    );
    if !composed.changed {
        return RebuildPlan::OverridesInert;
    }
    let app_id = AppId(request.app_id);
    let schema = request
        .schema
        .as_deref()
        .filter(|schema| !schema.is_empty());
    match rebuild_local_achievements_if_enabled(app_id.0, &request.local, &composed.unlocks, schema)
    {
        Some(rebuilt) => RebuildPlan::Rebuild(rebuilt),
        // The feature refused although the switch looked on, or it produced
        // nothing: both mean "behave as before", never "serve an empty page".
        None => RebuildPlan::SwitchOff,
    }
}

/// The counts a caller reports, for a plan that may not have rebuilt anything.
pub fn counts(plan: &RebuildPlan) -> Option<AchievementCounts> {
    match plan {
        RebuildPlan::Rebuild(rebuilt) => Some(rebuilt.counts),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
