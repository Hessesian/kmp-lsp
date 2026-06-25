use std::sync::Arc;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemTag, InsertTextFormat, Position, SymbolKind,
    Url,
};

use crate::indexer::Indexer;
use crate::parser::parse_by_extension;
use crate::stdlib::bare_completions;
use crate::stdlib_tail::dot_completions_for_lang;
use crate::types::{CallerContext, ImportEntry, SourceSet, Visibility};
use crate::LinesExt;
use crate::StrExt;

use super::infer::{
    find_field_type_in_class, find_fun_return_type_by_name, find_method_return_type,
    infer_receiver_type, infer_receiver_type_at, infer_variable_type_raw, ReceiverKind,
    ReceiverType,
};
use super::infer_lines::infer_callable_param_return_type;
use super::{
    already_imported, ensure_file_data, fqns_for_name, resolve_symbol_no_rg, walk_hierarchy,
};

// ─── CompletionItem.data JSON keys ───────────────────────────────────────────

/// Symbol definition URI.
pub(crate) const DATA_URI: &str = "u";
/// Symbol definition line (0-based).
pub(crate) const DATA_LINE: &str = "l";
/// Symbol definition UTF-16 column (0-based).
pub(crate) const DATA_COL: &str = "c";
/// Calling-site URI, present only for cross-file substitution context.
pub(crate) const DATA_CALLING_URI: &str = "cu";

// ─── match scoring ────────────────────────────────────────────────────────────

/// Returns true if `name` is SCREAMING_SNAKE_CASE (all letters are uppercase).
/// Used to suppress constants/enum variants when the user types a CamelCase prefix.
pub(crate) fn is_screaming_snake(name: &str) -> bool {
    name.chars().any(|c| c.is_alphabetic())
        && name
            .chars()
            .all(|c| c.is_uppercase() || c == '_' || c.is_ascii_digit())
}

/// Score how well `name` matches `prefix`. Lower = better.
///
/// - `0` — `name` starts with `prefix` (case-insensitive, fastest/best)
/// - `1` — camelCase acronym: every character in `prefix` (uppercase-as-given)
///   matches the first letter of successive CamelCase/underscore word
///   segments in `name` (e.g. `CB` → `ColumnButton`, `mSF` → `myStateFlow`)
/// - `2` — `name` contains `prefix` as a case-insensitive substring
/// - `None` — no match; exclude this symbol
pub(crate) fn match_score(name: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(0);
    }
    let name_lower = name.to_ascii_lowercase();
    let prefix_lower = prefix.to_ascii_lowercase();
    if name_lower.starts_with(&prefix_lower) {
        return Some(0);
    }
    if camel_acronym_match(name, prefix) {
        return Some(1);
    }
    if name_lower.contains(&prefix_lower) {
        return Some(2);
    }
    None
}

/// True if every character in `prefix` matches the first character of a successive
/// CamelCase or underscore-delimited word in `name`.
///
/// Matching is case-insensitive: both `prefix` and the collected word starts are
/// compared in lowercase.
///
/// Examples:
///   `CB`  vs `ColumnButton`    → true  (C=Column, B=Button)
///   `mSF` vs `myStateFlow`     → true  (m=my, S=State, F=Flow)
///   `CB`  vs `CoolBar`         → false (C=C ok, B must start next word; 'oolBar' has no word-start at 'B')
///   `CB`  vs `coolBar`         → true  (case-insensitive: c=cool, b=Bar)
fn camel_acronym_match(name: &str, prefix: &str) -> bool {
    // Collect the first character of each CamelCase / underscore segment.
    let mut word_starts: Vec<char> = Vec::new();
    let chars: Vec<char> = name.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let is_word_start = i == 0
            || c == '_'
            || (i > 0 && chars[i - 1] == '_')          // char immediately after underscore
            || (c.is_uppercase() && i > 0 && chars[i - 1].is_lowercase())
            || (c.is_uppercase() && i > 0 && chars[i - 1].is_uppercase()
                && i + 1 < chars.len() && chars[i + 1].is_lowercase());
        if is_word_start && c != '_' {
            word_starts.push(c.to_lowercase().next().unwrap_or(c));
        }
    }

    // Every prefix char must match successive word starts (in order, not necessarily consecutive).
    let prefix_chars: Vec<char> = prefix.to_ascii_lowercase().chars().collect();
    let mut wi = 0;
    for &pc in &prefix_chars {
        loop {
            if wi >= word_starts.len() {
                return false;
            }
            if word_starts[wi] == pc {
                wi += 1;
                break;
            }
            wi += 1;
        }
    }
    true
}

// ─── completion entry point ───────────────────────────────────────────────────

/// Maximum completion items returned per response.
/// When capped, `is_incomplete` should be set so the client re-queries.
pub(crate) const COMPLETION_CAP: usize = 500;

/// Prefix length at which local-symbol relevance score is reduced (longer prefix → more confident match).
const MIN_PREFIX_SCORE_REDUCTION: usize = 4;

/// Minimum prefix char count for camel-acronym cross-package matching.
/// Single-char prefixes still run collect_cross_package, but are restricted
/// to score-0 (case-insensitive prefix match) to avoid camel-acronym noise.
const MIN_CAMEL_ACRONYM_PREFIX: usize = 2;

/// Parsed receiver expression from text immediately before a `.` trigger.
///
/// Carries both the identifier chain and whether the receiver was a function
/// call, so resolution can be explicit rather than heuristic.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ReceiverExpr {
    /// Dotted identifier chain with call args stripped, e.g. `"productFlow"` or `"foo.bar"`.
    pub(crate) chain: String,
    /// `true` when the receiver was a call expression like `productFlow(...)`.
    pub(crate) is_call: bool,
}

impl ReceiverExpr {
    /// Parse the text before a `.` completion trigger.
    ///
    /// `"productFlow(trigger.isRefresh())."` → `Some(ReceiverExpr { chain: "productFlow", is_call: true })`
    /// `"foo.bar."` → `Some(ReceiverExpr { chain: "foo.bar", is_call: false })`
    /// `"nullable?."` → `Some(ReceiverExpr { chain: "nullable", is_call: false })`
    pub(crate) fn parse(before_prefix: &str) -> Option<Self> {
        // Kotlin's safe-call `?.` only affects null-handling at runtime — for
        // finding which type's members to suggest it's equivalent to a plain `.`.
        // Normalize before scanning so the rest of this function (and the
        // backward identifier scan, which doesn't otherwise allow `?`) doesn't
        // need its own safe-call handling.
        let normalized = before_prefix.replace("?.", ".");
        let before_dot = normalized.strip_suffix('.')?;
        let (before_call, is_call) = if before_dot.trim_end().ends_with(')') {
            (Self::strip_call_args(before_dot.trim_end()), true)
        } else {
            (before_dot, false)
        };
        // Scan backwards for a dotted identifier chain.
        // Includes bytes >= 0x80 to support non-ASCII identifiers.
        let bytes = before_call.as_bytes();
        let mut start = before_call.len();
        for i in (0..before_call.len()).rev() {
            let c = bytes[i];
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'.' || c >= 0x80 {
                start = i;
            } else {
                break;
            }
        }
        let chain = before_call[start..].trim();
        if chain.is_empty() || chain.starts_with('.') || chain.ends_with('.') {
            return None;
        }
        Some(Self {
            chain: chain.to_owned(),
            is_call,
        })
    }

