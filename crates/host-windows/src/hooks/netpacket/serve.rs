//! One inbound message, prepared: what was seen, and what to swap in.
//!
//! The body in [`super`] calls [`prepare`], forwards the original `RecvPkt`, and
//! then drops whatever [`Prepared`] gave it. Everything that can fail happens
//! here, before a single byte of Steam's packet is touched:
//!
//! ```text
//! capability off                  -> Prepared::pass()                (no reads)
//! no layout                       -> observe(NoLayout)               (no reads)
//! payload unreadable / no target   -> observe(NoTarget)               (no writes)
//! target, nothing cached          -> observe(NoCachedCode)           (no writes)
//! target, cached code             -> observe(none) + swap            (swap + restore)
//! ```
//!
//! # Ordering, because the body runs on Steam's thread
//!
//! The layout and the code cache are both read *before* the swap is installed and
//! released before the original call, and the payload is copied into a `Vec`
//! before it is parsed: no lock and no Steam pointer outlives this function. The
//! swap itself owns its bytes and restores them on drop; see [`super::swap`].

use core::ffi::c_void;

use crate::abi::netpacket::Layout;
use crate::capability::Capability;
use crate::hooks::state::HostState;

use super::decision::{self, Disposition, Inputs, Observation, RequestTarget, ServeBlocker};
use super::{payload, swap::PacketSwap};

/// What the body must do for one message.
pub struct Prepared {
    /// The record to append, or `None` when the capability is off.
    pub observation: Option<Observation>,
    /// The swap to keep installed across the original call.
    pub swap: Option<PacketSwap>,
}

impl Prepared {
    fn pass() -> Self {
        Self {
            observation: None,
            swap: None,
        }
    }
}

/// Read, decide, and (when a code is cached) swap the payload.
///
/// # Safety
/// `packet` must be null or a live `CNetPacket` of the build whose layout this
/// process decoded. When the returned [`Prepared`] carries a swap, it must be
/// dropped before Steam's caller releases the packet.
pub unsafe fn prepare(state: &HostState, packet: *mut c_void) -> Prepared {
    if !state.allows(Capability::CmInterception) {
        return Prepared::pass();
    }
    let layout = state.layout();
    // Every read of Steam's memory happens here, behind the layout and behind one
    // null check. A missing layout means "not readable", which the decision table
    // reports as `NoLayout` rather than the body guessing.
    // SAFETY: the caller guarantees `packet` is null or a live CNetPacket of the
    // layout the state published.
    let read = unsafe { read_payload(packet, layout) };
    let cached = read
        .target
        .and_then(|target| state.request_codes().get(target));
    let inputs = Inputs {
        cm_enabled: true,
        request_codes_enabled: state.allows(Capability::RequestCodes),
        layout,
        // The message id comes from an accessor this corpus does not anchor (see
        // `crate::abi::netpacket`), so nothing here claims to know it.
        emsg: None,
        target: read.target,
        cached,
    };
    match decision::decide(&inputs) {
        Disposition::Pass => Prepared::pass(),
        Disposition::Observe(observation) => Prepared {
            observation: Some(refine(observation, read.cause)),
            swap: None,
        },
        Disposition::Serve { target, code } => {
            let Some(layout) = layout else {
                // `decide` returns `NoLayout` before it can serve, so this is a
                // table bug rather than a message: record it and touch nothing.
                return Prepared {
                    observation: Some(observation_of(target, ServeBlocker::NoLayout)),
                    swap: None,
                };
            };
            let body = payload::response_body(code);
            // Logged before the swap is installed: a subscriber may format and
            // allocate, and nothing allocates once Steam's slots point at the
            // replacement.
            tracing::debug!(
                depot = target.depot,
                gid = target.gid,
                code,
                served = body.len(),
                provider = cached.and_then(|cached| cached.provider).unwrap_or("cache"),
                "netpacket: a cached request code is served for the duration of the dispatch"
            );
            // SAFETY: the same live packet the payload was read from; the caller
            // drops the swap after the original call returns.
            let swap = unsafe { PacketSwap::install(packet, layout, body) };
            Prepared {
                observation: Some(observation_of(target, ServeBlocker::Served)),
                swap: Some(swap),
            }
        }
    }
}

