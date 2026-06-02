use super::rename::whole_word_replace_file;
use super::Backend;
use crate::indexer::live_tree::lang_for_path;
use crate::indexer::Indexer;
use crate::inlay_hints::infer_type_from_init;
use crate::queries::*;
use crate::resolver::{already_imported, fqns_for_name};
use crate::LinesExt;
use crate::StrExt;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;

/// Returns true if `name` is a keyword that precedes a block but is NOT
/// a function call — i.e. we should NOT show signature help for it.
pub(super) fn is_non_call_keyword(name: &str) -> bool {
    matches!(
        name,
        "fun"
            | "if"
            | "while"
            | "for"
            | "when"
            | "catch"
            | "constructor"
            | "override"
            | "else"
            | "return"
            | "throw"
            | "try"
            | "finally"
            | "object"
            | "class"
            | "interface"
            | "enum"
            | "init"
    )
}

/// expression around it — e.g. `isRefreshing` → `refreshDashboardInteractor.isRefreshing()`.
///
/// - Expands LEFT:  eats `[a-zA-Z0-9_.]` (dotted receiver chain)
/// - Expands RIGHT: eats remaining identifier chars, then a balanced `(…)` if present
fn expand_call_expr(chars: &[char], s: usize, e: usize) -> (usize, usize) {
    // Expand left over [a-zA-Z0-9_.]
    let mut new_s = s;
    while new_s > 0 {
        let c = chars[new_s - 1];
        if c.is_alphanumeric() || c == '_' || c == '.' {
            new_s -= 1;
        } else {
            break;
        }
    }
    // Strip leading dots we may have swallowed.
    while new_s < e && chars[new_s] == '.' {
        new_s += 1;
    }

    // Expand right over remaining identifier chars.
    let mut new_e = e;
    while new_e < chars.len() {
        let c = chars[new_e];
        if c.is_alphanumeric() || c == '_' {
            new_e += 1;
        } else {
            break;
        }
    }
    // Eat balanced `(…)` if present.
    if new_e < chars.len() && chars[new_e] == '(' {
        let mut depth = 0usize;
        while new_e < chars.len() {
            match chars[new_e] {
                '(' => {
                    depth += 1;
                    new_e += 1;
                }
                ')' => {
                    new_e += 1;
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {
                    new_e += 1;
                }
            }
        }
    }
    (new_s, new_e)
}

/// Derive a local variable name from an expression.
///
/// `refreshDashboardInteractor.isRefreshing()` → `isRefreshing`
/// `user.getName()` → `name`  (strips "get" prefix)
/// `someValue` → `someValue`
fn derive_var_name(expr: &str) -> String {
    // Take the last `.`-separated segment, strip trailing `()` / `(…)`.
    let seg = expr.trim().rsplit('.').next().unwrap_or(expr.trim());
    let seg = if let Some(p) = seg.find('(') {
        &seg[..p]
    } else {
        seg
    };
    let seg = seg.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');

    // Strip common accessor prefixes: getXxx → xxx, isXxx → isXxx (keep),
    // hasXxx → hasXxx (keep), setXxx → skip (nothing useful).
    let result = if seg.starts_with("get") && seg.len() > "get".len() {
        let rest = &seg["get".len()..];
        // Only strip if next char is uppercase (proper camelCase).
        if rest.starts_with_uppercase() {
            let r = if let Some(first) = rest.chars().next() {
                let mut s = first.to_lowercase().collect::<String>();
                s.push_str(&rest[first.len_utf8()..]);
                s
            } else {
                rest.to_string()
            };
            r
        } else {
            seg.to_string()
        }
    } else {
        seg.to_string()
    };

    if result.is_empty() {
        "value".to_string()
    } else {
        result
    }
}

impl Backend {
    pub(super) async fn completion_impl(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>> {
        let pp = params.text_document_position;
        let uri = &pp.text_document.uri;
        let position = pp.position;
        let snippets = self.snippet_support.load(Ordering::Relaxed);

        let (mut items, hit_cap) = self.indexer.completions(uri, position, snippets);
        let still_indexing = self.indexer.indexing_in_progress.load(Ordering::Acquire);
        if items.is_empty() && !still_indexing {
            return Ok(None);
        }
        // Pre-select the best match so the editor highlights it without requiring
        // an extra keystroke (mirrors RA's preselect behaviour).
        if let Some(first) = items.first_mut() {
            first.preselect = Some(true);
        }
        // When hit_cap is true the list was truncated — tell the client to
        // re-request completions on every keystroke so the list stays tight
        // as the user types more characters.
        // Also mark incomplete while the workspace is still being indexed so
        // the client keeps re-querying instead of caching a partial result.
        Ok(Some(CompletionResponse::List(CompletionList {
            is_incomplete: hit_cap || still_indexing,
            items,
        })))
    }

    pub(super) async fn code_action_impl(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<Vec<CodeActionOrCommand>>> {
        let uri = &params.text_document.uri;
        let range = params.range;

        let lines = self.indexer.mem_lines_for(uri.as_str());
        let line_text: String = {
            let ln = range.start.line as usize;
            lines
                .as_ref()
                .and_then(|lines| lines.get(ln).cloned())
                .unwrap_or_default()
        };

        let trimmed = line_text.trim().to_owned();
        let is_import_ln = trimmed.starts_with("import ") || trimmed.starts_with("package ");
        let sel_start = range.start;
        let sel_end = range.end;
        let has_selection = sel_start != sel_end && sel_start.line == sel_end.line;

        let mut actions: Vec<CodeActionOrCommand> = Vec::with_capacity(6);

        if has_selection && !is_import_ln {
            if let Some(a) = build_introduce_variable(&line_text, uri, range) {
                actions.push(a);
            }
        }

        let all_lines: Vec<String> = lines
            .as_ref()
            .map(|lines| lines.as_ref().clone())
            .unwrap_or_default();

        let cursor_word = line_text.word_at_utf16_col(range.start.character as usize);
        let is_kotlin = crate::Language::from_path(uri.path()) == crate::Language::Kotlin;

        if let Some(a) = build_import_alias_action(&line_text, &trimmed, uri, range, is_kotlin) {
            actions.push(a);
        }

        if is_kotlin
            && !is_import_ln
            && !cursor_word.is_empty()
            && cursor_word.starts_with_uppercase()
        {
            if let Some(a) = build_rename_placeholder_action(&cursor_word, &all_lines, uri) {
                actions.push(a);
            }
        }

        // ── "Add missing import" quick-fix ─────────────────────────────────────
        if !is_import_ln && !cursor_word.is_empty() && cursor_word.starts_with_uppercase() {
            let lang = crate::Language::from_path(uri.path());
            let imports: Vec<crate::types::ImportEntry> =
                if lang == crate::Language::Kotlin || lang == crate::Language::Java {
                    all_lines.parse_imports()
                } else {
                    vec![]
                };
            let needs_semicolons = lang.needs_semicolons();
            for a in build_add_missing_import_actions(
                &self.indexer,
                &cursor_word,
                &imports,
                &all_lines,
                uri,
                needs_semicolons,
            ) {
                actions.push(a);
            }
        }

        // ── "Suppress warning" quick-fix ────────────────────────────────────────
        let diagnostics = &params.context.diagnostics;
        if !diagnostics.is_empty() && !is_import_ln && is_kotlin {
            for diag in diagnostics
                .iter()
                .filter(|d| d.range.start <= range.start && d.range.end >= range.end)
            {
                if let Some(a) =
                    build_suppress_warning_action(diag, &all_lines, uri, range.start.line)
                {
                    actions.push(a);
                    break; // One suppress action per problem is enough.
                }
            }
        }

        // ── "Specify type explicitly" code action ─────────────────────────────
        if is_kotlin
            && !is_import_ln
            && !line_text.contains(':')
            && (line_text.trim_start().starts_with("val ")
                || line_text.trim_start().starts_with("var "))
        {
            if let Some(a) =
                build_explicit_type_action(&self.indexer, &all_lines, uri, range.start.line)
            {
                actions.push(a);
            }
        }

        // ── "Generate override stubs" quick-fix ──────────────────────────────────
        if is_kotlin && !is_import_ln && !has_selection {
            if let Some(a) =
                build_generate_overrides_action(&self.indexer, &all_lines, uri, range.start.line)
            {
                actions.push(a);
            }
        }

        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }
}

/// Builds the "Introduce local variable" code action for the selected expression.
/// Build a code action that adds an explicit type annotation to an untyped
/// `val` / `var` declaration.
///
/// Reuses the same type inference logic as inlay hints / hover.
fn build_explicit_type_action(
    idx: &Arc<Indexer>,
    all_lines: &[String],
    uri: &Url,
    line: u32,
) -> Option<CodeActionOrCommand> {
    let _ = idx;
    let content = all_lines.join("\n");
    let lang = lang_for_path(uri.path())?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&lang).ok()?;
    let tree = parser.parse(&content, None)?;
    let bytes = content.as_bytes();

    // Walk root-level children to find a PROP_DECL on the target line.
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut initializer = None;
    let mut name_pos = None;
    let mut found = false;

    'outer: loop {
        let node = cursor.node();
        if node.kind() == KIND_PROP_DECL && node.start_position().row == line as usize {
            let mut nc = node.walk();
            loop {
                let c = nc.node();
                match c.kind() {
                    KIND_SIMPLE_IDENT if name_pos.is_none() => {
                        name_pos = Some(c.end_position());
                    }
                    KIND_COLON => {
                        // Already has a type → skip.
                        return None;
                    }
                    KIND_EQ if nc.goto_next_sibling() => {
                        let init_node = nc.node();
                        if init_node.kind() != KIND_EQ {
                            initializer = Some(init_node);
                        }
                    }
                    _ => {}
                }
                if !nc.goto_next_sibling() {
                    break;
                }
            }
            found = true;
            break 'outer;
        }
        if !cursor.goto_next_sibling() {
            break;
        }
    }

    if !found {
        return None;
    }
    let init_node = initializer?;
    let pos = name_pos?;
    let type_name = infer_type_from_init(init_node, bytes)?;
    let insert_pos = Position::new(pos.row as u32, pos.column as u32);

    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Specify type explicitly \": {type_name}\""),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: None,
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(
                uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: insert_pos,
                        end: insert_pos,
                    },
                    new_text: format!(": {type_name}"),
                }],
            )])),
            document_changes: None,
            change_annotations: None,
        }),
        is_preferred: None,
        disabled: None,
        data: None,
        command: None,
    }))
}

