//! Hand-built `.pdata` blobs and a minimal synthetic PE32+ image: the parser and
//! the anchor gate must agree with the documented rule on every fragment shape
//! that occurs in the Steam modules, and must error rather than panic on a
//! truncated table.

use super::synthetic::{image_bytes, table};
use super::*;

/// Fold with an explicit "which extents terminate" set, so the fallthrough
/// clause of rule 4 can be tested without a disassembler.
fn parsed_terminating(entries: &[(u32, u32)], terminated: &[u32]) -> Pdata {
    let raw = super::entries(&table(entries)).expect("table");
    let (functions, metrics) = fold(raw, |function| terminated.contains(&function.begin));
    Pdata { functions, metrics }
}

/// Fold with every extent terminating: the common case, where only rule 4.2
/// (unaligned fragment) joins anything.
fn parsed(entries: &[(u32, u32)]) -> Pdata {
    let begins = entries.iter().map(|(begin, _)| *begin).collect::<Vec<_>>();
    parsed_terminating(entries, &begins)
}

#[test]
fn unaligned_contiguous_fragments_merge_into_one_function() {
    // Function body then two unaligned funclets: three entries, one function.
    let pdata = parsed(&[(0x1000, 0x1011), (0x1011, 0x1083), (0x1083, 0x10a7)]);
    assert_eq!(
        pdata.functions(),
        &[Function {
            begin: 0x1000,
            end: 0x10a7
        }]
    );
    assert_eq!(pdata.metrics().fragment_joins, 2);
    assert_eq!(
        pdata.entry(0x1011),
        None,
        "a merged fragment is not an entry"
    );
    assert_eq!(pdata.containing(0x1011).map(|f| f.begin), Some(0x1000));
}

#[test]
fn aligned_flush_entry_after_a_terminating_extent_is_its_own_function() {
    // Two independent functions placed flush, both 16-byte aligned.
    let pdata = parsed(&[(0x2000, 0x2040), (0x2040, 0x2080)]);
    assert_eq!(pdata.len(), 2);
    assert_eq!(pdata.metrics().flush_starts, 1);
    assert_eq!(pdata.entry(0x2040).map(Function::len), Some(0x40));
}

#[test]
fn aligned_flush_entry_after_an_unterminated_extent_is_a_continuation() {
    // MSVC splits a function at an aligned boundary (a 16-byte prologue entry
    // followed by the body entry); the prologue has no terminator, so the
    // second entry continues it.
    let pdata = parsed_terminating(&[(0x3000, 0x3010), (0x3010, 0x3080)], &[0x3010]);
    assert_eq!(pdata.len(), 1);
    assert_eq!(pdata.functions()[0].len(), 0x80);
    assert_eq!(pdata.metrics().fallthrough_joins, 1);
    assert_eq!(pdata.entry(0x3000).map(Function::len), Some(0x80));
    assert_eq!(pdata.entry(0x3010), None);
}

#[test]
fn gap_starts_a_new_function_and_overlap_is_absorbed() {
    let pdata = parsed(&[(0x2000, 0x2010), (0x2010, 0x2030), (0x2100, 0x2110)]);
    assert_eq!(
        pdata.len(),
        3,
        "flush aligned entries are separate functions"
    );
    let gap = parsed(&[(0x2000, 0x2030), (0x2100, 0x2110)]);
    assert_eq!(gap.len(), 2, "a gap always starts a new function");
    let overlapping = parsed(&[(0x5000, 0x5010), (0x5008, 0x5030)]);
    assert_eq!(overlapping.len(), 1);
    assert_eq!(overlapping.functions()[0].end, 0x5030);
    assert_eq!(overlapping.metrics().overlapping_joins, 1);
}

#[test]
fn short_extent_is_kept_and_counted() {
    // A 5-byte `jmp` thunk is a real function: the rule must not filter it out.
    let pdata = parsed(&[(0x4000, 0x4005), (0x4010, 0x40c0)]);
    assert_eq!(pdata.len(), 2);
    assert_eq!(pdata.entry(0x4000).map(Function::len), Some(5));
    assert_eq!(pdata.metrics().short_functions, 1);
    assert_eq!(pdata.metrics().short_entries, 1);
}

