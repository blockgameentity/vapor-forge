//! Shader cache control — **decision ready, no anchor**.
//!
//! The Linux host hooks `CShaderCacheManager::GetShaderCacheDepot` and answers
//! `0` for a depot it controls but holds no key for
//! (`crates/hooks/src/client/shader.rs`), which stops Steam from downloading a
//! shader cache the host cannot decrypt afterwards. The decision itself belongs
//! to `features::depot_key::should_skip_shader_depot` and reads one thing beyond
//! the config: whether a 32-byte key exists for that depot.
//!
//! # Why there is no body here
//!
//! The win64 corpus resolves no shader symbol in either live build. Verified with
//! the miner against both DLLs and both corpus files: the steamclient sets are
//! `BBuildAndAsyncSendFrame`, `BuildDepotDependency`, `CloseAppCloud`,
//! `CPackageInfo::GetPackageInfo`, `CUser::CheckAppOwnership`,
//! `CUser::GetAppTicket`, `CUtlBufferEnsureCapacity`, `GetAppDataFromAppInfo`,
//! `GetDecryptionKey`, `GetPipeClient`, `IPCProcessMessage`,
//! `KeyValues_FindOrCreateKey`, `KeyValues_ReadAsBinary`, `LoadPackage`,
//! `MarkLicenseAsChanged`, `PchMsgNameFromEMsg` — there is no
//! `CShaderCacheManager::` entry to anchor a replacement at, so a body would
//! never install. Registering one anyway would be dead weight: the install pass
//! would report it as installed while nothing called it.
//!
//! [`decide`] is the portable half, and it is what a body would call the moment a
//! shader anchor is mined: a body would replace the returned depot id with `0`
//! when this says [`ShaderDepotAnswer::Skip`] and forward Steam's answer
//! otherwise.

use vapor_forge_config::DepotId;
use vapor_forge_features::depot_key::should_skip_shader_depot;

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// What the host wants Steam to be told about a shader depot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShaderDepotAnswer {
    /// Steam's own depot id stands, untouched.
    PassThrough,
    /// The depot belongs to an app we control and no key is known: report none.
    Skip,
}

/// Decide the answer for `depot_id`.
///
/// Pure: the config and the Lua-registered keys both come from the published
/// [`HostState`], so this is a function of one snapshot.
pub fn decide(state: &HostState, depot_id: DepotId) -> ShaderDepotAnswer {
    if !state.allows(Capability::ShaderCacheControl) {
        return ShaderDepotAnswer::PassThrough;
    }
    if should_skip_shader_depot(depot_id, state.config(), state.depot_keys()) {
        return ShaderDepotAnswer::Skip;
    }
    ShaderDepotAnswer::PassThrough
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};
    use std::collections::HashMap;
    use vapor_forge_features::depot_key::DEPOT_KEY_SIZE;

    fn keys(depot: u32) -> HashMap<DepotId, Vec<u8>> {
        HashMap::from([(DepotId(depot), vec![0u8; DEPOT_KEY_SIZE])])
    }

    #[test]
    fn a_keyless_controlled_depot_is_skipped() {
        let state = state_with(config_with(&[480]), Vec::new());
        assert_eq!(decide(&state, DepotId(480)), ShaderDepotAnswer::Skip);
    }

    #[test]
    fn a_depot_whose_key_is_known_is_left_to_steam() {
        let state = state_with(config_with(&[480]), Vec::new()).with_depot_keys(keys(480));
        assert_eq!(decide(&state, DepotId(480)), ShaderDepotAnswer::PassThrough);
    }

    #[test]
    fn a_short_key_is_not_a_key() {
        let state = state_with(config_with(&[480]), Vec::new())
            .with_depot_keys(HashMap::from([(DepotId(480), vec![0u8; 31])]));
        assert_eq!(decide(&state, DepotId(480)), ShaderDepotAnswer::Skip);
    }

    #[test]
    fn a_depot_we_do_not_control_is_left_to_steam() {
        let state = state_with(config_with(&[480]), Vec::new());
        assert_eq!(decide(&state, DepotId(999)), ShaderDepotAnswer::PassThrough);
        assert_eq!(decide(&state, DepotId(0)), ShaderDepotAnswer::PassThrough);
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with(&[480]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::ShaderCacheControl, "no anchor in the corpus");
        state.set_gate(gate);
        assert_eq!(decide(&state, DepotId(480)), ShaderDepotAnswer::PassThrough);
    }
}