fn build_introduce_variable(
    line_text: &str,
    uri: &Url,
    range: Range,
) -> Option<CodeActionOrCommand> {
    let chars: Vec<char> = line_text.chars().collect();
    let utf16_to_char = |utf16: usize| {
        let mut cu = 0usize;
        for (i, c) in chars.iter().enumerate() {
            if cu >= utf16 {
                return i;
            }
            cu += c.len_utf16();
        }
        chars.len()
    };
    let raw_s = utf16_to_char(range.start.character as usize);
    let raw_e = utf16_to_char(range.end.character as usize);
    let (s, e) = expand_call_expr(&chars, raw_s, raw_e);
    let expr: String = chars[s..e].iter().collect();
    if expr.trim().is_empty() {
        return None;
    }

    let var_name = derive_var_name(&expr);
    let indent: String = line_text
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let prefix: String = chars[..s].iter().collect();
    let suffix: String = chars[e..].iter().collect();
    let replaced_line = format!("{prefix}{var_name}{suffix}");
    let line_utf16_len: u32 = line_text.chars().map(|c| c.len_utf16() as u32).sum();
    let new_text = format!("{indent}val {var_name} = {expr}\n{replaced_line}");

    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: Position {
                    line: range.start.line,
                    character: 0,
                },
                end: Position {
                    line: range.start.line,
                    character: line_utf16_len,
                },
            },
            new_text,
        }],
    );
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Introduce local variable `{var_name}`"),
        kind: Some(CodeActionKind::REFACTOR_EXTRACT),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Builds the "Add import alias" action for an import line (Kotlin only).
