//! The MS-x64 side of argument analysis.
//!
//! [`crate::arg_flow`] follows arguments in System V order (`rdi, rsi, rdx,
//! rcx, r8, r9`) because the shipped Linux client is SysV. The Windows client is
//! MS-x64 instead: integer arguments arrive in `rcx, rdx, r8, r9`, the caller
//! owns 32 bytes of shadow space above the return address, and there is no frame
//! chain unless the function builds one.
//!
//! For a *candidate generator* the useful primitive is narrower than a full
//! dataflow: which of the four argument registers the function actually reads,
//! and what the first use of each one is. A function that reads `rdx` as a
//! memory base in its first few instructions is taking a pointer argument; one
//! that overwrites `r9` before reading it does not take a fourth argument at
//! all. That distinction is what separates a four-argument writer from a
//! three-argument sibling with the same prologue, and it survives rebuilds that
//! renumber the instructions.
//!
//! The walk is linear from the entry and stops after `window` bytes or at the
//! first `ret`, whichever comes first, so it describes the straight-line path
//! only. Callers must treat every fact as evidence, never as proof: a use found
//! later on a branch is not seen, and an argument consumed only by a callee (a
//! register handed to `call` unchanged) is still a read here.

use iced_x86::Register;

mod analysis;

pub use analysis::{adjust_stack_pointer, analyse};

/// MS-x64 integer argument registers, in order.
pub const MSABI_ARGUMENTS: [Register; 4] =
    [Register::RCX, Register::RDX, Register::R8, Register::R9];
/// Shadow space / home area the caller owns above the return address.
pub const SHADOW_SPACE: i64 = 0x20;
/// Offset, from the entry stack pointer, of the first home slot: the return
/// address sits at 0, so the four register arguments' copies start at 8.
pub const HOME_SLOT: i64 = 8;
/// Offset, from the entry stack pointer, of the fifth argument, the first one
/// the caller pushes. The callee's shadow space lies between it and the home
/// slots, so it is `SHADOW_SPACE + 8`, not `SHADOW_SPACE`.
pub const STACK_ARGUMENT: i64 = SHADOW_SPACE + 8;

/// Offset, from the entry stack pointer, of argument `position` on the stack,
/// for the positions the four registers do not cover.
pub fn stack_argument_offset(position: u8) -> Option<i64> {
    let index = i64::from(position.checked_sub(MSABI_ARGUMENTS.len() as u8)?);
    Some(STACK_ARGUMENT + 8 * index)
}

/// Registers a call may destroy: `rax, rcx, rdx, r8, r9, r10, r11`.
///
/// `rsi` and `rdi` are callee-saved here, which is the one place this differs
/// from the System V list in [`crate::arg_flow`] — a value parked in `rsi`
/// survives the `DCHECK` calls an argument-tracking walk crosses.
pub const VOLATILE: [Register; 7] = [
    Register::RAX,
    Register::RCX,
    Register::RDX,
    Register::R8,
    Register::R9,
    Register::R10,
    Register::R11,
];
/// Bytes decoded from an entry by default. Argument reads sit in the opening
/// checks and prologue spills, well inside this.
pub const WINDOW: usize = 0x80;

/// What a slot's incoming value was first used for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FirstUse {
    /// Consumed as a register operand (`test rcx, rcx`, `add rcx, r8`).
    Operand,
    /// Used as the base or index of a memory operand (`mov eax, [rdx]`).
    MemoryAddress,
    /// Copied into another register (`mov rbx, rcx`).
    CopiedTo(Register),
    /// Spilled into its caller-provided home slot, at this offset from the
    /// entry stack pointer. Offsets `0..0x20` are the argument's own home slot;
    /// anything else is an ordinary local spill.
    HomeSlot(i64),
    /// Overwritten without the incoming value ever being read.
    Overwritten,
}

/// One argument register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Slot {
    /// 0 = `rcx`, 1 = `rdx`, 2 = `r8`, 3 = `r9`.
    pub position: u8,
    pub register: Register,
    /// Offset, into the analysed code, of the first instruction that consumes
    /// the incoming value.
    pub first_read: Option<usize>,
    /// Offset of the first instruction that overwrites the register without
    /// reading it.
    pub first_write: Option<usize>,
    /// What the first consuming instruction did with it.
    pub first_use: Option<FirstUse>,
}

