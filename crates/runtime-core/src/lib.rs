#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RuntimePhase {
    Created = 0,
    AuditVersionSeen = 1,
    ObjectSeen = 2,
    ReadyForHeavyInit = 3,
    Closing = 4,
}

impl RuntimePhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::AuditVersionSeen,
            2 => Self::ObjectSeen,
            3 => Self::ReadyForHeavyInit,
            4 => Self::Closing,
            _ => Self::Created,
        }
    }
}

#[derive(Debug)]
pub struct Lifecycle {
    phase: AtomicU8,
    ready_for_heavy_init_seen: AtomicBool,
}

impl Lifecycle {
    pub const fn new() -> Self {
        Self {
            phase: AtomicU8::new(RuntimePhase::Created as u8),
            ready_for_heavy_init_seen: AtomicBool::new(false),
        }
    }

    pub fn mark_version_seen(&self) {
        self.raise_to(RuntimePhase::AuditVersionSeen);
    }

    pub fn mark_object_seen(&self) {
        self.raise_to(RuntimePhase::ObjectSeen);
    }

    pub fn mark_ready_for_heavy_init(&self) {
        self.ready_for_heavy_init_seen
            .store(true, Ordering::Release);
        self.raise_to(RuntimePhase::ReadyForHeavyInit);
    }

    pub fn mark_closing(&self) {
        self.phase
            .store(RuntimePhase::Closing as u8, Ordering::Release);
    }

    pub fn phase(&self) -> RuntimePhase {
        RuntimePhase::from_u8(self.phase.load(Ordering::Acquire))
    }

    pub fn has_reached_ready_for_heavy_init(&self) -> bool {
        self.ready_for_heavy_init_seen.load(Ordering::Acquire)
    }

    fn raise_to(&self, target: RuntimePhase) {
        let target = target as u8;
        let mut current = self.phase.load(Ordering::Acquire);

        while current < target {
            match self.phase.compare_exchange_weak(
                current,
                target,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(next) => current = next,
            }
        }
    }
}

impl Default for Lifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct AppId(pub u32);

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct DepotId(pub u32);

#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct ManifestId(pub u64);

impl std::fmt::Display for AppId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for DepotId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for ManifestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Address(pub usize);

#[derive(Clone, Debug, Default)]
pub struct ScriptState {
    /// Set of controlled apps registered via `addappid`.
    pub apps: HashSet<AppId>,
    pub depot_keys: HashMap<DepotId, Vec<u8>>,
    pub manifests: HashMap<DepotId, ManifestOverride>,
    pub app_tickets: HashMap<AppId, Vec<u8>>,
    pub enc_tickets: HashMap<AppId, Vec<u8>>,
    pub stat_steam_ids: HashMap<AppId, u64>,
    pub avatars: HashMap<AppId, AppId>,
    pub access_tokens: HashMap<AppId, u64>,
    /// Per-app max mtime across every `.lua` file that contributed the
    /// app via `addappid`; used to stamp the library's "purchased" date.
    pub app_purchase_times: HashMap<AppId, u32>,
    /// Per-app achievement state the user's Lua scripts asked for, keyed by
    /// achievement key and sorted so a reply always composes in the same order.
    ///
    /// This is the only source of achievement *intent* that does not come from
    /// Steam: definitions still come from the schema a reply carries, and the
    /// unlock state still starts from Steam's own reply. An override is applied
    /// last, which is what makes a controlled app's page show an achievement the
    /// donor account never unlocked.
    pub achievement_overrides: HashMap<AppId, BTreeMap<String, AchievementIntent>>,
}

/// The longest achievement key a Lua script may register.
///
/// A bound, not a layout fact: Steam's own API names are well under this, so a
/// longer one is a typo or an attempt to grow the state without limit.
pub const MAX_ACHIEVEMENT_KEY_LEN: usize = 128;

/// What a Lua script asked the achievements path to report for one key.
///
/// Only `unlocked` decides the wire flag. `unlock_time` is the rtime32 a client
/// renders next to an unlocked achievement; `None` means "unlocked, time
/// unknown", which Steam encodes with its own sentinel rather than a zero the
/// client would read as "never unlocked".
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
pub struct AchievementIntent {
    pub unlocked: bool,
    pub unlock_time: Option<u32>,
}

impl AchievementIntent {
    /// Unlocked, with the time to show.
    pub fn unlocked_at(unlock_time: u32) -> Self {
        Self {
            unlocked: true,
            unlock_time: Some(unlock_time),
        }
    }

    /// Unlocked, with no time known.
    pub fn unlocked() -> Self {
        Self {
            unlocked: true,
            unlock_time: None,
        }
    }

    /// Locked, which clears any time Steam or an earlier script reported.
    pub fn locked() -> Self {
        Self {
            unlocked: false,
            unlock_time: None,
        }
    }

    /// Whether `key` may be used as an achievement key at all.
    ///
    /// Empty, whitespace-only and over-long keys are refused here rather than
    /// trimmed: an achievement key is an identifier Steam's schema owns, so
    /// "nearly right" would silently match nothing.
    pub fn key_is_valid(key: &str) -> bool {
        !key.trim().is_empty() && key.len() <= MAX_ACHIEVEMENT_KEY_LEN && !key.contains('\0')
    }
}

