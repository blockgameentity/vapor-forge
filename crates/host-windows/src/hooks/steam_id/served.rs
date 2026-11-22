//! Which app a `GetSteamID` call can belong to — the guard against guessing.
//!
//! The Windows accessor carries no app id: it reads one member of `this` and
//! stores it through a pointer. The Linux host reads the caller's AppID from its
//! IPC dispatcher (`client::current_app`), and this host publishes no equivalent,
//! so an override has to be justified from the configuration alone.
//!
//! [`served_app`] is that justification, and it is deliberately narrow: it answers
//! only when there is exactly one app the call *can* be about. Presenting one
//! app's borrowed identity to another is the one failure this hook must not cause,
//! so an ambiguous configuration forwards Steam's own answer instead of picking.

use vapor_forge_config::{AppId, InjectApp, RuntimeConfig};

use crate::hooks::state::HostState;

/// The app a `GetSteamID` call in this process can belong to, when there is
/// exactly one.
///
/// Two rules keep the answer honest:
///
/// * the config must inject exactly one app. A DLC id is not a game and does not
///   count, but a second injected app does: with two of them any call could be
///   about either one;
/// * the effective controlled set must name nothing outside that app and its own
///   DLC. A script (`addappid`) app is invisible to the config but present in the
///   set, and we cannot tell whether the running game is the config's app or the
///   script's.
pub fn served_app(state: &HostState) -> Option<AppId> {
    let only = single_injected_app(state.config())?;
    let dlc = &only.dlc;
    let nothing_else = state
        .controlled_apps()
        .iter()
        .all(|app| *app == only.id || dlc.contains(app));
    if !nothing_else {
        tracing::debug!(
            app_id = only.id.0,
            "steam-id: more than one app is controlled; refusing to guess which one is asking"
        );
        return None;
    }
    Some(only.id)
}

fn single_injected_app(config: &RuntimeConfig) -> Option<&InjectApp> {
    match config.apps.inject() {
        [only] => Some(only),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::{config_with, state_with};

    fn inject_one(app: u32, dlc: &[u32]) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.apps.push_inject(InjectApp {
            id: AppId(app),
            dlc: dlc.iter().copied().map(AppId).collect(),
            ticket: Default::default(),
            purchase_time: 0,
        });
        config
    }

    #[test]
    fn one_injected_app_is_the_app_a_call_can_belong_to() {
        let state = state_with(inject_one(440, &[]), Vec::new());
        assert_eq!(served_app(&state), Some(AppId(440)));
    }

    #[test]
    fn two_injected_apps_name_no_served_app() {
        let state = state_with(config_with(&[440, 730]), Vec::new());
        assert_eq!(
            served_app(&state),
            None,
            "with two apps injected a call cannot be attributed to either"
        );
    }

    #[test]
    fn no_injected_app_names_no_served_app() {
        let state = state_with(config_with(&[]), Vec::new());
        assert_eq!(served_app(&state), None);
    }

    #[test]
    fn a_second_controlled_app_makes_the_answer_ambiguous() {
        // The config names one app, but the effective set names a script app too
        // (`addappid`): the process may be serving either, so neither is claimed.
        let state = state_with(config_with(&[440]), vec![AppId(440), AppId(480)]);
        assert_eq!(served_app(&state), None);
    }

    #[test]
    fn a_dlc_id_does_not_make_the_answer_ambiguous() {
        // A DLC id is controlled through its game, not instead of it: 731 is
        // 440's DLC, so 440 is still the one app a call can belong to.
        let state = state_with(inject_one(440, &[731]), vec![AppId(440), AppId(731)]);
        assert_eq!(served_app(&state), Some(AppId(440)));
    }
}
