//! Where the API job keeps its reply, read from the code that dereferences it.
//!
//! `CAPIJobRequestUserStats::BInit` runs the job. It validates the session,
//! sends the request and hands the job object (`this`) to the response parser.
//! The parser is what turns the reply into a body pointer:
//!
//! ```text
//! mov  r14, rcx            ; this
//! ...
//! mov  rcx, [r14+0x1b0]    ; the reply object, a member of the job
//! test rcx, rcx
//! lea  rdi, [rcx+0x18]     ; the body inside the reply object
//! cmovz rdi, r15           ; the "no reply" arm
//! ```
//!
//! The member offset is not portable: it is `0x1b0` in the public build and
//! `0x1a8` in publicbeta (measured, `res/patterns/win64/VTABLE-REPORT.md`), so
//! neither it nor the body displacement may be compiled in. The walk below
//! derives both the same way [`crate::cnet_packet::decode_init`] derives the
//! packet fields: track which register carries `this`, take the pointer load
//! from it, and require the next use of that pointer to be the body `lea`.
//!
//! Two things differ from the packet walk. The shape is one call deep — `BInit`
//! does not compute the body itself, its parser does — so the walk follows the
//! direct calls that are handed `this` as their receiver ([`walk`]), and it
//! follows both arms of a branch, because the call sits on the arm a linear walk
//! jumps over (`test al,al; jz …; mov edi,…; jmp …; mov rcx,rbx; call parser`).
//! A shape the walk cannot pin down is rejected with a reason, never guessed at.

use crate::cnet_packet::convention;
use crate::scan::CodeSegment;

mod state;
mod walk;

use walk::search;

/// The job's reply body, as the code that reads it lays it out.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobReply {
    /// Offset of the reply object from the job (`this`).
    pub reply_field: usize,
    /// Offset of the body from the reply object.
    pub body: usize,
}

/// Bytes walked in the job entry. `BInit` is 736 B in the public build.
const ENTRY_WINDOW: usize = 0x400;
/// Bytes walked in a callee the entry hands `this` to.
const CALLEE_WINDOW: usize = 0x100;
/// Largest gap between the reply load and the body `lea`. Measured: 0x27 bytes
/// public, 0x2a publicbeta — the gap is the null test plus two stores.
const BODY_WINDOW: i64 = 0x40;
/// Callees followed out of the entry; more than a handful is not this shape.
const MAX_CALLEES: usize = 8;
/// Basic blocks decoded inside one function, and instructions across them.
const MAX_BLOCKS: usize = 64;
const BUDGET: usize = 1024;
/// A pointer member of the job: past the vtable, inside the object.
const MIN_FIELD: usize = 8;
const MAX_FIELD: usize = 0x400;

/// Decode the job reply field and body displacement out of the job entry at
/// `binit_offset`, an offset into `segment.bytes`.
///
/// The entry is walked for the reply shape; when it does not carry the shape
/// itself, each direct call it makes with `this` as the receiver is walked in
/// turn (one level). Exactly one distinct `(field, body)` pair must be found.
pub fn decode_job_reply(
    segment: &CodeSegment<'_>,
    binit_offset: usize,
) -> Result<JobReply, &'static str> {
    let abi = convention(segment);
    let pointer_size = usize::from(segment.bits) / 8;
    if pointer_size != 4 && pointer_size != 8 {
        return Err("pointer width");
    }
    let mut pairs: Vec<(i64, i64)> = Vec::new();
    let mut late = false;
    let mut frontier = vec![(binit_offset, true)];
    let mut visited: Vec<usize> = Vec::new();
    while let Some((offset, entry)) = frontier.pop() {
        if visited.contains(&offset) {
            continue;
        }
        visited.push(offset);
        let window = if entry { ENTRY_WINDOW } else { CALLEE_WINDOW };
        let searched = search(segment, offset, abi, window)?;
        pairs.extend(searched.pairs);
        late |= searched.late;
        if entry {
            let callees = searched.callees;
            frontier.extend(callees.into_iter().take(MAX_CALLEES).map(|c| (c, false)));
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let (field, body) = match pairs[..] {
        [] if late => return Err("reply body outside the window"),
        [] => return Err("reply field load"),
        [pair] => pair,
        _ => return Err("ambiguous reply field"),
    };
    let reply_field = usize::try_from(field).map_err(|_| "negative reply field")?;
    let body = usize::try_from(body).map_err(|_| "negative body displacement")?;
    if body == 0 {
        return Err("body displacement");
    }
    if !(MIN_FIELD..=MAX_FIELD).contains(&reply_field) || reply_field % pointer_size != 0 {
        return Err("reply field outside the job object");
    }
    Ok(JobReply { reply_field, body })
}

#[cfg(test)]
mod tests;
