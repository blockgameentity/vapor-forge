use super::*;
use crate::valve_filter::bypasses_family_lock;
use vapor_forge_config::{AppsSection, InjectApp, SharedSection};

const LENDER: u32 = 9_999_999;

fn config(inject: &[u32], shared: SharedSection) -> RuntimeConfig {
    let mut config = RuntimeConfig {
        apps: AppsSection::with_inject(
            inject
                .iter()
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

fn shared(enabled: bool, include: &[u32], exclude: &[u32]) -> SharedSection {
    SharedSection {
        enabled,
        include: include.iter().copied().map(AppId).collect(),
        exclude: exclude.iter().copied().map(AppId).collect(),
    }
}

/// One row per interesting policy state: name, config, app, allowed, reason.
fn rows() -> Vec<(&'static str, RuntimeConfig, AppId, bool, PlayReason)> {
    let off = || shared(false, &[], &[]);
    vec![
        (
            "flag off, app not controlled",
            config(&[480], off()),
            AppId(570),
            false,
            PlayReason::SharingDisabled,
        ),
        (
            "flag off, app controlled",
            config(&[480], off()),
            AppId(480),
            true,
            PlayReason::ControlledApp,
        ),
        (
            "flag on, app not controlled",
            config(&[480], shared(true, &[], &[])),
            AppId(570),
            true,
            PlayReason::UnlockConfigured,
        ),
        (
            "flag on, app controlled",
            config(&[480], shared(true, &[], &[])),
            AppId(480),
            true,
            PlayReason::ControlledApp,
        ),
        (
            "include covers the app",
            config(&[480], shared(true, &[570], &[])),
            AppId(570),
            true,
            PlayReason::UnlockConfigured,
        ),
        (
            "include misses the app",
            config(&[480], shared(true, &[730], &[])),
            AppId(570),
            false,
            PlayReason::PolicyRefuses,
        ),
        (
            "exclude covers the app",
            config(&[480], shared(true, &[], &[570])),
            AppId(570),
            false,
            PlayReason::PolicyRefuses,
        ),
        (
            "exclude misses the app",
            config(&[480], shared(true, &[], &[730])),
            AppId(570),
            true,
            PlayReason::UnlockConfigured,
        ),
    ]
}

#[test]
fn concurrent_play_decision_matrix() {
    for (name, config, app_id, allowed, reason) in rows() {
        let decision = decide(&config, app_id, FamilyLock::held_by(Some(LENDER)));
        assert_eq!(decision.allowed, allowed, "{name}");
        assert_eq!(decision.reason, reason, "{name}");
    }
}

#[test]
fn no_lock_always_allows_and_says_so() {
    for (name, config, app_id, _, _) in rows() {
        let decision = decide(&config, app_id, FamilyLock::free());
        assert!(decision.allowed, "{name}");
        assert_eq!(decision.reason, PlayReason::NoFamilyLock, "{name}");
    }
}

// "Flag off -> current behaviour": flag off allows exactly the apps our own
// injection covers, flag on allows exactly what `bypasses_family_lock` says.
// This change adds no allow and no deny beyond the policy itself.
#[test]
fn the_decision_never_disagrees_with_the_shipped_bypass() {
    let policies = [
        shared(false, &[], &[]),
        shared(true, &[], &[]),
        shared(true, &[570], &[]),
        shared(true, &[], &[570]),
    ];
    for shared in policies {
        for inject in [&[480u32][..], &[570], &[]] {
            let config = config(inject, shared.clone());
            for id in [480u32, 570, 730] {
                let app_id = AppId(id);
                let decision = decide(&config, app_id, FamilyLock::held_by(None));
                let expected = if shared.enabled {
                    bypasses_family_lock(&config, app_id)
                } else {
                    config.is_controlled_app(app_id)
                };
                assert_eq!(decision.allowed, expected, "app={id} shared={shared:?}");
            }
        }
    }
}

#[test]
fn a_denied_decision_names_the_policy_that_refused_it() {
    let disabled = decide(
        &config(&[], shared(false, &[], &[])),
        AppId(570),
        FamilyLock::held_by(None),
    );
    assert_eq!(disabled.reason, PlayReason::SharingDisabled);
    let refused = decide(
        &config(&[], shared(true, &[730], &[])),
        AppId(570),
        FamilyLock::held_by(None),
    );
    assert_eq!(refused.reason, PlayReason::PolicyRefuses);
    assert_ne!(disabled.reason, refused.reason);
}

#[test]
fn every_reason_has_a_distinct_label() {
    let reasons = [
        PlayReason::NoFamilyLock,
        PlayReason::ControlledApp,
        PlayReason::UnlockConfigured,
        PlayReason::SharingDisabled,
        PlayReason::PolicyRefuses,
    ];
    let mut labels: Vec<&str> = reasons.iter().map(|reason| reason.label()).collect();
    labels.sort_unstable();
    labels.dedup();
    assert_eq!(labels.len(), reasons.len());
}

#[test]
fn the_decision_is_deterministic_and_logging_changes_nothing() {
    let config = config(&[480], shared(true, &[], &[]));
    let lock = FamilyLock::held_by(Some(LENDER));
    let first = decide(&config, AppId(570), lock);
    for _ in 0..8 {
        assert_eq!(decide(&config, AppId(570), lock), first);
    }
    first.log(AppId(570), lock, Provenance::FamilyShared);
    assert_eq!(decide(&config, AppId(570), lock), first);
}
