//! End-to-end install tests on the build host: a synthetic PE plus a tiny
//! hand-written corpus, with a fake installer standing in for Win32.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::capability::Capability;
use crate::module::{self, ModuleIdentity};

use super::*;

const OWNERSHIP: &str = "CUser::CheckAppOwnership";
const DEPOT: &str = "GetDecryptionKey";
const CLOUD: &str = "CloseAppCloud";
const RECV: &str = "CCMInterface::RecvPkt";

static NEXT_CORPUS: AtomicU64 = AtomicU64::new(0);

/// A temporary corpus root that removes itself after the test.
struct TempCorpus {
    root: PathBuf,
}

impl TempCorpus {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vf-host-windows-{}-{}-{label}",
            std::process::id(),
            NEXT_CORPUS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create temp corpus root");
        Self { root }
    }

    fn write(&self, text: &str) {
        let identity = identity();
        let path = corpus::path(&self.root, &identity).expect("corpus path");
        std::fs::create_dir_all(path.parent().expect("corpus parent")).expect("create corpus dir");
        std::fs::write(path, text).expect("write corpus");
    }
}

impl Drop for TempCorpus {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[derive(Default)]
struct FakeInstaller {
    attempts: Vec<(String, usize)>,
    fail: Option<String>,
}

impl FakeInstaller {
    fn failing_on(symbol: &str) -> Self {
        Self {
            attempts: Vec::new(),
            fail: Some(symbol.to_owned()),
        }
    }
}

impl DetourInstaller for FakeInstaller {
    fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
        self.attempts.push((symbol.to_owned(), address));
        if self.fail.as_deref() == Some(symbol) {
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

#[test]
fn a_matching_pattern_resolves_and_installs_through_the_fake() {
    let corpus = TempCorpus::new("match");
    corpus.write(&corpus_text(&[(OWNERSHIP, "AA BB CC DD")]));
    let mut installer = FakeInstaller::default();

    let report = run(
        &identity(),
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );

    assert_eq!(
        report.installed,
        vec![Installed {
            trampoline: None,
            symbol: OWNERSHIP.to_owned(),
            address: 0x1010,
        }]
    );
    assert_eq!(installer.attempts, vec![(OWNERSHIP.to_owned(), 0x1010)]);
    assert!(report.gate.is_enabled(Capability::Ownership));
    let plan = report.plan.as_ref().expect("plan");
    assert_eq!(plan.resolved().collect::<Vec<_>>(), vec![(OWNERSHIP, 0x10)]);
}

#[test]
fn a_missing_corpus_disables_everything_and_does_not_start_the_batch() {
    let corpus = TempCorpus::new("missing");
    let identity = identity();
    let mut installer = FakeInstaller::default();

    let report = run(
        &identity,
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );

    assert!(installer.attempts.is_empty(), "the batch must not start");
    assert!(report.plan.is_none());
    assert_eq!(report.installed.len(), 0);
    for capability in Capability::ALL {
        assert!(
            !report.gate.is_enabled(capability),
            "{capability} must be disabled with the rest of the batch"
        );
    }
    let reason = report
        .gate
        .reason(Capability::Ownership)
        .expect("disabled reason");
    assert!(reason.contains(&identity.sha256));
    assert!(reason.contains(&format!("win64/steamclient/{}.toml", identity.sha256)));
    assert!(report.report().contains(&identity.sha256));
}

#[test]
fn an_unresolved_symbol_disables_only_its_capability() {
    let corpus = TempCorpus::new("unresolved");
    corpus.write(&corpus_text(&[
        (OWNERSHIP, "AA BB CC DD"),
        (DEPOT, "11 22 33 44"),
        (CLOUD, "DE AD BE EF"),
    ]));
    let mut installer = FakeInstaller::default();

    let report = run(
        &identity(),
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );

    assert_eq!(report.installed.len(), 2);
    assert!(report.gate.is_enabled(Capability::Ownership));
    assert!(report.gate.is_enabled(Capability::DepotInjection));
    assert!(!report.gate.is_enabled(Capability::CloudControl));
    let reason = report
        .gate
        .reason(Capability::CloudControl)
        .expect("disabled reason");
    assert!(reason.contains(CLOUD));
    assert!(reason.contains("did not resolve"));
    assert!(report
        .failures
        .iter()
        .any(|failure| failure.symbol == CLOUD));
}

#[test]
fn a_failing_installer_disables_that_capability_and_finishes_the_batch() {
    let corpus = TempCorpus::new("install-failure");
    corpus.write(&corpus_text(&[
        (OWNERSHIP, "AA BB CC DD"),
        (DEPOT, "11 22 33 44"),
        (CLOUD, "55 66 77 88"),
    ]));
    let mut installer = FakeInstaller::failing_on(DEPOT);

    let report = run(
        &identity(),
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );

    assert_eq!(installer.attempts.len(), 3, "the rest of the batch runs");
    assert_eq!(report.installed.len(), 2);
    assert!(report.installed.iter().any(|entry| entry.symbol == CLOUD));
    assert!(report.gate.is_enabled(Capability::Ownership));
    assert!(report.gate.is_enabled(Capability::CloudControl));
    assert!(!report.gate.is_enabled(Capability::DepotInjection));
    let reason = report
        .gate
        .reason(Capability::DepotInjection)
        .expect("disabled reason");
    assert!(reason.contains(DEPOT));
    assert!(reason.contains("failed to install"));
}

#[test]
fn the_report_names_every_installed_symbol_and_disabled_capability() {
    let corpus = TempCorpus::new("report");
    corpus.write(&corpus_text(&[
        (OWNERSHIP, "AA BB CC DD"),
        (DEPOT, "DE AD BE EF"),
        (CLOUD, "12 34 56 78"),
    ]));
    let mut installer = FakeInstaller::default();

    let report = run(
        &identity(),
        &synthetic_pe64(),
        &corpus.root,
        0,
        &mut installer,
    );
    let text = report.report();

    assert!(text.contains(OWNERSHIP));
    assert!(text.contains(DEPOT));
    assert!(text.contains(CLOUD));
    for (capability, reason) in report.gate.disabled() {
        assert!(text.contains(capability.label()), "{capability} missing");
        assert!(text.contains(reason), "reason for {capability} missing");
    }
    assert!(text.contains("depot-injection"));
    assert!(text.contains("cloud-control"));
}

/// Keeps `TempCorpus::write` honest: the corpus path must be under the temp
/// root, so a stale file cannot make the missing-corpus test pass accidentally.
#[test]
fn the_temp_corpus_is_created_below_its_root() {
    let corpus = TempCorpus::new("layout");
    corpus.write(&corpus_text(&[]));
    let path = corpus::path(&corpus.root, &identity()).expect("path");
    assert!(path.starts_with(&corpus.root));
    assert!(path.exists());
    assert!(std::path::Path::new(&corpus.root).exists());
}

mod native_delivery;
