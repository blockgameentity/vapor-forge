//! The Lua override, end to end through the reply rewrite.
//!
//! Split out of [`super`] because it is the half of the body that does not come
//! from Steam: the same fake job object and the same decoded geometry, but the
//! answer depends on what a script declared with `setachievement`.

use super::*;
use vapor_forge_core::AchievementIntent;

/// [`rewriting_state`] plus the Lua intent a script would have declared.
fn overridden_state(intents: &[(&str, AchievementIntent)]) -> HostState {
    let overrides = intents
        .iter()
        .map(|(key, intent)| ((*key).to_owned(), *intent))
        .collect();
    rewriting_state().with_achievement_overrides(
        [(vapor_forge_config::AppId(480), overrides)]
            .into_iter()
            .collect(),
    )
}

#[test]
fn no_override_leaves_the_reply_byte_identical() {
    let _guard = serial();
    // The flag is on, the geometry is decoded and the definitions are known: the
    // only thing missing is a reason to rewrite, which is exactly the point of the
    // default. `local_rebuild = true` with no script intent must be inert.
    let state = rewriting_state();
    let mut fake = FakeJob::new(legacy_body());
    let original = fake.slots();
    let steam_body = fake.served();

    // SAFETY: the fake job outlives the call, and nothing is written on this path.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    assert!(
        prepared.swap.is_none(),
        "no reason to rewrite means no swap"
    );
    assert_eq!(
        prepared.observation.plan,
        crate::features::achievements::RebuildPlan::OverridesInert
    );
    assert_eq!(prepared.observation.reply, ReplyOutcome::NothingToRebuild);
    assert!(!prepared.observation.rebuilds());
    assert_eq!(fake.slots(), original);
    assert_eq!(fake.served(), steam_body, "Steam's own bytes, untouched");
    set_local_rebuild(false);
}

#[test]
fn an_override_for_an_unknown_key_changes_nothing() {
    let _guard = serial();
    // A key the schema does not name is ignored, never invented: the reply keeps
    // Steam's bytes and no swap is installed.
    let state = overridden_state(&[("ACH_INVENTED", AchievementIntent::unlocked())]);
    let mut fake = FakeJob::new(legacy_body());
    let steam_body = fake.served();

    // SAFETY: nothing is written on this path.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    assert!(prepared.swap.is_none());
    assert_eq!(
        prepared.observation.plan,
        crate::features::achievements::RebuildPlan::OverridesInert
    );
    assert_eq!(fake.served(), steam_body);
    set_local_rebuild(false);
}

#[test]
fn a_rebuilt_reply_is_served_for_the_call_and_steams_bytes_come_back() {
    let _guard = serial();
    // One override, one achievement: `ACH_THIRD` becomes unlocked and nothing else
    // may move.
    let state = overridden_state(&[("ACH_THIRD", AchievementIntent::unlocked_at(1_700_000_000))]);
    let mut fake = FakeJob::new(legacy_body());
    let original = fake.slots();
    let steam_body = fake.served();

    // SAFETY: the fake job and reply object outlive the swap, which is dropped here.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    let swap = prepared
        .swap
        .expect("an override that changes a defined achievement is served");
    let served = fake.served();
    assert_eq!(
        prepared.observation.reply,
        ReplyOutcome::Served {
            bytes: served.len()
        }
    );
    assert!(prepared.observation.rebuilds());
    drop(swap);

    let decoded = ClientGetUserStatsResponse::decode(served.as_slice()).expect("a legacy reply");
    assert_eq!(
        decoded.game_id,
        Some(game_id(480)),
        "the game id stays Steam's"
    );
    assert_eq!(decoded.eresult, Some(1), "the EResult stays Steam's");
    assert_eq!(
        decoded.achievement_blocks.len(),
        2,
        "the rebuilt payload speaks for both achievements stats, not just Steam's one"
    );
    // Stat 11: bit 0 was Steam's, bit 3 is the override.
    assert_eq!(decoded.achievement_blocks[0].achievement_id, Some(11));
    assert_eq!(decoded.achievement_blocks[0].unlock_time[0], 1_600_000_000);
    assert_eq!(decoded.achievement_blocks[0].unlock_time[3], 1_700_000_000);
    // Stat 12: `ACH_OTHER` was never unlocked and the override did not touch it.
    assert_eq!(decoded.achievement_blocks[1].achievement_id, Some(12));
    assert!(
        decoded.achievement_blocks[1]
            .unlock_time
            .iter()
            .all(|time| *time == 0),
        "an achievement nobody overrode stays locked"
    );

    assert_eq!(
        fake.slots(),
        original,
        "Steam's pointer and length are back"
    );
    assert_eq!(fake.served(), steam_body, "Steam's own bytes are back");
    set_local_rebuild(false);
}

#[test]
fn an_override_that_clears_an_unlock_removes_the_bit() {
    let _guard = serial();
    // `ACH_FIRST` is the one Steam reported unlocked; a `false` override must take
    // that back, which is the only way to render a locked achievement Steam says
    // is unlocked.
    let state = overridden_state(&[("ACH_FIRST", AchievementIntent::locked())]);
    let mut fake = FakeJob::new(legacy_body());

    // SAFETY: the fake job outlives the swap, which is dropped here.
    let prepared = unsafe { prepare(&state, fake.this(), 1) };
    let swap = prepared.swap.expect("clearing an unlock is a change");
    let served = fake.served();
    drop(swap);

    let decoded = ClientGetUserStatsResponse::decode(served.as_slice()).expect("a legacy reply");
    assert_eq!(
        decoded.achievement_blocks[0].unlock_time[0], 0,
        "a locked achievement carries no time on the legacy wire"
    );
    set_local_rebuild(false);
}
