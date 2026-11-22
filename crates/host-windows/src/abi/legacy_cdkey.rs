//! `IClientUser::RequiresLegacyCDKey` — the call shape the legacy-CD-key hook
//! stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (public RVA `0xa16f70`, publicbeta `0xa3fcf0`,
//! `CUser` slot 87):
//!
//! ```text
//! 40 53 56 57 41 56     push rbx / rsi / rdi / r14
//! 48 81 EC 58 02 00 00  sub rsp, 0x258
//! 41 C6 00 00           mov byte [r8], 0    ; writes a zero through r8
//! 48 8B F1              mov rsi, rcx        ; rcx read as 64 bits
//! 48                    (first byte of the next load)
//! ```
//!
//! Three facts, all measured: `r8` is a *pointer to one byte* (the function
//! writes a `0` through it before any other work), `rcx` is 64 bits, and the
//! out flag is cleared before the lock is even taken. One step past the anchor
//! (`0xa16f8b`-`0xa16f90`): `4D 8B F0` (`mov r14, r8`) and `8B FA`
//! (`mov edi, edx`), so `rdx` is a 32-bit argument.
//!
//! That the anchor itself clears the flag is what makes the hook's write safe:
//! the same pointer, the same width, the same value space.
//!
//! # Assumed
//!
//! * argument 1 (`rdx`) is an app id — from the Linux host's measured
//!   `RequiresLegacyCdKeyFn(this, app_id: u32, has_key: *mut bool)`;
//! * argument 0 (`rcx`) is the `IClientUser` receiver — the symbol is a vtable
//!   slot;
//! * the return value is `bool` in `al`, the same answer the out flag carries.
//!   The Linux host returns `false` for a suppressed app *and* clears the flag;
//!   this body does both, so a caller reading either one agrees;
//! * `r9` is unmeasured: forwarded and never read.

use core::ffi::c_void;

/// `RequiresLegacyCDKey(this, app_id, out_flag, r9) -> u8`.
///
/// `out_flag` is written only with a `0`, the exact byte the original writes at
/// its own entry.
pub type RequiresLegacyCdKeyFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32, out_flag: *mut u8, r9: usize) -> u8;
