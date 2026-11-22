//! End-to-end: a steamclient symbol with an anchor *and* a body now installs.
//!
//! The install pass is driven exactly as the host drives it — a real corpus file
//! written for the synthetic image's own sha256, a real `install::run`, and an
//! installer that answers from [`crate::hooks::REPLACEMENTS`] the way the
//! production `ProcessDetours` installer does. That makes the registry, not the
//! fake, the thing under test: a symbol installs because a body is registered for
//! it, and its capability stays enabled because the body installed.
//!
//! This is the steamclient half of the same flow test [`crate::hooks::ui`]'s tests
//! run for the SteamUI batch.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::capability::Capability;
use crate::hooks;
use crate::install::{self, corpus, DetourInstaller, InstallError, Installed};
use crate::module::{self, ModuleIdentity};

const SUBSCRIBED: &str = "CUser::GetSubscribedApps";
const SHADER: &str = "CShaderCacheManager::GetShaderCacheDepot";
const DLC_INSTALLED: &str = "IClientAppManager::IsAppDlcInstalled";
const DLC_ENABLED: &str = "IClientAppManager::BIsDlcEnabled";
const CALLBACK: &str = "CSteamEngine::SetAPICallResult";
const CDKEY: &str = "IClientUser::RequiresLegacyCDKey";
const LAUNCH_ENV: &str = "CUser::BuildSpawnEnvBlock";
/// The observation-only stats-merge body. Its row here is the achievements flow
/// proof: the body installs at its entry through the real registry and
/// [`Capability::Achievements`] stays enabled because it did.
const MERGE: &str = crate::abi::achievements::MERGE_SYMBOL;
const NO_BODY: &str = "CUser::GetSteamID";

/// Every symbol the corpus gained a body for in this pass, with the unique
/// four-byte pattern the synthetic image carries for it and the RVA that gives.
const ANCHORED: &[(&str, &str, usize)] = &[
    (SHADER, "AA BB CC DD", 0x1010),
    (DLC_INSTALLED, "11 22 33 44", 0x1030),
    (DLC_ENABLED, "55 66 77 88", 0x1050),
    (CALLBACK, "DE AD BE EF", 0x1070),
    (CDKEY, "01 02 03 04", 0x1020),
    (LAUNCH_ENV, "0A 0B 0C 0D", 0x1040),
    (SUBSCRIBED, "0F 1E 2D 3C", 0x1060),
    (MERGE, "5A 5B 5C 5D", 0x1080),
];

static NEXT_CORPUS: AtomicU64 = AtomicU64::new(0);

/// A temporary corpus root that removes itself after the test.
struct TempCorpus {
    root: PathBuf,
}

