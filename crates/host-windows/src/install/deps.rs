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
    // Only the merge is listed. It is the site that carries a body
    // (`crate::hooks::achievements`, which rewrites the reply body), so it is the symbol whose
    // failure has to cost the capability. The run method has **no** body, and a
    // bodyless symbol resolves-but-cannot-install in every batch: listing it would
    // disable `achievements` on every live install regardless of the merge, which
    // is the opposite of what a dependency row is for. The day `BInit` gets a
    // replacement, its row comes back here with it.
    (achievements::MERGE_SYMBOL, &[Capability::Achievements]),
    (
        "RegisterJsMethod",
        &[Capability::ConflictUiBridge],
    ),
    // The identity accessor takes an out-parameter on Windows (see
    // `res/patterns/win64/VTABLE-REPORT.md`); the body presents the override through
    // it and nothing else depends on it.
    ("CUser::GetSteamID", &[Capability::SteamIdOverride]),
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
    //
    // `native-response-delivery` is mapped here too, because on Windows the
    // delivery *is* this dispatch: the body fabricates the response and swaps it
    // into Steam's packet inside the original call, and there is no queue to gate
    // separately. The seventh pass of `res/patterns/win64/VTABLE-REPORT.md`
    // measures the post machinery the Linux host uses (a `CWorkItem` handed to a
    // `CWorkThreadPool`) and records why this host does not use it;
    // `crate::hooks::netpacket` says what would have to change for a queue ever
    // to be needed. Mapping it onto the symbol that serves keeps the report
    // honest: if this body cannot install, nothing can deliver a response, and
    // the capability is disabled with that reason rather than silently reported
    // as active.
    (
        "CCMInterface::RecvPkt",
        &[
            Capability::CmInterception,
            Capability::RequestCodes,
            Capability::NativeResponseDelivery,
        ],
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
mod tests;
