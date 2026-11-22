//! The `CUser::GetSteamID` body: Steam's answer first, the override second.
//!
//! The accessor is `CSteamID* GetSteamID(this, CSteamID* out)` — it loads the
//! account member at `CUser+0x20A`, stores it through `rdx`, and returns `rdx`
//! (see [`crate::abi::steam_id`] for what is measured and what is assumed). The
//! body therefore does exactly what the Linux `CUser::GetSteamID` hook does, in
//! the shape the Windows accessor has:
//!
//! 1. call the published original first, with both measured slots forwarded
//!    verbatim, so the caller's storage holds Steam's own answer;
//! 2. read that answer back and ask [`super::decide`] whether an override applies;
//! 3. when it does, write the override through the caller's own pointer and
//!    return that pointer — the accessor's return convention, never a `u64`.
//!
//! Every other path forwards: no published trampoline, no published host state,
//! a null out-pointer, an app the config does not control, a capability that did
//! not survive the install pass, a delegate window that is closed, or a configured
//! value that is refused. None of them writes a single byte, and none of them
//! invents an id: the buffer keeps whatever Steam put there.
//!
//! A null out-pointer is forwarded unchanged rather than repaired. Steam's own
//! body would fault on it; the only honest thing a replacement can do is hand the
//! call back untouched, because allocating storage the caller does not know about
//! would be a leak with a SteamID written into it.
//!
//! No `unwrap`, no allocation, no lock held across a call, nothing blocking:
//! this runs on whatever thread of the client asked for the account id.

use core::ffi::c_void;

use tracing::{debug, trace};
use vapor_forge_config::AppId;
use vapor_forge_features::ticket;

use crate::abi::steam_id::{CSteamId, GetSteamIdFn};
use crate::capability::Capability;
use crate::hooks::state::{self, HostState};
use crate::hooks::{original, steam_id};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CUser::GetSteamID";

/// Address of [`get_steam_id`], for the installer's replacement table.
pub fn address() -> usize {
    get_steam_id as GetSteamIdFn as usize
}

/// Replacement for `CUser::GetSteamID`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this` and `out`
/// must be whatever Steam passed. `out` is written only when it is non-null, and
/// only with the 8-byte `CSteamID` documented in [`crate::abi::steam_id`].
pub unsafe extern "system" fn get_steam_id(
    this: *mut c_void,
    out: *mut CSteamId,
) -> *mut CSteamId {
    // SAFETY: the type passed here is the two-slot shape documented in
    // `crate::abi::steam_id`, and the trampoline was published for this symbol
    // before its entry was patched.
    let Some(original) = (unsafe { original::original_of::<GetSteamIdFn>(SYMBOL) }) else {
        trace!("steam-id: accessor reached without a published original; reporting no id");
        // `null` rather than a pointer of our own: there is no answer to report,
        // and the caller's buffer is not ours to guess at.
        return core::ptr::null_mut();
    };
    // SAFETY: forwarding both measured slots verbatim; the trampoline runs the
    // instructions displaced from this entry, which read `rcx` and `rdx` only.
    let returned = unsafe { original(this, out) };
    // SAFETY: `out` is either null or the caller's own CSteamID storage, which
    // Steam's accessor just stored through.
    unsafe { override_into(state::get(), out) }.unwrap_or(returned)
}

/// Apply the host's decision to the answer Steam already wrote through `caller`.
///
/// `state` is `None` until the host has published its state, which is the "called
/// before initialisation" case. `None` is returned for every fall-through, so the
/// shell above can report Steam's own pointer unchanged. `Some(pointer)` is the
/// caller's pointer, to be reported as the accessor's return value.
///
/// # Safety
/// `caller` must be null or point at the 8 writable bytes the accessor's own
/// store went through — the caller's `CSteamID`, never a buffer of ours.
pub unsafe fn override_into(
    state: Option<&HostState>,
    caller: *mut CSteamId,
) -> Option<*mut CSteamId> {
    if caller.is_null() {
        return None;
    }
    let state = state?;
    // No app id arrives with the call, so the served app comes from the config
    // alone; with more than one candidate this is `None` and Steam's answer stays.
    let app = super::served::served_app(state)?;
    // SAFETY: `caller` is non-null, and Steam's own store through this same
    // pointer is what proves 8 writable bytes are there.
    let real = unsafe { CSteamId::read_at(caller) }.steam_id64();
    let steam_id::PresentedId::Override { steam_id64, source } =
        steam_id::decide(state, app, real, delegate(state, app))
    else {
        return None;
    };
    // SAFETY: as above — the caller's own storage, still non-null.
    unsafe { CSteamId::write_at(caller, CSteamId::new(steam_id64)) };
    debug!(
        app_id = app.0,
        steam_id = steam_id64,
        source = source.label(),
        "steam-id: override written through the caller's out-parameter"
    );
    Some(caller)
}

