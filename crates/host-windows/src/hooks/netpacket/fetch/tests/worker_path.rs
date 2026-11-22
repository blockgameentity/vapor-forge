//! The worker against a scripted fetcher: order, caching, failure, budget, and
//! the proof that the hook path never waits for it.

use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use super::super::outcome::Outcome;
use super::super::queue::{DropReason, Enqueued};
use super::super::MAX_PER_MINUTE;
use super::{enqueue, open_state, start, target, wait_for};

/// How long an enqueue may take while the fetcher is deliberately held open.
///
/// The bound is generous on purpose: the assertion is "the hook path did not wait
/// for the fetcher", not "the hook path is fast on this machine". The fetcher is
/// held open by a channel that is never sent on until after the measurement, so
/// any value below [`HELD_OPEN`] proves the point.
const ENQUEUE_BUDGET: Duration = Duration::from_millis(250);
const HELD_OPEN: Duration = Duration::from_secs(2);

fn served(code: u64, provider: &str) -> Outcome {
    Outcome::Served {
        code,
        provider: provider.to_owned(),
    }
}

#[test]
fn a_served_code_is_cached_under_the_provider_that_served_it() {
    let state = open_state();
    let (handle, _calls, _steps) = start(&state, vec![served(0xfeed, "a.host")], None, 0);
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the code to be cached", |state| {
        state.request_codes().get(target(7).target).is_some()
    });
    let cached = state
        .request_codes()
        .get(target(7).target)
        .expect("the code is cached");
    assert_eq!(cached.code, 0xfeed);
    assert_eq!(cached.provider, Some("a.host"));
    assert_eq!(state.code_fetch().queue().counters().served, 1);
    handle.join();
}

#[test]
fn targets_are_asked_for_one_at_a_time_in_the_order_they_arrived() {
    let state = open_state();
    let (handle, calls, _steps) = start(
        &state,
        vec![served(1, "a.host"), served(2, "b.host"), served(3, "c.host")],
        None,
        0,
    );
    for gid in [11, 12, 13] {
        assert_eq!(enqueue(&state, target(gid)), Enqueued::Queued);
    }
    wait_for(&state, "all three codes", |state| {
        [11, 12, 13]
            .iter()
            .all(|gid| state.request_codes().get(target(*gid).target).is_some())
    });
    let asked: Vec<u64> = calls
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .map(|request| request.target.gid)
        .collect();
    assert_eq!(asked, vec![11, 12, 13], "a serial worker keeps queue order");
    // Each code keeps the provider the chain reported for it, so a failover run
    // stays inspectable in the cache.
    let provider = |gid: u64| {
        state
            .request_codes()
            .get(target(gid).target)
            .and_then(|cached| cached.provider)
    };
    assert_eq!(provider(11), Some("a.host"));
    assert_eq!(provider(12), Some("b.host"));
    assert_eq!(provider(13), Some("c.host"));
    handle.join();
}

#[test]
fn a_transient_failure_does_not_poison_the_target() {
    let state = open_state();
    let (handle, calls, _steps) = start(
        &state,
        vec![
            Outcome::Transient {
                reason: "connection reset".to_owned(),
            },
            served(0xabc, "a.host"),
        ],
        None,
        0,
    );
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the transient failure to be counted", |state| {
        state.code_fetch().queue().counters().transient == 1
    });
    assert!(
        state.request_codes().get(target(7).target).is_none(),
        "a failure caches nothing"
    );
    // The next miss for the same target asks again: nothing negative is kept.
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the retry to cache a code", |state| {
        state.request_codes().get(target(7).target).is_some()
    });
    assert_eq!(
        state
            .request_codes()
            .get(target(7).target)
            .map(|cached| cached.code),
        Some(0xabc)
    );
    let asked = calls
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .len();
    assert_eq!(asked, 2, "the target was asked for twice, once per miss");
    handle.join();
}

#[test]
fn an_unarchived_target_is_counted_separately_and_also_not_poisoned() {
    let state = open_state();
    let (handle, _calls, _steps) = start(
        &state,
        vec![Outcome::NotArchived, served(5, "a.host")],
        None,
        0,
    );
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the refusal to be counted", |state| {
        state.code_fetch().queue().counters().not_archived == 1
    });
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the retry to cache a code", |state| {
        state.request_codes().get(target(7).target).is_some()
    });
    handle.join();
}

#[test]
fn a_missing_provider_is_counted_and_caches_nothing() {
    let state = open_state();
    let (handle, _calls, _steps) = start(&state, vec![Outcome::NoProvider], None, 0);
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the missing provider to be counted", |state| {
        state.code_fetch().queue().counters().no_provider == 1
    });
    assert!(state.request_codes().get(target(7).target).is_none());
    handle.join();
}

