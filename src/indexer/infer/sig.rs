//! Pure signature-extraction helpers for the Kotlin indexer.
//!
//! All public items are `pub(crate)`.  Most functions take pure string/slice
//! inputs or an immutable `&Indexer` reference for index look-ups.  Note that
//! `find_fun_signature_full` may trigger on-demand indexing via interior
//! mutability (`index_content`) and perform disk reads when a symbol is not yet
//! in the in-memory index.

use tower_lsp::lsp_types::{SymbolKind, Url};

use crate::indexer::Indexer;
use crate::types::SymbolEntry;

// ─── Call-site resolution types ──────────────────────────────────────────────

/// Maximum number of lines to scan when collecting a multi-line function signature.
/// The scope in which a function call is being resolved.
///
/// `SameFile` resolution does not filter nested classes — calling `Foo()` in the
/// file where `class Outer { data class Foo(...) }` is defined is valid.
///
/// `CrossFile` resolution skips `CLASS`/`STRUCT` symbols whose `container` is
/// non-`None` — an unqualified `Foo()` in another file cannot reach `Outer.Foo`
/// without a qualifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionScope {
    SameFile,
    CrossFile,
}

const SIGNATURE_SCAN_LINES: usize = 15;

/// Maximum number of lines to scan when collecting a function parameter list.
const FUN_PARAMS_SCAN_LINES: usize = 20;

// ─── Import reachability ─────────────────────────────────────────────────────

/// Check whether symbols from `def_uri` are reachable from `caller_uri`.
///
/// A definition file is reachable when any of the following hold:
/// - Both files share the same package declaration (same-package access).
/// - The caller has an explicit import `package.SymbolName`.
/// - The caller has a star import `package.*` that covers the definition's package.
///
/// This is best-effort, not sound — we have no full classpath.  When
/// reachability cannot be determined (missing `FileData`, no package info) we
/// return `true` to avoid false negatives.
pub(crate) fn is_import_reachable(
    idx: &Indexer,
    caller_uri: &str,
    def_uri: &str,
    symbol_name: &str,
) -> bool {
    if caller_uri == def_uri {
        return true;
    }
    let Some(caller_data) = idx.files.get(caller_uri) else {
        return true; // fail-open
    };
    let Some(def_data) = idx.files.get(def_uri) else {
        return true; // fail-open
    };
    let def_pkg = match def_data.package.as_deref() {
        Some(package) => package,
        None => return true, // no package info → can't filter
    };
    let caller_pkg = caller_data.package.as_deref().unwrap_or("");

    // Same package — always reachable
    if caller_pkg == def_pkg {
        return true;
    }

    for import in &caller_data.imports {
        if import.covers(def_pkg, symbol_name) {
            return true;
        }
    }

    // Kotlin/JVM default imports — always reachable without explicit import.
    matches!(
        def_pkg,
        "kotlin"
            | "kotlin.annotation"
            | "kotlin.collections"
            | "kotlin.comparisons"
            | "kotlin.io"
            | "kotlin.ranges"
            | "kotlin.sequences"
            | "kotlin.text"
            | "java.lang"
    )
}

/// Collect a human-readable function/class signature starting at `start_line`.
///
/// Rules:
/// - Track `(` / `)` depth.
/// - Once depth is back to 0, the signature ends at the current line.
/// - A line ending with `{` signals the start of the body — strip the `{`
///   and stop (we don't want the body in the hover).
/// - Lines ending with `,` inside balanced parens always continue.
/// - Cap at 15 lines to avoid runaway on pathological files.
pub(crate) fn collect_signature(lines: &[String], start_line: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut depth: i32 = 0;

    for raw_line in lines
        [start_line.min(lines.len())..(start_line + SIGNATURE_SCAN_LINES).min(lines.len())]
        .iter()
    {
        let raw = raw_line.trim();

        // Count parens in this line.
        for ch in raw.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                _ => {}
            }
        }

        if raw.ends_with('{') {
            // Body starts — include the line without the brace (shows inheritance).
            let trimmed = raw.trim_end_matches('{').trim_end();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_owned());
            }
            break;
        }

        parts.push(raw.to_owned());

        // Signature ends when parens are balanced and the line doesn't
        // look like a continuation (trailing comma means more params follow).
        if depth <= 0 && !raw.ends_with(',') {
            break;
        }
    }

    parts.join("\n")
}

