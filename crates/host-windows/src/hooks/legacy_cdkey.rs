//! Legacy CD key control — the hook body for `RequiresLegacyCDKey`.
//!
//! What the body does, in one sentence: for any app the host controls, Steam is
//! told the app requires no legacy CD key and the out flag Steam passes is
//! cleared, so its launch flow skips the `GettingLegacyKey` task and sends no
//! `ClientGetLegacyGameKey` request for a title that has no key to send.
//!
//! The decision is [`decide`], the portable half: an app the host controls never
//! requires a legacy CD key, and an app it does not control keeps Steam's answer.
//! It is unit-tested without a live client.
//!
//! Writing the out flag is safe *because the anchor measures it*: the original
//! clears that byte (`41 C6 00 00`, `mov byte [r8], 0`) before any other work, so
//! the pointer, the width and the value are Steam's own (see
//! [`crate::abi::legacy_cdkey`]). The body writes nothing else and never touches
//! `this`.
//!
//! Every failure mode is a fall-through: a missing trampoline, unpublished host
//! state, or a disabled `legacy-cd-key-control` capability returns Steam's answer
//! and leaves the flag as Steam set it.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_features::apps;

use crate::abi::legacy_cdkey::RequiresLegacyCdKeyFn;
use crate::capability::Capability;
use crate::hooks::state::HostState;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "IClientUser::RequiresLegacyCDKey";

/// Address of [`requires_legacy_cd_key`], for the installer's replacement table.
pub fn address() -> usize {
    requires_legacy_cd_key as RequiresLegacyCdKeyFn as usize
}

/// Replacement for `RequiresLegacyCDKey`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`
/// and the `r9` slot must be whatever Steam passed, and `out_flag` must be the
/// pointer the original writes its own `0` through (or null, which this body
/// tolerates).
pub unsafe extern "system" fn requires_legacy_cd_key(
    this: *mut c_void,
    app_id: u32,
    out_flag: *mut u8,
    r9: usize,
) -> u8 {
    // SAFETY: the signature is the one documented in `crate::abi::legacy_cdkey`,
    // and the trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<RequiresLegacyCdKeyFn>(SYMBOL) }) else {
        debug!("legacy-cdkey hook reached without a published original; forwarding nothing");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim.
    let steam = unsafe { original(this, app_id, out_flag, r9) };
    let Some(state) = state::get() else {
        return steam;
    };
    match decide(state, AppId(app_id)) {
        CdKeyAnswer::Suppress => {
            if !out_flag.is_null() {
                // SAFETY: the original writes this same byte through this same
                // pointer before doing any other work, so it is a writable byte
                // whenever the body runs at all.
                unsafe { out_flag.write(0) };
            }
            debug!(
                app_id,
                "legacy-cdkey: a controlled app is reported as needing no key"
            );
            0
        }
        CdKeyAnswer::PassThrough => steam,
    }
}

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
        gate.disable(Capability::LegacyCdKeyControl, "the symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(decide(&state, AppId(220)), CdKeyAnswer::PassThrough);
    }
}
