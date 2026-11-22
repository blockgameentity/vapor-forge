//! Denuvo authorisation as a time-bounded lease.
//!
//! No clock is read here: the caller passes the current instant (a `Duration`
//! since an epoch of its choosing), so every boundary can be tested exactly and
//! the logic stays deterministic.
//!
//! The states are [`AuthorisationState::Valid`], [`AuthorisationState::Expired`]
//! and [`AuthorisationState::Never`]. An expired (or never acquired) lease is
//! never reported as valid, and a clock that reports an instant *before*
//! acquisition counts as expired too: it cannot prove the lease is live, so the
//! safe answer is "acquire again".

use std::time::Duration;

use vapor_forge_config::{DenuvoMode, TicketSection};

/// Where a lease stands at one instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthorisationState {
    /// Acquired, and still inside its window.
    Valid { remaining: Duration },
    /// The window lapsed at `expired_at` (same epoch as the clock passed in).
    /// The caller has to acquire again before authorising.
    Expired { expired_at: Duration },
    /// Nothing has been acquired in this process yet.
    Never,
}

impl AuthorisationState {
    /// True only inside the window. This is the only way a caller should ask.
    pub fn is_valid(self) -> bool {
        matches!(self, Self::Valid { .. })
    }

    /// True when the caller must acquire (or renew) before authorising.
    pub fn needs_reacquisition(self) -> bool {
        !self.is_valid()
    }
}

/// An authorisation with an acquisition instant and an explicit validity
/// window. Constructing one does not authorise anything by itself; ask
/// [`AuthorisationWindow::state_at`] with the current instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthorisationWindow {
    acquired_at: Option<Duration>,
    validity: Duration,
}

impl AuthorisationWindow {
    /// Nothing acquired.
    pub fn never() -> Self {
        Self {
            acquired_at: None,
            validity: Duration::ZERO,
        }
    }

    /// A lease acquired at `at` that is valid for `validity`.
    ///
    /// A zero-length window is accepted and expires immediately: it is the
    /// honest answer for a misconfigured lease, and it can never be mistaken
    /// for a live one.
    pub fn acquire(at: Duration, validity: Duration) -> Self {
        Self {
            acquired_at: Some(at),
            validity,
        }
    }

    /// Replace the lease with a fresh one (the re-acquire path after expiry).
    pub fn renew(&mut self, at: Duration, validity: Duration) {
        *self = Self::acquire(at, validity);
    }

    /// Drop the lease entirely; the state becomes [`AuthorisationState::Never`].
    pub fn clear(&mut self) {
        *self = Self::never();
    }

    pub fn validity(&self) -> Duration {
        self.validity
    }

    pub fn acquired_at(&self) -> Option<Duration> {
        self.acquired_at
    }

    /// The instant the lease expires, when one was acquired.
    pub fn expires_at(&self) -> Option<Duration> {
        self.acquired_at
            .map(|acquired_at| acquired_at.saturating_add(self.validity))
    }

    /// The state at `now`, with the boundary instants spelled out: a lease is
    /// valid from its acquisition instant up to, but not including, its expiry.
    pub fn state_at(&self, now: Duration) -> AuthorisationState {
        let Some(acquired_at) = self.acquired_at else {
            return AuthorisationState::Never;
        };
        let expired_at = acquired_at.saturating_add(self.validity);
        // `now < acquired_at` is a clock that moved backwards. The lease may or
        // may not have lapsed; we cannot prove either way, so it is reported as
        // expired (the caller re-acquires, which is always safe).
        if now < acquired_at || now >= expired_at {
            return AuthorisationState::Expired { expired_at };
        }
        AuthorisationState::Valid {
            remaining: expired_at - now,
        }
    }

    pub fn is_valid_at(&self, now: Duration) -> bool {
        self.state_at(now).is_valid()
    }
}

