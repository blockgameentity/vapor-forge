//! `CCMInterface::RecvPkt` — the request-code serve body.
//!
//! # What the body does, in one sentence
//!
//! When Steam dispatches an inbound connection-manager message, read the payload
//! through the decoded packet layout, parse it as a manifest request-code request,
//! and — when the host already holds a code for that `(depot, gid)` — show Steam
//! the answer for the duration of the original call and then put its own bytes
//! back.
//!
//! # The three halves
//!
//! * [`payload`] parses a request body and encodes the response body. Pure.
//! * [`cache`] holds the `(depot, gid)` → code table. Bounded, no I/O.
//! * [`swap`] owns the swap-and-restore of the packet's `(data, size)` slots, which
//!   is the same discipline as the Linux host's
//!   `netpacket::boundary::PacketSwapGuard`.
//!
//! [`serve`] glues them for one message and is the only thing the FFI body calls
//! besides the displaced original.
//!
//! # What is anchored, and what is still missing
//!
//! The layout comes from the corpus — `CNetPacket::{Alloc, Init, Release}` are
//! decoded by `patterns::cnet_packet` (MS-x64) once per module in
//! [`crate::entry::worker`], so `(data, size)` are the build's own offsets and no
//! constant is compiled here. Two things are still missing, and each is a named
//! blocker rather than a silent no-op:
//!
//! 1. **The `EMsg`.** The dispatch obtains the message id through an accessor no
//!    corpus pattern anchors (see [`crate::abi::netpacket`]), so the host cannot
//!    ask "is this the request-code message". It asks the conservative opposite
//!    instead: does this body parse as that request, and does it name a target. A
//!    payload that does not parse is left alone, and every record says
//!    [`ServeBlocker::NoTarget`].
//! 2. **A producer for the cache.** Nothing on this host fills it yet: a provider
//!    round trip belongs on a worker, and this host has no request-code worker.
//!    So a live client records [`ServeBlocker::NoCachedCode`] for every message it
//!    can read — the cache is the seam a worker fills, not a second silent no-op.
//!    The body never dials: [`serve::prepare`] only ever reads.
//!
//! # Gating
//!
//! Two capabilities, consulted in this order and never merged:
//!
//! * `cm-interception` — may the host look at CM traffic at all? Off means the
//!   body is a plain fall-through: it does not read the packet, does not record,
//!   does not touch the ledger.
//! * `request-codes` — may the host answer Steam's round trip locally? Off means
//!   the host may observe, never serve.
//!
//! [`availability`] disables `request-codes` when no layout could be decoded, with
//! the corpus gap as the reason: a capability that cannot be honoured must say so.
//!
//! # What this body never does
//!
//! * It never allocates while the swap is installed, never dials, and never holds
//!   a lock across the original call.
//! * It writes exactly two slots of Steam's packet, and only when a code is
//!   served; both are restored by the swap guard before the body returns.

pub mod availability;
pub mod cache;
pub mod classify;
pub mod decision;
pub mod layout;
pub mod ledger;
pub mod lookup;
pub mod payload;
pub mod serve;
pub mod swap;

use core::ffi::c_void;

use tracing::{debug, trace};

use crate::abi::netpacket::RecvPktFn;
use crate::capability::Capability;
use crate::hooks::{original, state};

use decision::{Observation, ServeBlocker};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CCMInterface::RecvPkt";

/// Address of [`recv_pkt`], for the installer's replacement table.
pub fn address() -> usize {
    recv_pkt as RecvPktFn as usize
}

/// Replacement for `CCMInterface::RecvPkt`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this` and
/// `packet` must be whatever Steam passed.
pub unsafe extern "system" fn recv_pkt(this: *mut c_void, packet: *mut c_void) {
    // SAFETY: the type passed here is the one documented in `crate::abi::netpacket`;
    // the trampoline was published for this symbol before its entry was patched.
    let Some(original) = (unsafe { original::original_of::<RecvPktFn>(SYMBOL) }) else {
        debug!("netpacket hook reached without a published original; falling through to Steam");
        return;
    };
    // The swap guard is created before the call and dropped after it. Restoring is
    // therefore unconditional: it is `Drop`, not a branch, so no early return in
    // this body can leave Steam's payload rewritten.
    let swap = match state::get() {
        Some(state) => {
            // SAFETY: `packet` is the pointer Steam passed to this dispatch.
            let prepared = unsafe { serve::prepare(state, packet) };
            if let Some(observation) = prepared.observation {
                let sequence = state.netpacket().record(observation);
                log(state, sequence, &observation, packet.is_null());
            }
            prepared.swap
        }
        None => None,
    };
    // SAFETY: forwarding this callback's unchanged receiver and packet pointer.
    unsafe { original(this, packet) };
    // Steam's own bytes go back before its caller can release the packet.
    drop(swap);
}

