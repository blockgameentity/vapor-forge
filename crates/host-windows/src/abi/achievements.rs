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
//! **Only the merge carries a body**, and it is *observation only*:
//! [`crate::hooks::achievements`] forwards the call verbatim, classifies the
//! `EResult` and logs what the local-rebuild decision answers. It rebuilds
//! nothing, because the reply it would have to rewrite is not an argument and no
//! longer bytes: see [the conclusion](#why-the-reply-cannot-be-replaced-here).
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

/// The job-object fields a body would have to read, per live build.
///
/// Offsets are the **only** way into this object: both anchored functions take
/// nothing but `this` and an `EResult`, so the app id, the reply object and the
/// engine all live inside the allocation. Each offset is **Verified** by the
/// instruction that reads it, in that build; they are *not* interchangeable
/// between builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobFields {
    /// The `CSteamEngine` interface pointer whose vtable slot 5 (`+0x28`) waits
    /// for the service-method response. Verified: public `0xa8fda1`
    /// (`mov 0x170(%rbx),%rcx; mov (%rcx),%rax; call *0x28(%rax)`).
    pub engine: usize,
    /// The parsed stats block: the ordered dictionary the merge walks, replaces
    /// and frees. Verified: public `0xaa04a5` (`mov 0x1a8(%r13),%rax`), written
    /// back at `0xaa0ca4` (`mov %rbp,0x1a8(%r13)`) with the old block released.
    pub stats_block: usize,
    /// `CGameID`-shaped 8 bytes: low 24 bits the app id, byte 3 the id type
    /// (0/1/2), `+4` the mod id. Verified by the run method's validation of its
    /// own argument block: public `0xa8fd65` (`movzbl 0x3(%rsi)`),
    /// `0xa8fd99` (`test $0xffffff,(%rsi)`), `0xa8fd7a` (`cmp $0x0,0x4(%rsi)`).
    pub game_id: usize,
    /// The reply object the response parser reads the protobuf body out of.
    /// Verified twice per build: the run method stores it (`mov %rax,0x1b0(%rbx)`,
    /// public `0xa8fe11`; `mov %rax,0x1a8(%rbx)`, beta `0xab9a64`) and the parser
    /// loads it before the body (`mov 0x1b0(%r14),%rcx; … lea 0x18(%rcx),%rdi`,
    /// public `0xa93161`; the same on `0x1a8` at beta `0xabcc61`).
    pub reply: usize,
}

/// Public build `669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b`.
pub const PUBLIC: JobFields = JobFields {
    engine: 0x170,
    stats_block: 0x1a8,
    game_id: 0x198,
    reply: 0x1b0,
};

/// Publicbeta build `4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10`.
pub const PUBLICBETA: JobFields = JobFields {
    engine: 0x168,
    stats_block: 0x1a0,
    game_id: 0x190,
    reply: 0x1a8,
};

/// Where the protobuf body sits inside the reply object, in both builds.
///
/// **Verified**: public `0x138a9318c` (`lea 0x18(%rcx),%rdi` with `rcx` loaded
/// from `*(this+0x1b0)` one instruction earlier) and beta `0x138abcc8b` (the same
/// `lea` on `*(this+0x1a8)`). This is the *only* place a rebuilt stats reply could
/// be substituted, and it is inside the response parser — not inside either
/// anchored job method.
pub const REPLY_BODY_OFFSET: usize = 0x18;

/// The fields of the build whose module sha256 is `sha256`, or `None`.
///
/// Fail-closed on purpose: a build that is not listed here gets **no** body, so
/// a re-mined or updated Steam client costs the capability instead of reading a
/// wrong offset inside a live client. A third live build means a third row, and
/// the row is written from that build's own instructions — never by scaling.
pub fn fields_for(sha256: &str) -> Option<JobFields> {
    match sha256 {
        "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b" => Some(PUBLIC),
        "4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10" => Some(PUBLICBETA),
        _ => None,
    }
}

