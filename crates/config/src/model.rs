use serde::de::{MapAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use crate::{AppId, ConfigError};

/// Root runtime configuration. Lua scripts extend the app-specific state.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub runtime: RuntimeSection,
    #[serde(default)]
    pub toast: ToastSection,
    #[serde(default)]
    pub debug: DebugSection,
    #[serde(default)]
    pub apps: AppsSection,
    #[serde(default)]
    pub cloud: CloudSection,
    #[serde(default)]
    pub scripting: ScriptingSection,
    #[serde(default)]
    pub ticket: TicketSection,
    #[serde(default)]
    pub manifest: ManifestSection,
    #[serde(default)]
    pub achievements: AchievementsSection,
    #[serde(default)]
    pub app_avatar: AppAvatarSection,
    #[serde(default)]
    pub steam_id: SteamIdSection,
    #[serde(default)]
    pub library_inject: LibraryInjectSection,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub diagnostics: bool,
    #[serde(default = "default_patterns_url")]
    pub patterns_url: String,
}

/// Steam internal toast notifications shown through SteamUI WebUI.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToastSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub init: bool,
}

/// Development-only local control API.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugSection {
    #[serde(default = "default_debug_control_api")]
    pub control_api: bool,
}

/// Controlled apps.
///
/// - `inject`: apps the user does NOT own. Full ownership + optional DLC.
/// - `shared`: family sharing concurrent-play unlock.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppsSection {
    #[serde(default)]
    inject: Vec<InjectApp>,
    #[serde(default)]
    pub shared: SharedSection,
    #[serde(skip)]
    app_index: OnceLock<HashMap<AppId, AppCategory>>,
}

/// An app to inject ownership for, with optional DLC list.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InjectApp {
    pub id: AppId,
    #[serde(default)]
    pub dlc: Vec<AppId>,
    #[serde(default)]
    pub ticket: TicketMode,
    /// Unix timestamp shown as purchase date in the Steam library UI.
    /// Script-provided apps use the Lua source mtime when this is zero.
    #[serde(default)]
    pub purchase_time: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TicketMode {
    /// Default: derive tickets from the source app's ticket with current user's SteamID.
    #[default]
    Forge,
    /// Delegate to cached ticket from previous owner session. During the
    /// first few ticket requests after launch, return the cached ticket
    /// (with the original owner's SteamID). After the window closes,
    /// switch to derived mode with the current user's SteamID.
    Delegate,
}

/// Family sharing concurrent-play unlock.
///
/// Enabled by default for ALL family-shared apps.
/// - Set `include` to unlock ONLY listed apps (whitelist).
/// - Set `exclude` to unlock all EXCEPT listed apps (blacklist).
/// - `include` and `exclude` are mutually exclusive; if both are set, `include` takes precedence.
/// - Neither set = all family-shared apps are unlocked.
/// - `enabled = false` disables entirely.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub include: Vec<AppId>,
    #[serde(default)]
    pub exclude: Vec<AppId>,
    /// Re-validate a claimed lender before trusting a family label.
    ///
    /// Without this, a licence we fabricated ourselves reads back as borrowed
    /// and the library says "from family library". On by default: it is the
    /// correct behaviour, and turning it off restores the pre-fix label.
    #[serde(default = "default_true")]
    pub validate_lender: bool,
    /// Remove a borrower marker once an app is no longer borrowed.
    ///
    /// On by default; off leaves the marker in place, which is the pre-fix
    /// behaviour and can mislabel an ingested app.
    #[serde(default = "default_true")]
    pub clean_borrower_markers: bool,
}

impl SharedSection {
    pub fn allows(&self, app_id: AppId) -> bool {
        if !self.enabled {
            return false;
        }
        if !self.include.is_empty() {
            return self.include.contains(&app_id);
        }
        if !self.exclude.is_empty() {
            return !self.exclude.contains(&app_id);
        }
        true
    }
}

impl Default for SharedSection {
    fn default() -> Self {
        Self {
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            validate_lender: true,
            clean_borrower_markers: true,
        }
    }
}

/// Cloud backend selection and settings for controlled apps.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloudSection {
    #[serde(default)]
    pub backend: CloudBackendMode,
    #[serde(default)]
    pub local: LocalCloudSection,
    #[serde(default)]
    pub cumulus: CumulusCloudSection,
}

