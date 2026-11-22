//! Outbound stats request planning.
//!
//! A controlled app's stats request is rewritten to the donor account so Valve
//! answers with its authoritative schema. Planning is separate from the hook so
//! the decision is a pure function of the request, the config and the ownership
//! state: [`plan_send_service_stats`] and [`plan_send_legacy_stats`] return what
//! should happen, and the wrappers apply it.

use std::collections::HashMap;

use prost::Message;
use tracing::{debug, info};
use vapor_forge_config::{AppId, RuntimeConfig};
use vapor_forge_steam_protocol::{
    app_id_from_game_id, CMsgProtoBufHeader, ClientGetUserStatsRequest, PlayerGetUserStatsRequest,
};

use super::should_redirect_with_ownership;
use super::tracking::{add_legacy_pending, add_pending, get_ref_steamid, queue_offline_response};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatsSendPlan {
    Pass,
    DropOffline {
        app_id: AppId,
        job_id: Option<u64>,
    },
    Rewrite {
        app_id: AppId,
        body: Vec<u8>,
        donor_steam_id: u64,
        job_id: Option<u64>,
        was_probe: bool,
    },
}

pub fn plan_send_service_stats(
    hdr: &CMsgProtoBufHeader,
    body_bytes: &[u8],
    config: &RuntimeConfig,
    stat_steam_ids: &HashMap<AppId, u64>,
    ownership: impl FnOnce(AppId) -> crate::apps::OwnershipState,
) -> StatsSendPlan {
    let Ok(mut req) = PlayerGetUserStatsRequest::decode(body_bytes) else {
        return StatsSendPlan::Pass;
    };
    let Some(app_id) = req.appid.map(AppId) else {
        return StatsSendPlan::Pass;
    };
    if !should_redirect_with_ownership(app_id, config, ownership) {
        return StatsSendPlan::Pass;
    }

    let Some(job_id) = hdr.jobid_source.filter(|job_id| *job_id != 0) else {
        return StatsSendPlan::Pass;
    };
    if config.achievements.offline_schema {
        return StatsSendPlan::DropOffline {
            app_id,
            job_id: Some(job_id),
        };
    }

    let had_sha_schema = req.sha_schema.take().is_some();
    let had_crc_schema = req.crc_schema.take().is_some();
    let was_probe = had_sha_schema || had_crc_schema;
    let donor_steam_id = get_ref_steamid(stat_steam_ids, app_id);
    req.steamid = Some(donor_steam_id);
    StatsSendPlan::Rewrite {
        app_id,
        body: req.encode_to_vec(),
        donor_steam_id,
        job_id: Some(job_id),
        was_probe,
    }
}

pub fn plan_send_legacy_stats(
    body_bytes: &[u8],
    config: &RuntimeConfig,
    stat_steam_ids: &HashMap<AppId, u64>,
    ownership: impl FnOnce(AppId) -> crate::apps::OwnershipState,
) -> StatsSendPlan {
    let Ok(mut req) = ClientGetUserStatsRequest::decode(body_bytes) else {
        return StatsSendPlan::Pass;
    };
    let Some(app_id) = req.game_id.and_then(app_id_from_game_id).map(AppId) else {
        return StatsSendPlan::Pass;
    };
    if !should_redirect_with_ownership(app_id, config, ownership) {
        return StatsSendPlan::Pass;
    }
    if config.achievements.offline_schema {
        return StatsSendPlan::DropOffline {
            app_id,
            job_id: None,
        };
    }

    let was_probe = req.crc_stats.take().is_some() || req.schema_local_version != Some(-1);
    req.schema_local_version = Some(-1);
    let donor_steam_id = get_ref_steamid(stat_steam_ids, app_id);
    req.steam_id_for_user = Some(donor_steam_id);
    StatsSendPlan::Rewrite {
        app_id,
        body: req.encode_to_vec(),
        donor_steam_id,
        job_id: None,
        was_probe,
    }
}

/// Process an outgoing ServiceMethod (EMsg 151) Player.GetUserStats#1 request.
/// Returns `Some(modified_body_bytes)` if the request was rewritten, `None` to pass through.
pub fn on_send_service_stats(
    hdr: &CMsgProtoBufHeader,
    body_bytes: &[u8],
    config: &RuntimeConfig,
    stat_steam_ids: &HashMap<AppId, u64>,
) -> Option<Vec<u8>> {
    match plan_send_service_stats(
        hdr,
        body_bytes,
        config,
        stat_steam_ids,
        crate::apps::actual_ownership,
    ) {
        StatsSendPlan::Pass => None,
        StatsSendPlan::DropOffline {
            app_id,
            job_id: Some(job_id),
        } => {
            queue_offline_response(app_id, job_id, hdr);
            Some(Vec::new())
        }
        StatsSendPlan::Rewrite {
            app_id,
            body,
            donor_steam_id,
            job_id: Some(job_id),
            was_probe,
        } => {
            add_pending(job_id, app_id);
            info!(
                app_id = app_id.0,
                ref_id = donor_steam_id,
                probe = was_probe,
                "achievements: redirected stats request"
            );
            Some(body)
        }
        StatsSendPlan::DropOffline { job_id: None, .. }
        | StatsSendPlan::Rewrite { job_id: None, .. } => unreachable!("service plan has a job id"),
    }
}

/// Process an outgoing Legacy (EMsg 818) CMsgClientGetUserStats request.
/// Returns `Some(modified_body_bytes)` if rewritten, `None` to pass through.
pub fn on_send_legacy_stats(
    body_bytes: &[u8],
    config: &RuntimeConfig,
    stat_steam_ids: &HashMap<AppId, u64>,
) -> Option<Vec<u8>> {
    match plan_send_legacy_stats(
        body_bytes,
        config,
        stat_steam_ids,
        crate::apps::actual_ownership,
    ) {
        StatsSendPlan::Pass => None,
        StatsSendPlan::DropOffline {
            app_id: _,
            job_id: None,
        } => Some(Vec::new()),
        StatsSendPlan::Rewrite {
            app_id,
            body,
            donor_steam_id,
            job_id: None,
            was_probe,
        } => {
            add_legacy_pending(app_id);
            debug!(
                app_id = app_id.0,
                ref_id = donor_steam_id,
                probe = was_probe,
                "achievements: redirected legacy full-schema request"
            );
            Some(body)
        }
        StatsSendPlan::DropOffline {
            job_id: Some(_), ..
        }
        | StatsSendPlan::Rewrite {
            job_id: Some(_), ..
        } => unreachable!("legacy plan has no job id"),
    }
}
