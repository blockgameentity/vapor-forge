//! `CUser::CheckAppOwnership` — the call shape the ownership hook stands in for.
//!
//! # Verified from the corpus
//!
//! Anchored in both live builds (`669fa08e…` public, `4a2d7952…` publicbeta):
//!
//! ```text
//! 48 8B C4     mov rax, rsp
//! 89 50 10     mov [rax+0x10], edx   ; rdx's home slot, 32-bit store
//! 48 89 48 08  mov [rax+8], rcx      ; rcx's home slot, 64-bit store
//! 55 53 48     push rbp / push rbx
//! ```
//!
//! So argument 0 (`rcx`) is 64-bit, argument 1 (`rdx`) is 32 bits wide, and the
//! function takes a pointer as its first argument. Nothing about `r8`/`r9` is
//! measured: the pattern stops before either slot is used.
//!
//! # Assumed
//!
//! * argument 2 (`r8`) is the `CAppOwnershipInfo*` out parameter — from the
//!   Linux host's measured signature in `crates/steam-native-abi`;
//! * the function returns `bool` (in `al`) — likewise;
//! * argument 3 (`r9`) and any stack arguments are unknown, so the type carries
//!   `r9` as an opaque slot and forwards it unchanged;
//! * [`CAppOwnershipInfo`]'s field layout is `steamclientpublic.h` as the Linux
//!   host already implements it. It has never been re-measured on a Windows
//!   build, and this host rewrites fields through it.

use core::ffi::c_void;

/// The ownership record Steam fills for `CheckAppOwnership`.
///
/// `#[repr(C, packed)]` matches the public header's field order without padding
/// (0x38 bytes); alignment is 1, so a pointer Steam hands us is never misaligned
/// for this type.
///
/// **Assumed layout.** The offsets are asserted in this module's tests so a
/// future change is a test failure rather than a silent write at the wrong
/// offset, but the assertion can only prove that *this* crate agrees with the
/// Linux host — not that a Windows build ships the same record.
#[repr(C, packed)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CAppOwnershipInfo {
    pub package_id: i32,
    pub release_state: i32,
    pub owner_account_id: u32,
    pub master_subscription_app_id: i32,
    pub trial_seconds: u32,
    pub exist_in_package_nums: i32,
    pub purchase_country_code: [u8; 4],
    pub purchase_time: u32,
    pub license_expiration_time: u32,
    pub owns_license: u8,
    pub license_expired: u8,
    pub is_permanent: u8,
    pub low_violence: u8,
    pub free_license: u8,
    pub region_restricted: u8,
    pub from_free_weekend: u8,
    pub license_locked: u8,
    pub license_pending: u8,
    pub retail_license: u8,
    pub auto_grant: u8,
    pub license_permanent: u8,
    pub guest_pass: u8,
    pub borrowed: u8,
    pub any_site_license: u8,
    pub all_site_licenses: u8,
    pub all_activation_required: u8,
    pub family_shared: u8,
    pub _unknown_36: u8,
    pub _unknown_37: u8,
}

impl CAppOwnershipInfo {
    pub const SIZE: usize = 0x38;

    /// `EAppReleaseState::Released` in `steamclientpublic.h`.
    const RELEASE_STATE_RELEASED: i32 = 4;

    /// Package identifier the host synthesises for a controlled app, so DRM and
    /// downstream code see one consistent package association.
    const INJECTED_PACKAGE_ID: i32 = 0;

    /// Fill the fields Steam checks when it decides whether the caller owns an
    /// app. Purchase metadata is left alone.
    pub fn grant_spoofed_ownership(&mut self) {
        self.package_id = Self::INJECTED_PACKAGE_ID;
        self.release_state = Self::RELEASE_STATE_RELEASED;
        self.owner_account_id = 1;
        self.exist_in_package_nums = 2;
        self.owns_license = 1;
        self.license_expired = 0;
        self.is_permanent = 1;
        self.free_license = 0;
        self.family_shared = 0;
    }

    pub fn owns_license(&self) -> bool {
        self.owns_license != 0
    }

    pub fn is_family_shared(&self) -> bool {
        self.family_shared != 0
    }

    pub fn clear_family_shared(&mut self) {
        self.family_shared = 0;
    }
}

/// `CheckAppOwnership(this, app_id, out, r9)`.
///
/// `r9` is forwarded and never read: see the module docs. The return value is
/// the raw `al` byte for the reason given in [`super`]'s docs.
pub type CheckAppOwnershipFn = unsafe extern "system" fn(
    this: *mut c_void,
    app_id: u32,
    out: *mut CAppOwnershipInfo,
    r9: usize,
) -> u8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_assumed_record_matches_the_linux_host_layout() {
        assert_eq!(
            core::mem::size_of::<CAppOwnershipInfo>(),
            CAppOwnershipInfo::SIZE
        );
        assert_eq!(core::mem::offset_of!(CAppOwnershipInfo, owns_license), 0x24);
        assert_eq!(core::mem::offset_of!(CAppOwnershipInfo, is_permanent), 0x26);
        assert_eq!(
            core::mem::offset_of!(CAppOwnershipInfo, family_shared),
            0x35
        );
    }

    #[test]
    fn granting_spoofed_ownership_sets_every_field_steam_checks() {
        let mut info = CAppOwnershipInfo {
            family_shared: 1,
            purchase_time: 0xcafe_babe,
            ..CAppOwnershipInfo::default()
        };
        info.grant_spoofed_ownership();
        assert!(info.owns_license());
        // Packed struct: read the fields out by value before comparing.
        let owner = { info.owner_account_id };
        let package_count = { info.exist_in_package_nums };
        let purchase_time = { info.purchase_time };
        assert_eq!(owner, 1);
        assert_eq!(package_count, 2);
        assert!(
            !info.is_family_shared(),
            "our own title is never family-shared"
        );
        assert_eq!(purchase_time, 0xcafe_babe, "purchase metadata is untouched");
    }

    #[test]
    fn a_foreign_family_flag_can_be_cleared() {
        let mut info = CAppOwnershipInfo {
            family_shared: 1,
            ..CAppOwnershipInfo::default()
        };
        assert!(info.is_family_shared());
        info.clear_family_shared();
        assert!(!info.is_family_shared());
    }
}
