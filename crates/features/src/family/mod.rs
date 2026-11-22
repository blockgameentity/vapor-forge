//! Family-share semantics: provenance, the borrower marker, concurrent play.
//!
//! Steam's family library gives an app a second kind of title: a lender's
//! license instead of our own. Three decisions depend on telling those apart,
//! and all three live here as pure functions, so the hooks layer only executes
//! them (the same shape as [`crate::depotcache`]):
//!
//! * [`provenance`] — where an app's ownership really came from, and the label
//!   the library may show for it;
//! * [`marker`] — the borrower marker stamped while a lender covers an app, and
//!   removing it once the app is no longer borrowed;
//! * [`concurrent`] — whether a family-locked app may start while the lender is
//!   playing.
//!
//! The Windows fork we are learning from needed three fixes here: a fake
//! ingested app was labelled "from family library", the borrower marker
//! outlived the license, and concurrent play with a lender stayed blocked.
//!
//! Invariants, each enforced by a test in the module named after it:
//!
//! 1. An app whose only title is our own pkg0 injection is never reported as
//!    family-shared — [`provenance`].
//! 2. An app genuinely covered by a lender's license stays family-shared, and
//!    unlocking concurrent play never rewrites that label — [`provenance`].
//! 3. Removing a borrower marker is idempotent, is a no-op that says so when
//!    the marker is absent, and never re-adds an app to the library —
//!    [`marker`].
//! 4. Every decision is a pure function of its inputs: same inputs, same
//!    output, no order dependence, no panics on malformed input —
//!    [`provenance`], [`marker`], [`concurrent`].

pub mod concurrent;
pub mod marker;
pub mod provenance;

/// Family-share knobs that `[apps.shared]` does not carry.
///
/// These are a value, not a global: the host builds one from its config and
/// passes it to the decisions. The config workstream does not own these fields
/// yet, so the `with_*` setters are the documented seam — *wired by the host
/// config later*. Both default to on, which is the behaviour the Windows host
/// is aiming for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FamilySettings {
    /// Re-validate a claimed lender before trusting a family label. Without
    /// this, a license we fabricated ourselves reads back as borrowed and the
    /// library says "from family library" (invariant 1).
    pub validate_lender: bool,
    /// Remove a borrower marker once the app is no longer borrowed (invariant 3).
    /// Off leaves the marker in place, which is the pre-fix behaviour.
    pub clean_borrower_markers: bool,
}

impl Default for FamilySettings {
    fn default() -> Self {
        Self::fixed()
    }
}

impl FamilySettings {
    /// The fixed behaviour, as a constant so tables can live in `const` arrays.
    pub const fn fixed() -> Self {
        Self {
            validate_lender: true,
            clean_borrower_markers: true,
        }
    }

    /// Wired by the host config later (`[apps.shared] validate_lender`).
    #[must_use]
    pub const fn with_lender_validation(mut self, on: bool) -> Self {
        self.validate_lender = on;
        self
    }

    /// Wired by the host config later (`[apps.shared] clean_borrower_markers`).
    #[must_use]
    pub const fn with_borrower_marker_cleanup(mut self, on: bool) -> Self {
        self.clean_borrower_markers = on;
        self
    }

    /// The pre-fix behaviour, for the "flag off" side of every test table.
    pub const fn legacy() -> Self {
        Self {
            validate_lender: false,
            clean_borrower_markers: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_fixes_default_on_and_setters_are_independent() {
        let default = FamilySettings::default();
        assert!(default.validate_lender);
        assert!(default.clean_borrower_markers);

        let only_marker = default.with_lender_validation(false);
        assert!(!only_marker.validate_lender);
        assert!(only_marker.clean_borrower_markers);

        let only_lender = default.with_borrower_marker_cleanup(false);
        assert!(only_lender.validate_lender);
        assert!(!only_lender.clean_borrower_markers);

        assert!(!FamilySettings::legacy().validate_lender);
        assert!(!FamilySettings::legacy().clean_borrower_markers);
    }
}