// ─── Parameter-list extraction ────────────────────────────────────────────────

/// Collect the full parameter-list text for a function named `fn_name`.
/// Fast path only — no rg, no disk I/O, no index mutations.
/// Used by signature help (fires on every keystroke).
fn find_fun_signature(fn_name: &str, idx: &Indexer, uri: &Url) -> Option<String> {
    // 1. Import-aware resolution using only already-indexed files (no rg/disk).
    if let Some(params) = sig_step_import_aware(fn_name, idx, uri) {
        return Some(params);
    }

    // 2. Fallback: current file, then other workspace files that declare `fn_name`.
    //    The text scan only ever matches a *workspace* function (library params come
    //    from indexed symbol detail in step 1, or the JAR fallback in step 3), so the
    //    helper scopes to workspace defs and caps the scan. For stdlib/library callees
    //    (`let`, `forEachIndexed`, Compose builders) `definitions` has no entry, so
    //    this is skipped entirely.
    if let Some(sig) = collect_fun_params_text(fn_name, uri.as_str(), idx) {
        return Some(sig);
    }
    if let Some(sig) = idx.find_in_workspace_defs(fn_name, |loc| {
        let def_uri = loc.uri.as_str();
        if def_uri == uri.as_str() {
            return None;
        }
        collect_fun_params_text(fn_name, def_uri, idx)
    }) {
        return Some(sig);
    }

    // 3. JAR fallback.
    sig_step_jar(fn_name, idx)
}

/// Step 1 of `find_fun_signature`: resolve `fn_name` through imports/package/etc.
/// (no rg) and read params from the matching indexed symbol's `detail`.
fn sig_step_import_aware(fn_name: &str, idx: &Indexer, uri: &Url) -> Option<String> {
    for loc in idx
        .resolve_symbol_no_rg(fn_name, uri)
        .iter()
        .take(crate::indexer::MAX_BY_NAME_DEFS)
    {
        if let Some(data) = idx.files.get(loc.uri.as_str()) {
            let start_line = loc.range.start.line;
            if let Some(symbol_entry) = data.symbols.iter().find(|s| {
                s.name == fn_name
                    && s.range.start.line == start_line
                    && (s.kind == SymbolKind::FUNCTION || s.kind == SymbolKind::METHOD)
            }) {
                if let Some(params) = extract_params_from_detail(&symbol_entry.detail) {
                    return Some(params);
                }
            }
        }
    }
    None
}

/// Step 3 of `find_fun_signature`: JAR fallback. Sidecar symbols carry params in
/// their `detail` string (e.g. "fun <T> ImmutableList<T>.fastForEach(action: (T) -> Unit)").
fn sig_step_jar(fn_name: &str, idx: &Indexer) -> Option<String> {
    // Promote-before-read (zero budget): signature inference runs per-callee
    // on the inlay/hover hot path — no blocking sidecar IPC here.
    let mut cache_backed_only = 0usize;
    crate::indexer::jar::ensure_jar_definitions_for(idx, fn_name, &mut cache_backed_only);
    let jar_locs = idx.jar_definitions.get(fn_name)?;
    for loc in jar_locs.iter() {
        if let Some(file_data) = idx.jar_files.get(loc.uri.as_str()) {
            // JAR symbols use a synthetic line == index into `symbols`, so address the
            // entry directly. A linear `.iter().find()` here scans the whole jar's
            // symbol list (100k+ for a large AAR) per callee — which made inlay/hover
            // inference take seconds once the gradle JARs were indexed.
            if let Some(jar_symbol_entry) = file_data
                .symbols
                .get(loc.range.start.line as usize)
                .filter(|s| s.name == fn_name && !s.detail.is_empty())
            {
                if let Some(params) = extract_params_from_detail(&jar_symbol_entry.detail) {
                    return Some(params);
                }
            }
        }
    }
    None
}

/// Full signature lookup including rg + on-demand indexing.
/// Used by hover and lambda type inference where latency is acceptable.
pub(crate) fn find_fun_signature_full(fn_name: &str, idx: &Indexer, uri: &Url) -> Option<String> {
    let cache_key = (fn_name.to_owned(), uri.to_string());
    if let Some(cached) = idx.sig_cache.get(&cache_key) {
        return cached.clone();
    }
    let result = find_fun_signature_full_uncached(fn_name, idx, uri);
    idx.sig_cache.insert(cache_key, result.clone());
    result
}

