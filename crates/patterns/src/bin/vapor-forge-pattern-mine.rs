//! Mine Windows signature patterns from hint corpora into registry TOMLs.
//!
//! Gate-0 tool. Input is a Windows Steam module (`steamclient64.dll`,
//! `steamui.dll`) plus one or more *hint* TOMLs in the OST `steam-monitor`
//! layout (`[<crc>] name = "..." sig = "AA BB .." rva = "0x.."`). The `rva`
//! field is ignored on purpose: we never bind to an offset, we re-derive an
//! IDA-style wildcard byte signature and require it to be unique in the
//! executable section of *this* image before it is written out.
//!
//! Every emitted pattern is then re-parsed and re-resolved with the real
//! scanner (`scan_code_segment`) so a mined TOML cannot be "unique" only
//! according to this tool's own matcher.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::exit;

use memchr::memchr;
use sha2::{Digest, Sha256};
use vapor_forge_patterns::pdata::{self, verify, Pdata};
use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::registry::parse_toml_patterns;
use vapor_forge_patterns::scan::{
    group_variants, scan_code_segment, CodeSegment, PatternRef, PatternScanStatus,
};

const DEFAULT_MAX_LEN: usize = 48;

/// One signature token: a concrete byte, or a wildcard when `None`.
type Token = Option<u8>;
type Signature = Vec<Token>;
type HintMap = BTreeMap<String, Vec<Signature>>;

fn main() {
    if let Err(error) = run() {
        eprintln!("vapor-forge-pattern-mine: {error}");
        exit(1);
    }
}

#[derive(Debug)]
struct Args {
    dll: PathBuf,
    module: String,
    arch: String,
    hints: Vec<PathBuf>,
    out: Option<PathBuf>,
    max_len: usize,
    verify: Option<PathBuf>,
    /// Skip the anchor gate (`--allow-unverified-anchors`).
    allow_unverified: bool,
}

