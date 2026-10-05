//! Diagnostic: detect a plain `.` member access on a nullable receiver.
//!
//! Kotlin requires a safe call (`?.`) or non-null assertion (`!!.`) to access a
//! member through a nullable receiver. Walks the live CST for all
//! `navigation_expression` nodes using a plain `.`, and for each whose receiver
//! has a nullable inferred type — either a simple variable (`repo.load()`) or a
//! pure field-access chain (`holder.repo.load()`, where `repo` is a nullable
//! field) — checks whether the accessed name resolves to:
//! - a real class member (always an error on a nullable receiver), or
//! - an extension function/property whose own declared receiver is itself
//!   non-nullable (also an error — Kotlin won't pick that overload for a
//!   nullable argument).
//!
//! Skipped cases (too ambiguous without full type resolution):
//! - Receivers that aren't a plain identifier-and-field chain (e.g. a call
//!   result `getFoo().bar`, an index `xs[0]`, or a `?.`/`::` in the chain).
//! - `this`/`super` receivers.
//! - Names that don't resolve to either a member or a known extension (could be
//!   an unindexed JAR/stdlib symbol, smart-cast, etc.) — skip rather than guess.

use tower_lsp::lsp_types::*;

use crate::features::text_utils::utf16_column;
use crate::indexer::{live_tree::LiveDoc, local_scope_occurrences, Indexer, NodeExt};
use crate::queries::{KIND_NAV_EXPR, KIND_SIMPLE_IDENT};
use crate::resolver::infer::{
    find_field_type_in_class_impl, find_fun_return_type_by_name, find_fun_return_type_reachable,
    resolve_method_return_type_substituted,
};
use crate::resolver::infer_lines::infer_type_in_lines_raw;
use crate::resolver::{infer_variable_type_raw, ReceiverKind, ReceiverType, Resolver};
use crate::StrExt;

/// Scan a file for plain-`.` member access on nullable receivers.
///
/// The caller provides a `LiveDoc` parsed from the *same text* that was just
/// indexed, guaranteeing the CST and the indexed signature data are consistent.
pub(crate) fn nullable_dot_call_diagnostics(
    indexer: &Indexer,
    uri: &Url,
    doc: &LiveDoc,
) -> Vec<Diagnostic> {
    // NOTE: unlike `call_arg_diagnostics`, this diagnostic is *not* gated on
    // `jar_phase.is_loading()`. JAR indexing on a large project can take many
    // seconds, and gating here meant the diagnostic stayed invisible for that
    // whole window (the symptom that surfaced this: "no diagnostics on live
    // lines"). It is safe to run during loading because:
    //   * Every true positive resolves to a workspace-local symbol (a project
    //     class member via `Resolver::resolve_member`, or a project extension
    //     via `extension_by_receiver`), all of which are populated by the fast
    //     source scan — none depend on JAR symbols.
    //   * The diagnostic only fires when it *positively* resolves a member or a
    //     non-nullable extension; a partial index that simply lacks a symbol
    //     yields a skip, never a false flag.
    //   * Generic stdlib scope functions (`let`/`also`/`run`/…) are keyed in
    //     `extension_by_receiver` under their type-parameter receiver (`T`),
    //     not a concrete leaf type, so a concrete-leaf lookup never matches
    //     them — `s.let { }` on a nullable `s` is never flagged.
    let bytes = &doc.bytes;
    let mut diagnostics = Vec::new();
    for node in crate::indexer::walk::descendants(doc.tree.root_node()) {
        check_nav_node(node, bytes, indexer, uri, &mut diagnostics);
    }
    diagnostics
}

fn check_nav_node(
    node: tree_sitter::Node,
    bytes: &[u8],
    indexer: &Indexer,
    uri: &Url,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if node.kind() != KIND_NAV_EXPR {
        return;
    }
    if let Some(diagnostic) = check_nullable_dot_call(&node, bytes, indexer, uri) {
        diagnostics.push(diagnostic);
    }
}

