//! Which capabilities each resolved hook symbol serves.
//!
//! The corpus only names symbols; it does not describe what they are for. The
//! mapping lives here so a missing signature disables exactly the capabilities
//! that cannot work without it. Symbols absent from this table are still
//! installed (new corpus entries should not be dropped) but carry no gate
//! dependency yet.

use crate::abi::achievements;
use crate::capability::Capability;

/// One symbol and the capabilities that stop working when it does not install.
const DEPENDENCIES: &[(&str, &[Capability])] = &[
    ("CUser::CheckAppOwnership", &[Capability::Ownership]),
    ("CUser::GetSubscribedApps", &[Capability::Ownership]),
    ("CAppOwnershipInfo::Fill", &[Capability::Ownership]),
    ("MarkLicenseAsChanged", &[Capability::Ownership]),
    ("LoadPackage", &[Capability::PackageInjection]),
    ("GetAppDataFromAppInfo", &[Capability::PackageInjection]),
    (
        "CPackageInfo::GetPackageInfo",
        &[Capability::PackageInjection],
    ),
    ("CUtlBufferEnsureCapacity", &[Capability::PackageInjection]),
    ("KeyValues_ReadAsBinary", &[Capability::PackageInjection]),
    ("KeyValues_FindOrCreateKey", &[Capability::PackageInjection]),
    ("GetDecryptionKey", &[Capability::DepotInjection]),
    (
        "CDepotDownloadMgr::LoadDepotDecryptionKey",
        &[Capability::DepotInjection],
    ),
    ("CUser::GetAppTicket", &[Capability::TicketOverrides]),
    // The stats path is a job chain, not a getter: `CUserStats` slot 6
    // constructs a `CAPIJobRequestUserStats`, its run method (`BInit`) sends the
    // request and calls the response parser and the merge, and
    // `YieldingMergeStats` merges the parsed reply. Both are anchored in both
    // live builds. The name this table used to carry, `CUser::GetUserStats`, is
    // refuted by `res/patterns/win64/VTABLE-REPORT.md` §1: it appears in neither
    // module and no slot has its shape, so it described a dependency the install
    // pass could never satisfy.
    //
    // Neither job symbol has a body yet (`crate::abi::achievements` records the
    // measurement that rules one out, and what a hook would need instead), so a
    // live install reports `achievements` as disabled with "no replacement body
    // for it yet" — which is the truth. Listing them here is what makes an
    // anchor that stops resolving cost the capability instead of passing
    // silently; the day a body is written, the same row keeps the gate honest
    // without an edit.
    (achievements::BINIT_SYMBOL, &[Capability::Achievements]),
    (achievements::MERGE_SYMBOL, &[Capability::Achievements]),
    (
        "CloseAppCloud",
        &[Capability::CloudControl, Capability::CloudHttp],
    ),
    ("PchMsgNameFromEMsg", &[Capability::CmInterception]),
    ("IPCProcessMessage", &[Capability::CmInterception]),
    ("BBuildAndAsyncSendFrame", &[Capability::CmInterception]),
    ("GetPipeClient", &[Capability::CmInterception]),
    // The dispatch site the capability actually needs. The previous name here was
    // `CNetPacket::Dispatch`, which `VTABLE-REPORT.md` §3 refutes: the packet
    // classes have no dispatch slot and the table lives at `CCMInterface` slot 3.
    (
        "CCMInterface::RecvPkt",
        &[Capability::CmInterception, Capability::RequestCodes],
    ),
    // SteamUI. Each body owns exactly one capability, so a missing library
    // anchor costs one library feature rather than the whole batch.
    ("ShouldShowAppInLibrary", &[Capability::LibraryUi]),
    (
        "CSteamUIAppController::FillInAppOverview",
        &[Capability::OverviewMetadata],
    ),
    (
        "CSteamUIAppController::BuildCompleteAppOverviewChange",
        &[Capability::LibrarySnapshot],
    ),
    (
        "CUpdateManager::MarkAppChange",
        &[Capability::LibrarySnapshot],
    ),
    // The six bodies added once the corpus anchored them. Each names the one
    // capability it serves, so a corpus that stops resolving one symbol costs one
    // feature instead of the batch.
    (
        "CShaderCacheManager::GetShaderCacheDepot",
        &[Capability::ShaderCacheControl],
    ),
    (
        "IClientAppManager::IsAppDlcInstalled",
        &[Capability::DlcOverrides],
    ),
    (
        "IClientAppManager::BIsDlcEnabled",
        &[Capability::DlcOverrides],
    ),
    (
        "CSteamEngine::SetAPICallResult",
        &[Capability::CallbackEvents],
    ),
    (
        "IClientUser::RequiresLegacyCDKey",
        &[Capability::LegacyCdKeyControl],
    ),
    (
        "CUser::BuildSpawnEnvBlock",
        &[Capability::LaunchEnvironment],
    ),
];

