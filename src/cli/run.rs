//! CLI command runner.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tower_lsp::lsp_types::Location;

use crate::indexer::{Indexer, NoopReporter};
use crate::rg::{rg_find_definition, rg_word_search, RgSearchRequest};

use super::args::{CliArgs, Mode, OutputFmt, ResultFilters, Subcommand};
use super::complete::completions_at;
use super::hover::hover_at;
use super::output::{print_results, CliResult, PrintOpts};
use super::tokens::{dump_tree, print_token_rows, token_rows, token_rows_phases};

// ── Relative-path resolution ──────────────────────────────────────────────────

/// Auto-enable `--relative` when stdout isn't a TTY (typical AI-agent invocation,
/// where the absolute workspace prefix is pure token waste). `--relative` and
/// `--absolute` always win over the auto-default.
fn resolve_effective_relative(mut filters: ResultFilters, absolute_flag: bool) -> ResultFilters {
    use std::io::IsTerminal;
    if !filters.relative && !absolute_flag && !std::io::stdout().is_terminal() {
        filters.relative = true;
    }
    filters
}

// ── Root resolution ───────────────────────────────────────────────────────────

/// Resolve the workspace root: explicit --root, then nearest .git ancestor, then cwd.
fn resolve_root(explicit: Option<&Path>) -> PathBuf {
    if let Some(r) = explicit {
        return r.to_path_buf();
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_git_root(&cwd).unwrap_or(cwd)
}

/// Walk up from `start` looking for a `.git` directory.
fn find_git_root(start: &Path) -> Option<PathBuf> {
    let mut cur = start;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// Resolve workspace root for file-centric commands: tries explicit root first,
/// then walks up from the file's directory, then falls back to CWD-based detection.
fn resolve_root_for_file(explicit: Option<&Path>, file: &Path) -> PathBuf {
    if let Some(r) = explicit {
        return r.to_path_buf();
    }
    let file_dir = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let file_dir = file_dir.parent().unwrap_or(&file_dir);
    if let Some(root) = find_git_root(file_dir) {
        return root;
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    find_git_root(&cwd).unwrap_or(cwd)
}

// ── Column resolution helpers ─────────────────────────────────────────────────

/// Resolve a 1-based UTF-16 column for `complete`, applying `--dot` / `--eol`
/// when an explicit col is absent or when the flags are set.
///
/// - `--dot` (`dot=true`): position just after the last `.` on the line.
///   Returns `Err` if the line contains no `.`.
/// - `--eol` (`eol=true`): position after the last non-whitespace character.
///   Returns `Err` if the line is blank/whitespace-only.
/// - explicit col: used as-is.
/// - fallback (no flags, no col): col 1 (beginning of line).
fn resolve_col(
    file: &Path,
    line: u32,
    col: Option<u32>,
    dot: bool,
    eol: bool,
) -> Result<u32, String> {
    if !dot && !eol {
        return Ok(col.unwrap_or(1));
    }
    let line_text = read_line(file, line)?;
    if dot {
        col_after_last_dot(&line_text).ok_or_else(|| format!("no '.' found on line {line}"))
    } else {
        col_after_last_nonws(&line_text)
            .ok_or_else(|| format!("line {line} is blank — cannot use --eol"))
    }
}

/// Read line `line` (1-based) from `file` using a buffered reader —
/// stops at the target line without loading the whole file.
/// Returns `Err` on I/O error or when `line` is out of range.
fn read_line(file: &Path, line: u32) -> Result<String, String> {
    use std::io::BufRead;
    let f =
        std::fs::File::open(file).map_err(|e| format!("cannot open {}: {e}", file.display()))?;
    let reader = std::io::BufReader::new(f);
    let target = (line as usize).saturating_sub(1);
    reader
        .lines()
        .nth(target)
        .ok_or_else(|| format!("line {line} is out of range in {}", file.display()))?
        .map_err(|e| format!("cannot read line {line} from {}: {e}", file.display()))
}

/// Return 1-based UTF-16 column just after the last `.` in `text`, or `None`
/// if there is no dot.
fn col_after_last_dot(text: &str) -> Option<u32> {
    // byte index of last '.'
    let dot_byte = text.rfind('.')?;
    // UTF-16 length up to and including the dot, then +1 for "after the dot"
    let utf16_before: usize = text[..dot_byte].encode_utf16().count();
    // +2: +1 for the dot itself, +1 for 1-based
    Some((utf16_before + 2) as u32)
}

/// Return 1-based UTF-16 column just after the last non-whitespace character,
/// or `None` if the line is blank.
fn col_after_last_nonws(text: &str) -> Option<u32> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let utf16_len = trimmed.encode_utf16().count();
    Some((utf16_len + 1) as u32)
}

// ── Cache probe ───────────────────────────────────────────────────────────────

fn cache_exists(root: &Path) -> bool {
    crate::indexer::workspace_cache_path(root).exists()
}

// ── Indexer bootstrap ─────────────────────────────────────────────────────────

/// Build (or load from cache) a full workspace index.  Reports progress to stderr.
///
/// Source paths are collected from:
/// 1. `workspace.json` (JetBrains IDE format) `sourcePaths` field at the workspace root
/// 2. `~/.kotlin-lsp/sources` — the default `extract-sources` output dir
///    (skipped when `no_stdlib` is true)
async fn build_index(root: &Path, no_stdlib: bool) -> Arc<Indexer> {
    build_index_inner(root, collect_cli_source_paths(root, no_stdlib)).await
}

/// Build a full workspace index with explicitly provided source paths.
/// Bypasses all workspace.json / global-default discovery — for tests.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) async fn build_index_with_sources(
    root: &Path,
    source_paths: Vec<std::path::PathBuf>,
) -> Arc<Indexer> {
    let strs: Vec<String> = source_paths
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    build_index_inner(root, strs).await
}

async fn build_index_inner(root: &Path, source_paths: Vec<String>) -> Arc<Indexer> {
    let idx = Arc::new(Indexer::new());
    if !source_paths.is_empty() {
        *idx.source_paths_raw.write().expect("source_paths lock") = source_paths;
    }
    // Populate workspace source roots from workspace.json so resolver/infer rg fallbacks
    // are scoped when the CLI is run in a project with configured module sourceRoots.
    let workspace_roots: Vec<String> = crate::workspace_json::load_source_paths(root)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    if !workspace_roots.is_empty() {
        *idx.workspace_source_roots
            .write()
            .expect("workspace_source_roots lock") = workspace_roots;
    }
    Arc::clone(&idx)
        .index_workspace_full(root, Arc::new(NoopReporter))
        .await;
    idx
}

/// Collect source paths for CLI indexing: workspace.json + default extract dir.
///
/// Build-layout paths auto-detected under `root` are intentionally excluded —
/// those files are already covered by `index_workspace_full`'s workspace scan.
/// Only paths that live *outside* the workspace root need a separate indexing pass.
///
/// When `no_stdlib` is true, `~/.kotlin-lsp/sources` is excluded regardless of
/// whether it appears in `workspace.json` or is auto-detected. Use this for fast
/// workspace-only completions (~2s vs ~10s).
fn collect_cli_source_paths(root: &Path, no_stdlib: bool) -> Vec<String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    #[allow(deprecated)]
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let default_sources = home.join(".kotlin-lsp").join("sources");
    let canonical_default_sources = default_sources
        .canonicalize()
        .unwrap_or_else(|_| default_sources.clone());

    let is_external = |p: &std::path::PathBuf| -> bool {
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        !canonical.starts_with(&canonical_root)
    };
    let is_stdlib = |p: &std::path::PathBuf| -> bool {
        let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
        canonical == canonical_default_sources
    };

    let mut paths: Vec<String> = Vec::new();

    let json_paths = crate::workspace_json::load_source_paths(root);
    for p in &json_paths {
        if is_external(p) && !(no_stdlib && is_stdlib(p)) {
            let s = p.to_string_lossy().into_owned();
            if !paths.contains(&s) {
                paths.push(s);
            }
        }
    }

    // If workspace.json declares explicit sourcePaths, use those and skip the
    // global default.  An absent key (None) falls through to the global default.
    if let Some(configured) = crate::workspace_json::load_configured_source_paths(root) {
        for p in configured {
            if is_external(&p) && !(no_stdlib && is_stdlib(&p)) {
                let s = p.to_string_lossy().into_owned();
                if !paths.contains(&s) {
                    paths.push(s);
                }
            }
        }
        return paths;
    }

    if no_stdlib {
        return paths;
    }

    // Auto-include the well-known `extract-sources` output dir if present.
    if default_sources.is_dir() {
        let s = default_sources.to_string_lossy().into_owned();
        if !paths.contains(&s) {
            paths.push(s);
        }
    }

    paths
}

