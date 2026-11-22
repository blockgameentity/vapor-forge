//! Which host features are active, and every inactive one with its reason.
//!
//! The degradation contract applies to features exactly as it applies to
//! capabilities: a feature that cannot work is switched off *with a reason* that
//! the user can act on. The reasons live in a [`Gate`], so a disabled feature is
//! reported by the same code that reports a disabled hook and a partial batch
//! never looks like a working one.
//!
//! The gate belongs to the *feature* report, not to the install gate: a feature
//! can be inactive because the user turned it off, because the configuration
//! cannot be honoured (cloud refusal) or because a hook did not resolve. Merging
//! it into the install gate is offered ([`FeatureReport::merge_into`]) for callers
//! that want one dialog, and the reasons are then the first ones recorded.

use crate::capability::{Capability, Gate};
use crate::features::cloud::CloudPlan;
use crate::features::depotcache::DepotCacheReport;
use crate::features::identity::IdentitySelection;

/// The four host-agnostic features this host wires.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HostFeature {
    Cloud,
    DepotCache,
    Identity,
    Achievements,
}

impl HostFeature {
    pub const ALL: [Self; 4] = [
        Self::Cloud,
        Self::DepotCache,
        Self::Identity,
        Self::Achievements,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cloud => "cloud-control",
            Self::DepotCache => "depot-cache-preseed",
            Self::Identity => "per-game-identity",
            Self::Achievements => "achievements",
        }
    }

    /// The capability whose gate entry governs this feature.
    ///
    /// Per-game identity follows `ownership`: the id the client presents comes
    /// from the same `CUser` path ownership is read from, and an ownership hook
    /// that did not resolve cannot honour an override either.
    pub fn capability(self) -> Capability {
        match self {
            Self::Cloud => Capability::CloudControl,
            Self::DepotCache => Capability::DepotInjection,
            Self::Identity => Capability::Ownership,
            Self::Achievements => Capability::Achievements,
        }
    }
}

/// Active, or inactive with a reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeatureState {
    Active { detail: String },
    Inactive { reason: String },
}

impl FeatureState {
    pub fn active(detail: impl Into<String>) -> Self {
        Self::Active {
            detail: detail.into(),
        }
    }

    pub fn inactive(reason: impl Into<String>) -> Self {
        Self::Inactive {
            reason: reason.into(),
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Active { detail } => Some(detail),
            Self::Inactive { .. } => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Inactive { reason } => Some(reason),
            Self::Active { .. } => None,
        }
    }
}

/// One feature and its state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FeatureEntry {
    pub feature: HostFeature,
    pub state: FeatureState,
}

/// The whole feature half of the host report.
#[derive(Clone, Debug)]
pub struct FeatureReport {
    pub cloud: CloudPlan,
    pub identities: Vec<IdentitySelection>,
    pub depotcache: DepotCacheReport,
    entries: Vec<FeatureEntry>,
    gate: Gate,
}

impl FeatureReport {
    pub fn new(
        cloud: CloudPlan,
        identities: Vec<IdentitySelection>,
        depotcache: DepotCacheReport,
        entries: Vec<FeatureEntry>,
    ) -> Self {
        let mut gate = Gate::all_enabled();
        for entry in &entries {
            if let FeatureState::Inactive { reason } = &entry.state {
                gate.disable(entry.feature.capability(), reason.clone());
            }
        }
        Self {
            cloud,
            identities,
            depotcache,
            entries,
            gate,
        }
    }

    /// Every feature, in [`HostFeature::ALL`] order.
    pub fn entries(&self) -> &[FeatureEntry] {
        &self.entries
    }

    pub fn state(&self, feature: HostFeature) -> Option<&FeatureState> {
        self.entries
            .iter()
            .find(|entry| entry.feature == feature)
            .map(|entry| &entry.state)
    }

    pub fn is_active(&self, feature: HostFeature) -> bool {
        self.state(feature).is_some_and(FeatureState::is_active)
    }

    /// The inactive features with their reasons, in gate form.
    pub fn gate(&self) -> &Gate {
        &self.gate
    }

    /// Merge the inactive features into an existing gate (first reason wins).
    pub fn merge_into(&self, gate: &mut Gate) {
        for (capability, reason) in self.gate.disabled() {
            gate.disable(capability, reason);
        }
    }

    /// Human-facing summary: every feature, and each inactive one's reason.
    pub fn report(&self) -> String {
        let mut out = String::from("Windows host features:\n");
        for entry in &self.entries {
            match &entry.state {
                FeatureState::Active { detail } => out.push_str(&format!(
                    "  - {}: active ({detail})\n",
                    entry.feature.label()
                )),
                FeatureState::Inactive { reason } => out.push_str(&format!(
                    "  - {}: inactive ({reason})\n",
                    entry.feature.label()
                )),
            }
        }
        if !self.depotcache.is_empty() {
            out.push_str("  depotcache candidates:\n");
            for entry in self.depotcache.entries() {
                out.push_str(&format!(
                    "    - {}: {}\n",
                    entry.candidate.file_name(),
                    entry.outcome_label()
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(feature: HostFeature, reason: Option<&str>) -> FeatureEntry {
        FeatureEntry {
            feature,
            state: match reason {
                Some(reason) => FeatureState::inactive(reason),
                None => FeatureState::active("configured"),
            },
        }
    }

    #[test]
    fn every_inactive_feature_appears_in_the_gate_with_its_reason() {
        let entries = vec![
            entry(HostFeature::Cloud, Some("no endpoint")),
            entry(HostFeature::DepotCache, None),
            entry(HostFeature::Identity, Some("no override")),
            entry(HostFeature::Achievements, Some("rebuild off")),
        ];
        let report = FeatureReport::new(
            CloudPlan::default(),
            Vec::new(),
            DepotCacheReport::default(),
            entries,
        );
        assert_eq!(report.gate().disabled().count(), 3);
        assert_eq!(
            report.gate().reason(Capability::CloudControl),
            Some("no endpoint")
        );
        assert_eq!(
            report.gate().reason(Capability::Ownership),
            Some("no override")
        );
        assert_eq!(
            report.gate().reason(Capability::Achievements),
            Some("rebuild off")
        );
        assert!(report.gate().is_enabled(Capability::DepotInjection));

        let text = report.report();
        for line in [
            "cloud-control",
            "depot-cache-preseed",
            "per-game-identity",
            "achievements",
        ] {
            assert!(text.contains(line), "report was {text}");
        }
        assert!(text.contains("no endpoint"));
    }

    #[test]
    fn merging_leaves_an_earlier_reason_in_place() {
        let report = FeatureReport::new(
            CloudPlan::default(),
            Vec::new(),
            DepotCacheReport::default(),
            vec![entry(HostFeature::Cloud, Some("no endpoint"))],
        );
        let mut gate = Gate::all_enabled();
        gate.disable(Capability::CloudControl, "CloseAppCloud did not resolve");
        report.merge_into(&mut gate);
        assert_eq!(
            gate.reason(Capability::CloudControl),
            Some("CloseAppCloud did not resolve")
        );
    }
}
