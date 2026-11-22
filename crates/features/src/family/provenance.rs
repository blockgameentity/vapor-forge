//! Where an app's ownership came from, and what the library may claim about it.
//!
//! Two values, deliberately separate:
//!
//! * [`Provenance`] is the *fact* — our license, a lender's license, our own
//!   injection, or nothing at all;
//! * [`ReportedProvenance`] is the *claim* the library is allowed to make from
//!   it.
//!
//! Keeping them apart is the whole point: `[apps.shared]` may unlock concurrent
//! play for a borrowed app, but it must never change what the library says the
//! app is. A label is factual, a lock is policy.

use crate::family::FamilySettings;
use vapor_forge_config::{AppId, RuntimeConfig};

/// Account id Steam uses for "a license that maps to no account".
///
/// `valve_filter` reports this id for apps whose family lock must not apply.
/// Re-used here for the same reason: a value *we* fabricate can never pass for
/// a lender, so our own injection cannot manufacture a family label.
pub const NO_ACCOUNT: u32 = crate::valve_filter::FAMILY_LOCK_FREE_OWNER_ID;

/// Where an app's ownership actually came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Provenance {
    /// The license walk has not reported this app yet.
    Unknown,
    /// Genuine entitlement held by this account.
    Owned,
    /// Covered only by another family member's license.
    FamilyShared,
    /// Our own pkg0 injection is the app's only title.
    IngestedByUs,
    /// No title at all: not in this account's library.
    Unowned,
}

impl Provenance {
    /// Every variant, for exhaustive decision tables in tests and reports.
    pub const ALL: [Self; 5] = [
        Self::Unknown,
        Self::Owned,
        Self::FamilyShared,
        Self::IngestedByUs,
        Self::Unowned,
    ];

    /// Whether a lender's license covers the app.
    pub const fn is_borrowed(self) -> bool {
        matches!(self, Self::FamilyShared)
    }

    /// Stable label for logs and the diagnostics bundle.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Owned => "owned",
            Self::FamilyShared => "family-shared",
            Self::IngestedByUs => "ingested-by-us",
            Self::Unowned => "unowned",
        }
    }
}

/// Ownership signals exactly as the license/package snapshot reported them,
/// before any policy runs.
///
/// `license_owner` is the account the covering license is attributed to, or
/// `None` when the snapshot did not name one. That distinction matters: a
/// missing owner is missing evidence, an owner that is zero, us, or
/// [`NO_ACCOUNT`] is evidence the claim is not a lender.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnershipSignals {
    /// This account holds the license itself.
    pub self_owned: bool,
    /// The snapshot says another account's license covers the app.
    pub borrowed: bool,
    /// Account the covering license is attributed to, when known.
    pub license_owner: Option<u32>,
    /// We wrote this app into pkg0 ourselves.
    pub injected_by_us: bool,
}

/// A lender we can name: not zero, not us, not the fabricated sentinel.
pub fn lender_account(license_owner: Option<u32>, self_account_id: u32) -> Option<u32> {
    let owner = license_owner?;
    if owner == 0 || owner == NO_ACCOUNT || (self_account_id != 0 && owner == self_account_id) {
        return None;
    }
    Some(owner)
}

/// Where ownership really came from.
///
/// Precedence, highest first: our own license, a lender's license, our
/// injection, nothing.
///
/// A *verified* lender outranks our injection on purpose. When a member really
/// does lend the app, that label is factual even if we also control the app,
/// and dropping it would be the overreach invariant 2 forbids.
///
/// With [`FamilySettings::validate_lender`] off the claimed family flag is
/// trusted verbatim. That is the pre-fix behaviour: an app whose license we
/// fabricated came back looking borrowed, so it was labelled "from family
/// library" (invariant 1). Turning the fix on accepts a claim with no named
/// owner (the snapshot simply did not say) unless we injected the app — a
/// claim we cannot check is not a claim we may use for our own injection.
pub fn classify(
    signals: OwnershipSignals,
    self_account_id: u32,
    settings: FamilySettings,
) -> Provenance {
    if signals.self_owned {
        return Provenance::Owned;
    }
    if signals.borrowed {
        let verified = lender_account(signals.license_owner, self_account_id).is_some();
        let unnamed = signals.license_owner.is_none();
        if !settings.validate_lender || verified || (unnamed && !signals.injected_by_us) {
            return Provenance::FamilyShared;
        }
    }
    if signals.injected_by_us {
        return Provenance::IngestedByUs;
    }
    Provenance::Unowned
}

/// What the library is allowed to say about an app.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReportedProvenance {
    /// Leave Steam's own label alone: we have no sample for this app.
    Unchanged,
    /// Present as this account's own title.
    Owned,
    /// Present as "from family library".
    FamilyShared,
    /// Do not present the app at all.
    NotInLibrary,
}

impl ReportedProvenance {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "unchanged",
            Self::Owned => "owned",
            Self::FamilyShared => "family-shared",
            Self::NotInLibrary => "not-in-library",
        }
    }
}

/// The label for one app, plus everything the caller needs to log it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProvenanceReport {
    /// What the library must say.
    pub reported: ReportedProvenance,
    /// Whether `[apps.shared]` unlocks the family lock for this app. Recorded
    /// for the caller's log only: the label never depends on it (invariant 2).
    pub lock_unlocked: bool,
    /// Why the label came out this way.
    pub reason: &'static str,
}

/// Decide the reported provenance for `app_id` from its real provenance and
/// our configuration.
///
/// The `[apps.shared]` policy is consulted only to report whether the lock is
/// unlocked. It cannot turn a family-shared app into an owned one, and it
/// cannot turn our own injection into a family-shared one.
pub fn reported(real: Provenance, config: &RuntimeConfig, app_id: AppId) -> ProvenanceReport {
    let lock_unlocked = config.should_bypass_sharing(app_id);
    let (reported, reason) = match real {
        Provenance::Unknown => (ReportedProvenance::Unchanged, "no ownership sample yet"),
        Provenance::Owned => (
            ReportedProvenance::Owned,
            "genuine entitlement on this account",
        ),
        Provenance::FamilyShared => (
            ReportedProvenance::FamilyShared,
            "covered by a lender's license",
        ),
        Provenance::IngestedByUs => (
            ReportedProvenance::Owned,
            "our own injection is the only title",
        ),
        Provenance::Unowned => (ReportedProvenance::NotInLibrary, "no title on this account"),
    };

    if real == Provenance::IngestedByUs && !config.is_controlled_app(app_id) {
        // The snapshot still claims our injection, but the effective config no
        // longer controls the app. Keep presenting it (hiding a library row is
        // the more destructive choice) and make the inconsistency visible.
        tracing::warn!(
            app_id = app_id.0,
            "family: ingested provenance for an app the config does not control"
        );
    }
    if real.is_borrowed() && lock_unlocked {
        tracing::debug!(
            app_id = app_id.0,
            "family: family lock unlocked, label stays family-shared"
        );
    }

    ProvenanceReport {
        reported,
        lock_unlocked,
        reason,
    }
}

#[cfg(test)]
mod tests;
