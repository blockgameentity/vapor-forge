//! Structural Denuvo detection and the manual override that wins over it.
//!
//! Detection looks only at structure the caller already has in hand: PE section
//! names, loaded DLL names, and a flag bit the launcher sets when it has
//! independent evidence. No app id is special-cased, no file is opened here, and
//! the detection result is a value so it can be reasoned about in tests.
//!
//! A structural verdict can go stale when the game is updated, so
//! [`DenuvoTracker`] remembers the last version and fingerprint per app and
//! reports a change as the cue to re-evaluate.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use tracing::info;
use vapor_forge_config::{AppId, TicketSection};

/// Flag bit a caller sets when it already knows the app is Denuvo-protected.
pub const FLAG_DENUVO: u32 = 1 << 0;

/// Marker looked for in section and DLL names, case-insensitively.
const MARKER: &str = "denuvo";

/// The app metadata the detector is allowed to look at.
#[derive(Clone, Copy, Debug)]
pub struct AppMetadata<'a> {
    pub app: AppId,
    /// App version/build, when the caller knows it. A change is what makes the
    /// verdict worth re-running.
    pub version: Option<u64>,
    pub section_names: &'a [String],
    pub dll_names: &'a [String],
    pub flags: u32,
}

/// Why an app looked Denuvo-protected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Evidence {
    Section { name: String },
    Dll { name: String },
    Flag { bit: u32 },
}

/// Structural verdict for one app.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DenuvoDetection {
    evidence: Vec<Evidence>,
    fingerprint: u64,
}

impl DenuvoDetection {
    pub fn is_denuvo_like(&self) -> bool {
        !self.evidence.is_empty()
    }

    pub fn evidence(&self) -> &[Evidence] {
        &self.evidence
    }

    /// Stable for as long as the same evidence is seen, so a changed value means
    /// the protection layer changed underneath us.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

/// The verdict plus where it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DenuvoDecision {
    /// `[ticket] denuvo_override` decided; detection was not consulted.
    Overridden { denuvo_like: bool },
    /// Structural detection decided.
    Detected(DenuvoDetection),
    /// Detection is disabled and no override is set: nothing is treated as
    /// Denuvo-protected, which is the historical behaviour.
    Undetermined,
}

impl DenuvoDecision {
    pub fn is_denuvo_like(&self) -> bool {
        match self {
            Self::Overridden { denuvo_like } => *denuvo_like,
            Self::Detected(detection) => detection.is_denuvo_like(),
            Self::Undetermined => false,
        }
    }

    /// Short, log-safe description of where the verdict came from.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Overridden { denuvo_like: true } => "manual override: Denuvo",
            Self::Overridden { denuvo_like: false } => "manual override: not Denuvo",
            Self::Detected(_) => "structural detection",
            Self::Undetermined => "detection disabled",
        }
    }
}

/// Classify one app from its metadata. Pure: same input, same verdict.
pub fn detect(metadata: &AppMetadata<'_>) -> DenuvoDetection {
    let mut evidence = Vec::new();
    for name in metadata.section_names {
        if is_marked(name) {
            evidence.push(Evidence::Section { name: name.clone() });
        }
    }
    for name in metadata.dll_names {
        if is_marked(name) {
            evidence.push(Evidence::Dll { name: name.clone() });
        }
    }
    if metadata.flags & FLAG_DENUVO != 0 {
        evidence.push(Evidence::Flag { bit: FLAG_DENUVO });
    }

    let fingerprint = if evidence.is_empty() {
        0
    } else {
        fingerprint_of(&evidence)
    };
    DenuvoDetection {
        evidence,
        fingerprint,
    }
}

/// Manual override first, then detection, then the historical "not Denuvo".
pub fn decide(metadata: &AppMetadata<'_>, section: &TicketSection) -> DenuvoDecision {
    if let Some(denuvo_like) = section.denuvo_override {
        return DenuvoDecision::Overridden { denuvo_like };
    }
    if section.denuvo_detection {
        return DenuvoDecision::Detected(detect(metadata));
    }
    DenuvoDecision::Undetermined
}

fn is_marked(name: &str) -> bool {
    name.to_ascii_lowercase().contains(MARKER)
}

/// Order-independent digest of the evidence: the same structure listed in a
/// different order is the same structure.
fn fingerprint_of(evidence: &[Evidence]) -> u64 {
    let mut keys: Vec<String> = evidence.iter().map(key_of).collect();
    keys.sort();
    let mut hasher = DefaultHasher::new();
    keys.hash(&mut hasher);
    hasher.finish()
}

fn key_of(evidence: &Evidence) -> String {
    match evidence {
        Evidence::Section { name } => format!("section:{name}"),
        Evidence::Dll { name } => format!("dll:{name}"),
        Evidence::Flag { bit } => format!("flag:{bit}"),
    }
}

/// What changed since the last observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerdictChange {
    /// First time this app was classified in this process.
    FirstSeen,
    /// Same version and same structure.
    Unchanged,
    /// Version or structure changed, so the verdict was re-evaluated.
    Changed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Seen {
    version: Option<u64>,
    fingerprint: u64,
}

/// Last verdict input per app.
///
/// Bounded by the apps the caller observes; [`DenuvoTracker::forget`] drops one
/// when the app exits.
#[derive(Debug, Default)]
pub struct DenuvoTracker {
    seen: HashMap<AppId, Seen>,
}

impl DenuvoTracker {
    /// Classify an app and report whether anything changed since last time.
    pub fn observe(
        &mut self,
        metadata: &AppMetadata<'_>,
        section: &TicketSection,
    ) -> (DenuvoDecision, VerdictChange) {
        let decision = decide(metadata, section);
        let fingerprint = match &decision {
            DenuvoDecision::Detected(detection) => detection.fingerprint(),
            // An override is a policy, not a structure: only the version can
            // make it worth re-evaluating.
            DenuvoDecision::Overridden { .. } | DenuvoDecision::Undetermined => 0,
        };
        let current = Seen {
            version: metadata.version,
            fingerprint,
        };
        let change = match self.seen.get(&metadata.app) {
            None => VerdictChange::FirstSeen,
            Some(previous) if *previous == current => VerdictChange::Unchanged,
            Some(_) => VerdictChange::Changed,
        };
        self.seen.insert(metadata.app, current);
        if change == VerdictChange::Changed {
            info!(
                app = metadata.app.0,
                version = ?metadata.version,
                reason = decision.reason(),
                "ticket: Denuvo verdict re-evaluated"
            );
        }
        (decision, change)
    }

    /// The fingerprint recorded for `app`, if it has been observed.
    pub fn last_fingerprint(&self, app: AppId) -> Option<u64> {
        self.seen.get(&app).map(|seen| seen.fingerprint)
    }

    /// Drop the record for `app`; returns whether there was one.
    pub fn forget(&mut self, app: AppId) -> bool {
        self.seen.remove(&app).is_some()
    }
}

#[cfg(test)]
mod tests;
