//! `CloseAppCloud` — the cloud hook body (capabilities `cloud-http` and
//! `cloud-control`).
//!
//! The Linux host hooks `CHTTPRequestJob::Start` and short-circuits a cloud
//! transfer whose target is the process-local sentinel authority
//! (`crates/hooks/src/client/cloud_http.rs`): a transfer is admitted locally only
//! when its live fields agree with an unconsumed transfer contract, and every
//! other case leaves Steam's target alone. The Windows corpus anchors no
//! `CHTTPRequestJob::Start`, no cloud file open and no cloud write — the one cloud
//! symbol it resolves in both live builds is `CloseAppCloud`.
//!
//! So this body is the reporting half of that decision, at the only cloud
//! boundary the corpus gives us. When Steam closes cloud for an app, the body
//! asks the one owner of the decision — `features::cloud`'s resolved plan — what
//! the host would have done with that app's transfers, and says so once. It never
//! rewrites a request, never invents a target and never touches the returned
//! byte: with no transfer handle in hand, "report the plan" is the only honest
//! action, exactly as [`super::depot`] reports the depotcache precondition at the
//! depot key site.
//!
//! [`decide`] is the portable half and is unit-tested: given the resolved plan
//! and an app id it answers *route to the configured backend*, *leave Steam's
//! target alone* or *refuse*, each with a reason. It is what
//! `CHTTPRequestJob::Start` would consult if that anchor ever appears; the body
//! below is what consults it today.
//!
//! Flags, all of which are a pure fall-through when they say no:
//!
//! * the `cloud-http` capability survived the install pass;
//! * the host state is published;
//! * the call names one app (argument 1 `!= 0`). The `0` form means "every app"
//!   and only gets a one-line plan summary, because it carries no app id to
//!   decide about.

use core::ffi::c_void;

use tracing::{debug, trace};
use vapor_forge_config::AppId;

use crate::abi::cloud::{CloseAppCloudFn, EVERY_APP};
use crate::capability::Capability;
use crate::features::{CloudBackend, CloudDecision};
use crate::hooks::original;
use crate::hooks::state::{self, HostState};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CloseAppCloud";

/// Address of [`close_app_cloud`], for the installer's replacement table.
pub fn address() -> usize {
    close_app_cloud as CloseAppCloudFn as usize
}

/// Replacement for `CloseAppCloud`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `app_id`
/// and the two opaque register slots must be whatever Steam passed, and the
/// returned byte is Steam's own.
pub unsafe extern "system" fn close_app_cloud(
    this: *mut c_void,
    app_id: u32,
    r8: usize,
    r9: usize,
) -> u8 {
    // SAFETY: the signature is the one documented in `crate::abi::cloud`, and the
    // trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<CloseAppCloudFn>(SYMBOL) }) else {
        debug!(
            app_id,
            "cloud hook reached without a published original; falling through to Steam"
        );
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim, as documented.
    let steam = unsafe { original(this, app_id, r8, r9) };
    if let Some(state) = state::get() {
        report_cloud_route(state, app_id);
    }
    steam
}

/// What the host's cloud plan says about one app's cloud traffic.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloudRoute {
    /// Nothing of ours applies; Steam keeps its own target.
    PassThrough { reason: String },
    /// The plan would send this app's cloud traffic to `backend`.
    Redirect { backend: CloudBackend },
    /// The plan refuses cloud for controlled apps, so nothing is redirected.
    Refuse { reason: String },
}

impl CloudRoute {
    /// The one-line form a log field needs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::PassThrough { .. } => "pass-through",
            Self::Redirect { .. } => "redirect",
            Self::Refuse { .. } => "refuse",
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::PassThrough { reason } | Self::Refuse { reason } => Some(reason),
            Self::Redirect { .. } => None,
        }
    }
}

/// Ask the resolved cloud plan what it would do with `app_id`'s transfers.
///
/// Pure: the plan is the host's one decision (`features::cloud::plan_cloud`), so
/// this function adds the capability gate and nothing else. A disabled
/// `cloud-http` capability is the pre-fix behaviour: Steam's target stands.
pub fn decide(state: &HostState, app_id: AppId) -> CloudRoute {
    if !state.allows(Capability::CloudHttp) {
        return CloudRoute::PassThrough {
            reason: "the cloud-http capability is disabled for this run".to_owned(),
        };
    }
    match state.cloud().decision_for(app_id) {
        CloudDecision::Redirect { backend, .. } => CloudRoute::Redirect { backend },
        CloudDecision::LeaveAlone { reason, .. } => CloudRoute::PassThrough { reason },
        CloudDecision::Refuse { reason, .. } => CloudRoute::Refuse { reason },
    }
}

