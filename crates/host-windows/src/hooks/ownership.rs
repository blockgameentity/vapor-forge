//! `CUser::CheckAppOwnership` — the ownership hook body.
//!
//! The body is the consumer the decision layer was missing: it turns one call
//! into `features::apps`' decision, `features::family`'s provenance and
//! `features::package`'s borrower marker, in that order, and then writes the
//! answer back through Steam's own record.
//!
//! Flags, all of which are a pure fall-through when they say no:
//!
//! * the `ownership` capability survived the install pass;
//! * the app is controlled by the effective config (`[apps] inject`, script
//!   `addappid`) — an app we do not control is never touched;
//! * the caller passed a non-null out record.
//!
//! The invariant that matters most is checked here rather than trusted: an app
//! whose only title is our own injection must never read back as "from family
//! library". `features::family::provenance::reported` decides the label and, when
//! it says `Owned` while Steam's record still claims a family share, the family
//! bit is cleared before the record is handed back.
//!
//! The decision is a pure function of `(HostState, Steam's answer, record)`, so
//! it is unit-tested without a live client; the FFI body only forwards arguments
//! and calls it.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_features::apps::{self, OwnershipObservation};
use vapor_forge_features::family::provenance::{lender_account, reported, ReportedProvenance};

use crate::abi::ownership::CAppOwnershipInfo;
use crate::abi::ownership::CheckAppOwnershipFn;
use crate::abi::ret;
use crate::capability::Capability;
use crate::hooks::family_play;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CUser::CheckAppOwnership";

/// Address of [`check_app_ownership`], for the installer's replacement table.
pub fn address() -> usize {
    check_app_ownership as CheckAppOwnershipFn as usize
}

/// Replacement for `CUser::CheckAppOwnership`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`
/// and `out` must be whatever Steam passed. `out` is written only when Steam
/// passed a non-null record, and only with the layout documented in
/// [`crate::abi::ownership`].
pub unsafe extern "system" fn check_app_ownership(
    this: *mut c_void,
    app_id: u32,
    out: *mut CAppOwnershipInfo,
    r9: usize,
) -> u8 {
    // SAFETY: the signature is the one documented for this symbol, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<CheckAppOwnershipFn>(SYMBOL) }) else {
        debug!(
            app_id,
            "ownership hook reached without a published original; reporting unowned"
        );
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim.
    let steam = unsafe { original(this, app_id, out, r9) };
    if out.is_null() {
        return steam;
    }
    let Some(state) = state::get() else {
        return steam;
    };
    // SAFETY: `out` is non-null and, per the signature this hook stands in for,
    // points at the record Steam itself just filled.
    let record = unsafe { &mut *out };
    decide(state, app_id, steam, record).map_or(steam, ret)
}

