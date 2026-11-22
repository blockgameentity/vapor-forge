//! Achievement/stats schema interception.
//!
//! For controlled apps, outgoing stats requests are redirected to a donor
//! account only to fetch Valve's authoritative schema. The hooks crate then
//! drops the donor response, combines that schema with backend-authoritative
//! stats when available, and injects a response for Steam's original job.
//!
//! Schema retrieval supports both ServiceMethod (EMsg 151/147) and legacy
//! EMsg 818/819. Donor requests clear cached schema/stats tokens so Valve sends
//! full schema data; donor stat values must never be passed through as the
//! controlled account's state.
//!
//! Two things sit on top of that path:
//!
//! * [`tracking`] remembers which redirect is still outstanding (donor requests
//!   and their responses are handled on different threads);
//! * [`local`] rebuilds the achievement payload from locally known definitions
//!   when the schema path cannot describe it. It is off unless a host turns it
//!   on, see [`set_local_rebuild`].
//! * [`overrides`] is the one input that does not come from Steam: the user's Lua
//!   intent (`setachievement`), composed on top of the schema's definitions and
//!   Steam's own unlock map. Nothing is rewritten until an override actually
//!   changes what Steam reported, which is why the rebuild can be on by default.

use prost::Message;
use tracing::{debug, info};
use vapor_forge_config::{AppId, RuntimeConfig};
use vapor_forge_steam_protocol::*;

pub use local::{
    local_rebuild_enabled, local_rebuild_settings, rebuild_local_achievements,
    rebuild_local_achievements_if_enabled, set_local_rebuild, AchievementCounts,
    AchievementDisplay, AchievementUnlockState, LocalAchievement, LocalAchievementGroup,
    LocalAchievementSet, LocalDefinitionError, LocalRebuildSettings, RebuildSource,
    RebuiltAchievement, RebuiltAchievementGroup, RebuiltAchievements, MAX_LOCAL_ACHIEVEMENT_BIT,
};
pub use overrides::{compose_unlocks, ComposedUnlocks};
pub use send::{
    on_send_legacy_stats, on_send_service_stats, plan_send_legacy_stats, plan_send_service_stats,
    StatsSendPlan,
};
pub use tracking::{drain_offline_responses, has_legacy_pending, has_pending, OfflineResponse};

mod local;
mod overrides;
mod send;
mod tracking;

#[cfg(test)]
mod tests;

use tracking::{take_legacy_pending, take_pending};

pub const STATS_JOB_NAME: &str = vapor_forge_steam_protocol::PLAYER_GET_USER_STATS_JOB_NAME;
const DEFAULT_REF_STEAMID: u64 = 76561198028121353;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedAchievementSchema {
    pub app_id: u32,
    pub schema_version: Option<String>,
    pub content: Vec<u8>,
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(DIGITS[(byte >> 4) as usize] as char);
        value.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    value
}

// ---------------------------------------------------------------------------
// Ownership gate shared by the outbound and inbound paths
// ---------------------------------------------------------------------------

fn should_redirect_with_ownership(
    app_id: AppId,
    config: &RuntimeConfig,
    ownership: impl FnOnce(AppId) -> crate::apps::OwnershipState,
) -> bool {
    crate::apps::classify_app_with_ownership(config, app_id, ownership)
        .requires_injected_ownership()
}

// ---------------------------------------------------------------------------
// Incoming response processing (called from recv hook)
// ---------------------------------------------------------------------------

/// Process an incoming ServiceMethod response (EMsg 147) for stats.
/// Returns `Some(new_body_bytes)` if response was stripped, `None` to pass through.
pub fn on_recv_service_stats(
    hdr: &CMsgProtoBufHeader,
    body_bytes: &[u8],
) -> Option<(Vec<u8>, Vec<u8>, Option<CapturedAchievementSchema>)> {
    let job_id = hdr.jobid_target?;
    let app_id = take_pending(job_id)?;

    let mut resp = PlayerGetUserStatsResponse::decode(body_bytes).ok()?;
    let captured = resp
        .schema
        .as_ref()
        .filter(|schema| !schema.is_empty())
        .map(|schema| CapturedAchievementSchema {
            app_id: app_id.0,
            schema_version: resp.sha_schema.as_deref().map(hex),
            content: schema.clone(),
        });
    resp.stats.clear();

    let mut new_hdr = hdr.clone();
    new_hdr.eresult = Some(ERESULT_OK);

    info!(
        app_id = app_id.0,
        "achievements: cleared stats from response"
    );
    Some((new_hdr.encode_to_vec(), resp.encode_to_vec(), captured))
}

/// Remove reference-account values from a controlled App's EMsg 819 response.
pub fn on_recv_legacy_stats(
    body_bytes: &[u8],
    config: &RuntimeConfig,
) -> Option<(Vec<u8>, Option<CapturedAchievementSchema>)> {
    let mut response = ClientGetUserStatsResponse::decode(body_bytes).ok()?;
    let app_id = AppId(app_id_from_game_id(response.game_id?)?);
    if !should_redirect_with_ownership(app_id, config, crate::apps::actual_ownership) {
        return None;
    }

    let matched = has_legacy_pending() && take_legacy_pending(app_id);
    let captured = matched
        .then_some(response.schema.as_ref())
        .flatten()
        .filter(|schema| !schema.is_empty())
        .map(|schema| CapturedAchievementSchema {
            app_id: app_id.0,
            schema_version: None,
            content: schema.clone(),
        });
    response.stats.clear();
    response.achievement_blocks.clear();
    response.eresult = Some(ERESULT_OK);
    debug!(
        app_id = app_id.0,
        matched_pending = matched,
        "achievements: cleared legacy stats response"
    );
    Some((response.encode_to_vec(), captured))
}
