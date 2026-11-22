//! End-to-end worker tests on the build host: a synthetic PE, a hand-written
//! corpus, a fake installer, and a recording state sink.
//!
//! These are the tests the entry point cannot run: nothing here needs Win32, a
//! Steam client, or a detour. The Windows worker differs from these fakes only in
//! which implementations are handed to [`super::worker::run`].

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::hooks::state::HostState;
use crate::install::{DetourInstaller, InstallError, Installed};
use crate::module::{self, ModuleIdentity};

use super::worker::{HostEnv, LoadedModule, StateSink};

const OWNERSHIP: &str = "CUser::CheckAppOwnership";
const DEPOT: &str = "GetDecryptionKey";
const CLOUD: &str = "CloseAppCloud";
const TICKET: &str = "CUser::GetAppTicket";

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// A temporary directory that removes itself.
struct TempDir {
    root: PathBuf,
}

impl TempDir {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vf-host-windows-entry-{}-{}-{label}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create temp dir");
        Self { root }
    }

    fn path(&self) -> &Path {
        &self.root
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, text).expect("write");
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Minimal PE32+ with one `.text` section (RVA 0x1000, raw 0x200, 0x100 bytes).
fn synthetic_pe64() -> Vec<u8> {
    let mut data = vec![0u8; 0x400];
    data[0..2].copy_from_slice(b"MZ");
    data[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    let pe = 0x80;
    data[pe..pe + 4].copy_from_slice(b"PE\0\0");
    let coff = pe + 4;
    data[coff..coff + 2].copy_from_slice(&0x8664u16.to_le_bytes());
    data[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
    data[coff + 16..coff + 18].copy_from_slice(&0xF0u16.to_le_bytes());
    let optional = coff + 20;
    data[optional..optional + 2].copy_from_slice(&0x20Bu16.to_le_bytes());
    data[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
    let table = optional + 0xF0;
    data[table..table + 5].copy_from_slice(b".text");
    data[table + 8..table + 12].copy_from_slice(&0x100u32.to_le_bytes());
    data[table + 12..table + 16].copy_from_slice(&0x1000u32.to_le_bytes());
    data[table + 16..table + 20].copy_from_slice(&0x100u32.to_le_bytes());
    data[table + 20..table + 24].copy_from_slice(&0x200u32.to_le_bytes());
    data[table + 36..table + 40].copy_from_slice(&0x2000_0000u32.to_le_bytes());
    data[0x200..0x300].fill(0x90);
    data[0x210..0x214].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
    data[0x230..0x234].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    data[0x250..0x254].copy_from_slice(&[0x55, 0x66, 0x77, 0x88]);
    data[0x270..0x274].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    data
}

fn identity() -> ModuleIdentity {
    module::identify("steamclient64.dll", &synthetic_pe64()).expect("identify synthetic PE")
}

fn corpus_text(entries: &[(&str, &str)]) -> String {
    let mut text = String::from("[hotfix]\nformat = 1\narchitecture = \"x86_64\"\n\n");
    for (name, pattern) in entries {
        text.push_str(&format!(
            "[steamclient.\"{name}\"]\nfollow = \"none\"\npattern = \"{pattern}\"\n\n"
        ));
    }
    text
}

/// Write the corpus file the worker will select for the synthetic module.
fn write_corpus(root: &Path, entries: &[(&str, &str)]) -> PathBuf {
    let path = crate::install::corpus::path(root, &identity()).expect("corpus path");
    std::fs::create_dir_all(path.parent().expect("parent")).expect("create corpus dir");
    std::fs::write(&path, corpus_text(entries)).expect("write corpus");
    path
}

fn config_text(apps: &[u32]) -> String {
    let mut text = String::new();
    for id in apps {
        text.push_str(&format!("[[apps.inject]]\nid = {id}\n"));
    }
    text
}

/// Every symbol any fake installer was asked to hook, in order.
type AttemptLog = Rc<RefCell<Vec<String>>>;

struct FakeInstaller {
    attempts: AttemptLog,
    fail_on: Option<String>,
}

impl DetourInstaller for FakeInstaller {
    fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
        self.attempts.borrow_mut().push(symbol.to_owned());
        if self.fail_on.as_deref() == Some(symbol) {
            return Err(InstallError::NoReplacement {
                symbol: symbol.to_owned(),
            });
        }
        Ok(Installed {
            symbol: symbol.to_owned(),
            address,
            trampoline: None,
        })
    }
}

/// The installer factory the worker calls once per module.
fn factory(
    attempts: &AttemptLog,
    fail_on: Option<&str>,
) -> impl FnMut() -> Box<dyn DetourInstaller> {
    let attempts = Rc::clone(attempts);
    let fail_on = fail_on.map(str::to_owned);
    move || {
        Box::new(FakeInstaller {
            attempts: Rc::clone(&attempts),
            fail_on: fail_on.clone(),
        })
    }
}

/// The environment the worker reads, with every part replaceable.
struct FakeEnv {
    config: Option<PathBuf>,
    corpus: Option<PathBuf>,
    depot: Option<PathBuf>,
    modules: Vec<LoadedModule>,
    lua: Vec<String>,
}

impl FakeEnv {
    fn new(config: Option<PathBuf>, corpus: Option<PathBuf>) -> Self {
        Self {
            config,
            corpus,
            depot: None,
            lua: Vec::new(),
            modules: vec![LoadedModule {
                name: "steamclient64.dll".to_owned(),
                base: 0,
                file: synthetic_pe64(),
            }],
        }
    }
}

impl HostEnv for FakeEnv {
    fn config_path(&self) -> Option<PathBuf> {
        self.config.clone()
    }

    fn corpus_root(&self) -> Option<PathBuf> {
        self.corpus.clone()
    }

    fn depotcache_dir(&self) -> Option<PathBuf> {
        self.depot.clone()
    }

    fn ticket_dir(&self) -> Option<PathBuf> {
        None
    }

    fn lua_dirs(&self) -> Vec<String> {
        self.lua.clone()
    }

    fn steam_modules(&mut self) -> Vec<LoadedModule> {
        std::mem::take(&mut self.modules)
    }
}

#[derive(Default)]
struct RecordingSink {
    published: Option<Arc<HostState>>,
    failure: Option<String>,
}

impl StateSink for RecordingSink {
    fn publish(&mut self, state: Arc<HostState>) -> Result<(), String> {
        match &self.failure {
            Some(reason) => Err(reason.clone()),
            None => {
                self.published = Some(state);
                Ok(())
            }
        }
    }
}

mod flow;
