//! The request-code lookup the dispatch hook would perform, decided from config.
//!
//! `features::request_code` owns the provider chain: which entries exist, whether
//! failover walks past a failure, per-provider pacing and cooldowns. The worker in
//! [`super::fetch`] runs that chain for real, on a thread of its own and only once
//! a code is actually missing. What this module adds is the chain's **decision**
//! projected over the config the host already holds, so the first dispatch can log
//! who would be asked before anything has been fetched:
//!
//! * `request_code::should_intercept_with_ownership` — does this controlled app
//!   need an injected code at all, or is it genuinely owned (a code would be
//!   pointless and Steam's own round trip is correct)?
//! * the configured provider order and `failover`, projected into "which provider
//!   would be asked first" so the log says who would serve.
//!
//! The projection is deliberately *weaker* than the chain and says so: the chain
//! also re-orders providers that recently served the same gid and skips ones
//! cooling down after a 429, and both tables are private to
//! `features::request_code`. So this answers "the entry the chain would try first,
//! given the config", never "the provider that would definitely serve".
//!
//! Nothing here dials, resolves a URL or reads a token: the endpoint template is
//! never expanded, because a provider URL may carry credentials in its path or
//! query and this log line is not the place for them.

use vapor_forge_config::{AppId, ManifestSection, ManifestSource, RuntimeConfig};
use vapor_forge_features::apps::OwnershipState;
use vapor_forge_features::depotcache::Candidate;
use vapor_forge_features::request_code;

use super::decision::RequestTarget;

/// Which provider the configured chain would ask first.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProviderChoice {
    /// `[manifest] providers` is empty: the chain has nothing to ask, which
    /// `features::request_code` reports as `ChainError::NoProviderConfigured`.
    NoProviderConfigured,
    /// The chain would contact `provider` first, after skipping `skipped`
    /// unusable entries.
    WouldAsk {
        provider: String,
        skipped: Vec<String>,
    },
    /// Every configured entry is unusable, so the chain would reject each one.
    NoUsableProvider { rejected: Vec<String> },
}

impl ProviderChoice {
    /// One line describing the choice, for the log and the report.
    pub fn summary(&self) -> String {
        match self {
            Self::NoProviderConfigured => {
                "no [manifest] providers entry: nothing would be asked for a request code"
                    .to_owned()
            }
            Self::WouldAsk { provider, skipped } if skipped.is_empty() => {
                format!("the provider chain would ask `{provider}` first")
            }
            Self::WouldAsk { provider, skipped } => format!(
                "the provider chain would ask `{provider}` first, after skipping {} unusable entry(ies): {}",
                skipped.len(),
                skipped.join(", ")
            ),
            Self::NoUsableProvider { rejected } => format!(
                "every configured provider entry is unusable ({}): the chain would serve nothing",
                rejected.join(", ")
            ),
        }
    }
}

/// What the lookup would do, as a pure decision over the host's config.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LookupPlan {
    /// The `(depot, gid)` pairs the host wants manifests for.
    ///
    /// From manifest request planning (`DepotPlan::wanted`); empty by default,
    /// because this host plans no depots yet.
    pub targets: Vec<RequestTarget>,
    /// Which provider the chain would ask first.
    pub provider_choice: ProviderChoice,
    /// Controlled apps that pass the ownership gate and would need a code.
    pub apps_needing_codes: Vec<AppId>,
    /// How many controlled apps were classified, so "0 of 0" is distinguishable
    /// from "0 of 3".
    pub apps_checked: usize,
}

impl LookupPlan {
    /// One line for the log: who would be asked, for how many manifests, and how
    /// many controlled apps the ownership gate still applies to.
    pub fn summary(&self) -> String {
        format!(
            "{}; {} wanted manifest(s) would be looked up; \
             {} of {} controlled app(s) need an injected code",
            self.provider_choice.summary(),
            self.targets.len(),
            self.apps_needing_codes.len(),
            self.apps_checked
        )
    }

    /// Whether this plan has anything to say at all.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty() && self.apps_needing_codes.is_empty()
    }
}

/// Build the lookup plan for `config`, `wanted` depot/manifest pairs and the
/// controlled apps the host computed at start-up.
///
/// Pure: one snapshot of config and one list of candidates, no I/O.
pub fn plan(config: &RuntimeConfig, wanted: &[Candidate], controlled: &[AppId]) -> LookupPlan {
    LookupPlan {
        targets: wanted
            .iter()
            .map(|candidate| RequestTarget {
                depot: candidate.depot_id.0,
                gid: candidate.manifest_id.0,
            })
            .collect(),
        provider_choice: provider_choice(&config.manifest),
        apps_needing_codes: apps_needing_codes(config, controlled),
        apps_checked: controlled.len(),
    }
}

fn apps_needing_codes(config: &RuntimeConfig, controlled: &[AppId]) -> Vec<AppId> {
    controlled
        .iter()
        .copied()
        .filter(|&app_id| {
            // Ownership is passed explicitly instead of read from the process-wide
            // table: the Windows host records no ownership observation
            // (`features::apps::record_original_observation` is never called here),
            // so "unknown" is the truthful input and `Unknown` is what makes a
            // controlled app need the injection. Passing it keeps this a function
            // of the snapshot a caller handed in.
            request_code::should_intercept_with_ownership(app_id, config, |_| {
                OwnershipState::Unknown
            })
        })
        .collect()
}

/// The first provider the chain would contact, or why it would contact none.
fn provider_choice(manifest: &ManifestSection) -> ProviderChoice {
    if manifest.providers.is_empty() {
        return ProviderChoice::NoProviderConfigured;
    }
    if !manifest.failover {
        // Failover off: only the head of the list is ever asked, chosen or not.
        // `features::request_code` reports a rejected head as `Rejected` and stops.
        let head = &manifest.providers[0];
        return match rejection(head) {
            Some(_) => ProviderChoice::NoUsableProvider {
                rejected: vec![head.name().to_owned()],
            },
            None => ProviderChoice::WouldAsk {
                provider: head.name().to_owned(),
                skipped: Vec::new(),
            },
        };
    }
    let mut skipped = Vec::new();
    let mut rejected = Vec::new();
    for provider in &manifest.providers {
        match rejection(provider) {
            Some(reason) => {
                rejected.push(provider.name().to_owned());
                skipped.push(format!("{} ({reason})", provider.name()));
            }
            None => {
                return ProviderChoice::WouldAsk {
                    provider: provider.name().to_owned(),
                    skipped,
                }
            }
        }
    }
    ProviderChoice::NoUsableProvider { rejected }
}

fn rejection(provider: &ManifestSource) -> Option<String> {
    provider.rejection()
}

#[cfg(test)]
mod tests;