fn build_import_alias_action(
    line_text: &str,
    trimmed: &str,
    uri: &Url,
    range: Range,
    is_kotlin: bool,
) -> Option<CodeActionOrCommand> {
    if !is_kotlin || !trimmed.starts_with("import ") || trimmed.contains(" as ") {
        return None;
    }
    let path = trimmed
        .trim_start_matches("import ")
        .trim()
        .trim_end_matches(".*");
    let alias = path.rsplit('.').next().unwrap_or(path);
    if alias.is_empty() {
        return None;
    }

    let ln = range.start.line;
    // Use the full (unstripped) line for the column so that indented import lines
    // (uncommon but valid) get the alias appended at the correct position.
    let col: u32 = line_text.chars().map(|c| c.len_utf16() as u32).sum();
    let mut changes = std::collections::HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: Position {
                    line: ln,
                    character: col,
                },
                end: Position {
                    line: ln,
                    character: col,
                },
            },
            new_text: format!(" as {alias}"),
        }],
    );
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Add import alias `as {alias}`"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Builds the "Alias/rename in file" action for an uppercase type name (Kotlin only).
fn build_rename_placeholder_action(
    cursor_word: &str,
    all_lines: &[String],
    uri: &Url,
) -> Option<CodeActionOrCommand> {
    if all_lines.is_empty() {
        return None;
    }
    let placeholder = format!("_{cursor_word}");
    let new_content = whole_word_replace_file(all_lines, cursor_word, &placeholder);
    let last_line = (all_lines.len() - 1) as u32;
    let last_col = all_lines
        .last()
        .map(|l| l.chars().map(|c| c.len_utf16() as u32).sum::<u32>())
        .unwrap_or(0);

    // Check if there's a matching import to also alias.
    let import_edit = all_lines
        .iter()
        .enumerate()
        .find(|(_, l)| {
            let t = l.trim();
            t.starts_with("import ")
                && !t.contains(" as ")
                && t.rsplit(['.', ' '])
                    .next()
                    .map(|s| s == cursor_word)
                    .unwrap_or(false)
        })
        .map(|(import_ln, import_line_text)| {
            let col = import_line_text
                .chars()
                .map(|c| c.len_utf16() as u32)
                .sum::<u32>();
            TextEdit {
                range: Range {
                    start: Position {
                        line: import_ln as u32,
                        character: col,
                    },
                    end: Position {
                        line: import_ln as u32,
                        character: col,
                    },
                },
                new_text: format!(" as {placeholder}"),
            }
        });

    let mut body_edit = TextEdit {
        range: Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: last_line,
                character: last_col,
            },
        },
        new_text: new_content,
    };

    // Splice the import alias into the already-replaced body content so we
    // emit a single TextEdit (LSP doesn't guarantee ordering for overlapping edits).
    if let Some(ie) = import_edit {
        let mut body_lines: Vec<String> = body_edit
            .new_text
            .split('\n')
            .map(|s| s.to_owned())
            .collect();
        let iln = ie.range.start.line as usize;
        if iln < body_lines.len() {
            body_lines[iln].push_str(&ie.new_text);
        }
        body_edit.new_text = body_lines.join("\n");
    }

    let title = if body_edit.new_text.contains(&placeholder) {
        format!("Alias `{cursor_word}` as `{placeholder}` in file (then :%s/{placeholder}/NewName)")
    } else {
        format!("Rename `{cursor_word}` → `{placeholder}` in file")
    };

    let mut changes = std::collections::HashMap::new();
    changes.insert(uri.clone(), vec![body_edit]);
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::REFACTOR),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

