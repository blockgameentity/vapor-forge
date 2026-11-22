//! The whole worker flow, driven through fakes.

use std::cell::RefCell;
use std::rc::Rc;

use super::{
    config_text, factory, write_corpus, FakeEnv, RecordingSink, TempDir, CLOUD, DEPOT, OWNERSHIP,
    TICKET,
};
use crate::capability::Capability;
use crate::entry::worker;

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
        ],
    );
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

    assert_eq!(outcome.modules.len(), 1);
    let module = &outcome.modules[0];
    assert!(module.error.is_none(), "{:?}", module.error);
    let report = module.report.as_ref().expect("a batch was planned");
    let installed: Vec<&str> = report.installed.iter().map(|i| i.symbol.as_str()).collect();
    assert_eq!(installed, vec![OWNERSHIP, DEPOT]);
    assert!(
        report.gate.is_enabled(Capability::Ownership),
        "an installed body keeps its capability"
    );
    assert!(
        report.gate.is_enabled(Capability::DepotInjection),
        "the depot body is implemented"
    );
    assert!(
        !report.gate.is_enabled(Capability::CloudControl),
        "a symbol without a body must disable its capability"
    );
    let reason = report
        .gate
        .reason(Capability::CloudControl)
        .expect("reason");
    assert!(reason.contains("no replacement body"), "{reason}");

    let text = outcome.report();
    assert!(text.contains("module steamclient64.dll:"));
    assert!(text.contains(OWNERSHIP));
    assert!(text.contains("no replacement body"));

    let state = sink.published.as_ref().expect("the state was published");
    assert!(state.allows(Capability::Ownership));
    assert!(!state.allows(Capability::CloudControl));
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

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

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

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

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

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

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

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

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

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

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

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

    assert!(outcome.modules[0].error.is_none());
    let state = sink.published.as_ref().expect("state");
    assert!(
        !state.family_settings().validate_lender,
        "an off flag must reach the hook bodies, not just the feature report"
    );
    assert!(!state.family_settings().clean_borrower_markers);
}

/// Lua is how the host tool expresses per-app intent, so the worker must load it
/// before publishing the state: an app that only exists in a script has to end up
/// controlled, exactly as it does on the Linux host.
#[test]
fn scripts_from_config_lua_reach_the_published_state() {
    let dir = TempDir::new("lua-ingest");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD")]);

    let lua_dir = dir.path().join("config/lua");
    std::fs::create_dir_all(&lua_dir).expect("lua dir");
    std::fs::write(lua_dir.join("480.lua"), "addappid(480)\n").expect("lua script");

    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    env.lua = vec![lua_dir.display().to_string()];
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

    let text = outcome.report();
    assert!(text.contains("lua: 1 dir(s)"), "{text}");
    assert!(!text.contains("lua: 0 dir(s)"), "{text}");

    let state = sink.published.as_ref().expect("state published");
    let controlled: Vec<u32> = state.controlled_apps().iter().map(|app| app.0).collect();
    assert!(
        controlled.contains(&480),
        "the script's app must be controlled, got {controlled:?}"
    );
    assert!(
        controlled.contains(&730),
        "the config's app must stay controlled, got {controlled:?}"
    );
}

/// A machine with no `config/lua` must be a note, never an error.
#[test]
fn a_missing_lua_directory_is_not_a_failure() {
    let dir = TempDir::new("no-lua");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD")]);

    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let mut installer = factory(&Rc::new(RefCell::new(Vec::new())), None);
    let mut sink = RecordingSink::default();

    let outcome = worker::run(&mut env, &mut installer, &mut sink);
    assert_eq!(outcome.failed_modules(), 0);
    assert!(outcome.report().contains("lua: 0 dir(s)"));
}

/// The ticket body used to be uninstallable because no corpus entry existed for
/// `CUser::GetAppTicket`. With the anchor now mined for both live builds, a
/// corpus that carries it must install the body and keep its capability.
#[test]
fn the_ticket_body_installs_now_that_its_symbol_is_anchored() {
    let dir = TempDir::new("ticket-anchor");
    let config = dir.write("config.toml", &config_text(&[730]));
    let corpus_root = dir.path().join("corpus");
    write_corpus(&corpus_root, &[(OWNERSHIP, "AA BB CC DD"), (TICKET, "DE AD BE EF")]);
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = worker::run(&mut env, &mut installer, &mut sink);

    let report = outcome.modules[0].report.as_ref().expect("batch planned");
    let installed: Vec<&str> = report.installed.iter().map(|i| i.symbol.as_str()).collect();
    assert!(installed.contains(&TICKET), "installed {installed:?}");
    assert!(
        report.gate.is_enabled(Capability::TicketOverrides),
        "reason: {:?}",
        report.gate.reason(Capability::TicketOverrides)
    );
}
