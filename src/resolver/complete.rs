use std::sync::Arc;
use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, CompletionItemTag,
    InsertTextFormat, Location, Position, SymbolKind, Url,
};

use crate::indexer::Indexer;
use crate::parser::parse_by_extension;
use crate::stdlib::bare_completions;
use crate::stdlib_tail::dot_completions_for_lang;
use crate::types::{CallerContext, ImportEntry, SourceSet, SymbolEntry, Visibility};
use crate::LinesExt;
use crate::StrExt;

use super::infer::{infer_receiver_type, infer_receiver_type_at, ReceiverKind, ReceiverType};
use super::infer_lines::infer_callable_param_return_type;
use super::resolve::{jar_symbol_package, range_encloses};
use super::{
    already_imported, ensure_file_data, fqns_for_name, resolve_symbol_no_rg, walk_hierarchy,
    Resolver, MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
};

/// Throttle counter for [`members_for_jar_backed_type`]'s `jar_symbol_packages`
/// side-table misalignment warning — see [`crate::util::throttled_warn`].
static JAR_SYMBOL_PACKAGES_MISALIGNED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

// ─── CompletionItem.data JSON keys ───────────────────────────────────────────

/// Symbol definition URI.
pub(crate) const DATA_URI: &str = "u";
/// Symbol definition line (0-based).
pub(crate) const DATA_LINE: &str = "l";
/// Symbol definition UTF-16 column (0-based).
pub(crate) const DATA_COL: &str = "c";
/// Calling-site URI, present only for cross-file substitution context.
pub(crate) const DATA_CALLING_URI: &str = "cu";
/// Fully-qualified name of an UNMATERIALIZED jar-backed candidate (stub).
/// Present instead of `DATA_URI`/`DATA_LINE`: the symbol has no location
/// yet — `completionItem/resolve` materializes it on demand from this FQN
/// (one user-selected candidate, unbudgeted like hover).
pub(crate) const DATA_FQN: &str = "f";

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

/// Maximum number of synchronous, blocking `ensure_jar_materialized` calls a
/// single `complete_bare` request will attempt. Each attempt is a real
/// sidecar IPC round trip (not just the `try_lock_sidecar_bounded` mutex
/// attempt) — a short/ambiguous prefix can match dozens of Tier-1-only
/// candidates at once, and without a cap a single completion request can
/// fan out into many sequential round trips (measured against a real
/// ~756-JAR Gradle cache: ~17 sequential promotions totaling ~20s for one
/// response). Candidates beyond the cap are still offered by name (Task 9's
/// Tier-1 merge into `bare_name_cache` guarantees that independent of
/// promotion) — they just keep the name-only/qualifier-stub `detail` for
/// this request; a later request (narrowed prefix, hover, goto-def) can
/// still promote them individually.
pub(crate) const MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION: usize = 5;

/// Per-request ceiling on TOTAL jar materializations triggered by the
/// cross-package bare-name walk — cache-backed promotions cost no sidecar
/// IPC, but `populate_from_symbols` on a large AAR is real CPU and a real
/// memory step, and this walk can match hundreds of Tier-1-only names in
/// one request (short prefixes, fresh process, warm disk cache). Generous:
/// realistic fan-outs (a few dozen jars, once per session) fit under it;
/// only the pathological first-keystroke storm is clipped, and clipped
/// names still appear by name via the Tier-1 merge.
const MAX_CACHE_BACKED_MATERIALIZATIONS_PER_COMPLETION: usize = 32;

/// Dot-completion receiver derived from the CST (speculative marker parse).
///
/// Built by `features::completion_context::derive_dot_receiver` while the
/// speculative tree is alive; only this owned value flows downstream.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DotReceiver {
    /// `it`, `this`, `this@label` — resolved by `ScopeContext`, routed to
    /// `complete_lambda_dot` before member collection.
    Scope(String),
    Super,
    /// Any other receiver expression.
    Expr {
        /// Receiver text — for a call receiver, the callee text (the final
        /// argument list is implied by `is_call`). Feeds the retained
        /// text-keyed type fallbacks.
        text: String,
        /// The receiver subtree was a `call_expression`.
        is_call: bool,
        /// Type resolved by `CstQuery::expr_type` at analysis time. `None`
        /// for simple identifiers (smart-cast must get first look) and for
        /// CST-unresolvable receivers.
        resolved: Option<String>,
    },
}

impl DotReceiver {
    /// Plain variable / type-name receiver (the `complete_symbol` entry).
    pub(crate) fn expr(text: &str) -> Self {
        Self::Expr {
            text: text.to_owned(),
            is_call: false,
            resolved: None,
        }
    }

