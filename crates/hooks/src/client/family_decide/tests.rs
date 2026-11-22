//! Both directions of every family decision the hooks layer now executes.
//!
//! The decision tables themselves live in `features::family` and are tested
//! there; these tests only prove the hooks layer reaches them, and that a flag
//! which is off reproduces the answer the Linux host gave before this module
//! existed.
use super::*;
use vapor_forge_config::InjectApp;
use vapor_forge_features::family::concurrent::PlayReason;

/// `[apps.shared]` policy plus one app, controlled or not.
fn config(shared_enabled: bool, controlled: bool) -> (RuntimeConfig, AppId) {
    let mut config = RuntimeConfig::default();
    config.apps.shared.enabled = shared_enabled;
    let app = AppId(480);
    if controlled {
        config.apps.push_inject(InjectApp {
            id: app,
            dlc: Vec::new(),
            ticket: Default::default(),
            purchase_time: 0,
        });
    }
    (config, app)
}

/// The answer this host gave before the wiring: `is_controlled_app ||
/// should_bypass_sharing`, and no lock at all.
fn pre_wiring_answer(config: &RuntimeConfig, app: AppId) -> bool {
    config.is_controlled_app(app) || config.should_bypass_sharing(app)
}

#[test]
fn concurrent_play_matches_the_pre_wiring_answer_for_every_policy() {
    for shared_enabled in [true, false] {
        for controlled in [true, false] {
            let (config, app) = config(shared_enabled, controlled);
            for locking_account in [0, 42] {
                let decision = concurrent_play(&config, app, locking_account, Provenance::Unknown);
                assert_eq!(
                    decision.allowed,
                    locking_account == 0 || pre_wiring_answer(&config, app),
                    "shared_enabled={shared_enabled} controlled={controlled} \
                     lock={locking_account}"
                );
            }
        }
    }
}

#[test]
fn the_shared_decision_supplies_the_reason_the_host_used_to_re_derive() {
    let (mut config, app) = config(true, false);
    config.apps.shared.exclude = vec![app];
    let held = concurrent_play(&config, app, 7, Provenance::FamilyShared);
    assert!(!held.allowed);
    assert_eq!(held.reason, PlayReason::PolicyRefuses);
    assert!(concurrent_play(&config, app, 0, Provenance::Owned).allowed);
    assert_eq!(
        concurrent_play(&config, app, 0, Provenance::Owned).reason,
        PlayReason::NoFamilyLock
    );
    let mut disabled = config.clone();
    disabled.apps.shared.enabled = false;
    assert_eq!(
        concurrent_play(&disabled, app, 7, Provenance::FamilyShared).reason,
        PlayReason::SharingDisabled
    );
}

#[test]
fn a_controlled_app_is_unlocked_for_the_shared_controlled_app_reason() {
    let (config, app) = config(false, true);
    let decision = concurrent_play(&config, app, 7, Provenance::IngestedByUs);
    assert!(decision.allowed);
    assert_eq!(decision.reason, PlayReason::ControlledApp);
}

/// Flag off: the label still comes from the shared decision, but the write it
/// could authorise does not happen. That is the pre-fix behaviour.
#[test]
fn clearing_a_recorded_share_needs_the_lender_validation_flag() {
    let (config, app) = config(true, true);
    let shared = provenance::reported(Provenance::FamilyShared, &config, app);
    assert_eq!(shared.reported, ReportedProvenance::FamilyShared);
    assert!(
        !clears_recorded_share(true, &shared, FamilySettings::default()),
        "a lender's license stays a lender's license"
    );
    let owned = provenance::reported(Provenance::Owned, &config, app);
    assert_eq!(owned.reported, ReportedProvenance::Owned);
    assert!(
        clears_recorded_share(true, &owned, FamilySettings::default()),
        "with the flag on the shared decision is acted on"
    );
    assert!(
        !clears_recorded_share(true, &owned, FamilySettings::legacy()),
        "with the flag off the record is left alone, exactly as before"
    );
    assert!(
        !clears_recorded_share(false, &owned, FamilySettings::default()),
        "a record that claims no share needs no clearing"
    );
}

#[test]
fn a_lender_is_only_named_when_the_account_is_someone_else() {
    assert_eq!(lender_from_owner(4_242, 1_000), Some(4_242));
    assert_eq!(lender_from_owner(0, 1_000), None);
    assert_eq!(lender_from_owner(1_000, 1_000), None);
    assert_eq!(
        lender_from_owner(provenance::NO_ACCOUNT, 1_000),
        None,
        "the fabricated sentinel is not a lender"
    );
}

#[test]
fn the_marker_pass_is_disabled_by_its_own_flag() {
    let package = PackageState::new();
    package.stamp_borrower_marker(AppId(480), 4_242);
    let disabled = note_unborrowed(&package, AppId(480), false, FamilySettings::legacy());
    assert_eq!(disabled, MarkerOutcome::Disabled);
    assert_eq!(package.borrower_marker(AppId(480)), Some(4_242));
    let removed = note_unborrowed(&package, AppId(480), false, FamilySettings::default());
    assert_eq!(removed, MarkerOutcome::Removed { lender: 4_242 });
    assert_eq!(package.borrower_marker(AppId(480)), None);
    assert_eq!(
        note_unborrowed(&package, AppId(480), false, FamilySettings::default()),
        MarkerOutcome::Absent,
        "a second pass is a no-op that says so"
    );
}

#[test]
fn a_stamped_marker_records_the_lender_and_accepts_no_sentinel() {
    let _guard = crate::client::ACCOUNT_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let package = PackageState::new();
    assert_eq!(note_lender(&package, AppId(480), 0, 1_000), None);
    assert_eq!(package.borrower_marker(AppId(480)), None);
    assert_eq!(note_lender(&package, AppId(480), 4_242, 1_000), Some(4_242));
    assert_eq!(package.borrower_marker(AppId(480)), Some(4_242));
    assert_eq!(apps::family_owner(AppId(480)), Some(4_242));
    apps::record_family_shared(AppId(480), false);
    assert_eq!(apps::family_owner(AppId(480)), None, "cleanup");
}

/// The settings seam reads the value `config_apply` published, not a copy.
#[test]
fn settings_are_the_config_applied_value() {
    // `apply` publishes both the family settings and the achievement flag, so this
    // test shares the crate's feature-state lock with the achievement tests.
    let _guard = crate::test_lock::lock();
    let mut config = RuntimeConfig::default();
    config.apps.shared.validate_lender = false;
    config.apps.shared.clean_borrower_markers = false;
    let applied = config_apply::apply(&config);
    assert_eq!(settings(), applied.family);
    assert!(!settings().validate_lender);
    assert!(!settings().clean_borrower_markers);
    let defaults = config_apply::apply(&RuntimeConfig::default());
    assert_eq!(settings(), defaults.family);
    assert!(settings().validate_lender, "the default stays on");
    assert!(settings().clean_borrower_markers);
}
