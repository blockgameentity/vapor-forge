//! `CUser::GetSubscribedApps` — the ownership hook that reports the host's app
//! set.
//!
//! Steam answers "which apps is this user subscribed to" by walking its license
//! list. An app the host controls by injection was never purchased, so Steam's
//! walk does not see it; this body adds those ids to the answer so the rest of the
//! client treats the app as owned. The Linux equivalent is
//! `crates/hooks/src/client/ownership.rs::hk_get_subscribed_apps`, and this body
//! follows it case for case:
//!
//! 1. **sizing call** — `app_list` is null or `size == 0`: Steam only wants a
//!    count, so the answer is Steam's count plus the number of apps this host
//!    would add. An app already in pkg0 is enumerated by Steam's own walk, so the
//!    adjustment is `0` in that case rather than a double count.
//! 2. **buffer too small** — Steam's count exceeds the buffer's slot count:
//!    nothing is written and Steam's count is returned, because there is nowhere
//!    to put an addition.
//! 3. **fill call** — the configured apps that are not already in the list are
//!    appended, every slot past the new count is zero-filled, and the new count is
//!    returned. Steam sizes its receiving vector from the sizing call, so a slot
//!    left untouched would expose stale heap contents as an app id.
//!
//! The decision is [`decide`]: a pure function of the published [`HostState`], the
//! buffer and Steam's count, so the whole table is unit-tested without a live
//! client. The FFI body only builds the slice and logs.
//!
//! One side effect is inherited from `features::apps`: the first fill call latches
//! Steam's license sync as complete, which is what lets deferred ownership spoofing
//! start. That is the feature's decision, not this body's.
//!
//! Fall-through: a missing trampoline or unpublished host state returns Steam's
//! answer and writes nothing.

use core::ffi::c_void;

use tracing::{debug, trace};
use vapor_forge_features::apps;

use crate::abi::subscribed::GetSubscribedAppsFn;
use crate::capability::Capability;
use crate::hooks::state::HostState;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CUser::GetSubscribedApps";

/// Address of [`get_subscribed_apps`], for the installer's replacement table.
pub fn address() -> usize {
    get_subscribed_apps as GetSubscribedAppsFn as usize
}

/// Replacement for `CUser::GetSubscribedApps`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_list`,
/// `size` and `a3` must be whatever Steam passed. `app_list` is written only when
/// Steam passed a non-null buffer with a non-zero size, and only within the `size`
/// slots it declares.
pub unsafe extern "system" fn get_subscribed_apps(
    this: *mut c_void,
    app_list: *mut u32,
    size: u32,
    a3: u8,
) -> u32 {
    // SAFETY: the signature is the one documented in `crate::abi::subscribed`, and
    // the trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<GetSubscribedAppsFn>(SYMBOL) }) else {
        debug!("subscribed-apps hook reached without a published original; reporting none");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim, so Steam's own list is filled
    // exactly as it would have been without the hook.
    let steam = unsafe { original(this, app_list, size, a3) };
    let Some(state) = state::get() else {
        return steam;
    };
    let buffer = if app_list.is_null() || size == 0 {
        // The sizing call: Steam asks how many slots it needs and writes nothing.
        None
    } else {
        let slots = usize::try_from(size).unwrap_or(0);
        // SAFETY: this is the fill call - Steam passed a non-null buffer and told
        // the body how many `u32` slots it holds, which is the same contract the
        // original just filled it under.
        Some(unsafe { std::slice::from_raw_parts_mut(app_list, slots) })
    };
    let answer = decide(state, buffer, steam);
    match answer {
        SubscribedAnswer::Reported { count } => debug!(
            steam,
            count,
            controlled = state.controlled_apps().len(),
            "subscribed-apps: the host's app set is reported alongside Steam's"
        ),
        SubscribedAnswer::PassThrough { reason, .. } => {
            trace!(steam, reason, "subscribed-apps: Steam's own list stands")
        }
    }
    answer.count()
}

/// What the host wants Steam's subscription list to be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscribedAnswer {
    /// Steam's own list stands, unchanged, for the reason given.
    PassThrough { count: u32, reason: &'static str },
    /// The host's controlled apps belong in the list; `count` is the new length.
    Reported { count: u32 },
}

impl SubscribedAnswer {
    /// The count the body returns, whichever case this is.
    pub fn count(self) -> u32 {
        match self {
            Self::PassThrough { count, .. } | Self::Reported { count } => count,
        }
    }

    /// Whether the host's app set was reported.
    pub fn is_reported(self) -> bool {
        matches!(self, Self::Reported { .. })
    }
}

