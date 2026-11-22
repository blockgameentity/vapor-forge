//! Module identity: what the pattern corpus is keyed on.
//!
//! Windows corpus files are selected by the sha256 of the module binary (see
//! `res/patterns/win64/README.md`), so identity has to be derived from the file
//! itself, never from its path.

use sha2::{Digest, Sha256};

use crate::HostError;

/// Identity of one Steam module in the running (or inspected) client.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleIdentity {
    /// File name as Steam ships it (`steamclient64.dll`, `SteamUI.dll`).
    pub name: String,
    /// 32 or 64, from the PE optional header.
    pub bits: u8,
    /// Lowercase hex sha256 of the whole file; the corpus key.
    pub sha256: String,
}

/// Identify a module from its name and bytes.
pub fn identify(name: &str, bytes: &[u8]) -> Result<ModuleIdentity, HostError> {
    let kind = vapor_forge_pe::pe_kind(bytes).ok_or_else(|| HostError::NotPe(name.to_owned()))?;
    let bits = match kind {
        vapor_forge_pe::PeKind::Pe32 => 32,
        vapor_forge_pe::PeKind::Pe32Plus => 64,
    };
    Ok(ModuleIdentity {
        name: normalize_name(name, bits),
        bits,
        sha256: hex(&Sha256::digest(bytes)),
    })
}

/// Steam is inconsistent about case (`SteamUI.dll` vs `steamui.dll`); the
/// corpus directory name must not depend on how the file happened to be spelled.
pub fn normalize_name(name: &str, _bits: u8) -> String {
    let lower = name.to_ascii_lowercase();
    match lower.as_str() {
        "steamclient64.dll" => "steamclient".to_owned(),
        "steamclient.dll" => "steamclient32".to_owned(),
        "steamui.dll" => "steamui".to_owned(),
        _ => lower,
    }
}

/// Which corpus component a module maps to (`steamclient` / `steamui`).
pub fn corpus_component(identity: &ModuleIdentity) -> Option<&'static str> {
    match identity.name.as_str() {
        "steamclient" | "steamclient32" => Some("steamclient"),
        "steamui" => Some("steamui"),
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_pe64() -> Vec<u8> {
        let mut data = vec![0u8; 0x200];
        data[0..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        let pe = 0x40;
        data[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        data[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        data[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes());
        data[coff + 20..coff + 22].copy_from_slice(&0x20Bu16.to_le_bytes());
        data
    }

    #[test]
    fn identifies_a_pe64_module_and_keys_on_its_bytes() {
        let identity = identify("steamclient64.dll", &synthetic_pe64()).expect("identify");
        assert_eq!(identity.name, "steamclient");
        assert_eq!(identity.bits, 64);
        assert_eq!(identity.sha256.len(), 64);
        assert_eq!(corpus_component(&identity), Some("steamclient"));

        let mut changed = synthetic_pe64();
        changed[0x100] = 0xFF;
        let other = identify("steamclient64.dll", &changed).expect("identify");
        assert_ne!(identity.sha256, other.sha256, "corpus key must track content");
    }

    #[test]
    fn steamui_spelling_does_not_affect_the_component() {
        let upper = identify("SteamUI.dll", &synthetic_pe64()).expect("identify");
        let lower = identify("steamui.dll", &synthetic_pe64()).expect("identify");
        assert_eq!(upper.name, "steamui");
        assert_eq!(upper.name, lower.name);
        assert_eq!(corpus_component(&upper), Some("steamui"));
    }

    #[test]
    fn rejects_non_pe_input() {
        assert!(matches!(
            identify("nope.dll", b"not a pe"),
            Err(HostError::NotPe(_))
        ));
    }
}
