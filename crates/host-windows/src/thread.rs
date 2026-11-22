//! The one raw Win32 call the entry point makes that is not module memory.
//!
//! `DllMain` runs under the loader lock, so it may not allocate, take a lock, or
//! touch the filesystem. Creating a thread is one of the few things it *may* do,
//! which is why the whole host runs on a thread of its own and the entry point
//! only hands off.
//!
//! Nothing else in this crate creates a thread. On a non-Windows host
//! [`spawn_worker`] reports that instead of pretending, so the crate keeps
//! building and testing anywhere.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::HostError;

/// Set by `DLL_PROCESS_DETACH`; the worker checks it between steps.
static STOP: AtomicBool = AtomicBool::new(false);

/// Ask the worker to stop at its next step boundary. Never blocks, so the loader
/// can call it from `DLL_PROCESS_DETACH` without risking a hang.
pub fn request_stop() {
    STOP.store(true, Ordering::Release);
}

/// Whether a stop was requested.
pub fn stop_requested() -> bool {
    STOP.load(Ordering::Acquire)
}

/// Body of the worker thread: `extern "system"` because Win32 calls it, `u32`
/// because that is `DWORD`.
pub type StartRoutine = extern "system" fn(*mut core::ffi::c_void) -> u32;

/// Start `routine` on a new OS thread and return the thread handle as an
/// address.
///
/// The handle is deliberately *not* closed: it is the only way to tell whether
/// the worker is still alive, and it costs one handle for the process lifetime.
pub fn spawn_worker(routine: StartRoutine) -> Result<usize, HostError> {
    imp::spawn_worker(routine)
}

/// Tell the loader this DLL does not need thread attach/detach notifications.
///
/// Called from `DLL_PROCESS_ATTACH`, before the worker exists: with no
/// thread-attach callbacks the loader does not re-enter this DLL for every thread
/// Steam creates, which is both faster and one fewer path under the loader lock.
pub fn disable_thread_library_calls(module: *mut core::ffi::c_void) {
    imp::disable_thread_library_calls(module);
}

#[cfg(windows)]
mod imp {
    use super::{HostError, StartRoutine};

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateThread(
            attributes: *mut core::ffi::c_void,
            stack_size: usize,
            start: Option<StartRoutine>,
            parameter: *mut core::ffi::c_void,
            flags: u32,
            thread_id: *mut u32,
        ) -> *mut core::ffi::c_void;
        fn DisableThreadLibraryCalls(module: *mut core::ffi::c_void) -> i32;
    }

    pub fn spawn_worker(routine: StartRoutine) -> Result<usize, HostError> {
        let mut thread_id = 0u32;
        // SAFETY: a plain CreateThread with no caller pointer beyond the routine
        // address; the routine has the exact Win32 start signature.
        let handle = unsafe {
            CreateThread(
                std::ptr::null_mut(),
                0,
                Some(routine),
                std::ptr::null_mut(),
                0,
                &mut thread_id,
            )
        };
        if handle.is_null() {
            return Err(HostError::Unsupported(
                "CreateThread refused the host worker".to_owned(),
            ));
        }
        tracing::debug!(
            thread_id,
            handle = format_args!("{handle:p}"),
            "host worker thread started"
        );
        Ok(handle as usize)
    }

    pub fn disable_thread_library_calls(module: *mut core::ffi::c_void) {
        // SAFETY: `module` is this DLL's handle, passed to DllMain by the loader.
        // The call has no other preconditions and its result is advisory.
        let ok = unsafe { DisableThreadLibraryCalls(module) };
        if ok == 0 {
            tracing::debug!("DisableThreadLibraryCalls was refused; thread callbacks stay enabled");
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{HostError, StartRoutine};

    pub fn spawn_worker(_routine: StartRoutine) -> Result<usize, HostError> {
        Err(HostError::Unsupported(
            "only a Windows host spawns the worker from a DLL entry point".to_owned(),
        ))
    }

    pub fn disable_thread_library_calls(_module: *mut core::ffi::c_void) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_non_windows_host_reports_that_it_cannot_spawn_a_worker() {
        if cfg!(windows) {
            return;
        }
        extern "system" fn never(_: *mut core::ffi::c_void) -> u32 {
            0
        }
        assert!(matches!(
            spawn_worker(never),
            Err(HostError::Unsupported(_))
        ));
        // Must be a no-op, not a crash, on a host without the loader.
        disable_thread_library_calls(std::ptr::null_mut());
    }
}
