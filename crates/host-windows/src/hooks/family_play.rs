//! Concurrent play while a lender is playing — the consult the decision lacked.
//!
//! `features::family::concurrent` has answered "may this app start while a lender
//! holds it, and why" since the family model landed, and nothing called it: the
//! Windows host had the decision and no consumer, so a family-shared title stayed
//! blocked even with `[apps.shared]` on. The Linux host consults the same rule
//! from `CFamilySharing::GetLockingAccount`, except it has an entry point to
//! override; this host has no anchored family symbol (the win64 corpus resolves
//! no `CFamilySharing::` entry), so the consult happens where the host already
//! learns the app's family state: the ownership body (which Steam itself tells
//! about a share) and the package body (which refreshes borrower markers).
//!
//! Two rules keep this honest:
//!
//! * the capability gate is applied here, never in `features::family`. Disabling
//!   `family-lock-bypass` must leave Steam's refusal exactly as it was, which is
//!   what [`consult`] returning `None` means;
//! * the decision is only announced when a lender actually holds the app. The
//!   package body runs on every package lookup, so "nothing holds it" is a trace
//!   line, not an info one.

use vapor_forge_config::AppId;
use vapor_forge_features::apps;
use vapor_forge_features::family::concurrent::{self, ConcurrentPlayDecision, FamilyLock};
use vapor_forge_features::family::provenance::Provenance;

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// The family lock for one app, from what the caller observed.
pub fn lock(locked_by_lender: bool, lender: Option<u32>) -> FamilyLock {
    if locked_by_lender {
        FamilyLock::held_by(lender)
    } else {
        FamilyLock::free()
    }
}

/// The lock as the host's own family state reports it for `app_id`.
///
/// Used where no Steam record is in hand (the package body): the recorded share
/// bit and the recorded lender are what `apps` learned from an earlier
/// ownership answer.
pub fn observed_lock(app_id: AppId) -> FamilyLock {
    lock(apps::is_family_shared(app_id), apps::family_owner(app_id))
}

/// Ask `features::family::concurrent` whether `app_id` may start while `lock`
/// holds it, and say so once.
///
/// `None` means "no opinion": the `family-lock-bypass` capability is disabled, so
/// Steam's own refusal stands untouched and no line is logged above trace.
pub fn consult(
    state: &HostState,
    app_id: AppId,
    lock: FamilyLock,
) -> Option<ConcurrentPlayDecision> {
    if !state.allows(Capability::FamilyLockBypass) {
        tracing::trace!(
            app_id = app_id.0,
            "family: concurrent play not consulted; capability disabled"
        );
        return None;
    }
    let decision = concurrent::decide(state.config(), app_id, lock);
    if lock.locked_by_lender {
        // `log` is the decision's own one-line form, so the reason is never
        // re-derived here.
        decision.log(app_id, lock, provenance(state, app_id));
    } else {
        tracing::trace!(
            app_id = app_id.0,
            reason = decision.reason.label(),
            "family: nothing holds the app"
        );
    }
    Some(decision)
}

/// The provenance the decision is logged with.
fn provenance(state: &HostState, app_id: AppId) -> Provenance {
    apps::provenance(
        app_id,
        state.package().is_injected_into_pkg0(app_id),
        state.self_account_id(),
        state.family_settings(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_features::family::concurrent::PlayReason;

    #[test]
    fn nothing_holds_the_app_so_it_may_start() {
        let state = state_with(config_with(&[620]), Vec::new());
        let decision = consult(&state, AppId(620), FamilyLock::free()).expect("consulted");
        assert!(decision.allowed);
        assert_eq!(decision.reason, PlayReason::NoFamilyLock);
    }

    #[test]
    fn a_controlled_app_bypasses_the_lock_because_we_injected_it() {
        let state = state_with(config_with(&[620]), Vec::new());
        let lock = FamilyLock::held_by(Some(4711));
        let decision = consult(&state, AppId(620), lock).expect("consulted");
        assert!(decision.allowed);
        assert_eq!(decision.reason, PlayReason::ControlledApp);
    }

    #[test]
    fn a_shared_policy_app_is_unlocked_when_the_flag_is_on() {
        // Not one of our injected apps: the unlock comes from `[apps.shared]`.
        let mut config = config_with(&[]);
        config.apps.shared.enabled = true;
        let state = state_with(config, Vec::new());
        let lock = FamilyLock::held_by(Some(4711));
        let decision = consult(&state, AppId(621), lock).expect("consulted");
        assert!(decision.allowed);
        assert_eq!(decision.reason, PlayReason::UnlockConfigured);
    }

    #[test]
    fn a_policy_that_excludes_the_app_keeps_steams_refusal() {
        let mut config = config_with(&[]);
        config.apps.shared.enabled = true;
        config.apps.shared.include = vec![AppId(1)];
        let state = state_with(config, Vec::new());
        let lock = FamilyLock::held_by(None);
        let decision = consult(&state, AppId(624), lock).expect("consulted");
        assert!(!decision.allowed);
        assert_eq!(decision.reason, PlayReason::PolicyRefuses);
    }

    #[test]
    fn with_sharing_off_steams_refusal_stands() {
        let mut config = config_with(&[]);
        config.apps.shared.enabled = false;
        let state = state_with(config, Vec::new());
        let lock = FamilyLock::held_by(Some(4711));
        let decision = consult(&state, AppId(622), lock).expect("consulted");
        assert!(!decision.allowed);
        assert_eq!(decision.reason, PlayReason::SharingDisabled);
    }

    #[test]
    fn a_disabled_capability_consults_nothing_at_all() {
        let state = state_with(config_with(&[620]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::FamilyLockBypass, "symbol did not resolve");
        state.set_gate(gate);
        let lock = FamilyLock::held_by(Some(4711));
        assert_eq!(consult(&state, AppId(620), lock), None);
    }

    #[test]
    fn the_observed_lock_follows_the_recorded_share_state() {
        // The observed lock comes from the process-wide family state, so this test
        // publishes no host state at all. App id 623 is not used by any other test.
        assert_eq!(observed_lock(AppId(623)), FamilyLock::free());
        apps::record_family_shared(AppId(623), true);
        apps::record_family_owner(AppId(623), 4711);
        assert_eq!(
            observed_lock(AppId(623)),
            FamilyLock::held_by(Some(4711)),
            "a recorded share with a named lender is a lock"
        );
        apps::record_family_shared(AppId(623), false);
        assert_eq!(observed_lock(AppId(623)), FamilyLock::free());
    }
}
