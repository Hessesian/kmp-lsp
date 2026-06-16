//! Disk cache for sources-JAR parse results.
//!
//! Mirrors `jar_cache.rs` (compiled-JAR sidecar cache) but stores tree-sitter
//! parse output per source file: `(uri, content_hash, FileData)`.  Keeping
//! per-file granularity (instead of flattening symbols per JAR) preserves
//! go-to-definition into individual `jar:file://…!/path/File.kt` entries and
//! lets cache hits reuse the exact same apply path as fresh parses.
//!
//! JARs in the Gradle cache are immutable after download, so `(mtime, size)`
//! fingerprinting is safe.  Entries whose JAR no longer exists on disk are
//! pruned (`prune_deleted_jars`) — unlike the compiled-JAR cache, entries here
//! hold full source text, so unbounded growth would reach GBs.
//!
//! Writers use an atomic rename (write temp → rename) to avoid corruption.
//! Concurrent kmp-lsp processes race load→modify→save; the last writer wins
//! and the loser re-parses on its next start.  Accepted trade-off (same as
//! `jar_cache.rs`) — no lock file.

// Wired into `index_sources_jars` by the parse-cache integration task; until
// then only tests reference this module.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::types::FileData;

/// Bump when the on-disk schema of THIS module changes.  Changes to `FileData`
/// or `SymbolEntry` are covered automatically: the cache filename embeds
/// `cache::CACHE_VERSION`, which the project rule (mem:core) already bumps on
/// every such change — bincode 1.x is positional and can silently mis-decode
/// reordered same-shaped fields, so filename coupling is load-bearing.
const SOURCES_JAR_CACHE_VERSION: u32 = 2;

#[derive(Deserialize)]
struct SourcesJarCacheDisk {
    version: u32,
    entries: HashMap<String, SourcesJarEntry>,
}

/// Borrow-only view used for serialization — avoids cloning the entries map.
#[derive(Serialize)]
struct SourcesJarCacheDiskRef<'a> {
    version: u32,
    entries: &'a HashMap<String, SourcesJarEntry>,
}

/// Cached parse results for one sources JAR.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct SourcesJarEntry {
    pub(crate) mtime_secs: u64,
    pub(crate) mtime_nanos: u32,
    pub(crate) file_size: u64,
    /// One entry per successfully parsed `.kt`/`.java` file in the JAR.
    pub(crate) files: Vec<SourcesFileEntry>,
}

/// Cached parse result for one source file inside a JAR.
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct SourcesFileEntry {
    /// Full synthetic URI, e.g. `jar:file:///…/lib-sources.jar!/com/example/Core.kt`.
    pub(crate) uri: String,
    pub(crate) content_hash: u64,
    /// Stored with `source_set` already set to `Library` so the apply path
    /// never needs to clone-and-override.
    pub(crate) file_data: Arc<FileData>,
}

/// `(mtime, size)` identity of a JAR file, captured BEFORE extraction so a
/// concurrent JAR replacement cannot pair new metadata with old parse results.
pub(crate) struct JarFingerprint {
    pub(crate) mtime_secs: u64,
    pub(crate) mtime_nanos: u32,
    pub(crate) file_size: u64,
}

/// Read the current fingerprint of a JAR file. `None` if unreadable.
pub(crate) fn jar_fingerprint(jar_path: &Path) -> Option<JarFingerprint> {
    let metadata = std::fs::metadata(jar_path).ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Some(JarFingerprint {
        mtime_secs: duration.as_secs(),
        mtime_nanos: duration.subsec_nanos(),
        file_size: metadata.len(),
    })
}

/// Check whether a cache entry still matches the JAR's current fingerprint.
pub(crate) fn entry_is_fresh(entry: &SourcesJarEntry, current: &JarFingerprint) -> bool {
    entry.file_size == current.file_size
        && entry.mtime_secs == current.mtime_secs
        && entry.mtime_nanos == current.mtime_nanos
}

