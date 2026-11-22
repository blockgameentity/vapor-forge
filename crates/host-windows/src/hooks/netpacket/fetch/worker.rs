//! The fetch worker: one thread, one target at a time, bounded on every side.
//!
//! The thread is the whole reason the hook path never dials. It pulls one target
//! from [`Queue`], asks the [`Fetcher`] for a code, inserts whatever arrived into
//! the code cache and clears the in-flight marker. Nothing else runs on it, and
//! nothing on it is unbounded:
//!
//! | bound | value | why |
//! | --- | --- | --- |
//! | waiting targets | [`CAPACITY`](super::queue::CAPACITY) | the ring evicts the oldest, counted |
//! | concurrent fetches | 1 | a serial worker cannot burst a shared provider |
//! | requests per 60 s | `min(MAX_PER_MINUTE, 60_000 / min_interval_ms)` | a hard cap, whatever the config says |
//! | provider names interned | [`MAX_INTERNED`] | the cache holds `&'static str` |
//! | remembered failures | [`SEEN_CAPACITY`](super::outcome::SEEN_CAPACITY) | one `debug` line per target and class |
//! | time on one target | the chain's own `timeout_connect_ms` + `timeout_ms` | then the target is abandoned |
//!
//! The rate budget is computed from the same `[manifest] min_interval_ms` the
//! chain paces with, so a config that asks for one request every five seconds
//! gets at most twelve a minute here as well — and a config that asks for none
//! still cannot exceed [`MAX_PER_MINUTE`], because this host cannot know how
//! patient a provider really is.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tracing::debug;

use crate::hooks::netpacket::decision::CachedCode;
use crate::hooks::state::HostState;

use super::fetcher::Fetcher;
use super::outcome::{Class, Outcome, Seen, UNKNOWN_PROVIDER};
use super::queue::Tally;

/// Hard ceiling on provider requests in any 60-second window.
///
/// The chain paces per provider from `min_interval_ms`, but a config that leaves
/// it at `0` would otherwise let a big update burst at once. Thirty a minute is
/// one every two seconds, which no configured provider has ever needed.
pub const MAX_PER_MINUTE: u64 = 30;

/// The window the budget refills over.
const WINDOW: Duration = Duration::from_secs(60);

/// How long the worker waits for work before checking the stop signal again.
///
/// A stop is not an enqueue, so the wait must expire on its own; keeping it short
/// is what lets the thread exit promptly at `DLL_PROCESS_DETACH`. This is
/// therefore also the longest a stopped worker takes to notice, which is what the
/// shutdown test asserts against.
pub const STOP_GRACE: Duration = Duration::from_millis(200);

/// How many provider names are interned for the cache's `&'static str` slot.
///
/// A [`CachedCode`] is copied out of the cache on Steam's networking thread, so
/// its provider name cannot be an owned `String` without an allocation on that
/// thread. Names therefore need a `'static` lifetime; the worker interns them,
/// once per distinct name, and the count is bounded by the provider list the host
/// tool writes (a handful). Past the bound a code is still cached, under
/// [`UNKNOWN_PROVIDER`].
const MAX_INTERNED: usize = 8;

/// The per-minute request budget.
#[derive(Debug)]
struct Budget {
    limit: u64,
    used: u64,
    window: Instant,
}

impl Budget {
    /// A budget for a config whose providers are paced at `min_interval_ms`.
    fn per_minute(min_interval_ms: u64) -> u64 {
        let paced = 60_000 / min_interval_ms.max(1);
        paced.clamp(1, MAX_PER_MINUTE)
    }

    fn new(limit: u64) -> Self {
        Self {
            limit: limit.max(1),
            used: 0,
            window: Instant::now(),
        }
    }

    /// Spend one request, or refuse because the window is full.
    fn take(&mut self, now: Instant) -> bool {
        if now.saturating_duration_since(self.window) >= WINDOW {
            self.window = now;
            self.used = 0;
        }
        if self.used >= self.limit {
            return false;
        }
        self.used += 1;
        true
    }
}

/// Provider names, interned once each so a cache entry stays `Copy`.
#[derive(Debug, Default)]
struct Names {
    interned: Vec<&'static str>,
}

