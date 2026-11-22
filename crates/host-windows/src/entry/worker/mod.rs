//! The worker the DLL entry point spawns, and the same worker the tests drive.
//!
//! One pass, in this order:
//!
//! 1. load the backend's config (the same `config.toml` the Linux host reads)
//!    and resolve the host settings from it;
//! 2. publish the host state *before* anything is patched, so a hook body that
//!    is called while the batch installs sees the whole configuration rather
//!    than half of it;
//! 3. identify every Steam module that is loaded, resolve its corpus, and run
//!    its install batch — a module that fails disables only its own
//!    capabilities;
//! 4. tighten the gate to the install result, run the one host feature pass, and
//!    render + write the report bundle.
//!
//! None of it panics: every step is a `Result`/`Option` and every failure either
//! disables a capability through [`Gate`] or is recorded in the report. That is
//! the whole reason the flow is a plain function over three traits — the Windows
//! entry point supplies real implementations, and the tests supply fakes for
//! every one of them.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use tracing::{debug, info, warn};
use vapor_forge_config::RuntimeConfig;
use vapor_forge_config::AppId;
use vapor_forge_features::config_apply;
use vapor_forge_features::package::controlled_app_ids;

use crate::capability::{Capability, Gate};
use crate::entry::report;
use crate::features::{self, FeatureReport, HostSettings};
use crate::hooks::depot_store::DepotPlan;
use crate::hooks::state::HostState;
use crate::install::{DetourInstaller, RunReport};

pub mod module;

/// One Steam module as the host found it: its file name, its load address in
/// this process, and its bytes as shipped on disk.
///
/// The bytes are read from disk rather than from the mapped image because the
/// corpus key is the sha256 of the *file* and [`install::plan_batch`] walks the
/// PE section table by file offset. A mapped image has neither property.
pub struct LoadedModule {
    pub name: String,
    pub base: usize,
    pub file: Vec<u8>,
}

/// Everything the worker needs from the process it runs in.
pub trait HostEnv {
    /// The backend's `config.toml`.
    fn config_path(&self) -> Option<PathBuf>;
    /// The directory that holds `win64/<component>/<sha256>.toml`.
    fn corpus_root(&self) -> Option<PathBuf>;
    /// `<steam>/depotcache`, when it could be resolved.
    fn depotcache_dir(&self) -> Option<PathBuf>;
    /// Where ticket material may be cached.
    fn ticket_dir(&self) -> Option<PathBuf>;
    /// `<steam>/config/lua`, when it exists: the scripts the host tool writes
    /// (addappid/setmanifestid/setAppTicket and friends).
    ///
    /// A missing directory is `None`, not an error: a machine with no Lua is a
    /// machine where the config alone decides.
    fn lua_dirs(&self) -> Vec<String>;
    /// The Steam modules currently loaded, at most one per component.
    fn steam_modules(&mut self) -> Vec<LoadedModule>;
}

/// Where the published [`HostState`] goes: the process-global slot in the real
/// host, a recording fake in tests.
pub trait StateSink {
    fn publish(&mut self, state: Arc<HostState>) -> Result<(), String>;
}

/// The installer a batch can use. Boxed and process-owned: see
/// [`crate::hooks::installer`].
pub type InstallerFactory<'a> = dyn FnMut() -> Box<dyn DetourInstaller + 'a> + 'a;

/// What happened to one module.
pub struct ModuleOutcome {
    pub name: String,
    /// `None` when the module could not be planned at all.
    pub report: Option<RunReport>,
    pub error: Option<String>,
}

/// What happened to the whole run.
pub struct HostOutcome {
    pub modules: Vec<ModuleOutcome>,
    /// Every module's disabled capabilities, merged: the first reason wins, so
    /// the report names the original cause.
    pub gate: Gate,
    pub features: Option<FeatureReport>,
    /// Configuration and environment problems that are not capability-specific.
    pub notes: Vec<String>,
}

