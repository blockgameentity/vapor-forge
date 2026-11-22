//! `CUser::GetSubscribedApps` — the call shape the subscription hook stands in
//! for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (public RVA `0xa0bca0`, publicbeta `0xa34310`,
//! `CUser` slot 183). This is the one shape in the win64 corpus whose four
//! argument slots are all measured by the anchor itself:
//!
//! ```text
//! 48 8B C4              mov rax, rsp
//! 44 88 48 20           mov [rax+0x20], r9b    ; r9's home slot, 8-bit store
//! 44 89 40 18           mov [rax+0x18], r8d    ; r8's home slot, 32-bit store
//! 48 89 50 10           mov [rax+0x10], rdx    ; rdx's home slot, 64-bit store
//! 48 89 48 08           mov [rax+8], rcx       ; rcx's home slot, 64-bit store
//! 55 56 57              push rbp / rsi / rdi
//! 48 8D 68 A1           lea rbp, [rax-0x5F]
//! 48 81 EC A0 00 00 00  sub rsp, 0xA0
//! 48                    (first byte of the next store)
//! ```
//!
//! So argument 0 (`rcx`) and argument 1 (`rdx`) are 64 bits, argument 2 (`r8`)
//! is 32, and argument 3 (`r9`) is a **byte** — the mining verdict reads the
//! same thing as a bool (`movzbl %r9b` at public RVA `0xa0bce4`). That is the
//! Linux host's measured `GetSubscribedApps(app_list, size, a3)` shape exactly,
//! four slots, same widths, in the same order.
//!
//! # Assumed
//!
//! * the return value is the count of app ids written, as a `u32` — from the
//!   Linux host. Nothing in the anchor sets a return register;
//! * argument 1 is a `*mut u32` buffer of argument 2 `u32` slots. Steam calls
//!   this twice — once with a null buffer and size 0 to size its vector, once
//!   with the buffer — and the widths measured here fit that use, but the slot
//!   semantics are the Linux host's, not the anchor's;
//! * argument 3 is forwarded and never read, exactly as the Linux host does;
//! * the count the body returns is honoured by the caller (the Linux host's own
//!   comment says Steam sizes its vector from the first call and ignores the
//!   second answer, which is why every slot past the count is zero-filled).

use core::ffi::c_void;

/// `GetSubscribedApps(this, app_list, size, a3) -> u32`.
///
/// `app_list` is written only when Steam passed a non-null buffer with a
/// non-zero size, and only within the `size` slots it declares.
pub type GetSubscribedAppsFn =
    unsafe extern "system" fn(this: *mut c_void, app_list: *mut u32, size: u32, a3: u8) -> u32;
