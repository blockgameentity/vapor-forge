//! The SteamUI call shapes the library hook bodies stand in for.
//!
//! Every body in [`crate::hooks::ui`] stands in for one of four symbols. This
//! module records, per symbol, what the corpus pattern proves (where the entry
//! is), what a decode of the extent behind that anchor proves (which registers
//! the body actually reads), and what is only assumed. The measurements were
//! taken on the two live builds in `/tmp/gate0` — public
//! `2dfb0a5d…` (`SteamUI.dll`, 19,675,800 bytes) and publicbeta
//! `55f7a4f9…` (19,924,120 bytes) — with
//! `vapor-forge-pe-functions --dll SteamUI.dll --dump <rva>`.
//!
//! No Windows client has ever executed this host, so **every offset into a Steam
//! object is assumed**, and the assumed layout is the Linux host's measured
//! 64-bit one (`crates/steam-native-abi`). Treat nothing below as measured on
//! Windows.
//!
//! # Verified
//!
//! ## `ShouldShowAppInLibrary` — both builds
//!
//! Anchored in public (rva `0x608e40`) and publicbeta (rva `0x622c00`); both
//! extents are 200 bytes and decodes to the same instruction stream, branch
//! targets aside:
//!
//! ```text
//! 40 53            push rbx
//! 48 83 EC 20      sub  rsp, 0x20
//! 48 8B 01         mov  rax, [rcx]      ; first qword of argument 0
//! 48 8B D9         mov  rbx, rcx
//! FF 10            call qword [rax+0]   ; through that qword
//! 3D D6 0C 09 00   cmp  eax, 0x90CD6
//! 0F 84 …          je   → the `mov al, 1` epilogue
//! …
//! 32 C0 / C3       xor  al, al / ret    ; the "no" epilogue
//! B0 01 / C3       mov  al, 1  / ret    ; the "yes" epilogue
//! ```
//!
//! * **one** register input: `rcx`, and only `rcx`. Across the whole 200-byte
//!   extent `rdx`, `r8` and `r9` are never read *or* written — the decoded
//!   operand list has no reference to any of them — so they are not inputs and
//!   there is nothing for a replacement to forward.
//! * `rcx` is an object whose first qword is a table of function pointers: the
//!   body loads `[rcx]` and calls through `[rax+0]`, `[rax+0xb0]` and
//!   `[rax+0x140]`.
//! * the return is `bool` in `al`. The body has exactly two epilogues,
//!   `xor al, al` and `mov al, 1`, so `u8` is the faithful trampoline type (see
//!   `super::ret` for why a Rust `bool` is not).
//!
//! ## `CSteamUIAppController::FillInAppOverview` — publicbeta only
//!
//! Anchored in publicbeta (rva `0x655ce0`, extent 6283 bytes) and **absent from
//! the public corpus**, so on a public build the symbol never resolves.
//!
//! ```text
//! 48 89 54 24 10   mov [rsp+0x10], rdx   ; 64-bit home slot
//! 48 89 4C 24 08   mov [rsp+0x8],  rcx   ; 64-bit home slot
//! …
//! 4C 8B E1         mov r12, rcx
//! 48 8B FA         mov rdi, rdx
//! 49 8B F0         mov rsi, r8
//! 44 8B 08         mov r9d, [rax]        ; r9 written before it is ever read
//! ```
//!
//! * three 64-bit register inputs: `rcx`, `rdx`, `r8`.
//! * `r9` is **not** an input: the body overwrites it from `[rax]` before any
//!   read, exactly as `GetPackageInfo` does.
//! * the return value is not measured (`eax` carries an early-out value the
//!   pattern does not identify), so the trampoline type returns `()` and the
//!   replacement never invents a value.
//!
//! ## `CSteamUIAppController::BuildCompleteAppOverviewChange` — both builds
//!
//! Anchored in public (rva `0x634280`, extent 2424 bytes) and publicbeta
//! (rva `0x64ee50`, extent 2270 bytes).
//!
//! ```text
//! 4C 89 44 24 18   mov [rsp+0x18], r8    ; 64-bit home slot
//! 48 89 54 24 10   mov [rsp+0x10], rdx   ; 64-bit home slot
//! 48 89 4C 24 08   mov [rsp+0x8],  rcx   ; public variant only
//! 55 53 56 57 41 54 41 55 41 56 41 57    ; push rbp, rbx, rsi, rdi, r12-r15
//! 48 8D 6C 24 E1   lea rbp, [rsp-0x1F]
//! …
//! 4C 8B FA         mov r15, rdx
//! 4C 8B F1         mov r14, rcx
//! ```
//!
//! * three 64-bit register inputs: `rcx`, `rdx`, `r8` — the body routes both
//!   `rcx` and `rdx` into callee-saved registers and spills `r8` at 64-bit width.
//! * `r9` is not measured: neither variant references it before the first call,
//!   so the trampoline type carries it as an opaque slot and forwards it.
//! * the return is unmeasured; the type returns `()`.
//!
//! ## `CUpdateManager::MarkAppChange` — public only
//!
//! Anchored in public (rva `0x6cf3b0`, extent 721 bytes) and absent from the
//! publicbeta corpus.
//!
//! ```text
//! 48 83 EC 78      sub  rsp, 0x78
//! 8B F2            mov  esi, edx        ; argument 1 is 32 bits
//! 4C 8B E9         mov  r13, rcx        ; argument 0 is a 64-bit `this`
//! 45 8B F8         mov  r15d, r8d       ; argument 2 is 32 bits
//! 44 8B 08         mov  r9d, [rax]      ; r9 written before it is ever read
//! …
//! 44 09 7C C8 14   or   [rax+rcx*8+0x14], r15d
//! ```
//!
//! * `rcx` is a 64-bit pointer, `rdx` is 32 bits, `r8` is 32 bits.
//! * `r9` is not an input (written before read), so the type carries three
//!   arguments like the Linux host's.
//! * the extent supports the two meaning assumptions below: the body indexes a
//!   per-app entry from `rdx` (`lea rcx, [rax+rax*2]` then
//!   `[rax+rcx*8+0x14]`) and ORs `r8` into a flags word.
//!
//! # Assumed
//!
//! * **the app record.** `ShouldShowAppInLibrary`'s only argument and
//!   `FillInAppOverview`'s third argument are taken to be the SteamUI app record
//!   (`CSteamApp`), and its `app_id` is read at `0x10` and its `purchased_time`
//!   written at `0x2c`. Those offsets are the Linux host's measured 64-bit
//!   layout; nothing measures them on a Windows build. Both are read through
//!   [`record`] and shown as such in the module docs of each body.
//!   Failure mode and blast radius are stated there.
//! * `FillInAppOverview`'s second argument (`rdx` — 64-bit, measured) is the
//!   `CAppOverview` being filled; the body never dereferences it.
//! * `MarkAppChange`'s `rdx` is an app id and its `r8` is the `EAppChange` flag
//!   word, from the Linux signature. The body only reports them.
//! * `BuildCompleteAppOverviewChange`'s `(rdx, r8)` are `(change, callback)` as
//!   on Linux. The body never dereferences either, so a different meaning costs
//!   nothing more than a mislabeled log field.
//! * `ShouldShowAppInLibrary` returns `true` when Steam would show the app.

