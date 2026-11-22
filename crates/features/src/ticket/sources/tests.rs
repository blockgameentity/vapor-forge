//! Tests for [`super`].

use super::*;
use std::collections::HashMap;

use crate::ticket::cache::TicketCache;

enum Answer {
    Ticket(Vec<u8>),
    Nothing,
    Fail(&'static str),
}

struct Fake {
    kind: TicketSourceKind,
    answer: Answer,
}

impl Fake {
    fn new(kind: TicketSourceKind, answer: Answer) -> Self {
        Self { kind, answer }
    }
}

impl TicketSource for Fake {
    fn kind(&self) -> TicketSourceKind {
        self.kind
    }

    fn ticket(&self, app: AppId) -> Result<Option<Ticket>, SourceFailure> {
        match &self.answer {
            Answer::Ticket(bytes) => Ok(Some(Ticket::new(app, bytes.clone()))),
            Answer::Nothing => Ok(None),
            Answer::Fail(reason) => Err(SourceFailure::new(*reason)),
        }
    }
}

fn chain(order: &[TicketSourceKind]) -> SourceChain {
    SourceChain::new(order.iter().copied()).expect("order should be valid")
}

#[test]
fn source_order_is_respected_and_first_hit_wins() {
    let cache_hit = Fake::new(TicketSourceKind::LocalCache, Answer::Ticket(vec![0x01]));
    let store_hit = Fake::new(
        TicketSourceKind::CredentialStore,
        Answer::Ticket(vec![0x02]),
    );
    let registry_hit = Fake::new(TicketSourceKind::Registry, Answer::Ticket(vec![0x03]));
    let sources = TicketSources::new()
        .with(&cache_hit)
        .with(&store_hit)
        .with(&registry_hit);

    let registry_first = chain(&[TicketSourceKind::Registry, TicketSourceKind::LocalCache]);
    let resolved = registry_first
        .resolve(AppId(480), &sources)
        .expect("registry has a ticket");
    assert_eq!(resolved.source, TicketSourceKind::Registry);
    assert_eq!(resolved.ticket.bytes(), &[0x03]);
    assert_eq!(resolved.attempts.len(), 1);
    assert!(resolved.attempts[0].is_hit());

    // The same sources with the default order answer from the cache.
    let cache_first =
        SourceChain::from_config(&TicketSection::default()).expect("default order is valid");
    let resolved = cache_first
        .resolve(AppId(480), &sources)
        .expect("cache has a ticket");
    assert_eq!(resolved.source, TicketSourceKind::LocalCache);
    assert_eq!(resolved.ticket.bytes(), &[0x01]);
}

#[test]
fn misses_and_failures_are_skipped_and_recorded() {
    let cache_miss = Fake::new(TicketSourceKind::LocalCache, Answer::Nothing);
    let store_failure = Fake::new(
        TicketSourceKind::CredentialStore,
        Answer::Fail("credential store locked"),
    );
    let registry_hit = Fake::new(TicketSourceKind::Registry, Answer::Ticket(vec![0x09]));
    let sources = TicketSources::new()
        .with(&cache_miss)
        .with(&store_failure)
        .with(&registry_hit);

    let resolved = chain(&[
        TicketSourceKind::LocalCache,
        TicketSourceKind::CredentialStore,
        TicketSourceKind::Registry,
    ])
    .resolve(AppId(730), &sources)
    .expect("registry has a ticket");

    assert_eq!(resolved.source, TicketSourceKind::Registry);
    assert_eq!(
        resolved.attempts,
        vec![
            SourceAttempt {
                source: TicketSourceKind::LocalCache,
                outcome: SourceOutcome::Miss,
            },
            SourceAttempt {
                source: TicketSourceKind::CredentialStore,
                outcome: SourceOutcome::Failed {
                    reason: "credential store locked".to_owned(),
                },
            },
            SourceAttempt {
                source: TicketSourceKind::Registry,
                outcome: SourceOutcome::Hit,
            },
        ]
    );
}

#[test]
fn exhausted_chain_reports_every_attempt() {
    let cache_miss = Fake::new(TicketSourceKind::LocalCache, Answer::Nothing);
    let registry_failure = Fake::new(TicketSourceKind::Registry, Answer::Fail("no session"));
    let sources = TicketSources::new()
        .with(&cache_miss)
        .with(&registry_failure);

    let error = chain(&[TicketSourceKind::LocalCache, TicketSourceKind::Registry])
        .resolve(AppId(480), &sources)
        .expect_err("no source has a ticket");

    assert_eq!(error.app, AppId(480));
    assert_eq!(error.attempts.len(), 2);
    assert_eq!(error.attempts[0].outcome, SourceOutcome::Miss);
    assert!(matches!(
        error.attempts[1].outcome,
        SourceOutcome::Failed { .. }
    ));
}

#[test]
fn source_answering_for_a_different_app_is_rejected() {
    // The fake answers for whatever app it is asked for; this one is built to
    // answer for a fixed app so the guard is exercised.
    struct WrongApp;

    impl TicketSource for WrongApp {
        fn kind(&self) -> TicketSourceKind {
            TicketSourceKind::CredentialStore
        }

        fn ticket(&self, _app: AppId) -> Result<Option<Ticket>, SourceFailure> {
            Ok(Some(Ticket::new(AppId(1), vec![0x0F])))
        }
    }

    let wrong = WrongApp;
    let registry_hit = Fake::new(TicketSourceKind::Registry, Answer::Ticket(vec![0x0B]));
    let sources = TicketSources::new().with(&wrong).with(&registry_hit);

    let resolved = chain(&[
        TicketSourceKind::CredentialStore,
        TicketSourceKind::Registry,
    ])
    .resolve(AppId(480), &sources)
    .expect("the registry has a ticket");

    assert_eq!(resolved.source, TicketSourceKind::Registry);
    assert_eq!(resolved.ticket.app(), AppId(480));
    assert!(matches!(
        resolved.attempts[0].outcome,
        SourceOutcome::Failed { .. }
    ));
}

#[test]
fn configured_source_without_an_implementation_is_recorded() {
    let cache_hit = Fake::new(TicketSourceKind::LocalCache, Answer::Ticket(vec![0x0A]));
    let sources = TicketSources::new().with(&cache_hit);

    let resolved = chain(&[
        TicketSourceKind::CredentialStore,
        TicketSourceKind::LocalCache,
    ])
    .resolve(AppId(480), &sources)
    .expect("cache has a ticket");

    assert_eq!(
        resolved.attempts[0].outcome,
        SourceOutcome::Unavailable,
        "a configured-but-unwired source must be recorded, not skipped silently"
    );
    assert_eq!(resolved.source, TicketSourceKind::LocalCache);
}

#[test]
fn repeated_source_is_rejected_when_the_chain_is_built() {
    let error = SourceChain::new([
        TicketSourceKind::LocalCache,
        TicketSourceKind::Registry,
        TicketSourceKind::LocalCache,
    ])
    .expect_err("a repeated source must be rejected");
    assert_eq!(
        error,
        ChainConfigError::RepeatedSource {
            kind: TicketSourceKind::LocalCache
        }
    );
    assert!(error.to_string().contains("local_cache"));
    assert!(error.to_string().contains("more than once"));
}

#[test]
fn empty_order_is_rejected_when_the_chain_is_built() {
    let error = SourceChain::new([]).expect_err("an empty order must be rejected");
    assert_eq!(error, ChainConfigError::EmptyOrder);
    assert!(error.to_string().contains("empty"));
}

#[test]
fn default_config_chain_is_the_historical_lookup() {
    let cache = TicketCache::new(None);
    cache.store_app_ticket(AppId(480), vec![0x11, 0x22], false);
    let lua_tickets = HashMap::new();
    let source = LocalCacheSource::new(&cache, &lua_tickets);
    let sources = TicketSources::new().with(&source);

    let resolved = SourceChain::from_config(&TicketSection::default())
        .expect("default order is valid")
        .resolve(AppId(480), &sources)
        .expect("cache has the ticket");

    assert_eq!(
        resolved.ticket.bytes(),
        cache
            .get_app_ticket(AppId(480), &lua_tickets)
            .expect("cache has the ticket")
            .as_slice()
    );
    assert_eq!(resolved.source, TicketSourceKind::LocalCache);
}

#[test]
fn ticket_debug_never_renders_the_bytes() {
    let ticket = Ticket::new(AppId(480), vec![0xDE, 0xAD, 0xBE, 0xEF]);
    let rendered = format!("{ticket:?}");
    assert!(rendered.contains("bytes_redacted"));
    assert!(!rendered.contains("222"));
    assert!(!rendered.contains("dead"));
}
