//! Language provider traits and implementations.
//!
//! Each supported language implements [`LanguageParser`], which is the single
//! dispatch point for language-specific parsing and keyword behaviour.
//! Use [`crate::Language::parser()`] to get the provider for a given file.

pub(crate) mod java;
pub(crate) mod kotlin;
pub(crate) mod swift;

use tower_lsp::lsp_types::SymbolKind;

/// Language-specific capabilities exposed to the rest of the codebase.
///
/// Implementations are stateless singletons; tree-sitter parser state and query
/// caches remain in module-level statics inside `parser.rs`.
pub(crate) trait LanguageParser: Send + Sync {
    /// LSP language identifier string (e.g. `"kotlin"`, `"java"`, `"swift"`).
    fn language_id(&self) -> &'static str;

    /// File extensions handled by this provider (without leading dot).
    fn file_extensions(&self) -> &[&'static str];

    /// Return the declaration keyword for `kind` in this language.
    ///
    /// Examples: Kotlin/Java → `"fun"`, Swift → `"func"`;
    ///           Kotlin/Java → `"val"`, Swift → `"let"`.
    fn symbol_keyword(&self, kind: SymbolKind) -> &'static str;
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

/// Keyword that introduces a declaration of `kind` in Kotlin-flavoured syntax.
pub(crate) fn symbol_kw(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::CLASS => "class",
        SymbolKind::INTERFACE => "interface",
        SymbolKind::FUNCTION => "fun",
        SymbolKind::METHOD => "fun",
        SymbolKind::VARIABLE => "var",
        SymbolKind::CONSTANT => "val",
        SymbolKind::OBJECT => "object",
        SymbolKind::TYPE_PARAMETER => "typealias",
        SymbolKind::ENUM => "enum class",
        SymbolKind::FIELD => "field",
        _ => "symbol",
    }
}