impl Names {
    fn intern(&mut self, name: &str) -> &'static str {
        if let Some(held) = self.interned.iter().find(|held| **held == name) {
            return held;
        }
        if self.interned.len() >= MAX_INTERNED {
            return UNKNOWN_PROVIDER;
        }
        // The leak is the point: a `CachedCode` is `Copy` and is copied out on
        // Steam's networking thread, so the name must be `'static`. It happens
        // once per distinct provider name and is capped at `MAX_INTERNED` for the
        // process lifetime.
        let interned: &'static str = Box::leak(name.to_owned().into_boxed_str());
        self.interned.push(interned);
        interned
    }
}

/// The handle the lifecycle keeps so it can ask the worker to stop.
///
/// Dropping it detaches the thread rather than joining it: the worker exits on
/// its own stop signal or on [`crate::thread::stop_requested`], and a host that
/// is shutting down must not block inside `DLL_PROCESS_DETACH`.
#[derive(Debug)]
pub struct WorkerHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl WorkerHandle {
    /// Ask the worker to stop at its next step boundary. Never blocks.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    /// Ask the worker to stop and wait for it. Process teardown never does this;
    /// tests and the report path need the join.
    pub fn join(mut self) {
        self.stop();
        let Some(thread) = self.thread.take() else {
            return;
        };
        // A panicking worker must not take its joiner down with it: the panic has
        // already been reported, and there is nothing to do with the result.
        if thread.join().is_err() {
            debug!("the request-code fetcher thread panicked");
        }
    }
}

/// Create the worker thread for `state`.
///
/// `None` means the thread could not be created; the caller closes the queue and
/// reports the fetcher as idle, so no request is ever offered to a worker that
/// does not exist.
pub fn spawn(
    state: Arc<HostState>,
    fetcher: Box<dyn Fetcher>,
    min_interval_ms: u64,
) -> Option<WorkerHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let limit = Budget::per_minute(min_interval_ms);
    let spawned = std::thread::Builder::new()
        .name("vf-request-code".to_owned())
        .spawn(move || run(state, fetcher, flag, limit));
    match spawned {
        Ok(thread) => Some(WorkerHandle {
            stop,
            thread: Some(thread),
        }),
        Err(error) => {
            tracing::warn!(%error, "the request-code fetcher thread could not be created");
            None
        }
    }
}

/// Stop condition: our own flag, or the host's process-wide one.
fn stopping(stop: &AtomicBool) -> bool {
    stop.load(Ordering::Acquire) || crate::thread::stop_requested()
}

/// The worker body.
fn run(state: Arc<HostState>, mut fetcher: Box<dyn Fetcher>, stop: Arc<AtomicBool>, limit: u64) {
    let queue = state.code_fetch().queue();
    let mut budget = Budget::new(limit);
    let mut names = Names::default();
    let mut seen = Seen::new();
    debug!(
        limit_per_minute = limit,
        capacity = super::queue::CAPACITY,
        "request-code fetcher running"
    );
    while !stopping(&stop) {
        let Some(request) = queue.next(STOP_GRACE) else {
            if !queue.open_now() {
                break;
            }
            continue;
        };
        if !budget.take(Instant::now()) {
            queue.record(Tally::OverBudget);
            if seen.first(request, Class::OverBudget) {
                debug!(
                    app = request.app,
                    depot = request.target.depot,
                    gid = request.target.gid,
                    limit_per_minute = limit,
                    "request code not fetched: the per-minute budget is spent"
                );
            }
            queue.finish();
            continue;
        }
        let outcome = fetcher.fetch(request);
        queue.record(outcome.tally());
        match &outcome {
            Outcome::Served { code, provider } => {
                state.request_codes().insert(
                    request.target,
                    CachedCode {
                        code: *code,
                        provider: Some(names.intern(provider)),
                    },
                );
                debug!(
                    app = request.app,
                    depot = request.target.depot,
                    gid = request.target.gid,
                    code = *code,
                    provider = provider.as_str(),
                    "request code fetched and cached"
                );
            }
            failure => {
                // Nothing negative is remembered about the target, so a transient
                // failure cannot poison it: the next cache miss asks again.
                if let Some(class) = failure.class() {
                    if seen.first(request, class) {
                        debug!(
                            app = request.app,
                            depot = request.target.depot,
                            gid = request.target.gid,
                            reason = failure.reason().as_str(),
                            "request code not fetched"
                        );
                    }
                }
            }
        }
        queue.finish();
    }
    queue.close();
    debug!(
        queued = queue.counters().queued,
        served = queue.counters().served,
        evicted = queue.counters().evicted_oldest,
        budget = queue.counters().over_budget,
        "request-code fetcher stopped"
    );
}

/// Scripted requests, for the tests that drive the worker without a socket.
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