/// Capabilities that depend on `symbol`; empty for an unknown symbol.
pub fn capabilities_for_symbol(symbol: &str) -> &'static [Capability] {
    DEPENDENCIES
        .iter()
        .find(|(name, _)| *name == symbol)
        .map_or(&[], |(_, capabilities)| capabilities)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_symbols_name_their_capabilities() {
        assert_eq!(
            capabilities_for_symbol("CUser::CheckAppOwnership"),
            &[Capability::Ownership]
        );
        assert_eq!(
            capabilities_for_symbol("CloseAppCloud"),
            &[Capability::CloudControl, Capability::CloudHttp]
        );
    }

    #[test]
    fn unknown_symbols_have_no_dependency() {
        assert!(capabilities_for_symbol("SomeFutureSymbol").is_empty());
    }

    #[test]
    fn the_stats_job_symbols_gate_achievements_and_nothing_else() {
        // The two anchored sites of the job chain `VTABLE-REPORT.md` §1 measures.
        // Both are the same capability's dependency: an install that resolves only
        // one of them still has no achievements path, so a failure at either is
        // one disabled feature, not two.
        for symbol in [
            crate::abi::achievements::BINIT_SYMBOL,
            crate::abi::achievements::MERGE_SYMBOL,
        ] {
            assert_eq!(
                capabilities_for_symbol(symbol),
                &[Capability::Achievements],
                "{symbol} must gate achievements"
            );
        }
    }

    #[test]
    fn the_refuted_getter_is_not_a_dependency_any_more() {
        // The table used to name `CUser::GetUserStats`. Keeping it would have read
        // as a dependency the install pass can satisfy, while the symbol exists in
        // no live build.
        assert!(capabilities_for_symbol("CUser::GetUserStats").is_empty());
    }

    #[test]
    fn each_anchored_body_names_the_capability_it_serves() {
        // The six bodies the corpus gained in this pass, plus the subscription body
        // that shares `ownership` with `CheckAppOwnership`.
        for (symbol, capability) in [
            (
                "CShaderCacheManager::GetShaderCacheDepot",
                Capability::ShaderCacheControl,
            ),
            (
                "IClientAppManager::IsAppDlcInstalled",
                Capability::DlcOverrides,
            ),
            ("IClientAppManager::BIsDlcEnabled", Capability::DlcOverrides),
            ("CSteamEngine::SetAPICallResult", Capability::CallbackEvents),
            (
                "IClientUser::RequiresLegacyCDKey",
                Capability::LegacyCdKeyControl,
            ),
            ("CUser::BuildSpawnEnvBlock", Capability::LaunchEnvironment),
            ("CUser::GetSubscribedApps", Capability::Ownership),
        ] {
            assert_eq!(
                capabilities_for_symbol(symbol),
                &[capability],
                "{symbol} must gate {capability} and nothing else"
            );
        }
    }

    #[test]
    fn both_dlc_entry_points_gate_the_same_capability() {
        // Steam asks two questions and the host answers both: a corpus that anchors
        // only one of them still needs the same capability, so a failure there is one
        // disabled feature, not two.
        for symbol in [
            "IClientAppManager::IsAppDlcInstalled",
            "IClientAppManager::BIsDlcEnabled",
        ] {
            assert_eq!(capabilities_for_symbol(symbol), &[Capability::DlcOverrides]);
        }
    }

    #[test]
    fn the_dispatch_site_gates_interception_and_request_codes() {
        // The body reads the packet and, when the layout exists, answers a
        // request code: both capabilities depend on the same symbol. The refuted
        // name must not be mapped, or a corpus that never resolves it would look
        // like a dependency the install pass can satisfy.
        assert_eq!(
            capabilities_for_symbol("CCMInterface::RecvPkt"),
            &[Capability::CmInterception, Capability::RequestCodes]
        );
        assert!(capabilities_for_symbol("CNetPacket::Dispatch").is_empty());
    }

    #[test]
    fn the_one_cloud_symbol_gates_both_cloud_capabilities() {
        // `CloseAppCloud` is the only cloud site the win64 corpus anchors, so a
        // failed install there has to cost both cloud capabilities.
        assert_eq!(
            capabilities_for_symbol("CloseAppCloud"),
            &[Capability::CloudControl, Capability::CloudHttp]
        );
    }

    #[test]
    fn each_steamui_symbol_names_its_own_library_capability() {
        assert_eq!(
            capabilities_for_symbol("ShouldShowAppInLibrary"),
            &[Capability::LibraryUi]
        );
        assert_eq!(
            capabilities_for_symbol("CSteamUIAppController::FillInAppOverview"),
            &[Capability::OverviewMetadata]
        );
        assert_eq!(
            capabilities_for_symbol("CSteamUIAppController::BuildCompleteAppOverviewChange"),
            &[Capability::LibrarySnapshot]
        );
        assert_eq!(
            capabilities_for_symbol("CUpdateManager::MarkAppChange"),
            &[Capability::LibrarySnapshot]
        );
    }

    #[test]
    fn a_steamui_anchor_never_gates_a_steamclient_capability() {
        // A library symbol that fails to resolve must cost a library feature, not
        // ownership or package injection: the two batches are independent.
        for symbol in [
            "ShouldShowAppInLibrary",
            "CSteamUIAppController::FillInAppOverview",
            "CSteamUIAppController::BuildCompleteAppOverviewChange",
            "CUpdateManager::MarkAppChange",
        ] {
            for capability in capabilities_for_symbol(symbol) {
                assert!(
                    matches!(
                        capability,
                        Capability::LibraryUi
                            | Capability::OverviewMetadata
                            | Capability::LibrarySnapshot
                    ),
                    "{symbol} must not gate {capability}"
                );
            }
        }
    }
}
