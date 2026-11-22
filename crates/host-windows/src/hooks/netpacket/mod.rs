//! `CCMInterface::RecvPkt` — the request-code hook body, and its gap.
//!
//! # What the body does, in one sentence
//!
//! When Steam dispatches an inbound connection-manager message, decide whether a
//! cached manifest request code could be served for it — and, because this corpus
//! provides no packet layout, record the message as *unreadable with the cause
//! named* and forward it to Steam untouched.
//!
//! # Which scope this is, and why
//!
//! The Linux reference (`crates/hooks/src/client/network.rs::hk_recv_pkt` →
//! `netpacket::prepare_recv_packet`) does four things: discover the packet layout
//! at runtime, decide pass/drop/rewrite, keep rewritten bytes alive across the
//! original call, and restore Steam's payload afterwards. Porting it required
//! answering one question first: is that layout discovery portable?
//!
//! It is portable as *code* — `patterns::cnet_packet::decode_init` takes a bitness
//! and code bytes, so nothing in it is Linux-specific — but it is **not available
//! on this corpus**, and that is the honest answer:
//!
//! * the offsets are decoded from `CNetPacket::Init`; those three symbols
//!   (`Init`, `Alloc`, `Release`) are in `res/patterns/x86_64.toml` (Linux) and in
//!   **neither** win64 corpus file;
//! * `res/patterns/win64/VTABLE-REPORT.md` §3 measures why: the packet classes'
//!   only route to the payload is a virtual call, and no packet-class vtable slot
//!   is dispatch shaped, so the dispatch lives one class up at
//!   `CCMInterface::RecvPkt` — the symbol this body hooks;
//! * the EMsg is not an argument either: the dispatch obtains it through an
//!   unanchored deserialising accessor (see [`crate::abi::netpacket`]).
//!
//! So the body is written to the second scope: gate → classify → record the
//! `(EMsg, depot, gid)` it could read into the host state → answer from a cached
//! code **if one exists and the layout exists** → otherwise fall through. The
//! missing capability is not a silent no-op: [`availability`] disables
//! `request-codes` at install time with the corpus gap as its reason, and every
//! record carries [`decision::ServeBlocker::NoLayout`].
//!
//! There is no request-code cache in the host state either, and that is not an
//! omission: `features::request_code::PendingQueue::queue_fetch` needs a decoded
//! `CMsgProtoBufHeader` (a `vapor_forge_steam_protocol` type this crate does not
//! depend on), and the body it would decode is exactly the payload this build
//! cannot read. A cache nobody can fill would be a second silent no-op, so the
//! seam is [`decision::Inputs::cached`] — typed, tested, and empty.
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
//! `cm-interception` also depends on four outbound/pipe symbols
//! (`BBuildAndAsyncSendFrame`, `IPCProcessMessage`, `GetPipeClient`,
//! `PchMsgNameFromEMsg`) that have no bodies on this host yet, so a live install
//! reports it disabled with *their* reason and this body passes everything
//! through. That is the existing contract of the capability, not something this
//! module changed: the moment those bodies land, this body starts recording
//! without an edit.
//!
//! # What this body never does
//!
//! * It never dereferences the packet. No layout, no accessor: every read here is
//!   a read of *host* state, never of Steam's memory.
//! * It never allocates unbounded or blocks: the ledger is a 32-entry ring behind
//!   one mutex, and nothing on this path dials, resolves or sleeps. A provider
//!   fetch would have to be queued to a worker, and it is not attempted here.

pub mod availability;
pub mod classify;
pub mod decision;
pub mod ledger;
pub mod lookup;

use core::ffi::c_void;

use tracing::{debug, trace};

use crate::abi::netpacket::RecvPktFn;
use crate::capability::Capability;
use crate::hooks::{original, state};

use decision::{Disposition, Inputs, Observation, ServeBlocker};

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
/// `packet` must be whatever Steam passed. Neither is dereferenced.
pub unsafe extern "system" fn recv_pkt(this: *mut c_void, packet: *mut c_void) {
    // SAFETY: the type passed here is the one documented in `crate::abi::netpacket`;
    // the trampoline was published for this symbol before its entry was patched.
    let Some(original) = (unsafe { original::original_of::<RecvPktFn>(SYMBOL) }) else {
        debug!("netpacket hook reached without a published original; falling through to Steam");
        return;
    };
    observe(packet.is_null());
    // SAFETY: forwarding this callback's unchanged receiver and packet pointer.
    unsafe { original(this, packet) }
}

/// Record one dispatch and say, once, what the host could not read.
fn observe(packet_is_null: bool) {
    let Some(state) = state::get() else {
        return;
    };
    if !state.allows(Capability::CmInterception) {
        return;
    }
    // The message id and the (depot, gid) would come out of the packet payload.
    // Neither is readable: `crate::abi::netpacket` names the two missing anchors.
    // Passing them as `None` keeps the gap visible in the record instead of
    // pretending the message carried nothing.
    let inputs = Inputs {
        cm_enabled: true,
        request_codes_enabled: state.allows(Capability::RequestCodes),
        layout: None,
        emsg: None,
        target: None,
        cached: None,
    };
    let observation = match decision::decide(&inputs) {
        Disposition::Pass => return,
        Disposition::Observe(observation) => observation,
        Disposition::Serve { target, code } => {
            // Unreachable without a layout (`decision::decide` returns `NoLayout`
            // first). If it ever fires, the payload must not be left rewritten:
            // nothing here holds a guard, so do not serve and do not lie.
            debug!(
                depot = target.depot,
                gid = target.gid,
                code,
                "netpacket: a serve was decided without a swap guard; falling through instead"
            );
            return;
        }
    };
    let sequence = state.netpacket().record(observation);
    log(state, sequence, &observation, packet_is_null);
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
        // Once, and at `debug` like every other hook body here: it proves the
        // hook is reached and names the gap. Everything after this is
        // per-message noise.
        debug!(
            capability = Capability::CmInterception.label(),
            blocker,
            request_codes = state.allows(Capability::RequestCodes),
            "netpacket: inbound CM dispatch observed; the payload is unreadable for this build"
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

/// Whether a cached code could be served for this host at all.
///
/// Exposed for the install report, which must not claim a capability it cannot
/// honour: `None` means "no layout, no serving", and the reason is
/// [`ServeBlocker::NoLayout`].
pub fn serving_gap() -> Option<ServeBlocker> {
    Some(ServeBlocker::NoLayout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_is_registered_under_its_anchored_name() {
        assert_eq!(SYMBOL, "CCMInterface::RecvPkt");
        assert_ne!(address(), 0);
    }

    #[test]
    fn a_body_reached_without_an_original_falls_through_without_panicking() {
        // No trampoline is published in this process, which is the "called before
        // initialisation" contract: the body must return, not unwind.
        // SAFETY: the body returns before touching either pointer; neither is
        // dereferenced on any path (see the module docs).
        unsafe { recv_pkt(core::ptr::null_mut(), core::ptr::null_mut()) };
    }

    #[test]
    fn serving_is_always_blocked_by_the_layout_gap() {
        assert_eq!(serving_gap(), Some(ServeBlocker::NoLayout));
    }
}
