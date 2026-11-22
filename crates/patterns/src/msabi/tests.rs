//! Hand-assembled byte sequences, one behaviour each. The bytes are written out
//! by hand rather than produced by an assembler, so each test states exactly
//! what is decoded and cannot drift with an assembler's encoding choice.

use super::{analyse, FirstUse, WINDOW};
use iced_x86::Register;

/// `test rcx, rcx` — reads the first argument.
#[test]
fn reads_an_argument_used_as_an_operand() {
    let code = [0x48, 0x85, 0xC9];
    let report = analyse(&code, 0, WINDOW);
    assert_eq!(report.used_mask(), [true, false, false, false]);
    assert_eq!(report.slots[0].first_read, Some(0));
    assert_eq!(report.slots[0].first_use, Some(FirstUse::Operand));
    assert_eq!(report.argument_count(), 1);
}

/// `mov eax, [rdx]` followed by `mov [rcx], eax`: the 32-bit store goes
/// through argument 1, so both pointers are consumed.
#[test]
fn flags_pointer_arguments_used_as_memory_bases() {
    let code = [0x8B, 0x02, 0x89, 0x01];
    let report = analyse(&code, 0, WINDOW);
    assert_eq!(report.used_mask(), [true, true, false, false]);
    assert_eq!(report.slots[0].first_use, Some(FirstUse::MemoryAddress));
    assert_eq!(report.slots[1].first_use, Some(FirstUse::MemoryAddress));
    assert_eq!(report.slots[0].first_read, Some(2));
    assert_eq!(report.slots[1].first_read, Some(0));
}

/// A prologue that spills its arguments to the caller-provided home slots:
/// `mov [rsp+8], rcx`, `mov [rsp+0x10], rdx`, `mov [rsp+0x18], r8`,
/// `mov [rsp+0x20], r9` — after `sub rsp, 0x28` the offsets shift, which is
/// why the stack pointer is tracked rather than assumed to be at entry.
#[test]
fn recognises_home_slot_spills() {
    let code = [
        0x48, 0x89, 0x4C, 0x24, 0x08, // mov [rsp+8], rcx
        0x48, 0x89, 0x54, 0x24, 0x10, // mov [rsp+0x10], rdx
        0x4C, 0x89, 0x44, 0x24, 0x18, // mov [rsp+0x18], r8
        0x48, 0x83, 0xEC, 0x28, // sub rsp, 0x28
    ];
    let report = analyse(&code, 0, WINDOW);
    assert_eq!(report.used_mask(), [true, true, true, false]);
    assert_eq!(report.slots[0].home_slot(), Some(0x08));
    assert_eq!(report.slots[1].home_slot(), Some(0x10));
    assert_eq!(report.slots[2].home_slot(), Some(0x18));
    assert_eq!(report.argument_count(), 3);
}

/// `xor ecx, ecx` writes without reading, so the slot carries no argument;
/// `mov rcx, rax` does the same.
#[test]
fn overwriting_an_argument_without_reading_it_is_not_a_use() {
    let zeroed = analyse(&[0x31, 0xC9], 0, WINDOW);
    assert_eq!(zeroed.used_mask(), [false, false, false, false]);
    assert_eq!(zeroed.slots[0].first_use, Some(FirstUse::Overwritten));
    assert_eq!(zeroed.argument_count(), 0);

    let moved = analyse(&[0x48, 0x8B, 0xC8], 0, WINDOW);
    assert!(!moved.slots[0].is_used());
    assert_eq!(moved.slots[0].first_write, Some(0));
}

/// `mov rbx, rcx` forwards the argument into a callee-saved register.
#[test]
fn records_a_forwarding_copy() {
    let report = analyse(&[0x48, 0x8B, 0xD9], 0, WINDOW);
    assert_eq!(report.slots[0].first_read, Some(0));
    assert_eq!(
        report.slots[0].first_use,
        Some(FirstUse::CopiedTo(Register::RBX))
    );
}

/// Reading past the window is not seen: the window bounds the walk.
#[test]
fn stops_at_the_window() {
    // `nop` padding, then `test rcx, rcx` past the limit.
    let mut code = vec![0x90; 16];
    code.extend_from_slice(&[0x48, 0x85, 0xC9]);
    let report = analyse(&code, 0, 16);
    assert!(!report.slots[0].is_used());
    let report = analyse(&code, 0, 19);
    assert!(report.slots[0].is_used());
}

/// The fourth argument is `r9`, and its 8-bit view is the same register.
#[test]
fn counts_a_fourth_argument() {
    // `test r9b, r9b`
    let report = analyse(&[0x45, 0x84, 0xC9], 0, WINDOW);
    assert_eq!(report.used_mask(), [false, false, false, true]);
    assert_eq!(report.argument_count(), 4);
}

/// A return stops the walk, so an argument restored after the epilogue is
/// not attributed to the body.
#[test]
fn stops_at_the_return() {
    let code = [
        0x48, 0x85, 0xC9, // test rcx, rcx
        0xC3, // ret
        0x48, 0x85, 0xD2, // test rdx, rdx (unreachable)
    ];
    let report = analyse(&code, 0, WINDOW);
    assert!(report.slots[0].is_used());
    assert!(!report.slots[1].is_used());
}

#[test]
fn summary_names_each_slot() {
    let report = analyse(&[0x8B, 0x02, 0x89, 0x01], 0, WINDOW);
    let summary = report.summary();
    assert!(summary.contains("rcx=ptr@0x2"), "{summary}");
    assert!(summary.contains("rdx=ptr@0x0"), "{summary}");
    assert!(summary.contains("r8=-"), "{summary}");
}
