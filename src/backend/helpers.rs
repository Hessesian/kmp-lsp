use crate::types::{FileData, SyntaxError};
use crate::StrExt;
use tower_lsp::lsp_types::*;

/// Determine the `(parent_class, declared_pkg)` scope for a `findReferences` request.
///
/// For uppercase symbols the scope is narrowed via import analysis or declaration
/// site lookup so that `rg_find_references` Pass A/B can restrict results to the
/// specific class variant (e.g. the right `Event` among many sealed interfaces).
///
/// For lowercase symbols (fields, methods) `(None, None)` is returned — an
/// unscoped bare-word search is used.  Injecting a parent class derived from
/// `this`/`it` type inference would narrow rg to `ClassName.fieldName` qualified
/// patterns which almost never appear in real Kotlin code, leaving only in-memory
/// hits in the current file.
pub(super) fn resolve_references_scope(
    idx: &crate::indexer::Indexer,
    uri: &Url,
    line: u32,
    name: &str,
) -> (Option<String>, Option<String>) {
    if !name.starts_with_uppercase() {
        return (None, None);
    }
    let on_decl = idx.is_declared_in(uri, name)
        && idx
            .definitions
            .get(name)
            .map(|locs| {
                locs.iter()
                    .any(|l| l.uri == *uri && l.range.start.line == line)
            })
            .unwrap_or(false);
    if on_decl {
        let parent = idx.enclosing_class_at(uri, line);
        let pkg = idx.package_of(uri);
        return (parent, pkg);
    }
    let (parent, pkg) = idx.resolve_symbol_via_import(uri, name);
    if parent.is_some() || pkg.is_some() {
        return (parent, pkg);
    }
    let parent = idx.declared_parent_class_of(name, uri);
    let pkg = idx.declared_package_of(name);
    (parent, pkg)
}

pub(super) fn syntax_diagnostics(errors: &[SyntaxError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| Diagnostic {
            range: e.range,
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("kotlin-lsp".into()),
            message: e.message.clone(),
            ..Default::default()
        })
        .collect()
}

/// Build diagnostics for deprecated symbols found in the file's parsed data.
///
/// Emits a warning-level diagnostic at the declaration site of any symbol
/// marked as `@Deprecated`.
pub(super) fn deprecation_diagnostics(file_data: &FileData) -> Vec<Diagnostic> {
    file_data
        .symbols
        .iter()
        .filter(|sym| sym.deprecated)
        .map(|sym| Diagnostic {
            range: sym.selection_range,
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("kotlin-lsp".into()),
            message: format!("'{}' is deprecated", sym.name),
            ..Default::default()
        })
        .collect()
}

/// Detect unused imports: imports whose local name never appears in the file body.
pub(super) fn import_diagnostics(lines: &[String], is_kotlin_or_java: bool) -> Vec<Diagnostic> {
    if !is_kotlin_or_java {
        return vec![];
    }
    let imports = crate::parser::parse_imports_from_lines(lines);
    let mut diags = Vec::new();
    let mut used = std::collections::HashSet::new();
    for line in lines.iter() {
        let t = line.trim_start();
        if t.starts_with("import ") || t.starts_with("package ") {
            continue;
        }
        for word in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
            if !word.is_empty() {
                used.insert(word.to_string());
            }
        }
    }
    let mut seen_once: std::collections::HashSet<String> = std::collections::HashSet::new();
    for imp in &imports {
        if imp.is_star {
            continue;
        }
        if !used.contains(&imp.local_name) {
            let line_idx = lines
                .iter()
                .position(|l| l.contains(&imp.full_path))
                .unwrap_or(0) as u32;
            diags.push(Diagnostic {
                range: Range {
                    start: Position {
                        line: line_idx,
                        character: 0,
                    },
                    end: Position {
                        line: line_idx,
                        character: 0,
                    },
                },
                severity: Some(DiagnosticSeverity::WARNING),
                source: Some("kotlin-lsp".into()),
                message: format!("unused import: {}", imp.full_path),
                ..Default::default()
            });

            // Detect duplicate imports.
            if seen_once.contains(&imp.full_path) {
                let dup_line = lines
                    .iter()
                    .position(|l| l.contains(&imp.full_path))
                    .unwrap_or(0) as u32;
                diags.push(Diagnostic {
                    range: Range {
                        start: Position {
                            line: dup_line,
                            character: 0,
                        },
                        end: Position {
                            line: dup_line,
                            character: 0,
                        },
                    },
                    severity: Some(DiagnosticSeverity::WARNING),
                    source: Some("kotlin-lsp".into()),
                    message: format!("duplicate import: {}", imp.full_path),
                    ..Default::default()
                });
            }
            seen_once.insert(imp.full_path.clone());
        }
    }
    diags
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod tests;
