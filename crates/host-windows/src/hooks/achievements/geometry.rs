//! Where one stats reply's body lives — decided by decoding the target's code.
//!
//! The merge site takes nothing but `this` and an `EResult`, so the reply is
//! reachable only through a job-object field, and the body only through a field of
//! that reply object. Both numbers are **decoded from the build's own code**, never
//! compiled: `patterns` is landing that decoder
//! (`patterns::job_fields::decode_job_reply`) and this module is the one call site.
//!
//! # The contract
//!
//! ```text
//! reply object  = *(this + reply_field)
//! body slot     = reply object + body          // a (pointer, length) pair
//! ```
//!
//! `body` is the offset of the body's **pointer slot**, and the length sits one
//! pointer further on. That is the shape the corpus already measures for a
//! CNetPacket's own payload (`res/patterns/win64/VTABLE-REPORT.md` §1.5: `data`
//! at `+0x10`, `size` at `+0x18`), and the reply object is the same
//! buffer-carrying object — which is why the length offset is derived as
//! `size_of::<usize>()` from the pointer slot rather than named here.
//!
//! # Until the decoder lands
//!
//! [`decode_plan`] reports [`PENDING`] and every reply rewrite therefore falls
//! through with that reason logged. The merge of
//! `patterns::job_fields` is a one-line change in [`CorpusDecoder::decode`], and
//! nothing else in this crate has to move: the entry point the worker calls, the
//! validation and the swap all take the decoder's output as data.

use vapor_forge_patterns::scan::CodeSegment;

use crate::abi::achievements::BINIT_SYMBOL;

/// The largest offset a job-object field may sit at.
///
/// A bound, not a layout fact: the job allocation is a few hundred bytes, so a
/// number past this is a decode failure rather than an offset. It exists so a
/// wild number cannot make the body read far outside a live object.
pub const MAX_JOB_FIELD: usize = 0x1000;

/// The largest offset a reply-object field may sit at.
pub const MAX_REPLY_FIELD: usize = 0x100;

/// Where one build's stats reply lives, as `patterns::job_fields::decode_job_reply`
/// returns it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobReply {
    /// Offset of the reply pointer inside the job object.
    pub reply_field: usize,
    /// Offset of the body's pointer slot inside the reply object.
    pub body: usize,
}

impl JobReply {
    /// `None` unless both numbers are pointer-aligned and inside their bounds.
    ///
    /// A decode that produces a misaligned slot would make the rewrite read
    /// across two fields, so it is refused here and the reply is left alone.
    pub fn new(reply_field: usize, body: usize) -> Option<Self> {
        let pointer = core::mem::size_of::<usize>();
        let aligned = reply_field % pointer == 0 && body % pointer == 0;
        let in_bounds = reply_field != 0
            && reply_field <= MAX_JOB_FIELD
            && body <= MAX_REPLY_FIELD;
        (aligned && in_bounds).then_some(Self { reply_field, body })
    }

    /// The offset of the length slot that pairs with [`Self::body`].
    ///
    /// Derived from the pointer slot, not named: a `(pointer, length)` pair puts
    /// the length one pointer past the pointer.
    pub fn size_offset(&self) -> usize {
        self.body + core::mem::size_of::<usize>()
    }
}

/// Why no geometry was decoded.
pub const PENDING: &str = "the stats-reply decoder (patterns::job_fields::decode_job_reply) is not in \
     this build yet, so the reply body's two offsets are unknown and the reply is left as Steam \
     sent it";

/// A decoder that reads the reply geometry out of a module's code.
///
/// The trait exists so the one call site does not depend on the patterns crate's
/// function being present: the corpus decoder is a hidden implementation, and a
/// test can supply its own.
pub trait ReplyDecoder {
    /// # Errors
    /// The decoder's own reason, which the caller reports.
    fn decode(
        &self,
        segment: &CodeSegment<'_>,
        binit_offset: usize,
    ) -> Result<JobReply, &'static str>;
}

/// The decoder this build has: the corpus one when it is merged, [`PENDING`]
/// before that.
pub struct CorpusDecoder;

impl ReplyDecoder for CorpusDecoder {
    fn decode(
        &self,
        _segment: &CodeSegment<'_>,
        _binit_offset: usize,
    ) -> Result<JobReply, &'static str> {
        // The merge replaces this line with the corpus decode:
        //
        //     vapor_forge_patterns::job_fields::decode_job_reply(segment, binit_offset)
        //         .and_then(|reply| JobReply::new(reply.reply_field, reply.body).ok_or(...))
        Err(PENDING)
    }
}

/// Offset of `CAPIJobRequestUserStats::BInit` inside `segment`, when resolved.
///
/// The decoder reads the run method because it is where the reply object is
/// stored on the job (`mov %rax, <reply_field>(%rbx)`), so its own offset is the
/// input rather than a number this crate keeps.
pub fn symbol_offset<'a>(
    resolved: impl Iterator<Item = (&'a str, usize)>,
    segment: &CodeSegment<'_>,
) -> Option<usize> {
    resolved
        .filter(|(name, _)| *name == BINIT_SYMBOL)
        .find_map(|(_, offset)| offset.checked_sub(segment.file_offset as usize))
}

/// Decode one module's reply geometry, with `decoder`.
pub fn decode_with(
    decoder: &dyn ReplyDecoder,
    plan: &crate::install::BatchPlan,
    image: &[u8],
) -> Result<JobReply, String> {
    let image = vapor_forge_patterns::pe_image::PeImage::parse(image)
        .map_err(|error| format!("module image could not be parsed: {error}"))?;
    let segment = image
        .code_segment()
        .map_err(|error| format!("module has no executable segment: {error}"))?;
    let offset = symbol_offset(plan.resolved(), &segment).ok_or_else(|| {
        format!("the corpus does not anchor {BINIT_SYMBOL}, so the reply cannot be located")
    })?;
    let reply = decoder
        .decode(&segment, offset)
        .map_err(str::to_owned)?;
    JobReply::new(reply.reply_field, reply.body)
        .ok_or_else(|| format!("decoded reply geometry is unusable: {reply:?}"))
}

/// Decode one module's reply geometry with the corpus decoder this build has.
pub fn decode_plan(plan: &crate::install::BatchPlan, image: &[u8]) -> Result<JobReply, String> {
    decode_with(&CorpusDecoder, plan, image)
}

#[cfg(test)]
mod tests;
