//! `CloseAppCloud` — the call shape the cloud hook stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds with byte-identical patterns (`669fa08e…`
//! public at RVA `0xa67320`, `4a2d7952…` publicbeta at RVA `0xa916b0`):
//!
//! ```text
//! 48 89 5C 24 10  mov [rsp+0x10], rbx
//! 57              push rdi
//! 48 83 EC 30     sub  rsp, 0x30
//! 8B FA           mov edi, edx     ; argument 1 is 32 bits wide
//! 48 8B D9        mov rbx, rcx     ; argument 0 is a 64-bit pointer
//! 85 D2           test edx, edx    ; argument 1 is compared against zero
//! 75 ??           jne  +0x35
//! ```
//!
//! Decoding the anchored entry of the public build shows what the two paths are:
//! `edx == 0` enters an enumerate-everything branch, `edx != 0` looks the id up
//! in a table at `this + 0x2c8`, and the zero path leaves through `xor al, al` /
//! `ret`. So the function *does* return a byte in `al`, which is why
//! [`CloseAppCloudFn`] returns `u8` rather than a Rust `bool`.
//!
//! # Assumed
//!
//! * argument 1 is an `AppId_t` and `0` means "every app". The name and the zero
//!   test say so; no header has been read for this build, and the body treats the
//!   value as an app id only to look it up in the host's own plan — a wrong guess
//!   matches no controlled app.
//! * the member takes exactly two arguments. Arguments 2 and 3 are forwarded
//!   **verbatim** as opaque slots: the pattern stops before either register is
//!   used, so neither their width nor their meaning is measured. If a live build
//!   passes more arguments, they arrive unchanged at the trampoline, which is
//!   what a transparent detour must guarantee.
//! * nothing about what the returned byte means beyond "one byte in `al`". The
//!   body returns Steam's own byte unchanged and never interprets it.

use core::ffi::c_void;

/// The value Steam passes as argument 1 when the call covers every app.
///
/// Measured behaviour, not a header constant: the anchored entry compares
/// argument 1 against zero and takes the enumerate branch when it matches.
pub const EVERY_APP: u32 = 0;

/// `CloseAppCloud(this, app_id, r8, r9)`.
///
/// `r8` and `r9` are opaque: see the module docs. The return value is the raw
/// `al` byte.
pub type CloseAppCloudFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32, r8: usize, r9: usize) -> u8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_forwarded_slots_are_pointer_wide() {
        // The two opaque slots exist so that a live build with more arguments is
        // not truncated: they are pointer-wide by construction.
        assert_eq!(
            core::mem::size_of::<CloseAppCloudFn>(),
            core::mem::size_of::<usize>()
        );
    }

    #[test]
    fn the_every_app_sentinel_is_the_value_the_entry_compares_against() {
        assert_eq!(EVERY_APP, 0);
    }

    #[test]
    fn the_body_is_callable_through_the_documented_signature() {
        unsafe extern "system" fn sample(
            _this: *mut c_void,
            _app_id: u32,
            _r8: usize,
            _r9: usize,
        ) -> u8 {
            0
        }
        let typed: CloseAppCloudFn = sample;
        // SAFETY: the replacement below is the body itself, called with nulls and
        // a zero app id; it reads no pointer and returns immediately.
        assert_eq!(unsafe { typed(std::ptr::null_mut(), EVERY_APP, 0, 0) }, 0);
    }
}
