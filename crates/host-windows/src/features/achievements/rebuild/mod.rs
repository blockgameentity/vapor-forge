//! The local achievement rebuild — **decision ready, merge site observed, rebuild
//! still blocked on one anchor**.
//!
//! `features::achievements::local` rebuilds a stats reply's achievement payload
//! from locally known definitions when the schema path cannot describe it. The
//! Linux host reaches it from the reply path it owns
//! (`crates/hooks/src/netpacket/stats_proxy.rs::inject_merged_stats_response` →
//! `client::achievement::local_reply`): it keeps Steam's schema when there is
//! one, and otherwise answers with the rebuilt page, encoded by
//! `RebuiltAchievements::{service_response, legacy_response}` into the packet body
//! about to be queued.
//!
//! # Both job sites, and why neither carries the rebuild
//!
//! `res/patterns/win64/VTABLE-REPORT.md` §1 replaced the old verdict ("no stats
//! symbol exists") with the real shape: a job chain. `CUserStats` slot 6 constructs
//! a `CAPIJobRequestUserStats`; the job's run method
//! [`crate::abi::achievements::BINIT_SYMBOL`] sends the request and calls the
//! response parser and then the merge; and
//! [`crate::abi::achievements::MERGE_SYMBOL`] merges the parsed reply into the
//! client's stats map. **Both are anchored in both live builds**, so this host now
//! has two candidate sites — and the merge is the semantically right one: it is the
//! completion point, after the reply is in hand and before it reaches the local
//! map, and it is the site its own log strings name
//! ("`YieldingMergeStats - beginning merge`", "`using %s stats data`").
//!
//! It still cannot carry the rebuild, for four measured reasons (the disassembly
//! behind each is in [`crate::abi::achievements`]):
//!
//! 1. The reply *bytes* are gone by then. The Linux body substitutes an encoded
//!    reply; at the merge the reply is already a stats block inside the job
//!    object, and the two arguments are only `this` and an `EResult`. The bytes
//!    are at `*(this + reply) + 0x18` (`REPLY_BODY_OFFSET`, the same `+0x18` in
//!    both builds) and nothing at the job site reads them any more.
//! 2. Replacing that block means constructing Steam's own ordered dictionary: the
//!    merge allocates a fresh block and inserts into it with **inlined** code
//!    (0x38-byte nodes, interned keys, a comparator called at `0x138d9ef40`). There
//!    is no writer function to anchor — so this is not a missing pattern, it is a
//!    shape that no pattern can supply.
//! 3. Every field of the job object is a per-build offset (the engine, the reply
//!    block, the reply *object*, the `CGameID` holding the app id — public
//!    `0x170`/`0x1a8`/`0x1b0`/`0x198`, publicbeta `0x168`/`0x1a0`/`0x1a8`/`0x190`).
//!    A body would carry a build table and fail closed off it; the corpus carries
//!    patterns only.
//! 4. The decision's `local` input has no producer on this host: definitions come
//!    from a stats schema parsed by `vapor_forge_steam_protocol`, which this crate
//!    does not depend on and which no Windows hook has ever fed.
//!
//! So the body that *would* work is not at the job at all: it is the reply-body
//! site (§1 site 2, the response parser at `0xa93120` / `0xabcc20`, unanchored) or
//! the CM receive path with the win64 packet layout
//! (`CNetPacket::{Init, Alloc, Release}`, in neither win64 corpus file) — where
//! `legacy_response`/`service_response` bytes can actually be substituted, exactly
//! as Linux does — plus a schema cache for this host. Registering a body that
//! served nothing would make the install report claim a rebuild the host does not
//! have, so the only body registered at the job is the **observation-only** one in
//! [`crate::hooks::achievements`]: it forwards the merge verbatim, classifies the
//! `EResult` and logs [`decide`]'s answer without mutating anything.
//!
//! [`decide`] is the portable half and the observation body's wrapper: it runs the
//! **real** feature entry point
//! (`rebuild_local_achievements_if_enabled`), so the flag, the schema precedence
//! and the definitions all behave exactly as the Linux path does.

use vapor_forge_config::AppId;
use vapor_forge_features::achievements::{
    local_rebuild_enabled, rebuild_local_achievements_if_enabled, AchievementCounts,
    AchievementUnlockState, LocalAchievementSet, RebuildSource, RebuiltAchievements,
};

use super::AchievementSettings;

/// One stats reply the host is asked about.
///
/// `unlocks` and `schema` are what the reply would carry; `local` is what the host
/// knows locally about `app_id`. Nothing here is read from the module image.
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
            Self::NoLocalDefinitions => {
                "no local achievement definitions for this app".to_owned()
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
    if request.schema.as_ref().is_some_and(|schema| !schema.is_empty()) {
        return RebuildPlan::SchemaAvailable;
    }
    if request.local.achievements.is_empty() {
        return RebuildPlan::NoLocalDefinitions;
    }
    let app_id = AppId(request.app_id);
    let schema = request.schema.as_deref().filter(|schema| !schema.is_empty());
    match rebuild_local_achievements_if_enabled(
        app_id.0,
        &request.local,
        &request.unlocks,
        schema,
    ) {
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
