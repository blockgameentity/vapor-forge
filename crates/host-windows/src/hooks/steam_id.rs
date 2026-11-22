//! SteamID override — **decision ready, no anchor**.
//!
//! The Linux host hooks `IClientUser::GetSteamID` and `CUser::GetSteamID`
//! (`crates/hooks/src/client/user.rs`). The first publishes the *real* account for
//! the process; the second is what an IPC request from a game reaches, and that is
//! where a per-game identity is presented: a delegate ticket's owner while its
//! window is open, otherwise the configured `[steam_id]` value for that app,
//! otherwise Steam's own account.
//!
//! Two invariants are inherited from [`crate::features::identity`] and are the
//! reason this decision is worth having on its own:
//!
//! * "no override" is a state, not a missing value — [`PresentedId::Steam`] says
//!   "Steam's own account", it never means zero;
//! * a configured value that is not an individual SteamID64 is refused, never
//!   replaced by a guessed one, and the refusal leaves Steam's answer in place.
//!
//! # Why there is no body here
//!
//! Neither `IClientUser::GetSteamID` nor `CUser::GetSteamID` is in the win64
//! corpus: verified with the miner against both live builds. The function returns a
//! `uint64` in `rax`, and replacing it without a measured signature would mean
//! inventing the return convention for a call that every game in the process
//! makes — the single most dangerous place to guess.
//!
//! [`decide`] is the portable half and mirrors the Linux precedence exactly, so a
//! body would be a two-line wrapper the day a `GetSteamID` anchor is mined.

use vapor_forge_config::AppId;
use vapor_forge_features::identity;

use crate::capability::Capability;
use crate::features::IdentitySelection;
use crate::hooks::state::HostState;

/// Where an overridden id came from, for the one log line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdSource {
    /// The owner of a delegate ticket whose window is open.
    Delegate,
    /// A validated `[steam_id]` (or script) value for this app.
    Configured,
}

impl IdSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delegate => "delegate",
            Self::Configured => "configured",
        }
    }
}

/// The id the host presents for one call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentedId {
    /// Steam's own account id, untouched.
    Steam { steam_id64: u64 },
    /// An id to present instead of Steam's.
    Override { steam_id64: u64, source: IdSource },
}

impl PresentedId {
    pub fn steam_id64(self) -> u64 {
        match self {
            Self::Steam { steam_id64 } | Self::Override { steam_id64, .. } => steam_id64,
        }
    }
}

/// Decide which id to present for one call made on behalf of `app_id`.
///
/// `real` is Steam's own answer and `delegate` is the owner of the delegate ticket
/// a caller just received, or `0` when no window is open. Precedence is the Linux
/// one: a delegate wins over a configured value, because the id has to match the
/// ticket the app was handed.
pub fn decide(state: &HostState, app_id: AppId, real: u64, delegate: u64) -> PresentedId {
    // Nothing to replace: a zero answer is "no account", not an account to spoof.
    if real == 0 {
        return PresentedId::Steam { steam_id64: 0 };
    }
    if !state.allows(Capability::SteamIdOverride) {
        return PresentedId::Steam { steam_id64: real };
    }
    if !state.config().is_controlled_app(app_id) {
        return PresentedId::Steam { steam_id64: real };
    }
    if delegate != 0 && identity::is_valid_individual_steam_id(delegate) {
        return PresentedId::Override {
            steam_id64: delegate,
            source: IdSource::Delegate,
        };
    }
    match crate::features::select_identity(state.identity_settings(), app_id) {
        IdentitySelection::Override { steam_id64, .. } => PresentedId::Override {
            steam_id64,
            source: IdSource::Configured,
        },
        IdentitySelection::NoOverride { .. } => PresentedId::Steam { steam_id64: real },
        IdentitySelection::Refuse { reason, .. } => {
            // A refusal is the failure path, and it leaves Steam's answer in
            // place: the one thing this must never do is invent an id.
            tracing::debug!(
                app_id = app_id.0,
                reason = reason.as_str(),
                "steam-id: no override applied"
            );
            PresentedId::Steam { steam_id64: real }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::IdentitySettings;
    use crate::hooks::tests_support::{config_with, state_with};
    use std::collections::BTreeMap;

    const REAL: u64 = 76_561_198_000_000_001;
    const DELEGATE: u64 = 76_561_198_000_000_002;
    const CONFIGURED: u64 = 76_561_198_000_000_003;

    fn state_with_overrides(app: u32, overrides: &[(u32, u64)]) -> HostState {
        state_with(config_with(&[app]), Vec::new()).with_identity_settings(IdentitySettings {
            overrides: overrides
                .iter()
                .map(|&(app, id)| (AppId(app), id))
                .collect::<BTreeMap<_, _>>(),
        })
    }

    #[test]
    fn without_an_override_steams_own_account_is_presented() {
        let state = state_with_overrides(440, &[]);
        assert_eq!(
            decide(&state, AppId(440), REAL, 0),
            PresentedId::Steam { steam_id64: REAL }
        );
    }

    #[test]
    fn a_configured_id_replaces_the_real_account() {
        let state = state_with_overrides(440, &[(440, CONFIGURED)]);
        assert_eq!(
            decide(&state, AppId(440), REAL, 0),
            PresentedId::Override {
                steam_id64: CONFIGURED,
                source: IdSource::Configured
            }
        );
    }

    #[test]
    fn a_delegate_window_wins_over_the_configuration() {
        let state = state_with_overrides(440, &[(440, CONFIGURED)]);
        assert_eq!(
            decide(&state, AppId(440), REAL, DELEGATE),
            PresentedId::Override {
                steam_id64: DELEGATE,
                source: IdSource::Delegate
            }
        );
    }

    #[test]
    fn an_app_we_do_not_control_keeps_steams_account_even_when_configured() {
        let state = state_with_overrides(440, &[(440, CONFIGURED)]);
        assert_eq!(
            decide(&state, AppId(441), REAL, DELEGATE),
            PresentedId::Steam { steam_id64: REAL }
        );
    }

    #[test]
    fn a_configured_value_that_is_not_an_individual_id_is_refused() {
        // 12345 is not a SteamID64: no id is invented and Steam's stays.
        let state = state_with_overrides(440, &[(440, 12_345)]);
        assert_eq!(
            decide(&state, AppId(440), REAL, 0),
            PresentedId::Steam { steam_id64: REAL }
        );
    }

    #[test]
    fn a_zero_answer_is_never_replaced() {
        let state = state_with_overrides(440, &[(440, CONFIGURED)]);
        assert_eq!(
            decide(&state, AppId(440), 0, DELEGATE),
            PresentedId::Steam { steam_id64: 0 }
        );
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with_overrides(440, &[(440, CONFIGURED)]);
        let mut gate = state.gate();
        gate.disable(Capability::SteamIdOverride, "no anchor in the corpus");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, AppId(440), REAL, DELEGATE),
            PresentedId::Steam { steam_id64: REAL }
        );
    }
}
