//! Detour round-trip through the public API: install, call the original through
//! the trampoline, uninstall, and call the original again.
//!
//! This is the unix half of the "an installed detour keeps the original callable"
//! requirement. The Windows half cannot run here (no Windows host); it is covered
//! by the writer's own tests, which drive the same state machine through a fake
//! memory backend on any host.
//!
//! The target function is process-global, so this file holds exactly one test and
//! it must not run alongside another test that patches the same bytes.

#![cfg(target_os = "linux")]

use std::sync::atomic::{AtomicUsize, Ordering};

use vapor_forge_hook_engine::detour::{create_detour, store_and_finalize, Detour};
use vapor_forge_hook_engine::original::original_detour;
use vapor_forge_hook_engine::plan::{AddressRange, HookTargetInput, validate_hook_target};

type Api = extern "C" fn(i32) -> i32;

const NAME: &str = "roundtrip::compute";

static CALLS: AtomicUsize = AtomicUsize::new(0);

// The process-lifetime slot the replacement reaches its original through. Never
// moved after installation; only ever addressed through `addr_of!`.
static mut SLOT: Option<Detour<Api>> = None;

/// The function under test. `inline(never)` keeps a real call at the patched
/// entry instead of a copy the optimizer placed there.
#[inline(never)]
extern "C" fn compute(value: i32) -> i32 {
    value.wrapping_mul(3).wrapping_add(1)
}

/// The replacement. It calls the original through the trampoline, so a
/// mis-planned entry shows up as a wrong result rather than a silent pass.
extern "C" fn hooked(value: i32) -> i32 {
    CALLS.fetch_add(1, Ordering::SeqCst);
    // SAFETY: installation initializes the process-lifetime slot before enabling
    // the detour, so the trampoline is valid for the duration of the call.
    let original = unsafe { original_detour::<Api>(NAME, std::ptr::addr_of!(SLOT)) };
    match original {
        Some(original) => original(value).wrapping_add(100),
        None => -1,
    }
}

/// The stored detour, after installation.
///
/// # Safety
/// Only call between a successful install and dropping the slot.
unsafe fn stored() -> &'static mut Detour<Api> {
    // SAFETY: the caller guarantees the slot is initialized and never moved.
    unsafe { (*std::ptr::addr_of_mut!(SLOT)).as_mut() }.expect("detour was installed")
}

#[test]
fn install_call_original_uninstall_and_drop_restore_the_entry() {
    let baseline = compute(21);
    assert_eq!(baseline, 64, "sanity: the target computes 3x+1");

    let target_address = compute as Api as usize;
    let plan = validate_hook_target(HookTargetInput {
        target_address,
        replacement_address: hooked as Api as usize,
        executable_range: AddressRange {
            start: target_address,
            end: target_address + 1,
        },
    })
    .expect("target is inside the executable range");

    // SAFETY: both functions have signature `Api` and the target is the live
    // entry of `compute`.
    let pending = unsafe { create_detour::<Api>(NAME, plan) }.expect("detour created");
    // SAFETY: `SLOT` is process-lifetime storage and is written exactly once.
    let installed = unsafe { store_and_finalize(NAME, std::ptr::addr_of_mut!(SLOT), Some(pending)) };
    assert!(installed, "detour must be finalized");

    // The entry now jumps to the replacement.
    assert_eq!(compute(21), baseline + 100, "replacement must run");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);

    // The original is still callable through the trampoline.
    // SAFETY: the slot is initialized and not being mutated here.
    let original = unsafe { original_detour::<Api>(NAME, std::ptr::addr_of!(SLOT)) }
        .expect("trampoline is available");
    assert_eq!(original(21), baseline, "trampoline must run the original");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1, "the original is not the hook");

    // Uninstall: the entry is the original again.
    // SAFETY: single-threaded test, and the slot is initialized.
    unsafe { stored().disable() }.expect("disable");
    assert_eq!(compute(21), baseline, "uninstall must restore the entry");
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);

    // Re-enable and then drop: `Drop` must restore the entry as well.
    // SAFETY: same as above.
    unsafe { stored().enable() }.expect("re-enable");
    assert_eq!(compute(21), baseline + 100, "re-enable must patch again");
    assert_eq!(CALLS.load(Ordering::SeqCst), 2);

    // SAFETY: dropping the only owner of the detour; nothing else uses the slot
    // afterwards.
    unsafe { *std::ptr::addr_of_mut!(SLOT) = None };
    assert_eq!(compute(21), baseline, "drop must restore the entry");
    assert_eq!(CALLS.load(Ordering::SeqCst), 2);

    // The trampoline is gone with the detour, so the lookup must report that
    // instead of handing back a dangling pointer.
    // SAFETY: reading the slot, which now holds `None`.
    assert!(unsafe { original_detour::<Api>(NAME, std::ptr::addr_of!(SLOT)) }.is_none());
}
