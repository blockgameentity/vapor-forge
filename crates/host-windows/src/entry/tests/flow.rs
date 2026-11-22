//! The whole worker flow, driven through fakes.

use std::cell::RefCell;
use std::rc::Rc;

use super::{
    config_text, factory, run_serialized, write_corpus, FakeEnv, RecordingSink, TempDir,
    ACHIEVEMENTS, CLOUD, DEPOT, KEYVALUES, OWNERSHIP,
};
use crate::capability::Capability;
use crate::hooks;

#[test]
fn a_loaded_module_installs_the_bodies_the_host_implements() {
    let dir = TempDir::new("full-run");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(
        &corpus_root,
        &[
            (OWNERSHIP, "AA BB CC DD"),
            (DEPOT, "11 22 33 44"),
            (CLOUD, "55 66 77 88"),
            // A symbol no capability depends on and this host has no body for:
            // the "not written yet" path stays covered here.
            (KEYVALUES, "99 AA BB CC"),
        ],
    );
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    assert_eq!(outcome.modules.len(), 1);
    let module = &outcome.modules[0];
    assert!(module.error.is_none(), "{:?}", module.error);
    let report = module.report.as_ref().expect("a batch was planned");
    let installed: Vec<&str> = report.installed.iter().map(|i| i.symbol.as_str()).collect();
    assert_eq!(installed, vec![OWNERSHIP, DEPOT, CLOUD]);
    assert!(
        report.gate.is_enabled(Capability::Ownership),
        "an installed body keeps its capability"
    );
    assert!(
        report.gate.is_enabled(Capability::DepotInjection),
        "the depot body is implemented"
    );
    assert!(
        report.gate.is_enabled(Capability::CloudControl)
            && report.gate.is_enabled(Capability::CloudHttp),
        "the cloud body is implemented, so both cloud capabilities survive"
    );
    assert!(
        !report.installed.iter().any(|i| i.symbol == KEYVALUES),
        "a symbol with no body must not be claimed as installed"
    );

    let text = outcome.report();
    assert!(text.contains("module steamclient64.dll:"));
    assert!(text.contains(OWNERSHIP));
    assert!(
        text.contains("no replacement body"),
        "a resolved symbol without a body is reported, not installed"
    );

    let state = sink.published.as_ref().expect("the state was published");
    assert!(state.allows(Capability::Ownership));
    assert!(state.allows(Capability::CloudControl));
    assert_eq!(state.controlled_apps().len(), 1);
    assert_eq!(state.controlled_apps()[0].0, 730);
}

#[test]
fn a_missing_corpus_disables_every_capability_and_still_publishes_the_state() {
    let dir = TempDir::new("no-corpus");
    let config = dir.write("config.toml", &config_text(&[730]));
    let mut env = FakeEnv::new(Some(config), Some(dir.path().join("absent")));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    assert_eq!(outcome.failed_modules(), 1);
    for capability in Capability::ALL {
        assert!(
            !outcome.gate.is_enabled(capability),
            "{capability} must be disabled with the batch"
        );
    }
    assert!(outcome.report().contains("no pattern corpus for"));
    assert!(sink.published.is_some(), "hooks still see a host state");
}

