use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tracing::debug;
use vapor_forge_config::{AppId, RuntimeConfig};

use crate::family::marker::{remove_if_unborrowed, BorrowerMarkers, MarkerOutcome};
use crate::family::provenance::Provenance;
use crate::family::FamilySettings;

/// Safe business logic for pkg0 injection.
///
/// This module does NOT touch Steam memory. It produces "what to do"
/// decisions that the unsafe hooks layer executes.
pub struct PackageState {
    active: AtomicBool,
    /// Track what we've injected so hot-reload can diff.
    injected_apps: Mutex<HashSet<AppId>>,
    /// Borrower markers stamped while a lender's license covers an app.
    ///
    /// Kept apart from `injected_apps` on purpose: a family-shared app is
    /// deliberately left out of pkg0, so pkg0 membership says nothing about
    /// whether an app is borrowed.
    borrower_markers: Mutex<BorrowerMarkers>,
}

/// What to inject on first pkg0 capture.
pub struct InjectionPlan {
    pub app_ids: Vec<AppId>,
}

/// What changed on hot-reload (config or Lua scripts changed).
pub struct ReloadDiff {
    pub additions: Vec<AppId>,
    pub removals: Vec<AppId>,
}

impl PackageState {
    pub fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            injected_apps: Mutex::new(HashSet::new()),
            borrower_markers: Mutex::new(BorrowerMarkers::new()),
        }
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    pub fn set_active(&self) {
        self.active.store(true, Ordering::Release);
    }

    /// Forget everything learned about the previous account.
    ///
    /// Borrower markers go too: a marker is only meaningful for the account
    /// whose licenses produced it, and a marker left behind is what keeps an app
    /// labelled "from family library" after it has left our library.
    pub fn reset_account_state(&self) {
        self.active.store(false, Ordering::Release);
        self.injected_apps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        self.borrower_markers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }

    /// Stamp the borrower marker for an app a lender covers, returning the
    /// lender it replaced.
    ///
    /// Wired by the host later: the license owner is only visible in the hooks
    /// layer, so nothing in this crate stamps a marker yet.
    pub fn stamp_borrower_marker(&self, app_id: AppId, lender: u32) -> Option<u32> {
        self.borrower_markers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .stamp(app_id, lender)
    }

    /// The lender the app's borrower marker records, if one is stamped.
    pub fn borrower_marker(&self, app_id: AppId) -> Option<u32> {
        self.borrower_markers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .lender(app_id)
    }

    pub fn borrower_marker_count(&self) -> usize {
        self.borrower_markers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Remove the app's marker once it is no longer borrowed.
    ///
    /// Idempotent and safe to call on every refresh: an app without a marker
    /// reports [`MarkerOutcome::Absent`] and nothing is written.
    pub fn remove_borrower_marker(
        &self,
        app_id: AppId,
        real: Provenance,
        settings: FamilySettings,
    ) -> MarkerOutcome {
        remove_if_unborrowed(
            &mut self
                .borrower_markers
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            app_id,
            real,
            settings,
        )
    }

    /// Compute what app IDs to inject into pkg0 on first capture.
    ///
    /// `controlled_ids` = union of config inject IDs + script addappid IDs.
    pub fn compute_injection(&self, controlled_ids: &[AppId]) -> InjectionPlan {
        let ids: Vec<AppId> = controlled_ids.to_vec();
        debug!(count = ids.len(), "package: injection plan computed");
        InjectionPlan { app_ids: ids }
    }

    /// Record what we injected so hot-reload can diff later.
    pub fn record_injected(&self, ids: &[AppId]) {
        let mut guard = self
            .injected_apps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.extend(ids.iter().copied());
    }

    /// Returns whether this AppId has actually been written into pkg0.
    ///
    /// This is runtime diagnostics state, not an ownership or network-policy
    /// signal. `RuntimeConfig::is_controlled_app` is authoritative for policy.
    pub fn is_injected_into_pkg0(&self, app_id: AppId) -> bool {
        self.injected_apps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&app_id)
    }

    pub fn injected_count(&self) -> usize {
        self.injected_apps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Compute the diff between current controlled set and what was previously injected.
    ///
    /// `controlled_ids` = union of config inject IDs + script addappid IDs.
    pub fn compute_hot_reload_diff(&self, controlled_ids: &[AppId]) -> ReloadDiff {
        let new_set: HashSet<AppId> = controlled_ids.iter().copied().collect();
        let guard = self
            .injected_apps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let additions: Vec<AppId> = new_set.difference(&guard).copied().collect();
        let removals: Vec<AppId> = guard.difference(&new_set).copied().collect();

        debug!(
            additions = additions.len(),
            removals = removals.len(),
            "package: reload diff computed"
        );

        ReloadDiff {
            additions,
            removals,
        }
    }

    /// Update the injected set after a hot-reload diff is applied.
    ///
    /// Borrower markers are deliberately untouched: an app dropped from pkg0
    /// because a lender now covers it must keep its marker, and stamping or
    /// clearing is a provenance decision, not a pkg0 one.
    pub fn apply_diff(&self, diff: &ReloadDiff) {
        let mut guard = self
            .injected_apps
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for &id in &diff.removals {
            guard.remove(&id);
        }
        for &id in &diff.additions {
            guard.insert(id);
        }
    }
}

