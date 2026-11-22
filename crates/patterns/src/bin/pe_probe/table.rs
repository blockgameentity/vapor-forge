//! The `.pdata` function table, and a disassembly dump for confirming a
//! candidate by reading it.
//!
//! The table comes from [`vapor_forge_patterns::pdata`], which owns the single
//! definition of a function extent (contiguous funclet/cold-fragment merge, see
//! the module docs there). The probe previously carried its own copy of that
//! parser; keeping two made a `.pdata` fragment look like a function to one tool
//! and not the other.

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind, Register};
use vapor_forge_patterns::pdata;
use vapor_forge_patterns::pe_image::PeImage;

pub use vapor_forge_patterns::pdata::Function;

/// The merged function extents of `image`, per the shared `.pdata` rule.
pub fn function_table(image: &PeImage<'_>, bytes: &[u8]) -> Result<Vec<Function>, String> {
    pdata::parse(image, bytes)
        .map(|table| table.functions().to_vec())
        .map_err(|error| error.to_string())
}

pub fn dump(image: &PeImage<'_>, table: &[Function], rva: u32) -> Result<(), String> {
    let function = table
        .iter()
        .find(|function| function.begin == rva)
        .ok_or_else(|| format!("{rva:#x} is not a merged .pdata entry"))?;
    let len = (function.end - function.begin) as usize;
    let code = image
        .bytes_at_rva(u64::from(function.begin), len)
        .ok_or_else(|| format!("{rva:#x} is not mapped"))?;
    println!("dump {rva:#x}..{:#x} ({len} bytes)", function.end);
    let mut decoder = Decoder::with_ip(64, code, u64::from(function.begin), DecoderOptions::NONE);
    let mut instruction = Instruction::default();
    while decoder.can_decode() {
        decoder.decode_out(&mut instruction);
        println!("  {:#010x}  {}", instruction.ip(), describe(&instruction));
    }
    Ok(())
}

/// `mnemonic op, op` without an Intel text formatter, which this build does not
/// link: registers, memory operands and branches are rendered by hand.
fn describe(instruction: &Instruction) -> String {
    let mut text = format!("{:?}", instruction.mnemonic());
    for operand in 0..instruction.op_count() {
        let rendered = match instruction.op_kind(operand) {
            OpKind::Register => format!("{:?}", instruction.op_register(operand)),
            OpKind::Memory => memory(instruction),
            OpKind::NearBranch64 => format!("{:#x}", instruction.near_branch_target()),
            _ => match immediate(instruction) {
                Some(value) => format!("{value:#x}"),
                None => format!("{:?}", instruction.op_kind(operand)),
            },
        };
        text.push_str(if operand == 0 { " " } else { ", " });
        text.push_str(&rendered);
    }
    text
}

fn memory(instruction: &Instruction) -> String {
    let base = instruction.memory_base();
    let index = instruction.memory_index();
    let scale = instruction.memory_index_scale();
    let displacement = instruction.memory_displacement64() as i64;
    let mut text = String::from("[");
    if base != Register::None {
        text.push_str(&format!("{base:?}"));
    }
    if index != Register::None {
        if base != Register::None {
            text.push('+');
        }
        text.push_str(&format!("{index:?}*{scale}"));
    }
    if displacement != 0 || (base == Register::None && index == Register::None) {
        text.push_str(&format!("{displacement:+#x}"));
    }
    text.push(']');
    text
}

/// The instruction's immediate, whatever its encoded width, so a dump shows
/// `0x3d` and not `Immediate8to64`.
fn immediate(instruction: &Instruction) -> Option<u64> {
    for operand in 0..instruction.op_count() {
        let value = match instruction.op_kind(operand) {
            OpKind::Immediate8 => u64::from(instruction.immediate8()),
            OpKind::Immediate16 => u64::from(instruction.immediate16()),
            OpKind::Immediate32 => u64::from(instruction.immediate32()),
            OpKind::Immediate64 => instruction.immediate64(),
            OpKind::Immediate8to16 => u64::from(instruction.immediate8to16() as u16),
            OpKind::Immediate8to32 => u64::from(instruction.immediate8to32() as u32),
            OpKind::Immediate8to64 => instruction.immediate8to64() as u64,
            OpKind::Immediate32to64 => instruction.immediate32to64() as u64,
            _ => continue,
        };
        return Some(value);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_renders_operands() {
        // mov byte ptr [rcx], 0x3d
        let mut decoder = Decoder::with_ip(64, &[0xC6, 0x01, 0x3D], 0, DecoderOptions::NONE);
        let mut instruction = Instruction::default();
        decoder.decode_out(&mut instruction);
        let text = describe(&instruction);
        assert!(text.starts_with("Mov"), "{text}");
        assert!(text.contains("0x3d"), "{text}");
    }
}
