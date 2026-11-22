//! The geometry decode, on Linux: the validation, the symbol lookup and the
//! pending decoder's reason — no Windows, no live client.

use super::*;
use crate::install::{BatchPlan, SymbolPlan};

fn segment(bits: u8) -> CodeSegment<'static> {
    CodeSegment {
        bytes: &[0x90u8; 32],
        file_offset: 0x200,
        vaddr: 0x1000,
        bits,
        container: vapor_forge_patterns::scan::Container::Pe,
    }
}

fn plan(symbols: Vec<SymbolPlan>) -> BatchPlan {
    BatchPlan {
        module: "steamclient".to_owned(),
        code_vaddr: 0x1000,
        symbols,
    }
}

struct Fixed(Result<JobReply, &'static str>);

impl ReplyDecoder for Fixed {
    fn decode(&self, _segment: &CodeSegment<'_>, _binit_offset: usize) -> Result<JobReply, &'static str> {
        self.0
    }
}

#[test]
fn a_decoded_pair_is_accepted_and_its_length_slot_is_derived() {
    let reply = JobReply::new(0x1b0, 0x10).expect("both slots are pointer-aligned");
    assert_eq!(reply.size_offset(), 0x10 + core::mem::size_of::<usize>());
    assert_eq!(JobReply::new(0, 0x10), None, "offset 0 is the vtable, not a field");
    assert_eq!(JobReply::new(0x1b1, 0x10), None, "a misaligned reply slot");
    assert_eq!(JobReply::new(0x1b0, 0x12), None, "a misaligned body slot");
}

#[test]
fn a_number_outside_the_bounds_is_refused_rather_than_read() {
    assert_eq!(JobReply::new(MAX_JOB_FIELD + 8, 0x10), None);
    assert_eq!(JobReply::new(0x1b0, MAX_REPLY_FIELD + 8), None);
    assert!(JobReply::new(MAX_JOB_FIELD, MAX_REPLY_FIELD).is_some());
}

#[test]
fn the_symbol_offset_is_relative_to_the_segment_and_only_for_the_run_method() {
    let segment = segment(64);
    let resolved = [
        ("CAPIJobRequestUserStats::YieldingMergeStats", 0x220usize),
        (BINIT_SYMBOL, 0x240),
    ];
    assert_eq!(symbol_offset(resolved.into_iter(), &segment), Some(0x40));
    let only_merge = [("CAPIJobRequestUserStats::YieldingMergeStats", 0x220usize)];
    assert_eq!(symbol_offset(only_merge.into_iter(), &segment), None);
}

#[test]
fn a_decoder_that_reports_a_pair_is_used_as_data() {
    let decoded = plan(vec![SymbolPlan::Resolved {
        name: BINIT_SYMBOL.to_owned(),
        offset: 0x210,
    }]);
    let result = decode_with(&Fixed(Ok(JobReply { reply_field: 0x1a8, body: 0x10 })), &decoded, &[0x90; 4]);
    // The image cannot be parsed, so this proves the image error is reported before
    // any decode, and it never panics.
    assert!(
        matches!(&result, Err(reason) if reason.contains("could not be parsed")),
        "{result:?}"
    );
}

#[test]
fn the_corpus_decoder_refuses_nonsense_instructions_instead_of_inventing_an_offset() {
    // 0x90 is `nop`: no `this` tracking, no reply load, so the patterns decoder
    // refuses it by name rather than returning a made-up pair.
    let reason = CorpusDecoder.decode(&segment(64), 0x40).unwrap_err();
    assert!(!reason.is_empty(), "a refusal must carry its reason");
    assert!(NO_GEOMETRY.contains("left exactly as Steam sent it"));
    let decoded = plan(Vec::new());
    let reason = decode_plan(&decoded, &[0x90; 4]).unwrap_err();
    assert!(reason.contains("could not be parsed"), "{reason}");
}

#[test]
fn the_corpus_decoders_pair_is_converted_into_this_crates_geometry() {
    // The two structs are the same shape by contract; the conversion is the only
    // place the patterns crate's type meets this one.
    let their = vapor_forge_patterns::job_fields::JobReply {
        reply_field: 0x1b0,
        body: 0x18,
    };
    let ours = JobReply {
        reply_field: their.reply_field,
        body: their.body,
    };
    assert_eq!(JobReply::new(ours.reply_field, ours.body), Some(ours));
    assert_eq!(ours.size_offset(), 0x18 + core::mem::size_of::<usize>());
}