/// Index-only function-signature lookup for hot inference paths (lambda receiver
/// resolution): resolve the name via imports/package/jar (no rg, no all-files scan)
/// and read the resolved symbol's parameter list from its `detail`. Returns `None`
/// quickly when the callee isn't indexed (e.g. a stdlib `let`/`forEachIndexed`),
/// which is exactly the common case the slow path used to spawn `rg` for.
pub(crate) fn find_fun_params_text_fast(fn_name: &str, idx: &Indexer, uri: &Url) -> Option<String> {
    let key = (fn_name.to_owned(), uri.to_string());
    if let Some(cached) = idx.sig_fast_cache.get(&key) {
        return cached.clone();
    }
    // Index-only (imports → current file → declaring workspace files → JAR); crucially
    // NO rg like find_fun_signature_full. The result is memoized in sig_fast_cache so
    // recursive lambda/receiver inference resolves each callee at most once (and across
    // requests, until the cache is invalidated on reindex).
    let result = find_fun_signature(fn_name, idx, uri);
    idx.sig_fast_cache.insert(key, result.clone());
    result
}

fn find_fun_signature_full_uncached(fn_name: &str, idx: &Indexer, uri: &Url) -> Option<String> {
    if let Some(sig) = find_fun_signature(fn_name, idx, uri) {
        return Some(sig);
    }
    // Slow path: rg to locate the definition, index on-demand.
    let open_file = uri.to_file_path().ok();
    let (root, source_roots, matcher) = idx.rg_scope_for_path(open_file.as_deref());
    let locs =
        crate::rg::rg_find_definition(fn_name, root.as_deref(), &source_roots, matcher.as_deref());
    for loc in &locs {
        let file_uri_str = loc.uri.as_str();
        if !idx.files.contains_key(file_uri_str) {
            if let Ok(path) = loc.uri.to_file_path() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    idx.index_content(&loc.uri, &content);
                }
            }
        }
        if let Some(sig) = collect_fun_params_text(fn_name, file_uri_str, idx) {
            return Some(sig);
        }
    }
    None
}

/// Collect everything between the outer `(…)` of a function's parameter list.
/// Scans the symbol's start line and up to 20 following lines.
/// Matches both top-level `fun` (FUNCTION) and class methods (METHOD).
pub(crate) fn collect_fun_params_text(
    fn_name: &str,
    uri_str: &str,
    idx: &Indexer,
) -> Option<String> {
    collect_all_fun_params_texts(fn_name, uri_str, idx)
        .into_iter()
        .next()
}

/// Like `collect_fun_params_text` but returns ALL params texts for every symbol
/// named `fn_name` in the file (a file may have multiple same-named nested classes).
///
/// For Java files, CLASS symbols are excluded because Java class nodes never carry
/// constructor params — use the indexed CONSTRUCTOR symbols instead.
pub(crate) fn collect_all_fun_params_texts(
    fn_name: &str,
    uri_str: &str,
    idx: &Indexer,
) -> Vec<String> {
    let data = match idx.files.get(uri_str) {
        Some(d) => d,
        None => return vec![],
    };
    let is_java = uri_str.ends_with(".java");
    data.symbols
        .iter()
        .filter(|s| {
            s.name == fn_name
                && (s.kind == SymbolKind::FUNCTION
                    || s.kind == SymbolKind::METHOD
                    || s.kind == SymbolKind::CONSTRUCTOR
                    || s.kind == SymbolKind::STRUCT
                    || (s.kind == SymbolKind::CLASS && !is_java))
        })
        .filter_map(|s| {
            // Prefer pre-computed params from CST (populated at index time)
            if !s.params.is_empty() {
                return Some(s.params.clone());
            }
            // Fallback: extract from detail string, then line scan
            extract_params_from_detail(&s.detail)
                .or_else(|| collect_params_from_line(&data.lines, s.range.start.line as usize))
        })
        .collect()
}

