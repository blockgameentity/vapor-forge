//! The ticket value and the failure a source reports.

use std::fmt;

use vapor_forge_config::AppId;

/// One ownership ticket. Memory only, and never rendered: `Debug` reports the
/// length instead of the bytes so no log line can carry ticket material.
#[derive(Clone, Eq, PartialEq)]
pub struct Ticket {
    app: AppId,
    bytes: Vec<u8>,
}

impl Ticket {
    pub fn new(app: AppId, bytes: Vec<u8>) -> Self {
        Self { app, bytes }
    }

    /// The app the ticket was issued for.
    pub fn app(&self) -> AppId {
        self.app
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
}

impl fmt::Debug for Ticket {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ticket")
            .field("app", &self.app)
            .field("bytes_redacted", &self.bytes.len())
            .finish()
    }
}

/// A source that could not answer at all (as opposed to having no ticket).
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("{reason}")]
pub struct SourceFailure {
    pub reason: String,
}

impl SourceFailure {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}