fn run() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1))?;
    let data = std::fs::read(&args.dll)
        .map_err(|error| format!("read {}: {error}", args.dll.display()))?;
    let image = PeImage::parse(&data)?;
    let table = pdata::parse(&image, &data).map_err(|error| error.to_string())?;
    let segment = image.code_segment()?;
    if let Some(toml) = args.verify.as_deref() {
        return verify_file(toml, &image, &table, &segment, &args.module, &args.dll);
    }
    let digest = hex(&Sha256::digest(&data));

    let hints = load_hints(&args.hints)?;
    let mut targets: Vec<(&str, Vec<Signature>)> = Vec::new();
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for (name, signatures) in &hints {
        let target = vapor_name(name);
        if let Some(previous) = seen.insert(target, name) {
            if previous != name.as_str() {
                eprintln!("note: hint names {previous:?} and {name:?} both map to {target:?}");
            }
        }
        targets.push((target, signatures.clone()));
    }

    let mut out = String::new();
    out.push_str(&format!(
        "# Mined by vapor-forge-pattern-mine from {} ({}, {} bytes, sha256 {})\n",
        args.dll.display(),
        args.arch,
        data.len(),
        digest
    ));
    out.push_str(
        "# Signatures are wildcard-free and uniqueness-checked against this image only;\n",
    );
    out.push_str("# re-mine per Steam build and record variants when a build stops resolving.\n");
    out.push_str(
        "# Each pattern resolves to a .pdata function entry whose body returns or tail-jumps\n\
         # (the anchor gate, see res/patterns/win64/README.md); anything else is not written.\n",
    );
    out.push_str("[hotfix]\nformat = 1\n");
    out.push_str(&format!("architecture = \"{}\"\n\n", args.arch));

    let mut resolved = 0usize;
    let mut ambiguous = 0usize;
    let mut missing = 0usize;
    let mut collided = 0usize;
    let mut rejected = 0usize;
    let mut rejected_names = Vec::new();
    let mined = targets
        .into_iter()
        .map(|(target, signatures)| (target, mine(&segment, &signatures, args.max_len)))
        .collect::<Vec<_>>();
    // Two symbols resolving to one address means at least one hint is wrong; a
    // shared prologue published under several names is not a signature.
    let mut rva_claims: BTreeMap<u64, usize> = BTreeMap::new();
    for (_, outcome) in &mined {
        if let MineOutcome::Resolved { rva, .. } = outcome {
            *rva_claims.entry(*rva).or_default() += 1;
        }
    }

    println!(
        "{:<44} {:<9} {:>8}  {:<12} {:<6} length",
        "symbol", "status", "matches", "rva", "gate"
    );
    for (target, outcome) in mined {
        let outcome = match outcome {
            MineOutcome::Resolved { pattern: _, rva }
                if rva_claims.get(&rva).copied().unwrap_or(0) > 1 =>
            {
                MineOutcome::Collide { rva }
            }
            other => other,
        };
        match outcome {
            MineOutcome::Resolved { pattern, rva } => {
                let rva = u32::try_from(rva)
                    .map_err(|_| format!("{target}: resolved rva exceeds u32"))?;
                let verdict = verify(&image, &table, rva);
                if !verdict.is_verified() && !args.allow_unverified {
                    rejected += 1;
                    rejected_names.push(format!("{target} @ {rva:#x}: {}", verdict.label()));
                    println!(
                        "{:<44} {:<9} {:>8}  {:#012x} {:<6} anchor gate rejected ({})",
                        target,
                        "REJECT",
                        1,
                        rva,
                        verdict.label(),
                        pattern.iter().flatten().count()
                    );
                    continue;
                }
                resolved += 1;
                let status = if verdict.is_verified() {
                    "OK"
                } else {
                    "UNVERIFIED"
                };
                println!(
                    "{:<44} {:<9} {:>8}  {:#012x} {:<6} {}",
                    target,
                    status,
                    1,
                    rva,
                    verdict.label(),
                    pattern.iter().flatten().count()
                );
                out.push_str(&format!(
                    "[{}.{:?}]\nfollow = \"none\"\npattern = \"{}\"\n\n",
                    args.module,
                    target,
                    render(&pattern)
                ));
            }
            MineOutcome::Ambiguous { matches } => {
                ambiguous += 1;
                println!(
                    "{:<44} {:<9} {:>8}  {:12} {:6} -",
                    target, "AMB", matches, "-", "-"
                );
            }
            MineOutcome::Missing => {
                missing += 1;
                println!(
                    "{:<44} {:<9} {:>8}  {:12} {:6} -",
                    target, "MISS", 0, "-", "-"
                );
            }
            MineOutcome::Collide { rva } => {
                collided += 1;
                println!(
                    "{:<44} {:<9} {:>8}  {:#012x} {:6} -",
                    target, "COLLIDE", 0, rva, "-"
                );
            }
        }
    }

    let total = resolved + ambiguous + missing + collided + rejected;
    println!(
        "\nmined {resolved}/{total} symbols (ambiguous {ambiguous}, collided {collided}, \
         missing {missing}, rejected by the anchor gate {rejected})"
    );
    if !rejected_names.is_empty() {
        eprintln!(
            "WARNING: the anchor gate rejected {} symbol(s); they are NOT in the output.",
            rejected_names.len()
        );
        eprintln!(
            "A pattern is corpus-worthy only when it resolves to a function entry whose body"
        );
        eprintln!(
            "returns or tail-jumps. Fix the hint, or pass --allow-unverified-anchors to include"
        );
        eprintln!("them anyway:");
        for name in &rejected_names {
            eprintln!("  {name}");
        }
    }

    verify_round_trip(&out, &segment, &args.module)?;

    if let Some(path) = args.out.as_deref() {
        std::fs::write(path, &out).map_err(|error| format!("write {}: {error}", path.display()))?;
        println!("wrote {}", path.display());
    }

    if resolved * 2 < total {
        return Err(format!(
            "gate floor not met: {resolved}/{total} resolved unambiguously"
        ));
    }
    Ok(())
}

enum MineOutcome {
    Resolved { pattern: Signature, rva: u64 },
    Ambiguous { matches: usize },
    Collide { rva: u64 },
    Missing,
}

