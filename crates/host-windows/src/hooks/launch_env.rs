//! Launch environment — **decision ready, no anchor**.
//!
//! The Linux host hooks `CUser::BuildSpawnEnvBlock` and `SetEnvString`
//! (`crates/hooks/src/client/env.rs`): before Steam forks the child it writes the
//! native libraries the launch needs into the environment map Steam is about to
//! snapshot, and merges them back in when Steam overwrites the same key itself.
//!
//! # Why there is no body here
//!
//! `CUser::BuildSpawnEnvBlock` is the one launch symbol either host would use and
//! the win64 corpus does not resolve it: verified with the miner against both live
//! builds (the class-B note in `docs/vapor-forge-parity.md` says the same). A body
//! also has to know the *shape* of the map it writes into — on Linux that is
//! `SetEnvString`'s ABI with a measured writer — and no Windows build has measured
//! it. Writing an environment map on an unmeasured layout is how a client starts
//! with a corrupted block, so nothing here writes.
//!
//! [`decide`] is the portable half: which variables belong in the child's
//! environment for one app, and why the host leaves the environment alone when it
//! does not. The names are the Steamworks SDK's own (`steam_api.h` documents the
//! variables a game reads), not a corpus measurement, and no value here is a
//! secret.

use vapor_forge_config::AppId;

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// One variable the child should see.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnvEntry {
    pub key: &'static str,
    pub value: String,
}

/// What the host wants the child environment to be.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EnvDecision {
    /// Steam builds the environment exactly as it does today.
    LeaveAlone { reason: String },
    /// These variables belong in the child environment for this app.
    Inject { entries: Vec<EnvEntry> },
}

impl EnvDecision {
    pub fn is_inject(&self) -> bool {
        matches!(self, Self::Inject { .. })
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::LeaveAlone { reason } => Some(reason),
            Self::Inject { .. } => None,
        }
    }
}

/// The variables that make a controlled app present as itself to its game.
///
/// `SteamAppId`, `SteamGameId` and `SteamOverlayGameId` are the SDK's documented
/// hand-off to the child; a title that starts with another app's id in these
/// believes it is that app, which is exactly the failure this decision prevents on
/// a shared machine.
const ID_VARIABLES: [&str; 3] = ["SteamAppId", "SteamGameId", "SteamOverlayGameId"];

/// Decide the environment for `app_id`.
pub fn decide(state: &HostState, app_id: AppId) -> EnvDecision {
    if !state.allows(Capability::LaunchEnvironment) {
        return EnvDecision::LeaveAlone {
            reason: "the launch-environment capability is disabled for this run".to_owned(),
        };
    }
    if !state.config().is_controlled_app(app_id) {
        return EnvDecision::LeaveAlone {
            reason: format!("app {} is not controlled by the host", app_id.0),
        };
    }
    let value = app_id.0.to_string();
    EnvDecision::Inject {
        entries: ID_VARIABLES
            .iter()
            .map(|&key| EnvEntry {
                key,
                value: value.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};

    #[test]
    fn a_controlled_app_carries_its_own_id_into_the_child() {
        let state = state_with(config_with(&[480]), Vec::new());
        let decision = decide(&state, AppId(480));
        let EnvDecision::Inject { entries } = &decision else {
            panic!("a controlled app must be given its environment, got {decision:?}");
        };
        assert_eq!(entries.len(), ID_VARIABLES.len());
        for entry in entries {
            assert_eq!(entry.value, "480");
        }
        assert!(entries.iter().any(|entry| entry.key == "SteamAppId"));
    }

    #[test]
    fn an_uncontrolled_app_keeps_steams_environment() {
        let state = state_with(config_with(&[480]), Vec::new());
        let decision = decide(&state, AppId(481));
        assert!(!decision.is_inject());
        assert!(decision
            .reason()
            .is_some_and(|reason| reason.contains("481")));
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with(&[480]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::LaunchEnvironment, "no anchor in the corpus");
        state.set_gate(gate);
        let decision = decide(&state, AppId(480));
        assert!(!decision.is_inject());
        assert!(decision
            .reason()
            .is_some_and(|reason| reason.contains("launch-environment")));
    }
}
