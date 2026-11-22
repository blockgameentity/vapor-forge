//! PE/COFF image parsing for Windows Steam client modules.
//!
//! Mirrors the surface of [`crate::elf::ElfImage`] so the pattern scanner needs
//! no ELF-specific code path: header validation, the section table, RVA <->
//! file-offset mapping, and the executable section that becomes the haystack.
//!
//! Only what signature scanning needs is implemented. Nothing here executes or
//! trusts the image: inputs are treated as untrusted bytes and every field read
//! is bounds-checked.

use crate::scan::CodeSegment;

pub const MACHINE_I386: u16 = 0x014c;
pub const MACHINE_AMD64: u16 = 0x8664;

const DOS_SIGNATURE: usize = 0x3c;
const PE_SIGNATURE: &[u8; 4] = b"PE\0\0";
const OPT_MAGIC_PE32: u16 = 0x010b;
const OPT_MAGIC_PE32_PLUS: u16 = 0x020b;
const SCN_MEM_EXECUTE: u32 = 0x2000_0000;
const SECTION_HEADER_SIZE: usize = 40;

#[derive(Clone, Debug)]
pub struct PeSection {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub executable: bool,
}

pub struct PeImage<'a> {
    data: &'a [u8],
    machine: u16,
    bits: u8,
    image_base: u64,
    sections: Vec<PeSection>,
    /// File offset of the data-directory array and its entry count.
    directories: (usize, usize),
}

impl<'a> PeImage<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self, String> {
        if data.get(..2) != Some(b"MZ") {
            return Err("input is not a PE file (missing MZ signature)".to_owned());
        }
        let pe_off = read_u32(data, DOS_SIGNATURE)? as usize;
        if data.get(pe_off..pe_off + 4) != Some(PE_SIGNATURE) {
            return Err("PE signature missing at e_lfanew".to_owned());
        }
        let coff = pe_off + 4;
        let machine = read_u16(data, coff)?;
        let section_count = read_u16(data, coff + 2)? as usize;
        let optional_size = read_u16(data, coff + 16)? as usize;
        let optional = coff + 20;
        let magic = read_u16(data, optional)?;
        let (bits, image_base) = match magic {
            OPT_MAGIC_PE32_PLUS => (64, read_u64(data, optional + 24)?),
            OPT_MAGIC_PE32 => (32, u64::from(read_u32(data, optional + 28)?)),
            other => return Err(format!("unsupported PE optional header magic {other:#06x}")),
        };
        if bits == 64 && machine != MACHINE_AMD64 {
            return Err(format!("PE32+ image with unexpected machine {machine:#06x}"));
        }
        if bits == 32 && machine != MACHINE_I386 {
            return Err(format!("PE32 image with unexpected machine {machine:#06x}"));
        }

        // Data directories live at the end of the optional header; PE32+ keeps
        // them 16 bytes further in than PE32.
        let dir_count = read_u32(data, optional + if bits == 64 { 108 } else { 92 })? as usize;
        let directories = (optional + if bits == 64 { 112 } else { 96 }, dir_count.min(16));

        let table = optional
            .checked_add(optional_size)
            .ok_or_else(|| "section table offset overflows".to_owned())?;
        let mut sections = Vec::with_capacity(section_count);
        for index in 0..section_count {
            let at = table + index * SECTION_HEADER_SIZE;
            let raw_name = data
                .get(at..at + 8)
                .ok_or_else(|| format!("section {index} header past EOF"))?;
            let name = String::from_utf8_lossy(raw_name)
                .trim_end_matches('\0')
                .to_owned();
            sections.push(PeSection {
                name,
                virtual_size: read_u32(data, at + 8)?,
                virtual_address: read_u32(data, at + 12)?,
                raw_size: read_u32(data, at + 16)?,
                raw_offset: read_u32(data, at + 20)?,
                executable: read_u32(data, at + 36)? & SCN_MEM_EXECUTE != 0,
            });
        }

        Ok(Self {
            data,
            machine,
            bits,
            image_base,
            sections,
            directories,
        })
    }

    pub fn bits(&self) -> u8 {
        self.bits
    }

    pub fn machine(&self) -> u16 {
        self.machine
    }

    pub fn image_base(&self) -> u64 {
        self.image_base
    }

    /// `(rva, size)` of data directory `index` (3 = exception directory).
    pub fn data_directory(&self, index: usize) -> Option<(u32, u32)> {
        let (offset, count) = self.directories;
        if index >= count {
            return None;
        }
        let at = offset + index * 8;
        let rva = read_u32(self.data, at).ok()?;
        let size = read_u32(self.data, at + 4).ok()?;
        Some((rva, size))
    }

    /// Borrow `len` bytes at an RVA.
    pub fn bytes_at_rva(&self, rva: u64, len: usize) -> Option<&'a [u8]> {
        let offset = self.rva_to_offset(rva)?;
        self.data.get(offset..offset + len)
    }

    pub fn sections(&self) -> &[PeSection] {
        &self.sections
    }

    pub fn section_names(&self) -> Vec<&str> {
        self.sections.iter().map(|s| s.name.as_str()).collect()
    }

    /// File offset for an RVA, using the section whose raw data covers it.
    pub fn rva_to_offset(&self, rva: u64) -> Option<usize> {
        let rva32 = u32::try_from(rva).ok()?;
        let section = self
            .sections
            .iter()
            .find(|s| rva32 >= s.virtual_address && rva32 < s.virtual_address.saturating_add(s.raw_size))?;
        let delta = rva32 - section.virtual_address;
        Some(section.raw_offset as usize + delta as usize)
    }

    /// RVA for a file offset, the inverse of [`Self::rva_to_offset`].
    pub fn offset_to_rva(&self, offset: usize) -> Option<u64> {
        let offset = u32::try_from(offset).ok()?;
        let section = self
            .sections
            .iter()
            .find(|s| offset >= s.raw_offset && offset < s.raw_offset.saturating_add(s.raw_size))?;
        Some(u64::from(section.virtual_address + (offset - section.raw_offset)))
    }

    /// True when `offset` lies inside a section flagged executable.
    pub fn in_text(&self, offset: usize) -> bool {
        let Ok(offset) = u32::try_from(offset) else {
            return false;
        };
        self.sections.iter().any(|s| {
            s.executable && offset >= s.raw_offset && offset < s.raw_offset.saturating_add(s.raw_size)
        })
    }

    /// The largest executable section, which is what the scanner walks.
    pub fn largest_executable_section(&self) -> Result<&PeSection, String> {
        self.sections
            .iter()
            .filter(|s| s.executable && s.raw_size > 0)
            .max_by_key(|s| s.raw_size)
            .ok_or_else(|| "PE image has no executable section".to_owned())
    }

    /// Borrow the executable section as a scan segment, mirroring
    /// [`crate::elf::ElfImage::largest_executable_segment`].
    pub fn code_segment(&self) -> Result<CodeSegment<'a>, String> {
        let section = self.largest_executable_section()?;
        let start = section.raw_offset as usize;
        let end = start
            .checked_add(section.raw_size as usize)
            .ok_or_else(|| "executable section range overflows usize".to_owned())?;
        let bytes = self
            .data
            .get(start..end)
            .ok_or_else(|| "executable section extends past EOF".to_owned())?;
        Ok(CodeSegment {
            bytes,
            file_offset: u64::from(section.raw_offset),
            vaddr: u64::from(section.virtual_address),
            bits: self.bits,
        })
    }
}

