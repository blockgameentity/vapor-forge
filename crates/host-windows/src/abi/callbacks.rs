//! `CSteamEngine::SetAPICallResult` — the call shape the callback hook stands in
//! for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (public RVA `0x9bf3d0`, publicbeta `0x9e50c0`):
//!
//! ```text
//! 40 53 55 56 57        push rbx / rbp / rsi / rdi
//! 48 83 EC 58           sub rsp, 0x58
//! 33 C0                 xor eax, eax
//! 48 89 54 24 20        mov [rsp+0x20], rdx   ; rdx spilled as 64 bits
//! 49 8B F0              mov rsi, r8           ; r8 read as 64 bits
//! 48                    (first byte of the next store)
//! ```
//!
//! So `rdx` and `r8` are 64 bits wide. One step past the anchor, in the same
//! image (`0x9bf3e8`-`0x9bf40b`): `48 8B D9` (`mov rbx, rcx`), then
//! `48 81 C1 A0 0C 00 00` (`add rcx, 0xCA0`, the result map the mining verdict
//! names), `41 8B F9` (`mov edi, r9d`) and `41 B8 01 00 00 00` (`mov r8d, 1`,
//! which overwrites `r8` for the map call — the incoming `r8` was already
//! consumed by the `mov rsi, r8` at entry). So `rcx` is 64 bits and `r9` is 32.
//!
//! The anchor proves four argument slots and no count. Everything past the
//! fourth slot is on the stack and invisible to a prefix: the callback id and the
//! payload size this host's decision needs are *not* measured here.
//!
//! # Assumed
//!
//! * the first four slots are `(engine, client_context, api_call, target_pipe)`
//!   — the Linux host's measured x86_64 `SetApiCallResultFn`, whose widths agree
//!   with the four measured here;
//! * the three stack slots are `(payload, payload_size, callback)`, in that
//!   order, again from the Linux signature. They are declared and forwarded
//!   verbatim: if the order is wrong the body hands Steam's own values back
//!   unchanged and only its *reading* of the id is wrong, which the decision
//!   answers with "not ours";
//! * the return value is unmeasured. The type is a `usize` so whatever the
//!   original leaves in `rax` — nothing for a `void` function, a pointer or an
//!   integer otherwise — is returned bit for bit.

use core::ffi::c_void;

/// `SetAPICallResult(engine, client_context, api_call, target_pipe, payload,
/// payload_size, callback)`.
///
/// No slot is ever written and `payload` is never dereferenced: this body only
/// reads the id and the size, and every other slot is forwarded as it arrived.
pub type SetApiCallResultFn = unsafe extern "system" fn(
    engine: *mut c_void,
    client_context: *mut c_void,
    api_call: u64,
    target_pipe: u32,
    payload: *mut c_void,
    payload_size: i32,
    callback: i32,
) -> usize;
