//! The installation boundary.
//!
//! The plan is independent of how a detour is written, so tests can run the
//! whole resolve/install/report flow on Linux with a fake installer. The real
//! Win32 implementation only exists when compiling for Windows.
//!
//! `vapor-forge-hook-engine` now builds for Windows (its page protection has a
//! `VirtualProtect` backend), but its writer still targets the ELF layouts the
//! Linux host needs. This trait stays the seam: the planner and the report do
//! not care who writes the bytes.
//!
//! The Win32 writer patching the entry delegates the two decisions that must
//! never be improvised to [`super::patch`]: how many bytes are safe to overwrite
//! (whole instructions only) and what the displaced instructions become
//! (a trampoline in freshly allocated executable memory, so a replacement can
//! still call the original).

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

    use super::super::patch;
    use super::{DetourInstaller, InstallError, Installed};
    use crate::inject;

    /// Writes an absolute jump to a registered replacement function.
    ///
    /// Replacement functions are supplied by the caller: this layer resolves
    /// and patches an address, it does not invent hook bodies. A symbol without
    /// a replacement is a failed install, which disables only that capability.
    pub struct Win32DetourInstaller {
        replacements: BTreeMap<String, usize>,
    }

    impl Win32DetourInstaller {
        pub fn new(replacements: impl IntoIterator<Item = (String, usize)>) -> Self {
            Self {
                replacements: replacements.into_iter().collect(),
            }
        }
    }

    impl DetourInstaller for Win32DetourInstaller {
        fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
            let replacement = self.replacements.get(symbol).copied().ok_or_else(|| {
                InstallError::NoReplacement {
                    symbol: symbol.to_owned(),
                }
            })?;

            // Read enough of the entry to reach the 12-byte patch boundary; the
            // planner picks the shortest whole-instruction run and refuses
            // anything it cannot relocate by copying.
            let window = inject::read_bytes(address, 32).map_err(|error| InstallError::Patch {
                symbol: symbol.to_owned(),
                address,
                message: error.to_string(),
            })?;
            let plan = patch::plan(&window, address as u64).map_err(|error| {
                InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                }
            })?;

            let trampoline = inject::allocate_executable(plan.trampoline.len()).map_err(
                |error| InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                },
            )?;
            // SAFETY: `trampoline` is a fresh allocation of exactly this size.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    plan.trampoline.as_ptr(),
                    trampoline as *mut u8,
                    plan.trampoline.len(),
                );
            }
            inject::flush_instruction_cache(trampoline, plan.trampoline.len()).map_err(|error| {
                InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                }
            })?;

            let bytes = plan.patch_bytes(replacement as u64);
            let protection = inject::make_writable(address, plan.patch_len).map_err(|error| {
                InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                }
            })?;

            // SAFETY: the caller resolved `address` as a function entry inside
            // the loaded module, the plan covers whole instructions only, and
            // `make_writable` accepted the same range.
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), address as *mut u8, plan.patch_len);
            }

            if let Err(error) = inject::restore(address, plan.patch_len, protection) {
                // Leaving the page writable is worse than a dead hook: release
                // the trampoline and report the failure so the capability is
                // disabled rather than silently half-installed.
                let _ = inject::free_executable(trampoline);
                return Err(InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                });
            }
            inject::flush_instruction_cache(address, plan.patch_len).map_err(|error| {
                InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                }
            })?;

            tracing::info!(
                symbol,
                address = format_args!("{address:#x}"),
                replacement = format_args!("{replacement:#x}"),
                trampoline = format_args!("{trampoline:#x}"),
                patch_len = plan.patch_len,
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

    #[test]
    fn absolute_jump_encoding_is_a_full_register_move_then_jump() {
        let patch = super::super::patch::absolute_jump(0x1_4000_1234);
        assert_eq!(
            patch,
            [0x48, 0xB8, 0x34, 0x12, 0x00, 0x40, 0x01, 0x00, 0x00, 0x00, 0xFF, 0xE0]
        );
    }
}
