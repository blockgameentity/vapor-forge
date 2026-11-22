//! The traversal that finds the reply shape.
//!
//! A job entry is not one straight line: the parser call sits on the arm after
//! an early `jmp`, so a linear walk never sees it. The walk here decodes basic
//! blocks from a worklist, carries the [`State`] across conditional branches,
//! and follows the direct calls that are handed `this` as their receiver.

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Mnemonic, OpAccess,
    OpKind, Register,
};

use super::state::{immediate, State, SYSV_VOLATILE};
use super::{BUDGET, MAX_BLOCKS};
use crate::arg_flow::Abi;
use crate::msabi;
use crate::scan::CodeSegment;

/// What one function's walk found.
pub(super) struct Search {
    /// `(reply field, body displacement)`, once per candidate site.
    pub pairs: Vec<(i64, i64)>,
    /// Offsets of direct calls that were handed `this`.
    pub callees: Vec<usize>,
    /// A chain matched, but its `lea` sat past the body window.
    pub late: bool,
}

/// Walk the function at `offset` and return everything the shape search needs.
pub(super) fn search(
    segment: &CodeSegment<'_>,
    offset: usize,
    abi: Abi,
    window: usize,
) -> Result<Search, &'static str> {
    let tail = segment
        .bytes
        .get(offset..)
        .ok_or("entry outside the code segment")?;
    let start = segment
        .vaddr
        .checked_add(offset as u64)
        .ok_or("entry outside the code segment")?;
    let end = start + tail.len().min(window) as u64;
    let bitness = u32::from(segment.bits);
    let mut machine = Machine {
        bytes: segment.bytes,
        vaddr: segment.vaddr,
        bitness,
        abi,
        start,
        end,
        budget: BUDGET,
        blocks: vec![(start, State::entry(abi, bitness))],
        visited: Vec::new(),
        search: Search {
            pairs: Vec::new(),
            callees: Vec::new(),
            late: false,
        },
    };
    machine.run();
    Ok(machine.search)
}

struct Machine<'a> {
    bytes: &'a [u8],
    vaddr: u64,
    bitness: u32,
    abi: Abi,
    /// First address of the entry being walked.
    start: u64,
    /// One past the last address this function's walk may decode.
    end: u64,
    budget: usize,
    blocks: Vec<(u64, State)>,
    visited: Vec<u64>,
    search: Search,
}

