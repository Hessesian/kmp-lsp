use tower_lsp::lsp_types::{Position, SymbolKind, Url};

use crate::indexer::Indexer;
use crate::types::FileData;
use crate::LinesExt;
use crate::StrExt;

use super::ensure_file_data;
use super::infer_lines::{
    extract_property_type_from_detail, extract_return_type_from_detail, find_rhs_str,
    has_dot_after_first_call,
};

// ─── InferenceChain trait ─────────────────────────────────────────────────────

/// Capability trait for type-inference queries over an indexed workspace.
///
/// Implemented by [`Indexer`] in production.  Mirrors the shape of
/// [`ResolutionChain`](super::resolve::ResolutionChain) — all methods
/// delegate to the free functions in this module so the trait is a zero-cost
/// façade.
///
/// `#[allow(dead_code)]` is retained until this trait is wired through the
/// resolution pipeline in a future pass (G4).
// TODO(G4): wire trait bound through resolution pipeline to enable test stubs
#[allow(dead_code)]
pub(crate) trait InferenceChain {
    fn infer_variable_type(&self, var_name: &str, uri: &Url) -> Option<String>;
    fn infer_variable_type_raw(&self, var_name: &str, uri: &Url) -> Option<String>;
    fn infer_field_type(&self, file_uri: &str, field_name: &str) -> Option<String>;
    fn find_field_type_in_class(&self, class_name: &str, field_name: &str) -> Option<String>;
    fn find_fun_return_type_by_name(&self, fn_name: &str) -> Option<String>;
    fn find_method_return_type(
        &self,
        type_name: &str,
        method_name: &str,
        from_uri: Option<&Url>,
    ) -> Option<String>;
    fn infer_receiver_type(&self, kind: ReceiverKind<'_>, uri: &Url) -> Option<ReceiverType>;
}

impl InferenceChain for Indexer {
    fn infer_variable_type(&self, var_name: &str, uri: &Url) -> Option<String> {
        infer_variable_type(self, var_name, uri)
    }
    fn infer_variable_type_raw(&self, var_name: &str, uri: &Url) -> Option<String> {
        infer_variable_type_raw(self, var_name, uri)
    }
    fn infer_field_type(&self, file_uri: &str, field_name: &str) -> Option<String> {
        infer_field_type(self, file_uri, field_name)
    }
    fn find_field_type_in_class(&self, class_name: &str, field_name: &str) -> Option<String> {
        find_field_type_in_class(self, class_name, field_name)
    }
    fn find_fun_return_type_by_name(&self, fn_name: &str) -> Option<String> {
        find_fun_return_type_by_name(self, fn_name)
    }
    fn find_method_return_type(
        &self,
        type_name: &str,
        method_name: &str,
        from_uri: Option<&Url>,
    ) -> Option<String> {
        find_method_return_type(self, type_name, method_name, from_uri)
    }
    fn infer_receiver_type(&self, kind: ReceiverKind<'_>, uri: &Url) -> Option<ReceiverType> {
        infer_receiver_type(self, kind, uri)
    }
}

// ─── Type-string helpers ──────────────────────────────────────────────────────

/// Strip generic parameters and nullability markers from a type string.
///
/// `"List<Product>"` → `"List"`, `"String?"` → `"String"`, `"Outer.Inner<T>"` → `"Outer.Inner"`
///
/// Mirrors the stripping done by [`infer_type_in_lines`](super::infer_lines::infer_type_in_lines)
/// so that `type_annotations` lookups return the same shape as line-scan results.
fn strip_generics(type_str: &str) -> String {
    let stripped: String = type_str
        .chars()
        .take_while(|&c| c.is_alphanumeric() || c == '_' || c == '.')
        .collect();
    stripped.trim_end_matches('.').to_owned()
}

// ─── Receiver type resolution ─────────────────────────────────────────────────

/// How the receiver expression should be resolved.
///
/// - `Variable`: a named val/var (e.g. `interactor`, `viewModel`).
///   Resolved via line-scan type annotation (`val name: Type`).
/// - `Contextual`: `it`, `this`, or a named lambda parameter.
///   Requires cursor `position` for scope analysis; falls back to
///   `infer_variable_type_raw` only if scope analysis returns nothing.
pub(crate) enum ReceiverKind<'a> {
    Variable(&'a str),
    Contextual { name: &'a str, position: Position },
}

/// A fully-normalised receiver type with multiple access forms.
///
/// All forms are derived from a single raw string (e.g. `"Outer.Inner<Param>"`):
/// - `raw`       — original with generics: `"Outer.Inner<Param>"`
/// - `qualified` — no generics, dots preserved: `"Outer.Inner"`
/// - `outer`     — first dot-segment: `"Outer"`  (used for file lookup)
/// - `leaf`      — last dot-segment: `"Inner"`   (used for fallback member lookup)
#[derive(Clone)]
pub(crate) struct ReceiverType {
    /// Full raw type string as inferred, e.g. `"StateFlow<UiState>?"`.
    pub raw: String,
    /// Type name with no generics and no `?`, e.g. `"StateFlow"` or `"Outer.Inner"`.
    pub qualified: String,
    /// Outermost segment of `qualified`, e.g. `"Outer"`.
    pub outer: String,
    /// Innermost segment of `qualified`, e.g. `"Inner"`.
    pub leaf: String,
    /// Whether the type was annotated as nullable (`?`), e.g. `val x: User?`.
    /// Available for hover/completion display; lookup sites use `qualified`.
    #[allow(dead_code)]
    pub nullable: bool,
}

impl ReceiverType {
    pub(crate) fn from_raw(raw: String) -> Self {
        // Strip generics and outer `?` — stop at first `<` or `?`.
        let qualified: String = raw.chars().take_while(|&c| c != '<' && c != '?').collect();
        let nullable = raw.contains('?');
        let outer = qualified
            .split('.')
            .next()
            .unwrap_or(&qualified)
            .to_string();
        let leaf = qualified
            .rsplit('.')
            .next()
            .unwrap_or(&qualified)
            .to_string();
        ReceiverType {
            raw,
            qualified,
            outer,
            leaf,
            nullable,
        }
    }
}

/// Infer the type of a receiver expression and normalise it into a
/// [`ReceiverType`].
///
/// Returns `None` when type inference fails (no annotation, unindexed file,
/// or lambda scope not resolvable).  Call sites then decide whether to skip
/// or fall back; this function never performs a global rg scan.
pub(crate) fn infer_receiver_type(
    indexer: &Indexer,
    kind: ReceiverKind<'_>,
    uri: &Url,
) -> Option<ReceiverType> {
    let raw = match kind {
        ReceiverKind::Variable(name) => infer_variable_type_raw(indexer, name, uri)?,
        ReceiverKind::Contextual { name, position } => {
            // Lambda / implicit-receiver path.
            if let Some(type_str) = indexer.infer_lambda_param_type_at(name, uri, position) {
                type_str
            } else {
                // Contextual fallback: ordinary annotated var that happens to
                // appear in a lambda context (e.g. captured val with explicit type).
                infer_variable_type_raw(indexer, name, uri)?
            }
        }
    };
    Some(ReceiverType::from_raw(raw))
}

/// Like [`infer_receiver_type`] but checks smart-cast narrowing at the given
/// position first.  If the variable is inside a `when (var) { is Type -> }`
/// branch or an `if (var is Type)` block, returns the narrowed type.
pub(crate) fn infer_receiver_type_at(
    indexer: &Indexer,
    name: &str,
    uri: &Url,
    position: Position,
) -> Option<ReceiverType> {
    // Try smart cast narrowing first when lines are available.
    let lines = indexer
        .live_lines
        .get(uri.as_str())
        .map(|ll| (*ll).clone())
        .or_else(|| indexer.files.get(uri.as_str()).map(|d| d.lines.clone()));
    if let Some(lines) = lines {
        if let Some(narrowed) =
            super::infer_lines::smart_cast_type_at_line(&lines, name, position.line)
        {
            return Some(ReceiverType::from_raw(narrowed));
        }
    }
    // Fallback to normal inference
    infer_receiver_type(indexer, ReceiverKind::Variable(name), uri)
}

/// Scan the current file's lines for a type annotation on `var_name` and return
/// the declared type name if found.  Delegates to [`infer_type_in_lines`] and
/// falls back to method return-type inference for `val x = receiver.method(...)`.
pub(crate) fn infer_variable_type(indexer: &Indexer, var_name: &str, uri: &Url) -> Option<String> {
    infer_variable_type_impl(indexer, var_name, uri, 4)
}

/// Like [`infer_variable_type`] but preserves generic parameters in the returned
/// type string.  e.g. `val items: List<Product>` → `"List<Product>"`.
///
/// Used by the `it`-completion path to extract the collection element type.
pub(crate) fn infer_variable_type_raw(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
) -> Option<String> {
    infer_variable_type_raw_impl(indexer, var_name, uri, 4)
}

fn infer_variable_type_impl(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
) -> Option<String> {
    infer_variable_type_core(indexer, var_name, uri, depth, false)
}

fn infer_variable_type_raw_impl(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
) -> Option<String> {
    infer_variable_type_core(indexer, var_name, uri, depth, true)
}

fn infer_variable_type_core(
    indexer: &Indexer,
    var_name: &str,
    uri: &Url,
    depth: u8,
    keep_generics: bool,
) -> Option<String> {
    if depth == 0 {
        return None;
    }
    let lines = {
        if let Some(ll) = indexer.live_lines.get(uri.as_str()) {
            let result = if keep_generics {
                ll.infer_type_raw(var_name)
            } else {
                ll.infer_type(var_name)
            };
            if result.is_some() {
                return result;
            }
            // Live lines didn't find the type — consult the indexed snapshot.
            // This handles the case where `val x: T` is in a different source
            // section from the live editor content (e.g. sig vs code in tests,
            // or a declaration from a file indexed before the editor opened it).
            if let Some(data) = indexer.files.get(uri.as_str()) {
                if let Some(ann) = data.type_annotations.iter().find(|(_, n, _)| n == var_name) {
                    return Some(if keep_generics {
                        ann.2.clone()
                    } else {
                        strip_generics(&ann.2)
                    });
                }
            }
            (*ll).clone()
        } else if let Some(data) = indexer.files.get(uri.as_str()) {
            if let Some(ann) = data.type_annotations.iter().find(|(_, n, _)| n == var_name) {
                return Some(if keep_generics {
                    ann.2.clone()
                } else {
                    strip_generics(&ann.2)
                });
            }
            let line_result = if keep_generics {
                data.lines.infer_type_raw(var_name)
            } else {
                data.lines.infer_type(var_name)
            };
            if line_result.is_some() {
                return line_result;
            }
            let rhs_match = data
                .rhs_types
                .iter()
                .find(|(_, n, _)| n == var_name)
                .map(|(_, _, type_name)| type_name.clone());
            let method_match = data
                .method_call_rhs
                .iter()
                .find(|(_, n, _, _)| n == var_name)
                .map(|(_, _, recv, method)| (recv.clone(), method.clone()));
            let field_match = data
                .field_access_rhs
                .iter()
                .find(|(_, n, _, _)| n == var_name)
                .map(|(_, _, recv, field)| (recv.clone(), field.clone()));
            let lines = data.lines.clone();
            drop(data);
            if let Some(type_name) = rhs_match {
                return Some(type_name);
            }
            if let Some((recv, method)) = method_match {
                let recv_type =
                    infer_variable_type_core(indexer, &recv, uri, depth - 1, keep_generics);
                if let Some(recv_type) = recv_type {
                    if let Some(ret) =
                        find_method_return_type(indexer, &recv_type, &method, Some(uri))
                    {
                        return Some(ret);
                    }
                }
            }
            if let Some((recv, field)) = field_match {
                let recv_type =
                    infer_variable_type_core(indexer, &recv, uri, depth - 1, keep_generics);
                if let Some(recv_type) = recv_type {
                    let recv_stripped = recv_type.split('<').next().unwrap_or(&recv_type);
                    let recv_base = recv_stripped.rsplit('.').next().unwrap_or(recv_stripped);
                    if let Some(field_type) = find_field_type_in_class(indexer, recv_base, &field) {
                        return Some(field_type);
                    }
                }
            }
            return infer_method_return_type(indexer, var_name, &lines, uri, depth - 1)
                .or_else(|| find_extension_property_type(indexer, var_name, uri));
        } else {
            let path = uri.to_file_path().ok()?;
            let content = std::fs::read_to_string(&path).ok()?;
            let lines: Vec<String> = content.lines().map(String::from).collect();
            return if keep_generics {
                lines.infer_type_raw(var_name)
            } else {
                lines.infer_type(var_name)
            };
        }
    };
    infer_method_return_type(indexer, var_name, &lines, uri, depth - 1)
        .or_else(|| find_extension_property_type(indexer, var_name, uri))
}

/// Scan a specific (possibly un-indexed) file for the declared type of `field_name`.
///
/// Checks CST type annotations first (indexed files), then falls back to line
/// scanning, then reads from disk for un-indexed files.
pub(crate) fn infer_field_type(
    indexer: &Indexer,
    file_uri: &str,
    field_name: &str,
) -> Option<String> {
    let uri = tower_lsp::lsp_types::Url::parse(file_uri).ok()?;
    let file_data = ensure_file_data(indexer, &uri)?;
    if let Some(ann) = file_data
        .type_annotations
        .iter()
        .find(|(_, n, _)| n == field_name)
    {
        return Some(strip_generics(&ann.2));
    }
    file_data.lines.infer_type(field_name)
}

/// Like `infer_field_type` but preserves generic parameters in the result.
///
/// Returns `"MutableList<MbAccount>"` rather than `"MutableList"`, which is
/// needed for collection element type extraction via `extract_collection_element_type`.
/// Checks live editor lines first (most up-to-date), then CST type annotations,
/// then falls back to indexed lines and finally to a disk read for un-indexed files.
pub(crate) fn infer_field_type_raw(
    indexer: &Indexer,
    file_uri: &str,
    field_name: &str,
) -> Option<String> {
    if let Some(live) = indexer.live_lines.get(file_uri) {
        if let Some(result) = live.infer_type_raw(field_name) {
            return Some(result);
        }
        // Fall through — live lines didn't have a type annotation;
        // check the indexed snapshot (indexer.files) which may have declarations
        // from a different source set (e.g. sig vs code in tests, or a file
        // that was indexed before the editor opened it live).
    }
    if let Some(data) = indexer.files.get(file_uri) {
        if let Some(ann) = data
            .type_annotations
            .iter()
            .find(|(_, n, _)| n == field_name)
        {
            return Some(ann.2.clone());
        }
        return data.lines.infer_type_raw(field_name);
    }
    let path = tower_lsp::lsp_types::Url::parse(file_uri)
        .ok()?
        .to_file_path()
        .ok()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let lines: Vec<String> = content.lines().map(String::from).collect();
    lines.infer_type_raw(field_name)
}

/// Look up the raw type of `field_name` declared inside class `class_name`,
/// resolving across files via the definitions index.
///
/// Used for multi-segment receiver chains like `result.availableBanks.map { it }`:
/// resolves `result` → `ResponseBody`, then looks up `availableBanks` in `ResponseBody`.
pub(crate) fn find_field_type_in_class(
    indexer: &Indexer,
    class_name: &str,
    field_name: &str,
) -> Option<String> {
    let locs = indexer.definitions.get(class_name)?;
    for loc in locs.iter() {
        if let Some(type_name) = infer_field_type_raw(indexer, loc.uri.as_str(), field_name) {
            return Some(type_name);
        }
    }
    // Fallback: full variable inference including CST-indexed field_access_rhs
    // and method_call_rhs data (handles unannotated `val x = recv.field`).
    let locs = indexer.definitions.get(class_name)?;
    for loc in locs.iter() {
        if let Some(type_name) = infer_variable_type_raw(indexer, field_name, &loc.uri) {
            return Some(type_name);
        }
    }
    None
}

// ─── Extension property type inference ───────────────────────────────────────

/// Look up the declared type of an extension property named `prop_name` that
/// is available on any class declared in the file at `uri`.
///
/// This is the fallback path for expressions like `viewModelScope.launch` where
/// `viewModelScope` is `val ViewModel.viewModelScope: CoroutineScope` — the
/// property is not declared inside the calling file, so line-scanning returns
/// nothing.  Here we:
/// 1. Collect all class names declared in the calling file.
/// 2. Build the ancestor set for each via `walk_hierarchy`.
/// 3. Scan the index for an extension property whose `extension_receiver` is in
///    that ancestor set and whose `name == prop_name`.
/// 4. Extract the return type from the symbol's `detail` string.
fn find_extension_property_type(indexer: &Indexer, prop_name: &str, uri: &Url) -> Option<String> {
    // TODO: This fallback considers ALL classes in the file, so in files with
    // multiple top-level classes, an extension for the wrong class could match.
    // Threading the enclosing class context through the full call chain is needed
    // for a proper fix; the primary (line-scanning) path handles the common case.
    use super::walk_hierarchy;
    use crate::types::{CallerContext, Visibility};

    // Use ensure_file_data so the function works even when the file has not been
    // indexed yet (e.g. first open before the workspace scan completes).
    let file = ensure_file_data(indexer, uri)?;

    // Collect class names declared in this file as starting points.
    let class_names: Vec<(String, String)> = file
        .symbols
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                SymbolKind::CLASS | SymbolKind::OBJECT | SymbolKind::INTERFACE | SymbolKind::STRUCT
            )
        })
        .map(|s| (s.name.clone(), uri.to_string()))
        .collect();

    if class_names.is_empty() {
        return None;
    }

    // Build a set of all ancestor type names across all classes in this file.
    let caller = CallerContext {
        uri: Some(uri.as_str()),
        cursor_line: None,
    };
    let mut ancestor_set: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (class_name, class_uri) in &class_names {
        ancestor_set.insert(class_name.clone());
        let supers: Vec<String> = walk_hierarchy(
            indexer,
            class_name,
            class_uri,
            caller,
            8,
            |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
        );
        ancestor_set.extend(supers);
    }

    // Use the reverse index: O(ancestors) instead of O(all_files).
    for ancestor in &ancestor_set {
        let Some(entries) = indexer.extension_by_receiver.get(ancestor) else {
            continue;
        };
        for entry in entries.iter() {
            if entry.name != prop_name {
                continue;
            }
            use tower_lsp::lsp_types::SymbolKind;
            if !matches!(entry.kind, SymbolKind::PROPERTY | SymbolKind::VARIABLE) {
                continue;
            }
            if matches!(
                entry.visibility,
                Visibility::Private | Visibility::Protected
            ) {
                continue;
            }
            let type_name = extract_property_type_from_detail(&entry.detail);
            if let Some(type_name) = type_name {
                return Some(type_name);
            }
        }
    }
    None
}

