//! Shared fixtures: one local set with a grouped layout, one account state, and
//! the binary KeyValues schemas the rebuild has to agree with.

use super::super::*;

pub(super) const APP_ID: u32 = 3_456_790;

pub(super) fn local_set() -> LocalAchievementSet {
    LocalAchievementSet {
        groups: vec![
            LocalAchievementGroup::new("combat", "Combat"),
            LocalAchievementGroup::new("story", "Story"),
        ],
        achievements: vec![
            LocalAchievement {
                display: AchievementDisplay {
                    display_name: "First Steps".to_owned(),
                    description: "Leave the vault".to_owned(),
                    hidden: false,
                },
                ..LocalAchievement::new("ACH_FIRST_STEPS", 10, 0)
            },
            LocalAchievement {
                display: AchievementDisplay {
                    display_name: "Sharpshooter".to_owned(),
                    description: "Ten headshots".to_owned(),
                    hidden: false,
                },
                groups: vec!["combat".to_owned()],
                ..LocalAchievement::new("ACH_SHARPSHOOTER", 10, 3)
            },
            LocalAchievement {
                display: AchievementDisplay {
                    display_name: "Finale".to_owned(),
                    description: "Finish the story".to_owned(),
                    hidden: false,
                },
                // Listed in two groups on purpose: one achievement, two memberships.
                groups: vec!["combat".to_owned(), "story".to_owned()],
                ..LocalAchievement::new("ACH_FINALE", 11, 1)
            },
            LocalAchievement {
                display: AchievementDisplay {
                    display_name: "The Secret".to_owned(),
                    description: "Find the hidden door".to_owned(),
                    hidden: true,
                },
                groups: vec!["story".to_owned()],
                ..LocalAchievement::new("ACH_SECRET", 11, 0)
            },
        ],
    }
}

pub(super) fn unlocks() -> Vec<AchievementUnlockState> {
    vec![
        AchievementUnlockState::unlocked("ACH_FIRST_STEPS", Some(1_700_000_000)),
        // Unlocked without a time: the flag is authoritative, the time is not.
        AchievementUnlockState::unlocked("ACH_FINALE", None),
        AchievementUnlockState::locked("ACH_SHARPSHOOTER"),
    ]
}

/// A binary KeyValues stats schema shaped like Valve's own: one achievements stat
/// per entry, each bit naming an achievement key.
pub(super) fn achievements_schema(stats: &[(u32, &[&str])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(0); // document start: a type-0 value ...
    bytes.extend_from_slice(b"root\0"); // ... whose key is the root name
    bytes.push(0);
    bytes.extend_from_slice(b"stats\0");
    for (stat_id, keys) in stats {
        bytes.push(0);
        bytes.extend_from_slice(format!("{stat_id}\0").as_bytes());
        bytes.push(1);
        bytes.extend_from_slice(b"type\0ACHIEVEMENTS\0");
        bytes.push(0);
        bytes.extend_from_slice(b"bits\0");
        for (bit, key) in keys.iter().enumerate() {
            bytes.push(0);
            bytes.extend_from_slice(format!("{bit}\0").as_bytes());
            bytes.push(1);
            bytes.extend_from_slice(b"name\0");
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(0);
            bytes.push(8); // end of the bit's achievement object
        }
        bytes.push(8); // end of bits
        bytes.push(8); // end of the stat
    }
    bytes.push(8); // end of stats
    bytes.push(8); // end of root
    bytes
}

/// A modern layout: achievements nested under named groups, not under a stat's
/// flat `bits` map.
pub(super) fn grouped_schema(stat_id: u32, groups: &[(&str, &[&str])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(0);
    bytes.extend_from_slice(b"root\0");
    bytes.push(0);
    bytes.extend_from_slice(b"stats\0");
    bytes.push(0);
    bytes.extend_from_slice(format!("{stat_id}\0").as_bytes());
    bytes.push(1);
    bytes.extend_from_slice(b"type\0ACHIEVEMENTS\0");
    bytes.push(0);
    bytes.extend_from_slice(b"groups\0");
    for (group_key, keys) in groups {
        bytes.push(0);
        bytes.extend_from_slice(format!("{group_key}\0").as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(b"bits\0");
        for (bit, key) in keys.iter().enumerate() {
            bytes.push(0);
            bytes.extend_from_slice(format!("{bit}\0").as_bytes());
            bytes.push(1);
            bytes.extend_from_slice(b"name\0");
            bytes.extend_from_slice(key.as_bytes());
            bytes.push(0);
            bytes.push(8);
        }
        bytes.push(8); // end of bits
        bytes.push(8); // end of the group
    }
    bytes.push(8); // end of groups
    bytes.push(8); // end of the stat
    bytes.push(8); // end of stats
    bytes.push(8); // end of root
    bytes
}

pub(super) fn keys(rebuilt: &RebuiltAchievements) -> Vec<&str> {
    rebuilt
        .achievements
        .iter()
        .map(|achievement| achievement.key.as_str())
        .collect()
}

pub(super) fn achievement<'a>(
    rebuilt: &'a RebuiltAchievements,
    key: &str,
) -> &'a RebuiltAchievement {
    rebuilt
        .achievements
        .iter()
        .find(|achievement| achievement.key == key)
        .unwrap_or_else(|| panic!("{key} is missing from the rebuilt list"))
}

pub(super) fn group<'a>(
    rebuilt: &'a RebuiltAchievements,
    key: &str,
) -> &'a RebuiltAchievementGroup {
    rebuilt
        .groups
        .iter()
        .find(|group| group.key == key)
        .unwrap_or_else(|| panic!("group {key} is missing from the rebuilt list"))
}
