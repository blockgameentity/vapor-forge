//! The installation boundary.
//!
//! The plan is independent of how a detour is written, so tests can run the
//! whole resolve/install/report flow on Linux with a fake installer. The real
//! Win32 implementation only exists when compiling for Windows.
//!
//! `vapor-forge-hook-engine` is not reusable as-is on Windows: its page
//! protection calls go straight to `libc::mprotect`/`libc::sysconf` and are not
//! cfg-gated, even though its `retour` backend is portable. This trait is the
//! seam where a Win32-capable hook engine can replace the absolute-jump writer
//! below without changing the planner or the report.
//!
//! The Win32 writer patches a 12-byte absolute jump and has no trampoline, so
//! a replacement must not call the original entry point. Trampoline creation
//! belongs to the hook engine that replaces this seam.

use thiserror::Error;

/// A detour that was written into the live module.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Installed {
    pub symbol: String,
    pub address: usize,
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

/// Length of `mov rax, imm64; jmp rax`, the architecture-sized absolute jump
/// used on x86-64.
#[cfg(any(windows, test))]
const ABSOLUTE_JUMP_LEN: usize = 12;

/// Patch bytes that transfer control to `target` without a distance limit.
#[cfg(any(windows, test))]
fn absolute_jump(target: usize) -> [u8; ABSOLUTE_JUMP_LEN] {
    let mut patch = [0u8; ABSOLUTE_JUMP_LEN];
    patch[0] = 0x48; // REX.W
    patch[1] = 0xB8; // mov rax, imm64
    patch[2..10].copy_from_slice(&(target as u64).to_le_bytes());
    patch[10] = 0xFF; // jmp rax
    patch[11] = 0xE0;
    patch
}

#[cfg(windows)]
mod win32 {
    use std::collections::BTreeMap;

    use super::{absolute_jump, DetourInstaller, InstallError, Installed, ABSOLUTE_JUMP_LEN};
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
            let patch = absolute_jump(replacement);
            let protection =
                inject::make_writable(address, ABSOLUTE_JUMP_LEN).map_err(|error| {
                    InstallError::Patch {
                        symbol: symbol.to_owned(),
                        address,
                        message: error.to_string(),
                    }
                })?;

            // SAFETY: the caller resolved `address` as a function entry inside
            // the loaded module and `make_writable` accepted the same range.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    patch.as_ptr(),
                    address as *mut u8,
                    ABSOLUTE_JUMP_LEN,
                );
            }

            inject::restore(address, ABSOLUTE_JUMP_LEN, protection).map_err(|error| {
                InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                }
            })?;
            inject::flush_instruction_cache(address, ABSOLUTE_JUMP_LEN).map_err(|error| {
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
                "windows detour installed"
            );
            Ok(Installed {
                symbol: symbol.to_owned(),
                address,
            })
        }
    }
}

#[cfg(windows)]
pub use win32::Win32DetourInstaller;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_jump_encoding_is_a_full_register_move_then_jump() {
        let patch = absolute_jump(0x1_4000_1234);
        assert_eq!(
            patch,
            [0x48, 0xB8, 0x34, 0x12, 0x00, 0x40, 0x01, 0x00, 0x00, 0x00, 0xFF, 0xE0]
        );
    }
}
