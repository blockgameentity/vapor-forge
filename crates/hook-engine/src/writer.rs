//! Instruction-boundary detour writer, generic over the memory it touches.
//!
//! An in-place detour has to answer three questions, and this module answers
//! them once for every target:
//!
//! 1. How many whole instructions at the entry may be overwritten? That is
//!    [`crate::patch::planner`]'s job, and it refuses an entry it cannot
//!    describe instead of guessing.
//! 2. What keeps the original callable? A trampoline holding the displaced
//!    instructions followed by a jump back to the first undisplaced one.
//! 3. What makes the write safe? Every write goes through
//!    [`CodeMemory::write_code`], which owns the protection window and the
//!    instruction-cache flush for its platform.
//!
//! The state machine in [`InstalledPatch`] is deliberately free of OS calls: the
//! Win32 backend ([`crate::writer::WindowsDetour`]) supplies them, and the tests
//! supply a fake that fails on demand. That is what makes "a failed install
//! leaves the target bytes unchanged" checkable on a Linux CI machine.

use thiserror::Error;

use crate::exec::ExecError;
use crate::patch::{PatchError, PatchPlan, planner};

/// Bytes read from an entry before planning.
///
/// The entry patch is twelve bytes; the window is larger so the decoder can also
/// see the whole instruction that straddles the boundary (the longest x86
/// instruction is fifteen bytes).
pub(crate) const READ_WINDOW: usize = 32;

/// Executable memory that holds a prepared trampoline.
///
/// Dropping the value releases the allocation, so a detour that owns one cannot
/// leak it by forgetting to uninstall.
pub(crate) trait TrampolineAllocation {
    /// Executable entry address of the trampoline.
    fn entry(&self) -> usize;
}

/// The memory operations a detour writer needs from its platform.
pub(crate) trait CodeMemory {
    type Trampoline: TrampolineAllocation;

    /// Bytes starting at `address`, at most `len`, stopping at the end of the
    /// committed region that contains `address`.
    ///
    /// # Safety
    /// `address` must lie inside a region of this process that the target reads
    /// as data, and the returned window must not be assumed longer than the
    /// reader reports.
    unsafe fn read_code(&self, address: usize, len: usize) -> Result<Vec<u8>, WriteError>;

    /// Copy `bytes` into fresh executable memory, publish it, and hand back the
    /// allocation that owns it.
    fn install_trampoline(&mut self, bytes: &[u8]) -> Result<Self::Trampoline, WriteError>;

    /// Overwrite `bytes.len()` bytes at `address`, then flush the instruction
    /// cache. The previous page protection is restored before returning.
    ///
    /// # Safety
    /// `address` must be the entry of a validated target, `bytes` must be a
    /// complete instruction sequence for it, and no other thread may be
    /// executing those bytes while they change.
    unsafe fn write_code(&self, address: usize, bytes: &[u8]) -> Result<(), WriteError>;
}

/// Why a patch could not be prepared, installed, or removed.
#[derive(Debug, Error)]
pub enum WriteError {
    /// The entry could not be turned into an instruction-boundary patch.
    #[error(transparent)]
    Plan(#[from] planner::PlanError),
    /// The write itself failed (protection or encoding).
    #[error(transparent)]
    Patch(#[from] PatchError),
    /// Executable memory for the trampoline could not be provided.
    #[error(transparent)]
    Exec(#[from] ExecError),
    /// The memory region holding the address could not be queried, so the
    /// number of readable bytes is unknown and nothing may be read from it.
    #[error("the memory region holding {address:#x} could not be queried")]
    RegionUnknown { address: usize },
}

/// A planned and prepared detour that is not installed yet.
///
/// Creation is side-effect free with respect to the target: [`Self::prepare`]
/// reads the entry and allocates the trampoline, and only [`Self::enable`]
/// overwrites code. That split is what lets an install fail without touching the
/// module.
#[derive(Debug)]
pub(crate) struct InstalledPatch<T: TrampolineAllocation> {
    target: usize,
    plan: PatchPlan,
    /// Entry bytes the patch overwrites, kept so uninstall can restore them
    /// byte for byte.
    original: Vec<u8>,
    /// The jump, padded with `nop` to the planned instruction boundary.
    patch: Vec<u8>,
    trampoline: T,
    enabled: bool,
}

impl<T: TrampolineAllocation> InstalledPatch<T> {
    /// Read the entry, plan the patch, and allocate the trampoline.
    ///
    /// The entry keeps its original bytes until [`Self::enable`] succeeds.
    ///
    /// # Safety
    /// `target` must be the entry of a function inside a live image and
    /// `replacement` a function whose ABI and signature match it.
    pub(crate) unsafe fn prepare<M: CodeMemory<Trampoline = T>>(
        memory: &mut M,
        target: usize,
        replacement: usize,
    ) -> Result<Self, WriteError> {
        // SAFETY: forwarded from the caller; the backend bounds the read to the
        // committed region that holds `target`.
        let window = unsafe { memory.read_code(target, READ_WINDOW) }?;
        let plan = planner::plan(&window, target as u64)?;
        let original = window
            .get(..plan.patch_len)
            .ok_or(planner::PlanError::TooShort {
                needed: plan.patch_len,
                available: window.len(),
            })?
            .to_vec();
        let patch = plan.patch_bytes(replacement as u64);
        let trampoline = memory.install_trampoline(&plan.trampoline)?;
        Ok(Self {
            target,
            plan,
            original,
            patch,
            trampoline,
            enabled: false,
        })
    }

    /// Write the entry jump.
    ///
    /// Does nothing when the patch is already enabled, so a second enable is not
    /// an error and does not write twice.
    ///
    /// # Safety
    /// No other thread may be executing the entry bytes while they change.
    pub(crate) unsafe fn enable<M: CodeMemory<Trampoline = T>>(
        &mut self,
        memory: &M,
    ) -> Result<(), WriteError> {
        if self.enabled {
            return Ok(());
        }
        // SAFETY: forwarded from the caller; `self.patch` is a complete jump
        // sequence planned for `self.target`.
        unsafe { memory.write_code(self.target, &self.patch) }?;
        self.enabled = true;
        Ok(())
    }

    /// Restore the original entry bytes.
    ///
    /// The trampoline stays allocated so the detour can be enabled again; it is
    /// released when the value is dropped.
    ///
    /// # Safety
    /// No other thread may be executing the entry bytes while they change.
    pub(crate) unsafe fn disable<M: CodeMemory<Trampoline = T>>(
        &mut self,
        memory: &M,
    ) -> Result<(), WriteError> {
        if !self.enabled {
            return Ok(());
        }
        // SAFETY: forwarded from the caller; `self.original` is exactly the byte
        // range this patch overwrote.
        unsafe { memory.write_code(self.target, &self.original) }?;
        self.enabled = false;
        Ok(())
    }

    /// Address the patch was planned for.
    pub(crate) fn target(&self) -> usize {
        self.target
    }

    /// Number of entry bytes the patch overwrites.
    pub(crate) fn patch_len(&self) -> usize {
        self.plan.patch_len
    }

    /// Executable entry address of the trampoline that holds the displaced
    /// instructions.
    pub(crate) fn trampoline_entry(&self) -> usize {
        self.trampoline.entry()
    }

    /// Whether the entry currently holds the jump.
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(windows)]
mod win32;

#[cfg(windows)]
pub use win32::WindowsDetour;

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
