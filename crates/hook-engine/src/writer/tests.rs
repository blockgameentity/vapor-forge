//! Host-independent tests for the writer state machine.
//!
//! [`super::fake`] supplies the module image and the trampoline allocator, so
//! these tests drive the same read/plan/allocate/write/restore sequence the Win32
//! backend runs — including the failure paths that must leave the target bytes
//! untouched.
//!
//! The writer's API is `unsafe` because it writes to a live code page; the
//! helpers below are safe wrappers around the fake, which never dereferences a
//! real address.

use super::fake::{FakeMemory, FakeTrampoline};
use super::*;
use crate::patch;

/// `push rbp; mov rbp,rsp; sub rsp,0x20; mov rax,[rbp+8]; ret`
const PROLOGUE: [u8; 16] = [
    0x55, 0x48, 0x89, 0xE5, 0x48, 0x83, 0xEC, 0x20, 0x48, 0x8B, 0x45, 0x08, 0xC3, 0x90, 0x90, 0x90,
];
const ENTRY: usize = 0x1_4000_1000;
const REPLACEMENT: usize = 0x1_4000_9000;

fn memory() -> FakeMemory {
    FakeMemory::new(ENTRY, &PROLOGUE)
}

/// Prepare a patch at `address`, which the fake treats as an entry of its image.
fn prepare_at(
    memory: &mut FakeMemory,
    address: usize,
) -> Result<InstalledPatch<FakeTrampoline>, WriteError> {
    // SAFETY: the fake memory never dereferences a real address; the caller's
    // "live function entry" premise only has to be true for a real module image.
    unsafe { InstalledPatch::prepare(memory, address, REPLACEMENT) }
}

fn prepare(memory: &mut FakeMemory) -> Result<InstalledPatch<FakeTrampoline>, WriteError> {
    prepare_at(memory, ENTRY)
}

fn prepared(memory: &mut FakeMemory) -> InstalledPatch<FakeTrampoline> {
    prepare(memory).expect("prepare")
}

fn enable(
    patch: &mut InstalledPatch<FakeTrampoline>,
    memory: &FakeMemory,
) -> Result<(), WriteError> {
    // SAFETY: the fake writes into a `Vec` behind a `RefCell`, so no code page is
    // modified and no other thread can observe it.
    unsafe { patch.enable(memory) }
}

fn disable(
    patch: &mut InstalledPatch<FakeTrampoline>,
    memory: &FakeMemory,
) -> Result<(), WriteError> {
    // SAFETY: same as `enable`; the fake never touches a real code page.
    unsafe { patch.disable(memory) }
}

#[test]
fn preparing_reads_the_entry_and_allocates_without_writing() {
    let mut memory = memory();
    let patch = prepared(&mut memory);

    assert!(!patch.is_enabled(), "prepare must not install");
    assert_eq!(patch.target(), ENTRY);
    assert_eq!(patch.patch_len(), 12, "whole instructions only");
    assert_eq!(
        memory.entry_bytes(PROLOGUE.len()),
        PROLOGUE,
        "prepare modified the entry"
    );
    assert_eq!(memory.live_allocations(), 1, "one trampoline allocated");
}

#[test]
fn the_trampoline_holds_the_displaced_instructions_then_a_jump_back() {
    let mut memory = memory();
    let patch = prepared(&mut memory);

    let bytes = memory.trampoline_bytes(&patch.trampoline);
    assert_eq!(&bytes[..12], &PROLOGUE[..12], "displaced bytes");
    assert_eq!(
        &bytes[12..],
        patch::absolute_jump((ENTRY + 12) as u64),
        "resumes at the first undisplaced instruction"
    );
    assert_eq!(patch.trampoline_entry(), 0x5000_0000);
    assert!(bytes.len() > patch::ABSOLUTE_JUMP_LEN, "trampoline is never empty");
}

