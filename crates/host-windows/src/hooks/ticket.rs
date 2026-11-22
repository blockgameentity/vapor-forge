//! `CUser::GetAppTicket` — the ticket hook body.
//!
//! # This hook has no corpus anchor
//!
//! Neither live build names `CUser::GetAppTicket`
//! (`res/patterns/win64/steamclient/669fa08e….toml`,
//! `4a2d7952….toml`), and the vtable report has no candidate for it. The install
//! pass therefore never resolves the symbol, `install::deps` disables the
//! `ticket-overrides` capability with the resolver's reason, and this body is
//! unreachable today. It exists because the decision it performs is the part of
//! the ticket work that can be written and tested without an anchor, and because
//! a future anchor must not have to re-invent it.
//!
//! # What it decides
//!
//! For a ticket request the body asks the two things `features::ticket` owns:
//! the configured **source chain** (`SourceChain::from_config`, asked in order,
//! first hit wins) and the **authorisation window** (`AuthorisationWindow`,
//! acquired once when the host starts, checked at the moment of the request).
//!
//! It never serves bytes. The out-buffer layout of `GetAppTicket` is unmeasured,
//! so writing ticket material into it would be a guess with a credential in it.
//! The body records the decision, and Steam's own answer is returned unchanged.
//!
//! Flags: the `ticket-overrides` capability must have survived the install pass
//! and `[ticket] source_order` must name at least one source. Otherwise the body
//! is a pure fall-through.

use core::ffi::c_void;
use std::time::Duration;

use tracing::debug;
use vapor_forge_config::{AppId, TicketSourceKind};
use vapor_forge_features::ticket::authorisation::AuthorisationState;
use vapor_forge_features::ticket::sources::LocalCacheSource;
use vapor_forge_features::ticket::{SourceChain, TicketSources};

use crate::abi::ticket::GetAppTicketFn;
use crate::capability::Capability;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces. **No corpus entry exists for it.**
pub const SYMBOL: &str = "CUser::GetAppTicket";

/// Address of [`get_app_ticket`], for the installer's replacement table.
pub fn address() -> usize {
    get_app_ticket as GetAppTicketFn as usize
}

/// Replacement for `CUser::GetAppTicket`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`
/// and the register slots must be whatever Steam passed. Unreachable in this
/// phase: the symbol has no anchor, so no detour is ever installed for it.
pub unsafe extern "system" fn get_app_ticket(
    this: *mut c_void,
    app_id: u32,
    out: usize,
    r9: usize,
) -> u8 {
    // SAFETY: the signature is the one documented for this symbol.
    let Some(original) = (unsafe { original::original_of::<GetAppTicketFn>(SYMBOL) }) else {
        debug!(
            app_id,
            "ticket hook reached without a published original; falling through to Steam"
        );
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim.
    let steam = unsafe { original(this, app_id, out, r9) };
    if let Some(state) = state::get() {
        match decide(state, app_id, now()) {
            TicketVerdict::Available { source, bytes } => debug!(
                app_id,
                source = source.name(),
                bytes,
                "ticket: a configured source has this ticket and the authorisation window is open; \
                 serving is not wired, so Steam's own ticket is returned"
            ),
            TicketVerdict::Unavailable { reason } => debug!(
                app_id,
                reason = reason.as_str(),
                "ticket: nothing to serve, Steam's own ticket is returned"
            ),
        }
    }
    steam
}

/// What the chain and the window say about one ticket request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TicketVerdict {
    /// A source answered and the authorisation window is open at this instant.
    Available {
        source: TicketSourceKind,
        bytes: usize,
    },
    /// Nothing may be served, with the reason to log.
    Unavailable { reason: String },
}

