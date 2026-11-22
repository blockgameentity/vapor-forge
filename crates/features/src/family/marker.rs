//! The borrower marker.
//!
//! While a lender's license covers an app we stamp a marker, so the library
//! presents it as borrowed rather than as a title of ours. The marker has to go
//! the moment the app is no longer borrowed: a marker that outlives the license
//! leaves the app labelled "from family library" for good, which is exactly the
//! second defect the Windows fork had to fix.
//!
//! There is deliberately no add path in [`remove_if_unborrowed`]: removing a
//! marker takes information away and can never put an app back into a library,
//! so the pass cannot resurrect an app that left it. Stamping happens at the
//! one place that can see the license, and is a caller decision.

use crate::family::provenance::Provenance;
use crate::family::FamilySettings;
use std::collections::BTreeMap;
use vapor_forge_config::AppId;

/// The markers currently stamped, keyed by app.
///
/// One marker per app: a second lender replaces the first instead of stacking,
/// so the set cannot grow without bound while Steam re-reports the same app.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BorrowerMarkers {
    lenders: BTreeMap<AppId, u32>,
}

impl BorrowerMarkers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Stamp the marker for an app a lender covers, returning any lender it
    /// replaced. Wired by the host later: only the hooks layer can see the
    /// license owner, so no stamp site exists in this crate.
    pub fn stamp(&mut self, app_id: AppId, lender: u32) -> Option<u32> {
        self.lenders.insert(app_id, lender)
    }

    /// The lender whose license the marker records, if any.
    pub fn lender(&self, app_id: AppId) -> Option<u32> {
        self.lenders.get(&app_id).copied()
    }

    pub fn contains(&self, app_id: AppId) -> bool {
        self.lenders.contains_key(&app_id)
    }

    /// Drop one marker without consulting provenance.
    pub fn forget(&mut self, app_id: AppId) -> Option<u32> {
        self.lenders.remove(&app_id)
    }

    pub fn len(&self) -> usize {
        self.lenders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lenders.is_empty()
    }

    /// Every stamped app, in app-id order (deterministic for logs and tests).
    pub fn app_ids(&self) -> impl Iterator<Item = AppId> + '_ {
        self.lenders.keys().copied()
    }

    /// Drop every marker. Used on an account switch, where no marker can be
    /// trusted any more.
    pub fn clear(&mut self) {
        self.lenders.clear();
    }
}

/// What happened to one app's marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarkerOutcome {
    /// No marker was stamped for this app: nothing to do.
    Absent,
    /// The marker was dropped because the app is no longer borrowed.
    Removed { lender: u32 },
    /// A lender still covers the app, so the marker stays.
    StillBorrowed { lender: u32 },
    /// The cleanup pass is switched off; whatever was stamped stays stamped.
    Disabled,
}

impl MarkerOutcome {
    /// Whether the caller has state to write back.
    pub const fn changed(self) -> bool {
        matches!(self, Self::Removed { .. })
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Removed { .. } => "removed",
            Self::StillBorrowed { .. } => "still-borrowed",
            Self::Disabled => "disabled",
        }
    }
}

