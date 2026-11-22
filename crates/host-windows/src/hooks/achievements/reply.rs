//! The stats reply body: which shape it is, and how a rebuilt payload replaces it.
//!
//! Two wire shapes carry the same information, and the host must handle both
//! because it cannot read the `EMsg` that would tell them apart:
//!
//! | shape | type | unlocks in |
//! | --- | --- | --- |
//! | service (`Player.GetUserStats#1`, EMsg 147) | `PlayerGetUserStatsResponse` | `stats[].stat_value` bits |
//! | legacy (EMsg 819) | `ClientGetUserStatsResponse` | `achievement_blocks[].unlock_time` |
//!
//! [`classify`] decides by decoding and re-encoding: prost 0.14 drops unknown
//! fields, so only the shape the bytes were written as survives a byte-exact round
//! trip. A body that round-trips as neither is [`RewriteBlocker::Malformed`] and is
//! left exactly as Steam sent it.
//!
//! [`apply`] replaces **only** what the rebuild speaks for — `stats` for the
//! service shape, `achievement_blocks` for the legacy one — so the schema, the
//! `EResult`, the `crc_stats` token and the game id stay Steam's. Everything here is
//! pure: bytes in, bytes out, no memory and no state.

use prost::Message;
use vapor_forge_features::achievements::{
    AchievementUnlockState, LocalAchievement, LocalAchievementSet, RebuiltAchievements,
};
use vapor_forge_steam_protocol::{
    parse_achievement_bit_mappings, AchievementBitMapping, ClientGetUserStatsResponse,
    PlayerGetUserStatsResponse,
};

/// A sanity bound on the reply body a job object may point at.
///
/// A bound, not a layout fact: a stats reply is kilobytes, so anything past this is
/// not one, and reading it would be reading an arbitrary allocation.
pub const MAX_REPLY_BODY: usize = 4 * 1024 * 1024;

/// A decoded stats reply.
#[derive(Clone, Debug)]
pub enum Shape {
    Service(PlayerGetUserStatsResponse),
    Legacy(ClientGetUserStatsResponse),
}

/// Why a reply cannot be rewritten.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RewriteBlocker {
    /// Empty body: nothing to decode.
    Empty,
    /// The bytes are not one of the two reply shapes.
    Malformed(String),
    /// The shape names no app, so there is nothing to look definitions up for.
    NoApp(&'static str),
}

impl RewriteBlocker {
    /// One line for the log, naming the cause rather than a code.
    pub fn reason(&self) -> String {
        match self {
            Self::Empty => "the stats reply body is empty".to_owned(),
            Self::Malformed(error) => {
                format!("the stats reply body is not a decode-or-encode-clean stats response: {error}")
            }
            Self::NoApp(shape) => format!(
                "{shape} carries no app id, and this host reads no per-build job offset to find one"
            ),
        }
    }
}

/// Decode one reply body, and say which shape it is.
pub fn classify(body: &[u8]) -> Result<Shape, RewriteBlocker> {
    if body.is_empty() {
        return Err(RewriteBlocker::Empty);
    }
    if let Ok(service) = PlayerGetUserStatsResponse::decode(body) {
        if service.encode_to_vec() == body {
            return Ok(Shape::Service(service));
        }
    }
    match ClientGetUserStatsResponse::decode(body) {
        Ok(legacy) if legacy.encode_to_vec() == body => Ok(Shape::Legacy(legacy)),
        Ok(_) => Err(RewriteBlocker::Malformed(
            "the body decodes but does not re-encode to the same bytes".to_owned(),
        )),
        Err(error) => Err(RewriteBlocker::Malformed(error.to_string())),
    }
}

/// The app a reply is about.
///
/// The legacy shape carries a `CGameID`-shaped `game_id`, whose low 24 bits are
/// the app id. The service shape carries no app at all; the job object's `game_id`
/// field would have it, and that is a per-build offset this crate no longer keeps.
pub fn app_id(shape: &Shape) -> Result<u32, RewriteBlocker> {
    match shape {
        Shape::Legacy(legacy) => match legacy.game_id {
            Some(game_id) if (game_id as u32 & 0x00ff_ffff) != 0 => {
                Ok(game_id as u32 & 0x00ff_ffff)
            }
            _ => Err(RewriteBlocker::NoApp("the legacy stats reply")),
        },
        Shape::Service(_) => Err(RewriteBlocker::NoApp("the service stats reply")),
    }
}

/// The schema a reply carries, when it carries a non-empty one.
pub fn schema(shape: &Shape) -> Option<&[u8]> {
    let schema = match shape {
        Shape::Service(service) => service.schema.as_deref(),
        Shape::Legacy(legacy) => legacy.schema.as_deref(),
    };
    schema.filter(|schema| !schema.is_empty())
}

