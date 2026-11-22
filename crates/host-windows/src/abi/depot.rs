//! `GetDecryptionKey` — the call shape the depot hook stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds:
//!
//! ```text
//! 40 53 55 56 57          push rbx / rbp / rsi / rdi
//! 48 81 EC 48 01 00 00    sub rsp, 0x148
//! 8B FA                   mov edi, edx     ; rdx read as 32 bits
//! ```
//!
//! So argument 1 (`rdx`) is 32 bits wide. Nothing else is measured: neither the
//! first argument's width nor what `r8`/`r9` carry.
//!
//! # Assumed
//!
//! * argument 0 (`rcx`) is the object pointer and argument 1 is a depot id —
//!   from the symbol name and from the Linux host's
//!   `LoadDepotDecryptionKey(_, depot_id: u32, out)` signature. The Windows
//!   corpus has no second anchor that would confirm the mapping;
//! * the function returns `bool` (in `al`) — likewise unverified here.

use core::ffi::c_void;

/// `GetDecryptionKey(this, depot_id, r8, r9) -> u8`.
///
/// `r8`/`r9` are forwarded and never read, because the pattern proves nothing
/// about them and dropping a slot would drop an argument.
pub type GetDecryptionKeyFn =
    unsafe extern "system" fn(this: *mut c_void, depot_id: u32, r8: usize, r9: usize) -> u8;
