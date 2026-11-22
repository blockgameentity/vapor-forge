use super::*;

#[test]
fn every_inactive_feature_is_listed_with_its_reason() {
    let _guard = serial();
    let report = run(&HostSettings::none(), &Gate::all_enabled(), None);

    assert_eq!(report.entries().len(), HostFeature::ALL.len());
    assert_eq!(report.gate().disabled().count(), HostFeature::ALL.len());
    let text = report.report();
    for feature in HostFeature::ALL {
        let state = state_of(&report, feature);
        assert!(!state.is_active(), "{feature:?} must be inactive");
        let reason = state.reason().expect("an inactive feature says why");
        assert!(
            text.contains(reason),
            "report must carry the reason for {feature:?}: {text}"
        );
        assert_eq!(report.gate().reason(feature.capability()), Some(reason));
    }
    assert!(text.contains("cloud-control"));
    assert!(text.contains("per-game-identity"));
    assert!(text.contains("depot-cache-preseed"));
    assert!(text.contains("achievements"));
}

#[test]
fn nothing_runs_when_the_capabilities_are_disabled() {
    let _guard = serial();
    let mut gate = Gate::all_enabled();
    gate.disable(Capability::CloudControl, "CloseAppCloud did not resolve");
    gate.disable(
        Capability::DepotInjection,
        "GetDecryptionKey did not resolve",
    );
    gate.disable(
        Capability::Ownership,
        "CUser::CheckAppOwnership did not resolve",
    );
    gate.disable(
        Capability::Achievements,
        "CUser::GetUserStats did not resolve",
    );

    let mut overrides = BTreeMap::new();
    overrides.insert(AppId(730), 76561198106179127);
    let settings = HostSettings {
        cloud: configured_cloud(true),
        depotcache: crate::features::DepotCacheSettings {
            wanted: vec![manifest(10, 100), manifest(11, 101)],
        },
        identity: IdentitySettings { overrides },
        achievements: AchievementSettings {
            local_rebuild: true,
        },
    };

    let mut store = PanicStore;
    let mut fetcher = PanicFetcher;
    let report = run(
        &settings,
        &gate,
        Some(DepotCachePorts {
            store: &mut store,
            fetcher: &mut fetcher,
        }),
    );

    // No archive dial, no disk touch (the fakes would have panicked), no
    // identity selection, and the cloud plan degraded to "leave alone".
    assert!(report.depotcache.is_empty());
    assert!(report.identities.is_empty());
    assert_eq!(report.cloud.mode, CloudMode::LeaveAlone);

    for feature in HostFeature::ALL {
        let reason = gate.reason(feature.capability()).expect("gate reason");
        assert_eq!(state_of(&report, feature).reason(), Some(reason));
    }
}

#[test]
fn the_local_rebuild_flag_is_surfaced_and_only_applied_when_enabled() {
    let _guard = serial();
    // Enabled capability + flag on: the host flips the process-wide switch and
    // reports the feature active.
    let settings = HostSettings {
        achievements: AchievementSettings {
            local_rebuild: true,
        },
        ..HostSettings::none()
    };
    let report = run(&settings, &Gate::all_enabled(), None);
    assert!(report.is_active(HostFeature::Achievements));
    assert!(
        local_rebuild_enabled(),
        "the host must wire the flag through"
    );

    // Disabled capability + flag off: no work, and the switch is left alone.
    let mut gate = Gate::all_enabled();
    gate.disable(
        Capability::Achievements,
        "CUser::GetUserStats did not resolve",
    );
    let report = run(&HostSettings::none(), &gate, None);
    assert_eq!(
        state_of(&report, HostFeature::Achievements).reason(),
        Some("CUser::GetUserStats did not resolve")
    );
    assert!(
        local_rebuild_enabled(),
        "a disabled capability must not touch the achievement switch"
    );
}

#[test]
fn the_run_report_carries_the_feature_half() {
    let _guard = serial();
    let features = run(&HostSettings::none(), &Gate::all_enabled(), None);
    let report = RunReport {
        fingerprint: "steamclient-abc".to_owned(),
        plan: None,
        gate: Gate::all_enabled(),
        installed: Vec::new(),
        failures: Vec::new(),
        features: None,
    }
    .with_features(features);
    let text = report.report();
    assert!(text.contains("Windows host features:"));
    assert!(text.contains("cloud-control: inactive"));
    assert!(text.contains("depot-cache-preseed: inactive"));
}

#[test]
fn a_default_cloud_plan_never_claims_a_redirect() {
    let plan = crate::features::plan_cloud(&CloudSettings::default());
    assert_eq!(plan.mode, CloudMode::LeaveAlone);
    assert!(plan.redirected_apps().is_empty());
    assert!(plan.reason.is_some(), "an inactive feature always says why");
    assert!(plan.summary().contains("leave alone"));
    assert!(!CloudPlan::default().is_refused());
    assert!(!CloudPlan::default().decision_for(AppId(1)).is_redirect());
}
