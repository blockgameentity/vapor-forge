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

    pub fn make_writable(_address: usize, _len: usize) -> Result<Protection, HostError> {
        unsupported()
    }

    pub fn restore(_address: usize, _len: usize, _protection: Protection) -> Result<(), HostError> {
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
    }
}
