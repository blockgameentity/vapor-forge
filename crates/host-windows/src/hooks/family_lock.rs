//! The Windows family shared-library lock — the hook body for
//! `CUser::GetFamilySharedLockAccount`.
//!
//! What the body does, in one sentence: when `features::family::concurrent`
//! allows the app to start, Steam is told that no account holds the app, so the
//! play-info flag and the launch result that Steam derives from this answer stop
//! refusing the launch; in every other case Steam's own answer is returned
//! unchanged.
//!
//! The symbol is not Linux's `CFamilySharing::GetLockingAccount`. That class does
//! not exist on Windows (see `res/patterns/win64/VTABLE-REPORT.md` section 11).
//! Windows ships family *groups* instead, and the client-side lock decision lives
//! in `CUser`: a per-app record recomputed by `CUser::UpdateFamilyRunningApps`
//! (which logs `Shared lock for app %u changed : available owner %u, preferred
//! owner %u, current player %u`) and queried by this function, which returns the
//! account holding the app — `0` when the app is free, or when the holder is the
//! local user. The eighth pass of that report derives the meaning from the two call
//! sites: `CUserAppManager` vtable slot 71 sets bit `0x100` of the play-info
//! flags from a non-zero answer, and the launch-decision function answers its
//! `0x21` result from it.
//!
//! Three rules keep this honest:
//!
//! * the capability gate is applied by [`family_play::consult`], never here and
//!   never in `features::family`. Disabling `family-lock-bypass` returns Steam's
//!   answer for every app;
//! * the body reads the original first, so the log line, the lender and the
//!   decision are all about the answer Steam actually produced. Nothing is
//!   re-derived from state this host would have to keep in sync;
//! * it writes nothing. The anchored function takes no out-parameter (see
//!   [`crate::abi::family_lock`]), so the only thing this body can change is the
//!   returned account id.
//!
//! What it does not cover: `CUser::UpdateFamilyRunningApps` still records the
//! lock when Steam tells it about a member starting a game, and any consumer
//! that reads that record directly instead of calling this query keeps its own
//! answer. The report records which consumers those are.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;

use crate::abi::family_lock::GetFamilySharedLockAccountFn;
use crate::hooks::state::HostState;
use crate::hooks::{family_play, original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CUser::GetFamilySharedLockAccount";

/// Address of [`get_family_shared_lock_account`], for the installer's
/// replacement table.
pub fn address() -> usize {
    get_family_shared_lock_account as GetFamilySharedLockAccountFn as usize
}

/// Replacement for `GetFamilySharedLockAccount`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this` and
/// `app_id` must be whatever Steam passed, and the trampoline must have been
/// published before the entry patch was enabled (the installer does that).
pub unsafe extern "system" fn get_family_shared_lock_account(
    this: *mut c_void,
    app_id: u32,
) -> u32 {
    // SAFETY: the signature is the one documented in `crate::abi::family_lock`.
    let Some(original) = (unsafe { original::original_of::<GetFamilySharedLockAccountFn>(SYMBOL) })
    else {
        // The entry now jumps here, so there is nothing left to forward to. `0`
        // is the value the original itself returns when no member holds the app,
        // which is the least destructive answer a query with no original can
        // give: it cannot invent a lock.
        debug!("family-lock hook reached without a published original; reporting no holder");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim.
    let steam = unsafe { original(this, app_id) };
    let Some(state) = state::get() else {
        return steam;
    };
    match decide(state, AppId(app_id), steam) {
        LockAnswer::NotLocked => 0,
        LockAnswer::PassThrough => steam,
    }
}

/// What the host wants Steam told about who holds `app_id`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockAnswer {
    /// Steam's own answer stands: whoever holds the app keeps holding it.
    PassThrough,
    /// The decision allows the app to start, so nobody holds it.
    NotLocked,
}

/// Decide the answer for `app_id`, given the account `steam` says holds it.
///
/// `steam` is the original's return value, so `0` means Steam already answered
/// "nobody holds it" and the decision can only agree. The capability gate and the
/// log line both live in [`family_play::consult`]; this function adds no rule of
/// its own beyond turning the decision into the two answers the body has.
pub fn decide(state: &HostState, app_id: AppId, steam: u32) -> LockAnswer {
    // A non-zero answer names a lender; `lock` turns it into the same
    // `FamilyLock` the ownership and package bodies report.
    let lock = family_play::lock(steam != 0, Some(steam));
    match family_play::consult(state, app_id, lock) {
        Some(decision) if decision.allowed => LockAnswer::NotLocked,
        _ => LockAnswer::PassThrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::hooks::tests_support::{config_with, state_with};

    /// A lender's account id, distinct from the local user the fixtures use.
    const LENDER: u32 = 4711;

    #[test]
    fn a_controlled_app_is_reported_as_held_by_nobody() {
        let state = state_with(config_with(&[620]), Vec::new());
        assert_eq!(decide(&state, AppId(620), LENDER), LockAnswer::NotLocked);
    }

    #[test]
    fn a_shared_policy_app_is_reported_as_held_by_nobody_when_the_flag_is_on() {
        let mut config = config_with(&[]);
        config.apps.shared.enabled = true;
        let state = state_with(config, Vec::new());
        assert_eq!(decide(&state, AppId(631), LENDER), LockAnswer::NotLocked);
    }

    #[test]
    fn a_policy_that_excludes_the_app_keeps_the_lenders_answer() {
        let mut config = config_with(&[]);
        config.apps.shared.enabled = true;
        config.apps.shared.include = vec![AppId(1)];
        let state = state_with(config, Vec::new());
        assert_eq!(decide(&state, AppId(632), LENDER), LockAnswer::PassThrough);
    }

    #[test]
    fn with_sharing_off_the_lenders_answer_stands() {
        let mut config = config_with(&[]);
        config.apps.shared.enabled = false;
        let state = state_with(config, Vec::new());
        assert_eq!(decide(&state, AppId(633), LENDER), LockAnswer::PassThrough);
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with(&[620]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::FamilyLockBypass, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(decide(&state, AppId(620), LENDER), LockAnswer::PassThrough);
    }

    #[test]
    fn an_app_nobody_holds_keeps_steams_zero() {
        // `0` from the original means Steam itself said no member holds the app;
        // the body must not turn that into a different answer either way.
        let state = state_with(config_with(&[620]), Vec::new());
        assert_eq!(decide(&state, AppId(620), 0), LockAnswer::NotLocked);
        let mut gate = state.gate();
        gate.disable(Capability::FamilyLockBypass, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(decide(&state, AppId(620), 0), LockAnswer::PassThrough);
    }

    #[test]
    fn the_body_is_registered_under_the_symbol_the_corpus_anchors() {
        // The corpus entry and the installer's table have to agree on one name,
        // otherwise the anchor resolves a symbol no body claims.
        assert_eq!(SYMBOL, "CUser::GetFamilySharedLockAccount");
        assert_ne!(address(), 0);
    }
}
