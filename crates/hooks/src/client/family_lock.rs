use super::family_decide;
use core::ffi::c_void;
use vapor_forge_config::AppId;
use vapor_forge_hook_engine::detour::Detour;
use vapor_forge_hook_engine::original::detour_or_return;

pub(crate) const GET_LOCKING_ACCOUNT_NAME: &str = "CFamilySharing::GetLockingAccount";
pub(crate) type GetLockingAccountFn = unsafe extern "C" fn(*mut c_void, u32) -> u32;
pub(crate) static mut GET_LOCKING_ACCOUNT_DETOUR: Option<Detour<GetLockingAccountFn>> = None;

/// Answers Steam's "who holds this app" lookup.
///
/// Steam blocks launching an app, flags it as in use and shows the locking
/// member in the UI from this lookup; the lock table itself stays intact.
///
/// The answer is `features::family::concurrent`'s, not this file's: a lock held
/// by another member becomes a [`vapor_forge_features::family::concurrent::FamilyLock`]
/// and the shared decision says whether the app may start anyway. The allow set
/// is unchanged — controlled apps and whatever `[apps.shared]` allows — but the
/// reason and the provenance it was made from are now logged once, by the
/// feature, instead of being re-derived here.
pub(crate) unsafe extern "C" fn hk_get_locking_account(family: *mut c_void, app_id: u32) -> u32 {
    let original = detour_or_return!(GET_LOCKING_ACCOUNT_NAME, GET_LOCKING_ACCOUNT_DETOUR, 0);
    // SAFETY: forwards Steam's family object and app ID to the original lookup.
    let account = unsafe { original(family, app_id) };
    if account == 0 || !crate::capability::is_ready(crate::capability::Capability::FamilyLockBypass)
    {
        return account;
    }
    let app = AppId(app_id);
    let config = crate::client::install::config();
    let settings = family_decide::settings();
    let injected_by_us =
        family_decide::injected_by_us(crate::client::install::package_state(), app);
    let family = family_decide::label(&config, app, injected_by_us, settings);
    let decision = family_decide::concurrent_play(&config, app, account, family.real);
    if decision.allowed {
        0
    } else {
        account
    }
}
