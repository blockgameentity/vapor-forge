//! The installation boundary.
//!
//! The plan is independent of how a detour is written, so tests can run the
//! whole resolve/install/report flow on Linux with a fake installer. The real
//! Win32 implementation only exists when compiling for Windows.
//!
//! The Win32 writer itself lives in `vapor-forge-hook-engine`, not here: the
//! engine grew its own Windows detour writer
//! (`vapor_forge_hook_engine::writer::WindowsDetour`) for the same reason this
//! crate needs one, and two writers would answer "how many instructions may this
//! patch displace" twice. The engine's writer plans the instruction-boundary
//! patch, allocates the trampoline, writes through `VirtualProtect`, flushes the
//! instruction cache, and — because the installer keeps ownership of the value —
//! restores the original bytes when the hook is dropped. This module only
//! decides which replacement belongs to which resolved symbol and reports what
//! happened.

use thiserror::Error;

/// A detour that was written into the live module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Installed {
    pub symbol: String,
    pub address: usize,
    /// Executable address of the displaced-instruction trampoline, when one was
    /// created. `None` for installers that do not preserve the original path.
    pub trampoline: Option<usize>,
}

/// One attempted hook installation.
pub trait DetourInstaller {
    /// Install a detour for `symbol` at `address` (the resolved function entry).
    fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError>;
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("{symbol} has no replacement function registered")]
    NoReplacement { symbol: String },
    #[error("{symbol} at {address:#x} could not be patched: {message}")]
    Patch {
        symbol: String,
        address: usize,
        message: String,
    },
}

#[cfg(windows)]
mod win32 {
    use std::collections::BTreeMap;

    use vapor_forge_hook_engine::writer::WindowsDetour;

    use super::{DetourInstaller, InstallError, Installed};

    /// Writes an absolute jump to a registered replacement function.
    ///
    /// Replacement functions are supplied by the caller: this layer resolves and
    /// patches an address, it does not invent hook bodies. A symbol without a
    /// replacement is a failed install, which disables only that capability; an
    /// entry the planner refuses is a failed install for the same reason, and
    /// leaves the module untouched.
    ///
    /// The installer owns every detour it installed, so dropping it restores the
    /// original entry bytes and releases each trampoline.
    pub struct Win32DetourInstaller {
        replacements: BTreeMap<String, usize>,
        installed: BTreeMap<String, WindowsDetour>,
    }

    impl Win32DetourInstaller {
        pub fn new(replacements: impl IntoIterator<Item = (String, usize)>) -> Self {
            Self {
                replacements: replacements.into_iter().collect(),
                installed: BTreeMap::new(),
            }
        }

        /// Number of detours this installer currently owns.
        pub fn installed_len(&self) -> usize {
            self.installed.len()
        }
    }

    impl DetourInstaller for Win32DetourInstaller {
        fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
            let replacement = self.replacements.get(symbol).copied().ok_or_else(|| {
                InstallError::NoReplacement {
                    symbol: symbol.to_owned(),
                }
            })?;

            // SAFETY: the caller resolved `address` as `symbol`'s entry inside a
            // loaded module and registered `replacement` for that symbol. Install
            // happens during startup, before the hooked entry can be reached from
            // another thread.
            let detour = unsafe { WindowsDetour::new(address, replacement) }.map_err(|error| {
                InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                }
            })?;

            let trampoline = detour.trampoline();
            let patch_len = detour.patch_len();
            // Replacing a symbol this installer already hooked drops the previous
            // detour, which restores the bytes it wrote before the new jump lands.
            let previous = self.installed.insert(symbol.to_owned(), detour);
            if previous.is_some() {
                tracing::warn!(symbol, "re-installed a symbol that was already hooked");
            }

            tracing::info!(
                symbol,
                address = format_args!("{address:#x}"),
                replacement = format_args!("{replacement:#x}"),
                trampoline = format_args!("{trampoline:#x}"),
                patch_len,
                "windows detour installed"
            );
            Ok(Installed {
                symbol: symbol.to_owned(),
                address,
                trampoline: Some(trampoline),
            })
        }
    }
}

#[cfg(windows)]
pub use win32::Win32DetourInstaller;

#[cfg(test)]
mod tests {
    use super::{DetourInstaller, InstallError, Installed};

    /// What a caller sees when a symbol has no replacement: a failed capability,
    /// not a panic and not a half-written entry.
    struct Empty;
    impl DetourInstaller for Empty {
        fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
            Err(InstallError::NoReplacement {
                symbol: symbol.to_owned(),
            })
            .inspect_err(|_| assert_ne!(address, 0, "a resolved entry is never null"))
        }
    }

    #[test]
    fn a_symbol_without_a_replacement_fails_closed() {
        let error = Empty.install("CSteamEngine::RunFrame", 0x1800_0000).unwrap_err();
        assert!(matches!(error, InstallError::NoReplacement { .. }));
        assert!(error.to_string().contains("CSteamEngine::RunFrame"));
    }
}
