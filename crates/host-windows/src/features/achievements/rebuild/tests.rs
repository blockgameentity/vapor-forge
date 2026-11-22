//! Unit tests for the decision, split out of `mod.rs` to keep it under 300 lines.

use super::*;
use vapor_forge_features::achievements::{set_local_rebuild, AchievementDisplay, LocalAchievement};

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
    let plan = decide(
        Some("CUser::GetUserStats did not resolve"),
        &on(),
        &request(),
    );
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
