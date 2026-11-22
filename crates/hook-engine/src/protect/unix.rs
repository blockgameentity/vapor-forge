//! `mprotect` page-protection backend.

use std::sync::OnceLock;

use super::{last_os_error, ProtectError, Protection};

pub(super) const READ: u32 = libc::PROT_READ as u32;
pub(super) const WRITE: u32 = libc::PROT_WRITE as u32;
pub(super) const READ_WRITE: u32 = (libc::PROT_READ | libc::PROT_WRITE) as u32;
pub(super) const READ_EXECUTE: u32 = (libc::PROT_READ | libc::PROT_EXEC) as u32;
pub(super) const READ_WRITE_EXECUTE: u32 =
    (libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC) as u32;

/// Make the pages covering `address..address+len` readable and writable,
/// returning the protection they had (found through the mapping's permissions).
pub(super) fn make_writable(address: usize, len: usize) -> Result<Protection, ProtectError> {
    let previous = protection_of(address, len)?;
    // Keep read and execute: a code page must stay executable, and callers
    // restore `previous` afterwards either way.
    let writable = Protection(previous.raw() | WRITE);
    set_protection(address, len, writable)?;
    Ok(previous)
}

pub(super) fn set_protection(
    address: usize,
    len: usize,
    protection: Protection,
) -> Result<(), ProtectError> {
    let (start, end) = page_range(address, len)?;
    // SAFETY: `start` is page-aligned, `end - start` is a whole number of pages,
    // and the range belongs to this process; mprotect only rewrites permission
    // bits of already-mapped memory.
    let rc = unsafe {
        libc::mprotect(
            start as *mut libc::c_void,
            end - start,
            protection.raw() as libc::c_int,
        )
    };
    if rc != 0 {
        return Err(ProtectError::ChangeFailed {
            address: start,
            end,
            os_error: last_os_error(),
        });
    }
    Ok(())
}

pub(super) fn flush_instruction_cache(_address: usize, _len: usize) {
    // x86 instruction caches are coherent with the data cache, and mprotect
    // already serialises the write before the page becomes executable again.
}

#[cfg(target_os = "linux")]
pub(super) fn protection_of(address: usize, len: usize) -> Result<Protection, ProtectError> {
    let (start, end) = page_range(address, len)?;
    let maps = std::fs::read_to_string("/proc/self/maps").map_err(|_| {
        ProtectError::ProtectionUnavailable {
            address: start,
            end,
        }
    })?;
    maps.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let mut bounds = fields.next()?.splitn(2, '-');
            let mapping_start = usize::from_str_radix(bounds.next()?, 16).ok()?;
            let mapping_end = usize::from_str_radix(bounds.next()?, 16).ok()?;
            let permissions = fields.next()?.as_bytes();
            if start < mapping_start || end > mapping_end || permissions.len() < 3 {
                return None;
            }
            let mut protection = 0u32;
            if permissions[0] == b'r' {
                protection |= READ;
            }
            if permissions[1] == b'w' {
                protection |= WRITE;
            }
            if permissions[2] == b'x' {
                protection |= libc::PROT_EXEC as u32;
            }
            Some(Protection(protection))
        })
        .ok_or(ProtectError::ProtectionUnavailable {
            address: start,
            end,
        })
}

#[cfg(not(target_os = "linux"))]
pub(super) fn protection_of(address: usize, len: usize) -> Result<Protection, ProtectError> {
    let (start, end) = page_range(address, len)?;
    // BSD and macOS expose protections through mach_vm_region/sysctl, which this
    // backend does not implement. Fail instead of guessing: the guess would be
    // handed back to mprotect when the region is restored.
    Err(ProtectError::ProtectionUnavailable {
        address: start,
        end,
    })
}

pub(super) fn page_size() -> Result<usize, ProtectError> {
    static PAGE_SIZE: OnceLock<Option<usize>> = OnceLock::new();
    PAGE_SIZE
        .get_or_init(|| {
            // SAFETY: sysconf is thread-safe, retains no pointers, and
            // _SC_PAGESIZE only reads a kernel constant.
            let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
            (size > 0).then_some(size as usize)
        })
        .ok_or(ProtectError::PageSizeUnavailable)
}

/// Round `address..address+len` out to whole pages.
fn page_range(address: usize, len: usize) -> Result<(usize, usize), ProtectError> {
    let page_size = page_size()?;
    if len == 0 {
        return Err(ProtectError::EmptyRange { address });
    }
    let start = address & !(page_size - 1);
    let end = address
        .checked_add(len)
        .and_then(|end| end.checked_add(page_size - 1))
        .map(|end| end & !(page_size - 1))
        .ok_or(ProtectError::RangeOverflow { address, len })?;
    Ok((start, end))
}