/// Remove `app_id`'s marker once the app is no longer borrowed.
///
/// Idempotent: the second call reports [`MarkerOutcome::Absent`] and writes
/// nothing. A marker whose app is still borrowed is left alone, so re-running
/// the pass is safe at any time. Nothing here indexes, unwraps or divides, so
/// an app id we have never seen is simply an absent marker.
pub fn remove_if_unborrowed(
    markers: &mut BorrowerMarkers,
    app_id: AppId,
    real: Provenance,
    settings: FamilySettings,
) -> MarkerOutcome {
    if !settings.clean_borrower_markers {
        return MarkerOutcome::Disabled;
    }
    let Some(lender) = markers.lender(app_id) else {
        tracing::debug!(app_id = app_id.0, "family: no borrower marker to remove");
        return MarkerOutcome::Absent;
    };
    if real.is_borrowed() {
        tracing::debug!(
            app_id = app_id.0,
            lender,
            "family: borrower marker kept, app is still borrowed"
        );
        return MarkerOutcome::StillBorrowed { lender };
    }
    markers.forget(app_id);
    tracing::info!(
        app_id = app_id.0,
        lender,
        provenance = real.label(),
        "family: borrower marker removed"
    );
    MarkerOutcome::Removed { lender }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LENDER: u32 = 9_999_999;

    fn stamped() -> BorrowerMarkers {
        let mut markers = BorrowerMarkers::new();
        markers.stamp(AppId(480), LENDER);
        markers
    }

    #[test]
    fn a_marker_is_removed_once_the_lender_is_gone() {
        let mut markers = stamped();
        let outcome = remove_if_unborrowed(
            &mut markers,
            AppId(480),
            Provenance::Owned,
            FamilySettings::default(),
        );
        assert_eq!(outcome, MarkerOutcome::Removed { lender: LENDER });
        assert!(outcome.changed());
        assert!(markers.is_empty());
    }

    // Invariant 3: idempotent, and the second call says there is nothing to do.
    #[test]
    fn removing_twice_is_a_no_op_that_says_so() {
        let mut markers = stamped();
        let settings = FamilySettings::default();
        assert!(
            remove_if_unborrowed(&mut markers, AppId(480), Provenance::Unowned, settings).changed()
        );
        let after_first = markers.clone();

        let second = remove_if_unborrowed(&mut markers, AppId(480), Provenance::Unowned, settings);
        assert_eq!(second, MarkerOutcome::Absent);
        assert!(!second.changed());
        assert_eq!(markers, after_first, "a no-op must not write anything");
    }

    #[test]
    fn an_absent_marker_is_reported_as_absent_for_every_provenance() {
        for real in Provenance::ALL {
            let mut markers = BorrowerMarkers::new();
            let outcome =
                remove_if_unborrowed(&mut markers, AppId(480), real, FamilySettings::default());
            assert_eq!(outcome, MarkerOutcome::Absent, "{real:?}");
            assert!(!outcome.changed());
            assert!(markers.is_empty());
        }
    }

    // Invariant 3: removal only ever takes state away, so an app that left the
    // library cannot come back through this pass.
    #[test]
    fn removal_never_resurrects_an_app() {
        let mut markers = stamped();
        let settings = FamilySettings::default();
        remove_if_unborrowed(&mut markers, AppId(480), Provenance::Unowned, settings);
        for real in Provenance::ALL {
            let outcome = remove_if_unborrowed(&mut markers, AppId(480), real, settings);
            assert_eq!(outcome, MarkerOutcome::Absent);
        }
        assert!(markers.is_empty());
        assert_eq!(markers.app_ids().count(), 0);
    }

    #[test]
    fn a_still_borrowed_app_keeps_its_marker() {
        let mut markers = stamped();
        let outcome = remove_if_unborrowed(
            &mut markers,
            AppId(480),
            Provenance::FamilyShared,
            FamilySettings::default(),
        );
        assert_eq!(outcome, MarkerOutcome::StillBorrowed { lender: LENDER });
        assert!(!outcome.changed());
        assert_eq!(markers.lender(AppId(480)), Some(LENDER));
    }

    #[test]
    fn the_legacy_setting_leaves_the_marker_behind() {
        let mut markers = stamped();
        let outcome = remove_if_unborrowed(
            &mut markers,
            AppId(480),
            Provenance::Owned,
            FamilySettings::legacy(),
        );
        assert_eq!(outcome, MarkerOutcome::Disabled);
        assert_eq!(markers.lender(AppId(480)), Some(LENDER));
    }

    #[test]
    fn one_marker_per_app_and_a_second_lender_replaces_the_first() {
        let mut markers = BorrowerMarkers::new();
        assert_eq!(markers.stamp(AppId(480), LENDER), None);
        assert_eq!(markers.stamp(AppId(480), 4_242), Some(LENDER));
        assert_eq!(markers.len(), 1);
        assert_eq!(markers.lender(AppId(480)), Some(4_242));
    }

    #[test]
    fn app_ids_are_ordered_and_clear_empties_the_set() {
        let mut markers = BorrowerMarkers::new();
        markers.stamp(AppId(730), 1);
        markers.stamp(AppId(480), 2);
        assert_eq!(
            markers.app_ids().collect::<Vec<_>>(),
            vec![AppId(480), AppId(730)]
        );
        markers.clear();
        assert!(markers.is_empty());
        assert!(!markers.contains(AppId(480)));
    }
}
