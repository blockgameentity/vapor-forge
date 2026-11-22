//! Cloud redirection / sync control: settings in, per-app decision out.
//!
//! The Linux host answers "does this app have Steam cloud?" from
//! `vapor_forge_features::cloud` and serves the requests from `cloud-local` or
//! `cloud-rpc`. The Windows host cannot serve a request yet, but the decision it
//! would make must already be the same one, with the same refusal rules, so the
//! behaviour does not change depending on which host runs it.
//!
//! Three inputs decide it: the configured backend (endpoint + whether
//! credentials are present), and which apps the host controls. A controlled app
//! whose backend cannot serve it is **refused with a reason** - it is never
//! redirected to a backend that cannot authenticate, and it is never quietly left
//! syncing against Valve while the user believes it is redirected. See
//! [`CloudMode`].

use vapor_forge_config::AppId;

/// Which cloud backend the host resolved from `[cloud]`.
///
/// `Disabled` is the default: with no backend configured a controlled app keeps
/// Steam's own cloud, which is what "leave alone" means.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum CloudBackend {
    #[default]
    Disabled,
    /// A local archive folder served by `cloud-local` (`[cloud.local] path`).
    Local { root: Option<String> },
    /// An operator endpoint served by `cloud-rpc` / `cloud-cumulus`.
    ///
    /// `credentials_present` is a boolean on purpose: the host decides whether a
    /// token exists, but the token itself never reaches this module, the report
    /// or a log line.
    Cumulus {
        endpoint: Option<String>,
        credentials_present: bool,
    },
}

impl CloudBackend {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Local { .. } => "local",
            Self::Cumulus { .. } => "cumulus",
        }
    }

    /// The configured destination, never a secret.
    pub fn endpoint(&self) -> Option<&str> {
        match self {
            Self::Disabled => None,
            Self::Local { root } => root.as_deref(),
            Self::Cumulus { endpoint, .. } => endpoint.as_deref(),
        }
    }
}

/// Everything the cloud decision reads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CloudSettings {
    pub backend: CloudBackend,
    /// Apps the host controls (config `[apps] inject` plus their DLC). Steam's
    /// own cloud cannot serve them, so each one needs an explicit decision.
    pub controlled_apps: Vec<AppId>,
}

/// The decision for the whole set of controlled apps.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloudMode {
    /// Serve the controlled apps from the configured backend.
    Redirect,
    /// Keep Steam's own cloud: the feature is off, or there is nothing to serve.
    #[default]
    LeaveAlone,
    /// The backend was asked to serve apps it cannot serve. Do not sync.
    Refuse,
}

impl CloudMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Redirect => "redirect",
            Self::LeaveAlone => "leave alone",
            Self::Refuse => "refuse",
        }
    }
}

/// The decision for one app.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CloudDecision {
    Redirect {
        app_id: AppId,
        backend: CloudBackend,
    },
    LeaveAlone {
        app_id: AppId,
        reason: String,
    },
    Refuse {
        app_id: AppId,
        reason: String,
    },
}

impl CloudDecision {
    pub fn app_id(&self) -> AppId {
        match self {
            Self::Redirect { app_id, .. }
            | Self::LeaveAlone { app_id, .. }
            | Self::Refuse { app_id, .. } => *app_id,
        }
    }

    pub fn is_redirect(&self) -> bool {
        matches!(self, Self::Redirect { .. })
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Redirect { .. } => None,
            Self::LeaveAlone { reason, .. } | Self::Refuse { reason, .. } => Some(reason),
        }
    }
}

/// One decision for every controlled app, plus the reason it applies.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CloudPlan {
    pub mode: CloudMode,
    /// Why the plan is not `Redirect`. Present exactly when it is not.
    pub reason: Option<String>,
    pub backend: CloudBackend,
    /// Controlled apps this plan governs. Empty when nothing is controlled.
    pub apps: Vec<AppId>,
}

