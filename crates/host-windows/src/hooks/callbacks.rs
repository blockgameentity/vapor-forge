//! Callback events — **decision ready, no anchor**.
//!
//! The Linux host registers Steam's internal callbacks
//! (`RegisterInternalCallback`) and reads API-call results (`SetAPICallResult`)
//! so it can deliver `AppMinutesPlayedDataNotice_t`, `AppInfoUpdateComplete_t`
//! and `UserStatsReceived_t` to its own consumers
//! (`crates/hooks/src/client/{internal_callbacks,callback_notify}.rs`, with the
//! contracts in `client/callback_dispatch.rs`). Three rules make that safe, and
//! they are the decision this module owns:
//!
//! 1. only a registered id is delivered — an unknown callback is not ours to
//!    interpret;
//! 2. the payload must be at least the registered size — a short one would be
//!    read past its end;
//! 3. nothing larger than [`MAX_INTERNAL_CALLBACK_PAYLOAD`] is accepted at all.
//!
//! # Why there is no body here
//!
//! Neither `RegisterInternalCallback` nor `SetAPICallResult` is in the win64
//! corpus: verified with the miner against both live builds. The symbols the
//! corpus does resolve look adjacent but are different functions —
//! `IPCProcessMessage` and `PchMsgNameFromEMsg` are the CM message path
//! (`Capability::CmInterception`), and a body there could only observe, not
//! deliver a callback.
//!
//! [`decide`] is the portable half: it is what a registration or delivery body
//! would call, and it is fully unit-tested here.

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// Largest internal-callback payload this host is willing to touch.
///
/// The Linux host's `MAX_INTERNAL_CALLBACK_PAYLOAD`, and for the same reason: the
/// registrations below are tens of bytes, so anything past this is a message we
/// have misidentified rather than one we should copy.
pub const MAX_INTERNAL_CALLBACK_PAYLOAD: usize = 32;

/// One callback id this host has a consumer for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Registration {
    /// The id Steam posts the callback with.
    pub id: i32,
    /// The name the corpus and the logs use.
    pub name: &'static str,
    /// Bytes the callback body carries; a shorter payload is a mis-read.
    pub payload_size: usize,
}

/// The registrations this host knows, taken from the Linux host's table.
pub const REGISTRATIONS: &[Registration] = &[
    Registration {
        id: 0x000f_908e,
        name: "AppMinutesPlayedDataNotice_t",
        payload_size: 4,
    },
    Registration {
        id: 1_270_003,
        name: "AppInfoUpdateComplete_t",
        payload_size: 12,
    },
    Registration {
        id: 1101,
        name: "UserStatsReceived_t",
        payload_size: 20,
    },
];

/// Why a callback was not delivered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DropReason {
    /// The `callback-events` capability did not survive the install pass.
    CapabilityDisabled,
    /// No registration matches the id.
    Unregistered,
    /// The payload is shorter than the registration promises.
    ShortPayload { expected: usize, received: usize },
    /// The payload is larger than this host attributes to an internal callback.
    OversizedPayload { received: usize, limit: usize },
}

impl DropReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::CapabilityDisabled => "capability-disabled",
            Self::Unregistered => "unregistered",
            Self::ShortPayload { .. } => "short-payload",
            Self::OversizedPayload { .. } => "oversized-payload",
        }
    }
}

/// What the host does with one callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// A registered id with a payload of the right size.
    Deliver,
    /// Not ours, or not safe to read. Steam is unaffected either way.
    Drop(DropReason),
}

/// The registration for `id`, when this host consumes that callback.
pub fn registration(id: i32) -> Option<&'static Registration> {
    REGISTRATIONS.iter().find(|entry| entry.id == id)
}

/// Decide whether one callback may be delivered.
pub fn decide(state: &HostState, id: i32, payload_len: usize) -> Delivery {
    if !state.allows(Capability::CallbackEvents) {
        return Delivery::Drop(DropReason::CapabilityDisabled);
    }
    let Some(entry) = registration(id) else {
        return Delivery::Drop(DropReason::Unregistered);
    };
    if payload_len > MAX_INTERNAL_CALLBACK_PAYLOAD {
        return Delivery::Drop(DropReason::OversizedPayload {
            received: payload_len,
            limit: MAX_INTERNAL_CALLBACK_PAYLOAD,
        });
    }
    if payload_len < entry.payload_size {
        return Delivery::Drop(DropReason::ShortPayload {
            expected: entry.payload_size,
            received: payload_len,
        });
    }
    Delivery::Deliver
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};

    fn state() -> HostState {
        state_with(config_with(&[730]), Vec::new())
    }

    #[test]
    fn every_registration_delivers_at_exactly_its_payload_size() {
        let state = state();
        for entry in REGISTRATIONS {
            assert_eq!(
                decide(&state, entry.id, entry.payload_size),
                Delivery::Deliver,
                "{} must deliver at its registered size",
                entry.name
            );
        }
    }

    #[test]
    fn a_short_payload_is_never_read_past_its_end() {
        let state = state();
        assert_eq!(
            decide(&state, 1_270_003, 11),
            Delivery::Drop(DropReason::ShortPayload {
                expected: 12,
                received: 11
            })
        );
    }

    #[test]
    fn an_empty_payload_is_short_not_delivered() {
        let state = state();
        assert!(matches!(
            decide(&state, 1101, 0),
            Delivery::Drop(DropReason::ShortPayload { .. })
        ));
    }

    #[test]
    fn an_unknown_id_is_not_ours_to_interpret() {
        let state = state();
        assert_eq!(
            decide(&state, 424_242, 8),
            Delivery::Drop(DropReason::Unregistered)
        );
    }

    #[test]
    fn an_oversized_payload_is_dropped_before_it_is_attributed() {
        let state = state();
        let delivery = decide(&state, 0x000f_908e, MAX_INTERNAL_CALLBACK_PAYLOAD + 1);
        assert_eq!(
            delivery,
            Delivery::Drop(DropReason::OversizedPayload {
                received: MAX_INTERNAL_CALLBACK_PAYLOAD + 1,
                limit: MAX_INTERNAL_CALLBACK_PAYLOAD,
            })
        );
        assert_eq!(
            match delivery {
                Delivery::Drop(reason) => reason.label(),
                Delivery::Deliver => "delivered",
            },
            "oversized-payload"
        );
    }

    #[test]
    fn a_disabled_capability_drops_every_callback() {
        let state = state();
        let mut gate = state.gate();
        gate.disable(Capability::CallbackEvents, "no anchor in the corpus");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, 1101, 20),
            Delivery::Drop(DropReason::CapabilityDisabled)
        );
    }
}
