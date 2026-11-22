use super::*;
use vapor_forge_config::{DepotId, ManifestId, ManifestResponseFormat, RuntimeConfig};

fn provider(name: &str, url: &str) -> ManifestSource {
    ManifestSource {
        name: name.to_owned(),
        url: url.to_owned(),
        user_agent: None,
        format: ManifestResponseFormat::Plain,
        json_field: None,
        min_interval_ms: None,
    }
}

fn wanted(depot: u32, gid: u64) -> Candidate {
    Candidate {
        depot_id: DepotId(depot),
        manifest_id: ManifestId(gid),
    }
}

#[test]
fn an_empty_provider_list_says_so_instead_of_inventing_a_host() {
    let config = RuntimeConfig::default();
    let plan = plan(&config, &[wanted(731, 9)], &[]);
    assert_eq!(
        plan.provider_choice,
        ProviderChoice::NoProviderConfigured
    );
    assert_eq!(plan.targets.len(), 1);
    assert!(plan.summary().contains("nothing would be asked"));
}

#[test]
fn failover_off_names_only_the_head_of_the_list() {
    let mut config = RuntimeConfig::default();
    config.manifest.providers = vec![
        provider("first", "https://a.example/{gid}"),
        provider("second", "https://b.example/{gid}"),
    ];
    config.manifest.failover = false;
    assert_eq!(
        plan(&config, &[], &[]).provider_choice,
        ProviderChoice::WouldAsk {
            provider: "first".to_owned(),
            skipped: Vec::new(),
        }
    );
}

#[test]
fn failover_on_skips_unusable_entries_and_names_the_first_usable_one() {
    let mut config = RuntimeConfig::default();
    config.manifest.providers = vec![
        // A url with no `{gid}` placeholder is rejected by config parsing.
        provider("broken", "https://a.example/no-placeholder"),
        provider("second", "https://b.example/{gid}"),
    ];
    config.manifest.failover = true;
    let choice = plan(&config, &[], &[]).provider_choice;
    let ProviderChoice::WouldAsk { provider, skipped } = &choice else {
        panic!("a later usable entry must be named: {choice:?}");
    };
    assert_eq!(provider, "second");
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0].starts_with("broken ("));
    assert!(choice.summary().contains("`second` first"));
}

#[test]
fn a_chain_of_unusable_entries_reports_no_usable_provider() {
    let mut config = RuntimeConfig::default();
    config.manifest.providers = vec![
        provider("broken-a", "https://a.example/no-placeholder"),
        provider("broken-b", "https://b.example/no-placeholder"),
    ];
    config.manifest.failover = true;
    assert_eq!(
        plan(&config, &[], &[]).provider_choice,
        ProviderChoice::NoUsableProvider {
            rejected: vec!["broken-a".to_owned(), "broken-b".to_owned()],
        }
    );

    // Failover off with a rejected head: the same "nothing usable" answer,
    // because the head is the only entry the chain would try.
    config.manifest.failover = false;
    assert_eq!(
        plan(&config, &[], &[]).provider_choice,
        ProviderChoice::NoUsableProvider {
            rejected: vec!["broken-a".to_owned()],
        }
    );
}

#[test]
fn the_ownership_gate_runs_the_real_request_code_decision() {
    // A controlled app needs an injected code; an app the config does not
    // control is never asked about.
    let config = crate::hooks::tests_support::config_with(&[480]);
    // The walked list is the host's effective set, so `apps_checked` counts it
    // whole even when one id is not in the config's control list: "1 of 2" has
    // to stay distinguishable from "0 of 2".
    let controlled = plan(&config, &[], &[AppId(480), AppId(999)]);
    assert_eq!(controlled.apps_checked, 2);
    assert_eq!(controlled.apps_needing_codes, vec![AppId(480)]);

    let uncontrolled = plan(&RuntimeConfig::default(), &[], &[AppId(480)]);
    assert!(uncontrolled.apps_needing_codes.is_empty());
    assert_eq!(uncontrolled.apps_checked, 1);
    assert!(uncontrolled.targets.is_empty());
}

#[test]
fn wanted_candidates_become_targets_without_expanding_a_url() {
    let mut config = RuntimeConfig::default();
    config.manifest.providers = vec![provider("a", "https://a.example/{depotId}/{gid}")];
    let plan = plan(&config, &[wanted(731, 0xfe)], &[]);
    assert_eq!(
        plan.targets,
        vec![RequestTarget {
            depot: 731,
            gid: 0xfe
        }]
    );
    // The endpoint never reaches the summary: a provider url can carry a
    // credential, and this line is logged.
    assert!(!plan.summary().contains("example"));
}