// ─── Method return-type inference ─────────────────────────────────────────────

fn infer_method_return_type(
    indexer: &Indexer,
    var_name: &str,
    lines: &[String],
    uri: &Url,
    depth: u8,
) -> Option<String> {
    let mut plain_fn_candidates: Vec<String> = Vec::new();
    let mut seen_receivers: std::collections::HashSet<&str> = std::collections::HashSet::new();

    for line in lines {
        let rhs = match find_rhs_str(line, var_name) {
            Some(r) => r,
            None => continue,
        };

        // Match `receiver.method(` where receiver is a simple identifier.
        let paren_pos = match rhs.find('(') {
            Some(p) => p,
            None => continue,
        };
        let before_paren = &rhs[..paren_pos];
        match before_paren.rfind('.') {
            Some(dot_pos) => {
                let receiver = before_paren[..dot_pos].trim();
                let method = before_paren[dot_pos + 1..].trim();

                if receiver.is_empty() || method.is_empty() {
                    continue;
                }
                // Skip `this`/`super` and multi-segment receivers.
                if receiver == "this" || receiver == "super" || receiver.contains('.') {
                    continue;
                }
                if !method.starts_with_lowercase() {
                    continue;
                }
                // Dedup: skip if we already tried this receiver (avoids exponential blowup).
                if !seen_receivers.insert(receiver) {
                    continue;
                }

                // Recursively infer the receiver type (DashMap guards already dropped).
                if let Some(receiver_type) = infer_variable_type_impl(indexer, receiver, uri, depth)
                {
                    if let Some(ret) =
                        find_method_return_type(indexer, &receiver_type, method, Some(uri))
                    {
                        return Some(ret);
                    }
                }
            }
            None => {
                // Plain function call: `val result = getFoo(args)` — no dot-receiver.
                // Guard: skip when the first call is part of a chain (`getFoo(...).bar()`).
                let fn_name = before_paren.trim();
                if !fn_name.is_empty()
                    && fn_name.starts_with_lowercase()
                    && !has_dot_after_first_call(rhs, paren_pos)
                {
                    plain_fn_candidates.push(fn_name.to_owned());
                }
            }
        }
    }

    // Secondary pass: plain function calls whose return type is in the definitions index.
    for fn_name in &plain_fn_candidates {
        if let Some(ret) = find_fun_return_type_by_name(indexer, fn_name) {
            return Some(ret);
        }
    }

    None
}

