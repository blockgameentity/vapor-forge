//! `CPackageInfo::GetPackageInfo` — the package hook body.
//!
//! A package lookup is the library's refresh boundary: it is where the Windows
//! fork left a borrower marker behind after a family license lapsed, so the app
//! stayed labelled "from family library". The body therefore runs the marker
//! housekeeping pass `features::package` already implements
//! (`PackageState::remove_borrower_marker` → `family::marker::remove_if_unborrowed`)
//! over every app the config controls, and nothing else.
//!
//! It never rewrites the returned structure. `CPackageInfo::GetPackageInfo`
//! returns a pointer into Steam's own package table; no field of that structure
//! is measured on a Windows build (the corpus anchor only proves argument 1 is
//! 32 bits and that `r9` is not an input), so the honest action is the one on our
//! own state.
//!
//! Flags: the `package-injection` capability must have survived the install pass
//! and at least one app must be controlled. Otherwise the body is a pure
//! fall-through.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_features::apps;
use vapor_forge_features::family::marker::MarkerOutcome;

use crate::abi::package::GetPackageInfoFn;
use crate::capability::Capability;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CPackageInfo::GetPackageInfo";

/// Address of [`get_package_info`], for the installer's replacement table.
pub fn address() -> usize {
    get_package_info as GetPackageInfoFn as usize
}

/// Replacement for `CPackageInfo::GetPackageInfo`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`,
/// `package_id` and `r8` must be whatever Steam passed, and the return value is
/// Steam's own package pointer, forwarded unchanged.
pub unsafe extern "system" fn get_package_info(
    this: *mut c_void,
    package_id: u32,
    r8: usize,
) -> *mut u8 {
    // SAFETY: the signature is the one documented for this symbol, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<GetPackageInfoFn>(SYMBOL) }) else {
        debug!(
            package_id,
            "package hook reached without a published original; falling through to Steam"
        );
        return std::ptr::null_mut();
    };
    // SAFETY: every slot is forwarded verbatim.
    let package = unsafe { original(this, package_id, r8) };
    if let Some(state) = state::get() {
        refresh_borrower_markers(state);
    }
    package
}

/// Run the marker pass for every controlled app; returns how many markers were
/// removed.
///
/// Idempotent: an app without a marker reports [`MarkerOutcome::Absent`] and
/// writes nothing, so calling this on every package lookup is safe.
pub fn refresh_borrower_markers(state: &HostState) -> usize {
    if !state.allows(Capability::PackageInjection) {
        return 0;
    }
    let settings = state.family_settings();
    let self_account = state.self_account_id();
    let mut removed = 0;
    for &app in state.controlled_apps() {
        let injected_by_us = state.package().is_injected_into_pkg0(app);
        let real = apps::provenance(app, injected_by_us, self_account, settings);
        match state.package().remove_borrower_marker(app, real, settings) {
            MarkerOutcome::Removed { lender } => {
                removed += 1;
                debug!(
                    app_id = app.0,
                    lender,
                    provenance = real.label(),
                    "package: borrower marker removed"
                );
            }
            // A refresh runs on every package lookup, so the "nothing to do"
            // answers stay at trace: only a change is worth a debug line.
            outcome => tracing::trace!(
                app_id = app.0,
                outcome = outcome.label(),
                "package: borrower marker left as it is"
            ),
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};
    use vapor_forge_config::AppId;
    use vapor_forge_features::family::provenance::Provenance;

    #[test]
    fn a_marker_is_removed_once_the_lender_is_gone() {
        let app = AppId(810);
        let state = state_with(config_with(&[app.0]), vec![app]);
        state.package().stamp_borrower_marker(app, 4711);
        assert_eq!(state.package().borrower_marker(app), Some(4711));
        // No recorded ownership and no injection: the app is no longer borrowed.
        assert_eq!(
            apps::provenance(app, false, 0, state.family_settings()),
            Provenance::Unknown
        );
        assert_eq!(refresh_borrower_markers(&state), 1);
        assert_eq!(state.package().borrower_marker(app), None);
        assert_eq!(
            refresh_borrower_markers(&state),
            0,
            "the pass is idempotent, so a second lookup changes nothing"
        );
    }

    #[test]
    fn a_marker_survives_while_a_lender_still_covers_the_app() {
        let app = AppId(811);
        let state = state_with(config_with(&[app.0]), vec![app]);
        state.package().stamp_borrower_marker(app, 4711);
        // A recorded sample is what gets `provenance` past its "nothing known
        // yet" short circuit; without it the honest answer is `Unknown` and the
        // marker would be removed.
        apps::record_actual_ownership(app, false);
        apps::record_family_shared(app, true);
        apps::record_family_owner(app, 4711);
        assert_eq!(
            apps::provenance(app, false, 0, state.family_settings()),
            Provenance::FamilyShared
        );
        assert_eq!(refresh_borrower_markers(&state), 0);
        assert_eq!(state.package().borrower_marker(app), Some(4711));
        apps::record_family_shared(app, false);
    }

    #[test]
    fn no_controlled_apps_means_no_work_at_all() {
        let state = state_with(vapor_forge_config::RuntimeConfig::default(), Vec::new());
        assert_eq!(refresh_borrower_markers(&state), 0);
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let app = AppId(812);
        let state = state_with(config_with(&[app.0]), vec![app]);
        state.package().stamp_borrower_marker(app, 7);
        let mut gate = state.gate();
        gate.disable(Capability::PackageInjection, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(refresh_borrower_markers(&state), 0);
        assert_eq!(state.package().borrower_marker(app), Some(7));
    }
}
