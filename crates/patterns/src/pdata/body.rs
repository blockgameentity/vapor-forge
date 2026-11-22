//! Body classification for `.pdata` extents, and the anchor quality gate.
//!
//! An RVA being a function entry is not enough to make a mined pattern
//! corpus-worthy. A merged extent can still be a fragment that never transfers
//! control back or onward (cold code, an unwind-only stub), and a resolved
//! pattern can land in the middle of a function. The gate accepts only an entry
//! whose body shows it is a real function:
//!
//! * it contains a `RET` (any epilogue; early returns are normal), or
//! * its last decodable instruction is an unconditional branch (a tail jump).
//!
//! Everything else — a mid-function hit, an extent with no terminator, an RVA
//! the table does not cover, an extent that is not file-backed — is rejected.

use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, Instruction, Mnemonic};
use thiserror::Error;

use super::{Function, Pdata};
use crate::pe_image::PeImage;

/// A function that ends in an unconditional branch and is at most this long is
/// a tail-call stub: hooking it would hook the stub, not the implementation.
pub const THUNK_MAX_BYTES: u32 = 0x20;

/// What one linear decode of an extent says about it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Body {
    pub instructions: usize,
    /// At least one `RET` anywhere in the extent.
    pub has_ret: bool,
    /// The last decodable instruction is an unconditional branch.
    pub ends_in_jump: bool,
    /// Ends in an unconditional branch and is short: a tail-call stub.
    pub is_thunk: bool,
    /// The final instruction did not decode, so the tail is not trustworthy.
    pub truncated: bool,
}

impl Body {
    /// A real body returns, or hands control to another function with a tail
    /// jump. This is the predicate the anchor gate applies.
    pub fn terminates(&self) -> bool {
        self.has_ret || self.ends_in_jump
    }

    /// One-word verdict for reports.
    pub fn label(&self) -> &'static str {
        if self.is_thunk {
            "tail-jump thunk"
        } else if self.terminates() {
            "returns"
        } else {
            "no terminator"
        }
    }
}

/// Decode `code` (the whole extent) at virtual address `rva` and classify it.
///
/// Trailing alignment padding (`nop`, `int3`) is skipped when deciding how the
/// body ends: MSVC pads the tail of an extent up to the next function, and
/// `jmp target` followed by padding is still a tail jump.
pub fn analyze(code: &[u8], rva: u32) -> Body {
    let mut decoder = Decoder::with_ip(64, code, u64::from(rva), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut body = Body::default();
    let mut last_decoded = Code::INVALID;
    let mut last_significant: Option<FlowControl> = None;
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        let flow = instruction.flow_control();
        last_decoded = instruction.code();
        body.instructions += 1;
        if flow == FlowControl::Return {
            body.has_ret = true;
        }
        if !matches!(instruction.mnemonic(), Mnemonic::Nop | Mnemonic::Int3) {
            last_significant = Some(flow);
        }
    }
    body.truncated = body.instructions > 0 && last_decoded == Code::INVALID;
    body.ends_in_jump = !body.truncated
        && matches!(
            last_significant,
            Some(FlowControl::UnconditionalBranch | FlowControl::IndirectBranch)
        );
    body.is_thunk = body.ends_in_jump && code.len() as u32 <= THUNK_MAX_BYTES;
    body
}

/// Does one extent end in a terminator (possibly before alignment padding)?
/// This is the fallthrough test the merge rule uses: a flush entry that follows
/// an extent with no terminator is a continuation of it, not a new function.
pub fn ends_terminated(image: &PeImage<'_>, function: Function) -> bool {
    analyze_in(image, function).is_some_and(|body| body.terminates())
}

/// Decode one extent out of the image. `None` when the extent is not file-backed.
pub fn analyze_in(image: &PeImage<'_>, function: Function) -> Option<Body> {
    let code = image.bytes_at_rva(u64::from(function.begin), function.len() as usize)?;
    Some(analyze(code, function.begin))
}

/// Convenience classifier for tools that only need the thunk flag.
pub fn is_thunk(image: &PeImage<'_>, function: Function) -> bool {
    analyze_in(image, function).is_some_and(|body| body.is_thunk)
}

/// The anchor gate verdict for one resolved RVA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnchorVerdict {
    /// On a function entry whose body terminates: corpus-worthy.
    Verified { function: Function, body: Body },
    /// Inside a function but not at its begin — a mid-function hit.
    MidFunction { function: Function },
    /// On an entry, but the extent shows no `RET` and no tail jump.
    NoTerminator { function: Function, body: Body },
    /// Not covered by the table, or not file-backed.
    Unmapped,
}

impl AnchorVerdict {
    pub fn is_verified(self) -> bool {
        matches!(self, Self::Verified { .. })
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Verified { .. } => "verified",
            Self::MidFunction { .. } => "mid-function",
            Self::NoTerminator { .. } => "no-body",
            Self::Unmapped => "unmapped",
        }
    }

    /// The function this verdict is about, if the table models it.
    pub fn function(self) -> Option<Function> {
        match self {
            Self::Verified { function, .. }
            | Self::MidFunction { function }
            | Self::NoTerminator { function, .. } => Some(function),
            Self::Unmapped => None,
        }
    }
}

/// Apply the gate: is `rva` a hook target this corpus may claim?
pub fn verify(image: &PeImage<'_>, table: &Pdata, rva: u32) -> AnchorVerdict {
    let Some(function) = table.containing(rva) else {
        return AnchorVerdict::Unmapped;
    };
    if function.begin != rva {
        return AnchorVerdict::MidFunction { function };
    }
    match analyze_in(image, function) {
        Some(body) if body.terminates() => AnchorVerdict::Verified { function, body },
        Some(body) => AnchorVerdict::NoTerminator { function, body },
        None => AnchorVerdict::Unmapped,
    }
}

/// Count the bodies of a whole table: how many extents terminate, how many are
/// thunks, how many look like nothing at all.
pub fn body_summary(image: &PeImage<'_>, table: &Pdata) -> (usize, usize, usize) {
    let (mut terminating, mut thunks, mut unbacked) = (0, 0, 0);
    for function in table.functions() {
        match analyze_in(image, *function) {
            Some(body) => {
                terminating += usize::from(body.terminates());
                thunks += usize::from(body.is_thunk);
            }
            None => unbacked += 1,
        }
    }
    (terminating, thunks, unbacked)
}

/// Mark a report line that the anchor gate rejected and say why, so a rejection
/// is never silent.
#[derive(Debug, Error)]
#[error("anchor gate rejected {name} at {rva:#x}: {verdict}")]
pub struct RejectedAnchor {
    pub name: String,
    pub rva: u32,
    pub verdict: &'static str,
}

/// Build the rejection error for one failed verdict.
pub fn apply_verdict(name: &str, rva: u32, verdict: AnchorVerdict) -> Option<RejectedAnchor> {
    (!verdict.is_verified()).then(|| RejectedAnchor {
        name: name.to_owned(),
        rva,
        verdict: verdict.label(),
    })
}

#[cfg(test)]
mod tests;
