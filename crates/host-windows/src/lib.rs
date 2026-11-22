//! Windows host for Vapor Forge.
//!
//! The host observes, decides which capabilities and host-agnostic features are
//! available, and reports. That is deliberate: the degradation contract ("disable
//! the failing capability with a reason, never brick the client") has to exist
//! before any hook is installed.
//!
//! [`features`] is where parity with the Linux host lives: cloud redirection,
//! per-game identity, depotcache pre-seed and the local achievement rebuild are
//! driven from the host's resolved settings, each behind a trait so the decisions
//! are testable without a live client. [`lifecycle`] composes the install pass and
//! the feature pass into one report.
//!
//! The Windows-specific FFI is confined to [`inject`], [`thread`] and
//! [`entry::windows`]; every other module is plain logic and is unit-tested on the
//! build host, so the crate builds and tests on Linux as well as on
//! `x86_64-pc-windows-gnu`.
//!
//! The crate is a loadable library (`cdylib`) as well as a library: [`entry::dll`]
//! is the `DllMain` a Steam process loads, and [`entry::worker`] is the flow it
//! spawns — see [`entry`]. [`hooks`] holds the replacement bodies that flow
//! installs, one per corpus symbol, and [`abi`] documents exactly which parts of
//! each call shape are measured and which are assumed.

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]

pub mod abi;
pub mod capability;
pub mod entry;
pub mod features;
pub mod hooks;
pub mod inject;
pub mod install;
pub mod lifecycle;
pub mod module;
pub mod proxy;
pub mod thread;

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
