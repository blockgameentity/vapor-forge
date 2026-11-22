//! One lock for tests that mutate process-wide feature state.
//!
//! `features::config_apply::apply` publishes the achievement flag and the family
//! settings globally, so a test that applies a config and a test that asserts on
//! the flag cannot run at the same time without depending on scheduling order.
//! Both take this lock; it is test-only and costs nothing in a shipped build.

#[cfg(test)]
use std::sync::{Mutex, MutexGuard};

#[cfg(test)]
static LOCK: Mutex<()> = Mutex::new(());

/// Serialize a test against every other test that touches the shared feature
/// state.
#[cfg(test)]
pub(crate) fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}
