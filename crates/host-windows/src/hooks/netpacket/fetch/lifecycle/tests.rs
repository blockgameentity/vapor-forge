//! The start decision, and the reasons it stays idle.

use super::*;
use crate::hooks::tests_support::{config_with, state_with};
use vapor_forge_config::ManifestSource;

fn provider(name: &str) -> ManifestSource {
    ManifestSource {
        name: name.to_owned(),
        url: "https://providers.invalid/manifest/{depotId}/{gid}".to_owned(),
        user_agent: None,
        format: Default::default(),
        json_field: None,
        min_interval_ms: None,
    }
}

/// A fetcher that panics if it is ever asked: the point of an idle fetcher is
/// that no round trip happens at all.
struct NeverAsked;

impl Fetcher for NeverAsked {
    fn fetch(&mut self, request: super::super::Request) -> super::super::Outcome {
        panic!("a fetch was attempted with {request:?} although no provider is configured");
    }
}

fn running_state() -> Arc<HostState> {
    Arc::new(state_with(config_with(&[480]), Vec::new()))
}

#[test]
fn an_empty_provider_list_idles_and_says_so_once() {
    let state = running_state();
    let manifest = ManifestSection::default();
    assert_eq!(
        start_with(&state, &manifest, Box::new(NeverAsked)),
        Start::Idle(NO_PROVIDER)
    );
    assert_eq!(state.code_fetch().lifecycle(), Lifecycle::Idle(NO_PROVIDER));
    assert!(
        !state.code_fetch().queue().open_now(),
        "an idle fetcher must refuse work, not queue it forever"
    );
    // The second start is a no-op, so the reason is not logged twice either.
    assert_eq!(
        start_with(&state, &manifest, Box::new(NeverAsked)),
        Start::AlreadyStarted
    );
}

#[test]
fn a_disabled_capability_idles_for_its_own_reason() {
    let state = running_state();
    let mut gate = state.gate();
    gate.disable(Capability::RequestCodes, "the layout was never decoded");
    state.set_gate(gate);
    let manifest = ManifestSection {
        providers: vec![provider("a.host")],
        ..ManifestSection::default()
    };
    assert_eq!(
        start_with(&state, &manifest, Box::new(NeverAsked)),
        Start::Idle(NO_CAPABILITY)
    );
}

#[test]
fn a_provider_list_whose_entries_are_all_unusable_idles() {
    let state = running_state();
    let manifest = ManifestSection {
        // No `{gid}` placeholder: the chain would reject it, so there is
        // nothing this host could ask.
        providers: vec![ManifestSource {
            url: "https://providers.invalid/manifest".to_owned(),
            ..provider("a.host")
        }],
        ..ManifestSection::default()
    };
    assert_eq!(manifest.rejections().len(), 1);
    assert_eq!(
        start_with(&state, &manifest, Box::new(NeverAsked)),
        Start::Idle(NO_USABLE_PROVIDER)
    );
}

#[test]
fn a_configured_provider_starts_the_worker_and_refuses_a_second_start() {
    let state = running_state();
    let manifest = ManifestSection {
        providers: vec![provider("a.host")],
        ..ManifestSection::default()
    };
    assert_eq!(
        start_with(&state, &manifest, Box::new(NeverAsked)),
        Start::Running
    );
    assert_eq!(state.code_fetch().lifecycle(), Lifecycle::Running);
    assert!(state.code_fetch().queue().open_now());
    // A second start must not create a second thread or reset the queue.
    assert_eq!(
        start_with(&state, &manifest, Box::new(NeverAsked)),
        Start::AlreadyStarted
    );
    // Let the detached worker exit before the test ends.
    state.code_fetch().queue().close();
}
