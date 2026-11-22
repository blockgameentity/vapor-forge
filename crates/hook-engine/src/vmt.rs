use std::sync::atomic::{AtomicU8, Ordering};

use tracing::{debug, error, info};

use crate::plan::ValidatedHookTarget;
use crate::protect;

const INSTALL_IDLE: u8 = 0;
const INSTALLING: u8 = 1;
const INSTALLED: u8 = 2;
const DISABLED: u8 = 3;

pub struct InstallGate(AtomicU8);

impl Default for InstallGate {
    fn default() -> Self {
        Self::new()
    }
}

impl InstallGate {
    pub const fn new() -> Self {
        Self(AtomicU8::new(INSTALL_IDLE))
    }

    pub(crate) fn is_installed(&self) -> bool {
        self.0.load(Ordering::Acquire) == INSTALLED
    }

    pub fn is_settled(&self) -> bool {
        self.is_installed() || self.0.load(Ordering::Acquire) == DISABLED
    }

    pub fn begin(&self) -> Option<InstallAttempt<'_>> {
        self.0
            .compare_exchange(
                INSTALL_IDLE,
                INSTALLING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .ok()
            .map(|_| InstallAttempt {
                gate: self,
                committed: false,
            })
    }
}

pub struct InstallAttempt<'a> {
    gate: &'a InstallGate,
    committed: bool,
}

impl InstallAttempt<'_> {
    pub fn commit(mut self) {
        self.gate.0.store(INSTALLED, Ordering::Release);
        self.committed = true;
    }

    pub fn disable(mut self) {
        self.gate.0.store(DISABLED, Ordering::Release);
        self.committed = true;
    }
}

impl Drop for InstallAttempt<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.gate.0.store(INSTALL_IDLE, Ordering::Release);
        }
    }
}

/// Swap a vtable slot pointer and return the original value.
///
/// # Safety
/// `this` must point to a valid C++ object with a vtable pointer as its first field.
/// `slot` must be within the vtable bounds. The validated replacement function
/// must match the slot's calling convention and signature.
pub unsafe fn swap_vtable_slot(
    name: &str,
    this: *mut std::ffi::c_void,
    slot: usize,
    target: ValidatedHookTarget,
) -> Option<usize> {
    if this.is_null() {
        error!(hook = name, "VMT swap failed: this is null");
        return None;
    }

    // SAFETY: this points to a C++ object; first field is vtable pointer.
    let vtable = unsafe { *(this as *const *const usize) };
    if vtable.is_null() {
        error!(hook = name, "VMT swap failed: vtable is null");
        return None;
    }

    // SAFETY: reading the original slot value.
    let slot_ptr = unsafe { vtable.add(slot) };
    // SAFETY: slot_ptr was derived from the validated vtable and slot index.
    let original = unsafe { *slot_ptr };
    if original != target.target_address {
        error!(
            hook = name,
            planned = format_args!("0x{:x}", target.target_address),
            actual = format_args!("0x{original:x}"),
            "VMT swap failed: slot changed after validation"
        );
        return None;
    }
    let replacement = target.replacement_address;

    debug!(
        hook = name,
        slot = slot,
        original = format_args!("0x{:x}", original),
        "vtable slot read"
    );

    // A vtable usually lives in read-only data: `.data.rel.ro` in an ELF
    // image, `.rdata` in an MSVC one. The guard restores the previous
    // protection from `Drop`, so the slot write cannot leak a writable page,
    // and `restore` below reports the failure when it can be undone.
    let region = match protect::make_writable(slot_ptr as usize, std::mem::size_of::<usize>()) {
        Ok(region) => region,
        Err(error) => {
            error!(
                hook = name,
                %error,
                "VMT swap failed: the slot page could not be made writable"
            );
            return None;
        }
    };

    // SAFETY: `slot_ptr` came from the validated vtable. Its address is
    // pointer-aligned because vtable entries are pointer-sized (8 bytes on
    // MS-x64, 4 on x86), so a single `usize` store is the whole slot, and the
    // range is writable for as long as `region` is alive.
    unsafe { *(slot_ptr as *mut usize) = replacement };

    if let Err(error) = region.restore() {
        error!(
            hook = name,
            %error,
            "VMT slot written, but restoring the page protection failed"
        );
    }

    info!(hook = name, slot = slot, "VMT hook INSTALLED");
    Some(original)
}

#[cfg(test)]
mod tests {
    use super::InstallGate;
    use crate::plan::{validate_hook_target, AddressRange, HookTargetInput};

    #[test]
    fn install_gate_retries_failure_and_commits_success() {
        let gate = InstallGate::new();
        {
            let _failed_attempt = gate.begin().expect("first attempt");
            assert!(gate.begin().is_none());
        }
        assert!(!gate.is_installed());

        gate.begin().expect("retry after failure").commit();
        assert!(gate.is_installed());
        assert!(gate.is_settled());
        assert!(gate.begin().is_none());
    }

    #[test]
    fn install_gate_distinguishes_disabled_from_installed() {
        let gate = InstallGate::new();
        gate.begin().unwrap().disable();
        assert!(gate.is_settled());
        assert!(!gate.is_installed());
        assert!(gate.begin().is_none());
    }

    #[test]
    fn swap_rejects_a_slot_that_changed_after_validation() {
        let vtable = [0x1200usize];
        let object_vtable = vtable.as_ptr();
        let this = std::ptr::addr_of!(object_vtable) as *mut std::ffi::c_void;
        let target = validate_hook_target(HookTargetInput {
            target_address: 0x1100,
            replacement_address: 0x2000,
            executable_range: AddressRange {
                start: 0x1000,
                end: 0x1800,
            },
        })
        .unwrap();

        // SAFETY: `this` points to a test-owned object with one readable slot;
        // the mismatch returns before any memory protection or write occurs.
        let result = unsafe { super::swap_vtable_slot("test", this, 0, target) };

        assert_eq!(result, None);
        assert_eq!(vtable[0], 0x1200);
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn swap_uses_pointer_sized_slots_on_64_bit_targets() {
        // MS-x64 (and 64-bit ELF) vtable entries are 8 bytes wide. Slot values
        // above 4 GiB prove the read, the comparison and the write all treat
        // the slot as a pointer-sized value rather than a 32-bit one.
        let original: usize = 0x0000_7ff6_1234_1000;
        let replacement: usize = 0x0000_7ff6_1234_2000;
        let vtable = [0usize, 0, original, 0];
        let object_vtable = vtable.as_ptr();
        let this = std::ptr::addr_of!(object_vtable) as *mut std::ffi::c_void;
        let target = validate_hook_target(HookTargetInput {
            target_address: original,
            replacement_address: replacement,
            executable_range: AddressRange {
                start: 0x0000_7ff6_1234_0000,
                end: 0x0000_7ff6_1235_0000,
            },
        })
        .unwrap();

        // SAFETY: `this` points to a test-owned object whose vtable is a
        // writable stack array with four slots; slot 2 holds `original`. The
        // swap writes into that test-owned page and restores its protection.
        let swapped = unsafe { super::swap_vtable_slot("test", this, 2, target) };

        assert_eq!(swapped, Some(original));
        assert_eq!(vtable[2], replacement);
        assert_eq!(vtable[0], 0, "neighbouring slots stay untouched");
        assert_eq!(vtable[3], 0, "neighbouring slots stay untouched");
    }
}
