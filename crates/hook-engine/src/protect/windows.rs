//! `VirtualProtect`/`FlushInstructionCache` page-protection backend.
//!
//! The kernel32 entry points live in [`crate::win_api`]; this module only maps
//! them onto the engine's [`Protection`] vocabulary.

use super::{ProtectError, Protection};
use crate::win_api;

/// `PAGE_READONLY`.
pub(super) const READ: u32 = 0x02;
/// `PAGE_READWRITE`.
pub(super) const READ_WRITE: u32 = 0x04;
/// `PAGE_EXECUTE_READ`.
pub(super) const READ_EXECUTE: u32 = 0x20;
/// `PAGE_EXECUTE_READWRITE`.
pub(super) const READ_WRITE_EXECUTE: u32 = win_api::PAGE_EXECUTE_READWRITE;

/// Change the protection of `address..address+len`, returning the previous one.
fn change(address: usize, len: usize, protection: u32) -> Result<u32, ProtectError> {
    win_api::virtual_protect(address, len, protection).ok_or_else(|| ProtectError::ChangeFailed {
        address,
        end: address.saturating_add(len),
        os_error: super::last_os_error(),
    })
}

/// Ask for `PAGE_EXECUTE_READWRITE` and let the OS report what the range had.
///
/// `VirtualProtect` hands back the previous protection, including modifier bits
/// (`PAGE_GUARD`, `PAGE_NOCACHE`), so restoring it is exact. The range may be a
/// plain data page (a vtable) — executable access is granted as well because the
/// same writable window is used for code pages, and Windows has no "read-write,
/// keep whatever execute bit" spelling.
pub(super) fn make_writable(address: usize, len: usize) -> Result<Protection, ProtectError> {
    change(address, len, READ_WRITE_EXECUTE).map(Protection)
}

pub(super) fn set_protection(
    address: usize,
    len: usize,
    protection: Protection,
) -> Result<(), ProtectError> {
    change(address, len, protection.raw()).map(|_previous| ())
}

pub(super) fn flush_instruction_cache(address: usize, len: usize) {
    if !win_api::flush_instruction_cache(address, len) {
        tracing::warn!(
            address = format_args!("{address:#x}"),
            len,
            os_error = super::last_os_error(),
            "FlushInstructionCache failed; patched code may not be visible to other cores"
        );
    }
}
