//! The wire form of a rebuilt achievement list.
//!
//! Steam asks for achievement state in one of two shapes and both carry the same
//! information: unlocked bits, plus a time per unlocked bit. `Player.GetUserStats#1`
//! (EMsg 151/147) packs the bits of one achievements stat into that stat's
//! `stat_value` and lists a time per set bit; the legacy EMsg 818/819 answer packs
//! the same bits into an `achievement_blocks` entry per stat, as an array indexed
//! by bit.
//!
//! Both are built from the same sorted list, so one rebuilt state always encodes
//! to the same bytes, and the two shapes always describe the same unlocks.

use tracing::debug;
use vapor_forge_steam_protocol::{
    AchievementBlock, ClientGetUserStatsResponse, PlayerAchievementUnlockTime,
    PlayerGetUserStatsResponse, PlayerStatsEntry, ERESULT_OK,
};

use super::rebuild::RebuiltAchievements;

impl RebuiltAchievements {
    /// The `Player.GetUserStats#1` body describing this state.
    ///
    /// There is one entry per achievements stat in the list, carrying the
    /// unlocked bits in `stat_value` and a time for each set bit, so a client
    /// that asks without a schema still sees the same unlocks it would have got
    /// from the schema path. `schema` is echoed to the client as given: the
    /// rebuild never invents schema bytes.
    pub fn service_response(&self, schema: Option<Vec<u8>>) -> PlayerGetUserStatsResponse {
        let mut stats: Vec<PlayerStatsEntry> = Vec::new();
        for achievement in &self.achievements {
            let Some(bit_mask) = 1_u32.checked_shl(achievement.bit) else {
                debug!(
                    key = achievement.key.as_str(),
                    bit = achievement.bit,
                    "achievements: achievement bit is not addressable, left out of the response"
                );
                continue;
            };
            let index = match stats
                .iter()
                .position(|entry| entry.stat_id == Some(achievement.stat_id))
            {
                Some(index) => index,
                None => {
                    stats.push(PlayerStatsEntry {
                        stat_id: Some(achievement.stat_id),
                        stat_value: Some(0),
                        unlock_times: Vec::new(),
                    });
                    stats.len() - 1
                }
            };
            let entry = &mut stats[index];
            let previous = entry.stat_value.unwrap_or(0);
            entry.stat_value = Some(if achievement.unlocked {
                previous | bit_mask
            } else {
                previous & !bit_mask
            });
            if let Some(unlock_time) = achievement.unlock_time {
                entry.unlock_times.push(PlayerAchievementUnlockTime {
                    achievement_bit: Some(achievement.bit),
                    unlock_time: Some(unlock_time),
                });
            }
        }
        for entry in &mut stats {
            entry
                .unlock_times
                .sort_by_key(|time| time.achievement_bit.unwrap_or(u32::MAX));
        }
        stats.sort_by_key(|entry| entry.stat_id.unwrap_or(u32::MAX));
        PlayerGetUserStatsResponse {
            schema,
            stats,
            ..Default::default()
        }
    }

    /// The legacy EMsg 819 body describing this state.
    ///
    /// `unlock_time` is indexed by bit and zero for a locked achievement, which
    /// is how the legacy path reports "not unlocked". `crc_stats` is left unset:
    /// the token names the app's whole stats state, which a rebuild of the
    /// achievement list alone cannot speak for.
    pub fn legacy_response(
        &self,
        game_id: u64,
        schema: Option<Vec<u8>>,
    ) -> ClientGetUserStatsResponse {
        let mut blocks: Vec<AchievementBlock> = Vec::new();
        for achievement in &self.achievements {
            let Some(bit_index) = usize::try_from(achievement.bit)
                .ok()
                .filter(|bit| *bit < u32::BITS as usize)
            else {
                debug!(
                    key = achievement.key.as_str(),
                    bit = achievement.bit,
                    "achievements: achievement bit is not addressable, left out of the response"
                );
                continue;
            };
            let index = match blocks
                .iter()
                .position(|block| block.achievement_id == Some(achievement.stat_id))
            {
                Some(index) => index,
                None => {
                    blocks.push(AchievementBlock {
                        achievement_id: Some(achievement.stat_id),
                        unlock_time: Vec::new(),
                    });
                    blocks.len() - 1
                }
            };
            let block = &mut blocks[index];
            if block.unlock_time.len() <= bit_index {
                block.unlock_time.resize(bit_index + 1, 0);
            }
            block.unlock_time[bit_index] = achievement.unlock_time.unwrap_or(0);
        }
        blocks.sort_by_key(|block| block.achievement_id.unwrap_or(u32::MAX));
        ClientGetUserStatsResponse {
            game_id: Some(game_id),
            eresult: Some(ERESULT_OK),
            crc_stats: None,
            schema,
            stats: Vec::new(),
            achievement_blocks: blocks,
        }
    }
}
