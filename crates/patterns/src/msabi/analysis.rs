//! The linear walk that fills a [`Report`], and the stack-pointer tracking the
//! home-area offsets depend on.
//!
//! Kept apart from the report types so the facts the tool prints stay readable
//! next to the code that decides them.

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess,
    OpKind, Register,
};

use super::{FirstUse, Report, Slot, MSABI_ARGUMENTS};
/// Analyse the function entry at `offset` in `code`.
///
/// Instruction offsets in the result are offsets into `code`, so they compare
/// directly with other resolved offsets.
pub fn analyse(code: &[u8], offset: usize, window: usize) -> Report {
    let mut report = Report::entry();
    let Some(tail) = code.get(offset..) else {
        return report;
    };
    let tail = &tail[..tail.len().min(window)];
    let mut decoder = Decoder::with_ip(64, tail, offset as u64, DecoderOptions::NONE);
    let mut factory = InstructionInfoFactory::new();
    let mut instruction = Instruction::default();
    // Stack pointer relative to its value at entry, while it is known. The
    // home slots are addressed from the entry stack pointer, so a spill after
    // `sub rsp, N` only lands on a home slot if this is tracked.
    let mut sp: Option<i64> = Some(0);

    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        let flow = instruction.flow_control();
        step(&mut report, &instruction, &mut factory, sp);
        sp = adjust_stack_pointer(&instruction, sp);
        if flow == FlowControl::Return {
            break;
        }
    }
    report
}

/// What the first instruction that touches an argument did. Operand indices are
/// those of the instruction, and the caller passes the index the argument was
/// seen at: an argument is read either as a source operand or as the base of a
/// memory operand, so for `mov dst, src` it is operand 1.
fn classify(instruction: &Instruction, operand: u32, sp: Option<i64>) -> FirstUse {
    // `mov reg, arg` forwards the value; `mov [rsp+N], arg` spills it to the
    // home area. Both are reads of the argument, not address computations.
    if instruction.mnemonic() == Mnemonic::Mov && operand == 1 {
        match instruction.op0_kind() {
            OpKind::Register if instruction.op1_kind() == OpKind::Register => {
                return FirstUse::CopiedTo(instruction.op0_register().full_register());
            }
            OpKind::Memory if instruction.memory_base().full_register() == Register::RSP => {
                if let Some(sp) = sp {
                    let displacement = instruction.memory_displacement64() as i64;
                    return FirstUse::HomeSlot(displacement - sp);
                }
            }
            _ => {}
        }
    }
    if instruction.op_kind(operand) == OpKind::Memory {
        FirstUse::MemoryAddress
    } else {
        FirstUse::Operand
    }
}

fn step(
    report: &mut Report,
    instruction: &Instruction,
    factory: &mut InstructionInfoFactory,
    sp: Option<i64>,
) {
    let ip = instruction.ip() as usize;
    // `xor reg, reg` and `sub reg, reg` zero the register; iced reports that as
    // read-write, but no incoming value is consumed.
    let zeroing = matches!(instruction.mnemonic(), Mnemonic::Xor | Mnemonic::Sub)
        && instruction.op0_kind() == OpKind::Register
        && instruction.op1_kind() == OpKind::Register
        && instruction.op0_register() == instruction.op1_register();

    let info = factory.info(instruction);
    for operand in 0..instruction.op_count() {
        let Some(position) = slot_of_operand(instruction, operand) else {
            continue;
        };
        let reads = operand_reads(instruction, operand, info.op_access(operand));
        if zeroing && !reads {
            note_write(&mut report.slots[usize::from(position)], ip);
            continue;
        }
        if reads {
            let use_ = classify(instruction, operand, sp);
            note_read(&mut report.slots[usize::from(position)], ip, use_);
        } else if is_write(info.op_access(operand)) {
            note_write(&mut report.slots[usize::from(position)], ip);
        }
    }
}

