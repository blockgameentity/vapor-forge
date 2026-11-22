//! The local achievement rebuild — **decision ready, both job sites anchored,
//! still no body**.
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
//! # Both job sites, and why neither carries a body yet
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
//!    object, and the two arguments are only `this` and an `EResult`.
//! 2. Replacing that block means constructing Steam's own ordered dictionary: the
//!    merge allocates a fresh block and inserts into it with **inlined** code
//!    (0x38-byte nodes, interned keys, a comparator called at `0x138d9ef40`). There
//!    is no writer function to anchor — so this is not a missing pattern, it is a
//!    shape that no pattern can supply.
//! 3. Every field of the job object is a per-build offset (`engine`, the reply
//!    block, the `CGameID` holding the app id — public `0x170`/`0x1a8`/`0x198`,
//!    publicbeta `0x168`/`0x1a0`/`0x190`). A body would carry a build table and
//!    fail closed off it; the corpus carries patterns only.
//! 4. The decision's `local` input has no producer on this host: definitions come
//!    from a stats schema parsed by `vapor_forge_steam_protocol`, which this crate
//!    does not depend on and which no Windows hook has ever fed.
//!
//! So the body that *would* work is not at the job at all: it is the reply-body
//! site (§1 site 2, the response parser at `0xa93120` / `0xabcc20`, unanchored) or
//! the CM receive path with the win64 packet layout
//! (`CNetPacket::{Init, Alloc, Release}`, in neither win64 corpus file) — where
//! `legacy_response`/`service_response` bytes can actually be substituted, exactly
//! as Linux does — plus a schema cache for this host. Registering a body at the
//! job today would make the install report claim a hook that serves nothing, so
//! nothing is registered.
//!
//! [`decide`] is the portable half, written so the future body is a wrapper: it
//! runs the **real** feature entry point
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
mod tests {
    use super::*;
    use vapor_forge_features::achievements::{
        set_local_rebuild, AchievementDisplay, LocalAchievement,
    };

    /// The rebuild flag is process-wide, and `features::run` writes it from other
    /// tests in this binary. Every test here holds the crate-wide lock
    /// ([`crate::test_lock`]) so no one can flip the flag between the write and
    /// the assertion, and each restores it before the guard is dropped.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn on() -> AchievementSettings {
        AchievementSettings {
            local_rebuild: true,
        }
    }

    fn local() -> LocalAchievementSet {
        LocalAchievementSet {
            groups: Vec::new(),
            achievements: vec![
                LocalAchievement::new("ACH_ONE", 1, 0),
                LocalAchievement::new("ACH_TWO", 1, 1),
            ],
        }
    }

    fn request() -> RebuildRequest {
        RebuildRequest {
            app_id: 480,
            local: local(),
            unlocks: vec![AchievementUnlockState {
                key: "ACH_TWO".to_owned(),
                unlocked: true,
                unlock_time: Some(1_600_000_000),
            }],
            schema: None,
        }
    }

    #[test]
    fn a_disabled_capability_wins_over_every_other_reason() {
        let plan = decide(Some("CUser::GetUserStats did not resolve"), &on(), &request());
        assert_eq!(
            plan,
            RebuildPlan::CapabilityDisabled("CUser::GetUserStats did not resolve".to_owned())
        );
        assert!(!plan.rebuilds());
        assert!(plan.summary().contains("disabled"));
    }

    #[test]
    fn the_flag_being_off_leaves_the_schema_path_alone() {
        let _guard = serial();
        let plan = decide(None, &AchievementSettings::default(), &request());
        assert_eq!(plan, RebuildPlan::FlagOff);
        assert!(counts(&plan).is_none());
    }

    #[test]
    fn a_setting_that_was_never_applied_is_refused_rather_than_served() {
        let _guard = serial();
        set_local_rebuild(false);
        let plan = decide(None, &on(), &request());
        assert_eq!(plan, RebuildPlan::SwitchOff);
    }

    #[test]
    fn a_reply_with_a_schema_keeps_the_schema_authoritative() {
        let _guard = serial();
        set_local_rebuild(true);
        let mut request = request();
        request.schema = Some(vec![0x01, 0x02, 0x03]);
        assert_eq!(decide(None, &on(), &request), RebuildPlan::SchemaAvailable);
        set_local_rebuild(false);
    }

    #[test]
    fn a_reply_that_names_no_app_is_not_rebuilt() {
        let _guard = serial();
        set_local_rebuild(true);
        let mut request = request();
        request.app_id = 0;
        assert_eq!(decide(None, &on(), &request), RebuildPlan::NoApp);
        set_local_rebuild(false);
    }

    #[test]
    fn no_local_definitions_means_nothing_to_rebuild_from() {
        let _guard = serial();
        set_local_rebuild(true);
        let mut request = request();
        request.local = LocalAchievementSet::default();
        assert_eq!(
            decide(None, &on(), &request),
            RebuildPlan::NoLocalDefinitions
        );
        set_local_rebuild(false);
    }

    #[test]
    fn an_enabled_flag_rebuilds_through_the_feature_entry_point() {
        let _guard = serial();
        set_local_rebuild(true);
        let plan = decide(None, &on(), &request());
        let RebuildPlan::Rebuild(rebuilt) = &plan else {
            panic!("an enabled flag with local definitions must rebuild: {plan:?}");
        };
        assert_eq!(rebuilt.app_id, 480);
        assert_eq!(rebuilt.counts.total, 2);
        assert_eq!(rebuilt.counts.unlocked, 1);
        assert_eq!(rebuilt.source, RebuildSource::Local);
        assert_eq!(rebuilt.achievements.len(), 2);
        assert_eq!(counts(&plan), Some(rebuilt.counts));
        assert!(plan.summary().contains("rebuilt 2 achievement(s)"));
        // The rebuilt display names come from the key, never from an invention.
        assert_eq!(
            rebuilt.achievements[0].display,
            AchievementDisplay::from_key("ACH_ONE")
        );
        set_local_rebuild(false);
    }
}
