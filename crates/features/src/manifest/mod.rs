//! Manifest handling.
//!
//! Two independent concerns live here and in [`deps`]:
//!
//! * Lua gid/size overrides for a depot that the script layer already knows
//!   about ([`find_patches`]).
//! * Dependency-aware depot resolution ([`resolve_depot_closure`]), so a depot
//!   set is complete before anything is installed.
//!
//! The environment's remaining metadata (appinfo, DLC lists) is not available
//! in this module: the host wires it through the
//! [`DepotDependencySource`] trait, which keeps the resolution logic pure.

use vapor_forge_config::{DepotId, ManifestId};
use vapor_forge_core::ScriptState;

mod deps;

pub use deps::{
    resolve_depot_closure, DependencyError, DependencyKind, DepotDependency, DepotDependencySource,
    DepotReason, ResolvedDepot, DEFAULT_MAX_DEPENDENCY_DEPTH,
};

#[cfg(test)]
mod tests;

pub struct ManifestPatch {
    pub depot_id: DepotId,
    pub new_gid: ManifestId,
    pub new_size: Option<u64>,
}

/// For each depot_id, check if there's a Lua manifest override.
pub fn find_patches(depot_ids: &[DepotId], script_state: &ScriptState) -> Vec<ManifestPatch> {
    depot_ids
        .iter()
        .filter_map(|&depot_id| {
            script_state
                .manifests
                .get(&depot_id)
                .map(|ov| ManifestPatch {
                    depot_id,
                    new_gid: ov.gid,
                    new_size: ov.size,
                })
        })
        .collect()
}