#[test]
fn the_per_minute_budget_is_a_hard_cap_that_drops_the_target() {
    let state = open_state();
    // One request a minute: the second target cannot be asked for at all.
    let (handle, calls, _steps) = start(&state, vec![served(1, "a.host")], None, 60_000);
    // The cap is what makes this test possible: one request a window.
    const { assert!(MAX_PER_MINUTE > 1) };
    assert_eq!(enqueue(&state, target(1)), Enqueued::Queued);
    assert_eq!(enqueue(&state, target(2)), Enqueued::Queued);
    wait_for(&state, "the budget to refuse the second target", |state| {
        state.code_fetch().queue().counters().over_budget == 1
    });
    assert!(state.request_codes().get(target(1).target).is_some());
    assert!(
        state.request_codes().get(target(2).target).is_none(),
        "a target refused by the budget caches nothing"
    );
    assert_eq!(
        calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len(),
        1,
        "the budget refuses instead of asking"
    );
    handle.join();
}

#[test]
fn the_same_target_is_not_asked_for_twice_while_it_is_in_flight() {
    let state = open_state();
    let (release, held) = channel();
    let (handle, calls, steps) = start(&state, vec![served(9, "a.host")], Some(held), 0);
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    // The fetcher reports the request it is holding, so the second offer can be
    // made while that one is genuinely in flight.
    assert_eq!(steps.recv_timeout(HELD_OPEN).map(|r| r.target.gid), Ok(7));
    assert_eq!(
        enqueue(&state, target(7)),
        Enqueued::Dropped(DropReason::InFlight),
        "one round trip per target is enough"
    );
    drop(release);
    wait_for(&state, "the held fetch to finish", |state| {
        state
            .request_codes()
            .get(target(7).target)
            .is_some()
    });
    assert_eq!(
        calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len(),
        1
    );
    handle.join();
}

#[test]
fn the_hook_path_returns_while_the_fetcher_is_held_open() {
    let state = open_state();
    let (release, held) = channel::<()>();
    // One scripted answer is enough; the second fetch runs unblocked so the
    // worker can finish once the first is released.
    let (handle, _calls, steps) = start(&state, vec![served(1, "a.host")], Some(held), 0);
    let started = Instant::now();
    assert_eq!(enqueue(&state, target(1)), Enqueued::Queued);
    assert!(
        started.elapsed() < ENQUEUE_BUDGET,
        "the first offer returned in {:?}",
        started.elapsed()
    );
    // The fetcher is now holding the first target open, and will not move until
    // this test releases it. The hook path must still return immediately.
    assert_eq!(steps.recv_timeout(HELD_OPEN).map(|r| r.target.gid), Ok(1));
    let while_held = Instant::now();
    assert_eq!(enqueue(&state, target(2)), Enqueued::Queued);
    let elapsed = while_held.elapsed();
    assert!(
        elapsed < ENQUEUE_BUDGET,
        "enqueue took {elapsed:?} while the fetcher was held open; the hook path must not wait"
    );
    // The second target is still queued, not fetched: the worker is stuck in the
    // first fetch, which is exactly what "held open" means.
    assert!(
        state.request_codes().get(target(1).target).is_none(),
        "nothing is cached while the fetch is in flight"
    );
    assert_eq!(state.code_fetch().queue().pending(), 1);
    drop(release);
    wait_for(&state, "both targets to be handled", |state| {
        state.request_codes().get(target(1).target).is_some()
            && state.code_fetch().queue().pending() == 0
    });
    handle.join();
}

#[test]
fn a_stopped_worker_exits_promptly_and_refuses_further_work() {
    let state = open_state();
    let (handle, _calls, _steps) = start(&state, Vec::new(), None, 0);
    let stopped = Instant::now();
    handle.join();
    assert!(
        stopped.elapsed() < HELD_OPEN,
        "a stopped worker took {:?} to exit",
        stopped.elapsed()
    );
    assert!(!state.code_fetch().queue().open_now());
    assert_eq!(
        enqueue(&state, target(7)),
        Enqueued::Dropped(DropReason::Inactive)
    );
}

#[test]
fn the_worker_handles_a_scripted_script_that_runs_out_without_panicking() {
    // No scripted answers: `Scripted` reports a transient failure rather than
    // unwrapping, so an empty script must still leave the worker healthy.
    let state = open_state();
    let (handle, _calls, _steps) = start(&state, Vec::new(), None, 0);
    assert_eq!(enqueue(&state, target(7)), Enqueued::Queued);
    wait_for(&state, "the empty script to be reported", |state| {
        state.code_fetch().queue().counters().transient == 1
    });
    handle.join();
}