/// Say once what the plan says about this call.
fn report_cloud_route(state: &HostState, app_id: u32) {
    if !state.allows(Capability::CloudHttp) {
        return;
    }
    if app_id == EVERY_APP {
        // No app id to decide about: the summary is all this call can carry.
        trace!(
            plan = state.cloud().summary().as_str(),
            "cloud: every-app close, plan summary only"
        );
        return;
    }
    let route = decide(state, AppId(app_id));
    match &route {
        CloudRoute::Redirect { backend } => debug!(
            app_id,
            route = route.label(),
            backend = backend.label(),
            endpoint = backend.endpoint().unwrap_or("(endpoint unset)"),
            "cloud: this app's transfers belong to the host's backend; \
             no transfer site is anchored, nothing was rewritten"
        ),
        CloudRoute::Refuse { reason } => debug!(
            app_id,
            route = route.label(),
            reason = reason.as_str(),
            "cloud: controlled apps are refused cloud; Steam's own target stands"
        ),
        // The pre-fix behaviour is every other run; a line per cloud close would
        // be noise, so it stays at trace.
        CloudRoute::PassThrough { reason } => trace!(
            app_id,
            route = route.label(),
            reason = reason.as_str(),
            "cloud: Steam's own target stands"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{plan_cloud, CloudSettings};
    use crate::hooks::tests_support::{config_with, state_with};

    fn redirecting(controlled: &[u32]) -> CloudSettings {
        CloudSettings {
            backend: CloudBackend::Local {
                root: Some("C:\\vapor-forge-cloud".to_owned()),
            },
            controlled_apps: controlled.iter().copied().map(AppId).collect(),
        }
    }

    #[test]
    fn the_symbol_is_the_anchored_one() {
        assert_eq!(SYMBOL, "CloseAppCloud");
        assert_ne!(address(), 0, "the body must be addressable for the table");
    }

    #[test]
    fn a_controlled_app_is_redirected_to_the_configured_backend() {
        let config = config_with(&[730]);
        let state =
            state_with(config, Vec::new()).with_cloud_plan(plan_cloud(&redirecting(&[730])));
        match decide(&state, AppId(730)) {
            CloudRoute::Redirect { backend } => assert_eq!(backend.label(), "local"),
            other => panic!("expected a redirect, got {other:?}"),
        }
    }

    #[test]
    fn an_uncontrolled_app_keeps_steams_target() {
        let state = state_with(config_with(&[730]), Vec::new())
            .with_cloud_plan(plan_cloud(&redirecting(&[730])));
        let route = decide(&state, AppId(99_001));
        assert_eq!(route.label(), "pass-through");
        assert!(route.reason().is_some(), "a pass-through always says why");
    }

    #[test]
    fn a_backend_that_cannot_authenticate_refuses_rather_than_half_working() {
        let settings = CloudSettings {
            backend: CloudBackend::Cumulus {
                endpoint: Some("https://cumulus.invalid".to_owned()),
                credentials_present: false,
            },
            controlled_apps: vec![AppId(730)],
        };
        let state =
            state_with(config_with(&[730]), Vec::new()).with_cloud_plan(plan_cloud(&settings));
        let route = decide(&state, AppId(730));
        assert_eq!(route.label(), "refuse");
        assert!(route
            .reason()
            .is_some_and(|reason| { reason.contains("credentials") }));
    }

    #[test]
    fn a_disabled_capability_is_a_pure_fall_through() {
        let state = state_with(config_with(&[730]), Vec::new())
            .with_cloud_plan(plan_cloud(&redirecting(&[730])));
        let mut gate = state.gate();
        gate.disable(Capability::CloudHttp, "symbol did not resolve");
        state.set_gate(gate);
        let route = decide(&state, AppId(730));
        assert_eq!(route.label(), "pass-through");
        assert!(route
            .reason()
            .is_some_and(|reason| reason.contains("cloud-http")));
    }

    #[test]
    fn a_default_plan_never_redirects_anything() {
        // Nothing attached: the body must not invent a backend.
        let state = state_with(config_with(&[730]), Vec::new());
        assert_eq!(decide(&state, AppId(730)).label(), "pass-through");
    }
}
