//! Hand-assembled bodies, one behaviour each, matching the counters in the
//! parent module.

use super::{measure_code, Facts};

#[test]
fn counts_u32_stores_through_the_out_pointer() {
    // mov eax, [rcx]      ; 8B 01
    // mov [rdx], eax      ; 89 02
    // lea rdx, [rdx+4]    ; 48 8D 52 04
    // mov [rdx], eax      ; 89 02
    // jne -0x07           ; 75 F9
    // ret                 ; C3
    let code = [
        0x8B, 0x01, 0x89, 0x02, 0x48, 0x8D, 0x52, 0x04, 0x89, 0x02, 0x75, 0xF9, 0xC3,
    ];
    let facts = measure_code(&code, 0x1000, 0x100d);
    assert!(facts.mapped);
    assert_eq!(facts.u32_stores, 2);
    assert_eq!(facts.u32_stores_via_arg, [0, 2, 0, 0]);
    assert_eq!(facts.u32_loads_via_arg, [1, 0, 0, 0]);
    assert_eq!(facts.loops, 1);
    assert!(facts.loop_stores_via_arg >= 1);
    assert_eq!(facts.args.argument_count(), 2);
}

#[test]
fn counts_equals_and_printable_immediates() {
    // mov byte ptr [rcx], 0x3d     ; C6 01 3D
    // mov byte ptr [rcx+1], 'A'    ; C6 41 01 41
    let code = [0xC6, 0x01, 0x3D, 0xC6, 0x41, 0x01, 0x41];
    let facts = measure_code(&code, 0, 7);
    assert_eq!(facts.byte_imm_stores, 2);
    assert_eq!(facts.equals_imm_stores, 1);
    assert_eq!(facts.printable_imm_stores, 2);
    assert_eq!(facts.u32_stores_via_arg, [0, 0, 0, 0]);
}

#[test]
fn sees_a_jump_table() {
    // jmp qword ptr [rax*8+0x1000]  ; FF 24 C5 00 10 00 00
    let code = [0xFF, 0x24, 0xC5, 0x00, 0x10, 0x00, 0x00];
    let facts = measure_code(&code, 0, 7);
    assert!(facts.jump_table);
    assert_eq!(facts.indirect_branches, 1);
}

#[test]
fn sees_a_narrow_compare_chain() {
    // cmp ecx, 1 / jz +2 ; cmp ecx, 2 / jz +2 ; cmp ecx, 3 / jz +2
    let mut code = Vec::new();
    for value in [1u8, 2, 3, 4] {
        code.extend_from_slice(&[0x83, 0xF9, value, 0x74, 0x02]);
    }
    code.push(0xC3);
    let facts = measure_code(&code, 0, code.len() as u32);
    assert_eq!(facts.cmp_imms, 4);
    assert_eq!(facts.cmp_imm_max, 4);
    assert_eq!(facts.chain_len, 4);
    assert!(facts.narrow_compare_chain(4, 0x1000));
}

#[test]
fn follows_a_pointer_reloaded_from_the_home_slot() {
    // mov [rsp+0x10], rdx  ; 48 89 54 24 10
    // mov rbx, [rsp+0x10]  ; 48 8B 5C 24 10
    // mov [rbx], eax       ; 89 03
    let code = [
        0x48, 0x89, 0x54, 0x24, 0x10, 0x48, 0x8B, 0x5C, 0x24, 0x10, 0x89, 0x03,
    ];
    let facts = measure_code(&code, 0, 12);
    assert_eq!(facts.u32_stores_via_arg, [0, 1, 0, 0]);
}

#[test]
fn records_direct_callees_sorted_and_unique() {
    // call 0x1005 from 0x1000, call 0x100a from 0x1005, call 0x1005 from 0x100a
    let code = [
        0xE8, 0x00, 0x00, 0x00, 0x00, 0xE8, 0x00, 0x00, 0x00, 0x00, 0xE8, 0xF6, 0xFF, 0xFF, 0xFF,
        0xC3,
    ];
    let facts = measure_code(&code, 0x1000, 0x1010);
    assert_eq!(facts.calls, 3);
    assert_eq!(facts.callees, vec![0x1005, 0x100a]);
}

#[test]
fn a_call_ends_the_attribution_of_caller_saved_pointers() {
    // mov [rdx], eax ; call +0        ; mov [rdx], eax
    let code = [0x89, 0x02, 0xE8, 0x00, 0x00, 0x00, 0x00, 0x89, 0x02];
    let facts = measure_code(&code, 0, 9);
    assert_eq!(facts.u32_stores, 2);
    assert_eq!(facts.u32_stores_via_arg, [0, 1, 0, 0]);
}

#[test]
fn unmapped_extents_report_nothing() {
    let facts = Facts::unmapped(0x10, 0x20);
    assert!(!facts.mapped);
    assert_eq!(facts.u32_stores_through_args(), 0);
}