    /// Construct a plain variable / type-name receiver (not a call expression).
    pub(crate) fn variable(name: &str) -> Self {
        Self {
            chain: name.to_owned(),
            is_call: false,
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.chain
    }

    /// Strip a balanced trailing `(...)`, e.g. `"foo(arg, bar())"` → `"foo"`.
    fn strip_call_args(s: &str) -> &str {
        let bytes = s.as_bytes();
        let mut depth = 0usize;
        let mut i = bytes.len();
        loop {
            if i == 0 {
                break;
            }
            i -= 1;
            match bytes[i] {
                b')' => depth += 1,
                b'(' => {
                    depth -= 1;
                    if depth == 0 {
                        return s[..i].trim_end();
                    }
                }
                _ => {}
            }
        }
        s
    }
}

/// Provide completion candidates for `prefix` at the current position.
///
/// - **Dot-completion** (`dot_receiver = Some("obj")`): infer the receiver's type
///   and return all its members (symbols + line-scanned constructor params).
/// - **Bare-word** (`dot_receiver = None`): return all symbols in scope.
pub(crate) fn complete_symbol(
    indexer: &Indexer,
    prefix: &str,
    dot_receiver: Option<&str>,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> (Vec<CompletionItem>, bool) {
    complete_symbol_with_context(
        indexer,
        prefix,
        dot_receiver.map(ReceiverExpr::variable),
        from_uri,
        snippets,
        false,
        cursor_line,
    )
}

/// Like `complete_symbol` but with explicit annotation context flag.
/// Called from `indexer::completions` after detecting a `@` trigger.
pub(crate) fn complete_symbol_with_context(
    indexer: &Indexer,
    prefix: &str,
    dot_receiver: Option<ReceiverExpr>,
    from_uri: &Url,
    snippets: bool,
    annotation_only: bool,
    cursor_line: Option<u32>,
) -> (Vec<CompletionItem>, bool) {
    if let Some(expr) = dot_receiver {
        return (
            complete_dot_expr(indexer, &expr, from_uri, snippets, cursor_line),
            false,
        );
    }
    complete_bare(
        indexer,
        prefix,
        from_uri,
        snippets,
        annotation_only,
        cursor_line,
    )
}

/// Detect whether the character immediately before `prefix` in `line` is `@`.
/// Used to restrict completions to annotation/class kinds only.
pub(crate) fn is_annotation_context(line: &str, prefix: &str) -> bool {
    line.strip_suffix(prefix)
        .map(|before| before.ends_with('@'))
        .unwrap_or(false)
}

/// Scan the index for extension functions whose `extension_receiver` matches
/// `receiver_type` or any of its supertypes, returning `CompletionItem`s with
/// auto-import `additionalTextEdits` when needed.
///
/// Hierarchy traversal works for source-indexed types. JAR-to-JAR hierarchy is
/// not currently supported because the sidecar does not populate `FileData.supers`.
///
/// Only called for Kotlin files; Java files don't consume Kotlin extension functions.
fn extension_fn_completions(
    indexer: &Indexer,
    receiver_type: &str,
    from_uri: &Url,
    snippets: bool,
) -> Vec<CompletionItem> {
    if receiver_type.is_empty() {
        return vec![];
    }

    // Build ancestor set: receiver_type + all source-indexed supertypes.
    let mut ancestor_set: std::collections::HashSet<String> =
        std::collections::HashSet::from([receiver_type.to_owned()]);
    if let Some(class_location) = resolve_symbol_no_rg(indexer, receiver_type, from_uri)
        .into_iter()
        .next()
    {
        let class_uri = class_location.uri.to_string();
        let caller = CallerContext {
            uri: Some(from_uri.as_str()),
            cursor_line: None,
        };
        let supers = walk_hierarchy(
            indexer,
            receiver_type,
            &class_uri,
            caller,
            8,
            |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
        );
        ancestor_set.extend(supers);
    }

    let context = ExtensionCompletionContext::build(indexer, from_uri);
    let mut builder = ExtensionCompletionBuilder::new(&context, receiver_type, snippets);

    for ancestor in &ancestor_set {
        if let Some(entries) = indexer.extension_by_receiver.get(ancestor) {
            for entry in entries.iter() {
                if crate::Language::from_path(&entry.file_uri) == crate::Language::Kotlin {
                    let is_library = is_library_extension(indexer, &entry.file_uri);
                    builder.add_entry(entry, is_library);
                }
            }
        }
    }

    builder.finish()
}

struct ExtensionCompletionContext {
    from_uri: String,
    imports: Vec<ImportEntry>,
    package_name: String,
    lines: Arc<Vec<String>>,
}

impl ExtensionCompletionContext {
    fn build(indexer: &Indexer, from_uri: &Url) -> Self {
        let live_lines = indexer
            .live_lines
            .get(from_uri.as_str())
            .map(|lines| lines.clone());
        let Some(file) = indexer.files.get(from_uri.as_str()) else {
            let lines = live_lines.clone().unwrap_or_default();
            return Self {
                from_uri: from_uri.as_str().to_owned(),
                imports: lines.parse_imports(),
                package_name: String::new(),
                lines,
            };
        };

        let lines = live_lines.clone().unwrap_or_else(|| file.lines.clone());
        let imports = if live_lines.is_some() {
            lines.parse_imports()
        } else {
            file.imports.clone()
        };
        Self {
            from_uri: from_uri.as_str().to_owned(),
            imports,
            package_name: file.package.clone().unwrap_or_default(),
            lines,
        }
    }
}

struct ExtensionCompletionBuilder<'a> {
    context: &'a ExtensionCompletionContext,
    snippets: bool,
    seen: std::collections::HashSet<String>,
    items: Vec<CompletionItem>,
}

impl<'a> ExtensionCompletionBuilder<'a> {
    fn new(
        context: &'a ExtensionCompletionContext,
        _receiver_type: &'a str,
        snippets: bool,
    ) -> Self {
        Self {
            context,
            snippets,
            seen: std::collections::HashSet::new(),
            items: Vec::new(),
        }
    }

    fn add_entry(&mut self, entry: &crate::types::ExtensionEntry, is_library: bool) {
        let is_same_file = entry.file_uri == self.context.from_uri;
        // Inaccessible from this file: private/protected from another file always;
        // `internal` when the symbol comes from a library (an external module's
        // internal members cannot be referenced from workspace code).
        if !is_same_file
            && (matches!(
                entry.visibility,
                Visibility::Private | Visibility::Protected
            ) || (is_library && entry.visibility == Visibility::Internal))
        {
            return;
        }
        // Deprecated policy: hide deprecated *library* symbols entirely (you can't
        // fix them, and editors like Android Studio keep them out of the list).
        // Deprecated *workspace* symbols are kept but tagged + deprioritized below,
        // since you may still reference your own code during a migration.
        if entry.deprecated && is_library {
            return;
        }
        // Dedup by name alone so a receiver shows a single completion entry per
        // extension, matching how IDEs present one `launch` (signature help
        // disambiguates overloads later). Keying on the signature instead would
        // surface every overload — and the same function arrives multiple ways:
        // coroutines 1.11.0's compiled JAR emits `launch` overloads, and the
        // sources JAR re-emits the same `launch` with a *different* package field
        // (the compiled path infers one imprecise per-jar package, the sources
        // path has the exact per-file one), so package-scoped keys wouldn't merge
        // them. The trailing-lambda form keeps its own `:lam` key so both
        // `launch(…)` and `launch { }` still appear.
        if !self.seen.insert(entry.name.clone()) {
            return;
        }
        self.items
            .push(self.build_item_from_entry(entry, is_same_file));

        // Offer a trailing-lambda variant when the last parameter is a function type.
        if entry.trailing_lambda {
            let lambda_key = format!("{}:lam", entry.name);
            if self.seen.insert(lambda_key) {
                self.items
                    .push(self.build_lambda_item_from_entry(entry, is_same_file));
            }
        }
    }

