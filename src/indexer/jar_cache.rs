//! Disk cache for JAR/AAR symbol data produced by the sidecar.
//!
//! JARs in the Gradle cache are immutable after download, so caching their
//! symbol data by `(path, mtime_secs, mtime_nanos, file_size)` is safe.
//!
//! Cache layout: one global file at
//! `~/.cache/kotlin-lsp/jar-symbols-v{VERSION}.bin`.
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
const JAR_CACHE_VERSION: u32 = 5;

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
        .join("kotlin-lsp")
        .join(format!("jar-symbols-v{JAR_CACHE_VERSION}.bin"))
}

/// Load the global JAR symbol cache.  Returns an empty map on any error.
pub(crate) fn load_jar_cache() -> HashMap<String, JarCacheEntry> {
    let path = cache_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => return HashMap::new(),
    };
    match bincode::deserialize::<JarCache>(&bytes) {
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
pub(crate) fn save_jar_cache(entries: &HashMap<String, JarCacheEntry>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("jar_cache: cannot create cache dir: {e}");
            return;
        }
    }
    let cache = JarCacheRef {
        version: JAR_CACHE_VERSION,
        entries,
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
        log::debug!("jar_cache: saved {} entries", entries.len());
    }
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
