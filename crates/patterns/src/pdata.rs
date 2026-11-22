//! The PE32+ `.pdata` function table: one definition, used by every consumer.
//!
//! PE32+ x64 images describe their own function layout in the exception
//! directory (`IMAGE_DIRECTORY_ENTRY_EXCEPTION`, section `.pdata`) as an array
//! of `RUNTIME_FUNCTION { BeginAddress, EndAddress, UnwindInfoAddress }`. That
//! array is *not* one entry per function: a function that owns exception
//! handlers, funclets or cold fragments contributes several entries, and the
//! linker can split one function at an aligned boundary, so raw entries
//! over-count functions and a fragment on its own is not a hook target.
//!
//! # The merge rule (normative)
//!
//! 1. Read every whole `RUNTIME_FUNCTION` in the directory. A table whose size
//!    is not a multiple of 12, or whose mapped range leaves the file, is an
//!    error, never a silently short table.
//! 2. Drop entries with `end <= begin` (none occur in the four Steam modules,
//!    but a corrupt table must not produce a phantom function) and count them.
//! 3. Sort by `begin`.
//! 4. An entry joins the previous extent when it is a **fragment of it**, and
//!    starts a new function otherwise. "Fragment" means any of:
//!    1. `begin < previous.end` — a real overlap, a table anomaly; the later
//!       `end` wins so no byte is ever lost.
//!    2. `begin == previous.end && begin % 16 != 0` — it begins flush *and*
//!       unaligned, which is where MSVC puts funclets and cold fragments.
//!    3. `begin == previous.end && begin % 16 == 0` **and the previous extent
//!       does not terminate** (no `ret` and no tail jump, ignoring trailing
//!       `nop`/`int3` padding) — the linker split one function at an aligned
//!       boundary and control falls through into the next entry.
//!
//!    An entry that begins flush on [`ALIGNMENT`] *after a terminating extent*
//!    is an independent function placed flush, not a fragment. The measurements
//!    behind rules 4.2 and 4.3 are in [`fold`].
//! 5. **Never drop a merged extent for being short.** A 5-byte extent is a real
//!    function (a `jmp` thunk, a leaf that returns immediately); deleting it
//!    makes an anchor that lands on a genuine entry report "not a function",
//!    which is how the two tools this module replaces disagreed (49,230 extents
//!    against the 49,087 that survive a `< 0x18` filter). Shortness is a
//!    *property* ([`Metrics::short_functions`]) and a *class*
//!    ([`Body::is_thunk`]), never a filter.
//!
//! Consequences every consumer inherits: an RVA is a function *entry* iff it
//! equals some merged `begin`; an RVA inside a merged extent but not at its
//! begin is a mid-function hit (a funclet that was merged into its parent), and
//! is never a hook target.
use std::fmt;

use thiserror::Error;

use crate::pe_image::PeImage;

pub mod body;
pub mod fold;

pub use body::{
    analyze, analyze_in, apply_verdict, body_summary, is_thunk, verify, AnchorVerdict, Body,
    RejectedAnchor, THUNK_MAX_BYTES,
};
pub use fold::fold;

/// `IMAGE_DIRECTORY_ENTRY_EXCEPTION`: `.pdata` on PE32+ images.
pub const EXCEPTION_DIRECTORY: usize = 3;

/// Encoded size of one `RUNTIME_FUNCTION` in a PE32+ image.
pub const RUNTIME_FUNCTION_SIZE: usize = 12;

/// Function-entry alignment MSVC uses on x64: a flush entry on this boundary is
/// a new function, a flush entry off it is a fragment (see the module docs).
pub const ALIGNMENT: u32 = 16;

/// Extents shorter than this cannot hold a prologue plus a body. Recorded as a
/// metric only: the merge rule keeps them.
pub const SHORT_EXTENT: u32 = 0x18;

/// Why a `.pdata` table could not be read.
#[derive(Debug, Error, Eq, PartialEq)]
pub enum PdataError {
    #[error("image has no exception directory (.pdata)")]
    Missing,
    #[error("exception directory RVA {0:#x} is not mapped")]
    Unmapped(u32),
    #[error(".pdata spans {len} bytes, which is not a multiple of {RUNTIME_FUNCTION_SIZE}")]
    Truncated { len: usize },
    #[error(".pdata entry {index} at file offset {at:#x} runs past the end of the image")]
    PastEof { index: usize, at: usize },
}

/// One merged function extent, `[begin, end)` in RVAs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct Function {
    pub begin: u32,
    pub end: u32,
}

impl Function {
    pub fn len(self) -> u32 {
        self.end.saturating_sub(self.begin)
    }

    pub fn is_empty(self) -> bool {
        self.end <= self.begin
    }

    /// True when `rva` is inside the extent, entry included.
    pub fn contains(self, rva: u32) -> bool {
        rva >= self.begin && rva < self.end
    }

    /// True when the extent begins on a 16-byte boundary, where MSVC aligns
    /// function entries. Byte-packed thunks do not, so this is a hint only.
    pub fn is_aligned(self) -> bool {
        self.begin % ALIGNMENT == 0
    }
}