/// Apply the host's decision to Steam's ownership record.
///
/// `None` means "nothing of ours applies": the caller must return Steam's answer
/// and leave the record untouched. `Some(result)` is the answer to return.
pub fn decide(
    state: &HostState,
    app_id: u32,
    steam: u8,
    record: &mut CAppOwnershipInfo,
) -> Option<bool> {
    if !state.allows(Capability::Ownership) {
        return None;
    }
    let caller = AppId(app_id);
    let config = state.config();
    if !config.is_controlled_app(caller) {
        return None;
    }

    let observation = OwnershipObservation {
        original_result: steam != 0,
        owns_license: record.owns_license(),
        family_shared: record.is_family_shared(),
    };
    // Recorded before anything is rewritten: pkg0 injection is not wired on this
    // host yet, so every observed answer is a pre-injection one.
    apps::record_original_observation(caller, observation);

    let injected_by_us = state.package().is_injected_into_pkg0(caller);
    let settings = state.family_settings();
    let self_account = state.self_account_id();
    let decision = apps::decide_check_ownership(config, caller, observation);
    let real = apps::provenance(caller, injected_by_us, self_account, settings);
    let label = reported(real, config, caller);

    // Concurrent play: the record is what says a lender holds the app, and
    // `features::family::concurrent` is the one owner of the answer. The share bit
    // is still Steam's at this point - the policy below is what clears it - so the
    // consult sees exactly what Steam reported. The decision logs its own reason.
    let lock = family_play::lock(record.is_family_shared(), apps::family_owner(caller));
    let concurrent = family_play::consult(state, caller, lock);

    // A lender we can name is the only thing that may stamp a borrower marker;
    // an unverifiable family claim is decided by `provenance`, not by this body.
    if let Some(lender) = lender_account(apps::family_owner(caller), self_account) {
        state.package().stamp_borrower_marker(caller, lender);
    }

    if decision.grant_spoofed_ownership {
        record.grant_spoofed_ownership();
    }
    if decision.clear_family_shared
        || (record.is_family_shared() && label.reported == ReportedProvenance::Owned)
    {
        record.clear_family_shared();
    }

    debug!(
        app_id,
        provenance = real.label(),
        reported = label.reported.label(),
        reason = label.reason,
        lock_unlocked = label.lock_unlocked,
        granted = decision.grant_spoofed_ownership,
        concurrent = concurrent.map_or("-", |concurrent| concurrent.reason.label()),
        "ownership decision applied"
    );
    Some(decision.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_features::family::provenance::Provenance;

    fn controlled(app: u32) -> HostState {
        state_with(config_with(&[app]), Vec::new())
    }

    #[test]
    fn an_uncontrolled_app_gets_no_decision_at_all() {
        let state = controlled(730);
        let mut record = CAppOwnershipInfo {
            family_shared: 1,
            ..CAppOwnershipInfo::default()
        };
        assert_eq!(decide(&state, 99_001, 0, &mut record), None);
        assert_eq!(record.family_shared, 1, "the record stays Steam's");
    }

    #[test]
    fn a_controlled_app_is_granted_ownership_once_the_license_sync_is_done() {
        let state = controlled(730);
        apps::mark_license_sync_complete();
        let mut record = CAppOwnershipInfo::default();
        let decision = decide(&state, 730, 0, &mut record).expect("a decision applies");
        assert!(
            decision,
            "the app is controlled, so Steam is told it is owned"
        );
        assert!(record.owns_license());
        assert!(!record.is_family_shared());
    }

    #[test]
    fn our_own_injection_is_never_left_labelled_family_shared() {
        let app = AppId(731);
        let state = state_with(config_with(&[app.0]), Vec::new());
        apps::record_actual_ownership(app, true);
        apps::record_family_shared(app, true);
        // No lender is recorded, so the provenance fix classifies this as our
        // own injection rather than a borrowed title.
        assert_eq!(
            apps::provenance(app, true, 0, state.family_settings()),
            Provenance::IngestedByUs
        );
        let mut record = CAppOwnershipInfo {
            family_shared: 1,
            ..CAppOwnershipInfo::default()
        };
        decide(&state, app.0, 1, &mut record).expect("a decision applies");
        assert!(
            !record.is_family_shared(),
            "an ingested title never reads as borrowed"
        );
        apps::record_family_shared(app, false);
        apps::record_actual_ownership(app, false);
    }

    #[test]
    fn a_genuine_lender_keeps_its_label() {
        let app = AppId(732);
        let state = state_with(config_with(&[app.0]), Vec::new());
        // A recorded sample is what gets `provenance` past its "nothing known
        // yet" short circuit; without it the honest answer is `Unknown`.
        apps::record_actual_ownership(app, false);
        apps::record_family_shared(app, true);
        apps::record_family_owner(app, 42);
        // Steam reports a genuine license and a family share.
        let mut record = CAppOwnershipInfo {
            family_shared: 1,
            owns_license: 1,
            ..CAppOwnershipInfo::default()
        };
        let result = decide(&state, app.0, 1, &mut record).expect("a decision applies");
        assert!(result, "a lender's license is a real entitlement");
        assert_eq!(
            apps::provenance(app, false, 0, state.family_settings()),
            Provenance::FamilyShared,
            "a named lender keeps its provenance"
        );
        assert_eq!(
            state.package().borrower_marker(app),
            Some(42),
            "the named lender is stamped so the package pass can clean it up"
        );
        assert!(
            !record.is_family_shared(),
            "the default [apps.shared] policy unlocks the family lock, which is what \
             clears the record's share bit - the label itself stays family-shared"
        );
        apps::record_family_shared(app, false);
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = controlled(730);
        let mut gate = state.gate();
        gate.disable(Capability::Ownership, "symbol did not resolve");
        state.set_gate(gate);
        let mut record = CAppOwnershipInfo::default();
        assert_eq!(decide(&state, 730, 0, &mut record), None);
        assert!(!record.owns_license());
    }
}
