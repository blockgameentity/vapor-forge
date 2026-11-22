//! Where an MS-x64 function's argument registers end up.
//!
//! `crate::msabi` answers the first question about an argument ("dereferenced?
//! copied? spilled?") and `crate::arg_flow` answers the deeper one, but
//! `arg_flow` seeds the *System V* register order, which is the wrong order for
//! a PE image. This module is the same idea for the Microsoft x64 convention —
//! `rcx, rdx, r8, r9`, in that order — narrowed to the facts a slot ranking
//! needs:
//!
//! * which argument each store in the body writes through (`mov [r8+8], eax`
//!   is a store through argument 2, and so is `mov rax, [r8]` followed by
//!   `mov [rax], ecx`, because the buffer's payload pointer came from it);
//! * which argument each call inside the walk was handed;
//! * whether an argument is consumed as a 32-bit or a 64-bit value;
//! * what `al` held at a `ret`, which is how a `bool` return becomes visible.
//!
//! The walk is linear from the entry, follows register copies, `lea` address
//! computations and home-slot spills, and stops at the first return or
//! unconditional jump. It describes the straight-line prefix, not every path,
//! so every fact here is evidence and never proof — the same limit
//! `arg_flow` documents.

mod ops;
#[cfg(test)]
mod tests;
mod walk;

pub use walk::analyse;

use iced_x86::Register;

/// The MS-x64 integer argument registers, in order.
pub const ARGUMENTS: [Register; 4] = [Register::RCX, Register::RDX, Register::R8, Register::R9];
/// Home-area slots are addressed from the entry `rsp`.
pub(crate) const SHADOW_SPACE: i64 = 0x20;
pub(crate) const MAX_INSNS: u32 = 20_000;
/// Registers a call is allowed to destroy under the MS-x64 convention.
pub(crate) const CALLER_SAVED: [Register; 7] = [
    Register::RAX,
    Register::RCX,
    Register::RDX,
    Register::R8,
    Register::R9,
    Register::R10,
    Register::R11,
];

/// What a register or stack slot holds, as far as the walk can tell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Value {
    /// The traced function's own argument, by position.
    Arg(u8),
    /// `lea r, [argN + displacement]`.
    Into { arg: u8, disp: i64 },
    /// Loaded from memory reached through an argument: a pointer the argument
    /// itself points at, e.g. the payload pointer inside a `CUtlBuffer`.
    Indirect(u8),
    /// A constant the function materialised.
    Imm(i64),
}

impl Value {
    /// The argument this value derives from, if any.
    pub fn argument(self) -> Option<u8> {
        match self {
            Self::Arg(position) | Self::Indirect(position) => Some(position),
            Self::Into { arg, .. } => Some(arg),
            Self::Imm(_) => None,
        }
    }

    /// Short name for the report line.
    pub fn summary(self) -> String {
        match self {
            Self::Arg(position) => format!("arg{position}"),
            Self::Into { arg, disp } => format!("arg{arg}+{disp:#x}"),
            Self::Indirect(position) => format!("[arg{position}]"),
            Self::Imm(imm) => format!("{imm}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AlSet {
    Imm(i64),
    /// `setcc al`: a `bool` produced from a comparison.
    Flag,
}

/// The facts one linear walk produces.
#[derive(Clone, Debug, Default)]
pub struct Flow {
    /// Stores whose address derives from argument N.
    pub stores_through: [u32; 4],
    /// Of those, the 4-byte ones.
    pub stores_u32_through: [u32; 4],
    /// Memory operands whose address derives from argument N, reads included.
    pub derefs: [u32; 4],
    /// Calls that were handed the value of argument N.
    pub forwarded: [u32; 4],
    /// Reads of argument N as a 32-bit / a 64-bit value.
    pub arg32_reads: [u32; 4],
    pub arg64_reads: [u32; 4],
    /// Calls seen in the walk.
    pub calls: u32,
    /// `rax` at the first `ret`.
    pub returned: Option<Value>,
    /// `ret` sites where `al` was a constant or a `setcc` result.
    pub bool_rets: u32,
    /// `ret` sites reached by the walk.
    pub rets: u32,
}

impl Flow {
    /// True when the body hands argument `position` to a callee.
    pub fn forwards(&self, position: usize) -> bool {
        self.forwarded[position] > 0
    }

    /// True when the body stores through argument `position`'s pointer.
    pub fn stores_into(&self, position: usize) -> bool {
        self.stores_through[position] > 0
    }

    /// Compact evidence tokens, e.g. `thru 2:3 u32 2:2 fwd 2:1 deref 2:4
    /// arg32 1:2 calls 5 ret 2+1b`.
    pub fn summary(&self) -> String {
        let positions = |counts: &[u32; 4]| -> String {
            counts
                .iter()
                .enumerate()
                .filter(|(_, count)| **count > 0)
                .map(|(index, count)| format!("{index}:{count}"))
                .collect::<Vec<_>>()
                .join(",")
        };
        let mut tokens: Vec<String> = Vec::new();
        let thru = positions(&self.stores_through);
        if !thru.is_empty() {
            tokens.push(format!("thru {thru}"));
            let narrow = positions(&self.stores_u32_through);
            if !narrow.is_empty() {
                tokens.push(format!("u32thru {narrow}"));
            }
        }
        let forwarded = positions(&self.forwarded);
        if !forwarded.is_empty() {
            tokens.push(format!("fwd {forwarded}"));
        }
        let derefs = positions(&self.derefs);
        if !derefs.is_empty() {
            tokens.push(format!("deref {derefs}"));
        }
        let narrow = positions(&self.arg32_reads);
        if !narrow.is_empty() {
            tokens.push(format!("arg32 {narrow}"));
        }
        tokens.push(format!("calls {}", self.calls));
        tokens.push(format!("ret {}+{}b", self.rets, self.bool_rets));
        tokens.join(" ")
    }
}