/// What one read of the packet produced.
#[derive(Clone, Copy, Debug, Default)]
struct Read {
    target: Option<RequestTarget>,
    /// Why the payload could not be turned into a target, when it could not.
    cause: Option<PayloadCause>,
}

/// Why the payload read failed, kept apart from [`ServeBlocker`] so the record
/// names the parse cause rather than collapsing every failure into `NoTarget`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PayloadCause {
    /// No layout was decoded, so no byte was read.
    NoLayout,
    /// The packet pointer was null.
    NullPacket,
    /// The layout's data slot was null or its size slot was zero.
    Empty,
    /// The body did not parse as a request, or named no target.
    Unreadable,
}

impl PayloadCause {
    /// The blocker the record carries for this cause.
    fn blocker(self) -> ServeBlocker {
        match self {
            Self::NoLayout => ServeBlocker::NoLayout,
            Self::NullPacket | Self::Empty | Self::Unreadable => ServeBlocker::NoTarget,
        }
    }

    /// One line describing what the host could not read.
    fn reason(self) -> &'static str {
        match self {
            Self::NoLayout => "no packet layout was decoded for this build",
            Self::NullPacket => "the dispatch was reached with a null packet",
            Self::Empty => "the packet carries no payload bytes",
            Self::Unreadable => {
                "the payload is not a GetManifestRequestCodeRequest naming a depot and a manifest"
            }
        }
    }
}

/// Read the packet's payload and turn it into a target.
///
/// # Safety
/// `packet` must be null or a live `CNetPacket` of `layout`'s build.
unsafe fn read_payload(packet: *mut c_void, layout: Option<Layout>) -> Read {
    let Some(layout) = layout else {
        return Read {
            target: None,
            cause: Some(PayloadCause::NoLayout),
        };
    };
    if packet.is_null() {
        return Read {
            target: None,
            cause: Some(PayloadCause::NullPacket),
        };
    }
    // SAFETY: the caller guarantees `packet` is a live CNetPacket of this layout;
    // `Layout::new` guaranteed both slots are aligned and disjoint inside it.
    let (data, size) = unsafe {
        (
            *(packet.cast::<u8>().add(layout.data).cast::<*const u8>()),
            *(packet.cast::<u8>().add(layout.size).cast::<u32>()),
        )
    };
    if data.is_null() || size == 0 {
        return Read {
            target: None,
            cause: Some(PayloadCause::Empty),
        };
    }
    // SAFETY: Steam's packet supplies a non-null pointer and its own byte count.
    let bytes = unsafe { std::slice::from_raw_parts(data, size as usize) };
    match payload::target(bytes) {
        Ok(target) => Read {
            target: Some(target),
            cause: None,
        },
        Err(blocker) => {
            tracing::trace!(
                size,
                reason = blocker.reason().as_str(),
                "netpacket: payload could not be read as a request-code request"
            );
            Read {
                target: None,
                cause: Some(PayloadCause::Unreadable),
            }
        }
    }
}

fn observation_of(target: RequestTarget, blocker: ServeBlocker) -> Observation {
    Observation {
        emsg: None,
        class: None,
        target: Some(target),
        blocker,
    }
}

/// Keep the decision's blocker, but let a more specific read cause replace the
/// catch-all `NoTarget` it reports when nothing was readable.
fn refine(mut observation: Observation, cause: Option<PayloadCause>) -> Observation {
    if observation.blocker == ServeBlocker::NoTarget {
        if let Some(cause) = cause {
            tracing::trace!(cause = cause.reason(), "netpacket: nothing was served");
            observation.blocker = cause.blocker();
        }
    }
    observation
}

#[cfg(test)]
mod tests;
