//! Instruction-boundary patch planning for an in-place detour.
//!
//! Two rules make an in-place detour safe, and both are decided here rather than
//! in a writer:
//!
//! 1. The patch must not split an instruction, so its length is the shortest
//!    run of whole instructions covering the [`ABSOLUTE_JUMP_LEN`]-byte
//!    `mov rax, imm64; jmp rax` form.
//! 2. The displaced instructions must be relocatable by plain copying. A
//!    RIP-relative operand or a relative branch anywhere in that run keeps
//!    referring to the *old* address once copied, so it is refused instead of
//!    producing a silently broken trampoline.
//!
//! Everything here is pure byte logic: no OS call, no allocation, no write. The
//! planner is therefore unit-tested on any host, including the MS-x64 layout
//! from a Linux CI machine, and the Windows writer only has to move the bytes
//! this module describes.

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, OpKind, Register};

use super::{ABSOLUTE_JUMP_LEN, absolute_jump};

/// Bytes the entry patch needs: `mov rax, imm64` (10) + `jmp rax` (2).
pub const PATCH_LEN: usize = ABSOLUTE_JUMP_LEN;

/// The instruction-boundary patch for one function entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchPlan {
    /// Bytes at the function entry that the patch overwrites.
    pub patch_len: usize,
    /// Instructions displaced from the entry, followed by a jump back.
    pub trampoline: Vec<u8>,
    /// Address the trampoline resumes at (entry + `patch_len`).
    pub resume_at: u64,
}

/// Why an entry cannot be patched in place.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PlanError {
    /// Not enough decodable code to fit the patch.
    #[error("need {needed} bytes at the entry, only {available} available")]
    TooShort { needed: usize, available: usize },
    /// The decoder rejected a byte in the run.
    #[error("undecodable instruction at {at:#x}")]
    Decode { at: u64 },
    /// A displaced instruction cannot be relocated by copying.
    #[error("instruction at {at:#x} needs relocation: {instruction}")]
    RelocationRequired { at: u64, instruction: String },
    /// The entry does not look like the start of a function body.
    #[error("entry at {at:#x} is not patchable: {reason}")]
    UnsupportedEntry { at: u64, reason: String },
}

