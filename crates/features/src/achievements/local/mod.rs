//! Local achievement rebuild.
//!
//! Steam answers a stats request with achievement *state*; the *definitions* a
//! client renders (names, descriptions, groups) come from the app's schema, which
//! the client only holds if it managed to fetch one. An app whose schema was never
//! fetched - a controlled app, a store page that cannot resolve the definition
//! table, a layout this build cannot read - therefore shows a blank achievement
//! page even though the account has unlocked achievements.
//!
//! This module answers such a request from locally known definitions instead:
//! [`rebuild_local_achievements`] turns a local set plus the account's unlock
//! state into one deterministically ordered list with display attributes, group
//! memberships and totals that agree with the unlocks, and
//! [`RebuiltAchievements::service_response`] / `legacy_response` encode that list
//! into the two wire shapes Steam asks in.
//!
//! What counts as the source of truth:
//!
//! * a schema that parses and declares achievements wins: it owns the wire ids
//!   and the definition list;
//! * anything else - no schema, unparsable bytes, or a layout whose achievements
//!   are nested under groups - falls back to the local set, which is what keeps a
//!   modern grouped layout from blanking the page.
//!
//! The whole path is behind [`set_local_rebuild`], which a host applies from its
//! config. That flag is on by default and inert without a Lua override: the reply
//! is composed as schema definitions, then Steam's own unlock map, then the
//! `setachievement` intent ([`super::overrides`]), and a composition that says
//! what Steam already said is not swapped in.

mod definitions;
mod groups;
mod model;
mod rebuild;
#[cfg(test)]
mod tests;
mod wire;

pub use model::{
    AchievementDisplay, AchievementUnlockState, LocalAchievement, LocalAchievementGroup,
    LocalAchievementSet, LocalDefinitionError, MAX_LOCAL_ACHIEVEMENT_BIT,
};
pub use rebuild::{
    local_rebuild_enabled, local_rebuild_settings, rebuild_local_achievements,
    rebuild_local_achievements_if_enabled, set_local_rebuild, AchievementCounts,
    LocalRebuildSettings, RebuildSource, RebuiltAchievement, RebuiltAchievementGroup,
    RebuiltAchievements,
};
