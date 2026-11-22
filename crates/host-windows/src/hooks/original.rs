//! The displaced-instruction trampoline each hook body calls through.
//!
//! A replacement function cannot "fall through" to the original entry: that entry
//! now holds the jump to us. The callable original is the trampoline the writer
//! allocated for the displaced instructions, and `hook-engine` hands its address
//! back as `WindowsDetour::trampoline()`.
//!
//! Publishing order matters and is the reason this is not part of the install
//! report: [`crate::hooks::installer`] records a symbol's trampoline *before* it
//! enables the entry patch, so any call that can reach a hook body already has a
//! published original. A symbol without one fails closed — see each body — and
//! that only happens if a body is somehow reached without the detour that
//! installed it.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

/// Symbol → trampoline address. Small (one entry per installed hook) and written
/// once per symbol during startup, read on every hooked call.
static ORIGINALS: OnceLock<RwLock<BTreeMap<String, usize>>> = OnceLock::new();

fn map() -> &'static RwLock<BTreeMap<String, usize>> {
    // Poisoning cannot lose an entry: every write is an insert of an immutable
    // address, so a recovered guard is as good as a clean one.
    ORIGINALS.get_or_init(|| RwLock::new(BTreeMap::new()))
}

fn read() -> std::sync::RwLockReadGuard<'static, BTreeMap<String, usize>> {
    map()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn write() -> std::sync::RwLockWriteGuard<'static, BTreeMap<String, usize>> {
    map()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Publish `symbol`'s trampoline. Re-publishing the same address is a no-op; a
/// different address replaces the old one and is logged, because two live
/// trampolines for one entry means the previous detour was dropped while a
/// replacement still pointed at it.
pub fn record(symbol: &str, trampoline: usize) {
    let mut originals = write();
    match originals.insert(symbol.to_owned(), trampoline) {
        Some(previous) if previous != trampoline => tracing::warn!(
            symbol,
            previous = format_args!("{previous:#x}"),
            trampoline = format_args!("{trampoline:#x}"),
            "a second trampoline was published for one symbol"
        ),
        _ => tracing::debug!(
            symbol,
            trampoline = format_args!("{trampoline:#x}"),
            "original path published"
        ),
    }
}

/// Address of `symbol`'s trampoline, when one has been published.
pub fn address(symbol: &str) -> Option<usize> {
    read().get(symbol).copied().filter(|address| *address != 0)
}

/// Typed callable original for `symbol`.
///
/// # Safety
/// `F` must be the signature the symbol's corpus anchor resolves to; the
/// trampoline executes the instructions displaced from that entry, so calling it
/// through any other signature is undefined behaviour. Every caller in
/// [`crate::hooks`] passes the type from [`crate::abi`] that documents which
/// parts of the shape are measured.
pub unsafe fn original_of<F: Copy>(symbol: &str) -> Option<F> {
    let address = address(symbol)?;
    // SAFETY: the caller upholds the signature contract; `address` is an
    // executable trampoline published by the installer for this symbol.
    Some(unsafe { std::mem::transmute_copy::<usize, F>(&address) })
}

#[cfg(test)]
pub(crate) fn forget(symbol: &str) {
    write().remove(symbol);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unpublished_symbol_has_no_original() {
        assert!(address("abi-test::never-installed").is_none());
    }

    #[test]
    fn a_published_trampoline_round_trips_through_the_typed_reader() {
        // SAFETY: the test decodes the address only; it never invokes it.
        let read_back = unsafe { original_of::<usize>("abi-test::round-trip") };
        assert!(read_back.is_none());

        record("abi-test::round-trip", 0x1234_5678);
        assert_eq!(address("abi-test::round-trip"), Some(0x1234_5678));
        // SAFETY: as above: the value is compared, not called.
        let typed = unsafe { original_of::<usize>("abi-test::round-trip") };
        assert_eq!(typed, Some(0x1234_5678));
        forget("abi-test::round-trip");
        assert!(address("abi-test::round-trip").is_none());
    }
}