fn mine(segment: &CodeSegment<'_>, signatures: &[Signature], max_len: usize) -> MineOutcome {
    let mut best_ambiguous = 0usize;
    for signature in signatures {
        let sites = find_all(segment.bytes, signature);
        match sites.len() {
            0 => continue,
            1 => {
                let site = sites[0];
                let pattern = extend(segment.bytes, signature, site, max_len)
                    .unwrap_or_else(|| signature.clone());
                if find_all(segment.bytes, &pattern).len() == 1 {
                    return MineOutcome::Resolved {
                        pattern,
                        rva: segment.vaddr + site as u64,
                    };
                }
                best_ambiguous = best_ambiguous.max(2);
            }
            count => best_ambiguous = best_ambiguous.max(count),
        }
    }
    if best_ambiguous > 0 {
        MineOutcome::Ambiguous {
            matches: best_ambiguous,
        }
    } else {
        MineOutcome::Missing
    }
}

/// Grow a matched signature outwards (right first, then left) until it stops
/// matching anywhere else in the segment, or the length cap is hit.
fn extend(haystack: &[u8], signature: &[Token], site: usize, max_len: usize) -> Option<Signature> {
    let mut pattern = signature.to_vec();
    let mut start = site;
    if pattern.len() >= max_len {
        return None;
    }
    let mut end = site + signature.len();
    while end < haystack.len() && pattern.len() < max_len {
        let token = haystack[end];
        pattern.push(Some(token));
        end += 1;
        if find_all(haystack, &pattern).len() == 1 {
            return Some(pattern);
        }
    }
    while start > 0 && pattern.len() < max_len {
        start -= 1;
        pattern.insert(0, Some(haystack[start]));
        if find_all(haystack, &pattern).len() == 1 {
            return Some(pattern);
        }
    }
    None
}

fn find_all(haystack: &[u8], pattern: &[Token]) -> Vec<usize> {
    let mut sites = Vec::new();
    if pattern.is_empty() || pattern.len() > haystack.len() {
        return sites;
    }
    let Some((anchor_idx, anchor)) = pattern
        .iter()
        .enumerate()
        .find_map(|(idx, token)| token.map(|byte| (idx, byte)))
    else {
        return sites;
    };
    let last_start = haystack.len() - pattern.len();
    let mut from = 0usize;
    while from <= last_start {
        let Some(hit) = memchr(anchor, &haystack[from..=last_start + anchor_idx]) else {
            break;
        };
        let start = from + hit - anchor_idx;
        if matches_at(haystack, pattern, start) {
            sites.push(start);
            if sites.len() > 64 {
                break;
            }
        }
        from = start + 1;
    }
    sites
}

fn matches_at(haystack: &[u8], pattern: &[Token], start: usize) -> bool {
    pattern
        .iter()
        .enumerate()
        .all(|(idx, token)| token.is_none_or(|byte| haystack[start + idx] == byte))
}

/// `--verify <toml>`: resolve a shipped corpus against one image, apply the
/// anchor gate, and report per symbol. Unresolved or gate-rejected patterns
/// fail the run: a shipped corpus that no longer lands on a real function is a
/// defect to look at, never something to silently re-mine.
fn verify_file(
    path: &Path,
    image: &PeImage<'_>,
    table: &Pdata,
    segment: &CodeSegment<'_>,
    module: &str,
    dll: &Path,
) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let parsed = parse_toml_patterns(&text)?;
    let refs = parsed
        .iter()
        .map(PatternRef::from)
        .collect::<Vec<PatternRef<'_>>>();
    let report = scan_code_segment(module, dll, segment, &refs, None, "PE64")?;
    println!(
        "verify {} against {} ({} patterns, text {:#x}+{})",
        path.display(),
        dll.display(),
        refs.len(),
        report.text_vaddr,
        report.text_size
    );
    println!(
        "{:<44} {:<9} {:>8}  {:<12} gate",
        "symbol", "status", "matches", "rva"
    );
    let mut gated = 0usize;
    let mut bad_gate = Vec::new();
    for entry in &report.entries {
        let rva = entry
            .target_offset
            .and_then(|offset| u32::try_from(report.text_vaddr + offset as u64).ok());
        let (verdict, shown) = match rva {
            Some(rva) if entry.status == PatternScanStatus::Ok => {
                let verdict = verify(image, table, rva);
                if verdict.is_verified() {
                    gated += 1;
                } else {
                    bad_gate.push(format!("{} @ {rva:#x}: {}", entry.name, verdict.label()));
                }
                (verdict.label(), format!("{rva:#012x}"))
            }
            _ => ("-", "-".to_owned()),
        };
        println!(
            "{:<44} {:<9} {:>8}  {:<12} {verdict}",
            entry.name,
            entry.status.label(),
            entry.match_count,
            shown
        );
    }
    if report.failure_count() != 0 {
        return Err(format!("{} pattern(s) unresolved", report.failure_count()));
    }
    if !bad_gate.is_empty() {
        return Err(format!(
            "anchor gate rejected {} of {} resolved pattern(s): {}",
            bad_gate.len(),
            report.ok_count(),
            bad_gate.join(", ")
        ));
    }
    println!("all {} patterns resolve and pass the body gate", gated);
    Ok(())
}

