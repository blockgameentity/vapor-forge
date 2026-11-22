//! Command-line arguments.

use std::path::PathBuf;

/// Shortlist size per probe when a table is too long to print in full.
pub const SHORTLIST_PER_PROBE: usize = 4;

#[derive(Debug)]
pub struct Args {
    pub dll: PathBuf,
    pub module: String,
    pub corpus: Option<PathBuf>,
    pub corpus_against: Option<PathBuf>,
    pub against: Option<PathBuf>,
    pub class: Option<String>,
    pub holds: Vec<u32>,
    pub all_slots: bool,
    pub top: usize,
}

impl Args {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut dll = None;
        let mut module = None;
        let mut corpus = None;
        let mut corpus_against = None;
        let mut against = None;
        let mut class = None;
        let mut holds = Vec::new();
        let mut all_slots = false;
        let mut top = SHORTLIST_PER_PROBE;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--dll" => dll = Some(PathBuf::from(next(&mut args, "--dll")?)),
                "--module" => module = Some(next(&mut args, "--module")?),
                "--corpus" => corpus = Some(PathBuf::from(next(&mut args, "--corpus")?)),
                "--corpus-against" => {
                    corpus_against = Some(PathBuf::from(next(&mut args, "--corpus-against")?));
                }
                "--against" => against = Some(PathBuf::from(next(&mut args, "--against")?)),
                "--class" => class = Some(next(&mut args, "--class")?),
                "--holds" => holds.push(parse_rva(&next(&mut args, "--holds")?)?),
                "--all-slots" => all_slots = true,
                "--top" => {
                    top = next(&mut args, "--top")?
                        .parse()
                        .map_err(|_| "--top needs a number".to_owned())?;
                }
                "-h" | "--help" => return Err(usage()),
                other => return Err(format!("unexpected argument {other:?}\n{}", usage())),
            }
        }
        Ok(Self {
            dll: dll.ok_or_else(|| format!("missing --dll\n{}", usage()))?,
            module: module.unwrap_or_else(|| "steamclient".to_owned()),
            corpus,
            corpus_against,
            against,
            class,
            holds,
            all_slots,
            top,
        })
    }
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

/// `0x1234` or `1234` (hex, as every rva in the reports is printed).
fn parse_rva(text: &str) -> Result<u32, String> {
    let digits = text.trim_start_matches("0x");
    u32::from_str_radix(digits, 16).map_err(|_| format!("--holds needs a hex rva, got {text:?}"))
}

fn usage() -> String {
    "usage: vapor-forge-vtable-pe --dll <pe> [--module <steamclient|steamui>]\n\
     \x20      [--corpus <corpus.toml>] [--class <RTTI class>] [--holds <rva>]... [--all-slots]\n\
     \x20      [--top <n>] [--against <other-pe> [--corpus-against <other.toml>]]"
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_and_optional_flags() {
        let args = Args::parse(
            [
                "--dll",
                "a.dll",
                "--module",
                "steamui",
                "--class",
                "CUser",
                "--all-slots",
            ]
            .into_iter()
            .map(str::to_owned),
        )
        .expect("parse");
        assert_eq!(args.dll, PathBuf::from("a.dll"));
        assert_eq!(args.module, "steamui");
        assert_eq!(args.class.as_deref(), Some("CUser"));
        assert!(args.all_slots);
        assert!(args.corpus.is_none());
        assert!(args.holds.is_empty());
        assert!(Args::parse(std::iter::empty()).is_err());
    }

    #[test]
    fn holds_accepts_hex_rvas() {
        let args = Args::parse(
            ["--dll", "a.dll", "--holds", "0xa16f70", "--holds", "597b20"]
                .into_iter()
                .map(str::to_owned),
        )
        .expect("parse");
        assert_eq!(args.holds, vec![0xa16f70, 0x597b20]);
        assert!(Args::parse(
            ["--dll", "a.dll", "--holds", "nope"]
                .into_iter()
                .map(str::to_owned)
        )
        .is_err());
    }

    #[test]
    fn rejects_unknown_flags_and_missing_values() {
        assert!(Args::parse(["--dll".to_owned()]).is_err());
        assert!(Args::parse(["--dll".to_owned(), "a".to_owned(), "--nope".to_owned()]).is_err());
    }
}
