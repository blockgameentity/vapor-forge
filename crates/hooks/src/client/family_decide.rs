//! The one place the Linux hooks ask `features::family` for a decision.
//!
//! Before this module the Linux host answered the family questions itself:
//! `family_lock` had its own concurrent-play answer, the license walk labelled
//! apps without asking `family::provenance`, and nothing on this host ever named
//! a lender or ran the borrower-marker pass. The Windows host asks
//! `features::family` for all three, so the answers were free to drift.
//!
//! Here the hooks layer only *executes* the decisions:
//!
//! * [`concurrent_play`] wraps [`vapor_forge_features::family::concurrent::decide`];
//! * [`label`] wraps `family::provenance::{classify, reported}` (via
//!   `features::apps::provenance`, which owns the recorded account state);
//! * [`lender`], [`note_lender`] and [`note_unborrowed`] wrap
//!   `family::provenance::lender_account`, `family::marker::BorrowerMarkers` and
//!   `family::marker::remove_if_unborrowed`.
//!
//! One resolution path for the settings: [`settings`] reads
//! `features::config_apply::family_settings()`, the value `config_apply::apply`
//! publishes from `[apps.shared] validate_lender` / `clean_borrower_markers` on
//! every config load and hot reload. The Windows host builds the same value from
//! its own config handle, so both hosts decide from one resolved struct.
//!
//! Behaviour, both directions:
//!
//! * [`concurrent_play`] reproduces today's Linux answer for every input: the
//!   allow set is exactly `is_controlled_app || should_bypass_sharing` (what
//!   `features::valve_filter::bypasses_family_lock` said), and the reason is now
//!   recorded instead of being re-derived by hand.
//! * [`label`] is a computation, not a write: it never changes what ships. The
//!   only write it can authorise is [`clears_recorded_share`], which is gated by
//!   `validate_lender` — with that flag off (the pre-fix behaviour) nothing is
//!   cleared just because provenance says so.
use vapor_forge_config::{AppId, RuntimeConfig};
use vapor_forge_features::apps;
use vapor_forge_features::config_apply;
use vapor_forge_features::family::concurrent::{self, ConcurrentPlayDecision, FamilyLock};
use vapor_forge_features::family::marker::MarkerOutcome;
use vapor_forge_features::family::provenance::{
    self, Provenance, ProvenanceReport, ReportedProvenance,
};
use vapor_forge_features::family::FamilySettings;
use vapor_forge_features::package::PackageState;

/// Family settings in force, as `config_apply` resolved them.
pub(crate) fn settings() -> FamilySettings {
    config_apply::family_settings()
}

/// The current account's id, or 0 while no identity is known.
///
/// A SteamID64 carries the account id in its low 32 bits, which is the same
/// number the license packet names as a package's owner.
pub(crate) fn self_account_id() -> u32 {
    (vapor_forge_features::identity::steam_id() & u64::from(u32::MAX)) as u32
}

/// Whether our own pkg0 injection is this app's only title.
pub(crate) fn injected_by_us(package: &PackageState, app_id: AppId) -> bool {
    package.is_injected_into_pkg0(app_id)
}

/// Provenance and the label the library may show, from the recorded state.
pub(crate) struct AppFamilyLabel {
    pub real: Provenance,
    pub report: ProvenanceReport,
}

/// Decide `app_id`'s real provenance and the label derived from it.
pub(crate) fn label(
    config: &RuntimeConfig,
    app_id: AppId,
    injected_by_us: bool,
    settings: FamilySettings,
) -> AppFamilyLabel {
    let real = apps::provenance(app_id, injected_by_us, self_account_id(), settings);
    AppFamilyLabel {
        real,
        report: provenance::reported(real, config, app_id),
    }
}

/// Whether the recorded family-share bit must be cleared for an app whose label
/// says it is ours.
///
/// Gated by `validate_lender`, the flag that decides whether a claim without a
/// nameable lender is trusted. With it off this returns `false` and the caller
/// keeps today's clearing rule (only `features::apps`' `should_bypass_sharing`
/// answer), which is the pre-fix behaviour.
pub(crate) fn clears_recorded_share(
    record_claims_share: bool,
    report: &ProvenanceReport,
    settings: FamilySettings,
) -> bool {
    settings.validate_lender && record_claims_share && report.reported == ReportedProvenance::Owned
}

/// Decide whether `app_id` may start while `locking_account` holds it.
///
/// `locking_account` is Steam's own answer from `GetLockingAccount`, so 0 means
/// there was no lock to unlock. The decision is logged once, with its reason and
/// the provenance it was made from.
pub(crate) fn concurrent_play(
    config: &RuntimeConfig,
    app_id: AppId,
    locking_account: u32,
    real: Provenance,
) -> ConcurrentPlayDecision {
    let lock = if locking_account == 0 {
        FamilyLock::free()
    } else {
        FamilyLock::held_by(Some(locking_account))
    };
    let decision = concurrent::decide(config, app_id, lock);
    decision.log(app_id, lock, real);
    decision
}

/// The lender whose license covers `app_id`, when one can be named.
pub(crate) fn lender(app_id: AppId, self_account_id: u32) -> Option<u32> {
    provenance::lender_account(apps::family_owner(app_id), self_account_id)
}

/// A lender from a license the walk just read, when it names one.
pub(crate) fn lender_from_owner(owner: u32, self_account_id: u32) -> Option<u32> {
    provenance::lender_account(Some(owner), self_account_id)
}

/// Record the lender a license names and stamp the borrower marker.
///
/// This is the one site that can see a license owner, so it is the one site that
/// writes `features::apps`' owner id and the package layer's marker. Returns the
/// lender it stamped, `None` when the claim names no lender we may trust.
pub(crate) fn note_lender(
    package: &PackageState,
    app_id: AppId,
    owner: u32,
    self_account_id: u32,
) -> Option<u32> {
    let lender = lender_from_owner(owner, self_account_id)?;
    apps::record_family_owner(app_id, lender);
    package.stamp_borrower_marker(app_id, lender);
    Some(lender)
}

/// Drop the borrower marker once the app is no longer borrowed.
///
/// Idempotent and safe to run on every license refresh: with
/// `clean_borrower_markers` off it reports [`MarkerOutcome::Disabled`] and leaves
/// whatever was stamped alone, which is the pre-fix behaviour.
pub(crate) fn note_unborrowed(
    package: &PackageState,
    app_id: AppId,
    injected_by_us: bool,
    settings: FamilySettings,
) -> MarkerOutcome {
    let real = apps::provenance(app_id, injected_by_us, self_account_id(), settings);
    package.remove_borrower_marker(app_id, real, settings)
}
#[cfg(test)]
mod tests;