fn render(pattern: &[Token]) -> String {
    pattern
        .iter()
        .map(|token| match token {
            Some(byte) => format!("{byte:02X}"),
            None => "??".to_owned(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn load_hints(paths: &[PathBuf]) -> Result<HintMap, String> {
    let mut hints: HintMap = BTreeMap::new();
    for path in paths {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("read hints {}: {error}", path.display()))?;
        let mut name: Option<String> = None;
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("name = ") {
                name = Some(rest.trim().trim_matches('"').to_owned());
            } else if let Some(rest) = line.strip_prefix("sig = ") {
                let Some(current) = name.take() else {
                    return Err(format!("sig without name in {}", path.display()));
                };
                let signature = parse_signature(rest.trim().trim_matches('"'))?;
                hints.entry(current).or_default().push(signature);
            }
        }
    }
    Ok(hints)
}

fn parse_signature(text: &str) -> Result<Signature, String> {
    text.split_whitespace()
        .map(|token| {
            if token == "?" || token == "??" {
                return Ok(None);
            }
            u8::from_str_radix(token, 16)
                .map(Some)
                .map_err(|_| format!("invalid pattern token {token:?}"))
        })
        .collect()
}

/// Re-parse what we are about to write and resolve it with the shipped engine.
fn verify_round_trip(text: &str, segment: &CodeSegment<'_>, module: &str) -> Result<(), String> {
    let parsed = parse_toml_patterns(text)?;
    let refs = parsed
        .iter()
        .map(PatternRef::from)
        .collect::<Vec<PatternRef<'_>>>();
    if refs.is_empty() {
        return Err("mined output contains no patterns".to_owned());
    }
    let entries = group_variants(&refs);
    let report = scan_code_segment(module, Path::new("<mined>"), segment, &refs, None, "PE64")
        .map_err(|error| format!("round-trip scan failed: {error}"))?;
    let failures = report
        .entries
        .iter()
        .filter(|entry| entry.status != PatternScanStatus::Ok)
        .collect::<Vec<_>>();
    if !failures.is_empty() {
        let names = failures
            .iter()
            .map(|entry| format!("{}={}", entry.name, entry.status.label()))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("round-trip unresolved: {names}"));
    }
    println!(
        "round-trip verified {} pattern group(s) with the shipped scanner",
        entries.len()
    );
    Ok(())
}

/// OST `steam-monitor` names mapped onto the symbol names the registry uses.
fn vapor_name(ost_name: &str) -> &str {
    match ost_name {
        "CheckAppOwnership" => "CUser::CheckAppOwnership",
        "GetSubscribedApps" => "CUser::GetSubscribedApps",
        "BuildSpawnEnvBlock" => "CUser::BuildSpawnEnvBlock",
        "RecvPkt" => "CCMConnection::RecvPkt",
        "GetPackageInfo" => "CPackageInfo::GetPackageInfo",
        "GetAppByID" => "CSteamUIAppController::GetAppByID",
        "FillInAppOverview" => "CSteamUIAppController::FillInAppOverview",
        "BuildCompleteAppOverviewChange" => "CSteamUIAppController::BuildCompleteAppOverviewChange",
        "CSteamUIAppControllerRunFrame" => "CSteamUIAppController::RunFrame",
        "MarkAppChange" => "CUpdateManager::MarkAppChange",
        other => other,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut dll = None;
        let mut module = None;
        let mut arch = None;
        let mut hints = Vec::new();
        let mut out = None;
        let mut max_len = DEFAULT_MAX_LEN;
        let mut verify = None;
        let mut allow_unverified = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dll" => dll = Some(PathBuf::from(next(&mut args, "--dll")?)),
                "--module" => {
                    let value = next(&mut args, "--module")?;
                    if value != "steamclient" && value != "steamui" {
                        return Err(format!("unsupported module {value:?}"));
                    }
                    module = Some(value);
                }
                "--arch" => {
                    let value = next(&mut args, "--arch")?;
                    if value != "x86_64" && value != "x86" {
                        return Err(format!("unsupported arch {value:?}"));
                    }
                    arch = Some(value);
                }
                "--hints" => hints.push(PathBuf::from(next(&mut args, "--hints")?)),
                "--out" => out = Some(PathBuf::from(next(&mut args, "--out")?)),
                "--verify" => verify = Some(PathBuf::from(next(&mut args, "--verify")?)),
                "--allow-unverified-anchors" => allow_unverified = true,
                "--max-len" => {
                    let value = next(&mut args, "--max-len")?;
                    max_len = value
                        .parse::<usize>()
                        .map_err(|_| format!("invalid --max-len {value:?}"))?;
                    if !(16..=256).contains(&max_len) {
                        return Err("--max-len must be within 16..=256".to_owned());
                    }
                }
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unexpected argument {other:?}\n{}", usage())),
            }
        }
        Ok(Self {
            dll: dll.ok_or_else(|| format!("missing --dll\n{}", usage()))?,
            module: module.ok_or_else(|| format!("missing --module\n{}", usage()))?,
            arch: arch.ok_or_else(|| format!("missing --arch\n{}", usage()))?,
            hints,
            out,
            max_len,
            verify,
            allow_unverified,
        })
    }
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn usage() -> String {
    "usage: vapor-forge-pattern-mine --dll <pe> --module steamclient|steamui --arch x86_64|x86 \
     --hints <toml> [--hints <toml> ..] [--out <toml>] [--max-len 48] \
     [--allow-unverified-anchors]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern(text: &str) -> Signature {
        parse_signature(text).expect("parse")
    }

    #[test]
    fn finds_unique_site_and_extend_makes_it_unique() {
        let mut haystack = vec![0u8; 64];
        haystack[10..14].copy_from_slice(&[0x48, 0x8B, 0xC4, 0x55]);
        haystack[40..44].copy_from_slice(&[0x48, 0x8B, 0xC4, 0x55]);
        let short = pattern("48 8B C4 55");
        assert_eq!(find_all(&haystack, &short).len(), 2);
        // Identical zero padding on both sides: nothing the length cap allows
        // can separate the two sites, so extension must give up rather than
        // return a still-ambiguous pattern.
        assert!(extend(&haystack, &short, 10, 8).is_none());

        haystack[41] = 0xAA;
        let extended = extend(&haystack, &short, 10, 8).expect("extend");
        assert_eq!(find_all(&haystack, &extended).len(), 1);
        assert!(matches_at(&haystack, &extended, 10));
    }

    #[test]
    fn wildcard_tokens_are_skipped_when_matching() {
        let haystack: [u8; 6] = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
        let signature = pattern("11 ?? 33");
        assert_eq!(find_all(&haystack, &signature), vec![0]);
    }

    #[test]
    fn missing_signature_reports_missing() {
        let haystack = vec![0u8; 32];
        let segment = CodeSegment {
            bytes: &haystack,
            file_offset: 0,
            vaddr: 0x1000,
            bits: 64,
            container: vapor_forge_patterns::scan::Container::Pe,
        };
        assert!(matches!(
            mine(&segment, &[pattern("DE AD BE EF")], 32),
            MineOutcome::Missing
        ));
    }
}