#[test]
fn unaligned_begin_is_kept_and_counted() {
    let pdata = parsed(&[(0x5003, 0x5040)]);
    assert_eq!(pdata.metrics().unaligned_functions, 1);
    assert_eq!(pdata.metrics().unaligned_entries, 1);
    assert!(pdata.entry(0x5003).is_some());
    assert!(pdata.entry(0x5004).is_none());
}

#[test]
fn truncated_table_is_an_error_not_a_panic() {
    let mut bytes = table(&[(0x1000, 0x1010), (0x1010, 0x1020)]);
    bytes.truncate(bytes.len() - 3);
    assert_eq!(
        entries(&bytes),
        Err(PdataError::Truncated { len: 21 }),
        "a table that is not a whole number of entries must error"
    );
    assert_eq!(entries(&[]), Ok(Vec::new()));
}

#[test]
fn invalid_entry_is_dropped_and_counted() {
    let pdata = parsed(&[(0x6000, 0x6000), (0x6010, 0x6020)]);
    assert_eq!(pdata.len(), 1);
    assert_eq!(pdata.metrics().invalid_entries, 1);
}

#[test]
fn unsorted_and_duplicate_entries_are_ordered() {
    let pdata = parsed(&[(0x7011, 0x7020), (0x7000, 0x7011)]);
    assert_eq!(pdata.functions()[0].begin, 0x7000);
    assert_eq!(pdata.functions()[0].end, 0x7020);
    assert_eq!(pdata.metrics().fragment_joins, 1);
}
#[test]
fn parse_reads_the_exception_directory_and_the_gate_classifies() {
    // 0x1000: push rbp / mov rbp,rsp / pop rbp / ret; 0x1010: 5-byte tail jump.
    let mut code = vec![0x55, 0x48, 0x89, 0xE5, 0x5D, 0xC3];
    code.resize(0x10, 0xCC);
    code.extend_from_slice(&[0xE9, 0x00, 0x00, 0x00, 0x00]);
    let table = table(&[(0x1000, 0x1006), (0x1010, 0x1015)]);
    let data = image_bytes(&code, &table);
    let image = PeImage::parse(&data).expect("synthetic PE");
    let pdata = parse(&image, &data).expect("pdata");

    assert_eq!(pdata.len(), 2, "a gap keeps two functions");
    assert_eq!(pdata.entry(0x1000).map(Function::len), Some(6));

    // Entry whose body returns: corpus-worthy.
    let returns = crate::pdata::verify(&image, &pdata, 0x1000);
    assert!(returns.is_verified(), "{returns:?}");
    assert!(matches!(
        returns,
        AnchorVerdict::Verified { body, .. } if body.has_ret && !body.ends_in_jump
    ));

    // A 5-byte tail jump as the whole body: a thunk, still a terminating body.
    assert!(matches!(
        crate::pdata::verify(&image, &pdata, 0x1010),
        AnchorVerdict::Verified { body, .. } if body.is_thunk
    ));

    // Inside the merged extent but not an entry: a fragment, never a hook target.
    assert_eq!(
        crate::pdata::verify(&image, &pdata, 0x1003).label(),
        "mid-function"
    );
    // Outside the table entirely.
    assert_eq!(
        crate::pdata::verify(&image, &pdata, 0x9000).label(),
        "unmapped"
    );
}

#[test]
fn parse_reports_a_missing_directory_and_a_truncated_image() {
    let table = table(&[(0x1000, 0x1005)]);
    let mut data = image_bytes(&[0xC3], &table);
    let dir = 0x40 + 24 + 112 + 3 * 8;
    // Zero size -> the image has no exception directory.
    data[dir + 4..dir + 8].copy_from_slice(&0u32.to_le_bytes());
    let image = PeImage::parse(&data).expect("synthetic PE");
    assert_eq!(parse(&image, &data), Err(PdataError::Missing));

    // Size larger than the file-backed section -> error, not a panic.
    let mut data = image_bytes(&[0xC3], &table);
    data[dir + 4..dir + 8].copy_from_slice(&0x1000u32.to_le_bytes());
    let image = PeImage::parse(&data).expect("synthetic PE");
    assert!(matches!(
        parse(&image, &data),
        Err(PdataError::PastEof { .. })
    ));
}
