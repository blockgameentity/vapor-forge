//! Function-table aware inspection of Windows Steam modules.
//!
//! PE32+ x64 images carry an exact function table in the exception directory
//! (`.pdata`, `RUNTIME_FUNCTION`), so on Windows there is no need to guess
//! function boundaries from prologues the way the ELF path does. This tool uses
//! that table to answer the three Gate-0 follow-up questions:
//!
//! * `--anchors`: does every pattern in a mined corpus land on a real function
//!   *entry*, and how big is the function it claims?
//! * `--callees`: which functions does an anchored function call? Structural
//!   derivation for symbols that were never published as hints.
//! * `--against`: is a function's *shape* preserved across two Steam builds even
//!   when its bytes are not? That is what makes re-mining mechanical.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::exit;

use iced_x86::{Code, Decoder, DecoderOptions, FlowControl, Instruction, OpKind, Register};
use vapor_forge_patterns::pdata::{self, Function, Pdata};
use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::registry::parse_toml_patterns;
use vapor_forge_patterns::scan::{
    group_variants, resolve_entry_group_with, select_variants_for_library_path, PatternRef,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("vapor-forge-pe-functions: {error}");
        exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1))?;
    let bytes = std::fs::read(&args.dll)
        .map_err(|error| format!("read {}: {error}", args.dll.display()))?;
    let image = PeImage::parse(&bytes)?;
    let table = pdata::parse(&image, &bytes).map_err(|error| error.to_string())?;
    let anchors = args
        .toml
        .as_deref()
        .map(|path| anchors_of(path, &image, &bytes))
        .transpose()?
        .unwrap_or_default();

    println!(
        "{}: {} functions, text {:#x}..{:#x}",
        args.dll.display(),
        table.len(),
        table.functions().first().map(|f| f.begin).unwrap_or(0),
        table.functions().last().map(|f| f.end).unwrap_or(0)
    );
    if args.metrics {
        print_metrics(&image, &table);
    }

    if let Some(against) = args.against.as_deref() {
        report_shape_match(&table, &image, against, &anchors)?;
    }
    if let Some(rva) = args.near {
        report_near(&table, &image, rva, args.window, &anchors);
    }
    if !anchors.is_empty() {
        report_anchors(&image, &table, &anchors);
    }
    if let Some(rva) = args.callees {
        report_callees(&table, &image, rva, &anchors);
    }
    if let Some(rva) = args.dump {
        dump_function(&table, &image, rva);
    }
    Ok(())
}

/// Resolve a mined corpus against the image and keep `name -> function begin`.
fn anchors_of(
    path: &Path,
    image: &PeImage<'_>,
    bytes: &[u8],
) -> Result<BTreeMap<String, u32>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let parsed = parse_toml_patterns(&text)?;
    let refs = parsed
        .iter()
        .map(PatternRef::from)
        .collect::<Vec<PatternRef<'_>>>();
    let segment = image.code_segment()?;
    let selected = select_variants_for_library_path(&refs, path);
    let module = parsed
        .first()
        .map(|(_, entry)| entry.module.clone())
        .unwrap_or_else(|| "steamclient".to_owned());
    let mut anchors = BTreeMap::new();
    for group in group_variants(&selected) {
        let name = group[0].name.to_owned();
        match resolve_entry_group_with(segment.bytes, u32::from(segment.bits), &group, None) {
            Ok(result) => {
                let rva = u32::try_from(segment.vaddr + result.target_offset as u64)
                    .map_err(|_| "resolved rva exceeds u32".to_owned())?;
                anchors.insert(name, rva);
            }
            Err(error) => println!("anchor {name}: unresolved ({error:?})"),
        }
    }
    let _ = bytes;
    let _ = module;
    Ok(anchors)
}