    fn build_item_from_entry(
        &self,
        entry: &crate::types::ExtensionEntry,
        is_same_file: bool,
    ) -> CompletionItem {
        let package_name = entry.package.as_deref().unwrap_or("");
        let fqn = extension_symbol_fqn(package_name, &entry.name);
        let needs_import = self.needs_import(&fqn, is_same_file);
        let ck = symbol_kind_to_completion(entry.kind);
        let is_callable = matches!(
            ck,
            CompletionItemKind::FUNCTION | CompletionItemKind::METHOD
        );
        let detail = if !entry.detail.is_empty() {
            Some(entry.detail.clone())
        } else {
            needs_import.then(|| package_of_fqn(&fqn).to_owned())
        };
        let mut item = CompletionItem {
            label: entry.name.clone(),
            kind: Some(ck),
            insert_text: (self.snippets && is_callable).then(|| format!("{}($1)", entry.name)),
            insert_text_format: (self.snippets && is_callable).then_some(InsertTextFormat::SNIPPET),
            sort_text: Some(format!("01:ext:{}", entry.name)),
            detail,
            command: (self.snippets && is_callable).then(trigger_parameter_hints),
            additional_text_edits: self.import_edit(&fqn, needs_import),
            ..Default::default()
        };
        mark_deprecated(&mut item, entry.deprecated);
        item
    }

    fn build_lambda_item_from_entry(
        &self,
        entry: &crate::types::ExtensionEntry,
        is_same_file: bool,
    ) -> CompletionItem {
        let package_name = entry.package.as_deref().unwrap_or("");
        let fqn = extension_symbol_fqn(package_name, &entry.name);
        let needs_import = self.needs_import(&fqn, is_same_file);
        let detail = if !entry.detail.is_empty() {
            Some(entry.detail.clone())
        } else {
            needs_import.then(|| package_of_fqn(&fqn).to_owned())
        };
        let mut item = CompletionItem {
            label: format!("{} {{ }}", entry.name),
            kind: Some(CompletionItemKind::FUNCTION),
            insert_text: self.snippets.then(|| format!("{} {{ $1 }}", entry.name)),
            insert_text_format: self.snippets.then_some(InsertTextFormat::SNIPPET),
            // Sort immediately after the regular form for this name.
            sort_text: Some(format!("01:ext:{}:z", entry.name)),
            detail,
            command: None,
            additional_text_edits: self.import_edit(&fqn, needs_import),
            ..Default::default()
        };
        mark_deprecated(&mut item, entry.deprecated);
        item
    }

    fn needs_import(&self, fqn: &str, is_same_file: bool) -> bool {
        let package_name = package_of_fqn(fqn);
        !is_same_file
            && !already_imported(fqn, &self.context.imports)
            && !self
                .context
                .imports
                .iter()
                .any(|entry| entry.is_star && entry.full_path == package_name)
            && package_name != self.context.package_name
    }

    fn import_edit(
        &self,
        fqn: &str,
        needs_import: bool,
    ) -> Option<Vec<tower_lsp::lsp_types::TextEdit>> {
        needs_import.then(|| vec![self.context.lines.make_import_edit(fqn, false)])
    }