#[derive(Clone, Debug)]
pub struct ManifestOverride {
    pub depot_id: DepotId,
    pub gid: ManifestId,
    pub size: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SteamModuleKind {
    SteamClient,
    SteamUi,
}

impl SteamModuleKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SteamClient => "steamclient.so",
            Self::SteamUi => "steamui.so",
        }
    }

    pub fn from_name_or_path(name_or_path: &str) -> Option<Self> {
        let name = name_or_path.rsplit('/').next().unwrap_or(name_or_path);
        match name {
            "steamclient.so" => Some(Self::SteamClient),
            "steamui.so" => Some(Self::SteamUi),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct SteamModuleState {
    steamclient_seen: AtomicBool,
    steamui_seen: AtomicBool,
}

impl SteamModuleState {
    pub const fn new() -> Self {
        Self {
            steamclient_seen: AtomicBool::new(false),
            steamui_seen: AtomicBool::new(false),
        }
    }

    pub fn mark_seen_by_name(&self, name_or_path: &str) -> Option<SteamModuleKind> {
        let kind = SteamModuleKind::from_name_or_path(name_or_path)?;

        match kind {
            SteamModuleKind::SteamClient => self.steamclient_seen.store(true, Ordering::Release),
            SteamModuleKind::SteamUi => self.steamui_seen.store(true, Ordering::Release),
        }

        Some(kind)
    }

    pub fn steamclient_seen(&self) -> bool {
        self.steamclient_seen.load(Ordering::Acquire)
    }

    pub fn steamui_seen(&self) -> bool {
        self.steamui_seen.load(Ordering::Acquire)
    }

    pub fn any_seen(&self) -> bool {
        self.steamclient_seen() || self.steamui_seen()
    }
}

impl Default for SteamModuleState {
    fn default() -> Self {
        Self::new()
    }
}

/// Whole seconds since the Unix epoch, saturating at 0 for clocks set earlier.
pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::{
        AchievementIntent, Lifecycle, RuntimePhase, ScriptState, SteamModuleKind, SteamModuleState,
        MAX_ACHIEVEMENT_KEY_LEN,
    };

    #[test]
    fn achievement_intent_has_one_meaning_per_constructor() {
        assert_eq!(
            AchievementIntent::unlocked_at(1_700_000_000),
            AchievementIntent {
                unlocked: true,
                unlock_time: Some(1_700_000_000),
            }
        );
        assert_eq!(
            AchievementIntent::unlocked(),
            AchievementIntent {
                unlocked: true,
                unlock_time: None,
            }
        );
        assert_eq!(AchievementIntent::locked(), AchievementIntent::default());
    }

    #[test]
    fn achievement_keys_are_bounded_and_non_blank() {
        assert!(AchievementIntent::key_is_valid("ACH_WIN"));
        assert!(!AchievementIntent::key_is_valid(""));
        assert!(!AchievementIntent::key_is_valid("   "));
        assert!(!AchievementIntent::key_is_valid("ACH\0WIN"));
        assert!(AchievementIntent::key_is_valid(
            &"A".repeat(MAX_ACHIEVEMENT_KEY_LEN)
        ));
        assert!(!AchievementIntent::key_is_valid(
            &"A".repeat(MAX_ACHIEVEMENT_KEY_LEN + 1)
        ));
    }

    #[test]
    fn script_state_starts_with_no_achievement_intent() {
        assert!(ScriptState::default().achievement_overrides.is_empty());
    }

    #[test]
    fn lifecycle_only_moves_forward_until_closing() {
        let lifecycle = Lifecycle::new();

        assert_eq!(lifecycle.phase(), RuntimePhase::Created);
        assert!(!lifecycle.has_reached_ready_for_heavy_init());
        lifecycle.mark_object_seen();
        lifecycle.mark_version_seen();
        assert_eq!(lifecycle.phase(), RuntimePhase::ObjectSeen);
        assert!(!lifecycle.has_reached_ready_for_heavy_init());

        lifecycle.mark_ready_for_heavy_init();
        assert_eq!(lifecycle.phase(), RuntimePhase::ReadyForHeavyInit);
        assert!(lifecycle.has_reached_ready_for_heavy_init());

        lifecycle.mark_closing();
        assert_eq!(lifecycle.phase(), RuntimePhase::Closing);
        assert!(lifecycle.has_reached_ready_for_heavy_init());
    }

    #[test]
    fn steam_module_state_tracks_public_module_names() {
        let state = SteamModuleState::new();

        assert_eq!(
            state.mark_seen_by_name("/home/user/.steam/steam/ubuntu12_32/steamclient.so"),
            Some(SteamModuleKind::SteamClient)
        );
        assert!(state.steamclient_seen());
        assert!(!state.steamui_seen());

        assert_eq!(
            state.mark_seen_by_name("steamui.so"),
            Some(SteamModuleKind::SteamUi)
        );
        assert!(state.any_seen());
        assert_eq!(state.mark_seen_by_name("libc.so.6"), None);
    }
}