/// Look up `method_name` in the symbol index for `type_name` and return its
/// return type, extracted from `SymbolEntry.detail`.
/// Look up the return type of a function by name, searching across all indexed files.
///
/// Unlike `find_method_return_type` this requires no receiver type — useful when
/// the caller is a method chain expression and the receiver type is unknown.
/// Returns the raw return type string (with generics preserved), e.g. `"List<Account>"`.
pub(crate) fn find_fun_return_type_by_name(indexer: &Indexer, fn_name: &str) -> Option<String> {
    let locations = indexer.definitions.get(fn_name)?;
    for loc in locations.iter() {
        if let Some(file_data) = indexer.files.get(loc.uri.as_str()) {
            for symbol in &file_data.symbols {
                if symbol.name != fn_name {
                    continue;
                }
                if !matches!(
                    symbol.kind,
                    SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
                ) {
                    continue;
                }
                if let Some(ret) = extract_return_type_from_detail(&symbol.detail) {
                    return Some(ret);
                }
                let start_line = symbol.selection_start() as usize;
                let full_sig = file_data.lines.collect_signature(start_line);
                if let Some(ret) = extract_return_type_from_detail(&full_sig) {
                    return Some(ret);
                }
            }
        }
    }
    None
}

pub(crate) fn find_method_return_type(
    indexer: &Indexer,
    type_name: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    let type_base = type_name.last_segment();

    // Extension functions take precedence over member functions.
    if let Some(ret) = find_extension_fn_return_type(indexer, type_base, method_name, from_uri) {
        return Some(ret);
    }

    // Then check member functions (container-based).
    let locations = indexer.definitions.get(type_base)?;
    for loc in locations.iter() {
        if let Some(file_data) = indexer.files.get(loc.uri.as_str()) {
            for symbol in &file_data.symbols {
                if symbol.name != method_name {
                    continue;
                }
                if !matches!(
                    symbol.kind,
                    SymbolKind::FUNCTION | SymbolKind::METHOD | SymbolKind::OPERATOR
                ) {
                    continue;
                }
                if symbol.container.as_deref() != Some(type_base) {
                    continue;
                }
                // Try detail first; fall back to source lines when detail is truncated.
                if let Some(ret) = extract_return_type_from_detail(&symbol.detail) {
                    return Some(ret);
                }
                // detail may be truncated (120 char limit) — try the source lines.
                let start_line = symbol.selection_start() as usize;
                let full_sig = file_data.lines.collect_signature(start_line);
                if let Some(ret) = extract_return_type_from_detail(&full_sig) {
                    return Some(ret);
                }
            }
        }
    }
    find_extension_fn_return_type(indexer, type_base, method_name)
}