impl CloudPlan {
    /// Apps that would be redirected. Empty unless [`CloudMode::Redirect`].
    pub fn redirected_apps(&self) -> &[AppId] {
        match self.mode {
            CloudMode::Redirect => &self.apps,
            CloudMode::LeaveAlone | CloudMode::Refuse => &[],
        }
    }

    pub fn is_refused(&self) -> bool {
        self.mode == CloudMode::Refuse
    }

    /// The per-app decision for `app_id`.
    ///
    /// An app that is not controlled is left alone: Steam serves it and the host
    /// has no business redirecting it.
    pub fn decision_for(&self, app_id: AppId) -> CloudDecision {
        if !self.apps.contains(&app_id) {
            return CloudDecision::LeaveAlone {
                app_id,
                reason: "app is not controlled by the host; Steam serves its cloud".to_owned(),
            };
        }
        match self.mode {
            CloudMode::Redirect => CloudDecision::Redirect {
                app_id,
                backend: self.backend.clone(),
            },
            CloudMode::LeaveAlone => CloudDecision::LeaveAlone {
                app_id,
                reason: self.fallback_reason(),
            },
            CloudMode::Refuse => CloudDecision::Refuse {
                app_id,
                reason: self.fallback_reason(),
            },
        }
    }

    /// One dialog line: mode, app count and destination (never a token).
    pub fn summary(&self) -> String {
        match self.mode {
            CloudMode::Redirect => format!(
                "redirect {} controlled app(s) to {} {}",
                self.apps.len(),
                self.backend.label(),
                self.backend.endpoint().unwrap_or("(endpoint unset)")
            ),
            CloudMode::LeaveAlone | CloudMode::Refuse => {
                format!("{}: {}", self.mode.label(), self.fallback_reason())
            }
        }
    }

    fn fallback_reason(&self) -> String {
        self.reason
            .clone()
            .unwrap_or_else(|| "cloud control is not active".to_owned())
    }
}

/// Decide what to do with the controlled apps.
///
/// Order matters, and each branch is a rule from the parity matrix: an off
/// backend never redirects, an empty controlled set needs no work, and a backend
/// that cannot authenticate refuses instead of half-working.
pub fn plan_cloud(settings: &CloudSettings) -> CloudPlan {
    let apps = settings.controlled_apps.clone();
    let (mode, reason) = match &settings.backend {
        CloudBackend::Disabled => (
            CloudMode::LeaveAlone,
            Some("cloud backend is disabled ([cloud] backend = \"disabled\")".to_owned()),
        ),
        _ if apps.is_empty() => (
            CloudMode::LeaveAlone,
            Some("no controlled app needs cloud redirection".to_owned()),
        ),
        CloudBackend::Local { root: None } => (
            CloudMode::Refuse,
            Some("local cloud backend has no watch folder ([cloud.local] path)".to_owned()),
        ),
        CloudBackend::Cumulus { endpoint: None, .. } => (
            CloudMode::Refuse,
            Some("cumulus cloud backend has no endpoint ([cloud.cumulus] server_url)".to_owned()),
        ),
        CloudBackend::Cumulus {
            credentials_present: false,
            ..
        } => (
            CloudMode::Refuse,
            Some(
                "cumulus cloud backend has no credentials ([cloud.cumulus] token); \
                 refusing to sync controlled apps"
                    .to_owned(),
            ),
        ),
        CloudBackend::Local { .. } | CloudBackend::Cumulus { .. } => (CloudMode::Redirect, None),
    };
    tracing::info!(
        backend = settings.backend.label(),
        mode = mode.label(),
        apps = apps.len(),
        reason = reason.as_deref().unwrap_or("-"),
        "cloud: plan resolved"
    );
    CloudPlan {
        mode,
        reason,
        backend: settings.backend.clone(),
        apps,
    }
}

#[cfg(test)]
mod tests;
