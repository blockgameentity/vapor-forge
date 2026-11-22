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
//! **Nothing here is installed.** [`crate::features::achievements`] keeps the
//! decision and [`crate::hooks::REPLACEMENTS`] carries no body for either symbol;
//! the last section of this module is the measurement that says why, and what a
//! hook would need instead. The shapes are recorded so the next pass does not
//! re-derive them from 26 MB of module bytes.
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
/// | `rcx` | `this`: the same job object | **Verified** — public `0xaa042c` (`mov %rcx,0x8(%rax)`), beta `0xac97dc`; the merge dereferences it throughout. |
/// | `edx` | `EResult` of the reply, supplied by the only caller from the response parser | **Verified** — public `0xaa0420` (`sub $0x2,%edx`), beta `0xac97d0`, and the caller sequence `mov %edi,%edx; mov %rbx,%rcx; call YieldingMergeStats` quoted in `VTABLE-REPORT.md` §1. 32 bits: the dispatch is `sub`, `sub`, `cmp $0xd`, not a pointer test. |
/// | `r8` | unmeasured | **Assumed** — the anchored entry does not read it; declared for verbatim forwarding. |
/// | `r9` | unmeasured | **Assumed** — same evidence as `r8`. |
///
/// # Return
///
/// Nothing. The epilogues (`public 0xaa1200`, `0xaa1209`) leave `eax` untouched
/// and the only caller ignores it, so a replacement is `-> ()`.
pub type MergeStatsFn =
    unsafe extern "system" fn(this: *mut c_void, eresult: i32, r8: usize, r9: usize);

/// The job-object fields a body would have to read, per live build.
///
/// Offsets are the **only** way into this object: both anchored functions take
/// nothing but `this` and an `EResult`, so the app id, the reply block and the
/// engine all live inside the allocation. Each offset is **Verified** by the
/// instruction that reads it, in that build; they are *not* interchangeable
/// between builds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobFields {
    /// The `CSteamEngine` interface pointer whose vtable slot 5 (`+0x28`) waits
    /// for the service-method response. Verified: public `0xa8fda1`
    /// (`mov 0x170(%rbx),%rcx; mov (%rcx),%rax; call *0x28(%rax)`).
    pub engine: usize,
    /// The reply's stats block: the ordered dictionary the merge walks, replaces
    /// and frees. Verified: public `0xaa04a5` (`mov 0x1a8(%r13),%rax`), written
    /// back at `0xaa0ca4` (`mov %rbp,0x1a8(%r13)`) with the old block released.
    pub stats_block: usize,
    /// `CGameID`-shaped 8 bytes: low 24 bits the app id, byte 3 the id type
    /// (0/1/2), `+4` the mod id. Verified by the run method's validation of its
    /// own argument block: public `0xa8fd65` (`movzbl 0x3(%rsi)`),
    /// `0xa8fd99` (`test $0xffffff,(%rsi)`), `0xa8fd7a` (`cmp $0x0,0x4(%rsi)`).
    pub game_id: usize,
}

/// Public build `669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b`.
pub const PUBLIC: JobFields = JobFields {
    engine: 0x170,
    stats_block: 0x1a8,
    game_id: 0x198,
};

/// Publicbeta build `4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10`.
pub const PUBLICBETA: JobFields = JobFields {
    engine: 0x168,
    stats_block: 0x1a0,
    game_id: 0x190,
};

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

// Why neither anchored site carries the local achievement rebuild yet.
//
// This is the module's conclusion, kept next to the shapes it is derived from.
//
// 1. **The reply bytes are already gone.** Linux replaces the encoded reply
//    body (`crates/hooks/src/netpacket/stats_proxy.rs`), before anything parses
//    it. Both anchored sites run *after* the parse: the reply is a stats block
//    inside `this` ([`JobFields::stats_block`]), not bytes.
// 2. **Replacing that block means constructing the client's own dictionary.**
//    The merge builds a replacement block, stores it over the old pointer
//    (`mov %rbp,0x1a8(%r13)`, public `0xaa0ca4`) and releases the old one with a
//    size of 0xa0 (`0xaa0c97`). The walk is inlined over 0x38-byte nodes with the
//    key at `+0x10`; the calls inside it are the dictionary's hash/lookup pair
//    (`0x138d9ef40`, `0x138d9fb50`) and, at the replacement, the block's
//    destructor and deallocation (`0x138a8c4a0`, `0x1388cead0`). None of the four
//    is in the corpus, and none is a writer the host could call: there is no
//    separable insert function at all, so this cannot be fixed by a second
//    pattern — the shape has to be reimplemented or replaced upstream of the
//    parse.
// 3. **Every field is a per-build offset.** [`PUBLIC`] and [`PUBLICBETA`] differ
//    by a uniform `-0x8`; the corpus format carries patterns only, so a body
//    would carry a build table ([`fields_for`]) and fail closed off it.
// 4. **The decision has no input producer on this host.** Definitions come from
//    a stats schema parsed by `vapor_forge_steam_protocol`'s
//    `parse_achievement_bit_mappings`, which is not a dependency of this crate
//    and which no Windows hook has ever fed.
//
// What the body would need instead: the reply *body* site — §1 site 2
// (`0xa93120` / `0xabcc20`, the response parser, unanchored) — or the CM receive
// path with the win64 packet layout (`CNetPacket::{Init, Alloc, Release}`, in
// neither win64 corpus file), plus a schema cache for this host. See
// [`crate::features::achievements`] for the wiring that *is* done.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;
    use crate::install::deps::capabilities_for_symbol;

    const PUBLIC_SHA: &str = "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b";
    const BETA_SHA: &str = "4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10";

    #[test]
    fn both_anchored_sites_gate_the_achievements_capability() {
        for symbol in [BINIT_SYMBOL, MERGE_SYMBOL] {
            assert_eq!(
                capabilities_for_symbol(symbol),
                &[Capability::Achievements],
                "{symbol} is the achievements site and must gate nothing else"
            );
        }
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
        ] {
            assert_eq!(public, beta + 0x8);
        }
    }
}
