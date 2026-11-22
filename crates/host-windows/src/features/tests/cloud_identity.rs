use super::*;

#[test]
fn a_controlled_app_without_credentials_is_refused_not_synced() {
    let _guard = serial();
    let settings = HostSettings {
        cloud: configured_cloud(false),
        ..HostSettings::none()
    };
    let report = run(&settings, &Gate::all_enabled(), None);

    assert_eq!(report.cloud.mode, CloudMode::Refuse);
    assert!(
        report.cloud.redirected_apps().is_empty(),
        "no silent redirect"
    );
    for app in [AppId(730), AppId(440)] {
        match report.cloud.decision_for(app) {
            CloudDecision::Refuse { reason, .. } => {
                assert!(reason.contains("token"), "reason was {reason}");
            }
            other => panic!("expected a refusal for {app}, got {other:?}"),
        }
    }
    let state = state_of(&report, HostFeature::Cloud);
    assert!(!state.is_active());
    assert!(state
        .reason()
        .is_some_and(|reason| reason.contains("refus")));
    assert!(report
        .gate()
        .reason(Capability::CloudControl)
        .is_some_and(|reason| reason.contains("token")));
}

#[test]
fn a_configured_backend_redirects_every_controlled_app() {
    let _guard = serial();
    let settings = HostSettings {
        cloud: configured_cloud(true),
        ..HostSettings::none()
    };
    let report = run(&settings, &Gate::all_enabled(), None);

    assert_eq!(report.cloud.mode, CloudMode::Redirect);
    assert_eq!(report.cloud.redirected_apps(), [AppId(730), AppId(440)]);
    assert!(report.is_active(HostFeature::Cloud));
    assert!(report.gate().is_enabled(Capability::CloudControl));
    assert!(matches!(
        report.cloud.decision_for(AppId(730)),
        CloudDecision::Redirect { .. }
    ));
    // An app the host does not control keeps Steam's own cloud.
    assert!(!report.cloud.decision_for(AppId(10)).is_redirect());
}

#[test]
fn without_controlled_apps_steam_keeps_its_cloud() {
    let _guard = serial();
    let settings = HostSettings {
        cloud: CloudSettings {
            backend: CloudBackend::Local {
                root: Some("/srv/vf-cloud".to_owned()),
            },
            controlled_apps: Vec::new(),
        },
        ..HostSettings::none()
    };
    let report = run(&settings, &Gate::all_enabled(), None);
    assert_eq!(report.cloud.mode, CloudMode::LeaveAlone);
    assert!(report.cloud.redirected_apps().is_empty());
    assert!(!report.is_active(HostFeature::Cloud));
}

#[test]
fn an_absent_identity_override_stays_an_explicit_no_override() {
    let _guard = serial();
    let report = run(&HostSettings::none(), &Gate::all_enabled(), None);

    assert!(report.identities.is_empty(), "nothing was configured");
    let state = state_of(&report, HostFeature::Identity);
    assert!(!state.is_active());
    assert!(state
        .reason()
        .is_some_and(|reason| reason.contains("no per-game SteamID override")));

    // The per-app selection API has the same explicit state.
    let selection = crate::features::select_identity(&IdentitySettings::default(), AppId(730));
    assert_eq!(
        selection,
        IdentitySelection::NoOverride { app_id: AppId(730) }
    );
}

#[test]
fn an_unusable_identity_override_is_refused_and_none_is_invented() {
    let _guard = serial();
    let mut overrides = BTreeMap::new();
    overrides.insert(AppId(730), 0x0000_0001_0000_0001);
    let settings = HostSettings {
        identity: IdentitySettings { overrides },
        ..HostSettings::none()
    };
    let report = run(&settings, &Gate::all_enabled(), None);

    assert_eq!(report.identities.len(), 1);
    assert_eq!(report.identities[0].steam_id64(), None, "no id is invented");
    assert!(report.identities[0].reason().is_some());
    assert!(!report.is_active(HostFeature::Identity));
    assert!(report
        .gate()
        .reason(Capability::Ownership)
        .is_some_and(|reason| reason.contains("not an individual account id")));
}

#[test]
fn a_valid_identity_override_is_reported_active() {
    let _guard = serial();
    let mut overrides = BTreeMap::new();
    overrides.insert(AppId(730), 76561198106179127);
    let settings = HostSettings {
        identity: IdentitySettings { overrides },
        ..HostSettings::none()
    };
    let report = run(&settings, &Gate::all_enabled(), None);
    assert!(report.is_active(HostFeature::Identity));
    assert_eq!(report.identities[0].steam_id64(), Some(76561198106179127));
    assert!(report.gate().is_enabled(Capability::Ownership));
}

#[test]
fn settings_resolved_from_config_reach_the_cloud_decision() {
    let _guard = serial();
    let mut config = vapor_forge_config::RuntimeConfig::default();
    config.cloud.backend = vapor_forge_config::CloudBackendMode::Cumulus;
    config.cloud.cumulus.server_url = "https://cloud.example".to_owned();
    config.cloud.cumulus.token = "secret-token".to_owned();
    config.apps.push_inject(vapor_forge_config::InjectApp {
        id: AppId(730),
        dlc: Vec::new(),
        ticket: Default::default(),
        purchase_time: 0,
    });
    let settings = HostSettings::resolve(&config, &vapor_forge_core::ScriptState::default());
    let report = run(&settings, &Gate::all_enabled(), None);

    assert_eq!(report.cloud.mode, CloudMode::Redirect);
    assert!(report.is_active(HostFeature::Cloud));
    assert_eq!(report.cloud.redirected_apps(), [AppId(730)]);
    // The token is never rendered anywhere in the report.
    assert!(!report.report().contains("secret-token"));
}