impl Default for CloudSection {
    fn default() -> Self {
        Self {
            backend: CloudBackendMode::Disabled,
            local: LocalCloudSection::default(),
            cumulus: CumulusCloudSection::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudBackendMode {
    #[default]
    Disabled,
    Local,
    Cumulus,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCloudSection {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub syncthing: SyncthingSection,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CumulusCloudSection {
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_timeout_connect_ms")]
    pub timeout_connect_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

impl std::fmt::Debug for CumulusCloudSection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CumulusCloudSection")
            .field("server_url", &self.server_url)
            .field("token", &"[REDACTED]")
            .field("timeout_connect_ms", &self.timeout_connect_ms)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl Default for CumulusCloudSection {
    fn default() -> Self {
        Self {
            server_url: String::new(),
            token: String::new(),
            timeout_connect_ms: default_timeout_connect_ms(),
            timeout_ms: default_timeout_ms(),
        }
    }
}

/// Optional Syncthing integration for the local cloud repository.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncthingSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_syncthing_url")]
    pub url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub folder_id: String,
    #[serde(default = "default_syncthing_timeout_ms")]
    pub timeout_ms: u64,
}

impl std::fmt::Debug for SyncthingSection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SyncthingSection")
            .field("enabled", &self.enabled)
            .field("url", &self.url)
            .field("api_key", &"[REDACTED]")
            .field("folder_id", &self.folder_id)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

impl Default for SyncthingSection {
    fn default() -> Self {
        Self {
            enabled: false,
            url: default_syncthing_url(),
            api_key: String::new(),
            folder_id: String::new(),
            timeout_ms: default_syncthing_timeout_ms(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptingSection {
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Achievement/stats schema configuration.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AchievementsSection {
    #[serde(default)]
    pub offline_schema: bool,
    /// Rebuild achievement replies from the local definition set when the schema
    /// cache is absent or unreadable.
    ///
    /// Off by default: with the flag off the stats path behaves exactly as it
    /// did before the rebuild existed (no stealth behaviour change).
    #[serde(default)]
    pub local_rebuild: bool,
}

/// AppAvatar: map a real AppId to another for networking.
///
/// Integer keys are static mappings. Use 0 as wildcard for all unowned apps.
/// `rules` is an array of flag-driven rules evaluated at SpawnProcess.
///
/// ```toml
/// [app_avatar]
/// 480 = 730
/// 0 = 730
///
/// [[app_avatar.rules]]
/// flag = "-onlinefix"
/// avatar = 480
/// ```
#[derive(Clone, Debug, Default)]
pub struct AppAvatarSection {
    pub static_map: HashMap<AppId, AppId>,
    pub rules: Vec<AppAvatarRule>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppAvatarRule {
    pub flag: String,
    pub avatar: AppId,
    #[serde(default)]
    pub apps: Vec<AppId>,
    #[serde(default)]
    pub exclude: Vec<AppId>,
}

impl<'de> serde::Deserialize<'de> for AppAvatarSection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};
        use std::fmt;

        struct AppAvatarVisitor;

        impl<'de> Visitor<'de> for AppAvatarVisitor {
            type Value = AppAvatarSection;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("app_avatar table with integer keys and optional rules array")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut static_map = HashMap::new();
                let mut rules = Vec::new();

                while let Some(key) = map.next_key::<String>()? {
                    if key == "rules" {
                        rules = map.next_value()?;
                    } else if let Ok(app_id) = key.parse::<u32>() {
                        let avatar: u32 = map.next_value()?;
                        static_map.insert(AppId(app_id), AppId(avatar));
                    } else {
                        return Err(serde::de::Error::unknown_field(
                            &key,
                            &["rules", "an unsigned 32-bit AppID"],
                        ));
                    }
                }

                Ok(AppAvatarSection { static_map, rules })
            }
        }

        deserializer.deserialize_map(AppAvatarVisitor)
    }
}

/// Per-app SteamID reported by `ISteamUser::GetSteamID`.
///
/// Keys are AppIDs and values are individual SteamID64s. Only the listed
/// app sees the replacement; Steam and other apps keep the real account.
/// Executables wrapped by Steam DRM or Denuvo compare the ownership ticket
/// with this SteamID and refuse to start.
///
/// ```toml
/// [steam_id]
/// 480 = 76561202255233023
/// ```
#[derive(Clone, Debug, Default)]
pub struct SteamIdSection {
    overrides: HashMap<AppId, u64>,
}

impl SteamIdSection {
    pub fn get(&self, app_id: AppId) -> Option<u64> {
        self.overrides.get(&app_id).copied()
    }
}

/// Individual account in a non-zero universe with a non-zero account number.
fn is_individual_steam_id(steam_id64: u64) -> bool {
    const ACCOUNT_TYPE_INDIVIDUAL: u64 = 1;
    let account_id = steam_id64 & 0xffff_ffff;
    let account_type = (steam_id64 >> 52) & 0xf;
    let universe = steam_id64 >> 56;
    account_id != 0 && account_type == ACCOUNT_TYPE_INDIVIDUAL && universe != 0
}

impl<'de> serde::Deserialize<'de> for SteamIdSection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use std::fmt;

        struct SteamIdVisitor;

        impl<'de> Visitor<'de> for SteamIdVisitor {
            type Value = SteamIdSection;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("steam_id table mapping AppIDs to SteamID64 values")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut overrides = HashMap::new();
                while let Some(key) = map.next_key::<String>()? {
                    let Some(app_id) = key.parse::<u32>().ok().filter(|id| *id != 0) else {
                        return Err(serde::de::Error::unknown_field(
                            &key,
                            &["a non-zero unsigned 32-bit AppID"],
                        ));
                    };
                    let steam_id64: u64 = map.next_value()?;
                    if !is_individual_steam_id(steam_id64) {
                        return Err(serde::de::Error::custom(format!(
                            "steam_id.{app_id}: {steam_id64} is not an individual SteamID64"
                        )));
                    }
                    overrides.insert(AppId(app_id), steam_id64);
                }
                Ok(SteamIdSection { overrides })
            }
        }

        deserializer.deserialize_map(SteamIdVisitor)
    }
}

/// Native .so injection via LD_PRELOAD, applied at BuildSpawnEnvBlock time.
///
/// Each entry lists a library path plus optional app/flag filters, mirroring
/// the AppAvatar rule shape. Rules are evaluated at SpawnProcess; matching paths
/// are joined and written into the child process env block.
///
/// ```toml
/// [[library_inject.libs]]
/// path = "/home/user/mylib.so"
/// flag = "-onlinefix"
/// apps = [480]
/// ```
///
/// Native `.so` entries are merged in front of the `LD_PRELOAD` value Steam
/// composes for the game (its overlay renderers), so they reach the game
/// process even though Steam writes that variable after the launch hooks ran.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryInjectSection {
    #[serde(default)]
    pub libs: Vec<LibraryInjectEntry>,
    #[serde(default)]
    pub helper_path: String,
    #[serde(default)]
    pub loader_fix: LoaderFixSection,
}

/// Loader fixes for third-party loaders, applied by the Proton helper.
///
/// The helper disables thread callouts on any extra loader entry that shares
/// a loaded module's name and entry point, as some third-party loaders register for
/// user32, which otherwise makes wine run that DllMain twice per thread attach
/// and exit. It also keeps NtSuspendThread from stopping a thread while that
/// thread holds Wine's PEB lock or loader lock, which deadlocks code that
/// suspends every other thread and then opens files or loads modules. Both are
/// only applied where asked for:
///
/// ```toml
/// [library_inject.loader_fix]
/// apps = [812140]      # always on for these apps
/// exclude = []         # never, even when flagged
/// flag = "-loaderfix"  # on for any app whose launch options carry the flag
/// ```
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoaderFixSection {
    #[serde(default)]
    pub apps: Vec<AppId>,
    #[serde(default)]
    pub exclude: Vec<AppId>,
    #[serde(default)]
    pub flag: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryInjectEntry {
    pub path: String,
    #[serde(default)]
    pub flag: String,
    #[serde(default)]
    pub apps: Vec<AppId>,
    #[serde(default)]
    pub exclude: Vec<AppId>,
}

/// Ticket configuration.
///
/// - Forge source tickets: always memory-only.
/// - Delegate captured tickets: always persisted to disk.
/// - Other tickets (Lua-provided, real intercepted): controlled by `cache`.
#[derive(Clone, Debug, Deserialize)]
#[serde(try_from = "TicketSectionRaw")]
pub struct TicketSection {
    /// Persistence for non-delegate, non-derived tickets (Lua-provided or
    /// intercepted real tickets). Default: disk.
    pub cache: TicketCacheMode,
    /// Automatically detect Denuvo-protected games (via PE section scanning
    /// in the vapor-forge-proton-inject helper) and enable delegate ticket mode for them.
    /// Requires library injection with a proton helper configured.
    pub auto_delegate: bool,
    /// Ticket sources to ask, in order. The first source that has the ticket
    /// wins; the default is the single source this build has always used, so
    /// leaving the key alone keeps the previous behaviour exactly.
    pub source_order: Vec<TicketSourceKind>,
    /// How a Denuvo app is authorised. `off` (default) keeps the historical
    /// one-shot ticket; `lease` gives the authorisation an explicit validity
    /// window that the caller has to renew.
    pub denuvo_mode: DenuvoMode,
    /// Manual Denuvo override: `true` treats every app as Denuvo-protected,
    /// `false` as never, `None` (default) lets detection decide. It wins over
    /// detection in both directions.
    pub denuvo_override: Option<bool>,
    /// Enable structural Denuvo detection (section names, DLL names, flags).
    /// Off by default, so an app is only ever treated as Denuvo-protected when
    /// `denuvo_override` says so.
    pub denuvo_detection: bool,
    /// Validity of one lease in seconds when `denuvo_mode = "lease"`.
    pub denuvo_lease_secs: u64,
}

impl Default for TicketSection {
    fn default() -> Self {
        Self {
            cache: TicketCacheMode::default(),
            auto_delegate: false,
            source_order: default_ticket_source_order(),
            denuvo_mode: DenuvoMode::default(),
            denuvo_override: None,
            denuvo_detection: false,
            denuvo_lease_secs: default_denuvo_lease_secs(),
        }
    }
}

/// A ticket source, as named in `[ticket] source_order`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TicketSourceKind {
    /// The ticket this process already holds, or the one in the local ticket
    /// cache (memory first, then disk). This is the only source used when
    /// `source_order` is left at its default.
    LocalCache,
    /// A ticket kept in the platform credential store.
    CredentialStore,
    /// A ticket published in the Steam registry.
    Registry,
}

impl TicketSourceKind {
    pub const ALL: [Self; 3] = [Self::LocalCache, Self::CredentialStore, Self::Registry];

    /// Config spelling, and the name used in logs and errors.
    pub fn name(self) -> &'static str {
        match self {
            Self::LocalCache => "local_cache",
            Self::CredentialStore => "credential_store",
            Self::Registry => "registry",
        }
    }
}

impl std::fmt::Display for TicketSourceKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.name())
    }
}

