//! Thread-safe ticket cache.
//!
//! - Forge source tickets: always memory-only.
//! - Delegate captured tickets: always persisted to disk.
//! - Other tickets: controlled by `persist` flag at store time.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing::{debug, info, warn};
use vapor_forge_config::AppId;

/// Thread-safe ticket cache.
pub struct TicketCache {
    cache_dir: Option<PathBuf>,
    app_tickets: Mutex<HashMap<AppId, Vec<u8>>>,
    enc_tickets: Mutex<HashMap<AppId, Vec<u8>>>,
}

impl TicketCache {
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self {
            cache_dir,
            app_tickets: Mutex::new(HashMap::new()),
            enc_tickets: Mutex::new(HashMap::new()),
        }
    }

    /// Whether we already have an app ticket cached (memory or disk).
    pub fn has_app_ticket(&self, app_id: AppId) -> bool {
        if self
            .app_tickets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(&app_id)
        {
            return true;
        }
        self.load_from_disk(app_id, "ticket").is_some()
    }

    /// Get an app ticket. Priority: Lua-provided → memory cache → disk cache.
    pub fn get_app_ticket(
        &self,
        app_id: AppId,
        lua_tickets: &HashMap<AppId, Vec<u8>>,
    ) -> Option<Vec<u8>> {
        if let Some(t) = lua_tickets.get(&app_id) {
            debug!(app_id = app_id.0, "ticket: using Lua-provided app ticket");
            return Some(t.clone());
        }
        if let Some(t) = self
            .app_tickets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&app_id)
        {
            debug!(app_id = app_id.0, "ticket: using cached app ticket");
            return Some(t.clone());
        }
        if let Some(t) = self.load_from_disk(app_id, "ticket") {
            debug!(app_id = app_id.0, "ticket: loaded app ticket from disk");
            self.app_tickets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(app_id, t.clone());
            return Some(t);
        }
        None
    }

    /// Get an encrypted app ticket. Priority: Lua-provided → memory cache → disk cache.
    pub fn get_enc_ticket(
        &self,
        app_id: AppId,
        lua_tickets: &HashMap<AppId, Vec<u8>>,
    ) -> Option<Vec<u8>> {
        if let Some(t) = lua_tickets.get(&app_id) {
            debug!(
                app_id = app_id.0,
                "ticket: using Lua-provided encrypted ticket"
            );
            return Some(t.clone());
        }
        if let Some(t) = self
            .enc_tickets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&app_id)
        {
            debug!(app_id = app_id.0, "ticket: using cached encrypted ticket");
            return Some(t.clone());
        }
        if let Some(t) = self.load_from_disk(app_id, "enc_ticket") {
            debug!(
                app_id = app_id.0,
                "ticket: loaded encrypted ticket from disk"
            );
            self.enc_tickets
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(app_id, t.clone());
            return Some(t);
        }
        None
    }

    /// Cache a ticket. `persist` = true writes to disk (for delegate tickets
    /// that must survive across account sessions).
    pub fn store_app_ticket(&self, app_id: AppId, data: Vec<u8>, persist: bool) {
        info!(
            app_id = app_id.0,
            size = data.len(),
            persist,
            "ticket: caching app ticket"
        );
        if persist {
            self.save_to_disk(app_id, "ticket", &data);
        }
        self.app_tickets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(app_id, data);
    }

    /// Cache an encrypted ticket. `persist` = true writes to disk.
    pub fn store_enc_ticket(&self, app_id: AppId, data: Vec<u8>, persist: bool) {
        info!(
            app_id = app_id.0,
            size = data.len(),
            persist,
            "ticket: caching encrypted ticket"
        );
        if persist {
            self.save_to_disk(app_id, "enc_ticket", &data);
        }
        self.enc_tickets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(app_id, data);
    }

    fn load_from_disk(&self, app_id: AppId, prefix: &str) -> Option<Vec<u8>> {
        let dir = self.cache_dir.as_ref()?;
        let path = dir.join(format!("{}_{}.bin", prefix, app_id.0));
        std::fs::read(&path).ok()
    }

    fn save_to_disk(&self, app_id: AppId, prefix: &str, data: &[u8]) {
        let Some(dir) = &self.cache_dir else { return };
        if let Err(error) = create_private_dir(dir) {
            warn!(%error, app_id = app_id.0, "ticket: cache directory setup failed");
            return;
        }
        let path = dir.join(format!("{}_{}.bin", prefix, app_id.0));
        if let Err(error) = write_private_file(&path, data) {
            warn!(%error, app_id = app_id.0, "ticket: disk cache write failed");
        }
    }
}

fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_private_file(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_stores_and_retrieves_app_ticket() {
        let cache = TicketCache::new(None);
        let ticket = vec![0xDE, 0xAD, 0xBE, 0xEF];
        cache.store_app_ticket(AppId(480), ticket.clone(), false);

        let empty_lua: HashMap<AppId, Vec<u8>> = HashMap::new();
        assert_eq!(cache.get_app_ticket(AppId(480), &empty_lua), Some(ticket));
        assert_eq!(cache.get_app_ticket(AppId(730), &empty_lua), None);
    }

    #[test]
    fn lua_tickets_take_priority() {
        let cache = TicketCache::new(None);
        cache.store_app_ticket(AppId(480), vec![0x01, 0x02], false);

        let mut lua: HashMap<AppId, Vec<u8>> = HashMap::new();
        lua.insert(AppId(480), vec![0xAA, 0xBB]);

        assert_eq!(
            cache.get_app_ticket(AppId(480), &lua),
            Some(vec![0xAA, 0xBB])
        );
    }

    #[test]
    fn persist_true_writes_to_disk() {
        let dir = std::env::temp_dir().join(format!("ticket-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = TicketCache::new(Some(dir.clone()));

        let ticket = vec![0x11, 0x22, 0x33, 0x44];
        cache.store_app_ticket(AppId(999), ticket.clone(), true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(dir.join("ticket_999.bin"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }

        // Fresh cache (simulates restart) reads from disk
        let cache2 = TicketCache::new(Some(dir.clone()));
        let empty_lua: HashMap<AppId, Vec<u8>> = HashMap::new();
        assert_eq!(cache2.get_app_ticket(AppId(999), &empty_lua), Some(ticket));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persist_false_does_not_touch_disk() {
        let dir = std::env::temp_dir().join(format!("ticket-nodisk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = TicketCache::new(Some(dir.clone()));

        cache.store_app_ticket(AppId(480), vec![0xFF], false);

        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enc_ticket_cache_works() {
        let cache = TicketCache::new(None);
        let ticket = vec![0xCA, 0xFE];
        cache.store_enc_ticket(AppId(480), ticket.clone(), false);

        let empty_lua: HashMap<AppId, Vec<u8>> = HashMap::new();
        assert_eq!(cache.get_enc_ticket(AppId(480), &empty_lua), Some(ticket));
        assert_eq!(cache.get_enc_ticket(AppId(730), &empty_lua), None);
    }
}
