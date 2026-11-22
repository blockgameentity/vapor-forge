//! The body's decision table: serve a cached request code, or observe and pass.
//!
//! Everything here is a function of values the body *could* have. Pure on
//! purpose: the same table runs on a Linux test with synthetic inputs and inside
//! Steam on a live client, so what is proven in `cargo test` is the thing that
//! runs.
//!
//! The table is written for the *whole* interception, including the branch the
//! win64 corpus cannot reach yet:
//!
//! ```text
//! interception capability off            -> Pass                 (no reads at all)
//! request codes on, layout+target+cached -> Serve                (rewrite + restore)
//! otherwise                              -> Observe(blocker)     (record, fall through)
//! ```
//!
//! The `Serve` row is live now that `CNetPacket::{Alloc, Init, Release}` decode on
//! MS-x64: a decoded [`Layout`] plus a target plus a cached code is what the body in
//! [`super`] serves, with the packet's two slots swapped and restored around the
//! original call. [`ServeBlocker::NoLayout`] stays as the executable form of the
//! remaining gap: a host that decoded no layout can never serve, and
//! [`super::availability`] disables `request-codes` for it.

use crate::abi::netpacket::Layout;

use super::classify::{self, MessageClass};

/// The `(depot, gid)` a message is about.
///
/// `depot` is the depot id Steam asked a code for, `gid` the manifest id; both
/// are decoded from the request body in the Linux host
/// (`features::request_code::plan_fetch`). On Windows both would come out of the
/// packet payload, which is unreadable until the layout is anchored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestTarget {
    pub depot: u32,
    pub gid: u64,
}

/// A request code the host already holds for a `(depot, gid)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CachedCode {
    pub code: u64,
    /// Provider that served it, when the cache knows.
    pub provider: Option<&'static str>,
}

/// What happened to the cached-code lookup for this message: the first reason it
/// was not served, or [`Self::Served`] when it was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeBlocker {
    /// The `request-codes` capability is off: the host may look, not answer.
    CapabilityDisabled,
    /// No packet layout: the payload cannot be read or replaced. The corpus gap.
    NoLayout,
    /// No `(depot, gid)` for this message: nothing to look up.
    NoTarget,
    /// Nothing cached for this `(depot, gid)` yet.
    NoCachedCode,
    /// Not a blocker: the code *was* served, and Steam's payload was swapped for the
    /// duration of the dispatch and restored afterwards.
    Served,
}

impl ServeBlocker {
    /// One line for the log or the record, naming the cause rather than a code.
    pub fn reason(self) -> &'static str {
        match self {
            Self::CapabilityDisabled => {
                "the request-codes capability is disabled, so the host may observe but not answer"
            }
            Self::NoLayout => {
                "no CNetPacket layout was decoded for this build, so the packet payload \
                 cannot be read or replaced"
            }
            Self::NoTarget => "no (depot, gid) was readable for this message",
            Self::NoCachedCode => "no request code is cached for this (depot, gid)",
            Self::Served => "a cached request code was served for this message",
        }
    }
}

/// Everything the body knows about one inbound message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Inputs {
    /// `cm-interception` survived the install pass.
    pub cm_enabled: bool,
    /// `request-codes` survived the install pass.
    pub request_codes_enabled: bool,
    /// The packet layout, when a build's offsets have been decoded.
    pub layout: Option<Layout>,
    /// The message id, when the packet could be read.
    pub emsg: Option<u32>,
    /// The `(depot, gid)` the message is about.
    pub target: Option<RequestTarget>,
    /// A code already held for `target`.
    pub cached: Option<CachedCode>,
}

/// What one inbound message was.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    pub emsg: Option<u32>,
    pub class: Option<MessageClass>,
    pub target: Option<RequestTarget>,
    /// Why the host could not serve instead of observing.
    pub blocker: ServeBlocker,
}

/// What the body should do with one inbound message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Disposition {
    /// Touch nothing: the capability is off or the host is not initialised.
    Pass,
    /// Record what was seen and forward the message to Steam unchanged.
    Observe(Observation),
    /// Replace the payload with the cached code for the duration of the call and
    /// restore Steam's bytes afterwards.
    Serve { target: RequestTarget, code: u64 },
}

/// Decide one message.
pub fn decide(inputs: &Inputs) -> Disposition {
    if !inputs.cm_enabled {
        return Disposition::Pass;
    }
    let class = inputs.emsg.map(classify::classify);
    if let Some(blocker) = serve_blocker(inputs) {
        return Disposition::Observe(Observation {
            emsg: inputs.emsg,
            class,
            target: inputs.target,
            blocker,
        });
    }
    // Unreachable while `serve_blocker` returns `NoLayout` for every build this
    // corpus covers; the branch is the table's contract, not a live path.
    match (inputs.target, inputs.cached) {
        (Some(target), Some(cached)) => Disposition::Serve {
            target,
            code: cached.code,
        },
        _ => Disposition::Observe(Observation {
            emsg: inputs.emsg,
            class,
            target: inputs.target,
            blocker: ServeBlocker::NoCachedCode,
        }),
    }
}

/// The first reason a cached code cannot be served, in precedence order.
fn serve_blocker(inputs: &Inputs) -> Option<ServeBlocker> {
    if !inputs.request_codes_enabled {
        return Some(ServeBlocker::CapabilityDisabled);
    }
    if inputs.layout.is_none() {
        return Some(ServeBlocker::NoLayout);
    }
    if inputs.target.is_none() {
        return Some(ServeBlocker::NoTarget);
    }
    if inputs.cached.is_none() {
        return Some(ServeBlocker::NoCachedCode);
    }
    None
}

#[cfg(test)]
mod tests;
