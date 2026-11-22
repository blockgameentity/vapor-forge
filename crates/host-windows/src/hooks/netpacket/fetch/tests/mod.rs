//! The fetch worker driven by a fake [`Fetcher`]: no socket, no provider, no sleep.
//!
//! Every test here starts the real worker thread with
//! [`worker::testing::Scripted`], which answers from a script instead of the
//! provider chain, and then waits for an observable effect — a code in the cache
//! or a counter — rather than for a duration.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::hooks::netpacket::decision::RequestTarget;
use crate::hooks::tests_support::{config_with, state_with};
use crate::hooks::state::HostState;

use super::outcome::Outcome;
use super::queue::Enqueued;
use super::worker::testing::Scripted;
use super::worker::{self, WorkerHandle};
use super::Request;

/// How long a test waits for the worker to reach an observable state.
const PATIENCE: Duration = Duration::from_secs(5);

pub(super) fn target(gid: u64) -> Request {
    Request {
        app: 480,
        target: RequestTarget { depot: 731, gid },
    }
}

/// A state with the fetch queue open, as the lifecycle leaves it.
pub(super) fn open_state() -> Arc<HostState> {
    let state = Arc::new(state_with(config_with(&[480]), Vec::new()));
    state.code_fetch().queue().open();
    state
}

/// Start the worker with a scripted fetcher, and hand back what it observes.
pub(super) fn start(
    state: &Arc<HostState>,
    script: Vec<Outcome>,
    release: Option<Receiver<()>>,
    min_interval_ms: u64,
) -> (WorkerHandle, Arc<std::sync::Mutex<Vec<Request>>>, Receiver<Request>) {
    let (fetcher, calls, steps) = Scripted::new(script, release);
    let handle = worker::spawn(Arc::clone(state), Box::new(fetcher), min_interval_ms)
        .expect("the test's worker thread must start");
    (handle, calls, steps)
}

/// Poll until `done`, or fail with the counters that were seen last.
pub(super) fn wait_for(state: &Arc<HostState>, what: &str, done: impl Fn(&HostState) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while Instant::now() < deadline {
        if done(state) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}; counters = {:?}", state.code_fetch().queue().counters());
}

pub(super) fn enqueue(state: &Arc<HostState>, request: Request) -> Enqueued {
    state.code_fetch().queue().enqueue(request)
}

mod worker_path;
