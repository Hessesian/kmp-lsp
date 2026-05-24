//! Hover Markdown formatting helpers.
//!
//! These functions turn a [`ResolvedSymbol`] into the final Markdown string
//! returned by the `textDocument/hover` handler.  They are presentation-only
//! and contain no resolution logic.

use crate::indexer::lookup::{lang_str, symbol_kw_for_lang};
use crate::indexer::resolution::ResolvedSymbol;
use tower_lsp::lsp_types::SymbolKind;

/// Format a standard symbol hover: optional KDoc block + fenced code block.
///
/// ```text
/// [`deprecated`]
///
/// /** KDoc comment */
///
/// ---
///
/// ````kotlin
/// [visibility] fun foo(x: Int): String
/// ````
///
/// [@Deprecated warning]
///
/// [Data class properties: a, b, c]
/// ```
pub(super) fn format_symbol_hover(info: &ResolvedSymbol, uri_path: &str) -> String {
    let lang = lang_str(uri_path);
    let sig = info.signature.as_str();

    let visibility_prefix = visibility_str(info.visibility);
    let code_block = if sig.is_empty() {
        // Signature unavailable — fall back to keyword + known symbol name.
        format!(
            "````{lang}\n{visibility_prefix}{} {}\n````",
            symbol_kw_for_lang(info.kind, lang),
            info.name
        )
    } else {
        format!("````{lang}\n{visibility_prefix}{sig}\n````")
    };

    // Build additional info sections.
    let mut sections = Vec::with_capacity(4);

    // KDoc
    if !info.doc.is_empty() {
        sections.push(info.doc.clone());
    }

    // Deprecation warning
    if info.deprecated {
        sections.push("⚠️ **Deprecated**".to_owned());
    }

    // Data class properties
    if !info.data_class_props.is_empty() {
        sections.push(format!("Properties: {}", info.data_class_props.join(", ")));
    }

    let main = if sections.is_empty() {
        code_block
    } else if sections.len() == 1 {
        format!("{}\n\n---\n\n{code_block}", sections[0])
    } else {
        format!(
            "{}\n\n---\n\n{}\n\n---\n\n{code_block}",
            sections[0],
            &sections[1..].join("\n\n"),
        )
    };

    main
}

/// Convert Visibility enum to a human-readable prefix.
fn visibility_str(vis: crate::types::Visibility) -> &'static str {
    match vis {
        crate::types::Visibility::Private => "private ",
        crate::types::Visibility::Protected => "protected ",
        crate::types::Visibility::Internal => "internal ",
        crate::types::Visibility::Public => "",
    }
}

/// Format a contextual hover for an `it` / named lambda parameter:
///
/// ```text
/// ````kotlin
/// val it: AccountType
/// ````
///
/// ---
///
/// <optional type-symbol hover>
/// ```
///
/// `type_sig_md` — the synthesized declaration line, e.g. `"val it: AccountType"`.
/// `type_detail` — optional hover markdown for the resolved type symbol itself.
pub(super) fn format_contextual_hover(
    type_sig_md: &str,
    uri_path: &str,
    type_detail: Option<&str>,
) -> String {
    let lang = lang_str(uri_path);
    let sig_block = format!("````{lang}\n{type_sig_md}\n````");
    match type_detail {
        Some(td) if !td.is_empty() => format!("{sig_block}\n\n---\n\n{td}"),
        _ => sig_block,
    }
}

/// Return the language keyword for a symbol kind (Swift-aware).
#[allow(dead_code)]
pub(super) fn kw_for_kind(kind: SymbolKind, uri_path: &str) -> &'static str {
    symbol_kw_for_lang(kind, lang_str(uri_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::resolution::ResolvedSymbol;
    use std::collections::HashMap;
    use tower_lsp::lsp_types::{Location, Position, Range, SymbolKind, Url};

    fn make_sym(name: &str, signature: &str, doc: &str) -> ResolvedSymbol {
        ResolvedSymbol {
            location: Location {
                uri: Url::parse("file:///test.kt").unwrap(),
                range: Range {
                    start: Position {
                        line: 1,
                        character: 1,
                    },
                    end: Position {
                        line: 1,
                        character: 10,
                    },
                },
            },
            name: name.into(),
            kind: SymbolKind::FUNCTION,
            visibility: crate::types::Visibility::Public,
            deprecated: false,
            raw_signature: signature.into(),
            signature: signature.into(),
            subst: HashMap::new(),
            doc: doc.into(),
            data_class_props: Vec::new(),
        }
    }

    #[test]
    fn hover_uses_code_fence() {
        let sym = make_sym("greet", "fun greet(name: String): String", "");
        let md = format_symbol_hover(&sym, "test.kt");
        assert!(md.contains("```kotlin"), "Expected code fence, got: {md}");
    }

    #[test]
    fn hover_includes_signature() {
        let sym = make_sym("greet", "fun greet(name: String): String", "");
        let md = format_symbol_hover(&sym, "test.kt");
        assert!(md.contains("fun greet(name: String): String"));
    }

    #[test]
    fn hover_includes_doc_when_present() {
        let sym = make_sym("greet", "fun greet(): String", "Says hello.");
        let md = format_symbol_hover(&sym, "test.kt");
        assert!(md.contains("Says hello."));
        assert!(md.contains("---"));
    }

    #[test]
    fn contextual_hover_includes_type_sig() {
        let md = format_contextual_hover("val it: String", "test.kt", None);
        assert!(md.contains("val it: String"));
    }
}