// Why the reply cannot be replaced here.
//
// This is the module's conclusion, kept next to the shapes it is derived from.
// Every claim below was re-read from the two live builds for this pass; none of
// it is inherited from the earlier "no merge-site hook" verdict without a quote.
//
// 1. **The reply is already a dictionary, not bytes.** The parser
//    (`0xa93120` / `0xabcc20`) reads the protobuf body at `*(this + reply) +
//    REPLY_BODY_OFFSET`, builds the ordered dictionary it stores at
//    `this + stats_block`, and returns the `EResult` the merge is then handed. By
//    the time either anchored job method is entered with an `EResult` in hand,
//    the encoded reply no longer exists in the job object.
// 2. **Nothing writes that dictionary but inlined code.** The merge copies the
//    reply's entries into the client's per-app store
//    (`0x138a95900(engine_ctx, &game_id, *(this+0x190))`, public `0xaa0498`) with
//    the insert inlined over `0x38`-byte nodes: interning/hashing at
//    `0x138d9ef40`, lookup at `0x138d9fb50`, the replacement written directly at
//    `0xaa0ca4` (`mov %rbp,0x1a8(%r13)`). The parser inlines the same pair
//    (`0x138a9383b`, `0x138a93848`). There is no `Insert(key, value)` function to
//    anchor, so "add an achievement entry" is not a missing pattern — it is a
//    shape no pattern can supply.
// 3. **Every field is a per-build offset.** [`PUBLIC`] and [`PUBLICBETA`] differ
//    by a uniform `-0x8`, and the corpus format carries patterns only, so a body
//    would carry a build table ([`fields_for`]) and fail closed off it.
// 4. **The decision has no input producer on this host.** Definitions come from
//    a stats schema parsed by `vapor_forge_steam_protocol`'s
//    `parse_achievement_bit_mappings`, which is not a dependency of this crate
//    and which no Windows hook has ever fed.
//
// What a real rebuild needs instead is exactly one more anchor: the response
// parser (`0xa93120` / `0xabcc20`, §1 site 2). It takes only `this` and returns
// the `EResult`, so a detour there would have the reply object in hand — and with
// it `*(this + reply) + REPLY_BODY_OFFSET`, the bytes the Linux host substitutes
// in `crates/hooks/src/netpacket/stats_proxy.rs`. It is in neither win64 corpus
// file, and adding it is the patterns workstream's call, not this crate's.
// [`crate::features::achievements`] holds the wiring that *is* done, and
// [`crate::hooks::achievements`] the observation-only body that stands at the
// merge today.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::install::deps::capabilities_for_symbol;

    const PUBLIC_SHA: &str = "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b";
    const BETA_SHA: &str = "4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10";

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
    fn the_job_fields_are_keyed_per_build_and_fail_closed() {
        let public = fields_for(PUBLIC_SHA).expect("public build");
        let beta = fields_for(BETA_SHA).expect("publicbeta build");
        assert_eq!(public, PUBLIC);
        assert_eq!(beta, PUBLICBETA);
        assert_eq!(fields_for("0".repeat(64).as_str()), None);
        assert_eq!(fields_for(""), None);
    }

    #[test]
    fn the_two_builds_shift_uniformly_which_is_why_a_table_is_required() {
        // One delta would today be enough to scale one row into the other, and
        // that is exactly what must not be done: a build whose layout moves by
        // anything other than this constant would read a wrong field.
        for (public, beta) in [
            (PUBLIC.engine, PUBLICBETA.engine),
            (PUBLIC.stats_block, PUBLICBETA.stats_block),
            (PUBLIC.game_id, PUBLICBETA.game_id),
            (PUBLIC.reply, PUBLICBETA.reply),
        ] {
            assert_eq!(public, beta + 0x8);
        }
    }

    #[test]
    fn the_reply_body_offset_is_the_same_in_both_builds() {
        // The one measured fact a future parser hook would not have to re-derive:
        // the body is at `+0x18` from the reply object even though the object's own
        // offset in the job moves by 8 between builds.
        assert_eq!(REPLY_BODY_OFFSET, 0x18);
        assert_ne!(PUBLIC.reply, PUBLICBETA.reply);
    }

    #[test]
    fn the_merge_dispatch_values_are_the_three_measured_ones() {
        assert_eq!(MERGE_EARLY_RETURN, [2, 3, 16]);
        // The merge runs for everything else, so a body must forward any value.
        assert!(!MERGE_EARLY_RETURN.contains(&1));
    }
}