/// Returns true when an extension function declared in `entry_package` is
/// accessible from the calling file, either via same-package visibility or
/// an explicit import in `caller_file_data`.
pub(crate) fn extension_is_in_scope(
    entry_package: Option<&String>,
    entry_name: &str,
    caller_package: Option<&String>,
    caller_file_data: Option<&FileData>,
) -> bool {
    entry_package.is_some_and(|ext_pkg| caller_package == Some(ext_pkg))
        || caller_file_data.is_some_and(|fd| {
            fd.imports.iter().any(|imp| {
                entry_package
                    .as_ref()
                    .is_some_and(|ext_pkg| imp.covers(ext_pkg, entry_name))
            })
        })
}

/// Find the return type of an extension function `method_name` declared with receiver
/// `ReceiverType` where `ReceiverType`'s base name == `receiver_base`.
///
/// When `from_uri` is provided, only extensions in scope (same package or imported)
/// at that URI are considered — matching the scope rules used by goto-definition.
/// When `from_uri` is `None`, a global unfiltered lookup is performed (for callers
/// that have no URI context).
///
/// Extension functions are stored with `container = None` and `extension_receiver = "Foo"`,
/// so `find_method_return_type` (which filters by `container == Some(type_base)`) misses them.
/// This function searches by the function name directly, then filters by receiver.
///
/// Example: `receiver_base = "Optional"`, `method_name = "getOrNull"` →
/// finds `public fun <T : Any> Optional<T>.getOrNull(): T?` and returns `"T?"`.
pub(crate) fn find_extension_fn_return_type(
    indexer: &Indexer,
    receiver_base: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    if let Some(uri) = from_uri {
        return find_extension_fn_return_type_scoped(indexer, receiver_base, method_name, uri);
    }
    find_extension_fn_return_type_global(indexer, receiver_base, method_name)
}

