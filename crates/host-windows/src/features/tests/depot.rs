use super::*;

#[test]
fn the_depotcache_batch_reports_every_candidate_and_never_aborts() {
    let _guard = serial();
    // Written, AlreadyPresent, NotArchived, Failed, Written: the failure sits in
    // the middle and the batch continues past it.
    let wanted = [
        manifest(10, 100),
        manifest(11, 101),
        manifest(12, 102),
        manifest(13, 103),
        manifest(14, 104),
    ];
    let mut store = FakeStore::with(wanted[1]);
    let mut fetcher = ScriptedFetcher::new(vec![
        Answer::Body,
        Answer::NotFound,
        Answer::Transient,
        Answer::Body,
    ]);
    let report = crate::features::preseed_wanted(&wanted, &mut store, &mut fetcher);

    assert_eq!(report.len(), 5, "one entry per candidate");
    assert_eq!(report.written(), 2);
    assert_eq!(report.already_present(), 1);
    assert_eq!(report.not_archived(), 1);
    assert_eq!(report.failed(), 1);
    assert_eq!(report.satisfied(), 3);
    assert_eq!(store.writes, 2, "already-present is not rewritten");

    let outcomes: Vec<String> = report
        .entries()
        .iter()
        .map(|entry| entry.outcome_label())
        .collect();
    assert!(outcomes[0].starts_with("written"));
    assert_eq!(outcomes[1], "already present");
    assert_eq!(outcomes[2], "not archived");
    assert!(outcomes[3].contains("archive timeout"));
    assert!(
        outcomes[4].starts_with("written"),
        "batch continued past the failure"
    );

    // The cached candidate is never fetched; the others are, in order.
    let fetched: Vec<String> = fetcher
        .calls
        .iter()
        .map(|candidate| candidate.file_name())
        .collect();
    assert_eq!(
        fetched,
        vec![
            wanted[0].file_name(),
            wanted[2].file_name(),
            wanted[3].file_name(),
            wanted[4].file_name()
        ]
    );
}

#[test]
fn the_depotcache_feature_runs_through_the_host_ports() {
    let _guard = serial();
    let wanted = vec![manifest(10, 100)];
    let mut store = FakeStore::default();
    let mut fetcher = ScriptedFetcher::new(vec![Answer::Body]);
    let settings = HostSettings {
        depotcache: crate::features::DepotCacheSettings {
            wanted: wanted.clone(),
        },
        ..HostSettings::none()
    };
    let report = run(
        &settings,
        &Gate::all_enabled(),
        Some(DepotCachePorts {
            store: &mut store,
            fetcher: &mut fetcher,
        }),
    );

    assert!(report.is_active(HostFeature::DepotCache));
    assert_eq!(report.depotcache.written(), 1);
    assert_eq!(store.writes, 1);
    assert_eq!(fetcher.calls.len(), 1);
    assert!(report.report().contains("written (13 bytes)"));
}