/// How a Denuvo app is authorised.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DenuvoMode {
    /// Historical behaviour: one ticket, acquired when the app asks for it.
    #[default]
    Off,
    /// Lease/pulse authorisation: the authorisation has an acquisition time and
    /// an explicit expiry, and the caller renews it.
    Lease,
}

/// Wire form of [`TicketSection`]. Separate so the order can be validated while
/// the config is parsed: a repeated (or empty) source list is a load error, not
/// something the ticket path discovers later.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketSectionRaw {
    #[serde(default)]
    cache: TicketCacheMode,
    #[serde(default)]
    auto_delegate: bool,
    #[serde(default = "default_ticket_source_order")]
    source_order: Vec<TicketSourceKind>,
    #[serde(default)]
    denuvo_mode: DenuvoMode,
    #[serde(default)]
    denuvo_override: Option<bool>,
    #[serde(default)]
    denuvo_detection: bool,
    #[serde(default = "default_denuvo_lease_secs")]
    denuvo_lease_secs: u64,
}

impl TryFrom<TicketSectionRaw> for TicketSection {
    type Error = String;

    fn try_from(raw: TicketSectionRaw) -> Result<Self, String> {
        let section = Self {
            cache: raw.cache,
            auto_delegate: raw.auto_delegate,
            source_order: raw.source_order,
            denuvo_mode: raw.denuvo_mode,
            denuvo_override: raw.denuvo_override,
            denuvo_detection: raw.denuvo_detection,
            denuvo_lease_secs: raw.denuvo_lease_secs,
        };
        if let Some(problem) = section.source_order_problems().into_iter().next() {
            return Err(problem);
        }
        Ok(section)
    }
}