    fn finish(self) -> Vec<CompletionItem> {
        self.items
    }
}

/// Whether an extension entry comes from a library (JAR/sources-JAR or a path
/// registered in `library_uris`) rather than workspace source.
///
/// Library symbols get the strict deprecated/internal filtering (hidden);
/// workspace symbols keep deprecated entries (tagged + deprioritized).
fn is_library_extension(indexer: &Indexer, file_uri: &str) -> bool {
    file_uri.starts_with("jar:") || indexer.library_uris.contains(file_uri)
}

/// Tag a completion item as deprecated and push it to the bottom of the list.
///
/// Sets the LSP `Deprecated` tag (clients render it struck-through) and rewrites
/// `sort_text` with a high-digit prefix so deprecated entries rank below all live
/// ones. No-op when `deprecated` is false. Used only for workspace symbols —
/// deprecated library symbols are filtered out entirely before reaching here.
fn mark_deprecated(item: &mut CompletionItem, deprecated: bool) {
    if !deprecated {
        return;
    }
    item.tags = Some(vec![CompletionItemTag::DEPRECATED]);
    item.sort_text = Some(match item.sort_text.take() {
        Some(existing) => format!("99:{existing}"),
        None => format!("99:{}", item.label),
    });
}

fn extension_symbol_fqn(package_name: &str, symbol_name: &str) -> String {
    if package_name.is_empty() {
        return symbol_name.to_owned();
    }
    format!("{package_name}.{symbol_name}")
}

fn package_of_fqn(fqn: &str) -> &str {
    fqn.rfind('.').map(|pos| &fqn[..pos]).unwrap_or("")
}

fn complete_super(indexer: &Indexer, from_uri: &Url, snippets: bool) -> Vec<CompletionItem> {
    if indexer.files.get(from_uri.as_str()).is_none() {
        return vec![];
    }

    let mut items = walk_hierarchy(
        indexer,
        "",
        from_uri.as_str(),
        CallerContext::default(),
        4,
        |index, _, class_uri, _| symbols_from_uri_as_completions(index, class_uri),
    );
    filter_inaccessible_completion_items(&mut items);
    strip_completion_snippets(&mut items, snippets);
    items.sort_by(|a, b| {
        kind_sort_rank(a.kind)
            .cmp(&kind_sort_rank(b.kind))
            .then_with(|| a.label.cmp(&b.label))
    });
    items.dedup_by(|a, b| a.label == b.label);
    items
}

/// Dot-completion: return all members of the receiver's inferred type,
/// sorted: methods first, then fields/vars, then class-level names last.
pub(crate) fn complete_dot(
    indexer: &Indexer,
    receiver: &str,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> Vec<CompletionItem> {
    complete_dot_expr(
        indexer,
        &ReceiverExpr::variable(receiver),
        from_uri,
        snippets,
        cursor_line,
    )
}

fn complete_dot_expr(
    indexer: &Indexer,
    expr: &ReceiverExpr,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> Vec<CompletionItem> {
    if expr.as_str() == "super" {
        return complete_super(indexer, from_uri, snippets);
    }

    let Some(receiver_type) = resolve_dot_receiver_type(indexer, expr, from_uri, cursor_line)
    else {
        return vec![];
    };

    let mut items = Vec::new();
    let file_found =
        resolve_dot_receiver_file(indexer, &receiver_type.outer, from_uri).map(|file_uri| {
            let context = DotCompletionContext {
                receiver_type: receiver_type.clone(),
                file_uri,
            };
            items.extend(direct_dot_completion_items(
                indexer,
                &context,
                from_uri,
                cursor_line,
            ));
            filter_inaccessible_completion_items(&mut items);
            collect_inherited_dot_completion_items(
                indexer,
                &context,
                from_uri,
                snippets,
                cursor_line,
                &mut items,
            );
        });

    dedup_completion_labels(&mut items);
    strip_completion_snippets(&mut items, snippets);
    items.sort_by_key(|item| kind_sort_rank(item.kind));
    append_dot_tail_completions(
        indexer,
        &receiver_type,
        from_uri,
        snippets,
        file_found.is_some(),
        &mut items,
    );
    items
}

struct DotCompletionContext {
    receiver_type: ReceiverType,
    file_uri: String,
}

fn resolve_dot_receiver_type(
    indexer: &Indexer,
    expr: &ReceiverExpr,
    from_uri: &Url,
    cursor_line: Option<u32>,
) -> Option<ReceiverType> {
    let receiver = expr.as_str();

    if expr.is_call {
        // Call expression: skip variable lookup, resolve by function return type.
        if receiver.contains('.') {
            if let Some(raw) = resolve_dotted_receiver_type(indexer, receiver, from_uri) {
                return Some(ReceiverType::from_raw(raw));
            }
        }
        if let Some(ret) = find_fun_return_type_by_name(indexer, receiver) {
            return Some(ReceiverType::from_raw(ret));
        }
        let file = ensure_file_data(indexer, from_uri)?;
        let ret = infer_callable_param_return_type(&file.lines, receiver)?;
        return Some(ReceiverType::from_raw(ret));
    }

    // Non-call expression: smart-cast, chain, variable, type name.
    if let Some(line) = cursor_line {
        let pos = Position::new(line, 0);
        if let Some(rt) = infer_receiver_type_at(indexer, receiver, from_uri, pos) {
            return Some(rt);
        }
    }

    if receiver.contains('.') {
        if let Some(raw) = resolve_dotted_receiver_type(indexer, receiver, from_uri) {
            return Some(ReceiverType::from_raw(raw));
        }
    }

    if let Some(rt) = infer_receiver_type(indexer, ReceiverKind::Variable(receiver), from_uri) {
        if let Some(ret) = extract_fn_type_return(&rt.raw) {
            return Some(ReceiverType::from_raw(ret));
        }
        return Some(rt);
    }

    if receiver.starts_with_uppercase() {
        return Some(ReceiverType::from_raw(receiver.to_string()));
    }

    // Fallback: function defined in scope (e.g. bare `productFlow` written without `()`).
    if let Some(ret) = find_fun_return_type_by_name(indexer, receiver) {
        return Some(ReceiverType::from_raw(ret));
    }
    let file = ensure_file_data(indexer, from_uri)?;
    let ret = infer_callable_param_return_type(&file.lines, receiver)?;
    Some(ReceiverType::from_raw(ret))
}

/// Iteratively resolve the type of a dot-separated receiver chain.
/// e.g. "MaterialTheme.colorScheme" -> "ColorScheme"
fn resolve_dotted_receiver_type(indexer: &Indexer, path: &str, uri: &Url) -> Option<String> {
    let segments: Vec<&str> = path.split('.').collect();
    if segments.is_empty() {
        return None;
    }

    let first = segments[0];
    let mut current_type = if let Some(type_name) = infer_variable_type_raw(indexer, first, uri) {
        type_name
    } else if first.starts_with(|c: char| c.is_uppercase()) {
        first.to_string()
    } else {
        return None;
    };

    for &segment in &segments[1..] {
        let current_base = current_type.split('<').next()?.trim();
        let current_base_leaf = current_base
            .rsplit('.')
            .next()?
            .trim()
            .trim_end_matches('?');

        let clean_segment = segment.trim_end_matches("()").trim();

        if let Some(next_type) = find_field_type_in_class(indexer, current_base_leaf, clean_segment)
        {
            current_type = next_type;
        } else if let Some(next_type) =
            find_method_return_type(indexer, current_base_leaf, clean_segment, Some(uri))
        {
            current_type = next_type;
        } else {
            return None;
        }
    }

    Some(current_type)
}

/// Extract the return type from a Kotlin function-type string.
///
/// `"(isRefresh: Boolean) -> Flow<ResultState<T>>"` → `"Flow<ResultState<T>>"`
/// `"() -> Unit"` → `"Unit"`
/// `"((Foo) -> Bar) -> Baz"` → `"Baz"` (depth-aware; not `"Bar) -> Baz"`)
fn extract_fn_type_return(fn_type: &str) -> Option<String> {
    let arrow = super::infer_lines::find_outer_arrow(fn_type)?;
    let ret = fn_type[arrow + 4..].trim();
    if ret.is_empty() {
        return None;
    }
    Some(ret.to_owned())
}

/// Resolve a dotted receiver chain to a `ReceiverType`.
///
/// Thin wrapper over `resolve_dotted_receiver_type` that skips contextual
/// keywords and converts the result to `ReceiverType`.  Exported for tests.
#[cfg(test)]
pub(crate) fn resolve_chain_receiver(
    indexer: &Indexer,
    chain: &str,
    from_uri: &Url,
) -> Option<ReceiverType> {
    const UNCHAINABLE: &[&str] = &["this", "super", "it", "self"];
    let head = chain.split('.').next()?;
    if UNCHAINABLE.contains(&head) {
        return None;
    }
    resolve_dotted_receiver_type(indexer, chain, from_uri).map(ReceiverType::from_raw)
}

fn resolve_dot_receiver_file(
    indexer: &Indexer,
    outer_type: &str,
    from_uri: &Url,
) -> Option<String> {
    Some(
        resolve_symbol_no_rg(indexer, outer_type, from_uri)
            .first()?
            .uri
            .to_string(),
    )
}

fn direct_dot_completion_items(
    indexer: &Indexer,
    context: &DotCompletionContext,
    from_uri: &Url,
    cursor_line: Option<u32>,
) -> Vec<CompletionItem> {
    symbols_from_nested_type(
        indexer,
        &context.file_uri,
        &context.receiver_type.leaf,
        CallerContext {
            uri: Some(from_uri.as_str()),
            cursor_line,
        },
    )
}

fn collect_inherited_dot_completion_items(
    indexer: &Indexer,
    context: &DotCompletionContext,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
    items: &mut Vec<CompletionItem>,
) {
    let caller = CallerContext {
        uri: Some(from_uri.as_str()),
        cursor_line,
    };
    let inherited = walk_hierarchy(
        indexer,
        &context.receiver_type.leaf,
        &context.file_uri,
        caller,
        4,
        |index, class_name, class_uri, hierarchy_caller| {
            let mut nested =
                symbols_from_nested_type(index, class_uri, class_name, hierarchy_caller);
            filter_inaccessible_completion_items(&mut nested);
            strip_completion_snippets(&mut nested, snippets);
            nested
        },
    );
    items.extend(inherited);
}

fn filter_inaccessible_completion_items(items: &mut Vec<CompletionItem>) {
    items.retain(|item| {
        item.sort_text
            .as_deref()
            .map(|sort_text| !sort_text.starts_with("prv:") && !sort_text.starts_with("prt:"))
            .unwrap_or(true)
    });
}

fn dedup_completion_labels(items: &mut Vec<CompletionItem>) {
    let mut seen_labels: std::collections::HashSet<String> = std::collections::HashSet::new();
    items.retain(|item| {
        !seen_labels.contains(item.label.as_str()) && seen_labels.insert(item.label.clone())
    });
}

fn strip_completion_snippets(items: &mut [CompletionItem], snippets: bool) {
    if snippets {
        return;
    }
    for item in items {
        item.insert_text = None;
        item.insert_text_format = None;
    }
}

fn append_dot_tail_completions(
    indexer: &Indexer,
    receiver_type: &ReceiverType,
    from_uri: &Url,
    snippets: bool,
    file_found: bool,
    items: &mut Vec<CompletionItem>,
) {
    let from_path = from_uri.path();
    // Stdlib fns (scope, collections, strings) are only meaningful when we confirmed a
    // concrete receiver type via file resolution. Skipping them for unresolved types
    // (e.g. generic type params like `T`) preserves the type-hint placeholder fallback.
    if file_found {
        items.extend(dot_completions_for_lang(
            from_path,
            &receiver_type.qualified,
            snippets,
        ));
    }
    if crate::Language::from_path(from_path) == crate::Language::Kotlin {
        // Extension functions from the reverse index: O(1) lookup, safe for any type.
        items.extend(extension_fn_completions(
            indexer,
            &receiver_type.outer,
            from_uri,
            snippets,
        ));
    }
}

/// Build a `CompletionItem` for a symbol found inside a nested type body.
///
/// Functions/methods get a snippet `name($1)`; all other kinds are plain-text.
/// The `sort_text` prefix is the `kind_sort_rank` value so the list is ordered
/// consistently with the rest of the completion results.
fn completion_item_for_nested_symbol(
    indexer: &Indexer,
    s: &crate::types::SymbolEntry,
    uri_str: &str,
    caller: CallerContext<'_>,
) -> CompletionItem {
    let kind = symbol_kind_to_completion(s.kind);
    let is_fn = matches!(
        kind,
        CompletionItemKind::FUNCTION | CompletionItemKind::METHOD
    );
    // Apply generic type param substitution when the symbol is from a different file.
    let detail_raw = if s.detail.is_empty() {
        None
    } else {
        Some(s.detail.clone())
    };
    let detail = detail_raw.map(|signature| match caller.uri {
        Some(calling_uri) => crate::indexer::resolution::cross_file_type_subst(
            indexer,
            uri_str,
            s.selection_start(),
            calling_uri,
            caller.cursor_line,
            &signature,
        ),
        None => signature,
    });
    let mut data = serde_json::json!({DATA_URI: uri_str, DATA_LINE: s.selection_start(), DATA_COL: s.selection_range.start.character});
    if let Some(calling_uri) = caller.uri {
        data[DATA_CALLING_URI] = serde_json::Value::String(calling_uri.to_owned());
    }
    CompletionItem {
        label: s.name.clone(),
        kind: Some(kind),
        insert_text: if is_fn {
            Some(format!("{}($1)", s.name))
        } else {
            None
        },
        insert_text_format: if is_fn {
            Some(InsertTextFormat::SNIPPET)
        } else {
            None
        },
        sort_text: Some(format!("{:02}:{}", kind_sort_rank(Some(kind)), s.name)),
        detail,
        command: if is_fn {
            Some(trigger_parameter_hints())
        } else {
            None
        },
        data: Some(data),
        ..Default::default()
    }
}

/// Return completions for symbols declared INSIDE `type_name` within the given file.
/// Uses the symbol's range end (the closing `}` of the class body) to determine
/// membership — no indentation heuristics needed.
fn symbols_from_nested_type(
    indexer: &Indexer,
    file_uri: &str,
    inner_name: &str,
    caller: CallerContext<'_>,
) -> Vec<CompletionItem> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };
    let Some(file_data) = ensure_file_data(indexer, &uri) else {
        return vec![];
    };
    let symbols = &file_data.symbols;

    // Prefer a type declaration (class/object/interface/enum) over a function with the
    // same name. Compose's MaterialTheme file declares both `fun MaterialTheme(...)` and
    // `object MaterialTheme { ... }` — taking the first match would pick the function and
    // return empty completions.
    let is_type_kind = |k: SymbolKind| {
        matches!(
            k,
            SymbolKind::CLASS
                | SymbolKind::OBJECT
                | SymbolKind::INTERFACE
                | SymbolKind::ENUM
                | SymbolKind::ENUM_MEMBER
                | SymbolKind::MODULE
                | SymbolKind::STRUCT
        )
    };
    let type_symbol = symbols
        .iter()
        .filter(|s| s.name == inner_name)
        .max_by_key(|s| u8::from(is_type_kind(s.kind)));
    let Some(type_symbol) = type_symbol else {
        return symbols
            .iter()
            .filter(|symbol| symbol.visibility != Visibility::Private)
            .map(|symbol| completion_item_for_nested_symbol(indexer, symbol, file_uri, caller))
            .collect();
    };

    let type_start = type_symbol.range.start;
    let type_end = type_symbol.range.end;
    symbols
        .iter()
        .filter(|symbol| {
            let start = symbol.range.start;
            let starts_after = start.line > type_start.line
                || (start.line == type_start.line && start.character > type_start.character);
            let starts_before = start.line < type_end.line
                || (start.line == type_end.line && start.character < type_end.character);
            starts_after && starts_before
        })
        .filter(|symbol| symbol.visibility != Visibility::Private)
        .map(|symbol| completion_item_for_nested_symbol(indexer, symbol, file_uri, caller))
        .collect()
}

/// Sort rank for completion item kinds: lower = appears earlier.
fn kind_sort_rank(kind: Option<CompletionItemKind>) -> u8 {
    match kind {
        Some(CompletionItemKind::FUNCTION) | Some(CompletionItemKind::METHOD) => 0,
        Some(CompletionItemKind::FIELD)
        | Some(CompletionItemKind::VARIABLE)
        | Some(CompletionItemKind::CONSTANT)
        | Some(CompletionItemKind::ENUM_MEMBER) => 1,
        Some(CompletionItemKind::CLASS)
        | Some(CompletionItemKind::INTERFACE)
        | Some(CompletionItemKind::ENUM)
        | Some(CompletionItemKind::MODULE) => 3,
        _ => 2,
    }
}

/// Returns the `sort_text` visibility prefix.
/// Private symbols get the `"prv:"` tag so `complete_dot` can filter them out.
fn vis_tag(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Private => "prv:",
        Visibility::Protected => "prt:",
        _ => "",
    }
}