fn check_nullable_dot_call(
    navigation_node: &tree_sitter::Node,
    bytes: &[u8],
    indexer: &Indexer,
    uri: &Url,
) -> Option<Diagnostic> {
    // navigation_expression named children: receiver_expr, navigation_suffix.
    let named_count = navigation_node.named_child_count();
    if named_count < 2 {
        return None;
    }
    let receiver_node = navigation_node.named_child(0)?;
    let suffix_node = navigation_node.named_child(named_count as u32 - 1)?;

    // The member-access operator is the suffix's first (anonymous) child:
    // "." for plain access, "?." for a safe call, "::" for a callable
    // reference. Only plain "." is unsafe on a nullable receiver.
    let operator = suffix_node.child(0)?;
    if operator.kind() != "." {
        return None;
    }

    let member_node = suffix_node.first_child_of_kind(KIND_SIMPLE_IDENT)?;
    let member_name = member_node.utf8_text_owned(bytes)?;

    // The receiver is either a simple variable (`repo.load()`) or a pure
    // field-access chain (`holder.repo.load()`, where `repo` is a nullable
    // field). `qualifier` is the text we hand to `Resolver::resolve_member` to
    // locate the member; `receiver_type` carries the inferred nullability.
    let (qualifier, receiver_type) = resolve_receiver(indexer, &receiver_node, bytes, uri)?;
    if !receiver_type.nullable {
        return None;
    }

    // 1. A real class member (declared in the body or inherited) is always an
    //    error on a nullable receiver, regardless of any extension with the
    //    same name. `resolve_member`'s underlying file search isn't
    //    container-scoped, so verify the resolved symbol is actually nested
    //    inside the receiver's class — otherwise a same-named top-level
    //    extension declared in the same file would be mistaken for a member.
    let member_locs = indexer.resolve_member(&member_name, &qualifier, uri);
    if member_locs
        .iter()
        .any(|location| is_member_of(indexer, location, &receiver_type.leaf))
    {
        return Some(diagnostic(&member_node, &qualifier, &member_name));
    }

    // 2. An extension function/property: safe only when its own declared
    //    receiver is itself nullable. `detail` is the full signature text
    //    (e.g. `"fun String?.isBlankCustom(): Boolean"`), so checking for
    //    `"?."` before the parameter list distinguishes a nullable receiver
    //    from a `?.`/`?` appearing only in a default parameter value.
    //
    //    Only consider extensions actually *visible* from this file (same
    //    package or imported). `extension_by_receiver` is workspace-global, so
    //    an unscoped match could flag a member off an extension that isn't in
    //    scope here, or be silenced by an out-of-scope nullable-receiver
    //    overload — see `extension_is_in_scope`.
    if let Some(entries) = indexer.extension_by_receiver.get(&receiver_type.leaf) {
        let caller_file_data = indexer.file_data_for(uri.as_str());
        let caller_file_data_ref = caller_file_data.as_deref();
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| entry.name == member_name)
            .filter(|entry| extension_in_scope_here(entry, uri, caller_file_data_ref))
            .collect();
        if matches.is_empty() {
            return None;
        }
        let any_nullable_safe = matches
            .iter()
            .any(|entry| extension_detail_has_nullable_receiver(&entry.detail));
        if any_nullable_safe {
            return None;
        }
        return Some(diagnostic(&member_node, &qualifier, &member_name));
    }

    None
}

/// Resolve a receiver node into the text used for member lookup plus its
/// inferred [`ReceiverType`].
///
/// Handles two shapes:
/// - a simple variable (`repo` in `repo.load()`), and
/// - a pure field-access chain (`holder.repo` in `holder.repo.load()`), where
///   a nullable data-class field is the receiver.
///
/// Returns `None` for `this`/`super` roots and for any receiver that isn't a
/// plain identifier-and-field chain (e.g. a call result like `getFoo().bar`),
/// which we can't reason about without fuller type resolution.
fn resolve_receiver(
    indexer: &Indexer,
    receiver_node: &tree_sitter::Node,
    bytes: &[u8],
    uri: &Url,
) -> Option<(String, ReceiverType)> {
    match receiver_node.kind() {
        KIND_SIMPLE_IDENT => {
            let name = receiver_node.utf8_text_owned(bytes)?;
            if name == "this" || name == "super" {
                return None;
            }
            // Scope-aware local first: a file-wide name scan can attribute the
            // receiver to an unrelated same-named declaration in another
            // function (real FP: a `tile: CoordGrid?` parameter vs
            // `val tile = free.removeAt(...)`).
            match scoped_local_receiver_type(indexer, uri, receiver_node, bytes, &name) {
                ScopedLocal::Known(raw) => Some((name, ReceiverType::from_raw(raw))),
                ScopedLocal::Unknown => None,
                ScopedLocal::Fallback => {
                    let receiver_type =
                        indexer.infer_receiver_type(ReceiverKind::Variable(&name), uri)?;
                    Some((name, receiver_type))
                }
            }
        }
        KIND_NAV_EXPR => {
            let chain = pure_field_chain(receiver_node, bytes)?;
            // Need a root plus at least one field segment, and not `this.x`/`super.x`.
            if chain.len() < 2 || chain[0] == "this" || chain[0] == "super" {
                return None;
            }
            // No CST point: unlike `fill_when` (one call per `when` node),
            // this runs once per `navigation_expression` in the whole file —
            // for a long field-access chain that's one call per segment, and
            // `enclosing_smart_cast_type`'s ancestor walk is only safe to
            // call this often because it bounds *segment count*, which does
            // NOT bound tree depth for the receiver nodes closest to the
            // chain's root (see `MAX_SMART_CAST_CHAIN_LEN`'s doc comment).
            // Falls back to the line-scanning `smart_cast_narrowed_type`
            // (root-only) inside `infer_field_chain_type` instead — no
            // regression from before this file started passing a CST point,
            // since it never did.
            let line = receiver_node.start_position().row as u32;
            let (receiver_type, _declaring_uri) =
                indexer.infer_field_chain_type(&chain, uri, line, None)?;
            Some((chain.join("."), receiver_type))
        }
        _ => None,
    }
}

