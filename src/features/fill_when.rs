//! Fill missing `when` branches for sealed classes and enums.
//!
//! This module provides a code action that detects an incomplete `when` expression
//! over a sealed class or enum, and generates the missing branches.
//!
//! Entry point: [`build_fill_when_action`].

use tower_lsp::lsp_types::*;

use crate::indexer::live_tree::utf16_col_to_byte;
use crate::indexer::Indexer;
use crate::queries::{
    KIND_BOOLEAN_LITERAL, KIND_ELSE, KIND_LBRACE, KIND_NAV_EXPR, KIND_NAV_SUFFIX, KIND_RBRACE,
    KIND_SIMPLE_IDENT, KIND_TYPE_IDENT, KIND_TYPE_TEST, KIND_USER_TYPE, KIND_WHEN_CONDITION,
    KIND_WHEN_ENTRY, KIND_WHEN_EXPR, KIND_WHEN_SUBJECT,
};
use crate::StrExt;

/// Memoizes `collect_sealed_members` results within a single `when_diagnostics` pass.
/// Key: `(sealed_name, parent_uri_string, range)`.
type SealedMembersCache =
    std::collections::HashMap<(String, String, u32, u32, u32, u32), Vec<WhenMember>>;

/// Memoizes `resolve_type_members` results within a single pass.
/// Key: `(subject type name, reachability uri)` — a bare type name alone
/// isn't enough to key on: two `when`s in the same file can resolve the same
/// name through different reachability anchors (a chained subject's leaf type
/// is resolved from *its own* declaring file, not necessarily this file's —
/// see `infer::infer_field_chain_type`), and a same-named-but-different type
/// from the wrong anchor must not be served from the other's cache entry.
type TypeMembersCache =
    std::collections::HashMap<(String, String), Option<(TypeKind, Vec<WhenMember>)>>;

/// Analysis result for incomplete when expressions — shared by code actions and diagnostics.
struct WhenAnalysis<'a> {
    when_node: tree_sitter::Node<'a>,
    subject_type: String,
    type_kind: TypeKind,
    missing: Vec<WhenMember>,
}

/// Analyze a single when expression for missing branches.
fn analyze_when<'a>(
    indexer: &Indexer,
    uri: &Url,
    when_node: tree_sitter::Node<'a>,
    source_bytes: &[u8],
    sealed_cache: &mut SealedMembersCache,
    type_members_cache: &mut TypeMembersCache,
) -> Option<WhenAnalysis<'a>> {
    let subject_node = when_node
        .children(&mut when_node.walk())
        .find(|c| c.kind() == KIND_WHEN_SUBJECT)?;

    let subject_segments = crate::resolver::infer::subject_segments(subject_node, source_bytes)?;

    let existing = collect_existing_branches(&when_node, source_bytes);

    if existing.iter().any(|b| b == "else") {
        return None;
    }

    // The file whose imports/package should be used to resolve `subject_type`'s
    // own members below. For a plain `when (var)` it's always the caller's own
    // file. For a chained `when (var.field)` it must be the *leaf field's own
    // declaring file* instead — the leaf type can be a class duplicated
    // workspace-wide and reachable only through the chain, never imported by
    // the caller directly (see `infer::infer_field_chain_type`'s doc comment).
    let (subject_type, members_uri) = match subject_segments.as_slice() {
        // A local or parameter: prefer the declaration the CST can see over a
        // whole-file name scan.
        [subject_var] => (
            crate::resolver::infer::resolve_declared_type_from_cst(
                when_node,
                subject_var,
                source_bytes,
            )
            .or_else(|| crate::resolver::infer::infer_variable_type(indexer, subject_var, uri))?,
            uri.clone(),
        ),
        // `userData.themeBrand` — walk the field chain to its leaf type.
        // `infer_field_chain_type` itself finds the longest smart-cast-narrowed
        // prefix of the chain from the CST (Kotlin narrows whole stable
        // paths, not just the root variable — see its own doc comment) and
        // walks any remaining fields from there.
        chain => {
            let line = when_node.start_position().row as u32;
            let (receiver_type, declaring_uri) = crate::resolver::infer::infer_field_chain_type(
                indexer,
                chain,
                uri,
                line,
                Some((when_node, source_bytes)),
            )?;
            (receiver_type.qualified, declaring_uri)
        }
    };
    let subject_type = subject_type.strip_nullable().to_string();

    let (type_kind, members) = resolve_type_members(
        indexer,
        &members_uri,
        &subject_type,
        sealed_cache,
        type_members_cache,
    )?;

    let missing: Vec<WhenMember> = members
        .into_iter()
        .filter(|m| !existing.contains(&m.name))
        .collect();

    if missing.is_empty() {
        return None;
    }

    Some(WhenAnalysis {
        when_node,
        subject_type,
        type_kind,
        missing,
    })
}

