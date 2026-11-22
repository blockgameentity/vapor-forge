#![forbid(unsafe_code)]
use super::achievement_local;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tracing::debug;

static NO_SCHEMA: OnceLock<Mutex<HashSet<u32>>> = OnceLock::new();

/// Valve answered a donor stats request without a schema for this app.
///
/// The donor request clears the client's cached schema tokens precisely so Valve
/// cannot answer "unchanged", so an empty schema means there is no schema: the app
/// declares no achievements and no stats. There is no snapshot layout to read.
///
/// The cached schema bytes are deliberately kept: they are this host's only
/// local definition source, and a reply that arrives with no schema is exactly
/// the case the local rebuild was written for. Nothing is rebuilt here — the
/// rebuild is asked for at the reply site, see [`local_reply`].
pub(crate) fn note_schema_unavailable(app_id: u32) {
    if app_id == 0 {
        return;
    }
    super::user_stats::remove_snapshot_schema(app_id);
    let first = NO_SCHEMA
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(app_id);
    if first {
        debug!(app_id, "achievement receiver: app declares no stats schema");
    }
}

/// Steam received an app's stats schema on the wire.
///
/// The schema arrival establishes that Steam's stats map can be read directly,
/// and it is kept as the app's local definition set so a later reply without a
/// schema can still be rebuilt locally.
pub(crate) fn register_packet_schema(app_id: u32, content: &[u8]) -> bool {
    if content.is_empty() {
        return false;
    }
    // A schema did arrive after all, so a previous absence no longer holds. The
    // donor can change, and so can the app.
    if let Some(seen) = NO_SCHEMA.get() {
        if let Ok(mut seen) = seen.lock() {
            seen.remove(&app_id);
        }
    }
    debug!(
        app_id,
        bytes = content.len(),
        "achievement receiver: schema registered"
    );
    achievement_local::remember_schema(app_id, content);
    // The definition set changed, so the local rebuild's view of this app is
    // refreshed now rather than at the next reply: with the flag off this is the
    // one `debug` line the feature emits, and nothing is computed.
    let _ = achievement_local::rebuild_for_reply(app_id);
    super::user_stats::register_snapshot_schema(app_id, content)
}

/// The achievement payload to answer an app's stats reply with, when the reply
/// cannot carry a usable schema.
///
/// This is the one call into `features::achievements`' local rebuild on this
/// host. `None` means "the reply keeps today's bytes": the flag is off, the reply
/// carries its own schema, or nothing local is known about the app.
pub(crate) fn local_reply(
    app_id: u32,
    schema: &[u8],
) -> Option<vapor_forge_features::achievements::RebuiltAchievements> {
    achievement_local::reply_for(app_id, schema)
}

pub(crate) fn observe_local_snapshot(
    snapshot: super::user_stats::AchievementSnapshot,
    intent: &super::user_stats::StatsSnapshotIntent,
) -> bool {
    let app_id = snapshot.app_id;
    let achievements = snapshot.achievements.len();
    let stats = snapshot.stats.len();
    achievement_local::remember_unlocks(app_id, unlock_state(&snapshot));
    let persisted = crate::achievement_worker::queue_official_snapshot(snapshot, intent);
    debug!(
        app_id,
        achievements, stats, persisted, "achievement receiver: official Steam snapshot observed"
    );
    persisted
}

/// The snapshot's unlock state, in the shape the local rebuild reads.
fn unlock_state(
    snapshot: &super::user_stats::AchievementSnapshot,
) -> Vec<vapor_forge_features::achievements::AchievementUnlockState> {
    use vapor_forge_features::achievements::AchievementUnlockState;
    snapshot
        .achievements
        .iter()
        .map(|achievement| AchievementUnlockState {
            key: achievement.key.clone(),
            unlocked: achievement.unlocked,
            // Zero is "unlocked, time unknown" on the wire, which the rebuild
            // turns into Steam's sentinel rather than a fabricated timestamp.
            unlock_time: (achievement.unlocked && achievement.unlock_time != 0)
                .then_some(achievement.unlock_time),
        })
        .collect()
}
