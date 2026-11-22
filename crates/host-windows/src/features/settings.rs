//! The host's resolved settings, in the shape the host-agnostic features need.
//!
//! The Windows host resolves `vapor_forge_config::RuntimeConfig` and the script
//! state once, into these plain values. Everything downstream is then a pure
//! function of them, which is what makes the feature decisions testable without
//! a config file, a client or an archive.
//!
//! [`HostSettings::resolve`] is that one resolution step. Three inputs cannot be
//! derived from config alone and are therefore named here instead of guessed:
//!
//! * the depotcache candidate list comes from manifest request planning
//!   (`vapor_forge_features::manifest`), not from config; the host fills
//!   [`DepotCacheSettings::wanted`] from the plan it is about to install;
//! * the local achievement rebuild flag has its own config field
//!   (`[achievements] local_rebuild`), which the worker applies through
//!   `vapor_forge_features::config_apply`, so `resolve` leaves it off here and the
//!   worker sets [`AchievementSettings`] from the applied value;
//! * `[steam_id]` overrides are not enumerable (`SteamIdSection` exposes only
//!   `get`), so `resolve` takes the per-game ids from script state
//!   (`ScriptState::stat_steam_ids`), which is the live source the Linux stats
//!   path already uses.

use std::collections::BTreeMap;

use vapor_forge_config::{CloudBackendMode, RuntimeConfig};
use vapor_forge_core::ScriptState;

use crate::features::achievements::AchievementSettings;
use crate::features::cloud::{CloudBackend, CloudSettings};
use crate::features::depotcache::DepotCacheSettings;
use crate::features::identity::IdentitySettings;

/// Every setting the four host-agnostic features read.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HostSettings {
    pub cloud: CloudSettings,
    pub depotcache: DepotCacheSettings,
    pub identity: IdentitySettings,
    pub achievements: AchievementSettings,
}

impl HostSettings {
    /// Settings with nothing configured: cloud disabled, no overrides, no
    /// wanted manifests, local rebuild off. The host degrades to Steam's own
    /// behaviour and the report says so for each feature.
    pub fn none() -> Self {
        Self::default()
    }

    /// Resolve the parts of the settings that config and script state own.
    ///
    /// Deliberately does not invent anything: an empty endpoint, an empty token
    /// and an empty folder all stay empty, so the cloud decision refuses instead
    /// of dialling a placeholder, and a per-game id that is not an individual
    /// SteamID64 stays visible so the identity decision can refuse it.
    pub fn resolve(config: &RuntimeConfig, state: &ScriptState) -> Self {
        let settings = Self {
            cloud: CloudSettings {
                backend: resolve_backend(config),
                controlled_apps: controlled_apps(config),
            },
            depotcache: DepotCacheSettings::default(),
            identity: IdentitySettings {
                overrides: state
                    .stat_steam_ids
                    .iter()
                    .map(|(&app_id, &steam_id64)| (app_id, steam_id64))
                    .collect::<BTreeMap<_, _>>(),
            },
            achievements: AchievementSettings::default(),
        };
        tracing::info!(
            backend = settings.cloud.backend.label(),
            controlled_apps = settings.cloud.controlled_apps.len(),
            identity_overrides = settings.identity.overrides.len(),
            "host features: settings resolved from config and script state"
        );
        settings
    }
}

fn resolve_backend(config: &RuntimeConfig) -> CloudBackend {
    match config.cloud.backend {
        CloudBackendMode::Disabled => CloudBackend::Disabled,
        CloudBackendMode::Local => CloudBackend::Local {
            root: non_empty(&config.cloud.local.path),
        },
        CloudBackendMode::Cumulus => CloudBackend::Cumulus {
            endpoint: non_empty(&config.cloud.cumulus.server_url),
            // Presence only: the token itself never leaves the config object.
            credentials_present: !config.cloud.cumulus.token.is_empty(),
        },
    }
}

/// Apps the user does not own, exactly the set the Linux cloud gate visits
/// (`[apps] inject`); DLC ids are not cloud-bearing apps and are not listed.
fn controlled_apps(config: &RuntimeConfig) -> Vec<vapor_forge_config::AppId> {
    config.apps.inject().iter().map(|app| app.id).collect()
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vapor_forge_config::{AppId, InjectApp};

    fn config_with_cumulus(endpoint: &str, token: &str) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.cloud.backend = CloudBackendMode::Cumulus;
        config.cloud.cumulus.server_url = endpoint.to_owned();
        config.cloud.cumulus.token = token.to_owned();
        config.apps.push_inject(InjectApp {
            id: AppId(730),
            dlc: vec![AppId(731)],
            ticket: Default::default(),
            purchase_time: 0,
        });
        config
    }

    #[test]
    fn default_settings_configure_nothing() {
        let settings = HostSettings::none();
        assert_eq!(settings.cloud.controlled_apps.len(), 0);
        assert_eq!(settings.depotcache.wanted.len(), 0);
        assert_eq!(settings.identity.overrides.len(), 0);
        assert!(!settings.achievements.local_rebuild);
    }

    #[test]
    fn resolves_a_cumulus_backend_and_the_controlled_apps() {
        let config = config_with_cumulus("https://cloud.example", "secret-token");
        let settings = HostSettings::resolve(&config, &ScriptState::default());
        assert_eq!(
            settings.cloud.backend,
            CloudBackend::Cumulus {
                endpoint: Some("https://cloud.example".to_owned()),
                credentials_present: true,
            }
        );
        assert_eq!(settings.cloud.controlled_apps, vec![AppId(730)]);
    }

    #[test]
    fn a_missing_token_stays_missing_instead_of_being_invented() {
        let config = config_with_cumulus("https://cloud.example", "");
        let settings = HostSettings::resolve(&config, &ScriptState::default());
        assert!(matches!(
            settings.cloud.backend,
            CloudBackend::Cumulus {
                credentials_present: false,
                ..
            }
        ));
    }

    #[test]
    fn an_empty_endpoint_stays_empty() {
        let config = config_with_cumulus("   ", "secret-token");
        let settings = HostSettings::resolve(&config, &ScriptState::default());
        assert_eq!(settings.cloud.backend.endpoint(), None);
    }

    #[test]
    fn a_local_backend_takes_its_folder_from_config() {
        let mut config = RuntimeConfig::default();
        config.cloud.backend = CloudBackendMode::Local;
        config.cloud.local.path = "  /srv/vf-cloud  ".to_owned();
        let settings = HostSettings::resolve(&config, &ScriptState::default());
        assert_eq!(settings.cloud.backend.endpoint(), Some("/srv/vf-cloud"));
    }

    #[test]
    fn identity_overrides_come_from_script_state_verbatim() {
        let mut state = ScriptState::default();
        state.stat_steam_ids.insert(AppId(730), 76561198106179127);
        state.stat_steam_ids.insert(AppId(440), 1);
        let settings = HostSettings::resolve(&RuntimeConfig::default(), &state);
        assert_eq!(settings.identity.overrides.len(), 2);
        assert_eq!(
            settings.identity.overrides.get(&AppId(730)),
            Some(&76561198106179127)
        );
        assert_eq!(
            settings.identity.overrides.get(&AppId(440)),
            Some(&1),
            "an unusable id is kept so the identity decision can refuse it"
        );
    }
}
