//! Whether the fetcher runs, and what it says when it does not.
//!
//! The hook body reads [`Fetch`] from the host state on Steam's networking
//! thread; the lifecycle reads it once at start-up. Everything here is a decision
//! over a snapshot of the config, so the tests drive it without a provider, a
//! thread or a socket.
//!
//! # The one start
//!
//! [`claim`](Fetch::claim) hands out exactly one start per process, and the state
//! is process-wide (see [`crate::hooks::state::publish`]), so "start it from the
//! lifecycle" and "a second start is a no-op" are the same property rather than
//! two that have to agree. A claimed start that ends idle is still claimed: the
//! reason is recorded and reported once, and nothing tries again.
//!
//! # Idle is a configuration state, not a failure
//!
//! A host with no `[manifest] providers` entry has nothing to ask, so the fetcher
//! never runs, the queue never opens, and the body's offer is refused without a
//! round trip. That is the truthful behaviour rather than a default endpoint:
//! this crate names no provider host, and it will not invent one. The reason is
//! logged once at `debug` and carried into the run's notes, so a client that
//! never fetches a code can be told exactly why.

use std::sync::{Arc, Mutex, MutexGuard};

use vapor_forge_config::ManifestSection;

use crate::capability::Capability;
use crate::hooks::state::HostState;

use super::fetcher::{ChainFetcher, Fetcher};
use super::queue::Queue;
use super::worker;

/// How far the fetcher got.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Lifecycle {
    /// No start has been attempted.
    #[default]
    NotStarted,
    /// The worker thread exists and the queue accepts work.
    Running,
    /// The one start happened and decided not to run, for this reason.
    Idle(&'static str),
}

/// What [`start`] did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Start {
    /// The worker is running and the queue accepts work.
    Running,
    /// The worker stays idle for the named reason; nothing will be asked.
    Idle(&'static str),
    /// This process already started (or idled) the fetcher once. A no-op.
    AlreadyStarted,
}

impl Start {
    /// The line the run's report should carry about this outcome, if any.
    ///
    /// An idle fetcher is the one outcome a user cannot see anywhere else: the
    /// dispatch body keeps recording `NoCachedCode`, which is true but says
    /// nothing about *why* no producer exists. `AlreadyStarted` reports nothing,
    /// so the reason appears once however often the lifecycle is entered.
    pub fn report(self) -> Option<String> {
        match self {
            Self::Idle(reason) => Some(format!("request codes: {reason}")),
            Self::Running | Self::AlreadyStarted => None,
        }
    }
}

/// `[manifest] providers` is empty: there is nothing to ask.
pub const NO_PROVIDER: &str = "no [manifest] providers entry is configured, so the host has \
     nothing to ask for a request code and Steam's own round trip continues; the host tool \
     (cheesetool) writes the provider list";

/// Every configured entry is unusable, so the chain would refuse each one.
pub const NO_USABLE_PROVIDER: &str = "every [manifest] providers entry is unusable, so the host \
     has nothing to ask for a request code and Steam's own round trip continues";

/// The body could never serve a code, so a fetched one would be inert.
pub const NO_CAPABILITY: &str = "the request-codes capability is disabled, so a fetched code \
     could never be served";

/// The worker thread could not be created.
pub const NO_THREAD: &str = "the request-code fetcher thread could not be created";

/// The queue and the start decision the hook body and the lifecycle share.
#[derive(Debug)]
pub struct Fetch {
    queue: Queue,
    lifecycle: Mutex<Lifecycle>,
}

impl Fetch {
    pub fn new() -> Self {
        Self {
            queue: Queue::new(),
            lifecycle: Mutex::new(Lifecycle::NotStarted),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Lifecycle> {
        self.lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The queue the dispatch body offers targets to.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// How far the fetcher got, for the report.
    pub fn lifecycle(&self) -> Lifecycle {
        *self.lock()
    }

    /// Take the one start this process gets.
    ///
    /// `false` means a start already happened, whatever it decided.
    fn claim(&self) -> bool {
        let mut lifecycle = self.lock();
        if *lifecycle != Lifecycle::NotStarted {
            return false;
        }
        // Provisional: settled by the caller once the decision is made, so a
        // concurrent second start cannot slip in between.
        *lifecycle = Lifecycle::Running;
        true
    }

    fn settle(&self, lifecycle: Lifecycle) {
        *self.lock() = lifecycle;
    }
}

impl Default for Fetch {
    fn default() -> Self {
        Self::new()
    }
}

/// Start the process-wide fetcher from the resolved config, or explain why not.
pub fn start(state: &Arc<HostState>) -> Start {
    let manifest = state.config().manifest.clone();
    let fetcher: Box<dyn Fetcher> = Box::new(ChainFetcher::new(&manifest));
    start_with(state, &manifest, fetcher)
}

/// Start the worker for `state`, with `fetcher` supplying the codes.
///
/// Called once from [`crate::entry::worker::run`], after the install pass has
/// settled the gate (so `request-codes` is known) and after the config is loaded.
pub fn start_with(
    state: &Arc<HostState>,
    manifest: &ManifestSection,
    fetcher: Box<dyn Fetcher>,
) -> Start {
    if !state.code_fetch().claim() {
        return Start::AlreadyStarted;
    }
    if let Some(reason) = idle_reason(state, manifest) {
        state.code_fetch().settle(Lifecycle::Idle(reason));
        tracing::debug!(
            reason,
            "request-code fetcher is idle; Steam's own request-code round trip continues"
        );
        return Start::Idle(reason);
    }
    let fetch = state.code_fetch();
    fetch.queue().open();
    match worker::spawn(Arc::clone(state), fetcher, manifest.min_interval_ms) {
        Some(handle) => {
            // Detached on purpose: the thread is the process's, it exits on
            // `thread::stop_requested()` at `DLL_PROCESS_DETACH`, and nothing in
            // this host joins it (the same contract as the host worker itself).
            drop(handle);
            fetch.settle(Lifecycle::Running);
            tracing::info!(
                providers = manifest.providers.len(),
                failover = manifest.failover,
                min_interval_ms = manifest.min_interval_ms,
                "request-code fetcher started"
            );
            Start::Running
        }
        None => {
            fetch.queue().close();
            fetch.settle(Lifecycle::Idle(NO_THREAD));
            tracing::debug!(reason = NO_THREAD, "request-code fetcher is idle");
            Start::Idle(NO_THREAD)
        }
    }
}

/// Why the fetcher must not run at all, given the settled state.
fn idle_reason(state: &HostState, manifest: &ManifestSection) -> Option<&'static str> {
    if !state.allows(Capability::RequestCodes) {
        return Some(NO_CAPABILITY);
    }
    if manifest.providers.is_empty() {
        return Some(NO_PROVIDER);
    }
    // Rejections are per entry and the chain skips an unusable one, so this is
    // only "nothing at all to ask" when every entry is unusable.
    if manifest.rejections().len() >= manifest.providers.len() {
        return Some(NO_USABLE_PROVIDER);
    }
    None
}

#[cfg(test)]
mod tests;
