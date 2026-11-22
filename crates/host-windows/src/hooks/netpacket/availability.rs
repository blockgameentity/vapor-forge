//! The one capability whose availability depends on a decode, and how the report
//! says so.
//!
//! `request-codes` means "answer Steam's manifest request-code round trip from a
//! code the host already holds". Answering needs two things, in this order:
//!
//! 1. a packet layout, so the payload can be read and replaced — decoded from
//!    `CNetPacket::{Alloc, Init, Release}` by `patterns::cnet_packet`, once per
//!    module after the install batch (see
//!    [`crate::hooks::netpacket::layout`]);
//! 2. a code in the cache — a producer this host does not have yet, which is why
//!    [`super::decision::ServeBlocker::NoCachedCode`] is the per-message blocker.
//!
//! Only the first is a capability question, and only the first can be answered
//! here: a host that decoded no layout can never serve, so the capability is
//! disabled with [`LAYOUT_GAP`]. A host that did decode one keeps the capability
//! and serves the moment a worker stores a code.
//!
//! [`settle`] is idempotent and never overwrites an install reason:
//! [`Gate::disable`] keeps the first reason, so if the dispatch symbol itself
//! failed to install, that failure is the more specific cause and stays.

use crate::abi::netpacket::Layout;
use crate::capability::{Capability, Gate};

/// Why `request-codes` is off even though the dispatch body is installed.
pub const LAYOUT_GAP: &str = "the packet layout could not be decoded for this Steam build: \
     CNetPacket::Init/Alloc/Release did not resolve in the corpus, or their bodies did not \
     decode to usable (data, size) slots, so a CM payload cannot be read or rewritten. The \
     dispatch is hooked and records what it sees; Steam's own request-code round trip is \
     unaffected.";

/// Disable `request-codes` when no layout was decoded, and leave it enabled when
/// one was.
pub fn settle(gate: &mut Gate, layout: Option<Layout>) {
    if layout.is_none() {
        gate.disable(Capability::RequestCodes, LAYOUT_GAP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_without_a_layout_disables_request_codes_and_nothing_else() {
        let mut gate = Gate::all_enabled();
        settle(&mut gate, None);
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
    fn a_decoded_layout_keeps_request_codes_enabled() {
        let mut gate = Gate::all_enabled();
        settle(&mut gate, Layout::new(0x10, 0x18));
        assert!(gate.is_enabled(Capability::RequestCodes));
        assert_eq!(gate.reason(Capability::RequestCodes), None);
        assert!(gate.report().contains("All Vapor Forge capabilities are active"));
    }

    #[test]
    fn an_install_reason_is_never_replaced_by_the_generic_gap() {
        let mut gate = Gate::all_enabled();
        gate.disable(Capability::RequestCodes, "hook CCMInterface::RecvPkt did not resolve");
        settle(&mut gate, None);
        assert_eq!(
            gate.reason(Capability::RequestCodes),
            Some("hook CCMInterface::RecvPkt did not resolve")
        );
    }

    #[test]
    fn settling_twice_is_the_same_as_settling_once() {
        let mut gate = Gate::all_enabled();
        settle(&mut gate, None);
        settle(&mut gate, None);
        assert_eq!(gate.reason(Capability::RequestCodes), Some(LAYOUT_GAP));
        assert!(gate.report().contains("request-codes: the packet layout could not be decoded"));
    }
}