/// Accumulates completion items across tiers, enforcing case-mode and dedup.
///
/// Tier-0 (same file), tier-1 (same pkg), and tier-3 (stdlib) all use the
/// symbol name as the dedup key. Tier-2 (cross-package) uses a `"name:fqn"`
/// key and is handled manually by `complete_bare` so per-FQN import edits
/// are preserved correctly.
struct BareCompleter {
    items: Vec<CompletionItem>,
    seen: std::collections::HashSet<String>,
    lowercase_mode: bool,
    uppercase_mode: bool,
    camel_mode: bool,
    local_max_score: u8,
    snippets: bool,
    annotation_only: bool,
}

impl BareCompleter {
    fn new(prefix: &str, snippets: bool, annotation_only: bool) -> Self {
        let first_char = prefix.chars().next();
        let lowercase_mode = first_char.map(|c| c.is_lowercase()).unwrap_or(false);
        let uppercase_mode = first_char.map(|c| c.is_uppercase()).unwrap_or(false);
        let camel_mode = uppercase_mode && prefix.chars().any(|c| c.is_lowercase());
        let local_max_score: u8 = if prefix.len() >= MIN_PREFIX_SCORE_REDUCTION {
            1
        } else {
            2
        };
        Self {
            items: Vec::new(),
            seen: std::collections::HashSet::new(),
            lowercase_mode,
            uppercase_mode,
            camel_mode,
            local_max_score,
            snippets,
            annotation_only,
        }
    }

    /// Add a symbol for tier 0 (same file) or tier 1 (same pkg).
    /// Dedup key is `name`. Respects case-mode, annotation-mode, and score gates.
    fn add(
        &mut self,
        name: &str,
        kind: CompletionItemKind,
        src_tier: u8,
        prefix: &str,
        detail: &str,
        item_data: Option<serde_json::Value>,
    ) {
        if self.annotation_only
            && matches!(
                kind,
                CompletionItemKind::FUNCTION
                    | CompletionItemKind::METHOD
                    | CompletionItemKind::VARIABLE
                    | CompletionItemKind::FIELD
                    | CompletionItemKind::PROPERTY
            )
        {
            return;
        }
        if self.lowercase_mode && name.starts_with_uppercase() {
            return;
        }
        if self.uppercase_mode && name.starts_with_lowercase() {
            return;
        }
        if self.camel_mode && is_screaming_snake(name) {
            return;
        }
        let score = match match_score(name, prefix) {
            Some(s) if s <= self.local_max_score => s,
            _ => return,
        };
        if !self.seen.insert(name.to_string()) {
            return;
        }
        let is_fn = self.snippets
            && !self.annotation_only
            && matches!(
                kind,
                CompletionItemKind::FUNCTION | CompletionItemKind::METHOD
            );
        self.items.push(CompletionItem {
            label: name.to_string(),
            kind: Some(kind),
            filter_text: Some(name.to_string()),
            sort_text: Some(format!("{}{}{}", src_tier, score, name.to_lowercase())),
            insert_text: if is_fn {
                Some(format!("{}($1)", name))
            } else {
                None
            },
            insert_text_format: if is_fn {
                Some(InsertTextFormat::SNIPPET)
            } else {
                None
            },
            detail: if detail.is_empty() {
                None
            } else {
                Some(detail.to_string())
            },
            command: if is_fn {
                Some(trigger_parameter_hints())
            } else {
                None
            },
            data: item_data,
            ..Default::default()
        });
    }
}

struct CurrentFileCompletionContext {
    imports: Vec<crate::types::ImportEntry>,
    package_name: String,
    lines: Arc<Vec<String>>,
    needs_semicolons: bool,
}