impl TicketSection {
    /// Problems with the configured source order, empty when it is usable.
    ///
    /// The order is a chain, so an entry that appears twice would be asked
    /// twice and an empty order would never answer at all. Both are reported
    /// against the config, with the offending source named.
    pub fn source_order_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.source_order.is_empty() {
            problems.push(
                "[ticket] source_order is empty: configure at least one ticket source".to_owned(),
            );
        }
        let mut seen = HashSet::new();
        for source in &self.source_order {
            if !seen.insert(*source) {
                problems.push(format!(
                    "[ticket] source_order lists `{source}` more than once: each ticket source may appear at most once"
                ));
            }
        }
        problems
    }
}

fn default_ticket_source_order() -> Vec<TicketSourceKind> {
    vec![TicketSourceKind::LocalCache]
}

fn default_denuvo_lease_secs() -> u64 {
    300
}

/// Persistence mode for non-delegate, non-derived tickets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TicketCacheMode {
    /// In-memory only. Lost when Steam restarts.
    Session,
    /// Persist to disk. Survives restarts.
    #[default]
    Disk,
}

/// Manifest request-code provider configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSection {
    /// Provider chain, in preference order.
    ///
    /// Written by the host tool (cheesetool); Vapor Forge resolves no provider
    /// names and ships no default list. An empty list means request codes are
    /// never fetched: the reason is reported and the request degrades instead.
    #[serde(default)]
    pub providers: Vec<ManifestSource>,
    /// Try the next entry of `providers` when one fails.
    ///
    /// Off by default, and that is the point: with failover off only the first
    /// entry is ever contacted, so a depot lookup cannot quietly travel to a
    /// host the user did not pick. When it is on, the chain is walked in order,
    /// every attempt is logged at `info`, and the provider that served the code
    /// is reported back to the caller.
    #[serde(default)]
    pub failover: bool,
    #[serde(default = "default_timeout_connect_ms")]
    pub timeout_connect_ms: u64,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Minimum gap between two requests to the same provider.
    ///
    /// Provider endpoints are typically small shared services that degrade for
    /// everyone when one client floods them, and Steam asks for one code per
    /// depot, so a big install would burst without this.
    #[serde(default = "default_min_interval_ms")]
    pub min_interval_ms: u64,
}

