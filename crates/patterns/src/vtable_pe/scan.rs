//! Candidate-vtable scanning: qword runs in non-executable sections.
//!
//! Kept apart from [`super`] so the acceptance rules ([`super::validate`]) and
//! the walk that produces runs can be read independently: the walk bounds a
//! table, the validator judges one.

use crate::pe_image::PeImage;

use super::{rtti, text_rva, validate, Corroboration, Enumeration, Functions, Rejected, Table};

/// Enumerate candidate vtables from every non-executable section.
pub fn enumerate(
    image: &PeImage<'_>,
    bytes: &[u8],
    functions: &Functions,
) -> Result<Enumeration, String> {
    let mut out = Enumeration::default();
    for section in image.sections() {
        if section.executable || section.raw_size == 0 {
            continue;
        }
        let start = section.raw_offset as usize;
        let end = start
            .checked_add(section.raw_size as usize)
            .ok_or_else(|| format!("section {} raw range overflows usize", section.name))?;
        let blob = bytes.get(start..end).ok_or_else(|| {
            format!(
                "section {} raw range {start:#x}..{end:#x} is past EOF",
                section.name
            )
        })?;
        // Entries are 8-aligned *virtual addresses*, so the first candidate in
        // a section is at the first offset that makes the RVA aligned.
        let mut offset = ((8 - section.virtual_address % 8) % 8) as usize;
        let mut run_start: Option<usize> = None;
        let mut run: Vec<u32> = Vec::new();
        while offset + 8 <= blob.len() {
            let mut qword = [0u8; 8];
            qword.copy_from_slice(&blob[offset..offset + 8]);
            match text_rva(image, u64::from_le_bytes(qword)) {
                Some(rva) => {
                    run_start.get_or_insert(offset);
                    run.push(rva);
                }
                None => {
                    if let Some(first) = run_start.take() {
                        accept_run(&mut out, image, functions, section, blob, first, &run);
                    }
                    run.clear();
                }
            }
            offset += 8;
        }
        if let Some(first) = run_start {
            accept_run(&mut out, image, functions, section, blob, first, &run);
        }
    }
    Ok(out)
}

fn accept_run(
    out: &mut Enumeration,
    image: &PeImage<'_>,
    functions: &Functions,
    section: &crate::pe_image::PeSection,
    blob: &[u8],
    first: usize,
    run: &[u32],
) {
    let base_rva = section.virtual_address + first as u32;
    match validate(image, run) {
        Ok(()) => out.tables.push(Table {
            base_rva,
            section: section.name.clone(),
            entries: run.to_vec(),
            corroboration: corroborate(image, functions, blob, first, base_rva, run),
        }),
        Err(reason) => out.rejected.push(Rejected { base_rva, reason }),
    }
}

fn corroborate(
    image: &PeImage<'_>,
    functions: &Functions,
    blob: &[u8],
    first: usize,
    base_rva: u32,
    run: &[u32],
) -> Corroboration {
    let pdata_entries = run.iter().filter(|rva| functions.is_entry(**rva)).count();
    let preceded_by_data_pointer = first >= 8 && {
        let mut qword = [0u8; 8];
        qword.copy_from_slice(&blob[first - 8..first]);
        let value = u64::from_le_bytes(qword);
        value >= image.image_base() && text_rva(image, value).is_none()
    };
    let rtti_class = rtti::class_name(image, base_rva);
    Corroboration {
        pdata_entries,
        preceded_by_data_pointer,
        rtti_class,
    }
}