/// Depth budget for chasing a scoped receiver chain (`val a = b.foo()` where
/// `b` is itself a scoped local). Mirrors `MAX_RAW_TYPE_INFER_DEPTH` (4),
/// which caps the same cycle one layer down.
const SCOPED_INFER_DEPTH: u8 = 4;

/// How the lexically visible declaration answers.
enum ScopedLocal {
    /// Not a lexically visible local — caller falls back to file-wide inference.
    Fallback,
    /// Visible local, type known.
    Known(String),
    /// Visible local, type unresolvable — caller must SKIP, not fall back:
    /// file-wide inference could attribute a same-named declaration from
    /// another function.
    Unknown,
}

/// Resolve a simple-identifier receiver to the type of the declaration
/// lexically visible at its position, via block-scope occurrences rather than
/// a file-wide name scan. A file-wide scan matches the first same-named
/// declaration anywhere — including an unrelated parameter or local in
/// another function — and attributes its (possibly nullable) type to this
/// use site.
fn scoped_local_receiver_type(
    indexer: &Indexer,
    uri: &Url,
    receiver_node: &tree_sitter::Node,
    bytes: &[u8],
    name: &str,
) -> ScopedLocal {
    scoped_variable_type(
        indexer,
        uri,
        bytes,
        name,
        receiver_node.start_position().row as u32,
        receiver_node.start_position().column,
        SCOPED_INFER_DEPTH,
    )
}

/// Core: type of `name` as declared visibly at `use_line`/`use_byte_col`.
fn scoped_variable_type(
    indexer: &Indexer,
    uri: &Url,
    bytes: &[u8],
    name: &str,
    use_line: u32,
    use_byte_col: usize,
    depth: u8,
) -> ScopedLocal {
    if depth == 0 {
        return ScopedLocal::Unknown;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return ScopedLocal::Fallback,
    };
    let use_line_text = match text.lines().nth(use_line as usize) {
        Some(line) => line,
        None => return ScopedLocal::Fallback,
    };
    let prefix = match use_line_text.get(..use_byte_col.min(use_line_text.len())) {
        Some(prefix) => prefix,
        None => return ScopedLocal::Fallback,
    };
    let pos = Position::new(use_line, utf16_column(prefix));
    let occurrences = match local_scope_occurrences(indexer, uri, pos) {
        Some(occurrences) if !occurrences.is_empty() => occurrences,
        _ => return ScopedLocal::Fallback,
    };
    // Visible declaration = latest declaration-form occurrence at or before
    // the use. Same-generation occurrences share one binding by construction.
    let mut decl_line: Option<u32> = None;
    for occurrence in &occurrences {
        let line = occurrence.range.start.line;
        if line > use_line {
            continue;
        }
        let occ_text = match text.lines().nth(line as usize) {
            Some(candidate) => candidate,
            None => continue,
        };
        if is_value_or_param_declaration(occ_text, name) {
            decl_line = Some(decl_line.map_or(line, |best| best.max(line)));
        }
    }
    let decl_line = match decl_line {
        Some(line) => line,
        None => return ScopedLocal::Fallback,
    };
    let decl_text = match text.lines().nth(decl_line as usize) {
        Some(line) => line,
        None => return ScopedLocal::Fallback,
    };
    // 1. Explicit annotation on the visible declaration itself.
    if let Some(known) = infer_type_in_lines_raw(&[decl_text.to_owned()], name) {
        return ScopedLocal::Known(known);
    }
    // 2. Unannotated val/var: initializer tables filtered to this exact line
    // (never another same-named declaration's entry).
    if is_value_declaration(decl_text, name) {
        if let Some(known) =
            scoped_initializer_type(indexer, uri, bytes, name, decl_text, decl_line, depth)
        {
            return ScopedLocal::Known(known);
        }
        return ScopedLocal::Unknown;
    }
    // 3. Anything else (lambda params, catch, for, destructuring): old behavior.
    ScopedLocal::Fallback
}

