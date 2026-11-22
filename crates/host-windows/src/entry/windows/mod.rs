//! The real environment for the worker: this process's paths and modules.
//!
//! Everything here is Win32 + filesystem and nothing else; the flow itself lives
//! in [`crate::entry::worker`], which is why the flow can be tested on Linux.
//!
//! ## Where each path comes from, and how sure that is
//!
//! | path | rule | confidence |
//! | --- | --- | --- |
//! | host module path | `GetModuleHandleExW(FROM_ADDRESS)` + `GetModuleFileNameW` | measured API, not observed in Steam |
//! | config | `%XDG_CONFIG_HOME%`/`%USERPROFILE%\.config\vapor-forge\config.toml` | **assumed** to match the backend |
//! | corpus | `<host dir>/patterns`, `<host dir>/res/patterns`, `<host dir>/../res/patterns`, `<config dir>/patterns` — first that holds `win64/` | **assumed**; the deployed layout is not documented |
//! | depotcache | `<dir of steamclient64.dll>/depotcache` | **assumed**; Steam's own layout |
//! | tickets | `<config dir>/tickets` | this host's own choice |
//!
//! A module's bytes come from its file on disk, not from the mapped image: the
//! corpus is keyed on the file's sha256 and the PE section table is walked by file
//! offset. The mapped head is still read once, to check that the image loaded in
//! this process is the same bitness as the file — a Steam update replacing the
//! file under a running client would otherwise install hooks into offsets that no
//! longer mean anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::{info, warn};
use vapor_forge_pe::pe_kind;

pub mod winapi;

use winapi::{module_path_by_name, own_module_path};

use crate::entry::paths;
use crate::entry::worker::{HostEnv, LoadedModule, StateSink};
use crate::hooks::state::HostState;
use crate::inject;

/// Bytes of a loaded module read from memory for the sanity check. Large enough
/// for the DOS header, the PE signature, the COFF header and a PE32+ optional
/// header (0x40 + 4 + 20 + 0xF0), which is all it is used for.
const IMAGE_HEAD_LEN: usize = 0x400;

/// How many times the host looks for a Steam module before giving up, and how
/// long it waits between attempts.
///
/// A DLL loaded into `steam.exe` may run before `steamclient64.dll` is mapped, so
/// the worker waits for it once, briefly. **Known limitation:** this is a poll,
/// not a loader notification; a module loaded later than this window is missed
/// for the process lifetime, and the report says so.
const MODULE_WAIT_ATTEMPTS: usize = 20;
const MODULE_WAIT_STEP: std::time::Duration = std::time::Duration::from_millis(500);

/// Module file names the host knows how to hook.
pub const STEAM_MODULES: [&str; 2] = ["steamclient64.dll", "SteamUI.dll"];

/// The Windows environment the worker reads.
pub struct WindowsEnv {
    config_dir: Option<PathBuf>,
    corpus_root: Option<PathBuf>,
    host_dir: Option<PathBuf>,
}

impl WindowsEnv {
    /// Resolve every path once, logging which rule produced each of them.
    pub fn new() -> Self {
        let config_dir = paths::config_dir();
        let host_dir = own_module_path().and_then(|path| path.parent().map(Path::to_path_buf));
        let mut candidates = host_dir
            .as_deref()
            .map(paths::corpus_candidates)
            .unwrap_or_default();
        if let Some(dir) = &config_dir {
            candidates.push(dir.join("patterns"));
        }
        let corpus_root = paths::first_corpus(&candidates);
        info!(
            host_dir = host_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            config_dir = config_dir
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            corpus_root = corpus_root
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            "windows host paths resolved"
        );
        Self {
            config_dir,
            corpus_root,
            host_dir,
        }
    }

    /// Where the report bundle is written, when the config directory is known.
    pub fn report_path(&self) -> Option<PathBuf> {
        self.config_dir.as_deref().map(paths::report_path_in)
    }

    /// Where the log file is written, when the config directory is known.
    pub fn log_path(&self) -> Option<PathBuf> {
        self.config_dir
            .as_deref()
            .map(|dir| dir.join("logs").join("windows-host.log"))
    }

