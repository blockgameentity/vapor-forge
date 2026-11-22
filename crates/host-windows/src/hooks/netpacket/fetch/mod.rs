//! Where request codes come from on this host: a queue, a worker, a provider chain.
//!
//! The dispatch body in [`super`] serves a code it holds. Until this module
//! existed nothing on Windows ever put one there, so every request-code message
//! recorded [`ServeBlocker::NoCachedCode`](super::decision::ServeBlocker::NoCachedCode)
//! and Steam's own round trip answered it. This is the producer.
//!
//! # The one rule: no network in the hook path
//!
//! `CCMInterface::RecvPkt` runs on Steam's networking thread. Blocking it on a
//! provider round trip would stall the client's CM traffic, so the body does
//! exactly one thing on a miss: [`Queue::enqueue`](queue::Queue::enqueue). That
//! takes one mutex the worker never holds across a fetch, compares a few
//! `(app, depot, gid)` triples, pushes onto a fixed-capacity ring and notifies a
//! condvar. No socket, no sleep, no allocation that grows with traffic.
//!
//! ```text
//! RecvPkt ── miss ──▶ Queue ──▶ worker thread ──▶ Fetcher ──▶ features::request_code
//!    │                            │                                   │
//!    └── hit ──▶ serve (unchanged)└──▶ CodeCache::insert ◀── provider answer
//! ```
//!
//! A code that arrives lands in [`super::cache::CodeCache`] under the provider
//! that served it; the *next* message for that `(depot, gid)` is served from the
//! cache, inside Steam's own call, with the payload swapped and restored. Nothing
//! is posted and nothing is queued for delivery: on this host serving *is*
//! delivering (see [`super`]).
//!
//! # What is bounded
//!
//! [`queue::CAPACITY`] waiting targets (oldest evicted, counted), one fetch in
//! flight, [`worker::MAX_PER_MINUTE`] requests per minute honouring
//! `[manifest] min_interval_ms`, [`worker`]'s interned provider names, and
//! [`outcome::SEEN_CAPACITY`] remembered failures so the log has one line per
//! target and reason class. Every refusal is a counter in
//! [`queue::Counters`], never silence.
//!
//! # What is *not* here
//!
//! No provider host, no endpoint template, no response parser and no pacing
//! logic: [`fetcher::ChainFetcher`] hands the target to
//! `features::request_code`, the same chain the Linux host runs. An empty
//! `[manifest] providers` leaves the fetcher idle — see [`lifecycle::NO_PROVIDER`].
//!
//! # The pieces, and what each one can be tested for
//!
//! * [`queue`] — the bounded, deduplicating hand-off and its counters. Pure.
//! * [`outcome`] — what a fetch produced, and the reason classes. Pure.
//! * [`fetcher`] — the [`Fetcher`] seam; [`fetcher::ChainFetcher`] is the only
//!   implementation that dials, and it dials through `features`.
//! * [`worker`] — the thread, the rate budget, the cache insert.
//! * [`lifecycle`] — one start per process, and the reasons it stays idle.

pub mod fetcher;
pub mod lifecycle;
pub mod outcome;
pub mod queue;
pub mod worker;

pub use fetcher::Fetcher;
pub use lifecycle::{start, Fetch, Lifecycle, Start};
pub use outcome::{Class, Outcome};
pub use queue::{Counters, DropReason, Enqueued, Queue, Tally};
pub use worker::{WorkerHandle, MAX_PER_MINUTE, STOP_GRACE};

#[cfg(test)]
mod tests;

use super::decision::RequestTarget;

/// One manifest a code is wanted for.
///
/// `app` is carried because a provider url may use the `{appId}` placeholder
/// (`ManifestSource::resolve_url` takes it), and `(depot, gid)` is carried because
/// that pair is what a code is keyed by. Those three numbers are the whole
/// request: nothing about the account, the user or the library travels with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    /// The app Steam asked the code for.
    pub app: u32,
    /// The `(depot, gid)` pair the code is keyed by.
    pub target: RequestTarget,
}
