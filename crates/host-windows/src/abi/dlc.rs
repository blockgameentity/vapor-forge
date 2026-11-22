//! The two DLC predicates — `IClientAppManager::IsAppDlcInstalled` and
//! `IClientAppManager::BIsDlcEnabled` — as one call-shape note, because the
//! hooks are two entry points of one object.
//!
//! # Verified from the corpus
//!
//! `IClientAppManager::IsAppDlcInstalled` (public RVA `0x4f6a40`, publicbeta
//! `0x50c740`, `CUserAppManager` slot 9):
//!
//! ```text
//! 48 8B C4              mov rax, rsp
//! 44 89 40 18           mov [rax+0x18], r8d   ; r8's home slot, 32-bit store
//! 48 89 48 08           mov [rax+8], rcx      ; rcx's home slot, 64-bit store
//! 55 56                 push rbp / push rsi
//! 48 8D 68 A1           lea rbp, [rax-0x5F]
//! 48 81 EC 88 00 00 00  sub rsp, 0x88
//! 48                    (first byte of the next store)
//! ```
//!
//! So `rcx` is 64 bits and `r8` is 32. One step past the anchor, `44 8B FA`
//! (`mov r15d, edx`, public RVA `0x4f6a73`) reads `rdx` as 32 bits as well.
//!
//! `IClientAppManager::BIsDlcEnabled` (public RVA `0x4eb080`, publicbeta
//! `0x500ce0`, `CUserAppManager` slot 11) — the anchor is almost all frame
//! setup, so it proves one thing:
//!
//! ```text
//! 40 55 53 56 57 41 54 41 56 41 57  push rbp/rbx/rsi/rdi/r12/r14/r15
//! 48 8B EC                          mov rbp, rsp
//! 48 83 EC 50                       sub rsp, 0x50
//! 45 33 E4                          xor r12d, r12d
//! 49 8B F1                          mov rsi, r9    ; r9 read as 64 bits
//! 41                                (first byte of the next instruction)
//! ```
//!
//! One step past the anchor, in the same image (`0x4eb098`-`0x4eb0a6`):
//! `41 8B F8` (`mov edi, r8d`), `44 8B F2` (`mov r14d, edx`), `4C 8B F9`
//! (`mov r15, rcx`), then `4D 85 C9 / 74 51` (`test r9, r9 ; je`) and
//! `45 88 21` (`mov [r9], r12b`). So all four slots are inputs, `r8`/`rdx` are
//! 32 bits, and `r9` is a pointer the function itself writes a zero byte
//! through. Nothing else is measured: the anchor proves no argument count.
//!
//! # Assumed
//!
//! * the two 32-bit slots are `(app_id, dlc_id)` in that order — from the Linux
//!   host's measured `IsAppDlcInstalledFn`/`BIsDlcEnabledFn` and from the symbol
//!   names. `BIsDlcEnabled`'s own body compares them (`cmp r8d, edx`), which is
//!   consistent with a pair but does not prove which is which;
//! * argument 0 (`rcx`) is the `IClientAppManager` receiver — the symbol is a
//!   vtable slot, and both functions carry `rcx` unchanged into a callee-saved
//!   register (`mov rbx, rcx` / `mov r15, rcx`) before any other use;
//! * both return `bool` in `al`. The only in-image evidence is that
//!   `BIsDlcEnabled` computes `sete al` and stores that byte into `*r9` before
//!   returning;
//! * `BIsDlcEnabled`'s `r9` is the out flag of the same answer as the return
//!   value. The body only writes it when it overrides, and only a `0`/`1` byte,
//!   which is exactly what the original writes there.

use core::ffi::c_void;

/// `IsAppDlcInstalled(this, app_id, dlc_id, r9) -> u8`.
///
/// `r9` is not read by the anchor, so it is forwarded and never read here.
pub type IsAppDlcInstalledFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32, dlc_id: u32, r9: usize) -> u8;

/// `BIsDlcEnabled(this, app_id, dlc_id, out_flag) -> u8`.
///
/// `out_flag` is measured: the original tests it for null and writes a byte
/// through it.
pub type BIsDlcEnabledFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32, dlc_id: u32, out_flag: *mut u8) -> u8;
