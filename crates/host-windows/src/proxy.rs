//! Proxy DLL planning.
//!
//! Steam is loaded through a proxy module (`version.dll`, `dwmapi.dll`, ...) so
//! the host DLL can be mapped before the client initialises. The proxy must
//! export *every* export of the module it replaces: a shortened list breaks the
//! client the moment Steam starts calling one we forgot.

use std::collections::BTreeSet;

use crate::HostError;

/// Forwarding table for a proxy that replaces `target_dll`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProxyPlan {
    /// DLL whose exports are shadowed (`SteamUI.dll`, `steamclient64.dll`, ...).
    pub target_dll: String,
    /// Export names to forward, sorted and deduplicated.
    pub exports: Vec<String>,
}

impl ProxyPlan {
    pub fn is_empty(&self) -> bool {
        self.exports.is_empty()
    }

    /// MSVC `.def` body that forwards each export to the real module.
    pub fn def_file(&self) -> String {
        let mut out = String::from("LIBRARY\nEXPORTS\n");
        for export in &self.exports {
            let target = &self.target_dll;
            out.push_str(&format!("  {export}={target}.{export}\n"));
        }
        out
    }
}

/// Build a proxy plan from the bytes of the module being shadowed.
pub fn plan(target_dll: &str, module_bytes: &[u8]) -> Result<ProxyPlan, HostError> {
    if vapor_forge_pe::pe_kind(module_bytes).is_none() {
        return Err(HostError::NotPe(target_dll.to_owned()));
    }
    let exports: BTreeSet<String> = vapor_forge_pe::export_names(module_bytes)
        .into_iter()
        .collect();
    if exports.is_empty() {
        return Err(HostError::Unsupported(format!(
            "{target_dll} exposes no named exports; a proxy cannot be generated"
        )));
    }
    Ok(ProxyPlan {
        target_dll: target_dll.to_owned(),
        exports: exports.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PE32+ with one export directory holding the given names.
    fn synthetic_pe_with_exports(names: &[&str]) -> Vec<u8> {
        let mut data = vec![0u8; 0x600];
        data[0..2].copy_from_slice(b"MZ");
        data[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        let pe = 0x40;
        data[pe..pe + 4].copy_from_slice(b"PE\0\0");
        let coff = pe + 4;
        data[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
        data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes()); // 1 section
        data[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes());
        let optional = coff + 20;
        data[optional..optional + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
        // Export directory RVA 0x1000, size 0x40.
        data[optional + 112..optional + 116].copy_from_slice(&0x1000u32.to_le_bytes());
        data[optional + 116..optional + 120].copy_from_slice(&0x40u32.to_le_bytes());
        // Section table: .rdata at RVA 0x1000, raw 0x200, 0x200 bytes.
        let table = optional + 0xF0;
        data[table..table + 6].copy_from_slice(b".rdata");
        data[table + 8..table + 12].copy_from_slice(&0x200u32.to_le_bytes());
        data[table + 12..table + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        data[table + 20..table + 24].copy_from_slice(&0x200u32.to_le_bytes());

        let dir = 0x200usize;
        data[dir + 24..dir + 28].copy_from_slice(&(names.len() as u32).to_le_bytes());
        data[dir + 28..dir + 32].copy_from_slice(&0x1040u32.to_le_bytes()); // functions
        data[dir + 32..dir + 36].copy_from_slice(&0x1080u32.to_le_bytes()); // names
        data[dir + 36..dir + 40].copy_from_slice(&0x10C0u32.to_le_bytes()); // ordinals
        let mut string_at = 0x1100u32;
        for (index, name) in names.iter().enumerate() {
            data[dir + 0x80 + index * 4..dir + 0x84 + index * 4]
                .copy_from_slice(&string_at.to_le_bytes());
            let off = 0x200 + (string_at - 0x1000) as usize;
            data[off..off + name.len()].copy_from_slice(name.as_bytes());
            data[off + name.len()] = 0;
            data[dir + 0xC0 + index * 2..dir + 0xC2 + index * 2]
                .copy_from_slice(&(index as u16).to_le_bytes());
            string_at += name.len() as u32 + 1;
        }
        data
    }

    #[test]
    fn forwards_every_named_export_exactly_once() {
        let bytes = synthetic_pe_with_exports(&["GetFileVersionInfoSizeW", "VerQueryValueW", "GetFileVersionInfoW"]);
        let plan = plan("version.dll", &bytes).expect("plan");
        assert_eq!(plan.exports.len(), 3);
        let def = plan.def_file();
        assert!(def.contains("  GetFileVersionInfoSizeW=version.dll.GetFileVersionInfoSizeW"));
        assert!(def.contains("  VerQueryValueW=version.dll.VerQueryValueW"));
    }

    #[test]
    fn duplicate_export_names_collapse() {
        let bytes = synthetic_pe_with_exports(&["A", "A", "B"]);
        let plan = plan("dwmapi.dll", &bytes).expect("plan");
        assert_eq!(plan.exports, vec!["A".to_owned(), "B".to_owned()]);
    }

    #[test]
    fn a_module_without_exports_is_refused_rather_than_silently_partial() {
        let bytes = synthetic_pe_with_exports(&[]);
        assert!(matches!(plan("dwmapi.dll", &bytes), Err(HostError::Unsupported(_))));
    }

    #[test]
    fn non_pe_input_is_refused() {
        assert!(matches!(plan("version.dll", b"nope"), Err(HostError::NotPe(_))));
    }
}
