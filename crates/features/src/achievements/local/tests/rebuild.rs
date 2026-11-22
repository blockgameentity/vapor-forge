//! The list itself: what the rebuild emits, from which source, and what it keeps
//! when the input is broken.

use super::super::*;
use super::fixtures::*;

#[test]
fn an_empty_schema_rebuilds_the_local_set_with_counts_that_match() {
    let rebuilt = rebuild_local_achievements(APP_ID, &local_set(), &unlocks(), Some(&[]));

    assert_eq!(rebuilt.app_id, APP_ID);
    assert_eq!(rebuilt.source, RebuildSource::Local);
    assert_eq!(
        keys(&rebuilt),
        vec![
            "ACH_FIRST_STEPS",
            "ACH_SHARPSHOOTER",
            "ACH_SECRET",
            "ACH_FINALE"
        ],
        "wire order: stat id, then bit, then key"
    );
    assert_eq!(
        rebuilt.counts,
        AchievementCounts {
            unlocked: 2,
            total: 4
        }
    );
    assert_eq!(
        rebuilt
            .achievements
            .iter()
            .filter(|achievement| achievement.unlocked)
            .count(),
        2,
        "the count has to describe the list it is attached to"
    );

    let first_steps = achievement(&rebuilt, "ACH_FIRST_STEPS");
    assert_eq!(first_steps.display.display_name, "First Steps");
    assert_eq!(first_steps.display.description, "Leave the vault");
    assert_eq!(first_steps.unlock_time, Some(1_700_000_000));

    // Unlocked without a known time still reports an unlocked bit.
    let finale = achievement(&rebuilt, "ACH_FINALE");
    assert!(finale.unlocked);
    assert_eq!(
        finale.unlock_time,
        Some(vapor_forge_steam_protocol::ACHIEVEMENT_UNLOCK_TIME_UNKNOWN)
    );

    // A locked achievement never carries a time, and an absent unlock state is locked.
    assert!(!achievement(&rebuilt, "ACH_SHARPSHOOTER").unlocked);
    assert_eq!(achievement(&rebuilt, "ACH_SHARPSHOOTER").unlock_time, None);
    assert!(!achievement(&rebuilt, "ACH_SECRET").unlocked);
}

#[test]
fn a_present_schema_wins_and_the_unlock_state_is_still_applied() {
    // The schema moves ACH_FINALE to another stat/bit, adds a key the local set
    // never heard of, and leaves out two the local set knows.
    let schema =
        achievements_schema(&[(10, &["ACH_FIRST_STEPS"]), (12, &["ACH_FINALE", "ACH_NEW"])]);
    let rebuilt = rebuild_local_achievements(APP_ID, &local_set(), &unlocks(), Some(&schema));

    assert_eq!(rebuilt.source, RebuildSource::Schema);
    assert_eq!(
        keys(&rebuilt),
        vec!["ACH_FIRST_STEPS", "ACH_FINALE", "ACH_NEW"],
        "the schema owns the list"
    );
    assert_eq!(
        rebuilt.counts,
        AchievementCounts {
            unlocked: 2,
            total: 3
        }
    );

    let finale = achievement(&rebuilt, "ACH_FINALE");
    assert_eq!(
        (finale.stat_id, finale.bit),
        (12, 0),
        "the schema owns the wire id"
    );
    assert!(
        finale.unlocked,
        "the unlock state is keyed by name, not by wire id"
    );
    assert_eq!(
        finale.display.display_name, "Finale",
        "local data fills display text"
    );
    assert_eq!(
        finale.groups,
        vec!["combat".to_owned(), "story".to_owned()],
        "groups survive a schema that cannot carry them"
    );

    let new = achievement(&rebuilt, "ACH_NEW");
    assert_eq!(
        new.display.display_name, "ACH_NEW",
        "an unknown key is shown as itself"
    );
    assert!(!new.unlocked);
    assert!(new.groups.is_empty());
    assert_eq!(
        group(&rebuilt, "combat").members,
        vec!["ACH_FINALE".to_owned()]
    );
    assert_eq!(
        group(&rebuilt, "story").members,
        vec!["ACH_FINALE".to_owned()]
    );
}

