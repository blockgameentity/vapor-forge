//! What one fetch produced, and the reason class it belongs to.
//!
//! [`Outcome`] is this host's vocabulary for a provider round trip. It is what a
//! [`Fetcher`](super::Fetcher) reports, so a test can drive every branch without
//! a socket, and it is what the worker turns into a cache insert or a single
//! `debug` line.
//!
//! # What the production chain can and cannot distinguish
//!
//! `features::request_code` runs the whole chain — provider order, failover,
//! per-provider pacing, cooldowns, response parsing — and reports one of its own
//! [`ChainError`](vapor_forge_features::request_code::ChainError) variants when
//! no code came back. Its *public* queue
//! ([`PendingQueue`](vapor_forge_features::request_code::PendingQueue)) is the
//! only way into that chain from this crate, and it collapses every refusal into
//! `FetchOutcome { code: 0, provider: None }`: the `AttemptStatus` that told a
//! `401 Denied` (the provider answers, it just has no code for this gid) apart
//! from a transport failure is not carried across the queue.
//!
//! So [`from_completed`] reports a collapsed refusal as [`Outcome::Transient`],
//! which is the correct reaction to all of them: retry later. [`Outcome::NotArchived`]
//! is what a fetcher that *can* tell reports, the fake in the tests does, and the
//! production adapter does too the moment the chain surfaces
//! `AttemptStatus::Denied` across its queue. It is a distinct variant rather than
//! being folded into `Transient` here because the two lead to different decisions
//! in the log (retrying a gid no provider stocks is noise) and because a variant
//! that only the tests can produce would be a lie about the seam.

use std::collections::VecDeque;

use vapor_forge_features::request_code::CompletedFetch;

use super::queue::Tally;
use super::Request;

/// Name recorded when a code arrives without a provider attached to it.
///
/// The chain always names its provider, so this is the honest fallback for a
/// fetcher that does not.
pub const UNKNOWN_PROVIDER: &str = "unknown";

/// One fetch, in this host's terms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// A code arrived, from the named provider.
    Served { code: u64, provider: String },
    /// A provider answered and has no code for this `(depot, gid)`.
    ///
    /// Not an error: nothing is wrong with the source, the target is simply not
    /// one it stocks, so the next `gid` may well be served by it.
    NotArchived,
    /// The attempt failed for a reason that may pass: transport, a 429, a
    /// cooldown, a zero, an unparsable body.
    Transient { reason: String },
    /// Nothing is configured to ask.
    NoProvider,
}

/// The reason class a failure is logged under.
///
/// One line per `(target, class)`: a depot that stays missing must not fill the
/// log with the same sentence on every retry, and a target that starts failing
/// for a *different* reason is still worth seeing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    NotArchived,
    Transient,
    NoProvider,
    /// The per-minute budget refused the target, so no provider was asked.
    OverBudget,
}

impl Outcome {
    /// The counter this outcome increments.
    pub fn tally(&self) -> Tally {
        match self {
            Self::Served { .. } => Tally::Served,
            Self::NotArchived => Tally::NotArchived,
            Self::Transient { .. } => Tally::Transient,
            Self::NoProvider => Tally::NoProvider,
        }
    }

    /// The reason class to log this under, or `None` when it is a success.
    pub fn class(&self) -> Option<Class> {
        match self {
            Self::Served { .. } => None,
            Self::NotArchived => Some(Class::NotArchived),
            Self::Transient { .. } => Some(Class::Transient),
            Self::NoProvider => Some(Class::NoProvider),
        }
    }

    /// One sentence for the log.
    pub fn reason(&self) -> String {
        match self {
            Self::Served { code, provider } => {
                format!("provider `{provider}` served code {code}")
            }
            Self::NotArchived => {
                "the provider answered and has no code for this manifest".to_owned()
            }
            Self::Transient { reason } => reason.clone(),
            Self::NoProvider => "no [manifest] providers entry is configured".to_owned(),
        }
    }
}