fn find_extension_fn_return_type_scoped(
    indexer: &Indexer,
    receiver_base: &str,
    method_name: &str,
    from_uri: &Url,
) -> Option<String> {
    let entries = indexer.extension_by_receiver.get(receiver_base)?;
    let caller_file_data = indexer.files.get(from_uri.as_str());
    let caller_file_data_ref: Option<&FileData> = caller_file_data.as_deref().map(|v| v.as_ref());
    let caller_package = caller_file_data.as_ref().and_then(|fd| fd.package.as_ref());
    for entry in entries.iter() {
        if entry.name != method_name {
            continue;
        }
        if !matches!(entry.kind, SymbolKind::FUNCTION) {
            continue;
        }
        if !extension_is_in_scope(
            entry.package.as_ref(),
            &entry.name,
            caller_package,
            caller_file_data_ref,
        ) {
            continue;
        }
        // Try detail first; fall back to source lines when detail is truncated.
        if let Some(ret) = extract_return_type_from_detail(&entry.detail) {
            return Some(ret);
        }
        // detail may be truncated (120 char limit) — try the source lines.
        let file_data = indexer
            .files
            .get(&entry.file_uri)
            .or_else(|| indexer.jar_files.get(&entry.file_uri))?;
        let start_line = file_data
            .symbols
            .iter()
            .find(|s| s.name == method_name)?
            .selection_start() as usize;
        let full_sig = file_data.lines.collect_signature(start_line);
        if let Some(ret) = extract_return_type_from_detail(&full_sig) {
            return Some(ret);
        }
    }
    None
}