use core::ffi::c_void;

/// The assumed SteamUI app record: `CSteamApp` as the Linux host measures it on
/// a 64-bit build.
///
/// Only two 32-bit fields are ever touched, both through raw byte offsets: this
/// module never defines the whole record, because the fields between the two
/// offsets are neither measured on Windows nor needed here.
pub mod record {
    use core::ffi::c_void;

    /// Offset of `CSteamApp::app_id`. **Assumed**, see the module docs.
    pub const APP_ID: usize = 0x10;

    /// Offset of `CSteamApp::purchased_time`. **Assumed**, see the module docs.
    pub const PURCHASED_TIME: usize = 0x2c;

    /// The app id in `record`, or `None` when the pointer is null.
    ///
    /// # Safety
    /// `record`, when non-null, must point at a live SteamUI app record — the
    /// argument Steam passed to the hooked call.
    pub unsafe fn app_id(record: *const c_void) -> Option<u32> {
        if record.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees a live record; the read is four bytes at
        // an in-object offset (see the module's Assumed section) and is
        // unaligned-safe.
        Some(unsafe { (record.cast::<u8>().add(APP_ID)).cast::<u32>().read_unaligned() })
    }

    /// The purchase time Steam currently has for `record` (0 when unset).
    ///
    /// # Safety
    /// As [`app_id`].
    pub unsafe fn purchased_time(record: *const c_void) -> u32 {
        if record.is_null() {
            return 0;
        }
        // SAFETY: as `app_id`; a null check precedes the read.
        unsafe { (record.cast::<u8>().add(PURCHASED_TIME)).cast::<u32>().read_unaligned() }
    }

