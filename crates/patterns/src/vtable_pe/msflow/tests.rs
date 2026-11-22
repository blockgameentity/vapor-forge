//! Walk fixtures: hand-assembled MS-x64 bodies, one fact each.

use super::*;

fn flow(code: &[u8]) -> Flow {
    analyse(code, 0x1000)
}

#[test]
fn a_store_through_the_payload_pointer_of_argument_three_counts() {
    // mov [rsp+0x18], r8 / mov rax, [rsp+0x18] / mov rax, [rax] /
    // mov [rax], edx / mov al, 1 / ret
    let code = [
        0x4C, 0x89, 0x44, 0x24, 0x18, // mov [rsp+0x18], r8
        0x48, 0x8B, 0x44, 0x24, 0x18, // mov rax, [rsp+0x18]
        0x48, 0x8B, 0x00, // mov rax, [rax]
        0x89, 0x10, // mov [rax], edx
        0xB0, 0x01, // mov al, 1
        0xC3, // ret
    ];
    let flow = flow(&code);
    assert_eq!(flow.stores_through[2], 1);
    assert_eq!(flow.stores_u32_through[2], 1);
    assert_eq!(flow.derefs[2], 1);
    assert_eq!(flow.bool_rets, 1);
    assert!(flow.arg32_reads[1] >= 1);
    assert_eq!(flow.rets, 1);
}

#[test]
fn a_forwarded_argument_is_named_at_the_call() {
    // mov rcx, rdx / call rax (indirect) ... the call ends the walk
    let code = [
        0x48, 0x89, 0xD1, // mov rcx, rdx
        0xFF, 0xD0, // call rax
        0xC3, // ret
    ];
    let flow = flow(&code);
    assert_eq!(flow.calls, 1);
    assert_eq!(flow.forwarded[1], 1);
}

#[test]
fn an_address_into_an_argument_keeps_the_argument() {
    // lea rsi, [r8+0x10] / mov dword ptr [rsi], 1 / xor eax, eax / ret
    let code = [
        0x49, 0x8D, 0x70, 0x10, // lea rsi, [r8+0x10]
        0xC7, 0x06, 0x01, 0x00, 0x00, 0x00, // mov dword ptr [rsi], 1
        0x31, 0xC0, // xor eax, eax
        0xC3, // ret
    ];
    let flow = flow(&code);
    assert_eq!(flow.stores_through[2], 1);
    assert_eq!(flow.stores_u32_through[2], 1);
    assert_eq!(flow.bool_rets, 1);
    assert_eq!(flow.returned, Some(Value::Imm(0)));
}

#[test]
fn an_overwritten_argument_is_not_reported_as_used() {
    // pop rcx / mov [rcx], eax / ret  -- rcx no longer means argument 0
    let code = [
        0x59, // pop rcx
        0x89, 0x01, // mov [rcx], eax
        0xC3, // ret
    ];
    let flow = flow(&code);
    assert_eq!(flow.stores_through[0], 0);
}
