//! The host lifecycle: install the hook batch, then resolve the features.
//!
//! Order is deliberate. The install layer runs first and produces the gate: a
//! capability is disabled exactly when its symbols did not resolve or did not
//! install. The feature pass then reads that gate, so a feature whose capability
//! is already gone does no work at all - it is reported as inactive with the
//! install reason, which is the same reason the user sees for the capability.
//!
//! Both halves are independent of each other and of the live client: the
//! installer is a trait, the feature ports are traits, and this function only
//! composes them. That is what lets the whole lifecycle run on Linux in tests.

use std::path::Path;

use crate::features::{self, DepotCachePorts, FeatureReport, HostSettings};
use crate::install::{self, DetourInstaller, RunReport};
use crate::module::ModuleIdentity;

/// Install the corpus batch for `identity`, then resolve the host features.
///
/// `ports` is `None` when no archive access is wired; the depotcache feature is
/// then reported inactive with that reason. Nothing is fetched or written unless
/// the `depot-injection` capability survived the install pass.
pub fn run_host(
    identity: &ModuleIdentity,
    image: &[u8],
    corpus_root: &Path,
    module_base: usize,
    installer: &mut dyn DetourInstaller,
    settings: &HostSettings,
    ports: Option<DepotCachePorts<'_>>,
) -> RunReport {
    let mut report = install::run(identity, image, corpus_root, module_base, installer);
    let features: FeatureReport = features::run(settings, &report.gate, ports);
    tracing::info!(
        fingerprint = report.fingerprint.as_str(),
        active = features
            .entries()
            .iter()
            .filter(|e| e.state.is_active())
            .count(),
        inactive = features
            .entries()
            .iter()
            .filter(|e| !e.state.is_active())
            .count(),
        "windows host features resolved"
    );
    report.features = Some(features);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::features::{AchievementSettings, CloudSettings, DepotCachePorts, HostFeature};
    use crate::install::InstallError;
    use crate::install::Installed;
    use vapor_forge_features::depotcache::{ArchiveFetcher, Candidate, DepotCacheStore};

    /// Panics if the install pass ever reaches it: a missing corpus must short
    /// circuit before a single detour is written.
    struct NeverInstaller;

    impl DetourInstaller for NeverInstaller {
        fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
            panic!("installer reached with {symbol} at {address:#x}");
        }
    }

    struct NeverStore;

    impl DepotCacheStore for NeverStore {
        fn exists(&self, _candidate: Candidate) -> bool {
            panic!("depotcache store read although the capability is disabled");
        }
        fn write(&mut self, _candidate: Candidate, _bytes: &[u8]) -> Result<(), String> {
            panic!("depotcache store written although the capability is disabled");
        }
    }

    struct NeverFetcher;

    impl ArchiveFetcher for NeverFetcher {
        fn fetch_manifest(&mut self, _candidate: Candidate) -> Result<Option<Vec<u8>>, String> {
            panic!("archive dialled although the capability is disabled");
        }
    }

    fn identity() -> ModuleIdentity {
        ModuleIdentity {
            name: "steamclient".to_owned(),
            bits: 64,
            sha256: "00".repeat(32),
        }
    }

    #[test]
    fn a_failed_install_leaves_every_feature_inactive_with_the_install_reason() {
        let root = std::env::temp_dir().join("vf-host-windows-no-such-corpus");
        let mut installer = NeverInstaller;
        let mut store = NeverStore;
        let mut fetcher = NeverFetcher;
        let settings = HostSettings {
            achievements: AchievementSettings {
                local_rebuild: true,
            },
            ..HostSettings::none()
        };
        let report = run_host(
            &identity(),
            b"not a pe image",
            &root,
            0,
            &mut installer,
            &settings,
            Some(DepotCachePorts {
                store: &mut store,
                fetcher: &mut fetcher,
            }),
        );

        assert!(report.plan.is_none(), "no batch may be planned");
        assert!(report.installed.is_empty());
        assert!(!report.gate.is_enabled(Capability::Achievements));

        let features = report.features.as_ref().expect("features are attached");
        for feature in HostFeature::ALL {
            assert!(
                !features.is_active(feature),
                "{feature:?} must be inactive after a corpus failure"
            );
            assert!(
                features.state(feature).and_then(|s| s.reason()).is_some(),
                "{feature:?} must say why it is inactive"
            );
        }
        let text = report.report();
        assert!(text.contains("Windows host features:"));
        assert!(text.contains("inactive"));
    }

    #[test]
    fn no_feature_runs_while_the_install_gate_keeps_its_own_reason() {
        let mut gate = crate::capability::Gate::all_enabled();
        gate.disable(Capability::CloudControl, "CloseAppCloud did not resolve");
        let plan = features::plan_cloud(&CloudSettings::default());
        let report = features::FeatureReport::new(
            plan,
            Vec::new(),
            features::DepotCacheReport::default(),
            vec![crate::features::FeatureEntry {
                feature: HostFeature::Cloud,
                state: features::FeatureState::inactive("CloseAppCloud did not resolve"),
            }],
        );
        let mut merged = gate.clone();
        report.merge_into(&mut merged);
        assert_eq!(
            merged.reason(Capability::CloudControl),
            Some("CloseAppCloud did not resolve")
        );
    }
}
