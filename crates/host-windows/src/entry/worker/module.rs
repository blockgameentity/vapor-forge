//! One module's install batch: plan resolved symbols against the corpus, skip the
//! ones this host has no body for, and install the rest.

use std::path::Path;

use tracing::{info, warn};

use crate::capability::Gate;
use crate::entry::worker::{disable_all, InstallerFactory, LoadedModule, ModuleOutcome};
use crate::install::{self, BatchPlan, RunReport, SymbolPlan};
use crate::module;

/// Plan and install one module's batch.
///
/// Symbols the host has no replacement body for are skipped rather than failed:
/// their capabilities are disabled with a reason that says so, which is the
/// difference between "we have not written this hook yet" and "the patch broke".
pub fn run_module(
    loaded: &LoadedModule,
    corpus_root: Option<&Path>,
    installer: &mut InstallerFactory<'_>,
) -> ModuleOutcome {
    let mut gate = Gate::all_enabled();
    let identity = match module::identify(&loaded.name, &loaded.file) {
        Ok(identity) => identity,
        Err(error) => {
            let reason = format!("{} cannot be identified: {error}", loaded.name);
            warn!(module = loaded.name.as_str(), reason = reason.as_str());
            return ModuleOutcome {
                name: loaded.name.clone(),
                report: None,
                error: Some(reason),
            };
        }
    };
    let fingerprint = install::corpus::fingerprint(&identity);
    let Some(corpus_root) = corpus_root else {
        return ModuleOutcome {
            name: loaded.name.clone(),
            report: None,
            error: Some(format!("{fingerprint}: no corpus directory resolved")),
        };
    };

    let plan = match install::plan_batch(&identity, &loaded.file, corpus_root) {
        Ok(plan) => plan,
        Err(error) => {
            let reason = error.to_string();
            warn!(fingerprint = fingerprint.as_str(), reason = reason.as_str());
            disable_all(&mut gate, &reason);
            return ModuleOutcome {
                name: loaded.name.clone(),
                report: Some(RunReport {
                    fingerprint,
                    plan: None,
                    gate,
                    installed: Vec::new(),
                    failures: Vec::new(),
                    features: None,
                }),
                error: Some(reason),
            };
        }
    };

    let (implemented, skipped): (Vec<SymbolPlan>, Vec<String>) = plan.symbols.iter().fold(
        (Vec::new(), Vec::new()),
        |(mut implemented, mut skipped), symbol| {
            if crate::hooks::replacement_address(symbol.name()).is_some() {
                implemented.push(symbol.clone());
            } else {
                skipped.push(symbol.name().to_owned());
            }
            (implemented, skipped)
        },
    );
    for name in &skipped {
        let reason = format!("{name} resolved, but this host has no replacement body for it yet");
        info!(
            symbol = name.as_str(),
            "hook symbol has no replacement body"
        );
        install::disable_dependents(&mut gate, name, reason);
    }

    let batch = BatchPlan {
        module: plan.module.clone(),
        code_vaddr: plan.code_vaddr,
        symbols: implemented,
    };
    let mut detours = installer();
    let outcome = install::install_plan(&batch, loaded.base, detours.as_mut(), &mut gate);
    info!(
        module = loaded.name.as_str(),
        fingerprint = fingerprint.as_str(),
        installed = outcome.installed.len(),
        failed = outcome.failures.len(),
        "module batch finished"
    );
    ModuleOutcome {
        name: loaded.name.clone(),
        report: Some(RunReport {
            fingerprint,
            plan: Some(plan),
            gate,
            installed: outcome.installed,
            failures: outcome.failures,
            features: None,
        }),
        error: None,
    }
}
