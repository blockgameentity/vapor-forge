//! `CUser::GetSteamID` — the call shape the identity hook stands in for.
//!
//! # Verified from the corpus
//!
//! The accessor is the same four instructions in the primary `CUser` table
//! (slot 4) and behind the `IClientUser` slot 10 adapter, measured in both live
//! builds (`res/patterns/win64/VTABLE-REPORT.md` §9 — the slot-10 body carries
//! the secondary-base displacement in its member offset, the primary table's
//! slot 4 does not):
//!
//! ```text
//! 48 8B 81 0A 02 00 00   mov 0x20a(%rcx),%rax   ; 8-byte load from this+0x20A
//! 48 89 02               mov %rax,(%rdx)         ; 8-byte store through arg 1
//! 48 89 D0               mov %rdx,%rax           ; the pointer is the result
//! C3                     ret
//! ```
//!
//! Three facts, and only three:
//!
//! * argument 0 (`rcx`) is `this`, and the account `CSteamID` is the 8-byte
//!   member at `CUser+0x20A` — the member the account-type test in
//!   `CUpdateSubscribedItemsJob` reads;
//! * argument 1 (`rdx`) is a *pointer* the body stores 8 bytes through. It is an
//!   out-parameter, not a second word of `this`;
//! * the body returns that same pointer in `rax`. It is **not** a `u64`
//!   `GetSteamID(this)` getter: the Linux host's shape (`mov rax, [rdi+disp];
//!   ret`) has zero occurrences in either Windows image, so the Linux return
//!   convention must not be carried over.
//!
//! # Assumed
//!
//! * a `CSteamID` is exactly 8 bytes and the store width is the whole value —
//!   from `steamclientpublic.h` as `crates/steam-native-abi` derives it. The
//!   corpus measures the store *width*, not the class;
//! * the pointer in `rdx` is writable for those 8 bytes and belongs to the
//!   caller (its own storage, not a Steam-internal buffer). Nothing in the
//!   pattern says who owns it; the alternative would be a value Steam kept and
//!   then we would be overwriting Steam's own account member;
//! * the alignment of that pointer is unknown — only the width is measured — so
//!   the body reads and writes it with [`CSteamId::read_at`] /
//!   [`CSteamId::write_at`], which impose no alignment requirement;
//! * there is no third integer argument. The measured body ends at `ret` without
//!   reading `r8`, so the type stops at two slots; a body that displaced more of
//!   the function would have to grow the type with it.

use core::ffi::c_void;

/// A `CSteamID` as the accessor copies it: one 64-bit value, nothing else.
///
/// **Assumed layout** (8 bytes, `uint64 m_unAll64Bits`). The size is asserted in
/// this module's tests so a future change is a test failure rather than a silent
/// 4-byte write.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CSteamId {
    steam_id64: u64,
}

impl CSteamId {
    pub const SIZE: usize = 8;

    pub const fn new(steam_id64: u64) -> Self {
        Self { steam_id64 }
    }

    pub const fn steam_id64(self) -> u64 {
        self.steam_id64
    }

    /// Read the value through a pointer Steam filled.
    ///
    /// # Safety
    /// `pointer` must be non-null and point at [`Self::SIZE`] readable bytes.
    /// Alignment is deliberately not required: the corpus measures the width of
    /// this member, never its alignment, and the caller's storage is not ours to
    /// assume aligned.
    pub unsafe fn read_at(pointer: *const Self) -> Self {
        // SAFETY: the caller guarantees the 8 readable bytes; `read_unaligned`
        // adds no alignment requirement of its own.
        unsafe { pointer.read_unaligned() }
    }

    /// Write the value through a pointer the caller handed Steam.
    ///
    /// # Safety
    /// `pointer` must be non-null and point at [`Self::SIZE`] writable bytes the
    /// caller owns. Alignment is not required, for the same reason as
    /// [`Self::read_at`].
    pub unsafe fn write_at(pointer: *mut Self, id: Self) {
        // SAFETY: the caller guarantees 8 writable bytes; `write_unaligned` adds
        // no alignment requirement of its own.
        unsafe { pointer.write_unaligned(id) }
    }
}

/// `CSteamID* GetSteamID(this, CSteamID* out)`.
///
/// The return value is the pointer the function was handed, per the verified
/// `mov %rdx,%rax`. A replacement that returns a pointer to anything else breaks
/// the caller's own convention.
pub type GetSteamIdFn = unsafe extern "system" fn(this: *mut c_void, out: *mut CSteamId) -> *mut CSteamId;

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: u64 = 76_561_198_000_000_001;

    #[test]
    fn a_steam_id_is_one_eight_byte_value() {
        assert_eq!(core::mem::size_of::<CSteamId>(), CSteamId::SIZE);
        assert_eq!(CSteamId::new(REAL).steam_id64(), REAL);
    }

    #[test]
    fn the_value_round_trips_through_an_unaligned_pointer() {
        // The corpus measures the width of `CUser+0x20A`, not the alignment of a
        // caller's buffer, so an odd offset must work: a plain `*mut` store would
        // be undefined behaviour here.
        let mut buffer = [0u8; CSteamId::SIZE + 8];
        let base = buffer.as_mut_ptr().cast::<u8>();
        // The offset is chosen from the address itself, so the pointer is odd and
        // therefore misaligned whatever the stack layout is: `base` even takes 1,
        // `base` odd takes 2, and both stay inside the buffer.
        let offset = 1 + (base as usize % 2);
        // SAFETY: the offset leaves `CSteamId::SIZE` writable bytes inside the
        // buffer; the pointer itself is deliberately misaligned.
        let misaligned = unsafe { base.add(offset) }.cast::<CSteamId>();
        assert_ne!(misaligned as usize % core::mem::align_of::<CSteamId>(), 0);
        // SAFETY: as above for both the write and the read.
        unsafe { CSteamId::write_at(misaligned, CSteamId::new(REAL)) };
        // SAFETY: the same 8 bytes, read back through the same pointer.
        let read_back = unsafe { CSteamId::read_at(misaligned) };
        assert_eq!(read_back.steam_id64(), REAL);
    }
}
