//! Auto-discovery of source roots from `workspace.json`.
//!
//! `workspace.json` is produced by JetBrains Gradle/Maven plugins and describes
//! project structure (modules, content roots, source directories). When the file
//! exists at the workspace root we extract every non-resource source root so the
//! indexer covers them without manual `sourcePaths` configuration.
//!
//! Placeholder substitution:
//! - `<WORKSPACE>` → absolute workspace root path
//! - `<MAVEN_REPO>` → skipped (library jars are not indexed)
//!
//! Source root types we index:
//! - `"java-source"` — production Kotlin/Java sources
//! - `"java-test"` — test Kotlin/Java sources

use serde::Deserialize;
use std::path::{Path, PathBuf};

const SOURCE_TYPES: &[&str] = &["java-source", "java-test"];
const WORKSPACE_PLACEHOLDER: &str = "<WORKSPACE>";

#[derive(Deserialize)]
struct WorkspaceData {
    #[serde(default)]
    modules: Vec<ModuleData>,
    /// Optional list of external library source directories.
    /// When present (even as `[]`), these override the global `~/.kmp-lsp/sources` default.
    /// Supports the `<WORKSPACE>` placeholder (substituted with the workspace root path).
    #[serde(default, rename = "sourcePaths")]
    source_paths: Option<Vec<String>>,
    /// Optional list of compiled `.jar`/`.aar` files — or directories containing them —
    /// to index for library symbols. For projects without a Gradle cache (Make/Bazel/
    /// manual builds), where the automatic Gradle-cache scan finds nothing.
    /// Supports `<WORKSPACE>`; relative paths resolve against the workspace root.
    #[serde(default, rename = "jarPaths")]
    jar_paths: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct ModuleData {
    #[serde(default, rename = "contentRoots")]
    content_roots: Vec<ContentRootData>,
}

#[derive(Deserialize)]
struct ContentRootData {
    #[serde(default, rename = "sourceRoots")]
    source_roots: Vec<SourceRootData>,
}

#[derive(Deserialize)]
struct SourceRootData {
    path: String,
    #[serde(rename = "type", default)]
    root_type: String,
}

/// Reads `<workspace_root>/workspace.json` and returns source root paths.
///
/// Returns an empty `Vec` (with a log warning) if the file is missing, malformed,
/// or contains no eligible source roots — never panics.
pub(crate) fn load_source_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(error) => {
            log::warn!("workspace.json: failed to read: {error}");
            return Vec::new();
        }
    };

    let data: WorkspaceData = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(error) => {
            log::warn!("workspace.json: failed to parse: {error}");
            return Vec::new();
        }
    };

    let workspace_str = workspace_root.to_string_lossy();
    let mut paths: Vec<PathBuf> = Vec::new();

    for module in &data.modules {
        for content_root in &module.content_roots {
            for source_root in &content_root.source_roots {
                if !SOURCE_TYPES.contains(&source_root.root_type.as_str()) {
                    continue;
                }
                let resolved = source_root
                    .path
                    .replace(WORKSPACE_PLACEHOLDER, &workspace_str);
                let path = PathBuf::from(resolved);
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }

    log::info!(
        "workspace.json: auto-discovered {} source roots",
        paths.len()
    );
    paths
}

/// Reads the `sourcePaths` key from `<workspace_root>/workspace.json`.
///
/// Returns `Some(paths)` when the key is present (even if the list is empty —
/// an empty list is an explicit "use no library sources").  Returns `None` when
/// the file is absent or the key is not present, so callers can fall back to
/// the global `~/.kmp-lsp/sources` default.
pub(crate) fn load_configured_source_paths(workspace_root: &Path) -> Option<Vec<PathBuf>> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return None;
    }

    let content = match std::fs::read_to_string(&json_path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!(
                "Failed to read workspace.json at {}: {e}",
                json_path.display()
            );
            return None;
        }
    };
    let data: WorkspaceData = match serde_json::from_str(&content) {
        Ok(d) => d,
        Err(e) => {
            log::warn!(
                "Failed to parse workspace.json at {}: {e}",
                json_path.display()
            );
            return None;
        }
    };

    // `None` means key was absent → caller applies global default.
    // `Some([])` means key present but empty → explicit "no library sources".
    let source_paths = data.source_paths?;

    let workspace_str = workspace_root.to_string_lossy();
    let paths = source_paths
        .iter()
        .map(|p| PathBuf::from(p.replace(WORKSPACE_PLACEHOLDER, &workspace_str)))
        .collect();

    Some(paths)
}