impl Default for PackageState {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the combined controlled-app-IDs list from config + script state.
/// Includes both main app IDs and their DLC IDs (DLC goes into pkg0 so
/// Steam downloads appinfo and handles DLC enumeration natively).
pub fn controlled_app_ids(config: &RuntimeConfig, script_apps: &HashSet<AppId>) -> Vec<AppId> {
    let mut seen: HashSet<AppId> = HashSet::new();
    let mut ids: Vec<AppId> = Vec::new();
    for app in config.apps.inject() {
        if seen.insert(app.id) {
            ids.push(app.id);
        }
        for &dlc in &app.dlc {
            if seen.insert(dlc) {
                ids.push(dlc);
            }
        }
    }
    for &id in script_apps {
        if seen.insert(id) {
            ids.push(id);
        }
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(ids: &[u32]) -> RuntimeConfig {
        let inject = ids
            .iter()
            .map(|&id| vapor_forge_config::InjectApp {
                id: AppId(id),
                dlc: Vec::new(),
                ticket: Default::default(),
                purchase_time: 0,
            })
            .collect();
        RuntimeConfig {
            apps: vapor_forge_config::AppsSection::with_inject(inject),
            ..Default::default()
        }
    }

    #[test]
    fn diff_additions_only() {
        let state = PackageState::new();
        state.record_injected(&[AppId(100), AppId(200)]);
        let diff = state.compute_hot_reload_diff(&[AppId(100), AppId(200), AppId(300)]);
        assert_eq!(diff.additions, vec![AppId(300)]);
        assert!(diff.removals.is_empty());
    }

    #[test]
    fn reports_actual_pkg0_injection_state() {
        let state = PackageState::new();
        assert!(!state.is_injected_into_pkg0(AppId(100)));
        state.record_injected(&[AppId(100)]);
        assert!(state.is_injected_into_pkg0(AppId(100)));
        assert!(!state.is_injected_into_pkg0(AppId(200)));
    }

    #[test]
    fn account_reset_clears_runtime_package_state() {
        let state = PackageState::new();
        state.set_active();
        state.record_injected(&[AppId(100), AppId(200)]);

        state.reset_account_state();

        assert!(!state.is_active());
        assert_eq!(state.injected_count(), 0);
    }

    #[test]
    fn diff_removals_only() {
        let state = PackageState::new();
        state.record_injected(&[AppId(100), AppId(200), AppId(300)]);
        let diff = state.compute_hot_reload_diff(&[AppId(100)]);
        assert!(diff.additions.is_empty());
        assert!(diff.removals.contains(&AppId(200)));
        assert!(diff.removals.contains(&AppId(300)));
    }

    #[test]
    fn diff_additions_and_removals() {
        let state = PackageState::new();
        state.record_injected(&[AppId(100), AppId(200)]);
        let diff = state.compute_hot_reload_diff(&[AppId(200), AppId(300)]);
        assert_eq!(diff.additions, vec![AppId(300)]);
        assert_eq!(diff.removals, vec![AppId(100)]);
    }

    #[test]
    fn diff_no_change() {
        let state = PackageState::new();
        state.record_injected(&[AppId(100), AppId(200)]);
        let diff = state.compute_hot_reload_diff(&[AppId(100), AppId(200)]);
        assert!(diff.additions.is_empty());
        assert!(diff.removals.is_empty());
    }

    #[test]
    fn apply_diff_updates_state() {
        let state = PackageState::new();
        state.record_injected(&[AppId(100), AppId(200)]);

        let diff = ReloadDiff {
            additions: vec![AppId(300)],
            removals: vec![AppId(100)],
        };
        state.apply_diff(&diff);

        let diff2 = state.compute_hot_reload_diff(&[AppId(200), AppId(300)]);
        assert!(diff2.additions.is_empty());
        assert!(diff2.removals.is_empty());
    }

    #[test]
    fn controlled_app_ids_dedup_script_apps() {
        let config = make_config(&[100, 200]);
        let script_apps: HashSet<AppId> = [AppId(200), AppId(300)].into_iter().collect();
        let ids = controlled_app_ids(&config, &script_apps);
        // Config-order entries come first, then any script-only extras. Script
        // apps that duplicate config entries are dropped.
        assert_eq!(&ids[..2], &[AppId(100), AppId(200)]);
        assert!(ids.contains(&AppId(300)));
        assert_eq!(ids.len(), 3);
    }

    // Invariant 3, through the package layer: removal is idempotent and says so.
    #[test]
    fn a_borrower_marker_is_removed_once_and_then_reported_absent() {
        let state = PackageState::new();
        state.stamp_borrower_marker(AppId(570), 9_999_999);
        assert_eq!(state.borrower_marker(AppId(570)), Some(9_999_999));

        let settings = FamilySettings::default();
        let first = state.remove_borrower_marker(AppId(570), Provenance::Owned, settings);
        assert!(first.changed());
        assert_eq!(state.borrower_marker_count(), 0);

        let second = state.remove_borrower_marker(AppId(570), Provenance::Owned, settings);
        assert_eq!(second, MarkerOutcome::Absent);
        assert!(!second.changed());
    }

    #[test]
    fn a_family_app_left_out_of_pkg0_keeps_its_borrower_marker() {
        let state = PackageState::new();
        state.record_injected(&[AppId(100), AppId(200), AppId(570)]);
        state.stamp_borrower_marker(AppId(570), 4_242);

        // 570 leaves pkg0 because a lender covers it, not because it left the
        // library: the marker must stay.
        let diff = ReloadDiff {
            additions: Vec::new(),
            removals: vec![AppId(570)],
        };
        state.apply_diff(&diff);

        assert_eq!(state.borrower_marker(AppId(570)), Some(4_242));
        assert_eq!(
            state.remove_borrower_marker(
                AppId(570),
                Provenance::FamilyShared,
                FamilySettings::default()
            ),
            MarkerOutcome::StillBorrowed { lender: 4_242 }
        );
    }

    #[test]
    fn an_account_switch_drops_every_borrower_marker() {
        let state = PackageState::new();
        state.stamp_borrower_marker(AppId(570), 4_242);
        state.stamp_borrower_marker(AppId(730), 5_353);

        state.reset_account_state();

        assert_eq!(state.borrower_marker_count(), 0);
        assert_eq!(state.borrower_marker(AppId(570)), None);
    }
}
