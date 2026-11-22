//! The `SetAPICallResult` body — the FFI half of [`crate::hooks::callbacks`].
//!
//! Split from the decision for the reason every hook in this crate is split, plus
//! one of its own: the decision module is a table ([`crate::hooks::callbacks`]'s
//! registrations and rules) and this module is the call that applies it, so the
//! table stays readable and small.
//!
//! The body calls the original first — Steam's own bookkeeping is never delayed or
//! reordered — and then applies the decision to the result's callback id and
//! payload size. On this host the two measured facts are `api_call` and
//! `target_pipe`; the callback id and the payload size are among the stack slots,
//! which [`crate::abi::callbacks`] states as assumed. A wrong assumption there
//! cannot corrupt anything: those slots are forwarded verbatim, so Steam sees
//! exactly what it passed, and the decision merely answers "not ours" to whatever
//! it read.
//!
//! What is still missing is the *consumer*: the Windows host has no delivery sink
//! wired (the Linux host's `callback_dispatch` has no counterpart here yet), so the
//! body reports the decision at `debug` and returns. A body that claimed delivery
//! without a sink would be a lie in the log.

use core::ffi::c_void;

use tracing::{debug, trace};

use crate::abi::callbacks::SetApiCallResultFn;
use crate::hooks::callbacks::{decide, Delivery};
use crate::hooks::{original, state};

/// Corpus symbol this body replaces.
pub const SYMBOL: &str = "CSteamEngine::SetAPICallResult";

/// Address of [`set_api_call_result`], for the installer's replacement table.
pub fn address() -> usize {
    set_api_call_result as SetApiCallResultFn as usize
}

/// Replacement for `CSteamEngine::SetAPICallResult`.
///
/// # Safety
/// Called by Steam through the detour that displaced the entry: every slot must be
/// whatever Steam passed. `payload` is forwarded and never dereferenced.
pub unsafe extern "system" fn set_api_call_result(
    engine: *mut c_void,
    client_context: *mut c_void,
    api_call: u64,
    target_pipe: u32,
    payload: *mut c_void,
    payload_size: i32,
    callback: i32,
) -> usize {
    // SAFETY: the signature is the one documented in `crate::abi::callbacks`, and
    // the trampoline was published before the entry patch was enabled.
    let Some(original) = (unsafe { original::original_of::<SetApiCallResultFn>(SYMBOL) }) else {
        debug!("callback hook reached without a published original; dropping the result");
        return 0;
    };
    // SAFETY: every slot is forwarded verbatim; Steam's own work happens first.
    let result = unsafe {
        original(
            engine,
            client_context,
            api_call,
            target_pipe,
            payload,
            payload_size,
            callback,
        )
    };
    observe(api_call, callback, payload_size);
    result
}

/// Apply [`decide`] to one stored API-call result.
///
/// Split from the FFI body so the decision stays unit-tested: this half only reads
/// two scalars and logs.
fn observe(api_call: u64, callback: i32, payload_size: i32) {
    let Some(state) = state::get() else {
        return;
    };
    let payload_len = usize::try_from(payload_size).unwrap_or(0);
    match decide(state, callback, payload_len) {
        Delivery::Deliver => debug!(
            api_call,
            callback,
            payload_len,
            "callback: a registered api-call result was seen; this host has no delivery sink wired yet"
        ),
        Delivery::Drop(reason) => trace!(
            api_call,
            callback,
            payload_len,
            reason = reason.label(),
            "callback: api-call result not ours to interpret"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpublished_original_returns_zero_before_it_touches_anything() {
        // No trampoline is published here, so the body fails closed: the
        // "called before initialisation" contract, with bytes that would be a live
        // frame elsewhere.
        // SAFETY: called with null pointers and no live Steam frame; every path this
        // test can take returns before touching memory.
        let result = unsafe {
            set_api_call_result(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
                0,
                std::ptr::null_mut(),
                0,
                0,
            )
        };
        assert_eq!(result, 0);
    }
}