impl Machine<'_> {
    fn run(&mut self) {
        while let Some((start, state)) = self.blocks.pop() {
            self.block(start, state);
        }
    }

    fn block(&mut self, start: u64, mut state: State) {
        if self.visited.len() >= MAX_BLOCKS || self.visited.contains(&start) || start >= self.end {
            return;
        }
        self.visited.push(start);
        let bytes = self.bytes;
        let Some(from) = start
            .checked_sub(self.vaddr)
            .and_then(|at| usize::try_from(at).ok())
        else {
            return;
        };
        let Some(tail) = bytes.get(from..) else {
            return;
        };
        let len = (self.end - start) as usize;
        let mut decoder = Decoder::with_ip(
            self.bitness,
            &tail[..tail.len().min(len)],
            start,
            DecoderOptions::NONE,
        );
        let mut factory = InstructionInfoFactory::new();
        while decoder.can_decode() {
            if self.budget == 0 {
                return;
            }
            self.budget -= 1;
            let instruction = decoder.decode();
            if instruction.is_invalid() {
                return;
            }
            match instruction.flow_control() {
                FlowControl::Return | FlowControl::IndirectBranch => return,
                FlowControl::UnconditionalBranch => {
                    self.branch(&instruction, state);
                    return;
                }
                FlowControl::ConditionalBranch => self.branch(&instruction, state.clone()),
                FlowControl::Call | FlowControl::IndirectCall => {
                    self.call(&instruction, &mut state);
                    continue;
                }
                _ => {}
            }
            self.step(&instruction, &mut state, &mut factory);
        }
    }

    /// Queue a branch target inside the window the walk may decode.
    fn branch(&mut self, instruction: &Instruction, state: State) {
        let target = instruction.near_branch_target();
        let relative = matches!(
            instruction.op0_kind(),
            OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
        );
        if relative && (self.start..self.end).contains(&target) {
            self.blocks.push((target, state));
        }
    }

    fn call(&mut self, instruction: &Instruction, state: &mut State) {
        let direct = matches!(
            instruction.op0_kind(),
            OpKind::NearBranch16 | OpKind::NearBranch32 | OpKind::NearBranch64
        );
        if direct && state.receiver() {
            if let Some(offset) = instruction
                .near_branch_target()
                .checked_sub(self.vaddr)
                .and_then(|at| usize::try_from(at).ok())
            {
                if !self.search.callees.contains(&offset) && offset < self.bytes.len() {
                    self.search.callees.push(offset);
                }
            }
        }
        // The call destroys `this` if it lived in a volatile register, and the
        // pending reply pointers it carried with it.
        let volatile: &[Register] = match (self.bitness, self.abi) {
            (64, Abi::MsX64) => &msabi::VOLATILE,
            (64, _) => &SYSV_VOLATILE,
            _ => &[Register::RAX, Register::RCX, Register::RDX],
        };
        state.clobber(volatile);
    }

    fn step(
        &mut self,
        instruction: &Instruction,
        state: &mut State,
        factory: &mut InstructionInfoFactory,
    ) {
        let op0 = instruction.op0_kind();
        match instruction.mnemonic() {
            Mnemonic::Mov if op0 == OpKind::Register => {
                let destination = instruction.op0_register().full_register();
                match instruction.op1_kind() {
                    OpKind::Register => {
                        let source = instruction.op1_register().full_register();
                        if destination == Register::RBP && source == Register::RSP {
                            state.frame_from_stack();
                        } else if destination == Register::RSP && source == Register::RBP {
                            state.stack_from_frame();
                        }
                        state.set(destination, state.holds(source));
                    }
                    OpKind::Memory => {
                        let field = state.field(instruction);
                        let stacked = state.stack_slot(instruction) == state.stack_this()
                            && instruction.memory_size().size() == 4;
                        state.set(destination, stacked);
                        if let Some(field) = field {
                            state.loaded(destination, field, instruction.ip());
                        }
                    }
                    _ => state.set(destination, false),
                }
            }
            Mnemonic::Lea if op0 == OpKind::Register => {
                let destination = instruction.op0_register().full_register();
                if let Some(pair) = state.body(instruction) {
                    self.search.pairs.push(pair);
                }
                // A pointer derived from `this` is not `this` itself, and a
                // register that held `this` stops holding it once written.
                state.set(destination, false);
            }
            Mnemonic::Push => state.shift_stack(i64::from(instruction.stack_pointer_increment())),
            Mnemonic::Pop => {
                state.shift_stack(-i64::from(instruction.stack_pointer_increment()));
                if op0 == OpKind::Register {
                    state.set(instruction.op0_register().full_register(), false);
                }
            }
            Mnemonic::Sub | Mnemonic::Add
                if op0 == OpKind::Register
                    && instruction.op0_register() == self.stack_pointer() =>
            {
                match immediate(instruction, 1) {
                    Some(delta) if instruction.mnemonic() == Mnemonic::Sub => {
                        state.shift_stack(-delta)
                    }
                    Some(delta) => state.shift_stack(delta),
                    None => state.unknown_stack(),
                }
            }
            _ => {
                for register in written(instruction, factory) {
                    state.set(register, false);
                }
            }
        }
        self.search.late |= state.late;
    }

    fn stack_pointer(&self) -> Register {
        if self.bitness == 64 {
            Register::RSP
        } else {
            Register::ESP
        }
    }
}

/// The registers an instruction writes.
fn written(instruction: &Instruction, factory: &mut InstructionInfoFactory) -> Vec<Register> {
    factory
        .info(instruction)
        .used_registers()
        .iter()
        .filter(|used| {
            matches!(
                used.access(),
                OpAccess::Write
                    | OpAccess::ReadWrite
                    | OpAccess::CondWrite
                    | OpAccess::ReadCondWrite
            )
        })
        .map(|used| used.register().full_register())
        .collect()
}