fn report_anchors(image: &PeImage<'_>, table: &Pdata, anchors: &BTreeMap<String, u32>) {
    let mut on_entry = 0usize;
    let mut gated = 0usize;
    for (name, rva) in anchors {
        let verdict = pdata::verify(image, table, *rva);
        // `Unmapped` and `MidFunction` are both off-entry; `MidFunction` names
        // the extent the hit fell inside.
        if matches!(
            verdict,
            pdata::AnchorVerdict::Verified { .. } | pdata::AnchorVerdict::NoTerminator { .. }
        ) {
            on_entry += 1;
        }
        match verdict {
            pdata::AnchorVerdict::Verified { function, body } => {
                gated += 1;
                println!(
                    "  {name:<44} {rva:#010x} entry ok  size {:<6} body: {}",
                    function.len(),
                    body.label()
                );
            }
            pdata::AnchorVerdict::MidFunction { function } => println!(
                "  {name:<44} {rva:#010x} NOT an entry (inside {:#x}, +{:#x})",
                function.begin,
                *rva - function.begin
            ),
            pdata::AnchorVerdict::NoTerminator { function, body } => println!(
                "  {name:<44} {rva:#010x} entry ok  size {:<6} body: {} (gate rejects)",
                function.len(),
                body.label()
            ),
            pdata::AnchorVerdict::Unmapped => {
                println!("  {name:<44} {rva:#010x} NOT an entry (not in .pdata)")
            }
            // Unreachable through the strict `verify`: a leaf is only returned
            // to a caller that passes uniqueness evidence.
            pdata::AnchorVerdict::Leaf { body } => println!(
                "  {name:<44} {rva:#010x} leaf       body: {} (accepted)",
                body.label()
            ),
        }
    }
    println!(
        "anchors on a function entry: {on_entry}/{}; passing the body gate: {gated}/{}",
        anchors.len(),
        anchors.len()
    );
}

/// Dump the table metrics the merge rule is defined against, plus what a linear
/// decode of every extent says about it. This is the evidence `README.md` quotes.
fn print_metrics(image: &PeImage<'_>, table: &Pdata) {
    let metrics = table.metrics();
    println!(".pdata: {metrics}");
    let (terminating, thunks, unbacked) = pdata::body_summary(image, table);
    println!(
        "  bodies: {terminating} terminate (ret or tail jump), {thunks} are tail-jump thunks \
         (<= {:#x} B), {unbacked} extents are not file-backed",
        pdata::THUNK_MAX_BYTES
    );
}

fn report_callees(table: &Pdata, image: &PeImage<'_>, rva: u32, anchors: &BTreeMap<String, u32>) {
    let Some(function) = table.functions().iter().find(|f| f.begin == rva) else {
        println!("rva {rva:#x} is not a function entry");
        return;
    };
    let by_target: BTreeMap<u32, &str> = anchors.iter().map(|(n, r)| (*r, n.as_str())).collect();
    let mut callers: BTreeMap<u32, usize> = BTreeMap::new();
    for info in disassemble(image, *function) {
        if info.flow != FlowControl::Call && info.flow != FlowControl::IndirectCall {
            continue;
        }
        let Some(target) = info.branch_target else {
            continue;
        };
        *callers.entry(target).or_default() += 1;
    }
    println!(
        "callees of {rva:#x}..{:#x} ({} bytes, {} direct calls)",
        function.end,
        function.end - function.begin,
        callers.values().sum::<usize>()
    );
    for (target, count) in callers {
        let entry = table.functions().iter().find(|f| f.begin == target);
        let label = by_target.get(&target).copied().unwrap_or("-");
        match entry {
            Some(f) => println!(
                "  -> {target:#010x} x{count} entry ok size {:<6} {label}",
                f.end - f.begin
            ),
            None => println!("  -> {target:#010x} x{count} not a table entry      {label}"),
        }
    }
}

struct Decoded {
    flow: FlowControl,
    branch_target: Option<u32>,
    shape: u64,
    store_bits: Option<u32>,
    is_load: bool,
}

fn disassemble(image: &PeImage<'_>, function: Function) -> Vec<Decoded> {
    let len = (function.end - function.begin) as usize;
    let Some(code) = image.bytes_at_rva(u64::from(function.begin), len) else {
        return Vec::new();
    };
    let mut decoder = Decoder::with_ip(64, code, u64::from(function.begin), DecoderOptions::NONE);
    let mut out = Vec::new();
    let mut instruction = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        let branch_target = if instruction.op0_kind() == OpKind::NearBranch64 {
            Some(instruction.near_branch64() as u32)
        } else {
            None
        };
        // A memory operand in operand 0 is a store; the instruction's memory
        // size tells us how wide. Anything else with a memory source is a load.
        let store_bits = if instruction.op0_kind() == OpKind::Memory {
            Some(memory_size_bits(&instruction))
        } else {
            None
        };
        let is_load = instruction.op0_kind() != OpKind::Memory
            && (0..instruction.op_count())
                .any(|index| instruction.op_kind(index) == OpKind::Memory);
        out.push(Decoded {
            flow: instruction.flow_control(),
            branch_target,
            shape: shape_hash(&instruction),
            store_bits,
            is_load,
        });
    }
    out
}