fn read_u16(data: &[u8], at: usize) -> Result<u16, String> {
    let bytes: [u8; 2] = data
        .get(at..at + 2)
        .ok_or_else(|| format!("u16 read past EOF at {at:#x}"))?
        .try_into()
        .map_err(|_| format!("u16 read past EOF at {at:#x}"))?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32(data: &[u8], at: usize) -> Result<u32, String> {
    let bytes: [u8; 4] = data
        .get(at..at + 4)
        .ok_or_else(|| format!("u32 read past EOF at {at:#x}"))?
        .try_into()
        .map_err(|_| format!("u32 read past EOF at {at:#x}"))?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(data: &[u8], at: usize) -> Result<u64, String> {
    let bytes: [u8; 8] = data
        .get(at..at + 8)
        .ok_or_else(|| format!("u64 read past EOF at {at:#x}"))?
        .try_into()
        .map_err(|_| format!("u64 read past EOF at {at:#x}"))?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PE32+ image: one `.text` section of 16 bytes at RVA 0x1000.
    fn synthetic_pe64() -> Vec<u8> {
        let mut data = vec![0u8; 0x400];
        data[0..2].copy_from_slice(b"MZ");
        data[DOS_SIGNATURE..DOS_SIGNATURE + 4].copy_from_slice(&0x80u32.to_le_bytes());
        let pe = 0x80;
        data[pe..pe + 4].copy_from_slice(PE_SIGNATURE);
        let coff = pe + 4;
        data[coff..coff + 2].copy_from_slice(&MACHINE_AMD64.to_le_bytes());
        data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // sections
        data[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes()); // optional size
        let optional = coff + 20;
        data[optional..optional + 2].copy_from_slice(&OPT_MAGIC_PE32_PLUS.to_le_bytes());
        data[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        let table = optional + 0xF0;
        data[table..table + 5].copy_from_slice(b".text");
        data[table + 8..table + 12].copy_from_slice(&16u32.to_le_bytes());
        data[table + 12..table + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        data[table + 16..table + 20].copy_from_slice(&16u32.to_le_bytes());
        data[table + 20..table + 24].copy_from_slice(&0x200u32.to_le_bytes());
        data[table + 36..table + 40].copy_from_slice(&SCN_MEM_EXECUTE.to_le_bytes());
        data[0x200..0x210].copy_from_slice(&[0x48, 0x8B, 0xC4, 0x55, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
        data
    }

    #[test]
    fn parses_pe64_headers_and_maps_rva() {
        let data = synthetic_pe64();
        let image = PeImage::parse(&data).expect("parse");
        assert_eq!(image.bits(), 64);
        assert_eq!(image.machine(), MACHINE_AMD64);
        assert_eq!(image.section_names(), vec![".text"]);
        assert_eq!(image.rva_to_offset(0x1000), Some(0x200));
        assert_eq!(image.offset_to_rva(0x204), Some(0x1004));
        assert!(image.in_text(0x204));
        assert!(!image.in_text(0x10));
    }

    #[test]
    fn code_segment_covers_the_executable_section() {
        let data = synthetic_pe64();
        let segment = PeImage::parse(&data).expect("parse").code_segment().expect("segment");
        assert_eq!(segment.bytes.len(), 16);
        assert_eq!(segment.vaddr, 0x1000);
        assert_eq!(segment.bits, 64);
        assert_eq!(&segment.bytes[..2], &[0x48, 0x8B]);
    }

    #[test]
    fn rejects_non_pe_and_truncated_input() {
        assert!(PeImage::parse(b"not a pe").is_err());
        let mut data = synthetic_pe64();
        data.truncate(0x40);
        assert!(PeImage::parse(&data).is_err());
    }
}