// ── "Add missing import" ─────────────────────────────────────────────────

/// Build one "Add import" code action per importable FQN for `type_name`.
///
/// Only proposes imports when the type is not already visible in the current
/// file — checks exact imports, star imports, and same-package visibility.
fn build_add_missing_import_actions(
    idx: &Indexer,
    type_name: &str,
    imports: &[crate::types::ImportEntry],
    lines: &[String],
    uri: &Url,
    needs_semicolons: bool,
) -> Vec<CodeActionOrCommand> {
    // Skip if the name is already visible via existing imports or same package.
    let package_name = idx
        .files
        .get(uri.as_str())
        .and_then(|f| f.package.clone())
        .unwrap_or_default();

    // Check same-file definition — if the type is defined in the same file, no import needed.
    let defined_in_file = idx
        .files
        .get(uri.as_str())
        .map(|f| f.symbols.iter().any(|s| s.name == type_name))
        .unwrap_or(false);
    if defined_in_file {
        return vec![];
    }

    let fqns = fqns_for_name(idx, type_name);
    if fqns.is_empty() {
        return vec![];
    }

    fqns.into_iter()
        .filter(|fqn| !already_imported(fqn, imports))
        .filter(|fqn| {
            let pkg = fqn.rfind('.').map(|i| &fqn[..i]).unwrap_or("");
            // Not importable from the same package — it's already visible.
            pkg != package_name
        })
        .map(|fqn| {
            let title = format!("Import `{fqn}`");
            let line = lines.import_insertion_line();
            let stmt = if needs_semicolons {
                format!("import {fqn};")
            } else {
                format!("import {fqn}")
            };
            let needs_blank = line > 0
                && lines
                    .get((line - 1) as usize)
                    .map(|l| l.trim_start().starts_with("package "))
                    .unwrap_or(false)
                && lines
                    .get(line as usize)
                    .map(|l| !l.trim().is_empty())
                    .unwrap_or(false);
            let new_text = if needs_blank {
                format!("\n{stmt}\n")
            } else {
                format!("{stmt}\n")
            };

            let mut changes = HashMap::new();
            changes.insert(
                uri.clone(),
                vec![TextEdit {
                    range: Range {
                        start: Position { line, character: 0 },
                        end: Position { line, character: 0 },
                    },
                    new_text,
                }],
            );
            CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(CodeActionKind::QUICKFIX),
                edit: Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }),
                ..Default::default()
            })
        })
        .collect()
}