impl Default for ManifestSection {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            failover: false,
            timeout_connect_ms: default_timeout_connect_ms(),
            timeout_ms: default_timeout_ms(),
            min_interval_ms: default_min_interval_ms(),
        }
    }
}

/// How a provider's 200 response carries the request code.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestResponseFormat {
    /// Whole body is the decimal code.
    #[default]
    Plain,
    /// Code sits in a top-level JSON string or number field.
    Json,
}

/// One entry of the provider chain.
///
/// Vapor Forge ships no provider catalogue and resolves no provider names: the
/// host tool (cheesetool) writes this list into the config. An entry describes
/// its own endpoint, and `url` may use the `{gid}`, `{depotId}` (legacy
/// `{depot_id}`) and `{appId}` placeholders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestSource {
    /// Identifies the entry in logs and in the rate-limit cooldown table.
    pub name: String,
    /// Endpoint; must contain a `{gid}` placeholder.
    pub url: String,
    /// Agent string to send instead of the default one.
    pub user_agent: Option<String>,
    /// How a 200 response carries the code.
    pub format: ManifestResponseFormat,
    /// Required when `format = "json"`.
    pub json_field: Option<String>,
    /// Overrides `[manifest] min_interval_ms` for this provider.
    pub min_interval_ms: Option<u64>,
}

/// Placeholders a provider `url` may use. `{depot_id}` is the legacy spelling
/// of `{depotId}` and is still expanded for configs written before the rename.
const URL_PLACEHOLDERS: [&str; 4] = ["{gid}", "{depotId}", "{depot_id}", "{appId}"];

/// The first placeholder in `url` that no substitution covers, if any.
///
/// A placeholder nothing expands would be sent to the host verbatim, so it is
/// refused while the config is parsed rather than producing a nonsense request.
fn url_placeholder_problem(url: &str) -> Option<String> {
    let allowed = URL_PLACEHOLDERS.join(", ");
    let mut rest = url;
    while let Some(open) = rest.find('{') {
        let after = &rest[open..];
        let Some(close) = after.find('}') else {
            return Some(format!("unterminated url placeholder in `{url}`"));
        };
        let token = &after[..=close];
        if !URL_PLACEHOLDERS.contains(&token) {
            return Some(format!(
                "unknown url placeholder `{token}`; allowed placeholders: {allowed}"
            ));
        }
        rest = &after[close + 1..];
    }
    None
}