/// Build-shape fingerprint: everything except the operands a rebuild perturbs
/// (relative call/jump targets, RIP-relative and absolute displacements,
/// immediates). Two builds of the same function agree on this even when their
/// bytes and addresses differ.
fn memory_size_bits(instruction: &Instruction) -> u32 {
    (instruction.memory_size().size() as u32) * 8
}

fn shape_hash(instruction: &Instruction) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut mix = |value: u64| {
        hash ^= value;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    };
    mix(u64::from(instruction.code() as u32));
    mix(u64::from(instruction.op_count()));
    for index in 0..instruction.op_count() {
        mix(match instruction.op_kind(index) {
            OpKind::Register => u64::from(instruction.op_register(index) as u32),
            OpKind::Memory => {
                let base = instruction.memory_base();
                let reg = if base == Register::None {
                    0xFFFF
                } else {
                    u64::from(base as u32)
                };
                reg + 1
            }
            other => u64::from(other as u32) + 0x1_0000,
        });
    }
    let _ = Code::INVALID;
    hash
}

fn report_shape_match(
    table: &Pdata,
    image: &PeImage<'_>,
    against: &Path,
    anchors: &BTreeMap<String, u32>,
) -> Result<(), String> {
    let other_bytes =
        std::fs::read(against).map_err(|error| format!("read {}: {error}", against.display()))?;
    let other = PeImage::parse(&other_bytes)?;
    let other_table = pdata::parse(&other, &other_bytes).map_err(|error| error.to_string())?;
    let mut other_shapes: BTreeMap<u64, Vec<u32>> = BTreeMap::new();
    for function in other_table.functions() {
        other_shapes
            .entry(shape_of(&other, *function))
            .or_default()
            .push(function.begin);
    }
    let mut matched = 0usize;
    for function in table.functions() {
        if other_shapes.contains_key(&shape_of(image, *function)) {
            matched += 1;
        }
    }
    println!(
        "shape match vs {}: {matched}/{} functions ({:.1}%)",
        against.display(),
        table.len(),
        100.0 * matched as f64 / table.len().max(1) as f64
    );
    // Re-mining feasibility: can an anchored function be re-found in the other
    // build by shape alone (1 candidate), or does it need manual work (0) or
    // disambiguation (n)?
    if !anchors.is_empty() {
        let (mut unique, mut none, mut multi) = (0, 0, 0);
        for (name, rva) in anchors {
            let Some(function) = table.functions().iter().find(|f| f.begin == *rva) else {
                continue;
            };
            match other_shapes
                .get(&shape_of(image, *function))
                .map(|sites| sites.len())
                .unwrap_or(0)
            {
                1 => unique += 1,
                0 => {
                    none += 1;
                    println!("  re-mine {name}: no shape match in other build");
                }
                count => {
                    multi += 1;
                    println!("  re-mine {name}: {count} shape candidates");
                }
            }
        }
        println!(
            "re-mine by shape: {unique} unique, {multi} ambiguous, {none} absent (of {})",
            anchors.len()
        );
    }
    Ok(())
}

/// Print the instructions of one function (verification aid for a mined anchor).
fn dump_function(table: &Pdata, image: &PeImage<'_>, rva: u32) {
    let Some(function) = table.functions().iter().find(|f| f.begin == rva) else {
        println!("rva {rva:#x} is not a function entry");
        return;
    };
    let len = (function.end - function.begin) as usize;
    let Some(code) = image.bytes_at_rva(u64::from(function.begin), len) else {
        println!("rva {rva:#x} is not mapped");
        return;
    };
    println!(
        "dump {rva:#x}..{:#x} ({} bytes){}",
        function.end,
        len,
        if pdata::is_thunk(image, *function) {
            " [thunk: tail jmp]"
        } else {
            ""
        }
    );
    let mut decoder = Decoder::with_ip(64, code, u64::from(function.begin), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        let target = if instruction.op0_kind() == OpKind::NearBranch64 {
            format!(" -> {:#x}", instruction.near_branch64())
        } else {
            String::new()
        };
        println!(
            "  {:#010x}  {:?}{target}",
            instruction.ip(),
            instruction.code()
        );
    }
}

