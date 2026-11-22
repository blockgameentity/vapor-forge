//! `CShaderCacheManager::GetShaderCacheDepot` — the call shape the shader hook
//! stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (`669fa08e…` public RVA `0x533ad0`,
//! `4a2d7952…` publicbeta RVA `0x546020`):
//!
//! ```text
//! 48 89 5C 24 08        mov [rsp+8], rbx        ; callee-saved spill, not an argument
//! 57                    push rdi
//! 48 83 EC 30           sub rsp, 0x30
//! 48 8B 05 ?? ?? ?? ??  mov rax, [rip+…]        ; one global
//! 48 8B F9              mov rdi, rcx            ; rcx read as 64 bits
//! 83 78 7C 00           cmp dword [rax+0x7C], 0
//! 0F 84 86 …            je rel32
//! ```
//!
//! Argument 0 (`rcx`) is read as 64 bits and nothing in the anchor touches
//! `rdx`, `r8` or `r9`: they are neither spilled nor copied, which is the
//! signature of a one-argument function.
//!
//! One step past the anchor, in the same function of the same image
//! (`8B 57 0C`, `mov edx, [rdi+0xC]` at public RVA `0x533b01`, and again at
//! `0x533b49`): the value in `rcx` is a pointer, and the field it reads at
//! `+0xC` is the depot id the function then works on. That is the field the hook
//! is placed to answer for — the hook never reads it itself, because the
//! original already returned it.
//!
//! # Assumed
//!
//! * the return value is the depot id as a `u32` — from the Linux host's
//!   `GetShaderCacheDepotFn` and from the mining verdict ("ABI is
//!   `(CAppInfo*) -> u32` depot id"), not from the anchor;
//! * `rdx`, `r8` and `r9` are forwarded verbatim and never read. A one-argument
//!   function ignores them; if the real shape is wider, forwarding is what keeps
//!   the call intact.
//!
//! Anything the function passes on the stack (argument 5 and beyond) is not
//! forwarded: a body that has not measured an argument cannot name it, and no
//! corpus entry suggests this function has five.

use core::ffi::c_void;

/// `GetShaderCacheDepot(this, rdx, r8, r9) -> u32`.
///
/// `rdx`/`r8`/`r9` are opaque: forwarded and never read.
pub type GetShaderCacheDepotFn =
    unsafe extern "system" fn(this: *mut c_void, rdx: usize, r8: usize, r9: usize) -> u32;
