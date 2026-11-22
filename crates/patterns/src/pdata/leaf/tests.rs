//! Leaf-gate tests: the acceptance and the four rejections, on a synthetic image
//! whose code section has one ordinary entry and one leaf at `0x1080`.

use super::*;
use crate::pdata::synthetic::{image_bytes, table};
use crate::pdata::{parse, verify_with, AnchorVerdict, Pdata};
use crate::pe_image::PeImage;

/// `mov 0x20a(%rcx),%rax; mov %rax,(%rdx); mov %rdx,%rax; ret` — the leaf both
/// `steamclient64.dll` builds share at `CUser` slot 4.
const GET_STEAM_ID: [u8; 14] = [
    0x48, 0x8B, 0x81, 0x0A, 0x02, 0x00, 0x00, 0x48, 0x89, 0x02, 0x48, 0x8B, 0xC2, 0xC3,
];

/// `push rbp; mov rbp, rsp; mov eax, 1; pop rbp; ret` at the very start of the
/// code section, so the table has one ordinary entry and the image parses.
const PROLOGUE: [u8; 11] = [0x55, 0x48, 0x89, 0xE5, 0xB8, 1, 0, 0, 0, 0x5D, 0xC3];

/// Code section for a leaf test: `PROLOGUE` at `0x1000`, non-padding bytes up to
/// `gap` bytes before `0x1080`, a `gap`-byte run of `int3` and `body` at `0x1080`.
fn leaf_code(body: &[u8], gap: usize) -> Vec<u8> {
    let mut code = vec![0x48; 0x100];
    code[..PROLOGUE.len()].copy_from_slice(&PROLOGUE);
    let start = 0x80 - gap;
    code[start..0x80].fill(0xCC);
    code[0x80..0x80 + body.len()].copy_from_slice(body);
    code
}

fn gate(data: &[u8], rva: u32, evidence: AnchorEvidence) -> AnchorVerdict {
    let image = PeImage::parse(data).expect("synthetic PE");
    let pdata: Pdata = parse(&image, data).expect("pdata");
    verify_with(&image, &pdata, rva, evidence)
}

/// `0x1000` is the only `.pdata` entry; `0x1080` is the leaf under test.
fn image(gap: usize, body: &[u8]) -> Vec<u8> {
    image_bytes(&leaf_code(body, gap), &table(&[(0x1000, 0x100B)]))
}

#[test]
fn leaf_after_int3_padding_passes_with_its_own_verdict() {
    let data = image(PATCH_BYTES as usize, &GET_STEAM_ID);
    let verdict = gate(&data, 0x1080, AnchorEvidence::unique());
    assert_eq!(verdict.label(), "leaf", "{verdict:?}");
    assert!(verdict.is_accepted());
    assert!(!verdict.is_verified(), "a leaf is never `verified`");
    assert_eq!(verdict.function(), None, "a leaf has no `.pdata` extent");
    assert!(matches!(verdict, AnchorVerdict::Leaf { body } if body.has_ret));
}

#[test]
fn leaf_without_unique_signature_evidence_is_rejected() {
    // The strict gate (no evidence) and an explicitly non-unique site both fail:
    // the relaxation is opt-in per resolved address, never a default.
    let data = image(PATCH_BYTES as usize, &GET_STEAM_ID);
    for evidence in [
        AnchorEvidence::default(),
        AnchorEvidence { unique_site: false },
    ] {
        let verdict = gate(&data, 0x1080, evidence);
        assert_eq!(verdict.label(), "unmapped", "{evidence:?}");
        assert!(!verdict.is_accepted());
    }
}

#[test]
fn address_preceded_by_code_bytes_is_not_a_leaf() {
    // Same leaf, but only 11 bytes of padding before it: the 12-byte patch would
    // reach back into the preceding body, so the evidence is not there.
    let data = image(11, &GET_STEAM_ID);
    let verdict = gate(&data, 0x1080, AnchorEvidence::unique());
    assert_eq!(verdict.label(), "unmapped", "{verdict:?}");
    assert!(!verdict.is_accepted());
}

#[test]
fn address_inside_a_pdata_extent_stays_a_mid_function_hit() {
    // The same bytes as the accepted leaf, but `0x1080` sits inside an extent
    // that begins at `0x1070`: the table branch answers first and the leaf rule
    // is never consulted, so padding evidence cannot rescue a mid-function hit.
    let data = image_bytes(
        &leaf_code(&GET_STEAM_ID, PATCH_BYTES as usize),
        &table(&[(0x1000, 0x100B), (0x1070, 0x1090)]),
    );
    let verdict = gate(&data, 0x1080, AnchorEvidence::unique());
    assert_eq!(verdict.label(), "mid-function", "{verdict:?}");
    assert!(!verdict.is_accepted());
    assert_eq!(verdict.function().map(|f| f.begin), Some(0x1070));
}

#[test]
fn leaf_without_a_ret_in_the_window_is_rejected() {
    // `mov` twice with no terminator anywhere in the 64-byte window.
    let body = [0x48, 0x8B, 0x81, 0x0A, 0x02, 0x00, 0x00, 0x48, 0x89, 0x02];
    let mut code = leaf_code(&body, PATCH_BYTES as usize);
    code[0x80 + body.len()..].fill(0x48);
    let data = image_bytes(&code, &table(&[(0x1000, 0x100B)]));
    assert_eq!(
        gate(&data, 0x1080, AnchorEvidence::unique()).label(),
        "unmapped"
    );
}

#[test]
fn anchor_inside_the_padding_run_is_not_a_leaf() {
    // `0x107c` has a full patch of `int3` before it, but it is padding itself,
    // not the entry the padding precedes.
    let data = image(PATCH_BYTES as usize, &GET_STEAM_ID);
    assert_eq!(
        gate(&data, 0x107C, AnchorEvidence::unique()).label(),
        "unmapped"
    );
}
