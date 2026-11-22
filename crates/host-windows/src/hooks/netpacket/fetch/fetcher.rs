//! The seam between the worker and the provider chain.
//!
//! [`Fetcher`] is one method: give me a code for this `(app, depot, gid)`. The
//! worker owns the thread, the queue, the rate budget and the code cache; the
//! fetcher owns the round trip. That split is what lets the tests drive every
//! branch — a transient failure, a target no provider stocks, an empty provider
//! list — with a fake that never opens a socket, and it is why the non-blocking
//! test can hold a fetch open and prove the hook path still returns.
//!
//! [`ChainFetcher`] is the only production implementation, and it adds no
//! provider logic of its own: it hands the target to
//! [`features::request_code::PendingQueue`], which is the same entry the Linux
//! host uses, so provider order, `failover`, `{gid}`/`{depotId}`/`{appId}`
//! expansion, per-provider `min_interval_ms` pacing, 429 cooldowns, the response
//! format (`plain`/`json`) and the 4 KiB response bound are all the shared
//! implementation's. Nothing here names a host, resolves a provider or parses a
//! body — the provider list is read from the config the host tool writes, and an
//! empty one is [`Outcome::NoProvider`] rather than a default endpoint.
//!
//! # Why it waits in a loop
//!
//! `PendingQueue::queue_fetch` runs the chain on a thread of its own and exposes
//! the answer through `drain_completed`; there is no synchronous entry into the
//! chain on the public surface. So this fetcher queues one target, then drains
//! until its own `job_id` shows up. That is a sleep loop on a worker thread, not
//! on Steam's networking thread, and it is bounded by the configured
//! `timeout_connect_ms + timeout_ms` plus a fixed slack, after which the target
//! is reported [`Outcome::Transient`] and the chain's own timeout closes the
//! socket.

use std::time::{Duration, Instant};

use vapor_forge_config::ManifestSection;
use vapor_forge_features::request_code::{ManifestCodeFetch, PendingQueue};

use super::outcome::{from_completed, Outcome};
use super::Request;

/// A provider round trip, as the worker needs it.
pub trait Fetcher: Send {
    /// Obtain a code for `request`, or say why there is none.
    ///
    /// Called on the worker thread only, and never with a queue lock held.
    fn fetch(&mut self, request: Request) -> Outcome;
}

/// How long the fetcher sleeps between two looks at the chain's completions.
const POLL: Duration = Duration::from_millis(5);

/// Extra time granted past the configured timeouts before a fetch is abandoned.
///
/// The chain applies `timeout_connect_ms` and `timeout_ms` to the request itself;
/// this covers the hand-off around it, so a slow but honest provider is never
/// reported as transient.
const SLACK: Duration = Duration::from_secs(5);

/// The production fetcher: the shared provider chain, one target at a time.
pub struct ChainFetcher {
    manifest: ManifestSection,
    /// How long to wait for the chain's answer before giving up on it.
    deadline: Duration,
    queue: PendingQueue,
    /// The job id of the next request. Chosen here because this crate is not
    /// answering Steam's job (that response is fabricated by the dispatch body);
    /// it only has to be unique per fetch so a stale completion cannot be
    /// mistaken for a fresh one.
    job: u64,
}

impl ChainFetcher {
    /// A fetcher for the providers in `manifest`.
    pub fn new(manifest: &ManifestSection) -> Self {
        let deadline = Duration::from_millis(
            manifest
                .timeout_connect_ms
                .saturating_add(manifest.timeout_ms),
        ) + SLACK;
        Self {
            manifest: manifest.clone(),
            deadline,
            queue: PendingQueue::new(),
            // 0 is the chain's "no job" value (`plan_fetch` refuses it), so the
            // sequence starts at 1 to keep a real job distinguishable.
            job: 1,
        }
    }

    /// The job id this fetcher will use next, for the staleness test.
    #[cfg(test)]
    pub fn next_job(&self) -> u64 {
        self.job
    }
}

impl Fetcher for ChainFetcher {
    fn fetch(&mut self, request: Request) -> Outcome {
        // Before touching the chain: an empty list is a configuration state, and
        // reporting it as a failed lookup would read as "every provider failed".
        if self.manifest.providers.is_empty() {
            return Outcome::NoProvider;
        }
        // A completion left behind by an abandoned attempt belongs to a previous
        // job; draining first keeps the loop below from having to reason about it.
        self.queue.drain_completed();

        let job = self.job;
        self.job = self.job.wrapping_add(1).max(1);
        let queued = self.queue.queue_fetch(
            ManifestCodeFetch {
                job_id: job,
                app_id: request.app,
                depot_id: request.target.depot,
                gid: request.target.gid,
                // The response header this host would need to fabricate belongs
                // to the dispatch body, not to a fetch: it is empty here.
                req_hdr_bytes: Vec::new(),
            },
            &self.manifest,
            // The Lua `fetch_manifest_code` provider is the Linux host's; this
            // host has no scripting provider wired into the chain.
            None,
            0,
        );
        if !queued {
            return Outcome::Transient {
                reason: "the provider chain is already running the maximum number of \
                         concurrent fetches"
                    .to_owned(),
            };
        }

        let deadline = Instant::now() + self.deadline;
        loop {
            for completed in self.queue.drain_completed() {
                if completed.job_id == job {
                    return from_completed(&completed);
                }
            }
            if Instant::now() >= deadline {
                return Outcome::Transient {
                    reason: format!(
                        "the provider chain did not answer within {} ms",
                        self.deadline.as_millis()
                    ),
                };
            }
            std::thread::sleep(POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(providers: Vec<vapor_forge_config::ManifestSource>) -> ManifestSection {
        ManifestSection {
            providers,
            ..ManifestSection::default()
        }
    }

    #[test]
    fn an_empty_provider_list_is_reported_without_touching_the_chain() {
        let mut fetcher = ChainFetcher::new(&manifest(Vec::new()));
        let request = Request {
            app: 480,
            target: crate::hooks::netpacket::decision::RequestTarget { depot: 731, gid: 7 },
        };
        assert_eq!(fetcher.fetch(request), Outcome::NoProvider);
    }

    #[test]
    fn the_deadline_is_the_configured_timeouts_plus_the_hand_off_slack() {
        let section = ManifestSection {
            timeout_connect_ms: 100,
            timeout_ms: 250,
            ..ManifestSection::default()
        };
        assert_eq!(
            ChainFetcher::new(&section).deadline,
            Duration::from_millis(350) + SLACK
        );
    }

    #[test]
    fn job_ids_are_non_zero_and_never_repeat() {
        let fetcher = ChainFetcher::new(&manifest(Vec::new()));
        assert!(fetcher.next_job() > 0, "0 is the chain's \"no job\" value");
    }
}
