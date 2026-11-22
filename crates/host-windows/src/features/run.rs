//! Turn the host's resolved settings into the four features' decisions.
//!
//! Order, and the reason for it:
//!
//! 1. read the install gate first — a feature whose capability is disabled does
//!    **no work at all** (no archive fetch, no depotcache write, no identity
//!    selection, no flip of the achievement switch);
//! 2. otherwise resolve the feature from settings, recording either an active
//!    detail line or the reason it is inactive;
//! 3. hand the whole set to [`FeatureReport`], which keeps the reasons in a gate.
//!
//! Nothing here installs a hook, patches the module image or dials anything: the
//! only I/O is the depotcache pre-seed, and only through the ports the host
//! supplies (and only when that capability is enabled).

use crate::capability::{Capability, Gate};
use crate::features::achievements::{self, AchievementSettings};
use crate::features::cloud::{plan_cloud, CloudMode, CloudPlan, CloudSettings};
use crate::features::depotcache::{
    preseed_wanted, DepotCachePorts, DepotCacheReport, DepotCacheSettings,
};
use crate::features::identity::{select_identities, IdentitySelection, IdentitySettings};
use crate::features::report::{FeatureEntry, FeatureReport, FeatureState, HostFeature};
use crate::features::settings::HostSettings;

/// Resolve every feature, gated by `gate`, and report which are active.
///
/// `ports` is `None` when the host has no archive access wired; the depotcache
/// feature is then inactive with that reason instead of silently doing nothing.
pub fn run(
    settings: &HostSettings,
    gate: &Gate,
    ports: Option<DepotCachePorts<'_>>,
) -> FeatureReport {
    let (cloud, cloud_entry) = cloud_entry(&settings.cloud, gate);
    let (depotcache, depot_entry) = depotcache_entry(&settings.depotcache, gate, ports);
    let (identities, identity_entry) = identity_entry(&settings.identity, gate);
    let achievements_entry = achievements_entry(&settings.achievements, gate);
    FeatureReport::new(
        cloud,
        identities,
        depotcache,
        vec![cloud_entry, depot_entry, identity_entry, achievements_entry],
    )
}

fn cloud_entry(settings: &CloudSettings, gate: &Gate) -> (CloudPlan, FeatureEntry) {
    let mut plan = plan_cloud(settings);
    let state = match gate.reason(Capability::CloudControl) {
        Some(reason) => {
            // The capability is off, so the app is left with Steam's own cloud.
            // The install reason wins over the configuration reason.
            plan.mode = CloudMode::LeaveAlone;
            plan.reason = Some(reason.to_owned());
            FeatureState::inactive(reason)
        }
        None => match plan.mode {
            CloudMode::Redirect => FeatureState::active(plan.summary()),
            CloudMode::LeaveAlone | CloudMode::Refuse => FeatureState::inactive(plan.summary()),
        },
    };
    (
        plan,
        FeatureEntry {
            feature: HostFeature::Cloud,
            state,
        },
    )
}

fn depotcache_entry(
    settings: &DepotCacheSettings,
    gate: &Gate,
    ports: Option<DepotCachePorts<'_>>,
) -> (DepotCacheReport, FeatureEntry) {
    let entry = |state| FeatureEntry {
        feature: HostFeature::DepotCache,
        state,
    };
    if let Some(reason) = gate.reason(Capability::DepotInjection) {
        return (
            DepotCacheReport::default(),
            entry(FeatureState::inactive(reason)),
        );
    }
    if settings.wanted.is_empty() {
        return (
            DepotCacheReport::default(),
            entry(FeatureState::inactive(
                "no depot/manifest pairs were wanted by the install plan",
            )),
        );
    }
    let Some(ports) = ports else {
        return (
            DepotCacheReport::default(),
            entry(FeatureState::inactive(
                "no archive fetcher or depotcache store is wired to the Windows host",
            )),
        );
    };
    let report = preseed_wanted(&settings.wanted, ports.store, ports.fetcher);
    let state = FeatureState::active(report.summary());
    (report, entry(state))
}

fn identity_entry(
    settings: &IdentitySettings,
    gate: &Gate,
) -> (Vec<IdentitySelection>, FeatureEntry) {
    let entry = |state| FeatureEntry {
        feature: HostFeature::Identity,
        state,
    };
    if let Some(reason) = gate.reason(Capability::Ownership) {
        // No selection is computed: with the ownership hook unresolved there is
        // nothing that could present the id, and inventing one is never allowed.
        return (Vec::new(), entry(FeatureState::inactive(reason)));
    }
    let selections = select_identities(settings);
    let state = if selections.is_empty() {
        FeatureState::inactive(
            "no per-game SteamID override is configured; identity stays with Steam's own account",
        )
    } else if let Some(reason) = selections.iter().find_map(|selection| selection.reason()) {
        FeatureState::inactive(format!(
            "a configured per-game SteamID override is unusable: {reason}"
        ))
    } else {
        let overrides = selections.iter().filter(|s| s.is_override()).count();
        FeatureState::active(format!("{overrides} per-game SteamID override(s) selected"))
    };
    (selections, entry(state))
}

fn achievements_entry(settings: &AchievementSettings, gate: &Gate) -> FeatureEntry {
    let state = match gate.reason(Capability::Achievements) {
        Some(reason) => FeatureState::inactive(reason),
        None => achievements::apply(settings),
    };
    FeatureEntry {
        feature: HostFeature::Achievements,
        state,
    }
}
