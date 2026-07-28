//! Diagnostic: flag an `import` statement whose local name is never used
//! anywhere in the file.
//!
//! The mirror image of `missing_import_diagnostics`: that feature narrows to
//! *bare* calls and *bare* type identifiers to stay high-confidence about what
//! counts as a genuine unresolved reference. This one needs the opposite
//! bias — it must catch *every* way an import's name could be used (bare
//! call, bare type, member/extension-function call name, annotation, generic
//! argument, supertype, …), since missing one shape means suggesting the
//! deletion of a still-needed import. Rather than enumerate shapes, it
//! collects the flat set of every `simple_identifier`/`type_identifier`
//! node's text anywhere in the file (skipping only the import/package
//! headers themselves) and checks each import's local name against that set.
//! A local variable that happens to shadow an import's name makes the import
//! look "used" when it technically isn't — a false negative, the safe
//! direction of error for a delete-this suggestion.
//!
//! Four further fixes, all found by running the aggregate benchmark (below)
//! against real projects or by code review — the exact "verify, don't guess"
//! discipline this feature was built with:
//!
//! - **Operator-convention imports** ([`OPERATOR_CONVENTION_NAMES`]) — Kotlin
//!   property-delegate (`by lazy { }`) and Gradle's Kotlin-DSL analogues
//!   (`=`/`()`/`[]` sugar in convention plugins) desugar to a call
//!   (`getValue`/`setValue`/`assign`/`invoke`/`get`/…) that the compiler
//!   synthesizes from special syntax — the name never appears as literal
//!   identifier text anywhere, so no amount of widening the identifier walk
//!   can catch it structurally. This was the dominant false-positive source
//!   (32 of 62 initial flags on nowInAndroid).
//! - **KDoc `[Reference]` links** — tree-sitter-kotlin does not parse KDoc
//!   comment bodies into structured sub-nodes at all (`multiline_comment` is
//!   one opaque leaf), so there is no CST node for `[Foo]` to find. A light
//!   text scan over comment-leaf text for `[Identifier` patterns is not
//!   "recovering structure the CST already has" (there is none to recover)
//!   — it is the same "genuine heuristic, no precise CST answer exists"
//!   carve-out the parent design already grants stdlib scope-function
//!   receiver inference. This accounted for essentially all the remaining
//!   nowInAndroid noise once the operator-convention exemption above was
//!   added.
//! - **Bare `$identifier` string-template interpolation** ([`KIND_INTERPOLATED_IDENT`])
//!   — found the hard way: deleting every flagged import across a real
//!   ~13k-file monorepo (Moneta) and running its Kotlin compiler surfaced one
//!   genuine build break, `Regex("^$REGEX_PHONE_WITH_OPTIONAL_PREFIX$")`.
//!   Unlike the two exemptions above, this one *is* a real CST-shape gap, not
//!   a "nothing to widen to" case: bare `$identifier` interpolation parses to
//!   its own dedicated `interpolated_identifier` leaf node (confirmed by
//!   dumping the parse tree), distinct from `simple_identifier`/
//!   `type_identifier` — the walk's kind filter simply didn't include it.
//!   Braced `${identifier}`/`${expr.member}` interpolation was already
//!   correctly handled: it wraps a real `navigation_expression`/
//!   `call_expression` whose own leaves are ordinary `simple_identifier`
//!   nodes the walk already collects.
//! - **`componentN` destructuring for arbitrary N** ([`is_component_n_name`])
//!   — found in PR review, not the benchmark: the first cut of
//!   `OPERATOR_CONVENTION_NAMES` hardcoded `component1`..`component5`, but
//!   Kotlin's destructuring convention is unbounded — any class can declare
//!   `operator fun component6()` and beyond, and a data class generates one
//!   `componentN` per property, however many that is. Fixed by recognizing
//!   the *shape* (`component` + a positive integer suffix) instead of
//!   enumerating a fixed set, the same principle as the interpolation fix
//!   above but caught by review instead of by compiling real code.
//!
//! Star imports (`import com.example.*`) are never flagged — there is no
//! single name to check usage of.
//!
//! The candidate-collection walk here is shared with the `unused-imports` CLI
//! subcommand (`cli::unused_import_poc`), which runs the same
//! [`collect_unused_import_flags`] over an entire workspace to measure
//! precision — see that module for the aggregate false-positive methodology.

use std::collections::HashSet;

