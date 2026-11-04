//! Layering ratchet: a module may depend on modules at its own layer or below,
//! never above. Dependencies flow one way, bottom to top.
//!
//! ```text
//! 5  workspace  backend  cli                     entry points, protocol, actor
//! 4  features  semantic_tokens  inlay_hints      one LSP feature each
//! 3  resolver                                    names -> declarations
//! 2  indexer (index store + CST inference)  jar_extract   what the workspace declares
//! 1  parser  rg                                  syntax and text extraction
//! 0  util  path_util  str_ext   queries  stdlib  stdlib_tail
//!    task_runner  sidecar  language  types  workspace_json  gradle_cache
//!    node_ext  text_utils   vocabulary
//! ```
//!
//! Each `(from, to)` entry below is an upward edge that existed when the
//! ratchet was introduced, with the number of `crate::to` mentions in
//! `from`'s production sources. A count only goes down: remove an edge and
//! lower the number (or drop the entry) in the same change. Never raise one
//! or add one — move the shared item down to the lower layer, or invert the
//! dependency (the lower layer defines a trait, the upper layer implements it).
//!
//! The scan is literal text over `crate::<module>` outside `//` comment lines,
//! so `use crate::{a, b}` and re-exports escape it; none exist today and the
//! test fails on `crate::{` to keep it that way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// `(module, layer)`; a module depends only on modules of an equal or lower layer.
const LAYERS: &[(&str, u8)] = &[
    ("util", 0),
    ("path_util", 0),
    ("str_ext", 0),
    ("queries", 0),
    ("stdlib", 0),
    ("stdlib_tail", 0),
    ("task_runner", 0),
    ("sidecar", 0),
    ("language", 0),
    ("types", 0),
    ("workspace_json", 0),
    ("gradle_cache", 0),
    ("node_ext", 0),
    ("detail_text", 0),
    ("text_utils", 0),
    ("parser", 1),
    ("rg", 1),
    ("indexer", 2),
    ("jar_extract", 2),
    ("resolver", 3),
    ("features", 4),
    ("semantic_tokens", 4),
    ("inlay_hints", 4),
    ("workspace", 5),
    ("backend", 5),
    ("cli", 5),
];

/// Upward edges that still exist: `((from, to), mentions)`.
const UPWARD_EDGES: &[((&str, &str), usize)] = &[
    (("indexer", "resolver"), 38),
    (("features", "backend"), 3),
    (("language", "parser"), 3),
];

fn layer_of(module: &str) -> Option<u8> {
    LAYERS
        .iter()
        .find(|(name, _)| *name == module)
        .map(|(_, layer)| *layer)
}

fn is_production_source(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    file_name.ends_with(".rs") && !file_name.ends_with("tests.rs")
}

/// The top-level module a file under `src/` belongs to.
fn module_of(source_root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(source_root).expect("path is under src/");
    let first = relative
        .components()
        .next()
        .expect("path has a component")
        .as_os_str()
        .to_string_lossy()
        .into_owned();
    first.trim_end_matches(".rs").to_string()
}

fn production_sources(source_root: &Path) -> Vec<(String, String)> {
    let mut pending: Vec<PathBuf> = vec![source_root.to_path_buf()];
    let mut sources = Vec::new();
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            let entries = std::fs::read_dir(&path).expect("source directory is readable");
            pending.extend(entries.map(|entry| entry.expect("directory entry").path()));
        } else if is_production_source(&path) {
            let text = std::fs::read_to_string(&path).expect("source is readable");
            sources.push((module_of(source_root, &path), text));
        }
    }
    sources
}

/// Names after each `crate::` on a non-comment line.
fn crate_mentions(text: &str) -> Vec<String> {
    let mut mentions = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("//") {
            continue;
        }
        for (index, _) in line.match_indices("crate::") {
            let rest = &line[index + "crate::".len()..];
            let name: String = rest
                .chars()
                .take_while(|character| character.is_alphanumeric() || *character == '_')
                .collect();
            mentions.push(name);
        }
    }
    mentions
}

fn upward_edge_counts() -> BTreeMap<(String, String), usize> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut counts = BTreeMap::new();
    for (module, text) in production_sources(&source_root) {
        let Some(from_layer) = layer_of(&module) else {
            continue;
        };
        for target in crate_mentions(&text) {
            let Some(to_layer) = layer_of(&target) else {
                continue;
            };
            if to_layer > from_layer && target != module {
                *counts.entry((module.clone(), target)).or_insert(0) += 1;
            }
        }
    }
    counts
}

#[test]
fn every_top_level_module_has_a_layer() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let unlayered: Vec<String> = production_sources(&source_root)
        .into_iter()
        .map(|(module, _)| module)
        .filter(|module| module != "main" && module != "lib" && layer_of(module).is_none())
        .collect();
    let mut unlayered = unlayered;
    unlayered.sort();
    unlayered.dedup();
    assert!(
        unlayered.is_empty(),
        "modules missing from LAYERS in tests/layering_ratchet.rs: {unlayered:?}"
    );
}

#[test]
fn no_grouped_crate_imports() {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let grouped: Vec<String> = production_sources(&source_root)
        .into_iter()
        .filter(|(_, text)| text.contains("crate::{"))
        .map(|(module, _)| module)
        .collect();
    assert!(
        grouped.is_empty(),
        "`use crate::{{..}}` hides edges from the layering scan; spell each path out: {grouped:?}"
    );
}

#[test]
fn dependencies_flow_upward_only() {
    let found = upward_edge_counts();
    let baseline: BTreeMap<(String, String), usize> = UPWARD_EDGES
        .iter()
        .map(|((from, to), count)| ((from.to_string(), to.to_string()), *count))
        .collect();
    let mut drift = Vec::new();
    for (edge, count) in &found {
        let allowed = baseline.get(edge).copied().unwrap_or(0);
        if *count != allowed {
            drift.push(format!(
                "{} -> {}: baseline {allowed}, found {count}",
                edge.0, edge.1
            ));
        }
    }
    for (edge, allowed) in &baseline {
        if !found.contains_key(edge) {
            drift.push(format!(
                "{} -> {}: baseline {allowed}, found 0 (remove the entry)",
                edge.0, edge.1
            ));
        }
    }
    assert!(
        drift.is_empty(),
        "upward dependencies drifted from tests/layering_ratchet.rs.\n\
         More than baseline: move the shared item down or invert the dependency.\n\
         Fewer than baseline: lower the baseline.\n\n{}",
        drift.join("\n")
    );
}