impl ManifestSource {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn url_template(&self) -> &str {
        &self.url
    }

    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    pub fn response_format(&self) -> ManifestResponseFormat {
        self.format
    }

    pub fn json_field(&self) -> Option<&str> {
        self.json_field.as_deref()
    }

    /// Minimum gap before the next request to this provider.
    pub fn min_interval(&self, section_default_ms: u64) -> std::time::Duration {
        let ms = self.min_interval_ms.unwrap_or(section_default_ms);
        std::time::Duration::from_millis(ms)
    }

    /// Expand the endpoint template for one request.
    ///
    /// `{gid}` is always substituted; `{depotId}` and `{appId}` let a provider
    /// that mints codes through licensed accounts take Valve's own
    /// GetManifestRequestCode shape (depot plus manifest). `{depot_id}` is the
    /// legacy spelling of `{depotId}` and is still expanded.
    pub fn resolve_url(&self, app_id: u32, depot_id: u32, gid: u64) -> String {
        self.url_template()
            .replace("{gid}", &gid.to_string())
            .replace("{depotId}", &depot_id.to_string())
            .replace("{depot_id}", &depot_id.to_string())
            .replace("{appId}", &app_id.to_string())
    }

    /// Why this entry cannot be used, or `None` when it is well-formed.
    ///
    /// An entry that parses but is unusable is skipped rather than failing the
    /// whole config: entries are written by the host tool, so one typo must not
    /// take the other sources down with it. Structural problems — a bare name,
    /// an unknown url placeholder — are refused at load instead, because they
    /// cannot be interpreted at all.
    pub fn rejection(&self) -> Option<String> {
        if self.name.trim().is_empty() {
            return Some("manifest provider has an empty name".to_owned());
        }
        if !self.url.contains("{gid}") {
            return Some(format!(
                "manifest provider `{}` has no {{gid}} placeholder in its url",
                self.name
            ));
        }
        if !self.url.starts_with("http://") && !self.url.starts_with("https://") {
            return Some(format!(
                "manifest provider `{}` must use an http:// or https:// url",
                self.name
            ));
        }
        if self.format == ManifestResponseFormat::Json
            && self.json_field.as_deref().unwrap_or("").trim().is_empty()
        {
            return Some(format!(
                "manifest provider `{}` sets format = \"json\" but no json_field",
                self.name
            ));
        }
        None
    }
}

/// The table form of one provider entry, exactly as TOML writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestSourceTable {
    name: String,
    url: String,
    #[serde(default)]
    user_agent: Option<String>,
    #[serde(default)]
    format: ManifestResponseFormat,
    #[serde(default)]
    json_field: Option<String>,
    #[serde(default)]
    min_interval_ms: Option<u64>,
}

impl From<ManifestSourceTable> for ManifestSource {
    fn from(table: ManifestSourceTable) -> Self {
        Self {
            name: table.name,
            url: table.url,
            user_agent: table.user_agent,
            format: table.format,
            json_field: table.json_field,
            min_interval_ms: table.min_interval_ms,
        }
    }
}

impl<'de> Deserialize<'de> for ManifestSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct SourceVisitor;

        impl<'de> Visitor<'de> for SourceVisitor {
            type Value = ManifestSource;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str(
                    "a [manifest] providers table with `name` and `url`, written by the host",
                )
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                // Bare names addressed a built-in catalogue this build no longer
                // ships, so the error says what to write instead of only
                // rejecting the value.
                Err(E::custom(format!(
                    "manifest provider `{value}` is a bare name, but providers are now tables \
                     written by the host tool (cheesetool) and Vapor Forge ships no built-in \
                     providers; write {{ name = \"{value}\", url = \"https://host.example/manifest/{{depotId}}/{{gid}}\" }}"
                )))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                let table = ManifestSourceTable::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                let source = ManifestSource::from(table);
                if let Some(problem) = url_placeholder_problem(&source.url) {
                    return Err(serde::de::Error::custom(format!(
                        "manifest provider `{}` has a {problem}",
                        source.name
                    )));
                }
                Ok(source)
            }
        }

        deserializer.deserialize_any(SourceVisitor)
    }
}

