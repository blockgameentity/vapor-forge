//! Tests for dependency-aware depot resolution.

use super::*;
use std::collections::HashMap;
use vapor_forge_config::DepotId;

/// Host stand-in: a fixed dependency table, in whatever order it is built.
struct Table(HashMap<u32, Vec<DepotDependency>>);

impl Table {
    fn new(entries: &[(u32, &[(u32, DependencyKind)])]) -> Self {
        Self(
            entries
                .iter()
                .map(|(depot, dependencies)| {
                    let dependencies = dependencies
                        .iter()
                        .map(|(dep, kind)| DepotDependency::new(DepotId(*dep), *kind))
                        .collect();
                    (*depot, dependencies)
                })
                .collect(),
        )
    }
}

impl DepotDependencySource for Table {
    fn dependencies(&self, depot_id: DepotId) -> Result<Vec<DepotDependency>, String> {
        Ok(self.0.get(&depot_id.0).cloned().unwrap_or_default())
    }
}

/// A source that cannot answer, standing in for missing appinfo.
struct Broken;

impl DepotDependencySource for Broken {
    fn dependencies(&self, _depot_id: DepotId) -> Result<Vec<DepotDependency>, String> {
        Err("appinfo has no depot metadata".to_owned())
    }
}

/// `(depot, depth, reason)` triples, so an assertion reads as the closure.
fn closure(
    requested: &[u32],
    table: &Table,
    max_depth: usize,
) -> Result<Vec<(u32, usize, String)>, DependencyError> {
    let requested: Vec<DepotId> = requested.iter().copied().map(DepotId).collect();
    resolve_depot_closure(&requested, table, max_depth).map(|resolved| {
        resolved
            .into_iter()
            .map(|depot| (depot.depot_id.0, depot.depth, depot.reason.to_string()))
            .collect()
    })
}

#[test]
fn closure_follows_dependencies_deterministically() {
    let table = Table::new(&[
        (10, &[(12, DependencyKind::Base), (11, DependencyKind::Dlc)]),
        (11, &[(13, DependencyKind::Build)]),
    ]);

    // Visited in depot-id order, so the host's own ordering is irrelevant.
    let resolved = closure(&[10], &table, DEFAULT_MAX_DEPENDENCY_DEPTH).unwrap();
    assert_eq!(
        resolved,
        vec![
            (10, 0, "required by the package".to_owned()),
            (11, 1, "dlc dependency of depot 10".to_owned()),
            (13, 2, "build dependency of depot 11".to_owned()),
            (12, 1, "base dependency of depot 10".to_owned()),
        ]
    );

    // Same graph, dependencies declared in the opposite order.
    let reversed = Table::new(&[
        (10, &[(11, DependencyKind::Dlc), (12, DependencyKind::Base)]),
        (11, &[(13, DependencyKind::Build)]),
    ]);
    assert_eq!(resolved, closure(&[10], &reversed, 8).unwrap());
}

#[test]
fn a_diamond_adds_the_shared_depot_once() {
    let table = Table::new(&[
        (
            10,
            &[(11, DependencyKind::Build), (12, DependencyKind::Build)],
        ),
        (11, &[(13, DependencyKind::Base)]),
        (12, &[(13, DependencyKind::Base)]),
    ]);

    let resolved = closure(&[10], &table, 8).unwrap();
    let ids: Vec<u32> = resolved.iter().map(|(depot, _, _)| *depot).collect();
    assert_eq!(ids, [10, 11, 13, 12]);
    // The first route wins, and the entry says which one it was.
    assert_eq!(resolved[2].2, "base dependency of depot 11");
}

#[test]
fn a_cycle_is_reported_instead_of_looping() {
    let table = Table::new(&[
        (10, &[(11, DependencyKind::Build)]),
        (11, &[(10, DependencyKind::Build)]),
    ]);
    let error = closure(&[10], &table, 8).expect_err("a cycle must be an error");
    assert_eq!(
        error,
        DependencyError::Cycle(vec![DepotId(10), DepotId(11), DepotId(10)])
    );
    assert!(error.to_string().contains("10 -> 11 -> 10"));
}

#[test]
fn a_self_dependency_is_a_cycle() {
    let table = Table::new(&[(10, &[(10, DependencyKind::Base)])]);
    assert_eq!(
        closure(&[10], &table, 8),
        Err(DependencyError::Cycle(vec![DepotId(10), DepotId(10)]))
    );
}

#[test]
fn depth_limit_is_enforced() {
    let table = Table::new(&[
        (10, &[(11, DependencyKind::Build)]),
        (11, &[(12, DependencyKind::Build)]),
        (12, &[(13, DependencyKind::Build)]),
    ]);

    // Depth 0..=3 reaches the end of the chain; two hops do not, because the
    // dependency of depot 12 sits at depth 3.
    assert_eq!(closure(&[10], &table, 3).unwrap().len(), 4);
    let error = closure(&[10], &table, 2).expect_err("depth 2 must not reach depot 13");
    assert_eq!(
        error,
        DependencyError::DepthLimit {
            depot: DepotId(13),
            limit: 2
        }
    );
    assert!(error.to_string().contains("deeper than the limit of 2"));
}

#[test]
fn a_lookup_failure_names_the_depot() {
    let error = resolve_depot_closure(&[DepotId(10)], &Broken, 8)
        .expect_err("an unanswerable source must not produce a partial closure");
    assert_eq!(
        error,
        DependencyError::Lookup {
            depot: DepotId(10),
            reason: "appinfo has no depot metadata".to_owned()
        }
    );
    assert!(error.to_string().contains("depot 10"));
}
