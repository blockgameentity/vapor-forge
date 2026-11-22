//! Slot rendering: one line per slot, plus a probe shortlist for long tables.
//!
//! The line carries the measurements and nothing else, so a reader can argue
//! with the evidence instead of the ordering: size from `.pdata`, the MS-x64
//! argument shape, store widths, loop/call counts, the licence-stride counters,
//! compare-chain length and the labelled direct callees. Probe scores are
//! appended in brackets as an ordering aid only.

use std::collections::BTreeMap;

use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::probe_rank::Probe;
use vapor_forge_patterns::vtable_pe::structure::{self, SlotReport};
use vapor_forge_patterns::vtable_pe::{Enumeration, Functions, Rejection, Table};

use crate::args::{Args, SHORTLIST_PER_PROBE};

/// A table longer than this is printed as a probe shortlist unless
/// `--all-slots` is given, so a 261-slot class stays readable.
const SUMMARY_SLOTS: usize = 48;

pub fn describe(table: &Table) -> String {
    let corroboration = &table.corroboration;
    format!(
        " [{}] class({}) pdata {}/{} col {}",
        table.section,
        table.class().unwrap_or("-"),
        corroboration.pdata_entries,
        table.len(),
        if corroboration.preceded_by_data_pointer {
            "yes"
        } else {
            "no"
        }
    )
}

pub fn rejection_breakdown(enumeration: &Enumeration) -> String {
    let (mut few, mut many) = (0usize, 0usize);
    for rejected in &enumeration.rejected {
        match rejected.reason {
            Rejection::TooFewSlots { .. } => few += 1,
            Rejection::TooManySlots { .. } => many += 1,
            Rejection::NonTextPointer { .. } => {}
        }
    }
    format!(" (too few {few}, too many {many})")
}

pub fn dedupe(tables: Vec<&Table>) -> Vec<&Table> {
    let mut seen: Vec<u32> = Vec::new();
    let mut out = Vec::new();
    for table in tables {
        if !seen.contains(&table.base_rva) {
            seen.push(table.base_rva);
            out.push(table);
        }
    }
    out
}

/// Corpus names first, then `Class#slot` for every other table entry.
pub fn build_labels(
    enumeration: &Enumeration,
    anchors: &BTreeMap<u32, String>,
) -> BTreeMap<u32, String> {
    let mut labels = anchors.clone();
    for table in &enumeration.tables {
        let Some(class) = table.class() else {
            continue;
        };
        for (slot, rva) in table.entries.iter().enumerate() {
            labels
                .entry(*rva)
                .or_insert_with(|| format!("{class}#{slot}"));
        }
    }
    labels
}

pub fn report_table(
    image: &PeImage<'_>,
    functions: &Functions,
    table: &Table,
    anchors: &BTreeMap<u32, String>,
    labels: &BTreeMap<u32, String>,
    args: &Args,
) {
    let reports = summarise_slots(image, functions, table, labels);
    let all = args.all_slots || table.len() <= SUMMARY_SLOTS;
    for report in &reports {
        if !all && !is_shortlisted(&reports, report.slot) {
            continue;
        }
        let anchor = anchors
            .get(&report.rva)
            .map(|name| format!("  * anchor:{name}"))
            .unwrap_or_default();
        let scores = Probe::ALL
            .iter()
            .map(|probe| format!("{}:{}", probe.name(), probe.score(&report.facts)))
            .collect::<Vec<_>>()
            .join(" ");
        println!("  {} [{scores}]{anchor}", report.line());
    }
    if !all {
        println!(
            "  ({} slots; shortlist shown, use --all-slots for every slot)",
            table.len()
        );
    }
    print_shortlist(&reports, args.top);
}

pub fn summarise_slots(
    image: &PeImage<'_>,
    functions: &Functions,
    table: &Table,
    labels: &BTreeMap<u32, String>,
) -> Vec<SlotReport> {
    table
        .entries
        .iter()
        .enumerate()
        .map(|(slot, rva)| structure::summarise(image, functions, slot, *rva, labels))
        .collect()
}

pub fn is_shortlisted(reports: &[SlotReport], slot: usize) -> bool {
    Probe::ALL
        .iter()
        .any(|probe| top_slots(reports, *probe, SHORTLIST_PER_PROBE).contains(&slot))
}

pub fn top_slots(reports: &[SlotReport], probe: Probe, top: usize) -> Vec<usize> {
    let mut scored: Vec<(u32, usize)> = reports
        .iter()
        .map(|report| (probe.score(&report.facts), report.slot))
        .collect();
    scored.sort_by_key(|entry| std::cmp::Reverse(*entry));
    scored.truncate(top);
    scored.into_iter().map(|(_, slot)| slot).collect()
}

fn print_shortlist(reports: &[SlotReport], top: usize) {
    for probe in Probe::ALL {
        let mut scored: Vec<(u32, &SlotReport)> = reports
            .iter()
            .map(|report| (probe.score(&report.facts), report))
            .collect();
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        let rendered = scored
            .iter()
            .take(top)
            .filter(|(score, _)| *score > 0)
            .map(|(score, report)| format!("slot {} {:#x} score {score}", report.slot, report.rva))
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            println!("  best for probe {}: {}", probe.name(), rendered.join("; "));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::args::Args;
    use std::path::PathBuf;

    fn args() -> Args {
        Args {
            dll: PathBuf::from("a.dll"),
            module: "steamclient".to_owned(),
            corpus: None,
            corpus_against: None,
            against: None,
            class: None,
            all_slots: false,
            top: 4,
        }
    }

    #[test]
    fn args_fixture_is_the_default_shape() {
        assert_eq!(args().top, 4);
        assert!(!args().all_slots);
    }
}
