//! Corpus selection: `win64/<component>/<sha256>.toml`.
//!
//! Selection is keyed on the module bytes, never on the path or architecture:
//! public and publicbeta Steam builds are live at the same time and each has
//! its own corpus file.

use std::path::{Path, PathBuf};

use crate::module::{corpus_component, ModuleIdentity};

use super::PlanError;

/// Human-facing Steam build fingerprint used in every failure reason.
pub fn fingerprint(identity: &ModuleIdentity) -> String {
    format!(
        "{} ({} bit) sha256 {}",
        identity.name, identity.bits, identity.sha256
    )
}

/// Expected corpus path for `identity` under `corpus_root`.
pub fn path(corpus_root: &Path, identity: &ModuleIdentity) -> Result<PathBuf, PlanError> {
    let component = corpus_component(identity).ok_or_else(|| PlanError::UnknownModule {
        fingerprint: fingerprint(identity),
    })?;
    Ok(corpus_root
        .join("win64")
        .join(component)
        .join(format!("{}.toml", identity.sha256)))
}

/// Read the corpus file. A missing or unreadable file is a `PlanError` that
/// names the fingerprint and the exact path that was expected.
pub fn read(corpus_root: &Path, identity: &ModuleIdentity) -> Result<(PathBuf, String), PlanError> {
    let path = path(corpus_root, identity)?;
    let text = std::fs::read_to_string(&path).map_err(|source| PlanError::ReadCorpus {
        fingerprint: fingerprint(identity),
        path: path.clone(),
        source,
    })?;
    tracing::debug!(
        path = %path.display(),
        fingerprint = fingerprint(identity).as_str(),
        "selected Windows pattern corpus"
    );
    Ok((path, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ModuleIdentity {
        ModuleIdentity {
            name: "steamclient".to_owned(),
            bits: 64,
            sha256: "669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b".to_owned(),
        }
    }

    #[test]
    fn path_is_keyed_on_the_module_sha256() {
        let path = path(Path::new("/corpus"), &identity()).expect("path");
        assert_eq!(
            path,
            Path::new("/corpus/win64/steamclient/669fa08e259277fbf5a1dfe15ce88afe734e89151194304b44d92f418dcc985b.toml")
        );
    }

    #[test]
    fn unknown_modules_are_named_in_the_error() {
        let mut identity = identity();
        identity.name = "steam.exe".to_owned();
        let error = path(Path::new("/corpus"), &identity).expect_err("unknown module");
        assert!(error.to_string().contains("steam.exe"));
    }

    #[test]
    fn missing_file_names_the_fingerprint_and_path() {
        let error = read(Path::new("/definitely/not/here"), &identity()).expect_err("missing");
        let text = error.to_string();
        assert!(text.contains("669fa08e259277fb"));
        assert!(text.contains("win64/steamclient/669fa08e"));
    }
}