use tower_lsp::lsp_types::*;
use tree_sitter::Node;

use crate::indexer::live_tree::LiveDoc;
use crate::indexer::NodeExt;
use crate::parser::import_entry_from_header;
use crate::queries::{
    KIND_IMPORT_HEADER, KIND_IMPORT_LIST, KIND_INTERPOLATED_IDENT, KIND_LINE_COMMENT,
    KIND_MULTILINE_COMMENT, KIND_PACKAGE_HEADER, KIND_SIMPLE_IDENT, KIND_TYPE_IDENT,
};

/// Kotlin's operator-convention function names (`by`, `+`, `[]`, `()`, …) plus
/// Gradle Kotlin DSL's `assign` (its own convention for `=` on typed
/// configuration properties in convention plugins). An import solely for one
/// of these never appears as literal identifier text anywhere — the compiler
/// synthesizes the call from special syntax — so these are never flagged,
/// regardless of whether the file's own syntax happens to use the
/// corresponding sugar. See the module doc for how this was found.
const OPERATOR_CONVENTION_NAMES: &[&str] = &[
    "getValue",
    "setValue",
    "provideDelegate",
    "invoke",
    "get",
    "set",
    "assign",
    "plus",
    "minus",
    "times",
    "div",
    "rem",
    "rangeTo",
    "rangeUntil",
    "contains",
    "iterator",
    "hasNext",
    "next",
    "compareTo",
    "equals",
    "unaryPlus",
    "unaryMinus",
    "not",
    "inc",
    "dec",
    "plusAssign",
    "minusAssign",
    "timesAssign",
    "divAssign",
    "remAssign",
];

/// Whether `name` is a Kotlin destructuring `componentN` operator function
/// (`component1`, `component2`, …). Unlike the fixed-arity names in
/// [`OPERATOR_CONVENTION_NAMES`], `componentN` is unbounded — any class can
/// declare `operator fun component6()` and beyond, and data classes generate
/// one per property, however many that is. A fixed `component1`..`component5`
/// allowlist would still flag `component6`+ as unused even though its use is
/// exactly as implicit as `component1`'s.
fn is_component_n_name(name: &str) -> bool {
    name.strip_prefix("component")
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
}

/// A flagged unused import: its fully-qualified path and declaration line.
pub(crate) struct UnusedImportFlag {
    pub full_path: String,
    pub line: u32,
}

/// Detect unused-import flags for one already-parsed document.
pub(crate) fn collect_unused_import_flags(doc: &LiveDoc) -> Vec<UnusedImportFlag> {
    let bytes = &doc.bytes;
    let root = doc.tree.root_node();

    let mut used_names: HashSet<&str> = HashSet::new();
    collect_used_identifier_texts(root, bytes, &mut used_names);

    let Some(import_list) = root.children_of_kind(KIND_IMPORT_LIST).into_iter().next() else {
        return Vec::new();
    };

    import_list
        .children_of_kind(KIND_IMPORT_HEADER)
        .into_iter()
        .filter_map(|header| {
            let entry = import_entry_from_header(header, bytes)?;
            let is_used = used_names.contains(entry.local_name.as_str())
                || OPERATOR_CONVENTION_NAMES.contains(&entry.local_name.as_str())
                || is_component_n_name(&entry.local_name);
            if entry.is_star || is_used {
                return None;
            }
            Some(UnusedImportFlag {
                full_path: entry.full_path,
                line: header.start_position().row as u32,
            })
        })
        .collect()
}

/// Walk the CST collecting the text of every `simple_identifier`/`type_identifier`
/// node (used names, see the module doc) plus every `[Identifier`-shaped KDoc
/// reference token found inside comment-leaf text, skipping the import/package
/// headers themselves. Deliberately unstructured — see the module doc for why.
fn collect_used_identifier_texts<'a>(node: Node<'a>, bytes: &'a [u8], out: &mut HashSet<&'a str>) {
    collect_used_identifier_texts_inner(node, bytes, false, out);
}

