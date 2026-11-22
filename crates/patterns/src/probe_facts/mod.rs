//! Behavioural facts about one Windows x64 function.
//!
//! A mined signature proves *identity* (this is byte-for-byte that function) but
//! says nothing about what a function does, so it cannot derive a symbol that
//! was never published as a hint. These facts are the other half: raw
//! measurements of behaviour that a human can read and either confirm or refute.
//!
//! Nothing here decides what a function *is*. The counts are cheap, uniform and
//! printed verbatim so a candidate that scores badly but looks right in the
//! evidence can still be believed, and one that scores well can be dismissed.
//!
//! Measurements are taken from a single linear decode of the function body
//! (`.pdata` gives the exact extent). Two consequences, both deliberate:
//!
//! * control flow is ignored for counting - a store counted twice by an unrolled
//!   loop counts twice, as it would at run time;
//! * pointer provenance is dropped at any unconditional branch, because the
//!   register state of a block reached only by a jump is not modelled. A store
//!   through `rdx` after such a jump is therefore *not* attributed to the second
//!   argument even if it came from there.

use std::collections::{BTreeSet, HashMap};

use iced_x86::{
    Decoder, DecoderOptions, FlowControl, Instruction, InstructionInfoFactory, Register,
};

use crate::msabi;
use crate::pe_image::PeImage;

/// Hard stop for pathological input; real functions here are far smaller.
const MAX_INSNS: u32 = 40_000;
/// Caller-saved under MS-x64: clobbered by every call, so no longer an argument.
const VOLATILE: [Register; 4] = [Register::RCX, Register::RDX, Register::R8, Register::R9];
/// `'='`, as stored when a `KEY=VALUE` string is assembled in place.
pub const EQUALS: u8 = 0x3d;

#[derive(Clone, Debug, Default)]
pub struct Facts {
    pub begin: u32,
    pub end: u32,
    /// False when the `.pdata` extent is not mapped in this image.
    pub mapped: bool,
    pub insns: u32,
    pub args: msabi::Report,
    /// 32-bit stores, anywhere in the body.
    pub u32_stores: u32,
    /// 32-bit stores whose address base traces back to argument `i`. This is the
    /// "writes through the `out` pointer" measurement.
    pub u32_stores_via_arg: [u32; 4],
    /// 32-bit loads whose address base traces back to argument `i`.
    pub u32_loads_via_arg: [u32; 4],
    /// Stores of a 1-byte immediate.
    pub byte_imm_stores: u32,
    /// Stores whose immediate contains `0x3d`, i.e. writes a literal `'='`.
    pub equals_imm_stores: u32,
    /// Stores whose immediate is entirely printable ASCII (`0x20..=0x7e`).
    pub printable_imm_stores: u32,
    /// Distinct backward branch targets: rotated/iterating bodies.
    pub loops: u32,
    /// Stores (any width) whose address base traces back to an argument and
    /// which sit inside a backward-branch span.
    pub loop_stores_via_arg: u32,
    pub conditional_branches: u32,
    /// Distinct immediates compared against, over `cmp reg, imm`.
    pub cmp_imms: u32,
    /// Largest immediate compared against, `0` when there is no compare chain.
    pub cmp_imm_max: u64,
    /// An indexed indirect branch, i.e. a jump table held in memory.
    pub jump_table: bool,
    pub indirect_branches: u32,
    /// Indirect branches through a register, which is how MSVC reaches a
    /// *relative offset* table: `movsxd rax, [table+idx*4]` / `add rax, table`
    /// / `jmp rax`. No memory indirect branch is involved, so a jump table in
    /// this binary looks like this and not like `jmp [table+idx*8]`.
    pub register_branches: u32,
    /// Indexed loads (`[base + index*4|8]`), the table read of a switch.
    pub table_loads: u32,
    /// Reads of `[rsp+0x28..0x40]` relative to the entry stack pointer, i.e. the
    /// home slots of arguments 5 and 6. MS-x64 passes those on the stack, so a
    /// function that reads them takes more than four arguments.
    pub stack_argument_reads: u32,
    /// `lea reg, [reg+reg*8]` sites: the `index * 9` idiom a loop over
    /// 9-qword (0x48-byte) entries uses. `CUser::GetSubscribedApps` iterates
    /// licences that way on the Linux side, so the same stride shows up here if
    /// the Windows body kept the structure.
    pub stride9_leas: u32,
    /// Reads of the low byte of an argument register (`movzx eax, r9b`,
    /// `test r8b, r8b`): how a `bool` parameter is consumed.
    pub byte_argument_reads: u32,
    /// Multiply/divide instructions. A copy loop has none; a resampler, mixer or
    /// checksum is full of them.
    pub multiplies: u32,
    /// Distinct immediates compared against by the most-compared register.
    pub chain_len: u32,
    /// Largest immediate in that register's compare set.
    pub chain_max_imm: u64,
    pub calls: u32,
    pub indirect_calls: u32,
    /// Direct call targets, as RVAs, sorted and deduplicated.
    pub callees: Vec<u32>,
}

