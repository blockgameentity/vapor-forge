//! `ShouldShowAppInLibrary` — the library visibility body.
//!
//! Steam's own answer decides whether a title appears in the library, and for an
//! ingested app that answer is "no": Steam has no license for it, so its filters
//! drop it. The body answers the question the library list is actually asking —
//! *is this one of ours and still unowned?* — and, when Steam said "no" for an app
//! the config controls, says "yes".
//!
//! What it changes is the return value and nothing else. The Linux host performs
//! the same override by temporarily clearing an ownership flag inside
//! `CSteamApp`; this host never writes to Steam's record here, because the only
//! field it would have to touch is at an assumed offset (see
//! [`crate::abi::ui`]). Overriding the predicate is both smaller and safer: a
//! wrong decision costs a wrong `bool` on one call, not a corrupted record.
//!
//! The app id is read from Steam's record through the assumed layout in
//! [`crate::abi::ui::record`]. If that offset is wrong for a live build the read
//! yields some unrelated number, which is not a controlled app id, so the body
//! falls through to Steam's own answer: the failure mode is "no override", never
//! "an app we do not control is forced into the library".
//!
//! Flags, all of which are a pure fall-through when they say no:
//!
//! * the `library-ui` capability survived the install pass;
//! * the app is controlled by the effective config;
//! * its authority still requires injected library visibility — a confirmed
//!   genuine license, or a DLC entry Steam filters natively, needs nothing;
//! * Steam's own answer is `false`. An app Steam already shows is left alone.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_features::apps::{self, AppAuthority};

use crate::abi::ui::{record, ShouldShowAppInLibraryFn};
use crate::abi::ret;
use crate::capability::Capability;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "ShouldShowAppInLibrary";

/// Address of [`should_show_app_in_library`], for the installer's table.
pub fn address() -> usize {
    should_show_app_in_library as ShouldShowAppInLibraryFn as usize
}

/// What the host wants Steam's visibility predicate to answer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibilityDecision {
    /// Steam's answer stands; the body returns it unchanged.
    Unchanged,
    /// Steam said no for an app we control: report it as visible.
    Visible,
}

/// Replacement for `ShouldShowAppInLibrary`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `app` must be
/// whatever Steam passed. Nothing is dereferenced beyond the two assumed 4-byte
/// fields documented in [`crate::abi::ui::record`].
pub unsafe extern "system" fn should_show_app_in_library(app: *mut c_void) -> u8 {
    // SAFETY: the signature is the one documented for this symbol, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<ShouldShowAppInLibraryFn>(SYMBOL) })
    else {
        // Fail closed: "do not show". Reaching this means the body was entered
        // without the detour that installed it, and inventing a `true` here would
        // force an unknown app into the library.
        debug!("library visibility reached without a published original; reporting hidden");
        return 0;
    };
    // SAFETY: the argument is forwarded verbatim.
    let steam = unsafe { original(app) };
    let Some(state) = state::get() else {
        return steam;
    };
    // SAFETY: `app` is the record Steam passed to this call.
    let Some(app_id) = (unsafe { record::app_id(app) }) else {
        return steam;
    };
    let authority = apps::classify_app(state.config(), AppId(app_id));
    match decide(state, authority, steam != 0) {
        VisibilityDecision::Unchanged => steam,
        VisibilityDecision::Visible => {
            debug!(
                app_id,
                steam_visible = false,
                "library visibility overridden for a controlled app"
            );
            ret(true)
        }
    }
}

/// Decide the answer for `app_id`, given what Steam's own predicate returned.
///
/// The authority is a parameter rather than looked up here: ownership state is a
/// process global, and taking it as an input is what makes every row of the test
/// table below a pure function call.
pub fn decide(state: &HostState, authority: AppAuthority, steam: bool) -> VisibilityDecision {
    if !state.allows(Capability::LibraryUi) {
        return VisibilityDecision::Unchanged;
    }
    if !authority.requires_injected_library_visibility() || steam {
        return VisibilityDecision::Unchanged;
    }
    VisibilityDecision::Visible
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_config::AppCategory;
    use vapor_forge_features::apps::OwnershipState;

    const APP: u32 = 730;

    fn state() -> HostState {
        state_with(config_with(&[APP]), vec![AppId(APP)])
    }

    fn controlled(category: AppCategory, ownership: OwnershipState) -> AppAuthority {
        AppAuthority::Controlled {
            category,
            ownership,
        }
    }

    /// (configuration, Steam's answer) → expected decision.
    #[test]
    fn the_decision_table() {
        let state = state();
        let cases = [
            (
                "uncontrolled app, Steam hides it",
                AppAuthority::Uncontrolled,
                false,
                VisibilityDecision::Unchanged,
            ),
            (
                "configured app, Steam hides it",
                controlled(AppCategory::Inject, OwnershipState::Unknown),
                false,
                VisibilityDecision::Visible,
            ),
            (
                "configured app with no genuine sample, Steam hides it",
                controlled(AppCategory::Inject, OwnershipState::Unowned),
                false,
                VisibilityDecision::Visible,
            ),
            (
                "configured app Steam already shows",
                controlled(AppCategory::Inject, OwnershipState::Unknown),
                true,
                VisibilityDecision::Unchanged,
            ),
            (
                "configured app with a genuine license, Steam hides it",
                controlled(AppCategory::Inject, OwnershipState::Owned),
                false,
                VisibilityDecision::Unchanged,
            ),
            (
                "DLC entry: Steam's own filtering is never overridden",
                controlled(
                    AppCategory::InjectDlc { parent: AppId(APP) },
                    OwnershipState::Unowned,
                ),
                false,
                VisibilityDecision::Unchanged,
            ),
        ];
        for (case, authority, steam, expected) in cases {
            assert_eq!(decide(&state, authority, steam), expected, "{case}");
        }
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state();
        let mut gate = state.gate();
        gate.disable(Capability::LibraryUi, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(
            decide(
                &state,
                controlled(AppCategory::Inject, OwnershipState::Unowned),
                false
            ),
            VisibilityDecision::Unchanged
        );
    }
}
