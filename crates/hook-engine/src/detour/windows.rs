//! The Windows detour backend.
//!
//! Unlike the unix build, MS-x64 does not go through `retour`: the entry patch is
//! planned to an instruction boundary, written through the engine's own writer
//! ([`crate::writer`]), and backed by a trampoline this process allocated, so a
//! replacement can call the original path. The same writer backs
//! `vapor-forge-host-windows`, so the two halves of the install cannot drift.
//!
//! `HookFn` is still `retour`'s `Function` trait even though no detour is built
//! with `retour` here: it is the crate's spelling for "a hookable function
//! pointer", callers are generic over it, and keeping it stable means hook sites
//! do not change shape between targets.

use std::marker::PhantomData;

use tracing::debug;

use crate::writer::{WindowsDetour, WriteError};

/// Function-pointer types the active backend can hook.
pub use retour::Function as HookFn;

/// An installed detour owning the entry patch and its trampoline.
///
/// Dropping the value restores the original entry bytes, restores the page
/// protection, and releases the trampoline; see [`WindowsDetour`].
pub struct Detour<F: HookFn> {
    detour: WindowsDetour,
    function: PhantomData<F>,
}

impl<F: HookFn> Detour<F> {
    /// Plan a detour without enabling it.
    ///
    /// The entry is read and a trampoline is allocated, but no module byte
    /// changes until [`Self::enable`]. That is the same contract the unix backend
    /// has, and callers rely on it to capture the original prologue first.
    ///
    /// # Safety
    /// `target` and `replacement` must both point to functions matching `F`.
    pub unsafe fn new(target: F, replacement: F) -> Result<Self, WriteError> {
        // SAFETY: the caller guarantees both function pointers are valid and use
        // signature `F`; `to_ptr` only erases their type.
        let detour = unsafe {
            WindowsDetour::prepare(target.to_ptr() as usize, replacement.to_ptr() as usize)
        }?;
        debug!(
            target = format_args!("{:#x}", detour.target()),
            trampoline = format_args!("{:#x}", detour.trampoline()),
            patch_len = detour.patch_len(),
            "detour prepared"
        );
        Ok(Self {
            detour,
            function: PhantomData,
        })
    }

    /// Write the entry jump.
    ///
    /// # Safety
    /// No other thread may be executing the bytes being overwritten.
    pub unsafe fn enable(&mut self) -> Result<(), WriteError> {
        // SAFETY: forwarded from the caller.
        unsafe { self.detour.install() }
    }

    /// Restore the original entry bytes, keeping the trampoline allocated.
    ///
    /// # Safety
    /// No other thread may be executing the bytes being restored.
    pub unsafe fn disable(&mut self) -> Result<(), WriteError> {
        // SAFETY: forwarded from the caller.
        unsafe { self.detour.uninstall() }
    }

    /// Whether the entry currently holds the jump.
    pub fn is_enabled(&self) -> bool {
        self.detour.is_installed()
    }

    /// Entry address of the trampoline holding the displaced instructions.
    ///
    /// Returns `&()` rather than a raw pointer so the signature matches the unix
    /// backend's `retour::GenericDetour::trampoline`; callers turn it into an
    /// address the same way on both targets. Valid until the detour is dropped.
    pub fn trampoline(&self) -> &() {
        // SAFETY: `()` is zero-sized, so every non-null address is a valid `&()`.
        // The trampoline entry comes from a successful allocation, and the
        // reference is only ever used to recover that address.
        unsafe { &*(self.detour.trampoline() as *const ()) }
    }
}
