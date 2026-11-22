//! Function extents from the PE exception directory (`.pdata`).
//!
//! **Duplicated on purpose.** The same walk exists in
//! `src/bin/vapor-forge-pe-functions.rs` (`function_table`), but that code lives
//! in a binary and cannot be imported, and `pe_image.rs` is being changed by the
//! `gate0/pdata-unify` workstream. This file is deliberately the only place in
//! `vtable_pe` that knows the `RUNTIME_FUNCTION` layout, so it can be swapped
//! for the shared reader once that lands.
//!
//! Semantics match the existing tool: contiguous regions are merged (a function
//! may be split into a prolog fragment and a body), so an extent is an upper
//! bound on the body and only a *begin* is a function entry.

use crate::pe_image::PeImage;

/// Data directory 3, the exception directory.
const EXCEPTION_DIRECTORY: usize = 3;
const RUNTIME_FUNCTION_SIZE: usize = 12;

#[derive(Clone, Debug, Default)]
pub struct Functions {
    /// Merged extents, sorted by begin.
    extents: Vec<(u32, u32)>,
}

impl Functions {
    /// Read and merge the exception directory. Errors only when the image has
    /// no exception directory at all: a slot that is not a function entry is a
    /// fact to report, not a read failure.
    pub fn read(image: &PeImage<'_>, bytes: &[u8]) -> Result<Self, String> {
        let (rva, size) = image
            .data_directory(EXCEPTION_DIRECTORY)
            .filter(|(_, size)| *size > 0)
            .ok_or_else(|| "image has no exception directory (.pdata)".to_owned())?;
        let offset = image
            .rva_to_offset(u64::from(rva))
            .ok_or_else(|| "exception directory RVA is not mapped".to_owned())?;
        let count = size as usize / RUNTIME_FUNCTION_SIZE;
        let mut raw = Vec::with_capacity(count);
        for index in 0..count {
            let at = offset + index * RUNTIME_FUNCTION_SIZE;
            let entry = bytes
                .get(at..at + RUNTIME_FUNCTION_SIZE)
                .ok_or_else(|| "exception directory entry past EOF".to_owned())?;
            let begin = read_u32(entry, 0)?;
            let end = read_u32(entry, 4)?;
            if end > begin {
                raw.push((begin, end));
            }
        }
        raw.sort_unstable();
        let mut extents: Vec<(u32, u32)> = Vec::with_capacity(raw.len());
        for (begin, end) in raw {
            match extents.last_mut() {
                Some(last) if last.1 == begin => last.1 = end,
                _ => extents.push((begin, end)),
            }
        }
        Ok(Self { extents })
    }

    /// `(begin, end)` when `rva` is exactly a function entry.
    pub fn extent(&self, rva: u32) -> Option<(u32, u32)> {
        self.extents
            .binary_search_by_key(&rva, |&(begin, _)| begin)
            .ok()
            .map(|index| self.extents[index])
    }

    pub fn is_entry(&self, rva: u32) -> bool {
        self.extent(rva).is_some()
    }

    pub fn len(&self) -> usize {
        self.extents.len()
    }

    pub fn is_empty(&self) -> bool {
        self.extents.is_empty()
    }

    /// Extents in address order; used to render function indices in reports.
    pub fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.extents.iter().copied()
    }
}

fn read_u32(bytes: &[u8], at: usize) -> Result<u32, String> {
    let slice = bytes
        .get(at..at + 4)
        .ok_or_else(|| format!("u32 read past entry end at {at:#x}"))?;
    let mut buf = [0u8; 4];
    buf.copy_from_slice(slice);
    Ok(u32::from_le_bytes(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_contiguous_fragments() {
        let functions = Functions {
            extents: Vec::new(),
        };
        assert!(functions.is_empty());
        // The merge rule is exercised through the image in `super::super::tests`;
        // here only the lookup contract is worth pinning.
        let functions = Functions {
            extents: vec![(0x1000, 0x1010), (0x2000, 0x2030)],
        };
        assert_eq!(functions.extent(0x1000), Some((0x1000, 0x1010)));
        assert_eq!(functions.extent(0x1008), None);
        assert!(functions.is_entry(0x2000));
        assert_eq!(functions.len(), 2);
    }
}
