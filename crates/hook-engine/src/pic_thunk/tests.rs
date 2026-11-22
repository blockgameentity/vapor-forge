//! The 32-bit decoder is exercised directly through the ungated `_32`
//! helpers, so its byte-level behaviour is covered on every host. The
//! target gate itself is covered by the `refuses_*` tests below.

use super::*;

fn make_thunk_bytes(reg: ThunkRegister) -> [u8; 4] {
    let modrm = 0x04
        | (match reg {
            ThunkRegister::Eax => 0,
            ThunkRegister::Ecx => 1,
            ThunkRegister::Edx => 2,
            ThunkRegister::Ebx => 3,
            ThunkRegister::Esi => 6,
            ThunkRegister::Edi => 7,
        } << 3);
    [0x8b, modrm, 0x24, 0xc3]
}

#[test]
fn detects_pic_thunk_bytes_for_all_registers() {
    for reg in [
        ThunkRegister::Eax,
        ThunkRegister::Ecx,
        ThunkRegister::Edx,
        ThunkRegister::Ebx,
        ThunkRegister::Esi,
        ThunkRegister::Edi,
    ] {
        assert!(
            is_pic_thunk_bytes(&make_thunk_bytes(reg)),
            "should detect thunk for {:?}",
            reg
        );
    }
}

#[test]
fn rejects_non_thunk_bytes() {
    assert!(!is_pic_thunk_bytes(&[0x8b, 0x04, 0x24, 0x90])); // no ret
    assert!(!is_pic_thunk_bytes(&[0x89, 0x04, 0x24, 0xc3])); // wrong opcode
    assert!(!is_pic_thunk_bytes(&[0x8b, 0x44, 0x24, 0xc3])); // [esp+disp8], not [esp]
}

#[test]
fn finds_pic_thunk_call_in_buffer() {
    // Simulate: call rel32 at offset 0, target = base + 10
    // Buffer base = 0x1000, call target = 0x100A
    let mut buffer = [0x90u8; 16];
    buffer[0] = 0xE8; // call rel32
    let disp: i32 = 5; // target = 0x1000 + 5 + 5 = 0x100A
    buffer[1..5].copy_from_slice(&disp.to_le_bytes());

    let thunk_bytes = make_thunk_bytes(ThunkRegister::Ebx);

    let result = find_pic_thunk_call_32(&buffer, 0x1000, &|addr| {
        if addr == 0x100A {
            Some(thunk_bytes)
        } else {
            None
        }
    });

    let site = result.expect("should find PIC thunk call");
    assert_eq!(site.offset_in_buffer, 0);
    assert_eq!(site.call_target, 0x100A);
    assert_eq!(site.register, ThunkRegister::Ebx);
}

#[test]
fn reports_no_thunk_in_empty_buffer() {
    let buffer = [0x90u8; 8];
    let result = find_pic_thunk_call_32(&buffer, 0x1000, &|_| None);
    assert_eq!(result, Err(PicThunkError::NoPicThunkFound));
}

#[test]
fn plans_correct_repair_bytes() {
    let site = PicThunkCallSite {
        offset_in_buffer: 0,
        call_target: 0xDEAD,
        register: ThunkRegister::Eax,
    };
    let plan = plan_pic_thunk_repair_32(site, 0x1000);

    assert_eq!(plan.correct_return_address, 0x1005);
    // mov eax, 0x00001005 = B8 05 10 00 00
    assert_eq!(plan.patch_bytes, [0xB8, 0x05, 0x10, 0x00, 0x00]);
}

#[test]
fn plans_correct_repair_for_ebx() {
    let site = PicThunkCallSite {
        offset_in_buffer: 3,
        call_target: 0xBEEF,
        register: ThunkRegister::Ebx,
    };
    let plan = plan_pic_thunk_repair_32(site, 0xABC0);

    assert_eq!(plan.correct_return_address, 0xABC5);
    // mov ebx, 0x0000ABC5 = BB C5 AB 00 00
    assert_eq!(plan.patch_bytes, [0xBB, 0xC5, 0xAB, 0x00, 0x00]);
}

