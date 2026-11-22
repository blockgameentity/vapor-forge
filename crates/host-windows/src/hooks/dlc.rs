//! DLC overrides — the hook bodies for `IsAppDlcInstalled` and `BIsDlcEnabled`.
//!
//! What the bodies do, in one sentence: a DLC id the config gives a controlled
//! app (`[apps] inject` with `dlc`) is reported as installed and enabled, so Steam
//! stops treating a purchased-but-unowned DLC as missing. A DLC the user
//! configured nowhere keeps Steam's own answer.
//!
//! The decisions are [`decide`] and [`decide_enabled`]; both are the portable half
//! owned by `features::dlc`, and both are unit-tested without a live client.
//!
//! Every failure mode is a fall-through: a missing trampoline, unpublished host
//! state, or a disabled `dlc-overrides` capability returns Steam's answer
//! unchanged. Only the non-pass path logs, at debug.
//!
//! The call shapes, and which parts of them are measured, are documented in
//! [`crate::abi::dlc`]. Two entry points are kept apart rather than collapsed into
//! one body because a build could anchor one and not the other, and because the
//! enabled predicate also reports through an out flag.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_features::dlc;

use crate::abi::dlc::{BIsDlcEnabledFn, IsAppDlcInstalledFn};
use crate::abi::ret;
use crate::capability::Capability;
use crate::hooks::state::HostState;
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "IClientAppManager::IsAppDlcInstalled";

/// Corpus symbol the "enabled" body replaces; a second entry point of the same
/// object, anchored separately.
pub const ENABLED_SYMBOL: &str = "IClientAppManager::BIsDlcEnabled";

/// Address of [`is_app_dlc_installed`], for the installer's replacement table.
pub fn address() -> usize {
    is_app_dlc_installed as IsAppDlcInstalledFn as usize
}

/// Address of [`b_is_dlc_enabled`].
pub fn enabled_address() -> usize {
    b_is_dlc_enabled as BIsDlcEnabledFn as usize
}

/// Replacement for `IsAppDlcInstalled`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`,
/// `dlc_id` and the `r9` slot must be whatever Steam passed.
pub unsafe extern "system" fn is_app_dlc_installed(
    this: *mut c_void,
    app_id: u32,
    dlc_id: u32,
    r9: usize,
) -> u8 {
    // SAFETY: the signature is the one documented in `crate::abi::dlc`, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<IsAppDlcInstalledFn>(SYMBOL) }) else {
        debug!("dlc hook reached without a published original; reporting not installed");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim.
    let steam = unsafe { original(this, app_id, dlc_id, r9) };
    let Some(state) = state::get() else {
        return steam;
    };
    match decide(state, AppId(app_id), AppId(dlc_id)) {
        DlcAnswer::Override => {
            debug!(
                app_id,
                dlc_id, "dlc: a configured dlc of a controlled app is reported installed"
            );
            ret(true)
        }
        DlcAnswer::PassThrough => steam,
    }
}

/// Replacement for `BIsDlcEnabled`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`,
/// `dlc_id` must be whatever Steam passed, and `out_flag` must be the pointer the
/// original writes a byte through (or null, which the original tolerates).
pub unsafe extern "system" fn b_is_dlc_enabled(
    this: *mut c_void,
    app_id: u32,
    dlc_id: u32,
    out_flag: *mut u8,
) -> u8 {
    // SAFETY: the signature is the one documented in `crate::abi::dlc`.
    let Some(original) = (unsafe { original::original_of::<BIsDlcEnabledFn>(ENABLED_SYMBOL) })
    else {
        debug!("dlc-enabled hook reached without a published original; reporting disabled");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim.
    let steam = unsafe { original(this, app_id, dlc_id, out_flag) };
    let Some(state) = state::get() else {
        return steam;
    };
    match decide_enabled(state, AppId(app_id), AppId(dlc_id)) {
        DlcAnswer::Override => {
            if !out_flag.is_null() {
                // SAFETY: the original writes this same byte through this same
                // pointer (`mov [r9], r12b`), so it is a writable byte when the
                // body runs at all. Only 0/1 is ever written.
                unsafe { out_flag.write(1) };
            }
            debug!(
                app_id,
                dlc_id, "dlc: a configured dlc of a controlled app is reported enabled"
            );
            ret(true)
        }
        DlcAnswer::PassThrough => steam,
    }
}

/// What the host wants Steam to be told about one DLC of one app.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DlcAnswer {
    /// Steam's own answer stands, untouched.
    PassThrough,
    /// The DLC is configured for a controlled app: report it as present.
    Override,
}

/// Decide the answer for `dlc_id` of `app_id`, given what Steam said.
///
/// Only a `false` from Steam is ever overridden: a DLC Steam already reports as
/// present needs nothing from us, and answering for a DLC the user configured
/// nowhere would be an invention.
pub fn decide(state: &HostState, app_id: AppId, dlc_id: AppId) -> DlcAnswer {
    if !state.allows(Capability::DlcOverrides) {
        return DlcAnswer::PassThrough;
    }
    if dlc::on_is_dlc_installed(state.config(), app_id, dlc_id, false) {
        return DlcAnswer::Override;
    }
    DlcAnswer::PassThrough
}

/// As [`decide`], for the "enabled" predicate, which is a separate Steam call.
///
/// Kept separate rather than collapsed into one answer because the two Linux
/// hooks are two entry points: a build could anchor one and not the other.
pub fn decide_enabled(state: &HostState, app_id: AppId, dlc_id: AppId) -> DlcAnswer {
    if !state.allows(Capability::DlcOverrides) {
        return DlcAnswer::PassThrough;
    }
    if dlc::on_is_dlc_enabled(state.config(), app_id, dlc_id, false) {
        return DlcAnswer::Override;
    }
    DlcAnswer::PassThrough
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::state_with;
    use vapor_forge_config::{InjectApp, RuntimeConfig};

    fn config_with_dlc(app: u32, dlcs: &[u32]) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.apps.push_inject(InjectApp {
            id: AppId(app),
            dlc: dlcs.iter().copied().map(AppId).collect(),
            ticket: Default::default(),
            purchase_time: 0,
        });
        config
    }

    #[test]
    fn a_configured_dlc_of_a_controlled_app_is_reported_present() {
        let state = state_with(config_with_dlc(480, &[505_730]), Vec::new());
        assert_eq!(
            decide(&state, AppId(480), AppId(505_730)),
            DlcAnswer::Override
        );
        assert_eq!(
            decide_enabled(&state, AppId(480), AppId(505_730)),
            DlcAnswer::Override
        );
    }

    #[test]
    fn a_dlc_the_user_configured_nowhere_is_left_to_steam() {
        let state = state_with(config_with_dlc(480, &[505_730]), Vec::new());
        assert_eq!(
            decide(&state, AppId(480), AppId(505_999)),
            DlcAnswer::PassThrough
        );
        // Same DLC id under an app we do not control: also not ours to answer.
        assert_eq!(
            decide(&state, AppId(999), AppId(505_730)),
            DlcAnswer::PassThrough
        );
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with_dlc(480, &[505_730]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::DlcOverrides, "the symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, AppId(480), AppId(505_730)),
            DlcAnswer::PassThrough
        );
        assert_eq!(
            decide_enabled(&state, AppId(480), AppId(505_730)),
            DlcAnswer::PassThrough
        );
    }
}
