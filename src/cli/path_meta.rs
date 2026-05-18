//! Derive `module`, `sourceSet`, and `relativePath` from a source file path.
//!
//! These three pieces of metadata are surfaced in agent-facing CLI output so
//! tools can filter or group results without parsing absolute paths themselves.
//!
//! - `relative_path`  — file path stripped of the workspace-root prefix.
//! - `source_set`     — the KMP source set the file belongs to: the path
//!   component between `src/` and the language directory (`commonMain`,
//!   `androidMain`, `iosMain`, …). `main` and `test` are stock Gradle layouts.
//! - `module`         — the Gradle module the file belongs to: every path
//!   component above `src/`, joined with `/`. For a root-level project we
//!   return `None` because there is no enclosing module.
//!
//! All inputs are expected to be absolute and normalized. The functions never
//! panic — they return `None` whenever the input shape doesn't match.
//!
//! Examples (workspace root `/repo`):
//!
//! | absolute path                                        | module          | source_set     |
//! |------------------------------------------------------|-----------------|----------------|
//! | `/repo/features/play-domain/src/commonMain/kotlin/X.kt` | `features/play-domain` | `commonMain` |
//! | `/repo/shared/src/iosArm64Main/kotlin/Y.kt`           | `shared`        | `iosArm64Main` |
//! | `/repo/app/src/main/java/Z.java`                      | `app`           | `main`         |
//! | `/repo/src/main/kotlin/W.kt`                          | `None`          | `main`         |

use std::path::{Component, Path, PathBuf};

/// Strip `root` from `path` and return the remainder as a forward-slash
/// joined string. Falls back to the original path string when `path` doesn't
/// live under `root`.
pub(crate) fn relative_path(path: &Path, root: &Path) -> String {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let rel = canonical_path
        .strip_prefix(&canonical_root)
        .or_else(|_| path.strip_prefix(root))
        .map(PathBuf::from)
        .unwrap_or_else(|_| path.to_path_buf());
    forward_slash(&rel)
}

/// Return the source-set component of `path`, i.e. the directory immediately
/// under the nearest `src/` ancestor. Looks at the deepest `src/` so nested
/// `gradle/foo/src/main/...` still resolves correctly.
pub(crate) fn source_set(path: &Path) -> Option<String> {
    let components: Vec<&std::ffi::OsStr> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    // Find the deepest "src" segment; the next segment is the source set.
    let src_idx = components.iter().rposition(|c| c.to_str() == Some("src"))?;
    let set = components.get(src_idx + 1)?;
    set.to_str().map(|s| s.to_owned())
}

/// Return the module path (every segment between `root` and the deepest `src/`
/// directory in `path`), joined with `/`. Returns `None` when there is no
/// enclosing module, or when `path` does not live under `root`.
pub(crate) fn module(path: &Path, root: &Path) -> Option<String> {
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let rel = canonical_path
        .strip_prefix(&canonical_root)
        .or_else(|_| path.strip_prefix(root))
        .ok()?;
    let segments: Vec<&std::ffi::OsStr> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();
    let src_idx = segments.iter().rposition(|c| c.to_str() == Some("src"))?;
    if src_idx == 0 {
        return None;
    }
    let module_segments: Vec<String> = segments[..src_idx]
        .iter()
        .filter_map(|s| s.to_str().map(str::to_owned))
        .collect();
    if module_segments.is_empty() {
        None
    } else {
        Some(module_segments.join("/"))
    }
}

fn forward_slash(path: &Path) -> String {
    path.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str().map(str::to_owned),
            Component::RootDir => Some(String::new()),
            Component::Prefix(p) => Some(p.as_os_str().to_string_lossy().into_owned()),
            Component::CurDir | Component::ParentDir => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
#[path = "path_meta_tests.rs"]
mod tests;