/// What the raw table looked like and what the rule made of it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Metrics {
    /// Whole entries read from the table.
    pub raw_entries: usize,
    /// Entries dropped for `end <= begin`.
    pub invalid_entries: usize,
    /// Flush joiners (`next.begin == previous.end`) that were unaligned, so they
    /// were absorbed as funclets/fragments.
    pub fragment_joins: usize,
    /// Flush joiners that were 16-aligned but followed an extent with no
    /// terminator: a continuation the linker split at an aligned boundary.
    pub fallthrough_joins: usize,
    /// Joiners where `next.begin < previous.end` (overlap anomaly), absorbed.
    pub overlapping_joins: usize,
    /// Flush joiners that were 16-aligned and followed a terminating extent:
    /// independent functions placed flush. Counted, not absorbed.
    pub flush_starts: usize,
    /// Raw entries whose `begin` is not 16-byte aligned.
    pub unaligned_entries: usize,
    /// Raw entries shorter than [`SHORT_EXTENT`].
    pub short_entries: usize,
    /// Merged extents whose `begin` is not 16-byte aligned.
    pub unaligned_functions: usize,
    /// Merged extents shorter than [`SHORT_EXTENT`], kept by rule 5.
    pub short_functions: usize,
}

impl fmt::Display for Metrics {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let functions = self.raw_entries
            - self.invalid_entries
            - self.fragment_joins
            - self.fallthrough_joins
            - self.overlapping_joins;
        write!(
            out,
            "{functions} functions from {} raw entries\n  \
             joins: {} unaligned fragments, {} aligned continuations, {} overlapping; \
             {} invalid dropped\n  \
             {} aligned flush entries followed a terminating extent and became their own function\n  \
             raw entries: {} short (< {SHORT_EXTENT:#x}), {} not 16-aligned\n  \
             merged extents: {} short (< {SHORT_EXTENT:#x}), {} not 16-aligned",
            self.raw_entries,
            self.fragment_joins,
            self.fallthrough_joins,
            self.overlapping_joins,
            self.invalid_entries,
            self.flush_starts,
            self.short_entries,
            self.unaligned_entries,
            self.short_functions,
            self.unaligned_functions,
        )
    }
}

/// A parsed `.pdata` table: sorted, disjoint, merged function extents.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Pdata {
    functions: Vec<Function>,
    metrics: Metrics,
}

impl Pdata {
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    pub fn metrics(&self) -> Metrics {
        self.metrics
    }

    pub fn len(&self) -> usize {
        self.functions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// The function whose `begin` is exactly `rva`, i.e. a function entry.
    pub fn entry(&self, rva: u32) -> Option<Function> {
        let index = self
            .functions
            .binary_search_by_key(&rva, |function| function.begin)
            .ok()?;
        Some(self.functions[index])
    }

    /// The function containing `rva`, entry included.
    pub fn containing(&self, rva: u32) -> Option<Function> {
        let index = self
            .functions
            .partition_point(|function| function.begin <= rva);
        let candidate = self.functions.get(index.checked_sub(1)?)?;
        candidate.contains(rva).then_some(*candidate)
    }
}

/// Split raw `.pdata` bytes into `(begin, end)` pairs, unsorted and unfiltered.
pub fn entries(table: &[u8]) -> Result<Vec<(u32, u32)>, PdataError> {
    if table.len() % RUNTIME_FUNCTION_SIZE != 0 {
        return Err(PdataError::Truncated { len: table.len() });
    }
    Ok(table
        .chunks_exact(RUNTIME_FUNCTION_SIZE)
        .map(|entry| {
            let begin = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
            let end = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
            (begin, end)
        })
        .collect())
}

/// The `.pdata` bytes of `image`, borrowed from the file bytes `data`.
fn directory<'a>(image: &PeImage<'_>, data: &'a [u8]) -> Result<&'a [u8], PdataError> {
    let (rva, size) = image
        .data_directory(EXCEPTION_DIRECTORY)
        .filter(|(_, size)| *size > 0)
        .ok_or(PdataError::Missing)?;
    let offset = image
        .rva_to_offset(u64::from(rva))
        .ok_or(PdataError::Unmapped(rva))?;
    let size = size as usize;
    data.get(offset..offset + size).ok_or(PdataError::PastEof {
        index: size / RUNTIME_FUNCTION_SIZE,
        at: offset + size,
    })
}

/// The raw entries exactly as stored, sorted by `begin`. Only measurement code
/// needs this; [`parse`] is what consumers resolve against.
pub fn raw_entries(image: &PeImage<'_>, data: &[u8]) -> Result<Vec<(u32, u32)>, PdataError> {
    let mut raw = entries(directory(image, data)?)?;
    raw.sort_by_key(|(begin, _)| *begin);
    Ok(raw)
}

/// Read the exception directory of `image` out of the file bytes `data` and
/// merge it. This is the only function a consumer needs.
pub fn parse(image: &PeImage<'_>, data: &[u8]) -> Result<Pdata, PdataError> {
    let (functions, metrics) = fold(entries(directory(image, data)?)?, |function| {
        body::ends_terminated(image, function)
    });
    Ok(Pdata { functions, metrics })
}

#[cfg(test)]
mod tests;
