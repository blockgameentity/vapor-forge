use super::*;
use vapor_forge_config::{AppsSection, InjectApp, SharedSection};

const SELF: u32 = 7_777_777;
const LENDER: u32 = 9_999_999;

fn config(shared: SharedSection) -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    config.apps.shared = shared;
    config
}

fn config_controlling(ids: &[u32], shared: SharedSection) -> RuntimeConfig {
    let mut config = RuntimeConfig {
        apps: AppsSection::with_inject(
            ids.iter()
                .map(|&id| InjectApp {
                    id: AppId(id),
                    dlc: Vec::new(),
                    ticket: Default::default(),
                    purchase_time: 0,
                })
                .collect(),
        ),
        ..Default::default()
    };
    config.apps.shared = shared;
    config
}

fn shared_off() -> SharedSection {
    SharedSection {
        enabled: false,
        include: Vec::new(),
        exclude: Vec::new(),
        ..Default::default()
    }
}

/// The two flag states every table is run under.
const SETTINGS: [FamilySettings; 2] = [FamilySettings::legacy(), FamilySettings::fixed()];

fn signals(
    self_owned: bool,
    borrowed: bool,
    owner: Option<u32>,
    injected: bool,
) -> OwnershipSignals {
    OwnershipSignals {
        self_owned,
        borrowed,
        license_owner: owner,
        injected_by_us: injected,
    }
}

// Invariant 1, at its root: our fabricated license owner can never pass for
// a lender, so an app only we hold a title for is never family-shared.
#[test]
fn our_own_injection_never_classifies_as_family_shared() {
    let fabricated = [Some(NO_ACCOUNT), Some(0), Some(SELF), None];
    for owner in fabricated {
        let provenance = classify(
            signals(false, true, owner, true),
            SELF,
            FamilySettings::default(),
        );
        assert_eq!(
            provenance,
            Provenance::IngestedByUs,
            "owner {owner:?} must not read as a lender for an app we injected"
        );
    }
}

// Invariant 1 with the flag off: the pre-fix behaviour really does produce
// the wrong label, which is what makes the table meaningful.
#[test]
fn legacy_lender_validation_reproduces_the_family_label_bug() {
    let poisoned = signals(false, true, Some(NO_ACCOUNT), true);
    assert_eq!(
        classify(poisoned, SELF, FamilySettings::legacy()),
        Provenance::FamilyShared
    );
    assert_eq!(
        classify(poisoned, SELF, FamilySettings::default()),
        Provenance::IngestedByUs
    );
}

// Invariant 2: a real lender keeps the family label in both flag states,
// whether or not we also injected the app.
#[test]
fn a_genuine_lender_stays_family_shared_in_both_flag_states() {
    for injected in [false, true] {
        for settings in SETTINGS {
            assert_eq!(
                classify(signals(false, true, Some(LENDER), injected), SELF, settings),
                Provenance::FamilyShared,
                "injected={injected} settings={settings:?}"
            );
        }
    }
}

// A claim we cannot check is not a claim we may use for our own injection,
// but it must still be honoured for an app we did not touch.
#[test]
fn an_unnamed_owner_is_trusted_only_when_we_did_not_inject() {
    let settings = FamilySettings::default();
    assert_eq!(
        classify(signals(false, true, None, false), SELF, settings),
        Provenance::FamilyShared
    );
    assert_eq!(
        classify(signals(false, true, None, true), SELF, settings),
        Provenance::IngestedByUs
    );
}

#[test]
fn own_license_outranks_every_other_signal() {
    for settings in SETTINGS {
        assert_eq!(
            classify(signals(true, true, Some(LENDER), true), SELF, settings),
            Provenance::Owned
        );
    }
}

#[test]
fn no_title_and_no_injection_is_unowned() {
    for settings in SETTINGS {
        assert_eq!(
            classify(signals(false, false, None, false), SELF, settings),
            Provenance::Unowned
        );
    }
}

