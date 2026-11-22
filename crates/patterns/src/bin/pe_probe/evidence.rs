//! Turning measurements into printed evidence: corpus names for callees, a
//! cross-build comparison, and the per-candidate block the human reads.

use std::path::Path;

use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::probe_facts::{self, Facts};
use vapor_forge_patterns::registry::parse_toml_patterns;
use vapor_forge_patterns::scan::{
    group_variants, resolve_entry_group_with, select_variants_for_library_path, PatternRef,
};

use super::table::function_table;
use super::MIN_SIZE;

/// The mined corpus of the build being probed, as `entry RVA -> symbol name`.
pub fn anchors_of(path: &Path, image: &PeImage<'_>) -> Result<Vec<(u32, String)>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let parsed = parse_toml_patterns(&text)?;
    let refs = parsed
        .iter()
        .map(PatternRef::from)
        .collect::<Vec<PatternRef<'_>>>();
    let segment = image.code_segment()?;
    let selected = select_variants_for_library_path(&refs, path);
    let mut anchors = Vec::new();
    for group in group_variants(&selected) {
        let name = group[0].name.to_owned();
        match resolve_entry_group_with(segment.bytes, u32::from(segment.bits), &group, None) {
            Ok(result) => {
                if let Ok(rva) = u32::try_from(segment.vaddr + result.target_offset as u64) {
                    anchors.push((rva, name));
                }
            }
            Err(error) => eprintln!("anchor {name}: unresolved ({error:?})"),
        }
    }
    Ok(anchors)
}

/// Every function of another build, keyed twice: an exact fingerprint (a
/// rebuild that kept everything, including extents) and a shape fingerprint (a
/// rebuild that renumbered instructions but kept the behaviour). A candidate
/// with a unique origin in the other build is the one worth re-mining; one with
/// none is probably shaped by this build's code layout alone.
pub struct Counterparts {
    entries: Vec<(u64, u64, u32)>,
}

impl Counterparts {
    pub fn measure(path: &Path) -> Result<Self, String> {
        let bytes =
            std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
        let image = PeImage::parse(&bytes)?;
        let table = function_table(&image, &bytes)?;
        let mut entries = Vec::with_capacity(table.len());
        for function in table {
            if function.end - function.begin < MIN_SIZE {
                continue;
            }
            let facts = probe_facts::measure(&image, function.begin, function.end);
            entries.push((exact(&facts), shape(&facts), facts.begin));
        }
        Ok(Self { entries })
    }

    /// `exact <rva>` when one function keeps everything, else `shape <n>`
    /// candidates, else `none`.
    fn describe(&self, facts: &Facts) -> String {
        let (exact_key, shape_key) = (exact(facts), shape(facts));
        let exacts = self
            .entries
            .iter()
            .filter(|(key, _, _)| *key == exact_key)
            .map(|(_, _, rva)| *rva)
            .collect::<Vec<_>>();
        if exacts.len() == 1 {
            return format!("exact {:#x}", exacts[0]);
        }
        let shapes = self
            .entries
            .iter()
            .filter(|(_, key, _)| *key == shape_key)
            .map(|(_, _, rva)| *rva)
            .collect::<Vec<_>>();
        match shapes.len() {
            0 => "none".to_owned(),
            1 => format!("shape {:#x}", shapes[0]),
            count => format!("shape {count} candidates"),
        }
    }
}

pub fn counterparts(path: &Path) -> Result<Counterparts, String> {
    Counterparts::measure(path)
}

fn fnv() -> u64 {
    0xcbf2_9ce4_8422_2325
}

fn mix(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x100_0000_01b3);
}

/// Behaviour plus extent: identical only if nothing moved.
pub fn exact(facts: &Facts) -> u64 {
    let mut hash = fnv();
    mix(&mut hash, u64::from(facts.end - facts.begin));
    mix(&mut hash, u64::from(facts.insns));
    let behaviour = shape(facts);
    mix(&mut hash, behaviour);
    hash
}