#[test]
fn applies_repair_to_buffer() {
    let mut buffer = [0xE8, 0x05, 0x00, 0x00, 0x00, 0x90, 0x90, 0x90];
    let site = PicThunkCallSite {
        offset_in_buffer: 0,
        call_target: 0xBEEF,
        register: ThunkRegister::Eax,
    };
    let plan = plan_pic_thunk_repair_32(site, 0x1000);
    apply_repair_to_buffer(&mut buffer, &plan).expect("apply should succeed");

    assert_eq!(buffer[0], 0xB8); // mov eax, imm32
    assert_eq!(&buffer[1..5], &0x1005u32.to_le_bytes());
    assert_eq!(&buffer[5..], &[0x90, 0x90, 0x90]); // untouched
}

#[test]
fn rejects_repair_outside_buffer() {
    let mut buffer = [0x90; 8];
    let plan = plan_pic_thunk_repair_32(
        PicThunkCallSite {
            offset_in_buffer: 4,
            call_target: 0xBEEF,
            register: ThunkRegister::Eax,
        },
        0x1000,
    );

    assert_eq!(
        apply_repair_to_buffer(&mut buffer, &plan),
        Err(PicThunkError::PatchOutsideBuffer)
    );
}

#[test]
fn end_to_end_find_plan_apply() {
    // Simulate a copied prologue with a thunk call
    let mut buffer = [0x90u8; 16];
    buffer[0] = 0xE8;
    let disp: i32 = 5;
    buffer[1..5].copy_from_slice(&disp.to_le_bytes());

    let thunk_bytes = make_thunk_bytes(ThunkRegister::Eax);
    let base_addr = 0x2000u32;
    let original_fn_addr = 0x1000u32;

    let site = find_pic_thunk_call_32(&buffer, base_addr, &|addr| {
        if addr == base_addr + 10 {
            Some(thunk_bytes)
        } else {
            None
        }
    })
    .expect("should find thunk");

    let plan = plan_pic_thunk_repair_32(site, original_fn_addr);
    apply_repair_to_buffer(&mut buffer, &plan).expect("apply should succeed");

    // Verify: buffer[0..5] is now mov eax, 0x1005
    assert_eq!(buffer[0], 0xB8);
    assert_eq!(&buffer[1..5], &0x1005u32.to_le_bytes());
}

/// A prologue that *is* a 32-bit PIC-thunk call, and the thunk it calls.
fn thunk_call_prologue() -> ([u8; 8], [u8; 4]) {
    let mut buffer = [0x90u8; 8];
    buffer[0] = 0xE8; // call rel32
    buffer[1..5].copy_from_slice(&5i32.to_le_bytes()); // target = base + 10
    (buffer, make_thunk_bytes(ThunkRegister::Eax))
}

#[test]
fn public_entry_points_are_gated_on_the_target() {
    let (buffer, thunk_bytes) = thunk_call_prologue();
    let site = PicThunkCallSite {
        offset_in_buffer: 0,
        call_target: 0x100a,
        register: ThunkRegister::Eax,
    };

    let scanned = find_pic_thunk_call(&buffer, 0x1000, &|_| Some(thunk_bytes));
    let planned = plan_pic_thunk_repair(site, 0x1000);

    if SUPPORTED {
        assert_eq!(
            scanned.map(|site| site.offset_in_buffer),
            Ok(0),
            "32-bit ELF must scan the prologue"
        );
        assert!(planned.is_ok(), "32-bit ELF must plan the repair");
    } else {
        assert_eq!(
            scanned.err(),
            Some(PicThunkError::UnsupportedTarget),
            "a 32-bit PIC prologue must be refused, not decoded as this target's code"
        );
        assert_eq!(planned.err(), Some(PicThunkError::UnsupportedTarget));
        assert_eq!(ensure_supported(), Err(PicThunkError::UnsupportedTarget));
    }
}

#[test]
fn only_32_bit_elf_targets_are_supported() {
    let expected = cfg!(all(
        target_arch = "x86",
        target_pointer_width = "32",
        target_family = "unix",
        not(target_vendor = "apple")
    ));
    assert_eq!(SUPPORTED, expected);
    assert_eq!(ensure_supported().is_ok(), expected);
}
