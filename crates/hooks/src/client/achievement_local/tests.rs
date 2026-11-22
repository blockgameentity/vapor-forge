//! Both directions of the local achievement rebuild wiring.
//!
//! `features::achievements::local` tests the rebuild itself (schema wins, local
//! set fills a gap, unknown unlocks are ignored, the output is deterministic), and
//! `features::achievements::overrides` tests the composition. These tests prove
//! that the Linux reply path reaches it, that a Lua override is what makes the
//! rebuilt payload differ, and that with no override nothing is reached at all.
use std::collections::BTreeMap;

use super::*;
use vapor_forge_core::AchievementIntent;

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

/// The intent a script would have declared for `key`.
fn intent(key: &str, intent: AchievementIntent) -> BTreeMap<String, AchievementIntent> {
    [(key.to_owned(), intent)].into_iter().collect()
}

/// Serialises every test in this module: the rebuild switch and both caches are
/// process-wide, so one test at a time keeps them honest.
///
/// Two locks, because two other modules touch this state. `test_lock` is shared
/// with the tests that call `config_apply::apply`, which republishes the
/// achievement flag; `ACCOUNT_TEST_LOCK` is shared with the account-switch test,
/// which calls `reset_account_state` and therefore clears the schema and unlock
/// caches this module asserts on. Without the second lock the cache-bound test
/// fails whenever the account test runs alongside it.
struct Guard {
    _feature: std::sync::MutexGuard<'static, ()>,
    _account: std::sync::MutexGuard<'static, ()>,
}

fn guard() -> Guard {
    let feature = crate::test_lock::lock();
    let account = crate::client::ACCOUNT_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    Guard {
        _feature: feature,
        _account: account,
    }
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
    assert_eq!(
        rebuild_for_reply(app_id, &intent("ACH_OTHER", AchievementIntent::unlocked())),
        None,
        "the flag is off"
    );
    assert_eq!(
        reply_for(
            app_id,
            &[],
            &intent("ACH_OTHER", AchievementIntent::unlocked())
        ),
        None,
        "a reply with no schema gets no rebuilt state while the flag is off"
    );
    // The definitions are still there: they are what a flag-on host would use,
    // and remembering them is not a behaviour change.
    assert!(definitions(app_id).is_some());
}

#[test]
fn a_reply_with_no_override_is_left_byte_identical() {
    let _guard = guard();
    let app_id = 3_110;
    seed(app_id);
    // The flag is on and the definitions are known: the only thing missing is a
    // reason to rewrite. That is what makes `local_rebuild` safe to leave on.
    let rebuilt = with_flag_on(|| reply_for(app_id, &[], &BTreeMap::new()));
    assert_eq!(
        rebuilt, None,
        "Steam's own unlock map composed with no intent is Steam's own map"
    );
}

#[test]
fn an_override_for_a_key_the_schema_does_not_define_changes_nothing() {
    let _guard = guard();
    let app_id = 3_111;
    seed(app_id);
    let rebuilt = with_flag_on(|| {
        reply_for(
            app_id,
            &[],
            &intent("ACH_NOT_IN_THE_SCHEMA", AchievementIntent::unlocked()),
        )
    });
    assert_eq!(rebuilt, None, "an undefined key is ignored, never invented");
}

#[test]
fn flag_on_uses_the_shared_rebuild_and_its_definitions() {
    let _guard = guard();
    let app_id = 3_101;
    seed(app_id);
    let overrides = intent("ACH_WIN", AchievementIntent::unlocked_at(1_800_000_000));
    let rebuilt = with_flag_on(|| reply_for(app_id, &[], &overrides))
        .expect("the flag is on, the definitions are known and an override changes them");
    assert_eq!(rebuilt.app_id, app_id);
    assert_eq!(rebuilt.counts.total, 1);
    assert_eq!(rebuilt.counts.unlocked, 1);
    assert_eq!(rebuilt.achievements[0].key, "ACH_WIN");
    assert_eq!(rebuilt.achievements[0].stat_id, 12);
    assert_eq!(rebuilt.achievements[0].bit, 3);
    assert!(rebuilt.achievements[0].unlocked);
    assert_eq!(
        rebuilt.achievements[0].unlock_time,
        Some(1_800_000_000),
        "the override's time, not the snapshot's"
    );
}

#[test]
fn an_override_reaches_the_rebuilt_payload() {
    let _guard = guard();
    let app_id = 3_120;
    seed(app_id);
    assert_eq!(cached_unlocks(app_id), vec![unlocked("ACH_WIN")]);

    // "ACH_WIN" is the schema's one achievement: a `false` override must clear the
    // unlock Steam reported, and that is the only change.
    let rebuilt =
        with_flag_on(|| reply_for(app_id, &[], &intent("ACH_WIN", AchievementIntent::locked())))
            .expect("clearing a defined achievement's unlock is a change");
    assert_eq!(rebuilt.counts.total, 1);
    assert_eq!(rebuilt.counts.unlocked, 0);
    assert!(!rebuilt.achievements[0].unlocked);
    assert_eq!(rebuilt.achievements[0].unlock_time, None);
}

#[test]
fn an_override_that_agrees_with_steam_changes_nothing() {
    let _guard = guard();
    let app_id = 3_121;
    seed(app_id);
    let rebuilt = with_flag_on(|| {
        reply_for(
            app_id,
            &[],
            &intent("ACH_WIN", AchievementIntent::unlocked_at(1_700_000_000)),
        )
    });
    assert_eq!(
        rebuilt, None,
        "the composition says what Steam already said"
    );
}

#[test]
fn a_reply_that_carries_its_own_schema_is_left_on_the_schema_path() {
    let _guard = guard();
    let app_id = 3_102;
    seed(app_id);
    let carried = with_flag_on(|| {
        reply_for(
            app_id,
            &schema(),
            &intent("ACH_WIN", AchievementIntent::locked()),
        )
    });
    assert_eq!(carried, None, "the schema path is authoritative");
}

#[test]
fn an_app_with_no_known_schema_has_nothing_to_rebuild_from() {
    let _guard = guard();
    let app_id = 3_103;
    remember_unlocks(app_id, vec![unlocked("ACH_WIN")]);
    assert_eq!(definitions(app_id), None);
    assert_eq!(
        with_flag_on(|| {
            reply_for(app_id, &[], &intent("ACH_WIN", AchievementIntent::locked()))
        }),
        None
    );
}

#[test]
fn an_account_switch_drops_the_cached_definitions_and_unlocks() {
    let _guard = guard();
    let app_id = 3_104;
    seed(app_id);
    forget_account_state();
    assert_eq!(definitions(app_id), None);
    assert_eq!(
        with_flag_on(|| {
            reply_for(app_id, &[], &intent("ACH_WIN", AchievementIntent::locked()))
        }),
        None
    );
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
