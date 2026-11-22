//! `CAPIJobRequestUserStats` — the measured shapes of the two stats-job sites.
//!
//! The Windows stats path is a job chain, not a getter: `CUserStats` slot 6
//! constructs a `CAPIJobRequestUserStats`, the job's run method (`BInit`) sends
//! the request, calls the response parser and then calls the merge, and
//! `YieldingMergeStats` merges the parsed reply into the client's stats map.
//! `res/patterns/win64/VTABLE-REPORT.md` §1 measures that chain; both of its
//! functions are anchored in both live builds:
//!
//! | symbol | public | publicbeta | folded `.pdata` extent |
//! | --- | --- | --- | --- |
//! | [`BINIT_SYMBOL`] | `0xa8fd20` | `0xab99a0` | 736 B / 902 B |
//! | [`MERGE_SYMBOL`] | `0xaa0420` | `0xac97d0` | 3,562 B / 3,491 B |
//!
//! **Only the merge carries a body**, and it rewrites the reply body in place:
//! [`crate::hooks::achievements`] forwards the call verbatim, classifies the
//! `EResult` and logs what the local-rebuild decision answers. It rebuilds
//! nothing, because the reply it would have to rewrite is not an argument and no
//! longer bytes: it is the `(pointer, length)` pair the geometry decoder locates
//! (see [why the fields are not named here](#why-the-job-objects-fields-are-not-named-here)).
//!
//! # Calling convention and evidence classes
//!
//! Both are `extern "system"` (MS-x64) methods of one object: `rcx` is `this`.
//! Steam returns `bool` in `al`, so a replacement returns `u8` and never a Rust
//! `bool` — see [`crate::abi`]. A register slot a body does not need is still
//! declared, because a Rust replacement clobbers every volatile register and a
//! dropped slot is a lost argument, not a harmless omission.
//!
//! Every argument below says **Verified** or **Assumed**, with the same meaning
//! as in [`crate::abi`]: *Verified* is read out of the anchored function's own
//! instructions in **both** live builds (the instruction is quoted with its
//! address); *Assumed* is unmeasured and must not be relied on.

use core::ffi::c_void;

/// Corpus symbol of the job's run method.
pub const BINIT_SYMBOL: &str = "CAPIJobRequestUserStats::BInit";

/// Corpus symbol of the completion/merge point.
pub const MERGE_SYMBOL: &str = "CAPIJobRequestUserStats::YieldingMergeStats";

/// `CAPIJobRequestUserStats::BInit` — the job's run method.
///
/// # Arguments
///
/// | slot | meaning | status |
/// | --- | --- | --- |
/// | `rcx` | `this`: the `CAPIJobRequestUserStats` the `CUserStats` slot-6 constructor allocated | **Verified** — read at public `0xa8fd29` (`lea 0x190(%rcx),%r14`) and beta `0xab99ad` (`add $0x188,%rcx`); the function only ever dereferences it. |
/// | `rdx` | unmeasured | **Assumed** — no instruction in the anchored function reads it as an incoming value (public: only `lea 0x480(%rax),%rdx` / `mov $0x2,%edx` writes appear). Declared so a body forwards it verbatim. |
/// | `r8` | unmeasured | **Assumed** — same evidence as `rdx`. |
/// | `r9` | unmeasured | **Assumed** — same evidence as `rdx`. |
///
/// # Return
///
/// `al` read as a byte, never as a Rust `bool`. The anchored body's failure arm
/// is `xor %al,%al; …; ret` (public `0xa8fdd5`); the caller hands the merge an
/// `EResult` derived from the response parser, not from this value.
pub type BInitFn =
    unsafe extern "system" fn(this: *mut c_void, rdx: usize, r8: usize, r9: usize) -> u8;

/// `CAPIJobRequestUserStats::YieldingMergeStats` — the completion/merge point.
///
/// # Arguments
///
/// | slot | meaning | status |
/// | --- | --- | --- |
/// | `rcx` | `this`: the same job object | **Verified** — public `0xaa042c` (`mov %rcx,0x8(%rax)`), beta `0xac97dc`; the merge dereferences it throughout (`mov 0x1a8(%r13),%rax` at public `0xaa04a5`, `mov 0x198(%r13),%rcx` at `0xaa0477`). |
/// | `edx` | `EResult` of the reply, supplied by the only caller from the response parser | **Verified** — public `0xaa0420` (`sub $0x2,%edx`), beta `0xac97d0`, and the caller sequence `call 0xa93120; mov %eax,%edi; … mov %edi,%edx; call 0xaa0420` (public `0xa8ff72`–`0xa8ff7e`). 32 bits: the dispatch is `sub`, `sub`, `cmp $0xd`, not a pointer test. |
/// | `r8` | unmeasured | **Assumed** — the anchored entry does not read it; declared for verbatim forwarding. |
/// | `r9` | unmeasured | **Assumed** — same evidence as `r8`. |
///
/// # Return
///
/// Nothing. The epilogues (`public 0xaa1200`, `0xaa1209`) leave `eax` untouched
/// and the only caller ignores it, so a replacement is `-> ()`.
///
/// # Which `EResult`s merge
///
/// See [`MERGE_EARLY_RETURN`]: the entry's own dispatch returns before any merge
/// for exactly three values, and every other value runs it.
pub type MergeStatsFn =
    unsafe extern "system" fn(this: *mut c_void, eresult: i32, r8: usize, r9: usize);

