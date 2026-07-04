//! Lazy-loading sources-JAR symbol cache.
//!
//! Design: instead of loading ALL cached symbols into the main `definitions` index
//! (which makes the resolver slow with 800K+ entries), we store them in a separate
//! cache and only materialize them into the index on demand — when the user hovers
//! over or navigates to a symbol that's imported from a sources JAR.
//!
//! Flow:
//! 1. On startup, load the cache metadata (JAR paths + mtime) but NOT the symbols.
//! 2. When resolving a symbol, check if any import points to a cached JAR.
//! 3. On first access, load that JAR's symbols from disk into a temporary index.
//! 4. The temporary index is consulted during resolution but doesn't pollute `definitions`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};

/// Bump when the on-disk format changes.
const CACHE_VERSION: u32 = 2;

// ─── On-disk format ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct CacheDisk {
    version: u32,
    entries: HashMap<String, JarCacheEntry>,
}

#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct JarCacheEntry {
    mtime_secs: u64,
    mtime_nanos: u32,
    file_size: u64,
    package: Option<String>,
    /// All public+internal symbols from this JAR.
    symbols: Vec<SymbolEntry>,
}

// ─── In-memory cache ──────────────────────────────────────────────────────────

/// Per-JAR symbol data, loaded on demand.
pub(crate) struct LoadedJar {
    pub file_data: Arc<FileData>,
    pub fake_uri: String,
}

/// The lazy-loading sources cache.
pub(crate) struct SourcesCache {
    /// JAR path → cached entry (metadata + symbols on disk).
    entries: HashMap<String, JarCacheEntry>,
    /// JAR path → loaded symbols (materialized into index on first access).
    loaded: HashMap<String, LoadedJar>,
}

impl SourcesCache {
    pub fn new(entries: HashMap<String, JarCacheEntry>) -> Self {
        Self {
            entries,
            loaded: HashMap::new(),
        }
    }

    /// Check if a JAR is cached and fresh.
    pub fn is_cached(&self, jar_path: &Path) -> bool {
        let key = jar_path.to_string_lossy().to_string();
        self.entries.contains_key(&key)
    }

    /// Load a JAR's symbols into the index on first access.
    /// Returns true if the JAR was loaded, false if not cached.
    pub fn load_jar(
        &mut self,
        jar_path: &Path,
        indexer: &crate::indexer::Indexer,
    ) -> bool {
        let key = jar_path.to_string_lossy().to_string();

        // Already loaded?
        if self.loaded.contains_key(&key) {
            return true;
        }

        // Not cached?
        let entry = match self.entries.get(&key) {
            Some(e) => e.clone(),
            None => return false,
        };

        let fake_uri = format!("jar:file://{}", jar_path.display());

        // Create a FileData with the cached symbols.
        let file_data = Arc::new(FileData {
            symbols: entry.symbols.clone(),
            source_set: SourceSet::Library,
            package: entry.package.clone(),
            ..Default::default()
        });

        // Insert into the main index so the resolver can find these symbols.
        indexer.files.insert(fake_uri.clone(), file_data.clone());

        // Insert symbols into definitions index.
        for sym in &entry.symbols {
            indexer
                .definitions
                .entry(sym.name.clone())
                .or_default()
                .push(tower_lsp::lsp_types::Location {
                    uri: tower_lsp::lsp_types::Url::parse(&fake_uri).unwrap_or_else(|_| {
                        tower_lsp::lsp_types::Url::parse("jar:file:///unknown").unwrap()
                    }),
                    range: sym.selection_range,
                });
        }

        // Insert into qualified index.
        if let Some(ref pkg) = entry.package {
            for sym in &entry.symbols {
                let fqn = match &sym.container {
                    Some(container) if !container.is_empty() => {
                        format!("{}.{}.{}", pkg, container, sym.name)
                    }
                    _ => format!("{}.{}", pkg, sym.name),
                };
                indexer.qualified.insert(
                    fqn,
                    tower_lsp::lsp_types::Location {
                        uri: tower_lsp::lsp_types::Url::parse(&fake_uri).unwrap_or_else(|_| {
                            tower_lsp::lsp_types::Url::parse("jar:file:///unknown").unwrap()
                        }),
                        range: sym.selection_range,
                    },
                );
            }
        }

        self.loaded.insert(
            key,
            LoadedJar {
                file_data,
                fake_uri,
            },
        );

        log::info!(
            "sources_cache: loaded {} symbols from {}",
            entry.symbols.len(),
            jar_path.display()
        );

        true
    }

    /// Save newly parsed JAR symbols to the cache.
    pub fn save_jar(
        &mut self,
        jar_path: &Path,
        package: Option<String>,
        symbols: Vec<SymbolEntry>,
    ) {
        let key = jar_path.to_string_lossy().to_string();
        let meta = match std::fs::metadata(jar_path) {
            Ok(m) => m,
            Err(_) => return,
        };
        let mtime = match meta.modified() {
            Ok(t) => t,
            Err(_) => return,
        };
        let duration = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();

        self.entries.insert(
            key,
            JarCacheEntry {
                mtime_secs: duration.as_secs(),
                mtime_nanos: duration.subsec_nanos(),
                file_size: meta.len(),
                package,
                symbols,
            },
        );
    }

    /// Check if a JAR's cache entry is still fresh.
    pub fn is_fresh(&self, jar_path: &Path) -> bool {
        let key = jar_path.to_string_lossy().to_string();
        let entry = match self.entries.get(&key) {
            Some(e) => e,
            None => return false,
        };
        let meta = match std::fs::metadata(jar_path) {
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
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        duration.as_secs() == entry.mtime_secs && duration.subsec_nanos() == entry.mtime_nanos
    }

    /// Get all cached JAR paths.
    pub fn cached_paths(&self) -> Vec<PathBuf> {
        self.entries
            .keys()
            .filter_map(|k| PathBuf::from(k).into())
            .collect()
    }
}

// ─── I/O ──────────────────────────────────────────────────────────────────────

use std::time::SystemTime;

pub fn load_cache() -> HashMap<String, JarCacheEntry> {
    let path = cache_path();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            log::info!("sources_cache: no cache file found, starting fresh");
            return HashMap::new();
        }
    };
    match bincode::deserialize::<CacheDisk>(&bytes) {
        Ok(disk) if disk.version == CACHE_VERSION => {
            log::info!("sources_cache: loaded {} JARs from disk", disk.entries.len());
            disk.entries
        }
        _ => {
            log::info!("sources_cache: version mismatch or corrupt, starting fresh");
            HashMap::new()
        }
    }
}

pub fn save_cache(entries: &HashMap<String, JarCacheEntry>) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("sources_cache: cannot create cache dir: {e}");
            return;
        }
    }
    let disk = CacheDisk {
        version: CACHE_VERSION,
        entries: entries.clone(),
    };
    let bytes = match bincode::serialize(&disk) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("sources_cache: serialize error: {e}");
            return;
        }
    };
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &bytes) {
        log::warn!("sources_cache: write temp error: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        log::warn!("sources_cache: rename error: {e}");
        let _ = std::fs::remove_file(&tmp);
    } else {
        log::info!("sources_cache: saved {} JARs to disk", entries.len());
    }
}

fn cache_path() -> PathBuf {
    crate::indexer::cache::xdg_cache_base()
        .join("kmp-lsp")
        .join(format!("sources-v{CACHE_VERSION}.bin"))
}
