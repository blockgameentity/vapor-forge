//! The rate budget and the name interning, in isolation from the thread.

use super::*;

#[test]
fn the_budget_is_the_paced_rate_capped_hard() {
    assert_eq!(Budget::per_minute(0), MAX_PER_MINUTE);
    assert_eq!(Budget::per_minute(1), MAX_PER_MINUTE);
    assert_eq!(Budget::per_minute(1_000), MAX_PER_MINUTE);
    assert_eq!(Budget::per_minute(2_000), MAX_PER_MINUTE);
    assert_eq!(Budget::per_minute(5_000), 12);
    assert_eq!(Budget::per_minute(60_000), 1);
    // A pace slower than the window still allows one per window, never zero.
    assert_eq!(Budget::per_minute(600_000), 1);
}

#[test]
fn the_budget_refuses_once_spent_and_refills_a_minute_later() {
    let mut budget = Budget::new(2);
    // Anchored on the budget's own window start, so the refill is asserted
    // exactly rather than approximately.
    let start = budget.window;
    assert!(budget.take(start));
    assert!(budget.take(start));
    assert!(!budget.take(start), "the third request in the window is refused");
    assert!(budget.take(start + WINDOW));
    assert!(budget.take(start + WINDOW));
    assert!(!budget.take(start + WINDOW));
}

#[test]
fn provider_names_are_interned_once_and_the_table_is_bounded() {
    let mut names = Names::default();
    let first = names.intern("Configured.Host");
    assert_eq!(first, "Configured.Host");
    assert_eq!(names.intern("Configured.Host"), first, "one leak per name");
    for index in 0..MAX_INTERNED * 2 {
        let name = format!("provider-{index}");
        let interned = names.intern(&name);
        // Past the bound a code is still cached, under the fallback name.
        assert!(interned == UNKNOWN_PROVIDER || interned == name);
    }
    assert_eq!(names.interned.len(), MAX_INTERNED);
}