/// Try to build a "fill missing when branches" code action for the cursor position.
///
/// Returns `None` if the cursor is not inside a `when` expression, the subject type
/// cannot be resolved, or all branches are already covered.
pub(crate) fn build_fill_when_action(
    indexer: &Indexer,
    uri: &Url,
    range: Range,
) -> Option<CodeActionOrCommand> {
    let live_doc = indexer.live_doc(uri)?;
    let source_bytes = &live_doc.bytes;
    let lines = indexer.mem_lines_for(uri.as_str())?;

    let cursor_byte = byte_offset_for_position(&lines, range.start)?;
    let when_node = find_enclosing_when(&live_doc.tree, source_bytes, cursor_byte)?;

    let analysis = analyze_when(
        indexer,
        uri,
        when_node,
        source_bytes,
        &mut SealedMembersCache::new(),
        &mut TypeMembersCache::new(),
    )?;

    let indent = detect_indent(&analysis.when_node, source_bytes);
    let (replace_range, brace_indent) =
        find_insert_position(&analysis.when_node, source_bytes, &lines)?;
    let missing_refs: Vec<&WhenMember> = analysis.missing.iter().collect();
    let mut insert_text = build_branch_text(
        &missing_refs,
        &analysis.subject_type,
        analysis.type_kind,
        &indent,
    );
    insert_text.push_str(&brace_indent);
    insert_text.push('}');

    let edit = TextEdit {
        range: replace_range,
        new_text: insert_text,
    };

    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![edit]);

    let action = CodeAction {
        title: format!("Fill missing '{}' branches", analysis.subject_type),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    };

    Some(CodeActionOrCommand::CodeAction(action))
}

/// Produce diagnostics for all incomplete `when` expressions in a file.
///
/// Scans the CST for every `when_expression` node and emits a warning
/// diagnostic on each one that has missing branches.
pub(crate) fn when_diagnostics(indexer: &Indexer, uri: &Url) -> Vec<Diagnostic> {
    if crate::Language::from_path(uri.path()) != crate::Language::Kotlin {
        return Vec::new();
    }
    let live_doc = match indexer.live_doc(uri) {
        Some(doc) => doc,
        None => return Vec::new(),
    };
    let source_bytes = &live_doc.bytes;
    let root = live_doc.tree.root_node();

    let diag_start = std::time::Instant::now();
    let mut diagnostics = Vec::new();
    let mut sealed_cache = SealedMembersCache::new();
    let mut type_members_cache = TypeMembersCache::new();
    collect_when_nodes(
        root,
        source_bytes,
        indexer,
        uri,
        &mut diagnostics,
        &mut sealed_cache,
        &mut type_members_cache,
        0,
    );
    let elapsed = diag_start.elapsed();
    if elapsed.as_millis() > 50 {
        log::info!(
            "when_diagnostics: {}ms, {} type-cache entries, {} sealed-cache entries — {}",
            elapsed.as_millis(),
            type_members_cache.len(),
            sealed_cache.len(),
            uri.path(),
        );
    }
    diagnostics
}

