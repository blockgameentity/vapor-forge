//! Vtable-slot inspection for Windows Steam modules.
//!
//! The four Gate-0 targets have no published signature hint, so this tool takes
//! the class route instead of the body route: a class's virtual methods are
//! consecutive function pointers in a read-only section, in a fixed order per
//! compiler, so the table that holds an *anchored* sibling pins the class and
//! the slot order the missing method lives in.
//!
//! * `--corpus <toml>` resolves the mined anchors of this build and, for each
//!   one that sits in a table, prints the table base, slot index and length,
//!   then a one-line structural summary of every other slot.
//! * `--class <name>` selects tables by MSVC RTTI class name, which is how a
//!   class whose anchors are not virtual at all still becomes reachable.
//! * `--against <dll>` measures slot-index stability between two builds: per
//!   slot, whether the structure survived at the same index.
//!
//! Every summary is evidence, not identity. Nothing here is confirmed without a
//! live client; see `res/patterns/win64/VTABLE-REPORT.md`.
//!
//! Split into modules because the rendering rules and the candidate choice are
//! worth unit tests, and `main` should stay orchestration only.

mod anchors;
mod args;
mod report;
mod stability;

use std::collections::BTreeMap;
use std::path::Path;
use std::process::exit;

use vapor_forge_patterns::pdata::Pdata;
use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::vtable_pe::{self, Enumeration, Table};

use args::Args;
use report::{build_labels, dedupe, describe, rejection_breakdown, report_table};
use stability::OtherBuild;

fn main() {
    if let Err(error) = run() {
        eprintln!("vapor-forge-vtable-pe: {error}");
        exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1))?;
    let bytes = read(&args.dll)?;
    let image = PeImage::parse(&bytes)?;
    let functions =
        vapor_forge_patterns::pdata::parse(&image, &bytes).map_err(|error| error.to_string())?;
    let enumeration = vtable_pe::enumerate(&image, &bytes, &functions)?;

    println!(
        "{} [{}]: {} .pdata extents, {} tables ({} with RTTI), {} runs rejected{}",
        args.dll.display(),
        args.module,
        functions.len(),
        enumeration.tables.len(),
        enumeration.with_class(),
        enumeration.rejected.len(),
        rejection_breakdown(&enumeration)
    );

    let anchors = match args.corpus.as_deref() {
        Some(path) => anchors::resolve_anchors(path, &image, &functions, &args.module)?,
        None => BTreeMap::new(),
    };
    // Corpus names are the strongest label; every other table slot is labelled
    // `Class#slot`, so a callee that is a known virtual method of a known class
    // is visible without a hint for it.
    let labels = build_labels(&enumeration, &anchors);

    let mut pending: Vec<&Table> = Vec::new();
    for (rva, name) in &anchors {
        let holders = vtable_pe::containing(&enumeration.tables, *rva);
        match holders.first() {
            None => println!("anchor {name:<42} {rva:#010x} -> no enumerated vtable holds it"),
            Some(_) => {
                for table in holders {
                    println!(
                        "anchor {name:<42} {rva:#010x} -> table {:#010x} slot {}/{}{}",
                        table.base_rva,
                        table.slot_of(*rva).unwrap_or(0),
                        table.len(),
                        describe(table)
                    );
                    pending.push(table);
                }
            }
        }
    }

    if let Some(class) = args.class.as_deref() {
        let selected = vtable_pe::of_class(&enumeration.tables, class);
        if selected.is_empty() {
            println!("no vtable has an RTTI class matching {class:?}");
        }
        for table in selected {
            println!(
                "class table {:#010x} len {}{}",
                table.base_rva,
                table.len(),
                describe(table)
            );
            report_table(&image, &functions, table, &anchors, &labels, &args);
        }
    }

    // Which tables hold a given function: the inverse of an anchor, used to
    // check whether a body reached by a call site is a virtual method at all.
    // An interior address is resolved to its `.pdata` entry first, because that
    // is what a cross-reference from a disassembly gives you.
    for rva in &args.holds {
        let mut probe = *rva;
        let mut interior = None;
        if vtable_pe::containing(&enumeration.tables, probe).is_empty() {
            if let Some(entry) = functions.containing(*rva) {
                if entry.begin < *rva {
                    interior = Some(*rva - entry.begin);
                    probe = entry.begin;
                }
            }
        }
        let holders = vtable_pe::containing(&enumeration.tables, probe);
        let prefix = match interior {
            Some(delta) => format!("holds {rva:#010x} -> entry {probe:#010x} (+{delta:#x})"),
            None => format!("holds {rva:#010x}"),
        };
        if holders.is_empty() {
            println!("{prefix}: not a slot of any enumerated table");
        }
        for table in holders {
            println!(
                "{prefix}: table {:#010x} slot {}/{}{}",
                table.base_rva,
                table.slot_of(probe).unwrap_or(0),
                table.len(),
                describe(table)
            );
        }
    }

    for table in dedupe(pending) {
        report_table(&image, &functions, table, &anchors, &labels, &args);
    }

    if let Some(against) = args.against.as_deref() {
        cross_build(&args, &image, &functions, &enumeration, &anchors, against)?;
    }
    Ok(())
}

fn cross_build(
    args: &Args,
    image: &PeImage<'_>,
    functions: &Pdata,
    enumeration: &Enumeration,
    anchors: &BTreeMap<u32, String>,
    against: &Path,
) -> Result<(), String> {
    let other_bytes = read(against)?;
    let other_image = PeImage::parse(&other_bytes)?;
    let other_functions = vapor_forge_patterns::pdata::parse(&other_image, &other_bytes)
        .map_err(|error| error.to_string())?;
    let other = vtable_pe::enumerate(&other_image, &other_bytes, &other_functions)?;
    let other_anchors = match args.corpus_against.as_deref() {
        Some(path) => anchors::resolve_anchors(path, &other_image, &other_functions, &args.module)?,
        None => BTreeMap::new(),
    };
    println!(
        "\n=== cross-build: {} ({} tables)",
        against.display(),
        other.tables.len()
    );
    let other_build = OtherBuild {
        image: &other_image,
        functions: &other_functions,
        tables: &other.tables,
    };
    let mut reported = 0usize;
    for (rva, name) in anchors {
        let holders = vtable_pe::containing(&enumeration.tables, *rva);
        let Some(table) = holders.first() else {
            continue;
        };
        let counterpart = other_anchors
            .iter()
            .find(|(_, other_name)| *other_name == name)
            .map(|(other_rva, _)| *other_rva)
            .and_then(|other_rva| {
                let candidates = vtable_pe::containing(&other.tables, other_rva);
                candidates
                    .first()
                    .map(|other_table| (*other_table, Some(other_rva)))
            });
        stability::report_stability(name, table, counterpart, image, functions, &other_build);
        reported += 1;
    }
    for class in args.class.iter() {
        for table in vtable_pe::of_class(&enumeration.tables, class) {
            stability::report_stability(class, table, None, image, functions, &other_build);
            reported += 1;
        }
    }
    println!("cross-build: {reported} table(s) compared");
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))
}
