//! Dependency-aware depot resolution.
//!
//! Steam asks for one manifest code per depot. A depot set that omits the
//! depots another depot's content sits on installs partially: the game starts
//! and then fails on a missing base depot. [`resolve_depot_closure`] widens a
//! requested depot set to everything it transitively declares, before any of it
//! is installed.
//!
//! The walk itself is pure: the host supplies declared dependencies through
//! [`DepotDependencySource`].

use std::collections::HashSet;

use thiserror::Error;
use tracing::debug;
use vapor_forge_config::DepotId;

/// Default bound on how far the closure may walk.
///
/// Real depot sets nest two or three levels (DLC depot -> base depot -> build
/// dependency). A longer chain means the metadata is wrong, so the walk stops
/// with an error instead of paging in most of the app.
pub const DEFAULT_MAX_DEPENDENCY_DEPTH: usize = 8;

/// The relationship that pulled a depot into the closure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DependencyKind {
    /// The base depot this depot's content sits on.
    Base,
    /// A DLC depot belonging to the same app.
    Dlc,
    /// A `BuildDepotDependency` from the app's build manifest.
    #[default]
    Build,
}

impl DependencyKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Dlc => "dlc",
            Self::Build => "build",
        }
    }
}

impl std::fmt::Display for DependencyKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `as_str` rather than `Debug`: this text reaches users.
        formatter.write_str(self.as_str())
    }
}

/// One dependency a depot declares.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepotDependency {
    pub depot_id: DepotId,
    pub kind: DependencyKind,
}

impl DepotDependency {
    pub const fn new(depot_id: DepotId, kind: DependencyKind) -> Self {
        Self { depot_id, kind }
    }
}

/// Why a depot belongs in the closure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DepotReason {
    /// The package the caller asked about requires this depot.
    Requested,
    /// A depot already in the closure declares this one.
    Dependency {
        declared_by: DepotId,
        kind: DependencyKind,
    },
}

impl std::fmt::Display for DepotReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Requested => formatter.write_str("required by the package"),
            Self::Dependency { declared_by, kind } => {
                write!(formatter, "{kind} dependency of depot {declared_by}")
            }
        }
    }
}

/// One depot in the resolved closure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedDepot {
    pub depot_id: DepotId,
    /// Hops from the requested set; a requested depot is `0`.
    pub depth: usize,
    /// Why this depot is here, ready to be shown to the user.
    pub reason: DepotReason,
}

/// Where declared depot dependencies come from.
///
/// The host implements this over its app/depot metadata (appinfo,
/// `BuildDepotDependency`, DLC lists). Keeping it a trait is what lets the
/// closure logic stay pure, deterministic and testable without a network or a
/// Steam session.
pub trait DepotDependencySource {
    /// Dependencies `depot_id` declares. An empty vector means "none known",
    /// which is a normal answer for a leaf depot.
    fn dependencies(&self, depot_id: DepotId) -> Result<Vec<DepotDependency>, String>;
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum DependencyError {
    /// The source could not answer for a depot; the closure is incomplete, and
    /// installing it would leave content behind.
    #[error("could not look up dependencies of depot {depot}: {reason}")]
    Lookup { depot: DepotId, reason: String },
    /// A depot depends on itself, directly or through other depots.
    #[error("depot dependency cycle: {}", .0.iter().map(DepotId::to_string).collect::<Vec<_>>().join(" -> "))]
    Cycle(Vec<DepotId>),
    /// The closure walked further than the caller allows.
    #[error("depot dependency closure is deeper than the limit of {limit} (at depot {depot})")]
    DepthLimit { depot: DepotId, limit: usize },
}

/// Resolve `requested` into a complete depot set.
///
/// The result is deterministic: the requested order is kept, a depot's own
/// dependencies are visited in ascending depot-id order (so a host that returns
/// its metadata in another order cannot reorder the closure), and a depot
/// reached by several routes — a diamond — is added once, by the first route
/// that reached it. Every entry carries the reason it is present.
///
/// A cycle is an error carrying the cycle itself, never a panic and never an
/// endless walk.
pub fn resolve_depot_closure(
    requested: &[DepotId],
    source: &impl DepotDependencySource,
    max_depth: usize,
) -> Result<Vec<ResolvedDepot>, DependencyError> {
    let mut walker = Walker {
        source,
        max_depth,
        resolved: Vec::new(),
        seen: HashSet::new(),
        path: Vec::new(),
    };
    for depot_id in requested {
        walker.visit(*depot_id, DepotReason::Requested, 0)?;
    }
    Ok(walker.resolved)
}

struct Walker<'a, S> {
    source: &'a S,
    max_depth: usize,
    resolved: Vec<ResolvedDepot>,
    /// Every depot already in the closure, so a diamond is added once.
    seen: HashSet<DepotId>,
    /// The current route from a requested depot, for cycle reporting.
    path: Vec<DepotId>,
}

impl<S: DepotDependencySource> Walker<'_, S> {
    fn visit(
        &mut self,
        depot_id: DepotId,
        reason: DepotReason,
        depth: usize,
    ) -> Result<(), DependencyError> {
        // Tested before `seen`: a depot that is already resolved *and* still on
        // the current route is a cycle, while one that is merely resolved is a
        // diamond.
        if let Some(start) = self.path.iter().position(|depot| *depot == depot_id) {
            let mut cycle = self.path[start..].to_vec();
            cycle.push(depot_id);
            return Err(DependencyError::Cycle(cycle));
        }
        if depth > self.max_depth {
            return Err(DependencyError::DepthLimit {
                depot: depot_id,
                limit: self.max_depth,
            });
        }
        if !self.seen.insert(depot_id) {
            return Ok(());
        }

        debug!(
            depot = %depot_id,
            depth,
            %reason,
            "manifest: depot added to dependency closure"
        );
        self.resolved.push(ResolvedDepot {
            depot_id,
            depth,
            reason,
        });

        let mut dependencies =
            self.source
                .dependencies(depot_id)
                .map_err(|reason| DependencyError::Lookup {
                    depot: depot_id,
                    reason,
                })?;
        dependencies.sort_unstable_by_key(|dependency| dependency.depot_id);
        dependencies.dedup_by_key(|dependency| dependency.depot_id);

        self.path.push(depot_id);
        for dependency in dependencies {
            self.visit(
                dependency.depot_id,
                DepotReason::Dependency {
                    declared_by: depot_id,
                    kind: dependency.kind,
                },
                depth + 1,
            )?;
        }
        self.path.pop();
        Ok(())
    }
}