/// The delegate ticket owner for `app`, while that capability is usable.
///
/// `0` is "no delegate window open", which [`steam_id::decide`] reads as "no
/// delegate". The lookup is process-local state only: nothing here dials, blocks
/// or waits.
fn delegate(state: &HostState, app: AppId) -> u64 {
    if state.allows(Capability::TicketOverrides) {
        ticket::delegate_steamid(app)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::IdentitySettings;
    use crate::hooks::tests_support::{config_with, state_with};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    const REAL: u64 = 76_561_198_000_000_001;
    const CONFIGURED: u64 = 76_561_198_000_000_003;
    const APP: u32 = 440;

    /// The state a live host publishes for one controlled app, optionally with a
    /// configured override.
    fn state(overrides: &[(u32, u64)]) -> HostState {
        state_with(config_with(&[APP]), Vec::new()).with_identity_settings(IdentitySettings {
            overrides: overrides
                .iter()
                .map(|&(app, id)| (AppId(app), id))
                .collect::<BTreeMap<_, _>>(),
        })
    }

    /// What Steam's own accessor would have handed us.
    fn steam_slot() -> CSteamId {
        CSteamId::new(REAL)
    }

    #[test]
    fn without_a_published_state_steams_answer_stands() {
        let mut slot = steam_slot();
        // SAFETY: `slot` is a live local of the documented type.
        assert_eq!(unsafe { super::override_into(None, &mut slot) }, None);
        assert_eq!(slot.steam_id64(), REAL);
    }

    #[test]
    fn a_disabled_capability_leaves_steams_answer_standing() {
        let state = state(&[(APP, CONFIGURED)]);
        let mut gate = state.gate();
        gate.disable(Capability::SteamIdOverride, "symbol did not resolve");
        state.set_gate(gate);
        let mut slot = steam_slot();
        // SAFETY: `slot` is a live local of the documented type.
        assert_eq!(unsafe { super::override_into(Some(&state), &mut slot) }, None);
        assert_eq!(slot.steam_id64(), REAL);
    }

    #[test]
    fn no_configured_override_leaves_steams_answer_standing() {
        let state = state(&[]);
        let mut slot = steam_slot();
        // SAFETY: `slot` is a live local of the documented type.
        assert_eq!(unsafe { super::override_into(Some(&state), &mut slot) }, None);
        assert_eq!(slot.steam_id64(), REAL);
    }

    #[test]
    fn a_configured_override_is_written_and_the_callers_pointer_is_returned() {
        let state = state(&[(APP, CONFIGURED)]);
        let mut slot = steam_slot();
        let caller = &mut slot as *mut CSteamId;
        // SAFETY: `caller` points at a live local of the documented type.
        assert_eq!(unsafe { super::override_into(Some(&state), caller) }, Some(caller));
        assert_eq!(
            slot.steam_id64(),
            CONFIGURED,
            "the out slot holds the override, not Steam's answer"
        );
    }

    #[test]
    fn a_null_out_pointer_is_neither_written_nor_dereferenced() {
        let state = state(&[(APP, CONFIGURED)]);
        // SAFETY: a null pointer is the one value the contract allows without
        // storage behind it, and the body returns before dereferencing it.
        let applied = unsafe { super::override_into(Some(&state), core::ptr::null_mut()) };
        assert_eq!(applied, None);
    }

    /// Steam's accessor as the corpus measures it: store the member, return the
    /// pointer. It bumps a counter so the test can prove the body called it first.
    unsafe extern "system" fn steam_accessor(
        _this: *mut c_void,
        out: *mut CSteamId,
    ) -> *mut CSteamId {
        ORIGINAL_CALLS.fetch_add(1, Ordering::SeqCst);
        if !out.is_null() {
            // SAFETY: the test passes a valid local slot.
            unsafe { CSteamId::write_at(out, CSteamId::new(REAL)) };
        }
        out
    }

    static ORIGINAL_CALLS: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn the_body_calls_steam_first_and_reports_the_callers_pointer() {
        // Both shell tests publish a trampoline for the real symbol, so they take
        // the process-global order lock rather than racing each other's
        // `record`/`forget`.
        let _guard = crate::test_lock::serial();
        original::record(SYMBOL, steam_accessor as GetSteamIdFn as usize);
        let mut slot = CSteamId::new(0);
        let caller = &mut slot as *mut CSteamId;
        let before = ORIGINAL_CALLS.load(Ordering::SeqCst);
        // SAFETY: the detour's two slots, with a `this` the real accessor would
        // accept: the fake reads neither `this` nor anything but `out`.
        let returned = unsafe { get_steam_id(core::ptr::null_mut(), caller) };
        original::forget(SYMBOL);

        assert!(
            ORIGINAL_CALLS.load(Ordering::SeqCst) > before,
            "the published original must run before any decision"
        );
        assert_eq!(
            returned, caller,
            "the accessor returns the pointer it was handed"
        );
        // Whether this is Steam's answer or the host's override depends on what
        // other tests published as the process-global state, so the value is not
        // asserted here: `override_into`'s own tests pin both answers hermetically.
        assert_ne!(slot.steam_id64(), 0, "the original filled the slot");
    }

    #[test]
    fn the_body_forwards_a_null_out_pointer_without_touching_it() {
        let _guard = crate::test_lock::serial();
        original::record(SYMBOL, steam_accessor as GetSteamIdFn as usize);
        let before = ORIGINAL_CALLS.load(Ordering::SeqCst);
        // SAFETY: a null out-pointer is a call shape the body must forward, not
        // repair; nothing is written through it.
        let returned = unsafe { get_steam_id(core::ptr::null_mut(), core::ptr::null_mut()) };
        original::forget(SYMBOL);
        assert_eq!(returned, core::ptr::null_mut());
        assert!(
            ORIGINAL_CALLS.load(Ordering::SeqCst) > before,
            "a null out-pointer is still Steam's call to answer"
        );
    }
}
