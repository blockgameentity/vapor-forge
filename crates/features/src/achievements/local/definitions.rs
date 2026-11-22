//! Definition resolution: where the rebuilt list's entries come from.
//!
//! A usable schema wins. Everything the schema cannot carry (display text,
//! group membership) is taken from the local set, and a key the schema declares
//! but the local set does not know is still emitted, under its own key, so the
//! list stays complete.

use std::collections::{BTreeMap, BTreeSet};

use tracing::{debug, warn};
use vapor_forge_steam_protocol::{
    parse_achievement_bit_mappings, AchievementBitMapping, ACHIEVEMENT_UNLOCK_TIME_UNKNOWN,
};

use super::model::{
    AchievementUnlockState, LocalAchievement, LocalAchievementSet, LocalDefinitionError,
};
use super::rebuild::RebuiltAchievement;

/// The schema's achievements, or `None` when there is nothing to trust.
pub(super) fn schema_mappings(
    schema: Option<&[u8]>,
    app_id: u32,
) -> Option<Vec<AchievementBitMapping>> {
    let bytes = schema.filter(|bytes| !bytes.is_empty())?;
    match parse_achievement_bit_mappings(bytes) {
        Ok(mappings) if !mappings.is_empty() => Some(mappings),
        Ok(_) => {
            debug!(
                app_id,
                bytes = bytes.len(),
                "achievements: schema declares no readable achievements, rebuilding from the local set"
            );
            None
        }
        Err(error) => {
            warn!(
                app_id,
                bytes = bytes.len(),
                error = ?error,
                "achievements: schema is unreadable, rebuilding from the local set"
            );
            None
        }
    }
}

/// The local definitions, as the rebuild consumes them.
pub(super) fn local_definitions(local: &LocalAchievementSet) -> Vec<LocalAchievement> {
    local.achievements.clone()
}

