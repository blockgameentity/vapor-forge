//! Which capabilities each resolved hook symbol serves.
//!
//! The corpus only names symbols; it does not describe what they are for. The
//! mapping lives here so a missing signature disables exactly the capabilities
//! that cannot work without it. Symbols absent from this table are still
//! installed (new corpus entries should not be dropped) but carry no gate
//! dependency yet.

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
    ("CUser::GetUserStats", &[Capability::Achievements]),
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
    ("CUpdateManager::MarkAppChange", &[Capability::LibrarySnapshot]),
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