impl Facts {
    fn unmapped(begin: u32, end: u32) -> Self {
        Self {
            begin,
            end,
            mapped: false,
            ..Self::default()
        }
    }

    /// Total 32-bit stores attributable to any argument pointer.
    pub fn u32_stores_through_args(&self) -> u32 {
        self.u32_stores_via_arg.iter().sum()
    }

    /// Narrow compare chain: at least `least` values compared against one
    /// register, none above `limit`. A dispatch over an `EMsg` range looks like
    /// this; a length check or a flag mask does not, because it compares one
    /// register once or twice.
    pub fn narrow_compare_chain(&self, least: u32, limit: u64) -> bool {
        self.chain_len >= least && self.chain_max_imm <= limit
    }

    /// A branch that computes its target from a table: either an indexed
    /// memory indirect branch, or the MSVC relative-offset idiom (a table load
    /// plus a register branch).
    pub fn computed_branch(&self) -> bool {
        self.jump_table || (self.table_loads > 0 && self.indirect_branches > 0)
    }
}

/// Measure the function occupying `begin..end`, which is the exact extent
/// `.pdata` reports.
pub fn measure(image: &PeImage<'_>, begin: u32, end: u32) -> Facts {
    let len = end.saturating_sub(begin) as usize;
    match image.bytes_at_rva(u64::from(begin), len) {
        Some(code) => measure_code(code, begin, end),
        None => Facts::unmapped(begin, end),
    }
}

/// Measure a function body that starts at `begin` and must be shorter than
/// `end - begin`.
pub fn measure_code(code: &[u8], begin: u32, end: u32) -> Facts {
    let mut facts = Facts {
        begin,
        end,
        mapped: true,
        args: msabi::analyse(code, 0, msabi::WINDOW),
        ..Facts::default()
    };
    let mut factory = InstructionInfoFactory::new();
    let mut decoder = Decoder::with_ip(64, code, u64::from(begin), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut state = State::entry();
    let mut compared = BTreeSet::new();
    let mut chains: HashMap<Register, BTreeSet<u64>> = HashMap::new();
    let mut loops: BTreeSet<u64> = BTreeSet::new();
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let mut arg_stores: Vec<(u64, u8)> = Vec::new();

    while decoder.can_decode() && facts.insns < MAX_INSNS {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        facts.insns += 1;
        let ip = instruction.ip();

        if writes_memory(&instruction, &mut factory) {
            let width = instruction.memory_size().size();
            if width == 4 {
                facts.u32_stores += 1;
            }
            if let Some(arg) = state.provenance(instruction.memory_base().full_register()) {
                if width == 4 {
                    facts.u32_stores_via_arg[usize::from(arg)] += 1;
                }
                arg_stores.push((ip, arg));
            }
        }
        count_immediate_store(&instruction, &mut facts, &mut factory);
        count_compare(&instruction, &mut compared, &mut chains);
        count_table_load(&instruction, &mut facts, &mut factory);
        count_stack_argument(&instruction, &state, &mut facts, &mut factory);
        count_arithmetic(&instruction, &mut facts);
        count_stride_index(&instruction, &mut facts);
        count_byte_argument(&instruction, &mut facts, &mut factory);
        count_load(&instruction, &state, &mut facts, &mut factory);
        count_branches(&instruction, &mut facts, &mut loops, &mut spans);

        match instruction.flow_control() {
            FlowControl::Call | FlowControl::IndirectCall => {
                facts.calls += 1;
                if instruction.flow_control() == FlowControl::IndirectCall {
                    facts.indirect_calls += 1;
                }
                if let Some(target) = direct_target(&instruction) {
                    facts.callees.push(target);
                }
                state.clobber();
            }
            FlowControl::UnconditionalBranch
            | FlowControl::IndirectBranch
            | FlowControl::Return => {
                state.forget();
            }
            _ => {}
        }
        state.step(&instruction, &mut factory);
    }

    facts.cmp_imms = compared.len() as u32;
    facts.cmp_imm_max = compared.iter().copied().max().unwrap_or(0);
    let widest = chains.values().max_by_key(|values| values.len());
    facts.chain_len = widest.map(|values| values.len()).unwrap_or(0) as u32;
    facts.chain_max_imm = widest
        .and_then(|values| values.iter().copied().max())
        .unwrap_or(0);
    facts.loops = loops.len() as u32;
    facts.loop_stores_via_arg = arg_stores
        .iter()
        .filter(|(ip, _)| spans.iter().any(|(low, high)| ip >= low && ip < high))
        .count() as u32;
    facts.callees.sort_unstable();
    facts.callees.dedup();
    facts
}

mod counting;
mod pointer_state;
#[cfg(test)]
mod tests;

use counting::{
    count_arithmetic, count_branches, count_byte_argument, count_compare, count_immediate_store,
    count_load, count_stack_argument, count_stride_index, count_table_load, direct_target,
    writes_memory,
};
use pointer_state::State;
