//! Generic detour and VMT hooking mechanics: pattern-resolved trampoline
//! creation, PIC repair, trampoline page protection, and vtable slot swapping.
//!
//! This crate carries no knowledge of Steam's classes, interfaces, or modules;
//! callers supply opaque installation plans validated against their target
//! module's executable range.
//!
//! # Targets
//!
//! The detour lifecycle is the same on both targets — plan, create, enable,
//! restore — but the bytes and the backend differ:
//!
//! * unix — `detour::unix` builds `retour` detours. `retour` relocates the
//!   prologue (RIP-relative operands, relative branches) and owns its own
//!   trampoline memory; the engine adds PIC-thunk repair for 32-bit ELF and a
//!   trampoline hardening pass.
//! * windows — `writer` plans an instruction-boundary patch, allocates the
//!   trampoline in this process, writes the jump through [`protect`]
//!   (`VirtualProtect`) and flushes the instruction cache
//!   (`FlushInstructionCache`). An entry the planner cannot describe is refused
//!   instead of patched.

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]

pub mod detour;
// Executable trampoline memory and the writer that uses it exist where the
// engine installs its own patches (Windows) and under test, where a fake memory
// backend drives the same state machine on any host. The unix build hands the
// whole cycle to `retour` instead.
#[cfg(any(windows, test))]
pub mod exec;
pub mod original;
pub mod patch;
pub(crate) mod pic_repair;
pub(crate) mod pic_thunk;
pub mod plan;
pub mod protect;
pub mod vmt;
#[cfg(windows)]
pub(crate) mod win_api;
#[cfg(any(windows, test))]
pub mod writer;
