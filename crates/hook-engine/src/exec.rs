//! Executable memory for detour trampolines.
//!
//! A trampoline is code the engine produces at runtime: the instructions
//! displaced from the entry followed by a jump back. It needs its own
//! allocation, because nothing else owns a writable-then-executable page that
//! outlives the install.
//!
//! Backends:
//!
//! * windows — `VirtualAlloc`/`VirtualFree`, page hardened to read-execute after
//!   the copy and the instruction cache flushed.
//! * everything else — this crate's unix backend is `retour`, which allocates
//!   and protects trampolines itself, so [`ExecutableMemory::new`] reports
//!   [`ExecError::Unsupported`] rather than inventing a second allocator.

use crate::protect::ProtectError;
use crate::writer::TrampolineAllocation;

/// Why executable memory could not be provided.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ExecError {
    /// A trampoline always holds at least the jump back.
    #[error("an executable allocation must hold at least one byte")]
    Empty,
    /// The platform allocator refused.
    #[error("executable allocation of {len} bytes failed")]
    Allocate { len: usize },
    /// The allocation succeeded but could not be made read-execute.
    #[error(transparent)]
    Protect(#[from] ProtectError),
    /// This target's detour backend owns its own trampoline memory.
    #[error("this target allocates detour trampolines through its detour backend")]
    Unsupported,
}

/// One executable allocation, released on drop.
#[derive(Debug)]
pub struct ExecutableMemory {
    address: usize,
    len: usize,
}

impl ExecutableMemory {
    /// Copy `bytes` into fresh executable memory and harden the page.
    ///
    /// The allocation is read-write-execute only while the bytes are copied; it
    /// is read-execute from the moment this returns, and it is released when the
    /// value is dropped.
    pub fn new(bytes: &[u8]) -> Result<Self, ExecError> {
        if bytes.is_empty() {
            return Err(ExecError::Empty);
        }
        #[cfg(windows)]
        {
            let memory = Self::allocate(bytes.len())?;
            // A trampoline page that cannot be hardened is not handed out: the
            // error drops the allocation, so no read-write-execute page survives
            // a failed install.
            memory.copy_in(bytes)?;
            Ok(memory)
        }
        #[cfg(not(windows))]
        {
            // Documented above: the unix backend allocates trampolines itself.
            let _ = bytes;
            Err(ExecError::Unsupported)
        }
    }

    #[cfg(windows)]
    fn allocate(len: usize) -> Result<Self, ExecError> {
        let address =
            crate::win_api::allocate_executable(len).ok_or(ExecError::Allocate { len })?;
        Ok(Self { address, len })
    }

    /// Copy `bytes` to the allocation, publish them, and drop to read-execute.
    #[cfg(windows)]
    fn copy_in(&self, bytes: &[u8]) -> Result<(), ExecError> {
        // SAFETY: the allocation is exactly `self.len` bytes of committed
        // read-write-execute memory and `bytes.len() <= self.len`, so both the
        // destination range and the source are valid for the copy.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.address as *mut u8, bytes.len());
        }
        crate::protect::flush_instruction_cache(self.address, bytes.len());
        crate::protect::set_protection(
            self.address,
            self.len,
            crate::protect::Protection::READ_EXECUTE,
        )?;
        Ok(())
    }

    /// Entry address of the allocation.
    pub fn entry(&self) -> usize {
        self.address
    }

    /// Length in bytes of the allocation.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Always false: a zero-length allocation is rejected by [`Self::new`].
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl TrampolineAllocation for ExecutableMemory {
    fn entry(&self) -> usize {
        self.address
    }
}

impl Drop for ExecutableMemory {
    fn drop(&mut self) {
        // SAFETY: this is the single place the allocation created in `allocate`
        // is released, and `self.address` came from that call.
        let released = unsafe { drop_allocation(self.address) };
        if !released {
            tracing::error!(
                address = format_args!("{:#x}", self.address),
                len = self.len,
                "executable allocation could not be released"
            );
        }
    }
}

/// Release the allocation through this target's allocator.
///
/// # Safety
/// `address` must be a base address returned by the matching allocation call
/// exactly once.
#[cfg(windows)]
unsafe fn drop_allocation(address: usize) -> bool {
    crate::win_api::free_executable(address)
}

/// # Safety
/// Nothing was allocated on this target, so the caller must not have anything to
/// release.
#[cfg(not(windows))]
unsafe fn drop_allocation(address: usize) -> bool {
    let _ = address;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_allocations_are_refused_before_any_platform_call() {
        assert_eq!(ExecutableMemory::new(&[]).err(), Some(ExecError::Empty));
    }

    #[test]
    fn targets_without_an_engine_allocator_say_so() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(
            ExecutableMemory::new(b"\x90\xC3").err(),
            Some(ExecError::Unsupported)
        );
    }

    #[cfg(windows)]
    #[test]
    fn allocations_are_usable_and_released() {
        let memory = ExecutableMemory::new(b"\x90\xC3").expect("allocation");
        assert_ne!(memory.entry(), 0);
        assert_eq!(memory.len(), 2);
        assert!(!memory.is_empty());
        // SAFETY: the allocation is two bytes this test owns and just filled.
        let bytes = unsafe { std::slice::from_raw_parts(memory.entry() as *const u8, 2) };
        assert_eq!(bytes, b"\x90\xC3");
    }
}
