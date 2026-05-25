//! Gradle cache JAR/AAR scanning and sidecar-based symbol indexing.
//!
//! Scans `~/.gradle/caches/modules-2/files-2.1/` for non-sources JARs and AARs,
//! deduplicates by `(group, artifact, latest-version)`, and sends each file to
//! the `kotlin-jar-indexer` sidecar process to produce `SymbolEntry` items.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::SymbolKind;

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

    let mut total = 0usize;
    for path in paths {
        match index_single_jar(indexer, path, sidecar) {
            Some(count) => total += count,
            None => break, // sidecar disabled
        }
    }

    if total > 0 {
        log::info!(
            "jar: indexed {total} symbols from {} JARs/AARs",
            paths.len()
        );
        indexer.rebuild_bare_name_cache();
    }
}

/// Index one JAR/AAR. Returns `Some(symbol_count)` on success, `None` when the
/// sidecar crashes (caller should stop iterating).
fn index_single_jar(
    indexer: &crate::indexer::Indexer,
    path: &Path,
    sidecar: &mut Option<SidecarHandle>,
) -> Option<usize> {
    let sidecar_symbols = match sidecar.as_mut().unwrap().index_jar(path) {
        Ok(syms) => syms,
        Err(err) => {
            log::warn!(
                "jar: sidecar error on {}: {err} — disabling sidecar",
                path.display()
            );
            *sidecar = None;
            return None;
        }
    };

    if sidecar_symbols.is_empty() {
        return Some(0);
    }

    let fake_uri = match tower_lsp::lsp_types::Url::parse(&format!("jar:file://{}", path.display()))
    {
        Ok(u) => u,
        Err(e) => {
            log::warn!("jar: cannot build URI for {}: {e}", path.display());
            return Some(0);
        }
    };
    let fake_uri_str = fake_uri.to_string();

    // Remove stale data for this JAR before inserting fresh symbols.
    indexer.jar_files.remove(&fake_uri_str);
    indexer.jar_definitions.retain(|_, locs| {
        locs.retain(|l| l.uri != fake_uri);
        !locs.is_empty()
    });

    let count = build_jar_file_data(indexer, &fake_uri, &fake_uri_str, &sidecar_symbols);
    Some(count)
}

/// Build `FileData` + definition entries for one JAR and insert them into the index.
fn build_jar_file_data(
    indexer: &crate::indexer::Indexer,
    fake_uri: &tower_lsp::lsp_types::Url,
    fake_uri_str: &str,
    sidecar_symbols: &[crate::sidecar::SidecarSymbol],
) -> usize {
    let mut symbols: Vec<SymbolEntry> = Vec::with_capacity(sidecar_symbols.len());

    for (line_idx, sym) in sidecar_symbols.iter().enumerate() {
        let synthetic_range = tower_lsp::lsp_types::Range {
            start: tower_lsp::lsp_types::Position {
                line: line_idx as u32,
                character: 0,
            },
            end: tower_lsp::lsp_types::Position {
                line: line_idx as u32,
                character: sym.name.len() as u32,
            },
        };
        symbols.push(SymbolEntry {
            name: sym.name.clone(),
            kind: kind_str_to_lsp(&sym.kind),
            visibility: Visibility::Public,
            range: synthetic_range,
            selection_range: synthetic_range,
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
            doc: sym.doc.clone(),
        });
        indexer
            .jar_definitions
            .entry(sym.name.clone())
            .or_default()
            .push(tower_lsp::lsp_types::Location {
                uri: fake_uri.clone(),
                range: synthetic_range,
            });
    }

    let lines: Vec<String> = sidecar_symbols.iter().map(|s| s.detail.clone()).collect();

    let count = symbols.len();
    indexer.jar_files.insert(
        fake_uri_str.to_owned(),
        Arc::new(FileData {
            symbols,
            source_set: SourceSet::Library,
            lines: Arc::new(lines),
            ..Default::default()
        }),
    );
    indexer.library_uris.insert(fake_uri_str.to_owned());
    count
}

fn kind_str_to_lsp(kind: &str) -> SymbolKind {
    match kind {
        "class" => SymbolKind::CLASS,
        "interface" => SymbolKind::INTERFACE,
        "object" => SymbolKind::OBJECT,
        "fun" => SymbolKind::FUNCTION,
        "val" => SymbolKind::PROPERTY,
        "var" => SymbolKind::VARIABLE,
        "typealias" => SymbolKind::CLASS,
        _ => SymbolKind::NULL,
    }
}