/// The decision for `app_id` at `now`.
///
/// `now` is passed in rather than read so every boundary is testable, which is
/// the same contract `features::ticket::authorisation` documents.
pub fn decide(state: &HostState, app_id: u32, now: Duration) -> TicketVerdict {
    if !state.allows(Capability::TicketOverrides) {
        return unavailable(
            state
                .disabled_reason(Capability::TicketOverrides)
                .unwrap_or_else(|| "the ticket-overrides capability is disabled".to_owned()),
        );
    }
    let section = &state.config().ticket;
    if section.source_order.is_empty() {
        return unavailable("[ticket] source_order is empty");
    }

    if let Some(window) = state.authorisation() {
        match window.state_at(now) {
            AuthorisationState::Valid { remaining } => debug!(
                app_id,
                remaining_secs = remaining.as_secs(),
                "ticket: authorisation window is open"
            ),
            state_at => {
                return unavailable(match state_at {
                    AuthorisationState::Expired { expired_at } => {
                        format!("the Denuvo lease expired at {}s", expired_at.as_secs())
                    }
                    _ => "no Denuvo lease has been acquired in this process".to_owned(),
                });
            }
        }
    }

    let chain = match SourceChain::from_config(section) {
        Ok(chain) => chain,
        Err(error) => return unavailable(error.to_string()),
    };
    let local_cache = LocalCacheSource::new(state.ticket_cache(), state.lua_tickets());
    let sources = TicketSources::new().with(&local_cache);
    match chain.resolve(AppId(app_id), &sources) {
        Ok(resolution) => TicketVerdict::Available {
            source: resolution.source,
            bytes: resolution.ticket.len(),
        },
        Err(no_ticket) => unavailable(no_ticket.to_string()),
    }
}

fn unavailable(reason: impl Into<String>) -> TicketVerdict {
    TicketVerdict::Unavailable {
        reason: reason.into(),
    }
}

fn now() -> Duration {
    Duration::from_secs(u64::try_from(vapor_forge_core::unix_now()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with, state_with_authorisation};
    use vapor_forge_config::DenuvoMode;
    use vapor_forge_features::ticket::authorisation::AuthorisationWindow;

    fn state_with_ticket(app: u32) -> HostState {
        let config = config_with(&[app]);
        let state = state_with(config, vec![AppId(app)]);
        state
            .ticket_cache()
            .store_app_ticket(AppId(app), vec![0xaa; 20], false);
        state
    }

    #[test]
    fn the_default_chain_answers_from_the_local_cache() {
        let state = state_with_ticket(900);
        assert_eq!(
            decide(&state, 900, Duration::from_secs(10)),
            TicketVerdict::Available {
                source: TicketSourceKind::LocalCache,
                bytes: 20
            }
        );
    }

    #[test]
    fn an_app_without_a_cached_ticket_reports_why() {
        let state = state_with_ticket(901);
        match decide(&state, 902, Duration::from_secs(10)) {
            TicketVerdict::Unavailable { reason } => {
                assert!(reason.contains("no ticket source answered"), "{reason}");
            }
            other => panic!("expected no ticket, got {other:?}"),
        }
    }

    #[test]
    fn an_expired_lease_is_never_reported_as_available() {
        let mut config = config_with(&[910]);
        config.ticket.denuvo_mode = DenuvoMode::Lease;
        config.ticket.denuvo_lease_secs = 60;
        // Acquired at t=100 with a 60s validity, so t=159 is inside and t=160 is
        // not: the same boundary `features::ticket::authorisation` tests.
        let window =
            AuthorisationWindow::acquire(Duration::from_secs(100), Duration::from_secs(60));
        let state = state_with_authorisation(config, vec![AppId(910)], Some(window));
        state
            .ticket_cache()
            .store_app_ticket(AppId(910), vec![0xbb; 8], false);

        assert!(matches!(
            decide(&state, 910, Duration::from_secs(159)),
            TicketVerdict::Available { bytes: 8, .. }
        ));
        match decide(&state, 910, Duration::from_secs(160)) {
            TicketVerdict::Unavailable { reason } => {
                assert!(reason.contains("expired"), "{reason}");
            }
            other => panic!("expected an expired lease, got {other:?}"),
        }
    }

    #[test]
    fn denuvo_mode_off_has_no_window_at_all() {
        let state = state_with_ticket(903);
        assert!(
            state.authorisation().is_none(),
            "off is the historical one-shot path"
        );
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with_ticket(911);
        let mut gate = state.gate();
        gate.disable(
            Capability::TicketOverrides,
            "CUser::GetAppTicket did not resolve",
        );
        state.set_gate(gate);
        match decide(&state, 911, Duration::from_secs(0)) {
            TicketVerdict::Unavailable { reason } => {
                assert!(reason.contains("did not resolve"), "{reason}");
            }
            other => panic!("expected the install reason, got {other:?}"),
        }
    }
}
