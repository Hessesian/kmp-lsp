//! Kotlin language provider.

use tower_lsp::lsp_types::SymbolKind;

use super::symbol_kw;
use super::LanguageParser;

pub(crate) struct KotlinParser;

impl LanguageParser for KotlinParser {
    fn language_id(&self) -> &'static str {
        "kotlin"
    }

    fn file_extensions(&self) -> &[&'static str] {
        &["kt", "kts"]
    }

    fn symbol_keyword(&self, kind: SymbolKind) -> &'static str {
        symbol_kw(kind)
    }
}

#[cfg(test)]
#[path = "kotlin_tests.rs"]
mod tests;