#[test]
fn lender_account_rejects_the_fabricated_values() {
    assert_eq!(lender_account(Some(LENDER), SELF), Some(LENDER));
    assert_eq!(lender_account(Some(0), SELF), None);
    assert_eq!(lender_account(Some(NO_ACCOUNT), SELF), None);
    assert_eq!(lender_account(Some(SELF), SELF), None);
    assert_eq!(lender_account(None, SELF), None);
    // With no known self account, every non-zero, non-sentinel owner is a
    // candidate lender rather than silently "us".
    assert_eq!(lender_account(Some(LENDER), 0), Some(LENDER));
}

/// `owned / family / ingested × flag on/off`: the label column is identical
/// in both flag states; only `lock_unlocked` moves.
#[test]
fn reported_provenance_table_holds_for_both_flag_states() {
    let app_id = AppId(480);
    let expected = [
        (Provenance::Unknown, ReportedProvenance::Unchanged),
        (Provenance::Owned, ReportedProvenance::Owned),
        (Provenance::FamilyShared, ReportedProvenance::FamilyShared),
        (Provenance::IngestedByUs, ReportedProvenance::Owned),
        (Provenance::Unowned, ReportedProvenance::NotInLibrary),
    ];
    for (real, label) in expected {
        for shared in [shared_off(), SharedSection::default()] {
            let flag_on = shared.enabled;
            let report = reported(real, &config(shared), app_id);
            assert_eq!(report.reported, label, "{real:?} flag_on={flag_on}");
            assert_eq!(report.lock_unlocked, flag_on, "{real:?} flag_on={flag_on}");
            assert!(!report.reason.is_empty());
        }
    }
}

// Invariant 1 at the reporting layer, end to end from signals.
#[test]
fn an_ingested_app_is_never_reported_as_family_shared() {
    let app_id = AppId(480);
    let settings = FamilySettings::default();
    for shared in [shared_off(), SharedSection::default()] {
        let real = classify(signals(false, true, Some(NO_ACCOUNT), true), SELF, settings);
        let report = reported(real, &config_controlling(&[480], shared), app_id);
        assert_eq!(report.reported, ReportedProvenance::Owned);
    }
}

// Invariant 2 at the reporting layer: unlocking the lock is not relabelling.
#[test]
fn unlocking_the_family_lock_does_not_relabel_a_borrowed_app() {
    let app_id = AppId(570);
    let real = classify(
        signals(false, true, Some(LENDER), false),
        SELF,
        FamilySettings::default(),
    );
    let locked = reported(real, &config(shared_off()), app_id);
    let unlocked = reported(real, &config(SharedSection::default()), app_id);
    assert_eq!(locked.reported, ReportedProvenance::FamilyShared);
    assert_eq!(unlocked.reported, ReportedProvenance::FamilyShared);
    assert!(!locked.lock_unlocked);
    assert!(unlocked.lock_unlocked);
}

#[test]
fn an_ingest_claim_the_config_dropped_stays_visible() {
    let app_id = AppId(480);
    // Not in `[apps.inject]`: the config no longer controls the app.
    let report = reported(
        Provenance::IngestedByUs,
        &config(SharedSection::default()),
        app_id,
    );
    assert_eq!(report.reported, ReportedProvenance::Owned);
}

#[test]
fn classification_is_deterministic() {
    let samples = [
        signals(false, true, Some(NO_ACCOUNT), true),
        signals(false, true, Some(LENDER), false),
        signals(true, false, None, false),
        signals(false, false, None, true),
    ];
    for sample in samples {
        for settings in SETTINGS {
            let first = classify(sample, SELF, settings);
            for _ in 0..8 {
                assert_eq!(classify(sample, SELF, settings), first);
            }
            // Every classification is one of the known provenances.
            assert!(Provenance::ALL.contains(&first));
        }
    }
}

#[test]
fn every_provenance_has_a_distinct_label() {
    let mut labels: Vec<&str> = Provenance::ALL.into_iter().map(Provenance::label).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), Provenance::ALL.len());
}
