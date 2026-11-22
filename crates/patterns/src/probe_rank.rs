//! Ranking profiles for the Gate-0 derived targets.
//!
//! Each profile is a weighted list of *observable* properties, kept here rather
//! than in the tool so the weights are unit-testable and can be argued with in
//! one place. A score is an ordering aid only: every candidate is printed with
//! the raw numbers that produced it, so a reader can overrule the ranking. What
//! the four unwritten symbols are expected to look like, nothing more:
//!
//! * `GetSubscribedApps(this, uint32* out, uint32 max, bool)` writes 32-bit
//!   values through its second argument in a loop and returns the count.
//! * the user-stats path reads and writes a per-app structure through `this`,
//!   calls out, and does not build strings.
//! * the `CNetPacket` dispatch site selects a handler by `EMsg`, either with a
//!   jump table or with a compare chain over a narrow range.
//! * the `SetEnvString` equivalent appends `KEY=VALUE`, so it stores a literal
//!   `'='` and printable immediates.

use crate::msabi::FirstUse;
use crate::probe_facts::Facts;

/// Values compared against one register below which a chain is too short to be a
/// switch over a message-id space, and the widest such value still called
/// "narrow". Steam message ids run into the low thousands; a length or size
/// compare does not produce a chain of them.
pub const CHAIN_MINIMUM: u32 = 8;
pub const NARROW_RANGE: u64 = 0x4000;
/// Chain length from which a straight-line compare chain is dispatch shaped.
pub const NARROW_CHAIN: u32 = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Probe {
    GetSubscribedApps,
    UserStats,
    Dispatch,
    EnvString,
}

impl Probe {
    pub const ALL: [Probe; 4] = [
        Probe::GetSubscribedApps,
        Probe::UserStats,
        Probe::Dispatch,
        Probe::EnvString,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "getsubscribed" => Some(Self::GetSubscribedApps),
            "userstats" => Some(Self::UserStats),
            "dispatch" => Some(Self::Dispatch),
            "envstring" => Some(Self::EnvString),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::GetSubscribedApps => "getsubscribed",
            Self::UserStats => "userstats",
            Self::Dispatch => "dispatch",
            Self::EnvString => "envstring",
        }
    }

    /// What the profile rewards, printed above the shortlist so the ranking is
    /// never separated from the reason it ordered things that way.
    pub fn profile(self) -> &'static str {
        match self {
            Self::GetSubscribedApps => {
                "rcx(this)+rdx(out ptr)+r8(max)+r9(bool); few u32 stores through arg1 in one \
loop; no calls, no arithmetic, nothing read off the stack"
            }
            Self::UserStats => {
                "rcx+rdx, 2..64 u32 stores, >=2 calls, a loop, no '=' immediates"
            }
            Self::Dispatch => {
                "table-driven branch (indexed or relative-offset) or a long compare chain on one register"
            }
            Self::EnvString => "stored '=' immediate, printable immediates, >=3 args, loop store",
        }
    }

    pub fn score(self, facts: &Facts) -> u32 {
        match self {
            Self::GetSubscribedApps => get_subscribed_apps(facts),
            Self::UserStats => user_stats(facts),
            Self::Dispatch => dispatch(facts),
            Self::EnvString => env_string(facts),
        }
    }
}

/// True when the argument's first use dereferences it.
fn is_pointer(facts: &Facts, position: u8) -> bool {
    matches!(
        facts.args.slot(position).and_then(|slot| slot.first_use),
        Some(FirstUse::MemoryAddress) | Some(FirstUse::HomeSlot(_))
    )
}

fn get_subscribed_apps(facts: &Facts) -> u32 {
    // Gate: the profile is only meaningful for a function that writes 32-bit
    // values through its second argument inside a rotation. Without that, the
    // absence bonuses below would score every function in the module.
    if facts.u32_stores_via_arg[1] == 0 || facts.loops == 0 {
        return 0;
    }
    let used = facts.args.used_mask();
    let mut score = 0;
    if used[0] {
        score += 2;
    }
    if used[1] {
        score += if is_pointer(facts, 1) { 3 } else { 1 };
    }
    if used[2] {
        score += 2;
    }
    if used[3] {
        score += 1;
    }
    // A copy loop has a handful of store sites, not eighteen: an unrolled or
    // accumulated writer of the same size is not this shape.
    score += match facts.u32_stores_via_arg[1] {
        0 => 0,
        1..=4 => 4,
        5..=8 => 2,
        _ => 0,
    };
    score
        // Exactly one rotation: `for` over a container, not a nested scan.
        + u32::from(facts.loops == 1) * 3
        + u32::from(facts.loop_stores_via_arg >= 1) * 2
        // Reads the container through `this` ...
        + u32::from(facts.u32_loads_via_arg[0] >= 1) * 2
        // ... and never reads back through the out pointer, which is write-only.
        + u32::from(facts.u32_loads_via_arg[1] == 0) * 2
        // Nothing else: an out-parameter copy needs no callee.
        + u32::from(facts.calls == 0) * 2
        + u32::from(facts.equals_imm_stores == 0)
        // Four arguments in registers, so nothing is read off the stack ...
        + u32::from(facts.stack_argument_reads == 0) * 2
        // ... and no arithmetic: a resampler or checksum has both.
        + u32::from(facts.multiplies == 0) * 2
        // The two signatures the Linux sibling has: a `bool` argument read as a
        // byte, and a loop over 9-qword licence entries (`i * 9`).
        + u32::from(facts.byte_argument_reads > 0) * 2
        + u32::from(facts.stride9_leas > 0) * 4
}

