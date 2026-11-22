//! Both directions of the local achievement rebuild wiring.
//!
//! `features::achievements::local` tests the rebuild itself (schema wins, local
//! set fills a gap, unknown unlocks are ignored, the output is deterministic).
//! These tests prove only that the Linux reply path reaches it, and that with the
//! flag off nothing is reached at all.
use super::*;

/// One achievements stat (id 12) holding one achievement at bit 3, in Steam's
/// binary KeyValues schema shape.
fn schema() -> Vec<u8> {
    fn kv_string(out: &mut Vec<u8>, key: &str, value: &str) {
        out.push(1);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    fn kv_int(out: &mut Vec<u8>, key: &str, value: i32) {
        out.push(2);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn kv_object(out: &mut Vec<u8>, key: &str, body: impl FnOnce(&mut Vec<u8>)) {
        out.push(0);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        body(out);
        out.push(8);
    }
    let mut schema = Vec::new();
    kv_object(&mut schema, "480", |root| {
        kv_object(root, "stats", |stats| {
            kv_object(stats, "12", |stat| {
                kv_int(stat, "type", 4);
                kv_object(stat, "bits", |bits| {
                    kv_object(bits, "3", |achievement| {
                        kv_string(achievement, "name", "ACH_WIN");
                    });
                });
            });
        });
    });
    schema
}

fn unlocked(key: &str) -> AchievementUnlockState {
    AchievementUnlockState::unlocked(key, Some(1_700_000_000))
}

/// Serialises every test in this module: the rebuild switch and both caches are
/// process-wide, so one test at a time keeps them honest.
static SWITCH: Mutex<()> = Mutex::new(());

fn guard() -> std::sync::MutexGuard<'static, ()> {
    SWITCH.lock().unwrap_or_else(|error| error.into_inner())
}

fn with_flag_on<T>(body: impl FnOnce() -> T) -> T {
    vapor_forge_features::achievements::set_local_rebuild(true);
    let value = body();
    vapor_forge_features::achievements::set_local_rebuild(false);
    value
}

fn seed(app_id: u32) {
    remember_schema(app_id, &schema());
    remember_unlocks(app_id, vec![unlocked("ACH_WIN")]);
}

#[test]
fn flag_off_reaches_no_rebuild_and_keeps_todays_reply() {
    let _guard = guard();
    let app_id = 3_100;
    seed(app_id);
    assert!(!vapor_forge_features::achievements::local_rebuild_enabled());
    assert_eq!(rebuild_for_reply(app_id), None, "the flag is off");
    assert_eq!(
        reply_for(app_id, &[]),
        None,
        "a reply with no schema gets no rebuilt state while the flag is off"
    );
    // The definitions are still there: they are what a flag-on host would use,
    // and remembering them is not a behaviour change.
    assert!(definitions(app_id).is_some());
}

#[test]
fn flag_on_uses_the_shared_rebuild_and_its_definitions() {
    let _guard = guard();
    let app_id = 3_101;
    seed(app_id);
    let rebuilt = with_flag_on(|| reply_for(app_id, &[]))
        .expect("the flag is on and the definitions are known");
    assert_eq!(rebuilt.app_id, app_id);
    assert_eq!(rebuilt.counts.total, 1);
    assert_eq!(rebuilt.counts.unlocked, 1);
    assert_eq!(rebuilt.achievements[0].key, "ACH_WIN");
    assert_eq!(rebuilt.achievements[0].stat_id, 12);
    assert_eq!(rebuilt.achievements[0].bit, 3);
    assert!(rebuilt.achievements[0].unlocked);
    assert_eq!(rebuilt.achievements[0].unlock_time, Some(1_700_000_000));
}

#[test]
fn a_reply_that_carries_its_own_schema_is_left_on_the_schema_path() {
    let _guard = guard();
    let app_id = 3_102;
    seed(app_id);
    let carried = with_flag_on(|| reply_for(app_id, &schema()));
    assert_eq!(carried, None, "the schema path is authoritative");
}

#[test]
fn an_app_with_no_known_schema_has_nothing_to_rebuild_from() {
    let _guard = guard();
    let app_id = 3_103;
    remember_unlocks(app_id, vec![unlocked("ACH_WIN")]);
    assert_eq!(definitions(app_id), None);
    assert_eq!(with_flag_on(|| reply_for(app_id, &[])), None);
}

#[test]
fn an_account_switch_drops_the_cached_definitions_and_unlocks() {
    let _guard = guard();
    let app_id = 3_104;
    seed(app_id);
    forget_account_state();
    assert_eq!(definitions(app_id), None);
    assert_eq!(with_flag_on(|| reply_for(app_id, &[])), None);
}

#[test]
fn the_schema_cache_is_bounded_and_refuses_new_apps_when_full() {
    let _guard = guard();
    forget_account_state();
    let base = 3_200;
    for app_id in base..(base + MAX_CACHED_SCHEMAS as u32 + 4) {
        remember_schema(app_id, &schema());
    }
    // An app already cached is refreshed even at the cap; a new one is refused.
    let cached = base;
    remember_schema(cached, &schema());
    assert!(definitions(cached).is_some());
    let refused = base + MAX_CACHED_SCHEMAS as u32 + 3;
    assert_eq!(definitions(refused), None);
    forget_account_state();
}