fn log(state: &state::HostState, sequence: u64, observation: &Observation, packet_is_null: bool) {
    if packet_is_null {
        debug!(
            sequence,
            "netpacket: dispatch reached with a null packet; nothing could be read"
        );
        return;
    }
    let blocker = observation.blocker.reason();
    if sequence == 1 {
        // Once, and at `debug` like every other hook body here: it proves the hook
        // is reached and names the first reason a code was not served.
        debug!(
            capability = Capability::CmInterception.label(),
            blocker,
            layout = state.layout().is_some(),
            request_codes = state.allows(Capability::RequestCodes),
            "netpacket: inbound CM dispatch observed"
        );
        let plan = lookup::plan(
            state.config(),
            state.depot().wanted(),
            state.controlled_apps(),
        );
        if plan.is_empty() {
            debug!("netpacket: no wanted manifest and no controlled app, so no lookup to describe");
        } else {
            debug!(lookup = plan.summary().as_str(), "netpacket: request-code lookup");
        }
    } else {
        trace!(
            sequence,
            blocker,
            total = state.netpacket().total(),
            dropped = state.netpacket().dropped(),
            "netpacket: inbound CM dispatch observed"
        );
    }
}

/// Why the host would not serve a code today, given what it has decoded.
///
/// Exposed for the install report, which must not claim a capability it cannot
/// honour: `None` means "a layout exists, so the body can serve as soon as the
/// cache holds a code".
pub fn serving_gap(state: &state::HostState) -> Option<ServeBlocker> {
    if state.layout().is_none() {
        return Some(ServeBlocker::NoLayout);
    }
    if state.request_codes().is_empty() {
        return Some(ServeBlocker::NoCachedCode);
    }
    None
}

/// The decision an uninitialised host would take, for the tests that assert the
/// gate order without a live packet.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::netpacket::decision::{CachedCode, Disposition, Inputs, RequestTarget};
    use crate::hooks::tests_support::{config_with, state_with};

    #[test]
    fn the_body_is_registered_under_its_anchored_name() {
        assert_eq!(SYMBOL, "CCMInterface::RecvPkt");
        assert_ne!(address(), 0);
    }

    #[test]
    fn a_body_reached_without_an_original_falls_through_without_panicking() {
        // No trampoline is published in this process, which is the "called before
        // initialisation" contract: the body must return, not unwind.
        // SAFETY: the body returns before touching either pointer.
        unsafe { recv_pkt(core::ptr::null_mut(), core::ptr::null_mut()) };
    }

    #[test]
    fn the_gate_order_is_disabled_first_then_serve() {
        // `cm-interception` off is a plain pass, whatever else is available.
        assert_eq!(
            decision::decide(&Inputs {
                cm_enabled: false,
                request_codes_enabled: true,
                layout: None,
                emsg: None,
                target: None,
                cached: None,
            }),
            Disposition::Pass
        );
    }

    #[test]
    fn a_host_with_no_layout_reports_the_layout_gap() {
        let state = state_with(config_with(&[480]), Vec::new());
        assert_eq!(serving_gap(&state), Some(ServeBlocker::NoLayout));
    }

    #[test]
    fn a_host_with_a_layout_but_no_codes_reports_the_missing_code() {
        let state = state_with(config_with(&[480]), Vec::new());
        state.set_layout(crate::abi::netpacket::Layout::new(16, 24).expect("valid"));
        assert_eq!(serving_gap(&state), Some(ServeBlocker::NoCachedCode));

        state.request_codes().insert(
            RequestTarget { depot: 731, gid: 1 },
            CachedCode {
                code: 7,
                provider: None,
            },
        );
        assert_eq!(serving_gap(&state), None, "a layout and a code is a servable host");
    }
}
