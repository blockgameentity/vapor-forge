//! One stats merge, prepared: what the reply said, and what to swap in.
//!
//! The body in [`super`] calls [`prepare`], forwards the displaced original, and
//! drops whatever it got. The order is deliberate — every gate and every failure is
//! decided before Steam's memory is touched:
//!
//! ```text
//! capability off / flag off / switch off  -> gated, no read
//! no reply geometry                       -> no read
//! null job / null or empty reply          -> no read
//! body is not one of the two reply shapes  -> no read further, no write
//! reply names no app                      -> cached if it carried a schema, no write
//! decision refuses                        -> cached/blocks, no write
//! rebuilt payload differs                 -> swap + restore
//! ```
//!
//! The schema a reply carries is remembered even when nothing is rewritten: that is
//! what lets a later schema-less reply for the same app be rebuilt, which is the
//! order the Linux host's cache works in too.

use core::ffi::c_void;

use crate::capability::Capability;
use crate::features::achievements::{decide_rebuild, RebuildPlan, RebuildRequest};
use crate::hooks::state::HostState;

use super::geometry::{self, JobReply};
use super::reply::{self, Shape};
use super::swap::ReplySwap;
use super::{MergeOutcome, Observation, ReplyOutcome};

/// What the body must do for one merge.
pub struct Prepared {
    /// The record to log: the `EResult`, what it means, the decision and the reply.
    pub observation: Observation,
    /// The swap to keep installed across the original call.
    pub swap: Option<ReplySwap>,
}

fn observation(eresult: i32, plan: RebuildPlan, reply: ReplyOutcome) -> Observation {
    Observation {
        eresult,
        outcome: MergeOutcome::of(eresult),
        plan,
        reply,
    }
}

/// Read, decide, and (when a rebuilt payload is produced) swap the reply body.
///
/// # Safety
/// `this` must be a live `CAPIJobRequestUserStats` of the build whose geometry this
/// process decoded. When the result carries a swap, it must be dropped before
/// Steam's caller releases the object.
pub unsafe fn prepare(state: &HostState, this: *mut c_void, eresult: i32) -> Prepared {
    let capability_reason = state.disabled_reason(Capability::Achievements);
    let settings = state.achievement_settings();
    // The gate first, with an empty request: capability, flag and the process-wide
    // switch are all decided before a byte of Steam's memory is read.
    let gate = decide_rebuild(capability_reason.as_deref(), settings, &RebuildRequest::default());
    if matches!(
        gate,
        RebuildPlan::CapabilityDisabled(_) | RebuildPlan::FlagOff | RebuildPlan::SwitchOff
    ) {
        return Prepared {
            observation: observation(eresult, gate, ReplyOutcome::Gated),
            swap: None,
        };
    }
    let Some(geometry) = state.reply_geometry() else {
        return Prepared {
            observation: observation(eresult, gate, ReplyOutcome::NoGeometry(geometry::NO_GEOMETRY)),
            swap: None,
        };
    };
    // SAFETY: the caller guarantees `this` is a live job object of this geometry.
    let Some(body) = (unsafe { read_body(this, geometry) }) else {
        return Prepared {
            observation: observation(
                eresult,
                gate,
                ReplyOutcome::Unreadable(
                    "the job holds no reply object, or the reply body is empty".to_owned(),
                ),
            ),
            swap: None,
        };
    };
    let shape = match reply::classify(&body) {
        Ok(shape) => shape,
        Err(blocker) => {
            return Prepared {
                observation: observation(
                    eresult,
                    gate,
                    ReplyOutcome::Unreadable(blocker.reason()),
                ),
                swap: None,
            };
        }
    };
    let app_id = match reply::app_id(&shape) {
        Ok(app_id) => app_id,
        Err(blocker) => {
            // No app means no key to remember the schema under: a reply that names
            // no app teaches this host nothing.
            return Prepared {
                observation: observation(
                    eresult,
                    gate,
                    ReplyOutcome::Unreadable(blocker.reason()),
                ),
                swap: None,
            };
        }
    };
    // Remember what this reply carried before deciding anything: a schema-less reply
    // for the same app is rebuilt from it later.
    remember_schema(state, &shape, app_id);
    let cached_schema = state.achievement_schemas().schema(app_id);
    let mappings = cached_schema
        .as_deref()
        .and_then(reply::mappings)
        .unwrap_or_default();
    let request = RebuildRequest {
        app_id,
        local: reply::definitions_from(&mappings).unwrap_or_default(),
        unlocks: reply::unlocks(&shape, &mappings),
        schema: reply::schema(&shape).map(<[u8]>::to_vec),
    };
    let plan = decide_rebuild(capability_reason.as_deref(), settings, &request);
    let RebuildPlan::Rebuild(rebuilt) = &plan else {
        return Prepared {
            observation: observation(eresult, plan, ReplyOutcome::NothingToRebuild),
            swap: None,
        };
    };
    let Some(bytes) = reply::apply(&shape, rebuilt, &body) else {
        // The rebuild produced what Steam already sent: nothing to swap, and the
        // record still names the plan that was applied.
        return Prepared {
            observation: observation(eresult, plan, ReplyOutcome::NothingToRebuild),
            swap: None,
        };
    };
    // SAFETY: the same live job object the body was read from; the caller drops the
    // swap after the original call returns.
    match unsafe { ReplySwap::install(this, geometry, bytes) } {
        Some(swap) => {
            let bytes = swap.served_size();
            Prepared {
                observation: observation(
                    eresult,
                    plan,
                    ReplyOutcome::Served { bytes },
                ),
                swap: Some(swap),
            }
        }
        None => Prepared {
            observation: observation(
                eresult,
                plan,
                ReplyOutcome::Unreadable("the reply body could not be swapped".to_owned()),
            ),
            swap: None,
        },
    }
}

