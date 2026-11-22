//! Small instruction predicates the walk asks about.
//!
//! Kept apart from the walk itself: each is a pure function of one instruction
//! and its own unit, so the walk reads as control flow instead of decode detail.

use iced_x86::{Instruction, Mnemonic, OpKind};

pub(super) fn is_self_clear(instruction: &Instruction) -> bool {
    instruction.op_count() == 2
        && instruction.op0_kind() == OpKind::Register
        && instruction.op1_kind() == OpKind::Register
        && instruction.op0_register().full_register() == instruction.op1_register().full_register()
}

pub(super) fn is_setcc(mnemonic: Mnemonic) -> bool {
    matches!(
        mnemonic,
        Mnemonic::Sete
            | Mnemonic::Setne
            | Mnemonic::Setb
            | Mnemonic::Setae
            | Mnemonic::Setbe
            | Mnemonic::Seta
            | Mnemonic::Sets
            | Mnemonic::Setns
            | Mnemonic::Seto
            | Mnemonic::Setno
            | Mnemonic::Setp
            | Mnemonic::Setnp
            | Mnemonic::Setl
            | Mnemonic::Setge
            | Mnemonic::Setle
            | Mnemonic::Setg
    )
}

pub(super) fn immediate(instruction: &Instruction) -> Option<i64> {
    match instruction.op1_kind() {
        OpKind::Immediate8 => Some(i64::from(instruction.immediate8())),
        OpKind::Immediate8to32 => Some(i64::from(instruction.immediate8to32())),
        OpKind::Immediate8to64 => Some(instruction.immediate8to64()),
        OpKind::Immediate32 => Some(i64::from(instruction.immediate32())),
        OpKind::Immediate32to64 => Some(instruction.immediate32to64()),
        OpKind::Immediate64 => Some(instruction.immediate64() as i64),
        _ => None,
    }
}
