//! End-to-end: SteamUI symbols with an anchor *and* a body now install.
//!
//! The install pass is driven exactly as the host drives it — a real corpus file
//! written for the synthetic image's own sha256, a real `install::run`, and an
//! installer that answers from [`crate::hooks::REPLACEMENTS`] the way the
//! production `ProcessDetours` installer does. That makes the registry, not the
//! fake, the thing under test: a SteamUI symbol installs because a body is
//! registered for it, and one without a body still fails cleanly.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::capability::Capability;
use crate::hooks;
use crate::install::{self, corpus, DetourInstaller, InstallError, Installed};
use crate::module::{self, ModuleIdentity};

const VISIBILITY: &str = "ShouldShowAppInLibrary";
const SNAPSHOT: &str = "CSteamUIAppController::BuildCompleteAppOverviewChange";
const CHANGE: &str = "CUpdateManager::MarkAppChange";
const OVERVIEW: &str = "CSteamUIAppController::FillInAppOverview";
const NO_BODY: &str = "CSteamUIAppController::GetAppByID";

static NEXT_CORPUS: AtomicU64 = AtomicU64::new(0);

/// A temporary corpus root that removes itself after the test.
struct TempCorpus {
    root: PathBuf,
}

impl TempCorpus {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vf-host-windows-ui-{}-{}-{label}",
            std::process::id(),
            NEXT_CORPUS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create temp corpus root");
        Self { root }
    }

    /// Write a corpus for this image under the steamui component.
    fn write(&self, entries: &[(&str, &str)]) {
        let mut text = String::from("[hotfix]\nformat = 1\narchitecture = \"x86_64\"\n\n");
        for (name, pattern) in entries {
            text.push_str(&format!(
                "[steamui.\"{name}\"]\nfollow = \"none\"\npattern = \"{pattern}\"\n\n"
            ));
        }
        let path = corpus::path(&self.root, &identity()).expect("corpus path");
        std::fs::create_dir_all(path.parent().expect("corpus parent")).expect("create corpus dir");
        std::fs::write(path, text).expect("write corpus");
    }

    fn run(&self, installer: &mut RegistryInstaller) -> install::RunReport {
        install::run(&identity(), &synthetic_steamui_pe64(), &self.root, 0, installer)
    }
}

impl Drop for TempCorpus {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Answers from the real replacement table, like the production installer.
#[derive(Default)]
struct RegistryInstaller {
    attempted: Vec<String>,
}

impl DetourInstaller for RegistryInstaller {
    fn install(&mut self, symbol: &str, address: usize) -> Result<Installed, InstallError> {
        self.attempted.push(symbol.to_owned());
        if hooks::replacement_address(symbol).is_none() {
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

/// Minimal PE32+ with one `.text` section (RVA 0x1000, raw 0x200, 0x100 bytes)
/// carrying four unique four-byte patterns.
fn synthetic_steamui_pe64() -> Vec<u8> {
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

/// Identified from its bytes, so the corpus file it reads is its own sha256's.
fn identity() -> ModuleIdentity {
    module::identify("SteamUI.dll", &synthetic_steamui_pe64()).expect("identify synthetic PE")
}

#[test]
fn a_steamui_symbol_with_an_anchor_and_a_body_installs() {
    let corpus = TempCorpus::new("installs");
    corpus.write(&[
        (VISIBILITY, "AA BB CC DD"),
        (SNAPSHOT, "11 22 33 44"),
        (CHANGE, "55 66 77 88"),
    ]);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert_eq!(
        report
            .installed
            .iter()
            .map(|installed| installed.symbol.as_str())
            .collect::<Vec<_>>(),
        vec![VISIBILITY, SNAPSHOT, CHANGE],
        "every steamui body in the corpus was installed through the registry"
    );
    assert_eq!(report.installed[0].address, 0x1010);
    assert_eq!(report.installed[1].address, 0x1030);
    assert_eq!(report.installed[2].address, 0x1050);
    assert!(report.gate.is_enabled(Capability::LibraryUi));
    assert!(report.gate.is_enabled(Capability::LibrarySnapshot));
}

#[test]
fn an_anchored_symbol_without_a_body_still_fails_cleanly() {
    let corpus = TempCorpus::new("no-body");
    corpus.write(&[(NO_BODY, "DE AD BE EF"), (VISIBILITY, "AA BB CC DD")]);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert_eq!(installer.attempted.len(), 2, "the batch does not stop");
    assert_eq!(report.installed.len(), 1);
    assert_eq!(report.installed[0].symbol, VISIBILITY);
    let failure = report
        .failures
        .iter()
        .find(|failure| failure.symbol == NO_BODY)
        .expect("the bodyless symbol is reported");
    assert!(failure.reason.contains("no replacement"), "{}", failure.reason);
}

#[test]
fn an_unresolved_steamui_symbol_disables_only_its_own_capability() {
    let corpus = TempCorpus::new("own-capability");
    corpus.write(&[
        (VISIBILITY, "AA BB CC DD"),
        (SNAPSHOT, "11 22 33 44"),
        // A pattern no byte of this image matches: the anchor is absent.
        (OVERVIEW, "FE ED FA CE"),
    ]);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert_eq!(report.installed.len(), 2);
    assert!(report.gate.is_enabled(Capability::LibraryUi));
    assert!(report.gate.is_enabled(Capability::LibrarySnapshot));
    assert!(!report.gate.is_enabled(Capability::OverviewMetadata));
    let reason = report
        .gate
        .reason(Capability::OverviewMetadata)
        .expect("disabled with a reason");
    assert!(reason.contains(OVERVIEW), "{reason}");
    assert!(reason.contains("did not resolve"), "{reason}");
    // The capability that is still installed keeps working: one anchor missing
    // costs one library feature, not the batch. `report()` lists only what is
    // disabled, so an absent entry is the proof.
    let text = report.report();
    assert!(text.contains("overview-metadata"), "{text}");
    assert!(!text.contains("library-ui"), "{text}");
    assert!(!text.contains("library-snapshot"), "{text}");
}

#[test]
fn a_steamui_corpus_without_the_library_symbols_leaves_them_alone() {
    let corpus = TempCorpus::new("untouched");
    corpus.write(&[(NO_BODY, "DE AD BE EF")]);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert!(report.installed.is_empty());
    for capability in [
        Capability::LibraryUi,
        Capability::OverviewMetadata,
        Capability::LibrarySnapshot,
    ] {
        assert!(
            report.gate.is_enabled(capability),
            "{capability} must not be blamed for a symbol it never named"
        );
    }
}
