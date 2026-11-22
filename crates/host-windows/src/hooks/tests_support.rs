//! Builders for the state a hook test needs, so no test has to publish the
//! process-global one.

use vapor_forge_config::{AppId, InjectApp, RuntimeConfig};
use vapor_forge_features::family::FamilySettings;
use vapor_forge_features::ticket::authorisation::AuthorisationWindow;

use crate::hooks::depot_store::DepotPlan;
use crate::hooks::state::HostState;

/// A config that controls exactly `apps` (`[apps] inject`), nothing else.
pub(crate) fn config_with(apps: &[u32]) -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    for &id in apps {
        config.apps.push_inject(InjectApp {
            id: AppId(id),
            dlc: Vec::new(),
            ticket: Default::default(),
            purchase_time: 0,
        });
    }
    config
}

/// A state with the default family settings, no ticket directory, no depot plan,
/// and the `controlled` set the caller names explicitly.
///
/// `controlled` is passed separately from the config on purpose: the injection
/// plan is the config's, but the list a caller walks is the effective set the
/// host computed at start-up.
pub(crate) fn state_with(config: RuntimeConfig, controlled: Vec<AppId>) -> HostState {
    HostState::new(
        config,
        FamilySettings::default(),
        controlled,
        None,
        DepotPlan::none(),
    )
}

/// As [`state_with`], with an explicit authorisation lease.
pub(crate) fn state_with_authorisation(
    config: RuntimeConfig,
    controlled: Vec<AppId>,
    authorisation: Option<AuthorisationWindow>,
) -> HostState {
    state_with(config, controlled).with_authorisation(authorisation)
}
