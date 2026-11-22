//! Ticket forging: create a ticket for a target app from a source app's ticket.

/// The source app whose ticket we clone from: Steamworks Common
/// Redistributables, which every account owns.
pub const SOURCE_APP_ID: u32 = 228980;

/// Offset of the SteamID within a standard ownership ticket.
const TICKET_STEAMID_OFFSET: usize = 8;

/// Size of the RSA signature at the end of a ticket.
const SIGNATURE_SIZE: usize = 128;

/// A ticket derived from a source ticket for a different appId.
pub struct ForgedTicket {
    /// The complete derived ticket data.
    pub data: Vec<u8>,
    /// Total ticket size (the data.len() value Steam should see).
    pub total_size: u32,
    /// Byte offset of the appId field within the derived ticket.
    pub app_id_offset: u32,
    /// Byte offset of the SteamID field within the derived ticket.
    pub steam_id_offset: u32,
    /// Byte offset where the signature begins.
    pub signature_offset: u32,
    /// Size of the signature in bytes.
    pub signature_size: u32,
}

/// Forge a ticket for `target_app_id` from a source ticket.
///
/// Layout: `[signed_data] [target_app_id le32] [signature]`
///
/// The target appId is inserted between the signed body and the signature.
/// The reported `total_size` matches the source ticket length (not the
/// physical buffer length), so the DRM's signature-coverage calculation
/// still spans only the originally signed bytes.
pub fn forge_from_source(source_ticket: &[u8], target_app_id: u32) -> Option<ForgedTicket> {
    if source_ticket.len() <= SIGNATURE_SIZE {
        return None;
    }
    let signed_size = source_ticket.len() - SIGNATURE_SIZE;

    let mut data = Vec::with_capacity(source_ticket.len() + 4);
    data.extend_from_slice(&source_ticket[..signed_size]);
    data.extend_from_slice(&target_app_id.to_le_bytes());
    data.extend_from_slice(&source_ticket[signed_size..]);

    let app_id_offset = signed_size as u32;
    let signature_offset = app_id_offset + 4;

    Some(ForgedTicket {
        total_size: source_ticket.len() as u32,
        app_id_offset,
        steam_id_offset: TICKET_STEAMID_OFFSET as u32,
        signature_offset,
        signature_size: SIGNATURE_SIZE as u32,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forge_from_source_produces_valid_ticket() {
        // Create a test source ticket: 200 bytes of body + 128 bytes of signature
        let mut source = vec![0u8; 328];
        // Put some recognizable data
        for (i, b) in source.iter_mut().enumerate() {
            *b = (i & 0xFF) as u8;
        }

        let forged = forge_from_source(&source, 480).expect("forge should succeed");

        // Physical buffer is 4 bytes larger (inserted appId) but the reported
        // size matches the source: the DRM's signature-coverage calculation
        // uses total_size - signature_size, and must stop at the original
        // signed span for the source ticket's signature to still verify.
        assert_eq!(forged.data.len(), 332);
        assert_eq!(forged.total_size, source.len() as u32);

        // The first 200 bytes (signed body) should be identical
        assert_eq!(&forged.data[..200], &source[..200]);

        // Bytes 200..204 should be appId 480 in little-endian
        assert_eq!(&forged.data[200..204], &480u32.to_le_bytes());

        // Bytes 204..332 should be the original signature (bytes 200..328)
        assert_eq!(&forged.data[204..332], &source[200..328]);

        assert_eq!(forged.app_id_offset, 200);
        assert_eq!(forged.signature_offset, 204);
        assert_eq!(forged.signature_size, 128);
        assert_eq!(forged.steam_id_offset, 8);
    }

    #[test]
    fn forge_rejects_too_small_ticket() {
        // Ticket smaller than signature size, cannot derive.
        let source = vec![0u8; 64];
        assert!(forge_from_source(&source, 480).is_none());
    }
}
