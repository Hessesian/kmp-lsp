//! Gradle cache JAR/AAR scanning and sidecar-based symbol indexing.
//!
//! Scans `~/.gradle/caches/modules-2/files-2.1/` for non-sources JARs and AARs,
//! deduplicates by `(group, artifact, latest-version)`, and sends each file to
//! the `kotlin-jar-indexer` sidecar process to produce `SymbolEntry` items.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::{Range, SymbolKind};

use crate::cli::extract_sources::{default_gradle_home, parse_jar_meta, version_key, GradleMeta};
use crate::sidecar::SidecarHandle;
use crate::types::{FileData, SourceSet, SymbolEntry, Visibility};

// ── Gradle cache discovery ────────────────────────────────────────────────────

/// Scan the Gradle module cache and return deduplicated JAR/AAR paths.
///
/// Deduplication: for each `(group, artifact)` pair keep only the file
/// belonging to the highest-version directory — same logic as `extract-sources`.
/// `-sources.jar` and `-javadoc.jar` files are excluded (source already handled
/// by the extract-sources path; javadoc not useful for symbol indexing).
pub(crate) fn scan_gradle_jars(gradle_home: Option<&Path>) -> Vec<PathBuf> {
    let search_root = gradle_home
        .map(|p| p.to_owned())
        .unwrap_or_else(default_gradle_home)
        .join("caches")
        .join("modules-2")
        .join("files-2.1");

    if !search_root.exists() {
        log::debug!("jar: Gradle cache not found at {}", search_root.display());
        return Vec::new();
    }

    // Walk: collect all JAR/AAR paths that are not sources/javadoc.
    let mut candidates: Vec<PathBuf> = Vec::new();
    collect_jars(&search_root, &mut candidates);

    // Deduplicate: (group, artifact) → (version_key, path)
    let mut best: HashMap<
        (String, String),
        (Vec<crate::cli::extract_sources::VersionPart>, PathBuf),
    > = HashMap::new();

    for jar in candidates {
        let Some(GradleMeta {
            group,
            artifact,
            version,
        }) = parse_jar_meta(&jar)
        else {
            continue;
        };
        let vk = version_key(&version);
        let key = (group, artifact);
        match best.get(&key) {
            None => {
                best.insert(key, (vk, jar));
            }
            Some((best_vk, _)) if &vk > best_vk => {
                best.insert(key, (vk, jar));
            }
            _ => {}
        }
    }

    best.into_values().map(|(_, path)| path).collect()
}

fn collect_jars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jars(&path, out);
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            let is_jar = name.ends_with(".jar") || name.ends_with(".aar");
            let is_sources = name.contains("-sources") || name.contains("-javadoc");
            if is_jar && !is_sources {
                out.push(path);
            }
        }
    }
}

// ── Sidecar dispatch ──────────────────────────────────────────────────────────

/// Index the given JAR/AAR files using the sidecar, inserting results into the
/// indexer's symbol maps.  The sidecar handle is borrowed mutably so it can be
/// set to `None` on crash.
pub(crate) fn index_jars(
    indexer: &crate::indexer::Indexer,
    paths: &[PathBuf],
    sidecar: &mut Option<SidecarHandle>,
) {
    if sidecar.is_none() || paths.is_empty() {
        return;
    }

    let zero = Range::default();
    let mut total = 0usize;

    for path in paths {
        let sidecar_symbols = match sidecar.as_mut().unwrap().index_jar(path) {
            Ok(syms) => syms,
            Err(err) => {
                log::warn!(
                    "jar: sidecar error on {}: {err} — disabling sidecar",
                    path.display()
                );
                *sidecar = None;
                break;
            }
        };

        if sidecar_symbols.is_empty() {
            continue;
        }

        let fake_uri = format!("jar://{}", path.display());

        let mut symbols: Vec<SymbolEntry> = Vec::with_capacity(sidecar_symbols.len());
        for sym in &sidecar_symbols {
            let lsp_kind = kind_str_to_lsp(&sym.kind);
            let entry = SymbolEntry {
                name: sym.name.clone(),
                kind: lsp_kind,
                visibility: Visibility::Public,
                range: zero,
                selection_range: zero,
                detail: sym.detail.clone(),
                container: if sym.container.is_empty() {
                    None
                } else {
                    Some(sym.container.clone())
                },
                params: String::new(),
                param_counts: (0, 0),
                type_params: Vec::new(),
                extension_receiver: String::new(),
                extension_receiver_type: String::new(),
            };

            // Update definitions map: name → URI location
            let loc = tower_lsp::lsp_types::Location {
                uri: fake_uri.parse().unwrap_or_else(|_| {
                    tower_lsp::lsp_types::Url::parse("file:///unknown").unwrap()
                }),
                range: zero,
            };
            indexer
                .definitions
                .entry(sym.name.clone())
                .or_default()
                .push(loc);

            symbols.push(entry);
            total += 1;
        }

        // Insert FileData for this JAR (allows hover to find entries)
        let file_data = Arc::new(FileData {
            symbols,
            source_set: SourceSet::Library,
            ..Default::default()
        });
        indexer.files.insert(fake_uri.clone(), file_data);
        indexer.library_uris.insert(fake_uri);
    }

    if total > 0 {
        log::info!(
            "jar: indexed {total} symbols from {} JARs/AARs",
            paths.len()
        );
        indexer.rebuild_bare_name_cache();
    }
}

fn kind_str_to_lsp(kind: &str) -> SymbolKind {
    match kind {
        "class" => SymbolKind::CLASS,
        "interface" => SymbolKind::INTERFACE,
        "object" => SymbolKind::OBJECT,
        "fun" => SymbolKind::FUNCTION,
        "val" => SymbolKind::CONSTANT,
        "var" => SymbolKind::VARIABLE,
        "typealias" => SymbolKind::CLASS,
        _ => SymbolKind::NULL,
    }
}
