//! The `.pdata` function table, and a disassembly dump for confirming a
//! candidate by reading it.
//!
//! `RUNTIME_FUNCTION` entries are read the same way `vapor-forge-pe-functions`
//! reads them: contiguous regions of one function (prolog fragment, body,
//! funclets) are merged, because a 20-byte fragment on its own is not a function
//! and would make every behavioural count meaningless.

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind, Register};
use vapor_forge_patterns::pe_image::PeImage;

const EXCEPTION_DIRECTORY: usize = 3;
const RUNTIME_FUNCTION_SIZE: usize = 12;

#[derive(Clone, Copy, Debug)]
pub struct Function {
    pub begin: u32,
    pub end: u32,
}

pub fn function_table(image: &PeImage<'_>, bytes: &[u8]) -> Result<Vec<Function>, String> {
    let (rva, size) = image
        .data_directory(EXCEPTION_DIRECTORY)
        .filter(|(_, size)| *size > 0)
        .ok_or_else(|| "image has no exception directory (.pdata)".to_owned())?;
    let offset = image
        .rva_to_offset(u64::from(rva))
        .ok_or_else(|| "exception directory RVA is not mapped".to_owned())?;
    let mut functions = Vec::with_capacity(size as usize / RUNTIME_FUNCTION_SIZE);
    for index in 0..size as usize / RUNTIME_FUNCTION_SIZE {
        let at = offset + index * RUNTIME_FUNCTION_SIZE;
        let Some(entry) = bytes.get(at..at + RUNTIME_FUNCTION_SIZE) else {
            return Err("exception directory entry past EOF".to_owned());
        };
        let begin = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let end = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
        if end > begin {
            functions.push(Function { begin, end });
        }
    }
    functions.sort_by_key(|function| function.begin);
    let mut merged: Vec<Function> = Vec::with_capacity(functions.len());
    for function in functions {
        match merged.last_mut() {
            Some(last) if last.end == function.begin => last.end = function.end,
            _ => merged.push(function),
        }
    }
    Ok(merged)
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
