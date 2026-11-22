//! Both directions of the depotcache consultation.
//!
//! `features::depotcache` tests its own decision table (a miss is not an error, a
//! bad body is never written, a transient failure writes nothing); these tests
//! prove the Linux host reaches that table and that an unconfigured host gets
//! nothing at all.
use super::*;

fn candidate(depot: u32, manifest: u64) -> Candidate {
    Candidate {
        depot_id: DepotId(depot),
        manifest_id: ManifestId(manifest),
    }
}

fn configured_config() -> RuntimeConfig {
    let mut config = RuntimeConfig::default();
    config
        .manifest
        .providers
        .push(vapor_forge_config::ManifestSource {
            name: "test".to_owned(),
            url: "http://127.0.0.1/{gid}".to_owned(),
            user_agent: None,
            format: Default::default(),
            json_field: None,
            min_interval_ms: None,
        });
    config
}

#[test]
fn an_unconfigured_host_consults_nothing() {
    let config = RuntimeConfig::default();
    assert!(!configured(&config), "providers are empty by default");
    assert!(consult(&config, DepotId(731), Some(candidate(731, 9))).is_none());
    assert!(consult(&config, DepotId(731), None).is_none());
}

#[test]
fn a_configured_host_with_no_wanted_manifest_consults_nothing() {
    // No `setManifestid` override names a gid, so there is no candidate: the
    // host never guesses one, and the call reports nothing.
    assert!(consult(&configured_config(), DepotId(731), None).is_none());
}

#[test]
fn a_cached_manifest_satisfies_the_install_and_a_miss_falls_through() {
    let root = std::env::temp_dir().join("vf-hooks-depot-preseed");
    let _ = std::fs::remove_dir_all(&root);
    let mut store = SteamDepotCache::new(root.clone());
    let wanted = candidate(731, 9);
    assert!(!store.is_cached(wanted));
    store.write(wanted, b"manifest-bytes").expect("write");
    assert!(store.is_cached(wanted));
    // The miss case is the one that must stay a fall-through.
    let mut fetcher = NoArchiveClient;
    let outcome = depotcache::preseed(
        &mut store,
        &mut fetcher,
        candidate(731, 10),
        depotcache::plausible_manifest,
    );
    assert_eq!(outcome, PreseedOutcome::NotArchived);
    assert!(!outcome.satisfies_install());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_archive_side_reports_not_archived_and_never_supplies_a_body() {
    let mut fetcher = NoArchiveClient;
    assert_eq!(fetcher.fetch_manifest(candidate(1, 2)), Ok(None));
}

#[test]
fn a_candidate_can_only_name_a_file_inside_the_cache_directory() {
    // Both halves of the file name are integers, so no candidate can walk out.
    let store = SteamDepotCache::new(PathBuf::from("/steam/depotcache"));
    assert_eq!(
        store.path_of(candidate(731, 9)),
        PathBuf::from("/steam/depotcache/731_9.manifest")
    );
    assert!(!store.is_cached(candidate(731, 9)), "nothing is there yet");
}
