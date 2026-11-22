//! One-call orchestration: plan, install, report.

use std::path::Path;

use crate::capability::{Capability, Gate};
use crate::module::ModuleIdentity;

use super::installer::{DetourInstaller, Installed};
use super::{corpus, install_plan, plan_batch, BatchPlan, Failure};

/// Human-facing summary of one hook batch.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub fingerprint: String,
    pub plan: Option<BatchPlan>,
    pub gate: Gate,
    pub installed: Vec<Installed>,
    pub failures: Vec<Failure>,
}

impl RunReport {
    /// Installed symbols, then disabled capabilities and why (via
    /// [`Gate::report`]).
    pub fn report(&self) -> String {
        let mut out = format!(
            "Windows hook installation for {}:\n\n  installed: {}\n",
            self.fingerprint,
            self.installed.len()
        );
        for installed in &self.installed {
            out.push_str(&format!(
                "    - {} at {:#x}\n",
                installed.symbol, installed.address
            ));
        }
        for failure in &self.failures {
            out.push_str(&format!("  not installed: {}\n", failure.reason));
        }
        out.push('\n');
        out.push_str(&self.gate.report());
        out
    }
}

/// Run corpus selection, resolution, installation and reporting in one call.
///
/// A corpus-level error disables every capability and returns before the
/// installer is touched: half a batch is worse than no batch.
pub fn run(
    identity: &ModuleIdentity,
    image: &[u8],
    corpus_root: &Path,
    module_base: usize,
    installer: &mut dyn DetourInstaller,
) -> RunReport {
    let fingerprint = corpus::fingerprint(identity);
    let mut gate = Gate::all_enabled();
    let plan = match plan_batch(identity, image, corpus_root) {
        Ok(plan) => plan,
        Err(error) => {
            let reason = error.to_string();
            tracing::warn!(
                fingerprint = fingerprint.as_str(),
                reason = reason.as_str(),
                "windows hook batch disabled before installation"
            );
            disable_all(&mut gate, &reason);
            return RunReport {
                fingerprint,
                plan: None,
                gate,
                installed: Vec::new(),
                failures: Vec::new(),
            };
        }
    };
    let outcome = install_plan(&plan, module_base, installer, &mut gate);
    RunReport {
        fingerprint,
        plan: Some(plan),
        gate,
        installed: outcome.installed,
        failures: outcome.failures,
    }
}

fn disable_all(gate: &mut Gate, reason: &str) {
    for capability in Capability::ALL {
        gate.disable(capability, reason);
    }
}