pub(crate) fn load_configured_jar_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let json_path = workspace_root.join("workspace.json");
    if !json_path.exists() {
        return Vec::new();
    }
    let content = match std::fs::read_to_string(&json_path) {
        Ok(text) => text,
        Err(error) => {
            log::warn!("workspace.json: failed to read for jarPaths: {error}");
            return Vec::new();
        }
    };
    let data: WorkspaceData = match serde_json::from_str(&content) {
        Ok(parsed) => parsed,
        Err(error) => {
            log::warn!("workspace.json: failed to parse for jarPaths: {error}");
            return Vec::new();
        }
    };
    match data.jar_paths {
        Some(raw) => resolve_jar_path_specs(&raw, workspace_root),
        None => Vec::new(),
    }
}

/// Resolve a list of jar/aar path specs into concrete compiled-jar files.
///
/// Each spec may be a single `.jar`/`.aar` file or a directory (recursively
/// expanded to its `.jar`/`.aar` files). `<WORKSPACE>` is substituted with the
/// root; relative paths resolve against the root. `*-sources.jar` / `*-javadoc.jar`
/// are excluded — those are not compiled symbol jars (KDoc is read separately by
/// the sidecar from a sibling sources jar).
///
/// Shared by `workspace.json`'s `jarPaths` and the LSP `indexingOptions.jarPaths`.
pub(crate) fn resolve_jar_path_specs(specs: &[String], workspace_root: &Path) -> Vec<PathBuf> {
    let workspace_str = workspace_root.to_string_lossy();
    let mut out: Vec<PathBuf> = Vec::new();
    for spec in specs {
        let resolved = spec.replace(WORKSPACE_PLACEHOLDER, &workspace_str);
        let path = {
            let resolved_path = PathBuf::from(&resolved);
            if resolved_path.is_absolute() {
                resolved_path
            } else {
                workspace_root.join(resolved_path)
            }
        };
        if path.is_dir() {
            collect_compiled_jars(&path, &mut out);
        } else if path.is_file() && is_compiled_jar(&path) && !out.contains(&path) {
            out.push(path);
        } else if !path.exists() {
            log::warn!("jarPaths: configured path not found: {}", path.display());
        }
    }
    out
}

/// Whether `path` is a compiled jar/aar (excludes sources/javadoc jars).
/// Suffix-based so a legitimately-named jar like `my-sources-helper.jar` is kept.
fn is_compiled_jar(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    (name.ends_with(".jar") || name.ends_with(".aar"))
        && !name.ends_with("-sources.jar")
        && !name.ends_with("-javadoc.jar")
}

/// Recursively collect compiled `.jar`/`.aar` files under `dir`. Only descends
/// into *real* subdirectories (via `DirEntry::file_type`, which does not follow
/// symlinks) so a symlink cycle can't cause unbounded recursion.
fn collect_compiled_jars(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        // `DirEntry::file_type` does not traverse symlinks, so a symlinked dir
        // reports as a symlink (not a dir) and is skipped — no cycle recursion.
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_compiled_jars(&path, out);
        } else if file_type.is_file() && is_compiled_jar(&path) && !out.contains(&path) {
            out.push(path);
        }
    }
}
///
/// Activates when a build file (`build.gradle.kts`, `build.gradle`, `pom.xml`, …) exists
/// at the workspace root. Probes well-known source directories; returns only those that
/// actually exist on disk so the indexer never spins on empty paths.
///
/// Multi-module Gradle: `settings.gradle(.kts)` is parsed for `include(":module")` calls;
/// each listed module is treated as a subproject and its standard source dirs are probed.
/// Nested module paths (e.g. `":features:play-domain"`) are supported; colons are replaced
/// with the OS path separator so the result is a valid relative directory path.
///
/// Probed layout: every immediate child of `src/` that contains a `kotlin/` or `java/`
/// subdirectory is treated as a source root. This covers plain Gradle/Maven
/// (`src/main/kotlin`, `src/test/java`), every standard KMP source set (`commonMain`,
/// `androidMain`, `iosMain`, `jvmMain`, `wasmJsMain`, …), and any user-defined source
/// set without requiring an allowlist update.
///
/// These paths are typically already covered by the workspace root scan, but listing them
/// explicitly ensures consistent indexing when the workspace root is set to a parent dir.
pub(crate) fn detect_build_layout_source_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let has_gradle = ["build.gradle.kts", "build.gradle"]
        .iter()
        .any(|f| workspace_root.join(f).exists());
    let has_settings = ["settings.gradle.kts", "settings.gradle"]
        .iter()
        .any(|f| workspace_root.join(f).exists());
    let has_maven = workspace_root.join("pom.xml").exists();

    if !has_gradle && !has_settings && !has_maven {
        return Vec::new();
    }

    let mut roots: Vec<PathBuf> = Vec::new();

    // Subproject dirs from settings.gradle(.kts)
    let subprojects = settings_subprojects(workspace_root);

    // Probe candidates for each directory scope.
    let scan_dirs: Vec<PathBuf> = if subprojects.is_empty() {
        vec![workspace_root.to_owned()]
    } else {
        subprojects.iter().map(|s| workspace_root.join(s)).collect()
    };

    // Always include the root itself (root build.gradle may have sources too).
    let mut all_dirs = vec![workspace_root.to_owned()];
    for d in &scan_dirs {
        if d != workspace_root && !all_dirs.contains(d) {
            all_dirs.push(d.clone());
        }
    }

    for dir in &all_dirs {
        for path in probe_source_set_roots(dir) {
            if !roots.contains(&path) {
                roots.push(path);
            }
        }
    }

    if !roots.is_empty() {
        log::info!("build-layout: auto-discovered {} source roots", roots.len());
    }
    roots
}