/// What the provider chain's answer amounts to for this host.
pub fn from_completed(completed: &CompletedFetch) -> Outcome {
    if completed.code > 0 {
        return Outcome::Served {
            code: completed.code,
            provider: completed
                .provider
                .clone()
                .unwrap_or_else(|| UNKNOWN_PROVIDER.to_owned()),
        };
    }
    Outcome::Transient {
        reason: "the provider chain finished without a code; every refusal (transport, \
                 rate limit, a 401 with no code, an unparsable body) reaches this host \
                 as the chain's own zero outcome, and retrying is correct for all of them"
            .to_owned(),
    }
}

/// How many `(target, class)` pairs are remembered for the log-once rule.
///
/// A bounded set, not a map: the worker only ever asks "was this already
/// reported", so the oldest pair falling out costs one repeated line, never a
/// wrong decision.
pub const SEEN_CAPACITY: usize = 64;

/// The bounded "this failure was already reported" set.
#[derive(Debug, Default)]
pub struct Seen {
    kept: VecDeque<(Request, Class)>,
}

impl Seen {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this `(target, class)` pair is new, recording it when it is.
    pub fn first(&mut self, request: Request, class: Class) -> bool {
        if self.kept.contains(&(request, class)) {
            return false;
        }
        if self.kept.len() == SEEN_CAPACITY {
            self.kept.pop_front();
        }
        self.kept.push_back((request, class));
        true
    }

    /// How many pairs are held, for the bound's own test.
    pub fn len(&self) -> usize {
        self.kept.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kept.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::netpacket::decision::RequestTarget;

    fn request(gid: u64) -> Request {
        Request {
            app: 480,
            target: RequestTarget { depot: 731, gid },
        }
    }

    fn completed(code: u64, provider: Option<&str>) -> CompletedFetch {
        CompletedFetch {
            job_id: 1,
            gid: 7,
            code,
            provider: provider.map(str::to_owned),
            req_hdr_bytes: Vec::new(),
            response_generation: 0,
        }
    }

    #[test]
    fn a_served_code_carries_the_provider_that_served_it() {
        let outcome = from_completed(&completed(0xabc, Some("Configured.Host")));
        assert_eq!(
            outcome,
            Outcome::Served {
                code: 0xabc,
                provider: "Configured.Host".to_owned()
            }
        );
        assert_eq!(outcome.tally(), Tally::Served);
        assert_eq!(outcome.class(), None, "a served code is not a failure");
        assert!(outcome.reason().contains("Configured.Host"));
    }

    #[test]
    fn a_code_without_a_named_provider_is_still_inserted_under_a_name() {
        let outcome = from_completed(&completed(1, None));
        assert_eq!(
            outcome,
            Outcome::Served {
                code: 1,
                provider: UNKNOWN_PROVIDER.to_owned()
            }
        );
    }

    #[test]
    fn a_collapsed_refusal_is_transient_and_says_why() {
        let outcome = from_completed(&completed(0, None));
        assert_eq!(outcome.tally(), Tally::Transient);
        assert_eq!(outcome.class(), Some(Class::Transient));
        assert!(outcome.reason().contains("finished without a code"));
    }

    #[test]
    fn the_log_once_set_reports_each_pair_exactly_once() {
        let mut seen = Seen::new();
        assert!(seen.first(request(1), Class::Transient));
        assert!(!seen.first(request(1), Class::Transient));
        // A different class for the same target is a different line.
        assert!(seen.first(request(1), Class::NotArchived));
        // A different target is a different line.
        assert!(seen.first(request(2), Class::Transient));
        assert_eq!(seen.len(), 3);
    }

    #[test]
    fn the_log_once_set_is_bounded() {
        let mut seen = Seen::new();
        for gid in 0..(SEEN_CAPACITY as u64 * 2) {
            assert!(seen.first(request(gid), Class::Transient));
        }
        assert_eq!(seen.len(), SEEN_CAPACITY);
        // The oldest pair fell out, so it is reported again — a repeated line,
        // never a wrong decision.
        assert!(seen.first(request(0), Class::Transient));
        assert_eq!(seen.len(), SEEN_CAPACITY);
    }
}
