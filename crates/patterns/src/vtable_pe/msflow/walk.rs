//! The linear MS-x64 walk that fills a [`Flow`].
//!
//! Kept apart from the report types so the facts the tool prints stay readable
//! next to the code that decides them, the same split `crate::msabi` uses.

use std::collections::HashMap;

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess,
    OpKind, Register,
};

use super::ops::{immediate, is_self_clear, is_setcc};
use super::{AlSet, Flow, Value, ARGUMENTS, CALLER_SAVED, MAX_INSNS, SHADOW_SPACE};

struct State {
    registers: HashMap<Register, Value>,
    /// Home slots and spills, keyed by displacement from the entry `rsp`.
    stack: HashMap<i64, Value>,
    /// `rsp` relative to its value at entry, while it is known.
    sp: Option<i64>,
    al: Option<AlSet>,
}

impl State {
    fn entry() -> Self {
        let mut registers = HashMap::new();
        for (position, register) in ARGUMENTS.iter().enumerate() {
            registers.insert(*register, Value::Arg(position as u8));
        }
        Self {
            registers,
            stack: HashMap::new(),
            sp: Some(0),
            al: None,
        }
    }

    fn value_of(&self, register: Register) -> Option<Value> {
        self.registers.get(&register.full_register()).copied()
    }

    fn forget(&mut self, register: Register) {
        self.registers.remove(&register.full_register());
    }

    fn set(&mut self, register: Register, value: Value) {
        self.registers.insert(register.full_register(), value);
        if register.full_register() == Register::RAX {
            self.al = match value {
                Value::Imm(imm) => Some(AlSet::Imm(imm)),
                _ => None,
            };
        }
    }