/// Behaviour only, so a rebuild that renumbered instructions still matches.
pub fn shape(facts: &Facts) -> u64 {
    let mut hash = fnv();
    mix(&mut hash, u64::from(facts.args.argument_count()));
    for count in facts.u32_stores_via_arg {
        mix(&mut hash, u64::from(count));
    }
    mix(&mut hash, u64::from(facts.loops));
    mix(&mut hash, u64::from(facts.equals_imm_stores));
    mix(&mut hash, u64::from(facts.printable_imm_stores));
    mix(&mut hash, u64::from(facts.chain_len));
    mix(&mut hash, u64::from(facts.stride9_leas.min(8)));
    mix(&mut hash, u64::from(facts.byte_argument_reads.min(8)));
    mix(&mut hash, u64::from(facts.computed_branch()));
    mix(&mut hash, u64::from(facts.calls));
    hash
}

pub fn print_candidate(
    place: usize,
    score: u32,
    facts: &Facts,
    anchors: &[(u32, String)],
    other: Option<&Counterparts>,
) {
    let size = facts.end.saturating_sub(facts.begin);
    println!(
        "\n#{place} rva {:#010x} size {size:#x} insns {} score {score}",
        facts.begin, facts.insns
    );
    println!(
        "   args: {} (arity {})",
        facts.args.summary(),
        facts.args.argument_count()
    );
    println!(
        "   loads: u32 through rcx {} rdx {} r8 {} r9 {}",
        facts.u32_loads_via_arg[0],
        facts.u32_loads_via_arg[1],
        facts.u32_loads_via_arg[2],
        facts.u32_loads_via_arg[3]
    );
    println!(
        "   stores: u32 {} through rcx {} rdx {} r8 {} r9 {}; loops {} loop-stores-through-args {}",
        facts.u32_stores,
        facts.u32_stores_via_arg[0],
        facts.u32_stores_via_arg[1],
        facts.u32_stores_via_arg[2],
        facts.u32_stores_via_arg[3],
        facts.loops,
        facts.loop_stores_via_arg
    );
    println!(
        "   shape: multiplies {} stack-arg-reads {} stride9 {} byte-arg-reads {}",
        facts.multiplies, facts.stack_argument_reads, facts.stride9_leas, facts.byte_argument_reads
    );
    println!(
        "   strings: byte-imm {} '='-imm {} printable-imm {}",
        facts.byte_imm_stores, facts.equals_imm_stores, facts.printable_imm_stores
    );
    println!(
        "   dispatch: chain {} max {:#x} cmp-imms {} table-loads {} indirect-branches {} \
(register {}) cond-branches {}",
        facts.chain_len,
        facts.chain_max_imm,
        facts.cmp_imms,
        facts.table_loads,
        facts.indirect_branches,
        facts.register_branches,
        facts.conditional_branches
    );
    println!(
        "   calls {} (indirect {}){}",
        facts.calls,
        facts.indirect_calls,
        names(&facts.callees, anchors)
    );
    if let Some(other) = other {
        println!("   xbuild: {}", other.describe(facts));
    }
}

/// Direct callees named through the corpus, or `-` for one that is not anchored.
fn names(callees: &[u32], anchors: &[(u32, String)]) -> String {
    if callees.is_empty() {
        return String::new();
    }
    let named = callees
        .iter()
        .map(|target| {
            let name = anchors
                .iter()
                .find(|(rva, _)| rva == target)
                .map(|(_, name)| name.as_str())
                .unwrap_or("-");
            format!("{target:#x}({name})")
        })
        .collect::<Vec<_>>();
    format!(": {}", named.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exact_key_changes_when_only_the_extent_changes() {
        let mut facts = Facts {
            end: 0x40,
            insns: 12,
            ..Facts::default()
        };
        let before = exact(&facts);
        facts.end = 0x48;
        assert_ne!(before, exact(&facts));
        // The shape key ignores extent and instruction count.
        assert_eq!(
            shape(&facts),
            shape(&Facts {
                end: 0x80,
                insns: 40,
                ..Facts::default()
            })
        );
    }

    #[test]
    fn names_render_the_corpus_or_a_dash() {
        let callees = [0x10, 0x20];
        let anchors = [(0x20, "CUser::CheckAppOwnership".to_owned())];
        let text = names(&callees, &anchors);
        assert!(text.contains("0x10(-)"), "{text}");
        assert!(text.contains("0x20(CUser::CheckAppOwnership)"), "{text}");
        assert_eq!(names(&[], &anchors), "");
    }
}
