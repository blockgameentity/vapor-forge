//! The kernel32 entry points this crate uses on Windows.
//!
//! They are declared here instead of pulling in a Windows binding crate: this is
//! the whole Windows surface of the engine, and one file keeps it auditable.
//! [`crate::protect::windows`], [`crate::exec`] and the writer's Win32 memory
//! backend all call through these wrappers, so no module declares its own copy.

use std::ffi::c_void;

/// `MEM_COMMIT`.
const MEM_COMMIT: u32 = 0x1000;
/// `MEM_RESERVE`.
const MEM_RESERVE: u32 = 0x2000;
/// `MEM_RELEASE`.
const MEM_RELEASE: u32 = 0x8000;
/// `PAGE_NOACCESS`.
const PAGE_NOACCESS: u32 = 0x01;
/// `PAGE_GUARD`.
const PAGE_GUARD: u32 = 0x100;
/// `PAGE_EXECUTE_READWRITE`.
pub(crate) const PAGE_EXECUTE_READWRITE: u32 = 0x40;

/// `MEMORY_BASIC_INFORMATION`, as `VirtualQuery` fills it on x86-64.
///
/// `PartitionId` is a `WORD` followed by two bytes of padding before the
/// pointer-sized `RegionSize`, which is why the layout is spelled out instead of
/// derived from field order alone.
#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    allocation_base: *mut c_void,
    allocation_protect: u32,
    partition_id: u16,
    _padding: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    ty: u32,
}

// SAFETY: every declaration matches the Win32 signature in `memoryapi.h`/
// `processthreadsapi.h`: `BOOL VirtualProtect(LPVOID, SIZE_T, DWORD, PDWORD)`,
// `BOOL FlushInstructionCache(HANDLE, LPCVOID, SIZE_T)`,
// `HANDLE GetCurrentProcess(VOID)`, `LPVOID VirtualAlloc(LPVOID, SIZE_T, DWORD,
// DWORD)`, `BOOL VirtualFree(LPVOID, SIZE_T, DWORD)` and
// `SIZE_T VirtualQuery(LPCVOID, PMEMORY_BASIC_INFORMATION, SIZE_T)`. `BOOL` is a
// 32-bit `i32`, `HANDLE`/`LPVOID` are pointer-sized opaque values, and `SIZE_T`
// is `usize`, so those Rust types model them exactly.
unsafe extern "system" {
    fn VirtualProtect(
        address: *mut c_void,
        size: usize,
        new_protection: u32,
        previous: *mut u32,
    ) -> i32;
    fn FlushInstructionCache(process: *mut c_void, address: *const c_void, size: usize) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn VirtualAlloc(address: *mut c_void, size: usize, kind: u32, protect: u32) -> *mut c_void;
    fn VirtualFree(address: *mut c_void, size: usize, free_type: u32) -> i32;
    fn VirtualQuery(
        address: *const c_void,
        buffer: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
}

/// Ask for `protection` on `address..address+len`, returning what it had before.
///
/// `None` means the call failed; the caller reads the OS error itself.
pub(crate) fn virtual_protect(address: usize, len: usize, protection: u32) -> Option<u32> {
    let mut previous = 0u32;
    // SAFETY: the caller passes a committed range it owns or has validated, and
    // `previous` is a live out-parameter for the duration of the call.
    let ok = unsafe { VirtualProtect(address as *mut c_void, len, protection, &mut previous) };
    (ok != 0).then_some(previous)
}

/// `FlushInstructionCache` for the current process.
pub(crate) fn flush_instruction_cache(address: usize, len: usize) -> bool {
    // SAFETY: the pseudo-handle needs no cleanup and the range is code the caller
    // has just written.
    unsafe { FlushInstructionCache(GetCurrentProcess(), address as *const c_void, len) != 0 }
}

/// Allocate `len` bytes of read-write-execute memory, or `None`.
pub(crate) fn allocate_executable(len: usize) -> Option<usize> {
    // SAFETY: plain `VirtualAlloc` with no caller pointer involvement; the result
    // is checked for null before use.
    let address = unsafe {
        VirtualAlloc(
            std::ptr::null_mut(),
            len,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        )
    };
    (!address.is_null()).then_some(address as usize)
}

/// Release memory returned by [`allocate_executable`].
pub(crate) fn free_executable(address: usize) -> bool {
    // SAFETY: paired one-to-one with `allocate_executable`; `MEM_RELEASE` requires
    // a zero size and a base address, both of which the caller holds.
    unsafe { VirtualFree(address as *mut c_void, 0, MEM_RELEASE) != 0 }
}

/// How many bytes starting at `address` are committed and readable, up to `max`.
///
/// A function entry can sit near the end of its section, so a reader that trusts
/// `max` alone can walk into an unmapped page. `VirtualQuery` reports where the
/// current region ends; `None` means the region could not be described, which is
/// a reason to read nothing at all.
pub(crate) fn readable_len(address: usize, max: usize) -> Option<usize> {
    let mut info = MemoryBasicInformation {
        base_address: std::ptr::null_mut(),
        allocation_base: std::ptr::null_mut(),
        allocation_protect: 0,
        partition_id: 0,
        _padding: 0,
        region_size: 0,
        state: 0,
        protect: 0,
        ty: 0,
    };
    // SAFETY: `info` is a correctly sized and aligned buffer for the call, and it
    // outlives it.
    let written = unsafe {
        VirtualQuery(
            address as *const c_void,
            &mut info,
            std::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    if written == 0 {
        return None;
    }
    let base = info.base_address as usize;
    let end = base.checked_add(info.region_size)?;
    let committed = info.state & MEM_COMMIT != 0;
    let inaccessible = info.protect == 0
        || info.protect & PAGE_NOACCESS != 0
        || info.protect & PAGE_GUARD != 0;
    if !committed || inaccessible || address < base || address >= end {
        return None;
    }
    Some(max.min(end - address))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_basic_information_matches_the_win32_layout() {
        // 8 + 8 + 4 + 2 + 2 + 8 + 4 + 4 + 4 = 44, rounded to 8-byte alignment.
        assert_eq!(std::mem::size_of::<MemoryBasicInformation>(), 48);
        assert_eq!(
            std::mem::align_of::<MemoryBasicInformation>(),
            std::mem::align_of::<usize>()
        );
    }

    #[test]
    fn readable_len_of_an_unmapped_address_is_unknown() {
        // A null address can never be part of a committed region.
        assert_eq!(readable_len(0, 32), None);
    }
}