impl ManifestSection {
    /// Problems that make individual entries unusable, plus duplicate names.
    ///
    /// Names key the rate-limit cooldown table, so a duplicate would make two
    /// sources share one cooldown.
    pub fn rejections(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for source in &self.providers {
            if let Some(problem) = source.rejection() {
                problems.push(problem);
                continue;
            }
            if !seen.insert(source.name()) {
                problems.push(format!(
                    "duplicate manifest provider name `{}`",
                    source.name()
                ));
            }
        }
        problems
    }
}

fn default_timeout_connect_ms() -> u64 {
    5000
}

fn default_timeout_ms() -> u64 {
    15000
}

fn default_min_interval_ms() -> u64 {
    1000
}

fn default_syncthing_url() -> String {
    "http://127.0.0.1:8384".into()
}

fn default_syncthing_timeout_ms() -> u64 {
    2000
}

impl Default for RuntimeSection {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            diagnostics: false,
            patterns_url: default_patterns_url(),
        }
    }
}

impl Default for ToastSection {
    fn default() -> Self {
        Self {
            enabled: true,
            init: true,
        }
    }
}

impl Default for DebugSection {
    fn default() -> Self {
        Self {
            control_api: default_debug_control_api(),
        }
    }
}

/// Online pattern hotfixes come from this repository's own pattern sources, so
/// a Steam update can be answered by pushing a file instead of shipping a build.
/// `{arch}` selects the per-architecture file; ordinary and steamrt are already
/// covered inside it as `[[...variants]]`. An empty value disables the fetch.
fn default_patterns_url() -> String {
    "https://raw.githubusercontent.com/Hintay/vapor-forge/main/res/patterns/{arch}.toml".to_owned()
}

fn default_log_level() -> String {
    "info".to_owned()
}

fn default_true() -> bool {
    true
}

fn default_debug_control_api() -> bool {
    cfg!(debug_assertions)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppCategory {
    Inject,
    InjectDlc { parent: AppId },
}

impl AppsSection {
    pub fn with_inject(inject: Vec<InjectApp>) -> Self {
        let apps = Self {
            inject,
            ..Self::default()
        };
        apps.app_index();
        apps
    }

    pub fn inject(&self) -> &[InjectApp] {
        &self.inject
    }

    pub fn push_inject(&mut self, app: InjectApp) {
        if let Some(index) = self.app_index.get_mut() {
            index.reserve(1 + app.dlc.len());
            Self::index_app(index, &app);
        }
        self.inject.push(app);
    }

    pub fn extend_inject(&mut self, apps: Vec<InjectApp>) {
        if apps.is_empty() {
            return;
        }

        self.inject.reserve(apps.len());
        if let Some(index) = self.app_index.get_mut() {
            let additional_entries = apps.iter().map(|app| 1 + app.dlc.len()).sum();
            index.reserve(additional_entries);
            for app in &apps {
                Self::index_app(index, app);
            }
        }
        self.inject.extend(apps);
    }

    fn app_category(&self, app_id: AppId) -> Option<AppCategory> {
        self.app_index().get(&app_id).copied()
    }

    fn is_controlled_app(&self, app_id: AppId) -> bool {
        self.app_index().contains_key(&app_id)
    }

    fn app_index(&self) -> &HashMap<AppId, AppCategory> {
        self.app_index.get_or_init(|| {
            let entry_count = self.inject.iter().map(|app| 1 + app.dlc.len()).sum();
            let mut index = HashMap::with_capacity(entry_count);
            for app in &self.inject {
                Self::index_app(&mut index, app);
            }
            index
        })
    }

    fn index_app(index: &mut HashMap<AppId, AppCategory>, app: &InjectApp) {
        index.entry(app.id).or_insert(AppCategory::Inject);
        for &dlc in &app.dlc {
            index
                .entry(dlc)
                .or_insert(AppCategory::InjectDlc { parent: app.id });
        }
    }
}

impl RuntimeConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&text)?;
        config.apps.app_index();
        Ok(config)
    }

    pub fn app_category(&self, app_id: AppId) -> Option<AppCategory> {
        self.apps.app_category(app_id)
    }

    /// Returns whether this AppId is managed by the effective runtime config.
    ///
    /// Script-provided AppIds are merged into `apps.inject` before the runtime
    /// config is published, so this remains the single source of truth for
    /// both configured and scripted apps.
    pub fn is_controlled_app(&self, app_id: AppId) -> bool {
        self.apps.is_controlled_app(app_id)
    }

    pub fn purchase_time(&self, app_id: AppId) -> u32 {
        self.apps
            .inject()
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.purchase_time)
            .unwrap_or(0)
    }

    pub fn ticket_mode(&self, app_id: AppId) -> TicketMode {
        self.apps
            .inject()
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.ticket)
            .unwrap_or(TicketMode::Forge)
    }

    pub fn should_bypass_sharing(&self, app_id: AppId) -> bool {
        self.apps.shared.allows(app_id)
    }

    pub fn inject_app_ids(&self) -> HashSet<AppId> {
        let mut ids = HashSet::new();
        for app in self.apps.inject() {
            ids.insert(app.id);
            ids.extend(&app.dlc);
        }
        ids
    }

    pub fn inject_dlc_map(&self) -> HashMap<AppId, Vec<AppId>> {
        self.apps
            .inject()
            .iter()
            .filter(|app| !app.dlc.is_empty())
            .map(|app| (app.id, app.dlc.clone()))
            .collect()
    }

    pub fn has_any_inject_apps(&self) -> bool {
        !self.apps.inject().is_empty()
    }

    pub fn cloud_enabled_for_controlled_apps(&self) -> bool {
        self.cloud.backend != CloudBackendMode::Disabled
    }

    pub fn local_cloud_configured(&self) -> bool {
        self.cloud.backend == CloudBackendMode::Local
    }

    pub fn cumulus_configured(&self) -> bool {
        self.cloud.backend == CloudBackendMode::Cumulus
    }
}