impl HostOutcome {
    /// The rendered report bundle, as written to disk.
    pub fn report(&self) -> String {
        report::render(
            &self.modules,
            &self.gate,
            self.features.as_ref(),
            &self.notes,
        )
    }

    pub fn failed_modules(&self) -> usize {
        self.modules.iter().filter(|m| m.error.is_some()).count()
    }
}

/// Decode the `CNetPacket` layout from this module's resolved symbols, once.
///
/// The state is published before the batch so hook bodies see the real config;
/// the offsets only exist afterwards, so the layout is filled in here. A failure
/// is a note, not an error: every message then records `NoLayout` and Steam keeps
/// its own request-code round trip.
fn publish_packet_layout(state: &HostState, report: &crate::install::RunReport, module: &[u8]) {
    if state.layout().is_some() {
        return;
    }
    let Some(plan) = report.plan.as_ref() else {
        return;
    };
    let Ok(image) = vapor_forge_patterns::pe_image::PeImage::parse(module) else {
        debug!("packet layout: module image could not be parsed");
        return;
    };
    let Ok(segment) = image.code_segment() else {
        debug!("packet layout: module has no executable segment");
        return;
    };
    let Some(offsets) = crate::hooks::netpacket::layout::offsets(plan.resolved(), &segment) else {
        debug!("packet layout: the corpus does not anchor the CNetPacket family");
        return;
    };
    match crate::hooks::netpacket::layout::decode(&segment, offsets) {
        Ok(layout) => {
            info!(
                data = layout.data,
                size = layout.size,
                "packet layout decoded; request-code serving is possible"
            );
            state.set_layout(layout);
        }
        Err(error) => warn!(
            error = error.as_str(),
            "packet layout could not be decoded; messages stay observation-only"
        ),
    }
}

