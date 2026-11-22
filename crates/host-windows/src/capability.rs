//! Capability gating: the "degrade, never brick" contract.
//!
//! Every behaviour the host can offer is a capability. A capability is either
//! enabled or disabled *with a reason*, and disabling one never disables
//! another. A Steam update that breaks one signature set therefore costs the
//! user exactly that feature, with a message, instead of a dead client.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Capability {
    Ownership,
    PackageInjection,
    TicketOverrides,
    DepotInjection,
    FamilyLockBypass,
    DlcOverrides,
    CmInterception,
    CloudControl,
    Achievements,
    RequestCodes,
}

impl Capability {
    pub const ALL: [Self; 10] = [
        Self::Ownership,
        Self::PackageInjection,
        Self::TicketOverrides,
        Self::DepotInjection,
        Self::FamilyLockBypass,
        Self::DlcOverrides,
        Self::CmInterception,
        Self::CloudControl,
        Self::Achievements,
        Self::RequestCodes,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Ownership => "ownership",
            Self::PackageInjection => "package-injection",
            Self::TicketOverrides => "ticket-overrides",
            Self::DepotInjection => "depot-injection",
            Self::FamilyLockBypass => "family-lock-bypass",
            Self::DlcOverrides => "dlc-overrides",
            Self::CmInterception => "cm-interception",
            Self::CloudControl => "cloud-control",
            Self::Achievements => "achievements",
            Self::RequestCodes => "request-codes",
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Which capabilities this process can honour right now, and why not.
#[derive(Clone, Debug, Default)]
pub struct Gate {
    disabled: BTreeMap<Capability, String>,
}

impl Gate {
    pub fn all_enabled() -> Self {
        Self::default()
    }

    /// Disable one capability with a human-readable reason. Idempotent: the
    /// first reason wins, so the report names the original cause.
    pub fn disable(&mut self, capability: Capability, reason: impl Into<String>) {
        self.disabled.entry(capability).or_insert_with(|| reason.into());
        tracing::warn!(
            capability = capability.label(),
            reason = self.disabled[&capability].as_str(),
            "capability disabled"
        );
    }

    pub fn is_enabled(&self, capability: Capability) -> bool {
        !self.disabled.contains_key(&capability)
    }

    pub fn reason(&self, capability: Capability) -> Option<&str> {
        self.disabled.get(&capability).map(String::as_str)
    }

    pub fn disabled(&self) -> impl Iterator<Item = (Capability, &str)> {
        self.disabled
            .iter()
            .map(|(capability, reason)| (*capability, reason.as_str()))
    }

    /// One dialog-sized summary of what is off and why.
    pub fn report(&self) -> String {
        if self.disabled.is_empty() {
            return "All Vapor Forge capabilities are active.".to_owned();
        }
        let mut out = String::from("Some Vapor Forge capabilities are disabled:\n\n");
        for (capability, reason) in self.disabled() {
            out.push_str(&format!("  - {capability}: {reason}\n"));
        }
        out.push_str(
            "\nSteam still runs normally. Update Vapor Forge, or report the Steam build \
             fingerprint from the diagnostics bundle.",
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabling_one_capability_leaves_the_others_alone() {
        let mut gate = Gate::all_enabled();
        gate.disable(Capability::Achievements, "stats signature unresolved");
        assert!(!gate.is_enabled(Capability::Achievements));
        for capability in Capability::ALL {
            if capability != Capability::Achievements {
                assert!(gate.is_enabled(capability), "{capability} must stay enabled");
            }
        }
    }

    #[test]
    fn the_first_reason_is_kept() {
        let mut gate = Gate::all_enabled();
        gate.disable(Capability::Ownership, "pattern missing");
        gate.disable(Capability::Ownership, "second guess");
        assert_eq!(gate.reason(Capability::Ownership), Some("pattern missing"));
    }

    #[test]
    fn report_names_every_disabled_capability_and_stays_calm() {
        let mut gate = Gate::all_enabled();
        gate.disable(Capability::Ownership, "steamclient64.dll signature unresolved");
        gate.disable(Capability::CloudControl, "cloud endpoint not configured");
        let report = gate.report();
        assert!(report.contains("ownership: steamclient64.dll signature unresolved"));
        assert!(report.contains("cloud-control: cloud endpoint not configured"));
        assert!(report.contains("Steam still runs normally"));
        assert_eq!(Gate::all_enabled().report(), "All Vapor Forge capabilities are active.");
    }
}
