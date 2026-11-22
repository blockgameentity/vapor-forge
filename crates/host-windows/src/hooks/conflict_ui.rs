//! Cloud-conflict UI bridge — **decision ready, no anchor**.
//!
//! The Linux host reverses the direction of the cloud-conflict dialog
//! (`crates/hooks/src/ui/reverse_bridge.rs`): the UI's "keep local / keep remote /
//! retry" choice arrives as a registered JS method
//! (`Apps.VaporForgeResolveCloudConflict` and friends), is matched against the
//! *window generation* it was registered for, and only then is turned into a
//! callback for the client. Three of its rules are portable and are the decision
//! this module owns:
//!
//! 1. a choice that arrives after its window was retired is stale — the UI keeps
//!    old windows alive across a reload, and acting on their tokens would apply a
//!    decision to the wrong session;
//! 2. nothing is applied before the bridge says it is ready, because the confirm
//!    signal is what tells the client the dialog exists at all;
//! 3. with the capability off, every arrival is refused and the dialog is left to
//!    Steam.
//!
//! # Why there is no body here
//!
//! The Linux bridge anchors `RegisterJsMethod` and the string-handler destructor.
//! Neither is in the win64 corpus: verified with the miner against both live
//! builds, whose SteamUI set is `AddProtobufAsBinary`,
//! `BuildCompleteAppOverviewChange`, `GetAppByID`, `RunFrame`,
//! `LoadModuleWithPath`, `ShouldShowAppInLibrary` and `FillInAppOverview`. Of those
//! only `RunFrame` is a tick, and a tick with no registration behind it has
//! nothing to drain: wiring it would install a body that can never see a conflict.
//!
//! [`decide`] is the portable half: the generation and readiness rules, with a
//! refusal that always says which rule fired.

pub mod registration;

use vapor_forge_config::AppId;

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// Why a conflict choice was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// The `conflict-ui-bridge` capability did not survive the install pass.
    CapabilityDisabled,
    /// The window that registered the choice is no longer live.
    StaleWindow { reported: u64, live: u64 },
    /// The bridge has not confirmed itself to the UI yet.
    BridgeNotReady,
}

impl Refusal {
    pub fn label(self) -> &'static str {
        match self {
            Self::CapabilityDisabled => "capability-disabled",
            Self::StaleWindow { .. } => "stale-window",
            Self::BridgeNotReady => "bridge-not-ready",
        }
    }
}

/// What to do with one arrival from the UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resolution {
    /// The choice belongs to the live window: it may be applied.
    Accept,
    /// Do nothing; Steam's own dialog state stands.
    Refuse(Refusal),
}

/// Decide whether a conflict choice that names `reported_generation` may be
/// applied.
///
/// `live` is the generation the host published for that window, and `bridge_ready`
/// is whether the UI has confirmed the bridge. Order matters: capability first
/// (nothing of ours is running), then staleness, then readiness — a stale
/// generation is refused even when the bridge is ready, because "ready" says
/// nothing about *which* window.
pub fn decide(
    state: &HostState,
    _app_id: AppId,
    reported_generation: u64,
    live: u64,
    bridge_ready: bool,
) -> Resolution {
    if !state.allows(Capability::ConflictUiBridge) {
        return Resolution::Refuse(Refusal::CapabilityDisabled);
    }
    if reported_generation != live {
        return Resolution::Refuse(Refusal::StaleWindow {
            reported: reported_generation,
            live,
        });
    }
    if !bridge_ready {
        return Resolution::Refuse(Refusal::BridgeNotReady);
    }
    Resolution::Accept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};

    fn state() -> HostState {
        state_with(config_with(&[730]), Vec::new())
    }

    #[test]
    fn a_choice_for_the_live_window_is_accepted_once_the_bridge_is_ready() {
        let state = state();
        assert_eq!(decide(&state, AppId(730), 7, 7, true), Resolution::Accept);
    }

    #[test]
    fn a_choice_from_a_retired_window_is_stale() {
        let state = state();
        assert_eq!(
            decide(&state, AppId(730), 6, 7, true),
            Resolution::Refuse(Refusal::StaleWindow {
                reported: 6,
                live: 7
            })
        );
    }

    #[test]
    fn nothing_is_applied_before_the_bridge_confirms_itself() {
        let state = state();
        assert_eq!(
            decide(&state, AppId(730), 7, 7, false),
            Resolution::Refuse(Refusal::BridgeNotReady)
        );
    }

    #[test]
    fn a_disabled_capability_refuses_every_arrival() {
        let state = state();
        let mut gate = state.gate();
        gate.disable(Capability::ConflictUiBridge, "no anchor in the corpus");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, AppId(730), 7, 7, true),
            Resolution::Refuse(Refusal::CapabilityDisabled)
        );
    }

    #[test]
    fn a_refusal_always_names_its_rule() {
        assert_eq!(Refusal::BridgeNotReady.label(), "bridge-not-ready");
        assert_eq!(
            Refusal::StaleWindow {
                reported: 1,
                live: 2
            }
            .label(),
            "stale-window"
        );
    }
}
