//! Concurrent play while a lender is playing.
//!
//! Steam refuses to start a family-shared app while the member who owns it is
//! playing. `[apps.shared]` is the opt-out: with the flag on, the app starts and
//! the family lock is ignored. With the flag off this decision must reproduce
//! the pre-fix behaviour exactly — our own injected apps still bypass (that is
//! the ownership feature, not the family unlock) and everything else keeps
//! Steam's refusal.
//!
//! The decision always carries its reason, so a launch attempt costs exactly one
//! log line and never has to be re-derived.
//!
//! The caller still owns the capability gate: `Capability::FamilyLockBypass`
//! must be enabled before `allowed` is acted on, and disabling it must leave
//! Steam's refusal in place. This module never consults that gate itself — the
//! host layer is the only place a capability is known.

use crate::family::provenance::Provenance;
use vapor_forge_config::{AppId, RuntimeConfig};

/// The family-lock state for one app, as the last snapshot reported it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FamilyLock {
    /// Steam would refuse: another member holds the app.
    pub locked_by_lender: bool,
    /// The account holding it, when the snapshot named one.
    pub lender: Option<u32>,
}

impl FamilyLock {
    /// No lender holds the app.
    pub const fn free() -> Self {
        Self {
            locked_by_lender: false,
            lender: None,
        }
    }

    /// A lender holds the app; `lender` is the account id when it is known.
    pub const fn held_by(lender: Option<u32>) -> Self {
        Self {
            locked_by_lender: true,
            lender,
        }
    }
}

/// Why the decision came out the way it did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayReason {
    /// Nothing holds the app, so there was nothing to unlock.
    NoFamilyLock,
    /// We injected the app's ownership; the family lock is not Steam's to hold.
    ControlledApp,
    /// `[apps.shared]` allows this app.
    UnlockConfigured,
    /// `[apps.shared] enabled = false`: the pre-family-share behaviour.
    SharingDisabled,
    /// `[apps.shared]` is on, but its include/exclude list refuses this app.
    PolicyRefuses,
}

impl PlayReason {
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoFamilyLock => "no-family-lock",
            Self::ControlledApp => "controlled-app",
            Self::UnlockConfigured => "unlock-configured",
            Self::SharingDisabled => "sharing-disabled",
            Self::PolicyRefuses => "policy-refuses",
        }
    }
}

/// Whether the app may start, and why.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConcurrentPlayDecision {
    pub allowed: bool,
    /// Always populated: the caller logs it and nothing else.
    pub reason: PlayReason,
}

impl ConcurrentPlayDecision {
    /// Emit the one log line this decision is worth.
    pub fn log(self, app_id: AppId, lock: FamilyLock, real: Provenance) {
        if self.allowed {
            tracing::info!(
                app_id = app_id.0,
                reason = self.reason.label(),
                provenance = real.label(),
                lender = lock.lender,
                "family: concurrent play allowed"
            );
        } else {
            tracing::info!(
                app_id = app_id.0,
                reason = self.reason.label(),
                provenance = real.label(),
                lender = lock.lender,
                "family: concurrent play blocked, Steam's lock stands"
            );
        }
    }
}

/// Decide whether `app_id` may start while a lender holds it.
///
/// Order matters and is asserted by the tests: no lock, then the ownership
/// bypass, then `[apps.shared]`, then the deny reasons. Reading the policy's
/// `enabled` flag only to explain a refusal keeps `SharedSection::allows` the
/// single implementation of the include/exclude rules.
pub fn decide(config: &RuntimeConfig, app_id: AppId, lock: FamilyLock) -> ConcurrentPlayDecision {
    if !lock.locked_by_lender {
        return ConcurrentPlayDecision {
            allowed: true,
            reason: PlayReason::NoFamilyLock,
        };
    }
    if config.is_controlled_app(app_id) {
        return ConcurrentPlayDecision {
            allowed: true,
            reason: PlayReason::ControlledApp,
        };
    }
    if config.should_bypass_sharing(app_id) {
        return ConcurrentPlayDecision {
            allowed: true,
            reason: PlayReason::UnlockConfigured,
        };
    }
    let reason = if config.apps.shared.enabled {
        PlayReason::PolicyRefuses
    } else {
        PlayReason::SharingDisabled
    };
    ConcurrentPlayDecision {
        allowed: false,
        reason,
    }
}

#[cfg(test)]
mod tests;