impl TempCorpus {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "vf-host-windows-bodies-{}-{}-{label}",
            std::process::id(),
            NEXT_CORPUS.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create temp corpus root");
        Self { root }
    }

    /// Write a corpus for this image under the steamclient component.
    fn write(&self, entries: &[(&str, &str)]) {
        let mut text = String::from("[hotfix]\nformat = 1\narchitecture = \"x86_64\"\n\n");
        for (name, pattern) in entries {
            text.push_str(&format!(
                "[steamclient.\"{name}\"]\nfollow = \"none\"\npattern = \"{pattern}\"\n\n"
            ));
        }
        let path = corpus::path(&self.root, &identity()).expect("corpus path");
        std::fs::create_dir_all(path.parent().expect("corpus parent")).expect("create corpus dir");
        std::fs::write(path, text).expect("write corpus");
    }

    fn run(&self, installer: &mut RegistryInstaller) -> install::RunReport {
        install::run(
            &identity(),
            &synthetic_steamclient_pe64(),
            &self.root,
            0,
            installer,
        )
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
/// carrying seven unique four-byte patterns.
fn synthetic_steamclient_pe64() -> Vec<u8> {
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
    data[0x220..0x224].copy_from_slice(&[0x01, 0x02, 0x03, 0x04]);
    data[0x230..0x234].copy_from_slice(&[0x11, 0x22, 0x33, 0x44]);
    data[0x240..0x244].copy_from_slice(&[0x0A, 0x0B, 0x0C, 0x0D]);
    data[0x250..0x254].copy_from_slice(&[0x55, 0x66, 0x77, 0x88]);
    data[0x260..0x264].copy_from_slice(&[0x0F, 0x1E, 0x2D, 0x3C]);
    data[0x270..0x274].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
    data[0x280..0x284].copy_from_slice(&[0x5A, 0x5B, 0x5C, 0x5D]);
    data
}

/// Identified from its bytes, so the corpus file it reads is its own sha256's.
fn identity() -> ModuleIdentity {
    module::identify("steamclient64.dll", &synthetic_steamclient_pe64())
        .expect("identify synthetic PE")
}

#[test]
fn every_anchored_body_installs_and_keeps_its_capability() {
    let corpus = TempCorpus::new("installs");
    let entries: Vec<(&str, &str)> = ANCHORED
        .iter()
        .map(|(symbol, pattern, _)| (*symbol, *pattern))
        .collect();
    corpus.write(&entries);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert_eq!(
        report
            .installed
            .iter()
            .map(|installed| installed.symbol.as_str())
            .collect::<Vec<_>>(),
        ANCHORED
            .iter()
            .map(|(symbol, _, _)| *symbol)
            .collect::<Vec<_>>(),
        "every anchored body installed through the registry"
    );
    for (installed, (symbol, _, rva)) in report.installed.iter().zip(ANCHORED) {
        assert_eq!(&installed.symbol, symbol);
        assert_eq!(
            installed.address, *rva,
            "{symbol} was not patched at its entry"
        );
        assert!(
            hooks::replacement_address(symbol).is_some(),
            "{symbol} installed without a registered body"
        );
    }
    for capability in [
        Capability::ShaderCacheControl,
        Capability::DlcOverrides,
        Capability::CallbackEvents,
        Capability::LegacyCdKeyControl,
        Capability::LaunchEnvironment,
        Capability::Ownership,
        Capability::Achievements,
    ] {
        assert!(
            report.gate.is_enabled(capability),
            "{capability} must stay enabled when its body installed"
        );
    }
}

#[test]
fn one_missing_anchor_costs_only_its_own_capability() {
    let corpus = TempCorpus::new("one-gap");
    let mut entries: Vec<(&str, &str)> = ANCHORED
        .iter()
        .filter(|(symbol, _, _)| *symbol != SHADER)
        .map(|(symbol, pattern, _)| (*symbol, *pattern))
        .collect();
    // A pattern no byte of this image matches: the shader anchor is absent.
    entries.push((SHADER, "FE ED FA CE"));
    corpus.write(&entries);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert_eq!(report.installed.len(), ANCHORED.len() - 1);
    assert!(!report.gate.is_enabled(Capability::ShaderCacheControl));
    let reason = report
        .gate
        .reason(Capability::ShaderCacheControl)
        .expect("disabled with a reason");
    assert!(reason.contains(SHADER), "{reason}");
    assert!(reason.contains("did not resolve"), "{reason}");
    for capability in [
        Capability::DlcOverrides,
        Capability::CallbackEvents,
        Capability::LegacyCdKeyControl,
        Capability::LaunchEnvironment,
        Capability::Ownership,
        Capability::Achievements,
    ] {
        assert!(
            report.gate.is_enabled(capability),
            "{capability} must not be blamed for a symbol it never named"
        );
    }
}

#[test]
fn an_anchored_symbol_without_a_body_still_fails_cleanly() {
    let corpus = TempCorpus::new("no-body");
    corpus.write(&[(NO_BODY, "AA BB CC DD"), (SUBSCRIBED, "0F 1E 2D 3C")]);
    let mut installer = RegistryInstaller::default();

    let report = corpus.run(&mut installer);

    assert_eq!(installer.attempted.len(), 2, "the batch does not stop");
    assert_eq!(report.installed.len(), 1);
    assert_eq!(report.installed[0].symbol, SUBSCRIBED);
    let failure = report
        .failures
        .iter()
        .find(|failure| failure.symbol == NO_BODY)
        .expect("the bodyless symbol is reported");
    assert!(
        failure.reason.contains("no replacement"),
        "{}",
        failure.reason
    );
    // It maps to no capability either, so a bodyless symbol costs no feature: the
    // decision-only `steam_id` module keeps its own capability untouched.
    assert!(report.gate.is_enabled(Capability::SteamIdOverride));
    assert!(report.gate.is_enabled(Capability::Ownership));
}
