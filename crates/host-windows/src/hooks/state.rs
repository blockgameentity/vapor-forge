//! What a hook body is allowed to read, published once during startup.
//!
//! The bodies run on Steam's threads and must be callable before the host has
//! finished starting: [`get`] returning `None` is the "not initialised" answer
//! and every body then falls through to the original. Publication happens before
//! the first entry patch is enabled (see [`crate::hooks::installer`]), so in
//! practice a body either sees the whole state or the detour is not installed.
//!
//! The gate is behind a lock and tightened after the install pass: the state is
//! published with every capability enabled so the window between "first entry
//! patched" and "install results known" behaves like a normal run, and the
//! capabilities whose symbols did not resolve are disabled immediately
//! afterwards.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use vapor_forge_config::{AppId, DepotId, RuntimeConfig};
use vapor_forge_features::family::FamilySettings;
use vapor_forge_features::package::PackageState;
use vapor_forge_features::ticket::authorisation::{self, AuthorisationWindow};
use vapor_forge_features::ticket::TicketCache;

use crate::abi::netpacket::Layout;
use crate::capability::{Capability, Gate};
use crate::features::achievements::AchievementSettings;
use crate::features::{CloudPlan, IdentitySettings};
use crate::hooks::achievements::cache::SchemaCache as AchievementSchemaCache;
use crate::hooks::achievements::geometry::JobReply;
use crate::hooks::depot_store::DepotPlan;
use crate::hooks::netpacket::cache::CodeCache as RequestCodeCache;
use crate::hooks::netpacket::ledger::Ledger as NetPacketLedger;

/// Everything a hook body may consult. Immutable except for the gate and the
/// interior state of the feature objects.
pub struct HostState {
    config: RuntimeConfig,
    gate: RwLock<Gate>,
    package: PackageState,
    family: FamilySettings,
    /// Apps the effective config controls, DLC ids included, computed once.
    controlled: Vec<AppId>,
    ticket_cache: TicketCache,
    /// Lua-provided tickets, from `<steam>/config/lua`.
    lua_tickets: HashMap<AppId, Vec<u8>>,
    /// The Denuvo authorisation lease, acquired once when the host starts.
    ///
    /// `[ticket] denuvo_mode = "off"` (the default) means there is no lease at
    /// all, which is the historical one-shot behaviour. Checking one acquired
    /// window - rather than acquiring a fresh one per request - is what makes
    /// expiry observable.
    authorisation: Option<AuthorisationWindow>,
    depot: DepotPlan,
    /// The cloud route plan, resolved once at start-up.
    ///
    /// Empty by default ([`CloudPlan::default`] leaves every app alone), so a
    /// body reached before the worker attaches one sees "no redirection" rather
    /// than a placeholder endpoint.
    cloud: CloudPlan,
    /// Lua-registered depot decryption keys, from `<steam>/config/lua`.
    ///
    /// The same map the Linux host feeds to
    /// `features::depot_key::should_skip_shader_depot`; a depot with no entry is
    /// keyless, which is exactly what the shader decision has to know.
    depot_keys: HashMap<DepotId, Vec<u8>>,
    /// The per-game SteamID overrides, config and script state merged.
    identity: IdentitySettings,
    /// The resolved `[achievements]` settings, which is the flag the stats-merge
    /// hook consults.
    ///
    /// The worker resolves it from `config_apply` (`config.achievements.local_rebuild`)
    /// and attaches it here, so a hook body reads the value the feature pass
    /// applied rather than a second copy of the config.
    achievements: AchievementSettings,
    /// What the CM dispatch hook saw, bounded and read-only once recorded.
    ///
    /// A hook body runs on Steam's networking thread, so the record is a fixed
    /// ring behind one mutex rather than anything that grows with traffic; see
    /// [`crate::hooks::netpacket::ledger`].
    netpacket: NetPacketLedger,
    /// The decoded `CNetPacket` field layout.
    ///
    /// Written once, after the install pass has resolved the three corpus
    /// symbols — the state itself is published before any entry is patched, so
    /// this is interior state like the gate. `None` means "no layout for this
    /// build", which is what a message records as
    /// [`crate::hooks::netpacket::decision::ServeBlocker::NoLayout`].
    layout: RwLock<Option<Layout>>,
    /// The manifest request codes this process holds, keyed by `(depot, gid)`.
    ///
    /// The dispatch body reads it on Steam's networking thread, so it is a bounded
    /// table behind one mutex and never dials: a provider fetch belongs on a
    /// worker, and this host has none yet (see
    /// [`crate::hooks::netpacket::cache`]).
    request_codes: RequestCodeCache,
    /// The stats-reply geometry the corpus decoder resolved, if it has landed.
    ///
    /// Written once per module after the install batch, like [`Self::layout`],
    /// and read by the stats-merge body to find `*(this + reply_field) + body`.
    reply: RwLock<Option<JobReply>>,
    /// The stats schemas replies have carried, per app.
    ///
    /// The local achievement rebuild answers a reply that carries *no* schema, so
    /// the definitions come from a schema observed earlier; the same table exists on
    /// the Linux host (`client::achievement_local`). Bounded and lock-released on
    /// every access (see [`crate::hooks::achievements::cache`]).
    achievement_schemas: AchievementSchemaCache,
}

