//! Cross-build slot stability report.
//!
//! The alignment itself lives in [`vapor_forge_patterns::vtable_pe::cross_build`]
//! and is unit-tested there; this module only chooses candidates and renders the
//! answer. A candidate is the table the same anchor landed in when the other
//! build's corpus resolves it, otherwise every table of the same RTTI class (or
//! of the same length when the class has no RTTI).

use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::vtable_pe::cross_build::{self, Candidate};
use vapor_forge_patterns::vtable_pe::structure;
use vapor_forge_patterns::vtable_pe::{Functions, Table};

/// The other build, borrowed once per run.
pub struct OtherBuild<'a> {
    pub image: &'a PeImage<'a>,
    pub functions: &'a Functions,
    pub tables: &'a [Table],
}

pub fn report_stability(
    name: &str,
    table: &Table,
    counterpart: Option<(&Table, Option<u32>)>,
    image: &PeImage<'_>,
    functions: &Functions,
    other: &OtherBuild<'_>,
) {
    let shapes = table_shapes(image, functions, table);
    let candidates = candidates_for(counterpart, table, other);
    let Some(matched) = cross_build::best_match(table, &shapes, &candidates) else {
        println!(
            "{name}: table {:#010x} len {} has no candidate in the other build",
            table.base_rva,
            table.len()
        );
        return;
    };
    let class = candidates
        .iter()
        .find(|candidate| candidate.base_rva == matched.b_base_rva)
        .and_then(|candidate| candidate.class.as_deref());
    println!(
        "{name}: table {:#010x} len {} -> {:#010x} len {} class({}) | same length {} | slot \
agreement {}/{} ({:.0}%) | credible {}",
        table.base_rva,
        table.len(),
        matched.b_base_rva,
        matched.b_len,
        class.unwrap_or("-"),
        if matched.same_length { "yes" } else { "no" },
        matched.agreement,
        matched.comparable,
        100.0 * matched.agreement_fraction(),
        if matched.credible() { "yes" } else { "no" }
    );
    let total_disagreements = matched.disagreements().count();
    let mut disagreements = matched
        .disagreements()
        .map(|slot| {
            format!(
                "{}({:#x}->{:#x})",
                slot.slot,
                slot.a_rva,
                slot.b_rva.unwrap_or(0)
            )
        })
        .collect::<Vec<_>>();
    disagreements.truncate(40);
    if !disagreements.is_empty() {
        println!(
            "  {total_disagreements} slot(s) changed shape at the same index: {}",
            disagreements.join(" ")
        );
    }
}

fn candidates_for(
    counterpart: Option<(&Table, Option<u32>)>,
    table: &Table,
    other: &OtherBuild<'_>,
) -> Vec<Candidate> {
    let selected: Vec<&Table> = match counterpart {
        Some((other_table, _)) => vec![other_table],
        None => other
            .tables
            .iter()
            .filter(|candidate| match table.class() {
                Some(class) => candidate.class() == Some(class),
                None => candidate.len() == table.len(),
            })
            .collect(),
    };
    selected
        .into_iter()
        .map(|candidate| {
            Candidate::from_table(
                candidate,
                table_shapes(other.image, other.functions, candidate),
                candidate.corroboration.rtti_class.clone(),
            )
        })
        .collect()
}

fn table_shapes(image: &PeImage<'_>, functions: &Functions, table: &Table) -> Vec<Option<u64>> {
    table
        .entries
        .iter()
        .map(|rva| {
            let (begin, end) = functions
                .extent(*rva)
                .unwrap_or((*rva, rva.saturating_add(0x20)));
            structure::shape_of(image, begin, end)
        })
        .collect()
}
