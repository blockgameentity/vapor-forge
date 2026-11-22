//! Body classification and verdict tests, on hand-built instruction bytes.

use super::*;

const RVA: u32 = 0x1000;

/// `push rbp; mov rbp, rsp; mov eax, 1; pop rbp; ret; int3`
fn ret_body() -> Vec<u8> {
    vec![
        0x55, 0x48, 0x89, 0xE5, 0xB8, 0x01, 0, 0, 0, 0x5D, 0xC3, 0xCC,
    ]
}

#[test]
fn body_that_returns_passes() {
    let body = analyze(&ret_body(), RVA);
    assert!(body.has_ret);
    assert!(!body.ends_in_jump);
    assert!(!body.is_thunk);
    assert!(body.terminates());
    assert_eq!(body.label(), "returns");
}

#[test]
fn tail_jump_thunk_is_recognised() {
    // `jmp rel32` into a neighbouring function: a 5-byte tail-call stub.
    let stub = [0xE9, 0xFB, 0x0F, 0x00, 0x00];
    let body = analyze(&stub, RVA);
    assert!(body.ends_in_jump);
    assert!(body.is_thunk);
    assert!(body.terminates(), "a thunk still ends in a jump");
    assert_eq!(body.label(), "tail-jump thunk");

    // Same shape but long: an ordinary tail call, not a stub.
    let mut long = vec![0x48; 0x40];
    long.extend_from_slice(&stub);
    assert!(!analyze(&long, RVA).is_thunk);
}

#[test]
fn indirect_tail_jump_counts_as_a_terminating_body() {
    // `jmp qword ptr [rip+0x1234]`
    let stub = [0xFF, 0x25, 0x34, 0x12, 0x00, 0x00];
    let body = analyze(&stub, RVA);
    assert!(body.ends_in_jump);
    assert!(!body.has_ret);
    assert!(body.terminates());
}

#[test]
fn fragment_without_terminator_fails() {
    // A mid-function slice: no RET, no jump, decodes cleanly.
    let slice = [0x48, 0x8B, 0x44, 0x24, 0x08, 0x48, 0x85, 0xC0];
    let body = analyze(&slice, RVA);
    assert!(!body.terminates());
    assert_eq!(body.label(), "no terminator");
}

#[test]
fn empty_extent_has_no_body() {
    let body = analyze(&[], RVA);
    assert_eq!(body, Body::default());
    assert!(!body.terminates());
}

#[test]
fn undecodable_tail_is_not_a_tail_jump() {
    // 0x62 starts an invalid opcode in 64-bit mode.
    let junk = [0x62, 0xFF, 0xFF, 0xFF];
    let body = analyze(&junk, RVA);
    assert!(body.truncated);
    assert!(!body.ends_in_jump);
    assert!(!body.terminates());
}

#[test]
fn rejected_anchor_is_reported_for_every_failure() {
    let rejected = apply_verdict(
        "CUser::CheckAppOwnership",
        0x1010,
        AnchorVerdict::MidFunction {
            function: Function {
                begin: 0x1000,
                end: 0x1080,
            },
        },
    )
    .expect("mid-function must be rejected");
    assert!(rejected.to_string().contains("mid-function"));
    assert!(rejected.to_string().contains("CUser::CheckAppOwnership"));
    assert!(apply_verdict(
        "ok",
        0x1000,
        AnchorVerdict::Verified {
            function: Function {
                begin: 0x1000,
                end: 0x1080
            },
            body: Body {
                has_ret: true,
                ..Body::default()
            }
        }
    )
    .is_none());
}