/// Returns every `src/<set>/kotlin` and `src/<set>/java` directory under `module_dir`.
///
/// Discovery is structural: any child of `src/` that has a `kotlin/` or `java/` subdir
/// is treated as a source root. Catches plain layouts (`src/main/kotlin`, `src/test/java`),
/// all standard KMP source sets, and user-defined sets without an allowlist.
fn probe_source_set_roots(module_dir: &Path) -> Vec<PathBuf> {
    const SOURCE_LANG_DIRS: &[&str] = &["kotlin", "java"];
    let src = module_dir.join("src");
    let Ok(entries) = std::fs::read_dir(&src) else {
        return Vec::new();
    };

    let mut sets: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    sets.sort();

    let mut roots = Vec::new();
    for set_dir in sets {
        for lang in SOURCE_LANG_DIRS {
            let candidate = set_dir.join(lang);
            if candidate.is_dir() {
                roots.push(candidate);
            }
        }
    }
    roots
}

/// Extracts subproject directory names from `settings.gradle` / `settings.gradle.kts`.
///
/// Handles both forms:
/// - `include(":app", ":core")` — Gradle convention (colon prefix)
/// - `include("app", "core")` — variant without colon
/// - Nested: `include(":feature:login")` → maps to `feature/login`
fn settings_subprojects(workspace_root: &Path) -> Vec<String> {
    for filename in &["settings.gradle.kts", "settings.gradle"] {
        let path = workspace_root.join(filename);
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        return parse_include_calls(&content);
    }
    Vec::new()
}

/// Parses `include("...", "...")` calls and returns directory paths.
///
/// Handles both double- and single-quoted project names, and both Kotlin DSL
/// (`include(":app")`) and Groovy (`include ':app'`) styles. Lines starting
/// with `includeBuild` or `includeFlat` are intentionally ignored.
fn parse_include_calls(content: &str) -> Vec<String> {
    let mut result = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // Only match `include(` — reject `includeBuild(`, `includeFlat(`, etc.
        if !trimmed.starts_with("include(") {
            continue;
        }
        // Extract all single- or double-quoted strings on this line.
        let mut chars = trimmed.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '"' || c == '\'' {
                let quote = c;
                let token: String = chars.by_ref().take_while(|&d| d != quote).collect();
                // ":app" → "app", ":feature:login" → "feature/login"
                let dir = token
                    .trim_start_matches(':')
                    .replace(':', std::path::MAIN_SEPARATOR_STR);
                if !dir.is_empty() && !result.contains(&dir) {
                    result.push(dir);
                }
            }
        }
    }
    result
}

/// Auto-detect Android SDK source directories.
///
/// Checks, in order:
/// 1. `sdk.dir` property in `<workspace_root>/local.properties`
/// 2. `$ANDROID_HOME` environment variable
/// 3. `$ANDROID_SDK_ROOT` environment variable
///
/// When an SDK directory is found, returns the highest API-level
/// `sources/android-XX` subdirectory that exists on disk, which
/// contains the Android platform Java sources.  Returns an empty `Vec`
/// when no SDK is found or the SDK has no `sources/` directory.
pub(crate) fn detect_android_sdk_source_paths(workspace_root: &Path) -> Vec<PathBuf> {
    let Some(sdk) = resolve_android_sdk_root(workspace_root) else {
        return Vec::new();
    };

    let sources_root = sdk.join("sources");
    if !sources_root.is_dir() {
        return Vec::new();
    }

    // Find highest android-XX API level present under sdk/sources/.
    let best = highest_api_level_dir(&sources_root);

    match best {
        Some(path) => {
            log::info!("android-sdk: auto-detected sources at {}", path.display());
            vec![path]
        }
        None => Vec::new(),
    }
}

