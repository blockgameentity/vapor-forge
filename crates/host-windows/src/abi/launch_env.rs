//! `CUser::BuildSpawnEnvBlock` — the call shape the launch-environment hook
//! stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (public RVA `0xa1c5d0`, publicbeta `0xa45520`).
//! The anchor is the longest of the set and it measures the argument *layout*,
//! not just widths:
//!
//! ```text
//! 48 89 5C 24 18                  mov [rsp+0x18], rbx   ; callee-saved spill
//! 48 89 4C 24 08                  mov [rsp+8], rcx      ; rcx spilled as 64 bits
//! 55 56 57 41 54 41 55 41 56 41 57  push rbp/rsi/rdi/r12/r13/r14/r15
//! 48 8D AC 24 ?? ?? FF FF         lea rbp, [rsp+disp]
//! 48 81 EC ?? ?? 00 00            sub rsp, imm32
//! 8B 42 18                        mov eax, [rdx+0x18]   ; rdx is a pointer, 32-bit field
//! 0F 57 C0                        xorps xmm0, xmm0
//! 48 8B 1A                        mov rbx, [rdx]        ; pointer-sized field at +0
//! 4D 8B F8                        mov r15, r8           ; r8 read as 64 bits
//! 48 8B 72 08                     mov rsi, [rdx+8]      ; pointer-sized field at +8
//! ```
//!
//! So: `rcx` is 64 bits, `rdx` is a pointer the function dereferences
//! unconditionally before doing anything else (the `mov eax, [rdx+0x18]` is its
//! first non-frame instruction), and `r8` is 64 bits. One step past
//! the anchor, in the same image (`0xa1c607`-`0xa1c60e`): `48 8B 7A 10`
//! (`mov rdi, [rdx+0x10]`), `49 C1 EC 18` (`shr r12, 0x18`, where `r12` is the
//! qword loaded from `[rdx]`), and `44 0F B6 6A 20` / `0F B6 42 21`
//! (byte loads from `[rdx+0x20]`, `[rdx+0x21]`).
//!
//! The `shr 0x18` on the first qword of the structure is the shape of a
//! `CGameID` byte-3 read — the same byte the Linux host's note calls the *type*
//! (2 = app). It is measured that the byte is extracted; what the byte *is* is
//! assumed.
//!
//! # Assumed
//!
//! * `[rdx]` is a `CGameID` and its low 24 bits are the app id. The Linux host
//!   receives the same value as a `CGameID *` argument; here the id sits at a
//!   struct field, so the body reads `[rdx] & 0x00FF_FFFF` and writes nothing;
//! * the return value is unmeasured. The type is a `usize`, so whatever the
//!   original leaves in `rax` is returned bit for bit (the Linux host types the
//!   same call as `-> i32`);
//! * `r9` is unmeasured: forwarded and never read;
//! * the environment map is **not** written. Neither the map's argument slot nor
//!   its element layout is measured on a Windows build, and the Linux host writes
//!   through a separate `SetEnvString` entry that the win64 corpus does not
//!   anchor. A body that guessed either would corrupt the block Steam hands to
//!   the child;
//! * stack arguments (slot 5 and beyond) are not forwarded. The Linux signature
//!   has eight parameters, so a Windows build plausibly has four on the stack,
//!   but no corpus bytes measure them and this host does not invent them.

use core::ffi::c_void;

/// `BuildSpawnEnvBlock(rcx, launch, r8, r9)` — the first argument is spilled and
/// forwarded, never read.
///
/// `launch` is an opaque pointer: the body reads one masked dword from it and
/// nothing else. The original dereferences the same pointer immediately, so the
/// read can never fault where Steam's own first instruction would not.
pub type BuildSpawnEnvBlockFn =
    unsafe extern "system" fn(rcx: *mut c_void, launch: *const u8, r8: usize, r9: usize) -> usize;

/// The `CGameID` a `launch` structure starts with, as the app id it names.
///
/// Returns `None` for a null pointer or an id of `0`, which is what the caller
/// uses to mean "no app to decide for".
///
/// # Safety
/// `launch` must be the pointer Steam passed to `BuildSpawnEnvBlock`, which the
/// original dereferences unconditionally; `None` means the caller passed null,
/// in which case Steam's own first instruction faults before ours is reached.
pub unsafe fn game_id_app_id(launch: *const u8) -> Option<u32> {
    if launch.is_null() {
        return None;
    }
    // SAFETY: the function this hook replaces loads this exact qword with
    // `mov rbx, [rdx]` before it does anything else, so the eight bytes are
    // readable whenever a body can run at all. `read_unaligned` because the
    // structure's alignment is not measured.
    let game_id = unsafe { launch.cast::<u64>().read_unaligned() };
    let app = u32::try_from(game_id & 0x00FF_FFFF).unwrap_or(0);
    (app != 0).then_some(app)
}
