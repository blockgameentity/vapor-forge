use super::*;

/// A private read-only anonymous page, unmapped on drop.
struct ReadOnlyPage {
    address: usize,
    len: usize,
}

impl ReadOnlyPage {
    fn new() -> Self {
        let len = page_size().expect("page size");
        // SAFETY: a fresh anonymous mapping with no aliases; the pointer is
        // checked against MAP_FAILED before use.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(address, libc::MAP_FAILED, "mmap failed");
        Self {
            address: address as usize,
            len,
        }
    }

    fn protection(&self) -> Protection {
        protection_of(self.address, self.len).expect("protection query")
    }

    fn write(&self, byte: u8) {
        // SAFETY: the caller made this page writable for the write; the
        // mapping is one page long.
        unsafe { *(self.address as *mut u8) = byte };
    }

    fn read(&self) -> u8 {
        // SAFETY: the mapping is one readable page.
        unsafe { *(self.address as *const u8) }
    }
}

impl Drop for ReadOnlyPage {
    fn drop(&mut self) {
        // SAFETY: unmapping the mapping created in `new` exactly once.
        unsafe { libc::munmap(self.address as *mut libc::c_void, self.len) };
    }
}

#[test]
fn writable_region_restores_the_previous_protection() {
    let page = ReadOnlyPage::new();
    assert_eq!(page.protection(), Protection::READ);

    {
        let region = make_writable(page.address, 1).expect("make writable");
        assert_eq!(region.previous(), Protection::READ);
        assert_eq!(region.address(), page.address);
        assert!(!region.is_empty());
        assert_eq!(page.protection(), Protection::READ_WRITE);
        page.write(0xab);
    }

    assert_eq!(page.protection(), Protection::READ);
    assert_eq!(page.read(), 0xab, "the patched byte must survive restore");
}

#[test]
fn failed_patch_step_does_not_leak_a_writable_page() {
    let page = ReadOnlyPage::new();

    let result: Result<Result<(), &str>, ProtectError> =
        with_writable(page.address, 4, || Err("patch writer rejected the site"));

    assert_eq!(
        result.expect("region"),
        Err("patch writer rejected the site")
    );
    assert_eq!(page.protection(), Protection::READ, "page leaked writable");
}

#[test]
fn explicit_restore_reports_success() {
    let page = ReadOnlyPage::new();
    let region = make_writable(page.address, page.len).expect("make writable");
    assert_eq!(page.protection(), Protection::READ_WRITE);
    region.restore().expect("restore");
    assert_eq!(page.protection(), Protection::READ);
}

#[test]
fn zero_length_ranges_are_rejected() {
    assert_eq!(
        make_writable(0x1000, 0).err(),
        Some(ProtectError::EmptyRange { address: 0x1000 })
    );
}
