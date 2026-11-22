//! `CUser::GetFamilySharedLockAccount` — the call shape the family-lock hook
//! stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (public RVA `0xa07460`, publicbeta `0xa2f7f0`;
//! wildcard-free, unique, resolved by `vapor-forge-pattern-mine --verify` in
//! each image):
//!
//! ```text
//! 48 89 5C 24 08        mov  [rsp+0x8], rbx
//! 48 89 7C 24 10        mov  [rsp+0x10], rdi
//! 44 8B 89 30 6A 00 00  mov  r9d, [rcx+0x6a30]   ; rcx is a pointer, read at 32 bits
//! 44 8B DA              mov  r11d, edx           ; rdx is a 32-bit argument
//! 48 8B F9              mov  rdi, rcx
//! 41 83 F9 FF           cmp  r9d, -1
//! 74 54                 je   ...
//! 48 8B 99 48 6A 00 00  mov  rbx, [rcx+0x6a48]   ; rcx read as a 64-bit pointer
//! ```
//!
//! Four facts, all measured: `rcx` is a 64-bit pointer (`r9`/`rbx` are loaded
//! from it through a `rex.W` opcode), `rdx` is a 32-bit integer argument, and
//! neither `r8` nor `r9` is read as an argument (they are never spilled, only
//! used as scratch — `r9d` is overwritten by the first load). The return value
//! is `eax`: the not-found path is `xor eax, eax`, the found path is
//! `mov eax, [r10+rdx*8]`, and the "that is us" filter is
//! `cmp eax, [rdi+0x20a]` followed by `cmove eax, ecx`.
//!
//! # Assumed
//!
//! * argument 1 (`rcx`) is the `CUser` receiver, and the two member
//!   displacements are the family shared-lock tree the anchor reads. The
//!   disassembly of the body is what says so, not the name: the same function
//!   compares its result against the account id at `[rcx+0x20a]`, which is the
//!   `CSteamID` member `CUser::GetSteamID` returns.
//! * argument 2 (`rdx`) is an app id. Both call sites load it from a 32-bit slot
//!   that is also the app id they pass to the ownership-info call.
//! * the return value is the account id of the member holding the app, `0`
//!   meaning "nobody, or us". The corpus measures the register, not the meaning;
//!   `res/patterns/win64/VTABLE-REPORT.md`, eighth pass, derives the meaning from
//!   the two call sites.
//!
//! The function writes no argument through a pointer, so this body never
//! writes through one either: the only state it can change is its return value.

use core::ffi::c_void;

/// `GetFamilySharedLockAccount(this, app_id) -> u32`.
pub type GetFamilySharedLockAccountFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32) -> u32;
