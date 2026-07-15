//! Disk cache for JAR/AAR symbol data produced by the sidecar.
//!
//! JARs in the Gradle cache are immutable after download, so caching their
//! symbol data by `(path, mtime_secs, mtime_nanos, file_size)` is safe.
//!
//! Cache layout: one global file at
//! `~/.cache/kmp-lsp/jar-symbols-v{VERSION}.bin`.
//! It is a bincode-serialized `HashMap<String, JarCacheEntry>` keyed by the
//! JAR's absolute path string.  Entries for JARs not present in the current
//! workspace are retained so other workspaces can benefit.
//!
//! Writers use an atomic rename (write temp → rename) to avoid corruption.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::sidecar::SidecarSymbol;

/// Bump when `JarCacheEntry` schema changes.
/// v3 → v4: SidecarSymbol gained `trailing_lambda: bool` (bincode 1.x is positional, no serde(default)).
/// v4 → v5: SidecarSymbol gained `doc: String` inserted before `type_params` — positional mismatch.
/// v5 → v6: SidecarSymbol gained `deprecated: bool` after `trailing_lambda` — positional mismatch.
/// v6 → v7: JAR `SymbolEntry`s now carry real `(required, total)` param counts parsed
///          from the sidecar signature `detail` (`params_from_detail`); v6 stored `(0, 0)`.
/// v7 → v8: `SidecarSymbol` gained `pkg` + `top_level` (real per-symbol package);
///          positional bincode layout changed.
/// v8 → v9: sidecar now picks the real `-sources.jar` over `-samples-sources.jar`,
///          so JAR symbols (e.g. compose `stringResource`) carry real KDoc — force re-scan.
/// v9 → v10: sidecar KDoc regex now handles generic functions (`fun <T> remember`) — re-scan.
/// v10 → v11: sidecar KDoc regex now skips multi-line annotations (`@Target(...)` before
///            `annotation class Composable`) — re-scan.
/// v11 → v12: sidecar strips non-KDoc comments before matching, so comments containing `)`
///            inside a multi-line annotation no longer hide the declaration (`@Composable`).
/// v12 → v13: sidecar now emits each class's direct super types (`supers`), populating
///            jar `FileData.supers` so inheritance can be walked through library types.
const JAR_CACHE_VERSION: u32 = 13;

#[derive(Serialize, Deserialize)]
struct JarCache {
    version: u32,
    entries: HashMap<String, JarCacheEntry>,
}

/// Borrow-only view of the cache used for serialization — avoids cloning the
/// entire entries map when writing to disk.
#[derive(Serialize)]
struct JarCacheRef<'a> {
    version: u32,
    entries: &'a HashMap<String, JarCacheEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct JarCacheEntry {
    pub mtime_secs: u64,
    pub mtime_nanos: u32,
    pub file_size: u64,
    pub symbols: Vec<SidecarSymbol>,
}

fn cache_path() -> std::path::PathBuf {
    super::cache::xdg_cache_base()
        .join("kmp-lsp")
        .join(format!("jar-symbols-v{JAR_CACHE_VERSION}.bin"))
}

// Test-only instrumentation: counts, per test thread, how many times
// `load_jar_cache` has actually opened and deserialized the on-disk cache
// file. Used to prove that `Indexer::jar_symbol_cache` (see `index_jars` in
// `jar.rs`) memoizes the decoded map — decoding at most once per `Indexer`,
// not once per `index_jars` call.
//
// `thread_local!`, not a process-wide `static`: `cargo test` runs tests
// concurrently across a fixed-size thread pool, so a shared static counter
// would pick up increments from unrelated tests running on other threads at
// the same time. A thread-local only ever reflects calls made by code this
// specific test invoked on its own thread, so a before/after delta within
// one test function is race-free regardless of what other tests are doing.
#[cfg(test)]
thread_local! {
    pub(crate) static LOAD_JAR_CACHE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `(mtime_secs, mtime_nanos, file_size)` of the cache file as of this
/// process's last load or save, keyed by path (tests isolate
/// `XDG_CACHE_HOME` per test, so several paths coexist in one test binary).
/// Lets `save_jar_cache` skip its merge reload when nobody else has written
/// the file since we last saw it — the overwhelmingly common case. A small
/// linear-scan Vec (const-constructible in a `static`) rather than a map:
/// one entry per distinct cache path, which is 1 in production.
static CACHE_FILE_FINGERPRINTS: std::sync::Mutex<Vec<(std::path::PathBuf, CacheFileFingerprint)>> =
    std::sync::Mutex::new(Vec::new());

/// `(mtime_secs, mtime_nanos, file_size)` of a cache file at observation time.
type CacheFileFingerprint = (u64, u32, u64);

fn file_fingerprint(path: &Path) -> Option<CacheFileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    let duration = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Some((duration.as_secs(), duration.subsec_nanos(), meta.len()))
}

fn record_cache_file_fingerprint(path: &Path) {
    let fingerprint = file_fingerprint(path);
    let mut recorded = CACHE_FILE_FINGERPRINTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match (
        recorded.iter_mut().find(|(known, _)| known == path),
        fingerprint,
    ) {
        (Some(slot), Some(fingerprint)) => slot.1 = fingerprint,
        (Some(_), None) => recorded.retain(|(known, _)| known != path),
        (None, Some(fingerprint)) => recorded.push((path.to_owned(), fingerprint)),
        (None, None) => {}
    }
}

