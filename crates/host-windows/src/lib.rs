//! Windows host for Vapor Forge.
//!
//! Phase 1 scope: module identity, capability gating and proxy export
//! generation. Nothing here changes Steam's behaviour yet — the host only
//! observes, decides which capabilities are available, and reports. That is
//! deliberate: the degradation contract ("disable the failing capability with a
//! reason, never brick the client") has to exist before any hook is installed.
//!
//! The Windows-specific FFI is confined to [`inject`]; every other module is
//! plain logic and is unit-tested on the build host, so the crate builds and
//! tests on Linux as well as on `x86_64-pc-windows-gnu`.

pub mod capability;
pub mod inject;
pub mod module;
pub mod proxy;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum HostError {
    #[error("io error reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{0} is not a PE image")]
    NotPe(String),
    #[error("unsupported PE bitness in {0}")]
    Bitness(String),
    #[error("{0}")]
    Unsupported(String),
}

/// Read a Steam module from disk and identify it for corpus selection.
pub fn identify_path(path: &std::path::Path) -> Result<module::ModuleIdentity, HostError> {
    let bytes = std::fs::read(path).map_err(|source| HostError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    module::identify(&name, &bytes)
}
