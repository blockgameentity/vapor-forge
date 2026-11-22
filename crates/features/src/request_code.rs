//! Safe business logic for manifest request code interception.
//!
//! This module handles queuing, fetching, and response fabrication for
//! `ContentServerDirectory.GetManifestRequestCode#1` RPCs, all without
//! any `unsafe` code.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use prost::Message;
use thiserror::Error;
use tracing::{debug, info, warn};
use vapor_forge_config::{
    AppId, ManifestResponseFormat, ManifestSection, ManifestSource, RuntimeConfig,
};
use vapor_forge_steam_protocol::{
    CMsgProtoBufHeader, GetManifestRequestCodeResponse, EMSG_SERVICE_METHOD_RESPONSE,
    ERESULT_NO_CONNECTION, K_MSG_HDR_PROTO_FLAG,
};

pub type ManifestCodeCallback =
    Arc<dyn Fn(u32, u32, u64) -> Result<Option<u64>, String> + Send + Sync>;

pub struct ManifestCodeFetch {
    pub job_id: u64,
    pub app_id: u32,
    pub depot_id: u32,
    pub gid: u64,
    pub req_hdr_bytes: Vec<u8>,
}

#[derive(Debug)]
pub enum ManifestFetchError {
    Decode(prost::DecodeError),
    MissingJobId,
}

pub fn plan_fetch(
    header: &CMsgProtoBufHeader,
    header_bytes: &[u8],
    body_bytes: &[u8],
) -> Result<ManifestCodeFetch, ManifestFetchError> {
    let request = vapor_forge_steam_protocol::GetManifestRequestCodeRequest::decode(body_bytes)
        .map_err(ManifestFetchError::Decode)?;
    Ok(ManifestCodeFetch {
        job_id: header
            .jobid_source
            .filter(|job_id| *job_id != 0)
            .ok_or(ManifestFetchError::MissingJobId)?,
        app_id: request.app_id.unwrap_or(0),
        depot_id: request.depot_id.unwrap_or(0),
        gid: request.manifest_id.unwrap_or(0),
        req_hdr_bytes: header_bytes.to_vec(),
    })
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// ServiceMethod name we intercept.
pub const TARGET_JOB_NAME: &str = vapor_forge_steam_protocol::MANIFEST_REQUEST_CODE_JOB_NAME;

const MAX_PENDING_FETCHES: usize = 16;

// ---------------------------------------------------------------------------
// Pending fetch state
// ---------------------------------------------------------------------------

/// Name reported for the script (Lua) provider. The chain never contacts a
/// host for it, but the caller still deserves to know who served the code.
const SCRIPT_PROVIDER: &str = "script";

/// What one completed fetch produced.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FetchOutcome {
    /// `0` when no provider served the code, which the response layer turns
    /// into `EResult No Connection`.
    pub code: u64,
    /// Provider that served the code, so the caller can surface it.
    pub provider: Option<String>,
}

struct Pending {
    job_id: u64,
    gid: u64,
    result: Arc<Mutex<Option<FetchOutcome>>>,
    done: Arc<AtomicBool>,
    /// Serialized request header bytes used to fabricate the response header.
    req_hdr_bytes: Vec<u8>,
    response_generation: u64,
}

/// A completed manifest code fetch, ready for injection.
pub struct CompletedFetch {
    pub job_id: u64,
    pub gid: u64,
    pub code: u64,
    /// Provider that served `code`; `None` when no provider did.
    pub provider: Option<String>,
    pub req_hdr_bytes: Vec<u8>,
    pub response_generation: u64,
}

/// Thread-safe queue of in-flight manifest code fetches.
pub struct PendingQueue {
    list: Mutex<Vec<Pending>>,
}

impl PendingQueue {
    pub fn new() -> Self {
        Self {
            list: Mutex::new(Vec::new()),
        }
    }

    /// Queue a new manifest code fetch. Spawns a background thread to call
    /// the configured providers and stores the result for later draining.
    pub fn queue_fetch(
        &self,
        request: ManifestCodeFetch,
        manifest: &ManifestSection,
        script_callback: Option<ManifestCodeCallback>,
        response_generation: u64,
    ) -> bool {
        let ManifestCodeFetch {
            job_id,
            app_id,
            depot_id,
            gid,
            req_hdr_bytes,
        } = request;
        let result = Arc::new(Mutex::new(None));
        let done = Arc::new(AtomicBool::new(false));

        let pending = Pending {
            job_id,
            gid,
            result: Arc::clone(&result),
            done: Arc::clone(&done),
            req_hdr_bytes,
            response_generation,
        };

        {
            let mut list = self.list.lock().unwrap();
            if list.len() >= MAX_PENDING_FETCHES {
                warn!(job_id, gid, "request_code: pending queue full");
                return false;
            }
            list.push(pending);
        }

        let result_clone = Arc::clone(&result);
        let done_clone = Arc::clone(&done);
        let providers = manifest.providers.clone();
        let failover = manifest.failover;
        let timeout_connect_ms = manifest.timeout_connect_ms;
        let timeout_ms = manifest.timeout_ms;
        let min_interval_ms = manifest.min_interval_ms;
        let spawn_result = std::thread::Builder::new()
            .name("manifest-code-fetch".to_owned())
            .spawn(move || {
                let script_code = script_callback.and_then(|callback| {
                    match catch_unwind(AssertUnwindSafe(|| callback(app_id, depot_id, gid))) {
                        Ok(Ok(code)) => code.filter(|code| *code > 0),
                        Ok(Err(error)) => {
                            warn!(gid, %error, "request_code: script provider unavailable");
                            None
                        }
                        Err(_) => {
                            warn!(gid, "request_code: script provider panicked");
                            None
                        }
                    }
                });
                let fetched = match script_code {
                    Some(code) => {
                        info!(gid, depot_id, code, "request_code: manifest code obtained");
                        Ok(ChainSuccess {
                            code,
                            provider: SCRIPT_PROVIDER.to_owned(),
                            attempts: vec![ProviderAttempt {
                                provider: SCRIPT_PROVIDER.to_owned(),
                                status: AttemptStatus::Served,
                            }],
                        })
                    }
                    None => fetch_manifest_code(
                        FetchContext {
                            app_id,
                            depot_id,
                            gid,
                        },
                        &providers,
                        failover,
                        timeout_connect_ms,
                        timeout_ms,
                        min_interval_ms,
                    ),
                };
                let outcome = match fetched {
                    Ok(success) => FetchOutcome {
                        code: success.code,
                        provider: Some(success.provider),
                    },
                    Err(error) => {
                        // The caller can only answer with "no connection", so
                        // the reason has to be visible here or nowhere.
                        warn!(gid, depot_id, %error, "request_code: no manifest code obtained");
                        FetchOutcome::default()
                    }
                };
                {
                    let mut lock = result_clone.lock().unwrap();
                    *lock = Some(outcome.clone());
                }
                done_clone.store(true, Ordering::Release);
                debug!(
                    gid,
                    code = outcome.code,
                    provider = outcome.provider.as_deref().unwrap_or("none"),
                    "request_code: fetch complete"
                );
                // Dispatch the completed manifest response now instead of waiting
                // for the next inbound packet.
                crate::inject_wake::wake(crate::inject_wake::InjectionSource::Manifest);
            });
        if let Err(error) = spawn_result {
            let mut list = self.list.lock().unwrap();
            if let Some(index) = list
                .iter()
                .position(|pending| pending.job_id == job_id && pending.gid == gid)
            {
                list.swap_remove(index);
            }
            warn!(job_id, gid, %error, "request_code: failed to start fetch thread");
            return false;
        }
        true
    }

