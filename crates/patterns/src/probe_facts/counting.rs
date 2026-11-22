//! The per-instruction counters: stores through an argument pointer, string
//! immediates, compares and branches.

use std::collections::{BTreeSet, HashMap};

use iced_x86::{FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpKind, Register};

use super::pointer_state::{is_read, is_write, State};
use super::{Facts, EQUALS};

pub(super) fn writes_memory(
    instruction: &Instruction,
    factory: &mut InstructionInfoFactory,
) -> bool {
    instruction.op0_kind() == OpKind::Memory && is_write(factory.info(instruction).op_access(0))
}

/// Count stored immediates that build a string: a literal `'='` is what a
/// `KEY=VALUE` writer emits, and a run of printable immediates is what an
/// inlined string build looks like.
pub(super) fn count_immediate_store(
    instruction: &Instruction,
    facts: &mut Facts,
    factory: &mut InstructionInfoFactory,
) {
    if instruction.op0_kind() != OpKind::Memory || instruction.op_count() < 2 {
        return;
    }
    // `cmp [reg], 0x3d` reads the memory, it does not store to it.
    if !is_write(factory.info(instruction).op_access(0)) {
        return;
    }
    let Some(value) = stored_immediate(instruction) else {
        return;
    };
    let bytes = value.to_le_bytes();
    let width = instruction.memory_size().size().min(8);
    let bytes = &bytes[..width];
    if width == 1 {
        facts.byte_imm_stores += 1;
    }
    // A literal `'='` is either a one-byte store or part of an all-printable
    // chunk like `"KEY="`. A wide immediate that merely happens to contain the
    // byte (a magic value such as 0xef103d19) is not a string build.
    let printable = bytes.iter().all(|byte| (0x20..=0x7e).contains(byte));
    if bytes.contains(&EQUALS) && (width == 1 || printable) {
        facts.equals_imm_stores += 1;
    }
    if bytes.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
        facts.printable_imm_stores += 1;
    }
}

fn stored_immediate(instruction: &Instruction) -> Option<u64> {
    for operand in 1..instruction.op_count() {
        let value = match instruction.op_kind(operand) {
            OpKind::Immediate8 => u64::from(instruction.immediate8()),
            OpKind::Immediate16 => u64::from(instruction.immediate16()),
            OpKind::Immediate32 => u64::from(instruction.immediate32()),
            OpKind::Immediate64 => instruction.immediate64(),
            OpKind::Immediate8to32 => instruction.immediate8to32() as u32 as u64,
            OpKind::Immediate8to64 => instruction.immediate8to64() as u64,
            OpKind::Immediate32to64 => instruction.immediate32to64() as u64,
            _ => continue,
        };
        return Some(value);
    }
    None
}

/// Record `cmp reg, imm` per compared-against register: a dispatch chain
/// compares one value (the message id) against many constants, so the per
/// register set is what separates it from unrelated range checks on other
/// variables in the same function.
pub(super) fn count_compare(
    instruction: &Instruction,
    compared: &mut BTreeSet<u64>,
    chains: &mut HashMap<Register, BTreeSet<u64>>,
) {
    if instruction.mnemonic() != Mnemonic::Cmp || instruction.op0_kind() != OpKind::Register {
        return;
    }
    let Some(value) = stored_immediate(instruction) else {
        return;
    };
    compared.insert(value);
    chains
        .entry(instruction.op0_register().full_register())
        .or_default()
        .insert(value);
}

/// A read from the stack arguments: `[rsp+0x28]` and above relative to the
/// entry stack pointer, past the four register home slots.
pub(super) fn count_stack_argument(
    instruction: &Instruction,
    state: &State,
    facts: &mut Facts,
    factory: &mut InstructionInfoFactory,
) {
    if instruction.memory_base().full_register() != Register::RSP {
        return;
    }
    let Some(sp) = state.stack_pointer() else {
        return;
    };
    let offset = instruction.memory_displacement64() as i64 - sp;
    if !(0x28..0x40).contains(&offset) {
        return;
    }
    let read = (0..instruction.op_count()).any(|operand| {
        instruction.op_kind(operand) == OpKind::Memory
            && is_read(factory.info(instruction).op_access(operand))
    });
    if read {
        facts.stack_argument_reads += 1;
    }
}

/// Multiply and divide, which separate a copy loop from a resampler or a
/// checksum: those are dominated by `imul`/`sar` scaling pairs.
pub(super) fn count_arithmetic(instruction: &Instruction, facts: &mut Facts) {
    if matches!(
        instruction.mnemonic(),
        Mnemonic::Imul | Mnemonic::Mul | Mnemonic::Idiv | Mnemonic::Div
    ) {
        facts.multiplies += 1;
    }
}

