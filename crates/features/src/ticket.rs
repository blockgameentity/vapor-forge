//! Ownership tickets.
//!
//! The concerns are split by what they answer:
//!
//! * [`cache`] — the ticket cache (memory, optional disk) and [`delegate`], the
//!   window in which a captured ticket from a previous owner session is served.
//! * [`sources`] — an ordered chain of ticket sources. Every attempt is
//!   recorded, and the result says which source produced the ticket.
//! * [`authorisation`] — a Denuvo authorisation as a lease with an explicit
//!   acquisition time and expiry, so a lapsed authorisation is reported instead
//!   of being reused.
//! * [`detection`] — structural Denuvo classification plus the manual override
//!   that wins over it.
//! * [`forge`] — deriving one app's ticket from another app's ticket.
//!
//! Nothing here transmits ticket material: every value stays in this process,
//! and [`sources::Ticket`]'s `Debug` impl redacts the bytes so a stray log line
//! cannot leak a ticket.

pub mod authorisation;
pub mod cache;
pub mod delegate;
pub mod detection;
pub mod forge;
pub mod sources;

pub use authorisation::{AuthorisationState, AuthorisationWindow};
pub use cache::TicketCache;
pub use delegate::{
    add_auto_delegate, clear_delegate_steamid, delegate_steamid, in_delegate_window,
    is_auto_delegate, remove_auto_delegate, reset_delegate_window, set_delegate_steamid,
};
pub use detection::{DenuvoDecision, DenuvoDetection, DenuvoTracker, Evidence, VerdictChange};
pub use sources::{
    ChainConfigError, ChainResolution, NoTicket, SourceAttempt, SourceChain, SourceFailure,
    SourceOutcome, Ticket, TicketSource, TicketSources,
};