/// The `EResult`s at which [`MERGE_SYMBOL`] returns without merging.
///
/// **Verified** from the anchored entry in both builds: public `0xaa0420` is
/// `sub $0x2,%edx; je …` (`0xaa0423`), `sub $0x1,%edx; je …` (`0xaa043f`) and
/// `cmp $0xd,%edx; je …` (`0xaa0448`), i.e. `2`, `3` and `16` leave the client's
/// stats store alone. `3` is also the value the run method passes on its own
/// "no connection to Steam" path (public `0xa8ff24`, `mov $0x3,%edi`).
pub const MERGE_EARLY_RETURN: [i32; 3] = [2, 3, 16];

// Why the job object's fields are not named here.
//
// They used to be: a `JobFields` struct with one row per live build (the engine,
// the stats block, the `CGameID` and the reply object), plus the reply body's
// `+0x18`. That table is gone on purpose. A structure offset compiled into this
// crate is a number that survives a Steam rebuild it does not apply to, and the
// rule is that structural facts come from the corpus, never from a constant.
//
// What replaced it, and where each number now comes from:
//
// * the reply object and the reply body — `patterns::job_fields::decode_job_reply`,
//   which decodes `CAPIJobRequestUserStats::BInit` and the parser that reads the
//   body and returns `JobReply { reply_field, body }`. It is wired through
//   [`crate::hooks::achievements::geometry`], which validates the pair and refuses
//   anything misaligned or out of bounds, and it is published once per module after
//   the install batch; [`crate::hooks::achievements::swap`] does the rewrite.
// * the app id — the reply itself, for the legacy shape (`CGameID`'s low 24 bits).
//   The job object's own `game_id` field is no longer read, which is why a
//   service-shaped reply has no app id and is reported as such
//   ([`crate::hooks::achievements::reply::RewriteBlocker::NoApp`]) instead of
//   guessed.
// * the `EResult` — the merge's own second argument, verified above.
//
// The measurement behind every one of those offsets is in
// `res/patterns/win64/VTABLE-REPORT.md` §1, where the decoder reads it. Nothing in
// this crate needs a copy of it to work, and a copy is what goes stale.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::install::deps::capabilities_for_symbol;

    #[test]
    fn the_merge_is_the_one_site_that_gates_the_capability() {
        // The merge is the site with a body (`crate::hooks::achievements`), so it is
        // the dependency. The run method has none, and a bodyless symbol would
        // disable the capability on every real install.
        assert_eq!(
            capabilities_for_symbol(MERGE_SYMBOL),
            &[Capability::Achievements],
            "{MERGE_SYMBOL} is the achievements site and must gate nothing else"
        );
        assert!(capabilities_for_symbol(BINIT_SYMBOL).is_empty());
    }

    #[test]
    fn the_refuted_read_side_name_gates_nothing() {
        // `CUser::GetUserStats` is the name the old dependency table carried. It
        // exists in neither live build (VTABLE-REPORT §1), so a table that still
        // named it described a dependency the install pass could never satisfy.
        assert!(capabilities_for_symbol("CUser::GetUserStats").is_empty());
    }

    #[test]
    fn no_structure_offset_is_compiled_into_this_module() {
        // The rule this module follows: the reply's two offsets come from the
        // decoder, not from a table here. `geometry::PENDING` is the honest gap until
        // that decoder is merged.
        assert!(crate::hooks::achievements::geometry::PENDING.contains("decode_job_reply"));
    }

    #[test]
    fn the_merge_dispatch_values_are_the_three_measured_ones() {
        assert_eq!(MERGE_EARLY_RETURN, [2, 3, 16]);
        // The merge runs for everything else, so a body must forward any value.
        assert!(!MERGE_EARLY_RETURN.contains(&1));
    }
}
