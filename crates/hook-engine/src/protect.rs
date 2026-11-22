//! Page-protection changes for this crate's patch writers.
//!
//! Two writers in this crate have to write to memory the loader mapped
//! read-only or read-execute, and both have to put the loader's protection back
//! afterwards: the detour's trampoline/PIC repair and the vtable slot swap.
//! [`WritableRegion`] owns that contract. It captures the previous protection
//! when it is created and restores it from [`Drop`], so no early return, `?`,
//! or panic between the two steps can leave a page writable.
//!
//! Backends:
//!
//! * unix — `mprotect`. `mprotect` does not report the previous protection, so
//!   it is read from `/proc/self/maps` on Linux. Other unices report
//!   protections through a different API (`mach_vm_region`, `sysctl`) which
//!   this backend does not implement; they get a clear error instead of a
//!   guessed protection, because a wrong guess would be restored later.
//! * windows — `VirtualProtect`, which reports the previous protection in its
//!   `lpflOldProtect` out-parameter, plus `FlushInstructionCache`. Both
//!   entry points are declared in `protect::windows`; no Windows binding crate
//!   is used.

use std::fmt;
use thiserror::Error;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
use windows as sys;

#[cfg(not(any(unix, windows)))]
compile_error!(
    "vapor-forge-hook-engine has no page-protection backend for this target; \
     add one under crates/hook-engine/src/protect/"
);

/// A page protection value, in the platform's own encoding.
///
/// Callers never assemble these from raw flags. They either name what they
/// want ([`Protection::READ_EXECUTE`] to harden code) or hand back the value
/// the platform reported as the previous protection
/// ([`WritableRegion::previous`]).
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Protection(u32);

impl Protection {
    /// Readable, not writable: the state of read-only data.
    pub const READ: Self = Self(sys::READ);
    /// Readable and writable: what a read-only data page becomes.
    pub const READ_WRITE: Self = Self(sys::READ_WRITE);
    /// Readable and executable: the steady state for code.
    pub const READ_EXECUTE: Self = Self(sys::READ_EXECUTE);
    /// Readable, writable and executable: what a code page becomes while it is
    /// being patched.
    pub const READ_WRITE_EXECUTE: Self = Self(sys::READ_WRITE_EXECUTE);

    /// The platform-native encoding (POSIX `PROT_*` bits, Win32 `PAGE_*`).
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Debug for Protection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Protection({:#x})", self.0)
    }
}

/// A page-protection failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ProtectError {
    #[error("the page range starting at {address:#x} is empty")]
    EmptyRange { address: usize },
    #[error("page range for {address:#x}+{len} overflows the address space")]
    RangeOverflow { address: usize, len: usize },
    #[error("the platform page size is unavailable")]
    PageSizeUnavailable,
    #[error("the protection of {address:#x}..{end:#x} cannot be determined on this platform")]
    ProtectionUnavailable { address: usize, end: usize },
    #[error("the protection of {address:#x}..{end:#x} could not be changed (os error {os_error})")]
    ChangeFailed {
        address: usize,
        end: usize,
        os_error: i32,
    },
}

/// A range made writable for the lifetime of the value.
///
/// The previous protection is restored by [`Drop`]. Use [`Self::restore`] when
/// the caller wants the restoration error instead of a logged warning.
#[derive(Debug)]
pub struct WritableRegion {
    address: usize,
    len: usize,
    previous: Protection,
    restored: bool,
}

impl WritableRegion {
    /// Start address of the range that was made writable.
    pub fn address(&self) -> usize {
        self.address
    }

    /// Length in bytes of the range that was made writable.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Always false: a zero-length region is rejected by [`make_writable`].
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The protection the range had before it was made writable.
    pub fn previous(&self) -> Protection {
        self.previous
    }

    /// Restore the previous protection now and report a failure.
    pub fn restore(mut self) -> Result<(), ProtectError> {
        self.restored = true;
        sys::set_protection(self.address, self.len, self.previous)
    }
}

impl Drop for WritableRegion {
    fn drop(&mut self) {
        if self.restored {
            return;
        }
        match sys::set_protection(self.address, self.len, self.previous) {
            Ok(()) => tracing::debug!(
                address = format_args!("{:#x}", self.address),
                len = self.len,
                previous = ?self.previous,
                "page protection restored"
            ),
            Err(error) => tracing::error!(
                address = format_args!("{:#x}", self.address),
                len = self.len,
                previous = ?self.previous,
                %error,
                "failed to restore the page protection; the range stays writable"
            ),
        }
    }
}

/// Make `address..address+len` writable, remembering the previous protection.
///
/// The returned region restores that protection when it is dropped.
pub fn make_writable(address: usize, len: usize) -> Result<WritableRegion, ProtectError> {
    if len == 0 {
        return Err(ProtectError::EmptyRange { address });
    }
    let previous = sys::make_writable(address, len)?;
    tracing::trace!(
        address = format_args!("{address:#x}"),
        len,
        previous = ?previous,
        "page range made writable"
    );
    Ok(WritableRegion {
        address,
        len,
        previous,
        restored: false,
    })
}

/// Run `patch` with `address..address+len` writable.
///
/// The previous protection is restored after `patch` returns, whether it
/// succeeded, failed, or panicked, so a failed patch step cannot leak a
/// writable page. Restoration failures are logged; use [`make_writable`] and
/// [`WritableRegion::restore`] when they must be reported to the caller.
pub fn with_writable<T>(
    address: usize,
    len: usize,
    patch: impl FnOnce() -> T,
) -> Result<T, ProtectError> {
    let region = make_writable(address, len)?;
    let value = patch();
    drop(region);
    Ok(value)
}

/// Force an absolute protection on `address..address+len`.
///
/// This is for pages whose steady state is known (trampoline pages going back
/// to read-execute), not for patching: it does not report what the range was.
pub fn set_protection(
    address: usize,
    len: usize,
    protection: Protection,
) -> Result<(), ProtectError> {
    if len == 0 {
        return Err(ProtectError::EmptyRange { address });
    }
    sys::set_protection(address, len, protection)
}

/// Publish freshly written code to the instruction caches.
///
/// A no-op on unix/x86, where the instruction cache is coherent with the data
/// cache and `mprotect` already serialises. On Windows it is
/// `FlushInstructionCache`.
pub fn flush_instruction_cache(address: usize, len: usize) {
    sys::flush_instruction_cache(address, len);
}

/// The protection currently held by `address..address+len`.
///
/// Only unix needs this: `mprotect` does not report the previous protection,
/// while `VirtualProtect` does.
#[cfg(unix)]
pub fn protection_of(address: usize, len: usize) -> Result<Protection, ProtectError> {
    sys::protection_of(address, len)
}

/// The platform page size in bytes.
///
/// Only unix needs this: `mprotect` requires page-aligned ranges, while
/// `VirtualProtect` rounds the range it is given.
#[cfg(unix)]
pub fn page_size() -> Result<usize, ProtectError> {
    sys::page_size()
}

/// The OS error number of the most recent failed call.
fn last_os_error() -> i32 {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or_default()
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
