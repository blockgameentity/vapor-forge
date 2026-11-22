use super::*;

fn controlled() -> Vec<AppId> {
    vec![AppId(730), AppId(440)]
}

#[test]
fn disabled_backend_leaves_steam_alone() {
    let plan = plan_cloud(&CloudSettings {
        backend: CloudBackend::Disabled,
        controlled_apps: controlled(),
    });
    assert_eq!(plan.mode, CloudMode::LeaveAlone);
    assert!(plan.reason.is_some());
    assert!(plan.redirected_apps().is_empty());
}

#[test]
fn a_configured_backend_redirects_every_controlled_app() {
    let plan = plan_cloud(&CloudSettings {
        backend: CloudBackend::Cumulus {
            endpoint: Some("https://cloud.example".to_owned()),
            credentials_present: true,
        },
        controlled_apps: controlled(),
    });
    assert_eq!(plan.mode, CloudMode::Redirect);
    assert_eq!(plan.redirected_apps(), controlled());
    assert!(plan.decision_for(AppId(730)).is_redirect());
    assert!(plan.summary().contains("https://cloud.example"));
    assert!(!plan.summary().contains("token"));
}

#[test]
fn missing_credentials_refuse_instead_of_redirecting() {
    let plan = plan_cloud(&CloudSettings {
        backend: CloudBackend::Cumulus {
            endpoint: Some("https://cloud.example".to_owned()),
            credentials_present: false,
        },
        controlled_apps: controlled(),
    });
    assert_eq!(plan.mode, CloudMode::Refuse);
    assert!(plan.is_refused());
    assert!(plan.redirected_apps().is_empty());
    let reason = plan.reason.as_deref().expect("refusal names a reason");
    assert!(reason.contains("token"), "reason was {reason}");
    assert!(matches!(
        plan.decision_for(AppId(730)),
        CloudDecision::Refuse { .. }
    ));
}

#[test]
fn apps_outside_the_controlled_set_stay_with_steam() {
    let plan = plan_cloud(&CloudSettings {
        backend: CloudBackend::Local {
            root: Some("/srv/vf-cloud".to_owned()),
        },
        controlled_apps: controlled(),
    });
    assert!(plan.decision_for(AppId(10)).reason().is_some());
    assert!(!plan.decision_for(AppId(10)).is_redirect());
}

#[test]
fn local_backend_without_a_folder_refuses() {
    let plan = plan_cloud(&CloudSettings {
        backend: CloudBackend::Local { root: None },
        controlled_apps: controlled(),
    });
    assert_eq!(plan.mode, CloudMode::Refuse);
    assert!(plan.summary().contains("watch folder"));
}
