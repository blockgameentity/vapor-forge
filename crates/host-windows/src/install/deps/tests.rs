//! Capability dependencies: one row per symbol, tested for the traps above.
//!
//! Split from `deps.rs` to keep both files readable; the table itself stays
//! next to its doc comment.

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
fn the_merge_symbol_gates_achievements_and_the_run_method_gates_nothing() {
    // The merge carries the reply-rewrite body, so it is the dependency.
    assert_eq!(
        capabilities_for_symbol(crate::abi::achievements::MERGE_SYMBOL),
        &[Capability::Achievements]
    );
    // The run method has no body. Mapping it would make every real install
    // resolve a symbol it cannot install and disable `achievements` with
    // "no replacement body", even though the merge body installed.
    assert!(
        capabilities_for_symbol(crate::abi::achievements::BINIT_SYMBOL).is_empty(),
        "a bodyless symbol must not gate a capability it cannot serve"
    );
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
fn the_dispatch_site_gates_interception_request_codes_and_delivery() {
    // The body reads the packet and, when the layout exists, answers a
    // request code: all three capabilities depend on the same symbol. The
    // refuted name must not be mapped, or a corpus that never resolves it
    // would look like a dependency the install pass can satisfy.
    assert_eq!(
        capabilities_for_symbol("CCMInterface::RecvPkt"),
        &[
            Capability::CmInterception,
            Capability::RequestCodes,
            Capability::NativeResponseDelivery,
        ]
    );
    assert!(capabilities_for_symbol("CNetPacket::Dispatch").is_empty());
}

#[test]
fn native_response_delivery_is_covered_by_the_synchronous_serve_path() {
    // Windows has no queue between "a response exists" and "Steam sees it":
    // the serve body fabricates and swaps inside Steam's own dispatch. The
    // capability is therefore mapped onto that dispatch, and onto nothing
    // else -- a second row would make the report claim a mechanism this host
    // does not have.
    let gating: Vec<&str> = DEPENDENCIES
        .iter()
        .filter(|(_, capabilities)| capabilities.contains(&Capability::NativeResponseDelivery))
        .map(|(symbol, _)| *symbol)
        .collect();
    assert_eq!(gating, vec!["CCMInterface::RecvPkt"]);
}

#[test]
fn the_retired_post_mechanism_is_not_a_dependency() {
    // The Linux host gates its queue on `CWorkThreadPool::AddWorkItem` and the
    // work-item ABI. Windows has the same classes (seventh pass of
    // `res/patterns/win64/VTABLE-REPORT.md`), but
    // this host never posts: naming one of them here would read as a
    // dependency the install pass could satisfy, exactly like the refuted
    // `CNetPacket::Dispatch` did. The names stay unresolved on purpose.
    for symbol in [
        "CWorkThreadPool::AddWorkItem",
        "CWorkThreadPool",
        "CWorkItem::Execute",
        "NativeInjectionWorkItem",
    ] {
        assert!(
            capabilities_for_symbol(symbol).is_empty(),
            "{symbol} must not gate a capability this host cannot honour"
        );
    }
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

#[test]
fn the_identity_accessor_gates_the_identity_capability_alone() {
    assert_eq!(
        capabilities_for_symbol("CUser::GetSteamID"),
        &[Capability::SteamIdOverride]
    );
    assert_eq!(
        capabilities_for_symbol("RegisterJsMethod"),
        &[Capability::ConflictUiBridge]
    );
}