/// Run the whole host flow.
pub fn run(
    env: &mut dyn HostEnv,
    installer: &mut InstallerFactory<'_>,
    sink: &mut dyn StateSink,
) -> HostOutcome {
    let mut notes = Vec::new();
    let config = load_config(env, &mut notes);
    // The one place user-visible flags reach the feature modules: `[apps.shared]`
    // decides lender validation and marker cleanup, `[achievements]` the local
    // rebuild. The hook bodies read the applied value from the host state, never
    // from a second copy of the config.
    let applied = config_apply::apply(&config);

    // Lua is how the host tool expresses per-app intent (addappid, manifests,
    // tickets, purchase times). Loading it before the state is published means a
    // hook never observes an empty script state that later fills in.
    let lua_dirs = env.lua_dirs();
    let scripts = vapor_forge_scripting::execute_scripts_runtime(&lua_dirs);
    let script = &scripts.state;
    notes.push(format!(
        "lua: {} dir(s), {} app(s), {} depot key(s), {} manifest(s), {} ticket(s)",
        lua_dirs.len(),
        script.apps.len(),
        script.depot_keys.len(),
        script.manifests.len(),
        script.app_tickets.len()
    ));

    let mut settings = HostSettings::resolve(&config, script);
    settings.achievements.local_rebuild = applied.achievements_local_rebuild;

    let script_apps: HashSet<AppId> = script.apps.iter().copied().collect();
    let controlled = controlled_app_ids(&config, &script_apps);
    let lua_tickets = script
        .app_tickets
        .iter()
        .map(|(app, ticket)| (*app, ticket.clone()))
        .collect();
    let state = Arc::new(
        HostState::new(
            config,
            applied.family,
            controlled,
            env.ticket_dir(),
            DepotPlan::new(env.depotcache_dir(), Vec::new()),
        )
        .with_lua_tickets(lua_tickets)
        // The four inputs the hook bodies cannot derive themselves: the cloud plan
        // (one decision, made once), the Lua depot keys the shader decision needs,
        // the per-game ids the SteamID decision presents, and the achievement flag
        // the stats-merge observation reports.
        .with_cloud_plan(crate::features::plan_cloud(&settings.cloud))
        .with_depot_keys(script.depot_keys.clone())
        .with_identity_settings(settings.identity.clone())
        // The stats-merge hook consults the flag this pass applied, so the
        // resolved setting travels with the state instead of being re-read from
        // the config inside a hook body on Steam's thread.
        .with_achievement_settings(settings.achievements),
    );
    // Published before any entry is patched: a body reached during the batch
    // sees the real configuration, and the gate is tightened as soon as the
    // install result is known.
    if let Err(reason) = sink.publish(Arc::clone(&state)) {
        warn!(
            reason = reason.as_str(),
            "host state could not be published"
        );
        notes.push(format!("host state was not published: {reason}"));
    }

    let corpus_root = env.corpus_root();
    let modules = env.steam_modules();
    if modules.is_empty() {
        let reason = "no Steam module (steamclient64.dll, SteamUI.dll) is loaded in this process";
        warn!("{reason}");
        notes.push(reason.to_owned());
    }

    let mut outcomes = Vec::with_capacity(modules.len());
    let mut merged = Gate::all_enabled();
    for loaded in &modules {
        if crate::thread::stop_requested() {
            let reason = "the host was asked to stop before this module's batch started";
            warn!("{reason}");
            notes.push(reason.to_owned());
            break;
        }
        let outcome = module::run_module(loaded, corpus_root.as_deref(), installer);
        if let Some(report) = &outcome.report {
            publish_packet_layout(&state, report, &loaded.file);
            for (capability, reason) in report.gate.disabled() {
                merged.disable(capability, reason.to_owned());
            }
        }
        if let Some(error) = &outcome.error {
            disable_all(&mut merged, error);
        }
        outcomes.push(outcome);
    }
    if corpus_root.is_none() {
        let reason = "no pattern corpus directory (win64/…) could be resolved next to the host";
        disable_all(&mut merged, reason);
        notes.push(reason.to_owned());
    }
    state.set_gate(merged.clone());

    // One feature pass for the whole process: the install gate is now final, and
    // no archive fetcher is wired, so `features::run` reports the depotcache
    // pre-seed as inactive with that reason instead of dialling anything.
    //
    // The feature report keeps its *own* gate and is deliberately not merged into
    // the capability gate: an inactive feature ("no per-game SteamID override is
    // configured") is not a broken hook, and merging would disable the capability
    // the installed body needs. `lifecycle::run_host` keeps the two apart for the
    // same reason.
    let feature_report = features::run(&settings, &merged, None);
    state.set_gate(merged.clone());

    let outcome = HostOutcome {
        modules: outcomes,
        gate: merged,
        features: Some(feature_report),
        notes,
    };
    log_summary(&outcome);
    outcome
}

fn load_config(env: &dyn HostEnv, notes: &mut Vec<String>) -> RuntimeConfig {
    let Some(path) = env.config_path() else {
        let reason = "the backend config path could not be resolved";
        warn!("{reason}");
        notes.push(reason.to_owned());
        return RuntimeConfig::default();
    };
    match RuntimeConfig::load(&path) {
        Ok(config) => {
            info!(path = %path.display(), "host config loaded");
            config
        }
        Err(error) => {
            let reason = format!("{}: {error}", path.display());
            warn!(reason = reason.as_str(), "host config could not be loaded");
            notes.push(format!(
                "config not loaded ({reason}); every hook falls through"
            ));
            RuntimeConfig::default()
        }
    }
}

pub(crate) fn disable_all(gate: &mut Gate, reason: &str) {
    for capability in Capability::ALL {
        gate.disable(capability, reason);
    }
}

fn log_summary(outcome: &HostOutcome) {
    let installed: usize = outcome
        .modules
        .iter()
        .filter_map(|module| module.report.as_ref())
        .map(|report| report.installed.len())
        .sum();
    info!(
        modules = outcome.modules.len(),
        failed_modules = outcome.failed_modules(),
        installed,
        disabled = outcome.gate.disabled().count(),
        "windows host run finished"
    );
}