fn find_extension_fn_return_type_global(
    indexer: &Indexer,
    receiver_base: &str,
    method_name: &str,
) -> Option<String> {
    let locations = indexer.definitions.get(method_name)?;
    for loc in locations.iter() {
        let Some(file_data) = indexer.files.get(loc.uri.as_str()) else {
            continue;
        };
        for symbol in &file_data.symbols {
            if symbol.name != method_name {
                continue;
            }
            if !matches!(symbol.kind, SymbolKind::FUNCTION) {
                continue;
            }
            if symbol.extension_receiver != receiver_base {
                continue;
            }
            // Try detail first; fall back to source lines when detail is truncated.
            if let Some(ret) = extract_return_type_from_detail(&symbol.detail) {
                return Some(ret);
            }
            // detail may be truncated (120 char limit) — try the source lines.
            let start_line = symbol.selection_start() as usize;
            let full_sig = file_data.lines.collect_signature(start_line);
            if let Some(ret) = extract_return_type_from_detail(&full_sig) {
                return Some(ret);
            }
        }
    }
    None
}

/// Walk the class hierarchy to find an inherited method's return type.
///
/// When `find_method_return_type(indexer, "BuildingSavingsReducer", "reduce")` returns
/// `None` because `reduce` is declared on supertype `FlowReducer`, this function:
/// 1. Finds the subclass's supertype declarations (with type args)
/// 2. Looks up the method on each supertype
/// 3. Substitutes the supertype's generic type params with the concrete type args
///
/// Returns `None` if the method is not found on any supertype.
pub(crate) fn find_method_return_type_via_supertypes(
    indexer: &Indexer,
    class_name: &str,
    method_name: &str,
    from_uri: Option<&Url>,
) -> Option<String> {
    let class_base = class_name.split('<').next().unwrap_or(class_name);
    let class_locs = indexer.definitions.get(class_base)?;

    for class_loc in class_locs.iter() {
        let Some(file_data) = indexer.files.get(class_loc.uri.as_str()) else {
            continue;
        };
        let Some(class_sym) = file_data.symbols.iter().find(|s| s.name == class_base) else {
            continue;
        };
        let class_line = class_sym.selection_start();

        for (line, super_name, type_args) in file_data.supers.iter() {
            if *line != class_line {
                continue;
            }
            let raw_return_type =
                find_method_return_type(indexer, super_name, method_name, from_uri);
            let Some(raw) = raw_return_type else {
                continue;
            };

            if type_args.is_empty() {
                return Some(raw);
            }

            let super_type_params = find_class_type_params(indexer, super_name);
            if super_type_params.is_empty() {
                return Some(raw);
            }

            let substituted = apply_supertype_subst(&raw, &super_type_params, type_args);
            return Some(substituted);
        }
    }
    None
}

