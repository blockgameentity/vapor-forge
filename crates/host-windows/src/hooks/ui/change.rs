//! `CUpdateManager::MarkAppChange` — the library change body.
//!
//! Steam calls this to tell its update manager that one app's UI data changed.
//! Linux uses the call for two things: it captures the update-manager `this`
//! pointer so a later drain can re-mark an app, and it asks the UI-work queue to
//! run. Neither is ported, and for a stated reason: the queue it feeds
//! (`features::toast::request_ui_work` → `RunFrame`) is drained by a body this
//! host does not install, and re-marking an app requires `GetAppByID` plus a
//! stamp — a chain that would be invented here rather than measured.
//!
//! What is left is the observation, which is worth having: on the only call
//! Steam makes per app change, the host can say whether the change is about an
//! app it controls and still injects. The body never writes to Steam's memory and
//! never allocates.
//!
//! Flags: the `library-snapshot` capability must have survived the install pass
//! and the app must be controlled by the effective config *and* still need
//! injected ownership. Everything else returns Steam's own answer untouched.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_features::apps::{self, AppAuthority};

use crate::abi::ui::MarkAppChangeFn;
use crate::capability::Capability;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
///
/// Anchored in the public corpus only; on a publicbeta build the symbol does not
/// resolve, so `install::deps` disables `library-snapshot` with that reason.
pub const SYMBOL: &str = "CUpdateManager::MarkAppChange";

/// Address of [`mark_app_change`], for the installer's table.
pub fn address() -> usize {
    mark_app_change as MarkAppChangeFn as usize
}

/// What the host makes of one change notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangeDecision {
    /// Not ours, or nothing left to do for it.
    Unchanged,
    /// A controlled app that still relies on injected ownership changed.
    Observed { app_id: u32, flags: u32 },
}

/// Replacement for `CUpdateManager::MarkAppChange`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`
/// and `flags` must be whatever Steam passed. `this` is forwarded and never
/// dereferenced, and no Steam-owned memory is read.
pub unsafe extern "system" fn mark_app_change(this: *mut c_void, app_id: u32, flags: u32) {
    if let Some(state) = state::get() {
        let authority = apps::classify_app(state.config(), AppId(app_id));
        if let ChangeDecision::Observed { app_id, flags } = decide(state, authority, app_id, flags) {
            debug!(
                app = app_id,
                flags,
                "Steam marked a controlled app changed"
            );
        }
    }
    // SAFETY: the signature is the one documented for this symbol, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<MarkAppChangeFn>(SYMBOL) }) else {
        debug!("change hook reached without a published original; falling through to Steam");
        return;
    };
    // SAFETY: every slot is forwarded verbatim.
    unsafe { original(this, app_id, flags) };
}

/// Decide what `app_id`'s change notification means for this host.
pub fn decide(
    state: &HostState,
    authority: AppAuthority,
    app_id: u32,
    flags: u32,
) -> ChangeDecision {
    if !state.allows(Capability::LibrarySnapshot) {
        return ChangeDecision::Unchanged;
    }
    if !authority.is_controlled() || !authority.requires_injected_ownership() {
        return ChangeDecision::Unchanged;
    }
    ChangeDecision::Observed { app_id, flags }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_config::AppCategory;
    use vapor_forge_features::apps::OwnershipState;

    const APP: u32 = 730;
    const FLAGS: u32 = 1;

    fn state() -> HostState {
        state_with(config_with(&[APP]), vec![AppId(APP)])
    }

    fn controlled(ownership: OwnershipState) -> AppAuthority {
        AppAuthority::Controlled {
            category: AppCategory::Inject,
            ownership,
        }
    }

    /// (configuration, Steam's answer) → expected decision.
    #[test]
    fn the_decision_table() {
        let state = state();
        let cases = [
            (
                "uncontrolled app",
                AppAuthority::Uncontrolled,
                ChangeDecision::Unchanged,
            ),
            (
                "controlled app still injected",
                controlled(OwnershipState::Unowned),
                ChangeDecision::Observed {
                    app_id: APP,
                    flags: FLAGS,
                },
            ),
            (
                "controlled app with no ownership sample yet",
                controlled(OwnershipState::Unknown),
                ChangeDecision::Observed {
                    app_id: APP,
                    flags: FLAGS,
                },
            ),
            (
                "controlled app with a genuine license",
                controlled(OwnershipState::Owned),
                ChangeDecision::Unchanged,
            ),
        ];
        for (case, authority, expected) in cases {
            assert_eq!(decide(&state, authority, APP, FLAGS), expected, "{case}");
        }
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state();
        let mut gate = state.gate();
        gate.disable(Capability::LibrarySnapshot, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, controlled(OwnershipState::Unowned), APP, FLAGS),
            ChangeDecision::Unchanged
        );
    }
}
