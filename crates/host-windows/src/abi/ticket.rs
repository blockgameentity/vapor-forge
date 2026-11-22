//! `CUser::GetAppTicket` — the call shape the ticket hook stands in for.
//!
//! # Verified from the corpus
//!
//! **Nothing.** There is no corpus entry for `CUser::GetAppTicket` in either
//! live build: `res/patterns/win64/steamclient/669fa08e….toml` (public) and
//! `4a2d7952….toml` (publicbeta) do not name it, and
//! `res/patterns/win64/VTABLE-REPORT.md` has no candidate for it either. The
//! install pass therefore never resolves this symbol, so the body in
//! [`crate::hooks::ticket`] is registered but cannot be installed today.
//!
//! # Assumed
//!
//! Everything below: the argument order is copied from the shape the Linux host
//! uses for the same class method (`CUser::GetAppTicket(this, app_id,
//! CUtlBuffer *out)`, `app_id` a `u32`), and the return value is assumed to be
//! `bool` in `al`. Both are guesses that a future anchor has to confirm before
//! this hook may ship; they exist so the decision path is written, unit-tested
//! and reviewable, not because the layout is known.

use core::ffi::c_void;

/// `GetAppTicket(this, app_id, out, r9) -> u8`.
///
/// `out` and `r9` are forwarded and never written: the buffer layout is
/// unmeasured, so the body only decides and never serves ticket bytes.
pub type GetAppTicketFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32, out: usize, r9: usize) -> u8;
