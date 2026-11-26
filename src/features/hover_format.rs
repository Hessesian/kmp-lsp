//! Hover Markdown formatting helpers.
//!
//! These functions turn a [`ResolvedSymbol`] into the final Markdown string
//! returned by the `textDocument/hover` handler.  They are presentation-only
//! and contain no resolution logic.

use crate::indexer::lookup::{lang_str, symbol_kw_for_lang};
use crate::indexer::resolution::ResolvedSymbol;

/// Format a standard symbol hover: optional KDoc block + fenced code block.
///
/// ```text
/// /** KDoc comment */
///
/// ---
///
/// ```kotlin
/// fun foo(x: Int): String
/// ```
/// ```
pub(crate) fn format_symbol_hover(info: &ResolvedSymbol, uri_path: &str) -> String {
    let lang = lang_str(uri_path);
    let sig = info.signature.as_str();

    let code_block = if sig.is_empty() {
        // Signature unavailable — fall back to keyword + known symbol name.
        format!(
            "```{lang}\n{} {}\n```",
            symbol_kw_for_lang(info.kind, lang),
            info.name
        )
    } else {
        format!("```{lang}\n{sig}\n```")
    };

    // Body assembled from the widened record: an optional deprecation marker
    // above the signature and an optional `package.Container` context footer for
    // members — both read straight off `ResolvedSymbol`, no second index lookup.
    let mut body = String::new();
    if info.deprecated {
        body.push_str("**⚠ Deprecated**\n\n");
    }
    body.push_str(&code_block);
    if let Some(ctx) = qualified_context(info) {
        body.push_str(&format!("\n\n*in `{ctx}`*"));
    }

    if info.doc.is_empty() {
        body
    } else {
        format!("{}\n\n---\n\n{body}", info.doc)
    }
}

/// The `package.Container` home of a *member* symbol, for the hover footer.
///
/// Returns `None` for top-level declarations (no enclosing container) so their
/// hover stays unchanged; `package` is folded in when the file declares one.
fn qualified_context(info: &ResolvedSymbol) -> Option<String> {
    let container = info.container.as_deref()?;
    match info.package.as_deref() {
        Some(pkg) => Some(format!("{pkg}.{container}")),
        None => Some(container.to_string()),
    }
}

/// Format a contextual hover for an `it` / named lambda parameter:
///
/// ```text
/// ```kotlin
/// val it: AccountType
/// ```
///
/// ---
///
/// <optional type-symbol hover>
/// ```
///
/// `type_sig_md` — the synthesized declaration line, e.g. `"val it: AccountType"`.
/// `type_detail` — optional hover markdown for the resolved type symbol itself.
pub(crate) fn format_contextual_hover(
    type_sig_md: &str,
    uri_path: &str,
    type_detail: Option<&str>,
) -> String {
    let lang = lang_str(uri_path);
    let sig_block = format!("```{lang}\n{type_sig_md}\n```");
    match type_detail {
        Some(td) if !td.is_empty() => format!("{sig_block}\n\n---\n\n{td}"),
        _ => sig_block,
    }
}