/// Describe the functions around an anchor: size, shape of the body, and which
/// anchored functions it calls. Used to derive symbols that were never published
/// as hints, by structural comparison with their class siblings.
fn report_near(
    table: &Pdata,
    image: &PeImage<'_>,
    rva: u32,
    window: u32,
    anchors: &BTreeMap<String, u32>,
) {
    let by_target: BTreeMap<u32, &str> = anchors.iter().map(|(n, r)| (*r, n.as_str())).collect();
    let low = rva.saturating_sub(window);
    let high = rva.saturating_add(window);
    println!("functions near {rva:#x} (+-{window:#x}):");
    for function in table
        .functions()
        .iter()
        .filter(|f| f.begin >= low && f.begin <= high)
    {
        let decoded = disassemble(image, *function);
        let stores = decoded.iter().filter(|d| d.store_bits == Some(32)).count();
        let loads = decoded.iter().filter(|d| d.is_load).count();
        let callees = decoded
            .iter()
            .filter_map(|d| d.branch_target)
            .filter(|target| by_target.contains_key(target))
            .map(|target| by_target[&target])
            .collect::<Vec<_>>();
        println!(
            "  {:#010x} size {:<5} insns {:<4} u32stores {:<3} loads {:<3} {}{}",
            function.begin,
            function.end - function.begin,
            decoded.len(),
            stores,
            loads,
            if function.begin == rva { "* " } else { "" },
            callees.join(",")
        );
    }
}

fn shape_of(image: &PeImage<'_>, function: Function) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for decoded in disassemble(image, function).iter().take(24) {
        hash ^= decoded.shape;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

#[derive(Debug)]
struct Args {
    dll: std::path::PathBuf,
    toml: Option<std::path::PathBuf>,
    callees: Option<u32>,
    against: Option<std::path::PathBuf>,
    near: Option<u32>,
    window: u32,
    dump: Option<u32>,
    metrics: bool,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut dll = None;
        let mut toml = None;
        let mut callees = None;
        let mut against = None;
        let mut near = None;
        let mut dump = None;
        let mut metrics = false;
        let mut window = 0x400;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dll" => dll = Some(std::path::PathBuf::from(next(&mut args, "--dll")?)),
                "--toml" => toml = Some(std::path::PathBuf::from(next(&mut args, "--toml")?)),
                "--callees" => {
                    let value = next(&mut args, "--callees")?;
                    callees = Some(parse_rva(&value)?);
                }
                "--against" => {
                    against = Some(std::path::PathBuf::from(next(&mut args, "--against")?))
                }
                "--dump" => {
                    let value = next(&mut args, "--dump")?;
                    dump = Some(parse_rva(&value)?);
                }
                "--near" => {
                    let value = next(&mut args, "--near")?;
                    near = Some(parse_rva(&value)?);
                }
                "--window" => {
                    let value = next(&mut args, "--window")?;
                    window = parse_rva(&value)?;
                }
                "--metrics" => metrics = true,
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unexpected argument {other:?}\n{}", usage())),
            }
        }
        Ok(Self {
            dll: dll.ok_or_else(|| format!("missing --dll\n{}", usage()))?,
            toml,
            callees,
            against,
            near,
            window,
            dump,
            metrics,
        })
    }
}

fn parse_rva(text: &str) -> Result<u32, String> {
    let text = text.trim_start_matches("0x");
    u32::from_str_radix(text, 16).map_err(|_| format!("invalid rva {text:?}"))
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn usage() -> String {
    "usage: vapor-forge-pe-functions --dll <pe> [--toml <corpus.toml>] [--callees <rva>] \
     [--against <other-pe>] [--near <rva>] [--window <hex>] [--dump <rva>] [--metrics]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rva_in_hex() {
        assert_eq!(parse_rva("0xa03a70").unwrap(), 0xa03a70);
        assert_eq!(parse_rva("A03A70").unwrap(), 0xa03a70);
        assert!(parse_rva("nope").is_err());
    }

    #[test]
    fn shape_hash_ignores_branch_targets_but_tracks_shape() {
        // call rel32 with two different displacements, same instruction code.
        let bytes_a: [u8; 5] = [0xE8, 0x00, 0x00, 0x00, 0x00];
        let bytes_b: [u8; 5] = [0xE8, 0x11, 0x22, 0x33, 0x44];
        let shape = |bytes: &[u8]| {
            let mut decoder = Decoder::with_ip(64, bytes, 0, DecoderOptions::NONE);
            let mut instruction = Instruction::default();
            decoder.decode_out(&mut instruction);
            shape_hash(&instruction)
        };
        assert_eq!(shape(&bytes_a), shape(&bytes_b));

        let mut push = Decoder::with_ip(64, &[0x55], 0, DecoderOptions::NONE);
        let mut instruction = Instruction::default();
        push.decode_out(&mut instruction);
        assert_ne!(shape(&bytes_a), shape_hash(&instruction));
    }
}