impl CurrentFileCompletionContext {
    fn from_indexer(indexer: &Indexer, from_uri: &Url) -> Self {
        let needs_semicolons = crate::Language::from_path(from_uri.as_str()).needs_semicolons();
        let live_lines = indexer
            .live_lines
            .get(from_uri.as_str())
            .map(|lines| lines.clone());
        let (imports, package_name, lines) = indexer
            .files
            .get(from_uri.as_str())
            .map(|file| {
                let lines = live_lines.clone().unwrap_or_else(|| file.lines.clone());
                let imports = if live_lines.is_some() {
                    lines.parse_imports()
                } else {
                    file.imports.clone()
                };
                (imports, file.package.clone().unwrap_or_default(), lines)
            })
            .unwrap_or_else(|| {
                let lines = live_lines.clone().unwrap_or_default();
                let imports = lines.parse_imports();
                (imports, String::new(), lines)
            });

        Self {
            imports,
            package_name,
            lines,
            needs_semicolons,
        }
    }

    fn needs_import(&self, fully_qualified_name: &str) -> bool {
        let qualifier = fully_qualified_name
            .rsplit_once('.')
            .map(|(qualifier, _)| qualifier)
            .unwrap_or_default();

        !already_imported(fully_qualified_name, &self.imports)
            && !self
                .imports
                .iter()
                .any(|import_entry| import_entry.is_star && import_entry.full_path == qualifier)
            && qualifier != self.package_name
    }
}

struct BareCompletionWalk<'a> {
    indexer: &'a Indexer,
    prefix: &'a str,
    from_uri: &'a Url,
    cursor_line: Option<u32>,
    completer: BareCompleter,
}

impl<'a> BareCompletionWalk<'a> {
    fn new(
        indexer: &'a Indexer,
        prefix: &'a str,
        from_uri: &'a Url,
        snippets: bool,
        annotation_only: bool,
        cursor_line: Option<u32>,
    ) -> Self {
        Self {
            indexer,
            prefix,
            from_uri,
            cursor_line,
            completer: BareCompleter::new(prefix, snippets, annotation_only),
        }
    }

    fn collect_local_file(&mut self) {
        let Some(file) = self.indexer.files.get(self.from_uri.as_str()) else {
            return;
        };

        for symbol in &file.symbols {
            self.completer.add(
                &symbol.name,
                symbol_kind_to_completion(symbol.kind),
                0,
                self.prefix,
                &symbol.detail,
                Some(serde_json::json!({DATA_URI: self.from_uri.as_str(), DATA_LINE: symbol.selection_start(), DATA_COL: symbol.selection_range.start.character})),
            );
        }

        if self.completer.lowercase_mode {
            for declared_name in &file.declared_names {
                self.completer.add(
                    declared_name,
                    CompletionItemKind::VARIABLE,
                    0,
                    self.prefix,
                    "",
                    None,
                );
            }
        }
    }

    fn collect_same_package(&mut self) {
        let Some(package_name) = self.current_package_name() else {
            return;
        };
        let Some(package_uris) = self.indexer.packages.get(&package_name) else {
            return;
        };
        let caller_source_set = self
            .indexer
            .files
            .get(self.from_uri.as_str())
            .map(|file| file.source_set)
            .unwrap_or_default();

        for package_uri in package_uris.iter() {
            if package_uri == self.from_uri.as_str() {
                continue;
            }
            let Some(file) = self.indexer.files.get(package_uri.as_str()) else {
                continue;
            };
            if file.source_set == SourceSet::Test && caller_source_set != SourceSet::Test {
                continue;
            }
            for symbol in &file.symbols {
                self.completer.add(
                    &symbol.name,
                    symbol_kind_to_completion(symbol.kind),
                    1,
                    self.prefix,
                    &symbol.detail,
                    Some(serde_json::json!({DATA_URI: package_uri.as_str(), DATA_LINE: symbol.selection_start(), DATA_COL: symbol.selection_range.start.character})),
                );
            }
        }
    }

    fn current_package_name(&self) -> Option<String> {
        self.indexer
            .files
            .get(self.from_uri.as_str())
            .and_then(|file| file.package.clone())
            .filter(|package_name| !package_name.is_empty())
    }

    /// Wave 2: functions and properties from star-imported packages (`import pkg.*`).
    ///
    /// Fills the gap between project-source symbols (wave 1) and the cross-package
    /// class-name index (wave 3). Covers lowercase symbols like `launch`, `flowOf`,
    /// `withContext`, etc., which are excluded from `collect_cross_package` (uppercase
    /// only) but are directly usable because they are already star-imported.
    fn collect_star_imported_functions(&mut self) {
        if self.completer.annotation_only || !self.completer.lowercase_mode {
            return;
        }
        // Resolve imports from live_lines when available so newly added imports work.
        let imports = self
            .indexer
            .live_lines
            .get(self.from_uri.as_str())
            .map(|ll| ll.parse_imports())
            .or_else(|| {
                self.indexer
                    .files
                    .get(self.from_uri.as_str())
                    .map(|f| f.imports.clone())
            })
            .unwrap_or_default();

        let caller_source_set = self
            .indexer
            .files
            .get(self.from_uri.as_str())
            .map(|f| f.source_set)
            .unwrap_or_default();

        for import in &imports {
            if !import.is_star {
                continue;
            }
            let Some(pkg_uris) = self.indexer.packages.get(&import.full_path) else {
                continue;
            };
            for pkg_uri in pkg_uris.iter() {
                if pkg_uri.as_str() == self.from_uri.as_str() {
                    continue; // already covered by collect_local_file
                }
                let Some(file) = self.indexer.files.get(pkg_uri.as_str()) else {
                    continue;
                };
                if file.source_set == crate::types::SourceSet::Test
                    && caller_source_set != crate::types::SourceSet::Test
                {
                    continue;
                }
                for symbol in &file.symbols {
                    // Classes / interfaces / objects / enums are handled by
                    // collect_cross_package; skip them here to avoid tier inflation.
                    if matches!(
                        symbol.kind,
                        SymbolKind::CLASS
                            | SymbolKind::INTERFACE
                            | SymbolKind::STRUCT
                            | SymbolKind::ENUM
                            | SymbolKind::OBJECT
                    ) {
                        continue;
                    }
                    self.completer.add(
                        &symbol.name,
                        symbol_kind_to_completion(symbol.kind),
                        1, // same tier as same-package
                        self.prefix,
                        &symbol.detail,
                        Some(serde_json::json!({
                            DATA_URI: pkg_uri.as_str(),
                            DATA_LINE: symbol.selection_start(),
                            DATA_COL: symbol.selection_range.start.character
                        })),
                    );
                }
            }
        }
    }

    fn collect_cross_package(&mut self) {
        // Only run for uppercase-starting prefixes — the bare_name_cache holds
        // class names (PascalCase/SCREAMING_SNAKE), so digits, underscores, or
        // lowercase prefixes produce zero matches at the cost of a full scan.
        // Exception: annotation context (@) must scan even with an empty prefix
        // so that typing `@` alone yields results and keeps the session open.
        if !self.completer.uppercase_mode && !self.completer.annotation_only {
            return;
        }

        let current_context =
            CurrentFileCompletionContext::from_indexer(self.indexer, self.from_uri);
        self.indexer.ensure_bare_names_fresh();
        let Ok(cache) = self.indexer.bare_name_cache.read() else {
            return;
        };

        for bare_name in cache.iter() {
            self.add_cross_package_name(bare_name, &current_context);
        }
    }

    fn add_cross_package_name(
        &mut self,
        bare_name: &str,
        current_context: &CurrentFileCompletionContext,
    ) {
        if bare_name.starts_with_lowercase() {
            return;
        }
        if self.completer.camel_mode && is_screaming_snake(bare_name) {
            return;
        }
        let Some(score) = self.cross_package_score(bare_name) else {
            return;
        };
        if self.completer.seen.contains(bare_name) {
            return;
        }

        let fully_qualified_names = fqns_for_name(self.indexer, bare_name);
        if fully_qualified_names.is_empty() {
            self.add_cross_package_name_without_imports(bare_name, score);
            return;
        }

        for fully_qualified_name in &fully_qualified_names {
            self.add_cross_package_symbol(bare_name, fully_qualified_name, score, current_context);
        }
    }