// ── "Suppress warning" ────────────────────────────────────────────────────

/// Build a "Suppress" code action for a diagnostic at the cursor position.
///
/// Currently handles:
/// - Add `@Suppress("unused")` annotation for unused symbol warnings
/// - Add `@Suppress("DEPRECATION")` for deprecation warnings
fn build_suppress_warning_action(
    diag: &Diagnostic,
    lines: &[String],
    uri: &Url,
    cursor_line: u32,
) -> Option<CodeActionOrCommand> {
    let msg = diag.message.to_lowercase();
    let category = if msg.contains("unused") || msg.contains("never used") {
        "\"unused\""
    } else if msg.contains("deprecat") {
        "\"DEPRECATION\""
    } else if msg.contains("unchecked") {
        "\"UNCHECKED_CAST\""
    } else {
        return None;
    };

    let line = cursor_line as usize;
    if line >= lines.len() {
        return None;
    }

    let current_line = &lines[line];
    let indent: String = current_line
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    let new_text = format!("{indent}@Suppress({category})\n");
    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: Position {
                    line: cursor_line,
                    character: 0,
                },
                end: Position {
                    line: cursor_line,
                    character: 0,
                },
            },
            new_text,
        }],
    );
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Suppress `@{category}` warning"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

// ── "Generate override stubs" ─────────────────────────────────────────────

