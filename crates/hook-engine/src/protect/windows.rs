//! `VirtualProtect`/`FlushInstructionCache` page-protection backend.
//!
//! The kernel32 entry points this crate needs are declared here rather than
//! pulling in a Windows binding crate: they are the whole Windows surface of
//! the engine.

use std::ffi::c_void;

use super::{last_os_error, ProtectError, Protection};

/// `PAGE_READONLY`.
pub(super) const READ: u32 = 0x02;
/// `PAGE_READWRITE`.
pub(super) const READ_WRITE: u32 = 0x04;
/// `PAGE_EXECUTE_READ`.
pub(super) const READ_EXECUTE: u32 = 0x20;
/// `PAGE_EXECUTE_READWRITE`.
pub(super) const READ_WRITE_EXECUTE: u32 = 0x40;

// SAFETY: every declaration below matches the Win32 signature in
// `memoryapi.h`/`processthreadsapi.h`: `BOOL VirtualProtect(LPVOID, SIZE_T,
// DWORD, PDWORD)`, `BOOL FlushInstructionCache(HANDLE, LPCVOID, SIZE_T)`, and
// `HANDLE GetCurrentProcess(VOID)`. `BOOL` is a 32-bit `i32` and `HANDLE` is a
// pointer-sized opaque value, which `i32`/`*mut c_void` model exactly.
unsafe extern "system" {
    fn VirtualProtect(
        address: *mut c_void,
        size: usize,
        new_protection: u32,
        previous: *mut u32,
    ) -> i32;
    fn FlushInstructionCache(process: *mut c_void, address: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
}

/// Ask for `PAGE_EXECUTE_READWRITE` and let the OS report what the range had.
///
/// `VirtualProtect` hands back the previous protection, including modifier bits
/// (`PAGE_GUARD`, `PAGE_NOCACHE`), so restoring it is exact. The range may be a
/// plain data page (a vtable) — executable access is granted as well because
/// the same writable window is used for code pages, and Windows has no
/// "read-write, keep whatever execute bit" spelling.
pub(super) fn make_writable(address: usize, len: usize) -> Result<Protection, ProtectError> {
    let mut previous = 0u32;
    // SAFETY: `address..address+len` is the caller's validated target range and
    // is committed; VirtualProtect only widens its protection.
    let ok = unsafe {
        VirtualProtect(
            address as *mut c_void,
            len,
            READ_WRITE_EXECUTE,
            &mut previous,
        )
    };
    if ok == 0 {
        return Err(ProtectError::ChangeFailed {
            address,
            end: address.saturating_add(len),
            os_error: last_os_error(),
        });
    }
    Ok(Protection(previous))
}

pub(super) fn set_protection(
    address: usize,
    len: usize,
    protection: Protection,
) -> Result<(), ProtectError> {
    let mut previous = 0u32;
    // SAFETY: same contract as `make_writable`; the requested value is either a
    // `Protection` this module produced or a named constant from it.
    let ok =
        unsafe { VirtualProtect(address as *mut c_void, len, protection.raw(), &mut previous) };
    if ok == 0 {
        return Err(ProtectError::ChangeFailed {
            address,
            end: address.saturating_add(len),
            os_error: last_os_error(),
        });
    }
    Ok(())
}

pub(super) fn flush_instruction_cache(address: usize, len: usize) {
    // SAFETY: the pseudo-handle needs no cleanup and `address..address+len` is
    // the code range the caller has just written.
    let ok = unsafe { FlushInstructionCache(GetCurrentProcess(), address as *const c_void, len) };
    if ok == 0 {
        tracing::warn!(
            address = format_args!("{address:#x}"),
            len,
            os_error = last_os_error(),
            "FlushInstructionCache failed; patched code may not be visible to other cores"
        );
    }
}
