//! The only place in this crate that touches Win32 directly.
//!
//! Everything here is deliberately small: resolve a module base, read its
//! bytes, and (later) make a code range writable for the detour engine. The
//! detour engine itself lives in `hook-engine`; this module never installs a
//! hook, so the FFI surface stays auditable.
//!
//! On non-Windows hosts every entry point returns [`HostError::Unsupported`], so
//! the crate compiles and tests anywhere.

use crate::HostError;

/// Page protection to restore after patching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Protection(pub u32);

/// Base address of a module loaded in the current process.
pub fn module_base(name: &str) -> Result<usize, HostError> {
    imp::module_base(name)
}

/// Bytes of a loaded module's executable section start (first `len` bytes).
pub fn read_module_head(name: &str, len: usize) -> Result<Vec<u8>, HostError> {
    imp::read_module_head(name, len)
}

/// Copy `len` bytes out of this process at `address`.
pub fn read_bytes(address: usize, len: usize) -> Result<Vec<u8>, HostError> {
    imp::read_bytes(address, len)
}

/// Allocate executable memory for a trampoline. Returns the address.
pub fn allocate_executable(len: usize) -> Result<usize, HostError> {
    imp::allocate_executable(len)
}

/// Release memory returned by [`allocate_executable`].
pub fn free_executable(address: usize) -> Result<(), HostError> {
    imp::free_executable(address)
}

/// Mark `len` bytes at `address` writable, returning the previous protection.
///
/// Callers must restore it: leaving `.text` writable weakens the process.
pub fn make_writable(address: usize, len: usize) -> Result<Protection, HostError> {
    imp::make_writable(address, len)
}

/// Restore a protection returned by [`make_writable`].
pub fn restore(address: usize, len: usize, protection: Protection) -> Result<(), HostError> {
    imp::restore(address, len, protection)
}

/// Flush the CPU instruction cache after patching code.
pub fn flush_instruction_cache(address: usize, len: usize) -> Result<(), HostError> {
    imp::flush_instruction_cache(address, len)
}

#[cfg(windows)]
mod imp {
    use super::{HostError, Protection};
    use std::os::windows::ffi::OsStrExt;

    const PAGE_EXECUTE_READWRITE: u32 = 0x40;

    #[repr(C)]
    struct MemoryBasicInformation {
        base_address: *mut core::ffi::c_void,
        allocation_base: *mut core::ffi::c_void,
        allocation_protect: u32,
        partition_id: u16,
        _padding: u16,
        region_size: usize,
        state: u32,
        protect: u32,
        ty: u32,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut core::ffi::c_void;
        fn VirtualAlloc(
            address: *mut core::ffi::c_void,
            size: usize,
            allocation_type: u32,
            protect: u32,
        ) -> *mut core::ffi::c_void;
        fn VirtualFree(address: *mut core::ffi::c_void, size: usize, free_type: u32) -> i32;
        fn GetCurrentProcess() -> *mut core::ffi::c_void;
        fn FlushInstructionCache(
            process: *mut core::ffi::c_void,
            address: *const core::ffi::c_void,
            len: usize,
        ) -> i32;
        fn VirtualProtect(
            address: *mut core::ffi::c_void,
            size: usize,
            new_protect: u32,
            old_protect: *mut u32,
        ) -> i32;
    }

    fn wide(name: &str) -> Vec<u16> {
        std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn module_base(name: &str) -> Result<usize, HostError> {
        let wide = wide(name);
        // SAFETY: the buffer is NUL-terminated and outlives the call.
        let handle = unsafe { GetModuleHandleW(wide.as_ptr()) };
        if handle.is_null() {
            return Err(HostError::Unsupported(format!(
                "{name} is not loaded in this process"
            )));
        }
        Ok(handle as usize)
    }

    pub fn read_module_head(name: &str, len: usize) -> Result<Vec<u8>, HostError> {
        let base = module_base(name)?;
        // SAFETY: the module is mapped; we read a caller-bounded prefix of it.
        let bytes = unsafe { std::slice::from_raw_parts(base as *const u8, len) };
        Ok(bytes.to_vec())
    }

    pub fn read_bytes(address: usize, len: usize) -> Result<Vec<u8>, HostError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: the caller passes a range inside a loaded module.
        let slice = unsafe { std::slice::from_raw_parts(address as *const u8, len) };
        Ok(slice.to_vec())
    }

