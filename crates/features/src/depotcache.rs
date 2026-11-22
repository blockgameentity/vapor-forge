//! Pre-seed `<steam>/depotcache` before Steam asks for a manifest request code.
//!
//! Steam installs a depot without any request-code round trip when
//! `<steam>/depotcache/<depot>_<gid>.manifest` already exists, so an archive hit
//! removes an entire third-party request-code fetch from the install path. This
//! module owns the decision logic only: fetching and the filesystem are traits,
//! so the host wires the real archive client and the real directory, and the
//! tests run without either.
//!
//! Invariants:
//! * a miss (404) is not an error and writes nothing;
//! * a bad body is never written, even when the fetch succeeded;
//! * a transient failure leaves the cache untouched and says so;
//! * nothing here ever reads or writes outside `<steam>/depotcache`.

use vapor_forge_config::{DepotId, ManifestId};

/// One depot/manifest pair the install path wants on disk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Candidate {
    pub depot_id: DepotId,
    pub manifest_id: ManifestId,
}

impl Candidate {
    pub fn file_name(&self) -> String {
        format!("{}_{}.manifest", self.depot_id, self.manifest_id)
    }
}

/// What happened to one candidate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreseedOutcome {
    /// The manifest was already on disk; nothing was fetched or written.
    AlreadyPresent,
    /// The archived manifest was validated and written.
    Written { bytes: usize },
    /// The archive answered 404: this manifest is genuinely not archived yet.
    NotArchived,
    /// Anything else. The cache is unchanged; the caller logs the reason.
    Failed { reason: String },
}

impl PreseedOutcome {
    /// True when the caller may skip the request-code path for this depot.
    pub fn satisfies_install(&self) -> bool {
        matches!(self, Self::AlreadyPresent | Self::Written { .. })
    }
}

/// The on-disk depot cache.
pub trait DepotCacheStore {
    fn exists(&self, candidate: Candidate) -> bool;
    fn write(&mut self, candidate: Candidate, bytes: &[u8]) -> Result<(), String>;
}

/// The manifest archive.
pub trait ArchiveFetcher {
    /// `Ok(Some(bytes))` on 200, `Ok(None)` on a definitive 404 (not archived),
    /// `Err(_)` for anything transient.
    fn fetch_manifest(&mut self, candidate: Candidate) -> Result<Option<Vec<u8>>, String>;
}

/// Reject bodies that must never reach the cache.
///
/// Deliberately weak: it only refuses input that cannot be a depot manifest at
/// all. Hosts that have the real parser must pass their own validator instead
/// rather than trusting this.
pub fn plausible_manifest(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() < 8 {
        return Err(format!("body too short to be a manifest ({} bytes)", bytes.len()));
    }
    Ok(())
}