    /// Drain all completed entries from the pending list.
    pub fn drain_completed(&self) -> Vec<CompletedFetch> {
        let mut list = self.list.lock().unwrap();
        let mut completed = Vec::new();
        let mut i = 0;

        while i < list.len() {
            if list[i].done.load(Ordering::Acquire) {
                let entry = list.swap_remove(i);

                let outcome = entry.result.lock().unwrap().clone().unwrap_or_default();
                completed.push(CompletedFetch {
                    job_id: entry.job_id,
                    gid: entry.gid,
                    code: outcome.code,
                    provider: outcome.provider,
                    req_hdr_bytes: entry.req_hdr_bytes,
                    response_generation: entry.response_generation,
                });
                // Don't increment i because swap_remove moved the last element to position i.
            } else {
                i += 1;
            }
        }

        completed
    }
}

impl Default for PendingQueue {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Interception check
// ---------------------------------------------------------------------------

/// Returns `true` if this app requires a local manifest request-code response.
pub fn should_intercept(app_id: AppId, config: &RuntimeConfig) -> bool {
    should_intercept_with_ownership(app_id, config, crate::apps::actual_ownership)
}

pub fn should_intercept_with_ownership(
    app_id: AppId,
    config: &RuntimeConfig,
    ownership: impl FnOnce(AppId) -> crate::apps::OwnershipState,
) -> bool {
    crate::apps::classify_app_with_ownership(config, app_id, ownership)
        .requires_injected_ownership()
}

// ---------------------------------------------------------------------------
// Provider chain: response parsing, pacing and cooldown tuning
// ---------------------------------------------------------------------------

const MAX_PROVIDER_RESPONSE_BYTES: u64 = 4096;

/// Cooldown applied when a provider answers 429 without a usable `Retry-After`.
const DEFAULT_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(60);
/// Ceiling on any cooldown, so one absurd `Retry-After` cannot park a provider
/// for the rest of the Steam session.
const MAX_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(300);
/// Applied when a provider itself is unhealthy (transport error, 5xx, a
/// Cloudflare block), once another provider has served the same gid. Steam
/// asks for one code per depot, so without this a dead source costs a wasted
/// round trip on every single gid.
const FAILURE_COOLDOWN: Duration = Duration::from_secs(60);

/// How long a served code keeps its provider at the back of the chain for the
/// same gid.
///
/// Steam asks for a gid's code again only when it needs that manifest again.
/// Within minutes that means the CDN refused the code: Steam logs the 401 as
/// "No Connection" and retries the whole update. Every provider hands out a
/// refused code now and then, and asking the same one again tends to repeat
/// it, so a quick re-request goes to the others first.
const REREQUEST_WINDOW: Duration = Duration::from_secs(600);

/// Provider bookkeeping shared by every fetch thread.
#[derive(Default)]
struct ProviderState {
    /// Instants before which a provider must not be contacted again.
    ///
    /// Steam resolves several depot manifests back to back. Without this, one
    /// 429 would be re-earned on every following gid: a wasted round trip each
    /// time, and more load on a provider that just asked us to back off.
    cooldowns: Mutex<HashMap<String, Instant>>,
    /// Providers that served a code within [`REREQUEST_WINDOW`], per gid,
    /// least recent first.
    served: Mutex<HashMap<u64, Vec<(String, Instant)>>>,
}

static PROVIDER_STATE: OnceLock<ProviderState> = OnceLock::new();

fn provider_state() -> &'static ProviderState {
    PROVIDER_STATE.get_or_init(ProviderState::default)
}

/// When each provider was last contacted, for the self-imposed pacing.
static PROVIDER_LAST_REQUEST: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn provider_last_request() -> &'static Mutex<HashMap<String, Instant>> {
    PROVIDER_LAST_REQUEST.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What one manifest code request is for. `{depotId}` / `{appId}` providers
/// need more than the gid.
#[derive(Clone, Copy, Debug)]
struct FetchContext {
    app_id: u32,
    depot_id: u32,
    gid: u64,
}

/// How long to wait before the next request to `name` may go out, reserving
/// the slot when the wait is zero.
///
/// Returning the wait instead of sleeping keeps the lock out of the sleep and
/// keeps this testable.
fn reserve_slot_in(
    last: &mut HashMap<String, Instant>,
    name: &str,
    now: Instant,
    min_interval: Duration,
) -> Option<Duration> {
    match last.get(name) {
        Some(&previous) => {
            let elapsed = now.saturating_duration_since(previous);
            if elapsed < min_interval {
                return Some(min_interval - elapsed);
            }
            last.insert(name.to_owned(), now);
            None
        }
        None => {
            last.insert(name.to_owned(), now);
            None
        }
    }
}

/// Block until this provider may be contacted again.
fn wait_for_slot(name: &str, min_interval: Duration) {
    if min_interval.is_zero() {
        return;
    }
    loop {
        let wait = {
            let mut last = provider_last_request()
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            reserve_slot_in(&mut last, name, Instant::now(), min_interval)
        };
        match wait {
            Some(wait) => std::thread::sleep(wait),
            None => return,
        }
    }
}

/// Outcome of one provider attempt. 429 is kept distinct from ordinary failure
/// so the caller can park the provider instead of just moving to the next one.
#[derive(Debug)]
enum ProviderOutcome {
    Code(u64),
    RateLimited(Duration),
    /// The provider is answering, but not for this target — a provider that
    /// looks up codes through licensed accounts says so per request (a 401
    /// `Unauthorized`). Nothing is wrong with the source, so it must NOT be
    /// put on cooldown: the next gid may well be one it can serve.
    Denied(u16),
    /// The provider itself is unhealthy; put it on cooldown.
    Failed(String),
}

/// What one attempt amounts to, kept for the logs and for the caller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttemptStatus {
    /// The provider returned a usable code.
    Served,
    /// The provider answered but had no code for this gid.
    Denied(u16),
    /// The provider asked us to come back later.
    RateLimited { cooldown_ms: u64 },
    /// The provider answered with a zero, which is not a code.
    Zero,
    /// Transport error, unexpected status, or an unparsable body.
    Failed(String),
    /// Not contacted: still inside a cooldown the provider asked for.
    CoolingDown { remaining_ms: u64 },
    /// Not contacted: the entry itself is malformed.
    Rejected(String),
}

impl std::fmt::Display for AttemptStatus {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Served => formatter.write_str("served a code"),
            Self::Denied(status) => {
                write!(formatter, "has no code for this manifest (http {status})")
            }
            Self::RateLimited { cooldown_ms } => {
                write!(formatter, "rate limited, backing off for {cooldown_ms} ms")
            }
            Self::Zero => formatter.write_str("returned zero"),
            Self::Failed(error) => write!(formatter, "failed: {error}"),
            Self::CoolingDown { remaining_ms } => {
                write!(
                    formatter,
                    "skipped, still cooling down for {remaining_ms} ms"
                )
            }
            Self::Rejected(reason) => write!(formatter, "skipped, unusable entry: {reason}"),
        }
    }
}

/// One provider the chain considered, in the order it considered it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderAttempt {
    pub provider: String,
    pub status: AttemptStatus,
}

/// A served manifest code plus the trail that produced it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChainSuccess {
    pub code: u64,
    /// Name of the provider that served the code.
    pub provider: String,
    /// Every provider contacted or skipped before that, ending with the one
    /// that served. This is what makes a failover run inspectable: no attempt
    /// is invisible, even the ones the logs rolled past.
    pub attempts: Vec<ProviderAttempt>,
}

