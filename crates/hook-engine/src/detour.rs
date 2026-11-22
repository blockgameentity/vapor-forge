//! Detour installation: plan validation, trampoline creation, PIC repair, and
//! the process-lifetime storage slots hooks call through.
//!
//! The backend is chosen per target (`retour` on unix, the engine's own writer on
//! Windows); everything in this module is shared. Callers see one type,
//! [`Detour`], and one way to reach the original entry point,
//! [`crate::original::original_detour`].

use tracing::{debug, error, info};

use crate::pic_repair;
use crate::pic_thunk;
use crate::plan::ValidatedHookTarget;

mod trampolines;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
use unix as backend;
#[cfg(windows)]
use windows as backend;

pub use backend::{Detour, HookFn};
pub use trampolines::restore_rx as restore_trampoline_pages_rx;

/// A created detour that has not been enabled yet.
///
/// The entry still holds its original bytes; enabling it is
/// [`store_and_finalize`]'s job so hook callbacks can find the stored detour
/// first.
pub struct PendingDetour<F: HookFn> {
    pub detour: Detour<F>,
    pub callee_addr: usize,
}

/// Create a detour without enabling it.
///
/// On unix this asks `retour` for a trampoline; on Windows it plans the entry
/// patch, allocates the trampoline, and leaves the entry untouched.
///
/// # Safety
/// The validated target and replacement addresses must both point to functions
/// matching the signature `F`.
pub unsafe fn create_detour<F: HookFn>(
    name: &str,
    target: ValidatedHookTarget,
) -> Option<PendingDetour<F>> {
    let callee_addr = target.target_address;
    // Sanity check: reject obviously invalid target addresses. During
    // la_objopen an image may not be fully relocated yet, which can produce
    // garbage addresses from pattern resolution.
    if callee_addr < 0x10000 {
        error!(
            hook = name,
            addr = format_args!("0x{:x}", callee_addr),
            "target address suspiciously low, skipping"
        );
        return None;
    }

    trampolines::make_writable();

    // SAFETY: caller guarantees the validated addresses use signature F.
    let target_fn = unsafe { F::from_ptr(target.target_address as *const ()) };
    // SAFETY: caller guarantees the validated addresses use signature F.
    let replacement_fn = unsafe { F::from_ptr(target.replacement_address as *const ()) };

    // SAFETY: caller guarantees both function pointers are valid.
    // Note: creating a detour does NOT overwrite the original function; that
    // only happens on enable(), so the original prologue is still readable at
    // callee_addr until `finalize_detour` enables it.
    match unsafe { Detour::new(target_fn, replacement_fn) } {
        Ok(detour) => {
            debug!(hook = name, "detour created");
            Some(PendingDetour {
                detour,
                callee_addr,
            })
        }
        Err(e) => {
            error!(hook = name, error = %e, "detour creation failed");
            None
        }
    }
}

/// Store a pending detour into its process-lifetime slot and finalize it.
///
/// Returns whether a detour was present and finalized.
///
/// # Safety
/// `storage` must point to a valid `Option<Detour<F>>` slot that lives for the
/// process lifetime.
pub unsafe fn store_and_finalize<F: HookFn>(
    name: &str,
    storage: *mut Option<Detour<F>>,
    pending: Option<PendingDetour<F>>,
) -> bool {
    let Some(p) = pending else { return false };
    let callee_addr = p.callee_addr;

    // SAFETY: storing detour before enable so hook callbacks can access it.
    unsafe { storage.write(Some(p.detour)) };

    // SAFETY: storage points to the slot we just initialized.
    unsafe {
        let Some(detour) = (*storage).as_mut() else {
            error!(hook = name, "stored detour missing after initialization");
            return false;
        };
        finalize_detour(name, detour, callee_addr)
    }
}

/// Apply PIC-thunk repair to a trampoline and enable the detour.
///
/// # Safety
/// The detour must have been created and not yet enabled.
pub(crate) unsafe fn finalize_detour<F: HookFn>(
    name: &str,
    detour: &mut Detour<F>,
    callee_addr: usize,
) -> bool {
    let tramp_addr = detour.trampoline() as *const _ as usize;
    trampolines::remember(tramp_addr);

    // The prologue is only read for PIC-thunk repair, which exists on 32-bit
    // ELF alone. Everywhere else the buffer stays zeroed and the repair returns
    // immediately, so a target is never read for a repair that cannot apply.
    let mut prologue = [0u8; 16];
    if pic_thunk::SUPPORTED {
        // SAFETY: enable() hasn't been called yet, so callee_addr still has
        // the original function bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(callee_addr as *const u8, prologue.as_mut_ptr(), 16);
        }
    }

    pic_repair::repair_pic_thunk(tramp_addr, callee_addr, &prologue);

    // SAFETY: enabling the detour.
    if let Err(e) = unsafe { detour.enable() } {
        error!(hook = name, error = %e, "detour enable failed");
        return false;
    }
    info!(hook = name, "detour INSTALLED");
    true
}