/// Auto-detect the Android platform's compiled `android.jar`.
///
/// Uses the same SDK-root discovery as `detect_android_sdk_source_paths`
/// (`local.properties`' `sdk.dir`, then `$ANDROID_HOME`, then
/// `$ANDROID_SDK_ROOT`), but returns the highest `platforms/android-XX/
/// android.jar` present — a different subdirectory of the SDK than
/// `sources/android-XX`, since a developer can have a platform installed
/// without its optional sources component, or vice versa. Returns an empty
/// `Vec` when no SDK, no `platforms/` directory, or no `android.jar` inside
/// any `android-XX` platform directory is found.
pub(crate) fn detect_android_sdk_jar_path(workspace_root: &Path) -> Vec<PathBuf> {
    let Some(sdk) = resolve_android_sdk_root(workspace_root) else {
        return Vec::new();
    };

    let platforms_root = sdk.join("platforms");
    if !platforms_root.is_dir() {
        return Vec::new();
    }

    // Find the highest android-XX API level whose android.jar actually
    // exists on disk — a platform directory can exist without the JAR in a
    // partial SDK install, so presence must be checked, not assumed.
    let best = std::fs::read_dir(&platforms_root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let api = parse_android_api_level(&e.file_name().to_string_lossy())?;
            let jar = e.path().join("android.jar");
            jar.is_file().then_some((api, jar))
        })
        .max_by_key(|(api, _)| *api)
        .map(|(_, jar)| jar);

    match best {
        Some(path) => {
            log::info!(
                "android-sdk: auto-detected compiled JAR at {}",
                path.display()
            );
            vec![path]
        }
        None => Vec::new(),
    }
}

/// Resolve the local Android SDK root directory, checking (in order)
/// `local.properties`' `sdk.dir`, then `$ANDROID_HOME`, then
/// `$ANDROID_SDK_ROOT`. Shared by `detect_android_sdk_source_paths` and
/// `detect_android_sdk_jar_path` so the lookup logic exists in one place.
fn resolve_android_sdk_root(workspace_root: &Path) -> Option<PathBuf> {
    sdk_dir_from_local_properties(workspace_root)
        .or_else(|| std::env::var("ANDROID_HOME").ok().map(PathBuf::from))
        .or_else(|| std::env::var("ANDROID_SDK_ROOT").ok().map(PathBuf::from))
        .filter(|p| p.is_dir())
}

/// Read `sdk.dir` from `<workspace_root>/local.properties`.
fn sdk_dir_from_local_properties(workspace_root: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(workspace_root.join("local.properties")).ok()?;
    content.lines().find_map(|line| {
        let (key, val) = line.split_once('=')?;
        if key.trim() == "sdk.dir" {
            Some(PathBuf::from(val.trim()))
        } else {
            None
        }
    })
}

/// Returns the subdirectory of `root` named `android-XX` (or `android-XX.Y`)
/// with the highest API level, per `parse_android_api_level`.
fn highest_api_level_dir(root: &Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().ok().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let api = parse_android_api_level(&e.file_name().to_string_lossy())?;
            Some((api, e.path()))
        })
        .max_by_key(|(api, _)| *api)
        .map(|(_, path)| path)
}

/// Parses an `android-XX` or `android-XX.Y` SDK directory name into a
/// comparable `(major, minor)` API level. Modern Android SDK installs use a
/// decimal extension-level suffix for some platforms (e.g. `android-36.1`,
/// `android-37.0`, confirmed present on a real developer machine alongside
/// plain `android-36`) — this must sort higher than a same-major plain
/// directory, and plain directories are treated as minor level `0`.
fn parse_android_api_level(dir_name: &str) -> Option<(u32, u32)> {
    let level = dir_name.strip_prefix("android-")?;
    match level.split_once('.') {
        Some((major, minor)) => Some((major.parse().ok()?, minor.parse().ok()?)),
        None => Some((level.parse().ok()?, 0)),
    }
}

#[cfg(test)]
#[path = "workspace_json_tests.rs"]
mod tests;
