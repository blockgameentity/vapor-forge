//! One-line structural summary of a vtable slot.
//!
//! A slot is a function RVA; the summary is what a human needs to judge it
//! without reading the disassembly: how big the `.pdata` extent is, which of the
//! MS-x64 argument registers the body consumes and how, what widths it stores,
//! how many callees it has and whether any callee is an anchored symbol. The
//! behavioural counts come from [`crate::probe_facts`], which already decodes
//! the extent once; only the store-width histogram is measured here, because
//! the existing facts keep 32-bit and byte-immediate stores but not the widths
//! of ordinary stores.
//!
//! A summary is evidence, never proof. It says "this slot moves 32-bit values
//! through its second argument in a loop", not "this slot is
//! `GetSubscribedApps`".

use std::collections::BTreeMap;

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind};

use crate::pe_image::PeImage;
use crate::probe_facts::{self, Facts};

use crate::pdata::Pdata;

/// Used for slots that are inside `.text` but not a `.pdata` entry (import
/// thunks, `_purecall` stubs): enough bytes to see the shape of the stub.
const UNINDEXED_WINDOW: u32 = 0x20;
const MAX_INSNS: u32 = 20_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StoreWidths {
    pub b1: u32,
    pub b2: u32,
    pub b4: u32,
    pub b8: u32,
}
impl StoreWidths {
    pub fn total(self) -> u32 {
        self.b1 + self.b2 + self.b4 + self.b8
    }

    pub fn summary(self) -> String {
        format!(
            "1B:{} 2B:{} 4B:{} 8B:{}",
            self.b1, self.b2, self.b4, self.b8
        )
    }
}

#[derive(Clone, Debug)]
pub struct SlotReport {
    pub slot: usize,
    pub rva: u32,
    /// `.pdata` extent length, `None` when the slot is not a function entry.
    pub size: Option<u32>,
    pub facts: Facts,
    pub stores: StoreWidths,
    /// `lea r, [r + r*4]` sites: the first half of a `index * 5` stride. The
    /// 64-bit `GetSubscribedApps` shape multiplies by 0x50 with `lea`+`shl 4`,
    /// so the interesting pair is this and [`Self::shift4`].
    pub stride5_leas: u32,
    /// `shl r, 4` sites. Together with [`Self::stride5_leas`] these are the
    /// `index * 0x50` licence stride the Linux body uses.
    pub shift4: u32,
    pub shape: Option<u64>,
    /// Direct callees that carry a label: a corpus anchor name, or
    /// `Class#slot` for a known vtable method.
    pub anchored_callees: Vec<String>,
}

impl SlotReport {
    /// The required one-liner. Fields are separated by `|` in three groups:
    /// identity, argument/store shape, control flow and callees.
    pub fn line(&self) -> String {
        let size = match self.size {
            Some(size) => format!("{size:>5}"),
            None => "    -".to_owned(),
        };
        let anchored = if self.anchored_callees.is_empty() {
            "-".to_owned()
        } else {
            self.anchored_callees.join(",")
        };
        format!(
            "slot {:>3} {:#010x} size {size} insns {:<5} | {} | stores {} | loops {} calls {} mul {} \
| u32@arg {} eq {} stride9 {} stride5 {} shl4 {} bytearg {} | chain {}/{}{} | callees {anchored}",
            self.slot,
            self.rva,
            self.facts.insns,
            self.facts.args.summary(),
            self.stores.summary(),
            self.facts.loops,
            self.facts.calls,
            self.facts.multiplies,
            via_args(self.facts.u32_stores_via_arg),
            self.facts.equals_imm_stores,
            self.facts.stride9_leas,
            self.stride5_leas,
            self.shift4,
            self.facts.byte_argument_reads,
            self.facts.chain_len,
            self.facts.chain_max_imm,
            if self.facts.computed_branch() { " table" } else { "" },
        )
    }
}

fn via_args(counts: [u32; 4]) -> String {
    let parts = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count > 0)
        .map(|(index, count)| format!("{index}:{count}"))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "-".to_owned()
    } else {
        parts.join(",")
    }
}

