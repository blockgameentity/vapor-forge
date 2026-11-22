//! Behavioural candidate generator for Windows Steam modules.
//!
//! Four Gate-0 targets were never published in any hint corpus - `CUser::
//! GetSubscribedApps`, the user-stats/achievement path, the `CNetPacket`
//! dispatch site and a `SetEnvString` equivalent - so they cannot be recovered
//! by mining and have to be derived from the binary. This tool measures every
//! function `.pdata` reports and prints a ranked shortlist with the numbers
//! behind each rank.
//!
//! It never claims a symbol is found. A rank means "this body shows the
//! behaviour the profile describes"; a human still has to read the candidate
//! (`--dump`) and check it against a call site or a caller.

use std::path::PathBuf;

use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::probe_facts;
use vapor_forge_patterns::probe_rank::{rank, Probe};

mod evidence;
mod table;

use crate::pe_probe::evidence::{anchors_of, counterparts, print_candidate};
use crate::pe_probe::table::{dump, function_table};

/// Bodies shorter than this cannot show a loop plus a store sequence; skipping
/// them keeps the sweep fast without dropping anything the profiles can match.
const MIN_SIZE: u32 = 0x18;
const DEFAULT_TOP: usize = 8;

/// Entry point: measure, rank, print. Errors surface as a non-zero exit with the
/// message on stderr.
pub fn run() -> Result<(), String> {
    let args = Args::parse(std::env::args().skip(1))?;
    let bytes = std::fs::read(&args.dll)
        .map_err(|error| format!("read {}: {error}", args.dll.display()))?;
    let image = PeImage::parse(&bytes)?;
    let table = function_table(&image, &bytes)?;
    if let Some(rva) = args.dump {
        return dump(&image, &table, rva);
    }
    let anchors = args
        .toml
        .as_deref()
        .map(|path| anchors_of(path, &image))
        .transpose()?
        .unwrap_or_default();
    let other = args.against.as_deref().map(counterparts).transpose()?;

    println!(
        "{} module={} functions={} text={:#x}..{:#x} anchors={}",
        args.dll.display(),
        args.module,
        table.len(),
        table.first().map(|f| f.begin).unwrap_or(0),
        table.last().map(|f| f.end).unwrap_or(0),
        anchors.len()
    );
    for (rva, name) in &anchors {
        println!("  anchor {rva:#010x} {name}");
    }
    let facts = table
        .iter()
        .filter(|function| function.end - function.begin >= MIN_SIZE)
        .filter(|function| match args.near {
            // Class methods cluster in a translation unit, so a known sibling
            // constrains the search for a symbol that has no hint at all.
            Some(near) => {
                function.begin >= near.saturating_sub(args.window)
                    && function.begin <= near.saturating_add(args.window)
            }
            None => true,
        })
        .map(|function| probe_facts::measure(&image, function.begin, function.end))
        .collect::<Vec<_>>();
    println!(
        "measured {} functions (>= {MIN_SIZE:#x} bytes); evidence per candidate, never a verdict",
        facts.len()
    );

    if let Some(target) = args.callers {
        return report_callers(&facts, target, &anchors);
    }
    let probes = match args.probe {
        Some(probe) => vec![probe],
        None => Probe::ALL.to_vec(),
    };
    for probe in probes {
        let scored = facts.iter().filter(|facts| probe.score(facts) > 0).count();
        let ranked = rank(probe, &facts, args.top);
        println!(
            "\n== {} ({scored} of {} functions scored, top {} shown) profile: {}",
            probe.name(),
            facts.len(),
            ranked.len(),
            probe.profile()
        );
        for (place, (score, index)) in ranked.iter().enumerate() {
            print_candidate(place + 1, *score, &facts[*index], &anchors, other.as_ref());
        }
    }
    Ok(())
}

/// Who calls a function: the strongest confirmation for a derived candidate is
/// a caller whose own role is already known, so the call graph is printed rather
/// than guessed at.
fn report_callers(
    facts: &[probe_facts::Facts],
    target: u32,
    anchors: &[(u32, String)],
) -> Result<(), String> {
    let callers = facts
        .iter()
        .filter(|facts| facts.callees.contains(&target))
        .collect::<Vec<_>>();
    if callers.is_empty() {
        return Err(format!("no measured function calls {target:#x} directly"));
    }
    println!("\n{} caller(s) of {target:#x}:", callers.len());
    for caller in callers {
        let name = anchors
            .iter()
            .find(|(rva, _)| *rva == caller.begin)
            .map(|(_, name)| name.as_str())
            .unwrap_or("-");
        println!(
            "  {:#010x} size {:#x} insns {} {name}",
            caller.begin,
            caller.end - caller.begin,
            caller.insns
        );
    }
    Ok(())
}

#[derive(Debug)]
struct Args {
    dll: PathBuf,
    module: String,
    toml: Option<PathBuf>,
    against: Option<PathBuf>,
    probe: Option<Probe>,
    top: usize,
    dump: Option<u32>,
    callers: Option<u32>,
    near: Option<u32>,
    window: u32,
}

impl Args {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut dll = None;
        let mut module = None;
        let mut toml = None;
        let mut against = None;
        let mut probe = None;
        let mut top = DEFAULT_TOP;
        let mut dump = None;
        let mut callers = None;
        let mut near = None;
        let mut window = 0x10000;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dll" => dll = Some(PathBuf::from(next(&mut args, "--dll")?)),
                "--module" => module = Some(next(&mut args, "--module")?),
                "--toml" => toml = Some(PathBuf::from(next(&mut args, "--toml")?)),
                "--against" => against = Some(PathBuf::from(next(&mut args, "--against")?)),
                "--probe" => {
                    let value = next(&mut args, "--probe")?;
                    probe = Some(Probe::parse(&value).ok_or_else(|| {
                        format!(
                            "unknown probe {value:?} (getsubscribed|userstats|dispatch|envstring)"
                        )
                    })?);
                }
                "--top" => {
                    let value = next(&mut args, "--top")?;
                    top = value
                        .parse()
                        .map_err(|_| format!("invalid --top {value:?}"))?;
                }
                "--dump" => dump = Some(parse_rva(&next(&mut args, "--dump")?)?),
                "--callers" => callers = Some(parse_rva(&next(&mut args, "--callers")?)?),
                "--near" => near = Some(parse_rva(&next(&mut args, "--near")?)?),
                "--window" => window = parse_rva(&next(&mut args, "--window")?)?,
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unexpected argument {other:?}\n{}", usage())),
            }
        }
        Ok(Self {
            dll: dll.ok_or_else(|| format!("missing --dll\n{}", usage()))?,
            module: module.ok_or_else(|| format!("missing --module\n{}", usage()))?,
            toml,
            against,
            probe,
            top,
            dump,
            callers,
            near,
            window,
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
    "usage: vapor-forge-pe-probe --dll <pe> --module <name> [--toml <corpus.toml>] \
     [--probe getsubscribed|userstats|dispatch|envstring] [--top <n>] \
     [--against <other-pe>] [--near <rva> --window <hex>] [--dump <rva>] [--callers <rva>]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rva_in_hex() {
        assert_eq!(parse_rva("0xa03a70").unwrap(), 0xa03a70);
        assert!(parse_rva("zzz").is_err());
    }

    #[test]
    fn rejects_unknown_probes_and_missing_flags() {
        let args = ["--dll", "x", "--module", "m", "--probe", "nope"]
            .into_iter()
            .map(str::to_owned);
        assert!(Args::parse(args).is_err());
        let args = ["--module", "m"].into_iter().map(str::to_owned);
        assert!(Args::parse(args).is_err());
    }
}
