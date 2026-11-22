//! Depotcache pre-seed: the existing decision logic over host-provided ports.
//!
//! `vapor_forge_features::depotcache` owns the invariants (a 404 is not an error
//! and writes nothing, a bad body is never written, a transient failure leaves
//! the cache untouched) and the traits (`DepotCacheStore`, `ArchiveFetcher`).
//! This module adds only what the Windows host needs: a list of wanted
//! depot/manifest pairs, one report entry per candidate, and the guarantee that
//! one candidate failing never aborts the rest of the batch - the batch is the
//! install path's list, and dropping the remainder would silently cost the
//! request-code savings for every depot after the first failure.

use vapor_forge_features::depotcache::{
    plausible_manifest, preseed, ArchiveFetcher, Candidate, DepotCacheStore, PreseedOutcome,
};

/// The pairs the install path wants on disk (manifest request planning).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DepotCacheSettings {
    pub wanted: Vec<Candidate>,
}

/// The fetch/store pair the host provides.
///
/// Both are trait objects so the report and the run loop stay non-generic, and so
/// the real archive client and the real `<steam>/depotcache` directory can be
/// swapped for fakes in tests without a live client.
pub struct DepotCachePorts<'a> {
    pub store: &'a mut dyn DepotCacheStore,
    pub fetcher: &'a mut dyn ArchiveFetcher,
}

/// One candidate and what happened to it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DepotCacheEntry {
    pub candidate: Candidate,
    pub outcome: PreseedOutcome,
}

impl DepotCacheEntry {
    /// One report line, including the reason for a failure.
    pub fn outcome_label(&self) -> String {
        match &self.outcome {
            PreseedOutcome::AlreadyPresent => "already present".to_owned(),
            PreseedOutcome::Written { bytes } => format!("written ({bytes} bytes)"),
            PreseedOutcome::NotArchived => "not archived".to_owned(),
            PreseedOutcome::Failed { reason } => format!("failed: {reason}"),
        }
    }
}

/// Every candidate's outcome, in request order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DepotCacheReport {
    entries: Vec<DepotCacheEntry>,
}

impl DepotCacheReport {
    pub fn entries(&self) -> &[DepotCacheEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn written(&self) -> usize {
        self.count(|outcome| matches!(outcome, PreseedOutcome::Written { .. }))
    }

    pub fn already_present(&self) -> usize {
        self.count(|outcome| matches!(outcome, PreseedOutcome::AlreadyPresent))
    }

    pub fn not_archived(&self) -> usize {
        self.count(|outcome| matches!(outcome, PreseedOutcome::NotArchived))
    }

    pub fn failed(&self) -> usize {
        self.count(|outcome| matches!(outcome, PreseedOutcome::Failed { .. }))
    }

    /// Candidates whose manifest is on disk, so the request-code path can skip.
    pub fn satisfied(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.outcome.satisfies_install())
            .count()
    }

    /// One dialog line for the report.
    pub fn summary(&self) -> String {
        format!(
            "{} candidate(s): {} written, {} already present, {} not archived, {} failed",
            self.len(),
            self.written(),
            self.already_present(),
            self.not_archived(),
            self.failed()
        )
    }

    fn count(&self, predicate: impl Fn(&PreseedOutcome) -> bool) -> usize {
        self.entries
            .iter()
            .filter(|entry| predicate(&entry.outcome))
            .count()
    }
}

/// Run every wanted candidate through the cache/archive pair.
///
/// One failed candidate is recorded and the loop continues: `preseed` already
/// turns a fetch error, a bad body and a write error into
/// `PreseedOutcome::Failed`, so there is no error path here that could abort the
/// batch.
pub fn preseed_wanted(
    wanted: &[Candidate],
    store: &mut dyn DepotCacheStore,
    fetcher: &mut dyn ArchiveFetcher,
) -> DepotCacheReport {
    let mut store = DynStore(store);
    let mut fetcher = DynFetcher(fetcher);
    let mut entries = Vec::with_capacity(wanted.len());
    for &candidate in wanted {
        let outcome = preseed(&mut store, &mut fetcher, candidate, plausible_manifest);
        tracing::debug!(
            depot = ?candidate.depot_id,
            manifest = ?candidate.manifest_id,
            "depotcache: candidate resolved"
        );
        entries.push(DepotCacheEntry { candidate, outcome });
    }
    DepotCacheReport { entries }
}

/// `preseed` takes `&mut impl Trait` (sized); these two adapters let the host
/// hand in trait objects so the report and the run loop stay non-generic.
struct DynStore<'a>(&'a mut dyn DepotCacheStore);

impl DepotCacheStore for DynStore<'_> {
    fn exists(&self, candidate: Candidate) -> bool {
        self.0.exists(candidate)
    }

    fn write(&mut self, candidate: Candidate, bytes: &[u8]) -> Result<(), String> {
        self.0.write(candidate, bytes)
    }
}

struct DynFetcher<'a>(&'a mut dyn ArchiveFetcher);

impl ArchiveFetcher for DynFetcher<'_> {
    fn fetch_manifest(&mut self, candidate: Candidate) -> Result<Option<Vec<u8>>, String> {
        self.0.fetch_manifest(candidate)
    }
}
