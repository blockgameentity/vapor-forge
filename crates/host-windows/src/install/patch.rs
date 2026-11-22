//! Detour patch planning: re-exported from `vapor-forge-hook-engine`.
//!
//! The planner used to live here. It moved to the engine when the engine grew a
//! Windows writer of its own (`vapor_forge_hook_engine::writer`), because two
//! copies of "how many whole instructions may an entry patch overwrite" would
//! drift: the host would plan one boundary and the engine install another. The
//! engine's `patch::planner` is the single copy, and this module keeps the
//! `install::patch` path and item names working for anything that already used
//! them.
//!
//! Everything here is pure byte logic: no Win32, so it is unit-tested on any
//! host, and the writer only has to move the bytes.

pub use vapor_forge_hook_engine::patch::plan;
pub use vapor_forge_hook_engine::patch::planner::{PatchPlan, PlanError as PatchError};

/// Bytes the entry patch needs: `mov rax, imm64` (10) + `jmp rax` (2).
pub use vapor_forge_hook_engine::patch::ABSOLUTE_JUMP_LEN as PATCH_LEN;

/// The 12 entry bytes: `mov rax, target; jmp rax`.
pub use vapor_forge_hook_engine::patch::absolute_jump;

#[cfg(test)]
mod tests {
    use super::*;

    /// push rbp; mov rbp,rsp; sub rsp,0x20; mov rax,[rbp+8]; ret
    const PROLOGUE: [u8; 16] = [
        0x55, 0x48, 0x89, 0xE5, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0x45, 0x08, 0xC3, 0x90, 0x90,
        0x90,
    ];

    #[test]
    fn the_host_and_the_engine_plan_the_same_boundary() {
        // Proves the re-export is the engine's planner and not a local copy: the
        // host must see the engine's boundary, its trampoline, and its refusals.
        let patch_plan = plan(&PROLOGUE, 0x1000).expect("plan");
        assert_eq!(patch_plan.patch_len, PATCH_LEN);
        assert_eq!(&patch_plan.trampoline[..PATCH_LEN], &PROLOGUE[..PATCH_LEN]);

        let short = plan(&PROLOGUE[..8], 0x1000).expect_err("short entry");
        assert!(matches!(short, PatchError::TooShort { .. }));
    }

    #[test]
    fn the_jump_encoding_is_the_engine_encoding() {
        assert_eq!(
            absolute_jump(0x1_4000_1234),
            [0x48, 0xB8, 0x34, 0x12, 0x00, 0x40, 0x01, 0x00, 0x00, 0x00, 0xFF, 0xE0]
        );
    }
}
