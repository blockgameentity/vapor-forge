//! Feature parity tests: the properties that must hold on the build host.
//!
//! Nothing here needs a live client, a config file or a network: the ports are
//! fakes and the decisions are pure. What is proven:
//!
//! * a controlled app whose cloud backend has no credentials is **refused with a
//!   reason and never redirected**;
//! * an absent identity override is an explicit "no override", and an unusable
//!   one is refused instead of invented;
//! * the depotcache loop reports one outcome per candidate and **never aborts**
//!   the batch after a failure;
//! * the report lists **every** inactive feature with its reason;
//! * a disabled capability means **no work at all** - no archive dial, no
//!   depotcache write, no identity selection, no achievement switch flip.

use std::collections::BTreeMap;
use std::sync::MutexGuard;

use vapor_forge_config::{AppId, DepotId, ManifestId};
use vapor_forge_features::achievements::local_rebuild_enabled;
use vapor_forge_features::depotcache::{ArchiveFetcher, Candidate, DepotCacheStore};

use crate::capability::{Capability, Gate};
use crate::features::{
    run, AchievementSettings, CloudBackend, CloudDecision, CloudMode, CloudPlan, CloudSettings,
    DepotCachePorts, FeatureReport, FeatureState, HostFeature, HostSettings, IdentitySelection,
    IdentitySettings,
};
use crate::install::RunReport;

/// `run` writes the process-wide achievement switch, so tests that call it are
/// serialised; no test can then observe another test's flip. The lock is shared
/// with `entry::tests`, whose `worker::run` reaches the same switch. The fakes and
/// the helpers below are shared by the per-property submodules.
fn serial() -> MutexGuard<'static, ()> {
    crate::test_lock::serial()
}

fn manifest(depot: u32, manifest: u64) -> Candidate {
    Candidate {
        depot_id: DepotId(depot),
        manifest_id: ManifestId(manifest),
    }
}

#[derive(Default)]
struct FakeStore {
    files: BTreeMap<String, Vec<u8>>,
    writes: usize,
}

impl FakeStore {
    fn with(candidate: Candidate) -> Self {
        let mut store = Self::default();
        store.files.insert(candidate.file_name(), vec![0u8; 8]);
        store
    }
}

impl DepotCacheStore for FakeStore {
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
    Body,
    NotFound,
    Transient,
}

struct ScriptedFetcher {
    answers: Vec<Answer>,
    calls: Vec<Candidate>,
}

impl ScriptedFetcher {
    fn new(answers: Vec<Answer>) -> Self {
        Self {
            answers,
            calls: Vec::new(),
        }
    }
}

impl ArchiveFetcher for ScriptedFetcher {
    fn fetch_manifest(&mut self, candidate: Candidate) -> Result<Option<Vec<u8>>, String> {
        self.calls.push(candidate);
        match self.answers.remove(0) {
            Answer::Body => Ok(Some(b"manifest-body".to_vec())),
            Answer::NotFound => Ok(None),
            Answer::Transient => Err("archive timeout".to_owned()),
        }
    }
}

/// Fails the test if any capability-disabled run reaches the archive or disk.
struct PanicStore;

impl DepotCacheStore for PanicStore {
    fn exists(&self, _candidate: Candidate) -> bool {
        panic!("depotcache store read although depot-injection is disabled");
    }
    fn write(&mut self, _candidate: Candidate, _bytes: &[u8]) -> Result<(), String> {
        panic!("depotcache store written although depot-injection is disabled");
    }
}

struct PanicFetcher;

impl ArchiveFetcher for PanicFetcher {
    fn fetch_manifest(&mut self, _candidate: Candidate) -> Result<Option<Vec<u8>>, String> {
        panic!("archive dialled although depot-injection is disabled");
    }
}

fn configured_cloud(credentials_present: bool) -> CloudSettings {
    CloudSettings {
        backend: CloudBackend::Cumulus {
            endpoint: Some("https://cloud.example".to_owned()),
            credentials_present,
        },
        controlled_apps: vec![AppId(730), AppId(440)],
    }
}

fn state_of(report: &FeatureReport, feature: HostFeature) -> &FeatureState {
    report.state(feature).expect("every feature is reported")
}

mod cloud_identity;
mod depot;
mod gate;