/// Whether `line` plausibly declares `name`: a `val`/`var` declaration or a
/// `name:` annotation/parameter form (whole word). Comment lines never
/// declare. Used only to pick WHICH in-scope occurrence is the declaration;
/// the type itself always comes from the annotation/initializer readers.
fn is_value_or_param_declaration(line: &str, name: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
        return false;
    }
    if is_value_declaration(line, name) {
        return true;
    }
    // `name:` form, whole word, not a `::` callable reference.
    let pattern = format!("{name}:");
    let mut search_from = 0;
    while let Some(relative) = line[search_from..].find(&pattern) {
        let pos = search_from + relative;
        let before_ok = pos == 0
            || !matches!(line.as_bytes().get(pos - 1), Some(b) if b.is_ascii_alphanumeric() || *b == b'_');
        let after_ok = !matches!(line.as_bytes().get(pos + pattern.len()), Some(b':'));
        if before_ok && after_ok {
            return true;
        }
        search_from = pos + 1;
    }
    false
}

/// Whether `line` declares `name` as a `val`/`var` (whole word).
fn is_value_declaration(line: &str, name: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
        return false;
    }
    for keyword in ["val ", "var "] {
        if let Some(rest) = trimmed.strip_prefix(keyword) {
            return rest == name
                || rest.strip_prefix(name).is_some_and(|after| {
                    !after
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphanumeric() || c == '_')
                });
        }
    }
    false
}

/// Type of an unannotated `val`/`var` from the initializer tables, filtered
/// to the visible declaration's own line — never another same-named
/// declaration's entry.
fn scoped_initializer_type(
    indexer: &Indexer,
    uri: &Url,
    bytes: &[u8],
    name: &str,
    decl_text: &str,
    decl_line: u32,
    depth: u8,
) -> Option<String> {
    let data = indexer.files.get(uri.as_str())?;
    if let Some(ty) = data
        .rhs_types
        .iter()
        .find(|(line, var, _)| *line == decl_line && var == name)
        .map(|(_, _, ty)| ty.clone())
    {
        return Some(ty);
    }
    // Clone out of the DashMap guard before recursing (re-entrant shard locks).
    let method_match = data
        .method_call_rhs
        .iter()
        .find(|(line, var, _, _)| *line == decl_line && var == name)
        .map(|(_, _, recv, method)| (recv.clone(), method.clone()));
    let field_match = data
        .field_access_rhs
        .iter()
        .find(|(line, var, _, _)| *line == decl_line && var == name)
        .map(|(_, _, recv, field)| (recv.clone(), field.clone()));
    drop(data);
    if let Some((recv, method)) = method_match {
        let eq = assignment_eq_pos(decl_text).unwrap_or(0);
        let recv_col = find_ident_col(decl_text, &recv, eq + 1).unwrap_or(eq + 1);
        let recv_raw = match scoped_variable_type(
            indexer,
            uri,
            bytes,
            &recv,
            decl_line,
            recv_col,
            depth - 1,
        ) {
            ScopedLocal::Known(raw) => raw,
            ScopedLocal::Unknown => return None,
            ScopedLocal::Fallback => infer_variable_type_raw(indexer, &recv, uri)?,
        };
        if let Some(ret) = resolve_method_return_type_substituted(indexer, &recv_raw, &method, uri)
        {
            return Some(ret);
        }
    }
    if let Some((recv, field)) = field_match {
        let eq = assignment_eq_pos(decl_text).unwrap_or(0);
        let recv_col = find_ident_col(decl_text, &recv, eq + 1).unwrap_or(eq + 1);
        let recv_raw = match scoped_variable_type(
            indexer,
            uri,
            bytes,
            &recv,
            decl_line,
            recv_col,
            depth - 1,
        ) {
            ScopedLocal::Known(raw) => raw,
            ScopedLocal::Unknown => return None,
            ScopedLocal::Fallback => infer_variable_type_raw(indexer, &recv, uri)?,
        };
        let recv_base = recv_raw
            .split('<')
            .next()
            .unwrap_or(&recv_raw)
            .rsplit('.')
            .next()
            .unwrap_or(&recv_raw)
            .strip_nullable();
        if let Some((field_type, _)) =
            find_field_type_in_class_impl(indexer, recv_base, &field, uri, depth - 1)
        {
            return Some(field_type);
        }
    }
    // Bare call: `val x = compute(...)` — same import-aware rule the old
    // line-scan fallback used (position-independent, so no scope hazard).
    if let Some(callee) = bare_call_callee(decl_text) {
        if let Some(ty) = find_fun_return_type_reachable(indexer, callee, uri)
            .or_else(|| find_fun_return_type_by_name(indexer, callee, uri))
        {
            return Some(ty);
        }
    }
    None
}

