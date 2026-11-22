//! `CSteamUIAppController::FillInAppOverview` — the overview metadata body.
//!
//! Steam builds every app's library overview from its own record, and for an
//! ingested app the purchase metadata is empty: Steam never sold it to this
//! account. `[apps] inject[].purchase_time` is the config's answer to that, and
//! this body is where it reaches the record Steam is about to read — the same
//! point, and the same order, as the Linux host (`stamp_purchase_time` runs
//! before the original fills the overview).
//!
//! # What it writes, and the risk it carries
//!
//! One 32-bit field, `purchased_time`, at the offset the Linux host measures for
//! a 64-bit `CSteamApp` (`0x2c`). That offset is **assumed** for a Windows build:
//! no Windows client has executed this host, so this is the one body whose
//! assumption is a write. If the layout differs the cost is a wrong 4-byte write
//! inside Steam's app record; that is why the decision is also the narrowest one
//! in this crate — it needs a controlled app, no genuine license yet, and a
//! configured value that differs from what Steam already has.
//!
//! Flags, all of which are a pure fall-through when they say no:
//!
//! * the `overview-metadata` capability survived the install pass;
//! * the app is controlled and still needs injected ownership (a confirmed
//!   license makes the stamp pointless and Steam's own value authoritative);
//! * `[apps] inject[].purchase_time` is non-zero — "unset" is not "epoch";
//! * Steam's current value differs from the configured one, so a call during a
//!   rebuild that is already correct writes nothing.

use core::ffi::c_void;

use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_features::apps::{self, AppAuthority};

use crate::abi::ui::{record, FillInAppOverviewFn};
use crate::capability::Capability;
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
///
/// Anchored in the publicbeta corpus only; on a public build the symbol does not
/// resolve and `install::deps` disables `overview-metadata` with that reason.
pub const SYMBOL: &str = "CSteamUIAppController::FillInAppOverview";

/// Address of [`fill_in_app_overview`], for the installer's table.
pub fn address() -> usize {
    fill_in_app_overview as FillInAppOverviewFn as usize
}

/// What the host wants written into the app record before Steam reads it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StampDecision {
    /// Leave the record as Steam has it.
    Unchanged,
    /// Write this purchase time.
    Stamp(u32),
}

/// Replacement for `CSteamUIAppController::FillInAppOverview`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`,
/// `overview` and `app` must be whatever Steam passed. `overview` is forwarded
/// and never dereferenced; `app` is read and possibly written at the two assumed
/// offsets documented in [`crate::abi::ui::record`].
pub unsafe extern "system" fn fill_in_app_overview(
    this: *mut c_void,
    overview: *mut c_void,
    app: *mut c_void,
) {
    // SAFETY: the signature is the one documented for this symbol, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<FillInAppOverviewFn>(SYMBOL) }) else {
        debug!("overview hook reached without a published original; falling through to Steam");
        return;
    };
    if let Some(state) = state::get() {
        // SAFETY: `app` is the record Steam passed to this call.
        if let Some(app_id) = unsafe { record::app_id(app) } {
            let app_id = AppId(app_id);
            let authority = apps::classify_app(state.config(), app_id);
            // SAFETY: as above; reads four bytes at the assumed offset.
            let steam = unsafe { record::purchased_time(app) };
            if let StampDecision::Stamp(value) = decide(state, app_id, authority, steam) {
                // SAFETY: as above; the one write into Steam's record.
                unsafe { record::set_purchased_time(app, value) };
                debug!(
                    app = app_id.0,
                    steam,
                    stamped = value,
                    "overview purchase time stamped"
                );
            }
        }
    }
    // SAFETY: every argument is forwarded verbatim, and the record has already
    // been adjusted for the original to read.
    unsafe { original(this, overview, app) };
}

/// Decide whether `app_id`'s record should carry the configured purchase time.
pub fn decide(
    state: &HostState,
    app_id: AppId,
    authority: AppAuthority,
    steam_purchased_time: u32,
) -> StampDecision {
    if !state.allows(Capability::OverviewMetadata) {
        return StampDecision::Unchanged;
    }
    if !authority.requires_injected_ownership() {
        return StampDecision::Unchanged;
    }
    let configured = state.config().purchase_time(app_id);
    if configured == 0 || configured == steam_purchased_time {
        return StampDecision::Unchanged;
    }
    StampDecision::Stamp(configured)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::hooks::tests_support::state_with;
    use vapor_forge_config::{AppCategory, InjectApp, RuntimeConfig};
    use vapor_forge_features::apps::OwnershipState;

    const APP: AppId = AppId(730);

    /// One configured app; `purchase_time` is the field under test, so it is set
    /// directly rather than through the shared builder.
    fn state_with_purchase_time(purchase_time: u32) -> HostState {
        let mut config = RuntimeConfig::default();
        config.apps.push_inject(InjectApp {
            id: APP,
            dlc: Vec::new(),
            ticket: Default::default(),
            purchase_time,
        });
        state_with(config, vec![APP])
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
        let stamped = state_with_purchase_time(1_700_000_000);
        let unstamped = state_with_purchase_time(0);
        let cases = [
            (
                "uncontrolled app",
                &stamped,
                AppAuthority::Uncontrolled,
                0,
                StampDecision::Unchanged,
            ),
            (
                "configured app, Steam has no purchase time",
                &stamped,
                controlled(OwnershipState::Unowned),
                0,
                StampDecision::Stamp(1_700_000_000),
            ),
            (
                "configured app, Steam already has the configured time",
                &stamped,
                controlled(OwnershipState::Unowned),
                1_700_000_000,
                StampDecision::Unchanged,
            ),
            (
                "configured app, Steam has a different time",
                &stamped,
                controlled(OwnershipState::Unknown),
                1,
                StampDecision::Stamp(1_700_000_000),
            ),
            (
                "configured app with a genuine license",
                &stamped,
                controlled(OwnershipState::Owned),
                0,
                StampDecision::Unchanged,
            ),
            (
                "app configured without a purchase time",
                &unstamped,
                controlled(OwnershipState::Unowned),
                0,
                StampDecision::Unchanged,
            ),
        ];
        for (case, state, authority, steam, expected) in cases {
            let decision = decide(state, APP, authority, steam);
            assert_eq!(decision, expected, "{case}");
        }
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with_purchase_time(1_700_000_000);
        let mut gate = state.gate();
        gate.disable(Capability::OverviewMetadata, "symbol did not resolve");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, APP, controlled(OwnershipState::Unowned), 0),
            StampDecision::Unchanged
        );
    }
}