/// The lease a `[ticket]` section asks for at `acquired_at`, or `None` while
/// `denuvo_mode` is `off` (the historical one-shot behaviour: no window exists).
pub fn from_config(section: &TicketSection, acquired_at: Duration) -> Option<AuthorisationWindow> {
    match section.denuvo_mode {
        DenuvoMode::Off => None,
        DenuvoMode::Lease => Some(AuthorisationWindow::acquire(
            acquired_at,
            Duration::from_secs(section.denuvo_lease_secs),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vapor_forge_config::DenuvoMode;

    const VALIDITY: Duration = Duration::from_secs(10);

    fn acquired() -> AuthorisationWindow {
        AuthorisationWindow::acquire(Duration::from_secs(100), VALIDITY)
    }

    #[test]
    fn never_acquired_is_never_valid() {
        let window = AuthorisationWindow::never();
        assert_eq!(
            window.state_at(Duration::from_secs(0)),
            AuthorisationState::Never
        );
        assert!(!window.is_valid_at(Duration::from_secs(0)));
        assert!(window
            .state_at(Duration::from_secs(0))
            .needs_reacquisition());
        assert_eq!(window.expires_at(), None);
    }

    #[test]
    fn state_transitions_at_the_expiry_boundary() {
        let window = acquired();

        // Just before the expiry.
        let just_before = Duration::from_millis(109_999);
        assert_eq!(
            window.state_at(just_before),
            AuthorisationState::Valid {
                remaining: Duration::from_millis(1)
            }
        );
        assert!(window.is_valid_at(just_before));

        // Exactly at the expiry: the window is closed.
        let at_expiry = Duration::from_secs(110);
        assert_eq!(
            window.state_at(at_expiry),
            AuthorisationState::Expired {
                expired_at: at_expiry
            }
        );
        assert!(!window.is_valid_at(at_expiry));
        assert!(window.state_at(at_expiry).needs_reacquisition());

        // Just after the expiry.
        let just_after = Duration::from_millis(110_001);
        assert_eq!(
            window.state_at(just_after),
            AuthorisationState::Expired {
                expired_at: at_expiry
            }
        );
        assert!(!window.is_valid_at(just_after));
    }

    #[test]
    fn expired_is_never_reported_as_valid() {
        let window = acquired();
        for millis in [110_000, 110_001, 150_000, 3_600_000] {
            let now = Duration::from_millis(millis);
            let state = window.state_at(now);
            assert_eq!(
                state,
                AuthorisationState::Expired {
                    expired_at: Duration::from_millis(110_000)
                },
                "state at {millis}ms"
            );
            assert!(!state.is_valid());
            assert!(state.needs_reacquisition());
        }
    }

    #[test]
    fn clock_before_acquisition_is_not_treated_as_valid() {
        let window = acquired();
        let before = Duration::from_secs(99);
        assert_eq!(
            window.state_at(before),
            AuthorisationState::Expired {
                expired_at: Duration::from_secs(110)
            }
        );
        assert!(!window.is_valid_at(before));
    }

    #[test]
    fn renew_restarts_the_window() {
        let mut window = acquired();
        assert!(!window.is_valid_at(Duration::from_secs(120)));

        window.renew(Duration::from_secs(200), VALIDITY);

        assert!(window.is_valid_at(Duration::from_secs(200)));
        assert_eq!(
            window.state_at(Duration::from_secs(205)),
            AuthorisationState::Valid {
                remaining: Duration::from_secs(5)
            }
        );
        assert!(!window.is_valid_at(Duration::from_secs(210)));
        assert_eq!(window.acquired_at(), Some(Duration::from_secs(200)));
        assert_eq!(window.expires_at(), Some(Duration::from_secs(210)));
    }

    #[test]
    fn clear_returns_to_never() {
        let mut window = acquired();
        window.clear();
        assert_eq!(
            window.state_at(Duration::from_secs(100)),
            AuthorisationState::Never
        );
    }

    #[test]
    fn zero_length_lease_expires_immediately() {
        let window = AuthorisationWindow::acquire(Duration::from_secs(50), Duration::ZERO);
        assert_eq!(
            window.state_at(Duration::from_secs(50)),
            AuthorisationState::Expired {
                expired_at: Duration::from_secs(50)
            }
        );
        assert!(!window.is_valid_at(Duration::from_secs(50)));
    }

    #[test]
    fn lease_mode_is_opt_in_and_off_by_default() {
        let default_section = TicketSection::default();
        assert_eq!(
            from_config(&default_section, Duration::from_secs(100)),
            None,
            "default config must not gain a lease"
        );

        let section = TicketSection {
            denuvo_mode: DenuvoMode::Lease,
            denuvo_lease_secs: 90,
            ..TicketSection::default()
        };
        let window = from_config(&section, Duration::from_secs(100)).expect("lease mode is on");
        assert_eq!(window.validity(), Duration::from_secs(90));
        assert!(window.is_valid_at(Duration::from_secs(189)));
        assert!(!window.is_valid_at(Duration::from_secs(190)));
    }
}
