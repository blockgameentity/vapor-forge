//! Which registers still hold an incoming argument pointer, and which home slots
//! do. MS-x64 prologues spill arguments to `[rsp+N]` and reload them, so
//! following the home area matters: a body that reloads `rdx` from `[rsp+0x10]`
//! would otherwise lose the pointer.

use std::collections::HashMap;

use iced_x86::{Instruction, InstructionInfoFactory, Mnemonic, OpAccess, OpKind, Register};

use crate::msabi::{self, adjust_stack_pointer};

use super::VOLATILE;

pub(super) struct State {
    /// Register -> argument position.
    registers: HashMap<Register, u8>,
    /// Home-slot offset (relative to the entry stack pointer) -> argument.
    home: HashMap<i64, u8>,
    sp: Option<i64>,
}

impl State {
    pub(super) fn entry() -> Self {
        let mut registers = HashMap::new();
        for (position, register) in msabi::MSABI_ARGUMENTS.iter().enumerate() {
            registers.insert(*register, position as u8);
        }
        Self {
            registers,
            home: HashMap::new(),
            sp: Some(0),
        }
    }

    /// `rsp` relative to its value at entry, while it is still known.
    pub(super) fn stack_pointer(&self) -> Option<i64> {
        self.sp
    }

    pub(super) fn provenance(&self, register: Register) -> Option<u8> {
        if register == Register::None || register == Register::RIP {
            return None;
        }
        self.registers.get(&register.full_register()).copied()
    }

    pub(super) fn clobber(&mut self) {
        for register in VOLATILE {
            self.registers.remove(&register);
        }
        self.registers.remove(&Register::RAX);
    }

    pub(super) fn forget(&mut self) {
        self.registers.clear();
        self.home.clear();
    }

    pub(super) fn step(&mut self, instruction: &Instruction, factory: &mut InstructionInfoFactory) {
        self.sp = adjust_stack_pointer(instruction, self.sp);
        let sp = self.sp;
        if instruction.mnemonic() == Mnemonic::Mov {
            match (instruction.op0_kind(), instruction.op1_kind()) {
                (OpKind::Register, OpKind::Register) => {
                    let source = self.provenance(instruction.op1_register());
                    self.set(instruction.op0_register(), source);
                    return;
                }
                // `mov reg, [rsp+N]` reloads a spilled argument.
                (OpKind::Register, OpKind::Memory)
                    if instruction.memory_base().full_register() == Register::RSP =>
                {
                    let value = sp.and_then(|sp| {
                        let offset = instruction.memory_displacement64() as i64 - sp;
                        self.home.get(&offset).copied()
                    });
                    self.set(instruction.op0_register(), value);
                    return;
                }
                (OpKind::Memory, _)
                    if instruction.memory_base().full_register() == Register::RSP =>
                {
                    // A store into the home area: record what went in. Only a
                    // register source is tracked; an immediate is not a pointer.
                    if let Some(sp) = sp {
                        let offset = instruction.memory_displacement64() as i64 - sp;
                        let value = (instruction.op1_kind() == OpKind::Register)
                            .then(|| self.provenance(instruction.op1_register()))
                            .flatten();
                        match value {
                            Some(arg) => {
                                self.home.insert(offset, arg);
                            }
                            None => {
                                self.home.remove(&offset);
                            }
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
        if instruction.mnemonic() == Mnemonic::Lea && instruction.op1_kind() == OpKind::Memory {
            let base = self.provenance(instruction.memory_base().full_register());
            self.set(instruction.op0_register(), base);
            return;
        }
        // Pointer arithmetic keeps the provenance; anything else forgets it.
        if matches!(
            instruction.mnemonic(),
            Mnemonic::Add | Mnemonic::Sub | Mnemonic::Inc | Mnemonic::Dec
        ) && instruction.op0_kind() == OpKind::Register
        {
            return;
        }
        let written = factory
            .info(instruction)
            .used_registers()
            .iter()
            .filter(|used| is_write(used.access()))
            .map(|used| used.register().full_register())
            .collect::<Vec<_>>();
        for register in written {
            self.set_register(register, None);
        }
    }

    fn set(&mut self, register: Register, value: Option<u8>) {
        self.set_register(register.full_register(), value);
    }

    fn set_register(&mut self, register: Register, value: Option<u8>) {
        if register == Register::RSP {
            return;
        }
        match value {
            Some(value) => {
                self.registers.insert(register, value);
            }
            None => {
                self.registers.remove(&register);
            }
        }
    }
}

pub(super) fn is_read(access: OpAccess) -> bool {
    matches!(
        access,
        OpAccess::Read | OpAccess::CondRead | OpAccess::ReadWrite | OpAccess::ReadCondWrite
    )
}

pub(super) fn is_write(access: OpAccess) -> bool {
    matches!(
        access,
        OpAccess::Write | OpAccess::ReadWrite | OpAccess::CondWrite | OpAccess::ReadCondWrite
    )
}