/// Extract the parameter text from a CST-derived `detail` string.
///
/// Given `"fun foo(x: Int, y: String): Boolean"`, returns `"x: Int, y: String"`.
/// Given `"fun bar()"`, returns `None` (empty params = no required args).
/// Returns `None` if the detail is truncated (no matching `)` found).
///
/// Skips any leading annotation arguments (e.g., `@Deprecated("x") class Foo(params)`)
/// by starting the search for `(` only AFTER a declaration keyword is found.
pub(crate) fn extract_params_from_detail(detail: &str) -> Option<String> {
    if detail.is_empty() {
        return None;
    }
    // Skip annotation preamble by finding the first declaration keyword and
    // searching for `(` only after it.  This prevents `@Deprecated("x")` from
    // being mistaken for the constructor parameter list.
    const KEYWORDS: &[&str] = &["fun ", "fun<", "class ", "constructor"];
    let search_from = KEYWORDS
        .iter()
        .filter_map(|kw| detail.find(kw).map(|p| p + kw.len()))
        .min()
        .unwrap_or(0);
    let open_pos = detail[search_from..].find('(').map(|p| search_from + p)?;
    let mut depth: i32 = 0;
    let mut end_pos = None;
    for (i, ch) in detail[open_pos..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end_pos = Some(open_pos + i);
                    break;
                }
            }
            _ => {}
        }
    }
    let close_pos = end_pos?;
    let inner = &detail[open_pos + 1..close_pos];
    let trimmed = inner.trim();
    // Return Some("") for zero-param functions — distinct from None (couldn't parse).
    // This ensures 0-param overloads are visible for overload detection.
    Some(trimmed.to_string())
}

/// Walk forward from `start_line`, accumulating characters until the outermost
/// `)` closes — that ends the parameter list.
///
/// We only track `()` depth (NOT `<>`) to avoid false-triggers on `->` arrows.
/// Skips annotation lines (`@Foo(...)`) before the `fun`/`class` keyword to avoid
/// parsing annotation arguments as function parameters.
pub(crate) fn collect_params_from_line(lines: &[String], start_line: usize) -> Option<String> {
    let end_line = start_line + FUN_PARAMS_SCAN_LINES;
    let decl_start = skip_annotation_lines(lines, start_line, end_line)?;
    extract_balanced_parens(lines, decl_start, end_line)
}

/// Returns the first line index in `[start, end)` that is NOT a pure annotation
/// (starts with `@` but contains none of `fun`/`class`/`constructor`).
/// Returns `None` when all lines in the window are pure annotations — avoids
/// `extract_balanced_parens` re-scanning them and misreading `@Anno(arg)` as params.
fn skip_annotation_lines(lines: &[String], start: usize, end: usize) -> Option<usize> {
    for ln in start..end {
        let trimmed = lines.get(ln)?.trim();
        if !is_pure_annotation_line(trimmed) {
            return Some(ln);
        }
    }
    None
}

fn is_pure_annotation_line(trimmed: &str) -> bool {
    trimmed.starts_with('@')
        && !trimmed.contains(" fun ")
        && !trimmed.contains(" fun<")
        && !trimmed.contains(" class ")
        && !trimmed.contains(" constructor")
}

fn has_declaration_keyword(trimmed: &str) -> bool {
    trimmed.starts_with("fun ")
        || trimmed.starts_with("fun<")
        || trimmed.contains(" fun ")
        || trimmed.contains(" fun<")
        || trimmed.starts_with("class ")
        || trimmed.starts_with("constructor")
        || trimmed.contains(" class ")
        || trimmed.contains(" constructor")
}

/// Scan `lines[start..end]`, collecting the content between the outermost `(…)`.
///
/// `{` encountered before any `(` on a keyword line signals a zero-arg class or
/// object declaration → `Some("")`.  The `end` bound preserves the original
/// `FUN_PARAMS_SCAN_LINES` window even when `start` was advanced by annotation skipping.
fn extract_balanced_parens(lines: &[String], start: usize, end: usize) -> Option<String> {
    let mut depth: i32 = 0;
    let mut found_open = false;
    let mut found_keyword = false;
    let mut params = String::new();

    'outer: for ln in start..end {
        let line = match lines.get(ln) {
            Some(l) => l,
            None => break,
        };
        if !found_keyword && has_declaration_keyword(line.trim()) {
            found_keyword = true;
        }
        for ch in line.chars() {
            match ch {
                '{' if !found_open && found_keyword => return Some(String::new()),
                '(' => {
                    depth += 1;
                    if depth == 1 {
                        found_open = true;
                        continue;
                    }
                    if found_open {
                        params.push(ch);
                    }
                }
                ')' => {
                    depth -= 1;
                    if found_open && depth == 0 {
                        break 'outer;
                    }
                    if found_open {
                        params.push(ch);
                    }
                }
                _ if found_open => params.push(ch),
                _ => {}
            }
        }
        if found_open {
            params.push('\n');
        }
    }

    if params.is_empty() {
        None
    } else {
        Some(params)
    }
}