/// `in_declaration_header` suppresses identifier-text collection while inside
/// an import/package header's own path segments (those aren't uses), but must
/// NOT suppress comment scanning: tree-sitter-kotlin attaches a comment
/// immediately following an import as a TRAILING CHILD of that import's own
/// `import_header` node, not a leading child of the next declaration — a
/// naive early-return-and-skip-the-whole-subtree would silently drop exactly
/// the KDoc `[Reference]` comments this function exists to scan.
fn collect_used_identifier_texts_inner<'a>(
    node: Node<'a>,
    bytes: &'a [u8],
    in_declaration_header: bool,
    out: &mut HashSet<&'a str>,
) {
    let kind = node.kind();
    let in_declaration_header =
        in_declaration_header || kind == KIND_IMPORT_HEADER || kind == KIND_PACKAGE_HEADER;

    if !in_declaration_header
        && matches!(
            kind,
            KIND_SIMPLE_IDENT | KIND_TYPE_IDENT | KIND_INTERPOLATED_IDENT
        )
    {
        if let Ok(text) = node.utf8_text(bytes) {
            out.insert(text);
        }
    }
    if matches!(kind, KIND_LINE_COMMENT | KIND_MULTILINE_COMMENT) {
        if let Ok(text) = node.utf8_text(bytes) {
            collect_kdoc_reference_names(text, out);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_used_identifier_texts_inner(child, bytes, in_declaration_header, out);
    }
}

/// Extract every identifier token found inside a `[...]` KDoc reference span
/// (`[Foo]`, `[Foo.bar]`, `[com.example.Foo]`) from raw comment text. Liberal
/// on purpose — every extracted token only ever suppresses a flag, matching
/// this feature's false-negative-safe bias throughout.
fn collect_kdoc_reference_names<'a>(text: &'a str, out: &mut HashSet<&'a str>) {
    let mut depth = 0usize;
    let mut token_start: Option<usize> = None;
    for (byte_index, character) in text.char_indices() {
        match character {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth > 0 && (character.is_alphanumeric() || character == '_') => {
                token_start.get_or_insert(byte_index);
                continue;
            }
            _ => {}
        }
        if let Some(start) = token_start.take() {
            out.insert(&text[start..byte_index]);
        }
    }
    if let Some(start) = token_start {
        out.insert(&text[start..]);
    }
}

/// Scan a file for unused imports and return diagnostics.
///
/// Unlike `missing_import_diagnostics`, this never reads JAR data, so there is
/// no jar-phase gate — the whole detection is purely a CST walk over the
/// already-parsed document.
pub(crate) fn unused_import_diagnostics(doc: &LiveDoc) -> Vec<Diagnostic> {
    collect_unused_import_flags(doc)
        .into_iter()
        .map(|flag| Diagnostic {
            range: import_line_range(doc, flag.line),
            severity: Some(DiagnosticSeverity::HINT),
            tags: Some(vec![DiagnosticTag::UNNECESSARY]),
            source: Some("kmp-lsp".into()),
            message: format!("Unused import '{}'", flag.full_path),
            ..Default::default()
        })
        .collect()
}

/// The UTF-16 LSP range spanning the whole text of the import line at `line`.
fn import_line_range(doc: &LiveDoc, line: u32) -> Range {
    let full_text = std::str::from_utf8(&doc.bytes).unwrap_or_default();
    let line_text = full_text.lines().nth(line as usize).unwrap_or_default();
    let end_character = crate::features::text_utils::utf16_column(line_text);
    Range::new(Position::new(line, 0), Position::new(line, end_character))
}

/// Quick-fix code action for this module's own diagnostics: "Remove unused
/// import" — a single edit deleting the whole import line (including its
/// trailing newline, so no blank line is left behind).
///
/// Reads the import's line directly from the diagnostic's own range rather
/// than re-walking the CST: the client already told us which of our own
/// diagnostics apply at this position via `CodeActionContext::diagnostics`.
pub(crate) fn unused_import_actions(
    uri: &Url,
    diagnostics: &[Diagnostic],
) -> Vec<CodeActionOrCommand> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.source.as_deref() == Some("kmp-lsp")
                && diagnostic.message.starts_with("Unused import '")
        })
        .map(|diagnostic| remove_import_action(uri, diagnostic))
        .collect()
}

fn remove_import_action(uri: &Url, diagnostic: &Diagnostic) -> CodeActionOrCommand {
    let line = diagnostic.range.start.line;
    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range::new(Position::new(line, 0), Position::new(line + 1, 0)),
            new_text: String::new(),
        }],
    );
    CodeActionOrCommand::CodeAction(CodeAction {
        title: "Remove unused import".to_owned(),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

#[cfg(test)]
#[path = "unused_import_diagnostics_tests.rs"]
mod tests;
