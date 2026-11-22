//! The register and stack model one basic block is walked with.
//!
//! Only two questions are asked of it: does a register still hold the job
//! object (`this`) itself, and which pointer member has just been loaded from
//! it. Every write the walk does not model forgets the register it writes, so a
//! stale answer cannot survive to anchor the shape.

use iced_x86::{Instruction, OpKind, Register};

use super::BODY_WINDOW;
use crate::arg_flow::Abi;
use crate::msabi;

/// The registers a System V call may destroy; an MS-x64 call keeps `rsi`/`rdi`.
pub(super) const SYSV_VOLATILE: [Register; 9] = [
    Register::RAX,
    Register::RCX,
    Register::RDX,
    Register::RSI,
    Register::RDI,
    Register::R8,
    Register::R9,
    Register::R10,
    Register::R11,
];

/// A pointer load from `this` whose body `lea` has not been seen yet.
#[derive(Clone, Copy)]
struct Load {
    register: Register,
    field: i64,
    ip: u64,
}

/// The registers and stack position a basic block starts with.
#[derive(Clone)]
pub(super) struct State {
    bitness: u32,
    abi: Abi,
    /// Registers known to hold `this` itself, never a pointer derived from it.
    this: Vec<Register>,
    /// Stack pointer relative to its value at entry, while it is known.
    sp: Option<i64>,
    /// Frame pointer relative to the entry stack pointer, while it holds one.
    fp: Option<i64>,
    /// The entry stack slot of a 32-bit `this`, if the entry takes one.
    stack_this: Option<i64>,
    /// Reply loads whose body `lea` has not been seen yet. A function reads
    /// other members of the job between the reply load and the body `lea`, so
    /// more than one pointer can be pending.
    loads: Vec<Load>,
    /// A reply chain matched, but its `lea` sat past [`BODY_WINDOW`].
    pub(super) late: bool,
}

impl State {
    pub(super) fn entry(abi: Abi, bitness: u32) -> Self {
        let (this, stack_this) = match (bitness, abi) {
            (64, Abi::MsX64) => (vec![Register::RCX], None),
            (64, _) => (vec![Register::RDI], None),
            // cdecl: the receiver is the caller's first stacked argument, and
            // the return address sits at the entry stack pointer.
            _ => (Vec::new(), Some(4)),
        };
        Self {
            bitness,
            abi,
            this,
            sp: Some(0),
            fp: None,
            stack_this,
            loads: Vec::new(),
            late: false,
        }
    }

    /// Would the call's first argument carry `this`?
    pub(super) fn receiver(&self) -> bool {
        if self.bitness != 64 {
            // A cdecl receiver is a stack slot whose value the callee re-reads
            // through its own frame; only the entry shape is offered for 32-bit.
            return false;
        }
        let argument = match self.abi {
            Abi::MsX64 => msabi::MSABI_ARGUMENTS[0],
            Abi::SysV => Register::RDI,
        };
        self.this.contains(&argument)
    }

    pub(super) fn holds(&self, register: Register) -> bool {
        self.this.contains(&register)
    }

    /// A stack adjustment the walk cannot follow: nothing stack-relative is
    /// trusted from here on.
    pub(super) fn unknown_stack(&mut self) {
        self.sp = None;
    }

    /// Record that `register` now holds `this` (`carries`) or something else.
    ///
    /// Either way a pending reply load in that register is gone: the pointer it
    /// named was overwritten.
    pub(super) fn set(&mut self, register: Register, carries: bool) {
        self.this.retain(|known| *known != register);
        if carries {
            self.this.push(register);
        }
        self.loads.retain(|load| load.register != register);
    }

    /// Note a pointer member load into `register`.
    pub(super) fn loaded(&mut self, register: Register, field: i64, ip: u64) {
        self.loads.push(Load {
            register,
            field,
            ip,
        });
    }

    /// A call destroyed every volatile register and the loads they carried.
    pub(super) fn clobber(&mut self, volatile: &[Register]) {
        self.this.retain(|register| !volatile.contains(register));
        self.loads.retain(|load| !volatile.contains(&load.register));
    }

    /// `mov rbp, rsp`: the frame pointer now marks the current stack position.
    pub(super) fn frame_from_stack(&mut self) {
        self.fp = self.sp;
    }

    /// `mov rsp, rbp`: the stack pointer is restored from the frame pointer.
    pub(super) fn stack_from_frame(&mut self) {
        self.sp = self.fp;
    }

    /// `push` (`increment` is the stack pointer's negative change) and `pop`.
    pub(super) fn shift_stack(&mut self, delta: i64) {
        self.sp = self.sp.map(|sp| sp + delta);
    }

    pub(super) fn stack_this(&self) -> Option<i64> {
        self.stack_this
    }

    /// The entry-relative stack slot a memory operand reads or writes.
    pub(super) fn stack_slot(&self, instruction: &Instruction) -> Option<i64> {
        if instruction.memory_index() != Register::None {
            return None;
        }
        let base = instruction.memory_base().full_register();
        let displacement = displacement(instruction, self.bitness);
        if base == Register::RSP {
            return self.sp.map(|sp| sp + displacement);
        }
        if base == Register::RBP {
            return self.fp.map(|fp| fp + displacement);
        }
        None
    }

    /// The reply field this load reads, when it reads a pointer member of `this`.
    pub(super) fn field(&self, instruction: &Instruction) -> Option<i64> {
        let base = instruction.memory_base().full_register();
        if instruction.memory_index() != Register::None
            || matches!(instruction.memory_segment(), Register::FS | Register::GS)
            || !self.holds(base)
        {
            return None;
        }
        let width = instruction.memory_size().size();
        (width == self.bitness as usize / 8).then(|| displacement(instruction, self.bitness))
    }

    /// The body displacement of a pending load, when this `lea` consumes it.
    pub(super) fn body(&mut self, instruction: &Instruction) -> Option<(i64, i64)> {
        let base = instruction.memory_base().full_register();
        if instruction.memory_index() != Register::None
            || matches!(instruction.memory_segment(), Register::FS | Register::GS)
        {
            return None;
        }
        let index = self.loads.iter().position(|load| load.register == base)?;
        let destination = instruction.op0_register().full_register();
        if destination != Register::RDI && destination != Register::RSI {
            return None;
        }
        let load = self.loads.remove(index);
        let gap = i64::try_from(instruction.ip().checked_sub(load.ip)?).ok()?;
        if gap > BODY_WINDOW {
            self.late = true;
            return None;
        }
        let body = displacement(instruction, self.bitness);
        (body > 0).then_some((load.field, body))
    }
}

pub(super) fn displacement(instruction: &Instruction, bitness: u32) -> i64 {
    if bitness == 64 {
        instruction.memory_displacement64() as i64
    } else {
        i64::from(instruction.memory_displacement32() as i32)
    }
}

pub(super) fn immediate(instruction: &Instruction, operand: u32) -> Option<i64> {
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