// ─── Parameter type accessors ─────────────────────────────────────────────────

/// Split `text` at top-level commas (depth 0), skipping commas inside `()`, `<>`, `[]`, `{}`.
/// The `->` Kotlin function-type arrow is handled: `>` preceded by `-` is NOT a closing
/// generic delimiter.
/// `{}` are tracked so that lambda defaults like `= { a, b -> a }` are not split at
/// the comma inside the lambda body.
///
/// Returns the raw slices between commas; does NOT trim or filter empty parts.
pub(crate) fn split_params_at_depth_zero(text: &str) -> Vec<&str> {
    let mut parts: Vec<&str> = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0;
    let mut prev = '\0';
    for (i, ch) in text.char_indices() {
        match ch {
            '(' | '<' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '>' if prev != '-' && depth > 0 => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        prev = ch;
    }
    parts.push(&text[start..]);
    parts
}

/// Split the flattened parameter list by `,` at depth-0 (respecting `()`, `<>`).
/// Returns the type string of the parameter at position `n` (0-based).
/// Falls back to the last parameter if `n` is out of range.
///
/// NOTE: `->` in Kotlin functional types (e.g. `(Boolean) -> Flow<T>`) contains
/// `>` which would falsely decrement `<>` depth.  We skip the `>` of any `->` by
/// tracking the previous character.
pub(crate) fn nth_fun_param_type_str(params_text: &str, n: usize) -> Option<String> {
    let mut parts = split_params_at_depth_zero(params_text);
    // Drop trailing-comma empty parts (Kotlin allows `fun f(a: A, b: B,) {}`).
    parts.retain(|p| !p.trim().is_empty());
    if parts.is_empty() {
        return None;
    }

    let param = parts.get(n).unwrap_or_else(|| parts.last().unwrap()).trim();
    // Strip leading non-identifier characters (annotations, whitespace).
    let param = param.trim_start_matches(|c: char| !c.is_alphanumeric() && c != '_');
    let colon = param.find(':')?;
    Some(param[colon + 1..].trim().to_owned())
}

/// Return the type string of the last parameter in `params_text`.
pub(crate) fn last_fun_param_type_str(params_text: &str) -> Option<String> {
    let count = split_params_at_depth_zero(params_text)
        .iter()
        .filter(|p| !p.trim().is_empty())
        .count();
    nth_fun_param_type_str(params_text, count.saturating_sub(1))
}

// ─── Pure string helper ───────────────────────────────────────────────────────

/// Strip a balanced trailing `(…)` argument list from the end of `s`.
///
/// `"collection.method(arg1, arg2)"` → `"collection.method"`
/// `"collection.forEach"`           → `"collection.forEach"`  (unchanged)
pub(crate) fn strip_trailing_call_args(s: &str) -> &str {
    if !s.ends_with(')') {
        return s;
    }
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth -= 1;
                if depth == 0 {
                    return &s[..i];
                }
            }
            _ => {}
        }
    }
    s
}

// ─── Receiver-aware signature lookup ─────────────────────────────────────────

/// `(required, total)` counts of a declaration. Compiled-JAR (sidecar)
/// symbols carry parameter types but no default-value markers, so their
/// `required` is not trusted and clamps to 0; sources-JAR and workspace files
/// are parsed and carry real defaults.
pub(crate) fn trusted_param_counts(symbol: &SymbolEntry, is_compiled_jar: bool) -> (u8, u8) {
    if is_compiled_jar {
        (0, symbol.param_counts.1)
    } else {
        symbol.param_counts
    }
}

/// Whether a signature could take the call at all: its arity admits the
/// written arguments, and a trailing lambda lands on a function-typed last
/// parameter.
pub(crate) fn candidate_applies_to_call(
    params_text: &str,
    (required, total): (u8, u8),
    shape: super::deps::CallShape,
) -> bool {
    let lambda_fits = !shape.trailing_lambda
        || last_fun_param_type_str(params_text)
            .map(|last_param| is_lambda_shaped(&last_param))
            .unwrap_or(false);
    shape.accepts(required, total) && lambda_fits
}

