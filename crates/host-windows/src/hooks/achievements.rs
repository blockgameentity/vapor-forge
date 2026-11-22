//! The stats-job merge site — **observation only, and labelled as such**.
//!
//! `CAPIJobRequestUserStats::YieldingMergeStats(this, EResult)` is the completion
//! point of the Windows stats chain: the job's run method
//! ([`crate::abi::achievements::BINIT_SYMBOL`]) sends the request, calls the
//! response parser and then calls this function, which merges the parsed reply
//! into the client's per-app stats store. Its own log string names it
//! (`… YieldingMergeStats - beginning merge. . .`), and it is anchored in both
//! live builds.
//!
//! **This body does not rebuild anything.** [`crate::abi::achievements`] measures
//! why the site cannot carry the local achievement rebuild: by the time the merge
//! runs, the reply is no longer bytes but a parsed stats block inside the job
//! object, and the only code that writes that block is inlined into the parser and
//! the merge themselves. The reply *body* is substituted one stage earlier, in the
//! response parser, and that site is not in the corpus. What is left at this site
//! is the `EResult` — the one argument whose meaning is *Verified* — so the body
//! does exactly three things and nothing else:
//!
//! 1. it hands the call to the displaced original with every register slot
//!    forwarded **verbatim**, so Steam's merge runs unchanged;
//! 2. it reads the `EResult` it was handed and says what it means for the merge
//!    ([`MergeOutcome`], measured from the function's own dispatch);
//! 3. it consults the real decision ([`crate::features::decide_rebuild`]) with a
//!    request that names no app, and logs the answer at debug.
//!
//! `this` is forwarded and **never dereferenced**: the per-build job-object
//! offsets that would name the app id are in [`crate::abi::achievements`], and a
//! body on Steam's thread that reads a stale offset would be worse than a body
//! that reports less. Nothing is mutated, nothing is logged above debug, and no
//! state is written, so the flag being off and the capability being off are both
//! "Steam's behaviour, unchanged".
//!
//! # Decision table
//!
//! | capability | `[achievements] local_rebuild` | switch applied | plan reported | body does |
//! | --- | --- | --- | --- | --- |
//! | disabled | any | any | [`RebuildPlan::CapabilityDisabled`] | forward, log reason |
//! | enabled | off | any | [`RebuildPlan::FlagOff`] | forward, log |
//! | enabled | on | off | [`RebuildPlan::SwitchOff`] | forward, log |
//! | enabled | on | on | [`RebuildPlan::NoApp`] | forward, log |
//! | enabled | on | on, and a request named an app | `NoLocalDefinitions`/`Rebuild` | **unreachable here**: the body passes no app id |
//!
//! Every row forwards the original call with the same arguments; the plan only
//! decides what the debug line says.

use core::ffi::c_void;

use tracing::{debug, error};

use crate::abi::achievements::{MergeStatsFn, MERGE_EARLY_RETURN, MERGE_SYMBOL};
use crate::capability::Capability;
use crate::features::achievements::{decide_rebuild, RebuildPlan, RebuildRequest};
use crate::hooks::state::HostState;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = MERGE_SYMBOL;

/// Address of [`merge_stats`], for the installer's replacement table.
pub fn address() -> usize {
    merge_stats as MergeStatsFn as usize
}

/// What the observed `EResult` means for the merge that is about to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeOutcome {
    /// The original merges the parsed reply into the client's store.
    Merges,
    /// The original returns at its dispatch, leaving the store alone.
    Refused,
}

impl MergeOutcome {
    /// Classify `eresult` exactly as the anchored entry does
    /// ([`MERGE_EARLY_RETURN`], measured from that entry's own dispatch).
    pub fn of(eresult: i32) -> Self {
        if MERGE_EARLY_RETURN.contains(&eresult) {
            Self::Refused
        } else {
            Self::Merges
        }
    }

    /// One word for the log line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Merges => "merges",
            Self::Refused => "refused",
        }
    }
}

/// One look at the completion of a stats job. Pure data: nothing here is mutated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    /// The `EResult` the merge was handed, forwarded verbatim to the original.
    pub eresult: i32,
    /// What that value does to the merge.
    pub outcome: MergeOutcome,
    /// What the local-rebuild decision answers for this site.
    pub plan: RebuildPlan,
}

impl Observation {
    /// Whether the host would replace Steam's payload here. Always `false`: this
    /// body is an observation, and [`crate::abi::achievements`] measures why the
    /// site cannot serve a rebuilt set.
    pub fn rebuilds(&self) -> bool {
        self.plan.rebuilds()
    }
}

/// Replacement for `YieldingMergeStats`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry, so `this` and the
/// three argument slots must be exactly what Steam passed. `this` is never
/// dereferenced here: the original reads it.
pub unsafe extern "system" fn merge_stats(this: *mut c_void, eresult: i32, r8: usize, r9: usize) {
    // SAFETY: the signature is the one documented in `crate::abi::achievements`,
    // and the trampoline was published for this symbol before the entry was
    // patched.
    let Some(forward) = (unsafe { original::original_of::<MergeStatsFn>(SYMBOL) }) else {
        // Nothing can merge without the displaced instructions, and a merge that
        // silently did not happen is worse than a loud line: report and return.
        error!(
            symbol = SYMBOL,
            eresult, "achievements hook reached without a published original"
        );
        return;
    };
    if let Some(state) = state::get() {
        let observation = decide(state, eresult);
        debug!(
            eresult = observation.eresult,
            merge = observation.outcome.label(),
            plan = observation.plan.summary().as_str(),
            rebuilds = observation.rebuilds(),
            "achievements: stats merge observed; the reply is Steam's, unchanged"
        );
    }
    // SAFETY: every slot is forwarded verbatim, exactly as documented.
    unsafe { forward(this, eresult, r8, r9) }
}