/// `lea reg, [reg+reg*8]` and `imul reg, reg, 9`: addressing an array whose
/// elements are 9 qwords (0x48) each.
pub(super) fn count_stride_index(instruction: &Instruction, facts: &mut Facts) {
    let nine = match instruction.mnemonic() {
        Mnemonic::Lea if instruction.op1_kind() == OpKind::Memory => {
            let base = instruction.memory_base().full_register();
            let index = instruction.memory_index().full_register();
            base != Register::None
                && base == index
                && instruction.memory_index_scale() == 8
                && instruction.memory_displacement64() == 0
        }
        Mnemonic::Imul => {
            matches!(
                instruction.op_kind(2),
                OpKind::Immediate8 | OpKind::Immediate32
            ) && stored_immediate(instruction) == Some(9)
                && instruction.op_count() >= 3
        }
        _ => false,
    };
    if nine {
        facts.stride9_leas += 1;
    }
}

/// Reads of an argument register's low byte, which is how a `bool` parameter is
/// consumed (`movzx eax, r9b`, `test r9b, r9b`).
pub(super) fn count_byte_argument(
    instruction: &Instruction,
    facts: &mut Facts,
    factory: &mut InstructionInfoFactory,
) {
    for operand in 0..instruction.op_count() {
        if instruction.op_kind(operand) != OpKind::Register {
            continue;
        }
        let register = instruction.op_register(operand);
        if register.size() != 1 || !register.is_gpr8() {
            continue;
        }
        let full = register.full_register();
        if !crate::msabi::MSABI_ARGUMENTS.contains(&full) {
            continue;
        }
        if is_read(factory.info(instruction).op_access(operand)) {
            facts.byte_argument_reads += 1;
            return;
        }
    }
}

/// An indexed load, `mov/movsxd reg, [base + index*4]` or `*8`: the table read
/// a switch performs before it jumps, and the only way into an offset table.
pub(super) fn count_table_load(
    instruction: &Instruction,
    facts: &mut Facts,
    factory: &mut InstructionInfoFactory,
) {
    if instruction.memory_index() == Register::None {
        return;
    }
    if !matches!(instruction.memory_index_scale(), 4 | 8) {
        return;
    }
    let read = (0..instruction.op_count()).any(|operand| {
        instruction.op_kind(operand) == OpKind::Memory
            && is_read(factory.info(instruction).op_access(operand))
    });
    if read {
        facts.table_loads += 1;
    }
}

pub(super) fn count_load(
    instruction: &Instruction,
    state: &State,
    facts: &mut Facts,
    factory: &mut InstructionInfoFactory,
) {
    let width = instruction.memory_size().size();
    if width != 4 {
        return;
    }
    for operand in 0..instruction.op_count() {
        if instruction.op_kind(operand) != OpKind::Memory {
            continue;
        }
        if !is_read(factory.info(instruction).op_access(operand)) {
            continue;
        }
        let base = instruction.memory_base().full_register();
        let Some(arg) = state.provenance(base) else {
            continue;
        };
        facts.u32_loads_via_arg[usize::from(arg)] += 1;
        return;
    }
}

pub(super) fn count_branches(
    instruction: &Instruction,
    facts: &mut Facts,
    loops: &mut BTreeSet<u64>,
    spans: &mut Vec<(u64, u64)>,
) {
    let flow = instruction.flow_control();
    if flow == FlowControl::ConditionalBranch {
        facts.conditional_branches += 1;
    }
    if flow == FlowControl::IndirectBranch {
        facts.indirect_branches += 1;
        if instruction.op0_kind() == OpKind::Register {
            facts.register_branches += 1;
        }
        // `jmp qword ptr [reg*8 + table]`: an indexed indirect branch is the
        // shape of a jump table, whatever the table holds.
        if instruction.op0_kind() == OpKind::Memory
            && instruction.memory_index() != Register::None
            && instruction.memory_index_scale() == 8
        {
            facts.jump_table = true;
        }
    }
    if !matches!(
        flow,
        FlowControl::ConditionalBranch | FlowControl::UnconditionalBranch
    ) {
        return;
    }
    let target = instruction.near_branch_target();
    if target < instruction.ip() {
        loops.insert(target);
        spans.push((target, instruction.next_ip()));
    }
}

pub(super) fn direct_target(instruction: &Instruction) -> Option<u32> {
    if instruction.op0_kind() != OpKind::NearBranch64 {
        return None;
    }
    u32::try_from(instruction.near_branch_target()).ok()
}