/// Run one candidate through the cache/archive pair.
pub fn preseed(
    store: &mut impl DepotCacheStore,
    fetcher: &mut impl ArchiveFetcher,
    candidate: Candidate,
    validate: impl Fn(&[u8]) -> Result<(), String>,
) -> PreseedOutcome {
    if store.exists(candidate) {
        tracing::debug!(
            depot = ?candidate.depot_id,
            manifest = ?candidate.manifest_id,
            "depotcache: already cached"
        );
        return PreseedOutcome::AlreadyPresent;
    }

    let body = match fetcher.fetch_manifest(candidate) {
        Ok(Some(body)) => body,
        Ok(None) => {
            tracing::debug!(
                depot = ?candidate.depot_id,
                "depotcache: not archived"
            );
            return PreseedOutcome::NotArchived;
        }
        Err(reason) => {
            tracing::warn!(
                depot = ?candidate.depot_id,
                reason = reason.as_str(),
                "depotcache: archive fetch failed"
            );
            return PreseedOutcome::Failed { reason };
        }
    };

    if let Err(reason) = validate(&body) {
        tracing::warn!(
            depot = ?candidate.depot_id,
            bytes = body.len(),
            reason = reason.as_str(),
            "depotcache: refusing unvalidated manifest"
        );
        return PreseedOutcome::Failed { reason };
    }

    match store.write(candidate, &body) {
        Ok(()) => {
            tracing::info!(
                depot = ?candidate.depot_id,
                manifest = ?candidate.manifest_id,
                bytes = body.len(),
                "depotcache: pre-seeded"
            );
            PreseedOutcome::Written { bytes: body.len() }
        }
        Err(reason) => PreseedOutcome::Failed { reason },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Store {
        files: BTreeMap<String, Vec<u8>>,
        writes: usize,
    }

    impl DepotCacheStore for Store {
        fn exists(&self, candidate: Candidate) -> bool {
            self.files.contains_key(&candidate.file_name())
        }
        fn write(&mut self, candidate: Candidate, bytes: &[u8]) -> Result<(), String> {
            self.writes += 1;
            self.files.insert(candidate.file_name(), bytes.to_vec());
            Ok(())
        }
    }

    enum Answer {
        Body(Vec<u8>),
        NotFound,
        Transient,
    }

    struct Fetcher {
        answer: Answer,
        calls: usize,
    }

    impl ArchiveFetcher for Fetcher {
        fn fetch_manifest(&mut self, _candidate: Candidate) -> Result<Option<Vec<u8>>, String> {
            self.calls += 1;
            match &self.answer {
                Answer::Body(body) => Ok(Some(body.clone())),
                Answer::NotFound => Ok(None),
                Answer::Transient => Err("timeout".to_owned()),
            }
        }
    }

    fn candidate() -> Candidate {
        Candidate {
            depot_id: DepotId(228_980),
            manifest_id: ManifestId(0x2dfb_0a5d_50dd_9066),
        }
    }

    #[test]
    fn file_name_matches_steams_depotcache_layout() {
        assert_eq!(candidate().file_name(), format!("228980_{}.manifest", 0x2dfb_0a5d_50dd_9066u64));
    }

    #[test]
    fn an_existing_manifest_is_left_alone_and_costs_no_fetch() {
        let mut store = Store::default();
        store
            .files
            .insert(candidate().file_name(), vec![0u8; 16]);
        let mut fetcher = Fetcher {
            answer: Answer::Body(vec![0u8; 64]),
            calls: 0,
        };
        assert_eq!(
            preseed(&mut store, &mut fetcher, candidate(), plausible_manifest),
            PreseedOutcome::AlreadyPresent
        );
        assert_eq!(fetcher.calls, 0);
        assert_eq!(store.writes, 0);
    }

    #[test]
    fn a_validated_body_is_written_once() {
        let mut store = Store::default();
        let mut fetcher = Fetcher {
            answer: Answer::Body(vec![7u8; 128]),
            calls: 0,
        };
        let outcome = preseed(&mut store, &mut fetcher, candidate(), plausible_manifest);
        assert_eq!(outcome, PreseedOutcome::Written { bytes: 128 });
        assert!(outcome.satisfies_install());
        assert_eq!(store.writes, 1);
        assert_eq!(store.files[&candidate().file_name()].len(), 128);
    }

    #[test]
    fn a_404_writes_nothing_and_is_reported_as_not_archived() {
        let mut store = Store::default();
        let mut fetcher = Fetcher {
            answer: Answer::NotFound,
            calls: 0,
        };
        let outcome = preseed(&mut store, &mut fetcher, candidate(), plausible_manifest);
        assert_eq!(outcome, PreseedOutcome::NotArchived);
        assert!(!outcome.satisfies_install());
        assert_eq!(store.writes, 0);
    }

    #[test]
    fn a_transient_failure_leaves_the_cache_untouched() {
        let mut store = Store::default();
        let mut fetcher = Fetcher {
            answer: Answer::Transient,
            calls: 0,
        };
        match preseed(&mut store, &mut fetcher, candidate(), plausible_manifest) {
            PreseedOutcome::Failed { reason } => assert_eq!(reason, "timeout"),
            other => panic!("expected failure, got {other:?}"),
        }
        assert_eq!(store.writes, 0);
    }

    #[test]
    fn an_unvalidated_body_is_never_written() {
        let mut store = Store::default();
        let mut fetcher = Fetcher {
            answer: Answer::Body(vec![0u8; 4]),
            calls: 0,
        };
        match preseed(&mut store, &mut fetcher, candidate(), plausible_manifest) {
            PreseedOutcome::Failed { reason } => assert!(reason.contains("too short")),
            other => panic!("expected failure, got {other:?}"),
        }
        assert_eq!(store.writes, 0, "an unvalidated body must not reach disk");
    }
}
