//! The Lua overrides, composed on top of what Steam reported.
//!
//! The achievements path has exactly three inputs, in this order:
//!
//! 1. **definitions** — the achievements stat's schema, which owns the keys, the
//!    stat ids and the bits;
//! 2. **Steam's own unlock map** — what the reply (or the last snapshot) says,
//!    which is the donor account's state for a controlled app;
//! 3. **the user's Lua intent** — `setachievement`, applied last and only where
//!    the schema names the key.
//!
//! Step 3 is an override, not a source: it cannot invent an achievement, because
//! an override for a key no definition names is dropped (with one `debug` line
//! per app) and the reply is then left exactly as Steam sent it. And it never
//! has to be applied for the reply to be correct: with no overrides the composed
//! map is Steam's map, so [`ComposedUnlocks::changed`] is false and the caller
//! must not rewrite anything.
//!
//! Nothing here reads a file, a socket or a module image.

use std::collections::BTreeMap;

use tracing::debug;
use vapor_forge_core::AchievementIntent;

use super::local::{AchievementUnlockState, LocalAchievementSet};

/// The unlock map a reply should report, and what composing it cost.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ComposedUnlocks {
    /// Steam's unlock map with every applicable override applied. Same order as
    /// the input, so a caller that indexes it positionally keeps doing so.
    pub unlocks: Vec<AchievementUnlockState>,
    /// Overrides that named an achievement the definitions declare.
    pub applied: usize,
    /// Overrides dropped because no definition names the key.
    pub ignored: usize,
    /// Whether any applied override actually differs from what Steam reported.
    ///
    /// `false` means the composed map says what Steam already said, so the reply
    /// must be forwarded untouched: a swap would be a write with no effect.
    pub changed: bool,
}

/// Apply `overrides` on top of `unlocks`, keyed by achievement.
///
/// `definitions` is what the schema declares, so it is also what decides whether
/// an override is addressed at all. `overrides` is `None` or empty for an app
/// nothing was written for, which always yields `changed == false`.
pub fn compose_unlocks(
    app_id: u32,
    definitions: &LocalAchievementSet,
    unlocks: &[AchievementUnlockState],
    overrides: Option<&BTreeMap<String, AchievementIntent>>,
) -> ComposedUnlocks {
    let mut composed = ComposedUnlocks {
        unlocks: unlocks.to_vec(),
        ..ComposedUnlocks::default()
    };
    let Some(overrides) = overrides.filter(|overrides| !overrides.is_empty()) else {
        return composed;
    };

    let mut unknown: Vec<&str> = Vec::new();
    for (key, intent) in overrides {
        if definitions.achievement(key).is_none() {
            unknown.push(key.as_str());
            continue;
        }
        composed.applied += 1;
        match composed.unlocks.iter_mut().find(|state| state.key == *key) {
            // Replacing in place keeps the first-wins order `unlock_map` relies on:
            // appending a second entry for the same key would be shadowed.
            Some(state) => {
                let wanted = resolved(key, *intent);
                if state.unlocked != wanted.unlocked || state.unlock_time != wanted.unlock_time {
                    composed.changed = true;
                    *state = wanted;
                }
            }
            None => {
                composed.changed = true;
                composed.unlocks.push(resolved(key, *intent));
            }
        }
    }

    composed.ignored = unknown.len();
    if !unknown.is_empty() {
        // Once per app, not once per key: a script that guesses at a hundred keys
        // must not fill the log, and the line still names every key it dropped.
        debug!(
            app_id,
            dropped = unknown.len(),
            keys = ?unknown,
            "achievements: Lua overrides for keys the schema does not define were ignored"
        );
    }
    debug!(
        app_id,
        applied = composed.applied,
        ignored = composed.ignored,
        changed = composed.changed,
        "achievements: Lua overrides composed on top of Steam's unlock state"
    );
    composed
}

/// The state an override asks for, as the rebuild reads it.
///
/// A locked override carries no time: keeping Steam's would leave a locked
/// achievement with an unlock timestamp on the wire.
fn resolved(key: &str, intent: AchievementIntent) -> AchievementUnlockState {
    let unlock_time = intent.unlocked.then_some(intent.unlock_time).flatten();
    AchievementUnlockState {
        key: key.to_owned(),
        unlocked: intent.unlocked,
        unlock_time,
    }
}

#[cfg(test)]
mod tests;