#[test]
fn no_loaded_module_is_a_note_not_a_failure() {
    let dir = TempDir::new("no-module");
    let config = dir.write("config.toml", &config_text(&[]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD")]);
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    env.modules.clear();
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    assert!(outcome.modules.is_empty());
    assert_eq!(outcome.failed_modules(), 0);
    assert!(outcome
        .notes
        .iter()
        .any(|note| note.contains("no Steam module")));
}

#[test]
fn an_unloadable_config_leaves_every_hook_falling_through() {
    let dir = TempDir::new("bad-config");
    let config = dir.write("config.toml", "this is not toml = = =");
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD")]);
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    assert!(outcome
        .notes
        .iter()
        .any(|note| note.contains("every hook falls through")));
    let state = sink.published.as_ref().expect("state");
    assert!(
        state.controlled_apps().is_empty(),
        "a default config controls nothing, so no hook rewrites anything"
    );
}

#[test]
fn a_publish_failure_is_reported_and_the_install_pass_still_runs() {
    let dir = TempDir::new("sink-failure");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD")]);
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink {
        published: None,
        failure: Some("already published".to_owned()),
    };

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    assert!(outcome
        .notes
        .iter()
        .any(|note| note.contains("already published")));
    assert_eq!(
        outcome.modules[0]
            .report
            .as_ref()
            .expect("report")
            .installed
            .len(),
        1
    );
}

#[test]
fn a_failing_install_disables_only_that_symbols_capability() {
    let dir = TempDir::new("install-failure");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(
        &corpus_root,
        &[(OWNERSHIP, "AA BB CC DD"), (DEPOT, "11 22 33 44")],
    );
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, Some(DEPOT));
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    let report = outcome.modules[0].report.as_ref().expect("report");
    assert_eq!(report.installed.len(), 1);
    assert_eq!(report.installed[0].symbol, OWNERSHIP);
    assert!(report.gate.is_enabled(Capability::Ownership));
    assert!(!report.gate.is_enabled(Capability::DepotInjection));
}

#[test]
fn the_config_flags_reach_the_hook_state() {
    let dir = TempDir::new("flags");
    let config = dir.write(
        "config.toml",
        "[[apps.inject]]\nid = 730\n\n[apps.shared]\nvalidate_lender = false\n\
         clean_borrower_markers = false\n",
    );
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD")]);
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    assert!(outcome.modules[0].error.is_none());
    let state = sink.published.as_ref().expect("state");
    assert!(
        !state.family_settings().validate_lender,
        "an off flag must reach the hook bodies, not just the feature report"
    );
    assert!(!state.family_settings().clean_borrower_markers);
}

/// A corpus that anchors the stats job resolves it, and the capability follows the
/// body table rather than the resolver.
///
/// This is the achievements flow in one place: `CAPIJobRequestUserStats::BInit` and
/// `::YieldingMergeStats` are anchored in both live builds and listed in
/// `install::deps`, but no replacement body exists (`crate::abi::achievements`
/// measures why). So today the batch resolves the symbol, declines to install it,
/// and reports `achievements` disabled with the reason naming the symbol — every
/// other capability is untouched. The assertion is written against
/// [`hooks::replacement_address`] on purpose: the day a body lands, this test
/// asserts the other half (installed, capability kept) without an edit.
#[test]
fn the_stats_job_anchor_resolves_and_achievements_follows_its_body() {
    let dir = TempDir::new("achievements");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(
        &corpus_root,
        &[(OWNERSHIP, "AA BB CC DD"), (ACHIEVEMENTS, "DE AD BE EF")],
    );
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    let report = outcome.modules[0].report.as_ref().expect("report");
    assert!(
        outcome.modules[0].error.is_none(),
        "{:?}",
        outcome.modules[0].error
    );
    let has_body = hooks::replacement_address(ACHIEVEMENTS).is_some();
    assert_eq!(
        report
            .installed
            .iter()
            .any(|entry| entry.symbol == ACHIEVEMENTS),
        has_body,
        "the stats job symbol is installed exactly when it has a body"
    );
    assert_eq!(
        report.gate.is_enabled(Capability::Achievements),
        has_body,
        "achievements is enabled exactly when the stats job has a body"
    );
    if !has_body {
        let reason = report
            .gate
            .reason(Capability::Achievements)
            .expect("the capability must name the symbol it lost");
        assert!(
            reason.contains(ACHIEVEMENTS) && reason.contains("no replacement body"),
            "unexpected reason: {reason}"
        );
        assert!(
            outcome.report().contains(ACHIEVEMENTS),
            "the report must name the symbol it did not install"
        );
    }
    // One unresolved capability never costs another: the ownership body installed
    // from the same corpus and keeps its gate open.
    assert!(report.gate.is_enabled(Capability::Ownership));
    assert_eq!(report.installed.len(), usize::from(has_body) + 1);
    assert!(
        sink.published.is_some(),
        "the state is published regardless"
    );
}