/// Report what this merge means for the local rebuild, without changing anything.
///
/// Pure: the capability's gate and the resolved `[achievements] local_rebuild`
/// flag both come from the published [`HostState`], and the decision is the
/// feature's own entry point. The request names no app because the body does not
/// read the job object, which is what makes the missing input explicit rather
/// than guessed.
pub fn decide(state: &HostState, eresult: i32) -> Observation {
    let plan = decide_rebuild(
        state.disabled_reason(Capability::Achievements).as_deref(),
        state.achievement_settings(),
        &RebuildRequest::default(),
    );
    Observation {
        eresult,
        outcome: MergeOutcome::of(eresult),
        plan,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::features::achievements::AchievementSettings;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_features::achievements::set_local_rebuild;

    /// The rebuild switch is process-wide and other tests in this binary write it,
    /// so every test here holds the crate-wide lock and restores it afterwards.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        crate::test_lock::serial()
    }

    fn on() -> AchievementSettings {
        AchievementSettings {
            local_rebuild: true,
        }
    }

    #[test]
    fn the_dispatch_values_are_the_ones_the_function_returns_on() {
        // Measured from both builds' entry; the other 32-bit values merge.
        for refused in [2, 3, 16] {
            assert_eq!(MergeOutcome::of(refused), MergeOutcome::Refused);
        }
        for merged in [0, 1, 4, 5, 7, 8, 12, 15, 17, 20, i32::MAX, i32::MIN] {
            assert_eq!(MergeOutcome::of(merged), MergeOutcome::Merges);
        }
        assert_eq!(MergeOutcome::Refused.label(), "refused");
    }

    #[test]
    fn flag_off_is_a_pure_observation() {
        let _guard = serial();
        set_local_rebuild(false);
        let state = state_with(config_with(&[480]), Vec::new());
        let observation = decide(&state, 1);
        assert_eq!(observation.plan, RebuildPlan::FlagOff);
        assert!(!observation.rebuilds());
        assert_eq!(observation.outcome, MergeOutcome::Merges);
        assert!(observation.plan.summary().contains("off"));
    }

    #[test]
    fn a_setting_that_was_never_applied_is_reported_rather_than_served() {
        let _guard = serial();
        set_local_rebuild(false);
        let state = state_with(config_with(&[480]), Vec::new()).with_achievement_settings(on());
        let observation = decide(&state, 2);
        assert_eq!(observation.plan, RebuildPlan::SwitchOff);
        assert!(!observation.rebuilds());
        assert_eq!(observation.outcome, MergeOutcome::Refused);
    }

    #[test]
    fn an_enabled_flag_still_names_the_missing_input_instead_of_rebuilding() {
        let _guard = serial();
        set_local_rebuild(true);
        let state = state_with(config_with(&[480]), Vec::new()).with_achievement_settings(on());
        let observation = decide(&state, 1);
        // The body reads no job object, so the request it consults carries no app
        // id and no definitions: the decision says so instead of inventing one.
        assert_eq!(observation.plan, RebuildPlan::NoApp);
        assert!(
            !observation.rebuilds(),
            "the site cannot serve a rebuilt set"
        );
        set_local_rebuild(false);
    }

    #[test]
    fn a_disabled_capability_wins_and_reports_the_install_reason() {
        let _guard = serial();
        set_local_rebuild(true);
        let state = state_with(config_with(&[480]), Vec::new()).with_achievement_settings(on());
        let mut gate = state.gate();
        gate.disable(Capability::Achievements, "the merge anchor did not resolve");
        state.set_gate(gate);
        let observation = decide(&state, 1);
        assert_eq!(
            observation.plan,
            RebuildPlan::CapabilityDisabled("the merge anchor did not resolve".to_owned())
        );
        assert!(!observation.rebuilds());
        set_local_rebuild(false);
    }

    #[test]
    fn the_body_without_a_published_original_returns_instead_of_calling_out() {
        // No trampoline is published in this test, so the body fails closed before
        // it reads any state or touches `this`. The lock keeps the forwarding test
        // below from publishing one underneath this call.
        let _guard = serial();
        crate::hooks::original::forget(SYMBOL);
        // SAFETY: called with a null receiver and no live Steam frame; every path
        // this test can take returns before touching memory.
        unsafe { merge_stats(std::ptr::null_mut(), 1, 0, 0) };
    }

    /// What the fake trampoline was called with.
    static FORWARDED: std::sync::Mutex<Option<(i32, usize, usize)>> = std::sync::Mutex::new(None);

    unsafe extern "system" fn fake_merge(_this: *mut c_void, eresult: i32, r8: usize, r9: usize) {
        *FORWARDED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((eresult, r8, r9));
    }

    #[test]
    fn the_body_forwards_every_slot_verbatim() {
        // The load-bearing property of this body: whatever Steam passed reaches the
        // displaced original unchanged, including the two slots whose meaning is
        // only Assumed. Nothing here may filter, rewrite or drop a slot.
        let _guard = serial();
        crate::hooks::original::record(SYMBOL, fake_merge as MergeStatsFn as usize);
        // SAFETY: the published "trampoline" is `fake_merge`, whose signature is
        // exactly `MergeStatsFn`'s; nothing dereferences `this`.
        unsafe { merge_stats(0x1000 as *mut c_void, 5, 0x11, 0x22) };
        let forwarded = (*FORWARDED
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()))
        .take();
        crate::hooks::original::forget(SYMBOL);
        assert_eq!(forwarded, Some((5, 0x11, 0x22)));
    }
}
