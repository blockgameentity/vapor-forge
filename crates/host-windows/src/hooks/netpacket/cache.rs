//! The request codes this process holds, keyed by `(depot, gid)`.
//!
//! The body in [`super`] runs on Steam's networking thread, so a lookup must
//! never allocate, never block on I/O and never hold a lock across the original
//! call. [`CodeCache`] is therefore a fixed-capacity, append-ordered table behind
//! one mutex: `get` copies a [`CachedCode`] out (it is `Copy`), `insert` overwrites
//! the oldest entry instead of growing.
//!
//! # Who fills it
//!
//! Not the hook. A provider fetch takes a network round trip, so it belongs on the
//! worker: the hook looks a code up, and if there is none it records
//! [`ServeBlocker::NoCachedCode`](super::decision::ServeBlocker::NoCachedCode) and
//! forwards the message. The next message for the same target is answered from the
//! cache once the worker has stored one. Nothing here dials.

use std::sync::Mutex;

use super::decision::{CachedCode, RequestTarget};

/// How many codes one process keeps. Bounded because the writer is a worker and
/// the reader is a hot path: 128 entries is ~4 KiB and covers far more manifests
/// than a session asks for.
pub const CAPACITY: usize = 128;

/// A `(depot, gid)` → code table with a fixed bound.
pub struct CodeCache {
    entries: Mutex<Entries>,
}

struct Entries {
    /// Oldest first; an insert past [`CAPACITY`] drops `front`.
    ordered: std::collections::VecDeque<(RequestTarget, CachedCode)>,
}

impl CodeCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Entries {
                ordered: std::collections::VecDeque::new(),
            }),
        }
    }

    /// The code held for `target`, if any.
    ///
    /// Copies the value out and releases the lock before returning, so the caller
    /// can then call into Steam without holding it.
    pub fn get(&self, target: RequestTarget) -> Option<CachedCode> {
        let entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        entries
            .ordered
            .iter()
            .find(|(held, _)| *held == target)
            .map(|(_, code)| *code)
    }

    /// Store `code` for `target`, evicting the oldest entry when full.
    ///
    /// Idempotent per target: a second code for the same pair replaces the first
    /// and moves it to the newest position, so a repeatedly refreshed manifest is
    /// not evicted while it is hot.
    pub fn insert(&self, target: RequestTarget, code: CachedCode) {
        let mut entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        entries.ordered.retain(|(held, _)| *held != target);
        while entries.ordered.len() >= CAPACITY {
            entries.ordered.pop_front();
        }
        entries.ordered.push_back((target, code));
    }

    /// How many codes are held, for the report and for tests.
    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .ordered
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for CodeCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(depot: u32, gid: u64) -> RequestTarget {
        RequestTarget { depot, gid }
    }

    fn code(code: u64) -> CachedCode {
        CachedCode {
            code,
            provider: Some("test"),
        }
    }

    #[test]
    fn a_cached_code_is_found_by_its_pair_and_nothing_else() {
        let cache = CodeCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.get(target(731, 1)), None);

        cache.insert(target(731, 1), code(0xabc));
        assert_eq!(cache.get(target(731, 1)).map(|held| held.code), Some(0xabc));
        // A different depot for the same manifest, and the same depot for a
        // different manifest, are different lookups.
        assert_eq!(cache.get(target(732, 1)), None);
        assert_eq!(cache.get(target(731, 2)), None);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_second_code_for_one_pair_replaces_the_first() {
        let cache = CodeCache::new();
        cache.insert(target(731, 1), code(1));
        cache.insert(target(731, 1), code(2));
        assert_eq!(cache.get(target(731, 1)).map(|held| held.code), Some(2));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn the_table_is_bounded_and_drops_the_oldest_entry() {
        let cache = CodeCache::new();
        for index in 0..CAPACITY {
            cache.insert(target(731, index as u64), code(index as u64));
        }
        assert_eq!(cache.len(), CAPACITY);
        cache.insert(target(731, 9_999), code(9_999));
        assert_eq!(cache.len(), CAPACITY, "the table never grows past its bound");
        assert_eq!(cache.get(target(731, 0)), None, "the oldest entry is gone");
        assert_eq!(cache.get(target(731, 9_999)).map(|held| held.code), Some(9_999));
    }

    #[test]
    fn refreshing_a_pair_keeps_it_newest_so_a_hot_manifest_survives() {
        let cache = CodeCache::new();
        cache.insert(target(731, 0), code(0));
        for index in 1..CAPACITY {
            cache.insert(target(731, index as u64), code(index as u64));
        }
        // Touch the oldest pair, then push one more: an unrelated pair goes.
        cache.insert(target(731, 0), code(42));
        cache.insert(target(731, 9_999), code(9_999));
        assert_eq!(cache.get(target(731, 0)).map(|held| held.code), Some(42));
        assert_eq!(cache.get(target(731, 1)), None);
    }
}
