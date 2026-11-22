//! The bounded record of inbound CM messages the host could not fully read.
//!
//! The body runs on Steam's networking thread and must not allocate without
//! bound, so the ledger is a fixed-capacity ring: the newest observations are kept
//! and the count of dropped ones is kept *as a count*, never as silence. A record
//! is only ever made when the `cm-interception` capability is enabled (see
//! [`super`]), and reading it back is a diagnostics concern, not a hook concern.
//!
//! Why record at all: the honest state of this hook is "the dispatch is hooked, the
//! payload is unreadable". A no-op body would leave nothing behind to prove the
//! hook is reached or to show *which* gap stopped it. One observation per inbound
//! message — with the gap named — is what a live client check reads.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::decision::Observation;

/// How many observations are kept. Steam dispatches these continuously, so the
/// ring is small on purpose: the first message is the interesting one and the
/// steady state is noise.
pub const CAPACITY: usize = 32;

/// One recorded observation and the sequence number that identified it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Recorded {
    /// 1-based position in the stream, so "the first message" is expressible even
    /// after the ring has wrapped.
    pub sequence: u64,
    pub observation: Observation,
}

#[derive(Debug, Default)]
struct Inner {
    kept: VecDeque<Recorded>,
    total: u64,
    dropped: u64,
}

/// Fixed-capacity record of what the dispatch hook saw.
#[derive(Debug, Default)]
pub struct Ledger {
    inner: Mutex<Inner>,
}

impl Ledger {
    /// An empty ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observation and return its 1-based sequence number.
    ///
    /// Never blocks longer than a mutex: no I/O, no allocation beyond the ring's
    /// fixed size. A poisoned lock is recovered, because losing the record is
    /// worse than trusting a map of plain values.
    pub fn record(&self, observation: Observation) -> u64 {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.total = inner.total.saturating_add(1);
        let sequence = inner.total;
        if inner.kept.len() == CAPACITY {
            inner.kept.pop_front();
            inner.dropped = inner.dropped.saturating_add(1);
        }
        inner.kept.push_back(Recorded {
            sequence,
            observation,
        });
        sequence
    }

    /// The observations still held, oldest first.
    pub fn snapshot(&self) -> Vec<Recorded> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.kept.iter().copied().collect()
    }

    /// How many observations were recorded in total, including the dropped ones.
    pub fn total(&self) -> u64 {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.total
    }

    /// How many observations fell out of the ring.
    pub fn dropped(&self) -> u64 {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::netpacket::decision::ServeBlocker;

    fn observation() -> Observation {
        Observation {
            emsg: None,
            class: None,
            target: None,
            blocker: ServeBlocker::NoLayout,
        }
    }

    #[test]
    fn a_fresh_ledger_records_nothing() {
        let ledger = Ledger::new();
        assert!(ledger.snapshot().is_empty());
        assert_eq!(ledger.total(), 0);
        assert_eq!(ledger.dropped(), 0);
    }

    #[test]
    fn sequence_numbers_keep_counting_after_the_ring_wraps() {
        let ledger = Ledger::new();
        for expected in 1..=(CAPACITY as u64 + 3) {
            assert_eq!(ledger.record(observation()), expected);
        }
        assert_eq!(ledger.total(), CAPACITY as u64 + 3);
        assert_eq!(ledger.dropped(), 3);
        let kept = ledger.snapshot();
        assert_eq!(kept.len(), CAPACITY);
        assert_eq!(kept.first().map(|entry| entry.sequence), Some(4));
        assert_eq!(
            kept.last().map(|entry| entry.sequence),
            Some(CAPACITY as u64 + 3)
        );
        assert!(kept.iter().all(|entry| entry.observation == observation()));
    }
}
