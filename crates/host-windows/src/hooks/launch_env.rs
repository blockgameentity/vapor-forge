//! Launch environment — the hook body for `BuildSpawnEnvBlock`.
//!
//! What the body does, in one sentence: before Steam forks the child it reads the
//! app the launch is for, asks [`decide`] which variables that app's game must
//! see, and reports the answer.
//!
//! # What the body deliberately does not do
//!
//! It does not write the environment map. Two things are missing, and both are
//! measured absences rather than omissions:
//!
//! 1. **The map's shape.** No Windows build has measured where
//!    `BuildSpawnEnvBlock` receives its map or how its elements are laid out.
//!    Writing a guess there is how a client starts a game with a corrupted block;
//!    [`crate::abi::launch_env`] records exactly which structure fields the
//!    anchor *does* measure and that the map is not among them.
//! 2. **The writer.** The Linux host merges its variables through
//!    `SetEnvString`, a separate entry the win64 corpus does not anchor. There is
//!    no measured way to add one variable to a map on this host.
//!
//! So the body installs, gates, reads and decides, and Steam's own block stands.
//! What it adds today is the decision *point*: the moment a map layout and a
//! writer are anchored, the write belongs here, and nothing else has to move.
//!
//! Every failure mode is a fall-through: a missing trampoline, unpublished host
//! state, or a disabled `launch-environment` capability forwards Steam's answer
//! and reads nothing at all.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;

use crate::abi::launch_env::{game_id_app_id, BuildSpawnEnvBlockFn};
use crate::capability::Capability;
use crate::hooks::state::HostState;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CUser::BuildSpawnEnvBlock";

/// Address of [`build_spawn_env_block`], for the installer's replacement table.
pub fn address() -> usize {
    build_spawn_env_block as BuildSpawnEnvBlockFn as usize
}

/// Replacement for `CUser::BuildSpawnEnvBlock`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `rcx`, `launch`,
/// `r8` and `r9` must be whatever Steam passed. `launch` is dereferenced, exactly
/// as the original dereferences it before any other work.
pub unsafe extern "system" fn build_spawn_env_block(
    rcx: *mut c_void,
    launch: *const u8,
    r8: usize,
    r9: usize,
) -> usize {
    // SAFETY: the signature is the one documented in `crate::abi::launch_env`, and
    // the trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<BuildSpawnEnvBlockFn>(SYMBOL) }) else {
        debug!("launch-env hook reached without a published original; building nothing");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim; the block Steam builds is unchanged.
    let result = unsafe { original(rcx, launch, r8, r9) };
    report_decision(launch);
    result
}

/// Decide for the app this launch is for, and log it.
///
/// Split from the FFI body so the decision stays unit-tested: this half reads one
/// masked dword and logs.
fn report_decision(launch: *const u8) {
    let Some(state) = state::get() else {
        return;
    };
    if !state.allows(Capability::LaunchEnvironment) {
        return;
    }
    // SAFETY: the original just ran on this same pointer, so it is the structure
    // Steam builds the block from and its first qword is readable.
    let Some(app_id) = (unsafe { game_id_app_id(launch) }) else {
        debug!("launch-env: no app id in the launch record; environment left to Steam");
        return;
    };
    match decide(state, AppId(app_id)) {
        EnvDecision::Inject { entries } => debug!(
            app_id,
            variables = entries.len(),
            "launch-env: this app's id belongs in the child environment, but no env-map writer is anchored on this host; Steam's block stands"
        ),
        EnvDecision::LeaveAlone { reason } => {
            debug!(app_id, reason, "launch-env: environment left to Steam")
        }
    }
}

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
        gate.disable(Capability::LaunchEnvironment, "the symbol did not resolve");
        state.set_gate(gate);
        let decision = decide(&state, AppId(480));
        assert!(!decision.is_inject());
        assert!(decision
            .reason()
            .is_some_and(|reason| reason.contains("launch-environment")));
    }
}
