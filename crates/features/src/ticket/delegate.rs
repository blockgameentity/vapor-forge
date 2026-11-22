//! Delegate mode: serve a cached ticket (from a previous owner session) during
//! an initial request window, then switch to derived mode.
//!
//! The window is counted in requests, not in time, and that is deliberate: it
//! covers the initial handshakes DRM makes and then closes, so the rest of the
//! Steam session runs under the real user.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use vapor_forge_config::AppId;

// ---------------------------------------------------------------------------
// Auto-delegate: runtime-detected Denuvo apps
// ---------------------------------------------------------------------------

static AUTO_DELEGATE_APPS: Mutex<Option<HashSet<AppId>>> = Mutex::new(None);

/// Mark an app as auto-detected Denuvo. Subsequent `is_auto_delegate`
/// checks will return true for this app. Does not modify config.
pub fn add_auto_delegate(app_id: AppId) {
    let mut guard = AUTO_DELEGATE_APPS.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(HashSet::new).insert(app_id);
}

/// Check if an app was auto-detected as Denuvo at runtime.
pub fn is_auto_delegate(app_id: AppId) -> bool {
    AUTO_DELEGATE_APPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|s| s.contains(&app_id))
}

/// Clear auto-delegate for an app (e.g. on game exit).
pub fn remove_auto_delegate(app_id: AppId) {
    if let Some(set) = AUTO_DELEGATE_APPS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        set.remove(&app_id);
    }
}

// ---------------------------------------------------------------------------
// Delegate window
// ---------------------------------------------------------------------------

/// Number of ticket requests per app that get served from cache before we
/// switch over to forging with the current user's SteamID.
const DELEGATE_WINDOW_SIZE: u32 = 2;

/// Per-AppId count of ticket requests seen so far in delegate mode.
static DELEGATE_COUNTS: Mutex<Option<HashMap<AppId, u32>>> = Mutex::new(None);

/// Per-AppId SteamID that GetSteamID reports to that app while its delegate
/// window is active.
static DELEGATE_STEAMIDS: Mutex<Option<HashMap<AppId, u64>>> = Mutex::new(None);

/// Check if we're still in the delegate window for this app.
///
/// Each call counts as one request. Returns true (serve cached ticket) for
/// the first `DELEGATE_WINDOW_SIZE` calls, then false afterwards. Mirrors
/// OpenSteamTool's DenuvoAuth `IsAuthorizedPipe`: the window covers the
/// initial handshakes DRM cares about, then closes — after which
/// `GetSteamID` must stop reporting the borrowed identity so the rest of
/// the Steam session runs under the real user.
pub fn in_delegate_window(app_id: AppId) -> bool {
    let mut guard = DELEGATE_COUNTS.lock().unwrap_or_else(|e| e.into_inner());
    let counts = guard.get_or_insert_with(HashMap::new);
    let count = counts.entry(app_id).or_insert(0);
    *count += 1;
    let in_window = *count <= DELEGATE_WINDOW_SIZE;
    drop(guard);
    if !in_window {
        clear_delegate_steamid(app_id);
    }
    in_window
}

/// Reset the delegate window for an app. Called when the game exits
/// (observed via CMsgClientGamesPlayed no longer listing the app), so a
/// future relaunch gets a fresh delegate window.
pub fn reset_delegate_window(app_id: AppId) {
    if let Some(counts) = DELEGATE_COUNTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        counts.remove(&app_id);
    }
    clear_delegate_steamid(app_id);
}

/// Set the SteamID that GetSteamID reports to `app_id` while its delegate
/// window is active.
pub fn set_delegate_steamid(app_id: AppId, steamid: u64) {
    DELEGATE_STEAMIDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(app_id, steamid);
}

/// Get the delegate SteamID for `app_id` (0 = no delegate active).
pub fn delegate_steamid(app_id: AppId) -> u64 {
    DELEGATE_STEAMIDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .and_then(|ids| ids.get(&app_id).copied())
        .unwrap_or(0)
}

