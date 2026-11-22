//! One process-wide lock for the tests that touch a process-global switch.
//!
//! `features::run` (and therefore `entry::worker::run`) ends in
//! `features::achievements::apply`, which writes the process-wide local-rebuild
//! flag. `features::tests::gate` asserts on that flag, so a second test running
//! `features::run` at the same time can flip it between the write and the
//! assertion. Both sides therefore take *the same* mutex, and the guard is held
//! for the whole test body.
//!
//! It lives here, rather than in either test module, because the two modules
//! cannot see each other's private items and a lock that only one side takes is
//! not a lock.

#[cfg(test)]
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take the process-global test lock.
///
/// Poisoning is recovered rather than propagated: the lock guards nothing but the
/// ordering of test bodies, so a test that panicked left no state behind it that a
/// clean guard would not.
#[cfg(test)]
pub(crate) fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}
