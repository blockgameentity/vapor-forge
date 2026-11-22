//! The replacement functions, and the table that binds them to corpus symbols.
//!
//! Each submodule is one hook: the ABI-documented body in [`crate::abi`], the
//! pure decision function it calls, and the symbol that anchors it in
//! `res/patterns/win64/`. The decision is separated from the FFI body on purpose —
//! `decide`-style functions take a [`state::HostState`] and plain values, so the
//! whole behaviour is unit-tested on Linux without a live client, a detour or a
//! published global.
//!
//! What is wired, and which corpus symbol backs it:
//!
//! | hook | corpus symbol | anchor in public / publicbeta |
//! | --- | --- | --- |
//! | [`ownership`] | `CUser::CheckAppOwnership` | yes / yes |
//! | [`subscribed`] | `CUser::GetSubscribedApps` | yes / yes |
//! | [`package`] | `CPackageInfo::GetPackageInfo` | yes / yes |
//! | [`depot`] | `GetDecryptionKey` | yes / yes |
//! | [`ticket`] | `CUser::GetAppTicket` | **no / no** — body written, cannot install |
//! | [`cloud_http`] | `CloseAppCloud` | yes / yes |
//! | [`netpacket`] | `CCMInterface::RecvPkt` | yes / yes |
//! | [`shader`] | `CShaderCacheManager::GetShaderCacheDepot` | yes / yes |
//! | [`dlc`] (2 bodies) | `IClientAppManager::IsAppDlcInstalled`, `IClientAppManager::BIsDlcEnabled` | yes / yes |
//! | [`callbacks`] | `CSteamEngine::SetAPICallResult` (body in [`callback_result`]) | yes / yes |
//! | [`legacy_cdkey`] | `IClientUser::RequiresLegacyCDKey` | yes / yes |
//! | [`launch_env`] | `CUser::BuildSpawnEnvBlock` | yes / yes |
//! | [`achievements`] (rewrites the reply) | `CAPIJobRequestUserStats::YieldingMergeStats` | yes / yes |
//! | [`ui`] (4 bodies) | `ShouldShowAppInLibrary`, `FillInAppOverview`, `BuildCompleteAppOverviewChange`, `MarkAppChange` | yes / yes, **no / yes**, yes / yes, yes / **no** |
//! | [`steam_id`] | `CUser::GetSteamID` | **no / no** — body written, cannot install until the corpus carries the symbol |
//!
//! The rows that carry an anchor gap are listed rather than hidden: `install::deps`
//! maps each of them to the capability it serves, so an absent anchor reports that
//! capability as disabled with the resolver's reason instead of silently doing
//! nothing.
//!
//! A body that installs is not automatically a body that can do its whole job.
//! [`callback_result`] has no delivery sink and [`launch_env`] has no measured
//! environment-map writer; both say so in their module docs and log it, rather than
//! claiming an effect they do not have.
//!
//! # Decision-only modules
//!
//! One capability has a portable decision and **no** body: [`conflict_ui`]
//! (`RegisterJsMethod` has no candidate in either SteamUI build). The module names
//! the Linux symbol that is missing, and each `decide` function is
//! unit-tested. It is not listed in [`REPLACEMENTS`]: a body whose symbol has no
//! anchor never installs, and registering one would make the install report claim a
//! hook it does not have.
//!
//! `steam_id` used to be on that list. It is not any more: the corpus work that
//! relaxed the anchor gate for a `.pdata`-less leaf made `CUser::GetSteamID`
//! controllable under that name, and [`steam_id::registration`] is the body. Until
//! the corpus entry reaches the shipped pattern files the symbol resolves in no
//! build, so the body registers and installs nowhere — `install::deps` maps it to
//! [`crate::capability::Capability::SteamIdOverride`], which therefore stays enabled
//! (a symbol absent from a batch is not a failed dependency) and simply has no
//! carrier on a live install yet.
//!
//! `achievements` is no longer in that list. Its decision lives in
//! [`crate::features::achievements`], its two candidate sites are **anchored**
//! (`CAPIJobRequestUserStats::BInit` and `::YieldingMergeStats`, both live
//! builds), and [`achievements`] registers a body at the merge that forwards the
//! call verbatim and rewrites the reply body when the local rebuild produces a
//! different payload: the geometry decoder locates the reply, the swap guard owns
//! the bytes for the duration of the call, and Steam's bytes are restored before
//! the body returns. Whether it can rewrite depends on one corpus decode
//! (`patterns::job_fields`, wired through
//! [`achievements::geometry`]); until that is merged the body records the gap and
//! forwards Steam's reply unchanged.
//!
//! [`family_play`] is the one decision that is not a hook: it is the concurrent-play
//! consult that [`ownership`] and [`package`] call.

