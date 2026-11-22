//! Golden decode of the `CNetPacket` layout from an installed Windows client.
//!
//! The decoder reads the field offsets out of `CNetPacket::Init`, `Release` and
//! `Alloc` at runtime, so the only honest test of it is the real image: find the
//! three cornerstoned functions with the corpus patterns, decode, and compare
//! against the layout that was measured by hand (`res/patterns/win64/VTABLE-REPORT.md`,
//! "CNetPacket layout family").
//!
//! The client DLL is not committed (27 MB, Valve's to distribute), so the test
//! runs only when `VAPOR_FORGE_TEST_STEAMCLIENT` points at one. Unset means
//! skipped, not failed:
//!
//! ```text
//! VAPOR_FORGE_TEST_STEAMCLIENT=/tmp/gate0/public/steamclient64.dll \
//!   cargo test -p vapor-forge-patterns --features tools cnet
//! ```

#![cfg(feature = "tools")]

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use vapor_forge_patterns::cnet_packet::{self, PacketApi};
use vapor_forge_patterns::pe_image::PeImage;

const ENGINE_VARIABLE: &str = "VAPOR_FORGE_TEST_STEAMCLIENT";
const MODULE: &str = "steamclient";

/// The three functions the decode walks, in the order [`PacketApi`] needs them.
const SYMBOLS: [&str; 3] = [
    "CNetPacket::Alloc",
    "CNetPacket::Init",
    "CNetPacket::Release",
];

/// Measured by hand, identical in both live builds: connection id `+0x00`,
/// `pubData +0x10`, `cubData +0x18`, the owned buffer `+0x20`, `m_cRef +0x1c`,
/// object size `0x58`.
const MEASURED: PacketApi = PacketApi {
    fields: vapor_forge_patterns::cnet_packet::PacketFields {
        conn_id: 0x00,
        data: 0x10,
        size: 0x18,
        owned_data: 0x20,
    },
    refcount: 0x1c,
    object_size: 0x58,
};

/// The RVAs the corpus anchors land on, per sha256 of the image. Pinned so a
/// build that moves the family fails here rather than silently decoding a
/// different function.
const ANCHORED: [(&str, [u32; 3]); 2] = [
    // public
    (
        "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b",
        [0xd3fe90, 0xead290, 0xead4d0],
    ),
    // publicbeta
    (
        "4a2d79520a72c55854c61c22cbfc53ec4e12cc13cc00e0c27d9b520648fe1c10",
        [0xd6bcd0, 0xebf080, 0xebf2c0],
    ),
];

#[test]
fn cnet_layout_from_the_installed_client() {
    let Some(path) = client_path() else {
        eprintln!("{ENGINE_VARIABLE} is unset: skipping the golden decode");
        return;
    };
    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!("read {} from {ENGINE_VARIABLE}: {error}", path.display())
    });
    let digest = sha256_hex(&bytes);
    let image = PeImage::parse(&bytes)
        .unwrap_or_else(|error| panic!("parse {}: {error}", path.display()));
    let segment = image
        .code_segment()
        .unwrap_or_else(|error| panic!("executable section of {}: {error}", path.display()));
    let corpus = corpus_for(&digest);

    let mut offsets = [0usize; 3];
    let mut rvas = [0u32; 3];
    for (slot, symbol) in SYMBOLS.iter().enumerate() {
        let pattern = pattern_for(&corpus, symbol);
        let sites = find_all(segment.bytes, &pattern);
        assert_eq!(
            sites.len(),
            1,
            "{symbol} must be unique in {}: found {} site(s)",
            path.display(),
            sites.len()
        );
        offsets[slot] = sites[0];
        rvas[slot] = u32::try_from(segment.vaddr + sites[0] as u64)
            .unwrap_or_else(|_| panic!("{symbol} RVA does not fit u32"));
    }

    let api = cnet_packet::decode_api_segment(&segment, offsets[0], offsets[1], offsets[2])
        .unwrap_or_else(|missing| {
            panic!(
                "decode {} at alloc {:#x}, init {:#x}, release {:#x}: {missing}",
                path.display(),
                rvas[0],
                rvas[1],
                rvas[2]
            )
        });
    println!(
        "{} (sha256 {digest}): alloc {:#x} init {:#x} release {:#x}",
        path.display(),
        rvas[0],
        rvas[1],
        rvas[2]
    );
    println!(
        "decoded conn_id={:#x} data={:#x} size={:#x} owned={:#x} refs={:#x} object={:#x}",
        api.fields.conn_id,
        api.fields.data,
        api.fields.size,
        api.fields.owned_data,
        api.refcount,
        api.object_size
    );

    assert_eq!(
        cnet_packet::convention(&segment),
        vapor_forge_patterns::arg_flow::Abi::MsX64,
        "{} is an MS-x64 image",
        path.display()
    );
    assert_eq!(api, MEASURED, "decoded layout against the measured table");
    if let Some((_, anchored)) = ANCHORED.iter().find(|(sha, _)| *sha == digest) {
        assert_eq!(rvas, *anchored, "corpus anchors against the measured RVAs");
    }
}

/// The client image to decode, when the environment names one.
fn client_path() -> Option<PathBuf> {
    let value = std::env::var_os(ENGINE_VARIABLE)?;
    let path = PathBuf::from(value);
    assert!(
        path.is_file(),
        "{ENGINE_VARIABLE} must name a file, not {}",
        path.display()
    );
    Some(path)
}

/// The committed corpus file for this build: the file names are the image
/// digests, so the corpus a live client is verified against is the one that
/// shipped for it.
fn corpus_for(digest: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../res/patterns/win64")
        .join(MODULE);
    let path = directory.join(format!("{digest}.toml"));
    assert!(
        path.is_file(),
        "the win64 corpus has no file for this image: {} (a new build needs \
         vapor-forge-pattern-mine and a fresh corpus entry)",
        path.display()
    );
    path
}

/// One `pattern = "..."` payload from the corpus, by symbol. The decoder test
/// only needs the bytes, so this reads them without a TOML parser: a section
/// header names the symbol, the next `pattern` line is its bytes, `??` is a
/// wildcard.
fn pattern_for(corpus: &Path, symbol: &str) -> Vec<Option<u8>> {
    let text = std::fs::read_to_string(corpus)
        .unwrap_or_else(|error| panic!("read {}: {error}", corpus.display()));
    let header = format!("[{MODULE}.\"{symbol}\"]");
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
            (token != "??")
                .then(|| {
                    u8::from_str_radix(token, 16)
                        .unwrap_or_else(|error| panic!("bad token {token:?}: {error}"))
                })
        })
        .collect()
}

/// Every offset in `haystack` where `pattern` matches, wildcards included.
fn find_all(haystack: &[u8], pattern: &[Option<u8>]) -> Vec<usize> {
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

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