#[allow(clippy::too_many_arguments)]
fn collect_when_nodes(
    node: tree_sitter::Node,
    source: &[u8],
    indexer: &Indexer,
    uri: &Url,
    diagnostics: &mut Vec<Diagnostic>,
    sealed_cache: &mut SealedMembersCache,
    type_members_cache: &mut TypeMembersCache,
    depth: usize,
) {
    // See `crate::util::MAX_CST_DESCENT_DEPTH`: bail rather than overflow the
    // stack on a pathologically deep tree (huge chained expression, or
    // ERROR-recovery on a huge malformed file).
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("collect_when_nodes", node);
        return;
    }

    if node.kind() == KIND_WHEN_EXPR {
        // Emit a warning whenever a `when` over a sealed class or enum is missing
        // branches and has no `else`.  This applies to both expression-form and
        // statement-form: a sealed class `when` should always be exhaustive or
        // have an `else` branch regardless of how the result is used.
        // `analyze_when` returns None if `else` is present, all branches are
        // covered, or the subject type is not a sealed class / enum.
        if let Some(analysis) =
            analyze_when(indexer, uri, node, source, sealed_cache, type_members_cache)
        {
            let missing_names: Vec<&str> =
                analysis.missing.iter().map(|m| m.name.as_str()).collect();
            let message = format!("'when' is missing branches: {}", missing_names.join(", "));
            let start = node.start_position();
            let keyword_end_col = start.column + 4; // "when" is 4 chars
            diagnostics.push(Diagnostic {
                range: Range::new(
                    Position::new(start.row as u32, start.column as u32),
                    Position::new(start.row as u32, keyword_end_col as u32),
                ),
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("kmp-lsp".into()),
                message,
                ..Default::default()
            });
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            collect_when_nodes(
                cursor.node(),
                source,
                indexer,
                uri,
                diagnostics,
                sealed_cache,
                type_members_cache,
                depth + 1,
            );
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

// ─── helpers ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TypeKind {
    Enum,
    Sealed,
    Boolean,
}

#[derive(Debug, Clone)]
struct WhenMember {
    name: String,
    is_object: bool,
    /// True if subtype is nested inside the parent type (for sealed classes).
    is_nested: bool,
}

fn byte_offset_for_position(lines: &[String], pos: Position) -> Option<usize> {
    let line = pos.line as usize;
    if line >= lines.len() {
        return None;
    }
    let mut offset = 0;
    for l in &lines[..line] {
        offset += l.len() + 1; // +1 for '\n'
    }
    let col_byte = utf16_col_to_byte(&lines[line], pos.character as usize);
    Some(offset + col_byte)
}

fn find_enclosing_when<'a>(
    tree: &'a tree_sitter::Tree,
    _source: &[u8],
    cursor_byte: usize,
) -> Option<tree_sitter::Node<'a>> {
    let node = tree
        .root_node()
        .descendant_for_byte_range(cursor_byte, cursor_byte)?;
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == KIND_WHEN_EXPR {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

/// Resolve whether the type is an enum, sealed class, or Boolean, and return its members.
fn resolve_type_members(
    indexer: &Indexer,
    from_uri: &Url,
    type_name: &str,
    sealed_cache: &mut SealedMembersCache,
    type_members_cache: &mut TypeMembersCache,
) -> Option<(TypeKind, Vec<WhenMember>)> {
    // Fast path: same (type, reachability anchor) was already resolved
    // earlier in this pass. We cache with empty existing_branches so the
    // result is independent of which `when` node queries first —
    // branches_fit_members would otherwise select different homonymous types
    // depending on call order. `analyze_when` applies its own missing-branch
    // filter after this call.
    let cache_key = (type_name.to_string(), from_uri.to_string());
    if let Some(cached) = type_members_cache.get(&cache_key) {
        return cached.clone();
    }
    let result = resolve_type_members_inner(indexer, from_uri, type_name, &[], sealed_cache);
    type_members_cache.insert(cache_key, result.clone());
    result
}

fn resolve_type_members_inner(
    indexer: &Indexer,
    from_uri: &Url,
    type_name: &str,
    existing_branches: &[String],
    sealed_cache: &mut SealedMembersCache,
) -> Option<(TypeKind, Vec<WhenMember>)> {
    // Boolean is a built-in — no index lookup needed
    if type_name == "Boolean" {
        let members = vec![
            WhenMember {
                name: "true".to_string(),
                is_object: true,
                is_nested: false,
            },
            WhenMember {
                name: "false".to_string(),
                is_object: true,
                is_nested: false,
            },
        ];
        return Some((TypeKind::Boolean, members));
    }

    // Use import/package-aware resolution to pick the correct type when multiple
    // homonymous types exist across packages. Falls back to definition_locations
    // if the index-only resolver finds nothing (e.g. during partial indexing).
    let mut locations =
        crate::resolver::resolve::resolve_type_index_only(indexer, type_name, from_uri);
    if locations.is_empty() {
        locations = indexer.definition_locations(type_name);
    }
    if locations.is_empty() {
        return None;
    }

    let mut fallback: Option<(TypeKind, Vec<WhenMember>)> = None;

    for location in &locations {
        let Some(file_data) = indexer.file_data_for(location.uri.as_str()) else {
            continue;
        };
        let Some(symbol) = find_symbol_at(&file_data, location) else {
            continue;
        };

        if symbol.kind == SymbolKind::ENUM {
            let members = collect_enum_members(&file_data, &symbol);
            if !members.is_empty() {
                if branches_fit_members(existing_branches, &members) {
                    return Some((TypeKind::Enum, members));
                }
                if fallback.is_none() {
                    fallback = Some((TypeKind::Enum, members));
                }
            }
        }

        if is_sealed(&symbol) {
            let members = collect_sealed_members(
                indexer,
                &symbol.name,
                &location.uri,
                &symbol.range,
                sealed_cache,
            );
            if !members.is_empty() {
                if branches_fit_members(existing_branches, &members) {
                    return Some((TypeKind::Sealed, members));
                }
                if fallback.is_none() {
                    fallback = Some((TypeKind::Sealed, members));
                }
            }
        }
    }

    fallback
}

fn branches_fit_members(existing_branches: &[String], members: &[WhenMember]) -> bool {
    if existing_branches.is_empty() {
        return true;
    }
    let member_names: std::collections::HashSet<&str> =
        members.iter().map(|m| m.name.as_str()).collect();
    existing_branches
        .iter()
        .filter(|b| b.as_str() != "else")
        .all(|b| member_names.contains(b.as_str()))
}

fn find_symbol_at(
    file_data: &crate::types::FileData,
    location: &Location,
) -> Option<crate::types::SymbolEntry> {
    file_data
        .symbols
        .iter()
        .find(|s| s.selection_range == location.range)
        .cloned()
}

fn is_sealed(symbol: &crate::types::SymbolEntry) -> bool {
    let detail = &symbol.detail;
    detail.starts_with("sealed class")
        || detail.starts_with("sealed interface")
        || detail.starts_with("abstract sealed")
}

fn collect_enum_members(
    file_data: &crate::types::FileData,
    enum_symbol: &crate::types::SymbolEntry,
) -> Vec<WhenMember> {
    file_data
        .symbols
        .iter()
        .filter(|s| {
            // Column-aware containment, not a line comparison: a one-line
            // `enum class Brand { DEFAULT, ANDROID }` declares its entries on
            // the enum's own start line, so requiring a strictly greater line
            // found no entries at all and silently disabled the diagnostic.
            s.kind == SymbolKind::ENUM_MEMBER
                && crate::resolver::resolve::range_encloses(enum_symbol.range, s.range)
        })
        .map(|s| WhenMember {
            name: s.name.clone(),
            is_object: true, // enum entries are always object-like
            is_nested: true,
        })
        .collect()
}

fn collect_sealed_members(
    indexer: &Indexer,
    sealed_name: &str,
    parent_uri: &Url,
    parent_range: &Range,
    sealed_cache: &mut SealedMembersCache,
) -> Vec<WhenMember> {
    let cache_key = (
        sealed_name.to_string(),
        parent_uri.to_string(),
        parent_range.start.line,
        parent_range.start.character,
        parent_range.end.line,
        parent_range.end.character,
    );
    if let Some(cached) = sealed_cache.get(&cache_key) {
        return cached.to_vec();
    }

    // Use the parent's package for same-package subclass filtering (PR #103: allow
    // sealed subtypes that live in sibling files in the same package).
    let parent_package = indexer
        .file_data_for(parent_uri.as_str())
        .and_then(|fd| fd.package.clone());

    let subtype_locations = indexer.subtypes_of(sealed_name);
    let mut members = Vec::new();

    for location in &subtype_locations {
        let Some(file_data) = indexer.file_data_for(location.uri.as_str()) else {
            continue;
        };
        // Accept subtypes in the same file as the sealed class OR in a sibling file
        // in the same package.  This rejects identically-named sealed classes from
        // unrelated packages while still finding all valid subtypes.
        let same_parent_file = location.uri == *parent_uri;
        let same_package = parent_package.as_deref() == file_data.package.as_deref();
        if !same_parent_file && !same_package {
            continue;
        }
        // Single pass: find the symbol at the subtype's location AND check (for
        // cross-file candidates) whether this file defines its own sealed class with
        // the same name — if so, the subtypes here extend THAT class, not ours.
        let mut found_symbol: Option<WhenMember> = None;
        let mut file_owns_sealed = false;
        for s in &file_data.symbols {
            if s.selection_range == location.range && found_symbol.is_none() {
                let is_object = s.kind == SymbolKind::OBJECT;
                let is_nested = same_parent_file
                    && s.range.start.line > parent_range.start.line
                    && s.range.end.line <= parent_range.end.line;
                found_symbol = Some(WhenMember {
                    name: s.name.clone(),
                    is_object,
                    is_nested,
                });
            }
            if !same_parent_file
                && s.name == sealed_name
                && matches!(
                    s.kind,
                    SymbolKind::CLASS | SymbolKind::INTERFACE | SymbolKind::ENUM_MEMBER
                )
            {
                file_owns_sealed = true;
            }
        }
        if !same_parent_file && file_owns_sealed {
            continue;
        }
        if let Some(member) = found_symbol {
            members.push(member);
        }
    }

    sealed_cache.insert(cache_key, members.clone());
    members
}

fn collect_existing_branches(when_node: &tree_sitter::Node, source: &[u8]) -> Vec<String> {
    let mut branches = Vec::new();
    for child in when_node.children(&mut when_node.walk()) {
        if child.kind() != KIND_WHEN_ENTRY {
            continue;
        }
        // Check for `else` branch
        for entry_child in child.children(&mut child.walk()) {
            if entry_child.kind() == KIND_ELSE {
                branches.push("else".to_string());
                continue;
            }
            if entry_child.kind() != KIND_WHEN_CONDITION {
                continue;
            }
            if let Some(name) = extract_branch_name(&entry_child, source) {
                branches.push(name);
            }
        }
    }
    branches
}

/// Extract the type/value name from a when_condition.
///
/// Handles:
/// - `is Effect.ShowToast` → "ShowToast"
/// - `Color.RED` → "RED"
/// - `is ShowToast` → "ShowToast"
/// - `OnAddMultibankClick` → "OnAddMultibankClick"
fn extract_branch_name(condition: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for child in condition.children(&mut condition.walk()) {
        match child.kind() {
            KIND_TYPE_TEST => {
                // type_test → "is" user_type → type_identifier ("." type_identifier)*
                return extract_last_type_identifier(&child, source);
            }
            KIND_NAV_EXPR => {
                // navigation_expression → simple_identifier "." simple_identifier
                return extract_nav_last_ident(&child, source);
            }
            // Bare identifier/type_identifier branch for object/data object entries.
            KIND_SIMPLE_IDENT | KIND_TYPE_IDENT => {
                return child.utf8_text(source).ok().map(|s| s.to_string());
            }
            // Boolean literals: `true` / `false`
            KIND_BOOLEAN_LITERAL => {
                return child.utf8_text(source).ok().map(|s| s.to_string());
            }
            _ => {}
        }
    }
    None
}

/// Extract the last type_identifier from a type_test node.
/// e.g. `is Effect.ShowToast` → "ShowToast", `is ShowToast` → "ShowToast"
fn extract_last_type_identifier(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut last_ident = None;
    for child in node.children(&mut node.walk()) {
        if child.kind() == KIND_USER_TYPE {
            last_ident = extract_last_type_from_user_type(&child, source);
        }
    }
    last_ident
}

fn extract_last_type_from_user_type(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    let mut last = None;
    for child in node.children(&mut node.walk()) {
        if child.kind() == KIND_TYPE_IDENT {
            last = child.utf8_text(source).ok().map(|s| s.to_string());
        }
    }
    last
}

/// Extract the last identifier from a navigation_expression.
/// e.g. `Color.RED` → "RED"
fn extract_nav_last_ident(node: &tree_sitter::Node, source: &[u8]) -> Option<String> {
    for child in node.children(&mut node.walk()) {
        if child.kind() == KIND_NAV_SUFFIX {
            for suffix_child in child.children(&mut child.walk()) {
                if suffix_child.kind() == KIND_SIMPLE_IDENT {
                    return suffix_child.utf8_text(source).ok().map(|s| s.to_string());
                }
            }
        }
    }
    None
}

fn build_branch_text(
    missing: &[&WhenMember],
    parent_type: &str,
    type_kind: TypeKind,
    indent: &str,
) -> String {
    let mut text = String::new();
    for member in missing {
        match type_kind {
            TypeKind::Boolean => {
                // Bare value: `true -> TODO()`, `false -> TODO()`
                text.push_str(&format!("{}{} -> TODO()\n", indent, member.name));
            }
            TypeKind::Enum => {
                text.push_str(&format!(
                    "{}{}.{} -> TODO()\n",
                    indent, parent_type, member.name
                ));
            }
            TypeKind::Sealed => {
                let qualified = if member.is_nested {
                    format!("{}.{}", parent_type, member.name)
                } else {
                    member.name.clone()
                };
                if member.is_object {
                    text.push_str(&format!("{}{} -> TODO()\n", indent, qualified));
                } else {
                    text.push_str(&format!("{}is {} -> TODO()\n", indent, qualified));
                }
            }
        }
    }
    text
}

/// Detect indentation for new branches.
/// Uses the first existing `when_entry`'s column, or falls back to when_expression column + 4.
fn detect_indent(when_node: &tree_sitter::Node, _source: &[u8]) -> String {
    for child in when_node.children(&mut when_node.walk()) {
        if child.kind() == KIND_WHEN_ENTRY {
            let col = child.start_position().column;
            return " ".repeat(col);
        }
    }
    let base = when_node.start_position().column;
    " ".repeat(base + 4)
}

/// Find the replace range for new branches.
///
/// When the block is empty (no existing entries), replaces from line after `{`
/// through `}` — cleaning up blank lines. When entries exist, replaces from
/// the line after the last entry through `}`.
///
/// Returns `(replace_range, closing_brace_indent)`.
fn find_insert_position(
    when_node: &tree_sitter::Node,
    _source: &[u8],
    _lines: &[String],
) -> Option<(Range, String)> {
    let child_count = when_node.child_count();
    if child_count == 0 {
        return None;
    }
    let last_child = when_node.child(child_count as u32 - 1)?;
    if last_child.kind() != KIND_RBRACE {
        return None;
    }
    let close_line = last_child.start_position().row as u32;
    let close_col = last_child.start_position().column as u32;

    // Find the last when_entry to insert after it, or `{` if none
    let last_entry = when_node
        .children(&mut when_node.walk())
        .filter(|c| c.kind() == KIND_WHEN_ENTRY)
        .last();

    let start_line = if let Some(entry) = last_entry {
        entry.end_position().row as u32 + 1
    } else {
        // No entries — find `{` and start after it
        let open = when_node
            .children(&mut when_node.walk())
            .find(|c| c.kind() == KIND_LBRACE)?;
        open.start_position().row as u32 + 1
    };

    // Clamp: if when is compact (single line), start at close_line
    let start_line = start_line.min(close_line);

    let start = Position::new(start_line, 0);
    let end = Position::new(close_line, close_col + 1);
    let brace_indent = " ".repeat(close_col as usize);
    Some((Range::new(start, end), brace_indent))
}

#[cfg(test)]
#[path = "fill_when_tests.rs"]
mod tests;
