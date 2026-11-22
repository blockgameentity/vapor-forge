//! Host-agnostic features, wired to the Windows host.
//!
//! The Linux host already runs cloud redirection/sync control, per-game
//! SteamID identity, the depotcache pre-seed and the local achievement rebuild.
//! Until this module existed those features were simply unreachable from the
//! Windows host: nothing turned the resolved settings into the inputs they need,
//! so "parity" was nominal.
//!
//! What lives here:
//!
//! * [`settings`](HostSettings) - the host's resolved settings in the plain shape
//!   the features consume ([`HostSettings`]). Deliberately not
//!   `vapor_forge_config::RuntimeConfig`: the host resolves config (and script
//!   state) once, so every decision below is a pure function of these values and
//!   is testable without a config file, a live client or an archive.
//! * [`cloud`] — endpoint + credentials presence + controlled apps become a
//!   per-app decision: redirect, leave alone, or refuse *with a reason*.
//! * [`identity`] — the per-game SteamID override selection with an explicit
//!   "no override" state; a configured value that is not an individual
//!   SteamID64 is refused, never replaced by an invented one.
//! * [`depotcache`] — the existing `vapor_forge_features::depotcache::preseed`
//!   over a list of wanted depot/manifest pairs, with a host-provided fetch and
//!   store, reporting one outcome per candidate and never aborting the batch.
//! * [`achievements`] — the local-rebuild flag, surfaced (and applied) here.
//! * [`report`] — which of the four features is active, and every inactive one
//!   with its reason, kept in a [`Gate`](crate::capability::Gate) so a disabled
//!   feature is reported exactly like a disabled capability.
//!
//! Nothing in this module installs a hook or touches the module image: it only
//! decides what the rest of the host may offer, and says why when it may not.
//! The one process-wide side effect is [`achievements::apply`], which sets the
//! local achievement rebuild flag, and only when the `achievements` capability is
//! enabled.

pub mod achievements;
pub mod cloud;
pub mod depotcache;
pub mod identity;
pub mod report;
mod run;
mod settings;

#[cfg(test)]
mod tests;

pub use achievements::{decide_rebuild, AchievementSettings, RebuildPlan, RebuildRequest};
pub use cloud::{plan_cloud, CloudBackend, CloudDecision, CloudMode, CloudPlan, CloudSettings};
pub use depotcache::{
    preseed_wanted, DepotCacheEntry, DepotCachePorts, DepotCacheReport, DepotCacheSettings,
};
pub use identity::{select_identities, select_identity, IdentitySelection, IdentitySettings};
pub use report::{FeatureEntry, FeatureReport, FeatureState, HostFeature};
pub use run::run;
pub use settings::HostSettings;
