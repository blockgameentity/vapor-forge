use retour::GenericDetour;
use std::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::pic_repair;
use crate::plan::ValidatedHookTarget;
use crate::protect::{self, Protection};

/// Trampolines this process has created, in creation order.
///
/// `retour` allocates trampolines in executable memory that is writable while
/// the detour is being built. The addresses are kept so an install pass can
/// reopen them ([`ensure_trampoline_pages_writable`]) and harden them back to
/// read-execute afterwards ([`restore_trampoline_pages_rx`]). Each entry is a
/// raw trampoline address; page rounding is the protection backend's job.
static TRAMPOLINES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Function-pointer types the active backend can hook.
///
/// Callers bound their generics on this instead of naming the backend, so the
/// backend can be swapped per target architecture.
pub use retour::Function as HookFn;

/// An installed detour owning the patch and its trampoline.
///
/// Backend-specific; callers hold it opaquely and reach the original entry
/// point through [`crate::original::original_detour`].
///
/// # Platform behaviour
///
/// The backend is `retour`, which patches 32-bit and 64-bit x86 on both unix
/// and Windows, so this type has the same shape and the same API on either
/// platform: `new`, `enable`, `disable`, `is_enabled`, `trampoline`, and the
/// original-entry-point lookup through
/// [`crate::original::original_detour`] all behave identically. There is no
/// Windows-only restriction on the type.
///
/// Two engine-level steps around the backend do differ, and are stated here so
/// no caller assumes otherwise:
///
/// * PIC-thunk repair ([`create_detour`] → the `finalize` path) is 32-bit ELF
///   only. On MS-x64 it is skipped by design: x64 reaches globals through
///   RIP-relative operands, so a copied prologue holds no
///   `call __x86.get_pc_thunk.*` whose return address would need rewriting.
///   The `pic_thunk` gate refuses instead of decoding 32-bit bytes on a
///   64-bit target.
/// * The backend's patcher changes page protection through its own allocator
///   and does not marshal the instruction-cache flush. This crate's own writer
///   ([`crate::patch::write_code_patch`]) does, via
///   [`protect::flush_instruction_cache`] (`FlushInstructionCache` on Windows).
///   Prefer that writer when installing a patch on Windows outside `retour`.
pub type Detour<F> = GenericDetour<F>;

pub struct PendingDetour<F: HookFn> {
    pub detour: Detour<F>,
    pub callee_addr: usize,
}

/// Create a detour without enabling it.
///
/// # Safety
/// The validated target and replacement addresses must both point to functions
/// matching the signature `F`.
pub unsafe fn create_detour<F: HookFn>(
    name: &str,
    target: ValidatedHookTarget,
) -> Option<PendingDetour<F>> {
    let callee_addr = target.target_address;
    // Sanity check: reject obviously invalid target addresses.
    // During la_objopen, steamclient.so may not be fully relocated yet,
    // which can produce garbage addresses from pattern resolution.
    if callee_addr < 0x10000 {
        error!(
            hook = name,
            addr = format_args!("0x{:x}", callee_addr),
            "target address suspiciously low, skipping"
        );
        return None;
    }

    ensure_trampoline_pages_writable();

    // SAFETY: caller guarantees the validated addresses use signature F.
    let target_fn = unsafe { F::from_ptr(target.target_address as *const ()) };
    // SAFETY: caller guarantees the validated addresses use signature F.
    let replacement_fn = unsafe { F::from_ptr(target.replacement_address as *const ()) };

    // SAFETY: caller guarantees both function pointers are valid.
    // Note: GenericDetour::new() does NOT overwrite the original function.
    // that only happens on enable(). So the original prologue is still
    // readable at callee_addr until finalize_detour calls enable().
    match unsafe { GenericDetour::new(target_fn, replacement_fn) } {
        Ok(detour) => {
            debug!(hook = name, "detour created");
            Some(PendingDetour {
                detour,
                callee_addr,
            })
        }
        Err(e) => {
            error!(hook = name, error = %e, "retour detour creation failed");
            None
        }
    }
}

/// Restore all trampoline pages touched by PIC repair to RX.
///
/// Called after a loader-consistent install pass. A future create_detour call
/// will reopen these pages before asking retour to allocate again.
pub fn restore_trampoline_pages_rx() {
    set_trampoline_pages_protection(Protection::READ_EXECUTE, "RX");
}

/// Store a pending detour into its process-lifetime slot and finalize it.
///
/// Returns whether a detour was present and finalized.
///
/// # Safety
/// `storage` must point to a valid `Option<Detour<F>>` slot that lives
/// for the process lifetime.
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

fn ensure_trampoline_pages_writable() {
    set_trampoline_pages_protection(Protection::READ_WRITE_EXECUTE, "RWX");
}

fn set_trampoline_pages_protection(protection: Protection, label: &str) {
    let Ok(trampolines) = TRAMPOLINES.lock() else {
        warn!("trampoline list is poisoned, leaving trampoline protection alone");
        return;
    };
    if trampolines.is_empty() {
        return;
    }
    let mut failures = 0usize;
    for &trampoline in trampolines.iter() {
        // `trampoline` is the start of a trampoline retour allocated for this
        // process: it stays mapped for the process lifetime, and one byte is
        // enough for the backend to find the page that contains it.
        if protect::set_protection(trampoline, 1, protection).is_err() {
            failures += 1;
        }
    }
    let trampolines = trampolines.len();
    if failures == 0 {
        debug!(
            trampolines,
            protection = label,
            "trampoline protection applied"
        );
    } else {
        warn!(
            trampolines,
            failures,
            protection = label,
            "trampoline protection failed"
        );
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
    remember_trampoline(tramp_addr);

    // Read the original prologue NOW, before enable() overwrites it.
    let mut prologue = [0u8; 16];
    // SAFETY: enable() hasn't been called yet, so callee_addr still has
    // the original function bytes.
    unsafe {
        std::ptr::copy_nonoverlapping(callee_addr as *const u8, prologue.as_mut_ptr(), 16);
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

fn remember_trampoline(trampoline: usize) {
    let Ok(mut trampolines) = TRAMPOLINES.lock() else {
        warn!("trampoline list is poisoned, trampoline protection not tracked");
        return;
    };
    if !trampolines.contains(&trampoline) {
        trampolines.push(trampoline);
    }
}
