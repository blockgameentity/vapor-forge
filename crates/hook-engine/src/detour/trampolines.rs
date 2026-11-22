//! Trampolines this process has created, in creation order.
//!
//! An install pass may be followed by a hardening pass, and a later pass has to
//! reopen the same pages, so the addresses are kept here. Each entry is a raw
//! trampoline address; page rounding is the protection backend's job.
//!
//! The registry is only load-bearing for the unix backend, whose `retour`
//! trampolines are created writable and have to be hardened afterwards. The
//! Windows writer hardens its own allocation before it returns, so the entries
//! recorded there are already read-execute and the calls below are no-ops that
//! still keep one bookkeeping path for both targets.

use std::sync::Mutex;

use tracing::{debug, warn};

use crate::protect::{self, Protection};

static TRAMPOLINES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Reopen every known trampoline page for writing.
///
/// Only the unix backend needs this: its trampolines are allocated writable and
/// hardened afterwards, and PIC repair patches them on the next install pass.
pub(crate) fn make_writable() {
    #[cfg(unix)]
    set_protection(Protection::READ_WRITE_EXECUTE, "RWX");
}

/// Harden every known trampoline page back to read-execute.
pub fn restore_rx() {
    set_protection(Protection::READ_EXECUTE, "RX");
}

/// Record a trampoline so later protection passes include it.
pub(crate) fn remember(trampoline: usize) {
    let Ok(mut trampolines) = TRAMPOLINES.lock() else {
        warn!("trampoline list is poisoned, trampoline protection not tracked");
        return;
    };
    if !trampolines.contains(&trampoline) {
        trampolines.push(trampoline);
    }
}

fn set_protection(protection: Protection, label: &str) {
    let Ok(trampolines) = TRAMPOLINES.lock() else {
        warn!("trampoline list is poisoned, leaving trampoline protection alone");
        return;
    };
    if trampolines.is_empty() {
        return;
    }
    let mut failures = 0usize;
    for &trampoline in trampolines.iter() {
        // `trampoline` is the start of a trampoline for this process: it stays
        // mapped until the detour that owns it is dropped, and one byte is enough
        // for the backend to find the page that contains it.
        if protect::set_protection(trampoline, 1, protection).is_err() {
            failures += 1;
        }
    }
    let trampolines = trampolines.len();
    if failures == 0 {
        debug!(
            trampolines,
            protection = label,
            "trampoline protection applied"
        );
    } else {
        warn!(
            trampolines,
            failures,
            protection = label,
            "trampoline protection failed"
        );
    }
}