    fn cross_package_score(&self, bare_name: &str) -> Option<u8> {
        // For single-char prefixes, only allow score-0 (case-insensitive prefix
        // match); camel-acronym matching (score 1) is too noisy for one character.
        // Use char count so a single non-ASCII char (len >= 2 bytes) is treated
        // correctly as a single character.
        let max_score: u8 = if self.prefix.chars().count() < MIN_CAMEL_ACRONYM_PREFIX {
            0
        } else {
            1
        };
        match match_score(bare_name, self.prefix) {
            Some(score) if score <= max_score => Some(score),
            _ => None,
        }
    }

    fn add_cross_package_name_without_imports(&mut self, bare_name: &str, score: u8) {
        if !self.completer.seen.insert(bare_name.to_string()) {
            return;
        }

        self.completer.items.push(CompletionItem {
            label: bare_name.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            filter_text: Some(bare_name.to_string()),
            sort_text: Some(format!("2{}:{}", score, bare_name.to_lowercase())),
            ..Default::default()
        });
    }

    fn add_cross_package_symbol(
        &mut self,
        bare_name: &str,
        fully_qualified_name: &str,
        score: u8,
        current_context: &CurrentFileCompletionContext,
    ) {
        let item_key = format!("{}:{}", bare_name, fully_qualified_name);
        if !self.completer.seen.insert(item_key) {
            return;
        }

        let qualifier = fully_qualified_name
            .rsplit_once('.')
            .map(|(qualifier, _)| qualifier)
            .unwrap_or_default();
        let needs_import = current_context.needs_import(fully_qualified_name);
        let additional_text_edits = needs_import.then(|| {
            vec![current_context
                .lines
                .make_import_edit(fully_qualified_name, current_context.needs_semicolons)]
        });
        let detail = needs_import.then(|| qualifier.to_string());

        self.completer.items.push(CompletionItem {
            label: bare_name.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            filter_text: Some(bare_name.to_string()),
            sort_text: Some(format!("2{}:{}", score, bare_name.to_lowercase())),
            detail,
            additional_text_edits,
            ..Default::default()
        });
    }

    fn collect_stdlib(&mut self) {
        // Kotlin stdlib contains no annotation classes — skip entirely in annotation context.
        if self.completer.annotation_only {
            return;
        }
        for mut item in bare_completions(self.completer.snippets) {
            if self.completer.lowercase_mode && item.label.starts_with_uppercase() {
                continue;
            }
            if self.completer.uppercase_mode && item.label.starts_with_lowercase() {
                continue;
            }
            if self.completer.camel_mode && is_screaming_snake(&item.label) {
                continue;
            }
            let score = match match_score(&item.label, self.prefix) {
                Some(score) if score <= 2 => score,
                _ => continue,
            };
            if !self.completer.seen.contains(item.label.as_str()) {
                self.completer.seen.insert(item.label.clone());
                item.sort_text = Some(format!("3{}:{}", score, item.label.to_lowercase()));
                item.filter_text = Some(item.label.clone());
                self.completer.items.push(item);
            }
        }
    }

    /// Collect bare-word extension members available on `this` — i.e., extension
    /// functions/properties whose receiver is a supertype of the enclosing class.
    ///
    /// Example: inside `DashboardProductsViewModel`, `viewModelScope` is available
    /// because `val ViewModel.viewModelScope` is an extension property on `ViewModel`
    /// and `DashboardProductsViewModel` inherits from it.
    fn collect_this_extensions(&mut self) {
        // Only Kotlin files can consume Kotlin extension functions.
        if crate::Language::from_path(self.from_uri.as_str()) != crate::Language::Kotlin {
            return;
        }
        // Annotations never need extension functions.
        if self.completer.annotation_only {
            return;
        }
        let cursor_line = match self.cursor_line {
            Some(line) => line,
            None => return,
        };

        // Find the enclosing class name at the cursor position.
        let enclosing_class = match self.indexer.enclosing_class_at(self.from_uri, cursor_line) {
            Some(name) => name,
            None => return,
        };

        // Resolve the enclosing class to find its file URI.
        let class_locations = resolve_symbol_no_rg(self.indexer, &enclosing_class, self.from_uri);
        let class_uri = match class_locations.into_iter().next() {
            Some(loc) => loc.uri.to_string(),
            None => return,
        };

        // Collect all ancestor type names (including the class itself).
        // The hierarchy is stable within a session — cache it to avoid re-running
        // walk_hierarchy + resolve_symbol_no_rg (depth-8 traversal) on every line change.
        let cache_key = format!("{}@{}", enclosing_class, class_uri);
        let ancestor_names: std::sync::Arc<std::collections::HashSet<String>> = self
            .indexer
            .this_ext_ancestor_cache
            .get(&cache_key)
            .map(|r| std::sync::Arc::clone(&*r))
            .unwrap_or_else(|| {
                let mut set = std::collections::HashSet::from([enclosing_class.clone()]);
                let caller = CallerContext {
                    uri: Some(self.from_uri.as_str()),
                    cursor_line: self.cursor_line,
                };
                let supers: Vec<String> = walk_hierarchy(
                    self.indexer,
                    &enclosing_class,
                    &class_uri,
                    caller,
                    8,
                    |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
                );
                set.extend(supers);
                let arc = std::sync::Arc::new(set);
                self.indexer
                    .this_ext_ancestor_cache
                    .insert(cache_key, std::sync::Arc::clone(&arc));
                arc
            });

        // Build the extension completion context (import tracking, package).
        let ext_context = ExtensionCompletionContext::build(self.indexer, self.from_uri);
        let builder = ExtensionCompletionBuilder::new(&ext_context, "", self.completer.snippets);

        // Use the reverse index: O(ancestors × entries_per_receiver) instead of O(all_files).
        let prefix = self.prefix;
        for ancestor in ancestor_names.iter() {
            let Some(entries) = self.indexer.extension_by_receiver.get(ancestor) else {
                continue;
            };
            for entry in entries.iter() {
                if crate::Language::from_path(&entry.file_uri) != crate::Language::Kotlin {
                    continue;
                }
                let is_same_file = entry.file_uri == ext_context.from_uri;
                let is_library = is_library_extension(self.indexer, &entry.file_uri);
                if !is_same_file
                    && (matches!(
                        entry.visibility,
                        Visibility::Private | Visibility::Protected
                    ) || (is_library && entry.visibility == Visibility::Internal))
                {
                    continue;
                }
                // Hide deprecated library symbols; workspace-deprecated ones are
                // kept and tagged/deprioritized by `build_item_from_entry`.
                if entry.deprecated && is_library {
                    continue;
                }
                if match_score(&entry.name, prefix).is_none() {
                    continue;
                }
                if self.completer.seen.contains(&entry.name) {
                    continue;
                }
                let item = builder.build_item_from_entry(entry, is_same_file);
                if self.completer.seen.insert(entry.name.clone()) {
                    self.completer.items.push(item);
                }
            }
        }
    }

    fn finish(mut self) -> (Vec<CompletionItem>, bool) {
        self.completer
            .items
            .sort_by(|left, right| left.sort_text.cmp(&right.sort_text));

        let hit_cap = self.completer.items.len() > COMPLETION_CAP;
        self.completer.items.truncate(COMPLETION_CAP);
        (self.completer.items, hit_cap)
    }
}

