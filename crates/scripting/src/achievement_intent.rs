//! `setachievement(appid, key, value)` — the Lua half of the achievement override.
//!
//! This is the only place a user can state achievement *intent*. Everything else
//! on the achievements path is derived: definitions from the schema a stats reply
//! carries, unlock state from Steam's own reply. An override is applied last (see
//! `vapor_forge_features::achievements::compose_unlocks`), so it can add an
//! achievement the account never unlocked, or clear one it did.
//!
//! Two accepted forms, chosen by the third argument's type:
//!
//! ```lua
//! setachievement(480, "ACH_WIN", true)          -- unlocked, time unknown
//! setachievement(480, "ACH_WIN", false)         -- locked: clears the time
//! setachievement(480, "ACH_WIN", 1700000000)    -- unlocked at that rtime32
//! setachievement(480, "ACH_WIN", 0)             -- unlocked, time unknown
//! ```
//!
//! Validation is total and a rejection is a script error, exactly like
//! `addappid`'s: an app id of `0`, a blank or over-long key, a negative time, a
//! time past `u32::MAX`, or a third argument that is neither a boolean nor an
//! integer. Nothing is written when validation fails, so a bad call costs the
//! line it is on, never the file's other statements — and the loader reports the
//! line number the way it reports every other binding error.

use mlua::prelude::*;
use vapor_forge_core::{AchievementIntent, AppId};

use crate::bindings::{lock_shared, register_function, RuntimeState, ScriptRunContext, Shared};

/// The binding name, spelled once so the registration and the errors agree.
const NAME: &str = "setachievement";

pub(crate) fn install(
    lua: &Lua,
    globals: &LuaTable,
    registry: &LuaTable,
    state: &Shared<RuntimeState>,
    ctx: &ScriptRunContext,
) -> Result<(), mlua::Error> {
    let state = state.clone();
    let ctx = ctx.clone();
    register_function(
        globals,
        registry,
        NAME,
        lua.create_function(move |_, args: LuaMultiValue| {
            let app_id = parse_app_id(args.front())?;
            let key = parse_key(args.get(1))?;
            let intent = parse_intent(args.get(2))?;

            lock_shared(&state)
                .achievement_overrides
                .entry(AppId(app_id))
                .or_default()
                .insert(key.clone(), intent);
            ctx.push_call(NAME, describe(app_id, &key, intent));
            Ok(())
        })?,
    )
}

/// The app the override is for. Zero is not an app, so it is a script error.
fn parse_app_id(value: Option<&LuaValue>) -> LuaResult<u32> {
    let Some(value) = value else {
        return Err(error("arg1 (appid) is required"));
    };
    let app_id = match value {
        LuaValue::Integer(raw) => u32::try_from(*raw).ok(),
        // LuaJIT stores every number as a double, so an integral value can arrive
        // as a float. Anything with a fraction is not an app id.
        LuaValue::Number(raw) if raw.fract() == 0.0 => u32::try_from(*raw as i64).ok(),
        _ => None,
    };
    match app_id {
        Some(0) | None => Err(error("arg1 (appid) must be an integer in 1..=4294967295")),
        Some(app_id) => Ok(app_id),
    }
}

/// The achievement key. Blank, over-long and NUL-carrying keys are refused.
fn parse_key(value: Option<&LuaValue>) -> LuaResult<String> {
    let Some(LuaValue::String(value)) = value else {
        return Err(error("arg2 (key) must be a non-empty string"));
    };
    let key = value.to_str()?.to_owned();
    if AchievementIntent::key_is_valid(&key) {
        Ok(key)
    } else {
        Err(error(&format!(
            "arg2 (key) must be 1..={} bytes, not blank and not containing NUL",
            vapor_forge_core::MAX_ACHIEVEMENT_KEY_LEN
        )))
    }
}

/// The state the override asks for, from a boolean or an integer time.
fn parse_intent(value: Option<&LuaValue>) -> LuaResult<AchievementIntent> {
    match value {
        Some(LuaValue::Boolean(unlocked)) => Ok(if *unlocked {
            AchievementIntent::unlocked()
        } else {
            AchievementIntent::locked()
        }),
        // A positive integer is the rtime32 to show. Zero is Steam's own "time
        // unknown" value, so it means "unlocked, no time" rather than a lock.
        Some(LuaValue::Integer(raw)) => parse_time(*raw),
        Some(LuaValue::Number(raw)) if raw.fract() == 0.0 => parse_time(*raw as i64),
        Some(LuaValue::Number(_)) => Err(error("arg3 (unlocked) must not have a fractional part")),
        _ => Err(error(
            "arg3 (unlocked) must be a boolean or a non-negative integer unix time",
        )),
    }
}

fn parse_time(raw: i64) -> LuaResult<AchievementIntent> {
    if raw < 0 {
        return Err(error("arg3 (unlocked) time must not be negative"));
    }
    let time = u32::try_from(raw)
        .map_err(|_| error("arg3 (unlocked) time must fit in an unsigned 32-bit rtime32"))?;
    Ok(if time == 0 {
        AchievementIntent::unlocked()
    } else {
        AchievementIntent::unlocked_at(time)
    })
}