/// Drop entries whose JAR no longer exists on disk.  Returns true if anything
/// was removed.  Keeps cross-workspace sharing intact: live JARs in
/// `~/.gradle` exist regardless of which workspace is open.
pub(crate) fn prune_deleted_jars(entries: &mut HashMap<String, SourcesJarEntry>) -> bool {
    let count_before = entries.len();
    entries.retain(|jar_path, _| Path::new(jar_path).exists());
    entries.len() != count_before
}

/// Cache file location.  `cache_dir` overrides the default
/// `~/.cache/kmp-lsp/` base — used by tests for isolation.
fn cache_file_path(cache_dir: Option<&Path>) -> PathBuf {
    let base = match cache_dir {
        Some(dir) => dir.to_owned(),
        None => super::cache::xdg_cache_base().join("kmp-lsp"),
    };
    base.join(format!(
        "sources-jar-v{SOURCES_JAR_CACHE_VERSION}-c{}.bin",
        super::cache::CACHE_VERSION
    ))
}

/// Load the global sources-JAR parse cache.  Empty map on any error.
pub(crate) fn load_sources_jar_cache(cache_dir: Option<&Path>) -> HashMap<String, SourcesJarEntry> {
    let path = cache_file_path(cache_dir);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return HashMap::new(),
    };
    let reader = std::io::BufReader::new(file);
    match bincode::deserialize_from::<_, SourcesJarCacheDisk>(reader) {
        Ok(disk) if disk.version == SOURCES_JAR_CACHE_VERSION => {
            log::debug!(
                "sources_jar_cache: loaded {} JAR entries",
                disk.entries.len()
            );
            disk.entries
        }
        _ => {
            log::debug!("sources_jar_cache: version mismatch or corrupt, starting fresh");
            HashMap::new()
        }
    }
}

/// Save the cache atomically (write temp → rename), then delete stale
/// `sources-jar-*.bin` files from older versions (each can be hundreds of MB).
/// Streams via `BufWriter` — the payload is far larger than the compiled-JAR
/// cache.
pub(crate) fn save_sources_jar_cache(
    cache_dir: Option<&Path>,
    entries: &HashMap<String, SourcesJarEntry>,
) {
    let path = cache_file_path(cache_dir);
    if let Some(parent) = path.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            log::warn!("sources_jar_cache: cannot create cache dir: {error}");
            return;
        }
    }
    let temp_path = path.with_extension(format!("tmp.{}", std::process::id()));
    let file = match std::fs::File::create(&temp_path) {
        Ok(f) => f,
        Err(error) => {
            log::warn!("sources_jar_cache: create temp error: {error}");
            return;
        }
    };
    let writer = std::io::BufWriter::new(file);
    let disk = SourcesJarCacheDiskRef {
        version: SOURCES_JAR_CACHE_VERSION,
        entries,
    };
    if let Err(error) = bincode::serialize_into(writer, &disk) {
        log::warn!("sources_jar_cache: serialize error: {error}");
        let _ = std::fs::remove_file(&temp_path);
        return;
    }
    if let Err(error) = std::fs::rename(&temp_path, &path) {
        log::warn!("sources_jar_cache: rename error: {error}");
        let _ = std::fs::remove_file(&temp_path);
        return;
    }
    log::debug!("sources_jar_cache: saved {} JAR entries", entries.len());
    remove_stale_version_files(&path);
}

/// Delete sibling `sources-jar-*.bin` files that are not the current cache
/// file.  In-flight temp files (`.tmp.<pid>` extension) are not matched.
fn remove_stale_version_files(current: &Path) {
    let Some(parent) = current.parent() else {
        return;
    };
    let Ok(dir_entries) = std::fs::read_dir(parent) else {
        return;
    };
    for dir_entry in dir_entries.flatten() {
        let entry_path = dir_entry.path();
        if entry_path == current {
            continue;
        }
        let Some(name) = entry_path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with("sources-jar-") && name.ends_with(".bin") {
            let _ = std::fs::remove_file(&entry_path);
        }
    }
}
