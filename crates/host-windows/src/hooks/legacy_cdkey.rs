//! Legacy CD key control — **decision ready, no anchor**.
//!
//! The Linux host hooks `IClientUser::RequiresLegacyCDKey`
//! (`crates/hooks/src/client/legacy_cdkey.rs`) and answers `false`, clearing the
//! out flag, for any app the host controls. Steam's launch flow then skips its
//! `GettingLegacyKey` task and no `ClientGetLegacyGameKey` request is sent for a
//! title that has no key to send.
//!
//! # Why there is no body here
//!
//! `IClientUser::RequiresLegacyCDKey` is not in the win64 corpus: verified with
//! the miner against both live builds. What the corpus resolves under `CUser::`
//! is `CheckAppOwnership` and `GetAppTicket`, which already carry their own
//! bodies. The Linux host also needs an adapter-thunk resolver
//! (`patterns::cuser_adapter::resolve_requires_legacy_cdkey_implementation`) to
//! reach the implementation behind the vtable entry; the Windows corpus has no
//! equivalent entry to resolve.
//!
//! A body here would do two things a decision cannot: call the original, and
//! write `false` through the out flag Steam passed. Neither is safe to write
//! blind — [`crate::abi`]'s rule is that an argument is only touched when its
//! width is measured — so the portable half is the answer, not the write.

use vapor_forge_config::AppId;
use vapor_forge_features::apps;

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// What the host wants Steam told about a legacy CD key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CdKeyAnswer {
    /// Steam's own answer stands: the app needs whatever key it needs.
    PassThrough,
    /// A controlled app never requires a legacy CD key.
    Suppress,
}

/// Decide the answer for `app_id`.
///
/// The rule is the Linux one and nothing else: an app the host controls does not
/// require a legacy CD key. An uncontrolled app keeps Steam's answer, which is
/// what "we did not touch it" has to mean.
pub fn decide(state: &HostState, app_id: AppId) -> CdKeyAnswer {
    if !state.allows(Capability::LegacyCdKeyControl) {
        return CdKeyAnswer::PassThrough;
    }
    if apps::classify_app(state.config(), app_id).is_controlled() {
        return CdKeyAnswer::Suppress;
    }
    CdKeyAnswer::PassThrough
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};

    #[test]
    fn a_controlled_app_never_requires_a_cd_key() {
        let state = state_with(config_with(&[220]), Vec::new());
        assert_eq!(decide(&state, AppId(220)), CdKeyAnswer::Suppress);
    }

    #[test]
    fn an_uncontrolled_app_keeps_steams_answer() {
        let state = state_with(config_with(&[220]), Vec::new());
        assert_eq!(decide(&state, AppId(221)), CdKeyAnswer::PassThrough);
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with(&[220]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::LegacyCdKeyControl, "no anchor in the corpus");
        state.set_gate(gate);
        assert_eq!(decide(&state, AppId(220)), CdKeyAnswer::PassThrough);
    }
}