/// Why no code could be obtained.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum ChainError {
    /// Failover is off: only the first configured provider was tried, so the
    /// failure is reported against that provider alone.
    #[error(
        "manifest request code provider `{provider}` {status}; failover is off, so no other provider was tried"
    )]
    NoFailover {
        provider: String,
        status: AttemptStatus,
    },
    /// Nothing is configured to ask: `[manifest] providers` is empty.
    ///
    /// Distinct from [`ChainError::ChainFailed`] on purpose — no provider was
    /// tried, so this is a configuration state, not a failed lookup, and it must
    /// not read as "every provider failed".
    #[error(
        "no manifest provider configured: the host tool must write at least one [manifest] providers entry, and none is set"
    )]
    NoProviderConfigured,
    /// Failover is on and the whole chain came up empty.
    #[error("no provider served gid {gid}: {}", describe_attempts(.attempts))]
    ChainFailed {
        gid: u64,
        attempts: Vec<ProviderAttempt>,
    },
}

/// `name status; name status` form, for logs and for [`ChainError::ChainFailed`].
fn describe_attempts(attempts: &[ProviderAttempt]) -> String {
    attempts
        .iter()
        .map(|attempt| format!("{} {}", attempt.provider, attempt.status))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Remaining cooldown for `provider`, clearing the entry once it has expired.
fn cooldown_remaining_in(
    cooldowns: &mut HashMap<String, Instant>,
    name: &str,
    now: Instant,
) -> Option<Duration> {
    let until = *cooldowns.get(name)?;
    if until > now {
        Some(until - now)
    } else {
        cooldowns.remove(name);
        None
    }
}

fn start_cooldown_in(
    cooldowns: &mut HashMap<String, Instant>,
    name: &str,
    now: Instant,
    retry_after: Duration,
) -> Duration {
    let capped = retry_after.min(MAX_RATE_LIMIT_COOLDOWN);
    cooldowns.insert(name.to_owned(), now + capped);
    capped
}

/// Providers that served `gid` within [`REREQUEST_WINDOW`], least recent first.
/// Entries past the window are dropped for every gid, so the table only holds
/// the codes of the current update.
fn served_by_in(
    served: &mut HashMap<u64, Vec<(String, Instant)>>,
    gid: u64,
    now: Instant,
) -> Vec<String> {
    served.retain(|_, entries| {
        entries.retain(|(_, at)| now.saturating_duration_since(*at) < REREQUEST_WINDOW);
        !entries.is_empty()
    });
    served
        .get(&gid)
        .map(|entries| entries.iter().map(|(name, _)| name.clone()).collect())
        .unwrap_or_default()
}

fn record_served_in(
    served: &mut HashMap<u64, Vec<(String, Instant)>>,
    gid: u64,
    name: &str,
    now: Instant,
) {
    let entries = served.entry(gid).or_default();
    entries.retain(|(served_name, _)| served_name != name);
    entries.push((name.to_owned(), now));
}

/// Chain order for one gid: providers that have not served it recently keep
/// their configured order, the ones that have go last, least recent first.
fn order_providers<'a>(
    providers: &'a [ManifestSource],
    served: &[String],
) -> Vec<&'a ManifestSource> {
    let mut ordered: Vec<_> = providers
        .iter()
        .filter(|provider| !served.iter().any(|name| name == provider.name()))
        .collect();
    for name in served {
        ordered.extend(providers.iter().filter(|provider| provider.name() == name));
    }
    ordered
}

impl ProviderState {
    fn cooldown_remaining(&self, name: &str) -> Option<Duration> {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cooldown_remaining_in(&mut cooldowns, name, Instant::now())
    }

    fn start_cooldown(&self, name: &str, retry_after: Duration) -> Duration {
        let mut cooldowns = self
            .cooldowns
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        start_cooldown_in(&mut cooldowns, name, Instant::now(), retry_after)
    }

    fn served_by(&self, gid: u64) -> Vec<String> {
        let mut served = self
            .served
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        served_by_in(&mut served, gid, Instant::now())
    }

    fn record_served(&self, gid: u64, name: &str) {
        let mut served = self
            .served
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        record_served_in(&mut served, gid, name, Instant::now());
    }
}

/// `Retry-After` in its delta-seconds form. The HTTP-date form is not accepted:
/// ManifestDeX documents seconds, and anything unparsable falls back to
/// [`DEFAULT_RATE_LIMIT_COOLDOWN`] rather than being treated as "retry now".
fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    let seconds = value?.trim().parse::<u64>().ok()?;
    Some(Duration::from_secs(seconds))
}

fn fetch_manifest_code(
    context: FetchContext,
    providers: &[ManifestSource],
    failover: bool,
    timeout_connect_ms: u64,
    timeout_ms: u64,
    min_interval_ms: u64,
) -> Result<ChainSuccess, ChainError> {
    let FetchContext {
        app_id,
        depot_id,
        gid,
    } = context;

    run_provider_chain(provider_state(), context, providers, failover, |provider| {
        let url = provider.resolve_url(app_id, depot_id, gid);
        // Pace before the request, not after a 429: these are small shared
        // services and the cooldown only reacts once the damage is done.
        wait_for_slot(provider.name(), provider.min_interval(min_interval_ms));
        fetch_from_provider(provider, &url, timeout_connect_ms, timeout_ms)
    })
}

fn run_provider_chain(
    state: &ProviderState,
    context: FetchContext,
    providers: &[ManifestSource],
    failover: bool,
    attempt: impl FnMut(&ManifestSource) -> ProviderOutcome,
) -> Result<ChainSuccess, ChainError> {
    if providers.is_empty() {
        return Err(ChainError::NoProviderConfigured);
    }
    if !failover {
        return run_configured_provider(state, context, providers, attempt);
    }
    run_failover_chain(state, context, providers, attempt)
}

/// Failover off: ask the first configured provider and nothing else.
///
/// The user picked a list; without `failover = true` only its head is used, so
/// a depot lookup can never land on a host they did not choose. A failure is
/// reported against that provider by name, which is also the only way the user
/// learns which host their request went to.
fn run_configured_provider(
    state: &ProviderState,
    context: FetchContext,
    providers: &[ManifestSource],
    mut attempt: impl FnMut(&ManifestSource) -> ProviderOutcome,
) -> Result<ChainSuccess, ChainError> {
    let FetchContext { depot_id, gid, .. } = context;
    let provider = &providers[0];
    let name = provider.name().to_owned();
    if providers.len() > 1 {
        // Not a warning: this is the default and the documented behaviour. It
        // is logged so a user wondering why host B was never asked can see it.
        debug!(
            provider = %name,
            unused = providers.len() - 1,
            "request_code: failover is off, only the first configured provider is used"
        );
    }

    // Malformed entries and cooldowns are handled exactly as in failover mode;
    // there is simply no next entry to move on to.
    let status = if let Some(rejection) = provider.rejection() {
        warn!(gid, %rejection, "request_code: provider entry is unusable");
        AttemptStatus::Rejected(rejection)
    } else if let Some(remaining) = state.cooldown_remaining(&name) {
        let remaining_ms = remaining.as_millis() as u64;
        debug!(
            provider = %name,
            gid, remaining_ms, "request_code: provider cooling down, not contacted"
        );
        AttemptStatus::CoolingDown { remaining_ms }
    } else {
        debug!(
            provider = %name,
            gid, depot_id, "request_code: trying provider"
        );
        match attempt(provider) {
            ProviderOutcome::Code(code) if code > 0 => {
                state.record_served(gid, &name);
                info!(
                    provider = %name,
                    gid, depot_id, code,
                    status = %AttemptStatus::Served,
                    "request_code: manifest code obtained"
                );
                return Ok(ChainSuccess {
                    code,
                    provider: name.clone(),
                    attempts: vec![ProviderAttempt {
                        provider: name,
                        status: AttemptStatus::Served,
                    }],
                });
            }
            other => record_refusal(state, context, &name, other),
        }
    };

    info!(
        provider = %name,
        gid, %status, "request_code: attempt finished"
    );
    Err(ChainError::NoFailover {
        provider: name,
        status,
    })
}

