//! `features::depotcache` consulted at the Linux depot site.
//!
//! Steam installs a depot without its request-code round trip when
//! `<steam>/depotcache/<depot>_<gid>.manifest` is already on disk. The decision
//! logic lives in `features::depotcache`; this module is the host half of it:
//!
//! * [`SteamDepotCache`] — the real `<steam>/depotcache` directory, seen through
//!   the feature's [`DepotCacheStore`] trait. Every path comes from
//!   [`Candidate::file_name`], which is built from two integers, so a candidate
//!   can never escape that directory.
//! * [`NoArchiveClient`] — the archive side. This host wires no archive client:
//!   fetching a manifest would mean network egress the runtime does not have, so
//!   a candidate the cache does not hold is reported as "not archived" and the
//!   depot path falls through exactly as it did before this module existed.
//! * [`consult`] — the one entry point. `None` means "do nothing at all", which
//!   is what an unconfigured host gets.
//!
//! The wanted manifest comes from the same place the depot-injection path pins
//! one: a Lua `setManifestid` override names a `ManifestId` for a `DepotId`. No
//! override means no candidate, and no candidate means nothing to consult — the
//! host never guesses a gid.
//!
//! Flag: the preseed path is only reached when `[manifest] providers` is
//! configured. That list is empty by default, and an empty list means this host
//! fetches no manifests at all, so nothing is consulted and nothing changes for
//! an existing config.
use std::path::PathBuf;
use tracing::{debug, info, warn};
#[cfg(test)]
use vapor_forge_config::ManifestId;
use vapor_forge_config::{DepotId, RuntimeConfig};
use vapor_forge_features::depotcache::{
    self, ArchiveFetcher, Candidate, DepotCacheStore, PreseedOutcome,
};

/// `<steam>/depotcache`, viewed through the feature's trait.
pub(crate) struct SteamDepotCache {
    root: PathBuf,
}

impl SteamDepotCache {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    fn path_of(&self, candidate: Candidate) -> PathBuf {
        self.root.join(candidate.file_name())
    }

    /// Whether `candidate`'s manifest is already on disk.
    pub(crate) fn is_cached(&self, candidate: Candidate) -> bool {
        self.path_of(candidate).is_file()
    }
}

impl DepotCacheStore for SteamDepotCache {
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

/// The manifest archive this host does not have.
///
/// `Ok(None)` is the feature's "definitively not archived" answer, which the
/// caller turns into a fall-through: no error line, no write, no retry.
pub(crate) struct NoArchiveClient;

impl ArchiveFetcher for NoArchiveClient {
    fn fetch_manifest(&mut self, candidate: Candidate) -> Result<Option<Vec<u8>>, String> {
        debug!(
            depot = ?candidate.depot_id,
            manifest = ?candidate.manifest_id,
            "depotcache: no manifest archive client is wired on this host"
        );
        Ok(None)
    }
}

/// Whether this host is configured to deal in manifests at all.
///
/// `[manifest] providers` is written by the host tool and empty by default; an
/// empty chain means no manifest is ever fetched, so the preseed question is not
/// asked either.
pub(crate) fn configured(config: &RuntimeConfig) -> bool {
    !config.manifest.providers.is_empty()
}

/// The depot cache directory, when the Steam install root resolves.
pub(crate) fn cache_dir() -> Option<PathBuf> {
    super::install::steam_install_root().map(|root| root.join("depotcache"))
}

/// Consult the cache for one depot and report what happened.
///
/// `None` covers every "nothing to say" case: the path is not configured, no
/// manifest is wanted for this depot, or the cache directory did not resolve. A
/// miss is [`PreseedOutcome::NotArchived`], whose `satisfies_install` is false,
/// so the caller keeps its own behaviour.
pub(crate) fn consult(
    config: &RuntimeConfig,
    depot_id: DepotId,
    wanted: Option<Candidate>,
) -> Option<PreseedOutcome> {
    if !configured(config) {
        debug!(
            depot = depot_id.0,
            "depotcache: no manifest provider configured; nothing to consult"
        );
        return None;
    }
    let candidate = wanted?;
    let root = cache_dir()?;
    let mut store = SteamDepotCache::new(root);
    let outcome = depotcache::preseed(
        &mut store,
        &mut NoArchiveClient,
        candidate,
        depotcache::plausible_manifest,
    );
    report(depot_id, candidate, &outcome);
    Some(outcome)
}

fn report(depot_id: DepotId, candidate: Candidate, outcome: &PreseedOutcome) {
    match outcome {
        PreseedOutcome::AlreadyPresent => info!(
            depot = depot_id.0,
            manifest = ?candidate.manifest_id,
            "depotcache: manifest already on disk; the install path needs no request code"
        ),
        PreseedOutcome::Written { bytes } => info!(
            depot = depot_id.0,
            manifest = ?candidate.manifest_id,
            bytes,
            "depotcache: pre-seeded"
        ),
        PreseedOutcome::NotArchived => debug!(
            depot = depot_id.0,
            manifest = ?candidate.manifest_id,
            "depotcache: wanted manifest is not on disk; the install path keeps its request code"
        ),
        PreseedOutcome::Failed { reason } => warn!(
            depot = depot_id.0,
            reason = reason.as_str(),
            "depotcache: pre-seed failed; falling through"
        ),
    }
}

#[cfg(test)]
mod tests;