    /// The directory this DLL sits in.
    pub fn host_dir(&self) -> Option<&Path> {
        self.host_dir.as_deref()
    }

    fn resolve_depotcache_dir(&self) -> Option<PathBuf> {
        let steamclient = module_path_by_name("steamclient64.dll")?;
        let root = steamclient.parent()?;
        Some(paths::depotcache_dir(root))
    }
}

impl Default for WindowsEnv {
    fn default() -> Self {
        Self::new()
    }
}

impl HostEnv for WindowsEnv {
    fn config_path(&self) -> Option<PathBuf> {
        self.config_dir.as_deref().map(paths::config_path_in)
    }

    fn corpus_root(&self) -> Option<PathBuf> {
        self.corpus_root.clone()
    }

    fn depotcache_dir(&self) -> Option<PathBuf> {
        self.resolve_depotcache_dir()
    }

    fn ticket_dir(&self) -> Option<PathBuf> {
        self.config_dir.as_deref().map(paths::ticket_dir_in)
    }

    fn steam_modules(&mut self) -> Vec<LoadedModule> {
        let mut found = Vec::new();
        for attempt in 0..MODULE_WAIT_ATTEMPTS {
            for name in STEAM_MODULES {
                if crate::thread::stop_requested() {
                    warn!("stop was requested while waiting for Steam modules");
                    return found;
                }
                if found
                    .iter()
                    .any(|module: &LoadedModule| module.name == name)
                {
                    continue;
                }
                if let Some(module) = load_module(name) {
                    found.push(module);
                }
            }
            if !found.is_empty() {
                return found;
            }
            if attempt + 1 < MODULE_WAIT_ATTEMPTS {
                std::thread::sleep(MODULE_WAIT_STEP);
            }
        }
        found
    }
}

/// Describe one loaded Steam module, or `None` when it is not loaded.
fn load_module(name: &str) -> Option<LoadedModule> {
    let base = match inject::module_base(name) {
        Ok(base) => base,
        Err(_) => return None,
    };
    let path = module_path_by_name(name)?;
    let file = match std::fs::read(&path) {
        Ok(file) => file,
        Err(error) => {
            warn!(module = name, path = %path.display(), %error, "module file could not be read");
            return None;
        }
    };
    let head = match inject::read_module_head(name, IMAGE_HEAD_LEN) {
        Ok(head) => head,
        Err(error) => {
            warn!(
                module = name,
                reason = error.to_string().as_str(),
                "module head could not be read"
            );
            return None;
        }
    };
    if pe_kind(&head).is_none() || pe_kind(&head) != pe_kind(&file) {
        warn!(
            module = name,
            path = %path.display(),
            "the loaded image does not match the file on disk; refusing to plan hooks for it"
        );
        return None;
    }
    info!(
        module = name,
        base = format_args!("{base:#x}"),
        path = %path.display(),
        bytes = file.len(),
        "steam module identified"
    );
    Some(LoadedModule {
        name: name.to_owned(),
        base,
        file,
    })
}

/// The process-global sink: publishes the one host state a process may have.
struct GlobalSink;

impl StateSink for GlobalSink {
    fn publish(&mut self, state: Arc<HostState>) -> Result<(), String> {
        crate::hooks::state::publish(state)
    }
}

/// The whole host, on the worker thread.
///
/// Called once per process by [`crate::entry::dll`]. Never returns an error: every
/// failure is folded into the report the run writes.
pub fn run_worker() {
    let mut env = WindowsEnv::new();
    crate::entry::log::install(env.log_path().as_deref());
    info!(
        worker = format_args!("{:#x}", crate::entry::dll::worker_handle()),
        "windows host worker started"
    );

    let mut installer = crate::hooks::installer::installer_factory();
    let mut sink = GlobalSink;
    let outcome = crate::entry::worker::run(&mut env, &mut installer, &mut sink);
    let text = outcome.report();
    crate::entry::report::write_best_effort(env.report_path().as_deref(), &text);
    info!("windows host worker finished");
}