/// Choose the candidate whose signature best matches how the call was
/// actually written, rather than the first one found.
///
/// Prefers a candidate that matches both arity and lambda-shape; falls back to
/// lambda-shape alone (the stronger signal — same-arity ties like `Flow`'s two
/// `collect`s are common, and only one of them accepts a trailing lambda), then
/// arity alone, then the first candidate — never worse than the old
/// first-match behaviour.
pub(crate) fn pick_candidate_matching_shape(
    candidates: Vec<(String, (u8, u8))>,
    shape: super::deps::CallShape,
) -> Option<String> {
    let index = best_candidate_index(&candidates, shape)?;
    candidates.into_iter().nth(index).map(|(text, _)| text)
}

/// Index of the candidate [`pick_candidate_matching_shape`] chooses; the
/// declaration at that index is the one a call's generic metadata must come
/// from too.
pub(crate) fn best_candidate_index(
    candidates: &[(String, (u8, u8))],
    shape: super::deps::CallShape,
) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    let scored: Vec<(bool, bool)> = candidates
        .iter()
        .map(|(text, (required, total))| {
            let arity_ok = shape.accepts(*required, *total);
            let lambda_ok = last_fun_param_type_str(text)
                .map(|t| is_lambda_shaped(&t))
                .unwrap_or(false)
                == shape.trailing_lambda;
            (arity_ok, lambda_ok)
        })
        .collect();
    for wants in [(true, true), (false, true), (true, false)] {
        if let Some(index) = scored.iter().position(|&score| score == wants) {
            return Some(index);
        }
    }
    Some(0)
}

/// Whether a parameter type string is a Kotlin function type (`(T) -> R`,
/// `suspend (T) -> R`, `() -> Unit`, …) — i.e. a trailing-lambda call could
/// supply it.
pub(crate) fn is_lambda_shaped(type_str: &str) -> bool {
    let t = type_str.trim();
    let t = t.strip_prefix("suspend").map(str::trim_start).unwrap_or(t);
    let is_receiver_function = super::lambda::lambda_type_receiver(t).is_some();
    (t.starts_with('(') || is_receiver_function) && t.contains("->")
}

pub(crate) fn is_class_like(kind: SymbolKind) -> bool {
    matches!(
        kind,
        SymbolKind::CLASS
            | SymbolKind::INTERFACE
            | SymbolKind::STRUCT
            | SymbolKind::ENUM
            | SymbolKind::OBJECT
    )
}

// ─── Unified call-site resolver ───────────────────────────────────────────────

