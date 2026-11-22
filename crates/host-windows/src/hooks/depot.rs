//! `GetDecryptionKey` — the depot hook body.
//!
//! What the body does, in one sentence: when Steam asks for a depot's
//! decryption key, ask `features::depotcache` whether that depot's manifest is
//! already in `<steam>/depotcache` and say so, then return the original answer
//! unchanged.
//!
//! What it deliberately does not do:
//!
//! * it never rewrites the key buffer — no field of that buffer is measured, so
//!   there is nothing safe to write;
//! * it never fetches a manifest — no archive client is wired and the egress
//!   policy has no exception for this crate;
//! * it never claims a request-code round trip was skipped. Steam reaches its
//!   request-code path elsewhere and that site (`CUser::BuildSpawnEnvBlock`) has
//!   no corpus anchor, so this phase reports the precondition only.
//!
//! Every failure mode is a fall-through: a missing trampoline, unpublished host
//! state, a disabled `depot-injection` capability, a depot the plan does not
//! want, or a store that is not there all return the original result.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::DepotId;
use vapor_forge_features::depotcache::PreseedOutcome;

use crate::abi::depot::GetDecryptionKeyFn;
use crate::capability::Capability;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "GetDecryptionKey";

/// Address of [`get_decryption_key`], for the installer's replacement table.
pub fn address() -> usize {
    get_decryption_key as GetDecryptionKeyFn as usize
}

/// Replacement for `GetDecryptionKey`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this` and the
/// register slots must be whatever Steam passed, `depot_id` is read as the
/// 32-bit argument the corpus pattern measures.
pub unsafe extern "system" fn get_decryption_key(
    this: *mut c_void,
    depot_id: u32,
    r8: usize,
    r9: usize,
) -> u8 {
    // SAFETY: the type passed here is the one documented in `crate::abi::depot`;
    // the trampoline was published for this symbol before its entry was patched.
    let Some(original) = (unsafe { original::original_of::<GetDecryptionKeyFn>(SYMBOL) }) else {
        debug!("depot hook reached without a published original; falling through to Steam");
        return 0;
    };
    // SAFETY: forwarding every slot verbatim, exactly as documented.
    let result = unsafe { original(this, depot_id, r8, r9) };
    report_preseed_state(depot_id);
    result
}

fn report_preseed_state(depot_id: u32) {
    let Some(state) = state::get() else {
        return;
    };
    if !state.allows(Capability::DepotInjection) {
        return;
    }
    let Some(store) = state.depot().store() else {
        debug!(
            depot = depot_id,
            "depotcache: no <steam>/depotcache directory resolved; nothing to check"
        );
        return;
    };
    let depot = DepotId(depot_id);
    for candidate in state.depot().wanted_for(depot) {
        if store.is_cached(candidate) {
            debug!(
                depot = depot_id,
                manifest = ?candidate.manifest_id,
                satisfied = PreseedOutcome::AlreadyPresent.satisfies_install(),
                "depotcache: manifest already on disk; Steam's request-code path has nothing to fetch for this depot"
            );
        } else {
            debug!(
                depot = depot_id,
                manifest = ?candidate.manifest_id,
                "depotcache: wanted manifest is not on disk; the install path still needs its request code"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::depot_store::{DepotPlan, DirStore};
    use crate::hooks::state::HostState;
    use std::path::PathBuf;
    use vapor_forge_config::{DepotId, ManifestId, RuntimeConfig};
    use vapor_forge_features::depotcache::{Candidate, DepotCacheStore};
    use vapor_forge_features::family::FamilySettings;

    fn cache_dir() -> PathBuf {
        std::env::temp_dir().join("vf-host-windows-depot-hook")
    }

    fn wanted() -> Candidate {
        Candidate {
            depot_id: DepotId(731),
            manifest_id: ManifestId(9),
        }
    }

    #[test]
    fn an_unpublished_host_state_leaves_the_original_answer_alone() {
        // The trampoline is absent too, so the body fails closed before it reads
        // any state: that is the "called before initialisation" contract.
        // SAFETY: the body is called with a null receiver and no live Steam
        // frame; every path it can take here returns before touching memory.
        let result = unsafe { get_decryption_key(std::ptr::null_mut(), 731, 0, 0) };
        assert_eq!(result, 0);
    }

    #[test]
    fn a_manifest_on_disk_satisfies_the_preseed_decision() {
        let root = cache_dir();
        let _ = std::fs::remove_dir_all(&root);
        let mut store = DirStore::new(root.clone());
        store.write(wanted(), b"manifest-bytes").expect("write");
        assert!(store.exists(wanted()));

        // The decision the hook reports is the one `features::depotcache`
        // documents, over the same store the hook would use.
        let plan = DepotPlan::new(Some(root.clone()), vec![wanted()]);
        let reported = plan.store().expect("store").is_cached(plan.wanted()[0]);
        assert!(reported);
        assert!(PreseedOutcome::AlreadyPresent.satisfies_install());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_hook_body_only_needs_a_state_with_a_plan() {
        // A state with no depot plan is what a build without manifest planning
        // publishes; constructing it here keeps the fields the hook reads honest.
        let state = HostState::new(
            RuntimeConfig::default(),
            FamilySettings::default(),
            Vec::new(),
            None,
            DepotPlan::none(),
        );
        assert!(state.depot().store().is_none());
        assert!(state.allows(Capability::DepotInjection));
    }
}