/// Clear the delegate SteamID for `app_id` (called when its window closes or
/// the game exits).
pub fn clear_delegate_steamid(app_id: AppId) {
    if let Some(ids) = DELEGATE_STEAMIDS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
    {
        ids.remove(&app_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Delegate tests share process-global state, so the test lock serializes them.
    static DELEGATE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn auto_delegate_set_is_per_app() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app = AppId(100_010);
        assert!(!is_auto_delegate(app));
        add_auto_delegate(app);
        assert!(is_auto_delegate(app));
        assert!(!is_auto_delegate(AppId(100_011)));
        remove_auto_delegate(app);
        assert!(!is_auto_delegate(app));
    }

    #[test]
    fn delegate_window_allows_first_n_requests_then_forges() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app = AppId(100_001);
        // First DELEGATE_WINDOW_SIZE (2) requests: still in window.
        assert!(in_delegate_window(app));
        assert!(in_delegate_window(app));
        // Third request: window closed.
        assert!(!in_delegate_window(app));
        assert!(!in_delegate_window(app));
    }

    #[test]
    fn delegate_window_is_independent_per_app() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app_a = AppId(100_002);
        let app_b = AppId(100_003);

        assert!(in_delegate_window(app_a));
        // app_b's window is unaffected by app_a's count.
        assert!(in_delegate_window(app_b));
        assert!(in_delegate_window(app_a));
        assert!(in_delegate_window(app_b));
        // Both windows close after their own 2 requests.
        assert!(!in_delegate_window(app_a));
        assert!(!in_delegate_window(app_b));
    }

    #[test]
    fn reset_delegate_window_restarts_the_count() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app = AppId(100_004);
        assert!(in_delegate_window(app));
        assert!(in_delegate_window(app));
        assert!(!in_delegate_window(app));

        reset_delegate_window(app);

        // Window reopened: first two requests are back in-window.
        assert!(in_delegate_window(app));
        assert!(in_delegate_window(app));
        assert!(!in_delegate_window(app));
    }

    #[test]
    fn delegate_steamid_set_get_clear() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app = AppId(100_006);
        set_delegate_steamid(app, 76561198000000001);
        assert_eq!(delegate_steamid(app), 76561198000000001);
        clear_delegate_steamid(app);
        assert_eq!(delegate_steamid(app), 0);
    }

    #[test]
    fn delegate_steamid_is_independent_per_app() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app_a = AppId(100_007);
        let app_b = AppId(100_008);
        reset_delegate_window(app_a);
        reset_delegate_window(app_b);
        set_delegate_steamid(app_a, 76561198000000003);
        set_delegate_steamid(app_b, 76561198000000004);
        assert_eq!(delegate_steamid(app_a), 76561198000000003);
        assert_eq!(delegate_steamid(app_b), 76561198000000004);

        // Closing one app's window leaves the other app's identity in place.
        assert!(in_delegate_window(app_a));
        assert!(in_delegate_window(app_a));
        assert!(!in_delegate_window(app_a));
        assert_eq!(delegate_steamid(app_a), 0);
        assert_eq!(delegate_steamid(app_b), 76561198000000004);

        reset_delegate_window(app_b);
        assert_eq!(delegate_steamid(app_b), 0);
    }

    #[test]
    fn delegate_window_closing_clears_steamid() {
        let _guard = DELEGATE_TEST_LOCK.lock().unwrap();
        let app = AppId(100_005);
        reset_delegate_window(app);
        assert!(in_delegate_window(app));
        assert!(in_delegate_window(app));
        set_delegate_steamid(app, 76561198000000002);
        // Window closes → GetSteamID must fall back to the real user, so the
        // delegate SteamID gets cleared (mirrors OpenSteamTool's DenuvoAuth
        // `IsAuthorizedPipe` returning false past the window).
        assert!(!in_delegate_window(app));
        assert_eq!(delegate_steamid(app), 0);
    }
}
