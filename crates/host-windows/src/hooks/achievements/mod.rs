//! `CAPIJobRequestUserStats::YieldingMergeStats` — the stats-reply rewrite.
//!
//! # What the body does, in one sentence
//!
//! When the stats job reaches its completion point, find the reply body Steam is
//! about to merge, and — when the local rebuild produces a different achievement
//! payload — show Steam the rebuilt body for the duration of the original call and
//! then put its own bytes back.
//!
//! # The three halves
//!
//! * [`geometry`] is where the reply lives: `*(this + reply_field)` and the
//!   `(pointer, length)` pair at `reply_object + body`, both **decoded** from the
//!   build's own code. No structure offset is compiled here.
//! * [`reply`] decodes the body, derives the unlock state and the definitions, and
//!   re-encodes the rebuilt payload. Pure prost.
//! * [`swap`] owns the swap-and-restore of the reply's pair, dropped after the
//!   original call returns.
//!
//! [`cache`] holds the schemas replies have carried, because the rebuild answers a
//! reply that carries *none* — the same fallback order the Linux host uses.
//!
//! # Gating
//!
//! `achievements` capability and `[achievements] local_rebuild`, in that order, via
//! [`crate::features::achievements::decide`] — the feature's own entry point, so the
//! flag cannot be bypassed. Every refusal is a named [`ReplyOutcome`], logged once
//! per merge at `debug`.
//!
//! # When the rewrite fires, and when it does not
//!
//! The geometry comes from `patterns::job_fields::decode_job_reply` (via
//! [`geometry::CorpusDecoder`]), published once per module by the worker. A merge
//! whose build decoded none records [`ReplyOutcome::NoGeometry`]; one whose reply
//! carries a schema keeps the schema path authoritative; one that names no app
//! records [`reply::RewriteBlocker::NoApp`]; one whose app has no remembered schema
//! records [`ReplyOutcome::NothingToRebuild`]. Every one of those forwards Steam's
//! reply untouched.
//!
//! # What this body never does
//!
//! * It never dereferences `this` before the gate and the geometry have both passed.
//! * It never allocates while the pair is swapped, never dials and never holds a lock
//!   across the original call.
//! * It writes exactly two slots of Steam's reply object, only when a rebuilt payload
//!   differs, and restores both unconditionally in `Drop`.

pub mod cache;
pub mod geometry;
pub mod reply;
pub mod serve;
pub mod swap;

use core::ffi::c_void;

use tracing::{debug, error};

use crate::abi::achievements::{MergeStatsFn, MERGE_EARLY_RETURN, MERGE_SYMBOL};
use crate::hooks::original;

use crate::features::achievements::RebuildPlan;

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = MERGE_SYMBOL;

/// Address of [`merge_stats`], for the installer's replacement table.
pub fn address() -> usize {
    merge_stats as MergeStatsFn as usize
}

/// What the observed `EResult` means for the merge that is about to run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MergeOutcome {
    /// The original merges the parsed reply into the client's store.
    Merges,
    /// The original returns at its dispatch, leaving the store alone.
    Refused,
}

impl MergeOutcome {
    /// Classify `eresult` exactly as the anchored entry does
    /// ([`MERGE_EARLY_RETURN`], measured from that entry's own dispatch).
    pub fn of(eresult: i32) -> Self {
        if MERGE_EARLY_RETURN.contains(&eresult) {
            Self::Refused
        } else {
            Self::Merges
        }
    }

    /// One word for the log line.
    pub fn label(self) -> &'static str {
        match self {
            Self::Merges => "merges",
            Self::Refused => "refused",
        }
    }
}

/// What happened to the reply body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReplyOutcome {
    /// The capability or the flag is off: Steam's reply is forwarded untouched.
    Gated,
    /// No reply geometry was decoded for this build.
    NoGeometry(&'static str),
    /// The reply could not be read, decoded or swapped, with the reason.
    Unreadable(String),
    /// The decision refused, or the rebuild produced what Steam already sent.
    NothingToRebuild,
    /// A rebuilt body was shown to the original call and restored afterwards.
    Served { bytes: usize },
}

/// One look at a stats merge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Observation {
    /// The `EResult` the merge was handed, forwarded verbatim to the original.
    pub eresult: i32,
    /// What that value does to the merge.
    pub outcome: MergeOutcome,
    /// What the local-rebuild decision answered for this reply.
    pub plan: RebuildPlan,
    /// What happened to the reply bytes.
    pub reply: ReplyOutcome,
}

impl Observation {
    /// Whether the host showed Steam a rebuilt reply.
    pub fn rebuilds(&self) -> bool {
        matches!(self.reply, ReplyOutcome::Served { .. })
    }
}

/// Replacement for `YieldingMergeStats`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry, so `this` and the
/// three argument slots must be exactly what Steam passed.
pub unsafe extern "system" fn merge_stats(this: *mut c_void, eresult: i32, r8: usize, r9: usize) {
    // SAFETY: the signature is the one documented in `crate::abi::achievements`,
    // and the trampoline was published for this symbol before the entry was patched.
    let Some(forward) = (unsafe { original::original_of::<MergeStatsFn>(SYMBOL) }) else {
        // Nothing can merge without the displaced instructions, and a merge that
        // silently did not happen is worse than a loud line: report and return.
        error!(
            symbol = SYMBOL,
            eresult, "achievements hook reached without a published original"
        );
        return;
    };
    // The swap guard is created before the call and dropped after it, so restoring
    // Steam's bytes is `Drop`, not a branch: no early return can skip it.
    let (swap, observation) = match crate::hooks::state::get() {
        Some(state) => {
            // SAFETY: `this` is the receiver Steam passed to this method, and the
            // geometry `prepare` reads came from this build's own code.
            let prepared = unsafe { serve::prepare(state, this, eresult) };
            (prepared.swap, Some(prepared.observation))
        }
        None => (None, None),
    };
    // SAFETY: every slot is forwarded verbatim, exactly as documented.
    unsafe { forward(this, eresult, r8, r9) };
    // Steam's own reply bytes go back before its caller can release the job object.
    drop(swap);
    // Logged after the restore: formatting may allocate, and nothing allocates while
    // Steam's reply object points at the replacement.
    if let Some(observation) = observation {
        debug!(
            eresult = observation.eresult,
            merge = observation.outcome.label(),
            plan = observation.plan.summary().as_str(),
            rebuilds = observation.rebuilds(),
            reply = serve::describe(&observation.reply).as_str(),
            "achievements: stats merge observed"
        );
    }
}

#[cfg(test)]
mod tests;
