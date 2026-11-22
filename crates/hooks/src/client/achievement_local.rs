//! The local achievement rebuild, as the Linux reply path reaches it.
//!
//! `features::achievements::local` turns a local definition set plus the
//! account's unlock state into a coherent achievement list, and
//! [`vapor_forge_features::achievements::rebuild_local_achievements_if_enabled`]
//! refuses to do anything while the flag is off. Nothing on this host used to
//! call it, so `[achievements] local_rebuild = true` was a switch with no wire.
//!
//! This module supplies the three inputs the rebuild needs and nothing else:
//!
//! * **definitions** — parsed from the last stats schema the client received
//!   (`parse_achievement_bit_mappings` owns the keys, stat ids and bits). This is
//!   the only local definition source this host has, so an app whose schema was
//!   never seen has no definitions and is left on the schema path.
//! * **unlock state** — the last snapshot read from Steam's own stats map, kept
//!   per app so a schema reply and the following state reply agree.
//! * **schema** — the same bytes, handed to the rebuild so a schema that parses
//!   keeps owning the wire ids (the rebuild prefers it and falls back to the
//!   local set only when it is missing or unusable).
//!
//! Where it is consulted: whatever is about to answer a stats reply with no
//! usable schema asks [`reply_for`]. With the flag off that call, and the
//! rebuild behind it, return `None` after one `debug` line — and no schema is
//! parsed — so the reply keeps exactly the bytes it had before this module
//! existed.
//!
//! Both caches are bounded and keyed by app: a schema is the largest thing kept
//! (Steam caps one at 256 KiB) and only the definitions parsed from it are ever
//! used, so the byte cache is dropped on an account switch and never grows past
//! [`MAX_CACHED_SCHEMAS`].
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, info};
use vapor_forge_features::achievements::{
    local_rebuild_enabled, rebuild_local_achievements_if_enabled, AchievementUnlockState,
    LocalAchievement, LocalAchievementSet, RebuiltAchievements,
};
use vapor_forge_steam_protocol::parse_achievement_bit_mappings;

/// How many app schemas are kept for the rebuild.
pub(crate) const MAX_CACHED_SCHEMAS: usize = 32;

/// The last stats schema seen per app, as received on the wire.
static SCHEMAS: OnceLock<Mutex<HashMap<u32, Arc<Vec<u8>>>>> = OnceLock::new();
/// The last unlock state read per app.
static UNLOCKS: OnceLock<Mutex<HashMap<u32, Vec<AchievementUnlockState>>>> = OnceLock::new();

fn schemas() -> &'static Mutex<HashMap<u32, Arc<Vec<u8>>>> {
    SCHEMAS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn unlocks() -> &'static Mutex<HashMap<u32, Vec<AchievementUnlockState>>> {
    UNLOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Keep `content` as the app's definition source.
///
/// A second schema for the same app replaces the first: the wire ids a client
/// addresses come from the newest schema Valve sent, so an older one would only
/// rebuild stale ids.
pub(crate) fn remember_schema(app_id: u32, content: &[u8]) {
    if app_id == 0 || content.is_empty() {
        return;
    }
    let mut held = schemas()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.len() >= MAX_CACHED_SCHEMAS && !held.contains_key(&app_id) {
        debug!(
            app_id,
            cached = held.len(),
            "achievements: schema cache is full; the local rebuild is skipped for this app"
        );
        return;
    }
    held.insert(app_id, Arc::new(content.to_vec()));
}

/// Keep the unlock state a snapshot read reported for `app_id`.
pub(crate) fn remember_unlocks(app_id: u32, state: Vec<AchievementUnlockState>) {
    if app_id == 0 {
        return;
    }
    unlocks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(app_id, state);
}

/// Drop everything kept for the account that just went away.
pub(crate) fn forget_account_state() {
    schemas()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
    unlocks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

fn cached_schema(app_id: u32) -> Option<Arc<Vec<u8>>> {
    schemas()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&app_id)
        .cloned()
}

fn cached_unlocks(app_id: u32) -> Vec<AchievementUnlockState> {
    unlocks()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&app_id)
        .cloned()
        .unwrap_or_default()
}

/// The local definitions for `app_id`, parsed from its cached schema.
///
/// `None` when no schema was ever seen for the app or none of it parses: without
/// definitions there is nothing to rebuild from, and the schema path stays the
/// only answer. Duplicate keys keep their first mapping, which is the order the
/// wire ids were declared in.
pub(crate) fn definitions(app_id: u32) -> Option<LocalAchievementSet> {
    let schema = cached_schema(app_id)?;
    let mappings = parse_achievement_bit_mappings(&schema).ok()?;
    let mut seen = std::collections::HashSet::new();
    let achievements = mappings
        .into_iter()
        .filter(|mapping| seen.insert(mapping.key.clone()))
        .map(|mapping| {
            LocalAchievement::new(&mapping.key, mapping.stat_id, mapping.achievement_bit)
        })
        .collect::<Vec<_>>();
    if achievements.is_empty() {
        return None;
    }
    Some(LocalAchievementSet {
        groups: Vec::new(),
        achievements,
    })
}

/// Rebuild `app_id`'s achievement payload, while the flag allows it.
///
/// `None` with the flag off (the pre-rebuild behaviour), with no local
/// definitions to rebuild from, or when the rebuild cannot address a single
/// achievement. The flag is read once up front so a host that never turned the
/// rebuild on does not even parse the schema: the decision itself still goes
/// through `rebuild_local_achievements_if_enabled`, which refuses on its own.
pub(crate) fn rebuild_for_reply(app_id: u32) -> Option<RebuiltAchievements> {
    if !local_rebuild_enabled() {
        debug!(
            app_id,
            "achievements: local rebuild is off; the reply keeps its schema path"
        );
        return None;
    }
    let local = definitions(app_id)?;
    let unlocks = cached_unlocks(app_id);
    let schema = cached_schema(app_id);
    let schema_bytes = schema.as_deref().map(|content| content.as_slice());
    let rebuilt = rebuild_local_achievements_if_enabled(app_id, &local, &unlocks, schema_bytes)?;
    info!(
        app_id,
        source = ?rebuilt.source,
        total = rebuilt.counts.total,
        unlocked = rebuilt.counts.unlocked,
        groups = rebuilt.groups.len(),
        "achievements: local rebuild answered the reply"
    );
    Some(rebuilt)
}

/// The payload to answer a stats reply with, when the schema cannot describe it.
///
/// `schema` is what the reply is about to carry. A usable one keeps the schema
/// path authoritative and this returns `None`; only an absent or unusable schema
/// reaches [`rebuild_for_reply`].
pub(crate) fn reply_for(app_id: u32, schema: &[u8]) -> Option<RebuiltAchievements> {
    if app_id == 0 || !schema.is_empty() {
        debug!(
            app_id,
            bytes = schema.len(),
            "achievements: reply carries a schema; the local rebuild is not consulted"
        );
        return None;
    }
    rebuild_for_reply(app_id)
}

#[cfg(test)]
mod tests;
