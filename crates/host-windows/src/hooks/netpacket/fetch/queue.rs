//! The bounded hand-off between Steam's networking thread and the fetch worker.
//!
//! [`Queue::enqueue`] is the only thing the `CCMInterface::RecvPkt` body calls,
//! and it may only do this: take one mutex that no other thread holds across a
//! network call, compare a handful of `(app, depot, gid)` triples, push onto a
//! fixed-capacity ring, and notify a condvar. No I/O, no allocation past the
//! ring's own growth, no wait on the worker.
//!
//! Every rejection is a counted reason rather than silence:
//!
//! ```text
//! the worker is not running          -> dropped_inactive      (one log line at start)
//! the same target is being fetched    -> dropped_in_flight     (one round trip is enough)
//! the same target is already queued   -> dropped_already_queued
//! the ring is full                    -> the oldest entry goes, evicted_oldest++
//! the per-minute budget is exhausted  -> over_budget           (worker side, see `worker`)
//! ```
//!
//! [`Counters`] is the whole record: fixed-size, saturating, and readable from a
//! test or from the report without touching the queue's ordering.

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

use super::Request;

/// How many targets may wait for the worker at once.
///
/// One request-code message per depot is what Steam sends, and a big library
/// updates a few dozen depots at a time, so this covers a normal update burst
/// without ever growing with traffic.
pub const CAPACITY: usize = 32;

/// Why the hook's offer never reached the worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropReason {
    /// The same target is in the worker's hands right now.
    InFlight,
    /// The same target is already waiting in the ring.
    AlreadyQueued,
    /// The worker is not running: no provider is configured, or it was stopped.
    Inactive,
}

/// What [`Queue::enqueue`] did with one offer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Enqueued {
    /// Accepted; nothing was displaced.
    Queued,
    /// Accepted, after evicting the oldest pending target to make room.
    QueuedEvictingOldest,
    /// Not accepted, for the named reason.
    Dropped(DropReason),
}

/// What a finished target amounted to, for [`Counters`].
///
/// Kept here, and not in `outcome`, so the counter table has no opinion about
/// what a provider answered — only about how many of each there were.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tally {
    Served,
    NotArchived,
    Transient,
    NoProvider,
    OverBudget,
}

/// The fixed-size record of everything the queue and the worker did.
///
/// Saturating rather than wrapping: a counter that has wrapped is a lie, and a
/// counter pinned at its maximum is still an honest "a lot".
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Counters {
    pub queued: u64,
    pub dropped_in_flight: u64,
    pub dropped_already_queued: u64,
    pub dropped_inactive: u64,
    pub evicted_oldest: u64,
    pub served: u64,
    pub not_archived: u64,
    pub transient: u64,
    pub no_provider: u64,
    pub over_budget: u64,
}

impl Counters {
    fn bump(slot: &mut u64) {
        *slot = slot.saturating_add(1);
    }

    fn record(&mut self, tally: Tally) {
        let slot = match tally {
            Tally::Served => &mut self.served,
            Tally::NotArchived => &mut self.not_archived,
            Tally::Transient => &mut self.transient,
            Tally::NoProvider => &mut self.no_provider,
            Tally::OverBudget => &mut self.over_budget,
        };
        Self::bump(slot);
    }
}

#[derive(Debug)]
struct Inner {
    /// Waiting targets, oldest first.
    pending: VecDeque<Request>,
    /// The target the worker took and has not finished, for dedup.
    in_flight: Option<Request>,
    /// Whether the worker exists and is accepting work.
    open: bool,
    counters: Counters,
}

/// The queue the hook fills and the worker drains.
#[derive(Debug)]
pub struct Queue {
    inner: Mutex<Inner>,
    /// Notified on every enqueue and on close, so the worker wakes instead of
    /// polling in a tight loop. It still waits with a timeout, because a stop
    /// signal is not an enqueue.
    wake: Condvar,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                pending: VecDeque::new(),
                in_flight: None,
                open: false,
                counters: Counters::default(),
            }),
            wake: Condvar::new(),
        }
    }

    /// A poisoned queue is recovered rather than propagated: it guards a ring of
    /// plain values, and losing the ability to serve codes because a worker
    /// panicked once is strictly worse than trusting the ring.
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Accept work. Called once by the lifecycle, after the worker exists.
    pub fn open(&self) {
        self.lock().open = true;
    }

    /// Stop accepting work and wake the worker so it can exit.
    pub fn close(&self) {
        self.lock().open = false;
        self.wake.notify_all();
    }

    /// Whether the worker is accepting work.
    pub fn open_now(&self) -> bool {
        self.lock().open
    }

    /// Offer one target to the worker.
    ///
    /// Returns without waiting for the worker: the lock is released before the
    /// condvar is notified, and the worker never holds it across a fetch.
    pub fn enqueue(&self, request: Request) -> Enqueued {
        let mut inner = self.lock();
        if !inner.open {
            Counters::bump(&mut inner.counters.dropped_inactive);
            return Enqueued::Dropped(DropReason::Inactive);
        }
        if inner.in_flight == Some(request) {
            Counters::bump(&mut inner.counters.dropped_in_flight);
            return Enqueued::Dropped(DropReason::InFlight);
        }
        if inner.pending.contains(&request) {
            Counters::bump(&mut inner.counters.dropped_already_queued);
            return Enqueued::Dropped(DropReason::AlreadyQueued);
        }
        let evicted = if inner.pending.len() >= CAPACITY {
            inner.pending.pop_front();
            Counters::bump(&mut inner.counters.evicted_oldest);
            true
        } else {
            false
        };
        inner.pending.push_back(request);
        Counters::bump(&mut inner.counters.queued);
        drop(inner);
        self.wake.notify_one();
        if evicted {
            Enqueued::QueuedEvictingOldest
        } else {
            Enqueued::Queued
        }
    }

    /// Wait up to `wait` for a target, then take the oldest one.
    ///
    /// `None` means the queue was closed (the worker must exit) — never "there
    /// was nothing to do", which is signalled by waiting and returning nothing
    /// only after `wait` expires.
    pub fn next(&self, wait: Duration) -> Option<Request> {
        let mut inner = self.lock();
        if inner.open && inner.pending.is_empty() {
            let (guard, _timed_out) = self
                .wake
                .wait_timeout(inner, wait)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner = guard;
        }
        if !inner.open {
            return None;
        }
        let request = inner.pending.pop_front()?;
        inner.in_flight = Some(request);
        Some(request)
    }

    /// The worker finished with the target it took, whatever the outcome was.
    ///
    /// Clearing this is what keeps a transient failure from poisoning a target:
    /// nothing negative is remembered, so the next miss asks again.
    pub fn finish(&self) {
        self.lock().in_flight = None;
    }

    /// Count one finished target.
    pub fn record(&self, tally: Tally) {
        self.lock().counters.record(tally);
    }

    /// The counters as they stand.
    pub fn counters(&self) -> Counters {
        self.lock().counters
    }

    /// How many targets are waiting, for tests and for the report.
    pub fn pending(&self) -> usize {
        self.lock().pending.len()
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
