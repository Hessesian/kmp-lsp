use std::path::Path;

use crate::types::SyntaxError;
use tower_lsp::lsp_types::*;

/// Returns true when `path` has a `.xml` extension (case-insensitive).
pub(crate) fn is_xml_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("xml"))
}

/// Returns true when `uri` refers to an XML file.
pub(crate) fn is_xml_uri(uri: &Url) -> bool {
    let path_ends_with_xml = uri.path().ends_with(".xml") || uri.path().ends_with(".XML");
    path_ends_with_xml
        || uri
            .to_file_path()
            .ok()
            .is_some_and(|path| is_xml_path(&path))
}

pub(crate) fn syntax_diagnostics(errors: &[SyntaxError]) -> Vec<Diagnostic> {
    errors
        .iter()
        .map(|e| Diagnostic {
            range: e.range,
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("kmp-lsp".into()),
            message: e.message.clone(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
#[path = "helpers_tests.rs"]
mod helpers_tests;

#[cfg(test)]
#[path = "xml_path_tests.rs"]
mod xml_path_tests;