// ── Location helpers ─────────────────────────────────────────────────────────

fn locs_to_results(locs: Vec<Location>, name: &str, kind: &str) -> Vec<CliResult> {
    locs.iter()
        .filter_map(|l| CliResult::from_location(l, name, kind))
        .collect()
}

// ── Workspace source roots for CLI ───────────────────────────────────────────

/// Load workspace.json module sourceRoots to scope rg searches in the CLI.
/// Mirrors the subset of `Backend::collect_workspace_source_roots` relevant for CLI.
fn cli_workspace_source_roots(root: &Path) -> Vec<String> {
    crate::workspace_json::load_source_paths(root)
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

// ── Smart-mode find ───────────────────────────────────────────────────────────

fn smart_find(indexer: &Arc<Indexer>, name: &str, root: &Path) -> Vec<CliResult> {
    // Query definitions index for exact name match.
    let locs = indexer.definition_locations(name);
    if !locs.is_empty() {
        return locs_to_results(locs, name, "");
    }
    let source_roots = cli_workspace_source_roots(root);
    let locs = rg_find_definition(name, Some(root), &source_roots, None);
    locs_to_results(locs, name, "")
}

// ── Smart-mode refs ───────────────────────────────────────────────────────────

fn smart_refs(indexer: &Arc<Indexer>, name: &str, root: &Path) -> Vec<CliResult> {
    let decl_locs = indexer.definition_locations(name);
    let decl_files: Vec<String> = decl_locs
        .iter()
        .filter_map(|l| l.uri.to_file_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();

    let dummy_uri: tower_lsp::lsp_types::Url = tower_lsp::lsp_types::Url::from_file_path(root)
        .unwrap_or_else(|_| "file:///".parse().expect("serialize JSON"));

    let source_roots = cli_workspace_source_roots(root);
    let request = RgSearchRequest::new(name, None, None, Some(root), true, &dummy_uri, &decl_files)
        .with_source_paths(&source_roots);
    let locs = crate::rg::rg_find_references(&request, None);
    locs_to_results(locs, name, "")
}

// ── Fast-mode find ────────────────────────────────────────────────────────────

fn fast_find(name: &str, root: &Path) -> Vec<CliResult> {
    let source_roots = cli_workspace_source_roots(root);
    let locs = rg_find_definition(name, Some(root), &source_roots, None);
    locs_to_results(locs, name, "")
}

// ── Fast-mode refs ────────────────────────────────────────────────────────────

fn fast_refs(name: &str, root: &Path) -> Vec<CliResult> {
    let source_roots = cli_workspace_source_roots(root);
    let locs = rg_word_search(name, root, &source_roots);
    locs_to_results(locs, name, "")
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub(crate) async fn run(args: CliArgs) {
    let json = args.fmt == OutputFmt::Json;
    let verbose = args.verbose;
    let absolute = args.absolute;
    let flat = args.flat;

    match args.subcommand {
        Subcommand::Index => {
            let root = resolve_root(args.root.as_deref());
            run_index(&root, verbose).await
        }
        Subcommand::Find { name, filters } => {
            let root = resolve_root(args.root.as_deref());
            let filters = resolve_effective_relative(filters, absolute);
            run_find(&root, args.mode, json, flat, verbose, &name, &filters).await
        }
        Subcommand::Refs {
            name,
            filters,
            explain,
        } => {
            let root = resolve_root(args.root.as_deref());
            let json = args.fmt == OutputFmt::Json;
            let verbose = args.verbose;
            let flat = args.flat;
            run_refs(
                &root, args.mode, json, flat, verbose, &name, &filters, explain,
            )
            .await
        }
        Subcommand::Hover { file, line, col } => {
            let root = resolve_root_for_file(args.root.as_deref(), &file);
            run_hover(&root, args.mode, json, verbose, &file, line, col).await
        }
        Subcommand::Complete {
            file,
            line,
            col,
            dot,
            eol,
            no_stdlib,
        } => {
            let root = resolve_root_for_file(args.root.as_deref(), &file);
            let resolved_col = match resolve_col(&file, line, col, dot, eol) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("error: {e}");
                    std::process::exit(1);
                }
            };
            run_complete(&root, json, verbose, &file, line, resolved_col, no_stdlib).await
        }
        Subcommand::Tokens {
            file,
            cst_only,
            resolve,
            phases,
            show_tree,
        } => {
            let root = resolve_root_for_file(args.root.as_deref(), &file);
            let use_index = resolve && !cst_only;
            let index = if use_index {
                if verbose {
                    eprintln!("Loading index for Phase 2 resolution...");
                }
                Some(build_index(&root, false).await)
            } else {
                None
            };
            run_tokens(json, &file, index.as_ref(), cst_only, phases, show_tree)
        }
        Subcommand::Tree { file } => run_tree(&file),
        Subcommand::Sources { explain } => {
            let root = resolve_root(args.root.as_deref());
            super::sources::run_sources(&root, json, explain)
        }
        Subcommand::Cache { sub } => {
            if sub == "stats" {
                let root = resolve_root(args.root.as_deref());
                let cache_path = crate::indexer::workspace_cache_path(&root);
                println!("Cache path: {}", cache_path.display());
                if cache_path.exists() {
                    if let Ok(meta) = std::fs::metadata(&cache_path) {
                        let size = meta.len();
                        println!("Size: {} bytes", size);
                    }
                    println!("Status: ✅ exists");
                } else {
                    println!("Status: ❌ (no cache found)");
                }
                return;
            }
            eprintln!("Unknown cache subcommand: {sub}. Use: stats");
        }

        Subcommand::ExtractSources {
            gradle_home,
            output,
            dry_run,
            patterns,
        } => super::extract_sources::run_extract_sources(super::extract_sources::ExtractOptions {
            gradle_home,
            output,
            dry_run,
            patterns,
        }),
        Subcommand::Batch { file, dry_run } => {
            super::batch::run_batch(&file, dry_run);
        }
        Subcommand::Insert {
            file,
            line,
            before,
            after,
            content,
            in_place,
        } => {
            super::insert::run_insert(&file, line, before, after, &content, in_place);
        }
        Subcommand::Inject { file } => {
            let root = resolve_root_for_file(None, &file);
            super::inject::run_inject(&file, &root, json, 50).await;
        }

        Subcommand::Check { files } => {
            if files.is_empty() {
                eprintln!("check requires at least one FILE argument");
                std::process::exit(1);
            }
            let expanded = super::check::expand_file_list(&files);
            super::check::run_check(&expanded, json);
        }
        Subcommand::OrganizeImports { files } => {
            if files.is_empty() {
                eprintln!("organize-imports requires at least one FILE argument");
                std::process::exit(1);
            }
            super::organize_imports::run_organize_imports(&files, json);
        }
        Subcommand::Context {
            file,
            line,
            col,
            expand,
        } => {
            run_context(&file, line, col, json, expand).await;
        }
        Subcommand::CallHierarchy {
            file,
            line,
            col,
            incoming,
            outgoing,
        } => {
            run_call_hierarchy(&file, line, col, incoming, outgoing, json).await;
        }
        Subcommand::TypeHierarchy {
            name,
            subtypes,
            supertypes,
        } => {
            run_type_hierarchy(&name, subtypes, supertypes, json).await;
        }
    }
}

async fn run_index(root: &Path, verbose: bool) {
    if verbose {
        eprintln!("Indexing workspace: {}", root.display());
    }
    let index = build_index(root, false).await;
    if verbose {
        eprintln!(
            "Done: {} files, {} symbols",
            index.files.len(),
            index.definitions.len()
        );
    }
}

async fn run_find(
    root: &Path,
    mode: Mode,
    json: bool,
    flat: bool,
    verbose: bool,
    name: &str,
    filters: &ResultFilters,
) {
    let results = match effective_mode(mode, root, "find", verbose) {
        Mode::Fast => fast_find(name, root),
        _ => {
            let index = build_index(root, false).await;
            smart_find(&index, name, root)
        }
    };
    let results = apply_filters(results, root, filters);
    exit_if_empty(
        &results,
        json,
        &format!("No declarations found for '{name}'"),
    );
    print_results(
        &results,
        &PrintOpts {
            json,
            relative: filters.relative,
            flat,
        },
    );
}

#[allow(clippy::too_many_arguments)]
async fn run_refs(
    root: &Path,
    mode: Mode,
    json: bool,
    flat: bool,
    verbose: bool,
    name: &str,
    filters: &ResultFilters,
    explain: bool,
) {
    let results = match effective_mode(mode, root, "refs", verbose) {
        Mode::Fast => fast_refs(name, root),
        _ => {
            let index = build_index(root, false).await;
            smart_refs(&index, name, root)
        }
    };
    let results = apply_filters(results, root, filters);
    exit_if_empty(&results, json, &format!("No references found for '{name}'"));

    // explain mode: classify each result by reference type
    let results: Vec<CliResult> = if explain {
        results
            .into_iter()
            .map(|mut r| {
                let line_text = std::fs::read_to_string(&r.file)
                    .ok()
                    .and_then(|s| s.lines().nth(r.line as usize - 1).map(|l| l.to_owned()))
                    .unwrap_or_default();
                let trimmed = line_text.trim_start();
                let label = if trimmed.starts_with(&r.name) && trimmed.contains('(') {
                    "declaration"
                } else if trimmed.starts_with("override ") && trimmed.contains(&r.name) {
                    "override"
                } else if trimmed.starts_with("import ") {
                    "import"
                } else {
                    "reference"
                };
                r.kind = label.to_owned();
                r
            })
            .collect()
    } else {
        results
    };

    print_results(
        &results,
        &PrintOpts {
            json,
            relative: filters.relative,
            flat,
        },
    );
}

/// Enrich results with module/relative_path metadata, apply `--module` /
/// `--source-set` / `--limit` filters. Always enriches when `--relative`,
/// `--module`, or `--source-set` is requested; when none of those is set we
/// still enrich because JSON callers benefit from the extra fields at near-zero
/// cost.
fn apply_filters(
    mut results: Vec<CliResult>,
    root: &Path,
    filters: &ResultFilters,
) -> Vec<CliResult> {
    for r in &mut results {
        r.enrich_with_root(root);
    }
    if let Some(needle) = filters.module.as_deref() {
        results.retain(|r| r.module.as_deref().is_some_and(|m| m.contains(needle)));
    }
    if !filters.kinds.is_empty() {
        results.retain(|r| filters.kinds.iter().any(|k| r.kind.eq_ignore_ascii_case(k)));
    }
    if !filters.source_sets.is_empty() {
        results.retain(|r| {
            r.source_set
                .as_deref()
                .is_some_and(|s| filters.source_sets.iter().any(|wanted| wanted == s))
        });
    }
    if let Some(limit) = filters.limit {
        results.truncate(limit);
    }
    results
}

async fn run_hover(
    root: &Path,
    mode: Mode,
    json: bool,
    verbose: bool,
    file: &Path,
    line: u32,
    col: u32,
) {
    if effective_mode(mode, root, "hover", verbose) == Mode::Fast {
        eprintln!("hover requires index; run `kotlin-lsp index` first or remove --fast");
        std::process::exit(1);
    }
    let index = build_index(root, false).await;
    let Some(text) = hover_at(&index, file, line, col) else {
        eprintln!("No symbol found at {}:{}:{}", file.display(), line, col);
        std::process::exit(1);
    };
    if json {
        let object = serde_json::json!({ "signature": text });
        println!("{}", serde_json::to_string(&object).unwrap_or_default());
    } else {
        println!("{text}");
    }
}

async fn run_complete(
    root: &Path,
    json: bool,
    verbose: bool,
    file: &Path,
    line: u32,
    col: u32,
    no_stdlib: bool,
) {
    if verbose {
        if no_stdlib {
            eprintln!("Loading workspace index (--no-stdlib, skipping ~/.kotlin-lsp/sources)...");
        } else {
            eprintln!("Loading index for completion...");
        }
    }
    let index = build_index(root, no_stdlib).await;
    let rows = completions_at(&index, file, line, col);
    if rows.is_empty() {
        eprintln!("No completions at {}:{}:{}", file.display(), line, col);
        std::process::exit(1);
    }
    if json {
        let arr: Vec<_> = rows
            .iter()
            .map(|r| {
                let mut obj = serde_json::json!({
                    "label": r.label,
                    "kind": r.kind,
                });
                if !r.detail.is_empty() {
                    obj["detail"] = serde_json::Value::String(r.detail.clone());
                }
                if let Some(ref import) = r.import {
                    obj["import"] = serde_json::Value::String(import.clone());
                }
                obj
            })
            .collect();
        println!("{}", serde_json::to_string(&arr).unwrap_or_default());
    } else {
        // Tab-separated: label \t kind \t detail \t import. Empty fields are
        // emitted as empty cells so column count stays stable — easy to split
        // with `cut -f1` etc. No padding (token cost).
        for row in &rows {
            let import = row.import.as_deref().unwrap_or("");
            println!("{}\t{}\t{}\t{}", row.label, row.kind, row.detail, import);
        }
        eprintln!("({} items)", rows.len());
    }
}

fn run_tokens(
    json: bool,
    file: &Path,
    index: Option<&Arc<Indexer>>,
    cst_only: bool,
    phases: bool,
    show_tree: bool,
) {
    if phases {
        match token_rows_phases(file, index) {
            Ok(output) => print!("{output}"),
            Err(error) => {
                eprintln!("error: {error}");
                std::process::exit(1);
            }
        }
        return;
    }
    match token_rows(file, index, cst_only) {
        Ok(rows) => {
            print_token_rows(&rows, json);
            if show_tree {
                eprintln!();
                if let Err(error) = dump_tree(file) {
                    eprintln!("tree: {error}");
                }
            }
        }
        Err(error) => {
            eprintln!("error: {error}");
            std::process::exit(1);
        }
    }
}

fn run_tree(file: &Path) {
    if let Err(error) = dump_tree(file) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn exit_if_empty(results: &[CliResult], json: bool, message: &str) {
    if results.is_empty() {
        if !json {
            eprintln!("{message}");
        }
        std::process::exit(1);
    }
}

// ── context ───────────────────────────────────────────────────────────────────

pub(crate) fn extract_type_names(sig: &str) -> Vec<String> {
    let mut types = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut word = String::new();
    for ch in sig.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            word.push(ch);
        } else {
            if word.chars().next().is_some_and(|c| c.is_uppercase()) && seen.insert(word.clone()) {
                types.push(word.clone());
            }
            word.clear();
        }
    }
    types
}

