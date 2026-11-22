//! Ordered ticket sources.
//!
//! A ticket request walks a configured order of sources. Every attempt is
//! recorded (hit, miss, failure, or "no implementation wired on this host"), the
//! walk stops at the first hit, and the result names the source that answered.
//! The order is validated when the chain is built from config, so a repeated or
//! empty order is a load error and never reaches a running game.

use std::collections::HashSet;

use tracing::{debug, info, warn};
use vapor_forge_config::{AppId, TicketSection, TicketSourceKind};

mod local_cache;
mod ticket;

pub use local_cache::LocalCacheSource;
pub use ticket::{SourceFailure, Ticket};

/// What one source did when asked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SourceOutcome {
    /// This source produced the ticket; the walk stopped here.
    Hit,
    /// The source answered, and had nothing for this app.
    Miss,
    /// The source itself failed. The walk continues and the reason is kept.
    Failed { reason: String },
    /// The source is configured, but this host has no implementation for it.
    Unavailable,
}

/// One source's attempt, in walk order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceAttempt {
    pub source: TicketSourceKind,
    pub outcome: SourceOutcome,
}

impl SourceAttempt {
    pub fn is_hit(&self) -> bool {
        self.outcome == SourceOutcome::Hit
    }
}

/// One place a ticket can come from.
pub trait TicketSource {
    fn kind(&self) -> TicketSourceKind;

    /// `Ok(Some)` = this source has the ticket, `Ok(None)` = it has nothing for
    /// this app, `Err` = the source failed and the walk should move on.
    fn ticket(&self, app: AppId) -> Result<Option<Ticket>, SourceFailure>;
}

/// The implementations wired for one host, looked up by kind.
#[derive(Default)]
pub struct TicketSources<'a> {
    sources: Vec<&'a dyn TicketSource>,
}

impl<'a> TicketSources<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one implementation. The first registration of a kind wins; a
    /// second one is logged and ignored rather than silently replacing it.
    pub fn with(mut self, source: &'a dyn TicketSource) -> Self {
        self.push(source);
        self
    }

    pub fn push(&mut self, source: &'a dyn TicketSource) {
        if self.find(source.kind()).is_some() {
            warn!(
                source = source.kind().name(),
                "ticket: duplicate source implementation ignored"
            );
            return;
        }
        self.sources.push(source);
    }

    pub fn find(&self, kind: TicketSourceKind) -> Option<&'a dyn TicketSource> {
        self.sources
            .iter()
            .copied()
            .find(|source| source.kind() == kind)
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

/// The configured order, validated.
#[derive(Clone, Debug)]
pub struct SourceChain {
    order: Vec<TicketSourceKind>,
}

/// Why a configured order cannot be used. Reported while the config is loaded,
/// with the offending source named.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ChainConfigError {
    #[error("[ticket] source_order is empty: configure at least one ticket source")]
    EmptyOrder,
    #[error(
        "[ticket] source_order lists `{kind}` more than once: \
         each ticket source may appear at most once"
    )]
    RepeatedSource { kind: TicketSourceKind },
}

/// The winning ticket, the source that produced it, and every attempt made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainResolution {
    pub ticket: Ticket,
    pub source: TicketSourceKind,
    pub attempts: Vec<SourceAttempt>,
}

/// No source had a ticket. The attempts say why, per source.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("no ticket source answered for app {app:?} ({} attempted)", attempts.len())]
pub struct NoTicket {
    pub app: AppId,
    pub attempts: Vec<SourceAttempt>,
}

impl SourceChain {
    pub fn new(
        order: impl IntoIterator<Item = TicketSourceKind>,
    ) -> Result<Self, ChainConfigError> {
        let order: Vec<TicketSourceKind> = order.into_iter().collect();
        Self::validate(&order)?;
        Ok(Self { order })
    }

    /// Build the chain a `[ticket]` section asks for.
    pub fn from_config(section: &TicketSection) -> Result<Self, ChainConfigError> {
        Self::new(section.source_order.iter().copied())
    }

    pub fn order(&self) -> &[TicketSourceKind] {
        &self.order
    }

    /// Ask each source in order and stop at the first hit.
    pub fn resolve(
        &self,
        app: AppId,
        sources: &TicketSources<'_>,
    ) -> Result<ChainResolution, NoTicket> {
        let mut attempts = Vec::with_capacity(self.order.len());
        for kind in &self.order {
            let Some(source) = sources.find(*kind) else {
                debug!(
                    app = app.0,
                    source = kind.name(),
                    "ticket: source is configured but not wired on this host"
                );
                attempts.push(SourceAttempt {
                    source: *kind,
                    outcome: SourceOutcome::Unavailable,
                });
                continue;
            };
            match source.ticket(app) {
                Ok(Some(ticket)) if ticket.app() != app => {
                    // A source that answers with another app's ticket is a bug
                    // or worse; serving it would hand Steam the wrong ticket.
                    let reason = format!(
                        "source answered for app {} instead of {}",
                        ticket.app().0,
                        app.0
                    );
                    warn!(
                        app = app.0,
                        source = kind.name(),
                        answered_for = ticket.app().0,
                        "ticket: source answered for a different app, ignored"
                    );
                    attempts.push(SourceAttempt {
                        source: *kind,
                        outcome: SourceOutcome::Failed { reason },
                    });
                }
                Ok(Some(ticket)) => {
                    info!(
                        app = app.0,
                        source = kind.name(),
                        bytes = ticket.len(),
                        "ticket: source answered"
                    );
                    attempts.push(SourceAttempt {
                        source: *kind,
                        outcome: SourceOutcome::Hit,
                    });
                    return Ok(ChainResolution {
                        ticket,
                        source: *kind,
                        attempts,
                    });
                }
                Ok(None) => {
                    debug!(
                        app = app.0,
                        source = kind.name(),
                        "ticket: source has no ticket for this app"
                    );
                    attempts.push(SourceAttempt {
                        source: *kind,
                        outcome: SourceOutcome::Miss,
                    });
                }
                Err(failure) => {
                    warn!(
                        app = app.0,
                        source = kind.name(),
                        reason = failure.reason.as_str(),
                        "ticket: source failed, trying the next source"
                    );
                    attempts.push(SourceAttempt {
                        source: *kind,
                        outcome: SourceOutcome::Failed {
                            reason: failure.reason,
                        },
                    });
                }
            }
        }
        Err(NoTicket { app, attempts })
    }

    fn validate(order: &[TicketSourceKind]) -> Result<(), ChainConfigError> {
        if order.is_empty() {
            return Err(ChainConfigError::EmptyOrder);
        }
        let mut seen = HashSet::with_capacity(order.len());
        for source in order {
            if !seen.insert(*source) {
                return Err(ChainConfigError::RepeatedSource { kind: *source });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