/// Callee of a bare (receiver-less, lowercase) call on a declaration RHS,
/// e.g. `compute` in `val x = compute(a)`. Returns `None` for qualified
/// calls (`a.b()` — handled via the tables above), chains (`a().b()`),
/// and non-call initializers.
fn bare_call_callee(decl_text: &str) -> Option<&str> {
    let eq = assignment_eq_pos(decl_text)?;
    let after = decl_text[eq + 1..].trim_start();
    let paren = after.find('(')?;
    let before = after[..paren].trim_end();
    if before.contains(['.', ' ', '{', '"', '\'']) {
        return None;
    }
    if !before.starts_with_lowercase() {
        return None;
    }
    // Chained `foo().bar()`: the call's own args end where depth returns to
    // zero; anything past that starting with `.` is a chain, not a bare call.
    let mut depth = 0u32;
    for (index, ch) in after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return if after[index + 1..].trim_start().starts_with('.') {
                        None
                    } else {
                        Some(before)
                    };
                }
            }
            _ => {}
        }
    }
    None
}

/// Byte offset of the `=` that assigns a declaration RHS, skipping `==`,
/// `=>`, `>=`, `<=`, `!=`.
fn assignment_eq_pos(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'=' {
            let prev = index.checked_sub(1).and_then(|i| bytes.get(i)).copied();
            let next = bytes.get(index + 1).copied();
            let prev_ok = prev.is_none_or(|b| !matches!(b, b'=' | b'>' | b'<' | b'!'));
            let next_ok = next.is_none_or(|b| !matches!(b, b'=' | b'>'));
            if prev_ok && next_ok {
                return Some(index);
            }
        }
        index += 1;
    }
    None
}

/// Byte column of the first whole-word `name` at or after `from` in `line`.
/// Used to place the scope cursor on a receiver ident inside a declaration
/// RHS. Returns `None` when there is no whole-word match (positions would
/// be guesses — the caller degrades instead).
fn find_ident_col(line: &str, name: &str, from: usize) -> Option<usize> {
    let mut search_from = from.min(line.len());
    loop {
        let rest = line.get(search_from..)?;
        let relative = rest.find(name)?;
        let pos = search_from + relative;
        let before_ok = pos == 0
            || !matches!(line.as_bytes().get(pos - 1), Some(b) if b.is_ascii_alphanumeric() || *b == b'_');
        let after_ok = !matches!(
            line.as_bytes().get(pos + name.len()),
            Some(b) if b.is_ascii_alphanumeric() || *b == b'_'
        );
        if before_ok && after_ok {
            return Some(pos);
        }
        search_from = pos + 1;
    }
}

/// Collect a pure field-access chain into its segment names, or `None` if the
/// node is anything other than a simple identifier optionally followed by
/// `.field` accesses (all plain `.`, no `?.`/`::`, no call/index suffixes).
///
/// `holder.repo` → `["holder", "repo"]`; `getFoo().bar` → `None`.
fn pure_field_chain(node: &tree_sitter::Node, bytes: &[u8]) -> Option<Vec<String>> {
    pure_field_chain_at(node, bytes, 0)
}

