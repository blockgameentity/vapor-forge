//! The once-per-module decodes, published for the hook bodies.
//!
//! Split from `worker/mod.rs` at the 300-line budget. Both functions are the same
//! shape: the install batch has just resolved this module's corpus symbols, so the
//! geometry the two serving bodies need can be decoded from the module's own code —
//! once, here, off every hook path. A failure is a reason and a debug line, never a
//! panic: the body it would serve falls back to forwarding Steam's bytes.

use tracing::{debug, info, warn};

use crate::hooks::state::HostState;

/// Decode the `CNetPacket` layout from this module's resolved symbols, once.
///
/// The state is published before the batch so hook bodies see the real config;
/// the offsets only exist afterwards, so the layout is filled in here. A failure
/// is a note, not an error: every message then records `NoLayout` and Steam keeps
/// its own request-code round trip.
pub fn packet_layout(state: &HostState, report: &crate::install::RunReport, module: &[u8]) {
    if state.layout().is_some() {
        return;
    }
    let Some(plan) = report.plan.as_ref() else {
        return;
    };
    match crate::hooks::netpacket::layout::decode_plan(plan, module) {
        Ok(layout) => {
            info!(
                data = layout.data,
                size = layout.size,
                "packet layout decoded; request-code serving is possible"
            );
            state.set_layout(layout);
        }
        Err(error) => warn!(
            error = error.as_str(),
            "packet layout could not be decoded; messages are observed, never served"
        ),
    }
}

/// Decode the stats-reply geometry from this module's resolved symbols, once.
///
/// The sibling of [`packet_layout`], and for the same reason: the geometry
/// is a decode of the target's own code (`CAPIJobRequestUserStats::BInit`), it
/// exists only after the install batch resolved the symbols, and a failure must
/// leave the reply untouched rather than guess an offset. See
/// [`crate::hooks::achievements::geometry`].
pub fn reply_geometry(state: &HostState, report: &crate::install::RunReport, module: &[u8]) {
    if state.reply_geometry().is_some() {
        return;
    }
    let Some(plan) = report.plan.as_ref() else {
        return;
    };
    match crate::hooks::achievements::geometry::decode_plan(plan, module) {
        Ok(reply) => {
            info!(
                reply_field = reply.reply_field,
                body = reply.body,
                "stats reply geometry decoded; the merge body can rewrite a reply"
            );
            state.set_reply_geometry(reply);
        }
        Err(reason) => debug!(
            reason,
            "stats reply geometry not decoded; the merge body stays observation-only"
        ),
    }
}