/// Build a "Generate override methods" code action.
///
/// Works when the cursor is inside a class body that extends a superclass or
/// implements interfaces. Finds the supertype's methods that are not yet
/// overridden in the current class and generates stub implementations.
fn build_generate_overrides_action(
    idx: &Indexer,
    lines: &[String],
    uri: &Url,
    cursor_line: u32,
) -> Option<CodeActionOrCommand> {
    // Find the enclosing class.
    let file_data = idx.files.get(uri.as_str())?;
    let class_name = file_data.containing_class_at(cursor_line)?;

    // Find the class symbol.
    let class_symbol = file_data.symbols.iter().find(|s| s.name == class_name)?;
    let class_start_line = class_symbol.selection_start();

    // Find the supertypes of this class.
    let super_names: Vec<String> = file_data
        .supers
        .iter()
        .filter(|(l, _, _)| *l == class_start_line)
        .map(|(_, name, _)| name.clone())
        .collect();

    if super_names.is_empty() {
        return None;
    }

    // Collect methods already defined in the current class (skip override if
    // already present — including `override fun` declarations).
    let existing_methods: HashSet<&str> = file_data
        .symbols
        .iter()
        .filter(|s| {
            s.selection_start() > class_start_line
                && s.selection_start() < class_symbol.range.end.line
                && matches!(s.kind, SymbolKind::METHOD | SymbolKind::FUNCTION)
        })
        .map(|s| s.name.as_str())
        .collect();

    // For each supertype, find its methods that can be overridden.
    let mut override_methods: Vec<(String, String)> = Vec::with_capacity(16);

    for super_name in &super_names {
        let super_locs = match idx.definitions.get(super_name.as_str()) {
            Some(locs) => locs,
            None => continue,
        };
        for loc in super_locs.iter() {
            let super_file = match idx.files.get(loc.uri.as_str()) {
                Some(f) => f,
                None => continue,
            };
            let super_class_sym = match super_file.symbols.iter().find(|s| {
                s.name == super_name.as_str() && s.selection_start() == loc.range.start.line
            }) {
                Some(s) => s,
                None => continue,
            };

            let super_end = super_class_sym.range.end.line;
            let super_start = super_class_sym.selection_start();

            for sym in &super_file.symbols {
                if sym.selection_start() <= super_start || sym.selection_start() >= super_end {
                    continue;
                }
                if !matches!(
                    sym.kind,
                    SymbolKind::METHOD | SymbolKind::FUNCTION | SymbolKind::OPERATOR
                ) {
                    continue;
                }
                if existing_methods.contains(sym.name.as_str()) {
                    continue;
                }
                if matches!(sym.visibility, crate::types::Visibility::Private) {
                    continue;
                }

                let signature = build_override_signature(sym);
                override_methods.push((sym.name.clone(), signature));
            }
        }
    }

    if override_methods.is_empty() {
        return None;
    }

    override_methods.sort_by(|a, b| a.0.cmp(&b.0));
    override_methods.dedup_by_key(|m| m.0.clone());

    // Build the insert text: insert before the closing `}` of the class.
    let class_end_line = class_symbol.range.end.line;
    let class_end_line_s = &lines[class_end_line as usize];
    let end_indent: String = class_end_line_s
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();

    let class_start_indent_line = &lines[class_symbol.range.start.line as usize];
    let class_indent: String = class_start_indent_line
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let body_indent = format!("{class_indent}    ");

    let mut stubs_text = String::new();
    let summary = if override_methods.len() == 1 {
        override_methods[0].0.clone()
    } else {
        format!("{} methods", override_methods.len())
    };

    for (_, signature) in &override_methods {
        stubs_text.push_str(&format!(
            "{body_indent}override {signature} {{\n{body_indent}    TODO()\n{body_indent}}}\n\n"
        ));
    }

    let new_text = format!("{stubs_text}{end_indent}");

    let mut changes = HashMap::new();
    changes.insert(
        uri.clone(),
        vec![TextEdit {
            range: Range {
                start: Position {
                    line: class_end_line,
                    character: 0,
                },
                end: Position {
                    line: class_end_line,
                    character: end_indent.len() as u32,
                },
            },
            new_text,
        }],
    );
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: format!("Generate overrides for {summary}"),
        kind: Some(CodeActionKind::QUICKFIX),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Build a Kotlin override function signature from a SymbolEntry's detail string.
///
/// Converts the detail (e.g. `"fun getItem(index: Int): String"`) into a
/// proper override signature.
pub(crate) fn build_override_signature(sym: &crate::types::SymbolEntry) -> String {
    let detail = &sym.detail;
    if detail.is_empty() {
        return format!("fun {}()", sym.name);
    }

    let s = detail.trim_start();
    let s = strip_visibility_and_modifiers(s);
    let params = extract_override_params(s);
    let ret = extract_override_return(s);

    format!("fun {}{}{}", sym.name, params, ret)
}

pub(crate) fn strip_visibility_and_modifiers(s: &str) -> &str {
    const PREFIXES: &[&str] = &[
        "private ",
        "protected ",
        "internal ",
        "public ",
        "open ",
        "abstract ",
        "override ",
        "final ",
        "inline ",
        "suspend ",
        "operator ",
        "tailrec ",
        "external ",
        "infix ",
    ];
    let mut result = s;
    loop {
        let mut changed = false;
        for pfx in PREFIXES {
            if let Some(r) = result.strip_prefix(pfx) {
                result = r.trim_start();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    result.strip_prefix("fun ").unwrap_or(result)
}

pub(crate) fn extract_override_params(detail: &str) -> String {
    let open = match detail.find('(') {
        Some(o) => o,
        None => return "()".to_owned(),
    };
    let mut depth = 0u32;
    for (i, c) in detail[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return detail[open..open + i + 1].to_owned();
                }
            }
            _ => {}
        }
    }
    "()".to_owned()
}

pub(crate) fn extract_override_return(detail: &str) -> String {
    let mut depth = 0u32;
    let mut close_pos = None;
    for (i, c) in detail.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    close_pos = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let after = match close_pos {
        Some(pos) => &detail[pos + 1..],
        None => return String::new(),
    };
    let after = after.trim();
    if let Some(type_part) = after.strip_prefix(':') {
        let clean: String = type_part
            .trim()
            .chars()
            .take_while(|&c| c != '{' && c != '=' && c != '\n')
            .collect::<String>()
            .trim()
            .to_owned();
        if !clean.is_empty() {
            return format!(": {clean}");
        }
    }
    String::new()
}
