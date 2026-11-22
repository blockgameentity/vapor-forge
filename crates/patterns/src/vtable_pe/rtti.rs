//! MSVC RTTI lookup for a vtable base.
//!
//! With RTTI enabled MSVC places a pointer to the class's
//! `RTTICompleteObjectLocator` immediately before the first virtual function
//! pointer, so `vtable[-1]` identifies the class. That is a *label*, not a
//! behavioural measurement, which is why it is used here only as corroboration:
//! a table is accepted on its structure (see [`super::validate`]) and the class
//! name, when present, is reported next to it.
//!
//! Layout used (x64, `RTTICompleteObjectLocator`):
//!
//! ```text
//! u32 signature (1) | u32 offset | u32 cd_offset | u32 type_descriptor_rva
//! ```
//!
//! and `TypeDescriptor` is `vftable ptr (8) | spare (8) | char name[]`, so the
//! mangled name starts 16 bytes into it. Every field is bounds-checked; any
//! surprise yields `None` rather than an error, because a table without RTTI is
//! a normal table.

use crate::pe_image::PeImage;

const COL_SIGNATURE: u32 = 1;
const TYPE_DESCRIPTOR_FROM_COL: u64 = 12;
const NAME_FROM_DESCRIPTOR: u64 = 16;
const MAX_NAME_BYTES: usize = 512;

/// Mangled type name of the class whose vtable starts at `table_rva`, e.g.
/// `.??_7CUser@@6B@`-style descriptors yield `.?AVCUser@@`.
pub fn class_name(image: &PeImage<'_>, table_rva: u32) -> Option<String> {
    let col_rva = column_locator_rva(image, table_rva)?;
    if read_u32(image, col_rva)? != COL_SIGNATURE {
        return None;
    }
    let descriptor_rva = u64::from(read_u32(image, col_rva + TYPE_DESCRIPTOR_FROM_COL)?);
    let name = c_string(image, descriptor_rva + NAME_FROM_DESCRIPTOR)?;
    // A `TypeDescriptor` is only believed when it really is a mangled type name;
    // otherwise `vtable[-1]` was some other data pointer.
    if name.starts_with(".?A") {
        Some(name)
    } else {
        None
    }
}

/// RVA of the `RTTICompleteObjectLocator` named by the qword before the table.
pub fn column_locator_rva(image: &PeImage<'_>, table_rva: u32) -> Option<u64> {
    let slot_rva = u64::from(table_rva).checked_sub(8)?;
    let value = read_u64(image, slot_rva)?;
    let image_base = image.image_base();
    if value < image_base {
        return None;
    }
    let rva = value - image_base;
    // The locator itself is data, never code: a qword before a table that
    // points into `.text` is a previous table's last slot, not RTTI.
    if image.bytes_at_rva(rva, 4).is_some() && !in_executable(image, rva) {
        Some(rva)
    } else {
        None
    }
}

/// True when the qword at `table_rva - 8` points into a non-executable section,
/// i.e. the table is preceded by data rather than by more function pointers.
pub fn preceded_by_data_pointer(image: &PeImage<'_>, table_rva: u32) -> bool {
    column_locator_rva(image, table_rva).is_some()
}

/// `.?AVCUser@@` -> `CUser`, `.?AUJob@...` -> `Job`, `.?AV?$CFoo@VBar@@@` ->
/// `CFoo`. Falls back to the input when it is not a mangled name.
pub fn short_name(mangled: &str) -> &str {
    let body = mangled.strip_prefix(".?A").unwrap_or(mangled);
    let body = match body.as_bytes().first() {
        Some(b'V' | b'U' | b'W' | b'T') if body.len() > 1 => &body[1..],
        _ => body,
    };
    let end = body.find(['@', '<']).unwrap_or(body.len());
    let name = &body[..end];
    name.strip_prefix("?$").unwrap_or(name)
}

fn in_executable(image: &PeImage<'_>, rva: u64) -> bool {
    let Ok(rva) = u32::try_from(rva) else {
        return false;
    };
    image.sections().iter().any(|section| {
        section.executable
            && rva >= section.virtual_address
            && rva < section.virtual_address.saturating_add(section.raw_size)
    })
}

fn read_u32(image: &PeImage<'_>, rva: u64) -> Option<u32> {
    let bytes = image.bytes_at_rva(rva, 4)?;
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_u64(image: &PeImage<'_>, rva: u64) -> Option<u64> {
    let bytes = image.bytes_at_rva(rva, 8)?;
    let mut buf = [0u8; 8];
    buf.copy_from_slice(bytes);
    Some(u64::from_le_bytes(buf))
}

fn c_string(image: &PeImage<'_>, rva: u64) -> Option<String> {
    let mut out = Vec::new();
    for index in 0..MAX_NAME_BYTES {
        let byte = *image.bytes_at_rva(rva + index as u64, 1)?.first()?;
        if byte == 0 {
            return Some(String::from_utf8_lossy(&out).into_owned());
        }
        if !byte.is_ascii_graphic() && byte != b' ' {
            return None;
        }
        out.push(byte);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::short_name;

    #[test]
    fn shortens_mangled_names() {
        assert_eq!(short_name(".?AVCUser@@"), "CUser");
        assert_eq!(short_name(".?AVCUserRemoteStorage@@"), "CUserRemoteStorage");
        assert_eq!(
            short_name(
                ".?AUAddPromoItemsJob@?1??AddPromoItems@CClientInventory@@UEAA_NPEAHPEBHI@Z@"
            ),
            "AddPromoItemsJob"
        );
        assert_eq!(
            short_name(".?AV?$CClientServiceMethodRPCImpl@VCBeginAuthSessionViaCredentials@AuthenticationService@@@@"),
            "CClientServiceMethodRPCImpl"
        );
        assert_eq!(short_name("not-mangled"), "not-mangled");
    }
}