/// Failover on: walk the configured order until a provider serves the gid.
fn run_failover_chain(
    state: &ProviderState,
    context: FetchContext,
    providers: &[ManifestSource],
    mut attempt: impl FnMut(&ManifestSource) -> ProviderOutcome,
) -> Result<ChainSuccess, ChainError> {
    let FetchContext { depot_id, gid, .. } = context;

    let served = state.served_by(gid);
    if !served.is_empty() {
        info!(
            gid,
            depot_id,
            served = ?served,
            "request_code: gid requested again, trying other providers first"
        );
    }

    // A failure only says something about the provider once another one has
    // answered the same gid. When the whole chain fails, the gid is the likelier
    // cause, and cooling every provider down would turn the following gids of
    // the same update into instant failures.
    let mut failed: Vec<String> = Vec::new();
    let mut attempts = Vec::new();

    for provider in order_providers(providers, &served) {
        let name = provider.name().to_owned();

        // A user-defined entry can be malformed; skip it and keep the rest of
        // the chain working rather than failing the whole fetch.
        if let Some(rejection) = provider.rejection() {
            warn!(gid, %rejection, "request_code: skipping unusable provider");
            attempts.push(ProviderAttempt {
                provider: name,
                status: AttemptStatus::Rejected(rejection),
            });
            continue;
        }

        if let Some(remaining) = state.cooldown_remaining(&name) {
            let remaining_ms = remaining.as_millis() as u64;
            debug!(
                provider = %name,
                gid, remaining_ms, "request_code: provider cooling down, skipping"
            );
            attempts.push(ProviderAttempt {
                provider: name,
                status: AttemptStatus::CoolingDown { remaining_ms },
            });
            continue;
        }

        debug!(
            provider = %name,
            gid, depot_id, "request_code: trying provider"
        );

        match attempt(provider) {
            ProviderOutcome::Code(code) if code > 0 => {
                info!(
                    provider = %name,
                    gid, depot_id, code,
                    status = %AttemptStatus::Served,
                    "request_code: manifest code obtained"
                );
                state.record_served(gid, &name);
                attempts.push(ProviderAttempt {
                    provider: name.clone(),
                    status: AttemptStatus::Served,
                });
                for failed_name in failed {
                    let cooldown = state.start_cooldown(&failed_name, FAILURE_COOLDOWN);
                    warn!(
                        provider = failed_name,
                        gid,
                        cooldown_ms = cooldown.as_millis() as u64,
                        "request_code: backing off failed provider"
                    );
                }
                return Ok(ChainSuccess {
                    code,
                    provider: name,
                    attempts,
                });
            }
            other => {
                let status = record_refusal(state, context, &name, other);
                // Every attempt is visible, including the ones that only
                // produced a reason to move on to the next provider.
                info!(
                    provider = %name,
                    gid, %status, "request_code: attempt finished"
                );
                if matches!(status, AttemptStatus::Failed(_)) {
                    failed.push(name.clone());
                }
                attempts.push(ProviderAttempt {
                    provider: name,
                    status,
                });
            }
        }
    }

    if !failed.is_empty() {
        warn!(
            gid,
            depot_id,
            failed = ?failed,
            "request_code: no provider served this gid, failures not backed off"
        );
    }
    Err(ChainError::ChainFailed { gid, attempts })
}

/// Record one refused attempt: apply the bookkeeping its class requires and
/// turn it into the status the logs and the caller see.
fn record_refusal(
    state: &ProviderState,
    context: FetchContext,
    provider: &str,
    outcome: ProviderOutcome,
) -> AttemptStatus {
    let FetchContext { depot_id, gid, .. } = context;
    match outcome {
        // `Code(0)` — the `> 0` guard above already took the usable ones.
        ProviderOutcome::Code(_) => {
            warn!(provider, gid, "request_code: provider returned zero");
            AttemptStatus::Zero
        }
        ProviderOutcome::Denied(status) => {
            // Per-target answer, not a provider fault: no cooldown.
            debug!(
                provider,
                gid, depot_id, status, "request_code: provider does not have this manifest"
            );
            AttemptStatus::Denied(status)
        }
        ProviderOutcome::RateLimited(retry_after) => {
            let cooldown = state.start_cooldown(provider, retry_after);
            let cooldown_ms = cooldown.as_millis() as u64;
            warn!(
                provider,
                gid, cooldown_ms, "request_code: provider rate limited, backing off"
            );
            AttemptStatus::RateLimited { cooldown_ms }
        }
        ProviderOutcome::Failed(error) => {
            warn!(provider, gid, %error, "request_code: provider failed");
            AttemptStatus::Failed(error)
        }
    }
}

fn fetch_from_provider(
    provider: &ManifestSource,
    url: &str,
    timeout_connect_ms: u64,
    timeout_ms: u64,
) -> ProviderOutcome {
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_millis(timeout_connect_ms)))
        .timeout_global(Some(Duration::from_millis(timeout_ms)))
        // Keep non-2xx as a response so 429 can be told apart from a transport
        // failure and its Retry-After header read.
        .http_status_as_error(false)
        .build()
        .new_agent();

    let mut request = agent.get(url);
    if let Some(user_agent) = provider.user_agent() {
        request = request.header("User-Agent", user_agent);
    }

    let mut response = match request.call() {
        Ok(response) => response,
        Err(error) => return ProviderOutcome::Failed(error.to_string()),
    };

    let status = response.status();
    if status.as_u16() == 429 {
        let retry_after = parse_retry_after(
            response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
        )
        .unwrap_or(DEFAULT_RATE_LIMIT_COOLDOWN);
        return ProviderOutcome::RateLimited(retry_after);
    }
    // 401/404 mean "not this manifest", not "this source is broken". Everything
    // else non-2xx — including 403, which here is a Cloudflare block rather than
    // a per-target refusal — is treated as the provider being unhealthy.
    if matches!(status.as_u16(), 401 | 404) {
        return ProviderOutcome::Denied(status.as_u16());
    }
    if !status.is_success() {
        return ProviderOutcome::Failed(format!("http status {}", status.as_u16()));
    }

    let body = match response
        .body_mut()
        .with_config()
        .limit(MAX_PROVIDER_RESPONSE_BYTES)
        .read_to_string()
    {
        Ok(body) => body,
        Err(error) => return ProviderOutcome::Failed(error.to_string()),
    };

    match parse_provider_response(provider, body.trim()) {
        Ok(code) => ProviderOutcome::Code(code),
        Err(error) => ProviderOutcome::Failed(error),
    }
}