impl HostState {
    pub fn new(
        config: RuntimeConfig,
        family: FamilySettings,
        controlled: Vec<AppId>,
        ticket_dir: Option<PathBuf>,
        depot: DepotPlan,
    ) -> Self {
        let authorisation = authorisation::from_config(&config.ticket, started_at());
        Self {
            config,
            gate: RwLock::new(Gate::all_enabled()),
            package: PackageState::new(),
            family,
            controlled,
            ticket_cache: TicketCache::new(ticket_dir),
            lua_tickets: HashMap::new(),
            authorisation,
            depot,
            cloud: CloudPlan::default(),
            depot_keys: HashMap::new(),
            identity: IdentitySettings::default(),
            achievements: AchievementSettings::default(),
            netpacket: NetPacketLedger::new(),
            layout: RwLock::new(None),
            request_codes: RequestCodeCache::new(),
            reply: RwLock::new(None),
            achievement_schemas: AchievementSchemaCache::new(),
        }
    }

    /// Publish the decoded packet layout. Idempotent: the first decode wins, so
    /// a second module's pass cannot replace a layout a hook already read.
    pub fn set_layout(&self, layout: Layout) {
        let mut current = self.layout.write().unwrap_or_else(|error| error.into_inner());
        if current.is_none() {
            *current = Some(layout);
        }
    }

    /// The packet layout this process decoded, if any.
    pub fn layout(&self) -> Option<Layout> {
        *self.layout.read().unwrap_or_else(|error| error.into_inner())
    }

    /// The request codes this process holds.
    pub fn request_codes(&self) -> &RequestCodeCache {
        &self.request_codes
    }

    /// Publish the decoded stats-reply geometry. First decode wins, like
    /// [`Self::set_layout`].
    pub fn set_reply_geometry(&self, reply: JobReply) {
        let mut current = self.reply.write().unwrap_or_else(|error| error.into_inner());
        if current.is_none() {
            *current = Some(reply);
        }
    }

    /// The stats-reply geometry, when a decoder resolved it for this build.
    pub fn reply_geometry(&self) -> Option<JobReply> {
        *self.reply.read().unwrap_or_else(|error| error.into_inner())
    }

    /// The stats schemas replies have carried, per app.
    pub fn achievement_schemas(&self) -> &AchievementSchemaCache {
        &self.achievement_schemas
    }

    /// Attach the tickets the Lua scripts declared.
    ///
    /// The worker resolves the script state once, before any hook can run, so
    /// this is a constructor-companion rather than a mutable global.
    #[must_use]
    pub fn with_lua_tickets(mut self, tickets: HashMap<AppId, Vec<u8>>) -> Self {
        self.lua_tickets = tickets;
        self
    }

    /// Replace the authorisation lease. Tests use it to check an exact instant
    /// without waiting; the host only ever sets the one time-derived lease.
    #[must_use]
    pub fn with_authorisation(mut self, authorisation: Option<AuthorisationWindow>) -> Self {
        self.authorisation = authorisation;
        self
    }

