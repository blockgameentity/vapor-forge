//! The installer that owns every detour for the process lifetime.
//!
//! Two properties this file exists for:
//!
//! 1. **The trampoline is published before the entry is patched.** The engine's
//!    writer splits `prepare` (read the entry, plan the boundary, allocate the
//!    trampoline) from `install` (write the jump), so the order here is:
//!    prepare → publish the displaced-instruction address → write the jump. A
//!    call that can reach a replacement body therefore always finds its callable
//!    original.
//! 2. **Nothing is ever dropped.** Dropping a `WindowsDetour` restores the
//!    original entry bytes and releases the trampoline, which is right at
//!    shutdown and wrong at any other moment: a replacement body would keep
//!    running from an entry that no longer jumps to it. The detour map is leaked,
//!    so the hooks stay installed until the process ends.

use std::collections::BTreeMap;

use tracing::{debug, warn};
use vapor_forge_hook_engine::writer::WindowsDetour;

use crate::hooks::original;
use crate::install::{DetourInstaller, InstallError, Installed};

/// One prepared and enabled detour per symbol, owned until the process exits.
pub struct ProcessDetours {
    detours: &'static mut BTreeMap<String, WindowsDetour>,
}

impl ProcessDetours {
    /// An installer whose storage outlives every caller.
    ///
    /// The leak is the point, not an oversight: see the module docs. One small
    /// map per hook batch is the whole cost.
    pub fn leak() -> Self {
        let detours: &'static mut BTreeMap<String, WindowsDetour> =
            Box::leak(Box::new(BTreeMap::new()));
        Self { detours }
    }

    /// Number of detours currently installed.
    pub fn len(&self) -> usize {
        self.detours.len()
    }

    pub fn is_empty(&self) -> bool {
        self.detours.is_empty()
    }
}

impl DetourInstaller for ProcessDetours {
    fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
        let Some(replacement) = crate::hooks::replacement_address(symbol) else {
            return Err(InstallError::NoReplacement {
                symbol: symbol.to_owned(),
            });
        };

        // SAFETY: the caller resolved `address` as this symbol's entry inside a
        // loaded module, and `replacement` is the body registered for exactly
        // that symbol. Steam may already be running other threads, so the jump
        // is written as soon as it is planned - the window a call could arrive in
        // is closed by publishing the trampoline first (next two statements).
        let mut detour = match unsafe { WindowsDetour::prepare(address, replacement) } {
            Ok(detour) => detour,
            Err(error) => {
                return Err(InstallError::Patch {
                    symbol: symbol.to_owned(),
                    address,
                    message: error.to_string(),
                });
            }
        };
        original::record(symbol, detour.trampoline());

        // SAFETY: as above; the plan was made for this entry and no other thread
        // may be executing the bytes while they change - the install pass runs
        // once, before Steam's own hook consumers are told anything.
        if let Err(error) = unsafe { detour.install() } {
            return Err(InstallError::Patch {
                symbol: symbol.to_owned(),
                address,
                message: error.to_string(),
            });
        }

        let trampoline = detour.trampoline();
        let patch_len = detour.patch_len();
        if self.detours.insert(symbol.to_owned(), detour).is_some() {
            warn!(
                symbol,
                "re-installed a symbol that was already hooked; the previous detour was restored"
            );
        }
        debug!(
            symbol,
            address = format_args!("{address:#x}"),
            replacement = format_args!("{replacement:#x}"),
            trampoline = format_args!("{trampoline:#x}"),
            patch_len,
            "detour installed from the process-owned installer"
        );
        Ok(Installed {
            symbol: symbol.to_owned(),
            address,
            trampoline: Some(trampoline),
        })
    }
}

/// The factory the Windows worker hands to the entry point.
///
/// Returns a fresh [`ProcessDetours`] per module: each batch owns its own
/// symbols, and both batches live for the process lifetime. The replacement
/// table is [`crate::hooks::replacement_address`], so the factory needs no
/// arguments.
pub fn installer_factory() -> impl FnMut() -> Box<dyn DetourInstaller> {
    || Box::new(ProcessDetours::leak())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symbol_without_a_replacement_body_fails_closed() {
        let mut installer = ProcessDetours::leak();
        let error = installer
            .install("CUser::NoSuchReplacement", 0x1800_0000)
            .expect_err("an unimplemented symbol must not be patched");
        assert!(matches!(error, InstallError::NoReplacement { .. }));
        assert!(installer.is_empty());
    }
}
