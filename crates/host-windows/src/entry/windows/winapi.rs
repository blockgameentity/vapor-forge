//! The two `kernel32` calls that resolve a module's path, and the buffers they
//! write into.

use std::path::PathBuf;

use crate::inject;

/// Full path of the module mapped at `module`.
pub fn module_path(module: *mut core::ffi::c_void) -> Option<PathBuf> {
    let mut buffer = vec![0u16; 32_768];
    // SAFETY: `module` is a handle the caller got from the loader (or from
    // `GetModuleHandleExW`), and the buffer is a live, writable slice of `u16`
    // whose length is passed exactly.
    let len = unsafe {
        imp::GetModuleFileNameW(
            module,
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).ok()?,
        )
    };
    if len == 0 || len as usize >= buffer.len() {
        return None;
    }
    buffer.truncate(len as usize);
    Some(PathBuf::from(String::from_utf16_lossy(&buffer)))
}

/// Path of the DLL this code is in, resolved from a function address.
pub fn own_module_path() -> Option<PathBuf> {
    // Any address inside this DLL will do; a zero-sized function in this module
    // is as good as any and cannot be confused with another image's address.
    fn marker() {}

    let mut module: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `FROM_ADDRESS` takes an address inside this DLL, and `module` is a
    // live out-parameter. `UNCHANGED_REFCOUNT` means the loader does not add a
    // reference we would have to release.
    let ok = unsafe {
        imp::GetModuleHandleExW(
            imp::GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS
                | imp::GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            marker as *const () as *const u16,
            &mut module,
        )
    };
    if ok == 0 {
        return None;
    }
    module_path(module)
}

pub(super) mod imp {
    use core::ffi::c_void;

    pub const GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT: u32 = 0x0000_0002;
    pub const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetModuleHandleExW(
            flags: u32,
            module_name: *const u16,
            module: *mut *mut c_void,
        ) -> i32;
        pub fn GetModuleFileNameW(module: *mut c_void, filename: *mut u16, size: u32) -> u32;
    }
}

/// Full path of the named module, when it is loaded in this process.
pub fn module_path_by_name(name: &str) -> Option<PathBuf> {
    let base = inject::module_base(name).ok()?;
    module_path(base as *mut core::ffi::c_void)
}
