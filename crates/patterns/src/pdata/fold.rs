//! The merge rule: how raw `.pdata` entries become functions.
//!
//! This module implements steps 2-5 of the rule documented on [`super`], and
//! records the measurements the join clauses were chosen from.
//!
//! ## Evidence
//!
//! Numbers below are public `steamclient64.dll`; all four Gate-0 modules are
//! tabulated in `res/patterns/win64/README.md`, reproduce with
//! `vapor-forge-pe-functions --metrics`.
//!
//! * 92,949 raw entries form 49,230 maximal contiguous runs. **No entry
//!   overlaps another** and none has `end <= begin`, so rule 4.1 never fires on
//!   real images: contiguity is the only kind of join that occurs. The probe's
//!   49,087 is not a second definition of "function" — it is those 49,230 runs
//!   minus the 143 that are shorter than `0x18`, i.e. a sweep filter.
//! * Alignment marks function starts in this corpus: 48,577 of the 49,230 run
//!   heads are 16-byte aligned (98.7%), and the 653 exceptions are byte-packed
//!   thunks.
//! * Contiguity alone is not enough: `LoadModuleWithPath` (public `SteamUI.dll`)
//!   has a 16-byte prologue entry at `0x8dd870` whose body continues in a
//!   separate *aligned* entry at `0x8dd880`. The prologue has no terminator, so
//!   rule 4.3 joins them into one 774-byte function and the mined anchor lands
//!   on its entry. `CUpdateManager::MarkAppChange` has the same shape (721 bytes
//!   once its epilogue entry at `0x6cf650` is joined).
//! * Alignment alone is not enough either: `0xb2ea0`, `0xb2ee0`, `0xb2f20`, …, `0xb30a0` in public
//!   `steamclient64.dll` are nine consecutive 64-byte functions
//!   (`0xb2ea0..0xb30ee`), each with a prologue and a tail jump, and publicbeta `GetAppDataFromAppInfo` at
//!   `0x4f37e0` (prologue `40 53 55 56 57 41 56 41 57 48 81 EC 78 01 00 00`,
//!   541 bytes, the same size as its public-build extent) sits flush after a
//!   27-byte fragment entry. Both are independent functions, and a contiguity-only rule folds
//!   them away so the gate rejects a shipped anchor.
//! * Rule 4.2 fires 37,903 times and rule 4.3 2,100 times, while 3,716 aligned
//!   flush entries follow a terminating extent and stay separate. Result:
//!   **52,946 functions**, of which 653 still begin unaligned (byte-packed
//!   thunks) and 254 are shorter than `0x18` (kept, rule 5).

use super::{Function, Metrics, ALIGNMENT, SHORT_EXTENT};

/// How a flush entry continues the previous extent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Join {
    Overlap,
    Fragment,
    Fallthrough,
}

/// Apply the merge rule (steps 2-5) to raw entries.
///
/// `ends_terminated` answers "does this extent end in a `ret` or a tail jump,
/// ignoring trailing alignment padding?" — the fallthrough clause of rule 4
/// needs it, and [`super::body::ends_terminated`] is what consumers pass.
pub fn fold(
    mut raw: Vec<(u32, u32)>,
    ends_terminated: impl Fn(Function) -> bool,
) -> (Vec<Function>, Metrics) {
    let mut metrics = Metrics {
        raw_entries: raw.len(),
        ..Metrics::default()
    };
    raw.retain(|(begin, end)| {
        let keep = end > begin;
        if !keep {
            metrics.invalid_entries += 1;
        }
        keep
    });
    for (begin, end) in &raw {
        if begin % ALIGNMENT != 0 {
            metrics.unaligned_entries += 1;
        }
        if end - begin < SHORT_EXTENT {
            metrics.short_entries += 1;
        }
    }
    raw.sort_by_key(|(begin, _)| *begin);

    let mut functions: Vec<Function> = Vec::with_capacity(raw.len());
    for (begin, end) in raw {
        let joined = match functions.last().copied() {
            // A real overlap is a table anomaly: absorb it so no byte is lost.
            Some(last) if begin < last.end => Some(Join::Overlap),
            // Flush and unaligned: a funclet or cold fragment of the previous
            // function (rule 4.2).
            Some(last) if begin == last.end && begin % ALIGNMENT != 0 => Some(Join::Fragment),
            // Flush and aligned, but the previous extent does not terminate:
            // the linker split one function at an aligned boundary (rule 4.3).
            Some(last) if begin == last.end && !ends_terminated(last) => Some(Join::Fallthrough),
            // Flush, aligned, previous extent terminates: a new function.
            Some(last) if begin == last.end => {
                metrics.flush_starts += 1;
                None
            }
            _ => None,
        };
        match joined {
            Some(Join::Overlap) => {
                metrics.overlapping_joins += 1;
                if let Some(last) = functions.last_mut() {
                    last.end = last.end.max(end);
                }
            }
            Some(Join::Fragment) => {
                metrics.fragment_joins += 1;
                if let Some(last) = functions.last_mut() {
                    last.end = end;
                }
            }
            Some(Join::Fallthrough) => {
                metrics.fallthrough_joins += 1;
                if let Some(last) = functions.last_mut() {
                    last.end = end;
                }
            }
            None => functions.push(Function { begin, end }),
        }
    }
    for function in &functions {
        if !function.is_aligned() {
            metrics.unaligned_functions += 1;
        }
        if function.len() < SHORT_EXTENT {
            metrics.short_functions += 1;
        }
    }
    (functions, metrics)
}
