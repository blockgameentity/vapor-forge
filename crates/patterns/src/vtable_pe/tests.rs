//! Synthetic-image tests for the vtable enumerator.
//!
//! A real `steamclient64.dll` cannot be committed, and asserting on it would
//! make these tests depend on a build we do not ship. Every case below is
//! therefore built from bytes: a PE32+ image with `.text`, `.rdata` and an
//! exception directory, and the table layouts the enumerator must accept or
//! reject.

use super::*;
use crate::pe_image::{PeImage, MACHINE_AMD64};

pub const TEXT_RVA: u32 = 0x1000;
pub const RDATA_RVA: u32 = 0x2000;
pub const PDATA_RVA: u32 = 0x3000;
const IMAGE_BASE: u64 = 0x1_4000_0000;

/// `image_base + rva`, the form MSVC stores in a vtable.
pub fn va(rva: u32) -> u64 {
    IMAGE_BASE + u64::from(rva)
}

/// Minimal PE32+ image with three sections. `pdata` entries are
/// `(begin, end)`; an empty list still produces the section, but a zero-size
/// exception directory (see [`crate::pdata::parse`]).
pub fn synthetic_pe(text: &[u8], rdata: &[u8], pdata: &[(u32, u32)]) -> Vec<u8> {
    const PE_OFF: usize = 0x80;
    const OPTIONAL_SIZE: usize = 0xF0;
    const SECTION_TABLE: usize = PE_OFF + 4 + 20 + OPTIONAL_SIZE;
    const SCN_EXECUTE: u32 = 0x2000_0000;
    const SCN_DATA: u32 = 0x4000_0000;
    const SCN_READ: u32 = 0x4000_0000;

    let raw_text = 0x400;
    let raw_rdata = align(raw_text + text.len(), 0x200);
    let mut pdata_bytes = Vec::with_capacity(pdata.len() * 12);
    for (begin, end) in pdata {
        pdata_bytes.extend_from_slice(&begin.to_le_bytes());
        pdata_bytes.extend_from_slice(&end.to_le_bytes());
        pdata_bytes.extend_from_slice(&0u32.to_le_bytes()); // unwind info RVA
    }
    let raw_pdata = align(raw_rdata + rdata.len(), 0x200);
    let mut data = vec![0u8; raw_pdata + pdata_bytes.len()];

    data[0..2].copy_from_slice(b"MZ");
    data[0x3c..0x40].copy_from_slice(&(PE_OFF as u32).to_le_bytes());
    data[PE_OFF..PE_OFF + 4].copy_from_slice(b"PE\0\0");
    let coff = PE_OFF + 4;
    data[coff..coff + 2].copy_from_slice(&MACHINE_AMD64.to_le_bytes());
    data[coff + 2..coff + 4].copy_from_slice(&3u16.to_le_bytes());
    data[coff + 16..coff + 18].copy_from_slice(&(OPTIONAL_SIZE as u16).to_le_bytes());
    let optional = coff + 20;
    data[optional..optional + 2].copy_from_slice(&0x020bu16.to_le_bytes());
    data[optional + 24..optional + 32].copy_from_slice(&IMAGE_BASE.to_le_bytes());
    data[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
    // Data directory 3 (exception) -> .pdata.
    let exception = optional + 112 + 3 * 8;
    data[exception..exception + 4].copy_from_slice(&PDATA_RVA.to_le_bytes());
    data[exception + 4..exception + 8].copy_from_slice(&(pdata_bytes.len() as u32).to_le_bytes());

    let sections: [(&str, u32, usize, u32, usize, u32); 3] = [
        (
            ".text",
            TEXT_RVA,
            text.len(),
            SCN_EXECUTE | SCN_READ,
            raw_text,
            SCN_EXECUTE | SCN_READ,
        ),
        (
            ".rdata",
            RDATA_RVA,
            rdata.len(),
            SCN_DATA,
            raw_rdata,
            SCN_DATA,
        ),
        (
            ".pdata",
            PDATA_RVA,
            pdata_bytes.len(),
            SCN_DATA,
            raw_pdata,
            SCN_DATA,
        ),
    ];
    for (index, (name, rva, size, _kind, raw_offset, characteristics)) in
        sections.iter().enumerate()
    {
        let at = SECTION_TABLE + index * 40;
        data[at..at + name.len()].copy_from_slice(name.as_bytes());
        data[at + 8..at + 12].copy_from_slice(&(*size as u32).to_le_bytes());
        data[at + 12..at + 16].copy_from_slice(&rva.to_le_bytes());
        data[at + 16..at + 20].copy_from_slice(&(*size as u32).to_le_bytes());
        data[at + 20..at + 24].copy_from_slice(&(*raw_offset as u32).to_le_bytes());
        data[at + 36..at + 40].copy_from_slice(&characteristics.to_le_bytes());
    }
    data[raw_text..raw_text + text.len()].copy_from_slice(text);
    data[raw_rdata..raw_rdata + rdata.len()].copy_from_slice(rdata);
    if !pdata_bytes.is_empty() {
        data[raw_pdata..raw_pdata + pdata_bytes.len()].copy_from_slice(&pdata_bytes);
    }
    data
}

fn align(value: usize, to: usize) -> usize {
    value.div_ceil(to) * to
}

pub fn put_qword(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// Three `.pdata` entries that do not merge, one per table slot.
fn entries_1010_1020_1030() -> [(u32, u32); 3] {
    [(0x1010, 0x1018), (0x1020, 0x1028), (0x1030, 0x1038)]
}

fn parse(bytes: &[u8]) -> PeImage<'_> {
    PeImage::parse(bytes).expect("synthetic PE parses")
}

#[test]
fn enumerates_a_valid_three_slot_table() {
    let mut rdata = vec![0u8; 0x40];
    put_qword(&mut rdata, 0x10, va(0x1010));
    put_qword(&mut rdata, 0x18, va(0x1020));
    put_qword(&mut rdata, 0x20, va(0x1030));
    let bytes = synthetic_pe(&[0x90; 0x40], &rdata, &entries_1010_1020_1030());
    let image = parse(&bytes);
    let functions = crate::pdata::parse(&image, &bytes).expect("pdata");

    let enumeration = enumerate(&image, &bytes, &functions).expect("enumerate");
    assert_eq!(enumeration.tables.len(), 1);
    let table = &enumeration.tables[0];
    assert_eq!(table.base_rva, RDATA_RVA + 0x10);
    assert_eq!(table.section, ".rdata");
    assert_eq!(table.entries, vec![0x1010, 0x1020, 0x1030]);
    assert_eq!(table.len(), 3);
    assert_eq!(table.end_rva(), RDATA_RVA + 0x28);
    assert_eq!(table.slot_of(0x1020), Some(1));
    assert_eq!(table.slot_of(0x1040), None);
    // RTTI-free corroboration: every slot is a function entry, and nothing
    // precedes the table.
    assert_eq!(table.corroboration.pdata_entries, 3);
    assert!(!table.corroboration.preceded_by_data_pointer);
    assert_eq!(table.corroboration.rtti_class, None);
    assert!(enumeration.rejected.is_empty());
}

#[test]
fn rejects_a_table_containing_an_out_of_text_pointer() {
    let bytes = synthetic_pe(&[0x90; 0x40], &[], &entries_1010_1020_1030());
    let image = parse(&bytes);
    let entries = [0x1010, RDATA_RVA + 0x38, 0x1020];
    assert_eq!(
        validate(&image, &entries),
        Err(Rejection::NonTextPointer { slot: 1 })
    );
    // A run stops at that qword, so the scanner only sees the 1-slot prefix,
    // which is rejected as too short rather than as a table.
    let mut rdata = vec![0u8; 0x40];
    put_qword(&mut rdata, 0x10, va(0x1010));
    put_qword(&mut rdata, 0x18, va(RDATA_RVA + 0x38));
    put_qword(&mut rdata, 0x20, va(0x1020));
    put_qword(&mut rdata, 0x28, va(0x1030));
    let bytes = synthetic_pe(&[0x90; 0x40], &rdata, &entries_1010_1020_1030());
    let image = parse(&bytes);
    let functions = crate::pdata::parse(&image, &bytes).expect("pdata");
    let enumeration = enumerate(&image, &bytes, &functions).expect("enumerate");
    // The two pointers after the data pointer are a real table; the single
    // pointer before it is not.
    let bases: Vec<u32> = enumeration.tables.iter().map(|t| t.base_rva).collect();
    assert_eq!(bases, vec![RDATA_RVA + 0x20]);
    assert_eq!(
        enumeration.rejected,
        vec![Rejected {
            base_rva: RDATA_RVA + 0x10,
            reason: Rejection::TooFewSlots { found: 1 },
        }]
    );
}

#[test]
fn duplicate_tables_sharing_a_pointer_are_both_enumerated() {
    let mut rdata = vec![0u8; 0x40];
    // Table A: 0x1010, 0x1020. Separator data pointer. Table B: 0x1030, 0x1010.
    put_qword(&mut rdata, 0x00, va(0x1010));
    put_qword(&mut rdata, 0x08, va(0x1020));
    put_qword(&mut rdata, 0x10, va(RDATA_RVA + 0x30));
    put_qword(&mut rdata, 0x18, va(0x1030));
    put_qword(&mut rdata, 0x20, va(0x1010));
    put_u32(&mut rdata, 0x30, 0); // not a COL signature
    let bytes = synthetic_pe(&[0x90; 0x40], &rdata, &entries_1010_1020_1030());
    let image = parse(&bytes);
    let functions = crate::pdata::parse(&image, &bytes).expect("pdata");
    let enumeration = enumerate(&image, &bytes, &functions).expect("enumerate");

    let bases: Vec<u32> = enumeration.tables.iter().map(|t| t.base_rva).collect();
    assert_eq!(bases, vec![RDATA_RVA, RDATA_RVA + 0x18]);
    let holders = containing(&enumeration.tables, 0x1010);
    assert_eq!(holders.len(), 2, "the shared pointer is in both tables");
    assert_eq!(holders[0].slot_of(0x1010), Some(0));
    assert_eq!(holders[1].slot_of(0x1010), Some(1));
    assert!(containing(&enumeration.tables, 0x1020).len() == 1);
}

#[test]
fn rejects_a_table_whose_length_is_implausible() {
    let slots = MAX_SLOTS + 100;
    let mut rdata = vec![0u8; slots * 8];
    for index in 0..slots {
        put_qword(&mut rdata, index * 8, va(0x1010));
    }
    let bytes = synthetic_pe(&[0x90; 0x40], &rdata, &entries_1010_1020_1030());
    let image = parse(&bytes);
    let functions = crate::pdata::parse(&image, &bytes).expect("pdata");
    let enumeration = enumerate(&image, &bytes, &functions).expect("enumerate");
    assert!(enumeration.tables.is_empty());
    assert_eq!(
        enumeration.rejected,
        vec![Rejected {
            base_rva: RDATA_RVA,
            reason: Rejection::TooManySlots { found: slots },
        }]
    );
}

#[test]
fn rejects_a_single_pointer_run_and_ignores_data_pointers() {
    let mut rdata = vec![0u8; 0x40];
    put_qword(&mut rdata, 0x00, va(0x1010));
    put_qword(&mut rdata, 0x08, va(RDATA_RVA + 0x30));
    put_qword(&mut rdata, 0x10, 0); // null: not a pointer at all
    let bytes = synthetic_pe(&[0x90; 0x40], &rdata, &entries_1010_1020_1030());
    let image = parse(&bytes);
    let functions = crate::pdata::parse(&image, &bytes).expect("pdata");
    let enumeration = enumerate(&image, &bytes, &functions).expect("enumerate");
    assert!(enumeration.tables.is_empty());
    assert_eq!(
        enumeration.rejected,
        vec![Rejected {
            base_rva: RDATA_RVA,
            reason: Rejection::TooFewSlots { found: 1 },
        }]
    );
}

#[test]
fn reads_the_rtti_class_from_the_qword_before_the_table() {
    let mut rdata = vec![0u8; 0x80];
    // COL at 0x00: signature 1, offset 0, cd_offset 0, type_descriptor_rva.
    put_u32(&mut rdata, 0x00, 1);
    put_u32(&mut rdata, 0x0c, RDATA_RVA + 0x20);
    // TypeDescriptor at 0x20; the mangled name starts 16 bytes in.
    rdata[0x30..0x30 + 12].copy_from_slice(b".?AVCUser@@\0");
    // Table at 0x50 preceded by the COL pointer at 0x48.
    put_qword(&mut rdata, 0x48, va(RDATA_RVA));
    put_qword(&mut rdata, 0x50, va(0x1010));
    put_qword(&mut rdata, 0x58, va(0x1020));
    let bytes = synthetic_pe(&[0x90; 0x40], &rdata, &entries_1010_1020_1030());
    let image = parse(&bytes);
    let functions = crate::pdata::parse(&image, &bytes).expect("pdata");
    let enumeration = enumerate(&image, &bytes, &functions).expect("enumerate");

    let table = enumeration
        .tables
        .iter()
        .find(|table| table.base_rva == RDATA_RVA + 0x50)
        .expect("table after the locator pointer");
    assert!(table.corroboration.preceded_by_data_pointer);
    assert_eq!(
        table.corroboration.rtti_class.as_deref(),
        Some(".?AVCUser@@")
    );
    assert_eq!(table.class(), Some("CUser"));
    assert_eq!(enumeration.with_class(), 1);
    let holder_of_class = of_class(&enumeration.tables, "user");
    assert_eq!(holder_of_class.len(), 1);
    assert_eq!(holder_of_class[0].base_rva, RDATA_RVA + 0x50);
    assert!(of_class(&enumeration.tables, "nope").is_empty());
}