    /// Replace the purchase time Steam has for `record`.
    ///
    /// # Safety
    /// As [`app_id`]. This writes four bytes into Steam's record: it is the only
    /// write this host performs at an assumed offset, and the reason
    /// `FillInAppOverview` is documented as the one body to check live.
    pub unsafe fn set_purchased_time(record: *mut c_void, value: u32) {
        if record.is_null() {
            return;
        }
        // SAFETY: as `app_id`; `write_unaligned` because the assumed offset has no
        // alignment guarantee.
        unsafe { (record.cast::<u8>().add(PURCHASED_TIME)).cast::<u32>().write_unaligned(value) }
    }
}

/// `ShouldShowAppInLibrary(this) -> bool` in `al`.
///
/// One argument: the extent never reads `rdx`, `r8` or `r9` (see the module
/// docs), so there is no slot for a replacement to forward.
pub type ShouldShowAppInLibraryFn = unsafe extern "system" fn(this: *mut c_void) -> u8;

/// `FillInAppOverview(this, overview, app)`.
///
/// `r9` is proved not to be an input, so it is absent from the type; the return
/// value is unmeasured, so the type returns nothing.
pub type FillInAppOverviewFn =
    unsafe extern "system" fn(this: *mut c_void, overview: *mut c_void, app: *mut c_void);

/// `BuildCompleteAppOverviewChange(this, change, callback_slot, r9)`.
///
/// Three 64-bit inputs are measured; `r9` is unknown and forwarded verbatim.
pub type BuildCompleteAppOverviewChangeFn = unsafe extern "system" fn(
    this: *mut c_void,
    change: *mut c_void,
    callback_slot: *mut c_void,
    r9: usize,
);

/// `MarkAppChange(this, app_id, flags)`.
///
/// `r9` is proved not to be an input, so the type carries the three measured
/// arguments and returns nothing.
pub type MarkAppChangeFn =
    unsafe extern "system" fn(this: *mut c_void, app_id: u32, flags: u32);

#[cfg(test)]
mod tests {
    use super::*;

    /// A buffer large enough for the two assumed fields, with the app id placed
    /// at the offset the layout assumes.
    fn record_bytes(app_id: u32, purchased_time: u32) -> [u8; 0x48] {
        let mut bytes = [0u8; 0x48];
        bytes[record::APP_ID..record::APP_ID + 4].copy_from_slice(&app_id.to_le_bytes());
        bytes[record::PURCHASED_TIME..record::PURCHASED_TIME + 4]
            .copy_from_slice(&purchased_time.to_le_bytes());
        bytes
    }

    #[test]
    fn the_two_assumed_offsets_do_not_overlap_and_fit_the_record() {
        assert_eq!(record::APP_ID, 0x10);
        assert_eq!(record::PURCHASED_TIME, 0x2c);
        // A compile-time check, so a future edit cannot silently make the two
        // fields overlap: the write would then clobber the app id.
        const { assert!(record::APP_ID + 4 <= record::PURCHASED_TIME) };
    }

    #[test]
    fn reading_uses_the_assumed_offsets() {
        let bytes = record_bytes(730, 1_700_000_000);
        let pointer = bytes.as_ptr().cast::<c_void>();
        // SAFETY: `pointer` is a live buffer of 0x48 bytes, larger than both
        // assumed offsets plus four.
        unsafe {
            assert_eq!(record::app_id(pointer), Some(730));
            assert_eq!(record::purchased_time(pointer), 1_700_000_000);
        }
    }

    #[test]
    fn writing_the_purchase_time_leaves_the_app_id_alone() {
        let mut bytes = record_bytes(730, 0);
        let pointer = bytes.as_mut_ptr().cast::<c_void>();
        // SAFETY: as above; the buffer is uniquely borrowed for the call.
        unsafe {
            record::set_purchased_time(pointer, 42);
            assert_eq!(record::app_id(pointer), Some(730));
            assert_eq!(record::purchased_time(pointer), 42);
        }
    }

    #[test]
    fn a_null_record_is_not_dereferenced() {
        // SAFETY: every accessor checks for null by contract.
        unsafe {
            assert_eq!(record::app_id(core::ptr::null()), None);
            assert_eq!(record::purchased_time(core::ptr::null()), 0);
            record::set_purchased_time(core::ptr::null_mut(), 1);
        }
    }
}
