//! The `RegisterJsMethod` body: record the bridge's own registrations.
//!
//! # Verified
//!
//! `RegisterJsMethod` is anchored in `SteamUI.dll` for both live builds (it
//! resolves at `0x20d430` public and passes the anchor body gate), and its second
//! argument is a NUL-terminated method name — the same shape the Linux bridge
//! hooks.
//!
//! # Assumed
//!
//! Arguments 3 and 4 are forwarded verbatim and never interpreted; the return
//! byte is Steam's and is returned untouched. The name is read through a bounded
//! copy, so a malformed pointer cannot walk out of the module.
//!
//! # What it does, and does not, do
//!
//! It records that a bridge method was registered, so a live client's log says
//! whether the reverse bridge exists at all. It does **not** drain a choice: the
//! generation/readiness rules in [`super::decide`] need a window the host does not
//! publish yet, and draining without one would apply a stale choice to the wrong
//! session — the failure those rules exist to prevent.

use core::ffi::{c_char, c_void};

use tracing::{debug, trace};

use crate::capability::Capability;

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "RegisterJsMethod";

/// Longest method name copied out of Steam's string before it is truncated.
const MAX_NAME: usize = 96;

/// Prefix the host's own bridge methods carry (the Linux bridge registers
/// `Apps.VaporForge…`); anything else is Steam's own and is left alone.
pub const BRIDGE_PREFIX: &str = "Apps.VaporForge";

type RegisterJsMethodFn =
    unsafe extern "system" fn(*mut c_void, *const c_char, *mut c_void, usize) -> u8;

/// Address of [`register_js_method`], for the installer's replacement table.
pub fn address() -> usize {
    register_js_method as RegisterJsMethodFn as usize
}

/// Replacement for `RegisterJsMethod`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: `this`, `name`,
/// `method` and `r9` are whatever Steam passed and are forwarded unchanged.
pub unsafe extern "system" fn register_js_method(
    this: *mut c_void,
    name: *const c_char,
    method: *mut c_void,
    r9: usize,
) -> u8 {
    // SAFETY: the trampoline was published for this symbol before the patch.
    let Some(original) =
        (unsafe { crate::hooks::original::original_of::<RegisterJsMethodFn>(SYMBOL) })
    else {
        debug!("conflict-ui: registration reached without a published original");
        return 0;
    };

    // SAFETY: `name` is Steam's own argument; the reader is bounded and
    // NUL-terminated, and a null or unreadable-looking pointer yields `None`.
    match unsafe { read_name(name) } {
        Some(name)
            if name.starts_with(BRIDGE_PREFIX)
                && crate::hooks::state::get()
                    .is_some_and(|state| state.allows(Capability::ConflictUiBridge)) =>
        {
            debug!(method = name.as_str(), "conflict-ui: bridge method registered");
        }
        _ => trace!("conflict-ui: non-bridge or unreadable registration forwarded unchanged"),
    }

    // SAFETY: every slot is forwarded verbatim.
    unsafe { original(this, name, method, r9) }
}

/// Copy a bounded, NUL-terminated name. `None` for a null or empty pointer.
///
/// # Safety
/// `name` must be null or point at readable memory; the copy stops at `MAX_NAME`
/// bytes and at the first NUL, so no read goes past either bound.
unsafe fn read_name(name: *const c_char) -> Option<String> {
    if name.is_null() {
        return None;
    }
    let mut bytes = Vec::with_capacity(MAX_NAME);
    for index in 0..MAX_NAME {
        // SAFETY: the caller's contract bounds the read to this window.
        let byte = unsafe { *name.add(index) } as u8;
        if byte == 0 {
            break;
        }
        bytes.push(byte);
    }
    if bytes.is_empty() {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn a_null_name_is_none_rather_than_a_read() {
        // SAFETY: null is explicitly allowed by the contract.
        assert!(unsafe { read_name(core::ptr::null()) }.is_none());
    }

    #[test]
    fn a_name_is_copied_up_to_the_terminator_and_the_bound() {
        let name = CString::new("Apps.VaporForgeResolveCloudConflict").unwrap();
        // SAFETY: a live CString, NUL-terminated.
        let read = unsafe { read_name(name.as_ptr()) }.expect("readable");
        assert_eq!(read, "Apps.VaporForgeResolveCloudConflict");
        assert!(read.starts_with(BRIDGE_PREFIX));

        let long = CString::new("A".repeat(MAX_NAME * 2)).unwrap();
        // SAFETY: as above.
        let read = unsafe { read_name(long.as_ptr()) }.expect("readable");
        assert_eq!(read.len(), MAX_NAME, "the copy must stop at the bound");
    }

    #[test]
    fn an_empty_name_is_none() {
        let name = CString::new("").unwrap();
        // SAFETY: a live CString with an immediate terminator.
        assert!(unsafe { read_name(name.as_ptr()) }.is_none());
    }
}
