//! Gradle cache JAR/AAR scanning and sidecar-based symbol indexing.
//!
//! Scans `~/.gradle/caches/modules-2/files-2.1/` for JARs and AARs,
//! deduplicates by `(group, artifact, latest-version)`.
//!
//! Processing:
//! - **Sources JARs** (`-sources.jar`): unzipped in-memory and indexed
//!   via tree-sitter for full quality (params, line positions).
//! - **Compiled JARs/AARs**: sent to the `kmp-jar-indexer` sidecar process
//!   for bytecode-level symbol extraction (name, kind, detail, receiver).
//! - `-javadoc.jar` files are still excluded (not useful for symbol indexing).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::SymbolKind;

use crate::cli::extract_sources::{default_gradle_home, parse_jar_meta, version_key, GradleMeta};
use crate::indexer::infer::sig::extract_params_from_detail;
use crate::sidecar::SidecarHandle;
use crate::types::{ExtensionEntry, FileData, SourceSet, SymbolEntry, Visibility};

// ── Gradle cache discovery ────────────────────────────────────────────────────

/// Scan the Gradle module cache and return deduplicated JAR/AAR paths.
///
/// Deduplication: for each `(group, artifact)` pair keep only the file
/// belonging to the highest-version directory — same logic as `extract-sources`.
/// `-javadoc.jar` files are excluded (not useful for symbol indexing).
/// `-sources.jar` files are now included — they're auto-unpacked in `index_jars`.
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

    // Walk: collect all JAR/AAR paths except javadoc.
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
            let is_javadoc = name.contains("-javadoc");
            if is_jar && !is_javadoc {
                out.push(path);
            }
        }
    }
}

/// Returns true if the filename (last component) suggests this is a sources JAR.
fn is_sources_jar(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("-sources"))
}

// ── Sidecar dispatch ──────────────────────────────────────────────────────────

/// Index the given JAR/AAR files, inserting results into the indexer's symbol maps.
///
/// Sources JARs are auto-unpacked via tree-sitter instead of going through the
/// sidecar (which only handles compiled bytecode).
pub(crate) fn index_jars(
    indexer: &crate::indexer::Indexer,
    paths: &[PathBuf],
    sidecar: &mut Option<SidecarHandle>,
) -> usize {
    if paths.is_empty() {
        return 0;
    }

    // Split: sources JARs handled inline, compiled JARs go to sidecar.
    let (sources_jars, compiled_jars): (Vec<&PathBuf>, Vec<&PathBuf>) =
        paths.iter().partition(|p| is_sources_jar(p));

    // Clear stale JAR symbols before re-indexing to prevent duplicates.
    indexer.jar_files.clear();
    indexer.jar_definitions.clear();
    indexer.jar_uri_to_defs.clear();

    let mut total = 0usize;

    // ── Sources JARs: unzip + tree-sitter (no cache needed — Gradle cache is immutable) ──
    for path in &sources_jars {
        total += index_sources_jar(indexer, path);
    }

    // ── Compiled JARs: sidecar with disk cache ──
    total += index_compiled_jars(indexer, &compiled_jars, sidecar);

    if total > 0 {
        indexer
            .bare_names_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut last) = indexer.last_completion.lock() {
            *last = None;
        }
        indexer
            .completion_epoch
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }
    total
}

/// Unzip a sources JAR and index each `.kt`/`.java` entry via `index_content`.
fn index_sources_jar(indexer: &crate::indexer::Indexer, path: &Path) -> usize {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("jar: cannot open sources jar {}: {e}", path.display());
            return 0;
        }
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            log::warn!("jar: cannot read ZIP {}: {e}", path.display());
            return 0;
        }
    };

    let mut count = 0usize;

    for entry_index in 0..archive.len() {
        let mut entry = match archive.by_index(entry_index) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let entry_name = entry.name().to_owned();
        if !entry_name.ends_with(".kt")
            && !entry_name.ends_with(".kts")
            && !entry_name.ends_with(".java")
        {
            continue;
        }

        let mut content = String::new();
        if entry.read_to_string(&mut content).is_err() {
            continue;
        }

        let uri = match tower_lsp::lsp_types::Url::parse(&format!(
            "jar:file://{}!/{}",
            path.display(),
            entry_name
        )) {
            Ok(u) => u,
            Err(_) => continue,
        };

        // Use parse_file directly — index_content's URI-based language
        // detection breaks on jar:file:// URLs with !/ separators.
        let result = crate::indexer::Indexer::parse_file(&uri, &content);
        indexer.apply_file_result(&result);
        count += 1;
    }

    if count > 0 {
        log::info!(
            "jar: indexed {} source files from {}",
            count,
            path.display()
        );
    }
    count
}