/// Collect params text and param_counts for callable symbols matching `name`
/// in `file_data`, applying `scope` filtering rules and optional receiver-type
/// filtering.
///
/// `CrossFile` scope: skips `CLASS`/`STRUCT` symbols with a container (nested
/// classes) unless the caller has an explicit import for that symbol by local
/// name, and excludes files not import-reachable from the caller.
///
/// When `receiver_base` is `Some`, further restricts to symbols whose
/// `container` matches the base name (member methods) or whose
/// `extension_receiver` matches (top-level extension functions for that type).
pub(crate) fn collect_params_from_file(
    name: &str,
    file_uri: &str,
    idx: &Indexer,
    caller_uri: &str,
    scope: ResolutionScope,
    receiver_base: Option<&str>,
) -> Vec<(String, (u8, u8))> {
    // Look up data from source files first, then the compiled-JAR cache. Track which:
    // only compiled-JAR (sidecar) symbols lack default-value markers.
    let (data, is_compiled_jar) = if let Some(file_data) = idx.files.get(file_uri) {
        (file_data, false)
    } else if let Some(file_data) = idx.jar_files.get(file_uri) {
        (file_data, true)
    } else {
        return vec![];
    };
    // File-level import reachability gates only UNQUALIFIED cross-file resolution.
    // Qualified (receiver-based) resolution reaches *members* via the receiver type,
    // not via an import (you import the class, never its methods), so don't reject the
    // file here; extension functions still get a per-symbol import check below.
    if scope == ResolutionScope::CrossFile
        && receiver_base.is_none()
        && !is_import_reachable(idx, caller_uri, file_uri, name)
    {
        return vec![];
    }
    // This is only the nested-class allowlist by local name (e.g. `import
    // pkg.Outer.Config`); `is_import_reachable()` still does the full package/path
    // validation via `ImportEntry::covers()`.
    let caller_imports_by_name = if scope == ResolutionScope::CrossFile {
        idx.files
            .get(caller_uri)
            .map(|d| d.imports.iter().any(|i| !i.is_star && i.local_name == name))
            .unwrap_or(true) // fail-open
    } else {
        false
    };
    let is_java = file_uri.ends_with(".java");
    let is_swift = file_uri.ends_with(".swift");

    // Compiled-JAR (sidecar) symbols carry parameter *types* in their signature
    // `detail` but no default-value markers (e.g. `fun WindowInsets(left: Int, …)`
    // even when the real declaration is `left: Int = 0`). So we trust the total
    // arity but not `required` — clamp `required` to 0 to avoid "expected N, found
    // <N" false positives on calls relying on library defaults. This applies ONLY
    // to compiled-JAR symbols: sources-JAR (`jar:…!/….kt`) and `sourcePaths`
    // `SourceSet::Library` files are parsed with tree-sitter and DO carry real
    // defaults, so their `required` is accurate and must be honoured.

    let name_matches = |symbol: &&SymbolEntry| {
        symbol.name == name
            && !(scope == ResolutionScope::CrossFile
                && symbol.container.is_some()
                && matches!(symbol.kind, SymbolKind::CLASS | SymbolKind::STRUCT)
                && !caller_imports_by_name)
            && (symbol.kind == SymbolKind::FUNCTION
                || symbol.kind == SymbolKind::METHOD
                || symbol.kind == SymbolKind::CONSTRUCTOR
                || symbol.kind == SymbolKind::STRUCT
                || (symbol.kind == SymbolKind::CLASS && !is_java))
    };

    let receiver_matches = |symbol: &&SymbolEntry| match receiver_base {
        None => true,
        Some(base) => {
            if symbol.container.as_deref() == Some(base) {
                // Member of the receiver type — reachable via the receiver instance,
                // no import needed.
                true
            } else if symbol.container.is_none() && symbol.extension_receiver() == base {
                // Extension function — must be import-reachable (or same package),
                // just like a top-level function.
                scope != ResolutionScope::CrossFile
                    || is_import_reachable(idx, caller_uri, file_uri, name)
            } else {
                false
            }
        }
    };

    data.symbols
        .iter()
        .filter(name_matches)
        .filter(receiver_matches)
        .flat_map(|symbol| -> Vec<(String, (u8, u8))> {
            // Swift structs/classes never carry constructor params on their own
            // CST node (no Kotlin-style inline primary constructor) — the real
            // signature lives on a nested `init` (explicit or synthesized at
            // parse time; see `synthesize_swift_implicit_init`). Resolve through
            // those instead of the type's own (always-empty) params/counts.
            // Emitting one candidate per `init` lets the caller's arity-envelope
            // dedup correctly detect an overloaded initializer as ambiguous.
            if is_swift && matches!(symbol.kind, SymbolKind::STRUCT | SymbolKind::CLASS) {
                return data
                    .symbols
                    .iter()
                    .filter(|c| {
                        c.kind == SymbolKind::CONSTRUCTOR
                            && c.container.as_deref() == Some(symbol.name.as_str())
                    })
                    .map(|c| (c.params.clone(), c.param_counts))
                    .collect();
            }
            let params_text = match extract_params_or_fallback(symbol, &data.lines) {
                Some(text) => text,
                None => return vec![],
            };
            vec![(params_text, trusted_param_counts(symbol, is_compiled_jar))]
        })
        .collect()
}

/// Extract a symbol's parameter text, falling back to detail/line scanning,
/// then to an explicit `()` for a genuine 0-arg constructor.
pub(crate) fn extract_params_or_fallback(s: &SymbolEntry, lines: &[String]) -> Option<String> {
    if !s.params.is_empty() {
        return Some(s.params.clone());
    }
    extract_params_from_detail(&s.detail)
        .or_else(|| collect_params_from_line(lines, s.range.start.line as usize))
        .or_else(|| {
            // 0-arg constructors (e.g. `class Foo`) may lack explicit `()`.
            if s.param_counts == (0, 0) {
                Some(String::new())
            } else {
                None
            }
        })
}

#[cfg(test)]
#[path = "sig_tests.rs"]
mod tests;
