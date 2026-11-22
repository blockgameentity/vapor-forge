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
//! | [`package`] | `CPackageInfo::GetPackageInfo` | yes / yes |
//! | [`depot`] | `GetDecryptionKey` | yes / yes |
//! | [`ticket`] | `CUser::GetAppTicket` | **no / no** — body written, cannot install |
//! | [`ui`] (4 bodies) | `ShouldShowAppInLibrary`, `FillInAppOverview`, `BuildCompleteAppOverviewChange`, `MarkAppChange` | yes / yes, **no / yes**, yes / yes, yes / **no** |
//!
//! The last two rows are listed rather than hidden: `install::deps` maps each of
//! them to the capability it serves, so an absent anchor reports that capability
//! as disabled with the resolver's reason instead of silently doing nothing.

pub mod depot;
pub mod depot_store;
pub mod original;
pub mod ownership;
pub mod package;
pub mod state;
pub mod ticket;
pub mod ui;

#[cfg(windows)]
pub mod installer;

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
    (package::SYMBOL, package::address),
    (depot::SYMBOL, depot::address),
    (ticket::SYMBOL, ticket::address),
    (ui::visibility::SYMBOL, ui::visibility::address),
    (ui::overview::SYMBOL, ui::overview::address),
    (ui::snapshot::SYMBOL, ui::snapshot::address),
    (ui::change::SYMBOL, ui::change::address),
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
        assert!(replacement_address("CUser::GetSubscribedApps").is_none());
        assert!(replacement_address("CUser::BuildSpawnEnvBlock").is_none());
        assert_eq!(
            replacement_address(ownership::SYMBOL),
            Some(ownership::address())
        );
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
}