/// Dispatch compiled JARs through the sidecar with disk-cache support.
fn index_compiled_jars(
    indexer: &crate::indexer::Indexer,
    paths: &[&PathBuf],
    sidecar: &mut Option<SidecarHandle>,
) -> usize {
    if paths.is_empty() {
        return 0;
    }

    let mut jar_cache = super::jar_cache::load_jar_cache();
    let mut total = 0usize;
    let mut cache_hits = 0usize;
    let mut cache_dirty = false;
    let mut missed: Vec<(PathBuf, String)> = Vec::new();

    for path in paths {
        let path_key = path.to_string_lossy().to_string();

        if let Some(entry) = jar_cache.get(&path_key) {
            if super::jar_cache::cache_entry_is_fresh(entry, path) {
                total += populate_from_symbols(indexer, path, &entry.symbols);
                cache_hits += 1;
                continue;
            }
        }

        missed.push(((*path).clone(), path_key));
    }

    if !missed.is_empty() {
        if let Some(ref mut sidecar_guard) = sidecar {
            let sidecar_paths: Vec<&Path> = missed.iter().map(|(p, _)| p.as_path()).collect();
            match sidecar_guard.index_jars(&sidecar_paths) {
                Ok(results) => {
                    let total_symbols: usize = results.iter().map(|r| r.len()).sum();
                    log::info!(
                        "jar: sidecar returned {total_symbols} symbols across {} JARs",
                        results.len()
                    );
                    for ((path, path_key), symbols) in missed.into_iter().zip(results) {
                        total += populate_from_symbols(indexer, &path, &symbols);
                        if let Some(entry) = super::jar_cache::make_cache_entry(&path, symbols) {
                            jar_cache.insert(path_key, entry);
                            cache_dirty = true;
                        }
                    }
                }
                Err(err) => {
                    log::warn!("jar: sidecar batch error: {err} — disabling sidecar");
                    *sidecar = None;
                }
            }
        }
    }

    if cache_dirty {
        super::jar_cache::save_jar_cache(&jar_cache);
    }

    if total > 0 {
        log::info!(
            "jar: indexed {total} symbols from {} compiled JARs ({cache_hits} from cache)",
            paths.len()
        );
    }
    total
}

/// Insert symbols for one JAR into the indexer.  Returns the symbol count.
fn populate_from_symbols(
    indexer: &crate::indexer::Indexer,
    path: &Path,
    sidecar_symbols: &[crate::sidecar::SidecarSymbol],
) -> usize {
    if sidecar_symbols.is_empty() {
        return 0;
    }
    let fake_uri = match tower_lsp::lsp_types::Url::parse(&format!("jar:file://{}", path.display()))
    {
        Ok(u) => u,
        Err(e) => {
            log::warn!("jar: cannot build URI for {}: {e}", path.display());
            return 0;
        }
    };
    let fake_uri_str = fake_uri.to_string();

    if let Some((_, names)) = indexer.jar_uri_to_defs.remove(&fake_uri_str) {
        for name in &names {
            if let Some(mut entry) = indexer.jar_definitions.get_mut(name) {
                entry.retain(|l| l.uri != fake_uri);
                if entry.is_empty() {
                    drop(entry);
                    indexer.jar_definitions.remove(name);
                }
            }
        }
    }
    indexer.jar_files.remove(&fake_uri_str);

    build_jar_file_data(indexer, &fake_uri, &fake_uri_str, sidecar_symbols)
}

/// Derive `(params_text, param_counts)` from a detail string like
/// `"fun foo(x: Int, y: String = \"\"): Boolean"`.
fn derive_param_counts(detail: &str) -> (String, (u8, u8)) {
    let Some(params_text) = extract_params_from_detail(detail) else {
        return (String::new(), (0, 0));
    };
    let trimmed = params_text.trim();
    if trimmed.is_empty() {
        return (params_text, (0, 0));
    }
    let parts = crate::indexer::infer::sig::split_params_at_depth_zero(trimmed);
    let total = parts.len() as u8;
    let required = parts.iter().filter(|p| !p.contains('=')).count() as u8;
    (params_text, (required, total))
}

