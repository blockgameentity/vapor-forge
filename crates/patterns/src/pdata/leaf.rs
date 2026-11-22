//! The anchor gate's leaf rule: an address `.pdata` cannot name.
//!
//! A leaf function calls nothing and needs no unwinding, so MSVC emits no
//! `RUNTIME_FUNCTION` for it and the table cannot name it. Rejecting every
//! address outside the table therefore rejects genuine, patchable code: on both
//! `steamclient64.dll` builds the account accessor `CUser::GetSteamID` is a
//! 13-byte leaf (`mov 0x20a(%rcx),%rax; mov %rax,(%rdx); mov rax,rdx; ret`) with
//! no table entry.
//!
//! Such an address is accepted only on positive evidence, never by default:
//!
//! * the signature that produced it matched exactly once in the image
//!   ([`AnchorEvidence::unique_site`]);
//! * the [`PATCH_BYTES`] bytes immediately before it are padding (`0xCC`/
//!   `0x90`), so the jump the hook writes cannot straddle the preceding body;
//! * the byte at the address is itself code, not the padding run it sits after;
//! * a `RET` appears within the first [`LEAF_SCAN_BYTES`] bytes.
//!
//! A leaf accepted this way is reported as `AnchorVerdict::Leaf`, never as
//! `verified`: the evidence is weaker and the verdict says so. [`super::verify_with`]
//! consults `.pdata` first, so an address *inside* an extent keeps failing as a
//! mid-function hit and the leaf rule is never reached for it.

use super::body::{analyze, Body};
use crate::pe_image::PeImage;

/// Bytes the hook writer overwrites at an anchor (an absolute jump). A leaf
/// entry must be preceded by at least this much padding, so the patch stops at
/// the entry instead of eating the tail of the function before it.
pub const PATCH_BYTES: u32 = 12;

/// How far into a leaf the gate looks for the `RET` that proves the site is a
/// function body and not padding or data.
pub const LEAF_SCAN_BYTES: usize = 64;

/// What a resolved site proves about itself, beyond what the image says.
///
/// The leaf rule needs a fact only the matcher knows: whether the signature that
/// produced the address is unique. It is a parameter, not a lookup, so a caller
/// that cannot prove uniqueness gets the strict gate by default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AnchorEvidence {
    /// The signature matched exactly once in the executable section.
    pub unique_site: bool,
}

impl AnchorEvidence {
    /// Evidence for a signature the matcher found exactly once.
    pub fn unique() -> Self {
        Self { unique_site: true }
    }
}

/// Is `rva` a leaf on the evidence the module docs list? `None` means "not
/// proven", which the gate reports as `unmapped`.
pub fn leaf_body(image: &PeImage<'_>, rva: u32, evidence: AnchorEvidence) -> Option<Body> {
    if !evidence.unique_site {
        return None;
    }
    // Executable code, not data that happens to follow padding.
    let offset = image.rva_to_offset(u64::from(rva))?;
    if !image.in_text(offset) {
        return None;
    }
    // A full patch-length run of padding must precede the entry.
    let before = rva.checked_sub(PATCH_BYTES)?;
    let padding = image.bytes_at_rva(u64::from(before), PATCH_BYTES as usize)?;
    if !padding.iter().all(|byte| is_padding(*byte)) {
        return None;
    }
    // The entry must not be inside that padding run.
    if is_padding(*image.bytes_at_rva(u64::from(rva), 1)?.first()?) {
        return None;
    }
    // Short leaves near the end of a section: take what is available, capped.
    let window = (1..=LEAF_SCAN_BYTES)
        .rev()
        .find_map(|len| image.bytes_at_rva(u64::from(rva), len))?;
    let body = analyze(window, rva);
    body.has_ret.then_some(body)
}

fn is_padding(byte: u8) -> bool {
    byte == 0xCC || byte == 0x90
}

#[cfg(test)]
mod tests;