/// Measure one slot. `labels` maps a function RVA to a display label (a
/// corpus anchor name or `Class#slot`); it is only used to name callees.
pub fn summarise(
    image: &PeImage<'_>,
    functions: &Pdata,
    slot: usize,
    rva: u32,
    labels: &BTreeMap<u32, String>,
) -> SlotReport {
    let extent = functions.entry(rva).map(|found| (found.begin, found.end));
    let (begin, end) = extent.unwrap_or((rva, rva.saturating_add(UNINDEXED_WINDOW)));
    let facts = probe_facts::measure(image, begin, end);
    let shapes = slot_shapes(image, begin, end);
    let anchored_callees = facts
        .callees
        .iter()
        .filter_map(|callee| labels.get(callee).cloned())
        .collect();
    SlotReport {
        slot,
        rva,
        size: extent.map(|(begin, end)| end - begin),
        facts,
        stores: shapes.stores,
        stride5_leas: shapes.stride5_leas,
        shift4: shapes.shift4,
        shape: shape_of(image, begin, end),
        anchored_callees,
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SlotShapes {
    pub stores: StoreWidths,
    pub stride5_leas: u32,
    pub shift4: u32,
}

/// One linear decode for the measurements `probe_facts` does not keep: store
/// widths and the `index * 0x50` licence stride (`lea r,[r+r*4]` + `shl r,4`).
pub fn slot_shapes(image: &PeImage<'_>, begin: u32, end: u32) -> SlotShapes {
    let mut shapes = SlotShapes::default();
    let Some(code) = image.bytes_at_rva(u64::from(begin), end.saturating_sub(begin) as usize)
    else {
        return shapes;
    };
    let mut decoder = Decoder::with_ip(64, code, u64::from(begin), DecoderOptions::NONE);
    let mut factory = iced_x86::InstructionInfoFactory::new();
    let mut instruction = Instruction::default();
    let mut decoded = 0u32;
    while decoder.can_decode() && decoded < MAX_INSNS {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        decoded += 1;
        if instruction.mnemonic() == iced_x86::Mnemonic::Lea
            && instruction.memory_base() == instruction.memory_index()
            && instruction.memory_index() != iced_x86::Register::None
            && instruction.memory_index_scale() == 4
        {
            shapes.stride5_leas += 1;
        }
        if instruction.mnemonic() == iced_x86::Mnemonic::Shl
            && instruction.op1_kind() == OpKind::Immediate8
            && instruction.immediate8() == 4
        {
            shapes.shift4 += 1;
        }
        if instruction.op0_kind() != OpKind::Memory {
            continue;
        }
        // A memory operand is a store exactly when iced marks it written, so
        // `cmp [rdi], 0x3d` and `test [rax], cl` do not count as stores.
        let written = factory
            .info(&instruction)
            .used_memory()
            .iter()
            .any(|memory| {
                matches!(
                    memory.access(),
                    iced_x86::OpAccess::Write | iced_x86::OpAccess::ReadWrite
                )
            });
        if !written {
            continue;
        }
        match instruction.memory_size().size() {
            1 => shapes.stores.b1 += 1,
            2 => shapes.stores.b2 += 1,
            4 => shapes.stores.b4 += 1,
            8 => shapes.stores.b8 += 1,
            _ => {}
        }
    }
    shapes
}

/// Store-width histogram over a linear decode of `begin..end`.
pub fn store_widths(image: &PeImage<'_>, begin: u32, end: u32) -> StoreWidths {
    slot_shapes(image, begin, end).stores
}

/// Build-shape fingerprint: instruction codes and operand kinds, but not the
/// operands a rebuild perturbs (branch targets, displacements, immediates).
///
/// Duplicated from `bin/vapor-forge-pe-functions.rs::shape_of`, which cannot be
/// imported from a library module; keep the two in step.
pub fn shape_of(image: &PeImage<'_>, begin: u32, end: u32) -> Option<u64> {
    let len = end.saturating_sub(begin) as usize;
    let code = image.bytes_at_rva(u64::from(begin), len)?;
    if code.is_empty() {
        return None;
    }
    let mut decoder = Decoder::with_ip(64, code, u64::from(begin), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut decoded = 0usize;
    while decoder.can_decode() && decoded < 24 {
        decoder.decode_out(&mut instruction);
        if instruction.is_invalid() {
            break;
        }
        decoded += 1;
        mix(&mut hash, u64::from(instruction.code() as u32));
        mix(&mut hash, u64::from(instruction.op_count()));
        for index in 0..instruction.op_count() {
            let value = match instruction.op_kind(index) {
                OpKind::Register => u64::from(instruction.op_register(index) as u32),
                OpKind::Memory => {
                    let base = instruction.memory_base();
                    if base == iced_x86::Register::None {
                        0xFFFF
                    } else {
                        u64::from(base as u32) + 1
                    }
                }
                other => u64::from(other as u32) + 0x1_0000,
            };
            mix(&mut hash, value);
        }
    }
    Some(hash)
}

fn mix(hash: &mut u64, value: u64) {
    *hash ^= value;
    *hash = hash.wrapping_mul(0x100_0000_01b3);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn via_args_renders_only_used_arguments() {
        assert_eq!(via_args([0, 3, 0, 0]), "1:3");
        assert_eq!(via_args([1, 0, 2, 0]), "0:1,2:2");
        assert_eq!(via_args([0, 0, 0, 0]), "-");
    }

    #[test]
    fn store_widths_are_counted_per_width() {
        // mov [rdx+rax*4], ecx / mov byte ptr [rcx], 0x3d / mov qword ptr [rdx], rax
        let code = [
            0x89, 0x0C, 0x82, // mov [rdx+rax*4], ecx
            0xC6, 0x01, 0x3D, // mov byte ptr [rcx], 0x3D
            0x48, 0x89, 0x02, // mov [rdx], rax
        ];
        let bytes = crate::vtable_pe::tests::synthetic_pe(&code, &[], &[]);
        let image = crate::pe_image::PeImage::parse(&bytes).expect("parse");
        let widths = store_widths(&image, 0x1000, 0x1000 + code.len() as u32);
        assert_eq!(widths.b4, 1);
        assert_eq!(widths.b1, 1);
        assert_eq!(widths.b8, 1);
        assert_eq!(widths.b2, 0);
    }
}