impl Slot {
    /// True when the function consumes the value its caller passed here.
    pub fn is_used(&self) -> bool {
        self.first_read.is_some()
    }

    /// True when the incoming value reaches a use before anything overwrites it.
    ///
    /// A slot that is never touched is *not* used: an untouched register says
    /// nothing about the arity, so it is reported separately from one that is
    /// read. This is deliberately strict; the summary is
    /// [`Report::argument_count`].
    pub fn read_before_write(&self) -> bool {
        match (self.first_read, self.first_write) {
            (Some(read), Some(write)) => read <= write,
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// The spill offset when the first use put the value in the home area.
    pub fn home_slot(&self) -> Option<i64> {
        match self.first_use {
            Some(FirstUse::HomeSlot(offset)) if (0..SHADOW_SPACE).contains(&offset) => Some(offset),
            _ => None,
        }
    }
}

/// Per-function argument summary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Report {
    pub slots: [Slot; 4],
}

impl Default for Report {
    /// An untouched entry state: every slot present, nothing used yet.
    fn default() -> Self {
        Self::entry()
    }
}

impl Report {
    fn entry() -> Self {
        let mut slots = [Slot {
            position: 0,
            register: Register::None,
            first_read: None,
            first_write: None,
            first_use: None,
        }; 4];
        for (position, slot) in slots.iter_mut().enumerate() {
            *slot = Slot {
                position: position as u8,
                register: MSABI_ARGUMENTS[position],
                first_read: None,
                first_write: None,
                first_use: None,
            };
        }
        Self { slots }
    }

    pub fn slot(&self, position: u8) -> Option<&Slot> {
        self.slots.get(usize::from(position))
    }

    /// `[used, ...]` for `rcx, rdx, r8, r9`, in that order.
    pub fn used_mask(&self) -> [bool; 4] {
        [
            self.slots[0].is_used(),
            self.slots[1].is_used(),
            self.slots[2].is_used(),
            self.slots[3].is_used(),
        ]
    }

    /// One past the highest slot whose incoming value is read before being
    /// overwritten, so a function that reads `rdx` but not `r8` reports 2.
    ///
    /// An argument a function never passes is read from a stale register, so
    /// only the highest read slot bounds the arity: a body that reads `rcx`,
    /// `rdx` and `r9` is passed four arguments, not three.
    pub fn argument_count(&self) -> u8 {
        self.slots
            .iter()
            .filter(|slot| slot.read_before_write())
            .map(|slot| slot.position + 1)
            .max()
            .unwrap_or(0)
    }

    /// Slots read before being written, ascending.
    pub fn arguments_read(&self) -> Vec<u8> {
        self.slots
            .iter()
            .filter(|slot| slot.read_before_write())
            .map(|slot| slot.position)
            .collect()
    }

    /// Compact rendering for evidence lines, one token per slot:
    /// `rcx=ptr@0x4 rdx=home[0x8]@0x0 r8=- r9=written@0x12`.
    pub fn summary(&self) -> String {
        let parts = self.slots.iter().map(describe_slot).collect::<Vec<_>>();
        parts.join(" ")
    }
}

fn describe_slot(slot: &Slot) -> String {
    let name = register_name(slot.register);
    match (slot.first_use, slot.first_read, slot.first_write) {
        (None, _, _) => format!("{name}=-"),
        (Some(FirstUse::Overwritten), _, write) => format!("{name}=written@{}", int(write)),
        (Some(FirstUse::Operand), read, _) => format!("{name}=read@{}", int(read)),
        (Some(FirstUse::MemoryAddress), read, _) => format!("{name}=ptr@{}", int(read)),
        (Some(FirstUse::CopiedTo(register)), read, _) => {
            format!("{name}=copy:{register:?}@{}", int(read))
        }
        (Some(FirstUse::HomeSlot(offset)), read, _) => {
            format!("{name}=home[{offset:#x}]@{}", int(read))
        }
    }
}

fn int(value: Option<usize>) -> String {
    match value {
        Some(value) => format!("{value:#x}"),
        None => "-".to_owned(),
    }
}

fn register_name(register: Register) -> &'static str {
    match register {
        Register::RCX => "rcx",
        Register::RDX => "rdx",
        Register::R8 => "r8",
        Register::R9 => "r9",
        _ => "?",
    }
}

#[cfg(test)]
mod tests;