fn remember_schema(state: &HostState, shape: &Shape, app_id: u32) {
    if let Some(schema) = reply::schema(shape) {
        state.achievement_schemas().remember(app_id, schema);
    }
}

/// Copy the reply body out of Steam's object.
///
/// # Safety
/// `this` must be a live job object of `geometry`, and the reply object it points
/// at must be live for at least `geometry.size_offset() + size_of::<usize>()` bytes.
unsafe fn read_body(this: *mut c_void, geometry: JobReply) -> Option<Vec<u8>> {
    if this.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees the job object, and `JobReply::new` validated
    // that `reply_field` is pointer-aligned and inside it.
    let reply_object = unsafe { *(this.cast::<u8>().add(geometry.reply_field).cast::<*const u8>()) };
    if reply_object.is_null() {
        return None;
    }
    // SAFETY: both slots are inside the live reply object, at the derived offsets.
    let (data, size) = unsafe {
        (
            *(reply_object.add(geometry.body).cast::<*const u8>()),
            *(reply_object.add(geometry.size_offset()).cast::<usize>()),
        )
    };
    if data.is_null() || size == 0 || size > super::reply::MAX_REPLY_BODY {
        return None;
    }
    // SAFETY: the pair is the one Steam's reply object holds, so it is valid for
    // `size` bytes.
    Some(unsafe { std::slice::from_raw_parts(data, size) }.to_vec())
}

/// One line naming what happened to the reply, for the log.
pub fn describe(reply: &ReplyOutcome) -> String {
    match reply {
        ReplyOutcome::Gated => {
            "the achievements capability or the local-rebuild flag is off, so Steam's reply is \
             forwarded unchanged"
                .to_owned()
        }
        ReplyOutcome::NoGeometry(reason) => (*reason).to_owned(),
        ReplyOutcome::Unreadable(reason) => reason.clone(),
        ReplyOutcome::NothingToRebuild => {
            "no rebuilt payload was produced, so Steam's reply is forwarded unchanged".to_owned()
        }
        ReplyOutcome::Served { bytes } => format!("served a rebuilt reply of {bytes} bytes"),
    }
}

#[cfg(test)]
mod tests;
