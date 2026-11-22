//! The unix detour backend.
//!
//! `retour` decodes the prologue, relocates what it can (RIP-relative operands
//! and relative branches), allocates the trampoline, and restores the original
//! bytes when the detour is disabled or dropped. The engine keeps its own
//! discipline around it: PIC-thunk repair on 32-bit ELF (see
//! [`crate::pic_repair`]) and the trampoline protection pass (see
//! [`super::trampolines`]).

use retour::GenericDetour;

/// Function-pointer types the active backend can hook.
pub use retour::Function as HookFn;

/// An installed detour owning the patch and its trampoline.
///
/// `retour` supplies the whole lifecycle here: the trampoline stays mapped for
/// the process lifetime, `enable`/`disable` toggle the entry patch, and `Drop`
/// disables and releases both.
pub type Detour<F> = GenericDetour<F>;
