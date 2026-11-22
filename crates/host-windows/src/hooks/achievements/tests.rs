//! The body's own properties on Linux: the dispatch values it classifies, the
//! forwarding contract, and the fail-closed path with no published trampoline.

use core::ffi::c_void;

use super::*;

fn serial() -> std::sync::MutexGuard<'static, ()> {
    crate::test_lock::serial()
}

#[test]
fn the_dispatch_values_are_the_ones_the_function_returns_on() {
    // Measured from both builds' entry; the other 32-bit values merge.
    for refused in [2, 3, 16] {
        assert_eq!(MergeOutcome::of(refused), MergeOutcome::Refused);
    }
    for merged in [0, 1, 4, 5, 7, 8, 12, 15, 17, 20, i32::MAX, i32::MIN] {
        assert_eq!(MergeOutcome::of(merged), MergeOutcome::Merges);
    }
    assert_eq!(MergeOutcome::Refused.label(), "refused");
}

#[test]
fn the_body_without_a_published_original_returns_instead_of_calling_out() {
    // No trampoline is published in this test, so the body fails closed before it
    // reads any state or touches `this`. The lock keeps the forwarding test below
    // from publishing one underneath this call.
    let _guard = serial();
    original::forget(SYMBOL);
    // SAFETY: called with a null receiver and no live Steam frame; every path this
    // test can take returns before touching memory.
    unsafe { merge_stats(std::ptr::null_mut(), 1, 0, 0) };
}

/// What the fake trampoline was called with.
static FORWARDED: std::sync::Mutex<Option<(i32, usize, usize)>> = std::sync::Mutex::new(None);

unsafe extern "system" fn fake_merge(_this: *mut c_void, eresult: i32, r8: usize, r9: usize) {
    *FORWARDED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((eresult, r8, r9));
}

#[test]
fn the_body_forwards_every_slot_verbatim() {
    // The load-bearing property of this body: whatever Steam passed reaches the
    // displaced original unchanged, including the two slots whose meaning is only
    // Assumed. Nothing here may filter, rewrite or drop a slot.
    let _guard = serial();
    original::record(SYMBOL, fake_merge as MergeStatsFn as usize);
    // SAFETY: the published "trampoline" is `fake_merge`, whose signature is exactly
    // `MergeStatsFn`'s; nothing dereferences `this` because no state is published in
    // this process, so the body takes the gated path.
    unsafe { merge_stats(0x1000 as *mut c_void, 5, 0x11, 0x22) };
    let forwarded = (*FORWARDED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()))
    .take();
    original::forget(SYMBOL);
    assert_eq!(forwarded, Some((5, 0x11, 0x22)));
}

#[test]
fn the_body_is_registered_under_its_anchored_name() {
    assert_eq!(SYMBOL, "CAPIJobRequestUserStats::YieldingMergeStats");
    assert_ne!(address(), 0);
}