/// The achievement bit mappings a schema declares.
///
/// Sorted by `(stat_id, bit)` by the protocol parser, so the order is stable and a
/// reply always rebuilds to the same bytes.
pub fn mappings(schema: &[u8]) -> Option<Vec<AchievementBitMapping>> {
    parse_achievement_bit_mappings(schema)
        .ok()
        .filter(|mappings| !mappings.is_empty())
}

/// The definitions a schema declares, in the rebuild's own shape.
///
/// Duplicate keys keep their first mapping, which is the order the wire ids were
/// declared in — the same rule the Linux host applies to its schema cache.
pub fn definitions(schema: &[u8]) -> Option<LocalAchievementSet> {
    definitions_from(&mappings(schema)?)
}

/// [`definitions`] from mappings that were already parsed, so one schema is parsed
/// once per reply.
pub fn definitions_from(mappings: &[AchievementBitMapping]) -> Option<LocalAchievementSet> {
    let mut seen = std::collections::HashSet::new();
    let achievements: Vec<LocalAchievement> = mappings
        .iter()
        .filter(|mapping| seen.insert(mapping.key.clone()))
        .map(|mapping| LocalAchievement::new(&mapping.key, mapping.stat_id, mapping.achievement_bit))
        .collect();
    (!achievements.is_empty()).then_some(LocalAchievementSet {
        groups: Vec::new(),
        achievements,
    })
}

/// The unlock state a reply reports, keyed by achievement key.
///
/// `mappings` is what the app's schema declared, so the wire ids a reply carries
/// become the keys the rebuild reads. An id no mapping names is ignored: it belongs
/// to an ordinary stat, not to an achievement.
pub fn unlocks(shape: &Shape, mappings: &[AchievementBitMapping]) -> Vec<AchievementUnlockState> {
    mappings
        .iter()
        .map(|mapping| unlock_state(shape, mapping))
        .collect()
}

fn unlock_state(shape: &Shape, mapping: &AchievementBitMapping) -> AchievementUnlockState {
    let (bit_set, unlock_time) = match shape {
        Shape::Service(service) => service
            .stats
            .iter()
            .find(|entry| entry.stat_id == Some(mapping.stat_id))
            .map(|entry| {
                let set = entry
                    .stat_value
                    .is_some_and(|value| value & (1_u32 << mapping.achievement_bit.min(31)) != 0);
                let time = entry
                    .unlock_times
                    .iter()
                    .find(|time| time.achievement_bit == Some(mapping.achievement_bit))
                    .and_then(|time| time.unlock_time);
                (set, time.filter(|time| *time != 0))
            })
            .unwrap_or((false, None)),
        Shape::Legacy(legacy) => legacy
            .achievement_blocks
            .iter()
            .find(|block| block.achievement_id == Some(mapping.stat_id))
            .map(|block| {
                let time = usize::try_from(mapping.achievement_bit)
                    .ok()
                    .and_then(|bit| block.unlock_time.get(bit).copied())
                    .filter(|time| *time != 0);
                // The legacy shape reports "not unlocked" as a zero time.
                (time.is_some(), time)
            })
            .unwrap_or((false, None)),
    };
    AchievementUnlockState {
        key: mapping.key.clone(),
        unlocked: bit_set,
        unlock_time,
    }
}

/// Replace what `rebuilt` speaks for in `shape`, and re-encode.
///
/// `original` is the body the shape was decoded from: the replacement is only
/// returned when it differs from those bytes, because swapping in a body that says
/// what Steam already said is a write with no effect. `None` therefore means "the
/// reply stays Steam's".
pub fn apply(shape: &Shape, rebuilt: &RebuiltAchievements, original: &[u8]) -> Option<Vec<u8>> {
    let rewritten = match shape {
        Shape::Service(service) => {
            let mut rewritten = service.clone();
            rewritten.stats = rebuilt.service_response(service.schema.clone()).stats;
            rewritten.encode_to_vec()
        }
        Shape::Legacy(legacy) => {
            let mut rewritten = legacy.clone();
            let game_id = legacy.game_id.unwrap_or_default();
            rewritten.achievement_blocks = rebuilt
                .legacy_response(game_id, legacy.schema.clone())
                .achievement_blocks;
            rewritten.encode_to_vec()
        }
    };
    if rewritten == original {
        return None;
    }
    // A replacement that does not decode back to the same shape would be worse than
    // Steam's own bytes: refuse rather than swap in something unparseable.
    if classify(&rewritten).is_err() {
        return None;
    }
    Some(rewritten)
}

#[cfg(test)]
mod tests;
