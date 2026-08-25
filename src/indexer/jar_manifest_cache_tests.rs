//! Tests for the Tier-1 JAR manifest cache (disk roundtrip through zstd,
//! corrupt-file degradation).

use super::jar_manifest_cache::{
    load_jar_manifest_cache, save_jar_manifest_cache, JarManifestEntry, JarManifestName,
};
use crate::indexer::test_helpers::with_xdg_cache;
use std::collections::HashMap;

#[test]
fn jar_manifest_cache_round_trips_through_zstd() {
    let tmp = tempfile::tempdir().expect("tempdir");
    with_xdg_cache(tmp.path(), || {
        let mut entries: HashMap<String, JarManifestEntry> = HashMap::new();
        entries.insert(
            "/gradle/caches/compose-ui-1.6.0.jar".to_owned(),
            JarManifestEntry {
                mtime_secs: 1_700_000_000,
                mtime_nanos: 0,
                file_size: 12345,
                names: vec![
                    JarManifestName {
                        name: "Column".to_owned(),
                        kind: "fun".to_owned(),
                        container: None,
                        package: Some("androidx.compose.foundation.layout".to_owned()),
                        extension_receiver: None,
                    },
                    JarManifestName {
                        name: "Modifier".to_owned(),
                        kind: "class".to_owned(),
                        container: None,
                        package: Some("androidx.compose.ui".to_owned()),
                        extension_receiver: None,
                    },
                ],
            },
        );
        save_jar_manifest_cache(&entries);
        let loaded = load_jar_manifest_cache();
        assert_eq!(loaded.len(), 1, "round trip must preserve entry count");
        let entry = loaded
            .get("/gradle/caches/compose-ui-1.6.0.jar")
            .expect("saved entry must load back");
        assert_eq!(entry.names.len(), 2);
        assert_eq!(entry.names[0].name, "Column");
        assert_eq!(entry.names[1].kind, "class");
        assert_eq!(
            entry.names[0].package.as_deref(),
            Some("androidx.compose.foundation.layout"),
            "package must round-trip — it's how Task 6 builds real FQNs into jar_qualified"
        );
    });
}

#[test]
fn jar_manifest_cache_missing_file_returns_empty_map() {
    let tmp = tempfile::tempdir().expect("tempdir");
    with_xdg_cache(tmp.path(), || {
        // Fresh, isolated XDG_CACHE_HOME with no manifest cache file ever
        // written under it — the "absent file" branch of
        // `load_jar_manifest_cache`, actually exercised (not just asserted
        // to exist structurally by the corrupt-file test below, which is a
        // different failure mode: a present-but-unreadable file).
        let loaded = load_jar_manifest_cache();
        assert!(
            loaded.is_empty(),
            "loading with no cache file present must return an empty map, not panic"
        );
    });
}

#[test]
fn jar_manifest_cache_ignores_a_stale_pre_field_extraction_version() {
    // Pins the fix for the cache-invalidation gap in the Android-SDK-jar PR:
    // adding field-symbol extraction changed what a fully-materialized JAR's
    // data SHOULD contain, but neither the JAR's own (mtime, size)
    // fingerprint nor a bumped version constant would otherwise notice — a
    // user upgrading kmp-lsp would silently keep serving pre-upgrade,
    // field-less cached data forever. `2` is deliberately hardcoded (not
    // `JAR_MANIFEST_CACHE_VERSION - 1`): it is the exact version number this
    // manifest cache shipped with immediately before the fix, i.e. what a
    // real user's on-disk cache is actually stamped with today. If the
    // version constant were still 2, this stale file would sit at the exact
    // path the loader reads and would be served as valid.
    let tmp = tempfile::tempdir().expect("tempdir");
    with_xdg_cache(tmp.path(), || {
        let mut stale_entries: HashMap<String, JarManifestEntry> = HashMap::new();
        stale_entries.insert(
            "/gradle/caches/android.jar".to_owned(),
            JarManifestEntry {
                mtime_secs: 1_700_000_000,
                mtime_nanos: 0,
                file_size: 999,
                names: vec![JarManifestName {
                    name: "View".to_owned(),
                    kind: "class".to_owned(),
                    container: None,
                    package: Some("android.view".to_owned()),
                    extension_receiver: None,
                }],
            },
        );
        super::jar_manifest_cache::write_versioned_manifest_cache_for_test(2, &stale_entries);

        let loaded = load_jar_manifest_cache();
        assert!(
            loaded.is_empty(),
            "a manifest cache written by the pre-field-extraction version must be ignored, \
             not served stale — the version constant must have been bumped past 2"
        );
    });
}

#[test]
fn jar_manifest_cache_corrupt_file_returns_empty_map() {
    let tmp = tempfile::tempdir().expect("tempdir");
    with_xdg_cache(tmp.path(), || {
        // Write garbage bytes to the manifest cache path directly, then confirm
        // the loader degrades to an empty map instead of panicking.
        let path = super::jar_manifest_cache::manifest_cache_path_for_test();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not a valid zstd/bincode blob").unwrap();
        let loaded = load_jar_manifest_cache();
        assert!(
            loaded.is_empty(),
            "corrupt manifest cache must degrade to empty, not panic"
        );
    });
}
