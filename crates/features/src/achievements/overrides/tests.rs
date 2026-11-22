//! Composition is a pure function, so every case here is an input and an output.

use std::collections::BTreeMap;

use vapor_forge_core::AchievementIntent;

use super::*;
use crate::achievements::local::{LocalAchievement, LocalAchievementSet};

/// Two achievements the schema declares, so the wire ids exist for both.
fn definitions() -> LocalAchievementSet {
    LocalAchievementSet {
        groups: Vec::new(),
        achievements: vec![
            LocalAchievement::new("ACH_ONE", 1, 0),
            LocalAchievement::new("ACH_TWO", 1, 1),
        ],
    }
}

/// Steam's own map: `ACH_TWO` unlocked, `ACH_ONE` locked.
fn steam() -> Vec<AchievementUnlockState> {
    vec![
        AchievementUnlockState::locked("ACH_ONE"),
        AchievementUnlockState::unlocked("ACH_TWO", Some(1_600_000_000)),
    ]
}

fn overrides(entries: &[(&str, AchievementIntent)]) -> BTreeMap<String, AchievementIntent> {
    entries
        .iter()
        .map(|(key, intent)| ((*key).to_owned(), *intent))
        .collect()
}

#[test]
fn no_overrides_composes_to_what_steam_sent() {
    let composed = compose_unlocks(480, &definitions(), &steam(), None);
    assert_eq!(composed.unlocks, steam(), "byte-for-byte Steam's own map");
    assert_eq!(composed.applied, 0);
    assert_eq!(composed.ignored, 0);
    assert!(!composed.changed);

    // An empty map is the same case: a configured app with nothing written is
    // not a reason to touch the reply.
    let empty = BTreeMap::new();
    let composed = compose_unlocks(480, &definitions(), &steam(), Some(&empty));
    assert_eq!(composed.unlocks, steam());
    assert!(!composed.changed);
}

#[test]
fn one_override_changes_exactly_one_achievement() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[(
            "ACH_ONE",
            AchievementIntent::unlocked_at(1_700_000_000),
        )])),
    );
    assert!(composed.changed);
    assert_eq!(composed.applied, 1);
    assert_eq!(composed.ignored, 0);
    assert_eq!(
        composed.unlocks,
        vec![
            AchievementUnlockState::unlocked("ACH_ONE", Some(1_700_000_000)),
            // Untouched, including its time and its position.
            AchievementUnlockState::unlocked("ACH_TWO", Some(1_600_000_000)),
        ]
    );
}

#[test]
fn the_unlock_time_form_sets_the_time_and_the_flag() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[(
            "ACH_ONE",
            AchievementIntent::unlocked_at(1_234),
        )])),
    );
    let one = composed
        .unlocks
        .iter()
        .find(|state| state.key == "ACH_ONE")
        .expect("ACH_ONE is still in the map");
    assert!(one.unlocked);
    assert_eq!(one.unlock_time, Some(1_234));
}

#[test]
fn true_unlocks_with_no_time_known() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[("ACH_ONE", AchievementIntent::unlocked())])),
    );
    let one = composed
        .unlocks
        .iter()
        .find(|state| state.key == "ACH_ONE")
        .expect("ACH_ONE is still in the map");
    assert!(one.unlocked);
    assert_eq!(one.unlock_time, None, "the rebuild writes Steam's sentinel");
}

#[test]
fn false_clears_an_unlock_and_its_time() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[("ACH_TWO", AchievementIntent::locked())])),
    );
    assert!(composed.changed);
    assert_eq!(
        composed.unlocks,
        vec![
            AchievementUnlockState::locked("ACH_ONE"),
            AchievementUnlockState::locked("ACH_TWO"),
        ],
        "a lock drops the time as well as the flag"
    );
}

#[test]
fn an_override_for_a_key_the_schema_does_not_define_is_ignored() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[
            ("ACH_MADE_UP", AchievementIntent::unlocked()),
            ("ACH_ONE", AchievementIntent::unlocked()),
        ])),
    );
    assert_eq!(composed.applied, 1, "only the defined key is applied");
    assert_eq!(composed.ignored, 1);
    assert!(
        composed
            .unlocks
            .iter()
            .all(|state| state.key != "ACH_MADE_UP"),
        "an undefined key is never invented"
    );
    assert!(composed.changed);
}

#[test]
fn only_undefined_keys_means_nothing_to_rewrite() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[("ACH_NOPE", AchievementIntent::unlocked())])),
    );
    assert_eq!(composed.applied, 0);
    assert_eq!(composed.ignored, 1);
    assert!(!composed.changed, "an ignored override is not a change");
    assert_eq!(composed.unlocks, steam());
}

#[test]
fn an_override_that_agrees_with_steam_is_not_a_change() {
    let composed = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[(
            "ACH_TWO",
            AchievementIntent::unlocked_at(1_600_000_000),
        )])),
    );
    assert_eq!(composed.applied, 1, "the key was addressed");
    assert!(
        !composed.changed,
        "but it asked for what Steam already said"
    );
    assert_eq!(composed.unlocks, steam());
}

#[test]
fn an_override_for_a_defined_key_steam_omitted_is_appended() {
    // A snapshot that never mentioned the key: the definition still names it, so
    // the override is addressed and the entry has to be added, not replaced.
    let composed = compose_unlocks(
        480,
        &definitions(),
        &[AchievementUnlockState::locked("ACH_ONE")],
        Some(&overrides(&[(
            "ACH_TWO",
            AchievementIntent::unlocked_at(99),
        )])),
    );
    assert!(composed.changed);
    assert_eq!(
        composed.unlocks,
        vec![
            AchievementUnlockState::locked("ACH_ONE"),
            AchievementUnlockState::unlocked("ACH_TWO", Some(99)),
        ]
    );
}

#[test]
fn composition_is_deterministic_regardless_of_the_map_order() {
    let first = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[
            ("ACH_ONE", AchievementIntent::unlocked_at(5)),
            ("ACH_TWO", AchievementIntent::locked()),
        ])),
    );
    let second = compose_unlocks(
        480,
        &definitions(),
        &steam(),
        Some(&overrides(&[
            ("ACH_TWO", AchievementIntent::locked()),
            ("ACH_ONE", AchievementIntent::unlocked_at(5)),
        ])),
    );
    assert_eq!(first, second, "the map is sorted, so order cannot leak out");
}
