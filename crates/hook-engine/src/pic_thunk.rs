#![allow(clippy::manual_range_contains)]

//! Repair of 32-bit PIC-thunk calls in a copied function prologue.
//!
//! A 32-bit ELF build of a shared library reaches the GOT through
//! `call __x86.get_pc_thunk.<reg>`, which materialises its own return address
//! in a register; the caller then adds a constant offset. When the prologue is
//! copied into a trampoline the return address changes and the computed GOT
//! base is wrong, so the `call` has to be rewritten into `mov reg, imm32`.
//!
//! Only 32-bit ELF images have this pattern. MS-x64 reaches globals through
//! RIP-relative operands and 64-bit ELF through RIP-relative `lea`, so a copied
//! prologue there has no thunk call to repair. [`ensure_supported`] states that
//! in one place and the public analysis entry points refuse to run elsewhere,
//! rather than decoding 32-bit bytes on a target where they mean something
//! else.

use thiserror::Error;

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum PicThunkError {
    #[error("no PIC thunk call found in the scanned region")]
    NoPicThunkFound,
    #[error("PIC thunk uses an unsupported register (reg_field={0})")]
    UnsupportedThunkRegister(u8),
    #[error(
        "PIC-thunk repair only applies to 32-bit x86 ELF images; refusing to \
         apply 32-bit PIC semantics on this target"
    )]
    UnsupportedTarget,
    #[cfg(test)]
    #[error("PIC thunk repair range is outside the target buffer")]
    PatchOutsideBuffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThunkRegister {
    Eax,
    Ecx,
    Edx,
    Ebx,
    Esi,
    Edi,
}

impl ThunkRegister {
    fn from_modrm_reg_field(field: u8) -> Result<Self, PicThunkError> {
        match field {
            0 => Ok(Self::Eax),
            1 => Ok(Self::Ecx),
            2 => Ok(Self::Edx),
            3 => Ok(Self::Ebx),
            6 => Ok(Self::Esi),
            7 => Ok(Self::Edi),
            _ => Err(PicThunkError::UnsupportedThunkRegister(field)),
        }
    }

