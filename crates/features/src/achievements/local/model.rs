//! What the rebuild treats as a locally known achievement.
//!
//! These types describe what a client renders, not how Valve's binary KeyValues
//! schema stores it: a key, the wire id the unlock bits live under, the display
//! attributes, and the groups the achievement is listed in. The host builds a
//! set from whatever local source it considers authoritative (an installed
//! schema, a cached definition table, a bundled catalog) and the rebuild never
//! mutates it.
//!
//! Nothing here trusts its input: a definition that cannot be rendered or
//! addressed on the wire reports [`LocalDefinitionError`] instead of being
//! emitted, so a damaged local catalog costs that one achievement, not the page.

/// The largest bit Steam's schema loader accepts within one achievements stat.
///
/// `parse_achievement_bit_mappings` drops anything above it, and the wire form
/// carries unlock flags as a 32-bit mask, so a higher bit could never be
/// addressed.
pub const MAX_LOCAL_ACHIEVEMENT_BIT: u32 = 31;

/// The attributes a client shows for one achievement.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AchievementDisplay {
    pub display_name: String,
    pub description: String,
    pub hidden: bool,
}

impl AchievementDisplay {
    /// The attributes to use when only the key is known.
    ///
    /// The key stands in for the name, because a row with no text reads as a
    /// broken entry. Nothing is assumed to be hidden: an achievement the client
    /// hides cannot be revealed through the list, so guessing "hidden" would
    /// cost the player an achievement they may well have unlocked.
    pub fn from_key(key: &str) -> Self {
        Self {
            display_name: key.to_owned(),
            description: String::new(),
            hidden: false,
        }
    }
}

/// One locally known achievement definition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAchievement {
    /// The achievement key, matching the schema's `name` value.
    pub key: String,
    /// The achievements stat the unlock bit belongs to.
    pub stat_id: u32,
    /// The bit within `stat_id`, `0..=MAX_LOCAL_ACHIEVEMENT_BIT`.
    pub bit: u32,
    pub display: AchievementDisplay,
    /// Group keys this achievement is listed in, in any order. Empty for a flat
    /// layout. The rebuild keeps every membership, so an achievement listed in
    /// two groups stays one achievement with two memberships.
    pub groups: Vec<String>,
}

impl LocalAchievement {
    pub fn new(key: &str, stat_id: u32, bit: u32) -> Self {
        Self {
            key: key.to_owned(),
            stat_id,
            bit,
            display: AchievementDisplay::from_key(key),
            groups: Vec::new(),
        }
    }

    /// Check the parts a client cannot render or the wire form cannot carry.
    pub fn validate(&self) -> Result<(), LocalDefinitionError> {
        if self.key.trim().is_empty() {
            return Err(LocalDefinitionError::BlankKey);
        }
        if self.bit > MAX_LOCAL_ACHIEVEMENT_BIT {
            return Err(LocalDefinitionError::BitOutOfRange { bit: self.bit });
        }
        Ok(())
    }
}

/// A display group achievements are listed under.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalAchievementGroup {
    pub key: String,
    pub display_name: String,
}

impl LocalAchievementGroup {
    pub fn new(key: &str, display_name: &str) -> Self {
        Self {
            key: key.to_owned(),
            display_name: display_name.to_owned(),
        }
    }

    pub fn validate(&self) -> Result<(), LocalDefinitionError> {
        if self.key.trim().is_empty() {
            return Err(LocalDefinitionError::BlankGroupKey);
        }
        Ok(())
    }
}

/// Everything known locally about one app's achievements.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalAchievementSet {
    pub groups: Vec<LocalAchievementGroup>,
    pub achievements: Vec<LocalAchievement>,
}

impl LocalAchievementSet {
    pub fn achievement(&self, key: &str) -> Option<&LocalAchievement> {
        self.achievements
            .iter()
            .find(|achievement| achievement.key == key)
    }
}

/// The account's state for one achievement.
///
/// Only `unlocked` decides the wire flag. `unlock_time` is the rtime32 the
/// client shows on an unlocked achievement; an unlocked achievement with no
/// usable time is encoded with Steam's "unlocked, time unknown" sentinel rather
/// than omitted, because the flag is authoritative and the time is not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AchievementUnlockState {
    pub key: String,
    pub unlocked: bool,
    pub unlock_time: Option<u32>,
}

impl AchievementUnlockState {
    pub fn unlocked(key: &str, unlock_time: Option<u32>) -> Self {
        Self {
            key: key.to_owned(),
            unlocked: true,
            unlock_time,
        }
    }

    pub fn locked(key: &str) -> Self {
        Self {
            key: key.to_owned(),
            unlocked: false,
            unlock_time: None,
        }
    }
}

/// Why a local definition cannot be rebuilt as-is.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum LocalDefinitionError {
    #[error("achievement key is blank")]
    BlankKey,
    #[error("achievement bit {bit} is outside the 0..=31 range a stats schema accepts")]
    BitOutOfRange { bit: u32 },
    #[error("group key is blank")]
    BlankGroupKey,
    #[error("achievement `{key}` reuses wire id {stat_id}/{bit} already used by `{first}`")]
    DuplicateWireId {
        key: String,
        first: String,
        stat_id: u32,
        bit: u32,
    },
}
