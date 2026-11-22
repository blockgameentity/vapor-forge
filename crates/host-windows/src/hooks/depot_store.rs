//! Depotcache pre-seed decisions at the one anchored depot site.
//!
//! `features::depotcache` answers one question the install path cares about: is
//! `<steam>/depotcache/<depot>_<manifest>.manifest` already on disk, in which
//! case Steam installs the depot without a request-code round trip
//! (`PreseedOutcome::satisfies_install`). The corpus anchors no manifest
//! request-code site and no `CUser::BuildSpawnEnvBlock`, so nothing in this
//! phase can *skip* that round trip; what it can do is answer the question where
//! Steam asks for a depot's decryption key, which is the moment the depot's
//! manifest is being used. The answer is recorded and logged.
//!
//! Two things are deliberately absent: nothing is fetched (this host wires no
//! archive client, and the egress policy forbids inventing one), and nothing is
//! written — [`DirStore::write`] exists because the trait requires it and is
//! used by tests, not by the hook.

use std::path::{Path, PathBuf};

use vapor_forge_config::{DepotId, ManifestId};
use vapor_forge_features::depotcache::{Candidate, DepotCacheStore};

/// The depot cache the host may consult, and the manifest it wants for each
/// depot.
///
/// `wanted` comes from manifest request planning, which this host does not run
/// yet (`features::depotcache::DepotCacheSettings::wanted` carries the same
/// list); the empty list is the honest default and makes the hook fall through.
#[derive(Clone, Debug, Default)]
pub struct DepotPlan {
    cache_dir: Option<PathBuf>,
    wanted: Vec<Candidate>,
}

impl DepotPlan {
    pub fn new(cache_dir: Option<PathBuf>, wanted: Vec<Candidate>) -> Self {
        Self { cache_dir, wanted }
    }

    /// A plan with nowhere to look: the hook then answers nothing.
    pub fn none() -> Self {
        Self::default()
    }

    /// `<steam>/depotcache`, when the host could resolve it.
    pub fn cache_dir(&self) -> Option<&Path> {
        self.cache_dir.as_deref()
    }

    pub fn wanted(&self) -> &[Candidate] {
        &self.wanted
    }

    /// The manifest the host wants for `depot`, if it wants one at all.
    pub fn wanted_for(&self, depot: DepotId) -> impl Iterator<Item = Candidate> + '_ {
        self.wanted
            .iter()
            .copied()
            .filter(move |candidate| candidate.depot_id == depot)
    }

    /// A store over [`Self::cache_dir`], or `None` when the directory is unknown.
    pub fn store(&self) -> Option<DirStore> {
        self.cache_dir.clone().map(|root| DirStore { root })
    }
}

/// `<steam>/depotcache`, viewed through `features::depotcache`'s trait.
///
/// Every path it builds comes from [`Candidate::file_name`], which is formed from
/// two integers, so a candidate can never escape the directory it was given.
pub struct DirStore {
    root: PathBuf,
}

impl DirStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path_of(&self, candidate: Candidate) -> PathBuf {
        self.root.join(candidate.file_name())
    }

    /// Whether `candidate`'s manifest is already on disk.
    ///
    /// Inherent so a caller does not have to import the trait just to ask the
    /// one question the depot hook asks.
    pub fn is_cached(&self, candidate: Candidate) -> bool {
        self.path_of(candidate).is_file()
    }
}

impl DepotCacheStore for DirStore {
    fn exists(&self, candidate: Candidate) -> bool {
        self.is_cached(candidate)
    }

    fn write(&mut self, candidate: Candidate, bytes: &[u8]) -> Result<(), String> {
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("{}: {error}", self.root.display()))?;
        let path = self.path_of(candidate);
        std::fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))
    }
}

/// Manifest the host wants for a depot, if the plan names one.
pub fn wanted_manifest(plan: &DepotPlan, depot: DepotId) -> Option<ManifestId> {
    plan.wanted_for(depot)
        .next()
        .map(|candidate| candidate.manifest_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(depot: u32, manifest: u64) -> Candidate {
        Candidate {
            depot_id: DepotId(depot),
            manifest_id: ManifestId(manifest),
        }
    }

    #[test]
    fn a_directory_store_sees_a_written_manifest() {
        let root = std::env::temp_dir().join("vf-host-windows-depotcache-store");
        let _ = std::fs::remove_dir_all(&root);
        let mut store = DirStore::new(root.clone());
        let wanted = candidate(731, 42);
        assert!(
            !store.is_cached(wanted),
            "nothing is cached before the write"
        );
        store.write(wanted, b"manifest").expect("write");
        assert!(store.is_cached(wanted));
        assert!(!store.is_cached(candidate(731, 43)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_cache_directory_answers_nothing_instead_of_failing() {
        let plan = DepotPlan::new(Some(PathBuf::from("/definitely/not/here")), Vec::new());
        let store = plan
            .store()
            .expect("a resolved directory still yields a store");
        assert!(!store.is_cached(candidate(1, 2)));
        assert!(DepotPlan::none().store().is_none());
    }

    #[test]
    fn wanted_lookups_are_per_depot() {
        let plan = DepotPlan::new(None, vec![candidate(731, 42), candidate(732, 7)]);
        assert_eq!(wanted_manifest(&plan, DepotId(731)), Some(ManifestId(42)));
        assert_eq!(wanted_manifest(&plan, DepotId(999)), None);
        assert_eq!(plan.wanted_for(DepotId(731)).count(), 1);
    }
}
