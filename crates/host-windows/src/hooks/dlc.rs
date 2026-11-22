//! DLC overrides — **decision ready, no anchor**.
//!
//! The Linux host hooks `IClientAppManager::IsAppDlcInstalled` and
//! `IClientAppManager::BIsDlcEnabled` (`crates/hooks/src/client/dlc.rs`) and
//! answers "yes" for a DLC id that belongs to an app we control, so Steam stops
//! treating a purchased-but-unowned DLC as missing. The decision belongs to
//! `features::dlc::on_is_dlc_installed` / `on_is_dlc_enabled`; the host's part is
//! the capability gate and the app/DLC pair.
//!
//! # Why there is no body here
//!
//! Neither symbol is in the win64 corpus: verified with the miner against both
//! live builds, the steamclient corpus (15 public / 14 publicbeta symbols) carries
//! no `IClientAppManager::` entry and no DLC predicate at all. The one package
//! symbol the corpus does resolve, `CPackageInfo::GetPackageInfo`, already carries
//! [`super::package`]'s body: two replacements cannot share one entry, and the
//! package body runs at a point where no DLC id is in hand.
//!
//! [`decide`] is the portable half. It is what a body would call if
//! `IsAppDlcInstalled` were mined: the body would return `true` when this says
//! [`DlcAnswer::Override`] while Steam said `false`, and forward Steam's answer in
//! every other case.

use vapor_forge_config::AppId;
use vapor_forge_features::dlc;

use crate::capability::Capability;
use crate::hooks::state::HostState;

/// What the host wants Steam to be told about one DLC of one app.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DlcAnswer {
    /// Steam's own answer stands, untouched.
    PassThrough,
    /// The DLC is configured for a controlled app: report it as present.
    Override,
}

/// Decide the answer for `dlc_id` of `app_id`, given what Steam said.
///
/// Only a `false` from Steam is ever overridden: a DLC Steam already reports as
/// present needs nothing from us, and answering for a DLC the user configured
/// nowhere would be an invention.
pub fn decide(state: &HostState, app_id: AppId, dlc_id: AppId) -> DlcAnswer {
    if !state.allows(Capability::DlcOverrides) {
        return DlcAnswer::PassThrough;
    }
    if dlc::on_is_dlc_installed(state.config(), app_id, dlc_id, false) {
        return DlcAnswer::Override;
    }
    DlcAnswer::PassThrough
}

/// As [`decide`], for the "enabled" predicate, which is a separate Steam call.
///
/// Kept separate rather than collapsed into one answer because the two Linux
/// hooks are two entry points: a build could anchor one and not the other.
pub fn decide_enabled(state: &HostState, app_id: AppId, dlc_id: AppId) -> DlcAnswer {
    if !state.allows(Capability::DlcOverrides) {
        return DlcAnswer::PassThrough;
    }
    if dlc::on_is_dlc_enabled(state.config(), app_id, dlc_id, false) {
        return DlcAnswer::Override;
    }
    DlcAnswer::PassThrough
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::tests_support::state_with;
    use vapor_forge_config::{InjectApp, RuntimeConfig};

    fn config_with_dlc(app: u32, dlcs: &[u32]) -> RuntimeConfig {
        let mut config = RuntimeConfig::default();
        config.apps.push_inject(InjectApp {
            id: AppId(app),
            dlc: dlcs.iter().copied().map(AppId).collect(),
            ticket: Default::default(),
            purchase_time: 0,
        });
        config
    }

    #[test]
    fn a_configured_dlc_of_a_controlled_app_is_reported_present() {
        let state = state_with(config_with_dlc(480, &[505_730]), Vec::new());
        assert_eq!(
            decide(&state, AppId(480), AppId(505_730)),
            DlcAnswer::Override
        );
        assert_eq!(
            decide_enabled(&state, AppId(480), AppId(505_730)),
            DlcAnswer::Override
        );
    }

    #[test]
    fn a_dlc_the_user_configured_nowhere_is_left_to_steam() {
        let state = state_with(config_with_dlc(480, &[505_730]), Vec::new());
        assert_eq!(
            decide(&state, AppId(480), AppId(505_999)),
            DlcAnswer::PassThrough
        );
        // Same DLC id under an app we do not control: also not ours to answer.
        assert_eq!(
            decide(&state, AppId(999), AppId(505_730)),
            DlcAnswer::PassThrough
        );
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with_dlc(480, &[505_730]), Vec::new());
        let mut gate = state.gate();
        gate.disable(Capability::DlcOverrides, "no anchor in the corpus");
        state.set_gate(gate);
        assert_eq!(
            decide(&state, AppId(480), AppId(505_730)),
            DlcAnswer::PassThrough
        );
        assert_eq!(
            decide_enabled(&state, AppId(480), AppId(505_730)),
            DlcAnswer::PassThrough
        );
    }
}
