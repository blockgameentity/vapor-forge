//! The Win32 memory backend for the detour writer.
//!
//! MS-x64 entry patches are installed by the engine itself rather than by the
//! `retour` backend the unix build uses: the planner has to refuse an entry it
//! cannot copy, the trampoline has to be a page this process owns, and uninstall
//! has to put the original bytes back. [`WindowsDetour`] is that whole cycle over
//! [`Win32Memory`], with every platform call going through
//! [`crate::protect`]/[`crate::exec`] so the protection window and the instruction
//! cache are handled in one place.

use tracing::{debug, error, info};

use crate::exec::ExecutableMemory;
use crate::patch;
use crate::win_api;

use super::{CodeMemory, InstalledPatch, WriteError};

/// The live Windows process, viewed as code memory.
#[derive(Debug)]
pub(crate) struct Win32Memory;

impl CodeMemory for Win32Memory {
    type Trampoline = ExecutableMemory;

    /// # Safety
    /// `address` must lie inside a region of this process that is read as data;
    /// the read itself is bounded by `VirtualQuery`.
    unsafe fn read_code(&self, address: usize, len: usize) -> Result<Vec<u8>, WriteError> {
        // Bound the read by the committed region that holds the entry: a function
        // entry can sit near the end of its section, and reading past it would
        // fault instead of returning a short window.
        let available =
            win_api::readable_len(address, len).ok_or(WriteError::RegionUnknown { address })?;
        let mut bytes = vec![0u8; available];
        if available > 0 {
            // SAFETY: VirtualQuery reported `available` readable bytes starting at
            // `address`, and the buffer is exactly that long.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    address as *const u8,
                    bytes.as_mut_ptr(),
                    available,
                );
            }
        }
        Ok(bytes)
    }

    fn install_trampoline(&mut self, bytes: &[u8]) -> Result<Self::Trampoline, WriteError> {
        Ok(ExecutableMemory::new(bytes)?)
    }

    /// # Safety
    /// `address` must be the entry of a validated target, `bytes` must be a
    /// complete instruction sequence for it, and no other thread may be
    /// executing those bytes while they change.
    unsafe fn write_code(&self, address: usize, bytes: &[u8]) -> Result<(), WriteError> {
        // SAFETY: the writer's contract, checked at the entry point of
        // `WindowsDetour::prepare`: `address` is a resolved function entry inside
        // a loaded image, `bytes` is a complete jump sequence for it, and the
        // caller owns the concurrency rule.
        unsafe { patch::write_code_patch(address, bytes) }?;
        Ok(())
    }
}

/// An installed detour with a callable original.
///
/// The entry is patched with an absolute jump to the replacement, and the
/// displaced instructions live in a trampoline this process owns, so a
/// replacement can still call the original path through
/// [`Self::trampoline`]. Dropping the value restores the original entry bytes,
/// restores the page protection, and releases the trampoline.
#[derive(Debug)]
pub struct WindowsDetour {
    memory: Win32Memory,
    patch: InstalledPatch<ExecutableMemory>,
}

impl WindowsDetour {
    /// Read `target`, plan the instruction-boundary patch, and allocate the
    /// trampoline. No byte of the target changes.
    ///
    /// # Safety
    /// `target` must be the entry of a function inside an image loaded in this
    /// process, and `replacement` must be a function whose ABI and signature
    /// match it.
    pub unsafe fn prepare(target: usize, replacement: usize) -> Result<Self, WriteError> {
        let mut memory = Win32Memory;
        // SAFETY: forwarded from the caller.
        let patch = unsafe { InstalledPatch::prepare(&mut memory, target, replacement) }?;
        debug!(
            target = format_args!("{target:#x}"),
            replacement = format_args!("{replacement:#x}"),
            trampoline = format_args!("{:#x}", patch.trampoline_entry()),
            patch_len = patch.patch_len(),
            "windows detour prepared"
        );
        Ok(Self { memory, patch })
    }

    /// Prepare and install in one step.
    ///
    /// # Safety
    /// Same contract as [`Self::prepare`], plus the concurrency rule: no other
    /// thread may be executing the bytes being overwritten.
    pub unsafe fn new(target: usize, replacement: usize) -> Result<Self, WriteError> {
        // SAFETY: forwarded from the caller.
        let mut detour = unsafe { Self::prepare(target, replacement) }?;
        // SAFETY: forwarded from the caller.
        unsafe { detour.install() }?;
        Ok(detour)
    }

    /// Write the entry jump.
    ///
    /// # Safety
    /// No other thread may be executing the bytes being overwritten.
    pub unsafe fn install(&mut self) -> Result<(), WriteError> {
        // SAFETY: forwarded from the caller; the patch was planned for this
        // target by `prepare`.
        unsafe { self.patch.enable(&self.memory) }?;
        info!(
            target = format_args!("{:#x}", self.patch.target()),
            trampoline = format_args!("{:#x}", self.patch.trampoline_entry()),
            patch_len = self.patch.patch_len(),
            "windows detour installed"
        );
        Ok(())
    }

    /// Address the detour was installed at.
    pub fn target(&self) -> usize {
        self.patch.target()
    }

    /// Executable entry address of the trampoline holding the displaced
    /// instructions. Valid until the detour is dropped.
    pub fn trampoline(&self) -> usize {
        self.patch.trampoline_entry()
    }

    /// Number of entry bytes the jump overwrites.
    pub fn patch_len(&self) -> usize {
        self.patch.patch_len()
    }

    /// Whether the entry currently holds the jump.
    pub fn is_installed(&self) -> bool {
        self.patch.is_enabled()
    }

    /// Restore the original entry bytes, keeping the trampoline allocated so the
    /// detour can be installed again.
    ///
    /// # Safety
    /// Same concurrency rule as [`Self::new`]: no other thread may be executing
    /// the bytes being restored.
    pub unsafe fn uninstall(&mut self) -> Result<(), WriteError> {
        // SAFETY: forwarded from the caller.
        unsafe { self.patch.disable(&self.memory) }
    }
}

impl Drop for WindowsDetour {
    fn drop(&mut self) {
        if !self.patch.is_enabled() {
            return;
        }
        // SAFETY: dropping the value is the point at which the caller stops
        // calling through the patched entry, exactly as documented on
        // `WindowsDetour::new`.
        match unsafe { self.patch.disable(&self.memory) } {
            Ok(()) => debug!(
                target = format_args!("{:#x}", self.patch.target()),
                "windows detour uninstalled on drop"
            ),
            Err(err) => error!(
                target = format_args!("{:#x}", self.patch.target()),
                %err,
                "windows detour could not be uninstalled on drop; the entry still holds the jump"
            ),
        }
    }
}
