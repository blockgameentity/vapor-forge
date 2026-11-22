//! Mined-corpus anchor resolution.
//!
//! Duplicated from `bin/vapor-forge-pe-functions.rs::anchors_of` because a binary
//! cannot import another binary's helpers; only the module filter and the
//! `rva -> name` direction are new. Anchors that resolve to a non-entry are
//! reported rather than dropped: on `steamclient64.dll` the `GetPipeClient`
//! pattern lands inside a function, and that is a fact about the corpus.

use std::collections::BTreeMap;
use std::path::Path;

use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::registry::parse_toml_patterns;
use vapor_forge_patterns::scan::{
    group_variants, resolve_entry_group_with, select_variants_for_library_path, PatternRef,
};
use vapor_forge_patterns::vtable_pe::Functions;

pub fn resolve_anchors(
    path: &Path,
    image: &PeImage<'_>,
    functions: &Functions,
    module: &str,
) -> Result<BTreeMap<u32, String>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    let parsed = parse_toml_patterns(&text)?;
    let refs = parsed
        .iter()
        .filter(|(_, entry)| entry.module == module)
        .map(PatternRef::from)
        .collect::<Vec<PatternRef<'_>>>();
    if refs.is_empty() {
        return Err(format!(
            "corpus {} has no {module:?} entries",
            path.display()
        ));
    }
    let segment = image.code_segment()?;
    let selected = select_variants_for_library_path(&refs, path);
    let mut anchors = BTreeMap::new();
    for group in group_variants(&selected) {
        let name = group[0].name.to_owned();
        match resolve_entry_group_with(segment.bytes, u32::from(segment.bits), &group, None) {
            Ok(result) => {
                let rva = u32::try_from(segment.vaddr + result.target_offset as u64)
                    .map_err(|_| "resolved rva exceeds u32".to_owned())?;
                if !functions.is_entry(rva) {
                    println!("anchor {name}: resolved to {rva:#x}, not a .pdata entry");
                }
                anchors.insert(rva, name);
            }
            Err(error) => println!("anchor {name}: unresolved ({error:?})"),
        }
    }
    Ok(anchors)
}