fn cache_file_changed_since_last_seen(path: &Path) -> bool {
    let current = file_fingerprint(path);
    let recorded = CACHE_FILE_FINGERPRINTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let known = recorded
        .iter()
        .find(|(known, _)| known == path)
        .map(|(_, fingerprint)| *fingerprint);
    current != known
}

/// Load the global JAR symbol cache.  Returns an empty map on any error.
pub(crate) fn load_jar_cache() -> HashMap<String, JarCacheEntry> {
    #[cfg(test)]
    LOAD_JAR_CACHE_CALLS.with(|count| count.set(count.get() + 1));
    let path = cache_path();
    record_cache_file_fingerprint(&path);
    // Stream-deserialize rather than std::fs::read + deserialize-from-buffer:
    // the latter holds the full raw file AND the deserialized map in memory
    // simultaneously — the same transient-peak shape the workspace apply path
    // was fixed to avoid. `load_sources_jar_cache` already streams via
    // BufReader; this brings the sibling loader in line.
    let Ok(file) = std::fs::File::open(&path) else {
        return HashMap::new();
    };
    let reader = std::io::BufReader::new(file);
    match bincode::deserialize_from::<_, JarCache>(reader) {
        Ok(c) if c.version == JAR_CACHE_VERSION => {
            log::debug!("jar_cache: loaded {} entries", c.entries.len());
            c.entries
        }
        _ => {
            log::debug!("jar_cache: version mismatch or corrupt, starting fresh");
            HashMap::new()
        }
    }
}

/// Save the global JAR symbol cache atomically (write temp → rename).
///
/// MERGES with the current on-disk state rather than overwriting it: the
/// file is shared by every kmp-lsp process on the machine (editor sessions,
/// CLI runs), and a plain whole-map write is last-writer-wins — a process
/// whose in-memory map is missing entries another process saved would
/// silently erase them. That happened in production: ~70MB of entries
/// vanished, and every on-demand promotion for a wiped JAR then missed the
/// cache and paid a real sidecar round trip (observed as an avalanche of
/// sequential one-JAR materializations and a 22s inlay stall). Union
/// semantics — our entries win conflicts, on-disk entries we never loaded
/// survive — make the cache grow monotonically. The extra load is
/// acceptable: saves happen only on cache-miss materializations (rare once
/// warm) and at crawl end.
pub(crate) fn save_jar_cache(entries: &HashMap<String, JarCacheEntry>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("jar_cache: cannot create cache dir: {e}");
            return;
        }
    }
    // The merge reload is only needed when some OTHER process wrote the file
    // since we last loaded/saved it. A full decode of a multi-hundred-MB
    // cache roughly doubles per-save cost (decode ≈ serialize+write), so the
    // common single-writer save skips it via the fingerprint check. A write
    // landing between this check and our rename can still be lost — the same
    // TOCTOU as any non-locked file protocol — but the window is the write
    // itself, not (as before this fix) every save unconditionally.
    let merged_storage;
    let entries_to_write: &HashMap<String, JarCacheEntry> =
        if cache_file_changed_since_last_seen(&path) {
            let mut on_disk = load_jar_cache();
            for (jar_path, entry) in entries {
                on_disk.insert(jar_path.clone(), entry.clone());
            }
            merged_storage = on_disk;
            &merged_storage
        } else {
            entries
        };
    let cache = JarCacheRef {
        version: JAR_CACHE_VERSION,
        entries: entries_to_write,
    };
    let bytes = match bincode::serialize(&cache) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("jar_cache: serialize error: {e}");
            return;
        }
    };
    // Write to a unique temp file then rename for atomicity.
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        log::warn!("jar_cache: write temp error: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("jar_cache: rename error: {e}");
        let _ = std::fs::remove_file(&tmp);
    } else {
        record_cache_file_fingerprint(&path);
        log::debug!("jar_cache: saved {} entries", entries.len());
    }
}

/// Test-only: forget this process's recorded fingerprint for the current
/// cache path — simulates a FRESH process (whose fingerprint table is empty)
/// saving to a file another process already wrote, which is the scenario the
/// merge-on-save exists for.
#[cfg(test)]
pub(crate) fn forget_cache_file_fingerprint_for_test() {
    let path = cache_path();
    CACHE_FILE_FINGERPRINTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|(known, _)| known != &path);
}

/// Check whether the cache entry for `jar` is still valid.
pub(crate) fn cache_entry_is_fresh(entry: &JarCacheEntry, jar: &Path) -> bool {
    let meta = match std::fs::metadata(jar) {
        Ok(m) => m,
        Err(_) => return false,
    };
    let file_size = meta.len();
    if file_size != entry.file_size {
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

/// Build a new cache entry for a JAR from its sidecar symbols.
pub(crate) fn make_cache_entry(jar: &Path, symbols: Vec<SidecarSymbol>) -> Option<JarCacheEntry> {
    let meta = std::fs::metadata(jar).ok()?;
    let mtime = meta.modified().ok()?;
    let duration = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Some(JarCacheEntry {
        mtime_secs: duration.as_secs(),
        mtime_nanos: duration.subsec_nanos(),
        file_size: meta.len(),
        symbols,
    })
}