    /// Attach the resolved cloud plan.
    ///
    /// The plan is what `features::cloud` decided from the settings the worker
    /// resolved, so the cloud body consults one decision, not a second opinion.
    #[must_use]
    pub fn with_cloud_plan(mut self, cloud: CloudPlan) -> Self {
        self.cloud = cloud;
        self
    }

    /// Attach the Lua-registered depot keys.
    #[must_use]
    pub fn with_depot_keys(mut self, depot_keys: HashMap<DepotId, Vec<u8>>) -> Self {
        self.depot_keys = depot_keys;
        self
    }

    /// Attach the resolved per-game SteamID overrides.
    #[must_use]
    pub fn with_identity_settings(mut self, identity: IdentitySettings) -> Self {
        self.identity = identity;
        self
    }

    /// Attach the resolved `[achievements]` settings.
    ///
    /// Without this the body sees [`AchievementSettings::default`], i.e. the
    /// rebuild off, which is the same behaviour as a user who never enabled it.
    #[must_use]
    pub fn with_achievement_settings(mut self, achievements: AchievementSettings) -> Self {
        self.achievements = achievements;
        self
    }

    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn package(&self) -> &PackageState {
        &self.package
    }

    pub fn family_settings(&self) -> FamilySettings {
        self.family
    }

    /// The current account's Steam account id, or 0 when it is not known.
    ///
    /// The Windows host has no measured way to read it yet, so it is always 0.
    /// `features::family::provenance::lender_account` treats 0 as "unknown", not
    /// as a lender, so a family label is never invented from it.
    pub fn self_account_id(&self) -> u32 {
        0
    }

    pub fn controlled_apps(&self) -> &[AppId] {
        &self.controlled
    }

    pub fn ticket_cache(&self) -> &TicketCache {
        &self.ticket_cache
    }

    pub fn lua_tickets(&self) -> &HashMap<AppId, Vec<u8>> {
        &self.lua_tickets
    }

    pub fn depot(&self) -> &DepotPlan {
        &self.depot
    }

    /// The cloud plan resolved at start-up.
    pub fn cloud(&self) -> &CloudPlan {
        &self.cloud
    }

    /// The Lua-registered depot decryption keys.
    pub fn depot_keys(&self) -> &HashMap<DepotId, Vec<u8>> {
        &self.depot_keys
    }

    /// The resolved per-game SteamID overrides.
    pub fn identity_settings(&self) -> &IdentitySettings {
        &self.identity
    }

    /// The resolved `[achievements]` settings.
    pub fn achievement_settings(&self) -> &AchievementSettings {
        &self.achievements
    }

    /// What the CM dispatch hook recorded, for diagnostics.
    pub fn netpacket(&self) -> &NetPacketLedger {
        &self.netpacket
    }

    /// The Denuvo lease this process holds, if `[ticket] denuvo_mode` asked for
    /// one.
    pub fn authorisation(&self) -> Option<AuthorisationWindow> {
        self.authorisation
    }

    /// Whether `capability` survived the install pass.
    pub fn allows(&self, capability: Capability) -> bool {
        self.gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_enabled(capability)
    }

    /// Snapshot of the gate, for the one feature pass that runs after install.
    pub fn gate(&self) -> Gate {
        self.gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The install pass's reason, when `capability` was disabled by it.
    pub fn disabled_reason(&self, capability: Capability) -> Option<String> {
        self.gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reason(capability)
            .map(str::to_owned)
    }

    /// Replace the gate with the install pass's result.
    pub fn set_gate(&self, gate: Gate) {
        *self
            .gate
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = gate;
    }
}

static STATE: OnceLock<Arc<HostState>> = OnceLock::new();

/// Seconds since the Unix epoch, saturating at 0 for a clock set before it.
fn started_at() -> Duration {
    Duration::from_secs(u64::try_from(vapor_forge_core::unix_now()).unwrap_or(0))
}

/// The published state, or `None` before the host finished starting.
pub fn get() -> Option<&'static HostState> {
    STATE.get().map(Arc::as_ref)
}

/// Publish `state` for the hook bodies.
///
/// `Err` means this process already published one: there is exactly one host
/// state per process, and a second publication would silently swap the config a
/// live hook is reading.
pub fn publish(state: Arc<HostState>) -> Result<(), String> {
    STATE
        .set(state)
        .map_err(|_| "the host state was already published in this process".to_owned())
}
