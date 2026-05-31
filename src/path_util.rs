//! Cross-platform helpers for path/URI handling.
//!
//! Several places in the codebase need to match path strings (glob patterns,
//! substring filters, qualified-key generation) in a way that doesn't depend
//! on the host OS's separator. These helpers centralise that.

use std::path::{Path, PathBuf};

use tower_lsp::lsp_types::Url;

/// Convert a `Path` to a `String` using `/` as the separator regardless of OS.
///
/// On Unix this is essentially `to_string_lossy().into_owned()`; on Windows
/// it walks the path components and joins with `/`. Used wherever a path is
/// fed into `globset` (which expects forward slashes) or compared against
/// embedded forward-slash literals.
pub(crate) fn to_forward_slash(path: &Path) -> String {
    if !cfg!(windows) {
        return path.to_string_lossy().into_owned();
    }
    // `Component::as_os_str()` returns `\` for RootDir on Windows, so we
    // can't just join component strings. Walk the variants explicitly.
    use std::path::Component;
    let mut out = String::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => {
                // e.g. `C:` or `\\?\C:` — keep verbatim, no separator inserted.
                out.push_str(&p.as_os_str().to_string_lossy());
            }
            Component::RootDir => {
                // Absolute root — always emit a single forward slash.
                if !out.ends_with('/') {
                    out.push('/');
                }
            }
            Component::CurDir => {
                if out.is_empty() {
                    out.push('.');
                }
            }
            Component::ParentDir => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str("..");
            }
            Component::Normal(n) => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&n.to_string_lossy());
            }
        }
    }
    out
}

/// Strip the Windows long-path prefix (`\\?\`) from a `PathBuf` if present.
///
/// `Path::canonicalize` on Windows returns a path with the `\\?\` verbatim
/// prefix, which `Url::from_file_path` happens to round-trip badly: the
/// produced URL can't always be round-tripped back through `to_file_path`,
/// and string comparisons against non-canonicalized paths fail. Strip the
/// prefix when we know the rest is a valid drive-letter path.
///
/// No-op on non-Windows.
pub(crate) fn strip_unc_prefix(path: PathBuf) -> PathBuf {
    if cfg!(windows) {
        let s = path.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            // Only strip when the remainder looks like a drive-letter path
            // (`C:\…`). UNC server paths (`\\server\share\…`) get prefixed as
            // `\\?\UNC\server\share\…` and need different handling, so we
            // leave those alone.
            if rest.len() >= 2
                && rest.as_bytes()[1] == b':'
                && rest.as_bytes()[0].is_ascii_alphabetic()
            {
                return PathBuf::from(rest);
            }
        }
    }
    path
}

/// Extract the file stem (basename without extension) from a `file://` URL.
///
/// Prefers `Url::to_file_path()` (which handles percent-decoding correctly)
/// and falls back to URL-path parsing when that fails. The fallback matters
/// on Windows: `Url::to_file_path` requires a drive letter, so URIs like
/// `file:///pkg/Foo.kt` (no drive) return `Err`. The fallback still extracts
/// the correct stem in that case.
pub(crate) fn file_stem_from_uri(uri: &Url) -> Option<String> {
    if let Ok(p) = uri.to_file_path() {
        if let Some(stem) = p.file_stem() {
            return Some(stem.to_string_lossy().into_owned());
        }
    }
    let path = uri.path();
    let last = path.rsplit('/').next()?;
    if last.is_empty() {
        return None;
    }
    let stem = match last.rfind('.') {
        // `.gitignore`-style names start with a dot; treat as stem-only.
        Some(0) => last,
        Some(dot) => &last[..dot],
        None => last,
    };
    if stem.is_empty() {
        None
    } else {
        Some(stem.to_owned())
    }
}

#[cfg(test)]
#[path = "path_util_tests.rs"]
mod tests;
