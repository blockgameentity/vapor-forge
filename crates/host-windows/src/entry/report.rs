//! The report bundle: one text file a user can attach to an issue.
//!
//! Contents, in order: the run's environment notes, each module's install report
//! (`install::RunReport::report`, which already names every installed symbol and
//! every disabled capability with its reason), and the feature pass. No ticket
//! material, no config secrets: the feature report is built from presence flags,
//! and the install report names symbols and addresses only.
//!
//! Writing it is best effort. A host that cannot write its report is still a
//! working host, so the failure is logged with the exact path and the run
//! continues.

use std::path::{Path, PathBuf};

use crate::features::FeatureReport;
use tracing::{info, warn};

use crate::capability::Gate;
use crate::entry::worker::ModuleOutcome;
use crate::HostError;

/// Render the whole bundle.
pub fn render(
    modules: &[ModuleOutcome],
    gate: &Gate,
    features: Option<&FeatureReport>,
    notes: &[String],
) -> String {
    let mut out = String::from("vapor-forge Windows host report\n");
    out.push_str("================================\n\n");
    if notes.is_empty() {
        out.push_str("environment: no problems recorded\n");
    } else {
        out.push_str("environment:\n");
        for note in notes {
            out.push_str(&format!("  - {note}\n"));
        }
    }
    out.push('\n');

    if modules.is_empty() {
        out.push_str("no Steam module was found in this process\n\n");
    }
    for module in modules {
        out.push_str(&format!("module {}:\n", module.name));
        if let Some(error) = &module.error {
            out.push_str(&format!("  not installed: {error}\n"));
        }
        if let Some(report) = &module.report {
            for line in report.report().lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push('\n');
    }

    if let Some(features) = features {
        out.push_str(&features.report());
        out.push('\n');
    }
    out.push_str("capabilities after the install pass:\n");
    out.push_str(&gate.report());
    out.push('\n');
    out
}

/// Write `text` to `path`, creating the parent directory.
pub fn write(path: &Path, text: &str) -> Result<PathBuf, HostError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| HostError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(path, text).map_err(|source| HostError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(path.to_path_buf())
}

/// Render and write in one step; never fails the run.
pub fn write_best_effort(path: Option<&Path>, text: &str) -> Option<PathBuf> {
    let path = path?;
    match write(path, text) {
        Ok(written) => {
            info!(path = %written.display(), "report bundle written");
            Some(written)
        }
        Err(error) => {
            warn!(
                path = %path.display(),
                reason = error.to_string().as_str(),
                "report bundle could not be written; the run continues"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::worker::HostOutcome;

    fn outcome() -> HostOutcome {
        let mut gate = Gate::all_enabled();
        gate.disable(
            crate::capability::Capability::Ownership,
            "CUser::CheckAppOwnership did not resolve",
        );
        HostOutcome {
            modules: vec![ModuleOutcome {
                name: "steamclient64.dll".to_owned(),
                report: None,
                error: Some("no corpus directory resolved".to_owned()),
            }],
            gate,
            features: None,
            notes: vec!["config not loaded: /nope/config.toml".to_owned()],
        }
    }

    #[test]
    fn the_bundle_names_the_module_and_every_disabled_capability() {
        let text = outcome().report();
        assert!(text.contains("module steamclient64.dll:"));
        assert!(text.contains("no corpus directory resolved"));
        assert!(text.contains("config not loaded: /nope/config.toml"));
        assert!(text.contains("ownership: CUser::CheckAppOwnership did not resolve"));
    }

    #[test]
    fn writing_creates_the_parent_directory() {
        let root = std::env::temp_dir().join("vf-host-windows-report/reports");
        let path = root.join("windows-host.txt");
        let written = write(&path, "hello").expect("write");
        assert_eq!(std::fs::read_to_string(&written).expect("read"), "hello");
        let _ = std::fs::remove_dir_all(root.parent().expect("parent"));
    }

    #[test]
    fn an_unwritable_path_is_reported_and_does_not_panic() {
        let path = Path::new("/proc/definitely/not/writable/report.txt");
        assert!(write_best_effort(Some(path), "x").is_none());
        assert!(write_best_effort(None, "x").is_none());
    }
}
