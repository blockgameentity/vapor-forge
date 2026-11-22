//! Packet layout resolution for the Windows client.
//!
//! The Linux host discovers the `CNetPacket` field offsets at runtime by
//! decoding `CNetPacket::Init`. The corpus now anchors the whole family for both
//! live builds (`Init`, `Alloc`, `Release`), and `patterns::cnet_packet` learned
//! to decode an MS-x64 body, so the same discovery works here — with one
//! difference: the shipped hook bodies never dereference the packet, so the
//! decode happens once, during the install pass, and the result is published in
//! the host state.
//!
//! A decode failure is not fatal: the layout stays `None`, every message is
//! recorded as `NoLayout`, and Steam keeps its own request-code round trip.

use vapor_forge_patterns::cnet_packet::{PacketApi, decode_api_segment};
use vapor_forge_patterns::scan::CodeSegment;

use crate::abi::netpacket::Layout;

/// Corpus symbols the decode needs, in the order [`decode_api_segment`] takes.
pub const SYMBOLS: [&str; 3] = [
    "CNetPacket::Alloc",
    "CNetPacket::Init",
    "CNetPacket::Release",
];

/// Offsets (relative to the segment) of the three functions, when all resolved.
pub fn offsets<'a>(
    resolved: impl Iterator<Item = (&'a str, usize)>,
    segment: &CodeSegment<'_>,
) -> Option<[usize; 3]> {
    let mut found = [None; 3];
    for (name, offset) in resolved {
        if let Some(slot) = SYMBOLS.iter().position(|symbol| *symbol == name) {
            found[slot] = offset.checked_sub(segment.file_offset as usize);
        }
    }
    match found {
        [Some(alloc), Some(init), Some(release)] => Some([alloc, init, release]),
        _ => None,
    }
}

/// Decode the packet layout from a module's executable segment.
pub fn decode(segment: &CodeSegment<'_>, offsets: [usize; 3]) -> Result<Layout, String> {
    let api: PacketApi = decode_api_segment(segment, offsets[0], offsets[1], offsets[2])
        .map_err(|error| format!("packet decode failed: {error}"))?;
    Layout::new(api.fields.data, api.fields.size)
        .ok_or_else(|| format!("decoded layout is unusable: {api:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_are_relative_to_the_segment_and_need_all_three() {
        let segment = CodeSegment {
            bytes: &[0u8; 4],
            file_offset: 0x200,
            vaddr: 0x1000,
            bits: 64,
            container: vapor_forge_patterns::scan::Container::Pe,
        };
        let all = [
            ("CNetPacket::Alloc", 0x210usize),
            ("CNetPacket::Init", 0x220),
            ("CNetPacket::Release", 0x230),
            ("CUser::CheckAppOwnership", 0x999),
        ];
        assert_eq!(offsets(all.into_iter(), &segment), Some([0x10, 0x20, 0x30]));

        let missing = [
            ("CNetPacket::Alloc", 0x210usize),
            ("CNetPacket::Init", 0x220),
        ];
        assert_eq!(offsets(missing.into_iter(), &segment), None);
    }

    #[test]
    fn a_bad_offset_is_a_decode_error_not_a_panic() {
        let segment = CodeSegment {
            bytes: &[0x90u8; 32],
            file_offset: 0,
            vaddr: 0x1000,
            bits: 64,
            container: vapor_forge_patterns::scan::Container::Pe,
        };
        let error = decode(&segment, [0, 1, 2]).expect_err("nonsense cannot decode");
        assert!(error.contains("packet decode failed"), "{error}");
    }
}
