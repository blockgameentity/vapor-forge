//! A fake module image for the writer tests.
//!
//! The writer's [`super::CodeMemory`] seam exists so this can drive the exact
//! sequence the Win32 backend runs without a Windows host: a module image it can
//! read and overwrite, an allocator whose trampolines report when they are freed,
//! and a switch that makes the next write fail the way a rejected
//! `VirtualProtect` would.

use std::cell::RefCell;
use std::rc::Rc;

use super::{CodeMemory, PatchError, TrampolineAllocation, WriteError};
use crate::protect::ProtectError;

/// Everything the fake process owns; shared with the trampolines it hands out so
/// a drop is observable.
#[derive(Debug, Default)]
struct FakeState {
    image: Vec<u8>,
    trampolines: Vec<Vec<u8>>,
    freed: usize,
    next_trampoline: usize,
    fail_write: bool,
}

/// A module image plus an "allocator" for trampolines.
pub(super) struct FakeMemory {
    image_base: usize,
    state: Rc<RefCell<FakeState>>,
}

impl FakeMemory {
    pub(super) fn new(image_base: usize, image: &[u8]) -> Self {
        Self {
            image_base,
            state: Rc::new(RefCell::new(FakeState {
                image: image.to_vec(),
                next_trampoline: 0x5000_0000,
                ..FakeState::default()
            })),
        }
    }

    pub(super) fn entry_bytes(&self, len: usize) -> Vec<u8> {
        self.state.borrow().image[..len].to_vec()
    }

    pub(super) fn trampoline_bytes(&self, trampoline: &FakeTrampoline) -> Vec<u8> {
        let state = self.state.borrow();
        state.trampolines[trampoline.index].clone()
    }

    pub(super) fn live_allocations(&self) -> usize {
        let state = self.state.borrow();
        state.trampolines.len() - state.freed
    }

    pub(super) fn freed_allocations(&self) -> usize {
        self.state.borrow().freed
    }

    /// Refuse the next write, the way a rejected `VirtualProtect` would.
    pub(super) fn fail_next_write(&self) {
        self.state.borrow_mut().fail_write = true;
    }
}

/// A trampoline the fake handed out; dropping it releases the allocation.
#[derive(Debug)]
pub(super) struct FakeTrampoline {
    entry: usize,
    index: usize,
    state: Rc<RefCell<FakeState>>,
}

impl TrampolineAllocation for FakeTrampoline {
    fn entry(&self) -> usize {
        self.entry
    }
}

impl Drop for FakeTrampoline {
    fn drop(&mut self) {
        self.state.borrow_mut().freed += 1;
    }
}

impl CodeMemory for FakeMemory {
    type Trampoline = FakeTrampoline;

    /// # Safety
    /// The fake never dereferences `address`; it indexes its own image buffer, so
    /// the caller's address-validity premise is unused here.
    unsafe fn read_code(&self, address: usize, len: usize) -> Result<Vec<u8>, WriteError> {
        let state = self.state.borrow();
        let offset = address
            .checked_sub(self.image_base)
            .ok_or(WriteError::RegionUnknown { address })?;
        let available = state.image.len().saturating_sub(offset);
        Ok(state.image[offset..offset + len.min(available)].to_vec())
    }

    fn install_trampoline(&mut self, bytes: &[u8]) -> Result<Self::Trampoline, WriteError> {
        let mut state = self.state.borrow_mut();
        let entry = state.next_trampoline;
        state.next_trampoline += 0x1000;
        let index = state.trampolines.len();
        state.trampolines.push(bytes.to_vec());
        Ok(FakeTrampoline {
            entry,
            index,
            state: Rc::clone(&self.state),
        })
    }

    /// # Safety
    /// The fake never dereferences `address`; it writes into its own image
    /// buffer, so the caller's no-concurrent-execution premise is unused here.
    unsafe fn write_code(&self, address: usize, bytes: &[u8]) -> Result<(), WriteError> {
        let mut state = self.state.borrow_mut();
        if state.fail_write {
            state.fail_write = false;
            return Err(WriteError::Patch(PatchError::Protect(
                ProtectError::ChangeFailed {
                    address,
                    end: address.saturating_add(bytes.len()),
                    os_error: 5,
                },
            )));
        }
        let offset = address
            .checked_sub(self.image_base)
            .ok_or(WriteError::RegionUnknown { address })?;
        let end = offset
            .checked_add(bytes.len())
            .ok_or(WriteError::RegionUnknown { address })?;
        let target = state
            .image
            .get_mut(offset..end)
            .ok_or(WriteError::RegionUnknown { address })?;
        target.copy_from_slice(bytes);
        Ok(())
    }
}

