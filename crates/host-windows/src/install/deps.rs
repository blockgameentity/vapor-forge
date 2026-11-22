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
    ("CloseAppCloud", &[Capability::CloudControl]),
    ("PchMsgNameFromEMsg", &[Capability::CmInterception]),
    ("IPCProcessMessage", &[Capability::CmInterception]),
    ("BBuildAndAsyncSendFrame", &[Capability::CmInterception]),
    ("GetPipeClient", &[Capability::CmInterception]),
    ("CNetPacket::Dispatch", &[Capability::CmInterception]),
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
            &[Capability::CloudControl]
        );
    }

    #[test]
    fn unknown_symbols_have_no_dependency() {
        assert!(capabilities_for_symbol("SomeFutureSymbol").is_empty());
    }
}
