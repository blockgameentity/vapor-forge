//! The stats schemas this process has seen, per app.
//!
//! The local rebuild is the *fallback* path: Steam's schema stays authoritative
//! when a reply carries one, and the rebuild answers only a reply that carries
//! none. Answering needs definitions, and the only source this host has is a
//! schema it observed earlier — the same shape as the Linux host's
//! `client::achievement_local` cache, which remembers the schema a reply carried so
//! a later schema-less reply can still be rebuilt.
//!
//! Bounded on purpose: the body that fills it runs on Steam's thread, so the table
//! keeps a handful of apps, evicts the oldest, and refuses a schema past
//! [`MAX_SCHEMA_BYTES`] rather than holding an arbitrary allocation.

use std::collections::VecDeque;
use std::sync::Mutex;

/// How many apps' schemas are kept.
pub const CAPACITY: usize = 8;

/// The largest schema kept, matching the protocol parser's own ceiling.
pub const MAX_SCHEMA_BYTES: usize = 512 * 1024;

/// A bounded `app id` → schema table, oldest evicted first.
pub struct SchemaCache {
    entries: Mutex<VecDeque<(u32, Vec<u8>)>>,
}

impl SchemaCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(VecDeque::new()),
        }
    }

    /// Remember `schema` for `app_id`.
    ///
    /// Idempotent per app: a newer schema replaces the older one and becomes the
    /// newest entry. An empty or oversized schema is ignored — an empty reply
    /// carries no definitions, and an oversized one is not a stats schema.
    pub fn remember(&self, app_id: u32, schema: &[u8]) {
        if app_id == 0 || schema.is_empty() || schema.len() > MAX_SCHEMA_BYTES {
            return;
        }
        let mut entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        entries.retain(|(held, _)| *held != app_id);
        while entries.len() >= CAPACITY {
            entries.pop_front();
        }
        entries.push_back((app_id, schema.to_vec()));
    }

    /// The schema remembered for `app_id`, copied out so no lock is held across the
    /// caller's own work.
    pub fn schema(&self, app_id: u32) -> Option<Vec<u8>> {
        let entries = self.entries.lock().unwrap_or_else(|error| error.into_inner());
        entries
            .iter()
            .find(|(held, _)| *held == app_id)
            .map(|(_, schema)| schema.clone())
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remembered_schema_is_found_by_its_app() {
        let cache = SchemaCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.schema(480), None);

        cache.remember(480, &[1, 2, 3]);
        assert_eq!(cache.schema(480), Some(vec![1, 2, 3]));
        assert_eq!(cache.schema(481), None);
    }

    #[test]
    fn an_empty_oversized_or_anonymous_schema_is_not_remembered() {
        let cache = SchemaCache::new();
        cache.remember(480, &[]);
        cache.remember(0, &[1, 2, 3]);
        cache.remember(481, &vec![0u8; MAX_SCHEMA_BYTES + 1]);
        assert!(cache.is_empty());
    }

    #[test]
    fn a_newer_schema_replaces_the_older_one_for_the_same_app() {
        let cache = SchemaCache::new();
        cache.remember(480, &[1]);
        cache.remember(480, &[2, 2]);
        assert_eq!(cache.schema(480), Some(vec![2, 2]));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn the_table_is_bounded_and_evicts_the_oldest_app() {
        let cache = SchemaCache::new();
        for app in 0..CAPACITY as u32 {
            cache.remember(app + 1, &[app as u8]);
        }
        cache.remember(9_999, &[7]);
        assert_eq!(cache.len(), CAPACITY);
        assert_eq!(cache.schema(1), None, "the oldest app is gone");
        assert_eq!(cache.schema(9_999), Some(vec![7]));
    }
}