    pub fn allocate_executable(len: usize) -> Result<usize, HostError> {
        const MEM_COMMIT_RESERVE: u32 = 0x3000;
        // SAFETY: plain VirtualAlloc with no caller pointer involvement.
        let address = unsafe {
            VirtualAlloc(
                std::ptr::null_mut(),
                len,
                MEM_COMMIT_RESERVE,
                PAGE_EXECUTE_READWRITE,
            )
        };
        if address.is_null() {
            return Err(HostError::Unsupported(format!(
                "VirtualAlloc failed for {len} bytes"
            )));
        }
        Ok(address as usize)
    }

    pub fn free_executable(address: usize) -> Result<(), HostError> {
        const MEM_RELEASE: u32 = 0x8000;
        // SAFETY: paired with allocate_executable; size must be 0 for release.
        let ok = unsafe { VirtualFree(address as *mut core::ffi::c_void, 0, MEM_RELEASE) };
        if ok == 0 {
            return Err(HostError::Unsupported(format!(
                "VirtualFree failed for {address:#x}"
            )));
        }
        Ok(())
    }

    pub fn make_writable(address: usize, len: usize) -> Result<Protection, HostError> {
        let mut old = 0u32;
        // SAFETY: caller passes a mapped range inside a loaded module.
        let ok = unsafe {
            VirtualProtect(
                address as *mut core::ffi::c_void,
                len,
                PAGE_EXECUTE_READWRITE,
                &mut old,
            )
        };
        if ok == 0 {
            return Err(HostError::Unsupported(format!(
                "VirtualProtect failed for {address:#x}+{len}"
            )));
        }
        Ok(Protection(old))
    }

    pub fn restore(address: usize, len: usize, protection: Protection) -> Result<(), HostError> {
        let mut ignored = 0u32;
        // SAFETY: mirrors make_writable on the same range.
        let ok = unsafe {
            VirtualProtect(
                address as *mut core::ffi::c_void,
                len,
                protection.0,
                &mut ignored,
            )
        };
        if ok == 0 {
            return Err(HostError::Unsupported(format!(
                "VirtualProtect restore failed for {address:#x}+{len}"
            )));
        }
        Ok(())
    }

    pub fn flush_instruction_cache(address: usize, len: usize) -> Result<(), HostError> {
        // SAFETY: the current process pseudo-handle needs no cleanup and the
        // range was just patched by the caller.
        let ok = unsafe {
            FlushInstructionCache(
                GetCurrentProcess(),
                address as *const core::ffi::c_void,
                len,
            )
        };
        if ok == 0 {
            return Err(HostError::Unsupported(format!(
                "FlushInstructionCache failed for {address:#x}+{len}"
            )));
        }
        Ok(())
    }

    #[allow(dead_code)]
    fn _assert_layout() {
        let _ = core::mem::size_of::<MemoryBasicInformation>();
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{HostError, Protection};

    pub fn module_base(_name: &str) -> Result<usize, HostError> {
        Err(HostError::Unsupported(
            "the Windows host can only run inside the Windows Steam client".to_owned(),
        ))
    }

    fn unsupported<T>() -> Result<T, HostError> {
        Err(HostError::Unsupported(
            "the Windows host can only run inside the Windows Steam client".to_owned(),
        ))
    }

    pub fn read_module_head(_name: &str, _len: usize) -> Result<Vec<u8>, HostError> {
        unsupported()
    }

    pub fn read_bytes(_address: usize, _len: usize) -> Result<Vec<u8>, HostError> {
        unsupported()
    }

    pub fn allocate_executable(_len: usize) -> Result<usize, HostError> {
        unsupported()
    }

    pub fn free_executable(_address: usize) -> Result<(), HostError> {
        unsupported()
    }

    pub fn make_writable(_address: usize, _len: usize) -> Result<Protection, HostError> {
        unsupported()
    }

    pub fn restore(_address: usize, _len: usize, _protection: Protection) -> Result<(), HostError> {
        unsupported()
    }

    pub fn flush_instruction_cache(_address: usize, _len: usize) -> Result<(), HostError> {
        unsupported()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_windows_hosts_report_unsupported_instead_of_panicking() {
        if cfg!(windows) {
            return;
        }
        assert!(matches!(
            module_base("steamclient64.dll"),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            make_writable(0x1000, 16),
            Err(HostError::Unsupported(_))
        ));
        assert!(matches!(
            flush_instruction_cache(0x1000, 12),
            Err(HostError::Unsupported(_))
        ));
    }
}
