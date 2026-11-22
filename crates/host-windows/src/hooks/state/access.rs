//! What a hook body may read: the accessor half of [`HostState`].
//!
//! Split from `state.rs` at the 300-line budget. These are pure reads of already
//! published state plus the interior writers (the gate and the once-decoded packet
//! layout / reply geometry), so the file that owns the type stays readable and this
//! one stays a table of accessors.

use std::collections::HashMap;

use vapor_forge_config::{AppId, DepotId, RuntimeConfig};
use vapor_forge_features::family::FamilySettings;
use vapor_forge_features::package::PackageState;
use vapor_forge_features::ticket::authorisation::AuthorisationWindow;
use vapor_forge_features::ticket::TicketCache;

use crate::capability::{Capability, Gate};
use crate::features::achievements::AchievementSettings;
use crate::features::{CloudPlan, IdentitySettings};
use crate::hooks::depot_store::DepotPlan;
use crate::hooks::netpacket::ledger::Ledger as NetPacketLedger;
use crate::hooks::state::HostState;

impl HostState {
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    pub fn package(&self) -> &PackageState {
        &self.package
    }

    pub fn family_settings(&self) -> FamilySettings {
        self.family
    }

    /// The current account's Steam account id, or 0 when it is not known.
    ///
    /// The Windows host has no measured way to read it yet, so it is always 0.
    /// `features::family::provenance::lender_account` treats 0 as "unknown", not
    /// as a lender, so a family label is never invented from it.
    pub fn self_account_id(&self) -> u32 {
        0
    }

    pub fn controlled_apps(&self) -> &[AppId] {
        &self.controlled
    }

    pub fn ticket_cache(&self) -> &TicketCache {
        &self.ticket_cache
    }

    pub fn lua_tickets(&self) -> &HashMap<AppId, Vec<u8>> {
        &self.lua_tickets
    }

    pub fn depot(&self) -> &DepotPlan {
        &self.depot
    }

    /// The cloud plan resolved at start-up.
    pub fn cloud(&self) -> &CloudPlan {
        &self.cloud
    }

    /// The Lua-registered depot decryption keys.
    pub fn depot_keys(&self) -> &HashMap<DepotId, Vec<u8>> {
        &self.depot_keys
    }

    /// The resolved per-game SteamID overrides.
    pub fn identity_settings(&self) -> &IdentitySettings {
        &self.identity
    }

    /// The resolved `[achievements]` settings.
    pub fn achievement_settings(&self) -> &AchievementSettings {
        &self.achievements
    }

    /// What the CM dispatch hook recorded, for diagnostics.
    pub fn netpacket(&self) -> &NetPacketLedger {
        &self.netpacket
    }

    /// The Denuvo lease this process holds, if `[ticket] denuvo_mode` asked for
    /// one.
    pub fn authorisation(&self) -> Option<AuthorisationWindow> {
        self.authorisation
    }

    /// Whether `capability` survived the install pass.
    pub fn allows(&self, capability: Capability) -> bool {
        self.gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_enabled(capability)
    }

    /// Snapshot of the gate, for the one feature pass that runs after install.
    pub fn gate(&self) -> Gate {
        self.gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The install pass's reason, when `capability` was disabled by it.
    pub fn disabled_reason(&self, capability: Capability) -> Option<String> {
        self.gate
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reason(capability)
            .map(str::to_owned)
    }

    /// Replace the gate with the install pass's result.
    pub fn set_gate(&self, gate: Gate) {
        *self
            .gate
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = gate;
    }
}