/// Which argument slot an operand names, if any. A memory operand names a slot
/// when the slot's register is its base or index.
fn slot_of_operand(instruction: &Instruction, operand: u32) -> Option<u8> {
    let register = match instruction.op_kind(operand) {
        OpKind::Register => instruction.op_register(operand).full_register(),
        OpKind::Memory => {
            let base = instruction.memory_base().full_register();
            let index = instruction.memory_index().full_register();
            if base == Register::RIP || base == Register::None {
                if index == Register::None {
                    return None;
                }
                index
            } else {
                base
            }
        }
        _ => return None,
    };
    MSABI_ARGUMENTS
        .iter()
        .position(|candidate| candidate.full_register() == register)
        .map(|position| position as u8)
}

/// Whether the instruction consumes the value in this operand.
fn operand_reads(instruction: &Instruction, operand: u32, access: OpAccess) -> bool {
    match instruction.op_kind(operand) {
        // A memory operand's base and index are always read, even when the
        // memory itself is only written (`mov [rdx], eax` reads `rdx`).
        OpKind::Memory => true,
        _ => is_read(access),
    }
}

fn note_read(slot: &mut Slot, ip: usize, use_: FirstUse) {
    if slot.first_read.is_none() {
        slot.first_read = Some(ip);
    }
    if slot.first_use.is_none() || slot.first_use == Some(FirstUse::Overwritten) {
        slot.first_use = Some(use_);
    }
}

fn note_write(slot: &mut Slot, ip: usize) {
    if slot.first_write.is_none() {
        slot.first_write = Some(ip);
    }
    if slot.first_use.is_none() {
        slot.first_use = Some(FirstUse::Overwritten);
    }
}

/// New value of `rsp` relative to its value at entry, after `instruction`.
///
/// `None` once the relation is unknown (`lea rsp, [...]`, `and rsp, -16`).
pub fn adjust_stack_pointer(instruction: &Instruction, sp: Option<i64>) -> Option<i64> {
    let sp = sp?;
    match instruction.mnemonic() {
        Mnemonic::Sub | Mnemonic::Add
            if instruction.op0_kind() == OpKind::Register
                && instruction.op0_register().full_register() == Register::RSP =>
        {
            let delta = signed_immediate(instruction, 1)?;
            Some(if instruction.mnemonic() == Mnemonic::Sub {
                sp - delta
            } else {
                sp + delta
            })
        }
        Mnemonic::Push => Some(sp - i64::from(instruction.stack_pointer_increment())),
        Mnemonic::Pop => Some(sp + i64::from(instruction.stack_pointer_increment())),
        // Anything else that writes rsp (`lea rsp, [...]`, `and rsp, -16`,
        // a frame teardown) leaves the relation unknown.
        _ if writes_rsp(instruction) => None,
        _ => Some(sp),
    }
}

fn writes_rsp(instruction: &Instruction) -> bool {
    (0..instruction.op_count()).any(|operand| {
        instruction.op_kind(operand) == OpKind::Register
            && instruction.op0_kind() == OpKind::Register
            && instruction.op0_register().full_register() == Register::RSP
    })
}

fn is_read(access: OpAccess) -> bool {
    matches!(
        access,
        OpAccess::Read | OpAccess::CondRead | OpAccess::ReadWrite | OpAccess::ReadCondWrite
    )
}

fn is_write(access: OpAccess) -> bool {
    matches!(
        access,
        OpAccess::Write | OpAccess::ReadWrite | OpAccess::CondWrite | OpAccess::ReadCondWrite
    )
}

fn signed_immediate(instruction: &Instruction, operand: u32) -> Option<i64> {
    Some(match instruction.op_kind(operand) {
        OpKind::Immediate8 => i64::from(instruction.immediate8() as i8),
        OpKind::Immediate16 => i64::from(instruction.immediate16() as i16),
        OpKind::Immediate32 => i64::from(instruction.immediate32() as i32),
        OpKind::Immediate64 => instruction.immediate64() as i64,
        OpKind::Immediate8to16 => i64::from(instruction.immediate8to16()),
        OpKind::Immediate8to32 => i64::from(instruction.immediate8to32()),
        OpKind::Immediate8to64 => instruction.immediate8to64(),
        OpKind::Immediate32to64 => instruction.immediate32to64(),
        _ => return None,
    })
}