fn user_stats(facts: &Facts) -> u32 {
    // Gate: at least `(this, appid)` and some 32-bit memory traffic through
    // `this`. This profile is deliberately weak - it is here to show which
    // functions have the right *smell*, and every one of them still has to be
    // read.
    if facts.args.argument_count() < 2 || facts.u32_stores == 0 {
        return 0;
    }
    let used = facts.args.used_mask();
    let mut score = u32::from(used[0]) + u32::from(used[1]) + u32::from(used[2]);
    if (2..=64).contains(&facts.u32_stores) {
        score += 2;
    }
    score
        + u32::from(facts.calls >= 2)
        + u32::from(facts.loops >= 1)
        + u32::from(facts.equals_imm_stores == 0)
        + u32::from(facts.u32_loads_via_arg[0] >= 1)
}

fn dispatch(facts: &Facts) -> u32 {
    // Gate: a table-driven branch or a compare chain on one register.
    if !facts.computed_branch() && facts.chain_len < 3 {
        return 0;
    }
    let mut score = 0;
    if facts.computed_branch() {
        score += 8;
    } else if facts.indirect_branches > 0 {
        score += 2;
    }
    if facts.narrow_compare_chain(CHAIN_MINIMUM, NARROW_RANGE) {
        score += 6;
    } else if facts.chain_len >= 3 {
        score += 2;
    }
    if facts.chain_len >= 16 {
        score += 3;
    }
    if facts.chain_len >= 48 {
        score += 3;
    }
    // A dispatch chain is straight-line: it is entered with a message id and
    // leaves through one of many handlers. A loop around the compares is a
    // parser (protobuf field numbers look identical otherwise), so a function
    // that also loops does not get this.
    if facts.chain_len >= NARROW_CHAIN && facts.loops <= 1 {
        score += 3;
    }
    score
}

fn env_string(facts: &Facts) -> u32 {
    let mut score = u32::from(facts.equals_imm_stores >= 1) * 6
        + u32::from(facts.printable_imm_stores >= 2) * 3;
    // Without a stored `'='` or printable immediates there is no string build
    // here at all, whatever else the body does.
    if score == 0 {
        return 0;
    }
    let arity = facts.args.argument_count();
    if arity >= 3 {
        score += 2;
    }
    if arity >= 4 {
        score += 1;
    }
    score + u32::from(facts.loop_stores_via_arg >= 1) * 2
}

/// Indices of the best-scoring facts, best first. Ties break on the lower RVA so
/// the shortlist is stable across runs.
pub fn rank(probe: Probe, facts: &[Facts], top: usize) -> Vec<(u32, usize)> {
    let mut scored = facts
        .iter()
        .enumerate()
        .map(|(index, facts)| (probe.score(facts), index))
        .filter(|(score, _)| *score > 0)
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(facts[a.1].begin.cmp(&facts[b.1].begin)));
    scored.truncate(top);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe_facts::measure_code;

    fn facts(code: &[u8]) -> Facts {
        measure_code(code, 0x1000, 0x1000 + code.len() as u32)
    }

    #[test]
    fn parses_every_probe_name() {
        for probe in Probe::ALL {
            assert_eq!(Probe::parse(probe.name()), Some(probe));
        }
        assert_eq!(Probe::parse("nope"), None);
    }

    #[test]
    fn an_error_return_scores_nothing() {
        // `xor eax, eax` / `ret`: no argument use, no store, no branch.
        let empty = facts(&[0x31, 0xC0, 0xC3]);
        for probe in Probe::ALL {
            assert_eq!(probe.score(&empty), 0, "{}", probe.name());
        }
    }

    #[test]
    fn an_out_array_writer_beats_a_plain_reader() {
        // mov [rdx], eax / lea rdx, [rdx+4] / mov [rdx], eax / jne / ret
        let writer = facts(&[
            0x89, 0x02, 0x48, 0x8D, 0x52, 0x04, 0x89, 0x02, 0x75, 0xF9, 0xC3,
        ]);
        // mov rdx, rcx / test rdx, rdx / ret
        let reader = facts(&[0x48, 0x8B, 0xD1, 0x48, 0x85, 0xD2, 0xC3]);
        let writer_score = Probe::GetSubscribedApps.score(&writer);
        assert!(
            writer_score > Probe::GetSubscribedApps.score(&reader),
            "{writer_score}"
        );
        assert!(writer_score >= 9, "{writer_score}");
        // The writer is not an environment builder.
        assert_eq!(Probe::EnvString.score(&writer), 0);
    }

    #[test]
    fn a_literal_equals_store_scores_for_the_environment_writer() {
        // mov byte ptr [rcx], 0x3d / mov byte ptr [rcx+1], 'A' / ret
        let builder = facts(&[0xC6, 0x01, 0x3D, 0xC6, 0x41, 0x01, 0x41, 0xC3]);
        assert!(Probe::EnvString.score(&builder) >= 9);
        assert_eq!(Probe::Dispatch.score(&builder), 0);
    }

    #[test]
    fn rank_is_ordered_and_drops_zero_scores() {
        let empty = facts(&[0x31, 0xC0, 0xC3]);
        let mut builder = facts(&[0xC6, 0x01, 0x3D, 0xC6, 0x41, 0x01, 0x41, 0xC3]);
        builder.begin = 0x2000;
        let all = vec![empty, builder];
        let ranked = rank(Probe::EnvString, &all, 8);
        assert_eq!(ranked.len(), 1);
        assert_eq!(all[ranked[0].1].begin, 0x2000);
    }
}
