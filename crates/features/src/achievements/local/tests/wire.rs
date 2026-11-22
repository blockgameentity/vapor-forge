//! The wire shapes: stable bytes, and the two response forms describing one state.

use prost::Message;

use super::super::*;
use super::fixtures::*;

#[test]
fn rebuilding_the_same_input_twice_produces_identical_bytes() {
    let schema = achievements_schema(&[(10, &["ACH_FIRST_STEPS", "ACH_SHARPSHOOTER"])]);
    let first = rebuild_local_achievements(APP_ID, &local_set(), &unlocks(), Some(&schema));
    let second = rebuild_local_achievements(APP_ID, &local_set(), &unlocks(), Some(&schema));

    assert_eq!(first, second);
    assert_eq!(
        first.service_response(None).encode_to_vec(),
        second.service_response(None).encode_to_vec()
    );
    assert_eq!(
        first
            .legacy_response(u64::from(APP_ID), None)
            .encode_to_vec(),
        second
            .legacy_response(u64::from(APP_ID), None)
            .encode_to_vec()
    );
    assert_eq!(
        first.service_response(None).encode_to_vec(),
        first.service_response(None).encode_to_vec(),
        "encoding the same state twice is stable"
    );
}

#[test]
fn both_wire_shapes_report_the_same_unlocks() {
    let rebuilt = rebuild_local_achievements(APP_ID, &local_set(), &unlocks(), None);
    let service = rebuilt.service_response(None);
    let legacy = rebuilt.legacy_response(u64::from(APP_ID), None);

    assert_eq!(service.stats.len(), 2, "one entry per achievements stat");
    assert_eq!(service.stats[0].stat_id, Some(10));
    assert_eq!(
        service.stats[0].stat_value,
        Some(0b0001),
        "bit 0 set, bit 3 clear"
    );
    assert_eq!(service.stats[0].unlock_times.len(), 1);
    assert_eq!(service.stats[0].unlock_times[0].achievement_bit, Some(0));
    assert_eq!(
        service.stats[0].unlock_times[0].unlock_time,
        Some(1_700_000_000)
    );
    assert_eq!(service.stats[1].stat_id, Some(11));
    assert_eq!(service.stats[1].stat_value, Some(0b0010));

    assert_eq!(legacy.game_id, Some(u64::from(APP_ID)));
    assert_eq!(legacy.achievement_blocks.len(), 2);
    assert_eq!(legacy.achievement_blocks[0].achievement_id, Some(10));
    assert_eq!(
        legacy.achievement_blocks[0].unlock_time,
        vec![1_700_000_000, 0, 0, 0],
        "indexed by bit, zero for a locked achievement"
    );
    assert_eq!(legacy.achievement_blocks[1].achievement_id, Some(11));
    assert_eq!(
        legacy.achievement_blocks[1].unlock_time,
        vec![
            0,
            vapor_forge_steam_protocol::ACHIEVEMENT_UNLOCK_TIME_UNKNOWN
        ]
    );
    assert!(
        legacy.stats.is_empty(),
        "the local set carries no plain stats"
    );
}
