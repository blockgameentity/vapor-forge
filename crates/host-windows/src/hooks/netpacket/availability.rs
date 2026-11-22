//! The one capability this corpus cannot serve, and the reason the report gives.
//!
//! `request-codes` means "answer Steam's manifest request-code round trip
//! locally". The body in [`super`] is installed at the dispatch site, so the
//! *interception* half exists — but answering needs a packet layout, and no
//! win64 corpus file anchors `CNetPacket::Init`, `CNetPacket::Alloc` or
//! `CNetPacket::Release` (both live builds; see [`crate::abi::netpacket`]).
//!
//! A capability that cannot be honoured must say so. [`apply`] runs immediately
//! after the install pass and disables exactly `request-codes`, leaving every
//! other capability to the symbols that decide it. It is idempotent and it never
//! overwrites an install reason: [`Gate::disable`] keeps the first reason, and if
//! the dispatch symbol itself failed to install, that failure is the more
//! specific cause and stays.

use crate::capability::{Capability, Gate};

/// Why `request-codes` is off even though the dispatch body is installed.
pub const LAYOUT_GAP: &str = "the packet layout is not anchored for this Steam build: \
     CNetPacket::Init/Alloc/Release are in the Linux corpus only, so a CM response cannot be \
     read or rewritten. The dispatch is hooked and records what it sees; Steam's own \
     request-code round trip is unaffected.";

/// Disable `request-codes` with [`LAYOUT_GAP`], unless something more specific
/// already disabled it.
pub fn apply(gate: &mut Gate) {
    gate.disable(Capability::RequestCodes, LAYOUT_GAP);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applying_the_gap_disables_request_codes_and_nothing_else() {
        let mut gate = Gate::all_enabled();
        apply(&mut gate);
        assert!(!gate.is_enabled(Capability::RequestCodes));
        assert_eq!(gate.reason(Capability::RequestCodes), Some(LAYOUT_GAP));
        // Interception itself survives: the body is installed and records.
        assert!(gate.is_enabled(Capability::CmInterception));
        for capability in Capability::ALL {
            if capability != Capability::RequestCodes {
                assert!(gate.is_enabled(capability), "{capability} must stay enabled");
            }
        }
    }

    #[test]
    fn an_install_reason_is_never_replaced_by_the_generic_gap() {
        let mut gate = Gate::all_enabled();
        gate.disable(Capability::RequestCodes, "hook CCMInterface::RecvPkt did not resolve");
        apply(&mut gate);
        assert_eq!(
            gate.reason(Capability::RequestCodes),
            Some("hook CCMInterface::RecvPkt did not resolve")
        );
    }

    #[test]
    fn applying_twice_is_the_same_as_applying_once() {
        let mut gate = Gate::all_enabled();
        apply(&mut gate);
        apply(&mut gate);
        assert_eq!(gate.reason(Capability::RequestCodes), Some(LAYOUT_GAP));
        assert!(gate.report().contains("request-codes: the packet layout is not anchored"));
    }
}