fn parse_provider_response(provider: &ManifestSource, body: &str) -> Result<u64, String> {
    let body = body.trim();
    if provider.response_format() == ManifestResponseFormat::Json {
        let field = provider
            .json_field()
            .ok_or_else(|| "json provider has no json_field".to_owned())?;
        let value: serde_json::Value =
            serde_json::from_str(body).map_err(|error| error.to_string())?;
        let found = value
            .get(field)
            .ok_or_else(|| format!("missing {field} field"))?;
        // Accept both `{"content":"123"}` and `{"content":123}`: a custom
        // endpoint's shape is not ours to dictate.
        return match found {
            serde_json::Value::String(text) => text
                .trim()
                .parse::<u64>()
                .map_err(|error| error.to_string()),
            serde_json::Value::Number(number) => number
                .as_u64()
                .ok_or_else(|| format!("{field} is not an unsigned integer")),
            _ => Err(format!("{field} is neither a string nor a number")),
        };
    }

    body.parse::<u64>().map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Response fabrication
// ---------------------------------------------------------------------------

/// Build a complete fabricated `ServiceMethodResponse` packet from parts.
///
/// Uses protobuf types from `steam-protocol` and `assemble_raw` to produce the
/// final byte vector using safe code, with no raw pointers involved.
pub fn build_response_packet(req_hdr_bytes: &[u8], _job_id: u64, _gid: u64, code: u64) -> Vec<u8> {
    // Parse the original request header to copy fields
    let req_hdr = CMsgProtoBufHeader::decode(req_hdr_bytes).unwrap_or_default();

    let eresult = if code > 0 {
        Some(1_i32)
    } else {
        Some(ERESULT_NO_CONNECTION)
    };

    let resp_hdr = CMsgProtoBufHeader {
        steamid: req_hdr.steamid,
        jobid_source: None,
        jobid_target: req_hdr.jobid_source, // route response to the original caller
        target_job_name: req_hdr.target_job_name.clone(),
        eresult,
        transport_error: None,
        seq_num: None,
        ..Default::default()
    };

    let resp_body = GetManifestRequestCodeResponse {
        manifest_request_code: if code > 0 { Some(code) } else { None },
    };

    let hdr_bytes = resp_hdr.encode_to_vec();
    let body_bytes = resp_body.encode_to_vec();
    let emsg_raw = EMSG_SERVICE_METHOD_RESPONSE | K_MSG_HDR_PROTO_FLAG;

    vapor_forge_steam_protocol::assemble_raw(emsg_raw, &hdr_bytes, &body_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    fn make_req_header(job_id: u64, method: &str) -> Vec<u8> {
        CMsgProtoBufHeader {
            steamid: Some(0x0110_0001_DEAD_BEEF),
            jobid_source: Some(job_id),
            target_job_name: Some(method.to_owned()),
            ..Default::default()
        }
        .encode_to_vec()
    }

    fn script_only_manifest() -> ManifestSection {
        ManifestSection {
            providers: Vec::new(),
            ..ManifestSection::default()
        }
    }

    #[test]
    fn build_response_routes_to_caller() {
        let req_hdr = make_req_header(42, TARGET_JOB_NAME);
        let packet = build_response_packet(&req_hdr, 42, 123, 99999);
        let (emsg, hdr_bytes, body_bytes) =
            vapor_forge_steam_protocol::unpack_raw(&packet).unwrap();
        assert_eq!(emsg, EMSG_SERVICE_METHOD_RESPONSE | K_MSG_HDR_PROTO_FLAG);

        let resp_hdr = CMsgProtoBufHeader::decode(hdr_bytes).unwrap();
        assert_eq!(resp_hdr.jobid_target, Some(42));
        assert_eq!(resp_hdr.eresult, Some(1));

        let resp_body = GetManifestRequestCodeResponse::decode(body_bytes).unwrap();
        assert_eq!(resp_body.manifest_request_code, Some(99999));
    }

    #[test]
    fn build_response_failure_returns_no_connection() {
        let req_hdr = make_req_header(7, TARGET_JOB_NAME);
        let packet = build_response_packet(&req_hdr, 7, 123, 0);
        let (_, hdr_bytes, body_bytes) = vapor_forge_steam_protocol::unpack_raw(&packet).unwrap();

        let resp_hdr = CMsgProtoBufHeader::decode(hdr_bytes).unwrap();
        assert_eq!(resp_hdr.eresult, Some(ERESULT_NO_CONNECTION));
        assert_eq!(resp_hdr.transport_error, None);
        assert_eq!(resp_hdr.seq_num, None);

        let resp_body = GetManifestRequestCodeResponse::decode(body_bytes).unwrap();
        assert_eq!(resp_body.manifest_request_code, None);
    }

    #[test]
    fn fetch_plan_preserves_request_context_and_rejects_missing_jobs() {
        let header_bytes = make_req_header(42, TARGET_JOB_NAME);
        let header = CMsgProtoBufHeader::decode(header_bytes.as_slice()).unwrap();
        let body = vapor_forge_steam_protocol::GetManifestRequestCodeRequest {
            app_id: Some(480),
            depot_id: Some(481),
            manifest_id: Some(1234),
            ..Default::default()
        }
        .encode_to_vec();
        let fetch = plan_fetch(&header, &header_bytes, &body).unwrap();
        assert_eq!(fetch.job_id, 42);
        assert_eq!(fetch.app_id, 480);
        assert_eq!(fetch.depot_id, 481);
        assert_eq!(fetch.gid, 1234);
        assert_eq!(fetch.req_hdr_bytes, header_bytes);

        assert!(matches!(
            plan_fetch(&CMsgProtoBufHeader::default(), &[], &body),
            Err(ManifestFetchError::MissingJobId)
        ));
        assert!(matches!(
            plan_fetch(&header, &[], &[0xff]),
            Err(ManifestFetchError::Decode(_))
        ));
    }

    #[test]
    fn should_intercept_only_when_injected_ownership_is_required() {
        let config = RuntimeConfig {
            apps: vapor_forge_config::AppsSection::with_inject(vec![
                vapor_forge_config::InjectApp {
                    id: AppId(480),
                    dlc: Vec::new(),
                    ticket: Default::default(),
                    purchase_time: 0,
                },
            ]),
            ..Default::default()
        };
        assert!(should_intercept(AppId(480), &config));
        assert!(!should_intercept(AppId(999), &config));

        let without_provider = RuntimeConfig {
            manifest: script_only_manifest(),
            ..config.clone()
        };
        assert!(should_intercept(AppId(480), &without_provider));

        let owned_app = AppId(246_813_583);
        let owned_config = RuntimeConfig {
            apps: vapor_forge_config::AppsSection::with_inject(vec![
                vapor_forge_config::InjectApp {
                    id: owned_app,
                    dlc: Vec::new(),
                    ticket: Default::default(),
                    purchase_time: 0,
                },
            ]),
            ..Default::default()
        };
        crate::apps::record_actual_ownership(owned_app, true);
        assert!(!should_intercept(owned_app, &owned_config));
    }

    #[test]
    fn pending_queue_uses_lua_callback_with_request_context() {
        let queue = PendingQueue::new();
        let seen = Arc::new(Mutex::new(None));
        let seen_by_callback = Arc::clone(&seen);
        let callback: ManifestCodeCallback = Arc::new(move |app_id, depot_id, gid| {
            *seen_by_callback.lock().unwrap() = Some((app_id, depot_id, gid));
            Ok(Some(4444))
        });

        assert!(queue.queue_fetch(
            ManifestCodeFetch {
                job_id: 99,
                app_id: 480,
                depot_id: 481,
                gid: 1234,
                req_hdr_bytes: make_req_header(99, TARGET_JOB_NAME),
            },
            &script_only_manifest(),
            Some(callback),
            7,
        ));

        let completed = (0..100)
            .find_map(|_| {
                let completed = queue.drain_completed();
                if completed.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    None
                } else {
                    Some(completed)
                }
            })
            .expect("Lua callback did not complete");
        assert_eq!(*seen.lock().unwrap(), Some((480, 481, 1234)));
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].code, 4444);
        // The Lua provider is named too, so the caller can say who answered.
        assert_eq!(completed[0].provider.as_deref(), Some("script"));
    }

    #[test]
    fn pending_queue_rejects_overload() {
        let queue = PendingQueue::new();
        let barrier = Arc::new(std::sync::Barrier::new(MAX_PENDING_FETCHES + 1));
        let callback: ManifestCodeCallback = {
            let barrier = Arc::clone(&barrier);
            Arc::new(move |_, _, _| {
                barrier.wait();
                Ok(Some(1))
            })
        };

        for job_id in 0..MAX_PENDING_FETCHES as u64 {
            assert!(queue.queue_fetch(
                ManifestCodeFetch {
                    job_id: job_id + 1,
                    app_id: 480,
                    depot_id: 481,
                    gid: job_id + 100,
                    req_hdr_bytes: Vec::new(),
                },
                &script_only_manifest(),
                Some(Arc::clone(&callback)),
                7,
            ));
        }
        assert!(!queue.queue_fetch(
            ManifestCodeFetch {
                job_id: 99,
                app_id: 480,
                depot_id: 481,
                gid: 999,
                req_hdr_bytes: Vec::new(),
            },
            &script_only_manifest(),
            Some(callback),
            7,
        ));
        barrier.wait();
    }

    #[test]
    fn pending_queue_completes_provider_failure() {
        let queue = PendingQueue::new();
        let callback: ManifestCodeCallback = Arc::new(|_, _, _| Err("unavailable".to_owned()));
        assert!(queue.queue_fetch(
            ManifestCodeFetch {
                job_id: 99,
                app_id: 480,
                depot_id: 481,
                gid: 1234,
                req_hdr_bytes: make_req_header(99, TARGET_JOB_NAME),
            },
            &script_only_manifest(),
            Some(callback),
            7,
        ));

        let completed = (0..100)
            .find_map(|_| {
                let completed = queue.drain_completed();
                if completed.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    None
                } else {
                    Some(completed)
                }
            })
            .expect("provider failure did not complete");
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].job_id, 99);
        assert_eq!(completed[0].code, 0);
        // Nothing served it, so no provider may be claimed.
        assert_eq!(completed[0].provider, None);
    }

    #[test]
    fn pending_queue_completes_provider_panic() {
        let queue = PendingQueue::new();
        let callback: ManifestCodeCallback = Arc::new(|_, _, _| panic!("provider panic"));
        assert!(queue.queue_fetch(
            ManifestCodeFetch {
                job_id: 99,
                app_id: 480,
                depot_id: 481,
                gid: 1234,
                req_hdr_bytes: make_req_header(99, TARGET_JOB_NAME),
            },
            &script_only_manifest(),
            Some(callback),
            7,
        ));

        let completed = (0..100)
            .find_map(|_| {
                let completed = queue.drain_completed();
                if completed.is_empty() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    None
                } else {
                    Some(completed)
                }
            })
            .expect("provider panic did not complete");
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].job_id, 99);
        assert_eq!(completed[0].code, 0);
    }

    fn provider(
        name: &str,
        format: ManifestResponseFormat,
        json_field: Option<&str>,
    ) -> ManifestSource {
        ManifestSource {
            name: name.to_owned(),
            url: format!("https://{name}.fixture.invalid/{{gid}}"),
            user_agent: None,
            format,
            json_field: json_field.map(str::to_owned),
            min_interval_ms: None,
        }
    }

    #[test]
    fn parses_plain_provider_responses() {
        let plain = provider("plain", ManifestResponseFormat::Plain, None);
        assert_eq!(parse_provider_response(&plain, "123456").unwrap(), 123456);
        assert_eq!(parse_provider_response(&plain, " 654321 ").unwrap(), 654321);
        assert!(parse_provider_response(&plain, "42abc").is_err());
    }

    #[test]
    fn parses_json_provider_responses() {
        // A JSON endpoint may quote the code or leave it a number.
        let json = provider("json", ManifestResponseFormat::Json, Some("content"));
        assert_eq!(
            parse_provider_response(&json, r#"{"content":"9999999999"}"#).unwrap(),
            9_999_999_999
        );
        assert_eq!(
            parse_provider_response(&json, r#"{"content":777}"#).unwrap(),
            777
        );
        assert!(parse_provider_response(&json, r#"{"other":"1"}"#).is_err());
        assert!(parse_provider_response(&json, r#"{"content":true}"#).is_err());

        // Without a field name a JSON entry cannot be read at all.
        let fieldless = provider("fieldless", ManifestResponseFormat::Json, None);
        assert!(parse_provider_response(&fieldless, r#"{"content":1}"#).is_err());
    }

    #[test]
    fn malformed_provider_entries_are_rejected_before_any_request() {
        let no_placeholder = ManifestSource {
            name: "mirror".to_owned(),
            url: "https://my.mirror.fixture.invalid/latest".to_owned(),
            user_agent: None,
            format: ManifestResponseFormat::Plain,
            json_field: None,
            min_interval_ms: None,
        };

        // fetch_manifest_code refuses malformed entries before any request, so
        // the failure names the entry and — the point of the check — no HTTP
        // request is issued. Failover on or off, a malformed entry is not a
        // reason to dial anything.
        assert!(no_placeholder.rejection().is_some());
        let context = FetchContext {
            app_id: 730,
            depot_id: 731,
            gid: 1,
        };
        assert_eq!(
            fetch_manifest_code(
                context,
                std::slice::from_ref(&no_placeholder),
                false,
                10,
                10,
                0
            ),
            Err(ChainError::NoFailover {
                provider: "mirror".to_owned(),
                status: AttemptStatus::Rejected(no_placeholder.rejection().unwrap()),
            })
        );
        assert!(matches!(
            fetch_manifest_code(context, &[no_placeholder], true, 10, 10, 0),
            Err(ChainError::ChainFailed { gid: 1, .. })
        ));
    }

    #[test]
    fn resolves_every_placeholder_in_a_provider_url() {
        let pool = ManifestSource {
            name: "pool".to_owned(),
            // Valve's own GetManifestRequestCode shape: depot, then manifest.
            url: "https://fixture.invalid/manifest/{depotId}/{gid}".to_owned(),
            user_agent: None,
            format: ManifestResponseFormat::Plain,
            json_field: None,
            min_interval_ms: None,
        };
        assert_eq!(
            pool.resolve_url(730, 731, 555),
            "https://fixture.invalid/manifest/731/555"
        );
        assert!(pool.rejection().is_none());

        let with_app = ManifestSource {
            name: "withapp".to_owned(),
            url: "https://fixture.invalid/{appId}/{depotId}/{gid}".to_owned(),
            user_agent: None,
            format: ManifestResponseFormat::Plain,
            json_field: None,
            min_interval_ms: None,
        };
        assert_eq!(
            with_app.resolve_url(730, 731, 555),
            "https://fixture.invalid/730/731/555"
        );

        // The legacy `{depot_id}` spelling still expands.
        let legacy = ManifestSource {
            name: "legacy".to_owned(),
            url: "https://fixture.invalid/{depot_id}/{gid}".to_owned(),
            user_agent: None,
            format: ManifestResponseFormat::Plain,
            json_field: None,
            min_interval_ms: None,
        };
        assert_eq!(
            legacy.resolve_url(730, 731, 555),
            "https://fixture.invalid/731/555"
        );

        // A gid-only entry leaves the other placeholders inert.
        let gid_only = ManifestSource {
            name: "gidonly".to_owned(),
            url: "https://fixture.invalid/manifest/{gid}".to_owned(),
            user_agent: None,
            format: ManifestResponseFormat::Plain,
            json_field: None,
            min_interval_ms: None,
        };
        assert_eq!(
            gid_only.resolve_url(730, 731, 555),
            "https://fixture.invalid/manifest/555"
        );
    }

    #[test]
    fn a_table_entry_is_requested_with_its_url_as_written() {
        // Nothing resolves provider names any more: the entry's own url is
        // what goes on the wire, placeholders expanded as written.
        let state = ProviderState::default();
        let providers = vec![ManifestSource {
            name: "pool".to_owned(),
            url: "https://fixture.invalid/manifest/{depotId}/{gid}".to_owned(),
            user_agent: Some("FixtureClient/1.0".to_owned()),
            format: ManifestResponseFormat::Plain,
            json_field: None,
            min_interval_ms: None,
        }];

        let mut asked = Vec::new();
        let result = run_provider_chain(&state, context(555), &providers, false, |provider| {
            asked.push(provider.resolve_url(730, 731, 555));
            ProviderOutcome::Code(42)
        });

        assert_eq!(asked, ["https://fixture.invalid/manifest/731/555"]);
        assert_eq!(served_code(result), 42);
        assert_eq!(
            providers[0].user_agent(),
            Some("FixtureClient/1.0"),
            "the entry's own agent string reaches the request"
        );
    }

    #[test]
    fn denial_is_not_a_provider_fault() {
        // A pool that holds no licence for the app answers 401 per request.
        // Cooling it down would hide it from every later gid, including the
        // ones it can serve — so Denied must stay distinct from Failed.
        let denied = ProviderOutcome::Denied(401);
        let failed = ProviderOutcome::Failed("http status 503".to_owned());

        assert!(matches!(denied, ProviderOutcome::Denied(401)));
        assert!(matches!(failed, ProviderOutcome::Failed(_)));

        // The cooldown table is only ever written for the failure classes.
        let mut cooldowns = HashMap::new();
        let now = Instant::now();
        start_cooldown_in(&mut cooldowns, "unhealthy", now, FAILURE_COOLDOWN);
        assert_eq!(
            cooldown_remaining_in(&mut cooldowns, "unhealthy", now),
            Some(FAILURE_COOLDOWN)
        );
        assert_eq!(
            cooldown_remaining_in(&mut cooldowns, "denied-but-healthy", now),
            None
        );
    }

    #[test]
    fn pacing_defers_a_second_request_then_lets_it_through() {
        let mut last = HashMap::new();
        let now = Instant::now();
        let interval = Duration::from_millis(1000);

        // First request reserves the slot immediately.
        assert_eq!(reserve_slot_in(&mut last, "pool", now, interval), None);

        // A second one inside the window is told how long to wait, and does
        // not reserve — otherwise concurrent callers would all think they won.
        assert_eq!(
            reserve_slot_in(
                &mut last,
                "pool",
                now + Duration::from_millis(400),
                interval
            ),
            Some(Duration::from_millis(600))
        );

        // Another provider is unaffected.
        assert_eq!(reserve_slot_in(&mut last, "other", now, interval), None);

        // Past the window it goes through and re-reserves.
        assert_eq!(
            reserve_slot_in(
                &mut last,
                "pool",
                now + Duration::from_millis(1200),
                interval
            ),
            None
        );
        assert_eq!(
            reserve_slot_in(
                &mut last,
                "pool",
                now + Duration::from_millis(1300),
                interval
            ),
            Some(Duration::from_millis(900))
        );
    }

    #[test]
    fn parses_retry_after_delta_seconds() {
        assert_eq!(parse_retry_after(Some("30")), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after(Some(" 5 ")), Some(Duration::from_secs(5)));
        assert_eq!(parse_retry_after(Some("0")), Some(Duration::ZERO));
        // HTTP-date form is deliberately unsupported; caller substitutes the default.
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(Some("")), None);
        assert_eq!(parse_retry_after(None), None);
    }

    #[test]
    fn cooldown_hides_a_provider_until_it_expires() {
        let mut cooldowns = HashMap::new();
        let now = Instant::now();

        assert_eq!(cooldown_remaining_in(&mut cooldowns, "alpha", now), None);

        start_cooldown_in(&mut cooldowns, "alpha", now, Duration::from_secs(30));

        assert_eq!(
            cooldown_remaining_in(&mut cooldowns, "alpha", now),
            Some(Duration::from_secs(30))
        );
        // Other providers stay usable.
        assert_eq!(cooldown_remaining_in(&mut cooldowns, "beta", now), None);

        let later = now + Duration::from_secs(31);
        assert_eq!(cooldown_remaining_in(&mut cooldowns, "alpha", later), None);
        // The expired entry is dropped rather than left to accumulate.
        assert!(cooldowns.is_empty());
    }

    #[test]
    fn cooldown_is_capped() {
        let mut cooldowns = HashMap::new();
        let now = Instant::now();

        let applied = start_cooldown_in(&mut cooldowns, "alpha", now, Duration::from_secs(86_400));

        assert_eq!(applied, MAX_RATE_LIMIT_COOLDOWN);
        assert_eq!(
            cooldown_remaining_in(&mut cooldowns, "alpha", now),
            Some(MAX_RATE_LIMIT_COOLDOWN)
        );
    }

    fn chain(names: &[&str]) -> Vec<ManifestSource> {
        names
            .iter()
            .map(|name| provider(name, ManifestResponseFormat::Plain, None))
            .collect()
    }

    fn context(gid: u64) -> FetchContext {
        FetchContext {
            app_id: 10,
            depot_id: 11,
            gid,
        }
    }

    /// Run the chain with scripted outcomes and return its result plus the
    /// order in which providers were actually asked.
    fn run_scripted(
        state: &ProviderState,
        gid: u64,
        providers: &[ManifestSource],
        failover: bool,
        outcome: impl Fn(&str) -> ProviderOutcome,
    ) -> (Result<ChainSuccess, ChainError>, Vec<String>) {
        let mut asked = Vec::new();
        let result = run_provider_chain(state, context(gid), providers, failover, |provider| {
            asked.push(provider.name().to_owned());
            outcome(provider.name())
        });
        (result, asked)
    }

    fn served_code(result: Result<ChainSuccess, ChainError>) -> u64 {
        result.expect("the chain should have served a code").code
    }

    #[test]
    fn a_rerequested_gid_goes_to_other_providers_first() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b", "c"]);
        let serve = |_: &str| ProviderOutcome::Code(7);

        assert_eq!(run_scripted(&state, 1, &providers, true, serve).1, ["a"]);
        assert_eq!(run_scripted(&state, 1, &providers, true, serve).1, ["b"]);
        assert_eq!(run_scripted(&state, 1, &providers, true, serve).1, ["c"]);
        // Every provider has served it: the least recent one comes back first.
        assert_eq!(run_scripted(&state, 1, &providers, true, serve).1, ["a"]);
        // Another gid keeps the configured order.
        assert_eq!(run_scripted(&state, 2, &providers, true, serve).1, ["a"]);
    }

    #[test]
    fn a_rerequested_gid_still_reaches_the_previous_provider() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let _ = run_scripted(&state, 1, &providers, true, |_| ProviderOutcome::Code(7));
        let (result, asked) = run_scripted(&state, 1, &providers, true, |name| match name {
            "a" => ProviderOutcome::Code(8),
            _ => ProviderOutcome::Denied(401),
        });

        assert_eq!(served_code(result), 8);
        assert_eq!(asked, ["b", "a"]);
    }

    #[test]
    fn served_entries_expire_after_the_rerequest_window() {
        let mut served = HashMap::new();
        let now = Instant::now();
        record_served_in(&mut served, 1, "a", now);
        record_served_in(&mut served, 1, "b", now + Duration::from_secs(1));
        record_served_in(&mut served, 1, "a", now + Duration::from_secs(2));

        assert_eq!(served_by_in(&mut served, 1, now), ["b", "a"]);
        assert!(served_by_in(
            &mut served,
            1,
            now + REREQUEST_WINDOW + Duration::from_secs(2)
        )
        .is_empty());
        assert!(served.is_empty());
    }

    #[test]
    fn a_failure_is_backed_off_once_another_provider_serves_the_gid() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let (result, _) = run_scripted(&state, 1, &providers, true, |name| match name {
            "a" => ProviderOutcome::Failed("http status 503".to_owned()),
            _ => ProviderOutcome::Code(7),
        });

        assert_eq!(served_code(result), 7);
        assert!(state.cooldown_remaining("a").is_some());
        assert!(state.cooldown_remaining("b").is_none());
        assert_eq!(
            run_scripted(&state, 2, &providers, true, |_| ProviderOutcome::Code(9)).1,
            ["b"]
        );
    }

    #[test]
    fn a_gid_no_provider_serves_backs_nobody_off() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let (result, asked) = run_scripted(&state, 1, &providers, true, |_| {
            ProviderOutcome::Failed("http status 502".to_owned())
        });

        assert!(matches!(
            result,
            Err(ChainError::ChainFailed { gid: 1, .. })
        ));
        assert_eq!(asked, ["a", "b"]);
        assert!(state.cooldown_remaining("a").is_none());
        assert!(state.cooldown_remaining("b").is_none());
        // The next gid of the same update is still asked everywhere.
        assert_eq!(
            run_scripted(&state, 2, &providers, true, |_| ProviderOutcome::Code(9)),
            (
                Ok(ChainSuccess {
                    code: 9,
                    provider: "a".to_owned(),
                    attempts: vec![ProviderAttempt {
                        provider: "a".to_owned(),
                        status: AttemptStatus::Served,
                    }],
                }),
                vec!["a".to_owned()]
            )
        );
    }

    #[test]
    fn a_rate_limit_is_backed_off_even_when_the_chain_fails() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let _ = run_scripted(&state, 1, &providers, true, |name| match name {
            "a" => ProviderOutcome::RateLimited(Duration::from_secs(30)),
            _ => ProviderOutcome::Failed("timeout".to_owned()),
        });

        assert!(state.cooldown_remaining("a").is_some());
        assert!(state.cooldown_remaining("b").is_none());
    }

    // -----------------------------------------------------------------------
    // Failover policy
    // -----------------------------------------------------------------------

    #[test]
    fn failover_off_asks_one_provider_and_names_it_on_failure() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b", "c"]);

        let (result, asked) = run_scripted(&state, 1, &providers, false, |_| {
            ProviderOutcome::Failed("http status 503".to_owned())
        });

        // One attempt only: the rest of the list is never contacted, so the
        // user's lookup cannot reach a host they did not choose.
        assert_eq!(asked, ["a"]);
        let error = result.expect_err("failover off means a failure is returned");
        assert_eq!(
            error,
            ChainError::NoFailover {
                provider: "a".to_owned(),
                status: AttemptStatus::Failed("http status 503".to_owned()),
            }
        );
        let message = error.to_string();
        assert!(message.contains("`a`"), "{message}");
        assert!(message.contains("failover is off"), "{message}");
        // A lone provider is not cooled down on a failed lookup: the gid is the
        // likelier cause, and cooling it would fail the rest of the update.
        assert!(state.cooldown_remaining("a").is_none());
    }

    #[test]
    fn failover_off_stops_at_the_first_success() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let (result, asked) = run_scripted(&state, 1, &providers, false, |_| {
            ProviderOutcome::Code(1234)
        });

        let success = result.expect("the first provider serves");
        assert_eq!(success.code, 1234);
        assert_eq!(success.provider, "a");
        assert_eq!(asked, ["a"]);
        assert_eq!(
            success.attempts,
            [ProviderAttempt {
                provider: "a".to_owned(),
                status: AttemptStatus::Served,
            }]
        );
    }

    #[test]
    fn failover_off_does_not_contact_a_cooling_down_provider() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);
        state.start_cooldown("a", Duration::from_secs(30));

        let (result, asked) =
            run_scripted(&state, 1, &providers, false, |_| ProviderOutcome::Code(7));

        assert!(asked.is_empty());
        assert!(matches!(
            result,
            Err(ChainError::NoFailover {
                status: AttemptStatus::CoolingDown { .. },
                ..
            })
        ));
    }

    #[test]
    fn failover_on_follows_the_configured_order_and_keeps_every_attempt() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b", "c"]);

        let (result, asked) = run_scripted(&state, 1, &providers, true, |name| match name {
            "a" => ProviderOutcome::Failed("http status 503".to_owned()),
            "b" => ProviderOutcome::Denied(401),
            _ => ProviderOutcome::Code(42),
        });

        // Order respected, first success wins, and every attempt is in the
        // result the caller gets — the failed one, the refusal, and the serve.
        let success = result.expect("failover walks to the provider that serves");
        assert_eq!(asked, ["a", "b", "c"]);
        assert_eq!(success.code, 42);
        assert_eq!(success.provider, "c");
        assert_eq!(
            success.attempts,
            [
                ProviderAttempt {
                    provider: "a".to_owned(),
                    status: AttemptStatus::Failed("http status 503".to_owned()),
                },
                ProviderAttempt {
                    provider: "b".to_owned(),
                    status: AttemptStatus::Denied(401),
                },
                ProviderAttempt {
                    provider: "c".to_owned(),
                    status: AttemptStatus::Served,
                },
            ]
        );
    }

    #[test]
    fn failover_on_stops_asking_after_the_first_success() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let (result, asked) =
            run_scripted(&state, 1, &providers, true, |_| ProviderOutcome::Code(5));

        assert_eq!(asked, ["a"]);
        assert_eq!(served_code(result), 5);
    }

    #[test]
    fn failover_on_reports_a_chain_that_served_nothing() {
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);

        let (result, _) = run_scripted(&state, 7, &providers, true, |_| {
            ProviderOutcome::Denied(404)
        });

        let error = result.expect_err("nobody served gid 7");
        assert!(matches!(error, ChainError::ChainFailed { gid: 7, .. }));
        let message = error.to_string();
        assert!(message.contains("a has no code"), "{message}");
        assert!(message.contains("b has no code"), "{message}");
    }

    #[test]
    fn an_empty_provider_list_is_reported_not_attempted() {
        let state = ProviderState::default();
        let (result, asked) = run_scripted(&state, 1, &[], true, |_| ProviderOutcome::Code(1));

        assert!(asked.is_empty());
        assert_eq!(result, Err(ChainError::NoProviderConfigured));
        // The reason is actionable and distinguishes "nothing configured" from
        // "everything failed".
        let message = ChainError::NoProviderConfigured.to_string();
        assert!(
            message.contains("no manifest provider configured"),
            "{message}"
        );
        assert!(message.contains("host tool"), "{message}");

        // The real entry point stops before any request is built or sent, with
        // failover on or off.
        for failover in [false, true] {
            assert_eq!(
                fetch_manifest_code(context(1), &[], failover, 10, 10, 0),
                Err(ChainError::NoProviderConfigured)
            );
        }
    }

    #[test]
    fn the_trail_keeps_a_rate_limited_first_attempt() {
        // A provider that was rate limited is still part of the trail, with the
        // cooldown it cost, so the run can be explained after the fact.
        let state = ProviderState::default();
        let providers = chain(&["a", "b"]);
        let (result, _) = run_scripted(&state, 1, &providers, true, |name| match name {
            "a" => ProviderOutcome::RateLimited(Duration::from_secs(5)),
            _ => ProviderOutcome::Code(99),
        });

        let success = result.expect("the second provider serves");
        assert_eq!(success.provider, "b");
        // The rate-limited entry is in the trail too, with its cooldown.
        assert_eq!(
            success.attempts.first(),
            Some(&ProviderAttempt {
                provider: "a".to_owned(),
                status: AttemptStatus::RateLimited { cooldown_ms: 5_000 },
            })
        );
    }
}