fn find_class_type_params(indexer: &Indexer, class_name: &str) -> Vec<String> {
    let Some(locations) = indexer.definitions.get(class_name) else {
        return Vec::new();
    };
    for loc in locations.iter() {
        if let Some(file_data) = indexer.files.get(loc.uri.as_str()) {
            if let Some(symbol) = file_data
                .symbols
                .iter()
                .find(|s| s.name == class_name && !s.type_params.is_empty())
            {
                return symbol.type_params.clone();
            }
        }
    }
    Vec::new()
}

/// Replace generic type parameter names with concrete type arguments.
///
/// Given `raw = "Flow<ReducedResult<EffectType, StateType>>"`,
/// `params = ["EventType", "EffectType", "StateType"]`,
/// `args = ["BuildingSavingsInputEvent", "BuildingSavingsEffect", "Sheet"]`,
/// returns `"Flow<ReducedResult<BuildingSavingsEffect, Sheet>>"`.
fn apply_supertype_subst(raw: &str, params: &[String], args: &[String]) -> String {
    let mut result = raw.to_string();
    for (param, arg) in params.iter().zip(args.iter()) {
        // Replace whole-word occurrences only (not substrings of other type names).
        let mut new_result = String::with_capacity(result.len());
        let mut remaining = result.as_str();
        while let Some(pos) = remaining.find(param.as_str()) {
            new_result.push_str(&remaining[..pos]);
            let after = pos + param.len();
            let before_ok = pos == 0
                || !remaining.as_bytes()[pos - 1].is_ascii_alphanumeric()
                    && remaining.as_bytes()[pos - 1] != b'_';
            let after_ok = after >= remaining.len()
                || !remaining.as_bytes()[after].is_ascii_alphanumeric()
                    && remaining.as_bytes()[after] != b'_';
            if before_ok && after_ok {
                new_result.push_str(arg);
            } else {
                new_result.push_str(param);
            }
            remaining = &remaining[after..];
        }
        new_result.push_str(remaining);
        result = new_result;
    }
    result
}

#[cfg(test)]
#[path = "infer_tests.rs"]
mod infer_tests;
