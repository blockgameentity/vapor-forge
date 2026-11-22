//! Corpus sigscan helpers shared by the golden tests.
//!
//! The Windows corpus is keyed by the sha256 of the module binary and each
//! entry is a wildcard pattern, so a golden test has to (a) read the bytes,
//! (b) find the corpus file that shipped for that exact image, and (c) locate
//! one symbol's pattern in the executable section. None of that needs a TOML
//! parser: a section header names the symbol and the next `pattern` line holds
//! its bytes.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// The module image a golden test decodes, when the environment names one.
///
/// Unset means the test is skipped, not failed: the client DLL is Valve's to
/// distribute and is never committed.
pub fn client_path(variable: &str) -> Option<PathBuf> {
    let value = std::env::var_os(variable)?;
    let path = PathBuf::from(value);
    assert!(
        path.is_file(),
        "{variable} must name a file, not {}",
        path.display()
    );
    Some(path)
}

/// The committed corpus file for a module and image digest.
pub fn corpus_for(module: &str, digest: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../res/patterns/win64")
        .join(module)
        .join(format!("{digest}.toml"));
    assert!(
        path.is_file(),
        "the win64 corpus has no file for this image: {} (a new build needs \
         vapor-forge-pattern-mine and a fresh corpus entry)",
        path.display()
    );
    path
}

/// One `pattern = "…"` payload from a corpus file, by symbol. `??` is a
/// wildcard byte, `None` here.
pub fn pattern_for(module: &str, corpus: &Path, symbol: &str) -> Vec<Option<u8>> {
    let text = std::fs::read_to_string(corpus)
        .unwrap_or_else(|error| panic!("read {}: {error}", corpus.display()));
    let header = format!("[{module}.\"{symbol}\"]");
    let start = text
        .find(&header)
        .unwrap_or_else(|| panic!("{} has no {header}", corpus.display()));
    let section = &text[start + header.len()..];
    let section = &section[..section.find('[').unwrap_or(section.len())];
    let pattern = section
        .lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix("pattern = "))
        .unwrap_or_else(|| panic!("{} has no pattern for {symbol}", corpus.display()))
        .trim_matches('"');
    pattern
        .split_whitespace()
        .map(|token| {
            (token != "??").then(|| {
                u8::from_str_radix(token, 16)
                    .unwrap_or_else(|error| panic!("bad token {token:?}: {error}"))
            })
        })
        .collect()
}

/// Every offset in `haystack` where `pattern` matches, wildcards included.
pub fn find_all(haystack: &[u8], pattern: &[Option<u8>]) -> Vec<usize> {
    let Some((anchor, lead)) = pattern
        .iter()
        .enumerate()
        .find_map(|(index, token)| token.map(|byte| (byte, index)))
    else {
        return Vec::new();
    };
    let mut sites = Vec::new();
    let mut from = 0usize;
    while let Some(found) = haystack[from..].iter().position(|&byte| byte == anchor) {
        let start = from + found;
        let site = start as isize - lead as isize;
        from = start + 1;
        if site < 0 {
            continue;
        }
        let site = site as usize;
        let Some(window) = haystack.get(site..site + pattern.len()) else {
            break;
        };
        if window
            .iter()
            .zip(pattern)
            .all(|(byte, token)| token.is_none_or(|token| token == *byte))
        {
            sites.push(site);
        }
    }
    sites
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
