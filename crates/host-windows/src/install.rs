//! Resolve the Windows pattern corpus into hook addresses, install them, and
//! report what happened.
//!
//! The contract is the same one the rest of the Windows host follows: a missing
//! corpus or a failed hook disables only the capabilities that need it. Nothing
//! here panics on bad input, and a missing file never starts the install batch.

pub mod corpus;
pub mod deps;
pub mod installer;
mod run;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use thiserror::Error;
use vapor_forge_patterns::pe_image::PeImage;
use vapor_forge_patterns::registry::parse_toml_patterns;
use vapor_forge_patterns::scan::{scan_code_segment, PatternRef};

use crate::capability::Gate;
use crate::module::ModuleIdentity;
use deps::capabilities_for_symbol;
pub use installer::{DetourInstaller, InstallError, Installed};
pub use run::{run, RunReport};

#[derive(Debug, Error)]
pub enum PlanError {
    #[error("no pattern corpus for {fingerprint}: expected {path}: {source}")]
    ReadCorpus {
        fingerprint: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("pattern corpus {path} for {fingerprint} is invalid: {message}")]
    ParseCorpus {
        fingerprint: String,
        path: PathBuf,
        message: String,
    },
    #[error("{module} cannot be scanned: {message}")]
    Image { module: String, message: String },
    #[error("module {fingerprint} has no Windows pattern component")]
    UnknownModule { fingerprint: String },
}

/// One resolved or failed symbol from the batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SymbolPlan {
    Resolved { name: String, offset: usize },
    Unresolved { name: String, reason: String },
}

impl SymbolPlan {
    pub fn name(&self) -> &str {
        match self {
            Self::Resolved { name, .. } | Self::Unresolved { name, .. } => name,
        }
    }
}

/// Every symbol in the selected corpus, resolved against one module image.
#[derive(Clone, Debug)]
pub struct BatchPlan {
    pub module: String,
    /// RVA of the executable section the offsets are relative to.
    pub code_vaddr: u64,
    pub symbols: Vec<SymbolPlan>,
}

impl BatchPlan {
    pub fn resolved(&self) -> impl Iterator<Item = (&str, usize)> {
        self.symbols.iter().filter_map(|symbol| match symbol {
            SymbolPlan::Resolved { name, offset } => Some((name.as_str(), *offset)),
            SymbolPlan::Unresolved { .. } => None,
        })
    }

    pub fn unresolved(&self) -> impl Iterator<Item = (&str, &str)> {
        self.symbols.iter().filter_map(|symbol| match symbol {
            SymbolPlan::Unresolved { name, reason } => Some((name.as_str(), reason.as_str())),
            SymbolPlan::Resolved { .. } => None,
        })
    }
}

/// Select the corpus for `identity`, parse it and resolve every pattern.
pub fn plan_batch(
    identity: &ModuleIdentity,
    image: &[u8],
    corpus_root: &Path,
) -> Result<BatchPlan, PlanError> {
    let (path, text) = corpus::read(corpus_root, identity)?;
    let entries = parse_toml_patterns(&text).map_err(|message| PlanError::ParseCorpus {
        fingerprint: corpus::fingerprint(identity),
        path: path.clone(),
        message,
    })?;
    let patterns: Vec<PatternRef<'_>> = entries.iter().map(PatternRef::from).collect();

    let pe = PeImage::parse(image).map_err(|message| PlanError::Image {
        module: identity.name.clone(),
        message,
    })?;
    let segment = pe.code_segment().map_err(|message| PlanError::Image {
        module: identity.name.clone(),
        message,
    })?;
    let kind = if pe.bits() == 64 { "PE64" } else { "PE32" };
    let report = scan_code_segment(&identity.name, &path, &segment, &patterns, None, kind)
        .map_err(|message| PlanError::ParseCorpus {
            fingerprint: corpus::fingerprint(identity),
            path: path.clone(),
            message,
        })?;

    let symbols: Vec<SymbolPlan> = report
        .entries
        .into_iter()
        .map(|entry| match entry.target_offset {
            Some(offset) => SymbolPlan::Resolved {
                name: entry.name,
                offset,
            },
            None => {
                let reason = entry
                    .error
                    .unwrap_or_else(|| format!("pattern status {}", entry.status.label()));
                SymbolPlan::Unresolved {
                    name: entry.name,
                    reason,
                }
            }
        })
        .collect();
    tracing::info!(
        module = identity.name.as_str(),
        fingerprint = corpus::fingerprint(identity).as_str(),
        symbols = symbols.len(),
        "windows pattern corpus planned"
    );
    Ok(BatchPlan {
        module: identity.name.clone(),
        code_vaddr: segment.vaddr,
        symbols,
    })
}

/// A failed or unresolved symbol, kept for the human-facing report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub symbol: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct InstallOutcome {
    pub installed: Vec<Installed>,
    pub failures: Vec<Failure>,
}

/// Install every resolved symbol. One failure never aborts the rest of the
/// batch; it disables the capabilities that depend on that symbol.
pub fn install_plan(
    plan: &BatchPlan,
    module_base: usize,
    installer: &mut dyn DetourInstaller,
    gate: &mut Gate,
) -> InstallOutcome {
    let mut outcome = InstallOutcome::default();
    let segment_base = module_base.saturating_add(plan.code_vaddr as usize);
    for symbol in &plan.symbols {
        match symbol {
            SymbolPlan::Resolved { name, offset } => {
                let address = segment_base.saturating_add(*offset);
                match installer.install(name, address) {
                    Ok(installed) => {
                        tracing::info!(
                            symbol = name.as_str(),
                            address = format_args!("{address:#x}"),
                            "windows hook installed"
                        );
                        outcome.installed.push(installed);
                    }
                    Err(error) => {
                        let reason = format!("hook {name} failed to install: {error}");
                        tracing::warn!(symbol = name.as_str(), reason = reason.as_str());
                        disable_dependents(gate, name, reason.clone());
                        outcome.failures.push(Failure {
                            symbol: name.clone(),
                            reason,
                        });
                    }
                }
            }
            SymbolPlan::Unresolved { name, reason } => {
                let reason = format!("hook {name} did not resolve: {reason}");
                tracing::warn!(symbol = name.as_str(), reason = reason.as_str());
                disable_dependents(gate, name, reason.clone());
                outcome.failures.push(Failure {
                    symbol: name.clone(),
                    reason,
                });
            }
        }
    }
    outcome
}

pub(crate) fn disable_dependents(gate: &mut Gate, symbol: &str, reason: String) {
    let capabilities = capabilities_for_symbol(symbol);
    if capabilities.is_empty() {
        tracing::debug!(
            symbol,
            reason = reason.as_str(),
            "hook has no capability dependency; nothing to disable"
        );
        return;
    }
    for &capability in capabilities {
        gate.disable(capability, reason.clone());
    }
}