fn describe(app_id: u32, key: &str, intent: AchievementIntent) -> String {
    match (intent.unlocked, intent.unlock_time) {
        (true, Some(time)) => format!("app_id={app_id} key={key} unlocked=true time={time}"),
        (true, None) => format!("app_id={app_id} key={key} unlocked=true time=(unknown)"),
        (false, _) => format!("app_id={app_id} key={key} unlocked=false"),
    }
}

/// A script error naming the binding and the problem, never a panic.
fn error(detail: &str) -> mlua::Error {
    mlua::Error::RuntimeError(format!("{NAME}: {detail}"))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A Lua instance with only this binding installed, plus the state it writes.
    fn harness() -> (Lua, Shared<RuntimeState>) {
        let lua = Lua::new();
        let state: Shared<RuntimeState> = Shared::new(Mutex::new(RuntimeState::default()));
        let ctx = ScriptRunContext::new(false);
        // The loader installs this before every binding, and it is what makes
        // `ADDAPPID` work as well as `addappid`.
        let registry = crate::bindings::install_case_insensitive_lookup(&lua, &lua.globals())
            .expect("a fresh registry table");
        install(&lua, &lua.globals(), &registry, &state, &ctx).expect("the binding installs");
        (lua, state)
    }

    fn overrides(
        state: &Shared<RuntimeState>,
    ) -> std::collections::HashMap<AppId, std::collections::BTreeMap<String, AchievementIntent>>
    {
        lock_shared(state).achievement_overrides.clone()
    }

    #[test]
    fn a_boolean_writes_an_override() {
        let (lua, state) = harness();
        lua.load("setachievement(480, 'ACH_WIN', true)")
            .exec()
            .expect("true is a valid intent");
        // The loader is case-insensitive for every binding, so this one is too.
        lua.load("SETACHIEVEMENT(480, 'ACH_LOSE', false)")
            .exec()
            .expect("false is a valid intent");

        let overrides = overrides(&state);
        let app = &overrides[&AppId(480)];
        assert_eq!(app["ACH_WIN"], AchievementIntent::unlocked());
        assert_eq!(app["ACH_LOSE"], AchievementIntent::locked());
    }

    #[test]
    fn an_integer_writes_an_unlock_time() {
        let (lua, state) = harness();
        lua.load("setachievement(480, 'ACH_WIN', 1700000000)")
            .exec()
            .expect("a positive time is a valid intent");

        let overrides = overrides(&state);
        assert_eq!(
            overrides[&AppId(480)]["ACH_WIN"],
            AchievementIntent::unlocked_at(1_700_000_000)
        );
    }

    #[test]
    fn zero_time_is_unlocked_with_no_time() {
        let (lua, state) = harness();
        lua.load("setachievement(480, 'ACH_WIN', 0)")
            .exec()
            .expect("zero is Steam's own time-unknown value");

        assert_eq!(
            overrides(&state)[&AppId(480)]["ACH_WIN"],
            AchievementIntent::unlocked()
        );
    }

    #[test]
    fn bad_input_is_a_script_error_and_writes_nothing() {
        for source in [
            "setachievement(0, 'ACH_WIN', true)",
            "setachievement(-1, 'ACH_WIN', true)",
            "setachievement('480', 'ACH_WIN', true)",
            "setachievement(480, '', true)",
            "setachievement(480, '   ', true)",
            "setachievement(480, 42, true)",
            "setachievement(480, 'ACH_WIN')",
            "setachievement(480, 'ACH_WIN', 'yes')",
            "setachievement(480, 'ACH_WIN', -1)",
            "setachievement(480, 'ACH_WIN', 1.5)",
            "setachievement(480, 'ACH_WIN', 4294967296)",
        ] {
            let (lua, state) = harness();
            let Err(error) = lua.load(source).exec() else {
                panic!("{source} must be a script error");
            };
            let message = error.to_string();
            assert!(message.contains("setachievement:"), "{source}: {message}");
            assert!(overrides(&state).is_empty(), "{source} must write nothing");
        }

        let long_key = "A".repeat(vapor_forge_core::MAX_ACHIEVEMENT_KEY_LEN + 1);
        let (lua, state) = harness();
        assert!(lua
            .load(format!("setachievement(480, '{long_key}', true)"))
            .exec()
            .is_err());
        assert!(overrides(&state).is_empty());
    }

    #[test]
    fn a_rejected_call_leaves_an_accepted_one_alone() {
        let (lua, state) = harness();
        lua.load("setachievement(480, 'ACH_WIN', true)")
            .exec()
            .expect("a valid intent");
        assert!(lua
            .load("setachievement(480, 'ACH_WIN', -1)")
            .exec()
            .is_err());

        assert_eq!(
            overrides(&state)[&AppId(480)]["ACH_WIN"],
            AchievementIntent::unlocked()
        );
    }
}
