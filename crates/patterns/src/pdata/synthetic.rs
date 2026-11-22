//! Hand-built image and `.pdata` bytes for the `pdata` tests.
//!
//! Shared by the parser tests and the anchor-gate tests so the gate cases and
//! the table cases are measured against one image layout, not two encoders that
//! can drift apart. Test-only: the parser itself is exercised against real
//! Steam modules elsewhere.

use super::RUNTIME_FUNCTION_SIZE;

/// Encode `(begin, end)` pairs the way `.pdata` stores them (unwind info is
/// irrelevant here, so it is a distinct dummy RVA per entry).
pub fn table(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(entries.len() * RUNTIME_FUNCTION_SIZE);
    for (index, (begin, end)) in entries.iter().enumerate() {
        bytes.extend_from_slice(&begin.to_le_bytes());
        bytes.extend_from_slice(&end.to_le_bytes());
        bytes.extend_from_slice(&(0x1_0000u32 + index as u32).to_le_bytes());
    }
    bytes
}

/// Minimal PE32+ image: one executable section holding code and one holding the
/// `.pdata` table, so `parse` can be tested through the real directory lookup.
///
/// Layout: DOS stub at 0, `PE\0\0` at 0x40, optional header 240 bytes, two
/// section headers, `.text` raw at 0x200 (RVA 0x1000, 0x100 bytes), `.pdata`
/// raw at 0x400 (RVA 0x2000).
pub fn image_bytes(code: &[u8], table: &[u8]) -> Vec<u8> {
    const PE_AT: usize = 0x40;
    const OPTIONAL: usize = PE_AT + 24;
    const SECTION_TABLE: usize = OPTIONAL + 240;
    let mut data = vec![0u8; 0x600];
    data[..2].copy_from_slice(b"MZ");
    data[0x3c..0x40].copy_from_slice(&(PE_AT as u32).to_le_bytes());
    data[PE_AT..PE_AT + 4].copy_from_slice(b"PE\0\0");
    // COFF: machine amd64, 2 sections, optional header 240 bytes.
    data[PE_AT + 4..PE_AT + 6].copy_from_slice(&crate::pe_image::MACHINE_AMD64.to_le_bytes());
    data[PE_AT + 6..PE_AT + 8].copy_from_slice(&2u16.to_le_bytes());
    data[PE_AT + 20..PE_AT + 22].copy_from_slice(&240u16.to_le_bytes());
    // PE32+ optional header: magic, image base, 16 data directories.
    data[OPTIONAL..OPTIONAL + 2].copy_from_slice(&0x020bu16.to_le_bytes());
    data[OPTIONAL + 24..OPTIONAL + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    data[OPTIONAL + 108..OPTIONAL + 112].copy_from_slice(&16u32.to_le_bytes());
    // Data directory 3 (exception directory) = the `.pdata` section.
    let dir = OPTIONAL + 112 + 3 * 8;
    data[dir..dir + 4].copy_from_slice(&0x2000u32.to_le_bytes());
    data[dir + 4..dir + 8].copy_from_slice(&(table.len() as u32).to_le_bytes());
    let mut section = |at: usize, name: &[u8], rva: u32, size: usize, raw: u32| {
        data[at..at + name.len()].copy_from_slice(name);
        let size = size as u32;
        data[at + 8..at + 12].copy_from_slice(&size.to_le_bytes());
        data[at + 12..at + 16].copy_from_slice(&rva.to_le_bytes());
        data[at + 16..at + 20].copy_from_slice(&size.to_le_bytes());
        data[at + 20..at + 24].copy_from_slice(&raw.to_le_bytes());
        data[at + 36..at + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes());
    };
    section(SECTION_TABLE, b".text\0\0\0", 0x1000, 0x100, 0x200);
    section(
        SECTION_TABLE + 40,
        b".pdata\0\0",
        0x2000,
        table.len(),
        0x400,
    );
    data[0x200..0x200 + code.len()].copy_from_slice(code);
    data[0x400..0x400 + table.len()].copy_from_slice(table);
    data
}