    fn mov_opcode(self) -> u8 {
        0xB8 + match self {
            Self::Eax => 0,
            Self::Ecx => 1,
            Self::Edx => 2,
            Self::Ebx => 3,
            Self::Esi => 6,
            Self::Edi => 7,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PicThunkCallSite {
    pub offset_in_buffer: usize,
    pub call_target: u32,
    pub register: ThunkRegister,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PicThunkRepairPlan {
    pub call_site: PicThunkCallSite,
    pub correct_return_address: u32,
    pub patch_bytes: [u8; 5],
}

/// Whether the compiled target is a 32-bit x86 ELF image, the only place a
/// `call __x86.get_pc_thunk.*` can appear in a copied prologue.
pub const SUPPORTED: bool = cfg!(all(
    target_arch = "x86",
    target_pointer_width = "32",
    target_family = "unix",
    not(target_vendor = "apple"),
));

/// Refuse to run the 32-bit repair on a target where it does not apply.
///
/// Callers use this as a gate before scanning anything, so an MS-x64 or 64-bit
/// ELF install reports why PIC repair was skipped instead of misreading a
/// 64-bit prologue as 32-bit code.
pub fn ensure_supported() -> Result<(), PicThunkError> {
    if SUPPORTED {
        Ok(())
    } else {
        Err(PicThunkError::UnsupportedTarget)
    }
}

/// Scan a byte buffer for a `call rel32` whose target is a PIC thunk
/// (`mov reg,[esp]; ret`), returning the first match.
///
/// This is a pure analysis function. It does not modify any memory. Returns
/// [`PicThunkError::UnsupportedTarget`] unless this is a 32-bit ELF target.
pub fn find_pic_thunk_call(
    buffer: &[u8],
    buffer_base_address: u32,
    read_target_bytes: &dyn Fn(u32) -> Option<[u8; 4]>,
) -> Result<PicThunkCallSite, PicThunkError> {
    ensure_supported()?;
    find_pic_thunk_call_32(buffer, buffer_base_address, read_target_bytes)
}

/// The 32-bit decoder itself, without the target gate. Unit-tested on any host;
/// the public wrapper above is what production code calls.
fn find_pic_thunk_call_32(
    buffer: &[u8],
    buffer_base_address: u32,
    read_target_bytes: &dyn Fn(u32) -> Option<[u8; 4]>,
) -> Result<PicThunkCallSite, PicThunkError> {
    let mut decoder = iced_x86::Decoder::with_ip(
        32,
        buffer,
        u64::from(buffer_base_address),
        iced_x86::DecoderOptions::NONE,
    );

    while decoder.can_decode() {
        let instr = decoder.decode();
        if instr.is_invalid() {
            break;
        }

        if instr.mnemonic() == iced_x86::Mnemonic::Call
            && instr.op0_kind() == iced_x86::OpKind::NearBranch32
        {
            let call_target = instr.near_branch32();
            let offset = (instr.ip() as u32 - buffer_base_address) as usize;

            if let Some(target_bytes) = read_target_bytes(call_target) {
                if is_pic_thunk_bytes(&target_bytes) {
                    let reg_field = (target_bytes[1] >> 3) & 0x7;
                    let register = ThunkRegister::from_modrm_reg_field(reg_field)?;
                    return Ok(PicThunkCallSite {
                        offset_in_buffer: offset,
                        call_target,
                        register,
                    });
                }
            }
        }
    }

    Err(PicThunkError::NoPicThunkFound)
}

/// Build a repair plan: what bytes to write at the call site to replace
/// `call thunk` with `mov reg, <correct_return_address>`.
///
/// `original_call_address` is the address of the `call __x86.get_pc_thunk.*`
/// instruction in the **original function** (before hooking). The correct return
/// address is `original_call_address + 5`.
///
/// Returns [`PicThunkError::UnsupportedTarget`] unless this is a 32-bit ELF
/// target: the plan is only meaningful where the call was a PIC thunk.
pub fn plan_pic_thunk_repair(
    call_site: PicThunkCallSite,
    original_call_address: u32,
) -> Result<PicThunkRepairPlan, PicThunkError> {
    ensure_supported()?;
    Ok(plan_pic_thunk_repair_32(call_site, original_call_address))
}

/// The 32-bit plan builder itself, without the target gate.
fn plan_pic_thunk_repair_32(
    call_site: PicThunkCallSite,
    original_call_address: u32,
) -> PicThunkRepairPlan {
    let correct_return_address = original_call_address.wrapping_add(5);
    let imm = correct_return_address.to_le_bytes();
    let patch_bytes = [
        call_site.register.mov_opcode(),
        imm[0],
        imm[1],
        imm[2],
        imm[3],
    ];

    PicThunkRepairPlan {
        call_site,
        correct_return_address,
        patch_bytes,
    }
}

/// Apply a repair plan to a mutable byte buffer (e.g. a test-owned buffer or
/// a trampoline made writable by the caller).
///
/// Returns `Ok(())` if the 5-byte patch was written at the call site offset.
#[cfg(test)]
fn apply_repair_to_buffer(
    buffer: &mut [u8],
    plan: &PicThunkRepairPlan,
) -> Result<(), PicThunkError> {
    let offset = plan.call_site.offset_in_buffer;
    let end = offset
        .checked_add(plan.patch_bytes.len())
        .ok_or(PicThunkError::PatchOutsideBuffer)?;
    let target = buffer
        .get_mut(offset..end)
        .ok_or(PicThunkError::PatchOutsideBuffer)?;
    target.copy_from_slice(&plan.patch_bytes);
    Ok(())
}

fn is_pic_thunk_bytes(bytes: &[u8; 4]) -> bool {
    // mov reg,[esp] = 8b XX 24 where XX has modrm [esp] encoding (mod=00, rm=100)
    // followed by ret = c3
    bytes[0] == 0x8b && bytes[2] == 0x24 && bytes[3] == 0xc3 && (bytes[1] & 0xC7) == 0x04
}

#[cfg(test)]
mod tests;