#[cfg(test)]
mod ticket_section_tests {
    use super::*;

    fn parse(text: &str) -> Result<TicketSection, String> {
        toml::from_str::<TicketSection>(text).map_err(|error| error.to_string())
    }

    #[test]
    fn ticket_section_defaults_preserve_current_behaviour() {
        let section = TicketSection::default();
        assert_eq!(section.cache, TicketCacheMode::Disk);
        assert!(!section.auto_delegate);
        assert_eq!(section.source_order, vec![TicketSourceKind::LocalCache]);
        assert_eq!(section.denuvo_mode, DenuvoMode::Off);
        assert_eq!(section.denuvo_override, None);
        assert!(!section.denuvo_detection);
        assert!(section.source_order_problems().is_empty());

        let parsed = parse("").expect("empty ticket table should parse");
        assert_eq!(parsed.source_order, vec![TicketSourceKind::LocalCache]);
        assert_eq!(parsed.denuvo_lease_secs, 300);
    }

    #[test]
    fn ticket_section_parses_the_new_flags() {
        let section = parse(
            r#"
            source_order = ["local_cache", "credential_store", "registry"]
            denuvo_mode = "lease"
            denuvo_override = false
            denuvo_detection = true
            denuvo_lease_secs = 90
            "#,
        )
        .expect("flags should parse");

        assert_eq!(
            section.source_order,
            vec![
                TicketSourceKind::LocalCache,
                TicketSourceKind::CredentialStore,
                TicketSourceKind::Registry,
            ]
        );
        assert_eq!(section.denuvo_mode, DenuvoMode::Lease);
        assert_eq!(section.denuvo_override, Some(false));
        assert!(section.denuvo_detection);
        assert_eq!(section.denuvo_lease_secs, 90);
    }

    #[test]
    fn every_ticket_source_round_trips_through_its_config_name() {
        for kind in TicketSourceKind::ALL {
            let text = format!("source_order = [\"{}\"]", kind.name());
            let section = parse(&text).expect("each source name must parse");
            assert_eq!(section.source_order, vec![kind]);
        }
    }

    #[test]
    fn repeated_ticket_source_is_rejected_while_parsing() {
        let error = parse(r#"source_order = ["local_cache", "registry", "local_cache"]"#)
            .expect_err("a repeated source must be rejected at load time");
        assert!(error.contains("local_cache"), "unclear error: {error}");
        assert!(error.contains("more than once"), "unclear error: {error}");
    }

    #[test]
    fn empty_ticket_source_order_is_rejected_while_parsing() {
        let error = parse("source_order = []").expect_err("an empty order must be rejected");
        assert!(error.contains("empty"), "unclear error: {error}");
    }

    #[test]
    fn unknown_ticket_source_is_rejected() {
        let error = parse(r#"source_order = ["carrier_pigeon"]"#)
            .expect_err("an unknown source must be rejected");
        assert!(
            error.contains("carrier_pigeon") || error.contains("unknown variant"),
            "unclear error: {error}"
        );
    }
}
