//! Architecture-sized entry-patch byte layouts and their guarded writer.
//!
//! Byte layouts are pure functions so they can be unit-tested on any host,
//! including the Windows layout from a Linux CI machine. Only
//! [`write_code_patch`] touches memory, and it goes through
//! [`crate::protect::with_writable`] so the previous page protection is
//! restored whether the write path succeeds or bails out.
//!
//! The Windows host installs its own absolute jump today; this module is the
//! reusable primitive for it, and it is the piece that flushes the instruction
//! cache (`FlushInstructionCache` on Windows) after the write.

use thiserror::Error;

use crate::protect::{self, ProtectError};

/// Length of the absolute x86-64 jump: `mov rax, imm64; jmp rax`.
pub const ABSOLUTE_JUMP_LEN: usize = 12;

/// Length of the relative x86 jump: `jmp rel32`.
pub const RELATIVE_JUMP_LEN: usize = 5;

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum PatchError {
    #[error(transparent)]
    Protect(#[from] ProtectError),
    #[error("a relative jump from {from:#x} to {to:#x} does not fit in 32 bits")]
    JumpOutOfRange { from: usize, to: usize },
}

/// An absolute jump that reaches any address: `mov rax, imm64; jmp rax`.
///
/// This is the only jump shape that needs no distance limit, which is why it is
/// used on x86-64 where a module and a replacement live further than 2 GiB
/// apart. The target is a `u64` because that is the width the encoding carries:
/// the function is meaningful on every host (and unit-tested on them), and a
/// 32-bit caller simply widens its address.
pub fn absolute_jump(target: u64) -> [u8; ABSOLUTE_JUMP_LEN] {
    let mut patch = [0u8; ABSOLUTE_JUMP_LEN];
    patch[0] = 0x48; // REX.W
    patch[1] = 0xB8; // mov rax, imm64
    patch[2..10].copy_from_slice(&target.to_le_bytes());
    patch[10] = 0xFF; // jmp r/m64
    patch[11] = 0xE0; // modrm: /4, rax
    patch
}

/// A `jmp rel32` from `from` to `to`, or [`PatchError::JumpOutOfRange`].
///
/// `from` is the address of the jump instruction itself, so the displacement is
/// measured from the end of the 5-byte encoding.
pub fn relative_jump(from: usize, to: usize) -> Result<[u8; RELATIVE_JUMP_LEN], PatchError> {
    let displacement = to as i128 - from as i128 - RELATIVE_JUMP_LEN as i128;
    let displacement =
        i32::try_from(displacement).map_err(|_| PatchError::JumpOutOfRange { from, to })?;
    let mut patch = [0u8; RELATIVE_JUMP_LEN];
    patch[0] = 0xE9;
    patch[1..].copy_from_slice(&displacement.to_le_bytes());
    Ok(patch)
}

/// Write `patch` at `address`, then flush the instruction cache.
///
/// The page is writable only for the duration of the copy: the previous
/// protection is restored on every path, including an early return from
/// [`protect::with_writable`].
///
/// # Safety
/// `address..address+len` must be the entry point of a validated target and
/// `patch` must be a complete instruction sequence that leaves the stack and
/// registers in the state the caller's thunk expects.
pub unsafe fn write_code_patch(address: usize, patch: &[u8]) -> Result<(), PatchError> {
    protect::with_writable(address, patch.len(), || {
        // SAFETY: the caller guarantees the destination is a validated target
        // and the page is writable for the duration of this closure.
        unsafe {
            std::ptr::copy_nonoverlapping(patch.as_ptr(), address as *mut u8, patch.len());
        }
    })?;
    protect::flush_instruction_cache(address, patch.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_jump_layout_is_mov_rax_imm64_then_jmp_rax() {
        let patch = absolute_jump(0x7ff6_1234_5678);

        assert_eq!(patch.len(), ABSOLUTE_JUMP_LEN);
        assert_eq!(&patch[..2], &[0x48, 0xB8], "REX.W + mov rax, imm64");
        assert_eq!(
            &patch[2..10],
            &[0x78, 0x56, 0x34, 0x12, 0xf6, 0x7f, 0x00, 0x00],
            "little-endian 64-bit target"
        );
        assert_eq!(&patch[10..], &[0xFF, 0xE0], "jmp rax");
    }

    #[test]
    fn absolute_jump_target_field_is_the_whole_64_bit_address() {
        // A target above 4 GiB proves the immediate is 64 bits wide, not a
        // truncated 32-bit offset.
        let target = 0x0000_7ff7_0000_0042u64;
        let patch = absolute_jump(target);
        let mut immediate = [0u8; 8];
        immediate.copy_from_slice(&patch[2..10]);
        assert_eq!(u64::from_le_bytes(immediate), 0x0000_7ff7_0000_0042u64);
    }

    #[test]
    fn relative_jump_measures_displacement_from_the_end_of_the_encoding() {
        let patch = relative_jump(0x1000, 0x2000).expect("in range");

        assert_eq!(patch[0], 0xE9);
        let mut displacement = [0u8; 4];
        displacement.copy_from_slice(&patch[1..]);
        assert_eq!(i32::from_le_bytes(displacement), 0x2000 - 0x1000 - 5);
    }

    #[test]
    fn relative_jump_refuses_targets_further_than_two_gigabytes() {
        // 0xf000_1000 is a 32-bit address, so this displacement is "too far"
        // on every host width, not only on x86-64.
        let error = relative_jump(0x1000, 0xf000_1000).expect_err("out of range");
        assert_eq!(
            error,
            PatchError::JumpOutOfRange {
                from: 0x1000,
                to: 0xf000_1000
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn write_code_patch_writes_bytes_and_restores_protection() {
        let len = protect::page_size().expect("page size");
        // SAFETY: one fresh anonymous page, checked against MAP_FAILED.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(address, libc::MAP_FAILED, "mmap failed");
        let address = address as usize;

        let patch = absolute_jump(0x7ff6_1234_5678);
        // SAFETY: the page is readable, executable, and owned by this test.
        unsafe { write_code_patch(address, &patch) }.expect("patch");

        assert_eq!(
            protect::protection_of(address, len).expect("protection"),
            protect::Protection::READ_EXECUTE
        );
        // SAFETY: the page is one readable page owned by this test.
        let written = unsafe { std::slice::from_raw_parts(address as *const u8, patch.len()) };
        assert_eq!(written, &patch);

        // SAFETY: unmapping the mapping created above exactly once.
        unsafe { libc::munmap(address as *mut libc::c_void, len) };
    }
}