/// Build `FileData` + definition entries for one JAR and insert them into the index.
fn build_jar_file_data(
    indexer: &crate::indexer::Indexer,
    fake_uri: &tower_lsp::lsp_types::Url,
    fake_uri_str: &str,
    sidecar_symbols: &[crate::sidecar::SidecarSymbol],
) -> usize {
    let mut symbols: Vec<SymbolEntry> = Vec::with_capacity(sidecar_symbols.len());
    let mut jar_names: Vec<String> = Vec::with_capacity(sidecar_symbols.len());

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
        let extension_receiver = sym
            .extension_receiver_type
            .split('<')
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_owned();

        let (params_text, param_counts) = derive_param_counts(&sym.detail);

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
            params: params_text,
            param_counts,
            type_params: sym.type_params.clone(),
            extension_receiver,
            extension_receiver_type: sym.extension_receiver_type.clone(),
            doc: sym.doc.clone(),
            trailing_lambda: sym.trailing_lambda,
        });
        indexer
            .jar_definitions
            .entry(sym.name.clone())
            .or_default()
            .push(tower_lsp::lsp_types::Location {
                uri: fake_uri.clone(),
                range: synthetic_range,
            });
        jar_names.push(sym.name.clone());
    }

    indexer
        .jar_uri_to_defs
        .insert(fake_uri_str.to_owned(), jar_names);

    let lines: Vec<String> = sidecar_symbols.iter().map(|s| s.detail.clone()).collect();

    let count = symbols.len();

    for sym in &symbols {
        if sym.extension_receiver.is_empty() {
            continue;
        }
        indexer
            .extension_by_receiver
            .entry(sym.extension_receiver.clone())
            .or_default()
            .push(ExtensionEntry {
                file_uri: fake_uri_str.to_owned(),
                name: sym.name.clone(),
                kind: sym.kind,
                detail: sym.detail.clone(),
                visibility: Visibility::Public,
                package: None,
                trailing_lambda: sym.trailing_lambda,
            });
    }

    // Infer package from symbol detail so resolve_via_imports can match imports.
    // Sidecar doesn't report package; extract from detail (e.g. "class a.b.C" → "a.b").
    let package: Option<String> = symbols.first().and_then(|sym| {
        let detail = &sym.detail;
        let after_kind = detail.find(' ').map(|pos| pos + 1).unwrap_or(0);
        let fqn = &detail[after_kind..];
        fqn.rfind('.').map(|dot| fqn[..dot].to_owned())
    });

    // Register in qualified index for FQN resolution (resolve_via_imports step i).
    if let Some(ref pkg) = package {
        for sym in &symbols {
            let fqn = format!("{pkg}.{}", sym.name);
            indexer.qualified.insert(
                fqn,
                tower_lsp::lsp_types::Location {
                    uri: fake_uri.clone(),
                    range: sym.range,
                },
            );
        }
    }

    indexer.jar_files.insert(
        fake_uri_str.to_owned(),
        Arc::new(FileData {
            symbols,
            source_set: SourceSet::Library,
            lines: Arc::new(lines),
            package,
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

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn param_counts_from_detail_zero_params() {
        let (params_text, (required, total)) = derive_param_counts("fun loadData(): Any");
        assert_eq!((required, total), (0, 0));
        assert!(params_text.is_empty());
    }

    #[test]
    fn param_counts_from_detail_single_required() {
        let (params_text, (required, total)) = derive_param_counts("fun foo(x: Int): String");
        assert_eq!((required, total), (1, 1));
        assert_eq!(params_text, "x: Int");
    }

    #[test]
    fn param_counts_from_detail_mixed_defaults() {
        let (params_text, (required, total)) = derive_param_counts(
            "fun bar(name: String, count: Int = 0, force: Boolean = false): Unit",
        );
        assert_eq!((required, total), (1, 3));
        assert!(params_text.contains("name: String"));
    }

    #[test]
    fn param_counts_from_detail_generic_type() {
        let (params_text, (required, total)) =
            derive_param_counts("fun <T> map(input: T, transform: (T) -> Boolean): List<T>");
        assert_eq!((required, total), (2, 2));
        assert!(params_text.contains("input: T"));
    }

    #[test]
    fn param_counts_from_detail_empty_detail() {
        let (params_text, (required, total)) = derive_param_counts("");
        assert_eq!((required, total), (0, 0));
        assert!(params_text.is_empty());
    }

    #[test]
    fn param_counts_from_detail_no_parens() {
        let (_params_text, (required, total)) = derive_param_counts("val x: Int");
        assert_eq!((required, total), (0, 0));
    }
}
