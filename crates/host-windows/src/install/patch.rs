//! Detour patch planning: instruction-boundary patch length and trampoline bytes.
//!
//! Two rules make an in-place detour safe, and both are decided here rather than
//! in the writer:
//!
//! 1. The patch must not split an instruction, so the length is the shortest run
//!    of whole instructions covering the 12-byte `mov rax, imm64; jmp rax` form.
//! 2. The displaced instructions must be relocatable by plain copying. A
//!    RIP-relative operand or a relative branch anywhere in that run keeps
//!    referring to the *old* address once copied, so it is refused instead of
//!    producing a silently broken trampoline.
//!
//! Everything here is pure byte logic: no Win32, so it is unit-tested on any
//! host and the writer only has to move bytes.

use iced_x86::{Decoder, DecoderOptions, FlowControl, Instruction, OpKind, Register};

/// Bytes the entry patch needs: `mov rax, imm64` (10) + `jmp rax` (2).
pub const PATCH_LEN: usize = 12;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PatchPlan {
    /// Bytes at the function entry that the patch overwrites.
    pub patch_len: usize,
    /// Instructions displaced from the entry, followed by a jump back.
    pub trampoline: Vec<u8>,
    /// Address the trampoline resumes at (entry + `patch_len`).
    pub resume_at: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PatchError {
    /// Not enough decodable code to fit the patch.
    TooShort { needed: usize, available: usize },
    /// The decoder rejected a byte in the run.
    Decode { at: u64 },
    /// A displaced instruction cannot be relocated by copying.
    RelocationRequired { at: u64, instruction: String },
    /// The entry does not look like the start of a function body.
    UnsupportedEntry { at: u64, reason: String },
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort { needed, available } => write!(
                f,
                "need {needed} bytes at the entry, only {available} available"
            ),
            Self::Decode { at } => write!(f, "undecodable instruction at {at:#x}"),
            Self::RelocationRequired { at, instruction } => {
                write!(f, "instruction at {at:#x} needs relocation: {instruction}")
            }
            Self::UnsupportedEntry { at, reason } => {
                write!(f, "entry at {at:#x} is not patchable: {reason}")
            }
        }
    }
}

impl std::error::Error for PatchError {}

/// Plan a detour patch at `entry_va` given the code currently there.
///
/// `code` must start at the function entry; only the first ~64 bytes matter.
pub fn plan(code: &[u8], entry_va: u64) -> Result<PatchPlan, PatchError> {
    if code.len() < PATCH_LEN {
        return Err(PatchError::TooShort {
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
            return Err(PatchError::TooShort {
                needed: PATCH_LEN,
                available: consumed,
            });
        }
        let at = decoder.ip();
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() || instruction.len() == 0 {
            return Err(PatchError::Decode { at });
        }
        if first && instruction.flow_control() == FlowControl::Return {
            return Err(PatchError::UnsupportedEntry {
                at,
                reason: "entry returns immediately".to_owned(),
            });
        }
        first = false;
        if requires_relocation(&instruction) {
            return Err(PatchError::RelocationRequired {
                at,
                instruction: format!("{:?}", instruction.code()),
            });
        }
        consumed += instruction.len();
    }

    let displaced = code.get(..consumed).ok_or(PatchError::TooShort {
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

/// The 12 entry bytes: `mov rax, target; jmp rax`.
pub fn absolute_jump(target: u64) -> [u8; PATCH_LEN] {
    let mut bytes = [0u8; PATCH_LEN];
    bytes[0] = 0x48; // REX.W
    bytes[1] = 0xB8; // mov rax, imm64
    bytes[2..10].copy_from_slice(&target.to_le_bytes());
    bytes[10] = 0xFF; // jmp rax
    bytes[11] = 0xE0;
    bytes
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
            Err(PatchError::RelocationRequired { at, .. }) => assert_eq!(at, 0x2000),
            other => panic!("expected relocation refusal, got {other:?}"),
        }
    }

    #[test]
    fn relative_branch_is_refused() {
        let code = [0xE8, 0x00, 0x00, 0x00, 0x00, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90];
        assert!(matches!(
            plan(&code, 0x3000),
            Err(PatchError::RelocationRequired { .. })
        ));
    }

    #[test]
    fn short_input_and_non_body_entries_fail_closed() {
        assert!(matches!(
            plan(&PROLOGUE[..8], 0x1000),
            Err(PatchError::TooShort { .. })
        ));
        // A function that returns immediately has no body worth patching.
        let mut tail = PROLOGUE;
        tail[0] = 0xC3;
        assert!(matches!(
            plan(&tail, 0x1000),
            Err(PatchError::UnsupportedEntry { .. })
        ));
    }

    #[test]
    fn absolute_jump_is_twelve_bytes_and_site_independent() {
        let jump = absolute_jump(0xDEAD_BEEF_1234_5678);
        assert_eq!(jump.len(), PATCH_LEN);
        assert_eq!(&jump[2..10], &0xDEAD_BEEF_1234_5678u64.to_le_bytes());
        assert_ne!(absolute_jump(1), absolute_jump(2));
    }
}