/// Plan a detour patch at `entry_va` given the code currently there.
///
/// `code` must start at the function entry. Only the first ~64 bytes matter; a
/// shorter slice is fine as long as it contains whole instructions covering
/// [`PATCH_LEN`], and anything shorter is refused as [`PlanError::TooShort`].
pub fn plan(code: &[u8], entry_va: u64) -> Result<PatchPlan, PlanError> {
    if code.len() < PATCH_LEN {
        return Err(PlanError::TooShort {
            needed: PATCH_LEN,
            available: code.len(),
        });
    }

    let mut decoder = Decoder::with_ip(64, code, entry_va, DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut consumed = 0usize;
    let mut first = true;

    while consumed < PATCH_LEN {
        if !decoder.can_decode() {
            return Err(PlanError::TooShort {
                needed: PATCH_LEN,
                available: consumed,
            });
        }
        let at = decoder.ip();
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() || instruction.len() == 0 {
            return Err(PlanError::Decode { at });
        }
        if first && instruction.flow_control() == FlowControl::Return {
            return Err(PlanError::UnsupportedEntry {
                at,
                reason: "entry returns immediately".to_owned(),
            });
        }
        first = false;
        if requires_relocation(&instruction) {
            return Err(PlanError::RelocationRequired {
                at,
                instruction: format!("{:?}", instruction.code()),
            });
        }
        consumed += instruction.len();
    }

    let displaced = code.get(..consumed).ok_or(PlanError::TooShort {
        needed: consumed,
        available: code.len(),
    })?;
    let resume_at = entry_va + consumed as u64;
    let mut trampoline = Vec::with_capacity(consumed + PATCH_LEN);
    trampoline.extend_from_slice(displaced);
    trampoline.extend_from_slice(&absolute_jump(resume_at));

    Ok(PatchPlan {
        patch_len: consumed,
        trampoline,
        resume_at,
    })
}

impl PatchPlan {
    /// Bytes to overwrite at the entry: the absolute jump, padded with `nop` up
    /// to the instruction boundary chosen by [`plan`].
    pub fn patch_bytes(&self, target: u64) -> Vec<u8> {
        let mut bytes = absolute_jump(target).to_vec();
        bytes.resize(self.patch_len, 0x90);
        bytes
    }
}

/// An instruction whose copied bytes would keep pointing at the original site.
fn requires_relocation(instruction: &Instruction) -> bool {
    let rip_relative = (0..instruction.op_count()).any(|index| {
        instruction.op_kind(index) == OpKind::Memory && instruction.memory_base() == Register::RIP
    });
    let relative_branch = matches!(
        instruction.flow_control(),
        FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch | FlowControl::Call
    ) && instruction.op0_kind() == OpKind::NearBranch64;
    rip_relative || relative_branch
}

#[cfg(test)]
mod tests {
    use super::*;

    /// push rbp; mov rbp,rsp; sub rsp,0x20; mov rax,[rbp+8]; ret
    const PROLOGUE: [u8; 16] = [
        0x55, 0x48, 0x89, 0xE5, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0x45, 0x08, 0xC3, 0x90, 0x90,
        0x90,
    ];

    #[test]
    fn patch_length_stops_on_an_instruction_boundary_at_least_twelve_bytes_in() {
        let plan = plan(&PROLOGUE, 0x1000).expect("plan");
        assert_eq!(plan.patch_len, 12, "1+3+4+4 bytes of whole instructions");
        assert_eq!(plan.resume_at, 0x1000 + 12);
        assert_eq!(&plan.trampoline[..12], &PROLOGUE[..12]);
    }

    #[test]
    fn trampoline_returns_to_the_first_undisplaced_instruction() {
        let plan = plan(&PROLOGUE, 0x1_4000_1000).expect("plan");
        let tail = &plan.trampoline[plan.patch_len..];
        assert_eq!(tail, absolute_jump(0x1_4000_1000 + 12));
        // mov rax, imm64 ; jmp rax
        assert_eq!(&tail[..2], &[0x48, 0xB8]);
        assert_eq!(&tail[10..], &[0xFF, 0xE0]);
    }

    #[test]
    fn rip_relative_operand_is_refused_instead_of_copied_wrong() {
        // mov rax, [rip+0x10]
        let code = [0x48, 0x8B, 0x05, 0x10, 0x00, 0x00, 0x00, 0x55, 0x90, 0x90, 0x90, 0x90, 0x90];
        match plan(&code, 0x2000) {
            Err(PlanError::RelocationRequired { at, .. }) => assert_eq!(at, 0x2000),
            other => panic!("expected relocation refusal, got {other:?}"),
        }
    }

    #[test]
    fn relative_branch_is_refused() {
        let code = [0xE8, 0x00, 0x00, 0x00, 0x00, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90];
        assert!(matches!(
            plan(&code, 0x3000),
            Err(PlanError::RelocationRequired { .. })
        ));
    }

    #[test]
    fn short_input_and_non_body_entries_fail_closed() {
        assert!(matches!(
            plan(&PROLOGUE[..8], 0x1000),
            Err(PlanError::TooShort { .. })
        ));
        // A function that returns immediately has no body worth patching.
        let mut tail = PROLOGUE;
        tail[0] = 0xC3;
        assert!(matches!(
            plan(&tail, 0x1000),
            Err(PlanError::UnsupportedEntry { .. })
        ));
    }

    #[test]
    fn patch_bytes_are_padded_to_the_planned_boundary() {
        let base_plan = plan(&PROLOGUE, 0x1000).expect("plan");
        assert_eq!(base_plan.patch_len, 12);
        assert_eq!(base_plan.patch_bytes(0x2000).len(), 12);

        // One byte more of displacement must still land on a boundary, and the
        // padding must be nops rather than part of the jump.
        let longer = [0x55, 0x48, 0x83, 0xEC, 0x28, 0x48, 0x8B, 0x45, 0x08, 0x48, 0x83, 0xC0, 0x01, 0xC3];
        let longer_plan = plan(&longer, 0x1000).expect("plan");
        assert_eq!(longer_plan.patch_len, 13, "whole instructions");
        let bytes = longer_plan.patch_bytes(0x2000);
        assert_eq!(bytes.len(), 13);
        assert_eq!(bytes[12], 0x90);
    }
}
