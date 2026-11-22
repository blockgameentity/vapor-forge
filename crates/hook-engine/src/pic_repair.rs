//! PIC-thunk repair applied to a detour trampoline.
//!
//! A 32-bit ELF image reaches the GOT through `call __x86.get_pc_thunk.<reg>`,
//! which materialises its own return address. `retour` copies the hooked
//! function's prologue into a trampoline, so that return address changes and the
//! GOT base the function computes becomes wrong. The repair rewrites the copied
//! `call` into `mov reg, imm32` with the address the original call would have
//! produced, using the analysis in [`crate::pic_thunk`].
//!
//! MS-x64 and 64-bit ELF need none of this: they reach globals through
//! RIP-relative operands. [`crate::pic_thunk::ensure_supported`] is therefore
//! checked before anything is read, and this module stays inert elsewhere.

use tracing::{error, info, trace};

use crate::pic_thunk;
use crate::protect;

/// Repair the PIC-thunk call in a trampoline, if the target has one.
///
/// `original_prologue` is the hooked function's bytes read *before* the detour
/// was enabled, because `retour` overwrites them with the jump.
pub(crate) fn repair_pic_thunk(
    tramp_addr: usize,
    callee_addr: usize,
    original_prologue: &[u8; 16],
) {
    // MS-x64 and 64-bit ELF have no PIC thunks to repair: x64 reaches globals
    // through RIP-relative operands, so the copied prologue contains no
    // `call __x86.get_pc_thunk.*`. The check lives in `pic_thunk` so the gate
    // and its error are defined in one place.
    if let Err(reason) = pic_thunk::ensure_supported() {
        trace!(reason = %reason, "PIC thunk repair not applicable");
        return;
    }

    // SAFETY: retour keeps the trampoline mapped for the process lifetime and
    // its relocated prologue occupies at least these 64 readable bytes.
    let tramp_bytes = unsafe { std::slice::from_raw_parts(tramp_addr as *const u8, 64) };

    // Scan the ORIGINAL prologue (pre-retour) for PIC thunk calls. The call
    // targets (the real PIC thunk functions) are still intact in the module:
    // only the hooked function's own prologue bytes were overwritten.
    let result = pic_thunk::find_pic_thunk_call(original_prologue, callee_addr as u32, &|addr| {
        let ptr = addr as usize as *const u8;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: a PIC thunk is a small function elsewhere in the module, not
        // affected by retour's prologue overwrite.
        Some(unsafe { [*ptr, *ptr.add(1), *ptr.add(2), *ptr.add(3)] })
    });

    let Ok(site) = result else { return };

    let offset = site.offset_in_buffer;
    let register = site.register;

    let Some(relocated_len) = trampoline_relocated_prefix_len32(
        tramp_bytes,
        tramp_addr as u32,
        callee_addr as u32,
        original_prologue.len(),
    ) else {
        return;
    };
    if !range_within_relocated_prefix(offset, 5, relocated_len) {
        return;
    }

    // Verify: the trampoline must still have a call (E8) at this offset.
    if offset >= tramp_bytes.len() || tramp_bytes[offset] != 0xE8 {
        return;
    }

    let call_address = callee_addr as u32 + offset as u32;
    let Ok(plan) = pic_thunk::plan_pic_thunk_repair(site, call_address) else {
        trace!("PIC thunk repair plan refused for this target");
        return;
    };

    // Make the trampoline writable, write, and let the guard restore the
    // previous protection. No path here can leave the page writable:
    // `with_writable` restores it on success, on a rejected write, and on a
    // failure to obtain write access in the first place.
    match protect::with_writable(tramp_addr, plan.patch_bytes.len(), || {
        // SAFETY: `tramp_addr + offset` was checked against the trampoline's
        // relocated prefix above, so the 5-byte patch stays inside the 64-byte
        // window read at the top of this function, and the page is writable for
        // the whole closure.
        unsafe {
            let patch_ptr = (tramp_addr + offset) as *mut u8;
            for (i, &byte) in plan.patch_bytes.iter().enumerate() {
                *patch_ptr.add(i) = byte;
            }
        }
    }) {
        Ok(()) => info!(
            register = format_args!("{:?}", register),
            offset = offset,
            "PIC thunk repaired"
        ),
        Err(error) => error!(
            error = %error,
            offset = offset,
            "PIC thunk repair: could not make the trampoline writable"
        ),
    }
}

/// Length of the prefix `retour` relocated, found from its return jump back
/// into the original function.
fn trampoline_relocated_prefix_len32(
    trampoline: &[u8],
    trampoline_address: u32,
    callee_address: u32,
    original_window_len: usize,
) -> Option<usize> {
    use iced_x86::{Decoder, DecoderOptions, Mnemonic, OpKind};

    let original_end = callee_address.checked_add(original_window_len as u32)?;
    let mut decoder = Decoder::with_ip(
        32,
        trampoline,
        u64::from(trampoline_address),
        DecoderOptions::NONE,
    );
    while decoder.can_decode() {
        let instruction = decoder.decode();
        if instruction.is_invalid() {
            return None;
        }
        if instruction.mnemonic() != Mnemonic::Jmp
            || !matches!(
                instruction.op0_kind(),
                OpKind::NearBranch16 | OpKind::NearBranch32
            )
        {
            continue;
        }
        let target = instruction.near_branch_target() as u32;
        if target > callee_address && target <= original_end {
            return Some(target.wrapping_sub(callee_address) as usize);
        }
    }
    None
}

fn range_within_relocated_prefix(offset: usize, len: usize, relocated_len: usize) -> bool {
    offset
        .checked_add(len)
        .is_some_and(|end| end <= relocated_len)
}

#[cfg(test)]
mod tests {
    use super::{range_within_relocated_prefix, trampoline_relocated_prefix_len32};

    fn append_return_jump(bytes: &mut Vec<u8>, trampoline_address: u32, target: u32) {
        let jump_address = trampoline_address + bytes.len() as u32;
        bytes.push(0xe9);
        bytes.extend_from_slice(&target.wrapping_sub(jump_address + 5).to_le_bytes());
    }

    #[test]
    fn measures_only_the_prefix_relocated_by_retour() {
        let trampoline_address = 0xf670_0054;
        let callee_address = 0xd22e_36b0;
        let mut bytes = vec![0x55, 0x89, 0xe5, 0x57, 0x56];
        append_return_jump(&mut bytes, trampoline_address, callee_address + 5);
        bytes.extend_from_slice(&[0xe8, 0, 0, 0, 0]);

        let relocated_len =
            trampoline_relocated_prefix_len32(&bytes, trampoline_address, callee_address, 16)
                .unwrap();
        assert_eq!(relocated_len, 5);
        assert!(!range_within_relocated_prefix(8, 5, relocated_len));
    }

    #[test]
    fn includes_a_pic_call_relocated_before_the_return_jump() {
        let trampoline_address = 0xf670_0054;
        let callee_address = 0xd22e_36b0;
        let mut bytes = vec![0x55, 0x57, 0x56, 0x53, 0xe8, 0, 0, 0, 0];
        append_return_jump(&mut bytes, trampoline_address, callee_address + 9);

        let relocated_len =
            trampoline_relocated_prefix_len32(&bytes, trampoline_address, callee_address, 16)
                .unwrap();
        assert_eq!(relocated_len, 9);
        assert!(range_within_relocated_prefix(4, 5, relocated_len));
    }
}
