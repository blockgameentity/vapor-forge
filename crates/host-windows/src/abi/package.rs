//! `CPackageInfo::GetPackageInfo` — the call shape the package hook stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds:
//!
//! ```text
//! 48 89 5C 24 18  mov [rsp+0x18], rbx   ; callee-saved spill, not an argument
//! 89 54 24 10     mov [rsp+0x10], edx   ; rdx's home slot, 32-bit store
//! 44 8B 49 20     mov r9d, [rcx+0x20]   ; r9 written before it is ever read
//! ```
//!
//! So argument 1 (`rdx`) is 32 bits wide, and `r9` is *not* an input argument:
//! Steam overwrites it with a field of `this` before reading it. That is the
//! only measured statement about the register slots; `r8` is untouched by the
//! pattern and stays unknown.
//!
//! # Assumed
//!
//! * argument 1 is a package id — from the Linux host's measured signature
//!   (`GetPackageInfoFn`), which the symbol name agrees with;
//! * the return value is a pointer to Steam's package-info structure — likewise;
//! * argument 2 (`r8`) is unknown and forwarded unchanged. The body never passes
//!   a fourth register slot because the prologue proves there is no `r9` input.

use core::ffi::c_void;

/// `GetPackageInfo(this, package_id, r8) -> *mut u8`.
///
/// The pointer return is not dereferenced anywhere in this host: no field of
/// the returned structure is measured on a Windows build.
pub type GetPackageInfoFn =
    unsafe extern "system" fn(this: *mut c_void, package_id: u32, r8: usize) -> *mut u8;
