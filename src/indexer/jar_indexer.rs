//! JAR source-file indexer — extracts symbols from `*-sources.jar` files.
//!
//! Scans common locations (Maven local repo, Gradle cache) for `*-sources.jar`,
//! reads `.kt` / `.java` files from them, and indexes the symbols into
//! `Indexer.jar_files` and `Indexer.jar_definitions` for go-to-definition,
//! hover, and completion of library symbols.
//!
//! The key insight: a JAR is just a ZIP file. We use the `zip` crate to read
//! entries without shelling out to `jar`.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use tower_lsp::lsp_types::{Location, Position, Range, Url};

use crate::types::FileData;

/// A single symbol extracted from a JAR source file.
#[derive(Debug, Clone)]
pub(crate) struct JarSymbol {
    pub name: String,
    pub kind: tower_lsp::lsp_types::SymbolKind,
    pub file_path: String,
    pub line: u32,
    pub detail: String,
}

/// Find all `*-sources.jar` files under `root_dir`.
/// Scans Maven local repo and Gradle cache.
pub(crate) fn find_sources_jars(root_dir: &Path) -> Vec<PathBuf> {
    let mut jars = Vec::new();

    // Maven local repo: ~/.m2/repository/**/*-sources.jar
    let m2 = home_dir()
        .map(|h| h.join(".m2").join("repository"))
        .unwrap_or_default();
    if m2.is_dir() {
        for entry in walkdir::WalkDir::new(&m2)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().map(|e| e == "jar").unwrap_or(false)
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.ends_with("-sources"))
                    .unwrap_or(false)
            {
                jars.push(path.to_path_buf());
            }
        }
    }

    // Gradle cache: ~/.gradle/caches/**/*-sources.jar
    let gradle_home = home_dir().map(|h| h.join(".gradle")).unwrap_or_default();
    if gradle_home.is_dir() {
        for entry in walkdir::WalkDir::new(&gradle_home)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().map(|e| e == "jar").unwrap_or(false)
                && path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.ends_with("-sources"))
                    .unwrap_or(false)
            {
                jars.push(path.to_path_buf());
            }
        }
    }

    jars.sort();
    jars.dedup();
    jars
}

/// Parse a single `*-sources.jar` and return extracted symbols.
pub(crate) fn index_sources_jar(jar_path: &Path) -> Result<Vec<JarSymbol>, JarError> {
    let file = std::fs::File::open(jar_path).map_err(JarError::Io)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| JarError::Zip(e.to_string()))?;
    let mut symbols = Vec::new();

    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(_) => continue,
        };

        let name = entry.name().to_string();
        // Only process .kt and .java files
        if !name.ends_with(".kt") && !name.ends_with(".java") {
            continue;
        }

        let mut content = String::new();
        if entry.read_to_string(&mut content).is_err() {
            continue;
        }

        // Quick scan for top-level type declarations
        extract_top_level_symbols(&name, &content, &mut symbols);
    }

    Ok(symbols)
}

/// Quick regex-free scan for top-level class/fun/val declarations.
fn extract_top_level_symbols(file_path: &str, content: &str, symbols: &mut Vec<JarSymbol>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim();

        // Skip package, import, annotations, blank, comments
        if line.is_empty()
            || line.starts_with("package ")
            || line.starts_with("import ")
            || line.starts_with('@')
            || line.starts_with("//")
            || line.starts_with("/*")
        {
            i += 1;
            continue;
        }

        // `public class Foo` / `class Foo` / `public interface Foo`
        if let Some(name) = extract_decl(
            line,
            &[
                "class ",
                "interface ",
                "enum class ",
                "object ",
                "annotation class ",
            ],
        ) {
            symbols.push(JarSymbol {
                name: name.to_owned(),
                kind: kind_for_decl(line),
                file_path: file_path.to_owned(),
                line: i as u32,
                detail: line.to_owned(),
            });
        }

        // `fun foo(` / `fun <T> foo(`
        if let Some(name) = extract_fun_name(line) {
            symbols.push(JarSymbol {
                name: name.to_owned(),
                kind: tower_lsp::lsp_types::SymbolKind::FUNCTION,
                file_path: file_path.to_owned(),
                line: i as u32,
                detail: line.to_owned(),
            });
        }

        i += 1;
    }
}

