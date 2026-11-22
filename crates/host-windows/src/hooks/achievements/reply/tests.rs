//! The pure half of the reply rewrite, on Linux: shape detection, the app id, the
//! unlock derivation and the replacement, all over literal protobuf bytes.

use super::*;
use vapor_forge_features::achievements::rebuild_local_achievements;
use vapor_forge_steam_protocol::{
    AchievementBlock, LegacyStatsEntry, PlayerAchievementUnlockTime, PlayerStatsEntry,
};

/// A schema the parser accepts: one achievements stat (11) with bits 0 and 3, and
/// a second (12) with bit 1.
fn schema() -> Vec<u8> {
    fn object(out: &mut Vec<u8>, key: &str, body: impl FnOnce(&mut Vec<u8>)) {
        out.push(0);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        body(out);
        out.push(8);
    }
    fn int(out: &mut Vec<u8>, key: &str, value: i32) {
        out.push(2);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(&value.to_le_bytes());
    }
    fn string(out: &mut Vec<u8>, key: &str, value: &str) {
        out.push(1);
        out.extend_from_slice(key.as_bytes());
        out.push(0);
        out.extend_from_slice(value.as_bytes());
        out.push(0);
    }
    let mut out = Vec::new();
    object(&mut out, "620", |root| {
        object(root, "stats", |stats| {
            object(stats, "11", |stat| {
                int(stat, "type", 4);
                object(stat, "bits", |bits| {
                    object(bits, "0", |achievement| string(achievement, "name", "ACH_FIRST"));
                    object(bits, "3", |achievement| string(achievement, "name", "ACH_THIRD"));
                });
            });
            object(stats, "12", |stat| {
                int(stat, "type", 4);
                object(stat, "bits", |bits| {
                    object(bits, "1", |achievement| string(achievement, "name", "ACH_OTHER"));
                });
            });
        });
    });
    out
}

/// CGameID-shaped: app 480 with id type 1.
fn game_id(app: u32) -> u64 {
    u64::from(app) | (1 << 24)
}

fn legacy_body(schema: Option<Vec<u8>>, blocks: Vec<AchievementBlock>) -> Vec<u8> {
    ClientGetUserStatsResponse {
        game_id: Some(game_id(480)),
        eresult: Some(1),
        crc_stats: Some(0xdead_beef),
        schema,
        stats: vec![LegacyStatsEntry {
            stat_id: Some(11),
            stat_value: Some(0b1001),
        }],
        achievement_blocks: blocks,
    }
    .encode_to_vec()
}

fn service_body(schema: Option<Vec<u8>>, stats: Vec<PlayerStatsEntry>) -> Vec<u8> {
    PlayerGetUserStatsResponse {
        sha_schema: Some(vec![0xaa]),
        crc_stats: Some(7),
        schema,
        stats,
        crc_schema: None,
    }
    .encode_to_vec()
}

#[test]
fn both_reply_shapes_round_trip_through_prost_and_are_told_apart() {
    let legacy = legacy_body(Some(schema()), Vec::new());
    let service = service_body(Some(schema()), Vec::new());

    assert!(matches!(classify(&legacy), Ok(Shape::Legacy(_))));
    assert!(matches!(classify(&service), Ok(Shape::Service(_))));
    // Round-trip in the other direction: the classified bytes re-encode to exactly
    // what came in, which is what makes the swap byte-for-byte honest.
    if let Ok(Shape::Legacy(decoded)) = classify(&legacy) {
        assert_eq!(decoded.encode_to_vec(), legacy);
    }
    if let Ok(Shape::Service(decoded)) = classify(&service) {
        assert_eq!(decoded.encode_to_vec(), service);
    }
}

#[test]
fn an_empty_or_malformed_body_is_a_named_blocker() {
    assert!(matches!(classify(&[]), Err(RewriteBlocker::Empty)));
    let blocker = classify(&[0xff, 0xff, 0xff]).unwrap_err();
    assert!(matches!(blocker, RewriteBlocker::Malformed(_)));
    assert!(blocker.reason().contains("not a decode-or-encode-clean stats response"));
}

#[test]
fn only_the_legacy_shape_names_an_app() {
    let legacy = legacy_body(None, Vec::new());
    assert_eq!(app_id(&classify(&legacy).unwrap()), Ok(480));

    let service = service_body(None, Vec::new());
    let blocker = app_id(&classify(&service).unwrap()).unwrap_err();
    assert!(matches!(blocker, RewriteBlocker::NoApp(_)));
    assert!(blocker.reason().contains("carries no app id"));

    // A game id whose app bits are zero names no app either.
    let anonymous = ClientGetUserStatsResponse {
        game_id: Some(1 << 24),
        ..Default::default()
    }
    .encode_to_vec();
    assert!(matches!(
        app_id(&classify(&anonymous).unwrap()),
        Err(RewriteBlocker::NoApp(_))
    ));
}

