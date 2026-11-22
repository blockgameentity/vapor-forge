//! Per-game SteamID identity: the override selection, never an invented id.
//!
//! `vapor_forge_features::identity` owns the process-wide identity state
//! (observe from packets, set from `IClientUser::GetSteamID`, generation). The
//! host's job is narrower: for a given app, which SteamID should be presented -
//! and *nothing* when the user configured nothing.
//!
//! Two rules make this safe:
//!
//! * "no override" is a state, not a missing value: [`IdentitySelection::NoOverride`]
//!   is explicit, so a caller cannot mistake "not configured" for "use zero" or
//!   for some other app's id;
//! * a configured value that is not an individual SteamID64 is refused with a
//!   reason. It is never replaced by a guessed, derived or default id - the
//!   whole point of the override is that it says exactly who the client should be.

use std::collections::BTreeMap;

use vapor_forge_config::AppId;
use vapor_forge_features::identity::is_valid_individual_steam_id;

/// The configured overrides (`[steam_id]` in config, script state merged in).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IdentitySettings {
    pub overrides: BTreeMap<AppId, u64>,
}

/// What the host should present for one app.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentitySelection {
    /// Nothing configured: Steam's own account id is used, untouched.
    NoOverride { app_id: AppId },
    /// A validated individual SteamID64 to present for this app.
    Override { app_id: AppId, steam_id64: u64 },
    /// Configured, but unusable. No id is invented and none is applied.
    Refuse { app_id: AppId, reason: String },
}

impl IdentitySelection {
    pub fn app_id(&self) -> AppId {
        match self {
            Self::NoOverride { app_id }
            | Self::Override { app_id, .. }
            | Self::Refuse { app_id, .. } => *app_id,
        }
    }

    /// The id to present, if any. `None` for "no override" and for a refusal.
    pub fn steam_id64(&self) -> Option<u64> {
        match self {
            Self::Override { steam_id64, .. } => Some(*steam_id64),
            Self::NoOverride { .. } | Self::Refuse { .. } => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Refuse { reason, .. } => Some(reason),
            Self::NoOverride { .. } | Self::Override { .. } => None,
        }
    }

    pub fn is_override(&self) -> bool {
        matches!(self, Self::Override { .. })
    }
}

/// Select the identity to present for one app.
pub fn select_identity(settings: &IdentitySettings, app_id: AppId) -> IdentitySelection {
    match settings.overrides.get(&app_id).copied() {
        None => {
            tracing::debug!(app_id = app_id.0, "identity: no override configured");
            IdentitySelection::NoOverride { app_id }
        }
        Some(steam_id64) if is_valid_individual_steam_id(steam_id64) => {
            tracing::info!(
                app_id = app_id.0,
                steam_id64,
                "identity: per-game override selected"
            );
            IdentitySelection::Override { app_id, steam_id64 }
        }
        Some(steam_id64) => {
            let reason = format!(
                "configured SteamID64 {steam_id64} for app {app_id} is not an individual \
                 account id; no override is applied"
            );
            tracing::warn!(
                app_id = app_id.0,
                reason = reason.as_str(),
                "identity: refused"
            );
            IdentitySelection::Refuse { app_id, reason }
        }
    }
}

/// Select one decision per configured app, in app-id order.
pub fn select_identities(settings: &IdentitySettings) -> Vec<IdentitySelection> {
    settings
        .overrides
        .keys()
        .map(|&app_id| select_identity(settings, app_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(u32, u64)]) -> IdentitySettings {
        IdentitySettings {
            overrides: pairs.iter().map(|&(app, id)| (AppId(app), id)).collect(),
        }
    }

    #[test]
    fn absent_override_is_an_explicit_no_override() {
        let selection = select_identity(&IdentitySettings::default(), AppId(730));
        assert_eq!(
            selection,
            IdentitySelection::NoOverride { app_id: AppId(730) }
        );
        assert_eq!(selection.steam_id64(), None);
        assert!(selection.reason().is_none());
    }

    #[test]
    fn a_valid_override_is_selected_verbatim() {
        let selection = select_identity(&settings(&[(730, 76561198106179127)]), AppId(730));
        assert_eq!(selection.steam_id64(), Some(76561198106179127));
        assert!(selection.is_override());
    }

    #[test]
    fn an_unusable_override_is_refused_rather_than_invented() {
        let selection = select_identity(&settings(&[(730, 0x0000_0001_0000_0001)]), AppId(730));
        assert!(matches!(selection, IdentitySelection::Refuse { .. }));
        assert_eq!(selection.steam_id64(), None, "no id may be invented");
        assert!(selection.reason().is_some());
    }

    #[test]
    fn every_configured_app_gets_exactly_one_selection() {
        let selections = select_identities(&settings(&[
            (730, 76561198106179127),
            (440, 1),
            (10, 76561198000000002),
        ]));
        assert_eq!(selections.len(), 3);
        assert_eq!(selections[0].app_id(), AppId(10));
        assert_eq!(
            selections.iter().filter(|s| s.is_override()).count(),
            2,
            "the invalid override must not count as one"
        );
    }
}
