//! `CSteamUIAppController::BuildCompleteAppOverviewChange` — the library
//! snapshot body.
//!
//! This is the one call Steam makes when it rebuilds the whole library list, and
//! Linux uses it to append ids to the change's `removed_appid` field so a rebuild
//! cannot restore a title the user removed. **That append is deliberately not
//! ported.** Two pieces it needs are missing on this host, and inventing either
//! would be a guess:
//!
//! * `google::protobuf::RepeatedField<uint32>::Add` — the Linux host resolves it
//!   as its own corpus symbol and reserves capacity before appending. There is no
//!   such anchor in `res/patterns/win64/steamui/`, so the field could only be
//!   mutated by guessing the protobuf internals.
//! * a removal ledger. `crates/hooks/src/ui/library.rs` keeps the ids to hide
//!   because that host can hot-reload the config and drop an app mid-session.
//!   This host resolves the config once at start-up and never removes an app from
//!   `[apps] inject` while Steam is running, so there is nothing to remove.
//!
//! What the body does instead is the part that is honest today: it calls the
//! original first (the snapshot must be Steam's), then reports the rebuild
//! boundary with a count of the controlled apps that still depend on this host's
//! ownership injection. A count of zero is the "unchanged" case — the rebuild
//! needs nothing from us — and a non-zero count is the set of apps whose overview
//! metadata and visibility the next `FillInAppOverview` and
//! `ShouldShowAppInLibrary` calls must cover.
//!
//! Flags: the `library-snapshot` capability must have survived the install pass,
//! at least one app must be controlled, and at least one of those must still need
//! injected ownership (a confirmed license is Steam's own answer).

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_features::apps::{self, AppAuthority};

use crate::abi::ui::BuildCompleteAppOverviewChangeFn;
use crate::capability::Capability;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CSteamUIAppController::BuildCompleteAppOverviewChange";

/// Address of [`build_complete_app_overview_change`], for the installer's table.
pub fn address() -> usize {
    build_complete_app_overview_change as BuildCompleteAppOverviewChangeFn as usize
}

/// What the host knows once Steam has rebuilt the library list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotDecision {
    /// Nothing to report: the capability is off, or no controlled app depends on
    /// this host any more.
    Unchanged,
    /// The rebuild completed while `pending` controlled apps still need injected
    /// ownership.
    Rebuilt { pending: usize },
}

/// Replacement for `CSteamUIAppController::BuildCompleteAppOverviewChange`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `change`,
/// `callback_slot` and `r9` must be whatever Steam passed. `change` is never
/// dereferenced — the protobuf layout is unmeasured on a Windows build.
pub unsafe extern "system" fn build_complete_app_overview_change(
    this: *mut c_void,
    change: *mut c_void,
    callback_slot: *mut c_void,
    r9: usize,
) {
    // SAFETY: the signature is the one documented for this symbol, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) =
        (unsafe { original::original_of::<BuildCompleteAppOverviewChangeFn>(SYMBOL) })
    else {
        debug!("snapshot hook reached without a published original; falling through to Steam");
        return;
    };
    // SAFETY: every slot is forwarded verbatim, including the unmeasured `r9`.
    unsafe { original(this, change, callback_slot, r9) };
    let Some(state) = state::get() else {
        return;
    };
    if let SnapshotDecision::Rebuilt { pending } = decide(state) {
        debug!(
            pending,
            "library rebuild completed with controlled apps still injected"
        );
    }
}

/// Decide what the rebuild boundary means, using this host's own state.
pub fn decide(state: &HostState) -> SnapshotDecision {
    decide_with(
        state,
        state
            .controlled_apps()
            .iter()
            .map(|&app| apps::classify_app(state.config(), app)),
    )
}

/// As [`decide`], over an explicit list of authorities.
///
/// The authorities are a parameter because they resolve process-global ownership
/// state; taking them as an input keeps every row of the test table pure.
pub fn decide_with(
    state: &HostState,
    authorities: impl IntoIterator<Item = AppAuthority>,
) -> SnapshotDecision {
    if !state.allows(Capability::LibrarySnapshot) {
        return SnapshotDecision::Unchanged;
    }
    let pending = authorities
        .into_iter()
        .filter(|authority| authority.requires_injected_ownership())
        .count();
    if pending == 0 {
        SnapshotDecision::Unchanged
    } else {
        SnapshotDecision::Rebuilt { pending }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_config::{AppCategory, AppId};
    use vapor_forge_features::apps::OwnershipState;

    fn state() -> HostState {
        state_with(config_with(&[730, 731]), vec![AppId(730), AppId(731)])
    }

    fn controlled(ownership: OwnershipState) -> AppAuthority {
        AppAuthority::Controlled {
            category: AppCategory::Inject,
            ownership,
        }
    }

    /// (configuration, ownership of the controlled apps) → expected decision.
    #[test]
    fn the_decision_table() {
        let state = state();
        let cases = [
            ("no controlled apps", Vec::new(), SnapshotDecision::Unchanged),
            (
                "every controlled app has a genuine license",
                vec![controlled(OwnershipState::Owned), controlled(OwnershipState::Owned)],
                SnapshotDecision::Unchanged,
            ),
            (
                "one controlled app still needs injection",
                vec![controlled(OwnershipState::Owned), controlled(OwnershipState::Unowned)],
                SnapshotDecision::Rebuilt { pending: 1 },
            ),
            (
                "both controlled apps still need injection",
                vec![controlled(OwnershipState::Unknown), controlled(OwnershipState::Unowned)],
                SnapshotDecision::Rebuilt { pending: 2 },
            ),
        ];
        for (case, authorities, expected) in cases {
            assert_eq!(decide_with(&state, authorities), expected, "{case}");
        }
    }

    #[test]
    fn an_uncontrolled_authority_is_never_pending() {
        let state = state();
        assert_eq!(
            decide_with(&state, [AppAuthority::Uncontrolled]),
            SnapshotDecision::Unchanged
        );
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state();
        let mut gate = state.gate();
        gate.disable(Capability::LibrarySnapshot, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(
            decide_with(&state, [controlled(OwnershipState::Unowned)]),
            SnapshotDecision::Unchanged
        );
    }
}