#[test]
fn a_grouped_layout_keeps_every_membership_without_duplicating_the_achievement() {
    let rebuilt = rebuild_local_achievements(APP_ID, &local_set(), &[], None);

    assert_eq!(rebuilt.achievements.len(), 4, "one row per achievement");
    assert_eq!(
        keys(&rebuilt),
        vec![
            "ACH_FIRST_STEPS",
            "ACH_SHARPSHOOTER",
            "ACH_SECRET",
            "ACH_FINALE"
        ]
    );

    let combat = group(&rebuilt, "combat");
    let story = group(&rebuilt, "story");
    assert_eq!(combat.display_name, "Combat");
    assert_eq!(story.display_name, "Story");
    assert_eq!(
        combat.members,
        vec!["ACH_FINALE".to_owned(), "ACH_SHARPSHOOTER".to_owned()]
    );
    assert_eq!(
        story.members,
        vec!["ACH_FINALE".to_owned(), "ACH_SECRET".to_owned()]
    );
    assert_eq!(
        achievement(&rebuilt, "ACH_FINALE").groups,
        vec!["combat".to_owned(), "story".to_owned()],
        "an achievement in two groups keeps both memberships"
    );
    assert!(achievement(&rebuilt, "ACH_SECRET").display.hidden);
}

#[test]
fn a_schema_whose_achievements_are_nested_under_groups_falls_back_to_the_local_set() {
    // This is the layout that used to blank the page: the flat reader finds no
    // achievement at all, so the response had definitions for nothing.
    let schema = grouped_schema(10, &[("combat", &["ACH_FIRST_STEPS", "ACH_SHARPSHOOTER"])]);
    let rebuilt = rebuild_local_achievements(APP_ID, &local_set(), &unlocks(), Some(&schema));

    assert_eq!(rebuilt.source, RebuildSource::Local);
    assert_eq!(rebuilt.achievements.len(), 4);
    assert_eq!(
        rebuilt.counts,
        AchievementCounts {
            unlocked: 2,
            total: 4
        }
    );
    assert_eq!(
        group(&rebuilt, "combat").members,
        vec!["ACH_FINALE".to_owned(), "ACH_SHARPSHOOTER".to_owned()]
    );
}

#[test]
fn malformed_local_data_costs_only_the_bad_entries() {
    let set = LocalAchievementSet {
        groups: vec![LocalAchievementGroup::new("", "blank group")],
        achievements: vec![
            LocalAchievement {
                bit: MAX_LOCAL_ACHIEVEMENT_BIT + 1,
                ..LocalAchievement::new("ACH_UNADDRESSABLE", 10, 0)
            },
            LocalAchievement::new("ACH_OK", 10, 1),
            LocalAchievement::new("ACH_REUSES_THE_BIT", 10, 1),
            LocalAchievement::new("", 10, 2),
        ],
    };
    let unlocks = vec![AchievementUnlockState::unlocked(
        "ACH_NOT_IN_THE_LIST",
        Some(5),
    )];
    let rebuilt = rebuild_local_achievements(APP_ID, &set, &unlocks, None);

    assert_eq!(keys(&rebuilt), vec!["ACH_OK"]);
    assert_eq!(
        rebuilt.counts,
        AchievementCounts {
            unlocked: 0,
            total: 1
        }
    );
    assert!(
        rebuilt.groups.is_empty(),
        "a group with a blank key is never rendered"
    );
}

#[test]
fn the_flag_off_leaves_the_response_path_untouched() {
    // The only test that touches the process-global gate; it restores it.
    set_local_rebuild(false);
    assert!(!local_rebuild_enabled());
    assert_eq!(
        rebuild_local_achievements_if_enabled(APP_ID, &local_set(), &unlocks(), None),
        None,
        "with the flag off the caller stays on the schema path"
    );

    set_local_rebuild(true);
    assert!(local_rebuild_enabled());
    let rebuilt = rebuild_local_achievements_if_enabled(APP_ID, &local_set(), &unlocks(), None)
        .expect("the enabled gate returns the rebuilt list");
    assert_eq!(rebuilt.source, RebuildSource::Local);

    set_local_rebuild(false);
    assert!(!local_rebuild_enabled());
}
