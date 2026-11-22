//! Golden decode of the job's reply fields from an installed Windows client.
//!
//! `decode_job_reply` reads the reply member offset and the body displacement
//! out of the code that dereferences them, so the only honest test of it is the
//! real image: find `CAPIJobRequestUserStats::BInit` with the corpus signature,
//! decode, and compare against the layout measured by hand
//! (`res/patterns/win64/VTABLE-REPORT.md`, "the job reply fields").
//!
//! The client DLL is not committed (27 MB, Valve's to distribute), so the test
//! runs only when `VAPOR_FORGE_TEST_STEAMCLIENT` points at one. Unset means
//! skipped, not failed:
//!
//! ```text
//! VAPOR_FORGE_TEST_STEAMCLIENT=/tmp/gate0/public/steamclient64.dll \
//!   cargo test -p vapor-forge-patterns --features tools job
//! ```
//!
//! Both live builds are covered by the measured table below, so the same test
//! also runs against `publicbeta` and checks the offset that build actually
//! uses. The decoder itself knows nothing about either build.

#![cfg(feature = "tools")]

mod support;

use std::path::Path;

use support::{client_path, corpus_for, find_all, pattern_for, sha256_hex};
use vapor_forge_patterns::job_fields::{decode_job_reply, JobReply};
use vapor_forge_patterns::pe_image::PeImage;

const ENGINE_VARIABLE: &str = "VAPOR_FORGE_TEST_STEAMCLIENT";
const MODULE: &str = "steamclient";
const SYMBOL: &str = "CAPIJobRequestUserStats::BInit";

/// What `res/patterns/win64/VTABLE-REPORT.md` measured: the reply object is
/// `this+0x1b0` in the public build and `this+0x1a8` in publicbeta, and the
/// body is `+0x18` off it in both. Keyed by image sha256 so a build that moves
/// either offset fails here rather than silently decoding another member.
const MEASURED: [(&str, JobReply); 2] = [
    // public
    (
        "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b",
        JobReply {
            reply_field: 0x1b0,
            body: 0x18,
        },
    ),
    // publicbeta
    (
        "4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10",
        JobReply {
            reply_field: 0x1a8,
            body: 0x18,
        },
    ),
];

/// `BInit`'s RVA per build, from the same report (`0xa8fd20` / `0xab99a0`).
const ANCHORED: [(&str, u32); 2] = [
    (
        "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b",
        0xa8fd20,
    ),
    (
        "4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10",
        0xab99a0,
    ),
];

#[test]
fn job_reply_fields_from_the_installed_client() {
    let Some(path) = client_path(ENGINE_VARIABLE) else {
        eprintln!("{ENGINE_VARIABLE} is unset: skipping the golden decode");
        return;
    };
    let bytes = read(&path);
    let digest = sha256_hex(&bytes);
    let image =
        PeImage::parse(&bytes).unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    let segment = image
        .code_segment()
        .unwrap_or_else(|error| panic!("executable section of {}: {error}", path.display()));

    let corpus = corpus_for(MODULE, &digest);
    let pattern = pattern_for(MODULE, &corpus, SYMBOL);
    let sites = find_all(segment.bytes, &pattern);
    assert_eq!(
        sites.len(),
        1,
        "{SYMBOL} must be unique in {}: found {} site(s)",
        path.display(),
        sites.len()
    );
    let offset = sites[0];
    let rva = u32::try_from(segment.vaddr + offset as u64)
        .unwrap_or_else(|_| panic!("{SYMBOL} RVA does not fit u32"));

    let decoded = decode_job_reply(&segment, offset).unwrap_or_else(|reason| {
        panic!(
            "decode {SYMBOL} at {rva:#x} in {}: {reason}",
            path.display()
        )
    });
    println!("{} (sha256 {digest}): {SYMBOL} at {rva:#x}", path.display());
    println!(
        "decoded reply_field={:#x} body={:#x}",
        decoded.reply_field, decoded.body
    );

    if let Some((_, anchored)) = ANCHORED.iter().find(|(sha, _)| *sha == digest) {
        assert_eq!(rva, *anchored, "corpus anchor against the measured RVA");
    }
    // An image without a measured row is still required to match the public
    // build: the environment variable is documented to name it.
    let expected = MEASURED
        .iter()
        .find(|(sha, _)| *sha == digest)
        .map(|(_, layout)| *layout)
        .unwrap_or(MEASURED[0].1);
    assert_eq!(
        decoded,
        expected,
        "{} decoded reply_field={:#x} body={:#x}, but VTABLE-REPORT.md records \
         reply_field 0x1b0 (public) / 0x1a8 (publicbeta) and body 0x18: the \
         decoder and the report disagree",
        path.display(),
        decoded.reply_field,
        decoded.body
    );
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|error| panic!("read {} from {ENGINE_VARIABLE}: {error}", path.display()))
}