pub mod achievements;
pub mod callback_result;
pub mod callbacks;
pub mod cloud_http;
pub mod conflict_ui;
pub mod depot;
pub mod depot_store;
pub mod dlc;
pub mod family_play;
pub mod launch_env;
pub mod legacy_cdkey;
pub mod netpacket;
pub mod original;
pub mod ownership;
pub mod package;
pub mod shader;
pub mod state;
pub mod steam_id;
pub mod subscribed;
pub mod ticket;
pub mod ui;

#[cfg(windows)]
pub mod installer;

#[cfg(test)]
mod tests_bodies;
#[cfg(test)]
mod tests_support;

/// Address of the replacement body for `symbol`, when this host implements it.
///
/// The install pass resolves *every* symbol in the corpus, including the ones no
/// body exists for. Those are skipped rather than failed: a missing body is not a
/// broken entry, and skipping them lets the report say "no replacement on this
/// host yet" instead of "the patch failed".
pub fn replacement_address(symbol: &str) -> Option<usize> {
    REPLACEMENTS
        .iter()
        .find(|(name, _)| *name == symbol)
        .map(|(_, address)| address())
}

/// Address resolver for one hook body.
pub type ReplacementAddress = fn() -> usize;

/// One corpus symbol and the body that replaces it.
pub type Replacement = (&'static str, ReplacementAddress);

/// Every symbol this host has a replacement body for.
pub const REPLACEMENTS: &[Replacement] = &[
    (ownership::SYMBOL, ownership::address),
    (subscribed::SYMBOL, subscribed::address),
    (package::SYMBOL, package::address),
    (depot::SYMBOL, depot::address),
    (ticket::SYMBOL, ticket::address),
    (cloud_http::SYMBOL, cloud_http::address),
    (shader::SYMBOL, shader::address),
    (dlc::SYMBOL, dlc::address),
    (dlc::ENABLED_SYMBOL, dlc::enabled_address),
    (callback_result::SYMBOL, callback_result::address),
    (legacy_cdkey::SYMBOL, legacy_cdkey::address),
    (launch_env::SYMBOL, launch_env::address),
    (achievements::SYMBOL, achievements::address),
    (conflict_ui::registration::SYMBOL, conflict_ui::registration::address),
    (ui::visibility::SYMBOL, ui::visibility::address),
    (ui::overview::SYMBOL, ui::overview::address),
    (ui::snapshot::SYMBOL, ui::snapshot::address),
    (ui::change::SYMBOL, ui::change::address),
    (netpacket::SYMBOL, netpacket::address),
    (steam_id::registration::SYMBOL, steam_id::registration::address),
];

/// The resolved symbol → body table, as the installer and the plan filter need it.
pub fn replacements() -> Vec<(&'static str, usize)> {
    REPLACEMENTS
        .iter()
        .map(|(symbol, address)| (*symbol, address()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_body_has_a_real_address() {
        for (symbol, address) in replacements() {
            assert!(!symbol.is_empty());
            assert_ne!(address, 0, "{symbol} must resolve to its body");
        }
    }

    #[test]
    fn an_unimplemented_symbol_has_no_replacement() {
        // `IClientUser::GetSteamID` — the IPC-facing proxy — is still without a
        // body: the Windows accessor the corpus measures is `CUser::GetSteamID`
        // (out-parameter shape, see `res/patterns/win64/VTABLE-REPORT.md` §9), and
        // the interface entry is reached through it.
        assert!(replacement_address("IClientUser::GetSteamID").is_none());
        assert_eq!(
            replacement_address(ownership::SYMBOL),
            Some(ownership::address())
        );
    }

    #[test]
    fn the_identity_body_is_registered_under_the_measured_symbol() {
        // The body replaces the out-parameter accessor, not the Linux `u64`
        // getter, so it is registered under the name the corpus will carry.
        assert_eq!(steam_id::registration::SYMBOL, "CUser::GetSteamID");
        assert_eq!(
            replacement_address("CUser::GetSteamID"),
            Some(steam_id::registration::address())
        );
    }

    #[test]
    fn every_newly_anchored_symbol_is_registered() {
        for (symbol, module_address) in [
            (shader::SYMBOL, shader::address()),
            (dlc::SYMBOL, dlc::address()),
            (dlc::ENABLED_SYMBOL, dlc::enabled_address()),
            (callback_result::SYMBOL, callback_result::address()),
            (legacy_cdkey::SYMBOL, legacy_cdkey::address()),
            (launch_env::SYMBOL, launch_env::address()),
            (subscribed::SYMBOL, subscribed::address()),
        ] {
            assert_eq!(
                replacement_address(symbol),
                Some(module_address),
                "{symbol} has a body and must be installable"
            );
        }
    }

    #[test]
    fn every_steamui_body_is_registered_under_its_anchored_name() {
        for symbol in [
            "ShouldShowAppInLibrary",
            "CSteamUIAppController::FillInAppOverview",
            "CSteamUIAppController::BuildCompleteAppOverviewChange",
            "CUpdateManager::MarkAppChange",
        ] {
            assert!(
                replacement_address(symbol).is_some(),
                "{symbol} has a body but is not in REPLACEMENTS"
            );
        }
    }

    #[test]
    fn the_dispatch_body_is_registered_under_the_real_symbol() {
        // `res/patterns/win64/VTABLE-REPORT.md` §3 refutes `CNetPacket::Dispatch`
        // as a name: the dispatch is `CCMInterface` slot 3.
        assert_eq!(netpacket::SYMBOL, "CCMInterface::RecvPkt");
        assert_eq!(
            replacement_address("CCMInterface::RecvPkt"),
            Some(netpacket::address())
        );
        assert!(replacement_address("CNetPacket::Dispatch").is_none());
    }

    #[test]
    fn the_cloud_body_is_registered_under_its_anchored_name() {
        assert_eq!(cloud_http::SYMBOL, "CloseAppCloud");
        assert_eq!(
            replacement_address("CloseAppCloud"),
            Some(cloud_http::address())
        );
    }

    #[test]
    fn a_decision_without_an_anchor_is_never_registered() {
        // These have a `decide` and no corpus symbol to install at: the win64
        // pattern files resolve none of them in either live build. Registering a
        // body would make the install report claim a hook the host does not have,
        // so the absence is asserted rather than assumed.
        for symbol in ["RegisterInternalCallback", "IClientUser::GetSteamID"] {
            assert_eq!(
                replacement_address(symbol),
                None,
                "{symbol} has no anchor, so it must not carry a body"
            );
        }
    }

    #[test]
    fn the_merge_body_is_observation_only_and_registered() {
        // The body replaces the stats-job merge and changes nothing: it forwards
        // the call and reports. The capability is gated by the same symbol
        // (`install::deps`), so registering it is what keeps `achievements`
        // enabled on a live install — and the install report says "installed",
        // which is exactly as much as this body earns.
        assert_eq!(
            replacement_address(crate::abi::achievements::MERGE_SYMBOL),
            Some(achievements::address())
        );
        // The run method stays without a body: it drives the whole
        // request/parse/merge cycle, so a detour there could only observe, and the
        // merge already does.
        assert_eq!(
            replacement_address(crate::abi::achievements::BINIT_SYMBOL),
            None,
            "the job's run method must not be claimed as installable while it has no body"
        );
    }
}