    pub(crate) fn text(&self) -> &str {
        match self {
            Self::Scope(text) | Self::Expr { text, .. } => text,
            Self::Super => "super",
        }
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
        dot_receiver.map(DotReceiver::expr),
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
    dot_receiver: Option<DotReceiver>,
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
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
            |_idx, super_name, _super_uri, _caller| vec![super_name.to_owned()],
        );
        ancestor_set.extend(supers);
    }

    let context = ExtensionCompletionContext::build(indexer, from_uri);
    let mut builder = ExtensionCompletionBuilder::new(&context, receiver_type, snippets);

    // Bounded across the whole ancestor walk, not per-ancestor: a common
    // receiver type ("String", "Iterable") can be declared on by dozens of
    // library JARs, so without a shared budget a single dot-completion could
    // trigger dozens of blocking sidecar round trips — the same cold-start
    // stall Task 12's review already found and capped for cross-package
    // completion (`MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION`).
    let mut jar_promotion_budget = MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION;
    for ancestor in &ancestor_set {
        // Atomic promote+read: `extension_by_receiver` is Tier 2 (populated
        // only by full JAR materialization), so a not-yet-touched JAR's
        // extensions (e.g. `viewModelScope`) would be silently invisible
        // without the accessor's promotion.
        if let Some(entries) =
            crate::indexer::jar::extension_entries_for(indexer, ancestor, &mut jar_promotion_budget)
        {
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
        // A member extension (`container: Some(_)`) has no import syntax in
        // Kotlin at all, and this builder has no evidence its declaring
        // container is actually an active receiver at the completion site —
        // offering it here would be a completion outside its real scope,
        // paired with an auto-import `build_item_from_entry` would compute
        // for a package.name Kotlin cannot use for an interface member.
        if entry.container.is_some() {
            return;
        }
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
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
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
///
/// Test-only thin wrapper over [`complete_dot_expr`]; production callers build
/// the [`DotReceiver`] directly.
#[cfg(test)]
pub(crate) fn complete_dot(
    indexer: &Indexer,
    receiver: &str,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> Vec<CompletionItem> {
    complete_dot_expr(
        indexer,
        &DotReceiver::expr(receiver),
        from_uri,
        snippets,
        cursor_line,
    )
}

fn complete_dot_expr(
    indexer: &Indexer,
    expr: &DotReceiver,
    from_uri: &Url,
    snippets: bool,
    cursor_line: Option<u32>,
) -> Vec<CompletionItem> {
    if matches!(expr, DotReceiver::Super) || expr.text() == "super" {
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

/// Inputs shared by every fallback strategy below.
struct ReceiverTypeCtx<'a> {
    indexer: &'a Indexer,
    receiver: &'a str,
    from_uri: &'a Url,
    cursor_line: Option<u32>,
}

/// Smart-cast narrowing at the cursor (`if (x is Foo) { x.<here> }`), which
/// must win over the variable's raw declared type below.
fn smart_cast_at_cursor(ctx: &ReceiverTypeCtx<'_>) -> Option<ReceiverType> {
    let pos = Position::new(ctx.cursor_line?, 0);
    infer_receiver_type_at(ctx.indexer, ctx.receiver, ctx.from_uri, pos)
}

/// The receiver as a declared variable (field/param/local), unwrapping a
/// callable-typed variable used bare (`val make: () -> Foo` as `make.`).
///
/// Ahead of `uppercase_type_name`: a variable whose name happens to start
/// uppercase is still a variable, not a type-name receiver.
fn variable_type(ctx: &ReceiverTypeCtx<'_>) -> Option<ReceiverType> {
    let resolved = infer_receiver_type(
        ctx.indexer,
        ReceiverKind::Variable(ctx.receiver),
        ctx.from_uri,
    )?;
    match extract_fn_type_return(&resolved.raw) {
        Some(ret) => Some(ReceiverType::from_raw(ret)),
        None => Some(resolved),
    }
}

/// An uppercase identifier nothing bound to a variable — far more likely a
/// type name (`String.format`) than the parenthesis-less function below.
fn uppercase_type_name(ctx: &ReceiverTypeCtx<'_>) -> Option<ReceiverType> {
    ctx.receiver
        .starts_with_uppercase()
        .then(|| ReceiverType::from_raw(ctx.receiver.to_owned()))
}

/// Last resort: a function in scope written without `()`
/// (bare `productFlow` used as `productFlow.collect { }`).
fn bare_scope_function_return_type(ctx: &ReceiverTypeCtx<'_>) -> Option<ReceiverType> {
    fn_or_callable_param_return_type(ctx.indexer, ctx.receiver, ctx.from_uri)
}

/// Resolve a bare name to a return type: global function first, then
/// callable-param inference from the file's own lines.
fn fn_or_callable_param_return_type(
    indexer: &Indexer,
    receiver: &str,
    from_uri: &Url,
) -> Option<ReceiverType> {
    if let Some(ret) = indexer.function_return_type(receiver, from_uri) {
        return Some(ReceiverType::from_raw(ret.into_inner()));
    }
    let file = ensure_file_data(indexer, from_uri)?;
    let ret = infer_callable_param_return_type(&file.lines, receiver)?;
    Some(ReceiverType::from_raw(ret))
}

fn resolve_dot_receiver_type(
    indexer: &Indexer,
    expr: &DotReceiver,
    from_uri: &Url,
    cursor_line: Option<u32>,
) -> Option<ReceiverType> {
    let (receiver, is_call, resolved) = match expr {
        DotReceiver::Expr {
            text,
            is_call,
            resolved,
        } => (text.as_str(), *is_call, resolved.as_deref()),
        // Scope receivers are routed to complete_lambda_dot before member
        // collection; reaching here means a plain-text receiver from the
        // complete_symbol entry — treat as a non-call expression.
        DotReceiver::Scope(text) => (text.as_str(), false, None),
        DotReceiver::Super => return None,
    };

    // Speculative-parse-time inference (smart-cast already applied) beats
    // re-deriving the same answer from scratch.
    if let Some(resolved) = resolved {
        return Some(ReceiverType::from_raw(resolved.to_owned()));
    }
    // A call receiver's text is the CALLEE name, so it resolves one way only:
    // consulting the fallbacks below would match a same-named variable or
    // class that has nothing to do with `make().`.
    if is_call {
        return fn_or_callable_param_return_type(indexer, receiver, from_uri);
    }

    let ctx = ReceiverTypeCtx {
        indexer,
        receiver,
        from_uri,
        cursor_line,
    };
    // Ordered most- to least-authoritative; first match wins. Each entry
    // documents why it outranks the next.
    let fallbacks: [fn(&ReceiverTypeCtx<'_>) -> Option<ReceiverType>; 4] = [
        smart_cast_at_cursor,
        variable_type,
        uppercase_type_name,
        bare_scope_function_return_type,
    ];
    fallbacks.iter().find_map(|strategy| strategy(&ctx))
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

fn resolve_dot_receiver_file(
    indexer: &Indexer,
    outer_type: &str,
    from_uri: &Url,
) -> Option<String> {
    // A receiver type declared only in a not-yet-materialized JAR is
    // invisible to `resolve_symbol_no_rg` (Tier-2 `jar_definitions` reads) —
    // promote it first, or this returns `None` and BOTH direct-member and
    // inherited-member completion are skipped wholesale for that receiver.
    let mut unbudgeted = usize::MAX;
    crate::indexer::jar::ensure_jar_definitions_for(indexer, outer_type, &mut unbudgeted);
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
        MembershipContext::Direct,
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
        MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
        |index, class_name, class_uri, hierarchy_caller| {
            let mut nested = symbols_from_nested_type(
                index,
                class_uri,
                class_name,
                hierarchy_caller,
                MembershipContext::Inherited,
            );
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
    symbol: &SymbolEntry,
    uri_str: &str,
    caller: CallerContext<'_>,
) -> CompletionItem {
    let kind = symbol_kind_to_completion(symbol.kind);
    let is_fn = matches!(
        kind,
        CompletionItemKind::FUNCTION | CompletionItemKind::METHOD
    );
    // Apply generic type param substitution when the symbol is from a different file.
    let detail_raw = if symbol.detail.is_empty() {
        None
    } else {
        Some(symbol.detail.clone())
    };
    let detail = detail_raw.map(|signature| match caller.uri {
        Some(calling_uri) => crate::indexer::resolution::cross_file_type_subst(
            indexer,
            uri_str,
            symbol.selection_start(),
            calling_uri,
            caller.cursor_line,
            &signature,
        ),
        None => signature,
    });
    let mut data = serde_json::json!({DATA_URI: uri_str, DATA_LINE: symbol.selection_start(), DATA_COL: symbol.selection_range.start.character});
    if let Some(calling_uri) = caller.uri {
        data[DATA_CALLING_URI] = serde_json::Value::String(calling_uri.to_owned());
    }
    CompletionItem {
        label: symbol.name.clone(),
        kind: Some(kind),
        insert_text: if is_fn {
            Some(format!("{}($1)", symbol.name))
        } else {
            None
        },
        insert_text_format: if is_fn {
            Some(InsertTextFormat::SNIPPET)
        } else {
            None
        },
        sort_text: Some(format!("{:02}:{}", kind_sort_rank(Some(kind)), symbol.name)),
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

/// Which symbol represents `inner_name` when several share that name.
///
/// A type declaration (or enum case) outranks a same-named function: Compose's
/// `MaterialTheme` file declares both `fun MaterialTheme(...)` and `object
/// MaterialTheme { … }`, and picking the function returns no members at all.
///
/// `is_container_kind` (`parser.rs`) is the canonical "does this kind nest
/// other symbols" predicate — the same one `assign_containers` uses to
/// populate the `container` field this module reads back. `ENUM_MEMBER` is
/// the deliberate delta: an enum case nests nothing, but its declaration
/// should still outrank an identically-named function.
fn is_preferred_type_symbol_kind(kind: SymbolKind) -> bool {
    crate::parser::is_container_kind(kind) || kind == SymbolKind::ENUM_MEMBER
}

/// How the caller reached `inner_name`'s members, which decides whether
/// nested type declarations belong in the result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MembershipContext {
    /// The receiver IS `inner_name`. `Outer.Nested` is a real Kotlin
    /// expression, so nested types are offered alongside instance members.
    Direct,
    /// `inner_name` is an ancestor whose members are folded into a
    /// descendant instance (`walk_hierarchy`). Only instance members are
    /// inherited — a nested type declaration never is.
    Inherited,
}

fn member_kind_allowed(context: MembershipContext, kind: SymbolKind) -> bool {
    context == MembershipContext::Direct || !crate::parser::is_container_kind(kind)
}

/// Completions for the members of `inner_name`, as declared in `file_uri`.
fn symbols_from_nested_type(
    indexer: &Indexer,
    file_uri: &str,
    inner_name: &str,
    caller: CallerContext<'_>,
    context: MembershipContext,
) -> Vec<CompletionItem> {
    let Ok(uri) = Url::parse(file_uri) else {
        return vec![];
    };
    let Some(file_data) = ensure_file_data(indexer, &uri) else {
        return vec![];
    };
    let symbols = &file_data.symbols;

    let type_symbol = symbols
        .iter()
        .filter(|s| s.name == inner_name)
        .max_by_key(|s| u8::from(is_preferred_type_symbol_kind(s.kind)));
    let Some(type_symbol) = type_symbol else {
        return symbols
            .iter()
            .filter(|symbol| symbol.visibility != Visibility::Private)
            .filter(|symbol| member_kind_allowed(context, symbol.kind))
            .map(|symbol| completion_item_for_nested_symbol(indexer, symbol, file_uri, caller))
            .collect();
    };

    // A JAR's synthetic `FileData` spans the whole archive and gives every
    // symbol a one-line range, so it needs a package axis the per-file
    // workspace path has no equivalent of — hence two implementations rather
    // than one with a dead parameter.
    if indexer.jar_files.contains_key(file_uri) {
        return members_for_jar_backed_type(
            indexer,
            file_uri,
            inner_name,
            symbols,
            type_symbol,
            caller,
            context,
        );
    }
    members_for_workspace_type(indexer, file_uri, symbols, type_symbol, caller, context)
}

/// JAR-backed member enumeration, keyed on `container` alone: a synthetic
/// `FileData` gives every symbol a one-line range, leaving no interior to
/// scan, so [`is_declared_in`]'s range check cannot apply here.
///
/// That costs the identity `is_declared_in` gets from ranges — one synthetic
/// `FileData` spans a whole JAR, where two same-named classes in different
/// packages really do occur — so package is the disambiguator instead, taken
/// from the caller's import when one names this class and the declaring
/// class's own package otherwise.
///
/// Deprecated members are dropped here rather than by the shared filter:
/// JAR symbols are always `Public`, so visibility never hides them.
fn members_for_jar_backed_type(
    indexer: &Indexer,
    file_uri: &str,
    inner_name: &str,
    symbols: &[SymbolEntry],
    type_symbol: &SymbolEntry,
    caller: CallerContext<'_>,
    context: MembershipContext,
) -> Vec<CompletionItem> {
    let member_indices: Vec<usize> = {
        let symbol_packages = indexer.jar_symbol_packages.get(file_uri);
        let package_at = |index: usize| -> Option<&str> {
            let packages = symbol_packages.as_ref()?;
            let values = packages.value();
            let result = values.get(index).map(String::as_str);
            // `jar_symbol_packages` is populated index-aligned with `symbols`
            // (see the population-site comment in `jar.rs`). The map entry
            // for this `file_uri` existing but being too short for `index`
            // means the two collections drifted apart — the same class of
            // bug that already made an explicitly-imported `padding` vanish
            // from chained-call completion once. A missing map entry
            // entirely is the legitimate pre-v8-sidecar case and isn't
            // logged.
            if result.is_none() && index >= values.len() {
                crate::util::throttled_warn(&JAR_SYMBOL_PACKAGES_MISALIGNED, 5, || {
                    format!(
                        "jar_symbol_packages misaligned for {file_uri}: symbol index {index} out \
                         of bounds (side table has {} entries) — member package disambiguation \
                         for `{inner_name}` completion falls back to the declaring class's own \
                         package, which can pick the wrong same-named class in a multi-package \
                         jar",
                        values.len(),
                    )
                });
            }
            result
        };
        // Kotlin resolves `Outer.member` through `Outer`'s own companion object
        // when `Outer` has no such member itself (implicit companion
        // forwarding) — the same idiom `members_for_workspace_type` already
        // special-cases for source-parsed files, via `is_declared_in`'s
        // container-name match. A JAR-derived companion's own
        // class-declaration symbol carries `container == inner_name` (see
        // `entriesFromClass` on the sidecar side), and its own members in turn
        // carry ITS bare name as their container — so a companion-of-`inner_name`'s
        // members are exactly those whose container matches that companion's
        // own name.
        let companion_names: Vec<&str> = symbols
            .iter()
            .filter(|symbol| {
                symbol.is_companion_object() && symbol.container.as_deref() == Some(inner_name)
            })
            .map(|symbol| symbol.name.as_str())
            .collect();
        // Candidate members: container match (direct or via a companion) + the
        // shared symbol filters.
        let candidate_indices: Vec<usize> = symbols
            .iter()
            .enumerate()
            .filter(|(_, symbol)| {
                symbol.container.as_deref() == Some(inner_name)
                    || symbol
                        .container
                        .as_deref()
                        .is_some_and(|c| companion_names.contains(&c))
            })
            .filter(|(_, symbol)| !symbol.deprecated)
            .filter(|(_, symbol)| symbol.visibility != Visibility::Private)
            .filter(|(_, symbol)| member_kind_allowed(context, symbol.kind))
            .map(|(index, _)| index)
            .collect();

        // Package disambiguation via IMPORT-COVERAGE semantics
        // (`ImportEntry::covers`), which — unlike a naive
        // `full_path.strip_suffix(".Name")` — understands nested-class
        // imports (`import com.example.Outer.Config` covers `Config`
        // declared in package `com.example`). Members the caller's
        // import covers win; when the import covers NONE of them (or no
        // import names this class), fall back to the declaring class
        // symbol's own package so the enumeration never goes empty just
        // because the import points at a different library's same-named
        // class.
        let caller_imports: Vec<crate::types::ImportEntry> = caller
            .uri
            .and_then(|caller_uri| {
                indexer.files.get(caller_uri).map(|caller_file| {
                    caller_file
                        .imports
                        .iter()
                        .filter(|import| !import.is_star && import.local_name == inner_name)
                        .cloned()
                        .collect()
                })
            })
            .unwrap_or_default();
        let import_covered: Vec<usize> = candidate_indices
            .iter()
            .copied()
            .filter(|index| {
                package_at(*index).is_some_and(|member_package| {
                    caller_imports
                        .iter()
                        .any(|import| import.covers(member_package, inner_name))
                })
            })
            .collect();
        if !import_covered.is_empty() {
            import_covered
        } else {
            let declaring_class_package = symbols
                .iter()
                .position(|symbol| std::ptr::eq(symbol, type_symbol))
                .and_then(package_at)
                .map(str::to_owned);
            candidate_indices
                .into_iter()
                .filter(|index| {
                    match (declaring_class_package.as_deref(), package_at(*index)) {
                        (Some(class_package), Some(member_package)) => {
                            class_package == member_package
                        }
                        // Older cache entries without per-symbol
                        // packages: keep the container match rather
                        // than dropping all.
                        _ => true,
                    }
                })
                .collect()
        }
        // `symbol_packages` dashmap guard drops here — before the item
        // construction below touches the indexer again.
    };
    member_indices
        .into_iter()
        .map(|index| completion_item_for_nested_symbol(indexer, &symbols[index], file_uri, caller))
        .collect()
}

/// Workspace-file member enumeration, keyed on each symbol's own
/// `container` — see [`is_declared_in`].
fn members_for_workspace_type(
    indexer: &Indexer,
    file_uri: &str,
    symbols: &[SymbolEntry],
    type_symbol: &SymbolEntry,
    caller: CallerContext<'_>,
    context: MembershipContext,
) -> Vec<CompletionItem> {
    // Kotlin resolves `Outer.member` through `Outer`'s companion when `Outer`
    // has no such member itself, so the companion's members join `Outer`'s own.
    let companions: Vec<&SymbolEntry> = symbols
        .iter()
        .filter(|symbol| symbol.is_companion_object() && is_declared_in(symbol, type_symbol))
        .collect();

    symbols
        .iter()
        .filter(|symbol| {
            is_declared_in(symbol, type_symbol)
                || companions
                    .iter()
                    .any(|companion| is_declared_in(symbol, companion))
        })
        .filter(|symbol| symbol.visibility != Visibility::Private)
        .filter(|symbol| member_kind_allowed(context, symbol.kind))
        .map(|symbol| completion_item_for_nested_symbol(indexer, symbol, file_uri, caller))
        .collect()
}

/// Whether `symbol` is declared directly inside `parent`.
///
/// `container` names the immediate parent, so nesting depth is handled for
/// free — a member of a nested type carries that nested type's name, never the
/// outer one. But a name is not an identity: one file may declare two nested
/// types sharing a simple name (`A.Config` and `B.Config`), and Kotlin gives
/// every anonymous companion the same implicit name `Companion`. The range
/// check pins the match to this specific `parent` declaration.
fn is_declared_in(symbol: &SymbolEntry, parent: &SymbolEntry) -> bool {
    symbol.container.as_deref() == Some(parent.name.as_str())
        && range_encloses(parent.range, symbol.range)
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
    /// Promotion budget consumed so far by this request — see
    /// `MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION`. Incremented once a
    /// candidate is confirmed Tier-1-only and not-yet-Tier-2 (not when it's
    /// checked-and-skipped as already-materialized or not JAR-sourced at
    /// this call site), but NOT a guarantee real sidecar IPC happened for
    /// every increment — `ensure_jar_materialized` can still no-op
    /// internally (a previously-failed candidate, or lock contention) after
    /// the budget for it was already reserved. Treat this as "how much
    /// budget this request has spent," not "how many real promotions ran."
    jar_promotion_attempts: usize,
    /// Total jar materializations this request triggered via the
    /// cross-package walk (cache-backed included) — see
    /// `MAX_CACHE_BACKED_MATERIALIZATIONS_PER_COMPLETION`.
    jar_materializations: usize,
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
            jar_promotion_attempts: 0,
            jar_materializations: 0,
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
        let Some(package_ids) = self.indexer.packages.get(&package_name) else {
            return;
        };
        let caller_source_set = self
            .indexer
            .files
            .get(self.from_uri.as_str())
            .map(|file| file.source_set)
            .unwrap_or_default();

        for package_id in package_ids.iter() {
            let Some(package_url) = self.indexer.file_table.url(*package_id) else {
                continue;
            };
            let package_uri = package_url.as_str();
            if package_uri == self.from_uri.as_str() {
                continue;
            }
            let Some(file) = self.indexer.files.get(package_uri) else {
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
                    Some(serde_json::json!({DATA_URI: package_uri, DATA_LINE: symbol.selection_start(), DATA_COL: symbol.selection_range.start.character})),
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
            let Some(pkg_ids) = self.indexer.packages.get(&import.full_path) else {
                continue;
            };
            for pkg_id in pkg_ids.iter() {
                let Some(pkg_url) = self.indexer.file_table.url(*pkg_id) else {
                    continue;
                };
                if pkg_url.as_str() == self.from_uri.as_str() {
                    continue; // already covered by collect_local_file
                }
                let Some(file) = self.indexer.files.get(pkg_url.as_str()) else {
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
                            DATA_URI: pkg_url.as_str(),
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

        // Tier-1-only candidate (in jar_bare_names but not yet in
        // jar_definitions): attempt promotion now. Candidates reaching this
        // point have already passed the prefix/score filter, so this is
        // bounded by what's actually going to be rendered — unlike full-cache
        // per-keystroke enumeration, which stays Tier-1-only per the design.
        // Cheap enough to do eagerly here rather than waiting for a separate
        // completionItem/resolve round-trip. `add_cross_package_symbol` below
        // reads `jar_definitions`/`jar_files` after this call, so a
        // successful promotion here does make the item's `detail` real; a
        // failed/timed-out promotion falls back to the name-only/FQN-only
        // stub already offered via Step 3's merge (graceful degradation,
        // Task 5).
        //
        // Bounded to `MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION` attempts per
        // request: each attempt is a real, blocking sidecar IPC round trip,
        // and a short/ambiguous prefix can match many Tier-1-only candidates
        // at once (Task 12 review finding — measured ~17 sequential
        // promotions / ~20s for one request against a real Gradle cache).
        // Candidates beyond the cap still get offered by name via the
        // fallthrough below; they just don't get real `detail` on this
        // request. The accessor spends from `remaining` only on genuinely
        // blocking (cache-miss) promotions — free cache-backed promotions
        // don't count against the request-wide cap — and the spent delta is
        // charged back onto the counter, mirroring `collect_this_extensions`.
        // Review finding on this migration: with cache-backed promotions no
        // longer charged against the blocking-IPC cap, this site — the
        // largest fan-out in the codebase (every prefix-matching bare name
        // in one request) — needs its own ceiling, or a 1-char prefix on a
        // fresh process with a warm disk cache materializes dozens-to-
        // hundreds of jars in one keystroke, clawing back the lazy-loading
        // memory win in a single request. Also restores the pre-migration
        // early-out: a name that already has materialized definitions never
        // spends promotion effort at all.
        if !self.indexer.jar_definitions.contains_key(bare_name)
            && self.jar_materializations < MAX_CACHE_BACKED_MATERIALIZATIONS_PER_COMPLETION
        {
            let materialized_before = self.indexer.materialized.len();
            let cap_remaining =
                MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION.saturating_sub(self.jar_promotion_attempts);
            let mut remaining = cap_remaining;
            crate::indexer::jar::ensure_jar_definitions_for(
                self.indexer,
                bare_name,
                &mut remaining,
            );
            self.jar_promotion_attempts += cap_remaining - remaining;
            self.jar_materializations += self
                .indexer
                .materialized
                .len()
                .saturating_sub(materialized_before);
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

        // If this candidate is backed by an already-materialized JAR symbol
        // (either it was never Tier-1-only, or the promotion attempt in
        // `add_cross_package_name` just succeeded), use its real signature
        // as `detail` and attach the same resolve-time `data` the Tier 0/1
        // paths use (`collect_local_file`/`collect_same_package`), so
        // `completionItem/resolve` can enrich its documentation too. Falls
        // back to the import-qualifier-only stub when there's no
        // materialized JAR symbol for this FQN yet (promotion failed or
        // hasn't happened, or this candidate isn't JAR-sourced at all).
        // The package hint that keeps identically-named candidates (five
        // `Modifier`s from five packages) tellable apart. Two delivery
        // routes, chosen by the client's `labelDetailsSupport` capability:
        // - supported (VS Code, blink.cmp): the LSP-standard
        //   `labelDetails.description` slot, rendered dimmed next to the
        //   label in the completion list; `detail` stays untouched.
        // - not supported (Helix — its menu renders label + kind only — and
        //   the CLI path): fold the package into a materialized candidate's
        //   signature `detail` as a Kotlin-style `package …` header line,
        //   which such clients DO render in their doc popup. Unmaterialized
        //   stubs already carry the package as their whole `detail`.
        //   `resolve_completion_item` preserves the header line when it
        //   re-derives `detail` from the enriched signature.
        let supports_label_details = self
            .indexer
            .client_label_details_support
            .load(std::sync::atomic::Ordering::Relaxed);
        let (detail, item_data) = match jar_symbol_detail(self.indexer, bare_name, qualifier) {
            Some((Some(signature), data)) if !supports_label_details && !qualifier.is_empty() => {
                (Some(format!("package {qualifier}\n{signature}")), data)
            }
            Some(pair) => pair,
            // Stub: no materialized symbol behind this FQN (yet). Carry the
            // FQN so `completionItem/resolve` can materialize the ONE
            // candidate the user actually selects and surface its real
            // signature + docs — without this the stub resolves to nothing
            // ("package but no signature/docs", the live report).
            None => (
                needs_import.then(|| qualifier.to_string()),
                Some(serde_json::json!({ DATA_FQN: fully_qualified_name })),
            ),
        };
        let label_details =
            (supports_label_details && !qualifier.is_empty()).then(|| CompletionItemLabelDetails {
                detail: None,
                description: Some(qualifier.to_string()),
            });

        self.completer.items.push(CompletionItem {
            label: bare_name.to_string(),
            kind: Some(CompletionItemKind::CLASS),
            label_details,
            filter_text: Some(bare_name.to_string()),
            sort_text: Some(format!("2{}:{}", score, bare_name.to_lowercase())),
            detail,
            additional_text_edits,
            data: item_data,
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
    /// Inherited REGULAR members (methods/properties of ancestor classes) for
    /// bare completion inside a class body — `setState` typed inside a
    /// subclass of a library `MviViewModel` must complete without a receiver.
    /// Dot-completion has had this since `collect_inherited_dot_completion_items`;
    /// bare completion never did (its other collectors are file/package/
    /// import/extension-scoped, and the cross-package path deliberately
    /// excludes lowercase member names). The hierarchy walk promotes
    /// Tier-1-only ancestor JARs via `supertype_targets`' gate, and
    /// `symbols_from_nested_type` enumerates jar-backed ancestors by
    /// `container` (synthetic one-line ranges can't nest).
    fn collect_inherited_members(&mut self) {
        if self.completer.annotation_only {
            return;
        }
        let cursor_line = match self.cursor_line {
            Some(line) => line,
            None => return,
        };
        let enclosing_class = match self.indexer.enclosing_class_at(self.from_uri, cursor_line) {
            Some(name) => name,
            None => return,
        };
        let class_locations = resolve_symbol_no_rg(self.indexer, &enclosing_class, self.from_uri);
        let class_uri = match class_locations.into_iter().next() {
            Some(location) => location.uri.to_string(),
            None => return,
        };
        let caller = CallerContext {
            uri: Some(self.from_uri.as_str()),
            cursor_line: self.cursor_line,
        };
        let inherited = walk_hierarchy(
            self.indexer,
            &enclosing_class,
            &class_uri,
            caller,
            4,
            MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
            |index, class_name, ancestor_uri, hierarchy_caller| {
                symbols_from_nested_type(
                    index,
                    ancestor_uri,
                    class_name,
                    hierarchy_caller,
                    MembershipContext::Inherited,
                )
            },
        );
        for item in inherited {
            // Unlike external dot-access, `this`-context completion inside
            // the subclass may see protected members — only private is
            // excluded (already filtered by `symbols_from_nested_type`).
            if match_score(&item.label, self.prefix).is_none() {
                continue;
            }
            if self.completer.seen.insert(item.label.clone()) {
                self.completer.items.push(item);
            }
        }
    }

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
                    MAX_SYNC_JAR_PROMOTIONS_PER_HIERARCHY_WALK,
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
            // Atomic promote+read via the accessor. Shares the per-request
            // blocking-IPC cap with the cross-package promotion below: the
            // accessor spends from `remaining`, and the delta is charged
            // back onto the request-wide counter.
            let cap_remaining =
                MAX_SYNC_JAR_PROMOTIONS_PER_COMPLETION.saturating_sub(self.jar_promotion_attempts);
            let mut remaining = cap_remaining;
            let entries =
                crate::indexer::jar::extension_entries_for(self.indexer, ancestor, &mut remaining);
            self.jar_promotion_attempts += cap_remaining - remaining;
            let Some(entries) = entries else {
                continue;
            };
            for entry in entries.iter() {
                if crate::Language::from_path(&entry.file_uri) != crate::Language::Kotlin {
                    continue;
                }
                // Same exclusion as `ExtensionCompletionBuilder::add_entry`:
                // a member extension's declaring container being one of this
                // call's ancestors doesn't prove the container is the actual
                // implicit receiver here, and `build_item_from_entry` would
                // still compute an auto-import Kotlin has no syntax for.
                if entry.container.is_some() {
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

/// Real `detail` text + resolve-time `data` for a cross-package candidate
/// backed by an already-materialized JAR symbol, or `None` when there isn't
/// one yet (Tier-1-only and promotion failed/didn't run, or this candidate
/// isn't JAR-sourced at all — the caller falls back to the import-qualifier
/// stub in that case).
///
/// Looks up `jar_definitions` for `bare_name`, picks the `Location` whose
/// real per-symbol package (`jar_symbol_package`, from the `jar_symbol_packages`
/// side table) matches `package` — disambiguating when the same bare name
/// exists in more than one JAR/package — then reads the real signature text
/// from that JAR's synthetic `FileData` (`jar_files`), mirroring how
/// `collect_local_file`/`collect_same_package` build `detail` from
/// `SymbolEntry::detail` and attach `DATA_URI`/`DATA_LINE`/`DATA_COL` for
/// `completionItem/resolve` doc enrichment.
pub(crate) fn jar_symbol_detail(
    indexer: &Indexer,
    bare_name: &str,
    package: &str,
) -> Option<(Option<String>, Option<serde_json::Value>)> {
    let locs = indexer.jar_definitions.get(bare_name)?;
    let loc: Location = locs
        .iter()
        .find(|loc| jar_symbol_package(indexer, loc).as_deref() == Some(package))?
        .clone();
    drop(locs);

    let uri_str = loc.uri.as_str();
    let file = indexer.jar_files.get(uri_str)?;
    let symbol = file.symbols.get(loc.range.start.line as usize)?;
    let detail = (!symbol.detail.is_empty()).then(|| symbol.detail.clone());
    let data = serde_json::json!({
        DATA_URI: uri_str,
        DATA_LINE: symbol.selection_start(),
        DATA_COL: symbol.selection_range.start.character,
    });
    Some((detail, Some(data)))
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
    completion_walk.collect_inherited_members();
    log::debug!(
        "bare: inherited_members {}ms",
        start_time.elapsed().as_millis()
    );
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

#[cfg(test)]
#[path = "complete_tests.rs"]
mod tests;
