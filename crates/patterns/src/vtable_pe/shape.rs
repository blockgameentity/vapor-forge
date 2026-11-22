//! Build-shape fingerprint of a function body.
//!
//! Two builds of the same module keep a function's instruction sequence but
//! reallocate displacements, immediates and branch targets. Hashing only the
//! instruction codes and operand *kinds* gives a value that survives a rebuild
//! and still changes when the body's shape changes, which is what the
//! cross-build comparison reports as "the same structure at this slot".
//!
//! Duplicated from `bin/vapor-forge-pe-functions.rs::shape_of`, which cannot be
//! imported from a library module; keep the two in step.

use iced_x86::{Decoder, DecoderOptions, Instruction, OpKind};

use crate::pe_image::PeImage;

/// Instructions that go into the fingerprint.
const FINGERPRINT_INSNS: usize = 24;

/// Fingerprint of the first [`FINGERPRINT_INSNS`] instructions of `begin..end`.
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
    while decoder.can_decode() && decoded < FINGERPRINT_INSNS {
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