fn pure_field_chain_at(
    node: &tree_sitter::Node,
    bytes: &[u8],
    depth: usize,
) -> Option<Vec<String>> {
    // Kind-bounded (only descends through KIND_NAV_EXPR), but an arbitrarily
    // long `a.b.c.d…` field-access chain is still an arbitrarily deep
    // recursion — cap it. See `crate::util::MAX_CST_DESCENT_DEPTH`.
    if depth >= crate::util::MAX_CST_DESCENT_DEPTH {
        crate::util::report_cst_depth_exceeded!("pure_field_chain_at", *node);
        return None;
    }
    match node.kind() {
        KIND_SIMPLE_IDENT => Some(vec![node.utf8_text_owned(bytes)?]),
        KIND_NAV_EXPR => {
            let named_count = node.named_child_count();
            if named_count < 2 {
                return None;
            }
            let receiver = node.named_child(0)?;
            let suffix = node.named_child(named_count as u32 - 1)?;
            // Only plain `.` field access — reject `?.`, `::`, and any suffix
            // whose accessed name isn't a simple identifier.
            if suffix.child(0)?.kind() != "." {
                return None;
            }
            let field = suffix.first_child_of_kind(KIND_SIMPLE_IDENT)?;
            let mut chain = pure_field_chain_at(&receiver, bytes, depth + 1)?;
            chain.push(field.utf8_text_owned(bytes)?);
            Some(chain)
        }
        _ => None,
    }
}

/// Whether `entry` (a workspace-global extension) is actually visible from the
/// file at `uri`: declared in the *same file*, or — for an ORDINARY top-level
/// extension only — in scope per `extension_is_in_scope` (same package,
/// including two default-package files, or covered by an import).
fn extension_in_scope_here(
    entry: &crate::types::ExtensionEntry,
    uri: &Url,
    caller_file_data: Option<&crate::types::FileData>,
) -> bool {
    // A member extension (`entry.container.is_some()`) is excluded from the
    // package/import check entirely, not just passed `None` for its own
    // `container` field: this diagnostic has no evidence a member
    // extension's dispatch receiver (e.g. `ColumnScope`) is actually active
    // at the call site, so letting the ordinary package-match rule below
    // independently succeed whenever the caller happens to share the member
    // extension's package — as it would for a plain `Some(&caller_file_data)`
    // in the same package — would let an unrelated `m.weight()` wrongly emit
    // or suppress a `?.` warning just as surely as the member-extension
    // short-circuit itself would. Only the *declaring file* remains a valid
    // positive match for a member extension here.
    entry.file_uri == uri.as_str()
        || (entry.container.is_none()
            && crate::resolver::infer::extension_is_in_scope(
                entry.package.as_ref(),
                &entry.name,
                None,
                crate::types::Visibility::Public,
                false,
                caller_file_data,
            ))
}

/// Whether the symbol declared at `location` is nested inside a container
/// named `class_name` — i.e. a real member, not a same-named top-level
/// symbol (e.g. an extension function) that happens to live in the same file.
fn is_member_of(indexer: &Indexer, location: &Location, class_name: &str) -> bool {
    let Some(file_data) = indexer.file_data_for(location.uri.as_str()) else {
        return false;
    };
    file_data
        .symbols
        .iter()
        .find(|symbol| symbol.selection_range == location.range)
        .is_some_and(|symbol| symbol.container.as_deref() == Some(class_name))
}

/// Whether an extension's signature text declares a nullable receiver, e.g.
/// `"fun String?.isBlankCustom(): Boolean"`. Checks only the text before the
/// first `(` so a `?.`/`?` inside a default parameter value doesn't count.
fn extension_detail_has_nullable_receiver(detail: &str) -> bool {
    detail.split('(').next().unwrap_or(detail).contains("?.")
}

fn diagnostic(
    member_node: &tree_sitter::Node,
    receiver_name: &str,
    member_name: &str,
) -> Diagnostic {
    let start = member_node.start_position();
    let end = member_node.end_position();
    Diagnostic {
        range: Range::new(
            Position::new(start.row as u32, start.column as u32),
            Position::new(end.row as u32, end.column as u32),
        ),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some("kmp-lsp".into()),
        message: format!(
            "{member_name}: receiver '{receiver_name}' is nullable — use '?.' or '!!.' to access this member"
        ),
        ..Default::default()
    }
}

#[cfg(test)]
#[path = "nullable_call_diagnostics_tests.rs"]
mod tests;
