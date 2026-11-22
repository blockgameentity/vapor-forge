//! A fetcher that answers from a script, for the tests that drive the worker.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use super::super::outcome::Outcome;
use super::super::{Fetcher, Request};

/// A fetcher that answers from a script and reports every call.
pub(crate) struct Scripted {
    script: Arc<Mutex<VecDeque<Outcome>>>,
    calls: Arc<Mutex<Vec<Request>>>,
    started: Sender<Request>,
    release: Option<Receiver<()>>,
}

impl Scripted {
    pub(crate) fn new(
        script: Vec<Outcome>,
        release: Option<Receiver<()>>,
    ) -> (Self, Arc<Mutex<Vec<Request>>>, Receiver<Request>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let (started, steps) = std::sync::mpsc::channel();
        (
            Self {
                script: Arc::new(Mutex::new(script.into())),
                calls: Arc::clone(&calls),
                started,
                release,
            },
            calls,
            steps,
        )
    }
}

impl Fetcher for Scripted {
    fn fetch(&mut self, request: Request) -> Outcome {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(request);
        // Sent before the script is popped so a held-open fetch reports the
        // request it is holding.
        let _ = self.started.send(request);
        if let Some(release) = self.release.take() {
            // Blocks until the test lets go: this is how "the fetcher is held
            // open" is expressed without a sleep.
            let _ = release.recv();
        }
        self.script
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pop_front()
            .unwrap_or(Outcome::Transient {
                reason: "the test script ran out of answers".to_owned(),
            })
    }
}