/// Turn schema mappings into definitions, filling in what a schema cannot carry.
pub(super) fn schema_definitions(
    local: &LocalAchievementSet,
    mappings: Vec<AchievementBitMapping>,
    app_id: u32,
) -> Vec<LocalAchievement> {
    let mut by_key: BTreeMap<&str, &LocalAchievement> = BTreeMap::new();
    for achievement in &local.achievements {
        if by_key.contains_key(achievement.key.as_str()) {
            debug!(
                key = achievement.key.as_str(),
                "achievements: duplicate local achievement key ignored"
            );
            continue;
        }
        by_key.insert(achievement.key.as_str(), achievement);
    }

    mappings
        .into_iter()
        .map(|mapping| match by_key.get(mapping.key.as_str()) {
            Some(definition) => {
                if definition.stat_id != mapping.stat_id
                    || definition.bit != mapping.achievement_bit
                {
                    debug!(
                        app_id,
                        key = mapping.key.as_str(),
                        local_stat = definition.stat_id,
                        local_bit = definition.bit,
                        schema_stat = mapping.stat_id,
                        schema_bit = mapping.achievement_bit,
                        "achievements: schema wire id replaces the local one"
                    );
                }
                LocalAchievement {
                    key: mapping.key,
                    stat_id: mapping.stat_id,
                    bit: mapping.achievement_bit,
                    display: definition.display.clone(),
                    groups: definition.groups.clone(),
                }
            }
            None => LocalAchievement::new(&mapping.key, mapping.stat_id, mapping.achievement_bit),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// List building
// ---------------------------------------------------------------------------

/// Unlock state indexed by achievement key. The first entry for a key wins so the
/// caller's order cannot make the result depend on how a list was assembled.
pub(super) type UnlockMap<'a> = BTreeMap<&'a str, &'a AchievementUnlockState>;

/// Index the unlock state by key, dropping what cannot be matched.
pub(super) fn unlock_map(unlocks: &[AchievementUnlockState]) -> UnlockMap<'_> {
    let mut map = BTreeMap::new();
    for state in unlocks {
        if state.key.trim().is_empty() {
            debug!("achievements: unlock state with a blank key ignored");
            continue;
        }
        if map.contains_key(state.key.as_str()) {
            debug!(
                key = state.key.as_str(),
                "achievements: duplicate unlock state ignored, the first one wins"
            );
            continue;
        }
        map.insert(state.key.as_str(), state);
    }
    map
}

/// Turn definitions into the emitted list: validate, de-duplicate wire ids,
/// apply unlock state, and sort into output order.
///
/// A definition that cannot be rendered or addressed is skipped with a warning:
/// a damaged local catalog must cost that achievement, never the whole page.
pub(super) fn build_achievements(
    definitions: Vec<LocalAchievement>,
    unlocks: &UnlockMap<'_>,
) -> Vec<RebuiltAchievement> {
    let mut wire_ids: BTreeMap<(u32, u32), String> = BTreeMap::new();
    let mut achievements = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if let Err(error) = definition.validate() {
            warn!(
                key = definition.key.as_str(),
                %error,
                "achievements: skipping malformed local definition"
            );
            continue;
        }
        let wire_id = (definition.stat_id, definition.bit);
        if let Some(first) = wire_ids.get(&wire_id) {
            let error = LocalDefinitionError::DuplicateWireId {
                key: definition.key.clone(),
                first: first.clone(),
                stat_id: definition.stat_id,
                bit: definition.bit,
            };
            warn!(
                key = definition.key.as_str(),
                %error,
                "achievements: skipping definition that reuses a wire id"
            );
            continue;
        }
        wire_ids.insert(wire_id, definition.key.clone());

        let (unlocked, unlock_time) =
            effective_unlock(unlocks.get(definition.key.as_str()).copied());
        achievements.push(RebuiltAchievement {
            key: definition.key,
            stat_id: definition.stat_id,
            bit: definition.bit,
            display: definition.display,
            groups: normalized_groups(&definition.groups),
            unlocked,
            unlock_time,
        });
    }
    achievements.sort_by(|left, right| {
        (left.stat_id, left.bit, left.key.as_str()).cmp(&(
            right.stat_id,
            right.bit,
            right.key.as_str(),
        ))
    });
    achievements
}

/// The state and the wire time for one achievement.
///
/// The unlock flag is authoritative. The time is not: an unlocked achievement
/// whose time is missing or unusable (rtime32 zero is not a real time) is
/// encoded with Steam's sentinel, which is what the client backfills itself when
/// it knows a bit is set but not when.
fn effective_unlock(state: Option<&AchievementUnlockState>) -> (bool, Option<u32>) {
    let Some(state) = state else {
        return (false, None);
    };
    if !state.unlocked {
        return (false, None);
    }
    let time = state
        .unlock_time
        .filter(|time| *time != 0)
        .unwrap_or(ACHIEVEMENT_UNLOCK_TIME_UNKNOWN);
    (true, Some(time))
}

/// Group keys of one achievement: blank entries dropped, sorted, de-duplicated.
fn normalized_groups(groups: &[String]) -> Vec<String> {
    let mut normalized: Vec<String> = groups
        .iter()
        .filter(|group| !group.trim().is_empty())
        .cloned()
        .collect();
    normalized.sort();
    normalized.dedup();
    normalized
}

/// Say so when the account's state names an achievement the list does not
/// contain (a schema revision, or a stale record): it is ignored, not invented.
pub(super) fn report_unknown_unlocks(achievements: &[RebuiltAchievement], unlocks: &UnlockMap<'_>) {
    let known: BTreeSet<&str> = achievements
        .iter()
        .map(|achievement| achievement.key.as_str())
        .collect();
    for key in unlocks.keys() {
        if !known.contains(key) {
            debug!(
                key,
                "achievements: unlock state for an achievement that is not in the list ignored"
            );
        }
    }
}
