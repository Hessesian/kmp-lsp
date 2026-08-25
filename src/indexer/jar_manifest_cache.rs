//! Tier-1 lightweight JAR manifest cache: name+kind+container only, no
//! detail/params/doc. This is the always-eager, cheap-by-construction store
//! `build_jar_manifest` (Task 6) reads and writes — kept entirely separate
//! from `jar_cache.rs`'s full `SidecarSymbol` cache so that decoding it never
//! requires materializing the full per-JAR symbol data. See design §Tier 1.
//!
//! Global (not per-workspace) file, same convention as `jar_cache.rs`:
//! `~/.cache/kmp-lsp/jar-manifest-v{VERSION}.bin`.
//!
//! zstd-compressed (level 3) — pattern borrowed directly from the
//! `qdsfdhvh/kotlin-lsp` community fork's PR #107, which validated this exact
//! wrapping (`zstd::encode_all`/`decode_all` around a bincode blob) for cache
//! files in a real deployment. That fork's compression targets a different
//! problem (CLI one-shot I/O latency, not server steady-state RAM) and must
//! NOT be applied to `jar_cache.rs`'s full symbol cache — encode_all/decode_all
//! materialize a full extra buffer, which would reintroduce exactly the
//! transient-peak problem this manifest cache exists to avoid for the FULL
//! cache. It's safe here specifically because the manifest is small by
//! construction (name+kind+container only).

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Bump when `JarManifestEntry`/`JarManifestName` schema changes, or when
/// the sidecar's extraction logic changes what the manifest SHOULD contain
/// for an unchanged JAR (the JAR's own (mtime, size) fingerprint can't
/// detect that on its own).
/// v2 → v3: sidecar's `JavaClassVisitor` now emits public field symbols
///          (`val`/`var`) in addition to classes/methods.
const JAR_MANIFEST_CACHE_VERSION: u32 = 3;

#[derive(Serialize, Deserialize)]
struct JarManifestCache {
    version: u32,
    entries: HashMap<String, JarManifestEntry>,
}

/// Borrow-only view of the cache used for serialization — avoids cloning the
/// entire entries map when writing to disk.
#[derive(Serialize)]
struct JarManifestCacheRef<'a> {
    version: u32,
    entries: &'a HashMap<String, JarManifestEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct JarManifestEntry {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub file_size: u64,
    pub names: Vec<JarManifestName>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct JarManifestName {
    pub name: String,
    pub kind: String,
    pub container: Option<String>,
    /// The declaring class's fully-qualified package, e.g.
    /// "androidx.compose.runtime". `None` for the default package or when
    /// the sidecar didn't emit one. Mirrors `SidecarSymbol::pkg` — this is
    /// what lets Task 6 build real FQNs into `jar_qualified` without ever
    /// touching the full per-JAR symbol cache.
    pub package: Option<String>,
    /// The extension's receiver leaf type name (generics stripped), e.g.
    /// "ViewModel" for `val ViewModel.viewModelScope: CoroutineScope`.
    /// `None` for non-extension symbols. Mirrors `SidecarSymbol::
    /// extension_receiver_type` (leaf-stripped the same way Tier 2's
    /// `build_jar_file_data` derives its `extension_by_receiver` key) — this
    /// is what lets Tier 1 know a JAR defines an extension on a given
    /// receiver type WITHOUT materializing it, closing the gap where
    /// extension completion (e.g. `viewModelScope`) silently disappeared for
    /// any not-yet-materialized JAR.
    pub extension_receiver: Option<String>,
}

fn cache_path() -> std::path::PathBuf {
    super::cache::xdg_cache_base()
        .join("kmp-lsp")
        .join(format!("jar-manifest-v{JAR_MANIFEST_CACHE_VERSION}.bin"))
}

#[cfg(test)]
pub(crate) fn manifest_cache_path_for_test() -> std::path::PathBuf {
    cache_path()
}

/// Test-only: write a JAR manifest cache file stamped with an arbitrary
/// `version`, at the path that version's `cache_path()` would resolve to —
/// simulating a manifest cache an older kmp-lsp build left on disk before a
/// schema change bumped `JAR_MANIFEST_CACHE_VERSION`.
#[cfg(test)]
pub(crate) fn write_versioned_manifest_cache_for_test(
    version: u32,
    entries: &HashMap<String, JarManifestEntry>,
) {
    let path = super::cache::xdg_cache_base()
        .join("kmp-lsp")
        .join(format!("jar-manifest-v{version}.bin"));
    std::fs::create_dir_all(path.parent().expect("cache path has a parent")).unwrap();
    let cache = JarManifestCacheRef { version, entries };
    let bytes = bincode::serialize(&cache).expect("serialize stale manifest cache for test");
    let compressed =
        zstd::encode_all(bytes.as_slice(), 3).expect("zstd-encode stale cache for test");
    std::fs::write(&path, compressed).expect("write stale manifest cache for test");
}

/// Load the global JAR manifest cache. Returns an empty map on any error
/// (absent file, corrupt zstd frame, corrupt bincode, version mismatch) —
/// callers treat "empty" as "nothing cached yet, build fresh."
pub(crate) fn load_jar_manifest_cache() -> HashMap<String, JarManifestEntry> {
    let path = cache_path();
    let Ok(compressed) = std::fs::read(&path) else {
        return HashMap::new();
    };
    let Ok(bytes) = zstd::decode_all(compressed.as_slice()) else {
        log::debug!("jar_manifest_cache: zstd decode failed, starting fresh");
        return HashMap::new();
    };
    match bincode::deserialize::<JarManifestCache>(&bytes) {
        Ok(c) if c.version == JAR_MANIFEST_CACHE_VERSION => {
            log::debug!("jar_manifest_cache: loaded {} entries", c.entries.len());
            c.entries
        }
        _ => {
            log::debug!("jar_manifest_cache: version mismatch or corrupt, starting fresh");
            HashMap::new()
        }
    }
}

/// Save the global JAR manifest cache atomically (write temp → rename),
/// zstd-compressed.
pub(crate) fn save_jar_manifest_cache(entries: &HashMap<String, JarManifestEntry>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("jar_manifest_cache: cannot create cache dir: {e}");
            return;
        }
    }
    let cache = JarManifestCacheRef {
        version: JAR_MANIFEST_CACHE_VERSION,
        entries,
    };
    let bytes = match bincode::serialize(&cache) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("jar_manifest_cache: serialize error: {e}");
            return;
        }
    };
    let compressed = match zstd::encode_all(bytes.as_slice(), 3) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("jar_manifest_cache: zstd encode error: {e}");
            return;
        }
    };
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &compressed) {
        log::warn!("jar_manifest_cache: write temp error: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("jar_manifest_cache: rename error: {e}");
        let _ = std::fs::remove_file(&tmp);
    } else {
        log::debug!(
            "jar_manifest_cache: saved {} entries ({} bytes raw -> {} bytes zstd)",
            entries.len(),
            bytes.len(),
            compressed.len()
        );
    }
}

/// Check whether the manifest entry for `jar` is still valid — identical
/// `(mtime, size)` freshness check to `jar_cache.rs::cache_entry_is_fresh`.
pub(crate) fn manifest_entry_is_fresh(entry: &JarManifestEntry, jar: &Path) -> bool {
    let meta = match std::fs::metadata(jar) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.len() != entry.file_size {
        return false;
    }
    let mtime = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let duration = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    duration.as_secs() == entry.mtime_secs && duration.subsec_nanos() == entry.mtime_nanos
}