    /// Where a memory operand points, in terms of the traced arguments.
    fn base(&self, instruction: &Instruction) -> MemoryBase {
        if instruction.is_ip_rel_memory_operand() {
            return MemoryBase::Unknown;
        }
        let base = instruction.memory_base().full_register();
        if instruction.memory_index() == Register::None && base == Register::RSP {
            if let Some(sp) = self.sp {
                return MemoryBase::Stack(instruction.memory_displacement64() as i64 - sp);
            }
        }
        match self.value_of(base) {
            Some(value) => MemoryBase::Argument(value),
            None => MemoryBase::Unknown,
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum MemoryBase {
    Argument(Value),
    /// A stack address, keyed by offset from the entry `rsp`.
    Stack(i64),
    Unknown,
}

/// Walk `code` (which lives at `base_rva` in the image; the address is only
/// carried in instruction IPs and never printed) and report the argument flow.
pub fn analyse(code: &[u8], base_rva: u32) -> Flow {
    let mut flow = Flow::default();
    let mut decoder = Decoder::with_ip(64, code, u64::from(base_rva), DecoderOptions::NONE);
    let mut factory = InstructionInfoFactory::new();
    let mut instruction = Instruction::default();
    let mut state = State::entry();
    let mut seen = 0u32;

    while decoder.can_decode() && seen < MAX_INSNS {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        seen += 1;
        note_reads(&instruction, &mut factory, &mut flow);
        match instruction.flow_control() {
            FlowControl::Return => {
                flow.rets += 1;
                flow.returned = state.value_of(Register::RAX);
                if matches!(state.al, Some(AlSet::Imm(0..=1)) | Some(AlSet::Flag)) {
                    flow.bool_rets += 1;
                }
                break;
            }
            FlowControl::UnconditionalBranch | FlowControl::IndirectBranch => break,
            FlowControl::Call | FlowControl::IndirectCall => {
                note_call(&state, &mut flow);
                // Caller-saved registers are dead after the call; a callee-saved
                // register still holds what the body put in it, which matters
                // because an out-parameter is often written only at the end.
                for register in CALLER_SAVED {
                    state.forget(register);
                }
                state.al = None;
                continue;
            }
            _ => {}
        }
        step(&instruction, &mut factory, &mut state, &mut flow);
    }
    flow
}

/// Count how each argument register is read, 32-bit or 64-bit.
fn note_reads(instruction: &Instruction, factory: &mut InstructionInfoFactory, flow: &mut Flow) {
    for used in factory.info(instruction).used_registers() {
        if !matches!(
            used.access(),
            OpAccess::Read | OpAccess::CondRead | OpAccess::ReadWrite | OpAccess::ReadCondWrite
        ) {
            continue;
        }
        let register = used.register();
        let Some(position) = ARGUMENTS
            .iter()
            .position(|argument| argument.full_register() == register.full_register())
        else {
            continue;
        };
        match register.size() {
            4 => flow.arg32_reads[position] += 1,
            8 => flow.arg64_reads[position] += 1,
            _ => {}
        }
    }
}

fn note_call(state: &State, flow: &mut Flow) {
    flow.calls += 1;
    // One call site hands an argument over once, even when the callee receives
    // it in two registers (`mov rcx, rdx` after copying): count per site.
    let mut seen = 0u8;
    for register in ARGUMENTS {
        if let Some(argument) = state.value_of(register).and_then(Value::argument) {
            seen |= 1 << argument;
        }
    }
    for position in 0..ARGUMENTS.len() {
        if seen & (1 << position) != 0 {
            flow.forwarded[position] += 1;
        }
    }
}

fn step(
    instruction: &Instruction,
    factory: &mut InstructionInfoFactory,
    state: &mut State,
    flow: &mut Flow,
) {
    match instruction.mnemonic() {
        Mnemonic::Mov => mov(instruction, state, flow),
        Mnemonic::Lea => lea(instruction, state),
        Mnemonic::Xor if is_self_clear(instruction) => {
            state.set(instruction.op0_register(), Value::Imm(0));
        }
        mnemonic if is_setcc(mnemonic) => {
            if instruction.op0_kind() == OpKind::Register
                && instruction.op0_register().full_register() == Register::RAX
            {
                state.al = Some(AlSet::Flag);
            }
        }
        _ => {
            // Any other instruction may overwrite a tracked register
            // (`add rbx, 8`, `pop rbx`, `imul`); drop it so a stale value is
            // never mistaken for the argument.
            let written: Vec<Register> = factory
                .info(instruction)
                .used_registers()
                .iter()
                .filter(|used| {
                    matches!(
                        used.access(),
                        OpAccess::Write | OpAccess::CondWrite | OpAccess::ReadWrite
                    )
                })
                .map(|used| used.register())
                .collect();
            for register in written {
                state.forget(register);
            }
        }
    }
}

fn mov(instruction: &Instruction, state: &mut State, flow: &mut Flow) {
    match (instruction.op0_kind(), instruction.op1_kind()) {
        (OpKind::Register, OpKind::Register) => match state.value_of(instruction.op1_register()) {
            Some(value) => state.set(instruction.op0_register(), value),
            None => state.forget(instruction.op0_register()),
        },
        (OpKind::Register, OpKind::Memory) => {
            let value = match state.base(instruction) {
                MemoryBase::Argument(value) => value.argument().map(Value::Indirect),
                MemoryBase::Stack(key) => state.stack.get(&key).copied(),
                MemoryBase::Unknown => None,
            };
            match value {
                Some(value) => state.set(instruction.op0_register(), value),
                None => state.forget(instruction.op0_register()),
            }
        }
        (OpKind::Memory, _) => {
            let value = if instruction.op1_kind() == OpKind::Register {
                state.value_of(instruction.op1_register())
            } else {
                immediate(instruction).map(Value::Imm)
            };
            store(instruction, value, state, flow);
        }
        (OpKind::Register, _) => match immediate(instruction) {
            Some(value) => state.set(instruction.op0_register(), Value::Imm(value)),
            None => state.forget(instruction.op0_register()),
        },
        _ => {}
    }
}

fn store(instruction: &Instruction, value: Option<Value>, state: &mut State, flow: &mut Flow) {
    match state.base(instruction) {
        MemoryBase::Argument(base) => {
            if let Some(argument) = base.argument() {
                flow.derefs[argument as usize] += 1;
                flow.stores_through[argument as usize] += 1;
                if instruction.memory_size().size() == 4 {
                    flow.stores_u32_through[argument as usize] += 1;
                }
            }
        }
        MemoryBase::Stack(key) => {
            // Only values the walk can still name are worth remembering; an
            // unknown spill must not resurrect a stale name on reload.
            if let Some(value) = value {
                if (0..=SHADOW_SPACE).contains(&key) || key < 0 {
                    state.stack.insert(key, value);
                }
            }
        }
        MemoryBase::Unknown => {}
    }
}

fn lea(instruction: &Instruction, state: &mut State) {
    if instruction.op0_kind() != OpKind::Register || instruction.op1_kind() != OpKind::Memory {
        return;
    }
    let displacement = instruction.memory_displacement64() as i64;
    let value = match state.base(instruction) {
        MemoryBase::Argument(base) => base.argument().map(|arg| Value::Into {
            arg,
            disp: displacement,
        }),
        _ => None,
    };
    match value {
        Some(value) => state.set(instruction.op0_register(), value),
        None => state.forget(instruction.op0_register()),
    }
}