/// Bare-word completion: match-scored across local file + same-package + index.
///
/// Case heuristic:
/// - **Lowercase prefix** → only return symbols whose name starts with a
///   lowercase letter (local vars, params, fields, fun names).  Class names are
///   excluded because they are rarely what the user wants when typing `acc…`.
/// - **Uppercase prefix or empty** → return everything (class names + members).
///
/// Returns `(items, hit_cap)` — callers should propagate `hit_cap` to
/// `CompletionList.is_incomplete` so the client re-queries each keystroke.
pub(crate) fn complete_bare(
    indexer: &Indexer,
    prefix: &str,
    from_uri: &Url,
    snippets: bool,
    annotation_only: bool,
    cursor_line: Option<u32>,
) -> (Vec<CompletionItem>, bool) {
    let start_time = std::time::Instant::now();
    let mut completion_walk = BareCompletionWalk::new(
        indexer,
        prefix,
        from_uri,
        snippets,
        annotation_only,
        cursor_line,
    );
    completion_walk.collect_local_file();
    log::debug!("bare: local_file {}ms", start_time.elapsed().as_millis());
    completion_walk.collect_same_package();
    log::debug!("bare: same_package {}ms", start_time.elapsed().as_millis());
    completion_walk.collect_star_imported_functions();
    log::debug!("bare: star_imported {}ms", start_time.elapsed().as_millis());
    completion_walk.collect_cross_package();
    log::debug!("bare: cross_package {}ms", start_time.elapsed().as_millis());
    completion_walk.collect_stdlib();
    log::debug!("bare: stdlib {}ms", start_time.elapsed().as_millis());
    completion_walk.collect_this_extensions();
    log::debug!(
        "bare: this_extensions {}ms",
        start_time.elapsed().as_millis()
    );
    completion_walk.finish()
}

/// Collect all symbols from a file URI as completion items.
/// Results are cached in `indexer.completion_cache` so the file is only parsed
/// (or converted) once; subsequent calls for the same URI return instantly.
fn symbols_from_uri_as_completions(indexer: &Indexer, file_uri: &str) -> Vec<CompletionItem> {
    // Fast path: already computed.
    if let Some(cached) = indexer.completion_cache.get(file_uri) {
        return cached.as_ref().clone();
    }

    let items = build_completion_items(indexer, file_uri);
    indexer
        .completion_cache
        .insert(file_uri.to_string(), Arc::new(items.clone()));
    items
}

/// Build completion items for a file, from index or on-demand disk parse.
/// Always builds with snippet fields set; callers strip them if the client
/// doesn't support snippets.
fn build_completion_items(indexer: &Indexer, file_uri: &str) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // From index if available.
    if let Some(f) = indexer.files.get(file_uri) {
        for symbol in &f.symbols {
            let ck = symbol_kind_to_completion(symbol.kind);
            let vt = vis_tag(symbol.visibility);
            let sort_txt = format!("{vt}{}{}", kind_sort_rank(Some(ck)), symbol.name);
            items.push(make_completion_item(&symbol.name, ck, sort_txt, true));
        }
        for name in &f.declared_names {
            if !items.iter().any(|i: &CompletionItem| i.label == *name) {
                items.push(make_completion_item(
                    name,
                    CompletionItemKind::FIELD,
                    format!("1{name}"),
                    true,
                ));
            }
        }
        return items;
    }

    // Fall back to on-demand parse.
    if let Ok(url) = Url::parse(file_uri) {
        if let Ok(path) = url.to_file_path() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let file_data = parse_by_extension(file_uri, &content);
                for symbol in &file_data.symbols {
                    let ck = symbol_kind_to_completion(symbol.kind);
                    let vt = vis_tag(symbol.visibility);
                    let sort_txt = format!("{vt}{}{}", kind_sort_rank(Some(ck)), symbol.name);
                    items.push(make_completion_item(&symbol.name, ck, sort_txt, true));
                }
                for name in &file_data.declared_names {
                    if !items.iter().any(|i: &CompletionItem| i.label == *name) {
                        items.push(make_completion_item(
                            name,
                            CompletionItemKind::FIELD,
                            format!("1{name}"),
                            true,
                        ));
                    }
                }
            }
        }
    }
    items
}

fn symbol_kind_to_completion(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::FUNCTION | SymbolKind::METHOD => CompletionItemKind::FUNCTION,
        SymbolKind::CLASS => CompletionItemKind::CLASS,
        SymbolKind::INTERFACE => CompletionItemKind::INTERFACE,
        SymbolKind::ENUM => CompletionItemKind::ENUM,
        SymbolKind::ENUM_MEMBER => CompletionItemKind::ENUM_MEMBER,
        SymbolKind::CONSTANT => CompletionItemKind::CONSTANT,
        SymbolKind::VARIABLE => CompletionItemKind::VARIABLE,
        SymbolKind::OBJECT | SymbolKind::MODULE => CompletionItemKind::MODULE,
        _ => CompletionItemKind::VALUE,
    }
}

/// Build a single `CompletionItem` for a named symbol.
///
/// Functions and methods get a snippet `name($1)` so the cursor lands inside
/// the parentheses after accepting the completion.  All other kinds are plain
/// text insertions.
fn make_completion_item(
    name: &str,
    ck: CompletionItemKind,
    sort_text: String,
    snippets: bool,
) -> CompletionItem {
    let is_fn = snippets
        && matches!(
            ck,
            CompletionItemKind::FUNCTION | CompletionItemKind::METHOD
        );
    CompletionItem {
        label: name.to_string(),
        kind: Some(ck),
        sort_text: Some(sort_text),
        insert_text: if is_fn {
            Some(format!("{}($1)", name))
        } else {
            None
        },
        insert_text_format: if is_fn {
            Some(InsertTextFormat::SNIPPET)
        } else {
            None
        },
        command: if is_fn {
            Some(trigger_parameter_hints())
        } else {
            None
        },
        ..Default::default()
    }
}

/// Public wrapper around `symbols_from_uri_as_completions` for use by the
/// pre-warmer in `indexer.rs`.  Builds + caches completion items for a file.
pub(crate) fn symbols_from_uri_as_completions_pub(
    indexer: &Indexer,
    file_uri: &str,
) -> Vec<CompletionItem> {
    symbols_from_uri_as_completions(indexer, file_uri)
}

/// LSP `Command` that tells the editor to open the parameter-hints (signature
/// help) popup immediately after a function completion is accepted.
/// Mirrors VS Code's built-in `editor.action.triggerParameterHints` command,
/// which is also what rust-analyzer emits.
fn trigger_parameter_hints() -> tower_lsp::lsp_types::Command {
    tower_lsp::lsp_types::Command {
        title: "triggerParameterHints".into(),
        command: "editor.action.triggerParameterHints".into(),
        arguments: None,
    }
}

// ─── impl Indexer wrappers ────────────────────────────────────────────────────

#[allow(dead_code)]
impl crate::indexer::Indexer {
    pub(crate) fn complete_dot(
        &self,
        receiver: &str,
        from_uri: &Url,
        snippets: bool,
    ) -> Vec<CompletionItem> {
        complete_dot(self, receiver, from_uri, snippets, None)
    }
    pub(crate) fn complete_bare(
        &self,
        prefix: &str,
        from_uri: &Url,
        snippets: bool,
        annotation_only: bool,
    ) -> (Vec<CompletionItem>, bool) {
        complete_bare(self, prefix, from_uri, snippets, annotation_only, None)
    }
    pub(super) fn complete_super_w(&self, from_uri: &Url, snippets: bool) -> Vec<CompletionItem> {
        complete_super(self, from_uri, snippets)
    }
    pub(super) fn symbols_from_uri_as_completions_w(&self, file_uri: &str) -> Vec<CompletionItem> {
        symbols_from_uri_as_completions(self, file_uri)
    }
    pub(super) fn build_completion_items_w(&self, file_uri: &str) -> Vec<CompletionItem> {
        build_completion_items(self, file_uri)
    }
    pub(crate) fn symbols_from_uri_as_completions_pub(
        &self,
        file_uri: &str,
    ) -> Vec<CompletionItem> {
        symbols_from_uri_as_completions_pub(self, file_uri)
    }
}

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
