//! Process-wide bookkeeping for redirected stats requests.
//!
//! A stats request is rewritten to the donor account on the way out, so the
//! response that comes back is the donor's. Both directions are hooks on
//! different threads and neither carries a correlation id of its own, so the
//! outbound side records what it is waiting for:
//!
//! * the ServiceMethod path keys pending requests by `jobid_source`, which the
//!   response echoes as `jobid_target`;
//! * the legacy 818/819 path has no job id at all, so it counts outstanding
//!   requests per app;
//! * an offline app produces its own response here, queued until the caller
//!   drains it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use prost::Message;
use tracing::debug;
use vapor_forge_config::AppId;
use vapor_forge_steam_protocol::{
    assemble_raw, CMsgProtoBufHeader, PlayerGetUserStatsResponse, EMSG_SERVICE_METHOD_RESPONSE,
    ERESULT_NO_CONNECTION, K_MSG_HDR_PROTO_FLAG,
};

use super::DEFAULT_REF_STEAMID;

pub(super) fn get_ref_steamid(stat_steam_ids: &HashMap<AppId, u64>, app_id: AppId) -> u64 {
    stat_steam_ids
        .get(&app_id)
        .copied()
        .unwrap_or(DEFAULT_REF_STEAMID)
}

// ---------------------------------------------------------------------------
// Pending request tracking (ServiceMethod path)
// ---------------------------------------------------------------------------

static PENDING: Mutex<Option<HashMap<u64, AppId>>> = Mutex::new(None);
static PENDING_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn add_pending(job_id: u64, app_id: AppId) {
    let mut guard = PENDING.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert(job_id, app_id);
    PENDING_COUNT.store(map.len(), Ordering::Release);
}

pub(super) fn take_pending(job_id: u64) -> Option<AppId> {
    let mut guard = PENDING.lock().unwrap();
    let map = guard.as_mut()?;
    let app_id = map.remove(&job_id)?;
    PENDING_COUNT.store(map.len(), Ordering::Release);
    Some(app_id)
}

pub fn has_pending() -> bool {
    PENDING_COUNT.load(Ordering::Acquire) > 0
}

// ---------------------------------------------------------------------------
// Pending request tracking (Legacy 818/819 path)
// ---------------------------------------------------------------------------

static LEGACY_PENDING: Mutex<Option<HashMap<AppId, usize>>> = Mutex::new(None);
static LEGACY_PENDING_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(super) fn add_legacy_pending(app_id: AppId) {
    let mut guard = LEGACY_PENDING.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    *map.entry(app_id).or_insert(0) += 1;
    LEGACY_PENDING_COUNT.store(map.values().sum(), Ordering::Release);
}

pub(super) fn take_legacy_pending(app_id: AppId) -> bool {
    let mut guard = LEGACY_PENDING.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return false;
    };
    let Some(count) = map.get_mut(&app_id) else {
        return false;
    };
    *count -= 1;
    if *count == 0 {
        map.remove(&app_id);
    }
    LEGACY_PENDING_COUNT.store(map.values().sum(), Ordering::Release);
    true
}

pub fn has_legacy_pending() -> bool {
    LEGACY_PENDING_COUNT.load(Ordering::Acquire) > 0
}

// ---------------------------------------------------------------------------
// Offline response queue
// ---------------------------------------------------------------------------

pub struct OfflineResponse {
    pub packet: Vec<u8>,
    pub generation: u64,
}

static OFFLINE_QUEUE: Mutex<Option<Vec<OfflineResponse>>> = Mutex::new(None);

pub(super) fn queue_offline_response(
    app_id: AppId,
    job_id: u64,
    req_hdr: &CMsgProtoBufHeader,
) -> bool {
    let resp_hdr = CMsgProtoBufHeader {
        steamid: req_hdr.steamid,
        jobid_source: None,
        jobid_target: Some(job_id),
        target_job_name: req_hdr.target_job_name.clone(),
        eresult: Some(ERESULT_NO_CONNECTION),
        transport_error: None,
        seq_num: None,
        ..Default::default()
    };
    let resp_body = PlayerGetUserStatsResponse::default();

    let hdr_bytes = resp_hdr.encode_to_vec();
    let body_bytes = resp_body.encode_to_vec();
    let emsg = EMSG_SERVICE_METHOD_RESPONSE | K_MSG_HDR_PROTO_FLAG;
    let packet = assemble_raw(emsg, &hdr_bytes, &body_bytes);

    {
        let mut guard = OFFLINE_QUEUE.lock().unwrap();
        let queue = guard.get_or_insert_with(Vec::new);
        queue.push(OfflineResponse {
            packet,
            generation: crate::inject_wake::injection_generation(),
        });
    }
    debug!(
        app_id = app_id.0,
        "achievements: queued offline NO_CONNECTION response"
    );
    // Dispatch now instead of waiting for the next inbound packet.
    crate::inject_wake::wake(crate::inject_wake::InjectionSource::Achievements);
    true
}

pub fn drain_offline_responses() -> Vec<OfflineResponse> {
    let mut guard = OFFLINE_QUEUE.lock().unwrap();
    let queue = match guard.as_mut() {
        Some(q) => q,
        None => return Vec::new(),
    };
    std::mem::take(queue)
}
