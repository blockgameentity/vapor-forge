//! Lua ingest and the ticket anchor, driven through fakes.
//!
//! Split from [`super::flow`] when that file crossed the 300-line budget: both
//! halves drive the same worker, and the split is by subject, not by fixture.

use std::cell::RefCell;
use std::rc::Rc;

use super::{
    config_text, factory, run_serialized, write_corpus, FakeEnv, RecordingSink, TempDir, OWNERSHIP,
    TICKET,
};
use crate::capability::Capability;

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

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

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

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);
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
    write_corpus(
        &corpus_root,
        &[(OWNERSHIP, "AA BB CC DD"), (TICKET, "DE AD BE EF")],
    );
    let mut env = FakeEnv::new(Some(config), Some(corpus_root));
    let attempts = Rc::new(RefCell::new(Vec::new()));
    let mut installer = factory(&attempts, None);
    let mut sink = RecordingSink::default();

    let outcome = run_serialized(&mut env, &mut installer, &mut sink);

    let report = outcome.modules[0].report.as_ref().expect("batch planned");
    let installed: Vec<&str> = report.installed.iter().map(|i| i.symbol.as_str()).collect();
    assert!(installed.contains(&TICKET), "installed {installed:?}");
    assert!(
        report.gate.is_enabled(Capability::TicketOverrides),
        "reason: {:?}",
        report.gate.reason(Capability::TicketOverrides)
    );
}
