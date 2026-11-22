//! The SteamUI bodies: what the library shows, and with what metadata.
//!
//! SteamUI is where an ingested app has to become visible at last. The
//! steamclient hooks in [`super`] decide ownership and packages, but the library
//! list is built by a different module with a different corpus
//! (`res/patterns/win64/steamui/<sha>.toml`), and this module is that module's
//! half of the host.
//!
//! | body | corpus symbol | capability | public / publicbeta |
//! | --- | --- | --- | --- |
//! | [`visibility`] | `ShouldShowAppInLibrary` | `LibraryUi` | yes / yes |
//! | [`overview`] | `CSteamUIAppController::FillInAppOverview` | `OverviewMetadata` | **no** / yes |
//! | [`snapshot`] | `CSteamUIAppController::BuildCompleteAppOverviewChange` | `LibrarySnapshot` | yes / yes |
//! | [`change`] | `CUpdateManager::MarkAppChange` | `LibrarySnapshot` | yes / **no** |
//!
//! Every body follows the shape the rest of [`crate::hooks`] uses: a pure
//! `decide`-style function over a [`HostState`](super::state::HostState) and plain
//! values, unit-tested on Linux, and a thin FFI body that calls it and otherwise
//! falls through to Steam's own answer. The call shapes, and which of their facts
//! are measured, are in [`crate::abi::ui`].
//!
//! Three of the four are safe by construction — one rewrites a `bool`, two only
//! report — and exactly one writes Steam memory: `FillInAppOverview` sets
//! `purchased_time` at an offset the Linux host measured and no Windows build
//! confirms. Its module docs and [`crate::abi::ui`] say so, and it is the single
//! assumption to check live.
//!
//! # What is deliberately not ported
//!
//! `crates/hooks/src/ui/` on Linux also removes apps from the library
//! (`queue_removal` → `BuildCompleteAppOverviewChange` appending to
//! `removed_appid`), re-marks changed apps through `GetAppByID` +
//! `CUpdateManager::MarkAppChange`, and drains the UI work queue from `RunFrame`.
//! None of the three is here, for the same reason: each needs either a corpus
//! symbol this host has no body for (`google::protobuf::RepeatedField<uint32>::Add`,
//! `CSteamUIAppController::GetAppByID`, `CSteamUIAppController::RunFrame`) or a
//! removal ledger that this host has no use for — it resolves the config once at
//! start-up and never drops an app from `[apps] inject` mid-session.
//!
//! # Known gap
//!
//! `install::deps` disables a capability when one of *its* symbols is in the
//! corpus and does not resolve. A symbol missing from a corpus file entirely
//! cannot disable anything that way, and the two builds disagree: the public
//! corpus has no `FillInAppOverview` entry, the publicbeta corpus has no
//! `MarkAppChange` entry. On those builds the corresponding capability is
//! reported enabled while no body is installed for that symbol. Closing it needs a
//! pass over [`super::REPLACEMENTS`] after the batch (in `install`), not a change
//! to this module; the bodies themselves are unreachable until their anchor
//! installs, so nothing misbehaves in the meantime.

pub mod change;
pub mod overview;
pub mod snapshot;
pub mod visibility;

#[cfg(test)]
mod tests;