#[test]
fn enable_writes_the_jump_and_uninstall_restores_the_entry_byte_for_byte() {
    let mut memory = memory();
    let mut patch = prepared(&mut memory);

    enable(&mut patch, &memory).expect("enable");
    assert!(patch.is_enabled());
    assert_eq!(
        memory.entry_bytes(12),
        patch::absolute_jump(REPLACEMENT as u64),
        "the entry must hold the absolute jump"
    );
    assert_eq!(memory.trampoline_bytes(&patch.trampoline).len(), 24);

    disable(&mut patch, &memory).expect("disable");
    assert!(!patch.is_enabled());
    assert_eq!(
        memory.entry_bytes(PROLOGUE.len()),
        PROLOGUE,
        "uninstall must restore the original bytes exactly"
    );
}

#[test]
fn a_failed_install_leaves_the_target_bytes_unchanged() {
    let mut memory = memory();
    let mut patch = prepared(&mut memory);
    memory.fail_next_write();

    let result = enable(&mut patch, &memory);

    assert!(result.is_err(), "the refused write must be reported");
    assert!(!patch.is_enabled(), "a refused write is not an install");
    assert_eq!(
        memory.entry_bytes(PROLOGUE.len()),
        PROLOGUE,
        "the target bytes changed despite the failed install"
    );

    // Dropping the prepared patch must release the trampoline it allocated.
    assert_eq!(memory.live_allocations(), 1);
    drop(patch);
    assert_eq!(memory.live_allocations(), 0, "trampoline leaked");
    assert_eq!(memory.freed_allocations(), 1, "released exactly once");
}

#[test]
fn an_entry_the_planner_refuses_never_allocates_or_writes() {
    // A relative call at the entry cannot be copied by plain memcpy.
    let mut memory = FakeMemory::new(
        ENTRY,
        &[0xE8, 0x00, 0x00, 0x00, 0x00, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90],
    );

    let error = prepare(&mut memory).expect_err("unrelocatable entry");

    assert!(matches!(
        error,
        WriteError::Plan(planner::PlanError::RelocationRequired { .. })
    ));
    assert_eq!(memory.live_allocations(), 0, "no trampoline for a refusal");
    assert_eq!(memory.entry_bytes(12)[0], 0xE8, "entry was modified");
}

#[test]
fn a_short_or_unreadable_entry_is_refused() {
    let mut short = FakeMemory::new(ENTRY, &PROLOGUE[..8]);
    assert!(matches!(
        prepare(&mut short),
        Err(WriteError::Plan(planner::PlanError::TooShort { .. }))
    ));
    assert_eq!(short.live_allocations(), 0);

    // An address outside the fake image stands in for a region the backend
    // cannot describe; both fail before anything is read or allocated.
    let mut elsewhere = memory();
    let error = prepare_at(&mut elsewhere, 0x10);
    assert!(matches!(error, Err(WriteError::RegionUnknown { .. })));
    assert_eq!(elsewhere.live_allocations(), 0);
}

#[test]
fn enable_and_disable_are_idempotent() {
    let mut memory = memory();
    let mut patch = prepared(&mut memory);

    disable(&mut patch, &memory).expect("disable before enable");
    assert_eq!(memory.entry_bytes(12), &PROLOGUE[..12]);

    enable(&mut patch, &memory).expect("enable");
    let installed = memory.entry_bytes(12);
    enable(&mut patch, &memory).expect("second enable");
    assert_eq!(memory.entry_bytes(12), installed, "double write");
}

#[test]
fn a_refused_restore_keeps_the_patch_marked_installed() {
    let mut memory = memory();
    let mut patch = prepared(&mut memory);
    enable(&mut patch, &memory).expect("enable");
    memory.fail_next_write();

    assert!(disable(&mut patch, &memory).is_err());
    assert!(
        patch.is_enabled(),
        "an entry that still holds the jump must stay marked installed"
    );
    assert_eq!(
        memory.entry_bytes(12),
        patch::absolute_jump(REPLACEMENT as u64)
    );
}