/// Decide the answer for one call, given the buffer Steam passed (if any) and the
/// count Steam's own walk produced.
///
/// `buffer` is `None` for the sizing call and `Some` for the fill call; a fill
/// call mutates it, which is why this takes an exclusive slice rather than a
/// length.
pub fn decide(state: &HostState, buffer: Option<&mut [u32]>, steam_count: u32) -> SubscribedAnswer {
    if !state.allows(Capability::Ownership) {
        return SubscribedAnswer::PassThrough {
            count: steam_count,
            reason: "the ownership capability is disabled for this run",
        };
    }
    let config = state.config();
    let pkg0_injected = state.package().injected_count() > 0;
    let Some(buffer) = buffer else {
        let extra = apps::get_subscribed_count_adjustment(config, pkg0_injected);
        return SubscribedAnswer::Reported {
            count: steam_count.saturating_add(extra),
        };
    };
    if usize::try_from(steam_count).unwrap_or(usize::MAX) > buffer.len() {
        return SubscribedAnswer::PassThrough {
            count: steam_count,
            reason: "Steam's list is larger than the buffer it passed",
        };
    }
    SubscribedAnswer::Reported {
        count: apps::on_get_subscribed_apps(config, buffer, steam_count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_config::AppId;

    fn state(configured: &[u32]) -> HostState {
        state_with(
            config_with(configured),
            configured.iter().copied().map(AppId).collect(),
        )
    }

    #[test]
    fn a_disabled_capability_returns_steams_count_and_writes_nothing() {
        let state = state(&[480]);
        let mut gate = state.gate();
        gate.disable(Capability::Ownership, "the symbol did not resolve");
        state.set_gate(gate);
        let mut slots = [730, 731, 999, 999];
        let answer = decide(&state, Some(&mut slots), 2);
        assert_eq!(
            answer,
            SubscribedAnswer::PassThrough {
                count: 2,
                reason: "the ownership capability is disabled for this run",
            }
        );
        assert_eq!(slots, [730, 731, 999, 999], "the buffer stays Steam's");
    }

    #[test]
    fn the_sizing_call_adds_the_controlled_apps_it_would_report() {
        let state = state(&[480, 481]);
        let answer = decide(&state, None, 3);
        assert_eq!(answer, SubscribedAnswer::Reported { count: 5 });
        assert!(answer.is_reported());
    }

    #[test]
    fn an_app_already_in_pkg0_is_not_counted_twice() {
        let state = state(&[480, 481]);
        let app = AppId(480);
        state.package().record_injected(&[app]);
        // Steam's own license walk already enumerates an injected app, so the
        // adjustment for the sizing call drops to zero there.
        assert_eq!(
            decide(&state, None, 3),
            SubscribedAnswer::Reported { count: 3 }
        );
        state.package().record_injected(&[]);
    }

    #[test]
    fn the_fill_call_appends_only_what_steam_does_not_already_report() {
        let state = state(&[480, 481]);
        let mut slots = [730, 480, 0, 0];
        let answer = decide(&state, Some(&mut slots), 2);
        assert_eq!(
            answer,
            SubscribedAnswer::Reported { count: 3 },
            "480 is already there, so only 481 is added"
        );
        assert_eq!(slots, [730, 480, 481, 0]);
    }

    #[test]
    fn an_unwritten_slot_is_never_left_with_stale_bytes() {
        let state = state(&[480, 481]);
        let mut slots = [730, 0xDEAD_BEEF, 0xDEAD_BEEF, 0xDEAD_BEEF];
        let answer = decide(&state, Some(&mut slots), 1);
        assert_eq!(answer, SubscribedAnswer::Reported { count: 3 });
        assert_eq!(
            slots[3], 0,
            "Steam sizes its vector from the sizing call and consumes every slot"
        );
    }

    #[test]
    fn a_buffer_too_small_for_steams_list_is_left_alone() {
        let state = state(&[480]);
        let mut slots = [730, 731];
        assert_eq!(
            decide(&state, Some(&mut slots), 3),
            SubscribedAnswer::PassThrough {
                count: 3,
                reason: "Steam's list is larger than the buffer it passed",
            }
        );
        assert_eq!(slots, [730, 731]);
    }

    #[test]
    fn a_full_buffer_is_never_overrun() {
        let state = state(&[480]);
        let mut slots = [730, 731];
        // No room for the addition: Steam's own answer is the only honest one, and
        // the buffer must not be touched past its two slots.
        assert_eq!(
            decide(&state, Some(&mut slots), 2),
            SubscribedAnswer::Reported { count: 2 }
        );
        assert_eq!(slots, [730, 731]);
    }

    #[test]
    fn an_unpublished_original_reports_nothing() {
        // SAFETY: the body is called with a null receiver and buffer and no live
        // Steam frame; every path it can take here returns before touching memory.
        let result =
            unsafe { get_subscribed_apps(std::ptr::null_mut(), std::ptr::null_mut(), 0, 0) };
        assert_eq!(result, 0);
    }
}
