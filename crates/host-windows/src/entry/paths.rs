//! Where the Windows host looks for its config, its corpus, the ticket folder and
//! the depot cache.
//!
//! These are *paths*, not settings: every setting comes from
//! `vapor_forge_config::RuntimeConfig`, loaded from the file resolved here. The
//! rules are deliberately the ones the Linux host already uses, so both hosts read
//! the same `config.toml`.
//!
//! Every function that touches the environment has a `_for` twin taking the
//! environment as arguments, so the rules are unit-tested without setting process
//! globals.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// File name of the backend configuration.
pub const CONFIG_FILE: &str = "config.toml";

/// Directory name the backend owns inside the platform config root.
const CONFIG_DIR_NAME: &str = "vapor-forge";

/// The config directory, from the two variables that can name a config root.
///
/// `XDG_CONFIG_HOME` wins when it is set (the Linux convention the backend
/// already follows); otherwise `<home>/.config`. `home` is `HOME` on unix and
/// `USERPROFILE` on Windows. **Assumed:** that the Windows backend writes to the
/// same `vapor-forge/config.toml` under the user profile.
pub fn config_dir_for(xdg_config_home: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    if let Some(root) = xdg_config_home.filter(|value| !value.is_empty()) {
        return Some(Path::new(root).join(CONFIG_DIR_NAME));
    }
    let home = home.filter(|value| !value.is_empty())?;
    Some(Path::new(home).join(".config").join(CONFIG_DIR_NAME))
}

/// [`config_dir_for`] over this process's environment.
pub fn config_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    config_dir_for(
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
        home.as_deref(),
    )
}

/// The config file inside `dir`.
pub fn config_path_in(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE)
}

/// Where the report bundle goes: `<config dir>/reports/windows-host.txt`.
pub fn report_path_in(dir: &Path) -> PathBuf {
    dir.join("reports").join("windows-host.txt")
}

/// Candidate corpus roots for a host binary sitting in `host_dir`, in order.
///
/// 1. `<host dir>/patterns` — an installed layout that ships the corpus beside
///    the DLL;
/// 2. `<host dir>/res/patterns` — a build tree;
/// 3. `<host dir>/../res/patterns` — the DLL one directory down from a build
///    tree.
///
/// **Assumed:** one of these three is where the corpus ships. Nothing in the
/// repository states the deployed layout, so the resolver tries each and reports
/// which one it used.
pub fn corpus_candidates(host_dir: &Path) -> Vec<PathBuf> {
    vec![
        host_dir.join("patterns"),
        host_dir.join("res").join("patterns"),
        host_dir.join("..").join("res").join("patterns"),
    ]
}

/// First candidate that actually contains a `win64` directory.
pub fn first_corpus(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates
        .iter()
        .find(|candidate| candidate.join("win64").is_dir())
        .cloned()
}

/// `<steam>/depotcache` for a Steam installation rooted at `steam_root`.
pub fn depotcache_dir(steam_root: &Path) -> PathBuf {
    steam_root.join("depotcache")
}

/// `<config dir>/tickets`, where ticket material may be cached.
pub fn ticket_dir_in(dir: &Path) -> PathBuf {
    dir.join("tickets")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_wins_over_home() {
        let dir = config_dir_for(Some(OsStr::new("/xdg")), Some(OsStr::new("/home/u")));
        assert_eq!(dir, Some(PathBuf::from("/xdg/vapor-forge")));
    }

    #[test]
    fn without_xdg_the_backend_directory_is_under_dot_config() {
        let dir = config_dir_for(None, Some(OsStr::new("/home/u")));
        assert_eq!(dir, Some(PathBuf::from("/home/u/.config/vapor-forge")));
        assert!(config_dir_for(None, None).is_none());
        assert!(config_dir_for(Some(OsStr::new("")), None).is_none());
    }

    #[test]
    fn the_config_report_and_ticket_paths_live_in_the_config_directory() {
        let dir = Path::new("/cfg/vapor-forge");
        assert_eq!(
            config_path_in(dir),
            PathBuf::from("/cfg/vapor-forge/config.toml")
        );
        assert_eq!(
            report_path_in(dir),
            PathBuf::from("/cfg/vapor-forge/reports/windows-host.txt")
        );
        assert_eq!(
            ticket_dir_in(dir),
            PathBuf::from("/cfg/vapor-forge/tickets")
        );
        assert_eq!(
            depotcache_dir(Path::new("/steam")),
            PathBuf::from("/steam/depotcache")
        );
    }

    #[test]
    fn the_corpus_resolver_takes_a_directory_that_actually_holds_win64() {
        let root = std::env::temp_dir().join("vf-host-windows-corpus-layout");
        let installed = root.join("patterns");
        std::fs::create_dir_all(installed.join("win64/steamclient")).expect("create");
        let candidates = corpus_candidates(&root);
        assert_eq!(first_corpus(&candidates), Some(installed));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_host_directory_without_a_corpus_resolves_to_nothing() {
        let root = std::env::temp_dir().join("vf-host-windows-no-corpus-layout");
        std::fs::create_dir_all(&root).expect("create");
        assert!(first_corpus(&corpus_candidates(&root)).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