/// Extract type/function name from a declaration line.
fn extract_decl<'a>(line: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for prefix in prefixes {
        if let Some(idx) = line.find(prefix) {
            let after = &line[idx + prefix.len()..];
            // Next word is the name
            let name = after.split(['(', '<', ' ']).next()?;
            if !name.is_empty() && !name.starts_with('(') {
                return Some(name);
            }
        }
    }
    None
}

/// Extract function name from a line like `fun foo(...)` or `fun <T> foo(...)`.
fn extract_fun_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if !trimmed.starts_with("fun ") && !trimmed.starts_with("fun<") && !trimmed.contains(" fun ") {
        return None;
    }
    let start = trimmed.find("fun ")? + 4;
    let after = trimmed[start..].trim_start();
    // Skip type params like <T>
    let after_type_params = if after.starts_with('<') {
        let close = after.find('>')?;
        after[close + 1..].trim_start()
    } else {
        after
    };
    let name = after_type_params.split(['(', ' ', '<']).next()?;
    if name.is_empty() || name.starts_with('(') {
        None
    } else {
        Some(name)
    }
}

/// Map a declaration line to a symbol kind.
fn kind_for_decl(line: &str) -> tower_lsp::lsp_types::SymbolKind {
    if line.contains("interface") {
        tower_lsp::lsp_types::SymbolKind::INTERFACE
    } else if line.contains("enum class") {
        tower_lsp::lsp_types::SymbolKind::ENUM
    } else if line.contains("annotation class") {
        tower_lsp::lsp_types::SymbolKind::INTERFACE
    } else if line.contains("object") {
        tower_lsp::lsp_types::SymbolKind::OBJECT
    } else {
        tower_lsp::lsp_types::SymbolKind::CLASS
    }
}

/// Convert JarSymbols to FileData and definitions suitable for merging into Indexer.
pub(crate) fn symbols_to_filedata(
    jar_path: &Path,
    symbols: &[JarSymbol],
) -> (FileData, Vec<(String, Location)>) {
    let mut file_symbols = Vec::new();
    let mut definitions = Vec::new();

    let uri_str = format!("jar://{}", jar_path.display());
    let jar_uri = Url::parse(&uri_str).unwrap_or_else(|_| {
        // Fallback for paths with special chars
        let encoded = uri_str.replace(' ', "%20");
        Url::parse(&encoded).unwrap()
    });

    for sym in symbols {
        let range = Range {
            start: Position::new(sym.line, 0),
            end: Position::new(sym.line, sym.detail.len() as u32),
        };
        let entry = crate::types::SymbolEntry {
            name: sym.name.clone(),
            kind: sym.kind,
            visibility: crate::types::Visibility::Public,
            range,
            selection_range: range,
            detail: sym.detail.clone(),
            type_params: Vec::new(),
            extension_receiver: String::new(),
            deprecated: false,
        };
        file_symbols.push(entry);
        definitions.push((
            sym.name.clone(),
            Location {
                uri: jar_uri.clone(),
                range,
            },
        ));
    }

    let fd = FileData {
        symbols: file_symbols,
        imports: Vec::new(),
        package: None,
        lines: std::sync::Arc::new(Vec::new()),
        declared_names: Vec::new(),
        supers: Vec::new(),
        rhs_types: Vec::new(),
        method_call_rhs: Vec::new(),
        syntax_errors: Vec::new(),
    };

    (fd, definitions)
}

/// Get the home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).or_else(|| {
        if cfg!(target_os = "windows") {
            std::env::var_os("USERPROFILE").map(PathBuf::from)
        } else {
            None
        }
    })
}

/// Error type for JAR indexing operations.
#[derive(Debug)]
pub(crate) enum JarError {
    Io(io::Error),
    Zip(String),
}

impl std::fmt::Display for JarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JarError::Io(e) => write!(f, "IO error: {e}"),
            JarError::Zip(e) => write!(f, "ZIP error: {e}"),
        }
    }
}

impl std::error::Error for JarError {}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "jar_indexer_tests.rs"]
mod tests;
