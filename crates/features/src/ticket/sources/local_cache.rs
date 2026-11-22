//! The `local_cache` ticket source.

use std::collections::HashMap;

use vapor_forge_config::{AppId, TicketSourceKind};

use super::{SourceFailure, Ticket, TicketSource};
use crate::ticket::cache::TicketCache;

/// The ticket this process already holds, or the one in the local ticket cache.
/// This is the single source the default `source_order` names, so it reproduces
/// the historical lookup exactly.
pub struct LocalCacheSource<'a> {
    cache: &'a TicketCache,
    lua_tickets: &'a HashMap<AppId, Vec<u8>>,
}

impl<'a> LocalCacheSource<'a> {
    pub fn new(cache: &'a TicketCache, lua_tickets: &'a HashMap<AppId, Vec<u8>>) -> Self {
        Self { cache, lua_tickets }
    }
}

impl TicketSource for LocalCacheSource<'_> {
    fn kind(&self) -> TicketSourceKind {
        TicketSourceKind::LocalCache
    }

    fn ticket(&self, app: AppId) -> Result<Option<Ticket>, SourceFailure> {
        Ok(self
            .cache
            .get_app_ticket(app, self.lua_tickets)
            .map(|bytes| Ticket::new(app, bytes)))
    }
}
