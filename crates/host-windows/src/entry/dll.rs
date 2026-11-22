//! `DllMain` — the only code of this host that runs under the loader lock.
//!
//! What the loader lock forbids is the reason this file is nine statements long:
//! no allocation, no file I/O, no `std::sync` lock, no hook installation, no
//! tracing subscriber setup. `DLL_PROCESS_ATTACH` therefore does exactly what is
//! safe — disable thread notifications and start one thread — and every other
//! step of the host happens on that thread (see [`crate::entry::worker`]).
//!
//! `DLL_PROCESS_DETACH` sets a stop flag and returns. It does not join the
//! worker, does not uninstall a detour, and does not flush the report: the
//! process is going away, and blocking here is how a Steam shutdown turns into a
//! hang. Unloading the host DLL keeps the hooks installed in `steamclient64.dll`
//! until the process exits, because their trampolines live in memory this DLL
//! allocated; that is a known limitation of a detour that outlives its DLL, and
//! the DLL is not meant to be unloaded while Steam runs.

use core::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `DLL_PROCESS_DETACH`.
const DLL_PROCESS_DETACH: u32 = 0;
/// `DLL_PROCESS_ATTACH`.
const DLL_PROCESS_ATTACH: u32 = 1;
/// `TRUE`, the value the loader expects for every reason.
const TRUE: i32 = 1;

/// Handle of the worker thread, kept so the entry point can report it and so a
/// detach can tell whether work was ever started.
static WORKER: AtomicUsize = AtomicUsize::new(0);

/// Handle of the worker thread, or 0 when none was started.
pub fn worker_handle() -> usize {
    WORKER.load(Ordering::Acquire)
}

/// The DLL entry point.
///
/// `module` is this DLL's handle, `reason` is one of the `DLL_PROCESS_*` /
/// `DLL_THREAD_*` values, and `reserved` is non-null only for a
/// `DLL_PROCESS_DETACH` caused by `FreeLibrary`.
///
/// # Safety
/// Called by the Windows loader. `module` must be a valid `HMODULE` for this DLL,
/// which the loader guarantees; nothing else is dereferenced here.
#[no_mangle]
pub extern "system" fn DllMain(module: *mut c_void, reason: u32, _reserved: *mut c_void) -> i32 {
    match reason {
        DLL_PROCESS_ATTACH => {
            crate::thread::disable_thread_library_calls(module);
            match crate::thread::spawn_worker(worker_entry) {
                Ok(handle) => {
                    WORKER.store(handle, Ordering::Release);
                }
                // Nothing to log with and nothing to allocate: the host simply
                // stays inert, and Steam runs exactly as without it.
                Err(_) => {
                    WORKER.store(0, Ordering::Release);
                }
            }
            TRUE
        }
        DLL_PROCESS_DETACH => {
            // Signal only: joining the worker or uninstalling a detour here would
            // block the loader while Steam is shutting down.
            crate::thread::request_stop();
            TRUE
        }
        _ => TRUE,
    }
}

/// Worker thread body.
///
/// A panic must never cross the FFI boundary, so the whole flow is inside
/// `catch_unwind`. The release profile builds with `panic = "abort"`, where no
/// unwind can occur at all; this exists for the dev profile, where a bug would
/// otherwise take Steam down with it.
extern "system" fn worker_entry(_parameter: *mut c_void) -> u32 {
    match std::panic::catch_unwind(crate::entry::windows::run_worker) {
        Ok(()) => 0,
        Err(_) => {
            // The panic payload may itself be a `String`; formatting it here is
            // the one allocation this thread may make, and it is outside any
            // loader lock.
            tracing::error!(
                "the host worker panicked; the client is left with Steam's own behaviour"
            );
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_reason_is_accepted_and_changes_nothing() {
        assert_eq!(
            DllMain(std::ptr::null_mut(), 0xdead, std::ptr::null_mut()),
            TRUE
        );
        assert!(!crate::thread::stop_requested());
    }
}
