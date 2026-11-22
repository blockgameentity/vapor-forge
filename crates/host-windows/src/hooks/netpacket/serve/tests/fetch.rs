//! The fetch hand-off: a cache miss offers the target, and nothing else does.
//!
//! These run through [`prepare`] with the same fake packet the serving tests use,
//! because the thing being asserted is what the *body* does on a miss — not what
//! the queue does once a target is in it (that is `fetch::queue`'s own suite).

use std::time::Duration;

use super::*;
use crate::hooks::netpacket::fetch::{DropReason, Enqueued, Request};

/// A state whose queue is open, as the lifecycle leaves a config with providers.
fn fetching_state() -> HostState {
    let state = serving_state();
    state.code_fetch().queue().open();
    state
}

/// What the worker would take next, if anything.
fn take(state: &HostState) -> Option<Request> {
    state.code_fetch().queue().next(Duration::ZERO)
}

#[test]
fn a_cache_miss_offers_the_app_and_the_target_to_the_fetch_worker() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = fetching_state();
    // SAFETY: the fake packet is live and its layout is the one the state holds.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none(), "a miss serves nothing");
    assert_eq!(
        prepared.observation.map(|observation| observation.blocker),
        Some(ServeBlocker::NoCachedCode)
    );
    assert_eq!(state.code_fetch().queue().pending(), 1, "the miss is queued");
    // The app travels because a provider url may name it; the target is the cache
    // key. Those three numbers are the whole request.
    assert_eq!(
        take(&state),
        Some(Request {
            app: 480,
            target: target()
        })
    );
}

#[test]
fn a_served_code_is_never_offered_for_a_fetch() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = fetching_state();
    state.request_codes().insert(
        target(),
        CachedCode {
            code: 0xfeed,
            provider: Some("test"),
        },
    );
    // SAFETY: the swap is dropped here, before the packet is.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_some(), "the cached code is served");
    assert_eq!(
        state.code_fetch().queue().pending(),
        0,
        "a hit must not ask for a code it already has"
    );
}

#[test]
fn a_disabled_capability_offers_nothing() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = fetching_state();
    let mut gate = state.gate();
    gate.disable(Capability::RequestCodes, "the install pass said no");
    state.set_gate(gate);
    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none());
    assert_eq!(state.code_fetch().queue().pending(), 0);
}

#[test]
fn no_layout_offers_nothing() {
    let state = state_with(config_with(&[480]), Vec::new());
    state.code_fetch().queue().open();
    // SAFETY: no layout means no dereference of the (deliberately dangling) pointer.
    let prepared = unsafe { prepare(&state, 0x1000 as *mut c_void) };
    assert!(prepared.swap.is_none());
    assert_eq!(state.code_fetch().queue().pending(), 0);
}

#[test]
fn a_payload_with_no_target_offers_nothing() {
    let body = vec![0xff, 0xff, 0xff];
    let mut packet = FakePacket::new(&body);
    let state = fetching_state();
    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none());
    assert_eq!(state.code_fetch().queue().pending(), 0);
}

#[test]
fn the_same_miss_is_offered_once_however_often_steam_repeats_it() {
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = fetching_state();
    for _ in 0..3 {
        // SAFETY: nothing is written while the code is missing.
        let prepared = unsafe { prepare(&state, packet.as_ptr()) };
        assert!(prepared.swap.is_none());
    }
    assert_eq!(state.code_fetch().queue().pending(), 1);
    assert_eq!(state.code_fetch().queue().counters().dropped_already_queued, 2);
}

#[test]
fn an_idle_fetcher_refuses_the_offer_without_blocking_the_dispatch() {
    // No provider configured: the lifecycle never opens the queue, so the offer is
    // counted and refused, and the body still falls through to Steam.
    let body = request_body(731, 0x0bad_cafe);
    let mut packet = FakePacket::new(&body);
    let state = serving_state();
    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, packet.as_ptr()) };
    assert!(prepared.swap.is_none());
    assert_eq!(
        prepared.observation.map(|observation| observation.blocker),
        Some(ServeBlocker::NoCachedCode)
    );
    assert_eq!(state.code_fetch().queue().counters().dropped_inactive, 1);
    assert_eq!(
        state.code_fetch().queue().enqueue(Request {
            app: 480,
            target: target()
        }),
        Enqueued::Dropped(DropReason::Inactive)
    );
}
