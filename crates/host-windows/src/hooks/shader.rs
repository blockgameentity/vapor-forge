//! Shader cache control — the hook body for `GetShaderCacheDepot`.
//!
//! What the body does, in one sentence: when Steam asks its shader-cache manager
//! which depot an app's shader cache lives in, a depot the host controls but
//! holds no key for is answered `0`, so Steam never downloads a cache this host
//! could not decrypt afterwards. Every other case returns Steam's own answer.
//!
//! The decision is [`decide`], the portable half owned by
//! `features::depot_key::should_skip_shader_depot`: it reads the config and the
//! Lua-registered keys, so it is unit-tested without a live client and the FFI
//! body only forwards the answer.
//!
//! Every failure mode is a fall-through: a missing trampoline, unpublished host
//! state, a disabled `shader-cache-control` capability, or a depot the host does
//! not control all return Steam's answer unchanged. The body logs only on the
//! non-pass path, at debug.
//!
//! The call shape, and which parts of it are measured, is documented in
//! [`crate::abi::shader`].

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::DepotId;
use vapor_forge_features::depot_key::should_skip_shader_depot;

use crate::abi::shader::GetShaderCacheDepotFn;
use crate::capability::Capability;
use crate::hooks::state::HostState;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CShaderCacheManager::GetShaderCacheDepot";

/// Address of [`get_shader_cache_depot`], for the installer's replacement table.
pub fn address() -> usize {
    get_shader_cache_depot as GetShaderCacheDepotFn as usize
}

/// Replacement for `GetShaderCacheDepot`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this` and the
/// three opaque slots must be whatever Steam passed. `this` is never
/// dereferenced here — the original reads it.
pub unsafe extern "system" fn get_shader_cache_depot(
    this: *mut c_void,
    rdx: usize,
    r8: usize,
    r9: usize,
) -> u32 {
    // SAFETY: the signature is the one documented in `crate::abi::shader`, and
    // the trampoline was published for this symbol before the entry was patched.
    let Some(original) = (unsafe { original::original_of::<GetShaderCacheDepotFn>(SYMBOL) }) else {
        debug!("shader hook reached without a published original; reporting no depot");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim, exactly as documented.
    let depot_id = unsafe { original(this, rdx, r8, r9) };
    let Some(state) = state::get() else {
        return depot_id;
    };
    match decide(state, DepotId(depot_id)) {
        ShaderDepotAnswer::Skip => {
            debug!(
                depot_id,
                "shader-cache: a controlled depot has no key here; reporting no shader cache"
            );
            0
        }
        ShaderDepotAnswer::PassThrough => depot_id,
    }
}

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
        gate.disable(Capability::ShaderCacheControl, "the symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(decide(&state, DepotId(480)), ShaderDepotAnswer::PassThrough);
    }

    #[test]
    fn an_unpublished_original_reports_no_depot_instead_of_calling_out() {
        // The trampoline is absent here, so the body fails closed before it reads
        // any state: that is the "called before initialisation" contract.
        // SAFETY: called with a null receiver and no live Steam frame; every path
        // this test can take returns before touching memory.
        let result = unsafe { get_shader_cache_depot(std::ptr::null_mut(), 0, 0, 0) };
        assert_eq!(result, 0, "no original means no answer to forward");
    }
}