#[test]
fn the_schema_a_reply_carries_becomes_definitions_in_wire_id_order() {
    let parsed = definitions(&schema()).expect("the fixture schema parses");
    let keys: Vec<&str> = parsed
        .achievements
        .iter()
        .map(|achievement| achievement.key.as_str())
        .collect();
    assert_eq!(keys, ["ACH_FIRST", "ACH_THIRD", "ACH_OTHER"]);
    assert_eq!(parsed.achievement("ACH_THIRD").map(|a| a.bit), Some(3));
    assert!(definitions(&[0xff, 0xff]).is_none(), "a schema that does not parse is absent");
}

#[test]
fn a_legacy_reply_reports_the_unlocks_its_blocks_carry() {
    let mappings = parse_achievement_bit_mappings(&schema()).unwrap();
    let body = legacy_body(
        None,
        vec![AchievementBlock {
            achievement_id: Some(11),
            // Bit 0 unlocked at 1_600_000_000; bit 3 still zero, i.e. locked.
            unlock_time: vec![1_600_000_000, 0, 0, 0],
        }],
    );
    let shape = classify(&body).unwrap();
    let unlocks = unlocks(&shape, &mappings);
    assert_eq!(unlocks.len(), 3);
    assert_eq!(unlocks[0].key, "ACH_FIRST");
    assert!(unlocks[0].unlocked);
    assert_eq!(unlocks[0].unlock_time, Some(1_600_000_000));
    assert_eq!(unlocks[1].key, "ACH_THIRD");
    assert!(!unlocks[1].unlocked, "a zero time is 'not unlocked' in this shape");
    assert_eq!(unlocks[2].key, "ACH_OTHER");
    assert!(!unlocks[2].unlocked, "an absent block is not unlocked");
}

#[test]
fn a_service_reply_reports_the_unlocks_its_stat_bits_carry() {
    let mappings = parse_achievement_bit_mappings(&schema()).unwrap();
    let body = service_body(
        None,
        vec![PlayerStatsEntry {
            stat_id: Some(11),
            // Bits 0 and 3 set, i.e. both of that stat's achievements.
            stat_value: Some(0b1001),
            unlock_times: vec![PlayerAchievementUnlockTime {
                achievement_bit: Some(3),
                unlock_time: Some(1_700_000_000),
            }],
        }],
    );
    let shape = classify(&body).unwrap();
    let unlocks = unlocks(&shape, &mappings);
    assert!(unlocks[0].unlocked, "bit 0 is set");
    assert_eq!(unlocks[0].unlock_time, None, "no time was published for bit 0");
    assert!(unlocks[1].unlocked, "bit 3 is set");
    assert_eq!(unlocks[1].unlock_time, Some(1_700_000_000));
    assert!(!unlocks[2].unlocked);
}

#[test]
fn the_replacement_keeps_every_field_the_rebuild_does_not_speak_for() {
    let definitions = definitions(&schema()).unwrap();
    let mappings = parse_achievement_bit_mappings(&schema()).unwrap();
    // Steam sent one unlocked achievement; the local definitions know all three.
    let body = legacy_body(
        None,
        vec![AchievementBlock {
            achievement_id: Some(11),
            unlock_time: vec![1_600_000_000],
        }],
    );
    let shape = classify(&body).unwrap();
    let unlocks = unlocks(&shape, &mappings);
    let rebuilt = rebuild_local_achievements(480, &definitions, &unlocks, None);
    let rewritten =
        apply(&shape, &rebuilt, &body).expect("two of three bits differ from Steam's reply");

    let decoded = ClientGetUserStatsResponse::decode(rewritten.as_slice()).unwrap();
    assert_eq!(decoded.game_id, Some(game_id(480)), "the game id stays Steam's");
    assert_eq!(decoded.eresult, Some(1), "the EResult stays Steam's");
    assert_eq!(decoded.crc_stats, Some(0xdead_beef), "the token stays Steam's");
    assert_eq!(decoded.schema, None, "the schema stays as the reply carried it");
    // The rebuilt blocks carry every achievement the definitions name, indexed by
    // bit, which is the whole point of the rebuild.
    assert_eq!(decoded.achievement_blocks.len(), 2);
    assert_eq!(decoded.achievement_blocks[0].achievement_id, Some(11));
    assert_eq!(decoded.achievement_blocks[0].unlock_time[0], 1_600_000_000);
    assert_eq!(decoded.achievement_blocks[1].achievement_id, Some(12));
    assert_eq!(decoded.achievement_blocks[1].unlock_time, vec![0, 0]);
}

#[test]
fn a_rebuild_that_says_what_steam_said_is_not_a_rewrite() {
    let definitions = definitions(&schema()).unwrap();
    // Every achievement locked, and a reply that already says exactly that.
    let rebuilt = rebuild_local_achievements(480, &definitions, &[], None);
    let stats = rebuilt.service_response(None).stats;
    let body = service_body(None, stats);
    let shape = classify(&body).unwrap();
    assert!(
        apply(&shape, &rebuilt, &body).is_none(),
        "a replacement that is byte-identical must not be swapped in"
    );
}