async fn run_context(file: &Path, line: u32, col: u32, json: bool, expand: usize) {
    let root = resolve_root_for_file(None, file);
    let index = build_index(&root, false).await;
    let uri = tower_lsp::lsp_types::Url::from_file_path(file).expect("valid file path");

    let word: String = {
        let lines = index.mem_lines_for(uri.as_str());
        lines
            .as_ref()
            .and_then(|l| {
                let li = line.saturating_sub(1) as usize;
                l.get(li).map(|ln| {
                    crate::StrExt::word_at_utf16_col(ln.as_str(), col.saturating_sub(1) as usize)
                })
            })
            .unwrap_or_default()
    };

    if word.is_empty() {
        eprintln!("No symbol at cursor");
        std::process::exit(1);
    }

    if json {
        let locs = index.resolve_symbol(&word, None, &uri);
        let sig = crate::indexer::resolution::resolve_symbol_info(
            index.as_ref(),
            &word,
            None,
            &uri,
            crate::indexer::resolution::SubstitutionContext::None,
            &crate::indexer::resolution::ResolveOptions::hover(),
        )
        .map(|s| {
            serde_json::json!({
                "signature": s.signature,
                "doc": s.doc,
                "deprecated": s.deprecated,
                "visibility": format!("{:?}", s.visibility),
            })
        })
        .unwrap_or_default();
        let output = serde_json::json!({
            "name": word,
            "definitions": locs.iter().map(|l| serde_json::json!({
                "uri": l.uri.to_string(),
                "line": l.range.start.line + 1,
                "col": l.range.start.character + 1,
            })).collect::<Vec<_>>(),
            "signature_markdown": sig,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("serialize JSON")
        );
    } else {
        println!("## Symbol: `{word}`");
        let locs = index.resolve_symbol(&word, None, &uri);
        if locs.is_empty() {
            println!("  (not found)");
        } else {
            for loc in &locs {
                println!(
                    "  Def: {}:{}:{}",
                    loc.uri,
                    loc.range.start.line + 1,
                    loc.range.start.character + 1
                );
            }
        }
        if let Some(info) = crate::indexer::resolution::resolve_symbol_info(
            index.as_ref(),
            &word,
            None,
            &uri,
            crate::indexer::resolution::SubstitutionContext::None,
            &crate::indexer::resolution::ResolveOptions::hover(),
        ) {
            println!("  Sig: {}", info.signature);
            if !info.doc.is_empty() {
                let first: String = info
                    .doc
                    .lines()
                    .take_while(|l| !l.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                println!("  Doc: {first}");
            }
        }

        if expand > 0 {
            if let Some(info) = crate::indexer::resolution::resolve_symbol_info(
                index.as_ref(),
                &word,
                None,
                &uri,
                crate::indexer::resolution::SubstitutionContext::None,
                &crate::indexer::resolution::ResolveOptions::hover(),
            ) {
                let types = extract_type_names(&info.signature);
                if !types.is_empty() {
                    println!("  Types ({})", types.len());
                    for t in types.iter().take(10) {
                        if let Some(ti) = crate::indexer::resolution::resolve_symbol_info(
                            index.as_ref(),
                            t,
                            None,
                            &uri,
                            crate::indexer::resolution::SubstitutionContext::None,
                            &crate::indexer::resolution::ResolveOptions::hover(),
                        ) {
                            println!("  {}: {}", t, ti.signature);
                        }
                    }
                }
            }
        }
        println!();
    }
}

// ── call-hierarchy ────────────────────────────────────────────────────────────

async fn run_call_hierarchy(
    file: &Path,
    line: u32,
    col: u32,
    incoming: bool,
    outgoing: bool,
    json: bool,
) {
    let root = resolve_root_for_file(None, file);
    let index = build_index(&root, false).await;
    let uri = tower_lsp::lsp_types::Url::from_file_path(file).expect("valid file path");

    let word: String = {
        let lines = index.mem_lines_for(uri.as_str());
        lines
            .as_ref()
            .and_then(|l| {
                let li = line.saturating_sub(1) as usize;
                l.get(li).map(|ln| {
                    crate::StrExt::word_at_utf16_col(ln.as_str(), col.saturating_sub(1) as usize)
                })
            })
            .unwrap_or_default()
    };

    if word.is_empty() {
        eprintln!("No symbol at cursor");
        std::process::exit(1);
    }

    let matcher = index
        .ignore_matcher
        .read()
        .expect("ignore_matcher lock")
        .clone();

    if json {
        let incoming_results = if incoming {
            find_callers_via_rg(&word, &root, matcher.as_deref())
        } else {
            vec![]
        };
        let output = serde_json::json!({
            "name": word,
            "incoming": incoming_results,
            "outgoing": serde_json::json!([]),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("serialize JSON")
        );
    } else {
        println!("## Call hierarchy for `{word}`\n");
        if incoming {
            println!("### Incoming calls (rg-based callers)");
            let callers = find_callers_via_rg(&word, &root, matcher.as_deref());
            if callers.is_empty() {
                println!("  (none)\n");
            } else {
                for caller in &callers {
                    println!("  - {}", caller);
                }
                println!();
            }
        }
        if outgoing {
            println!("### Outgoing calls");
            println!("  (not yet implemented)\n");
        }
    }
}

/// Use rg to find functions that call `name`.
fn find_callers_via_rg(
    name: &str,
    root: &Path,
    _matcher: Option<&crate::rg::IgnoreMatcher>,
) -> Vec<String> {
    use std::process::Command;
    let escaped = crate::rg::regex_escape(name);
    let mut cmd = Command::new("rg");
    cmd.args(["--no-heading", "--with-filename", "-n"]);
    for ext in crate::rg::SOURCE_EXTENSIONS {
        cmd.args(["--glob", &format!("*.{ext}")]);
    }
    cmd.args(["-e", &escaped]);
    cmd.arg(root);
    let out = match cmd.output() {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|s| s.to_owned())
        .collect()
}

// ── type-hierarchy ────────────────────────────────────────────────────────────

async fn run_type_hierarchy(name: &str, subtypes: bool, supertypes: bool, json: bool) {
    let root = resolve_root(None);
    let index = build_index(&root, false).await;

    // Collect supertypes by scanning the definitions index.
    let mut super_list: Vec<(String, tower_lsp::lsp_types::Location)> = Vec::new();
    if let Some(locs) = index.definitions.get(name) {
        for loc in locs.iter() {
            if let Some(data) = index.files.get(loc.uri.as_str()) {
                for sym in &data.symbols {
                    if sym.selection_start() == loc.range.start.line {
                        for (_, sn, _) in &data.supers {
                            super_list.push((sn.clone(), loc.clone()));
                        }
                        break;
                    }
                }
            }
        }
    }

    if json {
        let mut output = serde_json::json!({"name": name});
        if subtypes {
            let subs: Vec<serde_json::Value> = index
                .subtypes
                .get(name)
                .map(|locs| {
                    locs.iter()
                        .map(|l| {
                            serde_json::json!({
                                "uri": l.uri.to_string(),
                                "line": l.range.start.line + 1,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            output["subtypes"] = serde_json::json!(subs);
        }
        if supertypes {
            let supers: Vec<serde_json::Value> = super_list
                .iter()
                .map(|(n, l)| {
                    serde_json::json!({
                        "name": n,
                        "uri": l.uri.to_string(),
                        "line": l.range.start.line + 1,
                    })
                })
                .collect();
            output["supertypes"] = serde_json::json!(supers);
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&output).expect("serialize JSON")
        );
    } else {
        println!("## Type hierarchy for `{name}`\n");
        if subtypes {
            println!("### Subtypes");
            if let Some(locs) = index.subtypes.get(name) {
                for loc in locs.iter() {
                    let subtype_name = index
                        .files
                        .get(loc.uri.as_str())
                        .and_then(|f| {
                            f.symbols
                                .iter()
                                .find(|s| s.selection_start() == loc.range.start.line)
                                .map(|s| s.name.clone())
                        })
                        .unwrap_or_else(|| "?".to_owned());
                    println!(
                        "  - {} ({}:{})",
                        subtype_name,
                        loc.uri,
                        loc.range.start.line + 1
                    );
                }
            } else {
                println!("  (none)");
            }
            println!();
        }
        if supertypes {
            println!("### Supertypes");
            if super_list.is_empty() {
                println!("  (none)");
            } else {
                for (super_name, loc) in &super_list {
                    println!(
                        "  - {super_name} ({}:{})",
                        loc.uri,
                        loc.range.start.line + 1
                    );
                }
            }
            println!();
        }
    }
}

#[cfg(test)]
#[path = "run_tests.rs"]
mod tests;

// ── Mode resolution ───────────────────────────────────────────────────────────

fn effective_mode(requested: Mode, root: &Path, subcommand: &str, verbose: bool) -> Mode {
    match requested {
        Mode::Fast => Mode::Fast,
        Mode::Smart => {
            if !cache_exists(root) {
                eprintln!(
                    "error: --smart requires a pre-built index. \
                     Run `kotlin-lsp index` first."
                );
                std::process::exit(1);
            }
            Mode::Smart
        }
        Mode::Auto => {
            if cache_exists(root) {
                Mode::Smart
            } else {
                if subcommand == "hover" {
                    // hover can't work without index; report clearly
                    return Mode::Smart; // will build index
                }
                if verbose {
                    eprintln!(
                        "note: no index cache found for {}; using rg/fd (fast mode). \
                         Run `kotlin-lsp index` for precise results.",
                        root.display()
                    );
                }
                Mode::Fast
            }
        }
    }
}
