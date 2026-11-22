//! Group membership of the rebuilt list.
//!
//! A modern achievement layout nests definitions under groups, and one
//! achievement may be listed in several. The rebuilt list keeps the achievement
//! once and its memberships all: collapsing to a flat list would lose the page
//! layout, and de-duplicating memberships by achievement would drop the second
//! group.

use std::collections::{BTreeMap, BTreeSet};

use tracing::{debug, warn};

use super::model::LocalAchievementSet;
use super::rebuild::{RebuiltAchievement, RebuiltAchievementGroup};

pub(super) fn build_groups(
    local: &LocalAchievementSet,
    achievements: &[RebuiltAchievement],
) -> Vec<RebuiltAchievementGroup> {
    let mut declared: BTreeMap<&str, &str> = BTreeMap::new();
    for group in &local.groups {
        if let Err(error) = group.validate() {
            warn!(%error, "achievements: skipping malformed local group");
            continue;
        }
        declared
            .entry(group.key.as_str())
            .or_insert(group.display_name.as_str());
    }

    let mut members: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for achievement in achievements {
        for group in &achievement.groups {
            members
                .entry(group.as_str())
                .or_default()
                .insert(achievement.key.as_str());
        }
    }
    for group in declared.keys() {
        if !members.contains_key(group) {
            debug!(
                group,
                "achievements: local group has no rebuilt member and is not listed"
            );
        }
    }

    members
        .into_iter()
        .map(|(key, members)| RebuiltAchievementGroup {
            key: key.to_owned(),
            display_name: declared
                .get(key)
                .map(|name| (*name).to_owned())
                // A membership whose group has no local entry still has to be
                // shown, so the key stands in for the missing display name
                // rather than the membership being dropped.
                .unwrap_or_else(|| key.to_owned()),
            members: members.into_iter().map(str::to_owned).collect(),
        })
        .collect()
}
